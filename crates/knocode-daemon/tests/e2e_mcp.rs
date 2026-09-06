//! E2E for the daemon MCP surface (`POST /mcp`, JSON-RPC 2.0).
//!
//! Boots the real HTTP router on an ephemeral port against a temp-seeded global
//! tantivy index and asserts the MCP contract end to end:
//!   (a) initialize + tools/list + notifications/initialized (202)
//!   (b) tools/call knocode_context returns the enriched answer + structured metadata
//!   (c) zero-hit prompt → passthrough (text == prompt, passthrough: true)
//!   (d) readiness gate: knocode_context while indexing → JSON-RPC -32001 (HTTP 200)
//!   (e) transport errors: garbage → HTTP 400 / -32700; unknown method → -32601
//!   (f) knocode_compress answers unknown-tool (-32602): compression lives in RTK
//!
//! Runs as ONE test fn because it mutates process-global env vars (own process —
//! separate integration-test binary from e2e_hooks).

use std::path::PathBuf;
use std::sync::Arc;

use knocode_context::{ContextConfig, ContextEngine};
use knocode_daemon::http_server::{create_router, HttpServerState};
use knocode_events::EventBus;
use knocode_knowledge::KnowledgeHub;
use knocode_repo_intel::RepositoryIntelligence;
use knocode_storage::Database;

fn temp_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("knocode_e2e_mcp_{}_{tag}", uuid::Uuid::new_v4()))
}

fn index_repo(dir: &PathBuf, file_name: &str, content: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(file_name), format!("{content}\n")).unwrap();
    let db = Database::open(&PathBuf::from(":memory:")).unwrap();
    let mut ri = RepositoryIntelligence::new(dir.clone(), db, EventBus::new());
    ri.index_repository().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_mcp_contracts() {
    // Isolated global tantivy index for the whole run
    let index_dir = temp_dir("idx");
    std::env::set_var("KNOCODE_INDEX_DIR", index_dir.to_string_lossy().to_string());
    std::env::set_var("KNOCODE_REPO_STATE", "e2e-mcp-head");

    // ── Seed one repo into the shared index ──────────────────────────────
    let repo = temp_dir("eshop");
    index_repo(
        &repo,
        "checkout.cs",
        "// eshop basket checkout flow eshop_marker_alpha\npublic async Task CheckoutBasket() { }",
    );

    // ── Engine over a second (empty-ish) repo — queries scope by repository_path ──
    let repo_daemon = temp_dir("daemon");
    index_repo(&repo_daemon, "router.rs", "// daemon router placeholder\n");

    let hub = KnowledgeHub::new(
        Database::open(&PathBuf::from(":memory:")).unwrap(),
        EventBus::new(),
    );
    let engine = ContextEngine::new(
        RepositoryIntelligence::new(
            repo_daemon.clone(),
            Database::open(&PathBuf::from(":memory:")).unwrap(),
            EventBus::new(),
        ),
        hub,
        EventBus::new(),
        ContextConfig::default(),
    );
    let state = HttpServerState {
        context_engine: Arc::new(tokio::sync::Mutex::new(engine)),
    };

    // ── Boot HTTP server on an ephemeral port ────────────────────────────
    let router = create_router(state);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, router).await;
    });

    // A directly-booted router acts as a READY daemon (serve() flips this after
    // the initial index). The readiness-gate assertion below flips it temporarily.
    knocode_daemon::metrics::global().set_readiness(knocode_daemon::metrics::Readiness::Ready);

    let port = addr.port().to_string();

    // (a) initialize
    let (status, json) = post_mcp(
        &port,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 1,
            "method": "initialize",
            "params": { "protocolVersion": "2024-11-05", "capabilities": {}, "clientInfo": { "name": "e2e", "version": "0" } }
        }),
    )
    .await;
    assert_eq!(status, 200, "initialize: {json}");
    assert_eq!(json["result"]["protocolVersion"], "2024-11-05");
    assert_eq!(json["result"]["serverInfo"]["name"], "knocode");
    assert_eq!(json["result"]["serverInfo"]["version"], env!("CARGO_PKG_VERSION"));

    // (a) tools/list
    let (status, json) = post_mcp(&port, &serde_json::json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" })).await;
    assert_eq!(status, 200);
    let names: Vec<String> = json["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap().to_string())
        .collect();
    assert!(names.contains(&"knocode_context".to_string()), "{names:?}");
    // Compression removed from the MCP surface — RTK (github.com/rtk-ai/rtk) owns it.
    assert!(!names.contains(&"knocode_compress".to_string()), "{names:?}");

    // (a) notifications/initialized → HTTP 202, no body to parse
    let (status, _) = post_mcp(
        &port,
        &serde_json::json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }),
    )
    .await;
    assert_eq!(status, 202, "notifications/initialized must be acknowledged with 202");

    // (b) knocode_context on a seeded hit → enriched text + structured metadata
    let repo_path = repo.to_string_lossy().to_string();
    let (status, json) = post_mcp(
        &port,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 3,
            "method": "tools/call",
            "params": {
                "name": "knocode_context",
                "arguments": { "prompt": "eshop basket checkout flow eshop_marker_alpha", "repository_path": repo_path }
            }
        }),
    )
    .await;
    assert_eq!(status, 200, "context call: {json}");
    assert_eq!(json["error"], serde_json::Value::Null, "no jsonrpc error expected: {json}");
    let result = &json["result"];
    assert_eq!(result["isError"], false, "{result}");
    assert_eq!(result["structuredContent"]["type"], "context");
    assert_eq!(result["structuredContent"]["passthrough"], false);
    let text = result["content"][0]["text"].as_str().unwrap();
    assert!(text.contains("eshop_marker_alpha"), "seeded content must be in answer: {text}");
    let prov = result["structuredContent"]["provenance"].as_array().unwrap();
    assert!(!prov.is_empty(), "provenance expected: {result}");

    // (c) zero-hit prompt → passthrough (text == prompt, structured flag)
    let (status, json) = post_mcp(
        &port,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 4,
            "method": "tools/call",
            "params": {
                "name": "knocode_context",
                "arguments": { "prompt": "zzzqqq unrelated gibberish xyzzy plugh", "repository_path": repo_path }
            }
        }),
    )
    .await;
    assert_eq!(status, 200);
    let result = &json["result"];
    assert_eq!(result["structuredContent"]["passthrough"], true, "{result}");
    assert_eq!(
        result["content"][0]["text"].as_str().unwrap(),
        "zzzqqq unrelated gibberish xyzzy plugh",
        "F-2 parity: prompt byte-identical on passthrough"
    );

    // (d) readiness gate: while indexing, tools/call → -32001 (transport stays 200)
    knocode_daemon::metrics::global().set_readiness(knocode_daemon::metrics::Readiness::Indexing);
    let (status, json) = post_mcp(
        &port,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 5,
            "method": "tools/call",
            "params": { "name": "knocode_context", "arguments": { "prompt": "hi", "repository_path": repo_path } }
        }),
    )
    .await;
    knocode_daemon::metrics::global().set_readiness(knocode_daemon::metrics::Readiness::Ready);
    assert_eq!(status, 200, "gate error must keep HTTP 200: {json}");
    assert_eq!(json["error"]["code"], -32001, "{json}");
    assert!(json["error"]["message"].as_str().unwrap().contains("daemon_indexing"));

    // (e) removed tool: knocode_compress answers the standard unknown-tool error
    let (status, json) = post_mcp(
        &port,
        &serde_json::json!({
            "jsonrpc": "2.0",
            "id": 6,
            "method": "tools/call",
            "params": {
                "name": "knocode_compress",
                "arguments": { "content": "some output", "tool_name": "bash" }
            }
        }),
    )
    .await;
    assert_eq!(status, 200);
    assert_eq!(json["error"]["code"], -32602, "{json}");

    // (f) transport errors
    let (status, json) = post_mcp_raw(&port, "not json{{{").await;
    assert_eq!(status, 400);
    assert_eq!(json["error"]["code"], -32700);

    let (status, json) = post_mcp(&port, &serde_json::json!({ "jsonrpc": "2.0", "id": 7, "method": "definitely_not_a_method" })).await;
    assert_eq!(status, 200);
    assert_eq!(json["error"]["code"], -32601);
    assert_eq!(json["id"], 7);

    // Cleanup best-effort
    std::env::remove_var("KNOCODE_INDEX_DIR");
    std::env::remove_var("KNOCODE_REPO_STATE");
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&repo_daemon);
    let _ = std::fs::remove_dir_all(&index_dir);
}

/// POST raw bytes to /mcp and return (HTTP status, parsed JSON body or Null).
async fn post_mcp(port: &str, body: &serde_json::Value) -> (u16, serde_json::Value) {
    post_mcp_raw(port, &serde_json::to_string(body).unwrap()).await
}

async fn post_mcp_raw(port: &str, payload: &str) -> (u16, serde_json::Value) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap()))
        .await
        .unwrap();
    let req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        payload.len(),
        payload
    );
    stream.write_all(req.as_bytes()).await.unwrap();
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).await.unwrap();
    let raw = String::from_utf8_lossy(&buf);

    // HTTP status
    let status: u16 = raw
        .split_whitespace()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    // Body (may be empty for 202 notifications)
    let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
    let body = raw[body_start..].trim();
    if body.is_empty() {
        return (status, serde_json::Value::Null);
    }
    let json = serde_json::from_str(body).unwrap_or_else(|e| panic!("invalid JSON body: {e}: {body}"));
    (status, json)
}
