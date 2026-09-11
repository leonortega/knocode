//! MCP (Model Context Protocol) surface for the Knocode daemon.
//!
//! JSON-RPC 2.0 served at `POST /mcp` on the existing axum HTTP listener (the same
//! 127.0.0.1:9527 server on Windows and Unix — no extra socket, no extra process).
//!
//! Purpose — the "no conversions" path for client plugins (opencode-knocode): instead
//! of shaping prompts/answers into the internal `MessageRewrite` / `ToolOutput` wire
//! payloads, plugins call typed tools and receive natural text + structured metadata.
//! The agent never talks to the daemon: the plugin catches the prompt and drives these
//! tools before the model sees it.
//!
//! Scope: stateless subset of MCP — `initialize`, `notifications/initialized`, `ping`,
//! `tools/list`, `tools/call`. No sampling / prompts / resources, no batch requests.
//! `tools/call` while the index is building answers a JSON-RPC `-32001` error (HTTP 200)
//! instead of a transport 503, mirroring `/hook`'s `daemon_indexing` gating semantics.

use axum::{
    extract::State,
    http::StatusCode,
    response::Json,
};
use serde_json::{json, Value};

use crate::http_server::{handle_pre_generation, HttpResponsePayload, HttpServerState};

/// MCP protocol version we implement (2024-11-05 — the stable tools-only revision).
const PROTOCOL_VERSION: &str = "2024-11-05";
/// Application error: `tools/call` while the repository index is running (initial cold
/// index or an auto-reindex). Clients poll `GET /health` (`state: "ready"`) before
/// their first call; a reindex mid-session surfaces here and callers fail open.
const ERROR_DAEMON_INDEXING: i64 = -32001;
/// Application error: input exceeds the 100 KB cap `/hook` enforces.
const ERROR_INPUT_TOO_LARGE: i64 = -32002;

// ── JSON-RPC envelope ────────────────────────────────────────────────────

#[derive(serde::Deserialize)]
struct McpRequest {
    #[serde(default)]
    jsonrpc: Option<String>,
    #[serde(default)]
    id: Option<Value>,
    #[serde(default)]
    method: String,
    #[serde(default)]
    params: Option<Value>,
}

/// Build a JSON-RPC error response. Transport-level failures (parse/invalid request)
/// get HTTP 400 per the MCP streamable-HTTP convention; application errors keep 200.
fn error_response(id: Option<Value>, code: i64, message: &str, status: StatusCode) -> (StatusCode, Json<Value>) {
    (status, Json(json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": { "code": code, "message": message },
    })))
}

fn result_response(id: Option<Value>, result: Value) -> (StatusCode, Json<Value>) {
    (StatusCode::OK, Json(json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    })))
}

/// A single MCP tool result — the shape the plugin consumes (`text` is the natural
/// answer, `structuredContent` carries machine-readable metadata, `isError` drives
/// fail-open passthrough).
fn tool_result(text: String, structured: Value, is_error: bool) -> Value {
    json!({
        "content": [{ "type": "text", "text": text }],
        "structuredContent": structured,
        "isError": is_error,
    })
}

// ── Tool registry ────────────────────────────────────────────────────────

/// Resolve the deduplication session for an MCP `knocode_context` call.
///
/// The context engine dedups retrieved content per `session_id` (spec §3): a
/// session that asks the same question twice gets the repeat block suppressed.
/// That is correct for the HTTP `/hook` transport (each opencode session sends
/// its own `session_id`), but MCP is STATELESS — the transport has no session
/// concept. The previous hard-coded `"mcp"` put EVERY MCP caller (every plugin
/// window, every repo, every prompt) into ONE shared fingerprint bucket, so the
/// second identical prompt from anyone was silently deduped to an empty pack →
/// `no_context_hits` passthrough even though the repo index was fine.
///
/// Fix: scope the fingerprint bucket per CALLER.
///  - client supplied a `request_id` (the plugins generate a fresh UUID per
///    call) → bucket is that exact request id: nothing is ever deduped across
///    calls, restoring correct behavior for stateless callers;
///  - no `request_id` (curl, hand tests) → a fresh random UUID per call, same
///    guarantee;
///  - both fall back to a distinct `mcp:` prefix so the buckets can never
///    collide with real `/hook` session ids in the shared fingerprint map.
fn mcp_session_id(request_id: &Option<String>) -> String {
    const PREFIX: &str = "mcp:";
    match request_id {
        Some(id) if !id.trim().is_empty() => format!("{PREFIX}{}", id.trim()),
        _ => format!("{PREFIX}{}", uuid::Uuid::new_v4()),
    }
}

fn tools_list() -> Value {
    json!([
        {
            "name": "knocode_context",
            "description": "Build repository context for a prompt before the model sees it. ",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "prompt": {
                        "type": "string",
                        "description": "The user message / task to build context for."
                    },
                    "repository_path": {
                        "type": "string",
                        "description": "Absolute workspace root — daemon scopes retrieval to this repository (fallback: daemon CWD)."
                    },
                    "request_id": {
                        "type": "string",
                        "description": "Optional client-generated correlation id (UUID). Echoed in the daemon log lines and structuredContent so a plugin log line can be joined with the daemon log line for the same request. Also scopes this call's deduplication fingerprint: stateless MCP callers should send a fresh id per call so repeat prompts are never suppressed."
                    }
                },
                "required": ["prompt"]
            }
        }
    ])
}

// ── Handler ──────────────────────────────────────────────────────────────

/// `POST /mcp` — stateless JSON-RPC 2.0 (single requests only; batches rejected).
pub async fn handle_mcp(
    State(state): State<HttpServerState>,
    body: String,
) -> (StatusCode, Json<Value>) {
    if body.trim().is_empty() {
        tracing::debug!(code = -32700, "MCP call rejected: empty body");
        return error_response(None, -32700, "parse error: empty body", StatusCode::BAD_REQUEST);
    }
    let value: Value = match serde_json::from_str(&body) {
        Ok(v) => v,
        Err(_) => {
            tracing::debug!(code = -32700, "MCP call rejected: body is not valid JSON");
            return error_response(
                None,
                -32700,
                "parse error: request body is not valid JSON",
                StatusCode::BAD_REQUEST,
            )
        }
    };
    if value.is_array() {
        tracing::debug!(code = -32600, "MCP call rejected: batch requests are not supported");
        return error_response(None, -32600, "invalid request: batch requests are not supported", StatusCode::BAD_REQUEST);
    }
    let req: McpRequest = match serde_json::from_value(value) {
        Ok(r) => r,
        Err(_) => {
            tracing::debug!(code = -32600, "MCP call rejected: not a valid JSON-RPC 2.0 request object");
            return error_response(
                None,
                -32600,
                "invalid request: not a valid JSON-RPC 2.0 request object",
                StatusCode::BAD_REQUEST,
            )
        }
    };
    if req.jsonrpc.as_deref() != Some("2.0") || req.method.is_empty() {
        tracing::debug!(code = -32600, "MCP call rejected: jsonrpc must be \"2.0\" with a method");
        return error_response(req.id.clone(), -32600, "invalid request: jsonrpc must be \"2.0\" with a method", StatusCode::BAD_REQUEST);
    }

    // Verbosity: log EVERY inbound MCP call (method + id) at debug, so
    // `[logging] level = "debug"` / `KNOCODE_LOG_LEVEL=debug` (installer verbosity 2)
    // shows literally every daemon call in the log file — the info lines below only
    // cover the knocode_context outcome, not initialize/ping/tools/list/health.
    tracing::debug!(id = ?req.id, method = %req.method, "MCP call received");


    // Notification (no id) — nothing to answer. Per the streamable-HTTP convention we
    // acknowledge with 202 and an empty body (Json payload is not consumed on 202).
    if req.id.is_none() {
        return (StatusCode::ACCEPTED, Json(Value::Null));
    }
    let id = req.id.clone();

    match req.method.as_str() {
        "initialize" => result_response(
            id,
            json!({
                "protocolVersion": PROTOCOL_VERSION,
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "knocode", "version": env!("CARGO_PKG_VERSION") },
            }),
        ),
        "notifications/initialized" => result_response(id, json!({})),
        "ping" => result_response(id, json!({})),
        "tools/list" => result_response(id, json!({ "tools": tools_list() })),
        "tools/call" => handle_tools_call(state, id, req.params).await,
        method => error_response(id, -32601, &format!("Method not found: {method}"), StatusCode::OK),
    }
}

// ── tools/call ───────────────────────────────────────────────────────────

async fn handle_tools_call(
    state: HttpServerState,
    id: Option<Value>,
    params: Option<Value>,
) -> (StatusCode, Json<Value>) {
    let params = params.unwrap_or(Value::Null);
    let name = params.get("name").and_then(|v| v.as_str());
    let Some(name) = name else {
        return error_response(id, -32602, "invalid params: missing tool \"name\"", StatusCode::OK);
    };
    let mut args = params.get("arguments").cloned().unwrap_or(Value::Null);
    if !args.is_object() {
        args = Value::Null; // arguments may be omitted — treat as empty object
    }

    match name {
        "knocode_context" => tool_context(state, id, args).await,
        other => error_response(id, -32602, &format!("Unknown tool: {other}"), StatusCode::OK),
    }
}

/// `knocode_context` — build the enriched context answer for a prompt. Mirrors the
/// `/hook` MessageRewrite path (same engine, same zero-value passthrough) so a plugin
/// that switches transports keeps byte-identical enrichment behavior.
async fn tool_context(
    state: HttpServerState,
    id: Option<Value>,
    args: Value,
) -> (StatusCode, Json<Value>) {
    // Readiness gate: initialize/ping/tools/list always answer; a context BUILD needs
    // the engine, which is locked during the initial index / an auto-reindex.
    // Optional client-generated correlation id (§7 request correlation): echoed in every
    // log line and in structuredContent so plugin→daemon requests are traceable without
    // distributed tracing. Absent → "-" in logs, null in structuredContent.
    let request_id = args
        .get("request_id")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    let logged_request_id = request_id.as_deref().unwrap_or("-");

    if !state.is_ready() {
        tracing::warn!(request_id = %logged_request_id, "MCP tools/call knocode_context rejected — daemon indexing in progress (-32001)");
        return error_response(
            id,
            ERROR_DAEMON_INDEXING,
            "daemon_indexing: repository index in progress — poll /health until ready",
            StatusCode::OK,
        );
    }

    let prompt = match args.get("prompt").and_then(|v| v.as_str()) {
        Some(p) if !p.trim().is_empty() => p,
        _ => {
            return error_response(
                id,
                -32602,
                "invalid params: \"prompt\" (non-empty string) is required",
                StatusCode::OK,
            )
        }
    };
    if let Err(e) = crate::http_server::validate_input_len(prompt, 100 * 1024) {
        return error_response(id, ERROR_INPUT_TOO_LARGE, &e, StatusCode::OK);
    }
    let repository_path = args
        .get("repository_path")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let started = std::time::Instant::now();
    // Request count keyed by transport+tool — /hook counts "PreGeneration" at its own
    // dispatch site, so knocode_requests_total distinguishes MCP from HTTP traffic.
    crate::metrics::global().inc_requests("mcp_context");
    match handle_pre_generation(
        prompt.to_string(),
        mcp_session_id(&request_id),
        None,
        repository_path,
        &state.context_engine,
    )
    .await
    {
        Ok(HttpResponsePayload::RewrittenMessage { rewritten, context_pack, .. }) => {
            let provenance = context_pack
                .as_ref().as_ref()
                .map(|p| {
                    p.provenance
                        .iter()
                        .map(|pr| {
                            json!({
                                "path": pr.path,
                                "source": pr.source,
                                "retriever": pr.retriever,
                                "score": pr.score,
                                "reason": pr.reason,
                            })
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();
            let total_tokens = context_pack
                .as_ref().as_ref()
                .map(|p| p.token_usage.total_tokens)
                .unwrap_or(0);
            let repository_state = context_pack
                .as_ref().as_ref()
                .map(|p| p.repository_state.clone())
                .unwrap_or_default();
            tracing::info!(request_id = %logged_request_id, took_ms = %started.elapsed().as_millis(), "MCP knocode_context built");
            result_response(
                id,
                tool_result(
                    rewritten,
                    json!({
                        "type": "context",
                        "passthrough": false,
                        "repository_state": repository_state,
                        "total_tokens": total_tokens,
                        "provenance": provenance,
                        "request_id": request_id,
                    }),
                    false,
                ),
            )
        }
        Ok(HttpResponsePayload::OriginalPassthrough { original, reason }) => {
            // Zero-value suppression (TASK-031): text is the original prompt — the
            // plugin must leave the message untouched.
            tracing::info!(request_id = %logged_request_id, reason = %reason, "MCP knocode_context passthrough");
            result_response(
                id,
                tool_result(
                    original,
                    json!({ "type": "context", "passthrough": true, "reason": reason, "request_id": request_id }),
                    false,
                ),
            )
        }
        Ok(_) => result_response(
            id,
            tool_result(
                "internal: unexpected payload".to_string(),
                json!({ "type": "context", "passthrough": true, "reason": "internal_error" }),
                true,
            ),
        ),
        Err(e) => {
            crate::metrics::global().inc_fail_open();
            tracing::error!(request_id = %logged_request_id, error = %e, "MCP knocode_context build failed");
            result_response(
                id,
                tool_result(
                    format!("knocode context build failed: {e}"),
                    json!({ "type": "context", "passthrough": true, "reason": e, "request_id": request_id }),
                    true,
                ),
            )
        }
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::Arc;
    use knocode_context::{ContextConfig, ContextEngine};
    use knocode_events::EventBus;
    use knocode_knowledge::KnowledgeHub;
    use knocode_repo_intel::RepositoryIntelligence;
    use knocode_storage::Database;

    /// Minimal engine over an empty dir — enough to satisfy HttpServerState for the
    /// envelope tests (context calls are covered by the seeded e2e).
    fn empty_state() -> HttpServerState {
        let dir = std::env::temp_dir().join(format!("knocode_mcp_ut_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let engine = ContextEngine::new(
            RepositoryIntelligence::new(
                dir.clone(),
                Database::open(&PathBuf::from(":memory:")).unwrap(),
                EventBus::new(),
            ),
            KnowledgeHub::new(
                Database::open(&PathBuf::from(":memory:")).unwrap(),
                EventBus::new(),
            ),
            EventBus::new(),
            ContextConfig::default(),
        );
        HttpServerState {
            context_engine: Arc::new(tokio::sync::Mutex::new(engine)),
            readiness_override: None, // default: defer to the global singleton
        }
    }

    #[tokio::test]
    async fn test_initialize() {
        let state = empty_state();
        let (status, resp) = handle_mcp(
            State(state),
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"t","version":"0"}}}"#.to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let r = resp.0["result"].clone();
        assert_eq!(r["protocolVersion"], PROTOCOL_VERSION);
        assert_eq!(r["serverInfo"]["name"], "knocode");
        assert_eq!(r["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));
    }

    #[tokio::test]
    async fn test_tools_list() {
        let state = empty_state();
        let (_, resp) = handle_mcp(State(state), r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#.to_string()).await;
        let tools = resp.0["result"]["tools"].as_array().unwrap();
        let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"knocode_context"));
        // Compression removed from the MCP surface: RTK (github.com/rtk-ai/rtk) owns
        // tool-output compression — the daemon is context-only.
        assert!(!names.contains(&"knocode_compress"));
    }

    #[tokio::test]
    async fn test_parse_error_garbage() {
        let (status, resp) = handle_mcp(State(empty_state()), "not json{{{".to_string()).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp.0["error"]["code"], -32700);
    }

    #[tokio::test]
    async fn test_batch_rejected() {
        let (status, resp) = handle_mcp(
            State(empty_state()),
            r#"[{"jsonrpc":"2.0","id":1,"method":"ping"}]"#.to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp.0["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn test_bad_jsonrpc_version() {
        let (status, resp) = handle_mcp(
            State(empty_state()),
            r#"{"jsonrpc":"1.0","id":1,"method":"ping"}"#.to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(resp.0["error"]["code"], -32600);
    }

    #[tokio::test]
    async fn test_unknown_method() {
        let (status, resp) = handle_mcp(
            State(empty_state()),
            r#"{"jsonrpc":"2.0","id":7,"method":"nope"}"#.to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(resp.0["error"]["code"], -32601);
        assert_eq!(resp.0["id"], 7);
    }

    #[tokio::test]
    async fn test_notification_no_response_202() {
        let (status, _) = handle_mcp(
            State(empty_state()),
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#.to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::ACCEPTED);
    }

    #[tokio::test]
    async fn test_context_tool_readiness_gate() {
        // tools/call while indexing → -32001 application error (HTTP 200).
        // Hermetic: readiness is overridden on THIS test's state, never on the
        // process-global singleton — a global flip here raced parallel lib tests
        // (test_context_repeat_call_not_deduped) and flaked CI.
        let mut state = empty_state();
        state.readiness_override = Some(crate::metrics::Readiness::Indexing);
        let (status, resp) = handle_mcp(
            State(state),
            r#"{"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"knocode_context","arguments":{"prompt":"hi"}}}"#.to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(resp.0["error"]["code"], ERROR_DAEMON_INDEXING);
    }

    #[tokio::test]
    async fn test_removed_compress_tool_answers_unknown() {
        // knocode_compress was removed from the MCP surface (RTK owns compression).
        // Calling it must answer the standard unknown-tool error, not panic.
        let state = empty_state();
        let (status, resp) = handle_mcp(
            State(state),
            r#"{"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"knocode_compress","arguments":{"content":"x","tool_name":"bash"}}}"#.to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(resp.0["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn test_unknown_tool() {
        let state = empty_state();
        let (status, resp) = handle_mcp(
            State(state),
            r#"{"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"knocode_bogus","arguments":{}}}"#.to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(resp.0["error"]["code"], -32602);
    }

    #[tokio::test]
    async fn test_missing_tool_name() {
        let state = empty_state();
        let (status, resp) = handle_mcp(
            State(state),
            r#"{"jsonrpc":"2.0","id":12,"method":"tools/call","params":{}}"#.to_string(),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(resp.0["error"]["code"], -32602);
    }

    #[test]
    fn mcp_session_id_scopes_per_request_id() {
        // Same request_id → same bucket (stable echo), prefixed to never collide
        // with real /hook session ids.
        assert_eq!(
            mcp_session_id(&Some("abc-123".to_string())),
            "mcp:abc-123"
        );
        // Whitespace-trimmed echo.
        assert_eq!(
            mcp_session_id(&Some("  abc-123  ".to_string())),
            "mcp:abc-123"
        );
        // Absent / blank request_id → fresh random bucket per call.
        let a = mcp_session_id(&None);
        let b = mcp_session_id(&None);
        let c = mcp_session_id(&Some("   ".to_string()));
        assert_ne!(a, b, "no request_id must yield a unique bucket per call");
        assert_ne!(a, c);
        assert!(a.starts_with("mcp:"));
        assert!(b.starts_with("mcp:"));
        assert!(c.starts_with("mcp:"));
        // Never equal to a real /hook session id shape (no prefix).
        assert_ne!(mcp_session_id(&Some("real-session".to_string())), "real-session");
    }

    /// REGRESSION: the daemon used to pass session_id="mcp" for every MCP call,
    /// putting all callers in ONE dedup bucket — the second identical prompt was
    /// silently deduped to an empty pack (no_context_hits passthrough). With the
    /// per-request bucket, two identical calls must BOTH produce enriched packs.
    #[tokio::test]
    async fn test_context_repeat_call_not_deduped() {
        // Isolated tantivy index + seeded repo so retrieval has a real hit.
        // Env-var mutation happens before any await and the temp dirs are unique
        // per run, so the test is self-isolating (e2e_mcp runs in its own binary).
        let dir = std::env::temp_dir().join(format!("knocode_mcp_dedup_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let index_dir = dir.join("idx");
        std::env::set_var("KNOCODE_INDEX_DIR", index_dir.to_string_lossy().to_string());
        let repo = dir.join("repo");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::write(
            repo.join("marker.rs"),
            "// dedup regression marker gamma unique_159753\nfn unique_159753() {}",
        )
        .unwrap();
        let mut ri = RepositoryIntelligence::new(
            repo.clone(),
            Database::open(&PathBuf::from(":memory:")).unwrap(),
            EventBus::new(),
        );
        ri.index_repository().unwrap();

        let engine = ContextEngine::new(
            ri,
            KnowledgeHub::new(Database::open(&PathBuf::from(":memory:")).unwrap(), EventBus::new()),
            EventBus::new(),
            ContextConfig::default(),
        );
        let state = HttpServerState {
            context_engine: Arc::new(tokio::sync::Mutex::new(engine)),
            readiness_override: Some(crate::metrics::Readiness::Ready), // hermetic: no global flips
        };

        let prompt = "dedup regression marker gamma unique_159753";
        let repo_path = repo.to_string_lossy().to_string();
        // Build via serde_json (NOT string interpolation): the repo path contains
        // Windows backslashes that must be JSON-escaped.
        let call = |id: i64, req_id: &str| {
            serde_json::to_string(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": "tools/call",
                "params": {
                    "name": "knocode_context",
                    "arguments": {
                        "prompt": prompt,
                        "repository_path": repo_path,
                        "request_id": req_id,
                    }
                }
            }))
            .unwrap()
        };

        // Call 1 — enriched.
        let (status, resp) = handle_mcp(State(state.clone()), call(1, "dedup-req-1")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(resp.0["error"], serde_json::Value::Null, "call 1: {}", resp.0);
        assert_eq!(resp.0["result"]["structuredContent"]["passthrough"], false, "call 1 must enrich: {}", resp.0["result"]);
        assert!(resp.0["result"]["content"][0]["text"].as_str().unwrap().contains("unique_159753"), "call 1: {}", resp.0["result"]);

        // Call 2 — IDENTICAL prompt, fresh request_id (what plugins send per call).
        let (status, resp) = handle_mcp(State(state), call(2, "dedup-req-2")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(resp.0["error"], serde_json::Value::Null, "call 2: {}", resp.0);
        assert_eq!(
            resp.0["result"]["structuredContent"]["passthrough"], false,
            "repeat prompt must NOT be deduped into a passthrough: {}",
            resp.0["result"]
        );
        assert!(resp.0["result"]["content"][0]["text"].as_str().unwrap().contains("unique_159753"), "call 2 must still enrich: {}", resp.0["result"]);

        std::env::remove_var("KNOCODE_INDEX_DIR");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
