//! TASK-035 / F-6: E2E regression net for the hook contracts.
//!
//! Boots the real HTTP router on an ephemeral port against a temp-seeded global tantivy
//! index (`KNOCODE_INDEX_DIR`) and asserts:
//!   (a) provenance rows are unique,
//!   (b) zero cross-repo leakage with a second seeded repo,
//!   (c) passthrough on an empty-hit prompt,
//!   (d) readiness probe + PreToolCall/ToolOutput rejection (compression = RTK).
//!
//! Runs as ONE test fn because it mutates process-global env vars.

use std::path::PathBuf;
use std::sync::Arc;

use knocode_context::{ContextConfig, ContextEngine};
use knocode_daemon::http_server::{create_router, HttpServerState};
use knocode_events::EventBus;
use knocode_knowledge::KnowledgeHub;
use knocode_repo_intel::RepositoryIntelligence;
use knocode_storage::Database;

fn temp_dir(tag: &str) -> PathBuf {
    std::env::temp_dir().join(format!("knocode_e2e_{}_{tag}", uuid::Uuid::new_v4()))
}

fn index_repo(dir: &PathBuf, file_name: &str, content: &str) {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join(file_name), format!("{content}\n")).unwrap();
    let db = Database::open(&PathBuf::from(":memory:")).unwrap();
    let mut ri = RepositoryIntelligence::new(dir.clone(), db, EventBus::new());
    ri.index_repository().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn e2e_hook_contracts() {
    // Isolated global tantivy index for the whole run
    let index_dir = temp_dir("idx");
    std::env::set_var("KNOCODE_INDEX_DIR", index_dir.to_string_lossy().to_string());
    std::env::set_var("KNOCODE_REPO_STATE", "e2e-hook-contract-head");

    // ── Seed TWO repos into the shared index ─────────────────────────────
    let repo_eshop = temp_dir("eshop");
    let repo_knocode = temp_dir("knocode");
    index_repo(
        &repo_eshop,
        "checkout.cs",
        "// eshop basket checkout flow eshop_marker_alpha\npublic async Task CheckoutBasket() { }",
    );
    index_repo(
        &repo_knocode,
        "router.rs",
        "// knocode daemon router daemon_marker_beta\npub fn route_request() {}",
    );

    // ── Build the engine as a daemon STARTED IN repo_knocode would ───────
    let kh_db = Database::open(&PathBuf::from(":memory:")).unwrap();
    let hub = KnowledgeHub::new(
        kh_db,
        EventBus::new(),
    );
    let engine = ContextEngine::new(
        RepositoryIntelligence::new(repo_knocode.clone(), Database::open(&PathBuf::from(":memory:")).unwrap(), EventBus::new()),
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

    // The readiness gate (serve() sets this after the initial index) must be
    // flipped for a directly-booted router — this test acts as a READY daemon.
    knocode_daemon::metrics::global().set_readiness(knocode_daemon::metrics::Readiness::Ready);

    let port = addr.port().to_string();

    // ── (e) Readiness probe (HTTP) — mirrors the UDS/MessagePack `Probe` payload ──
    let body = serde_json::json!({
        "correlation_id": "e2e_probe_001",
        "hook_type": "Probe",
        "payload": { "type": "Probe" }
    });
    let json = post_hook(&port, &body).await;
    assert_eq!(json["payload"]["type"], "Probe", "probe payload expected: {json}");
    assert_eq!(json["payload"]["state"], "ready", "readiness flipped to Ready: {json}");
    assert_eq!(json["payload"]["version"], env!("CARGO_PKG_VERSION"));
    assert!(json["payload"]["index_files"].is_u64(), "index_files must be a number: {json}");

    async fn post_hook(port: &str, body: &serde_json::Value) -> serde_json::Value {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let payload = serde_json::to_string(body).unwrap();
        let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port.parse::<u16>().unwrap()))
            .await
            .unwrap();
        let req = format!(
            "POST /hook HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            payload.len(),
            payload
        );
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let raw = String::from_utf8_lossy(&buf);
        let json_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
        serde_json::from_str(raw[json_start..].trim()).expect("daemon returned valid JSON")
    }

    // ── (c) PreGeneration scoped to eShopOnWeb must NOT leak knocode paths ──
    let body = serde_json::json!({
        "correlation_id": "e2e_pg_001",
        "hook_type": "PreGeneration",
        "payload": {
            "type": "MessageRewrite",
            "session_id": "e2e-session",
            "message": "eshop basket checkout flow eshop_marker_alpha",
            "repository_path": repo_eshop.to_string_lossy()
        }
    });
    let json = post_hook(&port, &body).await;
    assert_eq!(json["payload"]["type"], "RewrittenMessage", "seeded hit expected; got {json}");
    let pack = &json["payload"]["context_pack"];
    assert!(!pack.is_null(), "context_pack must be present for assertions");

    let rewritten = json["payload"]["rewritten"].as_str().unwrap();
    assert!(rewritten.contains("eshop_marker_alpha"), "repo A content must be injected");
    assert!(
        !rewritten.contains("daemon_marker_beta") && !rewritten.contains("route_request"),
        "F-1: no cross-repo leakage, got: {rewritten}"
    );

    // Provenance paths must point at repo A only
    let prov = pack["provenance"].as_array().expect("provenance array");
    assert!(!prov.is_empty());
    for p in prov {
        let path = p["path"].as_str().unwrap();
        assert!(
            !path.contains("router.rs"),
            "F-1: provenance leaked other repo: {path}"
        );
    }

    // ── (b) provenance uniqueness (F-3) ───────────────────────────────────
    let mut keys: Vec<(String, String, String)> = prov
        .iter()
        .map(|p| {
            (
                p["path"].as_str().unwrap_or("").to_string(),
                p["source"].as_str().unwrap_or("").to_string(),
                p["retriever"].as_str().unwrap_or("").to_string(),
            )
        })
        .collect();
    keys.sort();
    keys.dedup();
    assert_eq!(keys.len(), prov.len(), "F-3: duplicate provenance rows: {prov:?}");

    // ── (d) passthrough on empty-hit prompt (F-2) ─────────────────────────
    let body = serde_json::json!({
        "correlation_id": "e2e_pg_002",
        "hook_type": "PreGeneration",
        "payload": {
            "type": "MessageRewrite",
            "session_id": "e2e-session",
            "message": "zzzqqq unrelated gibberish xyzzy plugh",
            "repository_path": repo_eshop.to_string_lossy()
        }
    });
    let json = post_hook(&port, &body).await;
    assert_eq!(json["payload"]["type"], "OriginalPassthrough", "zero-hit prompt must pass through untouched: {json}");
    assert_eq!(json["payload"]["reason"], "no_context_hits");
    assert_eq!(
        json["payload"]["original"].as_str().unwrap(),
        "zzzqqq unrelated gibberish xyzzy plugh",
        "F-2: prompt must be byte-identical on passthrough"
    );

    // ── (e) removed contract: PreToolCall/ToolOutput (compression = RTK) must be rejected ──
    let body = serde_json::json!({
        "correlation_id": "e2e_ptc_001",
        "hook_type": "PreToolCall",
        "payload": {
            "type": "ToolOutput",
            "tool_name": "bash",
            "output_type": "ShellOutput",
            "content": "some tool output",
            "repository_path": repo_eshop.to_string_lossy()
        }
    });
    let json = post_hook(&port, &body).await;
    assert_eq!(json["payload"]["reason"], "Unknown hook type: PreToolCall", "{json}");

    // Cleanup best-effort
    std::env::remove_var("KNOCODE_INDEX_DIR");
    std::env::remove_var("KNOCODE_REPO_STATE");
    let _ = std::fs::remove_dir_all(&repo_eshop);
    let _ = std::fs::remove_dir_all(&repo_knocode);
    let _ = std::fs::remove_dir_all(&index_dir);
}
