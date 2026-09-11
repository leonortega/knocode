//! AstGrepBackend — stable interface isolating ast-grep behind Knocode abstractions.
//!
//! ## Design
//!
//! ```text
//! StructuralRetriever (knocode-context)
//!         │
//!         ▼
//!     AstGrepBackend trait (this module)
//!         │
//!         ▼
//!     AstGrepImpl (adapter implementation)
//!         │
//!         ▼
//!     ast-grep-core + tree-sitter-language-pack
//! ```
//!
//! The trait defines what structural matching **does**.
//! The implementation defines how ast-grep **does it**.
//! If ast-grep changes its API, only `AstGrepImpl` changes.

use std::collections::HashMap;

/// A single structural match from ast-grep pattern search.
#[derive(Debug, Clone)]
pub struct AstMatch {
    /// Matched source text.
    pub text: String,
    /// Zero-indexed line number.
    pub line: u32,
    /// Zero-indexed column number.
    pub column: u32,
    /// Byte offset of match start in source.
    pub start_byte: u32,
    /// Byte offset of match end in source.
    pub end_byte: u32,
    /// AST node kind (e.g., "function_declaration", "struct_item").
    pub node_kind: String,
    /// Metavariable captures: variable name → matched text.
    /// Ordered by appearance in pattern.
    pub captures: Vec<(String, String)>,
}

impl AstMatch {
    /// Get a capture by variable name.
    pub fn capture(&self, name: &str) -> Option<&str> {
        self.captures.iter()
            .find(|(n, _)| n == name)
            .map(|(_, v)| v.as_str())
    }

    /// Get all captures as a HashMap.
    pub fn captures_map(&self) -> HashMap<&str, &str> {
        self.captures.iter()
            .map(|(n, v)| (n.as_str(), v.as_str()))
            .collect()
    }
}

/// Result of a structural pattern search on a single file.
#[derive(Debug, Clone)]
pub struct AstSearchResult {
    /// All matches found.
    pub matches: Vec<AstMatch>,
    /// Pattern that was used.
    pub pattern: String,
    /// Language used for parsing.
    pub language: String,
}

impl AstSearchResult {
    /// Total number of matches.
    pub fn match_count(&self) -> usize {
        self.matches.len()
    }

    /// Whether any matches were found.
    pub fn has_matches(&self) -> bool {
        !self.matches.is_empty()
    }
}

/// Error from structural pattern search.
#[derive(Debug, Clone)]
pub enum AstSearchError {
    /// The language has no tree-sitter grammar available.
    UnsupportedLanguage(String),
    /// The pattern could not be parsed (syntactically invalid for this language).
    InvalidPattern { pattern: String, language: String, detail: String },
    /// The pattern matched multiple ambiguous AST nodes (ast-grep MultipleNode).
    AmbiguousPattern(String),
    /// Search timed out.
    Timeout { pattern: String, elapsed_ms: u64 },
    /// Internal error.
    Internal(String),
}

impl std::fmt::Display for AstSearchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedLanguage(lang) => write!(f, "unsupported language: {}", lang),
            Self::InvalidPattern { pattern, language, detail } => {
                write!(f, "invalid pattern '{}' for {}: {}", pattern, language, detail)
            }
            Self::AmbiguousPattern(pattern) => write!(f, "ambiguous pattern: {}", pattern),
            Self::Timeout { pattern, elapsed_ms } => {
                write!(f, "timeout after {}ms for pattern: {}", elapsed_ms, pattern)
            }
            Self::Internal(msg) => write!(f, "internal error: {}", msg),
        }
    }
}

impl std::error::Error for AstSearchError {}

/// Stable interface for structural pattern matching via ast-grep.
///
/// Implementors isolate the unstable ast-grep Rust API. The rest of Knocode
/// depends only on this trait, never on ast-grep directly.
pub trait AstGrepBackend {
    /// Search source code for a structural pattern.
    ///
    /// Returns all matches with captures, positions, and node kinds.
    /// On error, returns an `AstSearchError` describing what went wrong.
    fn search(
        &self,
        pattern: &str,
        source: &str,
    ) -> Result<AstSearchResult, AstSearchError>;

    /// Check if this backend supports the given language.
    fn supports_language(&self, lang_pack_name: &str) -> bool;

    /// Get the language name this backend was configured for.
    fn language_name(&self) -> &str;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ast_match_capture_lookup() {
        let m = AstMatch {
            text: "fn foo() {}".into(),
            line: 0,
            column: 0,
            start_byte: 0,
            end_byte: 12,
            node_kind: "function_item".into(),
            captures: vec![("NAME".into(), "foo".into())],
        };
        assert_eq!(m.capture("NAME"), Some("foo"));
        assert_eq!(m.capture("MISSING"), None);
    }

    #[test]
    fn ast_search_result_api() {
        let r = AstSearchResult {
            matches: vec![],
            pattern: "fn $NAME() {}".into(),
            language: "rust".into(),
        };
        assert_eq!(r.match_count(), 0);
        assert!(!r.has_matches());
    }

    #[test]
    fn ast_search_error_display() {
        let e = AstSearchError::UnsupportedLanguage("mojo".into());
        assert!(e.to_string().contains("mojo"));

        let e = AstSearchError::InvalidPattern {
            pattern: "fn {{{".into(),
            language: "rust".into(),
            detail: "unexpected token".into(),
        };
        assert!(e.to_string().contains("fn {{{"));
    }
}
