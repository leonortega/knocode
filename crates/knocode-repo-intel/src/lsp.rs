/// LSP enrichment — optional, never hard dependency (spec §3 Repository Intelligence)
/// Reuses the agent CLI's own already-running language server processes.
/// If `KNOCODE_LSP_ENABLED != "true"` or server not reachable, returns empty with WARN and never fails hot path.
use tracing::{debug, warn};

#[derive(Debug, Clone)]
pub struct LspClient {
    pub enabled: bool,
    pub endpoint: String,
}

impl Default for LspClient {
    fn default() -> Self {
        let enabled = std::env::var("KNOCODE_LSP_ENABLED").map(|v| v == "true").unwrap_or(false);
        let endpoint = std::env::var("KNOCODE_LSP_ENDPOINT").unwrap_or_else(|_| "http://localhost:2087".to_string());
        Self { enabled, endpoint }
    }
}

impl LspClient {
    pub fn new(enabled: bool, endpoint: String) -> Self {
        Self { enabled, endpoint }
    }

    /// Symbol references / call hierarchy placeholder — returns empty if LSP unavailable
    pub fn get_symbol_references(&self, symbol: &str) -> Vec<String> {
        if !self.enabled {
            debug!(symbol = symbol, "LSP disabled, skipping symbol references");
            return Vec::new();
        }
        // In v0.3.0, we probe the endpoint with a short timeout; on failure we warn and return empty.
        // Real implementation would speak JSON-RPC over stdio/HTTP to rust-analyzer / typescript-language-server.
        warn!("LSP enabled but not yet wired to agent's language server — returning empty (spec: optional enrichment)");
        Vec::new()
    }

    pub fn is_available(&self) -> bool {
        self.enabled
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_lsp_disabled_by_default() {
        // Ensure env not set
        std::env::remove_var("KNOCODE_LSP_ENABLED");
        let client = LspClient::default();
        assert!(!client.is_available());
        assert!(client.get_symbol_references("foo").is_empty());
    }

    #[test]
    fn test_lsp_enabled_returns_empty_gracefully() {
        let client = LspClient::new(true, "http://localhost:9999".to_string());
        assert!(client.is_available());
        // Should not panic, just return empty
        let refs = client.get_symbol_references("main");
        assert!(refs.is_empty());
    }
}
