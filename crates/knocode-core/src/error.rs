use std::fmt;

use serde::{Deserialize, Serialize};

/// Result type alias for knocode operations
pub type Result<T> = std::result::Result<T, KnocodeError>;

/// Top-level error type for the AI Runtime
#[derive(Debug, thiserror::Error)]
pub enum KnocodeError {
    #[error("configuration error: {0}")]
    Config(#[from] ConfigError),

    #[error("timeout: {0}")]
    Timeout(String),

    #[error("invalid request: {0}")]
    InvalidRequest(String),

    #[error("repository index not ready: {0}")]
    IndexNotReady(String),

    #[error("context build failed: {0}")]
    ContextBuildFailed(String),

    #[error("knowledge retrieval failed: {0}")]
    KnowledgeRetrievalFailed(String),

    #[error("database error: {0}")]
    DatabaseError(String),

    #[error("index error: {0}")]
    IndexError(String),

    #[error("memory unavailable: {0}")]
    MemoryUnavailable(String),

    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),

    #[error("serialization error: {0}")]
    Serialization(String),
}

/// Configuration-specific errors
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file '{path}': {source}")]
    FileReadError {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("failed to parse config: {0}")]
    ParseError(String),

    #[error("invalid value for '{field}': {message}")]
    InvalidValue { field: String, message: String },

    #[error("failed to serialize config: {0}")]
    SerializeError(String),
}

impl From<toml::de::Error> for ConfigError {
    fn from(e: toml::de::Error) -> Self {
        ConfigError::ParseError(e.to_string())
    }
}

impl From<toml::ser::Error> for ConfigError {
    fn from(e: toml::ser::Error) -> Self {
        ConfigError::SerializeError(e.to_string())
    }
}

/// A correlation ID for tracing requests through the system
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct CorrelationId(String);

impl CorrelationId {
    /// Create a new correlation ID with the `req_` prefix
    pub fn new() -> Self {
        Self(format!("req_{}", uuid::Uuid::new_v4()))
    }

    /// Create from an existing string
    pub fn from_string(id: String) -> Self {
        Self(id)
    }

    /// Get the inner string
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for CorrelationId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl Default for CorrelationId {
    fn default() -> Self {
        Self::new()
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_correlation_id_format() {
        let id = CorrelationId::new();
        assert!(id.as_str().starts_with("req_"));
        assert!(id.as_str().len() > 4);
    }

    #[test]
    fn test_correlation_id_from_string() {
        let id = CorrelationId::from_string("req_abc123".to_string());
        assert_eq!(id.as_str(), "req_abc123");
    }

    #[test]
    fn test_correlation_id_display() {
        let id = CorrelationId::new();
        assert_eq!(format!("{}", id), id.as_str());
    }

    #[test]
    fn test_correlation_id_default() {
        let id = CorrelationId::default();
        assert!(id.as_str().starts_with("req_"));
    }

    #[test]
    fn test_knocode_error_display() {
        let err = KnocodeError::Timeout("request exceeded 30s".to_string());
        assert!(err.to_string().contains("timeout"));
    }

    #[test]
    fn test_config_error_display() {
        let err = ConfigError::InvalidValue {
            field: "test.field".to_string(),
            message: "must be positive".to_string(),
        };
        assert!(err.to_string().contains("test.field"));
        assert!(err.to_string().contains("must be positive"));
    }
}
