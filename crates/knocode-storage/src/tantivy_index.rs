use std::collections::HashMap;
use std::path::Path;
use std::sync::{Arc, OnceLock, RwLock};

use tantivy::collector::{Count, TopDocs};
use tantivy::query::{BooleanQuery, QueryParser, Query, TermQuery};
use tantivy::schema::*;
use tantivy::{doc, Index, IndexReader, IndexWriter, ReloadPolicy};
use tracing::info;

/// Global cache for opened indexes — avoids per-query MmapDirectory + Index open (P0 latency fix).
/// Keyed by index_path string; holds Arc for cheap cloning across concurrent preview calls.
static INDEX_CACHE: OnceLock<RwLock<HashMap<String, Arc<TantivyIndex>>>> = OnceLock::new();

fn index_cache() -> &'static RwLock<HashMap<String, Arc<TantivyIndex>>> {
    INDEX_CACHE.get_or_init(|| RwLock::new(HashMap::new()))
}

// ── Schema Definition ────────────────────────────────────────────────────

/// Schema for code search index — V1: title for docs (first Markdown heading) + file_class
pub struct CodeIndexSchema {
    pub schema: Schema,
    /// Tokenized path (TEXT) — for search matching
    pub path_field: Field,
    /// Exact raw path (STRING) — for deletion and exact lookups
    pub raw_path_field: Field,
    pub filename_field: Field,
    pub content_field: Field,
    pub title_field: Field,
    pub language_field: Field,
    pub symbols_field: Field,
    /// Individual symbol names (TEXT, tokenized) — for symbol-specific queries
    pub symbol_name_field: Field,
    /// Symbol kinds (STRING, exact) — class, method, function, property, etc.
    pub symbol_kind_field: Field,
    /// Repository scope field (TASK-030) — every doc is stamped so retrieval can be repo-scoped
    pub repository_field: Field,
    /// File classification (Source, Config, Documentation, etc.) — used for scoring boosts
    pub file_class_field: Field,
}

impl Default for CodeIndexSchema {
    fn default() -> Self {
        Self::new()
    }
}

impl CodeIndexSchema {
    pub fn new() -> Self {
        let mut schema_builder = Schema::builder();

        let path_field = schema_builder.add_text_field("path", TEXT | STORED);
        let raw_path_field = schema_builder.add_text_field("raw_path", STRING | STORED);
        let filename_field = schema_builder.add_text_field("filename", TEXT | STORED);
        // V1 0.8.1: content not STORED (latency + index size on 53k) — lazy via get_file_content for Top 20
        // Old indexes with STORED content still readable (fallback to empty), but new indexes require re-index (rm -rf index)
        let content_field = schema_builder.add_text_field("content", TEXT);
        let title_field = schema_builder.add_text_field("title", TEXT | STORED);
        let language_field = schema_builder.add_text_field("language", STRING | STORED | FAST);
        let symbols_field = schema_builder.add_text_field("symbols", TEXT | STORED);
        let symbol_name_field = schema_builder.add_text_field("symbol_name", TEXT | STORED);
        let symbol_kind_field = schema_builder.add_text_field("symbol_kind", STRING | STORED);
        let repository_field = schema_builder.add_text_field("repository_id", STRING | STORED);
        let file_class_field = schema_builder.add_text_field("file_class", STRING | STORED);

        Self {
            schema: schema_builder.build(),
            path_field,
            raw_path_field,
            filename_field,
            content_field,
            title_field,
            language_field,
            symbols_field,
            symbol_name_field,
            symbol_kind_field,
            repository_field,
            file_class_field,
        }
    }

    /// Get the file-class boost factor for scoring — generic V1 hierarchy:
    /// Documentation > Configuration > Source > Test (query-aware).
    /// Canonical table lives in `knocode_core::ranking` — this is a thin alias
    /// so every BM25 hit site uses the single source of truth.
    pub fn file_class_boost(file_class: &str) -> f32 {
        knocode_core::ranking::file_class_boost(file_class)
    }

    /// Query-aware adjustment for Test files: penalize unless query is about tests.
    /// `*test*` penalty: 0.6× unless query contains test-related terms, then 1.4× boost.
    /// Delegates to the canonical implementation in `knocode_core::ranking`.
    pub fn query_aware_test_multiplier(query: &str, file_class: &str) -> f32 {
        knocode_core::ranking::query_aware_test_multiplier_with(query, file_class, knocode_core::ranking::TEST_PENALTY, knocode_core::ranking::TEST_BOOST)
    }

    /// Get directory-based boost — generic only (V1: docs/workspace, no domain-specific eShop layers).
    /// Delegates to the canonical implementation in `knocode_core::ranking`.
    pub fn directory_boost(path: &str) -> f32 {
        knocode_core::ranking::directory_boost(path)
    }
}

// ── Query Sanitization ───────────────────────────────────────────────────

/// Stop words to ignore when building code search queries.
/// Canonical list lives in `knocode_core::ranking::STOP_WORDS` — kept as an
/// alias so the sanitizer (and its callers/tests) keep working unchanged.
const STOP_WORDS: &[&str] = knocode_core::ranking::STOP_WORDS;

/// Split PascalCase/camelCase identifiers into constituent words.
/// "UserProfile" -> ["user", "profile", "userprofile"]
    /// "SetUserModelAsync" -> ["set", "user", "model", "async", "setusermodelasync"]
fn split_pascal_case(s: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut prev_was_upper = false;
    let mut prev_was_lower = false;

    for c in s.chars() {
        if c == '_' || c == '-' {
            if !current.is_empty() {
                tokens.push(current.to_lowercase());
                current.clear();
            }
            prev_was_upper = false;
            prev_was_lower = false;
            continue;
        }

        if c.is_uppercase() {
            if prev_was_lower && !current.is_empty() {
                // Transition from lowercase to uppercase: "checkout" -> "M"
                tokens.push(current.to_lowercase());
                current.clear();
            }
            current.push(c);
            prev_was_upper = true;
            prev_was_lower = false;
        } else if c.is_lowercase() {
            if prev_was_upper && current.len() > 1 {
                // Transition from uppercase to lowercase: "Checkout" -> "M"
                // "CheckoutM" -> split: "Checkout" + "M..."
                let last_upper = current.pop().unwrap();
                if !current.is_empty() {
                    tokens.push(current.to_lowercase());
                }
                current = last_upper.to_string();
            }
            current.push(c);
            prev_was_upper = false;
            prev_was_lower = true;
        } else {
            current.push(c);
        }
    }

    if !current.is_empty() {
        tokens.push(current.to_lowercase());
    }

    // Also add the full identifier lowercased (for exact matches like "checkoutmodel")
    let full = s.to_lowercase();
    if !tokens.contains(&full) {
        tokens.push(full);
    }

    // Deduplicate while preserving order
    let mut seen = std::collections::HashSet::new();
    tokens.into_iter().filter(|t| seen.insert(t.clone())).collect()
}

/// Sanitize a natural-language or code query for Tantivy BM25 search.
///
/// Preprocess code content for indexing: split PascalCase identifiers into
/// constituent words so natural-language queries can match code symbols.
/// "UserProfile" in source becomes "UserProfile user profile userprofile"
fn preprocess_code_content(text: &str) -> String {
    let mut result = String::with_capacity(text.len() * 2);
    for word in text.split(|c: char| !c.is_alphanumeric() && c != '_') {
        if word.is_empty() {
            continue;
        }
        // Always include the original word
        result.push_str(word);
        result.push(' ');
        // If it's PascalCase/camelCase, also emit split parts
        let parts = split_pascal_case(word);
        for part in &parts {
            if part != &word.to_lowercase() {
                result.push_str(part);
                result.push(' ');
            }
        }
    }
    result
}

/// Deterministic aliases — small, vetted set only (V1: dtslint → pnpm test etc.)
/// Not giant OR expansion that diluted BM25.
fn expand_code_vocabulary(term: &str) -> Vec<String> {
    let t = term.to_lowercase();
    match t.as_str() {
        // V1 limited aliases (DefinitelyTyped + generic)
        "dtslint" => vec!["dtslint".to_string(), "pnpm".to_string(), "test".to_string()],
        "dts" => vec!["dts".to_string(), "index.d.ts".to_string()],
        "type" => vec!["type".to_string(), "index.d.ts".to_string()],
        "types" => vec!["types".to_string(), "index.d.ts".to_string()],
        _ => vec![],
    }
}

fn sanitize_code_query(query: &str) -> String {
    // Strip special Tantivy characters
    let cleaned: String = query
        .chars()
        .map(|c| match c {
            ':' | '+' | '-' | '(' | ')' | '"' | '~' | '*' | '?' | '\\' | '^' => ' ',
            _ => c,
        })
        .collect();

    // Extract meaningful keywords, splitting PascalCase identifiers
    let mut all_terms: Vec<String> = Vec::new();
    for word in cleaned.split_whitespace() {
        let w = word.trim_matches(|c: char| !c.is_alphanumeric() && c != '_');
        if w.len() < 2 || STOP_WORDS.contains(&w.to_lowercase().as_str()) {
            continue;
        }
        // Split PascalCase: "CheckoutModel" -> ["checkout", "model", "checkoutmodel"]
        let parts = split_pascal_case(w);
        for part in parts {
            if part.len() >= 2 && !all_terms.contains(&part) {
                all_terms.push(part);
            }
        }
        // Code vocabulary expansion: "controller" -> ["Controller"]
        for expanded in expand_code_vocabulary(w) {
            if !all_terms.contains(&expanded.to_lowercase()) {
                all_terms.push(expanded.to_lowercase());
            }
        }
    }

    if all_terms.is_empty() {
        // Fallback: use raw cleaned query
        let raw = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
        if raw.is_empty() { query.to_string() } else { raw }
    } else {
        // OR-join so any keyword can match (broadens recall)
        all_terms.join(" OR ")
    }
}

// ── Tantivy Index ────────────────────────────────────────────────────────

/// Tantivy-based full-text search index
pub struct TantivyIndex {
    index: Index,
    schema: CodeIndexSchema,
    index_path: String,
    /// Cached reader (OnCommitWithDelay, shared across queries). Lazily created, reloads on commit via reload().
    cached_reader: OnceLock<IndexReader>,
}

impl TantivyIndex {
    /// Create or open a tantivy index
    pub fn open(index_path: &str) -> Result<Self, String> {
        let schema = CodeIndexSchema::new();

        let path = Path::new(index_path);
        if !path.exists() {
            std::fs::create_dir_all(path)
                .map_err(|e| format!("Failed to create index directory: {}", e))?;
        }

        let index = match Index::open_or_create(
            tantivy::directory::MmapDirectory::open(path)
                .map_err(|e| format!("Failed to open index directory: {}", e))?,
            schema.schema.clone(),
        ) {
            Ok(idx) => idx,
            Err(e) => {
                // Schema drift (e.g. 0.8.0 content STORED→TEXT) — old index incompatible; recreate
                let msg = e.to_string();
                if msg.contains("schema") || msg.contains("Schema") {
                    tracing::warn!(path = index_path, error = %msg, "Tantivy schema mismatch — recreating index (re-index required)");
                    // Invalidate any cached handle for this index path (schema drift)
                    Self::invalidate_cached(index_path);
                    std::fs::remove_dir_all(path).ok();
                    std::fs::create_dir_all(path)
                        .map_err(|e2| format!("Failed to recreate index directory after schema mismatch: {}", e2))?;
                    Index::open_or_create(
                        tantivy::directory::MmapDirectory::open(path)
                            .map_err(|e2| format!("Failed to reopen index directory: {}", e2))?,
                        schema.schema.clone(),
                    )
                    .map_err(|e2| format!("Failed to open tantivy index after recreate: {} (orig: {})", e2, msg))?
                } else {
                    return Err(format!("Failed to open tantivy index: {}", e));
                }
            }
        };

        info!(path = index_path, "Tantivy index opened");

        Ok(Self {
            index,
            schema,
            index_path: index_path.to_string(),
            cached_reader: OnceLock::new(),
        })
    }

    /// Cached open — returns Arc from global cache if present (P0: avoids per-query MmapDirectory open).
    /// Falls back to `Self::open` and inserts into cache on first call per index_path.
    pub fn open_cached(index_path: &str) -> Result<Arc<Self>, String> {
        {
            let cache = index_cache().read().map_err(|e| format!("cache lock poisoned: {e}"))?;
            if let Some(hit) = cache.get(index_path) {
                return Ok(hit.clone());
            }
        }
        let fresh = Arc::new(Self::open(index_path)?);
        let mut cache = index_cache().write().map_err(|e| format!("cache lock poisoned: {e}"))?;
        // double-check after write lock
        if let Some(hit) = cache.get(index_path) {
            return Ok(hit.clone());
        }
        cache.insert(index_path.to_string(), fresh.clone());
        Ok(fresh)
    }

    /// Evict a cached index handle so the next `open_cached` rebuilds it from disk.
    /// Call after any in-process re-index that wrote through a DIFFERENT handle
    /// (e.g. `RepositoryIntelligence::index_repository`, which uses `open` not
    /// `open_cached`): it forces the next query to create a fresh `TantivyIndex`
    /// + `IndexReader` and thus observe the new commit immediately instead of
    ///   serving from the stale pre-reindex cached reader.
    pub fn invalidate_cached(index_path: &str) {
        if let Ok(mut cache) = index_cache().write() {
            cache.remove(index_path);
        }
    }

    /// Get or create the cached reader (reloads if index has committed since last reader creation).
    pub fn cached_reader(&self) -> Result<IndexReader, String> {
        if let Some(r) = self.cached_reader.get() {
            // Ensure we see latest commits (MmapDirectory with OnCommitWithDelay may need reload)
            // Reload is cheap if no new commit; searcher().num_docs() will reflect latest after reload.
            // We call reload on the cached reader's underlying index via `r.reload()` best-effort.
            let _ = r.reload();
            return Ok(r.clone());
        }
        let r = self.reader()?;
        let _ = self.cached_reader.set(r.clone());
        Ok(r)
    }

    /// Get a writer for indexing documents — adaptive heap based on index size
    pub fn writer(&self) -> Result<IndexWriter, String> {
        // 256MB heap — larger than default (150MB) to reduce segment merges during bulk indexing.
        // Adaptive heap was removed because creating a reader during init caused deadlocks.
        self.writer_with_heap(256_000_000)
    }

    /// Get a writer with explicit heap (tests can use smaller)
    pub fn writer_with_heap(&self, heap_bytes: usize) -> Result<IndexWriter, String> {
        self.index
            .writer(heap_bytes)
            .map_err(|e| format!("Failed to create index writer: {}", e))
    }

    /// Get a reader for searching
    pub fn reader(&self) -> Result<IndexReader, String> {
        self.index
            .reader_builder()
                .reload_policy(ReloadPolicy::OnCommitWithDelay)
                .try_into()
                .map_err(|e| format!("Failed to create index reader: {}", e))
    }

    /// Tokenize a file path into searchable terms.
    /// "src/components/User/Profile.cs" -> "src components user profile cs"
    fn tokenize_path(path: &str) -> String {
        let mut tokens = Vec::new();
        for component in path.split(['/', '\\']) {
            // Split each path component by PascalCase, dots, hyphens
            for part in component.split(['.', '-', '_']) {
                if part.is_empty() { continue; }
                // Add the full component lowercased
                let lower = part.to_lowercase();
                if !tokens.contains(&lower) {
                    tokens.push(lower.clone());
                }
                // Split PascalCase: "UserProfile" -> "userprofile", "user", "profile"
                for split in split_pascal_case(part) {
                    if split != lower && !tokens.contains(&split) {
                        tokens.push(split);
                    }
                }
            }
        }
        tokens.join(" ")
    }

    /// Extract first Markdown heading as title for docs (V1: no Tree-sitter for *.md)
    pub fn extract_markdown_title(content: &str) -> String {
        for line in content.lines() {
            let t = line.trim();
            if let Some(stripped) = t.strip_prefix("# ") {
                let title = stripped.trim();
                if !title.is_empty() { return title.to_string(); }
            } else if let Some(stripped) = t.strip_prefix("## ") {
                let title = stripped.trim();
                if !title.is_empty() { return title.to_string(); }
            }
        }
        String::new()
    }

    /// Extract all Markdown headings from content for better BM25 lexical matching.
    /// For Documentation files, headings like "#### Create a new package" should be
    /// searchable in the content field (not just the title field), so queries like
    /// "how to add" can match "Create a new package" via "create" + "package" terms.
    fn extract_all_markdown_headings(content: &str) -> String {
        let mut headings = Vec::new();
        for line in content.lines() {
            let t = line.trim();
            if t.starts_with('#') {
                // Strip all leading '#' characters and trim
                let stripped = t.trim_start_matches('#').trim();
                if !stripped.is_empty() {
                    headings.push(stripped.to_string());
                }
            }
        }
        headings.join(" ")
    }

    /// Add a document to the index, stamped with its repository scope (TASK-030)
    #[allow(clippy::too_many_arguments)]
    pub fn add_document(
        &self,
        writer: &mut IndexWriter,
        path: &str,
        content: &str,
        language: &str,
        symbols: &[String],
        symbol_kinds: &[String],
        repository_id: &str,
        file_class: &str,
    ) -> Result<(), String> {
        // Backward compat: title auto-extracted for Documentation
        let title = if file_class == "Documentation" { Self::extract_markdown_title(content) } else { String::new() };
        self.add_document_with_title(writer, path, content, language, symbols, symbol_kinds, repository_id, file_class, &title)
    }

    /// Add document with explicit title (V1 docs: first heading)
    #[allow(clippy::too_many_arguments)]
    pub fn add_document_with_title(
        &self,
        writer: &mut IndexWriter,
        path: &str,
        content: &str,
        language: &str,
        symbols: &[String],
        symbol_kinds: &[String],
        repository_id: &str,
        file_class: &str,
        title: &str,
    ) -> Result<(), String> {
        // For Documentation files: prepend all markdown headings to content so BM25
        // can match heading text (e.g., "Create a new package") in the content field,
        // not just the title field. This fixes procedural queries like "how to add"
        // where the query terms map to heading words like "create" + "package".
        let effective_content = if file_class == "Documentation" && !title.is_empty() {
            let headings = Self::extract_all_markdown_headings(content);
            if headings.is_empty() {
                content.to_string()
            } else {
                format!("{} {}", headings, content)
            }
        } else {
            content.to_string()
        };
        // Preprocess: split PascalCase in content and symbols for better BM25 matching
        let processed_content = preprocess_code_content(&effective_content);
        // Path-aware tokenization: add path segments as searchable terms
        let path_tokens = Self::tokenize_path(path);
        let symbols_with_splits: Vec<String> = symbols.iter().flat_map(|s| {
            let mut parts = vec![s.clone()];
            for part in split_pascal_case(s) {
                if part != s.to_lowercase() {
                    parts.push(part);
                }
            }
            parts
        }).collect();
        let symbols_text = symbols_with_splits.join(" ");
        // Extract filename without extension for better matching
        let filename = std::path::Path::new(path)
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("");

        let doc = doc!(
            self.schema.path_field => path_tokens.as_str(),
            self.schema.raw_path_field => path,
            self.schema.filename_field => filename,
            self.schema.content_field => processed_content.as_str(),
            self.schema.title_field => title,
            self.schema.language_field => language,
            self.schema.symbols_field => symbols_text.as_str(),
            self.schema.symbol_name_field => symbols.join(" ").as_str(),
            self.schema.symbol_kind_field => symbol_kinds.join(" ").as_str(),
            self.schema.repository_field => repository_id,
            self.schema.file_class_field => file_class,
        );

        writer
            .add_document(doc)
            .map_err(|e| format!("Failed to add document: {}", e))?;

        Ok(())
    }

    /// Delete a document from the index — scoped to a repository so two repos
    /// sharing a relative path never delete each other's docs (TASK-030)
    pub fn delete_document(
        &self,
        writer: &mut IndexWriter,
        path: &str,
        repository_id: &str,
    ) -> Result<(), String> {
        let repo_term = tantivy::Term::from_field_text(self.schema.repository_field, repository_id);
        // Use raw_path_field (STRING) for exact match — path_field is tokenized TEXT
        let path_term = tantivy::Term::from_field_text(self.schema.raw_path_field, path);
        let both = BooleanQuery::new(vec![
            (tantivy::query::Occur::Must, Box::new(TermQuery::new(repo_term, IndexRecordOption::Basic)) as Box<dyn Query>),
            (tantivy::query::Occur::Must, Box::new(TermQuery::new(path_term, IndexRecordOption::Basic)) as Box<dyn Query>),
        ]);
        let _ = writer
            .delete_query(Box::new(both))
            .map_err(|e| format!("Failed to delete document: {}", e))?;

        Ok(())
    }

    /// Commit changes to the index
    pub fn commit(&self, writer: &mut IndexWriter) -> Result<(), String> {
        writer
            .commit()
            .map_err(|e| format!("Failed to commit index: {}", e))?;

        Ok(())
    }

    /// Search the index — optionally scoped to a single repository (TASK-030).
    /// `repository_id: Some(id)` filters hits to docs stamped with that id; `None` searches all.
    /// `language_filter: Some(lang)` filters hits to docs with that language; `None` searches all.
    /// Results are boosted by file class (Source > Test > Config > Documentation).
    pub fn search(
        &self,
        reader: &IndexReader,
        query: &str,
        language_filter: Option<&str>,
        max_results: usize,
        repository_id: Option<&str>,
    ) -> Result<Vec<SearchHit>, String> {
        let searcher = reader.searcher();

        // Sanitize query: escape Tantivy special chars, extract meaningful keywords
        let sanitized = sanitize_code_query(query);
        if std::env::var("KNOCODE_PROFILE").is_ok() {
            eprintln!("[profile] tantivy.sanitized: '{}' -> '{}'", query, sanitized);
        }

        // Build query with field boosts:
        // - symbols_field: 2.0x (symbol name matches are most relevant)
        // - path_field: 1.5x (file path matches indicate structural relevance)
        // - filename_field: 1.5x (filename matches are strong signals)
        // - content_field: 1.0x (default, content matches are useful but less specific)
        let mut query_parser = QueryParser::for_index(
            &self.index,
            vec![
                self.schema.content_field,
                self.schema.title_field,
                self.schema.symbols_field,
                self.schema.symbol_name_field,
                self.schema.path_field,
                self.schema.filename_field,
            ],
        );
        query_parser.set_field_boost(self.schema.symbol_name_field, 3.0);
        query_parser.set_field_boost(self.schema.title_field, 2.5);
        query_parser.set_field_boost(self.schema.path_field, 2.5);
        query_parser.set_field_boost(self.schema.symbols_field, 2.0);
        query_parser.set_field_boost(self.schema.filename_field, 2.0);

        let user_query = query_parser
            .parse_query(&sanitized)
            .map_err(|e| format!("Failed to parse query '{}' (sanitized from '{}'): {}", sanitized, query, e))?;

        // Repository scope filter (TermQuery AND user query) — TASK-030/F-1
        let mut must_clauses: Vec<(tantivy::query::Occur, Box<dyn Query>)> = Vec::new();

        if let Some(repo) = repository_id {
            let repo_term =
                tantivy::Term::from_field_text(self.schema.repository_field, repo);
            must_clauses.push((
                tantivy::query::Occur::Must,
                Box::new(TermQuery::new(repo_term, IndexRecordOption::Basic)) as Box<dyn Query>,
            ));
        }

        // Language filter — TermQuery on the indexed language field
        if let Some(lang) = language_filter {
            if !lang.is_empty() {
                let lang_term =
                    tantivy::Term::from_field_text(self.schema.language_field, lang);
                must_clauses.push((
                    tantivy::query::Occur::Must,
                    Box::new(TermQuery::new(lang_term, IndexRecordOption::Basic)) as Box<dyn Query>,
                ));
            }
        }

        must_clauses.push((tantivy::query::Occur::Must, user_query));

        let full_query: Box<dyn Query> = if must_clauses.len() == 1 {
            must_clauses.remove(0).1
        } else {
            Box::new(BooleanQuery::new(must_clauses))
        };

        // Candidate pool size: max_results is candidateK (20/50/100/500/1000) — env KNOCODE_CANDIDATE_K
        // overrides config/CLI. Previously clamped to 200, which silently voided the documented
        // structural candidate_k of 500-1000 (bench_components showed Candidate K as +0.0% partly
        // because of this clamp). Clamp raised to 1000 to honor policy.
        let fetch_limit = std::env::var("KNOCODE_CANDIDATE_K")
            .ok()
            .and_then(|v| v.parse::<usize>().ok())
            .unwrap_or(max_results)
            .min(1000);
        let _search_start = std::time::Instant::now();
        let top_docs = searcher
            .search(&full_query, &TopDocs::with_limit(fetch_limit).order_by_score())
            .map_err(|e| format!("Search failed: {}", e))?;
        if std::env::var("KNOCODE_PROFILE").is_ok() {
            eprintln!("[profile] tantivy.search: {}ms (fetch_limit={})", _search_start.elapsed().as_millis(), fetch_limit);
        }

        let mut results = Vec::new();

        for (score, doc_addr) in top_docs {
            if results.len() >= max_results {
                break;
            }
            if let Ok(doc) = searcher.doc::<tantivy::TantivyDocument>(doc_addr) {
                let path = doc
                    .get_first(self.schema.raw_path_field)
                    .or_else(|| doc.get_first(self.schema.path_field))
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                // P0: Avoid fetching STORED content_field for all candidates (large for 53k).
                // Content is lazy-loaded via RepositoryIntelligence::get_file_content for final Top 20.
                // Keep empty here; ContextEngine will populate via file read after ranking.
                let content = String::new();

                let language = doc
                    .get_first(self.schema.language_field)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let symbols = doc
                    .get_first(self.schema.symbols_field)
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();

                let file_class = doc
                    .get_first(self.schema.file_class_field)
                    .and_then(|v| v.as_str())
                    .unwrap_or("Source")
                    .to_string();

                // Apply file-class boost (Documentation > Config > Source > Test) + query-aware Test penalty + directory boost
                let class_boost = CodeIndexSchema::file_class_boost(&file_class);
                let test_multiplier = CodeIndexSchema::query_aware_test_multiplier(query, &file_class);
                let dir_boost = CodeIndexSchema::directory_boost(&path);
                let boosted_score = score * class_boost * test_multiplier * dir_boost;

                // Skip files with zero boost (Binary, Vendor, Dependency, Stylesheet)
                if class_boost == 0.0 {
                    continue;
                }

                results.push(SearchHit {
                    path,
                    content,
                    language,
                    symbols,
                    score: boosted_score,
                    file_class,
                });
            }
        }

        Ok(results)
    }

    /// Count documents matching a query without fetching hits — O(1) vs O(N) for search.
    pub fn count(
        &self,
        reader: &IndexReader,
        query: &str,
        repository_id: Option<&str>,
    ) -> Result<usize, String> {
        let searcher = reader.searcher();

        let sanitized = sanitize_code_query(query);
        let query_parser = QueryParser::for_index(
            &self.index,
            vec![
                self.schema.content_field,
                self.schema.symbols_field,
                self.schema.path_field,
                self.schema.filename_field,
            ],
        );

        let user_query = query_parser
            .parse_query(&sanitized)
            .map_err(|e| format!("Failed to parse query: {}", e))?;

        let full_query: Box<dyn Query> = match repository_id {
            Some(repo) => {
                let repo_term =
                    tantivy::Term::from_field_text(self.schema.repository_field, repo);
                Box::new(BooleanQuery::new(vec![
                    (
                        tantivy::query::Occur::Must,
                        Box::new(TermQuery::new(repo_term, IndexRecordOption::Basic)) as Box<dyn Query>,
                    ),
                    (tantivy::query::Occur::Must, user_query),
                ]))
            }
            None => Box::new(user_query),
        };

        let count = searcher
            .search(&full_query, &Count)
            .map_err(|e| format!("Count failed: {}", e))?;

        Ok(count)
    }

    /// Get index statistics
    pub fn stats(&self, reader: &IndexReader) -> Result<IndexStats, String> {
        let searcher = reader.searcher();
        let doc_count = searcher.num_docs() as usize;

        Ok(IndexStats {
            doc_count,
            index_path: self.index_path.clone(),
        })
    }

    /// Count documents in a specific repository
    pub fn count_repo_docs(&self, reader: &IndexReader, repository_id: &str) -> Result<usize, String> {
        let searcher = reader.searcher();
        let repo_term = tantivy::Term::from_field_text(self.schema.repository_field, repository_id);
        let query = TermQuery::new(repo_term, IndexRecordOption::Basic);
        let count = searcher
            .search(&query, &Count)
            .map_err(|e| format!("Count failed: {}", e))?;
        Ok(count)
    }
}

// ── Data Types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct SearchHit {
    pub path: String,
    pub content: String,
    pub language: String,
    pub symbols: String,
    pub score: f32,
    pub file_class: String,
}

#[derive(Debug, Clone)]
pub struct IndexStats {
    pub doc_count: usize,
    pub index_path: String,
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_create_index() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().to_str().unwrap();

        let index = TantivyIndex::open(index_path);
        assert!(index.is_ok());
    }

    #[test]
    fn test_add_and_search() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().to_str().unwrap();

        let index = TantivyIndex::open(index_path).unwrap();
        let mut writer = index.writer().unwrap();

        // Add a document
        index
            .add_document(
                &mut writer,
                "src/main.rs",
                "fn main() { println!(\"Hello\"); }",
                "rust",
                &["main".to_string()],
                &[],
                "repo_a",
                "Source",
            )
            .unwrap();

        index.commit(&mut writer).unwrap();

        // Search — scoped to the right repo finds it (TASK-030)
        let reader = index.reader().unwrap();
        let results = index.search(&reader, "main", None, 10, Some("repo_a")).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].path, "src/main.rs");

        // Scoped to another repo → no cross-repo leakage (F-1)
        let other = index.search(&reader, "main", None, 10, Some("repo_b")).unwrap();
        assert_eq!(other.len(), 0);

        // Unscoped still sees it (back-compat)
        let all = index.search(&reader, "main", None, 10, None).unwrap();
        assert_eq!(all.len(), 1);
    }

    #[test]
    fn test_delete_document() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().to_str().unwrap();

        let index = TantivyIndex::open(index_path).unwrap();
        let mut writer = index.writer().unwrap();

        // Same relative path in two repos (TASK-030: deletes must be repo-scoped)
        index
            .add_document(&mut writer, "src/main.rs", "fn main() {}", "rust", &[], &[], "repo_a", "Source")
            .unwrap();
        index
            .add_document(&mut writer, "src/main.rs", "fn main() {}", "rust", &[], &[], "repo_b", "Source")
            .unwrap();
        index.commit(&mut writer).unwrap();

        // Delete only repo_a's copy
        index.delete_document(&mut writer, "src/main.rs", "repo_a").unwrap();
        index.commit(&mut writer).unwrap();

        let reader = index.reader().unwrap();
        let repo_a = index.search(&reader, "main", None, 10, Some("repo_a")).unwrap();
        assert_eq!(repo_a.len(), 0);
        let repo_b = index.search(&reader, "main", None, 10, Some("repo_b")).unwrap();
        assert_eq!(repo_b.len(), 1, "other repo's doc must survive");
    }



    #[test]
    fn test_index_stats() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().to_str().unwrap();

        let index = TantivyIndex::open(index_path).unwrap();
        let mut writer = index.writer().unwrap();

        index
            .add_document(&mut writer, "test.rs", "test", "rust", &[], &[], "repo_a", "Test")
            .unwrap();
        index.commit(&mut writer).unwrap();

        let reader = index.reader().unwrap();
        let stats = index.stats(&reader).unwrap();

        assert_eq!(stats.doc_count, 1);
    }

    #[test]
    fn test_language_filter() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().to_str().unwrap();

        let index = TantivyIndex::open(index_path).unwrap();
        let mut writer = index.writer().unwrap();

        index
            .add_document(&mut writer, "main.rs", "fn main() {}", "rust", &[], &[], "repo_a", "Source")
            .unwrap();
        index
            .add_document(&mut writer, "app.py", "def main(): pass", "python", &[], &[], "repo_a", "Source")
            .unwrap();
        index.commit(&mut writer).unwrap();

        let reader = index.reader().unwrap();

        // Without language filter — finds both
        let all = index.search(&reader, "main", None, 10, Some("repo_a")).unwrap();
        assert_eq!(all.len(), 2);

        // With language filter — only rust
        let rust = index.search(&reader, "main", Some("rust"), 10, Some("repo_a")).unwrap();
        assert_eq!(rust.len(), 1);
        assert_eq!(rust[0].language, "rust");

        // With language filter — only python
        let python = index.search(&reader, "main", Some("python"), 10, Some("repo_a")).unwrap();
        assert_eq!(python.len(), 1);
        assert_eq!(python[0].language, "python");

        // With non-matching language filter
        let none = index.search(&reader, "main", Some("java"), 10, Some("repo_a")).unwrap();
        assert_eq!(none.len(), 0);
    }

    #[test]
    fn test_count() {
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().to_str().unwrap();

        let index = TantivyIndex::open(index_path).unwrap();
        let mut writer = index.writer().unwrap();

        index
            .add_document(&mut writer, "a.rs", "fn alpha() {}", "rust", &[], &[], "repo_a", "Source")
            .unwrap();
        index
            .add_document(&mut writer, "b.rs", "fn beta() {}", "rust", &[], &[], "repo_a", "Source")
            .unwrap();
        index
            .add_document(&mut writer, "c.py", "def alpha(): pass", "python", &[], &[], "repo_a", "Source")
            .unwrap();
        index.commit(&mut writer).unwrap();

        let reader = index.reader().unwrap();

        // Count all docs
        let total = index.count(&reader, "alpha", None).unwrap();
        assert_eq!(total, 2);

        // Count repo-scoped
        let repo_a = index.count(&reader, "alpha", Some("repo_a")).unwrap();
        assert_eq!(repo_a, 2);

        let repo_b = index.count(&reader, "alpha", Some("repo_b")).unwrap();
        assert_eq!(repo_b, 0);

        // Count repo docs
        let count = index.count_repo_docs(&reader, "repo_a").unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_invalidate_cached_reopens_fresh() {
        // Daemon auto-reindex contract: a query caches an index handle + reader, then a
        // re-index commits NEW docs through a separate raw handle. Invalidation must
        // force the next query to REOPEN (fresh handle + reader) so it sees the new
        // commit instead of whatever the pre-reindex cached reader still serves.
        let temp_dir = TempDir::new().unwrap();
        let index_path = temp_dir.path().to_str().unwrap();

        // Commit "alpha" through a raw handle (same as RepositoryIntelligence::index_repository).
        // NOTE: tantivy's directory write-lock lives on the IndexWriter — it must be
        // dropped before a later run can create its own writer on the same directory.
        let raw_a = TantivyIndex::open(index_path).unwrap();
        let mut writer_a = raw_a.writer().unwrap();
        raw_a
            .add_document(&mut writer_a, "a.rs", "fn alpha_marker_one() {}", "rust", &[], &[], "repo_x", "Source")
            .unwrap();
        raw_a.commit(&mut writer_a).unwrap();
        drop(writer_a);
        drop(raw_a);

        // First query seeds the global cache and its cached reader with alpha.
        let idx_a = TantivyIndex::open_cached(index_path).unwrap();
        let reader_a = idx_a.cached_reader().unwrap();
        let hits_a = idx_a.search(&reader_a, "alpha_marker_one", None, 10, None).unwrap();
        assert!(!hits_a.is_empty(), "initial commit must be visible");

        // A later re-index commits "beta" through a second raw handle while the
        // cached reader above is still alive.
        let raw_b = TantivyIndex::open(index_path).unwrap();
        let mut writer_b = raw_b.writer().unwrap();
        raw_b
            .add_document(&mut writer_b, "b.rs", "fn beta_marker_two() {}", "rust", &[], &[], "repo_x", "Source")
            .unwrap();
        raw_b.commit(&mut writer_b).unwrap();
        drop(writer_b);
        drop(raw_b);

        // The next query must NOT keep serving from the pre-reindex cached reader:
        // invalidate so open_cached() rebuilds a fresh handle from disk.
        TantivyIndex::invalidate_cached(index_path);
        let idx_b = TantivyIndex::open_cached(index_path).unwrap();
        assert!(!Arc::ptr_eq(&idx_a, &idx_b), "invalidation must force a fresh handle");
        let reader_b = idx_b.cached_reader().unwrap();
        let hits_b = idx_b.search(&reader_b, "beta_marker_two", None, 10, None).unwrap();
        assert!(!hits_b.is_empty(), "fresh handle must observe the re-indexed commit");
    }
}
