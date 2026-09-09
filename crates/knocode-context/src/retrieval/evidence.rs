//! Evidence types — the stable contract between Retrieval and Context.
//! Replaces `Vec<(PathBuf,f32)>` with explainable evidence.

use std::path::PathBuf;

/// Why this file was ranked where it was — for diagnostics.
#[derive(Debug, Clone, PartialEq)]
pub enum RetrievalSignal {
    /// Raw Tantivy BM25 score (pre-boost)
    TantivyScore(f32),
    /// Symbol-match boost: `1.0 + (matched/total)*weight`
    SymbolMatch { matched: usize, total: usize, boost: f32 },
    /// Title field match
    TitleBoost(f32),
    /// Path field match
    PathBoost(f32),
    /// File-class boost (Documentation 1.4 etc.)
    FileClassBoost { class: String, boost: f32 },
    /// Directory/location boost
    DirectoryBoost(f32),
    /// Test penalty when query not test-related (0.6x)
    TestPenalty(f32),
    /// Test boost when query is test-related (1.4x)
    TestBoost(f32),
    /// Code-behind pairing (0.8x of view score)
    CodeBehindPenalty(f32),
    /// Graph connectivity boost (1.2x)
    GraphBoost(f32),
    /// Filename field boost
    FilenameBoost(f32),
    /// Symbols field boost
    SymbolsBoost(f32),
    /// Intent-aware class boost (relevance × intent authority)
    IntentBoost { intent: String, boost: f32 },
    /// Documentation authority prior (separate from relevance)
    DocAuthority(f32),
    /// Vocabulary expansion (add→create)
    QueryExpansion(f32),
    /// Doc prior damping: generic meta-doc with no query-token path match had
    /// its FULL score multiplied by `factor` (generic docs outranked code files
    /// via stacked priors plus broad-content BM25).
    DocPriorDamping { factor: f32 },
    /// Structural/AST pattern match (ast-grep-core or tree-sitter fallback)
    StructuralMatch { pattern: String, score: f32 },
}

impl std::fmt::Display for RetrievalSignal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::TantivyScore(s) => write!(f, "tantivy:{:.2}", s),
            Self::SymbolMatch { matched, total, boost } => write!(f, "symbol_match:{}/{} boost={:.2}", matched, total, boost),
            Self::TitleBoost(b) => write!(f, "title_boost:{:.2}", b),
            Self::PathBoost(b) => write!(f, "path_boost:{:.2}", b),
            Self::FileClassBoost { class, boost } => write!(f, "file_class:{}:{:.2}", class, boost),
            Self::DirectoryBoost(b) => write!(f, "directory_boost:{:.2}", b),
            Self::TestPenalty(b) => write!(f, "test_penalty:{:.2}", b),
            Self::TestBoost(b) => write!(f, "test_boost:{:.2}", b),
            Self::CodeBehindPenalty(b) => write!(f, "code_behind:{:.2}", b),
            Self::GraphBoost(b) => write!(f, "graph_boost:{:.2}", b),
            Self::FilenameBoost(b) => write!(f, "filename_boost:{:.2}", b),
            Self::SymbolsBoost(b) => write!(f, "symbols_boost:{:.2}", b),
            Self::IntentBoost { intent, boost } => write!(f, "intent:{}:{:.2}", intent, boost),
            Self::DocAuthority(b) => write!(f, "doc_authority:{:.2}", b),
            Self::QueryExpansion(b) => write!(f, "query_expansion:{:.2}", b),
            Self::DocPriorDamping { factor } => write!(f, "doc_prior_damping:{:.2}", factor),
            Self::StructuralMatch { pattern, score } => write!(f, "structural_match:{}:{:.2}", pattern, score),
        }
    }
}

/// Kind of evidence — determines how Context Engine budgets it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceKind {
    Code,
    Documentation,
    Config,
    Test,
    Other(String),
}

impl EvidenceKind {
    pub fn from_file_class(file_class: &str) -> Self {
        match file_class {
            "Source" => Self::Code,
            "Documentation" => Self::Documentation,
            "Config" => Self::Config,
            "Test" => Self::Test,
            other => Self::Other(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::Code => "code",
            Self::Documentation => "docs",
            Self::Config => "config",
            Self::Test => "test",
            Self::Other(s) => s.as_str(),
        }
    }
}

/// Where this evidence came from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EvidenceSource {
    Tantivy,
    Symbol,
    Graph,
    CodeBehind,
    Ripgrep,
    Hint,
    Structural,
}

impl std::fmt::Display for EvidenceSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Tantivy => write!(f, "tantivy"),
            Self::Symbol => write!(f, "symbol"),
            Self::Graph => write!(f, "graph"),
            Self::CodeBehind => write!(f, "code-behind"),
            Self::Ripgrep => write!(f, "ripgrep"),
            Self::Hint => write!(f, "hint"),
            Self::Structural => write!(f, "structural"),
        }
    }
}

/// A single piece of ranked evidence — the unit Context consumes.
#[derive(Debug, Clone)]
pub struct Evidence {
    pub path: PathBuf,
    pub score: f32,
    pub kind: EvidenceKind,
    pub signals: Vec<RetrievalSignal>,
    pub matched_symbols: Vec<String>,
    pub matched_terms: Vec<String>,
    pub source: EvidenceSource,
    /// Original file class string (for compatibility)
    pub file_class: String,
    /// Tantivy raw score before boosts (for explain)
    pub raw_score: f32,
    /// Line number (from tantivy/symbol search)
    pub line: usize,
    // ── Structural match fields (P0.3) ──
    /// Column number (0-indexed, from ast-grep position).
    pub column: Option<usize>,
    /// Language that produced this match (e.g., "rust", "typescript").
    pub language: Option<String>,
    /// AST node kind (e.g., "function_declaration", "struct_item").
    pub match_kind: Option<String>,
    /// Structured metavariable captures: variable name → matched text.
    /// For structural matches; empty for lexical/symbol evidence.
    pub captures: Vec<(String, String)>,
}

impl Evidence {
    pub fn new(path: impl Into<PathBuf>, score: f32, file_class: impl Into<String>) -> Self {
        let fc = file_class.into();
        let kind = EvidenceKind::from_file_class(&fc);
        Self {
            path: path.into(),
            score,
            kind,
            signals: Vec::new(),
            matched_symbols: Vec::new(),
            matched_terms: Vec::new(),
            source: EvidenceSource::Tantivy,
            file_class: fc,
            raw_score: score,
            line: 1,
            column: None,
            language: None,
            match_kind: None,
            captures: Vec::new(),
        }
    }

    /// Human-readable explain string: `README.md score:8.42 signals: title+2.50 path+2.50 ...`
    pub fn explain(&self) -> String {
        let signals = self.signals.iter().map(|s| s.to_string()).collect::<Vec<_>>().join(" ");
        let structural = if let Some(ref kind) = self.match_kind {
            let lang = self.language.as_deref().unwrap_or("?");
            format!(" lang:{} kind:{}", lang, kind)
        } else {
            String::new()
        };
        let caps = if self.captures.is_empty() {
            String::new()
        } else {
            let pairs: Vec<String> = self.captures.iter().map(|(n, v)| format!("{}={}", n, v)).collect();
            format!(" captures:[{}]", pairs.join(", "))
        };
        format!(
            "{} score:{:.2} raw:{:.2} kind:{:?} source:{} line:{}{}{} signals:[{}] terms:{:?}",
            self.path.display(),
            self.score,
            self.raw_score,
            self.kind,
            self.source,
            self.line,
            structural,
            caps,
            signals,
            self.matched_terms
        )
    }
}

/// Stable contract between Retrieval and Context.
#[derive(Debug, Clone)]
pub struct RetrievalResult {
    pub evidence: Vec<Evidence>,
    pub status: knocode_core::RetrievalStatus,
    pub diagnostics: RetrievalDiagnostics,
}

impl RetrievalResult {
    pub fn empty(status: knocode_core::RetrievalStatus) -> Self {
        Self {
            evidence: Vec::new(),
            status,
            diagnostics: RetrievalDiagnostics::default(),
        }
    }
}

/// Per-backend metrics — timing, candidate count, match count, and status.
/// Every retrieval backend exposes the same shape for observability.
#[derive(Debug, Clone, Default)]
pub struct BackendMetrics {
    /// Backend name ("tantivy", "symbol", "structural", "graph").
    pub backend: String,
    /// Query or pattern executed.
    pub query: String,
    /// Language used (if applicable).
    pub language: Option<String>,
    /// Number of candidates considered.
    pub candidate_count: usize,
    /// Number of matches returned.
    pub match_count: usize,
    /// Duration in milliseconds.
    pub duration_ms: u64,
    /// Status: "ok", "timeout", "error", "skipped".
    pub status: String,
}

impl BackendMetrics {
    pub fn new(backend: impl Into<String>) -> Self {
        Self {
            backend: backend.into(),
            ..Default::default()
        }
    }

    /// Format as a single-line summary for logging.
    pub fn summary(&self) -> String {
        format!(
            "{} {}ms {} candidates {} matches [{}]",
            self.backend, self.duration_ms, self.candidate_count, self.match_count, self.status
        )
    }
}

/// Per-retrieval diagnostics (timings + counts) — extends `knocode_core::RetrievalDiagnostic`.
#[derive(Debug, Clone, Default)]
pub struct RetrievalDiagnostics {
    pub candidate_count: usize,
    pub filtered_count: usize,
    pub tantivy_ms: u64,
    pub ranking_ms: u64,
    pub graph_ms: u64,
    pub structural_ms: u64,
    pub doc_count: usize,
    pub candidate_k: usize,
    pub max_files: usize,
    /// Per-backend metrics for observability.
    pub backends: Vec<BackendMetrics>,
}

impl RetrievalDiagnostics {
    /// Human-readable summary of all backend metrics.
    pub fn summary(&self) -> String {
        let lines: Vec<String> = self.backends.iter().map(|b| b.summary()).collect();
        let total_ms: u64 = self.backends.iter().map(|b| b.duration_ms).sum();
        let mut out = lines.join("\n");
        out.push_str(&format!("\nTotal {}ms", total_ms));
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evidence_explain_contains_signals() {
        let mut e = Evidence::new("README.md", 8.42, "Documentation");
        e.raw_score = 0.72;
        e.signals.push(RetrievalSignal::TitleBoost(2.5));
        e.signals.push(RetrievalSignal::FileClassBoost { class: "Documentation".into(), boost: 1.4 });
        let s = e.explain();
        assert!(s.contains("README.md"));
        assert!(s.contains("title_boost"));
        assert!(s.contains("file_class"));
    }

    #[test]
    fn evidence_kind_from_file_class() {
        assert_eq!(EvidenceKind::from_file_class("Source"), EvidenceKind::Code);
        assert_eq!(EvidenceKind::from_file_class("Documentation"), EvidenceKind::Documentation);
        assert_eq!(EvidenceKind::from_file_class("Test"), EvidenceKind::Test);
    }

    #[test]
    fn backend_metrics_summary() {
        let m = BackendMetrics {
            backend: "tantivy".into(),
            query: "auth".into(),
            language: Some("rust".into()),
            candidate_count: 87,
            match_count: 12,
            duration_ms: 45,
            status: "ok".into(),
        };
        let s = m.summary();
        assert!(s.contains("tantivy"));
        assert!(s.contains("45ms"));
        assert!(s.contains("87 candidates"));
        assert!(s.contains("12 matches"));
        assert!(s.contains("ok"));
    }

    #[test]
    fn backend_metrics_default() {
        let m = BackendMetrics::new("structural");
        assert_eq!(m.backend, "structural");
        assert_eq!(m.candidate_count, 0);
        assert_eq!(m.match_count, 0);
        assert_eq!(m.duration_ms, 0);
        assert!(m.status.is_empty());
    }

    #[test]
    fn diagnostics_summary_includes_all_backends() {
        let d = RetrievalDiagnostics {
            backends: vec![
                BackendMetrics { backend: "tantivy".into(), duration_ms: 120, candidate_count: 87, match_count: 15, ..Default::default() },
                BackendMetrics { backend: "structural".into(), duration_ms: 45, candidate_count: 12, match_count: 3, ..Default::default() },
            ],
            ..Default::default()
        };
        let s = d.summary();
        assert!(s.contains("tantivy"));
        assert!(s.contains("structural"));
        assert!(s.contains("Total 165ms"));
    }
}
