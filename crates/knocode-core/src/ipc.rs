use serde::{Deserialize, Serialize};

use crate::error::CorrelationId;

// ── Hook Types ──────────────────────────────────────────────────────────

/// Types of hooks that agents can register
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum HookType {
    PreGeneration,
    PreToolCall,
    /// Readiness probe — the daemon answers without touching the engine
    Probe,
}

// ── Request Types ───────────────────────────────────────────────────────

/// Message sent from agent to daemon — includes repository_id + timestamp for full trace (TASK-021)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentRequest {
    pub correlation_id: CorrelationId,
    pub hook_type: HookType,
    pub payload: RequestPayload,
    /// repository_id: hash of repo_path (TASK-021)
    #[serde(default)]
    pub repository_id: String,
    /// ISO8601 timestamp of request creation (TASK-021)
    #[serde(default)]
    pub timestamp: String,
}

/// Payload variants for incoming requests
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "PascalCase")]
pub enum RequestPayload {
    /// Pre-generation: enrich a user message with context
    MessageRewrite {
        session_id: String,
        message: String,
        context_hints: Option<ContextHints>,
        /// Absolute path of the agent's workspace root (TASK-036) — overrides daemon CWD for scoping
        #[serde(default)]
        repository_path: Option<String>,
    },
    /// Pre-tool: compress tool output before sending to LLM
    ToolOutput {
        tool_name: String,
        output_type: OutputType,
        content: String,
        context: Option<String>,
        /// Absolute path of the agent's workspace root (TASK-036)
        #[serde(default)]
        repository_path: Option<String>,
    },
    /// Lightweight readiness probe — the daemon answers `state`/`index_files`/`version`
    /// without locking the engine or building context. UDS/MessagePack parity with
    /// `GET /health`; clients poll until `state == "ready"` before sending requests.
    Probe,
}

// ── Response Types ──────────────────────────────────────────────────────

/// Message sent from daemon to agent
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentResponse {
    pub correlation_id: CorrelationId,
    pub hook_type: HookType,
    pub payload: ResponsePayload,
    pub latency_ms: u64,
    pub error: Option<String>,
}

/// Data for RewrittenMessage response
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RewrittenMessageData {
    pub original: String,
    pub rewritten: String,
    pub context_pack: Option<ContextPack>,
}

/// Payload variants for outgoing responses
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "PascalCase")]
pub enum ResponsePayload {
    /// Enriched message with context pack and routing decision
    RewrittenMessage(Box<RewrittenMessageData>),
    /// Compressed tool output
    CompressedOutput {
        original: String,
        compressed: String,
        original_tokens: usize,
        compressed_tokens: usize,
    },
    /// Pass-through on error/timeout (fail-open)
    OriginalPassthrough {
        original: String,
        reason: String,
    },
    /// Readiness probe answer — parity with `GET /health`
    Probe {
        state: String,
        index_files: usize,
        version: String,
    },
}

// ── Supporting Types ────────────────────────────────────────────────────

/// Hints from the agent about the current context
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextHints {
    pub files_mentioned: Option<Vec<String>>,
    pub language: Option<String>,
}

/// Output type of a tool call
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "PascalCase")]
pub enum OutputType {
    FileRead,
    SearchResult,
    ShellOutput,
    Other,
}

// ── Retrieval Diagnostic ─────────────────────────────────────────────────

/// Why a specific expected file was not retrieved — enables per-query diagnosis
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissType {
    /// File was never indexed (not in Tantivy or SQLite)
    NotIndexed,
    /// File text doesn't contain any query keywords (BM25 miss)
    LexicalMiss,
    /// File symbols don't match query terms (symbol index miss)
    SymbolMiss,
    /// Tree-sitter couldn't parse the file's language
    LanguageParserMiss,
    /// File isn't connected to any retrieved file via dependency graph
    GraphMiss,
    /// Query analysis produced wrong keywords for this file
    QueryExpansionMiss,
    /// File was retrieved but ranked too low (below top-k)
    RankedTooLow,
    /// Unknown / not yet classified
    Unclassified,
}

/// Per-file diagnostic entry: why was this expected file missed?
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileDiagnostic {
    /// The expected file path
    pub path: String,
    /// Whether this file was actually retrieved
    pub retrieved: bool,
    /// If retrieved, at what rank (1-indexed)
    pub rank: Option<usize>,
    /// If missed, the classified reason
    pub miss_type: Option<MissType>,
    /// Human-readable explanation
    pub explanation: String,
}

/// Diagnostic report for a single retrieval query — emitted when KNOCODE_RETRIEVAL_DIAG=1
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RetrievalDiagnostic {
    /// The query string
    pub query: String,
    /// Total number of results returned by the pipeline
    pub results_returned: usize,
    /// Number of expected files that were retrieved
    pub expected_found: usize,
    /// Total expected files
    pub expected_total: usize,
    /// Per-file diagnostics
    pub files: Vec<FileDiagnostic>,
    /// Stage-level metrics (all durations in ms)
    pub bm25_duration_ms: u64,
    pub symbol_duration_ms: u64,
    pub graph_duration_ms: u64,
    pub merge_duration_ms: u64,
    /// Number of files indexed in the repo
    pub files_indexed: usize,
    /// Number of symbols in the index
    pub symbols_indexed: usize,
}

impl RetrievalDiagnostic {
    /// Pretty-print the diagnostic to stderr
    pub fn print(&self) {
        eprintln!("\n══ Retrieval Diagnostic ══");
        eprintln!("Query: \"{}\"", self.query);
        eprintln!("Results returned: {}", self.results_returned);
        eprintln!("Expected files: {}/{} found", self.expected_found, self.expected_total);
        eprintln!("Index: {} files, {} symbols", self.files_indexed, self.symbols_indexed);
        eprintln!("Timing: BM25={}ms, Symbols={}ms, Graph={}ms, Merge={}ms",
            self.bm25_duration_ms, self.symbol_duration_ms,
            self.graph_duration_ms, self.merge_duration_ms);
        eprintln!();
        // Group by status
        let mut found: Vec<&FileDiagnostic> = self.files.iter().filter(|f| f.retrieved).collect();
        let mut missed: Vec<&FileDiagnostic> = self.files.iter().filter(|f| !f.retrieved).collect();
        found.sort_by_key(|f| f.rank.unwrap_or(usize::MAX));
        missed.sort_by_key(|f| format!("{:?}", f.miss_type));
        if !found.is_empty() {
            eprintln!("  ✓ Retrieved:");
            for f in &found {
                eprintln!("    #{} {}", f.rank.unwrap_or(0), f.path);
            }
        }
        if !missed.is_empty() {
            eprintln!("  ✗ Missed:");
            for f in &missed {
                let tag = match &f.miss_type {
                    Some(MissType::NotIndexed) => "NOT_INDEXED",
                    Some(MissType::LexicalMiss) => "LEXICAL_MISS",
                    Some(MissType::SymbolMiss) => "SYMBOL_MISS",
                    Some(MissType::LanguageParserMiss) => "LANGUAGE_PARSER_MISS",
                    Some(MissType::GraphMiss) => "GRAPH_MISS",
                    Some(MissType::QueryExpansionMiss) => "QUERY_EXPANSION_MISS",
                    Some(MissType::RankedTooLow) => "RANKED_TOO_LOW",
                    Some(MissType::Unclassified) | None => "UNCLASSIFIED",
                };
                eprintln!("    [{}] {} — {}", tag, f.path, f.explanation);
            }
        }
        eprintln!("══════════════════════════════════\n");
    }
}

/// Status of a retrieval operation — distinguishes "no match" from "retrieval failed"
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum RetrievalStatus {
    /// Results were found
    Found(usize),
    /// Search ran successfully but found nothing
    #[default]
    NoMatch,
    /// No index exists for this repository (init never ran or index was deleted)
    IndexNotBuilt,
    /// Index exists but is empty or unreachable
    IndexUnavailable,
    /// Tree-sitter grammars failed to load for some languages
    ParserFailed(Vec<String>),
    /// Knowledge Hub is not initialized or unavailable
    KnowledgeHubUnavailable,
    /// Search threw an error (e.g. query parse failure)
    RetrievalFailed(String),
    /// Used a fallback method (e.g. ripgrep after Tantivy miss)
    FallbackUsed(String),
}

/// The assembled context pack returned to agents — stable artifact (TASK-008)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextPack {
    pub docs_context: String,
    pub code_context: String,
    pub token_usage: TokenUsage,
    /// Provenance for each included item (TASK-009) — why it was selected
    #[serde(default)]
    pub provenance: Vec<ContextProvenance>,
    /// Determinism metadata: task hash + repo state hash
    #[serde(default)]
    pub metadata: ContextMetadata,
    /// Repository state (git HEAD hash) for determinism/stability (TASK-008)
    #[serde(default)]
    pub repository_state: String,
    /// Retrieval status for code search — tells callers WHY results are empty
    #[serde(default)]
    pub code_retrieval_status: RetrievalStatus,
    /// Per-query retrieval diagnostic (when KNOCODE_RETRIEVAL_DIAG=1)
    #[serde(default)]
    pub retrieval_diagnostic: Option<RetrievalDiagnostic>,
    /// Retrieval-stage stats (search only, excluding packing) — always computed,
    /// lets callers record retrieval-vs-packing latency without extra plumbing.
    #[serde(default)]
    pub retrieval_stats: Option<RetrievalStats>,
}

/// Retrieval-stage timing + candidate count (search only, excluding context packing).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default)]
pub struct RetrievalStats {
    /// Code-search stage duration in ms (the dominant retrieval stage)
    pub retrieval_ms: u64,
    /// Candidate results returned by the retrieval pipeline before packing/budgeting
    pub candidates: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ContextMetadata {
    pub task_hash: String,
    pub correlation_id: String,
    pub cache_order: Vec<String>,
    /// git HEAD hash of the repository state at build time (TASK-008/009)
    #[serde(default)]
    pub repository_state: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextProvenance {
    pub path: String,
    pub source: String, // "code" | "docs" | "skills"
    pub retriever: String, // "tantivy" | "skill_engine" | "bm25"
    pub score: f64,
    pub reason: String,
}

/// Token usage breakdown
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub total_tokens: usize,
    pub budget_remaining: usize,
    pub by_source: std::collections::HashMap<String, usize>,
}

// ── Task Types ──────────────────────────────────────────────────────────

/// A task request for context building
#[derive(Debug, Clone)]
pub struct TaskRequest {
    pub message: String,
    pub session_id: String,
    pub context_hints: Option<ContextHints>,
    /// Repository identity (hash of repo path) for scoped retrieval — TASK-030
    pub repository_id: String,
    /// Agent workspace root path — TASK-036 (daemon may serve multiple repos)
    pub repository_path: Option<String>,
    /// Expected files for diagnostic classification (benchmarks only, not sent over the wire)
    pub expected_files: Option<Vec<String>>,
}

impl TaskRequest {
    /// Convenience constructor with no repository scoping (daemon-CWD fallback)
    pub fn new(message: impl Into<String>, session_id: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            session_id: session_id.into(),
            context_hints: None,
            repository_id: String::new(),
            repository_path: None,
            expected_files: None,
        }
    }
}

// ── Repository Identity ─────────────────────────────────────────────────

/// Shared repository identity helper: SHA-256 of the repo path, first 12 hex chars.
/// Used by the daemon (per-request resolution, TASK-036) and repo-intel (index stamping, TASK-030)
/// so both sides derive identical ids from the same path.
pub fn repository_id_from_path(path: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(path.as_bytes());
    format!("{:x}", h.finalize())[..12].to_string()
}

// ── Search Types ────────────────────────────────────────────────────────

/// A single search result
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub path: String,
    pub line: usize,
    pub content: String,
    pub score: f64,
}

/// Collection of search results
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResults {
    pub results: Vec<SearchResult>,
    pub total_count: usize,
}

// ── Knowledge Types ─────────────────────────────────────────────────────

/// A knowledge entry
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KnowledgeEntry {
    pub id: Option<i64>,
    pub category: String,
    pub key: String,
    pub value: String,
    pub confidence: f64,
    pub source: String,
    pub relevance_score: Option<f64>,
}

// ── Code Types ──────────────────────────────────────────────────────────

/// A code file in the context pack
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeFile {
    pub path: String,
    pub content: String,
    pub language: String,
    pub line_range: (usize, usize),
    pub token_count: usize,
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_agent_request_serialization() {
        let req = AgentRequest {
            correlation_id: CorrelationId::new(),
            hook_type: HookType::PreGeneration,
            payload: RequestPayload::MessageRewrite {
                session_id: "sess_123".to_string(),
                message: "implement auth".to_string(),
                context_hints: Some(ContextHints {
                    files_mentioned: Some(vec!["src/auth.rs".to_string()]),
                    language: Some("rust".to_string()),
                }),
                repository_path: Some("C:\\repos\\demo".to_string()),
            },
            repository_id: "test_repo_hash".to_string(),
            timestamp: "2026-08-25T00:00:00Z".to_string(),
        };
        let json = serde_json::to_string(&req).unwrap();
        let parsed: AgentRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req.correlation_id, parsed.correlation_id);
        assert_eq!(req.hook_type, parsed.hook_type);
    }

    #[test]
    fn test_request_payload_repository_path_backcompat() {
        // Old payloads without repository_path still deserialize (TASK-036)
        let json = r#"{"type":"MessageRewrite","session_id":"s","message":"m"}"#;
        let parsed: RequestPayload = serde_json::from_str(json).unwrap();
        match parsed {
            RequestPayload::MessageRewrite { repository_path, .. } => assert!(repository_path.is_none()),
            _ => panic!("expected MessageRewrite"),
        }
    }

    #[test]
    fn test_repository_id_from_path_stable() {
        let a = crate::ipc::repository_id_from_path("C:\\repos\\demo");
        let b = crate::ipc::repository_id_from_path("C:\\repos\\demo");
        let c = crate::ipc::repository_id_from_path("C:\\repos\\other");
        assert_eq!(a, b);
        assert_ne!(a, c);
        assert_eq!(a.len(), 12);
    }

    #[test]
    fn test_probe_roundtrip() {
        // Request: unit variant — {"type":"Probe"}
        let req = RequestPayload::Probe;
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"Probe\""));
        let parsed: RequestPayload = serde_json::from_str(&json).unwrap();
        assert!(matches!(parsed, RequestPayload::Probe));
        assert_eq!(serde_json::to_string(&HookType::Probe).unwrap(), "\"Probe\"");

        // Response: state/index_files/version payload
        let resp = ResponsePayload::Probe {
            state: "indexing".to_string(),
            index_files: 0,
            version: "0.9.0".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ResponsePayload = serde_json::from_str(&json).unwrap();
        match parsed {
            ResponsePayload::Probe { state, index_files, version } => {
                assert_eq!(state, "indexing");
                assert_eq!(index_files, 0);
                assert_eq!(version, "0.9.0");
            }
            _ => panic!("Expected Probe"),
        }
    }

    #[test]
    fn test_response_payload_passthrough() {
        let resp = ResponsePayload::OriginalPassthrough {
            original: "hello".to_string(),
            reason: "timeout".to_string(),
        };
        let json = serde_json::to_string(&resp).unwrap();
        let parsed: ResponsePayload = serde_json::from_str(&json).unwrap();
        match parsed {
            ResponsePayload::OriginalPassthrough { original, reason } => {
                assert_eq!(original, "hello");
                assert_eq!(reason, "timeout");
            }
            _ => panic!("Expected OriginalPassthrough"),
        }
    }

    #[test]
    fn test_output_type_serialization() {
        let types = [
            OutputType::FileRead,
            OutputType::SearchResult,
            OutputType::ShellOutput,
            OutputType::Other,
        ];
        for t in &types {
            let json = serde_json::to_string(t).unwrap();
            let parsed: OutputType = serde_json::from_str(&json).unwrap();
            assert_eq!(*t, parsed);
        }
    }

    #[test]
    fn test_context_pack_serialization() {
        let pack = ContextPack {
            docs_context: "docs section".to_string(),
            code_context: "code section".to_string(),
            token_usage: TokenUsage {
                total_tokens: 8000,
                budget_remaining: 4000,
                by_source: [
                    ("docs".to_string(), 2000),
                    ("code".to_string(), 5000),
                ]
                .into(),
            },
            provenance: vec![],
            metadata: ContextMetadata::default(),
            repository_state: String::new(),
            code_retrieval_status: RetrievalStatus::Found(5),
            retrieval_diagnostic: None,
            retrieval_stats: None,
        };
        let json = serde_json::to_string(&pack).unwrap();
        let parsed: ContextPack = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.token_usage.total_tokens, 8000);
    }
}
