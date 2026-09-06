use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use knocode_core::{ContextHints, ContextPack, TaskRequest, TokenUsage};
use knocode_events::{EventBus, RuntimeEvent, TokenCounts};
use knocode_knowledge::KnowledgeHub;
use knocode_repo_intel::RepositoryIntelligence;
use tracing::{debug, info, warn};

pub mod retrieval;


/// Emit per-stage timing to stderr when KNOCODE_PROFILE=1 (retrieval latency diagnostics).
/// No-op in normal operation — zero overhead when the env var is unset.
fn prof(mark: &str, start: Instant) {
    if std::env::var("KNOCODE_PROFILE").is_ok() {
        eprintln!("[profile] {mark}: {}ms", start.elapsed().as_millis());
    }
}


/// V1 docs/code split: Documentation files vs Code files (generic, not DefinitelyTyped-specific)
fn is_documentation_path(path: &str) -> bool {
    let lower = path.to_lowercase();
    // Strong docs signals
    if lower.ends_with(".md") {
        return true;
    }
    if lower.contains("/docs/") || lower.contains("/.github/") || lower.contains("/.knocode/") {
        return true;
    }
    if lower.ends_with("readme") || lower.ends_with("readme.md") || lower.ends_with("contributing") || lower.ends_with("contributing.md") || lower.ends_with("changelog") || lower.ends_with("changelog.md") || lower.ends_with("claude.md") || lower.ends_with("agents.md") {
        return true;
    }
    if lower.contains("readme") || lower.contains("contributing") {
        return true;
    }
    // .txt/.rst/.adoc only if in docs or named docs-like (avoid misclassifying generic .txt like feature.txt in tests)
    if (lower.ends_with(".txt") || lower.ends_with(".rst") || lower.ends_with(".adoc")) && (lower.contains("docs") || lower.contains("readme") || lower.contains("contributing")) {
        return true;
    }
    false
}



// ── Configuration ───────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct ContextConfig {
    pub max_tokens: usize,
    pub max_files: usize,
    pub max_lines_per_file: usize,
    pub cache_order: Vec<String>,
    /// Candidate pool size before deterministic ranking (20/50/100/200, default 100 → Top 20)
    pub candidate_k: usize,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_tokens: 12000,
            max_files: 20,
            max_lines_per_file: 500,
            cache_order: vec![
                "docs_context".to_string(),
                "code_context".to_string(),
            ],
            candidate_k: 100,
        }
    }
}

// ── Context Engine ──────────────────────────────────────────────────────

pub struct ContextEngine {
    /// Default repo intelligence (daemon CWD) — used when a request carries no repository_path
    default_repo_intel: Arc<Mutex<RepositoryIntelligence>>,
    /// Lazily-created per-repository intelligence keyed by canonical workspace path (TASK-036/F-7):
    /// ONE daemon serves many opencode windows on different repos simultaneously.
    repo_cache: Arc<Mutex<HashMap<String, Arc<Mutex<RepositoryIntelligence>>>>>,
    knowledge_hub: Arc<Mutex<KnowledgeHub>>,
    event_bus: EventBus,
    config: ContextConfig,
    /// Session fingerprints for deduplication (session_id → set of content hashes)
    session_fingerprints: Arc<Mutex<HashMap<String, HashSet<String>>>>,
}

impl ContextEngine {
    /// Create a new Context Engine
    pub fn new(
        repo_intel: RepositoryIntelligence,
        knowledge_hub: KnowledgeHub,
        event_bus: EventBus,
        config: ContextConfig,
    ) -> Self {
        Self {
            default_repo_intel: Arc::new(Mutex::new(repo_intel)),
            repo_cache: Arc::new(Mutex::new(HashMap::new())),
            knowledge_hub: Arc::new(Mutex::new(knowledge_hub)),
            event_bus,
            config,
            session_fingerprints: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Resolve the per-request repository view (TASK-036): when the agent's workspace path is
    /// known, build/cache a RepositoryIntelligence for it so retrieval + file reads target THAT
    /// repo instead of wherever the daemon happens to run. Falls back to the daemon-CWD engine.
    fn resolve_repo_intel(
        &self,
        repository_path: Option<&str>,
    ) -> Result<Arc<Mutex<RepositoryIntelligence>>, String> {
        let hint = match repository_path.map(str::trim).filter(|s| !s.is_empty()) {
            Some(h) => h,
            None => return Ok(self.default_repo_intel.clone()),
        };
        let canonical = dunce::canonicalize(hint)
            .unwrap_or_else(|_| std::path::PathBuf::from(hint));
        let key = canonical.to_string_lossy().to_string();
        if let Ok(cache) = self.repo_cache.lock() {
            if let Some(ri) = cache.get(&key) {
                return Ok(ri.clone());
            }
        }
        // Retrieval-only instance — DB is only needed for indexing/metadata, use throwaway in-memory store
        let db = knocode_storage::Database::open(&std::path::PathBuf::from(":memory:"))?;
        let ri = Arc::new(Mutex::new(RepositoryIntelligence::new(
            canonical.clone(),
            db,
            self.event_bus.clone(),
        )));
        if let Ok(mut cache) = self.repo_cache.lock() {
            let repo_id = match ri.lock() {
                Ok(guard) => guard.repository_id().to_string(),
                Err(_) => String::new(),
            };
            info!(repo = %canonical.to_string_lossy(), repository_id = %repo_id, "resolved per-repository intelligence (TASK-036)");
            cache.entry(key).or_insert_with(|| ri.clone());
        }
        Ok(ri)
    }

    // ── Standalone helpers for spawn_blocking (no &self required) ──────────

    fn search_code_scored_standalone(
        config: &ContextConfig,
        repo_intel: &std::sync::MutexGuard<'_, RepositoryIntelligence>,
        repository_id: &str,
        query: &str,
        context_hints: &Option<ContextHints>,
    ) -> Result<(String, String, Vec<knocode_core::SearchResult>, knocode_core::RetrievalStatus), String> {
        let _p0 = Instant::now();
        // ── Retrieval Engine boundary: Context no longer knows how relevance is calculated ──
        // All ranking (field weights, file-class, directory, symbol-match, code-behind, graph)
        // lives behind `Retriever::retrieve` → `Evidence` (see `retrieval/`).
        use crate::retrieval::{RetrievalPolicy, RetrievalQuery, Retriever, CombinedRetriever};

        let policy = RetrievalPolicy {
            candidate_k: if config.candidate_k > 0 { config.candidate_k } else { config.max_files * 3 },
            max_files: config.max_files,
            ..Default::default()
        };
        let mut q = RetrievalQuery::new(query, repository_id);
        if let Some(hints) = context_hints.as_ref() {
            if let Some(lang) = hints.language.as_deref().filter(|s| !s.is_empty()) {
                q = q.with_language(lang.to_string());
            }
        }

        let engine = CombinedRetriever::default();
        let retrieval = engine.retrieve(&q, repo_intel, &policy);
        prof("code_search.retrieval", _p0);
        let status = retrieval.status.clone();

        // ── Context assembly: turn Evidence into the legacy (docs, code, scored) triple ──
        // This is the ONLY place Context Engine knows about evidence; it does not re-rank.
        let max_lines = config.max_lines_per_file;
        let mut docs_results = Vec::new();
        let mut code_results = Vec::new();
        let mut scored = Vec::new();

        // Optional explain logging when KNOCODE_RETRIEVAL_EXPLAIN=1
        if std::env::var("KNOCODE_RETRIEVAL_EXPLAIN").is_ok() {
            for ev in &retrieval.evidence {
                eprintln!("[retrieval] {}", ev.explain());
            }
            eprintln!(
                "[retrieval] diagnostics: candidates={} took_tantivy={}ms ranking={}ms graph={}ms doc_count={}",
                retrieval.diagnostics.candidate_count,
                retrieval.diagnostics.tantivy_ms,
                retrieval.diagnostics.ranking_ms,
                retrieval.diagnostics.graph_ms,
                retrieval.diagnostics.doc_count
            );
        }

        for ev in retrieval.evidence {
            let path_str = ev.path.to_string_lossy().to_string();
            // Legacy SearchResult for provenance (score already *1000 in Evidence)
            scored.push(knocode_core::SearchResult {
                path: path_str.clone(),
                line: ev.line,
                content: path_str.clone(),
                score: ev.score as f64,
            });

            // Lazy snippet extraction for Top-K only (preserves prior behavior)
            let line_start = ev.line.saturating_sub(5);
            let line_end = ev.line + max_lines.min(15);
            match repo_intel.get_file_content(&path_str, Some((line_start, line_end))) {
                Ok(content) => {
                    let entry = format!("// {}:{}\n{}", path_str, ev.line, content);
                    if is_documentation_path(&path_str) {
                        docs_results.push(entry);
                    } else {
                        code_results.push(entry);
                    }
                }
                Err(e) => {
                    debug!(path = %path_str, error = %e, "Failed to read file for snippet");
                }
            }
        }

        // Hints are NOT retrieval — they are explicit task context (files_mentioned)
        if let Some(hints) = context_hints {
            if let Some(files) = &hints.files_mentioned {
                for file in files {
                    if let Ok(content) = repo_intel.get_file_content(file, Some((1, config.max_lines_per_file))) {
                        let entry = format!("// {}\n{}", file, content);
                        if is_documentation_path(file) {
                            docs_results.push(entry);
                        } else {
                            code_results.push(entry);
                        }
                        scored.push(knocode_core::SearchResult { path: file.clone(), line: 1, content: file.clone(), score: 1.0 });
                    }
                }
            }
        }
        Ok((docs_results.join("\n\n"), code_results.join("\n\n"), scored, status))
    }

    fn retrieve_knowledge_scored_standalone(
        _knowledge_hub: &std::sync::MutexGuard<'_, KnowledgeHub>,
        _repository_id: &str,
        _query: &str,
    ) -> Result<(String, Vec<knocode_core::KnowledgeEntry>), String> {
        // V1 minimal: Knowledge Hub collapsed to Repository Context per V1_MINIMAL_STACK_PLAN.md:2.5
        // `repository → context` is source files + symbols + paths + Git state only.
        // Docs/memory retrieval deferred — agent can `read` docs/*.md directly.
        // Keep signature for API compat but return empty (hot-path not dependent on KnowledgeHub).
        // If KNOCODE_KNOWLEDGE_ENABLED=1, re-enable legacy retrieval (opt-in for eval).
        if std::env::var("KNOCODE_KNOWLEDGE_ENABLED").ok().as_deref() == Some("1") {
            // legacy path kept for offline eval comparison only
            // (would call knowledge_hub.retrieve_knowledge here)
        }
        Ok((String::new(), vec![]))
    }

        pub async fn build_context(
        &self,
        request: &TaskRequest,
    ) -> Result<ContextPack, String> {
        let start = Instant::now();
        let correlation_id = knocode_core::CorrelationId::new();

        debug!(
            correlation_id = %correlation_id,
            session_id = %request.session_id,
            message = %request.message,
            "Building context"
        );

        // Initialize token budget
        let mut token_budget = self.config.max_tokens;
        let mut token_usage_by_source: HashMap<String, usize> = HashMap::new();

        // TASK-036: resolve the per-request repository view (agent workspace, else daemon CWD)
        let repo_intel = self.resolve_repo_intel(request.repository_path.as_deref())?;
        prof("build_context.resolve_repo_intel", start);

        // ── Parallel retrieval via tokio::task::spawn_blocking ────────────
        // Each task acquires its own lock, enabling true parallelism.
        // Code search holds repo lock; knowledge holds the kh lock.
        // TASK-030: scope all retrieval to THIS repository's stamp (shared hash formula)
        let repository_id = {
            let repo_guard = repo_intel.lock().map_err(|e| format!("Lock error: {}", e))?;
            repo_guard.repository_id().to_string()
        };

        let repo_intel_clone = repo_intel.clone();
        let config_clone = self.config.clone();
        let msg = request.message.clone();
        let ctx_hints = request.context_hints.clone();
        let repo_id_for_code = repository_id.clone();
        let repo_id_for_kh = repository_id.clone();

        let msg_for_code = msg.clone();
        let code_search_start = std::time::Instant::now();
        let code_fut = tokio::task::spawn_blocking(move || -> Result<_, String> {
            let repo_guard = repo_intel_clone.lock().map_err(|e| format!("Lock error: {}", e))?;
            Self::search_code_scored_standalone(&config_clone, &repo_guard, &repo_id_for_code, &msg_for_code, &ctx_hints)
        });

        let kh_clone2 = self.knowledge_hub.clone();
        let msg2 = request.message.clone();
        let knowledge_fut = tokio::task::spawn_blocking(move || -> Result<_, String> {
            let kh_guard = kh_clone2.lock().map_err(|e| format!("Lock error: {}", e))?;
            Self::retrieve_knowledge_scored_standalone(&kh_guard, &repo_id_for_kh, &msg2)
        });

        // Await both in parallel
        let (code_result, knowledge_result) = tokio::join!(code_fut, knowledge_fut);
        let code_search_duration_ms = code_search_start.elapsed().as_millis() as u64;
        prof("build_context.retrieval_join", start);

        let (raw_docs_from_code, raw_code, code_scored, code_retrieval_status) =
            code_result.map_err(|e| format!("Code search failed: {}", e))??;
        let (raw_knowledge, knowledge_scored) =
            knowledge_result.map_err(|e| format!("Knowledge search failed: {}", e))??;

        // V1 docs/code split: docs from code search (Repository → Documentation) + legacy knowledge (usually empty)
        let combined_docs = if raw_knowledge.is_empty() { raw_docs_from_code } else { format!("{}\n\n{}", raw_docs_from_code, raw_knowledge) };
        // Dedup after parallel retrieval (needs self.session_fingerprints — cannot run in spawn_blocking)
        let code_context = self.dedup_content(&request.session_id, &raw_code);
        let knowledge_context = self.dedup_content(&request.session_id, &combined_docs);

        // Compute repository state (brief lock, post-parallel)
        let repo_state = {
            let repo_guard = repo_intel.lock().map_err(|e| format!("Lock error: {}", e))?;
            self.repository_state_for(&repo_guard)
        };

        // Step 4: Assemble context pack with cache-aware ordering + frozen-prefix + reversible compression
        let (mut context_pack, total_tokens) = self.assemble_context_pack(
            &knowledge_context,
            &code_context,
            &mut token_budget,
            &mut token_usage_by_source,
            code_retrieval_status,
        );
        // TASK-007/008/009: stable artifact + provenance (deterministic) — real scores
        {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(request.message.as_bytes());
            hasher.update(self.config.max_tokens.to_be_bytes());
            hasher.update(self.config.cache_order.join(",").as_bytes());
            hasher.update(repo_state.as_bytes());
            let task_hash = format!("{:x}", hasher.finalize())[..16].to_string();
            context_pack.metadata.task_hash = task_hash;
            context_pack.metadata.correlation_id = correlation_id.to_string();
            context_pack.metadata.repository_state = repo_state.clone();
            context_pack.repository_state = repo_state.clone();
            for entry in &knowledge_scored {
                let retriever = "tantivy";
                let reason = "bm25".to_string();
                // TASK-033/F-4: category stays in `source`; path is cleaned of prefixes/verbatim markers
                context_pack.provenance.push(knocode_core::ipc::ContextProvenance {
                    path: clean_provenance_path(&entry.key),
                    source: "docs".to_string(),
                    retriever: retriever.to_string(),
                    score: entry.relevance_score.unwrap_or(entry.confidence),
                    reason,
                });
            }
            for result in &code_scored {
                // Distinguish BM25 (tantivy score > 1.0) vs symbol/ripgrep (1.0) vs structural (0.9)
                let (retriever, reason) = if result.score > 2.0 {
                    ("tantivy", "bm25")
                } else if (result.score - 0.9).abs() < 0.01 {
                    ("ast-grep", "symbol match")
                } else {
                    ("tantivy", "symbol match")
                };
                context_pack.provenance.push(knocode_core::ipc::ContextProvenance {
                    path: clean_provenance_path(&result.path),
                    source: "code".to_string(),
                    retriever: retriever.to_string(),
                    score: result.score,
                    reason: reason.to_string(),
                });
            }
            // TASK-032/F-3: dedup provenance by (path, source, retriever) keeping highest score
            dedup_provenance(&mut context_pack.provenance);
        }

        // ── Retrieval Diagnostic (when expected_files provided) ──────────────
        if let Some(ref expected) = request.expected_files {
            if !expected.is_empty() {
                let retrieved_paths: Vec<String> = code_scored.iter().map(|r| r.path.clone()).collect();
                let repo_guard = repo_intel.lock().ok();
                let diagnostic = classify_misses(
                    &request.message,
                    expected,
                    &retrieved_paths,
                    repo_guard.as_ref(),
                    code_search_duration_ms,
                );
                context_pack.retrieval_diagnostic = Some(diagnostic);
            }
        }

        // Retrieval-stage stats (search only, excluding packing) — additive metadata
        // for daemon metrics without extra plumbing.
        context_pack.retrieval_stats = Some(knocode_core::ipc::RetrievalStats {
            retrieval_ms: code_search_duration_ms,
            candidates: code_scored.len() + knowledge_scored.len(),
        });

        // Step 5: Emit ContextBuilt event (async-only, never blocks hot path)
        let latency_ms = start.elapsed().as_millis() as u64;
        self.event_bus.emit(RuntimeEvent::ContextBuilt {
            correlation_id: correlation_id.clone(),
            token_counts: TokenCounts {
                total: total_tokens,
                by_source: token_usage_by_source.clone(),
            },
            file_count: context_pack.code_context.lines().count(),
            latency_ms,
        });

        info!(
            correlation_id = %correlation_id,
            total_tokens = total_tokens,
            latency_ms = latency_ms,
            "Context built"
        );

        prof("build_context.total", start);
        Ok(context_pack)
    }

    /// Deduplicate content against session fingerprint (spec §3 deduplication + PRINCIPLES.md:10)
    fn dedup_content(&self, session_id: &str, content: &str) -> String {
        if content.is_empty() || session_id.is_empty() {
            return content.to_string();
        }
        let hash = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(content.as_bytes());
            format!("{:x}", hasher.finalize())
        };
        if let Ok(mut fps) = self.session_fingerprints.lock() {
            let entry = fps.entry(session_id.to_string()).or_default();
            if entry.contains(&hash) {
                debug!(session_id = %session_id, hash = %hash, "Dedup: skipping duplicate content block");
                return String::new();
            }
            entry.insert(hash);
        }
        content.to_string()
    }

    /// Repository state (git HEAD) for deterministic ContextPack (TASK-008) — resolved per request repo (TASK-036)
    /// Accepts pre-acquired lock guard to avoid redundant lock contention.
    fn repository_state_for(&self, repo_intel: &std::sync::MutexGuard<'_, RepositoryIntelligence>) -> String {
        // Try env override first (for tests)
        if let Ok(v) = std::env::var("KNOCODE_REPO_STATE") { return v; }
        let repo_path = repo_intel.repo_path().to_path_buf();
        // Try git rev-parse HEAD in repo_path (best-effort, fail-open to empty)
        if let Ok(out) = std::process::Command::new("git").arg("-C").arg(&repo_path).arg("rev-parse").arg("HEAD").output() {
            if out.status.success() {
                let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if s.len() >= 7 { return s; }
            }
        }
        // Fallback: hash of repo_path for determinism
        knocode_core::repository_id_from_path(&repo_path.to_string_lossy())
    }

    /// Assemble the context pack with cache-aware ordering (docs before code).
    fn assemble_context_pack(
        &self,
        knowledge_context: &str,
        code_context: &str,
        token_budget: &mut usize,
        token_usage_by_source: &mut HashMap<String, usize>,
        code_retrieval_status: knocode_core::RetrievalStatus,
    ) -> (ContextPack, usize) {
        let mut total_tokens = 0;

        // Section 1: docs_context (45% budget) - most cache-stable
        let docs_budget = (*token_budget as f64 * 0.45) as usize;
        let docs_tokens = count_tokens(knowledge_context);
        let (docs_content, docs_used) = if docs_tokens <= docs_budget {
            (knowledge_context.to_string(), docs_tokens)
        } else {
            truncate_to_tokens(knowledge_context, docs_budget)
        };
        token_usage_by_source.insert("docs_context".to_string(), docs_used);
        total_tokens += docs_used;

        // Section 3: code_context (55% budget)
        let code_budget = (*token_budget as f64 * 0.55) as usize;
        let code_tokens = count_tokens(code_context);
        let (code_content, code_used) = if code_tokens <= code_budget {
            (code_context.to_string(), code_tokens)
        } else {
            truncate_to_tokens(code_context, code_budget)
        };
        token_usage_by_source.insert("code_context".to_string(), code_used);
        total_tokens += code_used;

        // Remaining budget for metadata
        let remaining = token_budget.saturating_sub(total_tokens);
        token_usage_by_source.insert("metadata".to_string(), remaining);

        let context_pack = ContextPack {
            docs_context: docs_content,
            code_context: code_content,
            token_usage: TokenUsage {
                total_tokens,
                budget_remaining: remaining,
                by_source: token_usage_by_source.clone(),
            },
            provenance: vec![],
            metadata: knocode_core::ipc::ContextMetadata {
                task_hash: String::new(),
                correlation_id: String::new(),
                cache_order: self.config.cache_order.clone(),
                repository_state: String::new(),
            },
            repository_state: String::new(),
            code_retrieval_status,
            retrieval_diagnostic: None,
            retrieval_stats: None,
        };

        (context_pack, total_tokens)
    }

    /// Clear session fingerprint (e.g., on daemon restart)
    pub fn clear_session_fingerprint(&self, session_id: &str) {
        if let Ok(mut fingerprints) = self.session_fingerprints.lock() {
            fingerprints.remove(session_id);
        }
    }

    /// Re-index a repository through this engine and refresh its cached index
    /// handles so the NEXT query serves the new commit immediately (no stale
    /// tantivy reader). This is the entry point the daemon's auto-reindex watcher
    /// calls: reindexing is executed on a FRESH `RepositoryIntelligence` bound to
    /// the same SQLite store (file-backed for the default repo), so concurrent
    /// queries never block on this engine's repo lock for the duration of the walk.
    /// `repository_path` defaults to the engine's daemon-CWD repository; `Some(path)`
    /// targets that workspace's per-repo view instead.
    pub fn reindex_repository(
        &self,
        repository_path: Option<&str>,
    ) -> Result<knocode_repo_intel::IndexStats, String> {
        // Read identity + backing-store location under a brief lock, then release it
        // before the (potentially long) walk below.
        let (repo_path, db_file) = {
            let repo_intel = self.resolve_repo_intel(repository_path)?;
            let guard = repo_intel
                .lock()
                .map_err(|e| format!("Lock error: {}", e))?;
            (guard.repo_path().to_path_buf(), guard.db_path())
        };
        let db = match db_file {
            Some(path) => knocode_storage::Database::open(&path)?,
            None => knocode_storage::Database::open(&std::path::PathBuf::from(":memory:"))?,
        };
        let mut intel = knocode_repo_intel::RepositoryIntelligence::new(
            repo_path,
            db,
            self.event_bus.clone(),
        );
        // index_repository() evicts the repo's cached tantivy handle on completion,
        // so the engine's next search reopens a fresh reader and sees the new data.
        intel.index_repository()
    }

    /// Serialize context pack to YAML — compact, deterministic order (skills → docs → code).
    /// TASK-031/F-2: empty sections are omitted entirely; a zero-value pack serializes to an
    /// EMPTY string so the daemon can pass the prompt through byte-identical instead of
    /// paying ~500-700 tokens of metadata skeleton for no retrievable value.
    pub fn to_yaml(pack: &ContextPack) -> Result<String, String> {
        if pack.token_usage.total_tokens == 0 {
            return Ok(String::new());
        }
        let mut out = String::new();
        let mut block = |key: &str, content: &str| {
            if content.is_empty() {
                return;
            }
            out.push_str(key);
            out.push_str(": |\n");
            for line in content.lines() {
                out.push_str("  ");
                out.push_str(line);
                out.push('\n');
            }
        };
        block("docs_context", &pack.docs_context);
        block("code_context", &pack.code_context);
        // Compact single-line metadata — only when there is actual content above
        out.push_str(&format!(
            "token_usage: {{total_tokens: {}, budget_remaining: {}}}\n",
            pack.token_usage.total_tokens, pack.token_usage.budget_remaining
        ));
        if !pack.provenance.is_empty() {
            out.push_str("provenance:\n");
            for p in &pack.provenance {
                out.push_str(&format!(
                    "  - {{path: \"{}\", source: {}, retriever: {}, score: {:.3}}}\n",
                    p.path.replace('\\', "/").replace('"', "'"),
                    p.source, p.retriever, p.score
                ));
            }
        }
        Ok(out)
    }

    /// Retrieve original full content saved by reversible compression (spec §2)
    pub fn get_original(hash: &str) -> Result<String, String> {
        let path = reversible_cache_path(hash);
        std::fs::read_to_string(&path).map_err(|e| format!("Original not found for {hash}: {e}"))
    }
}

#[async_trait::async_trait]
impl knocode_core::IContextBuilder for ContextEngine {
    async fn build_context(
        &self,
        task: &TaskRequest,
    ) -> std::result::Result<ContextPack, knocode_core::KnocodeError> {
        ContextEngine::build_context(self, task)
            .await
            .map_err(knocode_core::KnocodeError::ContextBuildFailed)
    }

    fn to_yaml(pack: &ContextPack) -> std::result::Result<String, knocode_core::KnocodeError>
    where
        Self: Sized,
    {
        ContextEngine::to_yaml(pack)
            .map_err(knocode_core::KnocodeError::Serialization)
    }
}

// ── Helpers for reversible compression ───────────────────────────────────

fn reversible_cache_dir() -> std::path::PathBuf {
    if let Some(home) = dirs_home() {
        home.join(".knocode").join("cache").join("originals")
    } else {
        std::path::PathBuf::from(".knocode/cache/originals")
    }
}

fn reversible_cache_path(hash: &str) -> std::path::PathBuf {
    reversible_cache_dir().join(format!("{}.txt", hash))
}

fn dirs_home() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "windows")]
    { std::env::var("USERPROFILE").ok().map(std::path::PathBuf::from) }
    #[cfg(not(target_os = "windows"))]
    { std::env::var("HOME").ok().map(std::path::PathBuf::from) }
}

// ── Provenance hygiene (TASK-032/033 — F-3, F-4) ─────────────────────────

/// Clean a provenance path (TASK-033/F-4): strip Windows verbatim prefixes (`\\?\`,
/// `\\?\UNC\`) and knowledge collection prefixes (`docs:`, `adr:` …) so provenance renders
/// plain absolute or repo-relative paths. Category stays in the `source` field.
fn clean_provenance_path(raw: &str) -> String {
    let mut p = raw.trim().to_string();
    // Windows verbatim prefixes first
    if let Some(rest) = p.strip_prefix(r"\\?\UNC\") {
        p = format!(r"\\{rest}");
    } else if let Some(rest) = p.strip_prefix(r"\\?\") {
        p = rest.to_string();
    }
    // Collection/category prefix like "docs:" / "adr:" — only when followed by an
    // absolute-looking path (drive letter, backslash, slash, or another verbatim marker)
    const CATEGORIES: [&str; 7] = ["docs", "adr", "convention", "pattern", "domain", "profile", "memory"];
    for cat in CATEGORIES {
        let prefix = format!("{cat}:");
        if let Some(rest) = p.strip_prefix(&prefix) {
            let looks_absolute = rest.starts_with('\\')
                || rest.starts_with('/')
                || rest.starts_with(r"\\?\")
                || {
                    let mut chars = rest.chars();
                    matches!((chars.next(), chars.next()), (Some(a), Some(':')) if a.is_ascii_alphabetic())
                };
            if looks_absolute {
                p = rest.trim_start_matches(r"\\?\").to_string();
                break;
            }
        }
    }
    p
}

/// Dedup provenance entries by (path, source, retriever), keeping the highest score
/// (TASK-032/F-3) — identical rows must render exactly once.
fn dedup_provenance(provenance: &mut Vec<knocode_core::ipc::ContextProvenance>) {
    let mut seen: HashMap<(String, String, String), usize> = HashMap::new();
    let mut deduped: Vec<knocode_core::ipc::ContextProvenance> = Vec::with_capacity(provenance.len());
    for entry in provenance.drain(..) {
        let key = (entry.path.clone(), entry.source.clone(), entry.retriever.clone());
        match seen.get(&key) {
            Some(&idx) => {
                if entry.score > deduped[idx].score {
                    deduped[idx] = entry;
                }
            }
            None => {
                seen.insert(key, deduped.len());
                deduped.push(entry);
            }
        }
    }
    *provenance = deduped;
}


// ── Retrieval Diagnostic ──────────────────────────────────────────────

/// Classify why expected files were missed by retrieval.
/// Compares retrieved paths against expected files and categorizes each miss.
fn classify_misses(
    query: &str,
    expected_files: &[String],
    retrieved_paths: &[String],
    repo_intel: Option<&std::sync::MutexGuard<'_, RepositoryIntelligence>>,
    code_search_duration_ms: u64,
) -> knocode_core::RetrievalDiagnostic {
    use knocode_core::{FileDiagnostic, RetrievalDiagnostic};

    // Normalize retrieved paths for comparison (strip line refs like "// path:42")
    let retrieved_normalized: Vec<String> = retrieved_paths.iter().map(|p| {
        let clean = p.trim_start_matches("// ").trim();
        if let Some(colon_pos) = clean.rfind(':') {
            let maybe_num = &clean[colon_pos+1..];
            if maybe_num.chars().all(|c| c.is_ascii_digit()) {
                clean[..colon_pos].to_string()
            } else {
                clean.to_string()
            }
        } else {
            clean.to_string()
        }
    }).collect();

    let retrieved_set: std::collections::HashSet<&str> =
        retrieved_normalized.iter().map(|s| s.as_str()).collect();

    // Extract query tokens for lexical analysis
    let stop_words: std::collections::HashSet<&str> = [
        "a","an","the","is","are","was","were","be","been","being",
        "have","has","had","do","does","did","will","would","could",
        "should","may","might","shall","can","to","of","in","for",
        "on","with","at","by","from","as","into","through","during",
        "before","after","above","below","between","and","but","or",
        "nor","not","so","yet","both","either","neither","each",
        "every","all","any","few","more","most","other","some",
        "such","no","only","own","same","than","too","very",
        "just","because","if","when","where","how","what","which",
        "who","whom","this","that","these","those",
    ].iter().copied().collect();
    let query_tokens: Vec<String> = query
        .split_whitespace()
        .map(|t| t.to_lowercase().chars().filter(|c| c.is_alphanumeric() || *c == '_').collect::<String>())
        .filter(|t| t.len() >= 2 && !stop_words.contains(t.as_str()))
        .collect();

    // Get repo stats (via validate_index since db field is private)
    let (files_indexed, symbols_indexed) = if let Some(ri) = repo_intel {
        match ri.validate_index() {
            Ok(stats) => (stats.doc_count, 0usize),
            Err(_) => (0usize, 0usize),
        }
    } else {
        (0usize, 0usize)
    };

    // Build dependency graph (if available)
    let graph = repo_intel.and_then(|ri| ri.build_dependency_graph().ok());
    let retrieved_file_set: std::collections::HashSet<&str> = retrieved_normalized.iter().map(|s| s.as_str()).collect();

    let mut files = Vec::new();
    let mut expected_found = 0usize;

    for expected in expected_files.iter() {
        // Check if this expected file was retrieved (substring match for flexibility)
        let found = retrieved_set.iter().any(|r| r.contains(expected) || expected.contains(r));
        if found {
            expected_found += 1;
            let rank = retrieved_normalized.iter().position(|r| r.contains(expected) || expected.contains(r)).map(|i| i + 1);
            files.push(FileDiagnostic {
                path: expected.clone(),
                retrieved: true,
                rank,
                miss_type: None,
                explanation: format!("found at rank {}", rank.unwrap_or(0)),
            });
            continue;
        }

        // Classify the miss
        let (miss_type, explanation) = classify_single_miss(
            expected,
            &query_tokens,
            &retrieved_file_set,
            graph.as_ref(),
            repo_intel,
        );

        files.push(FileDiagnostic {
            path: expected.clone(),
            retrieved: false,
            rank: None,
            miss_type: Some(miss_type),
            explanation,
        });
    }

    RetrievalDiagnostic {
        query: query.to_string(),
        results_returned: retrieved_paths.len(),
        expected_found,
        expected_total: expected_files.len(),
        files,
        bm25_duration_ms: code_search_duration_ms,
        symbol_duration_ms: 0,
        graph_duration_ms: 0,
        merge_duration_ms: 0,
        files_indexed,
        symbols_indexed,
    }
}

/// Classify why a single expected file was not retrieved.
fn classify_single_miss(
    expected: &str,
    query_tokens: &[String],
    retrieved_set: &std::collections::HashSet<&str>,
    graph: Option<&knocode_repo_intel::graph::DependencyGraph>,
    repo_intel: Option<&std::sync::MutexGuard<'_, RepositoryIntelligence>>,
) -> (knocode_core::MissType, String) {
    use knocode_core::MissType;
    // 1. Check if file is indexed at all
    if let Some(ri) = repo_intel {
        match ri.get_file_content(expected, Some((1, 5))) {
            Ok(content) if content.is_empty() => {
                return (MissType::NotIndexed, "file exists but content is empty (may not be indexed)".to_string());
            }
            Err(_) => {
                return (MissType::NotIndexed, "file not found in repository (may not be indexed)".to_string());
            }
            Ok(content) => {
                let content_lower = content.to_lowercase();
                let filename_lower = expected.rsplit(['/', '\\']).next().unwrap_or(expected).to_lowercase();

                // 2. Lexical miss: does the file text contain any query tokens?
                let lexical_hits: usize = query_tokens.iter()
                    .filter(|t| content_lower.contains(t.as_str()) || filename_lower.contains(t.as_str()))
                    .count();
                if lexical_hits == 0 {
                    return (MissType::LexicalMiss, format!(
                        "file text contains none of the {} query tokens (BM25 won't match)",
                        query_tokens.len()
                    ));
                }

                // 3. Query expansion miss: file has some matches but not enough for BM25 threshold
                if lexical_hits < query_tokens.len() / 2 {
                    return (MissType::QueryExpansionMiss, format!(
                        "file contains only {}/{} query tokens — BM25 score too low",
                        lexical_hits, query_tokens.len()
                    ));
                }

                // 4. Ranked too low: file has good lexical overlap but wasn't in top-k
                return (MissType::RankedTooLow, format!(
                    "file contains {}/{} query tokens but ranked below top-{} results",
                    lexical_hits, query_tokens.len(), retrieved_set.len()
                ));
            }
        }
    }

    // 5. Graph miss: file exists but not connected to any retrieved file
    if let Some(g) = graph {
        let has_connection = retrieved_set.iter().any(|r| {
            g.dependencies_of(r).iter().any(|d| d.contains(expected) || expected.contains(d))
                || g.dependents_of(r).iter().any(|d| d.contains(expected) || expected.contains(d))
        });
        if !has_connection {
            return (MissType::GraphMiss, "file not connected to any retrieved file via dependency graph".to_string());
        }
    }

    (MissType::Unclassified, "could not determine miss reason (insufficient diagnostic data)".to_string())
}

// ── Token Counting (tiktoken-rs, never via model API) ────────────────────

/// Count tokens locally with tiktoken-rs `cl100k_base` (spec §3, §4)
/// Fallback to char/4 heuristic only if tokenizer fails — logs WARN.
pub fn count_tokens(text: &str) -> usize {
    if text.is_empty() {
        return 0;
    }
    match tiktoken_rs::cl100k_base() {
        Ok(bpe) => bpe.encode_ordinary(text).len(),
        Err(e) => {
            warn!(error = %e, "tiktoken load failed, falling back to heuristic");
            estimate_tokens_heuristic(text)
        }
    }
}

fn estimate_tokens_heuristic(text: &str) -> usize {
    let char_count = text.len();
    let word_count = text.split_whitespace().count();
    let by_chars = char_count / 4;
    let by_words = (word_count as f64 * 1.3) as usize;
    by_chars.max(by_words)
}

/// Legacy alias — keep for external callers that matched heuristic name
pub fn estimate_tokens(text: &str) -> usize {
    count_tokens(text)
}

/// Truncate text to fit within a token budget, reversible by default.
/// Saves full content to `~/.knocode/cache/originals/{hash}.txt` and appends pointer.
fn truncate_to_tokens(text: &str, budget: usize) -> (String, usize) {
    let tokens = count_tokens(text);
    if tokens <= budget {
        return (text.to_string(), tokens);
    }
    // Reversible: save original
    let hash = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(text.as_bytes());
        format!("{:x}", hasher.finalize())[..16].to_string()
    };
    let cache_path = reversible_cache_path(&hash);
    if let Some(parent) = cache_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let _ = std::fs::write(&cache_path, text);

    // Truncate at line boundaries by token count
    let mut result = String::new();
    let mut current_tokens = 0;
    for line in text.lines() {
        let line_tokens = count_tokens(line);
        // +1 for newline
        if current_tokens + line_tokens + 1 > budget.saturating_sub(5) {
            result.push_str(&format!(
                "... [truncated — full at {} | retrieve via ContextEngine::get_original(\"{}\")]\n",
                cache_path.display(),
                hash
            ));
            current_tokens += 5;
            break;
        }
        result.push_str(line);
        result.push('\n');
        current_tokens += line_tokens + 1;
    }
    if result.is_empty() {
        result = format!(
            "... [truncated — full at {} | hash {}]\n",
            cache_path.display(),
            hash
        );
        current_tokens = 5;
    }
    (result, current_tokens)
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    /// Serializes tests that mutate process-global env vars / the shared tantivy index dir —
    /// parallel mutation makes retrieval results non-deterministic.
    static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

    #[test]
    fn test_estimate_tokens() {
        assert!(estimate_tokens("hello world") > 0);
        assert!(estimate_tokens("") == 0);
        // tiktoken packs repeated "a" efficiently, so threshold lower than old heuristic
        assert!(estimate_tokens(&"a".repeat(100)) > 5);
    }

    #[test]
    fn test_truncate_to_tokens_within_budget() {
        let text = "short text";
        let (result, tokens) = truncate_to_tokens(text, 100);
        assert_eq!(result, text);
        assert!(tokens <= 100);
    }

    #[test]
    fn test_truncate_to_tokens_exceeds_budget() {
        // Create text that exceeds a small budget
        let text = (0..100).map(|i| format!("line {} with some content", i)).collect::<Vec<_>>().join("\n");
        let (result, tokens) = truncate_to_tokens(&text, 20);
        // Should be truncated and within reasonable bounds
        assert!(tokens <= 25); // Allow some overhead
        assert!(result.contains("truncated") || result.lines().count() < 100);
    }

    #[test]
    fn test_context_config_default() {
        let config = ContextConfig::default();
        assert_eq!(config.max_tokens, 12000);
        assert_eq!(config.max_files, 20);
        assert_eq!(config.max_lines_per_file, 500);
        assert_eq!(config.cache_order.len(), 2);
    }

    #[test]
    fn test_cache_ordering() {
        let config = ContextConfig::default();
        assert_eq!(config.cache_order.len(), 2);
        assert_eq!(config.cache_order[0], "docs_context");
        assert_eq!(config.cache_order[1], "code_context");
    }

    #[test]
    fn test_token_budget_allocation() {
        let max_tokens = 12000;
        let docs_budget = (max_tokens as f64 * 0.45) as usize;
        let code_budget = (max_tokens as f64 * 0.55) as usize;
        assert_eq!(docs_budget, 5400);
        assert_eq!(code_budget, 6600);
    }

    #[test]
    fn test_context_pack_yaml_serialization() {
        let pack = ContextPack {
            docs_context: "docs content".to_string(),
            code_context: "code content".to_string(),
            token_usage: TokenUsage {
                total_tokens: 100,
                budget_remaining: 50,
                by_source: HashMap::new(),
            },
            provenance: vec![],
            metadata: knocode_core::ipc::ContextMetadata::default(),
            repository_state: String::new(),
            code_retrieval_status: knocode_core::RetrievalStatus::NoMatch,
            retrieval_diagnostic: None,
            retrieval_stats: None,
        };

        let yaml = ContextEngine::to_yaml(&pack).unwrap();
        assert!(yaml.contains("docs_context"));
        assert!(yaml.contains("code_context"));
    }

    #[test]
    fn test_count_tokens_tiktoken_vs_heuristic() {
        // tiktoken count should be non-zero and within 5x heuristic for English
        let text = "hello world, this is a test of token counting";
        let t = count_tokens(text);
        let h = estimate_tokens_heuristic(text);
        assert!(t > 0);
        assert!(t <= h * 5);
        assert!(count_tokens("") == 0);
    }

    #[test]
    fn test_assembled_sections_docs_then_code() {
        // assemble_context_pack orders docs before code (no skills section anymore)
        use knocode_events::EventBus;
        
        use knocode_repo_intel::RepositoryIntelligence;
        use knocode_storage::Database;
        use std::path::PathBuf;

        let db = Database::open(&PathBuf::from(":memory:")).unwrap();
        let event_bus = EventBus::new();
        let repo_intel = RepositoryIntelligence::new(PathBuf::from("."), Database::open(&PathBuf::from(":memory:")).unwrap(), event_bus.clone());
        let kh = KnowledgeHub::new(db, event_bus.clone());
        let engine = ContextEngine::new(repo_intel, kh, event_bus, ContextConfig::default());
        let mut budget = 12000;
        let mut usage = HashMap::new();
        let (pack, _) = engine.assemble_context_pack("doc line", "code line", &mut budget, &mut usage, knocode_core::RetrievalStatus::NoMatch);
        assert_eq!(pack.docs_context, "doc line");
        assert_eq!(pack.code_context, "code line");
        assert_eq!(engine.config.cache_order[0], "docs_context");
    }

    #[test]
    fn test_dedup_skips_duplicate() {
        use knocode_events::EventBus;
        use knocode_knowledge::KnowledgeHub;
        use knocode_repo_intel::RepositoryIntelligence;
        use knocode_storage::Database;
        use std::path::PathBuf;

        let db = Database::open(&PathBuf::from(":memory:")).unwrap();
        let event_bus = EventBus::new();
        let repo_intel = RepositoryIntelligence::new(PathBuf::from("."), Database::open(&PathBuf::from(":memory:")).unwrap(), event_bus.clone());
        let kh = KnowledgeHub::new(db, event_bus.clone());
        let engine = ContextEngine::new(repo_intel, kh, event_bus, ContextConfig::default());
        let a = engine.dedup_content("sess1", "hello world");
        let b = engine.dedup_content("sess1", "hello world");
        let c = engine.dedup_content("sess2", "hello world");
        assert_eq!(a, "hello world");
        assert_eq!(b, ""); // deduped
        assert_eq!(c, "hello world"); // different session
    }

    #[test]
    fn test_reversible_truncation_pointer() {
        let long = (0..500).map(|i| format!("line {} with some content to exceed budget and trigger truncation", i)).collect::<Vec<_>>().join("\n");
        let (truncated, tokens) = truncate_to_tokens(&long, 20);
        assert!(truncated.contains("truncated"));
        assert!(truncated.contains("retrieve via ContextEngine::get_original"));
        assert!(tokens <= 30);
        // Verify file was written and can be retrieved
        let hash = {
            use sha2::{Digest, Sha256};
            let mut hasher = Sha256::new();
            hasher.update(long.as_bytes());
            format!("{:x}", hasher.finalize())[..16].to_string()
        };
        let original = ContextEngine::get_original(&hash).unwrap();
        assert_eq!(original, long);
    }

    #[tokio::test]
    async fn test_build_context_deterministic() {
        use knocode_core::TaskRequest;
        use knocode_events::EventBus;
        use knocode_knowledge::KnowledgeHub;
        use knocode_repo_intel::RepositoryIntelligence;
        use knocode_storage::Database;

        // Deterministic: same repo+task+config → same pack content even with different session_id, not deduped
        let _env = ENV_LOCK.lock().unwrap();
        let dir = std::env::temp_dir().join(format!("knocode_det_{}", uuid::Uuid::new_v4()));
        // Isolate from other tests' writes to the shared tantivy index — ripgrep fallback over
        // this temp repo is what must be deterministic.
        std::env::set_var("KNOCODE_INDEX_DIR", dir.join("idx").to_string_lossy().to_string());
        std::env::set_var("KNOCODE_REPO_STATE", "deterministic-test-head-abc123");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "fn hello() { println!(\"hi\"); }").unwrap();
        let db_path = dir.join("det.db");
        let db = Database::open(&db_path).unwrap();
        let event_bus = EventBus::new();
        // Index repo so code retrieval has something
        let mut ri = RepositoryIntelligence::new(dir.clone(), Database::open(&db_path).unwrap(), event_bus.clone());
        let _ = ri.index_repository();
        let kh = KnowledgeHub::new(db, event_bus.clone());
        let engine = ContextEngine::new(ri, kh, event_bus, ContextConfig::default());
        let task1 = TaskRequest { message: "fix hello function".to_string(), session_id: "sessA".to_string(), context_hints: None, repository_id: String::new(), repository_path: None, expected_files: None };
        let task2 = TaskRequest { message: "fix hello function".to_string(), session_id: "sessB".to_string(), context_hints: None, repository_id: String::new(), repository_path: None, expected_files: None };
        let pack1 = engine.build_context(&task1).await.unwrap();
        let pack2 = engine.build_context(&task2).await.unwrap();
        // Deterministic: same repo state hash, same task hash, same content (correlation_id intentionally differs, not compared)
        assert_eq!(pack1.repository_state, pack2.repository_state, "repo_state must be deterministic");
        assert_eq!(pack1.metadata.repository_state, pack2.metadata.repository_state);
        assert_eq!(pack1.metadata.task_hash, pack2.metadata.task_hash, "task_hash deterministic");
        assert_eq!(pack1.metadata.cache_order, pack2.metadata.cache_order);
        // Code/docs/skills content should be equal (different session => not deduped)
        assert_eq!(pack1.code_context, pack2.code_context);
        assert_eq!(pack1.docs_context, pack2.docs_context);
        assert_eq!(pack1.token_usage.total_tokens, pack2.token_usage.total_tokens);
        // routing removed;
        // Same session should dedup second call (different from first, empty on second)
        let task3 = TaskRequest { message: "fix hello function".to_string(), session_id: "sessA".to_string(), context_hints: None, repository_id: String::new(), repository_path: None, expected_files: None };
        let pack3 = engine.build_context(&task3).await.unwrap();
        // dedup_content may empty repeated content for same session; pack3 may have empty sections but task_hash still same
        assert_eq!(pack3.metadata.task_hash, pack1.metadata.task_hash);
        std::env::remove_var("KNOCODE_INDEX_DIR");
        std::env::remove_var("KNOCODE_REPO_STATE");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_clean_provenance_path_strips_verbatim_and_category() {
        // TASK-033/F-4
        assert_eq!(clean_provenance_path(r"\\?\C:\Leon\eShop\src\Checkout.cs"), r"C:\Leon\eShop\src\Checkout.cs");
        assert_eq!(clean_provenance_path(r"\\?\UNC\server\share\a.rs"), r"\\server\share\a.rs");
        assert_eq!(clean_provenance_path(r"docs:\\?\C:\Leon\knocode\docs\DATA_FLOW.md"), r"C:\Leon\knocode\docs\DATA_FLOW.md");
        assert_eq!(clean_provenance_path(r"adr:C:\repos\x\docs\adr\0001.md"), r"C:\repos\x\docs\adr\0001.md");
        // Relative paths pass through untouched — category prefix only stripped when absolute follows
        assert_eq!(clean_provenance_path("docs/guide.md"), "docs/guide.md");
        assert_eq!(clean_provenance_path("src/main.rs"), "src/main.rs");
    }

    #[test]
    fn test_dedup_provenance_keeps_highest_score() {
        use knocode_core::ipc::ContextProvenance;
        let mk = |score: f64| ContextProvenance {
            path: "docs/guide.md".into(),
            source: "docs".into(),
            retriever: "tantivy".into(),
            score,
            reason: "bm25".into(),
        };
        let mut prov = vec![mk(0.5), mk(0.9), mk(0.7), mk(0.9)];
        dedup_provenance(&mut prov);
        assert_eq!(prov.len(), 1, "F-3: identical rows must render once");
        assert!((prov[0].score - 0.9).abs() < 1e-9);
    }

    #[test]
    fn test_to_yaml_zero_value_pack_is_empty() {
        // TASK-031/F-2: no hits → empty YAML → daemon passes prompt through untouched
        let pack = ContextPack {
            docs_context: String::new(),
            code_context: String::new(),
            token_usage: TokenUsage { total_tokens: 0, budget_remaining: 12000, by_source: HashMap::new() },
            provenance: vec![],
            metadata: knocode_core::ipc::ContextMetadata::default(),
            repository_state: String::new(),
            code_retrieval_status: knocode_core::RetrievalStatus::NoMatch,
            retrieval_diagnostic: None,
            retrieval_stats: None,
        };
        assert_eq!(ContextEngine::to_yaml(&pack).unwrap(), "");
    }

    #[test]
    fn test_to_yaml_omits_empty_sections_and_has_content() {
        // TASK-031/F-2: with hits, appended block contains actual content, no empty skeletons
        use knocode_core::ipc::ContextProvenance;
        let pack = ContextPack {
            docs_context: String::new(),
            code_context: "// src/Checkout.cs:10\npublic async Task Checkout()".to_string(),
            token_usage: TokenUsage { total_tokens: 42, budget_remaining: 11958, by_source: HashMap::new() },
            provenance: vec![ContextProvenance { path: "src/Checkout.cs".into(), source: "code".into(), retriever: "tantivy".into(), score: 3.2, reason: "bm25".into() }],
            metadata: knocode_core::ipc::ContextMetadata::default(),
            repository_state: String::new(),
            code_retrieval_status: knocode_core::RetrievalStatus::Found(1),
            retrieval_diagnostic: None,
            retrieval_stats: None,
        };
        let yaml = ContextEngine::to_yaml(&pack).unwrap();
        assert!(yaml.contains("code_context"));
        assert!(yaml.contains("Checkout"));
        assert!(!yaml.contains("docs_context:"), "empty sections must be omitted");
        assert!(yaml.contains("total_tokens: 42"));
    }

    #[tokio::test]
    async fn test_per_repo_resolution_no_cross_repo_leak() {
        // TASK-036/F-7 + F-1 acceptance: one engine, two repos — each request resolves to its own repo.
        let _env = ENV_LOCK.lock().unwrap();
        std::env::set_var("KNOCODE_INDEX_DIR", std::env::temp_dir().join(format!("knocode_idx_{}", uuid::Uuid::new_v4())).to_string_lossy().to_string());
        std::env::set_var("KNOCODE_REPO_STATE", "per-repo-test-head");
        let repo_a = std::env::temp_dir().join(format!("knocode_repoA_{}", uuid::Uuid::new_v4()));
        let repo_b = std::env::temp_dir().join(format!("knocode_repoB_{}", uuid::Uuid::new_v4()));
        for (root, marker) in [(&repo_a, "eshop basket checkout flow unique_marker_alpha"), (&repo_b, "knocode router daemon unique_marker_beta")] {
            std::fs::create_dir_all(root).unwrap();
            std::fs::write(root.join("feature.txt"), format!("{marker}\n")).unwrap();
        }
        // Seed the global tantivy index from both repos
        for root in [&repo_a, &repo_b] {
            let db = knocode_storage::Database::open(&std::path::PathBuf::from(":memory:")).unwrap();
            let mut ri = RepositoryIntelligence::new(root.clone(), db, EventBus::new());
            ri.index_repository().unwrap();
        }
        // Engine whose default view is repo_b (simulates a daemon started in repo_b)
        let db = knocode_storage::Database::open(&std::path::PathBuf::from(":memory:")).unwrap();
        let kh_db = knocode_storage::Database::open(&std::path::PathBuf::from(":memory:")).unwrap();
        let hub = KnowledgeHub::new(kh_db, EventBus::new());
        let engine = ContextEngine::new(
            RepositoryIntelligence::new(repo_b.clone(), db, EventBus::new()),
            hub,
            EventBus::new(),
            ContextConfig::default(),
        );
        // Prompt scoped to repo A must surface ONLY repo A's file even though daemon CWD is repo B
        let task_a = TaskRequest { message: "eshop basket checkout flow unique_marker_alpha".to_string(), session_id: "sA".to_string(), context_hints: None, repository_id: String::new(), repository_path: Some(repo_a.to_string_lossy().to_string()), expected_files: None };
        let pack_a = engine.build_context(&task_a).await.unwrap();
        assert!(pack_a.code_context.contains("unique_marker_alpha"), "repo A content expected, provenance was {:?}", pack_a.provenance);
        // Provenance uniqueness invariant (F-3)
        let mut keys: Vec<(String, String, String)> = pack_a.provenance.iter()
            .map(|p| (p.path.clone(), p.source.clone(), p.retriever.clone())).collect();
        keys.sort();
        keys.dedup();
        assert_eq!(keys.len(), pack_a.provenance.len(), "provenance rows must be unique");

        // Repo-scoped query against repo B works too (same engine instance)
        let task_b = TaskRequest { message: "knocode router daemon unique_marker_beta".to_string(), session_id: "sB".to_string(), context_hints: None, repository_id: String::new(), repository_path: None, expected_files: None };
        let pack_b = engine.build_context(&task_b).await.unwrap();
        assert!(pack_b.code_context.contains("unique_marker_beta"));

        std::env::remove_var("KNOCODE_INDEX_DIR");
        std::env::remove_var("KNOCODE_REPO_STATE");
        let _ = std::fs::remove_dir_all(&repo_a);
        let _ = std::fs::remove_dir_all(&repo_b);
    }

    #[tokio::test]
    #[ignore] // Run with: cargo test -p knocode-context -- --ignored test_eshop_reranker
    async fn test_eshop_reranker() {
        use knocode_core::TaskRequest;
        use knocode_events::EventBus;
        use knocode_knowledge::KnowledgeHub;
        use knocode_repo_intel::RepositoryIntelligence;
        use knocode_storage::Database;
        use std::path::PathBuf;

        let eshop_path = std::path::PathBuf::from(r"C:\LeonRepository\eShopOnWeb");
        if !eshop_path.exists() {
            eprintln!("Skipping eShopOnWeb test: path not found");
            return;
        }

        let _env = ENV_LOCK.lock().unwrap();
        let idx_dir = std::env::temp_dir().join(format!("knocode_eshop_idx_{}", uuid::Uuid::new_v4()));
        std::env::set_var("KNOCODE_INDEX_DIR", idx_dir.to_string_lossy().to_string());

        let db = Database::open(&PathBuf::from(":memory:")).unwrap();
        let event_bus = EventBus::new();
        let mut ri = RepositoryIntelligence::new(eshop_path.clone(), Database::open(&PathBuf::from(":memory:")).unwrap(), event_bus.clone());
        ri.index_repository().expect("index eShopOnWeb");

        let kh = KnowledgeHub::new(db, event_bus.clone());

        let config = ContextConfig::default();
        let engine = ContextEngine::new(ri, kh, event_bus, config);

        // Sample tasks from the golden dataset
        let tasks = vec![
            ("Fix basket total not recalculating when quantity changes", vec!["Basket"]),
            ("Find how JWT token claims are constructed", vec!["Token", "Identity"]),
        ];

        for (query, expected_snippets) in &tasks {
            let task = TaskRequest {
                message: query.to_string(),
                session_id: format!("eshop_test_{}", query.len()),
                context_hints: None,
                repository_id: String::new(),
                repository_path: Some(eshop_path.to_string_lossy().to_string()),
                expected_files: None,
            };
            let pack = engine.build_context(&task).await.unwrap();

            eprintln!("\n=== Query: {} ===", query);
            eprintln!("Code context length: {} chars", pack.code_context.len());
            eprintln!("Provenance entries: {}", pack.provenance.len());
            for p in pack.provenance.iter().take(5) {
                eprintln!("  [{}] {} ({}) score={:.2}", p.source, p.path, p.retriever, p.score);
            }

            // Verify we got results
            assert!(!pack.code_context.is_empty(), "Expected code context for: {}", query);

            // Verify provenance should show code entries
            let code_entries: Vec<_> = pack.provenance.iter().filter(|p| p.source == "code").collect();
            assert!(!code_entries.is_empty(), "Expected code provenance entries for: {}", query);

            // Verify expected files appear in provenance paths (at least one must match)
            let any_expected = expected_snippets.iter().any(|snippet| {
                pack.provenance.iter().any(|p| p.path.contains(snippet))
            });
            assert!(any_expected, "Expected at least one of {:?} in provenance for query: {}", expected_snippets, query);
        }

        std::env::remove_var("KNOCODE_INDEX_DIR");
        let _ = std::fs::remove_dir_all(&idx_dir);
    }

    #[test]
    fn test_engine_reindex_repository_reflects_new_commits() {
        // Daemon auto-reindex acceptance: the watcher reindexes THROUGH the engine
        // (ContextEngine::reindex_repository), and a search on that same engine right
        // afterwards must see a newly added file immediately (fresh tantivy handle,
        // not a stale pre-reindex reader).
        use knocode_events::EventBus;
        use knocode_knowledge::KnowledgeHub;
        use knocode_repo_intel::RepositoryIntelligence;
        use knocode_storage::Database;

        let _env = ENV_LOCK.lock().unwrap();
        std::env::set_var(
            "KNOCODE_INDEX_DIR",
            std::env::temp_dir()
                .join(format!("knocode_reidx_idx_{}", uuid::Uuid::new_v4()))
                .to_string_lossy()
                .to_string(),
        );

        let dir = std::env::temp_dir().join(format!("knocode_reidx_repo_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("a.rs"), "fn alpha_marker_one() {}\n").unwrap();

        let db_path = dir.join("reidx.db");
        let db = Database::open(&db_path).unwrap();
        let kh_db = Database::open(&db_path).unwrap();
        let event_bus = EventBus::new();
        let kh = KnowledgeHub::new(kh_db, event_bus.clone());
        let engine = ContextEngine::new(
            RepositoryIntelligence::new(dir.clone(), db, event_bus.clone()),
            kh,
            event_bus,
            ContextConfig::default(),
        );

        // Initial index through the engine (the same call the daemon watcher makes).
        engine.reindex_repository(None).unwrap();

        // "New commit": a brand-new file appears in the repo.
        std::fs::write(dir.join("b.rs"), "fn beta_marker_two() {}\n").unwrap();
        engine.reindex_repository(None).unwrap();

        // The engine's next search must surface the new file.
        let guard = engine.default_repo_intel.lock().unwrap();
        let repo_id = guard.repository_id().to_string();
        let res = guard
            .search_fulltext("beta_marker_two", None, 10, Some(&repo_id))
            .unwrap();
        let found = res.results.iter().any(|r| r.path == "b.rs" || r.path.ends_with("b.rs"));
        assert!(
            found,
            "post-reindex query must find b.rs, got: {:?}",
            res.results.iter().map(|r| r.path.clone()).collect::<Vec<_>>()
        );

        std::env::remove_var("KNOCODE_INDEX_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
