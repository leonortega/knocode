//! Structural Query Planner — converts natural language → ast-grep patterns.
//!
//! ## Architecture
//!
//! ```text
//! Query (natural language or explicit pattern)
//!   ↓
//! StructuralIntent (what kind of code structure?)
//!   ↓
//! QueryPlanner (selects/generates patterns per language)
//!   ↓
//! Vec<StructuralPattern> (ready for ast-grep execution)
//! ```
//!
//! The planner is deterministic and independently testable. It never
//! calls ast-grep itself — it only produces patterns.

use crate::retrieval::structural::InferredKind;

// ── StructuralIntent ──────────────────────────────────────────────────

/// High-level intent for structural search.
///
/// More specific than `InferredKind` — captures both the structural
/// kind and the query context (explicit pattern vs. inferred).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructuralIntent {
    /// User provided an explicit ast-grep pattern (contains `$` metavariables).
    ExplicitPattern(String),
    /// User wants to find declarations of a specific kind.
    FindDeclarations(InferredKind),
    /// User wants to find call sites or usages.
    FindCalls,
    /// User wants to find imports/requires.
    FindImports,
    /// User wants to find inheritance/implementation relationships.
    FindRelationships,
    /// No structural intent detected.
    None,
}

impl StructuralIntent {
    /// Parse query text into a structural intent.
    pub fn from_query(query: &str) -> Self {
        let q = query.trim();

        // Explicit pattern: contains `$` metavariables
        if q.contains('$') {
            return Self::ExplicitPattern(q.to_string());
        }

        // Inferred from natural language
        if let Some(kind) = InferredKind::from_query(q) {
            match kind {
                InferredKind::Calls => Self::FindCalls,
                InferredKind::Imports => Self::FindImports,
                InferredKind::Extends => Self::FindRelationships,
                other => Self::FindDeclarations(other),
            }
        } else {
            Self::None
        }
    }

    /// Whether this intent requires structural retrieval.
    pub fn is_structural(&self) -> bool {
        !matches!(self, Self::None)
    }
}

// ── StructuralQuery ───────────────────────────────────────────────────

/// A fully resolved structural query ready for pattern generation.
///
/// Created by `QueryPlanner` from a raw query string. Contains all the
/// information needed to execute a structural search.
#[derive(Debug, Clone)]
pub struct StructuralQuery {
    /// The original query text.
    pub raw_query: String,
    /// The resolved intent.
    pub intent: StructuralIntent,
    /// Target language (if known from file extension or query context).
    pub language: Option<String>,
    /// Maximum number of results to return.
    pub max_results: usize,
    /// Timeout per file in milliseconds.
    pub timeout_per_file_ms: u64,
}

impl StructuralQuery {
    /// Create a structural query with default limits.
    pub fn new(query: impl Into<String>) -> Self {
        let raw = query.into();
        let intent = StructuralIntent::from_query(&raw);
        Self {
            raw_query: raw,
            intent,
            language: None,
            max_results: 100,
            timeout_per_file_ms: 2000,
        }
    }

    /// Set the target language.
    pub fn with_language(mut self, lang: impl Into<String>) -> Self {
        let l = lang.into();
        if !l.is_empty() {
            self.language = Some(l);
        }
        self
    }

    /// Set the maximum number of results.
    pub fn with_max_results(mut self, max: usize) -> Self {
        self.max_results = max;
        self
    }

    /// Set the per-file timeout.
    pub fn with_timeout_ms(mut self, ms: u64) -> Self {
        self.timeout_per_file_ms = ms;
        self
    }
}

// ── StructuralPattern (planner output) ────────────────────────────────

/// A resolved structural pattern ready for ast-grep execution.
///
/// This is the output of `QueryPlanner::plan()`. Each pattern includes
/// the ast-grep pattern string and metadata about what it matches.
#[derive(Debug, Clone)]
pub struct ResolvedPattern {
    /// The ast-grep pattern string (e.g., `"fn $NAME($$$) { $$$ }"`).
    pub pattern: String,
    /// Human-readable description of what this pattern matches.
    pub description: String,
    /// The kind of declaration/usage this pattern targets.
    pub kind: String,
    /// Priority: lower = tried first. Useful when multiple patterns
    /// cover the same structural concept (e.g., Rust functions with/without return type).
    pub priority: u32,
}

// ── QueryPlanner ──────────────────────────────────────────────────────

/// Converts structural queries into resolved patterns for ast-grep.
///
/// The planner is deterministic: same input always produces same output.
/// It does not call ast-grep or access the filesystem.
pub struct QueryPlanner;

impl QueryPlanner {
    /// Plan a structural query into resolved patterns.
    ///
    /// For explicit patterns, returns the pattern as-is.
    /// For inferred intents, generates language-specific patterns.
    pub fn plan(query: &StructuralQuery) -> Vec<ResolvedPattern> {
        match &query.intent {
            StructuralIntent::ExplicitPattern(pattern) => {
                vec![ResolvedPattern {
                    pattern: pattern.clone(),
                    description: format!("explicit pattern: {}", pattern),
                    kind: "explicit".to_string(),
                    priority: 0,
                }]
            }
            StructuralIntent::FindDeclarations(kind) => {
                Self::plan_declarations(kind, query.language.as_deref())
            }
            StructuralIntent::FindCalls => {
                Self::plan_calls(query.language.as_deref())
            }
            StructuralIntent::FindImports => {
                Self::plan_imports(query.language.as_deref())
            }
            StructuralIntent::FindRelationships => {
                Self::plan_relationships(query.language.as_deref())
            }
            StructuralIntent::None => vec![],
        }
    }

    /// Plan declaration patterns (functions, classes, etc.).
    fn plan_declarations(kind: &InferredKind, language: Option<&str>) -> Vec<ResolvedPattern> {
        let display = kind.display();
        let lang = language.unwrap_or("typescript"); // Default to TS

        // Get patterns from the existing lang_patterns_for function
        let patterns = crate::retrieval::structural::InferredKind::to_ast_grep_patterns(kind, lang);

        patterns.into_iter().enumerate().map(|(i, pattern)| {
            ResolvedPattern {
                pattern,
                description: format!("find {} declarations in {}", display, lang),
                kind: display.to_string(),
                priority: i as u32,
            }
        }).collect()
    }

    /// Plan call-site patterns.
    fn plan_calls(language: Option<&str>) -> Vec<ResolvedPattern> {
        let lang = language.unwrap_or("typescript");
        let patterns = crate::retrieval::structural::lang_patterns_for("call", lang);

        patterns.into_iter().enumerate().map(|(i, pattern)| {
            ResolvedPattern {
                pattern,
                description: format!("find function/method calls in {}", lang),
                kind: "call".to_string(),
                priority: i as u32,
            }
        }).collect()
    }

    /// Plan import patterns.
    fn plan_imports(language: Option<&str>) -> Vec<ResolvedPattern> {
        let lang = language.unwrap_or("typescript");
        let patterns = crate::retrieval::structural::lang_patterns_for("import", lang);

        patterns.into_iter().enumerate().map(|(i, pattern)| {
            ResolvedPattern {
                pattern,
                description: format!("find import statements in {}", lang),
                kind: "import".to_string(),
                priority: i as u32,
            }
        }).collect()
    }

    /// Plan inheritance/implementation patterns.
    fn plan_relationships(language: Option<&str>) -> Vec<ResolvedPattern> {
        let lang = language.unwrap_or("typescript");
        let extends = crate::retrieval::structural::lang_patterns_for("extends", lang);
        let implements = crate::retrieval::structural::lang_patterns_for("implements", lang);

        let extends_len = extends.len();
        let mut patterns: Vec<ResolvedPattern> = Vec::new();
        for (i, pattern) in extends.into_iter().enumerate() {
            patterns.push(ResolvedPattern {
                pattern,
                description: format!("find extends relationships in {}", lang),
                kind: "extends".to_string(),
                priority: i as u32,
            });
        }
        for (i, pattern) in implements.into_iter().enumerate() {
            patterns.push(ResolvedPattern {
                pattern,
                description: format!("find implements relationships in {}", lang),
                kind: "implements".to_string(),
                priority: (extends_len + i) as u32,
            });
        }
        patterns
    }

    /// Validate a pattern by checking it against minimal source.
    ///
    /// Returns `Ok(())` if the pattern is syntactically valid for the language,
    /// or `Err(detail)` with a description of what went wrong.
    pub fn validate_pattern(
        pattern: &str,
        language: &str,
    ) -> Result<(), String> {
        let adapter = crate::retrieval::structural::adapter_for_ext(language)
            .ok_or_else(|| format!("unsupported language: {}", language))?;

        adapter.validate_pattern(pattern).map_err(|e| e.to_string())
    }
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── StructuralIntent tests ─────────────────────────────────────────

    #[test]
    fn intent_explicit_pattern() {
        let intent = StructuralIntent::from_query("fn $NAME() { }");
        assert_eq!(intent, StructuralIntent::ExplicitPattern("fn $NAME() { }".to_string()));
        assert!(intent.is_structural());
    }

    #[test]
    fn intent_find_functions() {
        let intent = StructuralIntent::from_query("find all functions in the codebase");
        assert_eq!(intent, StructuralIntent::FindDeclarations(InferredKind::Functions));
        assert!(intent.is_structural());
    }

    #[test]
    fn intent_find_classes() {
        let intent = StructuralIntent::from_query("show all classes");
        assert_eq!(intent, StructuralIntent::FindDeclarations(InferredKind::Classes));
    }

    #[test]
    fn intent_find_calls() {
        let intent = StructuralIntent::from_query("find all calls to foo");
        assert_eq!(intent, StructuralIntent::FindCalls);
    }

    #[test]
    fn intent_find_imports() {
        let intent = StructuralIntent::from_query("find all imports");
        assert_eq!(intent, StructuralIntent::FindImports);
    }

    #[test]
    fn intent_find_extends() {
        let intent = StructuralIntent::from_query("find all classes extending Base");
        assert_eq!(intent, StructuralIntent::FindRelationships);
    }

    #[test]
    fn intent_none_for_procedural() {
        let intent = StructuralIntent::from_query("How do I add a new package?");
        assert_eq!(intent, StructuralIntent::None);
        assert!(!intent.is_structural());
    }

    #[test]
    fn intent_none_for_debugging() {
        let intent = StructuralIntent::from_query("why does the build fail?");
        assert_eq!(intent, StructuralIntent::None);
    }

    // ── StructuralQuery tests ──────────────────────────────────────────

    #[test]
    fn query_defaults() {
        let q = StructuralQuery::new("find all functions");
        assert_eq!(q.raw_query, "find all functions");
        assert!(q.language.is_none());
        assert_eq!(q.max_results, 100);
        assert_eq!(q.timeout_per_file_ms, 2000);
    }

    #[test]
    fn query_with_language() {
        let q = StructuralQuery::new("fn $NAME() {}")
            .with_language("rust");
        assert_eq!(q.language.as_deref(), Some("rust"));
    }

    #[test]
    fn query_with_max_results() {
        let q = StructuralQuery::new("find all functions")
            .with_max_results(50);
        assert_eq!(q.max_results, 50);
    }

    #[test]
    fn query_with_timeout() {
        let q = StructuralQuery::new("find all functions")
            .with_timeout_ms(500);
        assert_eq!(q.timeout_per_file_ms, 500);
    }

    // ── QueryPlanner tests ─────────────────────────────────────────────

    #[test]
    fn plan_explicit_pattern() {
        let q = StructuralQuery::new("fn $NAME() { }");
        let patterns = QueryPlanner::plan(&q);
        assert_eq!(patterns.len(), 1);
        assert_eq!(patterns[0].pattern, "fn $NAME() { }");
        assert_eq!(patterns[0].kind, "explicit");
        assert_eq!(patterns[0].priority, 0);
    }

    #[test]
    fn plan_find_functions_rust() {
        let q = StructuralQuery::new("find all functions").with_language("rust");
        let patterns = QueryPlanner::plan(&q);
        assert!(!patterns.is_empty());
        assert!(patterns.iter().any(|p| p.kind == "function"));
        // Rust functions need both with/without return type
        assert!(patterns.len() >= 2);
    }

    #[test]
    fn plan_find_functions_python() {
        let q = StructuralQuery::new("find all functions").with_language("python");
        let patterns = QueryPlanner::plan(&q);
        assert!(!patterns.is_empty());
        assert!(patterns.iter().any(|p| p.pattern.contains("def")));
    }

    #[test]
    fn plan_find_classes() {
        let q = StructuralQuery::new("show all classes").with_language("typescript");
        let patterns = QueryPlanner::plan(&q);
        assert!(!patterns.is_empty());
        assert!(patterns.iter().any(|p| p.kind == "class"));
    }

    #[test]
    fn plan_find_calls() {
        let q = StructuralQuery::new("find all calls to foo").with_language("rust");
        let patterns = QueryPlanner::plan(&q);
        assert!(!patterns.is_empty());
        assert!(patterns.iter().any(|p| p.kind == "call"));
    }

    #[test]
    fn plan_find_imports() {
        let q = StructuralQuery::new("find all imports").with_language("python");
        let patterns = QueryPlanner::plan(&q);
        assert!(!patterns.is_empty());
        assert!(patterns.iter().any(|p| p.kind == "import"));
    }

    #[test]
    fn plan_find_extends() {
        let q = StructuralQuery::new("find all classes extending Base").with_language("typescript");
        let patterns = QueryPlanner::plan(&q);
        assert!(!patterns.is_empty());
        assert!(patterns.iter().any(|p| p.kind == "extends"));
    }

    #[test]
    fn plan_no_structural_for_procedural() {
        let q = StructuralQuery::new("How do I add a new package?");
        let patterns = QueryPlanner::plan(&q);
        assert!(patterns.is_empty());
    }

    #[test]
    fn plan_no_structural_for_debugging() {
        let q = StructuralQuery::new("why does the build fail?");
        let patterns = QueryPlanner::plan(&q);
        assert!(patterns.is_empty());
    }

    #[test]
    fn plan_multiple_patterns_for_complex_query() {
        // "find all function declarations" should produce multiple patterns
        // for languages with multiple function forms (e.g., Rust)
        let q = StructuralQuery::new("find all function declarations").with_language("rust");
        let patterns = QueryPlanner::plan(&q);
        assert!(patterns.len() >= 2, "Rust should have with/without return type patterns");
    }

    #[test]
    fn plan_pattern_metadata() {
        let q = StructuralQuery::new("fn $NAME() { }");
        let patterns = QueryPlanner::plan(&q);
        let p = &patterns[0];
        assert!(!p.description.is_empty());
        assert!(!p.pattern.is_empty());
    }

    // ── Regression tests ───────────────────────────────────────────────

    #[test]
    fn regression_explicit_beats_inferred() {
        // Explicit $NAME pattern should produce ExplicitPattern, not FindDeclarations
        let q = StructuralQuery::new("app.$METHOD($$$)");
        assert!(matches!(q.intent, StructuralIntent::ExplicitPattern(_)));
    }

    #[test]
    fn regression_procedural_no_structural() {
        // "How do I add a new package?" must never trigger structural search
        let q = StructuralQuery::new("How do I add a new package?");
        assert_eq!(q.intent, StructuralIntent::None);
        assert!(QueryPlanner::plan(&q).is_empty());
    }

    #[test]
    fn regression_debugging_no_structural() {
        let q = StructuralQuery::new("Why does the build fail?");
        assert_eq!(q.intent, StructuralIntent::None);
        assert!(QueryPlanner::plan(&q).is_empty());
    }

    #[test]
    fn regression_navigation_no_structural() {
        let q = StructuralQuery::new("where is Foo implemented?");
        assert_eq!(q.intent, StructuralIntent::None);
        assert!(QueryPlanner::plan(&q).is_empty());
    }
}
