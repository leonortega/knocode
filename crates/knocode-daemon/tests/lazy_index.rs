//! Multi-repo daemon: LAZY indexing regression net.
//!
//! A repo the daemon has never seen (no `--repo` flag, never requested before)
//! must be indexed on its FIRST request via `/hook`:
//!   (a) the request returns real context from the lazily-indexed repo
//!       (RewrittenMessage, not OriginalPassthrough),
//!   (b) the index WRITE persisted to the daemon's SQLite store (migration 008
//!       + `with_db_path`: a request-only repo previously got an in-memory DB,
//!       so its index writes were silently discarded and every request re-grep'd),
//!   (c) provenance points into the requested repo only (no leakage).
//!
//! Spawns the REAL daemon binary — the lazy path only exists in the daemon
//! wiring (`DaemonState::initialize` → `with_db_path`), not in a bare router.
//! Stores are isolated via KNOCODE_DATABASE_PATH / KNOCODE_INDEX_DIR temp dirs.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use knocode_core::repository_id_from_path;
use knocode_storage::Database;

/// Grab a free ephemeral port by binding and releasing a listener.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

fn http_get(addr: &str, path: &str) -> Option<String> {
    let mut stream = std::net::TcpStream::connect(addr).ok()?;
    stream
        .write_all(
            format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n")
                .as_bytes(),
        )
        .ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf).ok()?;
    Some(buf)
}

fn http_post_json(addr: &str, path: &str, body: &str) -> Option<String> {
    let mut stream = std::net::TcpStream::connect(addr).ok()?;
    stream
        .write_all(
            format!(
                "POST {path} HTTP/1.1\r\nHost: {addr}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            )
            .as_bytes(),
        )
        .ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(60))).ok()?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf).ok()?;
    Some(buf)
}

/// Poll GET /health until `state` reaches the wanted value (or timeout).
fn wait_for_state(addr: &str, want: &str, timeout: Duration) -> Option<String> {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if let Some(body) = http_get(addr, "/health") {
            if body.contains(&format!("\"state\":\"{want}\"")) {
                return Some(body);
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    None
}

#[test]
fn daemon_lazily_indexes_unknown_repo_on_first_request() {
    let id = std::process::id();

    // ── The never-seen repo: one file with a unique marker ───────────────
    let repo = std::env::temp_dir().join(format!("knocode_lazy_repo_{id}"));
    std::fs::create_dir_all(repo.join("src")).expect("create temp repo");
    std::fs::write(
        repo.join("src").join("greeter.rs"),
        "// lazy_marker_gamma unique greeting token\npub fn greet() -> &'static str { \"hello\" }\n",
    )
    .expect("write source file");

    // ── Daemon home: NOT the repo (repo-neutral startup, nothing eager) ──
    let home = std::env::temp_dir().join(format!("knocode_lazy_home_{id}"));
    std::fs::create_dir_all(&home).expect("create temp home");
    let db_path = home.join("data.db");
    let index_dir = home.join("index");

    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_knocode-daemon"))
        .arg("--port")
        .arg(port.to_string())
        // NO --repo: the daemon must start repo-neutral and index on request.
        .current_dir(&home)
        .env("KNOCODE_DATABASE_PATH", &db_path)
        .env("KNOCODE_INDEX_DIR", &index_dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn knocode-daemon");

    let addr = format!("127.0.0.1:{port}");
    let ready = wait_for_state(&addr, "ready", Duration::from_secs(30))
        .unwrap_or_else(|| panic!("daemon never reached ready on {addr} within 30s"));
    assert!(ready.contains("HTTP/1.1 200"), "expected 200, got: {ready}");

    // ── FIRST request for the unknown repo ───────────────────────────────
    let body = serde_json::json!({
        "correlation_id": "lazy_idx_001",
        "hook_type": "PreGeneration",
        "payload": {
            "type": "MessageRewrite",
            "session_id": "lazy-session",
            "message": "lazy_marker_gamma unique greeting token",
            "repository_path": repo.to_string_lossy()
        }
    })
    .to_string();
    let raw = http_post_json(&addr, "/hook", &body)
        .unwrap_or_else(|| panic!("/hook unreachable on {addr}"));
    let json_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(0);
    let json: serde_json::Value =
        serde_json::from_str(raw[json_start..].trim()).expect("daemon returned valid JSON");

    // (a) Context from the lazily-indexed repo — NOT passthrough.
    //     Lazy indexing runs synchronously inside this request (engine lock),
    //     so by the time the response arrives the index must be queryable.
    assert_eq!(
        json["payload"]["type"], "RewrittenMessage",
        "first request for an unknown repo must return lazily-indexed context, got: {json}"
    );
    let rewritten = json["payload"]["rewritten"].as_str().unwrap_or("");
    assert!(
        rewritten.contains("lazy_marker_gamma"),
        "lazy-indexed content must appear in the context pack, got: {rewritten}"
    );

    // (c) Provenance points into the requested repo. Provenance paths are
    //     stored REPO-RELATIVE (e.g. `src/greeter.rs`; native separators), so
    //     leakage would show up as an absolute path from somewhere else
    //     (daemon home, other repos).
    let prov = json["payload"]["context_pack"]["provenance"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert!(!prov.is_empty(), "provenance rows expected: {json}");
    let rel_path = |p: &serde_json::Value| p["path"].as_str().unwrap_or("").replace('\\', "/");
    let greeter_seen = prov.iter().any(|p| rel_path(p) == "src/greeter.rs");
    assert!(
        greeter_seen,
        "provenance must include the lazy repo's file, got: {prov:?}"
    );
    for p in &prov {
        let path = rel_path(p);
        assert!(
            !path.contains("knocode_lazy_home") && !path.contains(":/") && !path.starts_with('/'),
            "provenance leaked an absolute path from outside the repo: {path}"
        );
    }

    // (b) The index write PERSISTED to the daemon's SQLite store (the exact
    //     regression `with_db_path` fixed: in-memory per-request DBs discarded
    //     writes). Compute the repo_id the same way the daemon does.
    let canonical = dunce::canonicalize(&repo).expect("canonicalize temp repo");
    let repo_id = repository_id_from_path(&canonical.to_string_lossy());
    let db = Database::open(&db_path).expect("open daemon database from test");
    let files = db
        .get_file_count_for_repo(&repo_id)
        .expect("count files for lazy repo");
    assert!(
        files >= 1,
        "lazy index not persisted: repository_id {repo_id} has {files} file rows in {}",
        db_path.display()
    );

    // Cleanup even when the assertions above fail.
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&repo);
    let _ = std::fs::remove_dir_all(&home);
}
