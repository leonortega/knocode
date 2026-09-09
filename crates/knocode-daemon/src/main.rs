#![allow(linker_messages)]
// knocode-daemon: daemon binary — HTTP server, signal handling, process lifecycle

use std::path::PathBuf;

use knocode_core::Config;
use knocode_daemon::lifecycle::{DaemonState, DEFAULT_HTTP_PORT};

/// Parse a `--port <N>` / `--port=<N>` override from the daemon's CLI args.
///
/// Unknown args are ignored so launchers can pass extra flags safely. A
/// malformed explicit port is a hard error — silently falling back to the
/// default port is exactly the bug this flag exists to fix.
fn parse_port_arg(args: &[String]) -> Result<Option<u16>, String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--port=") {
            return parse_port_value(value);
        }
        if arg == "--port" {
            return match iter.next() {
                Some(value) => parse_port_value(value),
                None => Err("--port requires a value (e.g. knocode-daemon --port 9527)".to_string()),
            };
        }
    }
    Ok(None)
}

fn parse_port_value(value: &str) -> Result<Option<u16>, String> {
    let port: u16 = value
        .parse()
        .map_err(|_| format!("invalid --port value {value:?}: must be 1-65535"))?;
    if port == 0 {
        return Err(format!("invalid --port value {value:?}: must be 1-65535"));
    }
    Ok(Some(port))
}

/// Parse one or more `--repo <path>` / `--repo=<path>` flags: repositories to index
/// + watch EAGERLY at startup (multi-repo daemon — one daemon serves all repos).
/// Without any `--repo`, the daemon starts repo-neutral and indexes repositories
/// lazily on their first request. The FIRST path is also chdir'd into before config
/// load, preserving the legacy config-resolution behavior of `knocode serve`.
fn parse_repo_args(args: &[String]) -> Result<Vec<PathBuf>, String> {
    let mut repos = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if let Some(value) = arg.strip_prefix("--repo=") {
            repos.push(PathBuf::from(value));
            continue;
        }
        if arg == "--repo" {
            match iter.next() {
                Some(value) => repos.push(PathBuf::from(value)),
                None => {
                    return Err("--repo requires a value (e.g. knocode-daemon --repo C:/path/to/repo)"
                        .to_string())
                }
            }
        }
    }
    Ok(repos)
}

#[tokio::main]
async fn main() {
    // CLI overrides (parsed before config load so a bad --port fails fast)
    let args: Vec<String> = std::env::args().skip(1).collect();
    let port_override = match parse_port_arg(&args) {
        Ok(port) => port,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };

    // Load configuration
    // Optional eager repositories (`knocode serve --repo ...`). Chdir to the FIRST
    // one before config load so `[index].watch_mode`, context limits etc. resolve
    // from that repo's config (legacy `knocode serve` behavior preserved).
    let eager_repos = match parse_repo_args(&args) {
        Ok(repos) => repos,
        Err(e) => {
            eprintln!("{e}");
            std::process::exit(1);
        }
    };
    if let Some(first) = eager_repos.first() {
        if !first.is_dir() {
            eprintln!("--repo path is not a directory: {}", first.display());
            std::process::exit(1);
        }
        if let Err(e) = std::env::set_current_dir(first) {
            eprintln!("Failed to chdir to {}: {e}", first.display());
            std::process::exit(1);
        }
    }

    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let config = match Config::load(&project_root) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("Failed to load configuration: {}", e);
            std::process::exit(1);
        }
    };

    // Validate configuration
    if let Err(e) = config.validate() {
        eprintln!("Invalid configuration: {}", e);
        std::process::exit(1);
    }

    // Initialize daemon state (multi-repo: context engine, watcher registry, etc.)
    let state = match DaemonState::initialize(config, eager_repos) {
        Ok(state) => state,
        Err(e) => {
            eprintln!("Failed to initialize daemon: {}", e);
            std::process::exit(1);
        }
    };

    // Start the daemon (HTTP server + background indexing + signal handling)
    let http_port = port_override.unwrap_or(DEFAULT_HTTP_PORT);
    if let Err(e) = state.serve_on(http_port).await {
        eprintln!("Daemon error: {}", e);
        std::process::exit(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_port_flag_yields_none() {
        assert_eq!(parse_port_arg(&args(&[])).unwrap(), None);
        assert_eq!(parse_port_arg(&args(&["--config", "x"])).unwrap(), None);
    }

    #[test]
    fn space_and_eq_forms_parse() {
        assert_eq!(parse_port_arg(&args(&["--port", "9599"])).unwrap(), Some(9599));
        assert_eq!(parse_port_arg(&args(&["--port=9599"])).unwrap(), Some(9599));
    }

    #[test]
    fn malformed_port_is_hard_error_not_silent_default() {
        assert!(parse_port_arg(&args(&["--port", "abc"])).is_err());
        assert!(parse_port_arg(&args(&["--port=0"])).is_err());
        assert!(parse_port_arg(&args(&["--port"])).is_err());
    }
}
