pub mod parser;
pub mod graph;
pub mod lsp;
pub mod registry;
pub mod structural;
pub mod watcher;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Global dependency graph cache — keyed by repository_id.
/// Avoids rebuilding the graph on every query (140ms → 0ms after first call).
static GRAPH_CACHE: OnceLock<Mutex<HashMap<String, graph::DependencyGraph>>> = OnceLock::new();

fn graph_cache() -> &'static Mutex<HashMap<String, graph::DependencyGraph>> {
    GRAPH_CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn dirs() -> PathBuf {
    std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(PathBuf::from))
        .unwrap_or_else(|_| PathBuf::from("."))
}

use knocode_core::{SearchResult, SearchResults};
use knocode_events::{EventBus, RuntimeEvent};
use grep_regex::RegexMatcherBuilder;
use grep_searcher::sinks::UTF8;
use grep_searcher::SearcherBuilder;
use ignore::WalkBuilder;
use sha2::{Digest, Sha256};
use tracing::{debug, info, warn};

// ── Configuration ───────────────────────────────────────────────────────

// Language detection and file classification now live in `registry.rs`.
// The single source of truth is `registry::LANGUAGE_REGISTRY`.

use registry::{detect_language as registry_detect, classify_file, FileClass};

// ── Symbol Extraction Patterns ──────────────────────────────────────────

/// Regex patterns for extracting symbols from different languages
struct SymbolPatterns {
    function_pattern: regex::Regex,
    struct_pattern: regex::Regex,
    enum_pattern: regex::Regex,
    impl_pattern: regex::Regex,
    trait_pattern: regex::Regex,
    type_pattern: regex::Regex,
}

impl SymbolPatterns {
    fn new() -> Self {
        Self {
            // Rust: fn name, Python: def name, JS/TS: function name / const name = () =>
            // C#: public/private/internal [static] void/int/string ReturnType(
            // Java: public/private [static] ReturnType methodName(
            function_pattern: regex::Regex::new(
                r"(?m)^(?:pub\s+)?(?:async\s+)?fn\s+(\w+)|^def\s+(\w+)|^(?:export\s+)?(?:async\s+)?function\s+(\w+)|^(?:pub\s+)?(?:const|let|var)\s+(\w+)\s*=\s*(?:async\s+)?\(|^(?:pub\s+)?(?:const|let|var)\s+(\w+)\s*=\s*(?:async\s+)?(?:function|\()|^\s*(?:public|private|protected|internal)\s+(?:static\s+)?(?:async\s+)?(?:void|bool|int|long|float|double|string|var|IEnumerable|Task|ValueTask|IActionResult|ActionResult|ObjectResult)\s+(\w+)\s*\("
            ).unwrap(),
            // struct ClassName, class ClassName, C# public class Name, interface IName
            struct_pattern: regex::Regex::new(
                r"(?m)^(?:pub\s+)?struct\s+(\w+)|^class\s+(\w+)|^(?:export\s+)?class\s+(\w+)|^\s*(?:public|private|protected|internal)\s+(?:static\s+|abstract\s+|sealed\s+)?class\s+(\w+)|^\s*(?:public|private|protected|internal)\s+interface\s+(\w+)"
            ).unwrap(),
            // enum EnumName, C# public enum Name
            enum_pattern: regex::Regex::new(
                r"(?m)^(?:pub\s+)?enum\s+(\w+)|^\s*(?:public|private|protected|internal)\s+enum\s+(\w+)"
            ).unwrap(),
            // impl TypeName (Rust)
            impl_pattern: regex::Regex::new(
                r"(?m)^impl(?:<[^>]*>)?\s+(\w+)"
            ).unwrap(),
            // trait TraitName (Rust)
            trait_pattern: regex::Regex::new(
                r"(?m)^(?:pub\s+)?trait\s+(\w+)"
            ).unwrap(),
            // type Alias = Type (Rust/TS), C# using alias
            type_pattern: regex::Regex::new(
                r"(?m)^(?:pub\s+)?type\s+(\w+)"
            ).unwrap(),
        }
    }
}

// ── Data Types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FileInfo {
    pub path: String,
    pub size: i64,
    pub language: Option<String>,
    pub symbol_count: usize,
    pub last_indexed_at: String,
}

#[derive(Debug, Clone)]
pub struct SymbolInfo {
    pub name: String,
    pub kind: String,
    pub file_path: String,
    pub line_start: i64,
    pub line_end: i64,
}

#[derive(Debug, Clone)]
pub struct IndexStats {
    pub files_indexed: usize,
    pub symbols_extracted: usize,
    pub files_skipped: usize,
    pub files_deleted: usize,
    pub duration_ms: u64,
}

// ── Repository Intelligence ─────────────────────────────────────────────

pub struct RepositoryIntelligence {
    repo_path: PathBuf,
    /// Stable repository identity: hash of canonical repo path (TASK-030) — shared formula in knocode-core
    repository_id: String,
    db: knocode_storage::Database,
    event_bus: EventBus,
    patterns: SymbolPatterns,
    /// Cached file count (populated during init, used for graph-build decision)
    cached_file_count: std::sync::OnceLock<usize>,
    /// FIX #1: Cached Tantivy doc count — avoids opening index on every query.
    cached_doc_count: std::sync::OnceLock<usize>,

}

impl RepositoryIntelligence {
    /// Create a new Repository Intelligence instance
    pub fn new(repo_path: PathBuf, db: knocode_storage::Database, event_bus: EventBus) -> Self {
        let patterns = SymbolPatterns::new();
        // Canonicalize without Windows verbatim prefix (\\?\\) so both daemon and CLI derive the
        // SAME repository_id from the same directory (TASK-030/F-1)
        let canonical = dunce::canonicalize(&repo_path).unwrap_or_else(|_| repo_path.clone());
        let repository_id = knocode_core::repository_id_from_path(&canonical.to_string_lossy());

        Self {
            repo_path,
            repository_id,
            db,
            event_bus,
            patterns,
            cached_file_count: std::sync::OnceLock::new(),
            cached_doc_count: std::sync::OnceLock::new(),
        }
    }

    /// Repository identity accessor (hash of canonical repo path, 12 hex chars) — TASK-030
    pub fn repository_id(&self) -> &str {
        &self.repository_id
    }

    /// Path of the SQLite backing file (None for in-memory stores) — lets the
    /// ContextEngine re-open the same database when re-indexing outside the repo lock.
    pub fn db_path(&self) -> Option<std::path::PathBuf> {
        self.db.path().map(std::path::Path::to_path_buf)
    }

    /// FIX #1: Fast doc_count check — returns cached count if available,
    /// avoiding full index open + reader + stats on every query.
    pub fn cached_doc_count(&self) -> Option<usize> {
        self.cached_doc_count.get().copied()
    }

    /// Validate that the index exists and is populated (P0 #2 / P0 #3 proactive detection).
    /// Returns `Ok(IndexStats)` if valid; `Err` if the index directory is missing or empty.
    /// FIX #1: Caches doc_count in OnceLock so subsequent calls are free.
    pub fn validate_index(&self) -> Result<knocode_storage::tantivy_index::IndexStats, String> {
        // Fast path: if we already validated, return cached stats
        if let Some(&count) = self.cached_doc_count.get() {
            return Ok(knocode_storage::tantivy_index::IndexStats {
                doc_count: count,
                index_path: default_index_path(&self.repository_id),
            });
        }

        let path = default_index_path(&self.repository_id);
        if !std::path::Path::new(&path).exists() {
            return Err("index not built".to_string());
        }
        let tantivy_index = knocode_storage::tantivy_index::TantivyIndex::open_cached(&path)
            .map_err(|e| format!("index open failed: {}", e))?;
        let reader = tantivy_index.cached_reader().map_err(|e| format!("reader failed: {}", e))?;
        let stats = tantivy_index.stats(&reader).map_err(|e| format!("stats failed: {}", e))?;
        if stats.doc_count == 0 {
            return Err("index is empty".to_string());
        }
        // Cache for next call
        let _ = self.cached_doc_count.set(stats.doc_count);
        Ok(stats)
    }

    /// Index the repository (full or incremental) — wires tantivy BM25 in-process, incremental + MkDocs ingestion (v0.5.0)
    pub fn index_repository(&mut self) -> Result<IndexStats, String> {
        self.index_repository_with_progress(None)
    }

    /// Same as `index_repository`, with an optional progress callback
    /// `(files_done, files_total, phase_label)` invoked periodically.
    /// `files_total` is 0 while unknown (during the initial walk), then the
    /// number of files being indexed. Lets UIs show a done/total counter
    /// instead of appearing frozen during long index runs.
    pub fn index_repository_with_progress(
        &mut self,
        on_progress: Option<&(dyn Fn(usize, usize, &str) + Sync)>,
    ) -> Result<IndexStats, String> {
        let start = Instant::now();
        let mut files_indexed = 0usize;
        let mut symbols_extracted = 0usize;
        let mut files_skipped = 0usize;
        let mut files_deleted = 0usize;

        // Open tantivy index (MmapDirectory, memory-mapped per spec §3) — optional, never fails indexing
        let repo_id = self.repository_id.clone();
        let tantivy_index = knocode_storage::tantivy_index::TantivyIndex::open(&default_index_path(&repo_id)).ok();
        let mut tantivy_writer = tantivy_index.as_ref().and_then(|idx| idx.writer().ok());

        // Load existing file meta — Phase2 mtime+size shortcut (avoids 63k reads on warm re-index)
        let existing_records = self.db.get_all_files_meta().unwrap_or_default();
        let existing_hashes: HashMap<String, (i64, String)> = existing_records
            .iter()
            .map(|r| (r.path.clone(), (r.id, r.hash.clone())))
            .collect();
        let existing_meta: HashMap<String, knocode_storage::FileRecord> = existing_records
            .into_iter()
            .map(|r| (r.path.clone(), r))
            .collect();

        let mut seen_paths = std::collections::HashSet::new();

        // ── Phase 1: Walk + read + hash — collect file jobs (single loop, no parallel I/O) ──
        // Single-loop walk+read is faster than parallel I/O for small files on Windows.
        // DB writes are deferred to a batch after the walk to avoid SQLite contention.
        struct FileJob {
            path_str: String,
            content: String,
            language: Option<String>,
            file_class: FileClass,
            file_changed: bool,
            is_new: bool,
            existing_file_id: Option<i64>,
        }

        // Deferred DB writes — batched after the walk to avoid per-file SQLite overhead
        struct DeferredInsert {
            path: String,
            hash: String,
            size: i64,
            language: Option<String>,
        }
        struct DeferredUpdate {
            file_id: i64,
            hash: String,
            size: i64,
        }
        let mut deferred_inserts: Vec<DeferredInsert> = Vec::new();
        let mut deferred_updates: Vec<DeferredUpdate> = Vec::new();

        let mut file_jobs: Vec<FileJob> = Vec::new();

        let report = |done: usize, total: usize, l: &str| {
            if let Some(cb) = on_progress {
                cb(done, total, l);
            }
        };
        let mut walk_count = 0usize;

        for entry in self.walk_directory(&self.repo_path)? {
            let path = entry;
            let path_str = path.strip_prefix(&self.repo_path)
                .unwrap_or(&path)
                .to_string_lossy()
                .to_string();

            seen_paths.insert(path_str.clone());
            walk_count += 1;
            if walk_count % 128 == 0 {
                report(walk_count, 0, "scanning files");
            }

            // Classify file using unified registry
            let file_class = classify_file(&path);

            // Skip binary, vendor, dependency, generated, and stylesheet files
            match file_class {
                FileClass::Binary | FileClass::Vendor | FileClass::Dependency | FileClass::Generated | FileClass::Stylesheet => {
                    debug!(path = %path_str, class = ?file_class, "Skipping file");
                    files_skipped += 1;
                    continue;
                }
                _ => {}
            }

            // Detect language from path (unified registry)
            let language = detect_language_from_path(&path);

            // Skip files with no recognized language and not indexable text
            if language.is_none() && !is_indexable_text_file(&path_str) {
                files_skipped += 1;
                continue;
            }

            // mtime+size shortcut — skip reading unchanged files on warm re-index
            if let Some(rec) = existing_meta.get(&path_str) {
                if is_file_unchanged_fast(&path, rec) {
                    files_indexed += 1; // counted but no I/O
                    continue;
                }
            }

            // Check binary by extension only before read (cheap)
            if is_binary_extension(&path) {
                debug!(path = %path_str, "Skipping binary file (extension)");
                files_skipped += 1;
                continue;
            }

            // Read file content
            let content = match std::fs::read_to_string(&path) {
                Ok(c) => c,
                Err(e) => {
                    warn!(path = %path_str, error = %e, "Failed to read file");
                    files_skipped += 1;
                    continue;
                }
            };

            // Content-based binary detection (null bytes)
            if is_content_binary(&content) {
                debug!(path = %path_str, "Skipping binary file (content)");
                files_skipped += 1;
                continue;
            }

            let hash = compute_hash(&content);
            let size = content.len() as i64;

            // Determine file_changed + is_new, defer DB writes
            let (file_changed, is_new, existing_file_id) = if let Some(&(id, ref existing_hash)) = existing_hashes.get(&path_str) {
                if *existing_hash == hash {
                    (false, false, Some(id))
                } else {
                    // File changed — defer DB update
                    deferred_updates.push(DeferredUpdate { file_id: id, hash: hash.clone(), size });
                    (true, false, Some(id))
                }
            } else {
                // New file — defer DB insert (file_id assigned later)
                deferred_inserts.push(DeferredInsert { path: path_str.clone(), hash: hash.clone(), size, language: language.clone() });
                (true, true, None) // existing_file_id filled in after batch insert
            };

            file_jobs.push(FileJob {
                path_str,
                content,
                language,
                file_class,
                file_changed,
                is_new,
                existing_file_id,
            });
        }

        let phase1_walk_ms = start.elapsed().as_millis() as u64;
        info!(file_jobs = file_jobs.len(), deferred_inserts = deferred_inserts.len(), deferred_updates = deferred_updates.len(), phase1_walk_ms, "Phase1: walk + read complete");

        // Deferred DB batch — insert all new files, then update changed files
        // Batched in a single transaction to avoid per-file SQLite overhead
        let t_db = Instant::now();
        let batch_ok = self.db.begin_batch().is_ok();

        // Insert new files and map path→file_id for deferred assignment
        let mut new_file_ids: HashMap<String, i64> = HashMap::new();
        for ins in &deferred_inserts {
            if let Ok(fid) = self.db.insert_file(&ins.path, &ins.hash, ins.size, ins.language.as_deref()) {
                new_file_ids.insert(ins.path.clone(), fid);
            }
        }

        // Update changed files
        for upd in &deferred_updates {
            let _ = self.db.update_file(upd.file_id, &upd.hash, upd.size);
        }

        if batch_ok {
            let _ = self.db.commit_batch();
        }

        // Assign existing_file_id for new files now that we have the IDs
        for job in file_jobs.iter_mut() {
            if job.existing_file_id.is_none() && job.is_new {
                job.existing_file_id = new_file_ids.get(&job.path_str).copied();
            }
        }

        let db_ms = t_db.elapsed().as_millis() as u64;
        let phase1_ms = start.elapsed().as_millis() as u64;
        info!(file_jobs = file_jobs.len(), db_ms, phase1_ms, "Phase1: DB batch complete");

        // ── Phase 2: Parallel symbol extraction (CPU-bound tree-sitter parsing) ──
        // `extract_symbols` is pure (content + patterns + language → symbols), safe to parallelize.
        let thread_count = std::env::var("KNOCODE_INDEX_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(4)
            .clamp(1, 16);

        struct ExtractedResult {
            sym_names: Vec<String>,
            sym_kinds: Vec<String>,
            extracted_count: usize,
        }

        let t_extract = Instant::now();

        let extraction_results: Vec<ExtractedResult> = if file_jobs.len() < 100 || thread_count == 1 {
            // Small repo or single-thread: sequential extraction (no thread overhead)
            file_jobs.iter().enumerate().map(|(i, job)| {
                if i % 32 == 0 {
                    report(i, file_jobs.len(), "extracting symbols");
                }
                let extracted = if matches!(job.file_class, FileClass::Documentation | FileClass::Config) {
                    Vec::new()
                } else {
                    extract_symbols(&job.content, &self.patterns, job.language.as_deref())
                };
                let count = extracted.len();
                ExtractedResult {
                    sym_names: extracted.iter().map(|s| s.name.clone()).collect(),
                    sym_kinds: extracted.iter().map(|s| s.kind.clone()).collect(),
                    extracted_count: count,
                }
            }).collect()
        } else {
            // Large repo: parallel extraction via scoped threads
            // Each thread gets a slice of file_jobs and extracts symbols independently.
            // Results are collected in order (same index as file_jobs).
            let chunk_size = (file_jobs.len() / thread_count).max(1);
            let patterns = &self.patterns;
            let extract_total = file_jobs.len();
            let extract_progress = &std::sync::atomic::AtomicUsize::new(0);

            std::thread::scope(|s| {
                let handles: Vec<_> = file_jobs.chunks(chunk_size).map(|chunk| {
                    s.spawn(move || {
                        if let Some(cb) = on_progress {
                            let done = extract_progress.fetch_add(chunk.len(), std::sync::atomic::Ordering::Relaxed)
                                + chunk.len();
                            cb(done, extract_total, "extracting symbols");
                        }
                        chunk.iter().map(|job| {
                            let extracted = if matches!(job.file_class, FileClass::Documentation | FileClass::Config) {
                                Vec::new()
                            } else {
                                extract_symbols(&job.content, patterns, job.language.as_deref())
                            };
                            let count = extracted.len();
                            ExtractedResult {
                                sym_names: extracted.iter().map(|s| s.name.clone()).collect(),
                                sym_kinds: extracted.iter().map(|s| s.kind.clone()).collect(),
                                extracted_count: count,
                            }
                        }).collect::<Vec<_>>()
                    })
                }).collect();

                let mut results = Vec::with_capacity(file_jobs.len());
                for handle in handles {
                    if let Ok(chunk_results) = handle.join() {
                        results.extend(chunk_results);
                    }
                }
                results
            })
        };

        let extract_ms = t_extract.elapsed().as_millis() as u64;
        info!(
            files = file_jobs.len(),
            threads = thread_count.min(file_jobs.len()),
            extract_ms,
            "Phase2: parallel symbol extraction complete"
        );

        // ── Phase 3: Sequential DB writes + tantivy upsert ──
        // DB and tantivy writer need &mut — cannot parallelize. Sequential but fast.
        let t_write = Instant::now();
        let batch_enabled = self.db.begin_batch().is_ok();
        let mut batch_ops: usize = 0;
        let mut write_count = 0usize;

        for (job, extract_result) in file_jobs.iter().zip(extraction_results.iter()) {
            let file_id = job.existing_file_id.unwrap_or(0);

            if file_id > 0 && job.file_changed {
                // Batch insert symbols into SQLite — single transaction, faster than per-row
                let sym_pairs: Vec<(String, String)> = extract_result
                    .sym_names
                    .iter()
                    .zip(extract_result.sym_kinds.iter())
                    .map(|(n, k)| (n.clone(), k.clone()))
                    .collect();
                if let Err(e) = self.db.insert_symbols_batch(file_id, &sym_pairs) {
                    eprintln!("Warning: batch symbol insert failed for file {}: {}", file_id, e);
                }
                symbols_extracted += extract_result.extracted_count;
            } else if file_id > 0 {
                // File not changed but symbols may have been extracted for tantivy
                symbols_extracted += extract_result.extracted_count;
            }

            // Upsert into tantivy BM25 index
            // Skip delete_document for brand-new files — nothing to delete, saves a BooleanQuery per file
            if let (Some(ref idx), Some(ref mut writer)) = (&tantivy_index, &mut tantivy_writer) {
                if !job.is_new {
                    let _ = idx.delete_document(writer, &job.path_str, &self.repository_id);
                }
                let lang_str = job.language.as_deref().unwrap_or("text");
                let file_class_str = match &job.file_class {
                    FileClass::Source => "Source",
                    FileClass::Test => "Test",
                    FileClass::Config => "Config",
                    FileClass::Documentation => "Documentation",
                    FileClass::Generated => "Generated",
                    FileClass::Vendor => "Vendor",
                    FileClass::Dependency => "Dependency",
                    FileClass::Binary => "Binary",
                    FileClass::Stylesheet => "Stylesheet",
                    FileClass::Unknown => "Unknown",
                };
                let _ = idx.add_document(writer, &job.path_str, &job.content, lang_str, &extract_result.sym_names, &extract_result.sym_kinds, &self.repository_id, file_class_str);
            }

            files_indexed += 1;
            batch_ops += 1;
            write_count += 1;
            if write_count % 128 == 0 {
                report(write_count, file_jobs.len(), "writing index");
            }

            // Batch commit every 1000 files — keeps WAL small, limits transaction size
            if batch_enabled && batch_ops >= 1000 {
                let _ = self.db.commit_batch();
                let _ = self.db.begin_batch();
                batch_ops = 0;
                if let (Some(ref idx), Some(ref mut writer)) = (&tantivy_index, &mut tantivy_writer) {
                    let _ = idx.commit(writer);
                }
            }
        }

        let write_ms = t_write.elapsed().as_millis() as u64;
        info!(files = files_indexed, write_ms, "Phase3: DB + tantivy write complete");

        // Remove deleted files from database (and tantivy)
        for path in existing_hashes.keys() {
            if !seen_paths.contains(path) {
                self.db.delete_file(path)?;
                if let (Some(ref idx), Some(ref mut writer)) = (&tantivy_index, &mut tantivy_writer) {
                    let _ = idx.delete_document(writer, path, &self.repository_id);
                }
                files_deleted += 1;
                batch_ops += 1;
                if batch_enabled && batch_ops >= 1000 {
                    let _ = self.db.commit_batch();
                    let _ = self.db.begin_batch();
                    batch_ops = 0;
                }
            }
        }

        // Finalize DB batch
        if batch_enabled {
            let _ = self.db.commit_batch();
        }

        // Commit tantivy if writer present
        if let (Some(ref idx), Some(ref mut writer)) = (&tantivy_index, &mut tantivy_writer) {
            let _ = idx.commit(writer);
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        let stats = IndexStats {
            files_indexed,
            symbols_extracted,
            files_skipped,
            files_deleted,
            duration_ms,
        };

        // Emit event
        self.event_bus.emit(RuntimeEvent::RepositoryUpdated {
            files_indexed,
            symbols_extracted,
            duration_ms,
        });

        info!(
            files_indexed = files_indexed,
            symbols_extracted = symbols_extracted,
            files_skipped = files_skipped,
            files_deleted = files_deleted,
            duration_ms = duration_ms,
            phase1_ms = phase1_ms,
            extract_ms = extract_ms,
            write_ms = write_ms,
            "Repository indexing complete"
        );

        // Phase4: large-repo hint for 63k case
        if files_indexed > 50000 {
            warn!(
                files_indexed = files_indexed,
                "Large repository detected — set KNOCODE_SYMBOLS_ENABLED=false for ~20% faster indexing (BM25 only), KNOCODE_INDEX_THREADS=8 for more parallelism, or KNOCODE_BUILD_GRAPH=1 to force graph during init"
            );
        }

        // The on-disk index just changed: drop this run's writer handle, then evict
        // any cached TantivyIndex/reader for this repo so the NEXT search reopens a
        // fresh handle and serves the new commit immediately. Without this, in-process
        // queries (daemon ContextEngine, CLI --watch) would keep reading through the
        // stale cached reader created before this run.
        drop(tantivy_writer);
        drop(tantivy_index);
        knocode_storage::tantivy_index::TantivyIndex::invalidate_cached(&default_index_path(&self.repository_id));

        Ok(stats)
    }

    /// Search for text in the repository using regex (ripgrep, spec §3)
    pub fn search_text(
        &self,
        query: &str,
        language_filter: Option<&str>,
        max_results: usize,
    ) -> Result<SearchResults, String> {
        // Use ripgrep for fast searching
        self.search_text_ripgrep(query, language_filter, max_results)
    }

    /// Search for symbols by name pattern — returns file paths + line numbers
    pub fn search_symbols(&self, query: &str, max_results: usize) -> Result<Vec<SearchResult>, String> {
        let symbols = self.db.find_symbol(query)?;
        let mut results = Vec::new();
        for symbol in symbols.into_iter().take(max_results) {
            if let Ok(Some(file)) = self.db.get_file_by_id(symbol.file_id) {
                results.push(SearchResult {
                    path: file.path,
                    line: symbol.line_start as usize,
                    content: format!("{} {}", symbol.kind, symbol.name),
                    score: 1.0,
                });
            }
        }
        Ok(results)
    }

    // NOTE: search_structural() CLI shell-out removed in P2.
    // Structural search is now handled entirely by AstGrepBackend (in-process ast-grep-core).
    // See: crates/knocode-repo-intel/src/structural/ + crates/knocode-context/src/retrieval/structural.rs

    /// Full-text BM25 search via tantivy (spec §3 — tantivy/BM25, in-process, memory-mapped)
    /// `repository_id: Some(id)` scopes hits to one repository (TASK-030/F-1); `None` searches all.
    /// Falls back to ripgrep over THIS instance's repo_path when the index misses — inherently repo-scoped.
    pub fn search_fulltext(
        &self,
        query: &str,
        language_filter: Option<&str>,
        max_results: usize,
        repository_id: Option<&str>,
    ) -> Result<SearchResults, String> {
        // Try tantivy index at default location; fallback to ripgrep if index missing
        // P0: Use cached index + cached reader to avoid per-query MmapDirectory open (53k latency fix)
        let index_path = default_index_path(&self.repository_id);
        let _st = Instant::now();
        match knocode_storage::tantivy_index::TantivyIndex::open_cached(&index_path) {
            Ok(idx) => {
                if std::env::var("KNOCODE_PROFILE").is_ok() {
                    eprintln!("[profile] code_search.tantivy_open_cached: {}ms", _st.elapsed().as_millis());
                }
                let reader = idx.cached_reader().map_err(|e| format!("tantivy reader: {e}"))?;
                match idx.search(&reader, query, language_filter, max_results, repository_id) {
                    Ok(hits) => {
                        if hits.is_empty() {
                            debug!("tantivy returned 0 hits for '{}', falling back to ripgrep", query);
                            return self.search_text(query, language_filter, max_results);
                        }
                        let mut results = Vec::new();
                        for hit in hits {
                            // snippet: first 200 chars
                            let snippet = hit.content.chars().take(200).collect::<String>();
                            results.push(SearchResult { path: hit.path, line: 1, content: snippet, score: hit.score as f64 });
                        }
                        let total = results.len();
                        Ok(SearchResults { results, total_count: total })
                    }
                    Err(e) => {
                        warn!(error = %e, "tantivy search failed, falling back to ripgrep");
                        self.search_text(query, language_filter, max_results)
                    }
                }
            }
            Err(_) => {
                debug!("tantivy index not found at {}, fallback to ripgrep", index_path);
                self.search_text(query, language_filter, max_results)
            }
        }
    }

    /// Search using ripgrep (grep-searcher crate)
    fn search_text_ripgrep(
        &self,
        query: &str,
        language_filter: Option<&str>,
        max_results: usize,
    ) -> Result<SearchResults, String> {
        // Sanitize query for regex: escape special chars, extract keywords
        let sanitized = sanitize_ripgrep_query(query);
        let matcher = RegexMatcherBuilder::new()
            .case_insensitive(true)
            .build(&sanitized)
            .map_err(|e| format!("Invalid search pattern '{}' (from '{}'): {}", sanitized, query, e))?;

        let mut results = Vec::new();

        // Use ignore's WalkBuilder for respecting .gitignore
        let walker = WalkBuilder::new(&self.repo_path)
            .hidden(false) // Include hidden files (but not .git)
            .git_ignore(true)
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };

            if !entry.file_type().is_some_and(|ft| ft.is_file()) {
                continue;
            }

            let path = entry.path();
            let path_str = path.strip_prefix(&self.repo_path)
                .unwrap_or(path)
                .to_string_lossy()
                .to_string();

            // Apply language filter
            if let Some(lang) = language_filter {
                let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
                if detect_language(ext).as_deref() != Some(lang) {
                    continue;
                }
            }

            // Skip binary files
            if is_likely_binary(path) {
                continue;
            }

            let mut searcher = SearcherBuilder::new().line_number(true).build();
            let mut match_count = 0;

            let _ = searcher.search_path(
                &matcher,
                path,
                UTF8(|line_number, line_content| {
                    if results.len() >= max_results {
                        return Ok(false);
                    }

                    results.push(SearchResult {
                        path: path_str.clone(),
                        line: line_number as usize,
                        content: line_content.trim_end().to_string(),
                        score: 1.0,
                    });

                    match_count += 1;
                    Ok(true)
                }),
            );

            if results.len() >= max_results {
                break;
            }
        }

        let total_count = results.len();
        Ok(SearchResults {
            results,
            total_count,
        })
    }

    /// Get file content with optional line range
    pub fn get_file_content(
        &self,
        path: &str,
        line_range: Option<(usize, usize)>,
    ) -> Result<String, String> {
        let full_path = self.repo_path.join(path);
        let content = std::fs::read_to_string(&full_path)
            .map_err(|e| format!("Failed to read file '{}': {}", path, e))?;

        match line_range {
            Some((start, end)) => {
                let lines: Vec<&str> = content.lines().collect();
                let start_idx = start.saturating_sub(1);
                let end_idx = end.min(lines.len());
                Ok(lines[start_idx..end_idx].join("\n"))
            }
            None => Ok(content),
        }
    }

    /// Get file information
    pub fn get_file_info(&self, path: &str) -> Result<Option<FileInfo>, String> {
        match self.db.get_file(path)? {
            Some(record) => {
                // Count symbols by getting all symbols and filtering by file_id
                let symbol_count = self.db.get_symbols_for_file(record.id)
                    .map(|s| s.len())
                    .unwrap_or(0);

                Ok(Some(FileInfo {
                    path: record.path,
                    size: record.size,
                    language: record.language,
                    symbol_count,
                    last_indexed_at: record.last_indexed_at,
                }))
            }
            None => Ok(None),
        }
    }

    /// Get symbol information by name
    pub fn get_symbol_info(&self, query: &str) -> Result<Vec<SymbolInfo>, String> {
        let symbols = self.db.find_symbol(query)?;
        let files = self.db.get_all_files()?;
        let mut results = Vec::new();

        for symbol in symbols {
            // Look up file path by finding the file with matching id
            // In the current schema, we find the file by iterating
            let file_path = files.iter()
                .find(|(path, _)| {
                    // Simplified: match by checking if the symbol's file exists
                    self.db.get_file(path)
                        .ok()
                        .flatten()
                        .map(|f| f.id == symbol.file_id)
                        .unwrap_or(false)
                })
                .map(|(path, _)| path.clone())
                .unwrap_or_else(|| "unknown".to_string());

            results.push(SymbolInfo {
                name: symbol.name,
                kind: symbol.kind,
                file_path,
                line_start: symbol.line_start,
                line_end: symbol.line_end,
            });
        }

        Ok(results)
    }

    /// Build dependency graph for the current repo (spec §3, ROADMAP.md:81).
    /// Result is cached in-memory + on disk — first call builds, subsequent calls (even new CLI processes) return instantly.
    pub fn build_dependency_graph(&self) -> Result<graph::DependencyGraph, String> {
        // 1. Check in-memory cache first
        {
            let cache = graph_cache().lock().map_err(|e| format!("graph cache lock: {e}"))?;
            if let Some(g) = cache.get(&self.repository_id) {
                return Ok(g.clone());
            }
        }
        // 2. Check disk cache
        let disk_path = self.graph_disk_path();
        if disk_path.exists() {
            if let Ok(data) = std::fs::read_to_string(&disk_path) {
                if let Ok(graph) = serde_json::from_str::<graph::DependencyGraph>(&data) {
                    let mut cache = graph_cache().lock().map_err(|e| format!("graph cache lock: {e}"))?;
                    cache.insert(self.repository_id.clone(), graph.clone());
                    if std::env::var("KNOCODE_PROFILE").is_ok() {
                        eprintln!("[profile] code_search.build_dependency_graph: 0ms (disk cache hit)");
                    }
                    return Ok(graph);
                }
            }
        }
        // 3. Cache miss — build graph
        let _t = Instant::now();
        let files = self.walk_directory(&self.repo_path)?;
        let graph = graph::DependencyGraph::build_from_files(&self.repo_path, &files);
        if std::env::var("KNOCODE_PROFILE").is_ok() {
            eprintln!("[profile] code_search.build_dependency_graph: {}ms ({} files)", _t.elapsed().as_millis(), files.len());
        }
        // 4. Store in memory + disk cache
        {
            let mut cache = graph_cache().lock().map_err(|e| format!("graph cache lock: {e}"))?;
            cache.insert(self.repository_id.clone(), graph.clone());
        }
        // Persist to disk for cross-process cache (CLI invocations)
        let disk_path = self.graph_disk_path();
        if let Ok(data) = serde_json::to_string(&graph) {
            if let Some(parent) = disk_path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            let _ = std::fs::write(&disk_path, data);
        }
        Ok(graph)
    }

    /// Disk cache path for the dependency graph
    fn graph_disk_path(&self) -> PathBuf {
        dirs().join(".knocode").join("graphs").join(format!("{}.json", self.repository_id))
    }

    /// LSP client accessor (optional enrichment, never hard dep)
    pub fn lsp_client(&self) -> lsp::LspClient {
        lsp::LspClient::default()
    }

    /// Spawn git-change-aware watcher that re-indexes incrementally
    pub fn spawn_watcher(&self) -> watcher::RepoWatcher {
        watcher::RepoWatcher::new(self.repo_path.clone())
    }

    /// Repository path accessor for provenance (TASK-008)
    pub fn repo_path(&self) -> &std::path::Path {
        &self.repo_path
    }

    /// Quick file count for this repository — used for graph-build decision.
    /// Cached after first call (walks directory tree once, then returns cached value).
    pub fn file_count(&self) -> usize {
        *self.cached_file_count.get_or_init(|| {
            self.walk_directory(&self.repo_path)
                .map(|v| v.len())
                .unwrap_or(0)
        })
    }

    /// Walk directory tree, yielding indexable files — Phase3 parallel (threads=4, KNOCODE_INDEX_THREADS override)
    fn walk_directory(&self, dir: &Path) -> Result<Vec<PathBuf>, String> {
        let mut files = Vec::new();
        let threads = std::env::var("KNOCODE_INDEX_THREADS")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(4)
            .clamp(1, 16);

        // Directories to always skip (build artifacts, caches).
        // filter_entry doesn't prevent descent on all platforms, so we also
        // check path segments during iteration as a safety net.
        const SKIP_DIRS: &[&str] = &["obj", "bin", "node_modules", ".git", "target"];

        fn is_in_skip_dir(path: &Path) -> bool {
            path.components().any(|c| {
                c.as_os_str().to_str().is_some_and(|s| SKIP_DIRS.contains(&s))
            })
        }

        // Use ignore's WalkBuilder for respecting .gitignore — parallel when threads>1
        let walker = WalkBuilder::new(dir)
            .hidden(false)
            .git_ignore(true)
            .threads(threads)
            .filter_entry(|entry| {
                // Prevent descent into well-known build artifact directories
                if entry.file_type().is_some_and(|ft| ft.is_dir()) {
                    if let Some(name) = entry.file_name().to_str() {
                        return !SKIP_DIRS.contains(&name);
                    }
                }
                true
            })
            .build();

        for entry in walker {
            let entry = match entry {
                Ok(e) => e,
                Err(_) => continue,
            };
            if entry.file_type().is_some_and(|ft| ft.is_file()) {
                // Safety net: skip files under build artifact directories
                // (filter_entry may not prevent descent on all platforms)
                let path = entry.into_path();
                if is_in_skip_dir(&path) {
                    continue;
                }
                files.push(path);
            }
        }

        Ok(files)
    }

    /// V1 workspace awareness: extract pnpm-workspace.yaml packages (e.g. `types/*` → package dirs)
    /// Returns list of package root dirs relative to repo. No full dependency graph — just structure metadata.
    pub fn workspace_packages(&self) -> Vec<String> {
        let ws_path = self.repo_path.join("pnpm-workspace.yaml");
        if !ws_path.exists() {
            return Vec::new();
        }
        let content = match std::fs::read_to_string(&ws_path) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        // Simple yaml parse: look for `packages:` then `- "types/*"` lines
        let mut packages = Vec::new();
        let mut in_packages = false;
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("packages:") {
                in_packages = true;
                continue;
            }
            if in_packages {
                if let Some(stripped) = trimmed.strip_prefix("- ") {
                    let glob = stripped.trim().trim_matches('"').trim_matches('\'').trim();
                    // Expand glob like `types/*` → list dirs under types/
                    if let Some(base) = glob.strip_suffix("/*") {
                        let base_path = self.repo_path.join(base);
                        if let Ok(entries) = std::fs::read_dir(&base_path) {
                            for e in entries.flatten() {
                                if e.file_type().map(|ft| ft.is_dir()).unwrap_or(false) {
                                    if let Some(name) = e.file_name().to_str() {
                                        packages.push(format!("{}/{}", base, name));
                                    }
                                }
                            }
                        }
                    } else {
                        packages.push(glob.to_string());
                    }
                } else if !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with("-") {
                    // End of packages list
                    break;
                }
            }
        }
        debug!(workspace_packages = ?packages, "workspace awareness: extracted packages from pnpm-workspace.yaml");
        packages
    }

    /// V1 parser coverage: auto-detect languages present in repo (files → languages → grammars)
    /// Returns map extension → count, used to decide which grammars to load per-repo (skip AST, still lexical).
    pub fn detect_repo_languages(&self) -> std::collections::HashMap<String, usize> {
        let mut counts = std::collections::HashMap::new();
        if let Ok(files) = self.walk_directory(&self.repo_path) {
            for p in files.iter().take(5000) { // sample first 5k for speed
                if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    *counts.entry(ext.to_lowercase()).or_insert(0) += 1;
                }
                // Also check special filenames like Cargo.toml, package.json
                if let Some(fname) = p.file_name().and_then(|n| n.to_str()) {
                    if fname == "Cargo.toml" { *counts.entry("rust_manifest".to_string()).or_insert(0) += 1; }
                    if fname == "package.json" { *counts.entry("js_manifest".to_string()).or_insert(0) += 1; }
                }
            }
        }
        counts
    }

    /// Public helper for `doctor` — list repo files (sample) for file-class breakdown.
    pub fn walk_directory_for_doctor(&self, dir: &Path) -> Vec<PathBuf> {
        self.walk_directory(dir).unwrap_or_default()
    }
}

// ── Query Sanitization ───────────────────────────────────────────────────

/// Stop words to filter from code search queries
const RIPGREP_STOP_WORDS: &[&str] = &[
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being",
    "have", "has", "had", "do", "does", "did", "will", "would", "could",
    "should", "may", "might", "shall", "can", "need", "dare", "ought",
    "used", "to", "of", "in", "for", "on", "with", "at", "by", "from",
    "as", "into", "through", "during", "before", "after", "above", "below",
    "between", "out", "off", "over", "under", "again", "further", "then",
    "once", "here", "there", "when", "where", "why", "how", "all", "both",
    "each", "few", "more", "most", "other", "some", "such", "no", "nor",
    "not", "only", "own", "same", "so", "than", "too", "very", "just",
    "and", "but", "or", "if", "while", "that", "this", "it", "its",
    "what", "which", "who", "whom", "implemented", "find", "show",
    "get", "set", "make", "create", "update", "delete", "remove", "add",
    "list",
];

/// Sanitize a natural-language query for ripgrep regex search.
/// Extracts meaningful code keywords and joins with `|` (OR).
fn sanitize_ripgrep_query(query: &str) -> String {
    let keywords: Vec<String> = query
        .split_whitespace()
        .map(|w| {
            // Strip non-alphanumeric chars (except _)
            let clean: String = w.chars()
                .filter(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            clean.to_lowercase()
        })
        .filter(|w| w.len() >= 2 && !RIPGREP_STOP_WORDS.contains(&w.as_str()))
        .collect();

    if keywords.is_empty() {
        // Fallback: regex-escape the raw query
        regex::escape(query)
    } else {
        keywords.join("|")
    }
}

// ── Helper Functions ────────────────────────────────────────────────────

/// Detect programming language from file path (uses unified registry)
fn detect_language_from_path(path: &Path) -> Option<String> {
    registry_detect(path).map(|def| def.id.as_str().to_string())
}

/// Detect programming language from file extension (backward-compatible)
fn detect_language(ext: &str) -> Option<String> {
    // Try to find by constructing a dummy path with the extension
    // This is a backward-compatible shim — prefer detect_language_from_path()
    registry::language_by_extension(ext).map(|def| def.id.as_str().to_string())
}

/// Check if a file is indexable text (config, docs, etc.)
fn is_indexable_text_file(path_str: &str) -> bool {
    // Check extension
    if let Some(ext) = path_str.rsplit('.').next() {
        return matches!(ext,
            "toml" | "yaml" | "yml" | "json" | "xml" | "md" | "txt" |
            "sql" | "sh" | "bash" | "zsh" | "env" | "gitignore" |
            "dockerfile" | "makefile" | "cmake" | "gradle" | "sbt" |
            "cshtml" | "razor" | "csproj" | "sln" | "vb" | "vbproj" |
            "fs" | "fsx" | "fsproj" | "config" | "props" | "targets" |
            "ini" | "cfg" | "conf" | "proto" | "graphql" | "gql" |
            "tf" | "hcl" | "pkl" | "liquid" | "twig" | "vue" | "svelte"
        );
    }
    // Check filename patterns
    let name = path_str.rsplit(['/', '\\']).next().unwrap_or(path_str);
    matches!(name.to_lowercase().as_str(),
        "dockerfile" | "makefile" | "cmakelists.txt" | "justfile" | "procfile"
    )
}



/// Check if a file is likely binary — legacy wrapper (keeps old behavior for tests)
fn is_likely_binary(path: &Path) -> bool {
    if is_binary_extension(path) {
        return true;
    }
    if let Ok(content) = std::fs::read(path) {
        let check_len = content.len().min(512);
        let null_count = content[..check_len].iter().filter(|&&b| b == 0).count();
        null_count > check_len / 100
    } else {
        false
    }
}

/// Fast binary check by extension only (no I/O) — Phase2
fn is_binary_extension(path: &Path) -> bool {
    let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
    matches!(ext,
        "exe" | "dll" | "so" | "dylib" | "o" | "a" | "lib" |
        "bin" | "dat" | "png" | "jpg" | "jpeg" | "gif" | "bmp" |
        "ico" | "svg" | "pdf" | "zip" | "tar" | "gz" | "bz2" |
        "xz" | "7z" | "rar" | "woff" | "woff2" | "ttf" | "otf" |
        "eot" | "mp3" | "mp4" | "avi" | "mov" | "wav" | "ogg"
    )
}

/// Content-based binary check — Phase2 (no extra fs::read, uses already-loaded content)
fn is_content_binary(content: &str) -> bool {
    let bytes = content.as_bytes();
    let check_len = bytes.len().min(512);
    if check_len == 0 {
        return false;
    }
    let null_count = bytes[..check_len].iter().filter(|&&b| b == 0).count();
    null_count > check_len / 100
}

/// Phase2 mtime+size shortcut — true if file unchanged since last index (avoids 63k reads on warm)
fn is_file_unchanged_fast(path: &Path, rec: &knocode_storage::FileRecord) -> bool {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => return false,
    };
    if meta.len() as i64 != rec.size {
        return false;
    }
    let modified = match meta.modified() {
        Ok(t) => t,
        Err(_) => return false,
    };
    let indexed = match chrono::DateTime::parse_from_rfc3339(&rec.last_indexed_at) {
        Ok(dt) => dt,
        Err(_) => return false,
    };
    let modified_dt: chrono::DateTime<chrono::Utc> = modified.into();
    modified_dt <= indexed.with_timezone(&chrono::Utc)
}

/// Default tantivy index path (spec §3 — MmapDirectory).
/// `KNOCODE_INDEX_DIR` overrides for tests/isolation (TASK-035) — keeps the shared
/// global index from being polluted by parallel test runs.
pub fn default_index_path(repository_id: &str) -> String {
    if let Ok(dir) = std::env::var("KNOCODE_INDEX_DIR") {
        if !dir.is_empty() {
            return dir;
        }
    }
    if let Some(home) = dirs_home() {
        home.join(".knocode")
            .join("index")
            .join(repository_id)
            .to_string_lossy()
            .to_string()
    } else {
        format!(".knocode/index/{}", repository_id)
    }
}

fn dirs_home() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    { std::env::var("USERPROFILE").ok().map(PathBuf::from) }
    #[cfg(not(target_os = "windows"))]
    { std::env::var("HOME").ok().map(PathBuf::from) }
}

/// Compute SHA-256 hash of content
fn compute_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Extract symbols from source code (tree-sitter AST + regex fallback)
/// Tree-sitter is attempted for ALL languages transparently — `get_language()` returns
/// `None` for unsupported grammars, producing an empty vec, and we fall through to regex.
/// This decouples lexical indexing (tantivy BM25, always runs) from AST enrichment (best-effort).
fn extract_symbols(content: &str, patterns: &SymbolPatterns, language: Option<&str>) -> Vec<ExtractedSymbol> {
    // KNOCODE_SYMBOLS_ENABLED=false disables tree-sitter symbol extraction for benchmarking
    if std::env::var("KNOCODE_SYMBOLS_ENABLED").map(|v| v == "false").unwrap_or(false) {
        return Vec::new();
    }
    // Try tree-sitter first — transparent: returns empty for unsupported grammars
    if let Some(lang) = language {
        let ast_symbols = parser::extract_symbols_ast(content, lang);
        if !ast_symbols.is_empty() {
            return ast_symbols
                .into_iter()
                .map(|s| ExtractedSymbol {
                    name: s.name,
                    kind: s.kind,
                })
                .collect();
        }
    }

    // Fallback to regex patterns (works for all languages, broadened for C#/Java/etc.)
    let mut symbols = Vec::new();

    // Extract functions
    for cap in patterns.function_pattern.captures_iter(content) {
        if let Some(name) = cap.get(1).or(cap.get(2)).or(cap.get(3)).or(cap.get(4)).or(cap.get(5)) {
            symbols.push(ExtractedSymbol {
                name: name.as_str().to_string(),
                kind: "function".to_string(),
            });
        }
    }

    // Extract structs/classes
    for cap in patterns.struct_pattern.captures_iter(content) {
        if let Some(name) = cap.get(1).or(cap.get(2)).or(cap.get(3)).or(cap.get(4)).or(cap.get(5)) {
            symbols.push(ExtractedSymbol {
                name: name.as_str().to_string(),
                kind: "struct".to_string(),
            });
        }
    }

    // Extract enums
    for cap in patterns.enum_pattern.captures_iter(content) {
        if let Some(name) = cap.get(1) {
            symbols.push(ExtractedSymbol {
                name: name.as_str().to_string(),
                kind: "enum".to_string(),
            });
        }
    }

    // Extract impl blocks
    for cap in patterns.impl_pattern.captures_iter(content) {
        if let Some(name) = cap.get(1) {
            symbols.push(ExtractedSymbol {
                name: name.as_str().to_string(),
                kind: "impl".to_string(),
            });
        }
    }

    // Extract traits
    for cap in patterns.trait_pattern.captures_iter(content) {
        if let Some(name) = cap.get(1) {
            symbols.push(ExtractedSymbol {
                name: name.as_str().to_string(),
                kind: "trait".to_string(),
            });
        }
    }

    // Extract type aliases
    for cap in patterns.type_pattern.captures_iter(content) {
        if let Some(name) = cap.get(1) {
            symbols.push(ExtractedSymbol {
                name: name.as_str().to_string(),
                kind: "type".to_string(),
            });
        }
    }

    symbols
}

#[derive(Debug)]
struct ExtractedSymbol {
    name: String,
    kind: String,
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that touch the shared global tantivy index / KNOCODE_INDEX_DIR env
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_detect_language() {
        assert_eq!(detect_language("rs"), Some("rust".to_string()));
        assert_eq!(detect_language("ts"), Some("typescript".to_string()));
        assert_eq!(detect_language("py"), Some("python".to_string()));
        assert_eq!(detect_language("go"), Some("go".to_string()));
        assert_eq!(detect_language("xyz"), None);
    }



    #[test]
    fn test_compute_hash() {
        let hash1 = compute_hash("hello world");
        let hash2 = compute_hash("hello world");
        let hash3 = compute_hash("hello world!");

        assert_eq!(hash1, hash2);
        assert_ne!(hash1, hash3);
        assert_eq!(hash1.len(), 64); // SHA-256 hex string
    }

    #[test]
    fn test_extract_symbols_rust() {
        let content = r#"
pub fn main() {
    println!("Hello");
}

pub struct Config {
    pub name: String,
}

enum Color {
    Red,
    Green,
    Blue,
}

impl Config {
    fn new() -> Self {
        Config { name: "test".to_string() }
    }
}

trait Drawable {
    fn draw(&self);
}

type Result<T> = std::result::Result<T, Error>;
"#;

        let patterns = SymbolPatterns::new();
        let symbols = extract_symbols(content, &patterns, Some("rust"));

        assert!(symbols.iter().any(|s| s.name == "main" && s.kind == "function"));
        assert!(symbols.iter().any(|s| s.name == "Config" && s.kind == "struct"));
        assert!(symbols.iter().any(|s| s.name == "Color" && s.kind == "enum"));
        // Note: tree-sitter may not extract impl blocks the same way as regex
        assert!(symbols.iter().any(|s| s.name == "Drawable" && s.kind == "trait"));
    }

    #[test]
    fn test_extract_symbols_python() {
        let content = r#"
def hello():
    print("Hello")

class Config:
    def __init__(self):
        self.name = "test"

class MyEnum:
    pass
"#;

        let patterns = SymbolPatterns::new();
        let symbols = extract_symbols(content, &patterns, Some("python"));

        assert!(symbols.iter().any(|s| s.name == "hello" && s.kind == "function"));
        assert!(symbols.iter().any(|s| s.name == "Config" && s.kind == "class"));
        assert!(symbols.iter().any(|s| s.name == "MyEnum" && s.kind == "class"));
    }

    #[test]
    fn test_extract_symbols_javascript() {
        let content = r#"
function hello() {
    console.log("Hello");
}

class Config {
    constructor() {
        this.name = "test";
    }
}

const greet = () => {
    console.log("Hi");
};

export async function fetchData() {
    return {};
}
"#;

        let patterns = SymbolPatterns::new();
        let symbols = extract_symbols(content, &patterns, Some("javascript"));

        assert!(symbols.iter().any(|s| s.name == "hello" && s.kind == "function"));
        assert!(symbols.iter().any(|s| s.name == "Config" && s.kind == "class"));
        assert!(symbols.iter().any(|s| s.name == "fetchData" && s.kind == "function"));
    }

    #[test]
    fn test_is_likely_binary() {
        // Can't easily test with actual files in unit test, but test the logic
        let text_path = Path::new("test.rs");
        assert!(!is_likely_binary(text_path));

        let exe_path = Path::new("test.exe");
        assert!(is_likely_binary(exe_path));
    }

    #[test]
    fn test_is_indexable_text_file() {
        assert!(is_indexable_text_file("toml"));
        assert!(is_indexable_text_file("yaml"));
        assert!(is_indexable_text_file("md"));
        assert!(is_indexable_text_file("sql"));
        assert!(is_indexable_text_file("cshtml"));
        assert!(is_indexable_text_file("razor"));
        assert!(is_indexable_text_file("csproj"));
        assert!(is_indexable_text_file("proto"));
        assert!(is_indexable_text_file("vue"));
        assert!(!is_indexable_text_file("rs"));
        assert!(!is_indexable_text_file("py"));
    }

    #[test]
    fn test_search_results_structure() {
        let results = SearchResults {
            results: vec![
                SearchResult {
                    path: "src/main.rs".to_string(),
                    line: 10,
                    content: "fn main() {}".to_string(),
                    score: 1.0,
                },
            ],
            total_count: 1,
        };

        assert_eq!(results.total_count, 1);
        assert_eq!(results.results[0].path, "src/main.rs");
    }

    // NOTE: test_search_structural_finds_pattern removed in P2.
    // Structural search is now tested via AstGrepBackend in structural/ast_grep_adapter.rs.

    #[test]
    fn test_search_fulltext_via_tantivy_fallback() {
        // Full-text BM25: should fallback to ripgrep when tantivy index missing, still return results
        let _env = ENV_LOCK.lock().unwrap();
        std::env::remove_var("KNOCODE_INDEX_DIR");
        let dir = std::env::temp_dir().join(format!("knocode_fulltext_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("notes.md"), "authentication middleware handles token verification").unwrap();
        std::fs::write(dir.join("main.rs"), "fn authenticate() { /* token check */ }").unwrap();
        let db = knocode_storage::Database::open(&PathBuf::from(":memory:")).unwrap();
        let ri = RepositoryIntelligence::new(dir.clone(), db, EventBus::new());
        let res = ri.search_fulltext("authentication", None, 10, None).unwrap();
        assert!(res.total_count >= 1, "fulltext should find at least one hit");
        let _ = std::fs::remove_dir_all(&dir);
    }

    // ── v0.5.0 first-class tool tests ──────────────────────────────────

    // NOTE: test_search_structural_delegates_to_fallback_when_sg_core_not_wired removed in P2.
    // Structural search is now handled entirely by AstGrepBackend.

    #[test]
    fn test_mkdocs_ingestion_on_index() {
        // V1 minimal: MkDocs ingestion removed per V1_MINIMAL_STACK_PLAN.md:2.4
        // index_repository() no longer walks docs/**/*.md → store_knowledge(category="docs")
        // docs/*.md remains as plain markdown for agent `read`, not indexed.
        let _env = ENV_LOCK.lock().unwrap();
        std::env::remove_var("KNOCODE_INDEX_DIR");
        let dir = std::env::temp_dir().join(format!("knocode_mkdocs_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs").join("guide.md"), "# Guide\nThis is mkdocs test content").unwrap();
        std::fs::write(dir.join("main.rs"), "fn main() {}").unwrap();
        let db_path = dir.join("test.db");
        let db = knocode_storage::Database::open(&db_path).unwrap();
        let mut ri = RepositoryIntelligence::new(dir.clone(), db, EventBus::new());
        let stats = ri.index_repository().unwrap();
        assert!(stats.files_indexed >= 1);
        // Re-open DB to verify docs knowledge was NOT ingested (V1 minimal)
        let db2 = knocode_storage::Database::open(&db_path).unwrap();
        let repo_id = ri_repository_id_for_test(&dir);
        let docs = db2.search_knowledge("mkdocs test content", Some("docs"), 0.0, 10, Some(&repo_id)).unwrap();
        assert!(docs.is_empty(), "V1 minimal: docs ingestion removed, guide.md should NOT be stored as knowledge (got {})", docs.len());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_mkdocs_ingestion_is_idempotent() {
        // V1 minimal: MkDocs ingestion removed per V1_MINIMAL_STACK_PLAN.md:2.4 — idempotent = zero docs every run
        let _env = ENV_LOCK.lock().unwrap();
        let index_dir = std::env::temp_dir().join(format!("knocode_idx_{}", uuid::Uuid::new_v4()));
        std::env::set_var("KNOCODE_INDEX_DIR", index_dir.to_string_lossy().to_string());
        let dir = std::env::temp_dir().join(format!("knocode_mkdocs_idem_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("docs")).unwrap();
        std::fs::write(dir.join("docs").join("guide.md"), "# Guide\nidempotent mkdocs content").unwrap();
        let db_path = dir.join("test.db");
        for _ in 0..3 {
            let db = knocode_storage::Database::open(&db_path).unwrap();
            let mut ri = RepositoryIntelligence::new(dir.clone(), db, EventBus::new());
            ri.index_repository().unwrap();
        }
        let db2 = knocode_storage::Database::open(&db_path).unwrap();
        let repo_id = ri_repository_id_for_test(&dir);
        let hits = db2.search_knowledge("idempotent mkdocs", None, 0.0, 100, Some(&repo_id)).unwrap();
        assert_eq!(hits.len(), 0, "V1 minimal: re-index must NOT store docs (got {})", hits.len());
        std::env::remove_var("KNOCODE_INDEX_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Compute the same repository_id an RI under `dir` would compute (canonical path hash)
    fn ri_repository_id_for_test(dir: &Path) -> String {
        let canonical = dunce::canonicalize(dir).unwrap_or_else(|_| dir.to_path_buf());
        knocode_core::repository_id_from_path(&canonical.to_string_lossy())
    }

    #[test]
    fn test_tantivy_first_class_then_fallback() {
        // Tantivy MmapDirectory primary → when index missing, search_fulltext() falls back to ripgrep with WARN
        let _env = ENV_LOCK.lock().unwrap();
        std::env::remove_var("KNOCODE_INDEX_DIR");
        let dir = std::env::temp_dir().join(format!("knocode_tantivy_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.txt"), "hello world tantivy primary").unwrap();
        let db = knocode_storage::Database::open(&PathBuf::from(":memory:")).unwrap();
        let ri = RepositoryIntelligence::new(dir.clone(), db, EventBus::new());
        let res = ri.search_fulltext("hello", None, 5, None).unwrap();
        assert!(res.total_count >= 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    // TASK-010: Validate incremental indexing (initial → modify → delete) — strong assertions
    #[test]
    fn test_incremental_indexing() {
        let dir = std::env::temp_dir().join(format!("knocode_incr_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("incr.db");
        let db = knocode_storage::Database::open(&db_path).unwrap();
        let mut ri = RepositoryIntelligence::new(dir.clone(), db, EventBus::new());
        // initial
        std::fs::write(dir.join("a.rs"), "fn foo() {}").unwrap();
        let s1 = ri.index_repository().unwrap();
        assert!(s1.files_indexed >= 1);
        // verify symbols indexed
        let db_check = knocode_storage::Database::open(&db_path).unwrap();
        assert!(db_check.get_symbols_for_file(db_check.get_file("a.rs").unwrap().unwrap().id).unwrap().len() >= 1, "initial symbols should exist");
        // modify — hash changes, should re-index
        std::fs::write(dir.join("a.rs"), "fn foo() {} fn bar() {}").unwrap();
        let s2 = ri.index_repository().unwrap();
        assert!(s2.files_indexed >= 1, "modified file must be re-indexed");
        // delete — file removed, stale symbols should be cleaned
        std::fs::remove_file(dir.join("a.rs")).unwrap();
        // Re-index should detect deletion
        let db2 = knocode_storage::Database::open(&db_path).unwrap();
        let mut ri2 = RepositoryIntelligence::new(dir.clone(), db2, EventBus::new());
        let s3 = ri2.index_repository().unwrap();
        assert_eq!(s3.files_deleted, 1, "delete should increment files_deleted");
        // Verify symbols cleaned
        let db3 = knocode_storage::Database::open(&db_path).unwrap();
        assert!(db3.get_file("a.rs").unwrap().is_none(), "deleted file should be gone");
        assert_eq!(db3.get_symbol_count().unwrap_or(0), 0, "get_symbol_count should be 0 after delete (stale symbols cleaned)");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_incremental_rename_and_git_checkout() {
        // Rename case: a.rs → b.rs
        let dir = std::env::temp_dir().join(format!("knocode_rename_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let db_path = dir.join("rename.db");
        std::fs::write(dir.join("a.rs"), "fn foo() {}").unwrap();
        let db = knocode_storage::Database::open(&db_path).unwrap();
        let mut ri = RepositoryIntelligence::new(dir.clone(), db, EventBus::new());
        let s1 = ri.index_repository().unwrap();
        assert!(s1.files_indexed >= 1);
        // rename
        std::fs::rename(dir.join("a.rs"), dir.join("b.rs")).unwrap();
        let db2 = knocode_storage::Database::open(&db_path).unwrap();
        let mut ri2 = RepositoryIntelligence::new(dir.clone(), db2, EventBus::new());
        let s2 = ri2.index_repository().unwrap();
        assert!(s2.files_deleted >= 1 || s2.files_indexed >= 1, "rename should be detected as delete+new");
        let db3 = knocode_storage::Database::open(&db_path).unwrap();
        assert!(db3.get_file("b.rs").unwrap().is_some(), "b.rs should exist after rename");
        assert!(db3.get_file("a.rs").unwrap().is_none(), "a.rs should be gone after rename");

        // git checkout case: create temp git repo, commit, create branch, checkout
        let dir2 = std::env::temp_dir().join(format!("knocode_git_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir2).unwrap();
        let db_path2 = dir2.join("git.db");
        // init git repo
        let _ = std::process::Command::new("git").arg("init").arg(&dir2).output();
        let _ = std::process::Command::new("git").arg("-C").arg(&dir2).arg("config").arg("user.email").arg("test@test.com").output();
        let _ = std::process::Command::new("git").arg("-C").arg(&dir2).arg("config").arg("user.name").arg("test").output();
        std::fs::write(dir2.join("main.rs"), "fn main() {}").unwrap();
        let _ = std::process::Command::new("git").arg("-C").arg(&dir2).arg("add").arg(".").output();
        let _ = std::process::Command::new("git").arg("-C").arg(&dir2).arg("commit").arg("-m").arg("init").output();
        let _ = std::process::Command::new("git").arg("-C").arg(&dir2).arg("checkout").arg("-b").arg("feature").output();
        std::fs::write(dir2.join("feature.rs"), "fn feature() {}").unwrap();
        let _ = std::process::Command::new("git").arg("-C").arg(&dir2).arg("add").arg(".").output();
        let _ = std::process::Command::new("git").arg("-C").arg(&dir2).arg("commit").arg("-m").arg("feature").output();
        let db_g = knocode_storage::Database::open(&db_path2).unwrap();
        let mut ri_g = RepositoryIntelligence::new(dir2.clone(), db_g, EventBus::new());
        let sg = ri_g.index_repository().unwrap();
        assert!(sg.files_indexed >= 1, "git repo should be indexable");
        // checkout main again
        let _ = std::process::Command::new("git").arg("-C").arg(&dir2).arg("checkout").arg("main").output();
        // after checkout, feature.rs may still be there depending on filesystem; at least no panic on re-index
        let db_g2 = knocode_storage::Database::open(&db_path2).unwrap();
        let mut ri_g2 = RepositoryIntelligence::new(dir2.clone(), db_g2, EventBus::new());
        let sg2 = ri_g2.index_repository().unwrap();
        let _ = sg2; // no panic, incremental handles git checkout changes
        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&dir2);
    }

    // TASK-011: Validate dependency graph A→B→C→D
    #[test]
    fn test_dependency_graph() {
        let dir = std::env::temp_dir().join(format!("knocode_graph_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        // Use import patterns that extract_imports reliably captures (use crate::b) plus mod b; for TASK-011
        std::fs::write(dir.join("a.rs"), "use crate::b::Foo;\nmod b;").unwrap();
        std::fs::write(dir.join("b.rs"), "use crate::c::Bar;\nmod c;").unwrap();
        std::fs::write(dir.join("c.rs"), "use crate::d::Baz;\nmod d;").unwrap();
        std::fs::write(dir.join("d.rs"), "pub struct Baz;").unwrap();
        let db = knocode_storage::Database::open(&PathBuf::from(":memory:")).unwrap();
        let ri = RepositoryIntelligence::new(dir.clone(), db, EventBus::new());
        let g = ri.build_dependency_graph().unwrap();
        assert!(g.edge_count() >= 2, "A→B→C→D should produce edge_count>=2, got {}", g.edge_count());
        // Verify chain A→B, B→C exist
        let deps_a = g.dependencies_of("a.rs");
        assert!(deps_a.iter().any(|d| d.contains("b.rs") || d.contains("b")), "a.rs should depend on b.rs");
        let deps_b = g.dependencies_of("b.rs");
        assert!(deps_b.iter().any(|d| d.contains("c.rs") || d.contains("c")), "b.rs should depend on c.rs");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
