//! RetrievalQuery — normalized input to the Retrieval Engine.

use std::sync::OnceLock;

use super::intent::{QueryIntent, detect_intent};

/// Normalized query for the Retrieval Engine.
/// Deterministic and independently testable without an LLM.
/// FIX #6: Caches intent to avoid recomputing detect_intent() multiple times per query.
#[derive(Debug, Clone)]
pub struct RetrievalQuery {
    pub text: String,
    pub repository_id: String,
    pub language: Option<String>,
    /// Lazily computed intent — avoids calling detect_intent() 2x per query.
    cached_intent: OnceLock<QueryIntent>,
}

impl RetrievalQuery {
    pub fn new(text: impl Into<String>, repository_id: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            repository_id: repository_id.into(),
            language: None,
            cached_intent: OnceLock::new(),
        }
    }

    /// Get the cached intent, computing it on first access.
    pub fn intent(&self) -> QueryIntent {
        *self.cached_intent.get_or_init(|| detect_intent(&self.text))
    }

    pub fn with_language(mut self, lang: impl Into<String>) -> Self {
        let l = lang.into();
        if !l.is_empty() {
            self.language = Some(l);
        }
        self
    }

    /// Effective text for Tantivy query sanitization (empty check)
    pub fn is_empty(&self) -> bool {
        self.text.trim().is_empty()
    }
}
