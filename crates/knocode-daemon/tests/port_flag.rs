//! Regression net for `knocode serve --port N` → `knocode-daemon` pass-through
//! and the multi-repo daemon's eager `--repo` startup.
//!
//! The daemon used to hardcode 9527 and ignore the flag entirely, so
//! `knocode serve --port 9599` health-waited against a port nothing bound.
//! These tests spawn the REAL daemon binary and assert:
//!
//! 1. `--port <ephemeral>` → GET /health answers on exactly that port.
//! 2. `--repo <tmp>` → the daemon eagerly indexes that repo and /health flips
//!    to `state: "ready"` with a real `index_files` count (multi-repo eager
//!    path: readiness-gated indexing, HTTP listener bound first).
//!
//! Isolation: each test points `KNOCODE_DATABASE_PATH` and `KNOCODE_INDEX_DIR`
//! at unique temp locations (both are honored via config env overrides), so the
//! spawned daemons never touch the developer's real `~/.knocode` stores. The
//! only shared side effect is the daemon's log file (`~/.knocode/logs/knocode.log`),
//! identical to any real daemon run.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

/// Unique-per-test temp names (test threads share one process id).
static COUNTER: AtomicUsize = AtomicUsize::new(0);

fn unique_suffix() -> usize {
    COUNTER.fetch_add(1, Ordering::SeqCst)
}

/// Grab a free ephemeral port by binding and releasing a listener.
fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0")
        .expect("bind ephemeral port")
        .local_addr()
        .expect("local addr")
        .port()
}

/// Minimal blocking HTTP GET — std-only, no client dependency needed.
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

/// Spawn the real daemon binary with per-test store isolation and return it.
fn spawn_daemon(port: u16, repo: Option<&std::path::Path>, tag: &str) -> (Command, std::path::PathBuf, std::path::PathBuf, std::path::PathBuf) {
    let id = (std::process::id(), unique_suffix());
    let home = std::env::temp_dir().join(format!(
        "knocode_port_flag_{}_{}/{}",
        id.0, id.1, tag
    ));
    std::fs::create_dir_all(&home).expect("create temp home");
    let db_path = home.join("data.db");
    let index_dir = home.join("index");

    let mut cmd = Command::new(env!("CARGO_BIN_EXE_knocode-daemon"));
    cmd.arg("--port").arg(port.to_string());
    // Repo-neutral by default (multi-repo daemon: no CWD binding, no startup
    // index). With `--repo`, the daemon chdirs there and indexes eagerly.
    match repo {
        Some(r) => {
            cmd.arg("--repo").arg(r);
            cmd.current_dir(r);
        }
        None => {
            cmd.current_dir(&home);
        }
    }
    cmd.env("KNOCODE_DATABASE_PATH", &db_path);
    cmd.env("KNOCODE_INDEX_DIR", &index_dir);
    cmd.stdout(Stdio::null()).stderr(Stdio::null());
    (cmd, home, db_path, index_dir)
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
fn daemon_honors_port_flag() {
    let port = free_port();
    let (mut cmd, home, db_path, index_dir) = spawn_daemon(port, None, "neutral");
    let mut child = cmd
        .spawn()
        .expect("spawn knocode-daemon with --port");

    let addr = format!("127.0.0.1:{port}");
    let body = wait_for_state(&addr, "ready", Duration::from_secs(30))
        .unwrap_or_else(|| panic!("daemon never reached ready /health on {addr} within 30s"));

    // Clean up the spawned daemon even when the assertions below fail.
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_dir_all(&index_dir);

    assert!(body.contains("HTTP/1.1 200"), "expected 200, got: {body}");
    assert!(
        body.contains("\"status\":\"ok\""),
        "health payload missing status ok: {body}"
    );
    assert!(
        body.contains("\"index_files\""),
        "health payload missing index_files: {body}"
    );
}

#[test]
fn daemon_eager_repo_reaches_ready_and_indexes() {
    // A minimal repo: one source file for the eager index to pick up.
    let repo = std::env::temp_dir().join(format!(
        "knocode_port_flag_repo_{}_{}",
        std::process::id(),
        unique_suffix()
    ));
    std::fs::create_dir_all(repo.join("src")).expect("create temp repo");
    std::fs::write(
        repo.join("src").join("lib.rs"),
        "pub fn hello() -> &'static str { \"world\" }\n",
    )
    .expect("write source file");

    let port = free_port();
    let (mut cmd, home, db_path, index_dir) = spawn_daemon(port, Some(&repo), "eager");
    let mut child = cmd.spawn().expect("spawn knocode-daemon with --repo");

    let addr = format!("127.0.0.1:{port}");
    // Eager indexing is readiness-gated: /health says "indexing" while it runs,
    // then flips to "ready". One file — should be quick.
    let body = wait_for_state(&addr, "ready", Duration::from_secs(60))
        .unwrap_or_else(|| panic!("daemon never reached ready /health on {addr} within 60s"));

    let ok = body.contains("HTTP/1.1 200") && body.contains("\"status\":\"ok\"");

    // Cleanup before asserting.
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&home);
    let _ = std::fs::remove_file(&db_path);
    let _ = std::fs::remove_dir_all(&index_dir);
    let _ = std::fs::remove_dir_all(&repo);

    assert!(ok, "expected 200 ok, got: {body}");
    // The eager repo was indexed: index_files must reflect its file(s),
    // not the daemon home (the old bug: `knocode serve` indexed ~/.knocode).
    let files: usize = body
        .split("\"index_files\":")
        .nth(1)
        .and_then(|rest| rest.split([',', '}']).next())
        .and_then(|n| n.trim().parse().ok())
        .unwrap_or(0);
    assert!(
        files >= 1,
        "eager repo not indexed — index_files=0 in /health: {body}"
    );
}
