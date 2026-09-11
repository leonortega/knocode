//! QueryIntent — tiny deterministic intent, no LLM/embeddings.
//! Distinguishes *what kind* of evidence is authoritative for the task.

/// Retrieval intent — determines candidate pools and authority.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum QueryIntent {
    /// How-to / steps / procedural docs: "How do I add a new package?"
    Procedural,
    /// Factual / concept explanation
    Informational,
    /// Build/implement a feature
    Implementation,
    /// Why does X fail / error / bug
    Debugging,
    /// How are packages tested / test coverage
    Testing,
    /// Configure / install / env
    Configuration,
    /// Where is X / workspace organization / structure
    Architecture,
    /// Find/locate a file
    Navigation,
    /// Structural pattern: "fn $FUNC($$$) { $$$ }", "find all functions"
    Structural,
}

impl std::fmt::Display for QueryIntent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Self::Procedural => "procedural",
            Self::Informational => "informational",
            Self::Implementation => "implementation",
            Self::Debugging => "debugging",
            Self::Testing => "testing",
            Self::Configuration => "configuration",
            Self::Architecture => "architecture",
            Self::Navigation => "navigation",
            Self::Structural => "structural",
        };
        write!(f, "{}", s)
    }
}

/// Deterministic intent from cheap query patterns.
/// Order matters — procedural how-to beats generic implementation.
pub fn detect_intent(query: &str) -> QueryIntent {
    let q = query.to_lowercase();

    // Testing intent — explicit (must beat procedural for "How do I run package tests?")
    if q.contains("dtslint") || (q.contains("test") && (q.contains("how") || q.contains("run") || q.contains("coverage") || q.contains("spec"))) {
        return QueryIntent::Testing;
    }
    if q.contains("spec") && q.contains("test") {
        return QueryIntent::Testing;
    }
    if q == "how are packages tested" || q.contains("how are packages tested") || q.contains("testing") && q.contains("package") {
        return QueryIntent::Testing;
    }
    if q.contains("how are") && q.contains("tested") {
        return QueryIntent::Testing;
    }

    let is_procedural = q.contains("how do i")
        || q.contains("how to")
        || q.contains("steps to")
        || q.contains("getting started")
        || q.contains("set up")
        || q.contains("setup")
        || (q.contains("how") && (q.contains("add") || q.contains("create") || q.contains("install")));

    // Explicit procedural verbs without "how" still procedural when docs-oriented
    let has_create_add = q.contains("add a new package")
        || q.contains("create a new package")
        || q.contains("create a package")
        || q.contains("add package");

    if is_procedural || has_create_add {
        return QueryIntent::Procedural;
    }

    // Debugging
    if q.contains("why does") || q.contains("why is") || q.contains("fail") || q.contains("error") || q.contains("bug") || q.contains("broken") || q.contains("fix") {
        return QueryIntent::Debugging;
    }

    // Configuration
    if q.contains("configure") || q.contains("configuration") || q.contains("pnpm-workspace") || q.contains("workspace") && q.contains("organiz") {
        // disambiguate: "how is workspace organized" is Architecture, not Configuration
        if q.contains("organiz") || q.contains("structure") || q.contains("layout") {
            return QueryIntent::Architecture;
        }
        return QueryIntent::Configuration;
    }

    // Structural: S-expression patterns or explicit structural keywords (must be before Navigation)
    if q.contains('(') || q.contains("find all function") || q.contains("find all class")
        || q.contains("find all method") || q.contains("show all function")
        || q.contains("find all struct") || q.contains("find all enum")
        || q.contains("show all class") || q.contains("show all struct")
        || q.contains("list all function") || q.contains("list all class")
    {
        return QueryIntent::Structural;
    }

    // Architecture / navigation
    if q.contains("where is") || q.contains("where are") || q.contains("located") || q.contains("find ") || q.contains("locate") {
        return QueryIntent::Navigation;
    }
    if q.contains("organiz") || q.contains("structure") || q.contains("architecture") || q.contains("layout") {
        return QueryIntent::Architecture;
    }

    // Implementation (code)
    if q.contains("implement") || q.contains("build") || q.contains("develop") {
        return QueryIntent::Implementation;
    }

    // Fallback: if query explicitly mentions test/spec without how-to, still testing-ish
    if q.contains("dtslint") || q.contains(" running tests") {
        return QueryIntent::Testing;
    }

    QueryIntent::Informational
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn procedural_how_do_i_add_package() {
        assert_eq!(detect_intent("How do I add a new package?"), QueryIntent::Procedural);
        assert_eq!(detect_intent("how to create a new package"), QueryIntent::Procedural);
        assert_eq!(detect_intent("steps to add a package"), QueryIntent::Procedural);
        assert_eq!(detect_intent("Create a new package"), QueryIntent::Procedural);
    }

    #[test]
    fn testing_intent() {
        assert_eq!(detect_intent("How are packages tested?"), QueryIntent::Testing);
        assert_eq!(detect_intent("How do I run package tests?"), QueryIntent::Testing);
        assert_eq!(detect_intent("dtslint package"), QueryIntent::Testing);
    }

    #[test]
    fn debugging_intent() {
        assert_eq!(detect_intent("Why does package X fail?"), QueryIntent::Debugging);
        assert_eq!(detect_intent("fix the build error"), QueryIntent::Debugging);
    }

    #[test]
    fn architecture_intent() {
        assert_eq!(detect_intent("How is the workspace organized?"), QueryIntent::Architecture);
        assert_eq!(detect_intent("where is package X implemented?"), QueryIntent::Navigation);
        assert_eq!(detect_intent("where are types located"), QueryIntent::Navigation);
    }

    #[test]
    fn informational_fallback() {
        assert_eq!(detect_intent("what is pnpm"), QueryIntent::Informational);
    }

    #[test]
    fn structural_pattern() {
        assert_eq!(detect_intent("fn $FUNC($$$) { $$$ }"), QueryIntent::Structural);
        assert_eq!(detect_intent("find all functions in the codebase"), QueryIntent::Structural);
        assert_eq!(detect_intent("show all classes"), QueryIntent::Structural);
    }
}
