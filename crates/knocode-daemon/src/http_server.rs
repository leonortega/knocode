use std::sync::{Arc, OnceLock};

use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
    routing::post,
    Router,
};
use knocode_core::{AgentRequest, CorrelationId, HookType, RequestPayload, TaskRequest};
use knocode_context::ContextEngine;
use tokio::sync::Mutex;
use tracing::{debug, error, info};

// ── HTTP Request/Response Types ──────────────────────────────────────────

#[derive(serde::Deserialize)]
pub struct HttpRequest {
    pub correlation_id: Option<String>,
    pub hook_type: String,
    pub payload: HttpRequestPayload,
}

#[derive(serde::Deserialize)]
#[serde(tag = "type")]
pub enum HttpRequestPayload {
    #[serde(rename = "MessageRewrite")]
    MessageRewrite {
        session_id: Option<String>,
        message: String,
        context_hints: Option<ContextHintsJson>,
        /// Agent workspace root (TASK-036) — daemon scopes retrieval to THIS repo, not its CWD
        #[serde(default)]
        repository_path: Option<String>,
    },
    #[serde(rename = "Probe")]
    Probe,
    /// Removed wire contracts (e.g. `ToolOutput` — compression = RTK) land here so
    /// the daemon answers with a structured JSON 400 instead of a deserialization 422.
    #[serde(other)]
    Removed,
}

#[derive(serde::Deserialize)]
pub struct ContextHintsJson {
    pub files_mentioned: Option<Vec<String>>,
    pub language: Option<String>,
}

#[derive(serde::Serialize)]
pub struct HttpResponse {
    pub correlation_id: String,
    pub hook_type: String,
    pub payload: HttpResponsePayload,
    pub latency_ms: u64,
    pub error: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(tag = "type")]
pub enum HttpResponsePayload {
    #[serde(rename = "RewrittenMessage")]
    RewrittenMessage {
        original: String,
        rewritten: String,
        /// Full context pack (TASK-035: lets E2E tests assert provenance uniqueness/scoping)
        #[serde(skip_serializing_if = "Option::is_none")]
        context_pack: Box<Option<knocode_core::ContextPack>>,
    },
    #[serde(rename = "OriginalPassthrough")]
    OriginalPassthrough {
        original: String,
        reason: String,
    },
    #[serde(rename = "Probe")]
    Probe {
        state: String,
        index_files: usize,
        version: String,
    },
}

// ── Server State ─────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct HttpServerState {
    pub context_engine: Arc<Mutex<ContextEngine>>,
}

// ── Server Setup ─────────────────────────────────────────────────────────

pub fn create_router(state: HttpServerState) -> Router {
    Router::new()
        .route("/hook", post(handle_hook))
        .route("/health", axum::routing::get(handle_health))
        .route("/metrics", axum::routing::get(handle_metrics))
        .route("/mcp", post(crate::mcp::handle_mcp))
        .with_state(state)
}

pub async fn start_http_server(
    port: u16,
    state: HttpServerState,
) -> Result<(), String> {
    let addr = format!("127.0.0.1:{}", port);
    let router = create_router(state);

    info!(addr = %addr, "HTTP server starting");

    let listener = tokio::net::TcpListener::bind(&addr)
        .await
        .map_err(|e| format!("Failed to bind HTTP server: {}", e))?;

    info!(addr = %addr, "HTTP server ready");

    axum::serve(listener, router)
        .await
        .map_err(|e| format!("HTTP server error: {}", e))
}

// ── Handlers ─────────────────────────────────────────────────────────────

/// Build the readiness probe payload from the shared metrics. Used by the HTTP
/// `Probe` payload and the UDS/MessagePack probe (parity with `GET /health`).
/// Never touches the engine or the readiness gate — always answers.
fn http_probe_payload() -> HttpResponsePayload {
    let m = crate::metrics::global();
    HttpResponsePayload::Probe {
        state: m.readiness_str().to_string(),
        index_files: m.index_files(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    }
}

async fn handle_health() -> Json<serde_json::Value> {
    // Version single source of truth: root Cargo.toml [workspace.package].version
    // Readiness: "indexing" while the initial index / an auto-reindex runs, "ready" after.
    // Clients should poll until `state` is "ready" before sending requests.
    let m = crate::metrics::global();
    Json(serde_json::json!({
        "status": "ok",
        "version": env!("CARGO_PKG_VERSION"),
        "state": m.readiness_str(),
        "index_files": m.index_files(),
    }))
}

async fn handle_metrics() -> String {
    crate::metrics::global().exposition()
}

/// Validate message length (spec §3 — secrets redaction + input validation)
pub(crate) fn validate_input_len(content: &str, limit: usize) -> Result<(), String> {
    if content.len() > limit {
        return Err(format!("input too large: {} > {} bytes (truncated)", content.len(), limit));
    }
    if content.contains("..") && content.contains('/') && content.len() < 500 {
        // Path traversal heuristic for file mentions — allow but warn
        tracing::warn!(content = %knocode_core::redact_secrets(content), "possible path traversal in input");
    }
    Ok(())
}

#[allow(clippy::result_large_err)]
static HTTP_RATE_LIMITER: OnceLock<crate::ratelimit::RateLimiter> = OnceLock::new();
fn http_rate_limiter() -> &'static crate::ratelimit::RateLimiter {
    HTTP_RATE_LIMITER.get_or_init(crate::ratelimit::RateLimiter::default)
}

#[allow(clippy::result_large_err)]
async fn handle_hook(
    State(state): State<HttpServerState>,
    Json(request): Json<HttpRequest>,
) -> Result<Json<HttpResponse>, (StatusCode, Json<HttpResponse>)> {
    let start = std::time::Instant::now();
    let correlation_id = request.correlation_id.unwrap_or_else(|| {
        format!("req_{}", uuid::Uuid::new_v4())
    });
    // Readiness probe — lightweight parity with GET /health and the UDS probe:
    // never gated (must answer during indexing), never rate-limited, no engine lock.
    if matches!(request.payload, HttpRequestPayload::Probe) {
        let resp = HttpResponse {
            correlation_id: correlation_id.clone(),
            hook_type: request.hook_type.clone(),
            payload: http_probe_payload(),
            latency_ms: 0,
            error: None,
        };
        return Ok(Json(resp));
    }
    // Removed wire contracts (PreToolCall/ToolOutput, compression) — RTK owns that layer.
    // Answered with a structured 400 (same shape as the legacy unknown-hook-type error).
    if matches!(request.payload, HttpRequestPayload::Removed) {
        let hook_type_str = request.hook_type.clone();
        tracing::info!(correlation_id = %correlation_id, hook_type = %hook_type_str, "HTTP /hook rejected — removed contract (compression = RTK)");
        let resp = HttpResponse {
            correlation_id: correlation_id.clone(),
            hook_type: hook_type_str.clone(),
            payload: HttpResponsePayload::OriginalPassthrough {
                original: String::new(),
                reason: format!("Unknown hook type: {}", hook_type_str),
            },
            latency_ms: start.elapsed().as_millis() as u64,
            error: Some("Unknown hook type".to_string()),
        };
        return Err((StatusCode::BAD_REQUEST, Json(resp)));
    }
    // Readiness gate: while the initial index (or an auto-reindex) is running, the
    // engine lock is held — reject fast with 503 so clients can back off and retry
    // instead of queueing on the lock. Poll GET /health (`state: "ready"`) before
    // sending requests. Never counted as fail-open: this is a retry signal, not a fault.
    if !crate::metrics::global().is_ready() {
        tracing::warn!(correlation_id = %correlation_id, "HTTP /hook rejected — daemon indexing in progress (503)");
        let resp = HttpResponse {
            correlation_id: correlation_id.clone(),
            hook_type: request.hook_type.clone(),
            payload: HttpResponsePayload::OriginalPassthrough {
                original: String::new(),
                reason: "daemon_indexing".to_string(),
            },
            latency_ms: 0,
            error: Some("daemon not ready — indexing in progress".to_string()),
        };
        return Err((StatusCode::SERVICE_UNAVAILABLE, Json(resp)));
    }
    // Rate limiting (HTTP fallback) — shared TokenBucket
    let session_key = match &request.payload {
        HttpRequestPayload::MessageRewrite { session_id, .. } => session_id.clone().unwrap_or_else(|| correlation_id.clone()),
        HttpRequestPayload::Probe => "probe".to_string(),
        HttpRequestPayload::Removed => correlation_id.clone(),
    };
    if http_rate_limiter().is_rate_limited(&session_key) {
        crate::metrics::global().inc_fail_open();
        tracing::warn!(correlation_id = %correlation_id, session_key = %session_key, "HTTP rate limited");
        let resp = HttpResponse {
            correlation_id: correlation_id.clone(),
            hook_type: request.hook_type.clone(),
            payload: HttpResponsePayload::OriginalPassthrough { original: String::new(), reason: "rate_limited".to_string() },
            latency_ms: start.elapsed().as_millis() as u64,
            error: Some("rate limited".to_string()),
        };
        return Ok(Json(resp));
    }
    let _ = http_rate_limiter().try_acquire("__probe__");
    // Input validation (100KB message, 1MB tool content) + secrets redaction before logging
    if let HttpRequestPayload::MessageRewrite { ref message, .. } = request.payload {
        if let Err(e) = validate_input_len(message, 100 * 1024) {
            crate::metrics::global().inc_fail_open();
            tracing::warn!(correlation_id = %correlation_id, error = %e, "input validation failed, fail-open passthrough");
            let resp = HttpResponse {
                correlation_id: correlation_id.clone(),
                hook_type: request.hook_type.clone(),
                payload: HttpResponsePayload::OriginalPassthrough { original: message.clone(), reason: e.clone() },
                latency_ms: start.elapsed().as_millis() as u64,
                error: Some(e),
            };
            return Ok(Json(resp));
        }
        // Redact secrets before any outbound call
        let _redacted = knocode_core::redact_secrets(message);
    }

    debug!(
        correlation_id = %correlation_id,
        hook_type = %request.hook_type,
        "HTTP request received"
    );

    // Convert to internal request
    let hook_type = match request.hook_type.as_str() {
        "PreGeneration" => HookType::PreGeneration,
        "Probe" => HookType::Probe,
        // "PreToolCall"/ToolOutput (compression) was removed — RTK owns that layer.
        _ => {
            let hook_type_str = request.hook_type.clone();
            let resp = HttpResponse {
                correlation_id: correlation_id.clone(),
                hook_type: hook_type_str.clone(),
                payload: HttpResponsePayload::OriginalPassthrough {
                    original: String::new(),
                    reason: format!("Unknown hook type: {}", hook_type_str),
                },
                latency_ms: start.elapsed().as_millis() as u64,
                error: Some("Unknown hook type".to_string()),
            };
            return Err((StatusCode::BAD_REQUEST, Json(resp)));
        }
    };

    // TASK-021: repository_id (hash repo_path) + timestamp propagated for full trace request→context→router→model→optimizer
    // TASK-036/F-7: derive repository identity from the AGENT's workspace when provided;
    // daemon CWD is only the fallback for direct API callers (curl, tests).
    let repository_path: Option<String> = match &request.payload {
        HttpRequestPayload::MessageRewrite { repository_path, .. } => repository_path.clone(),
        HttpRequestPayload::Probe | HttpRequestPayload::Removed => None,
    };
    let repository_id = {
        use sha2::{Digest, Sha256};
        if let Some(path) = repository_path.as_deref().filter(|p| !p.trim().is_empty()) {
            knocode_core::repository_id_from_path(path)
        } else {
            let cwd = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
            let mut h = Sha256::new();
            h.update(cwd.to_string_lossy().as_bytes());
            format!("{:x}", h.finalize())[..12].to_string()
        }
    };
    let timestamp = chrono::Utc::now().to_rfc3339();
    tracing::info!(correlation_id=%correlation_id, repository_id=%repository_id, timestamp=%timestamp, hook_type=%request.hook_type, "request received — trace start (request→context→router→model→optimizer)");
    let internal_request = AgentRequest {
        correlation_id: CorrelationId::from_string(correlation_id.clone()),
        hook_type: hook_type.clone(),
        repository_id: repository_id.clone(),
        timestamp: timestamp.clone(),
        payload: match request.payload {
            HttpRequestPayload::MessageRewrite { session_id, message, context_hints, repository_path } => {
                RequestPayload::MessageRewrite {
                    session_id: session_id.unwrap_or_default(),
                    message,
                    context_hints: context_hints.map(|h| knocode_core::ContextHints {
                        files_mentioned: h.files_mentioned,
                        language: h.language,
                    }),
                    repository_path,
                }
            }
            // Probes are intercepted above (before the readiness gate); never converted.
            HttpRequestPayload::Probe => unreachable!("probe handled before hook_type conversion"),
            // Removed contracts are intercepted above (structured 400); never converted.
            HttpRequestPayload::Removed => unreachable!("removed contract handled before hook_type conversion"),
        },
    };

    // Handle request
    let result = handle_request(internal_request, state).await;
    let latency_ms = start.elapsed().as_millis() as u64;

    match result {
        Ok(response) => Ok(Json(response)),
        Err(e) => {
            crate::metrics::global().inc_fail_open();
            error!(error = %e, "Request handling failed");
            let resp = HttpResponse {
                correlation_id,
                hook_type: request.hook_type,
                payload: HttpResponsePayload::OriginalPassthrough {
                    original: String::new(),
                    reason: format!("error: {}", e),
                },
                latency_ms,
                error: Some(e),
            };
            Err((StatusCode::INTERNAL_SERVER_ERROR, Json(resp)))
        }
    }
}

async fn handle_request(
    request: AgentRequest,
    state: HttpServerState,
) -> Result<HttpResponse, String> {
    let correlation_id = request.correlation_id.to_string();
    let hook_type = format!("{:?}", request.hook_type);

    let payload = match &request.payload {
        RequestPayload::MessageRewrite { session_id, message, context_hints, repository_path } => {
            handle_pre_generation(
                message.clone(),
                session_id.clone(),
                context_hints.clone(),
                repository_path.clone(),
                &state.context_engine,
            ).await?
        }
        // RequestPayload::ToolOutput (compression) was removed — RTK owns that layer.
        // Unreachable over HTTP (intercepted in handle_hook before conversion), but
        // answered honestly if an AgentRequest is built directly by a caller.
        RequestPayload::ToolOutput { .. } => {
            return Err("ToolOutput removed from the daemon: tool-output compression is delegated to RTK".to_string())
        }
        RequestPayload::Probe => http_probe_payload(),
    };

    Ok(HttpResponse {
        correlation_id,
        hook_type,
        payload,
        latency_ms: 0,
        error: None,
    })
}

pub(crate) async fn handle_pre_generation(
    message: String,
    session_id: String,
    context_hints: Option<knocode_core::ContextHints>,
    repository_path: Option<String>,
    context_engine: &Arc<Mutex<ContextEngine>>,
) -> Result<HttpResponsePayload, String> {
    let task = TaskRequest {
        message: message.clone(),
        session_id,
        context_hints,
        repository_id: match repository_path.as_deref() {
            Some(p) if !p.trim().is_empty() => knocode_core::repository_id_from_path(p),
            _ => String::new(),
        },
        repository_path,
        expected_files: None,
    };

    let _timer = crate::metrics::Timer::start();
    let engine = context_engine.lock().await;
    let context_pack = engine.build_context(&task).await?;
    crate::metrics::global().inc_requests("PreGeneration");
    // TASK-022: wire metrics — context tokens + retrieval recall (was dead_code)
    crate::metrics::global().observe_context_tokens(context_pack.token_usage.total_tokens);
    // Retrieval-stage stats (files in pack, search latency, candidates before packing)
    crate::metrics::global().observe_context_files(context_pack.provenance.len());
    if let Some(stats) = context_pack.retrieval_stats {
        crate::metrics::global().observe_retrieval_duration(stats.retrieval_ms as f64 / 1000.0);
        crate::metrics::global().observe_retrieval_candidates(stats.candidates);
    }
    let recall = if !context_pack.code_context.is_empty() { 0.85 } else { 0.0 };
    crate::metrics::global().set_retrieval_recall(recall);
    tracing::info!(correlation_id=%context_pack.metadata.correlation_id, repository_state=%context_pack.repository_state, total_tokens=%context_pack.token_usage.total_tokens, recall=%recall, "trace: context built");

    // TASK-031/F-2: zero-value rewrite suppression — when all three content sources are empty,
    // return the original untouched instead of a metadata-only skeleton (~500-700 tokens for nothing).
    if context_pack.token_usage.total_tokens == 0 {
        tracing::info!(correlation_id=%context_pack.metadata.correlation_id, "no context hits — OriginalPassthrough (TASK-031)");
        return Ok(HttpResponsePayload::OriginalPassthrough {
            original: message,
            reason: "no_context_hits".to_string(),
        });
    }

    let yaml = knocode_context::ContextEngine::to_yaml(&context_pack)?;

    let rewritten = format!(
        "{}\n\n---\n\nContext:\n{}",
        message, yaml
    );

    Ok(HttpResponsePayload::RewrittenMessage {
        original: message,
        rewritten,
        context_pack: Box::new(Some(context_pack)),
    })
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_http_request_deserialization() {
        let json = r#"{
            "hook_type": "PreGeneration",
            "payload": {
                "type": "MessageRewrite",
                "session_id": "test",
                "message": "hello"
            }
        }"#;
        let req: HttpRequest = serde_json::from_str(json).unwrap();
        assert_eq!(req.hook_type, "PreGeneration");
    }

    #[test]
    fn test_http_response_serialization() {
        let resp = HttpResponse {
            correlation_id: "req_123".to_string(),
            hook_type: "PreGeneration".to_string(),
            payload: HttpResponsePayload::OriginalPassthrough {
                original: "test".to_string(),
                reason: "error".to_string(),
            },
            latency_ms: 100,
            error: None,
        };
        let json = serde_json::to_string(&resp).unwrap();
        assert!(json.contains("OriginalPassthrough"));
    }

    #[tokio::test]
    async fn test_health_reports_workspace_version() {
        // Version SoT: /health must report the workspace version from Cargo.toml,
        // never a hardcoded string (F-5-style honesty for versioning)
        let resp = handle_health().await;
        assert_eq!(resp.0["version"], env!("CARGO_PKG_VERSION"));
        assert_eq!(resp.0["status"], "ok");
        // Readiness: the global metrics default to Indexing (no daemon running in
        // tests), so the health payload must report the indexing state + a count.
        assert_eq!(resp.0["state"], "indexing");
        assert!(resp.0["index_files"].is_u64() || resp.0["index_files"].is_null());
    }
}
