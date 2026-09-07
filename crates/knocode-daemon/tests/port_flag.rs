//! Regression net for `knocode serve --port N` → `knocode-daemon` pass-through.
//!
//! The daemon used to hardcode 9527 and ignore the flag entirely, so
//! `knocode serve --port 9599` health-waited against a port nothing bound.
//! This spawns the REAL daemon binary with `--port <ephemeral>` and asserts
//! GET /health answers on exactly that port.

use std::io::{Read, Write};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

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
            format!("GET {path} HTTP/1.1\r\nHost: {addr}\r\nConnection: close\r\n\r\n").as_bytes(),
        )
        .ok()?;
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok()?;
    let mut buf = String::new();
    stream.read_to_string(&mut buf).ok()?;
    Some(buf)
}

#[test]
fn daemon_honors_port_flag() {
    // Empty temp cwd: nothing to index, no knocode.json surprises.
    let cwd = std::env::temp_dir().join(format!("knocode_port_flag_{}", std::process::id()));
    std::fs::create_dir_all(&cwd).unwrap();

    let port = free_port();
    let mut child = Command::new(env!("CARGO_BIN_EXE_knocode-daemon"))
        .arg("--port")
        .arg(port.to_string())
        .current_dir(&cwd)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn knocode-daemon with --port");

    let addr = format!("127.0.0.1:{port}");
    let deadline = Instant::now() + Duration::from_secs(30);
    let mut health: Option<String> = None;
    while Instant::now() < deadline {
        if let Some(body) = http_get(&addr, "/health") {
            if body.contains("\"status\":\"ok\"") {
                health = Some(body);
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    // Clean up the spawned daemon even when the assertions below fail.
    let _ = child.kill();
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&cwd);

    let body = health
        .unwrap_or_else(|| panic!("daemon never answered /health on {addr} within 30s"));
    assert!(body.contains("HTTP/1.1 200"), "expected 200, got: {body}");
    assert!(
        body.contains("\"state\""),
        "health payload missing readiness state: {body}"
    );
}
