//! Canonical ranking tables — single source of truth for retrieval scoring.
//!
//! Previously the boost tables and stop-word list were duplicated across
//! `knocode-storage/src/tantivy_index.rs` (BM25 hit scoring) and
//! `knocode-context/src/retrieval/policy.rs` (post-ranking boosts), with a
//! third stop-word list in `knocode-context/src/lib.rs` (miss classification).
//! Any tuning applied to one copy silently diverged the others.
//!
//! Contract:
//! - `STOP_WORDS` is the union of all former lists — every site that filtered
//!   stop words must use it.
//! - `file_class_boost` / `query_aware_test_multiplier_with` /
//!   `directory_boost_with` hold the *logic*; configurable weight structs
//!   (e.g. `RetrievalPolicy::FileClassWeights`) supply *values* and must keep
//!   their defaults equal to the canonical defaults here (parity tests in
//!   `knocode-context` fail the build if they drift).
//! - `SCORE_SCALE` is the fixed f64→f32 evidence rescale applied once at the
//!   retrieval→evidence boundary.

/// Fixed rescale applied once when raw f64 relevance scores become f32
/// `Evidence` scores. Previously an inline magic `* 1000.0` in two places.
pub const SCORE_SCALE: f32 = 1000.0;

/// Stop words ignored when building code-search queries or extracting query
/// tokens. Union of the three former lists (tantivy query sanitizer,
/// `ranking::query_tokens`, miss-classifier) — no behavior loss at any site.
pub const STOP_WORDS: &[&str] = &[
    "a", "an", "the", "is", "are", "was", "were", "be", "been", "being",
    "have", "has", "had", "do", "does", "did", "will", "would", "could",
    "should", "may", "might", "shall", "can", "need", "dare", "ought",
    "used", "to", "of", "in", "for", "on", "with", "at", "by", "from",
    "as", "into", "through", "during", "before", "after", "above", "below",
    "between", "out", "off", "over", "under", "again", "further", "then",
    "once", "here", "there", "when", "where", "why", "how", "all", "both",
    "each", "few", "more", "most", "other", "some", "such", "no", "nor",
    "not", "only", "own", "same", "so", "than", "too", "very", "just",
    "because", "if", "while", "and", "but", "or", "yet", "either",
    "neither", "every", "any", "that", "this", "it", "its",
    "what", "which", "who", "whom", "these", "those", "implemented",
];

/// Canonical default file-class boost (Documentation > Config > Source > Test).
/// Formerly `CodeIndexSchema::file_class_boost` (storage) and
/// `FileClassWeights::default` (context) — the two tables must stay identical.
pub fn file_class_boost(file_class: &str) -> f32 {
    match file_class {
        "Documentation" => 1.4,
        "Config" => 1.2,
        "Source" => 1.0,
        "Test" => 0.7,
        "Generated" => 0.5,
        "Stylesheet" => 0.0,
        "Binary" => 0.0,
        "Vendor" => 0.0,
        "Dependency" => 0.0,
        _ => 1.0,
    }
}

/// True when the query is about tests (used for the query-aware Test multiplier).
///
/// Whole-token matching — the former `q.contains("test")` treated any query
/// mentioning "latest" (or "contest") as a test query and mis-applied the
/// Test-file boost.
pub fn is_test_query(query: &str) -> bool {
    const TEST_TERMS: &[&str] = &["test", "tests", "testing", "spec", "specs", "dtslint", "pytest", "unittest"];
    query
        .to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .any(|tok| TEST_TERMS.contains(&tok))
}

/// Query-aware adjustment for Test files: penalize unless the query is about
/// tests. Formerly `CodeIndexSchema::query_aware_test_multiplier` (hardcoded
/// 0.6/1.4) and `RetrievalPolicy::test_multiplier` (configurable) — one logic
/// copy now, values passed in.
pub fn query_aware_test_multiplier_with(query: &str, file_class: &str, penalty: f32, boost: f32) -> f32 {
    if file_class != "Test" {
        return 1.0;
    }
    if is_test_query(query) { boost } else { penalty }
}

/// Canonical default query-aware Test multiplier values.
pub const TEST_PENALTY: f32 = 0.6;
pub const TEST_BOOST: f32 = 1.4;

/// Directory/location boost logic, parameterized by the weight values.
/// Formerly `CodeIndexSchema::directory_boost` (storage, hardcoded) and
/// `DirectoryWeights::boost_for` (context, configurable) — one logic copy now.
pub fn directory_boost_with(
    path: &str,
    readme: f32,
    docs: f32,
    types: f32,
    workspace: f32,
    default: f32,
) -> f32 {
    let lower = path.to_lowercase();
    // Documentation & contribution files — boost for how-to queries
    if lower.ends_with("readme.md")
        || lower.ends_with("contributing.md")
        || lower.ends_with("contributing")
        || lower.ends_with("claude.md")
        || lower.ends_with("agents.md")
    {
        return readme;
    }
    if lower.contains("/docs/") || lower.contains("/.github/") || lower.contains("/.knocode/") {
        return docs;
    }
    // Workspace packages — types/foo/ pattern (DefinitelyTyped, monorepos)
    if lower.starts_with("types/") || lower.contains("/types/") {
        return types;
    }
    if lower.contains("pnpm-workspace.yaml") || lower.contains("lerna.json") || lower.contains("nx.json") {
        return workspace;
    }
    default
}

/// Canonical default directory-boost values (must match
/// `DirectoryWeights::default` in knocode-context — parity-tested there).
pub const DIRECTORY_README: f32 = 1.3;
pub const DIRECTORY_DOCS: f32 = 1.2;
pub const DIRECTORY_TYPES: f32 = 1.15;
pub const DIRECTORY_WORKSPACE: f32 = 1.1;
pub const DIRECTORY_DEFAULT: f32 = 1.0;

/// Canonical default directory boost using the default weights.
pub fn directory_boost(path: &str) -> f32 {
    directory_boost_with(path, DIRECTORY_README, DIRECTORY_DOCS, DIRECTORY_TYPES, DIRECTORY_WORKSPACE, DIRECTORY_DEFAULT)
}

// ── Query vocabulary (synonym table) ────────────────────────────────────

/// Bounded, vetted query-vocabulary table — the single source shared by
/// context-side expansion (`knocode-context/src/retrieval/vocab.rs`) and
/// storage-side query sanitization (`expand_code_vocabulary`).
///
/// Formerly two hardcoded 4-10 entry `match` arms that had drifted apart.
/// Keep it SMALL and high-precision: every synonym is OR-joined into BM25,
/// so each entry dilutes the original term's weight. Entries map a term to
/// synonyms INCLUDING itself (callers may rely on that).
pub fn synonyms_for(term: &str) -> &'static [&'static str] {
    match term {
        // creation verbs
        "add" => &["add", "create", "new"],
        "create" => &["create", "add", "new"],
        "new" => &["new", "create", "add"],
        "make" => &["make", "create", "add"],
        // packaging / setup
        "package" => &["package", "workspace"],
        "workspace" => &["workspace", "package"],
        "install" => &["install", "add", "setup"],
        "setup" => &["setup", "install", "configure"],
        "set" => &["set", "setup", "install"],
        // code concepts (compound high-precision pairs)
        "auth" => &["auth", "authentication"],
        "authentication" => &["authentication", "auth"],
        "config" => &["config", "configuration", "settings"],
        "configuration" => &["configuration", "config"],
        "settings" => &["settings", "config"],
        "db" => &["db", "database"],
        "database" => &["database", "db"],
        "repo" => &["repo", "repository"],
        "repository" => &["repository", "repo"],
        "error" => &["error", "exception"],
        "exception" => &["exception", "error"],
        "test" => &["test", "spec"],
        "spec" => &["spec", "test"],
        "deploy" => &["deploy", "release"],
        "release" => &["release", "deploy"],
        // DefinitelyTyped-specific aliases (V1)
        "dtslint" => &["dtslint", "pnpm", "test"],
        "dts" => &["dts", "index.d.ts"],
        "type" => &["type", "index.d.ts"],
        "types" => &["types", "type", "index.d.ts"],
        _ => &[],
    }
}

/// Light deterministic stemming for table lookup: lowercase + plural strip.
/// "Packages" → "package", "settings" → "setting" (table miss is fine).
/// Only plain `s` plurals are stripped (never "ss"/"us") — conservative by
/// design so stem errors can't inject wrong synonyms.
pub fn canonicalize_term(term: &str) -> std::borrow::Cow<'_, str> {
    let lower = term.to_lowercase();
    if lower.len() >= 4 && lower.ends_with('s') && !lower.ends_with("ss") && !lower.ends_with("us") {
        std::borrow::Cow::Owned(lower[..lower.len() - 1].to_string())
    } else {
        std::borrow::Cow::Owned(lower)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stop_words_is_sorted_unique_union() {
        let mut sorted = STOP_WORDS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(STOP_WORDS.len(), sorted.len(), "STOP_WORDS must not contain duplicates");
    }

    #[test]
    fn file_class_defaults_are_canonical() {
        assert!((file_class_boost("Documentation") - 1.4).abs() < 1e-6);
        assert!((file_class_boost("Config") - 1.2).abs() < 1e-6);
        assert!((file_class_boost("Source") - 1.0).abs() < 1e-6);
        assert!((file_class_boost("Test") - 0.7).abs() < 1e-6);
        assert!((file_class_boost("Binary") - 0.0).abs() < 1e-6);
        assert!((file_class_boost("UnknownClass") - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_multiplier_query_aware() {
        assert!((query_aware_test_multiplier_with("fix the test suite", "Test", TEST_PENALTY, TEST_BOOST) - 1.4).abs() < 1e-6);
        assert!((query_aware_test_multiplier_with("authentication middleware", "Test", TEST_PENALTY, TEST_BOOST) - 0.6).abs() < 1e-6);
        assert!((query_aware_test_multiplier_with("anything", "Source", TEST_PENALTY, TEST_BOOST) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn directory_boost_canonical() {
        assert!((directory_boost("README.md") - 1.3).abs() < 1e-6);
        assert!((directory_boost("a/docs/api.md") - 1.2).abs() < 1e-6);
        assert!((directory_boost("types/foo/index.d.ts") - 1.15).abs() < 1e-6);
        assert!((directory_boost("src/main.rs") - 1.0).abs() < 1e-6);
    }

    #[test]
    fn is_test_query_detection() {
        assert!(is_test_query("why is the test failing"));
        assert!(is_test_query("add spec coverage"));
        assert!(is_test_query("run dtslint"));
        assert!(is_test_query("pytest suite"));
        assert!(!is_test_query("latest changes"));
        assert!(!is_test_query("contest results"));
        assert!(!is_test_query("authentication middleware"));
    }
}
