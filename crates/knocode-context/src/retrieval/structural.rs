//! StructuralRetriever — Level 3 retrieval via ast-grep.
//!
//! ## Three Search Levels
//!
//! ```text
//! Level 1 — Lexical    → Tantivy / ripgrep       "What text is relevant?"
//! Level 2 — Symbolic   → Tree-sitter symbols      "What declarations exist?"
//! Level 3 — Structural → ast-grep                  "What code has this syntactic shape?"
//! ```
//!
//! ## Architecture
//!
//! ```text
//! StructuralRetriever
//!     ├── AstGrepBackend (primary)
//!     │     Uses TsLangAdapter (tree-sitter-language-pack → ast-grep-core)
//!     │     e.g., `app.$METHOD($PATH, $$$HANDLER)` → extract METHOD, PATH, HANDLER
//!     │
//!     └── QueryPlanner
//!           Converts natural language → ast-grep patterns
//!           e.g., "find all functions" → function declaration pattern
//!
//! Single grammar source: tree-sitter-language-pack
//! Two consumers: symbol extraction (parser.rs) + structural matching (this module)
//! ```
//!
//! ## What ast-grep adds over raw tree-sitter
//!
//! Tree-sitter answers: "What is in this file?"  → symbols, AST, declarations
//! ast-grep answers: "Does this code have this structural shape?"  → metavariables, patterns
//!
//! For example, given: `app.get("/users", async (req, res) => { ... });`
//! - Tree-sitter identifies: `call_expression`, `member_expression`, symbols
//! - ast-grep matches: `app.$METHOD($PATH, $$$HANDLER)` → METHOD=get, PATH="/users"

use std::collections::HashSet;
use std::time::Instant;

use ignore::WalkBuilder;

use knocode_repo_intel::structural::{AstGrepBackend, TsLangAdapter, cached_adapter, ext_to_lang_pack_name};
// Re-export for structural_plan module
pub(crate) use knocode_repo_intel::structural::lang_patterns_for;
use knocode_repo_intel::RepositoryIntelligence;

use crate::retrieval::evidence::{Evidence, EvidenceSource, RetrievalResult, RetrievalSignal};
use crate::retrieval::policy::RetrievalPolicy;
use crate::retrieval::query::RetrievalQuery;

// ── Pattern Types ──────────────────────────────────────────────────────

/// Structural pattern parsed from a query.
#[derive(Debug, Clone)]
pub enum StructuralPattern {
    /// Explicit ast-grep pattern with metavariables (e.g., `app.$METHOD($PATH, $$$HANDLER)`)
    AstGrepPattern(String),
    /// Inferred from natural language (e.g., "find all functions" → function pattern)
    Inferred(InferredKind),
}

/// Kinds of structural search inferred from natural language queries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InferredKind {
    Functions,
    Classes,
    Methods,
    Impls,
    Traits,
    Enums,
    Interfaces,
    Modules,
    Calls,
    Imports,
    Extends,
}

impl InferredKind {
    pub fn from_query(query: &str) -> Option<Self> {
        let q = query.to_lowercase();
        // P1.5: More specific patterns checked FIRST (order matters!)
        // Extends must come before Classes ("classes extending" contains "class")
        if q.contains("extends") || q.contains("extending") || q.contains("inherits") || q.contains("subclass") {
            return Some(Self::Extends);
        }
        if q.contains("implements") && (q.contains("interface") || q.contains("class")) {
            return Some(Self::Extends);
        }
        if q.contains("call") || q.contains("invoke") || q.contains("usage of") {
            return Some(Self::Calls);
        }
        if q.contains("import") || q.contains("require") || q.contains("using") {
            return Some(Self::Imports);
        }
        if q.contains("function") || q.contains("fn ") || q.contains("def ") || q.contains("func ") {
            return Some(Self::Functions);
        }
        if q.contains("class") || q.contains("struct") {
            return Some(Self::Classes);
        }
        if q.contains("method") {
            return Some(Self::Methods);
        }
        if q.contains("impl ") {
            return Some(Self::Impls);
        }
        if q.contains("trait") || q.contains("interface") {
            return Some(Self::Interfaces);
        }
        if q.contains("enum") {
            return Some(Self::Enums);
        }
        if q.contains("module") || q.contains("namespace") {
            return Some(Self::Modules);
        }
        None
    }

    /// Convert inferred kind to ast-grep patterns per language.
    /// Delegates to `lang_patterns_for` in the adapter module.
    /// Returns multiple patterns for languages that need them (e.g., Rust functions with/without return type).
    pub fn to_ast_grep_patterns(&self, lang_pack_name: &str) -> Vec<String> {
        lang_patterns_for(self.display(), lang_pack_name)
    }

    pub fn display(&self) -> &str {
        match self {
            Self::Functions => "function",
            Self::Classes => "class",
            Self::Methods => "method",
            Self::Impls => "impl",
            Self::Traits => "trait",
            Self::Enums => "enum",
            Self::Interfaces => "interface",
            Self::Modules => "module",
            Self::Calls => "call",
            Self::Imports => "import",
            Self::Extends => "extends",
        }
    }
}

/// Parse query text into a structural pattern.
pub fn parse_structural_query(query: &str) -> Option<StructuralPattern> {
    let q = query.trim();

    // Check for ast-grep metavariable pattern (contains `$` identifiers)
    if q.contains('$') {
        return Some(StructuralPattern::AstGrepPattern(q.to_string()));
    }

    // P0 fix: inferred structural search runs a FULL ast-grep walk of the repo
    // (~1.2s on 53k files), so it must only fire when the query actually asks
    // for a structural enumeration. Debugging/informational questions that
    // merely CONTAIN a keyword ("why is the Express response type missing json
    // **method**") previously hijacked the structural backend into a full-repo
    // scan on every such query. Require an explicit enumeration trigger in
    // addition to the inferred kind.
    let q_lower = q.to_lowercase();
    let wants_enumeration = ["find ", "show ", "list ", "all ", "where is ", "where are "]
        .iter()
        .any(|t| q_lower.contains(t));
    if wants_enumeration {
        if let Some(kind) = InferredKind::from_query(q) {
            return Some(StructuralPattern::Inferred(kind));
        }
    }

    None
}

/// Create a `TsLangAdapter` from a file extension.
/// Returns `None` for non-code files or unsupported languages.
/// Uses the adapter cache to avoid recreating adapters per file.
pub fn adapter_for_ext(ext: &str) -> Option<TsLangAdapter> {
    let name = ext_to_lang_pack_name(ext)?;
    cached_adapter(name)
}

// ── StructuralRetriever ────────────────────────────────────────────────

/// Structural retriever — Level 3 retrieval via ast-grep.
pub struct StructuralRetriever;

impl StructuralRetriever {
    /// Retrieve structurally matching files.
    ///
    /// When `candidate_files` is provided, only those files are searched
    /// (candidate scoping — avoids scanning the entire repository).
    /// When `deadline` is provided, the search stops early if the deadline is exceeded.
    pub fn retrieve(
        &self,
        query: &RetrievalQuery,
        repo_intel: &RepositoryIntelligence,
        policy: &RetrievalPolicy,
    ) -> RetrievalResult {
        self.retrieve_with_candidates(query, repo_intel, policy, None, None)
    }

    /// Retrieve with explicit candidate files and deadline (P1.2 + P1.3).
    pub fn retrieve_with_candidates(
        &self,
        query: &RetrievalQuery,
        repo_intel: &RepositoryIntelligence,
        policy: &RetrievalPolicy,
        candidate_files: Option<&[String]>,
        deadline: Option<Instant>,
    ) -> RetrievalResult {
        let t0 = Instant::now();

        let pattern = match parse_structural_query(&query.text) {
            Some(p) => p,
            None => {
                return RetrievalResult::empty(knocode_core::RetrievalStatus::NoMatch);
            }
        };

        let repo_path = repo_intel.repo_path();
        let max_results = policy.max_files * 2;
        // Timeout: default 2s per file, or use deadline if provided
        let timeout_per_file_ms: u64 = 2000;

        let mut results = Vec::new();
        let mut files_searched = 0usize;
        let mut timed_out = false;

        // P1.6: Intent-aware file-class filtering
        let intent = query.intent();
        let skip_tests = !matches!(intent, crate::retrieval::intent::QueryIntent::Testing)
            && !query.text.to_lowercase().contains("test");

        // P1.2: Candidate scoping — use provided files or walk entire repo
        if let Some(candidates) = candidate_files {
            // Scoped: only search the provided candidate files
            for rel_path in candidates {
                if let Some(deadline) = deadline {
                    if Instant::now() >= deadline {
                        timed_out = true;
                        break;
                    }
                }
                let path = repo_path.join(rel_path);
                if !path.exists() {
                    continue;
                }
                if let Some(result) = self.search_single_file(
                    &path, rel_path, &pattern, skip_tests, timeout_per_file_ms,
                ) {
                    results.extend(result);
                    files_searched += 1;
                }
                if results.len() >= max_results {
                    break;
                }
            }
        } else {
            // Full walk: scan entire repository
            let walker = WalkBuilder::new(repo_path)
                .hidden(false)
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

                // Deadline check
                if let Some(deadline) = deadline {
                    if Instant::now() >= deadline {
                        timed_out = true;
                        break;
                    }
                }

                let path = entry.path();
                let path_str = path.strip_prefix(repo_path)
                    .unwrap_or(path)
                    .to_string_lossy()
                    .to_string();

                if let Some(result) = self.search_single_file(
                    path, &path_str, &pattern, skip_tests, timeout_per_file_ms,
                ) {
                    results.extend(result);
                    files_searched += 1;
                }

                if results.len() >= max_results {
                    break;
                }
            }
        }

        let ranking_ms = t0.elapsed().as_millis() as u64;

        // Dedup by path, keep highest score (P0.9: deterministic ordering)
        let mut seen = HashSet::new();
        results.retain(|ev| seen.insert(ev.path.clone()));
        // Deterministic: score descending, then path ascending for ties
        results.sort_by(|a, b| {
            b.score.partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
                .then_with(|| a.path.cmp(&b.path))
        });
        results.truncate(policy.max_files);

        let evidence_count = seen.len();
        let status = if timed_out {
            knocode_core::RetrievalStatus::FallbackUsed("structural:timeout".to_string())
        } else {
            knocode_core::RetrievalStatus::Found(evidence_count)
        };

        RetrievalResult {
            evidence: results,
            status,
            diagnostics: crate::retrieval::evidence::RetrievalDiagnostics {
                candidate_count: files_searched,
                filtered_count: 0,
                tantivy_ms: 0,
                ranking_ms: 0,
                graph_ms: 0,
                structural_ms: ranking_ms,
                doc_count: 0,
                candidate_k: max_results,
                max_files: policy.max_files,
                backends: vec![
                    crate::retrieval::evidence::BackendMetrics {
                        backend: "structural".into(),
                        query: query.text.clone(),
                        language: None,
                        candidate_count: files_searched,
                        match_count: evidence_count,
                        duration_ms: ranking_ms,
                        status: if timed_out { "timeout".into() } else { "ok".into() },
                    },
                ],
            },
        }
    }

    /// Search a single file for structural matches.
    /// Returns None if the file should be skipped (binary, unsupported lang, test file).
    /// FIX #9: timeout_ms is now documented as reserved for future per-file timeout.
    /// Currently unused — per-file timeout is handled at the walk level via `deadline`.
    fn search_single_file(
        &self,
        path: &std::path::Path,
        path_str: &str,
        pattern: &StructuralPattern,
        skip_tests: bool,
        _timeout_ms: u64, // Reserved: per-file timeout (currently using walk-level deadline)
    ) -> Option<Vec<Evidence>> {
        if is_skip_file(path_str) {
            return None;
        }

        // P1.6: Skip test files when query is not test-related
        if skip_tests && is_test_file(path_str) {
            return None;
        }

        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => return None,
        };

        let ext = path.extension().and_then(|e| e.to_str()).unwrap_or("");
        let adapter = adapter_for_ext(ext)?;

        // FIX #2: Wrap in catch_unwind to prevent ast-grep panics from crashing retrieval.
        // ast-grep can panic on ambiguous patterns (e.g., MultipleNode errors).
        let matches = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            match pattern {
                StructuralPattern::AstGrepPattern(pat) => {
                    ast_grep_search(&adapter, pat, &content, path_str)
                }
                StructuralPattern::Inferred(kind) => {
                    let lang_name = ext_to_lang_pack_name(ext).unwrap_or("");
                    let patterns = kind.to_ast_grep_patterns(lang_name);
                    let mut all_results = Vec::new();
                    for pat in &patterns {
                        all_results.extend(ast_grep_search(&adapter, pat, &content, path_str));
                    }
                    all_results
                }
            }
        }))
        .unwrap_or_default(); // On panic, return empty results (graceful degradation)

        if matches.is_empty() {
            None
        } else {
            Some(matches)
        }
    }
}

// ── AstGrepBackend — Level 3 primary ──────────────────────────────────

/// Execute an ast-grep pattern search against file content.
/// Uses `AstGrepBackend::search()` which handles panics, ambiguous patterns,
/// and unsupported languages internally.
fn ast_grep_search(
    backend: &dyn AstGrepBackend,
    pattern: &str,
    content: &str,
    path_str: &str,
) -> Vec<Evidence> {
    let result = match backend.search(pattern, content) {
        Ok(r) => r,
        Err(e) => {
            // Log error but don't fail — structural search is best-effort
            tracing::debug!(
                pattern = %pattern,
                path = %path_str,
                error = %e,
                "structural search error"
            );
            return Vec::new();
        }
    };

    let mut evidence = Vec::with_capacity(result.matches.len());
    for m in &result.matches {
        let score = compute_ast_grep_score(&m.text, pattern, &m.captures);
        let fc = infer_file_class(path_str);

        let mut ev = Evidence::new(path_str.to_string(), score, fc);
        ev.source = EvidenceSource::Structural;
        ev.line = m.line as usize + 1; // Convert 0-indexed to 1-indexed
        ev.column = Some(m.column as usize);
        ev.language = Some(result.language.clone());
        ev.match_kind = Some(m.node_kind.clone());
        ev.captures = m.captures.clone();
        ev.signals.push(RetrievalSignal::StructuralMatch {
            pattern: pattern.to_string(),
            score,
        });

        // Store captures in matched_symbols for backward compatibility
        for (name, value) in &m.captures {
            ev.matched_symbols.push(format!("{}={}", name, value));
        }

        evidence.push(ev);
    }

    evidence
}

/// Score for an ast-grep match — higher for more specific patterns.
fn compute_ast_grep_score(node_text: &str, pattern: &str, captures: &[(String, String)]) -> f32 {
    let mut score: f32 = 0.85; // ast-grep matches are higher confidence

    // Bonus for longer matches (more specific)
    let text_len = node_text.len() as f32;
    if text_len > 100.0 {
        score += 0.1;
    } else if text_len > 50.0 {
        score += 0.05;
    }

    // Bonus for metavariable captures (richer patterns)
    let metavar_count = pattern.matches('$').count();
    if metavar_count > 0 {
        score += 0.05 * metavar_count as f32;
    }

    // Bonus for actual captures extracted
    if !captures.is_empty() {
        score += 0.05 * captures.len() as f32;
    }

    score.min(1.0)
}

// ── Helpers ────────────────────────────────────────────────────────────

fn is_skip_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    if lower.starts_with("node_modules/") || lower.starts_with(".git/") {
        return true;
    }
    if lower.ends_with(".min.js") || lower.ends_with(".min.css") {
        return true;
    }
    // Match both "/vendor/" (in middle) and "vendor/" (at start)
    if lower.contains("/vendor/") || lower.starts_with("vendor/") {
        return true;
    }
    false
}

/// Check if a file path looks like a test file.
fn is_test_file(path: &str) -> bool {
    let lower = path.to_lowercase();
    // Match both "/tests/" (in middle) and "tests/" (at start)
    lower.starts_with("test/") || lower.starts_with("tests/")
        || lower.contains("/test/") || lower.contains("/tests/")
        || lower.contains("/__tests__/") || lower.contains("/spec/")
        || lower.ends_with("_test.rs") || lower.ends_with("_test.py")
        || lower.ends_with("_test.js") || lower.ends_with("_test.ts")
        || lower.ends_with("_spec.js") || lower.ends_with("_spec.ts")
        || lower.contains(".test.") || lower.contains(".spec.")
}

fn infer_file_class(path: &str) -> String {
    let lower = path.to_lowercase();
    // Match both "/tests/" (in middle) and "tests/" (at start)
    if lower.starts_with("test/") || lower.starts_with("tests/")
        || lower.contains("/test/") || lower.contains("/tests/")
        || lower.contains("_test.") || lower.contains("_spec.")
    {
        return "Test".to_string();
    }
    if lower.contains("/docs/") || lower.ends_with(".md") || lower.contains("readme") {
        return "Documentation".to_string();
    }
    if lower.ends_with(".toml") || lower.ends_with(".yaml") || lower.ends_with(".yml") || lower.ends_with(".json") || lower.ends_with(".env") {
        return "Config".to_string();
    }
    "Source".to_string()
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use knocode_repo_intel::structural::create_adapter;

    fn rust_adapter() -> TsLangAdapter {
        create_adapter("rust").expect("rust language pack not available")
    }

    fn python_adapter() -> TsLangAdapter {
        create_adapter("python").expect("python language pack not available")
    }

    fn ts_adapter() -> TsLangAdapter {
        create_adapter("typescript").expect("typescript language pack not available")
    }

    fn js_adapter() -> TsLangAdapter {
        create_adapter("javascript").expect("javascript language pack not available")
    }

    #[test]
    fn parse_ast_grep_metavar_pattern() {
        let pattern = parse_structural_query("fn $FUNC($$$) { $$$ }");
        assert!(matches!(pattern, Some(StructuralPattern::AstGrepPattern(_))));
    }

    #[test]
    fn parse_inferred_functions() {
        let pattern = parse_structural_query("find all functions in the codebase");
        assert!(matches!(pattern, Some(StructuralPattern::Inferred(InferredKind::Functions))));
    }

    #[test]
    fn no_structural_for_procedural() {
        let pattern = parse_structural_query("How do I add a new package?");
        assert!(pattern.is_none());
    }

    #[test]
    fn inferred_to_ast_grep_patterns_rust() {
        let kind = InferredKind::Functions;
        let patterns = kind.to_ast_grep_patterns("rust");
        assert!(!patterns.is_empty());
        assert!(patterns.iter().any(|p| p.contains("$NAME")));
        // Rust functions need both with and without return type
        assert!(patterns.len() >= 2);
    }

    #[test]
    fn inferred_to_ast_grep_patterns_python() {
        let kind = InferredKind::Functions;
        let patterns = kind.to_ast_grep_patterns("python");
        assert!(!patterns.is_empty());
        assert!(patterns.iter().any(|p| p.contains("def")));
    }

    #[test]
    fn adapter_for_ext_mapping() {
        assert!(adapter_for_ext("rs").is_some());
        assert!(adapter_for_ext("py").is_some());
        assert!(adapter_for_ext("ts").is_some());
        assert!(adapter_for_ext("go").is_some());
        assert!(adapter_for_ext("xyz").is_none());
    }

    #[test]
    fn test_ast_grep_rust_function() {
        let adapter = rust_adapter();
        let code = "fn main() { println!(\"hello\"); }\nfn add(a: i32, b: i32) -> i32 { a + b }";
        // Test: pattern without return type matches fn main()
        let r1 = ast_grep_search(&adapter, "fn $NAME($$$) { $$$ }", code, "test.rs");
        assert!(r1.len() >= 1, "should match at least fn main()");
        // Test: pattern with return type matches fn add()
        let r2 = ast_grep_search(&adapter, "fn $NAME($$$) -> $RET { $$$ }", code, "test.rs");
        assert!(r2.len() >= 1, "should match at least fn add()");
    }

    #[test]
    fn test_ast_grep_rust_struct() {
        let adapter = rust_adapter();
        let code = "struct Config {\n    name: String,\n}\nstruct Point {\n    x: f64,\n    y: f64,\n}";
        let results = ast_grep_search(&adapter, "struct $NAME { $$$ }", code, "test.rs");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_ast_grep_python_function() {
        let adapter = python_adapter();
        let code = "def hello():\n    print('hello')\ndef add(a, b):\n    return a + b";
        let results = ast_grep_search(&adapter, "def $NAME($$$): $$$", code, "test.py");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_ast_grep_typescript_class() {
        let adapter = ts_adapter();
        let code = "class UserService {\n    constructor() {}\n    getUser() {}\n}";
        let results = ast_grep_search(&adapter, "class $NAME { $$$ }", code, "test.ts");
        assert_eq!(results.len(), 1);
        assert!(results[0].matched_symbols.iter().any(|s| s.contains("UserService")));
    }

    #[test]
    fn test_ast_grep_with_metavariables() {
        let adapter = js_adapter();
        let code = r#"app.get("/users", async (req, res) => { res.json([]); });"#;
        let results = ast_grep_search(&adapter, "app.$METHOD($$$)", code, "app.js");
        assert!(!results.is_empty());
        // Should capture METHOD
        let has_method = results.iter().any(|e| e.matched_symbols.iter().any(|s| s.contains("METHOD=get")));
        assert!(has_method, "should capture METHOD=get, got: {:?}", results[0].matched_symbols);
    }

    #[test]
    fn test_inferred_query_via_ast_grep() {
        let adapter = rust_adapter();
        let code = "fn main() {}\nstruct Config {}";
        let kind = InferredKind::Functions;
        let patterns = kind.to_ast_grep_patterns("rust");
        let mut all_results = Vec::new();
        for pat in &patterns {
            all_results.extend(ast_grep_search(&adapter, pat, code, "test.rs"));
        }
        assert!(!all_results.is_empty());
        assert!(all_results.iter().any(|e| e.signals.iter().any(|s| matches!(s, RetrievalSignal::StructuralMatch { .. }))));
    }

    // ── P1.8: Structural Benchmark ──────────────────────────────────────

    /// Benchmark: measure structural pattern search latency across languages.
    /// This test documents the baseline performance for the structural backend.
    #[test]
    fn test_structural_benchmark_latency() {
        // Larger code samples for realistic benchmarking
        let rust_code = r#"
fn main() {
    let config = Config::new();
    println!("Hello, {}!", config.name);
}

fn add(a: i32, b: i32) -> i32 {
    a + b
}

struct Config {
    name: String,
    timeout: u64,
}

impl Config {
    fn new() -> Self {
        Config { name: "default".into(), timeout: 30 }
    }
}

enum Color {
    Red,
    Green,
    Blue,
}

trait Drawable {
    fn draw(&self);
}

mod utils {
    pub fn helper() -> String {
        "help".into()
    }
}
"#;

        let ts_code = r#"
import express from 'express';

interface User {
    name: string;
    age: number;
}

class UserController {
    getUser() { return {}; }
}

function createUser(data: any) {
    return { name: data.name, age: data.age };
}
"#;

        let py_code = r#"
import os

def hello():
    pass

def add(a, b):
    return a + b

class Config:
    pass
"#;

        // Benchmark scenarios: (name, lang, code, pattern, expected_min)
        let scenarios: Vec<(&str, &str, &str, &str, usize)> = vec![
            // Function declarations
            ("rust_fn", "rust", rust_code, "fn $NAME($$$) { $$$ }", 1), // main only; add has return type
            ("ts_fn", "typescript", ts_code, "function $NAME($$$) { $$$ }", 1), // createUser only
            ("py_fn", "python", py_code, "def $NAME($$$): $$$", 2), // hello, add
            // Class/struct declarations
            ("rust_struct", "rust", rust_code, "struct $NAME { $$$ }", 1),
            ("ts_class", "typescript", ts_code, "class $NAME { $$$ }", 1),
            ("py_class", "python", py_code, "class $NAME: $$$", 1),
            // Method calls
            ("py_call", "python", py_code, "$OBJ.$METHOD($$$)", 0), // no method calls in simplified code
            // Imports
            // TS import pattern: $PATH matches string literal in import statement
            ("ts_import", "typescript", ts_code, "import $NAME from $PATH", 1), // express
            // Interface/trait
            ("ts_interface", "typescript", ts_code, "interface $NAME { $$$ }", 1), // User
            ("rust_trait", "rust", rust_code, "trait $NAME { $$$ }", 1), // Drawable
        ];

        for (name, lang, code, pattern, expected_min) in &scenarios {
            let adapter = match create_adapter(lang) {
                Some(a) => a,
                None => continue,
            };

            let start = std::time::Instant::now();
            let results = ast_grep_search(&adapter, pattern, code, &format!("bench_{}.ext", lang));
            let elapsed = start.elapsed();

            assert!(
                results.len() >= *expected_min,
                "{}: expected >= {} matches, got {}",
                name, expected_min, results.len()
            );

            // Verify captures are present
            for ev in &results {
                assert!(!ev.captures.is_empty(), "{}: should have captures", name);
                assert!(ev.language.is_some(), "{}: should have language", name);
                assert!(ev.match_kind.is_some(), "{}: should have match_kind", name);
            }

            eprintln!("  {} — {} matches in {:.2}ms", name, results.len(), elapsed.as_secs_f64() * 1000.0);
        }
    }

    /// Benchmark: measure InferredKind detection accuracy.
    #[test]
    fn test_structural_benchmark_inferred_detection() {
        let queries = vec![
            ("find all functions in the codebase", Some(InferredKind::Functions)),
            ("show me all classes", Some(InferredKind::Classes)),
            ("list all methods", Some(InferredKind::Methods)),
            ("find all interfaces", Some(InferredKind::Interfaces)),
            ("show all enums", Some(InferredKind::Enums)),
            ("find all imports", Some(InferredKind::Imports)),
            ("find all calls to foo", Some(InferredKind::Calls)),
            ("find all classes extending Base", Some(InferredKind::Extends)),
            ("How do I add a new package?", None),
            ("why does the build fail?", None),
        ];

        for (query, expected) in &queries {
            let detected = parse_structural_query(query);
            match expected {
                Some(expected_kind) => {
                    match detected {
                        Some(StructuralPattern::Inferred(kind)) => {
                            assert_eq!(&kind, expected_kind, "query '{}' should detect {:?}", query, expected_kind);
                        }
                        _ => panic!("query '{}' should detect Inferred({:?}), got {:?}", query, expected_kind, detected),
                    }
                }
                None => {
                    assert!(detected.is_none(), "query '{}' should not detect structural pattern", query);
                }
            }
        }
    }

    /// Test that new pattern kinds (calls, imports, extends) work via InferredKind.
    #[test]
    fn test_inferred_new_pattern_kinds() {
        // Calls
        let kind = InferredKind::from_query("find all calls to foo").unwrap();
        assert_eq!(kind, InferredKind::Calls);
        let patterns = kind.to_ast_grep_patterns("rust");
        assert!(patterns.iter().any(|p| p.contains("$OBJ.$METHOD")));

        // Imports
        let kind = InferredKind::from_query("find all imports").unwrap();
        assert_eq!(kind, InferredKind::Imports);
        let patterns = kind.to_ast_grep_patterns("python");
        assert!(patterns.iter().any(|p| p.contains("import")));

        // Extends
        let kind = InferredKind::from_query("find all classes extending Base").unwrap();
        assert_eq!(kind, InferredKind::Extends);
        let patterns = kind.to_ast_grep_patterns("typescript");
        assert!(patterns.iter().any(|p| p.contains("extends")));
    }

    // ── P0.7: Adapter Cache Integration Tests ─────────────────────────

    #[test]
    fn test_adapter_for_ext_uses_cache() {
        use knocode_repo_intel::structural::{clear_adapter_cache, adapter_cache_size};
        clear_adapter_cache();
        let _a1 = adapter_for_ext("ts");
        let size_after_first = adapter_cache_size();
        assert!(size_after_first > 0, "adapter_for_ext should populate cache");
        // Second call should not grow cache
        let _a2 = adapter_for_ext("ts");
        assert_eq!(adapter_cache_size(), size_after_first, "should not grow cache for same extension");
    }

    #[test]
    fn test_adapter_for_ext_different_languages_cached() {
        use knocode_repo_intel::structural::{clear_adapter_cache, adapter_cache_size};
        clear_adapter_cache();
        let _rs = adapter_for_ext("rs");
        let _py = adapter_for_ext("py");
        let _ts = adapter_for_ext("ts");
        assert!(adapter_cache_size() >= 3, "should cache adapters for different languages");
    }

    // ── P0.9: Deterministic Ordering Tests ─────────────────────────────

    #[test]
    fn test_deterministic_score_ordering() {
        let adapter = rust_adapter();
        let code = "fn alpha() {}\nfn beta() {}\nfn gamma() {}";
        let results = ast_grep_search(&adapter, "fn $NAME() {}", code, "test.rs");
        // All have same score (same pattern, same length class)
        // Deterministic: should be sorted by path
        for window in results.windows(2) {
            assert!(window[0].path <= window[1].path,
                "ties should be broken by path ascending: {} > {}", window[0].path.display(), window[1].path.display());
        }
    }

    // ── P0: More Language Pattern Tests ────────────────────────────────

    #[test]
    fn test_ast_grep_rust_enum() {
        let adapter = rust_adapter();
        let code = "enum Color { Red, Green, Blue }\nenum Direction { North, South }";
        let results = ast_grep_search(&adapter, "enum $NAME { $$$ }", code, "test.rs");
        assert_eq!(results.len(), 2);
    }

    #[test]
    fn test_ast_grep_rust_trait() {
        let adapter = rust_adapter();
        let code = "trait Drawable { fn draw(&self); }";
        let results = ast_grep_search(&adapter, "trait $NAME { $$$ }", code, "test.rs");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].captures[0].1, "Drawable");
    }

    #[test]
    fn test_ast_grep_rust_impl() {
        let adapter = rust_adapter();
        let code = "impl Config { fn new() -> Self { Config {} } }";
        let results = ast_grep_search(&adapter, "impl $TYPE { $$$ }", code, "test.rs");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_ast_grep_rust_module() {
        let adapter = rust_adapter();
        let code = "mod utils { pub fn helper() {} }";
        let results = ast_grep_search(&adapter, "mod $NAME { $$$ }", code, "test.rs");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_ast_grep_typescript_interface() {
        let adapter = ts_adapter();
        let code = "interface User { name: string; age: number; }";
        let results = ast_grep_search(&adapter, "interface $NAME { $$$ }", code, "test.ts");
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].captures[0].1, "User");
    }

    #[test]
    fn test_ast_grep_typescript_enum() {
        let adapter = ts_adapter();
        let code = "enum Status { Active, Inactive }";
        let results = ast_grep_search(&adapter, "enum $NAME { $$$ }", code, "test.ts");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_ast_grep_python_class() {
        let adapter = python_adapter();
        let code = "class Config:\n    pass";
        let results = ast_grep_search(&adapter, "class $NAME: $$$", code, "test.py");
        assert_eq!(results.len(), 1);
    }

    #[test]
    fn test_ast_grep_rust_method_call() {
        let adapter = rust_adapter();
        let code = "config.save();\nrepo.insert(data);";
        let results = ast_grep_search(&adapter, "$OBJ.$METHOD($$$)", code, "test.rs");
        assert!(results.len() >= 2, "should find save() and insert() method calls");
    }

    #[test]
    fn test_ast_grep_empty_source() {
        let adapter = rust_adapter();
        let results = ast_grep_search(&adapter, "fn $NAME() {}", "", "test.rs");
        assert!(results.is_empty(), "empty source should produce no matches");
    }

    #[test]
    fn test_ast_grep_no_match() {
        let adapter = rust_adapter();
        let code = "struct Config { name: String }";
        let results = ast_grep_search(&adapter, "fn $NAME() {}", code, "test.rs");
        assert!(results.is_empty(), "should not match structs with function pattern");
    }

    // ── P0: Score Computation Tests ────────────────────────────────────

    #[test]
    fn test_compute_score_basic() {
        let score = compute_ast_grep_score("fn main() {}", "fn $NAME() {}", &[]);
        assert!(score >= 0.85 && score <= 1.0);
    }

    #[test]
    fn test_compute_score_with_captures() {
        let score_with = compute_ast_grep_score("fn main() {}", "fn $NAME() {}", &[("NAME".into(), "main".into())]);
        let score_without = compute_ast_grep_score("fn main() {}", "fn $NAME() {}", &[]);
        assert!(score_with > score_without, "captures should boost score");
    }

    #[test]
    fn test_compute_score_metavar_bonus() {
        let score_single = compute_ast_grep_score("x", "fn $NAME() {}", &[]);
        let score_double = compute_ast_grep_score("x", "fn $NAME($$$ARGS) {}", &[]);
        assert!(score_double > score_single, "more metavariables should boost score");
    }

    // ── P0: Helper Function Tests ──────────────────────────────────────

    #[test]
    fn test_is_skip_file() {
        assert!(is_skip_file("node_modules/foo/bar.js"));
        assert!(is_skip_file(".git/HEAD"));
        assert!(is_skip_file("vendor/lib/foo.rs"));
        assert!(is_skip_file("app.min.js"));
        assert!(is_skip_file("style.min.css"));
        assert!(!is_skip_file("src/main.rs"));
        assert!(!is_skip_file("lib/utils.ts"));
    }

    #[test]
    fn test_is_test_file() {
        assert!(is_test_file("tests/test_main.rs"));
        assert!(is_test_file("src/__tests__/foo.test.ts"));
        assert!(is_test_file("lib/utils_test.py"));
        assert!(is_test_file("src/app.spec.js"));
        assert!(!is_test_file("src/main.rs"));
        assert!(!is_test_file("lib/utils.ts"));
    }

    #[test]
    fn test_infer_file_class() {
        assert_eq!(infer_file_class("tests/test_main.rs"), "Test");
        assert_eq!(infer_file_class("docs/README.md"), "Documentation");
        assert_eq!(infer_file_class("README.md"), "Documentation");
        assert_eq!(infer_file_class("config.toml"), "Config");
        assert_eq!(infer_file_class("src/main.rs"), "Source");
    }

    // ── P0: Pattern Type Tests ─────────────────────────────────────────

    #[test]
    fn test_structural_pattern_ast_grep() {
        let p = parse_structural_query("fn $NAME() { }");
        assert!(matches!(p, Some(StructuralPattern::AstGrepPattern(_))));
    }

    #[test]
    fn test_structural_pattern_inferred() {
        let p = parse_structural_query("find all functions");
        assert!(matches!(p, Some(StructuralPattern::Inferred(InferredKind::Functions))));
    }

    #[test]
    fn test_structural_pattern_none() {
        assert!(parse_structural_query("How do I add a new package?").is_none());
        assert!(parse_structural_query("").is_none());
        assert!(parse_structural_query("   ").is_none());
    }

    #[test]
    fn test_inferred_kind_display() {
        assert_eq!(InferredKind::Functions.display(), "function");
        assert_eq!(InferredKind::Classes.display(), "class");
        assert_eq!(InferredKind::Methods.display(), "method");
        assert_eq!(InferredKind::Impls.display(), "impl");
        assert_eq!(InferredKind::Traits.display(), "trait");
        assert_eq!(InferredKind::Enums.display(), "enum");
        assert_eq!(InferredKind::Interfaces.display(), "interface");
        assert_eq!(InferredKind::Modules.display(), "module");
        assert_eq!(InferredKind::Calls.display(), "call");
        assert_eq!(InferredKind::Imports.display(), "import");
        assert_eq!(InferredKind::Extends.display(), "extends");
    }

    #[test]
    fn test_inferred_kind_from_query_comprehensive() {
        assert_eq!(InferredKind::from_query("find all functions"), Some(InferredKind::Functions));
        assert_eq!(InferredKind::from_query("show all classes"), Some(InferredKind::Classes));
        assert_eq!(InferredKind::from_query("list all methods"), Some(InferredKind::Methods));
        assert_eq!(InferredKind::from_query("find impl blocks"), Some(InferredKind::Impls));
        // Note: from_query maps both "trait" and "interface" to Interfaces
        assert_eq!(InferredKind::from_query("show all traits"), Some(InferredKind::Interfaces));
        assert_eq!(InferredKind::from_query("find all interfaces"), Some(InferredKind::Interfaces));
        assert_eq!(InferredKind::from_query("find all enums"), Some(InferredKind::Enums));
        assert_eq!(InferredKind::from_query("list all modules"), Some(InferredKind::Modules));
        assert_eq!(InferredKind::from_query("find all calls"), Some(InferredKind::Calls));
        assert_eq!(InferredKind::from_query("find all imports"), Some(InferredKind::Imports));
        assert_eq!(InferredKind::from_query("find all extends"), Some(InferredKind::Extends));
    }

    // ── Regression: structural pattern types ───────────────────────────

    #[test]
    fn regression_structural_pattern_ast_grep_has_metadata() {
        let p = parse_structural_query("app.$METHOD($$$)");
        match p {
            Some(StructuralPattern::AstGrepPattern(pat)) => {
                assert!(pat.contains("$METHOD"));
            }
            _ => panic!("should be AstGrepPattern"),
        }
    }

    #[test]
    fn regression_structural_pattern_inferred_has_kind() {
        let p = parse_structural_query("show all interfaces");
        match p {
            Some(StructuralPattern::Inferred(kind)) => {
                assert_eq!(kind, InferredKind::Interfaces);
            }
            _ => panic!("should be Inferred"),
        }
    }

    #[test]
    fn regression_structural_pattern_none_for_non_structural() {
        let non_structural = vec![
            "How do I add a new package?",
            "why does the build fail?",
            "where is Foo implemented?",
            "what is pnpm?",
            "how to create a new package",
        ];
        for query in non_structural {
            assert!(parse_structural_query(query).is_none(), "'{}' should not produce structural pattern", query);
        }
    }
}
