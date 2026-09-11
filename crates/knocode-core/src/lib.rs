// knocode-core: shared types, errors, and configuration for the AI Runtime

pub mod config;
pub mod error;
pub mod ipc;
pub mod ranking;
pub mod secrets;
pub mod traits;

// Re-export commonly used types
pub use config::{Config, WatchMode};
pub use error::{KnocodeError, ConfigError, CorrelationId, Result};
pub use ranking::{
    SCORE_SCALE, STOP_WORDS, TEST_BOOST, TEST_PENALTY,
    DIRECTORY_DEFAULT, DIRECTORY_DOCS, DIRECTORY_README, DIRECTORY_TYPES, DIRECTORY_WORKSPACE,
    directory_boost, file_class_boost, is_test_query,
    query_aware_test_multiplier_with,
};
pub use secrets::{contains_secret, redact_secrets};
pub use traits::IContextBuilder;
pub use ipc::{
    AgentRequest, AgentResponse, CodeFile, ContextHints, ContextPack, HookType,
    KnowledgeEntry, OutputType, RequestPayload, ResponsePayload, RetrievalStatus,
    RewrittenMessageData, SearchResult, SearchResults,
    TaskRequest, TokenUsage, FileDiagnostic, MissType, RetrievalDiagnostic,
    repository_id_from_path,
};
