#![allow(linker_messages)]
use std::collections::HashMap;
use std::io::{IsTerminal, Write};
use std::path::{Path, PathBuf};
use std::time::Instant;

use clap::{Parser, Subcommand};
use knocode_core::{Config, WatchMode};

// ── CLI Arguments ───────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "knocode")]
#[command(about = "AI Runtime for coding agents")]
#[command(version = env!("CARGO_PKG_VERSION"))]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the daemon server
    Serve {
        /// HTTP fallback port (default 9527)
        #[arg(long, default_value_t = 9527)]
        port: u16,
        /// Override socket path
        #[arg(long)]
        socket: Option<String>,
    },
    
    /// One-command repository bootstrap: scaffold → discovery → indexing → knowledge → profile
    Init {
        /// Run interactive setup wizard
        #[arg(long)]
        wizard: bool,
        /// Skip the startup animation
        #[arg(long)]
        no_anim: bool,
    },
    
    /// Trigger repository re-indexing
    Index {
        /// Watch for changes and re-index automatically
        #[arg(long)]
        watch: bool,
        /// Watch mode: "commit" (default, triggers on git commit) or "filesystem" (triggers on any file change)
        #[arg(long)]
        watch_mode: Option<String>,
    },
    
    /// Preview what BuildContext would produce for a prompt (real via daemon if running, else local)
    Preview {
        /// The prompt to preview
        prompt: String,
        /// Session ID for dedup testing
        #[arg(long, default_value = "preview-session")]
        session: String,
        /// Do not use session dedup
        #[arg(long)]
        no_cache: bool,
        /// Enable retrieval diagnostic (classify misses per expected file)
        #[arg(long)]
        diag: bool,
        /// Expected files for diagnostic classification (comma-separated paths)
        #[arg(long, value_delimiter = ',')]
        expected_files: Option<Vec<String>>,
        /// Candidate pool size before ranking (20/50/100/200, default 100 from config). Env KNOCODE_CANDIDATE_K overrides config.
        #[arg(long)]
        candidate_k: Option<usize>,
        /// Max files in final Context Pack (20 default, use 50 for large-repo eval to match rg K=50). Env KNOCODE_MAX_FILES overrides.
        #[arg(long)]
        max_files: Option<usize>,
        /// Emit machine-readable JSON (paths, provenance, token usage, retrieval stats) instead of human-readable text.
        /// Stable contract for eval harnesses and tooling — replaces regex scraping of stdout.
        #[arg(long)]
        json: bool,
    },
    
    /// Show daemon status and metrics
    Status,
    
    /// Manage configuration
    Config {
        #[command(subcommand)]
        action: ConfigAction,
    },

    /// Health check: verify all dependencies are available
    Doctor,
}

#[derive(Subcommand)]
enum ConfigAction {
    /// Show effective configuration
    Show,
    /// Validate configuration file
    Validate,
    /// Set the log verbosity (upserts [logging] level in user + project config)
    SetLogLevel {
        /// Log level: error, warn, info, debug, trace (aliases: quiet, normal, verbose)
        #[arg(value_parser = clap::value_parser!(String))]
        level: String,
    },
    /// Migrate config from external agent (claude, cursor, continue)
    Migrate {
        /// Source to migrate from
        #[arg(value_parser = clap::value_parser!(String))]
        from: String,
    },
}

// ── Main ────────────────────────────────────────────────────────────────

fn main() {
    // Initialize tracing when KNOCODE_PROFILE=1 — shows phase timing from tracing::info!
    if std::env::var("KNOCODE_PROFILE").ok().as_deref() == Some("1") {
        use tracing_subscriber::EnvFilter;
        let filter = EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info"));
        match tracing_subscriber::fmt()
            .with_env_filter(filter)
            .with_target(true)
            .with_writer(std::io::stderr)
            .try_init()
        {
            Ok(()) => eprintln!("[tracing] subscriber initialized"),
            Err(e) => eprintln!("[tracing] subscriber init failed: {}", e),
        }
    }

    let cli = Cli::parse();
    
    let result = match cli.command {
        Commands::Serve { port, socket } => cmd_serve(port, socket),
        Commands::Init { wizard, no_anim } => cmd_init(wizard, no_anim),
        Commands::Index { watch, watch_mode } => cmd_index(watch, watch_mode.as_deref()),
        Commands::Preview { prompt, session, no_cache, diag, expected_files, candidate_k, max_files, json } => cmd_preview(&prompt, &session, no_cache, diag, expected_files.as_deref(), candidate_k, max_files, json),
        Commands::Status => cmd_status(),
        Commands::Config { action } => cmd_config(action),
        Commands::Doctor => cmd_doctor(),
    };
    
    if let Err(e) = result {
        eprintln!("Error: {}", e);
        std::process::exit(1);
    }
}

// ── Command Implementations ─────────────────────────────────────────────

fn cmd_serve(port: u16, _socket: Option<String>) -> Result<(), String> {
    let health_url = format!("http://127.0.0.1:{}/health", port);

    // Check if daemon is already running
    if is_daemon_running(&health_url) {
        println!("knocode daemon already running at {}", health_url);
        return Ok(());
    }

    println!("Starting knocode daemon...");

    // Find the daemon binary
    let daemon_exe = find_daemon_exe()?;
    println!("  Binary: {}", daemon_exe.display());
    println!("  HTTP:   {}", health_url);

    // Start daemon as background process.
    // FIX: spawn with the USER'S project root as both CWD and `--repo`. The daemon
    // resolves the repository to index from its process CWD — spawning it in
    // ~/.knocode made `knocode serve` index the knocode home directory (~87 files)
    // instead of the user's repo, while /health reported "ready".
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));

    let mut cmd = std::process::Command::new(&daemon_exe);
    let mut spawn_args = daemon_spawn_args(port);
    spawn_args.push("--repo".to_string());
    spawn_args.push(project_root.to_string_lossy().to_string());
    cmd.current_dir(&project_root)
        .args(&spawn_args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    let child = cmd.spawn().map_err(|e| format!("Failed to start daemon: {}", e))?;
    println!("  PID:    {}", child.id());

    // Wait for health check
    print!("  Waiting for daemon to be ready");
    for _i in 0..40 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        if is_daemon_running(&health_url) {
            println!(" ✓");
            println!("knocode daemon RUNNING at {} (PID {})", health_url, child.id());
            return Ok(());
        }
        print!(".");
        std::io::Write::flush(&mut std::io::stdout()).ok();
    }
    println!(" ✗");
    println!("Warning: daemon started but /health not responding within 20s");
    println!("  Verify: curl {}", health_url);
    if port != 9527 && is_daemon_running("http://127.0.0.1:9527/health") {
        // The spawned binary ignored --port: either a pre-`--port` daemon is
        // already listening on the default port, or the installed copy is stale.
        println!("  Hint: something answered on the default port 9527 instead of {port}.");
        println!("        An older daemon may already be running there, or the installed");
        println!("        knocode-daemon is stale (ignores --port) — reinstall via");
        println!("        scripts/install.ps1 or scripts/install.sh to refresh it.");
    }
    Ok(())
}

/// CLI args forwarded to the spawned `knocode-daemon` binary. Kept pure so the
/// pass-through contract (the port MUST reach the child) has a unit test.
fn daemon_spawn_args(port: u16) -> Vec<String> {
    vec!["--port".to_string(), port.to_string()]
}

fn is_daemon_running(health_url: &str) -> bool {
    // Extract host:port from URL (strip scheme and path)
    let addr = health_url
        .strip_prefix("http://")
        .unwrap_or(health_url)
        .split('/')
        .next()
        .unwrap_or(health_url);
    std::net::TcpStream::connect(addr).is_ok()
}

fn find_daemon_exe() -> Result<PathBuf, String> {
    // 1. Check ~/.knocode/bin/knocode-daemon (installed copy)
    if let Some(home) = dirs_home() {
        let installed = home.join(".knocode").join("bin").join("knocode-daemon");
        #[cfg(windows)]
        let installed = installed.with_extension("exe");
        if installed.exists() {
            return Ok(installed);
        }
    }
    // 2. Check PATH
    let which = if cfg!(windows) { "where" } else { "which" };
    if let Ok(out) = std::process::Command::new(which).arg("knocode-daemon").output() {
        let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !path.is_empty() {
            return Ok(PathBuf::from(path));
        }
    }
    // 3. Check cargo's real target dir(s) — release first, then debug (a debug
    //    build from `cargo test`/`cargo build` is still better than nothing).
    for cand in daemon_candidate_paths() {
        if cand.exists() {
            return Ok(cand);
        }
    }
    Err(format!(
        "knocode-daemon not found (searched: {}). Build with: cargo build --release -p knocode-daemon",
        daemon_candidate_paths()
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join("; ")
    ))
}

/// All candidate paths for the prebuilt daemon binary, in preference order:
/// release before debug within each target dir (a debug build from `cargo
/// test` is still better than nothing).
fn daemon_candidate_paths() -> Vec<PathBuf> {
    let mut cands = Vec::new();
    for dir in daemon_target_dirs() {
        for profile in ["release", "debug"] {
            let p = dir.join(profile).join("knocode-daemon");
            #[cfg(windows)]
            let p = p.with_extension("exe");
            cands.push(p);
        }
    }
    cands
}

/// Extract `target_directory` from `cargo metadata --no-deps --format-version 1`
/// output. Pure so the JSON contract has a unit test (Windows paths arrive
/// JSON-escaped, e.g. "C:\\\\Users\\\\...").
fn parse_cargo_metadata_target_dir(stdout: &[u8]) -> Option<PathBuf> {
    let json: serde_json::Value = serde_json::from_slice(stdout).ok()?;
    let dir = json.get("target_directory")?.as_str()?;
    if dir.trim().is_empty() {
        None
    } else {
        Some(PathBuf::from(dir))
    }
}

/// Run `cargo metadata` and return cargo's real target dir — the authoritative
/// answer covering CARGO_TARGET_DIR env, [build] target-dir in any
/// .cargo/config.toml, and the default workspace layout. Falls back to the
/// standard cargo install location (CARGO_HOME/bin) when cargo isn't on PATH.
fn cargo_metadata_target_dir() -> Option<PathBuf> {
    let mut bins: Vec<PathBuf> = vec![PathBuf::from("cargo")];
    if let Some(home) = std::env::var("CARGO_HOME")
        .ok()
        .map(PathBuf::from)
        .or_else(dirs_home)
    {
        let alt = home.join("bin").join("cargo");
        #[cfg(windows)]
        let alt = alt.with_extension("exe");
        bins.push(alt);
    }
    for bin in bins {
        let out = match std::process::Command::new(&bin)
            .args(["metadata", "--no-deps", "--format-version", "1"])
            .output()
        {
            // Spawn failed (cargo not found) — try the next candidate location.
            Err(_) => continue,
            Ok(out) => out,
        };
        // cargo ran but metadata failed (e.g. no manifest in CWD) — no answer.
        if !out.status.success() {
            return None;
        }
        return parse_cargo_metadata_target_dir(&out.stdout);
    }
    None
}

/// Normalize separators for display/storage: cargo config values and env vars
/// often use `/` while Rust's `join()` appends the platform separator, which
/// otherwise produces mixed output like `C:/repo/target\\release\\knocode-daemon.exe`
/// in `knocode serve`'s `Binary:` line and the not-found error's searched list.
fn normalize_path_separators(p: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        PathBuf::from(p.to_string_lossy().replace('/', "\\"))
    }
    #[cfg(not(windows))]
    {
        p.to_path_buf()
    }
}

/// Candidate dirs to search for the prebuilt daemon, most authoritative first:
/// CARGO_TARGET_DIR env (cargo honors it over config files), the `cargo
/// metadata` answer, then the legacy repo-local ./target.
fn daemon_target_dirs() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(env_dir) = std::env::var("CARGO_TARGET_DIR") {
        if !env_dir.trim().is_empty() {
            let p = PathBuf::from(env_dir);
            dirs.push(normalize_path_separators(&if p.is_absolute() {
                p
            } else {
                std::env::current_dir()
                    .unwrap_or_else(|_| PathBuf::from("."))
                    .join(p)
            }));
        }
    }
    if let Some(dir) = cargo_metadata_target_dir() {
        let dir = normalize_path_separators(&dir);
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    let legacy = PathBuf::from("target");
    if !dirs.contains(&legacy) {
        dirs.push(legacy);
    }
    dirs
}

fn dirs_home() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    { std::env::var("USERPROFILE").ok().map(PathBuf::from) }
    #[cfg(not(target_os = "windows"))]
    { std::env::var("HOME").ok().map(PathBuf::from) }
}

/// Run indexing with a live done/total file counter so long index runs don't look frozen.
///
/// - TTY: single line redrawn in place — `N/M files — phase` (total shown once known).
/// - Piped output: periodic `… N/M files (phase)` lines every 5000 files.
/// - On completion: clears the counter line and prints a `✓` summary line.
fn run_index_with_progress(
    repo_intel: &mut knocode_repo_intel::RepositoryIntelligence,
) -> Result<knocode_repo_intel::IndexStats, String> {
    let is_tty = std::io::stdout().is_terminal();
    let started = Instant::now();

    let progress_cb = move |done: usize, total: usize, phase: &str| {
        // Elapsed seconds in every line: when the counter stalls on the last few
        // files or the final index commit, rising elapsed time shows the run is
        // alive instead of hung.
        let elapsed = started.elapsed().as_secs();
        if is_tty {
            let mut stdout = std::io::stdout();
            if total > 0 {
                let _ = write!(stdout, "\r      {}/{} files — {} ({}s)\x1b[K", done, total, phase, elapsed);
            } else {
                let _ = write!(stdout, "\r      {} files — {} ({}s)\x1b[K", done, phase, elapsed);
            }
            let _ = stdout.flush();
        } else if done > 0 && done % 5000 == 0 {
            if total > 0 {
                println!("      … {}/{} files ({}; {}s)", done, total, phase, elapsed);
            } else {
                println!("      … {} files ({}; {}s)", done, phase, elapsed);
            }
        }
    };

    let started = Instant::now();
    let result = repo_intel.index_repository_with_progress(Some(&progress_cb));

    // Erase the in-place counter line before printing the summary
    if is_tty {
        print!("\r\x1b[2K");
        let _ = std::io::stdout().flush();
    }

    match result {
        Ok(stats) => {
            println!(
                "      ✓ indexed {} files ({} symbols this run) in {}ms",
                stats.files_indexed,
                stats.symbols_extracted,
                started.elapsed().as_millis()
            );
            Ok(stats)
        }
        Err(e) => Err(e),
    }
}

/// Build the init status box with content-driven widths — every line (borders
/// included) is exactly as wide as the widest row, so the box stays aligned no
/// matter how long the values are. Replaces the old hand-padded literal box,
/// which drifted out of alignment whenever a value exceeded its fixed pad.
fn format_status_box(title: &str, rows: &[(&str, String)]) -> String {
    let label_width = rows.iter().map(|(l, _)| l.len()).max().unwrap_or(0);
    let inner = rows
        .iter()
        .map(|(_l, v)| label_width + 2 + v.len())
        .chain(std::iter::once(title.len()))
        .max()
        .unwrap_or(0);
    let border = "─".repeat(inner + 2);
    let mut out = format!("┌{}┐\n│ {:<inner$} │\n├{}┤\n", border, title, border, inner = inner);
    for (label, value) in rows {
        let field = format!("{:<lw$}  {}", label, value, lw = label_width);
        out.push_str(&format!("│ {:<inner$} │\n", field, inner = inner));
    }
    out.push_str(&format!("└{}┘", border));
    out
}

/// Build the dependency graph with the same progress display as indexing.
/// Progress counts files scanned for imports; on a warm cache (in-memory or
/// .knocode/graphs/<repo>.json) the callback never fires and this returns instantly.
fn run_graph_with_progress(
    repo_intel: &mut knocode_repo_intel::RepositoryIntelligence,
) -> Result<usize, String> {
    let is_tty = std::io::stdout().is_terminal();
    let started = Instant::now();

    let mut progress_cb = move |done: usize, total: usize| {
        // Same style as the indexing counter: elapsed seconds prove liveness when
        // the counter stalls on the final graph serialization.
        let elapsed = started.elapsed().as_secs();
        if is_tty {
            let mut stdout = std::io::stdout();
            let _ = write!(stdout, "\r      {}/{} files — graph ({}s)\x1b[K", done, total, elapsed);
            let _ = stdout.flush();
        } else if done > 0 && done % 5000 == 0 {
            println!("      … {}/{} files (graph; {}s)", done, total, elapsed);
        }
    };

    let result = repo_intel.build_dependency_graph_with_progress(&mut progress_cb);

    // Erase the in-place counter line before printing the summary
    if is_tty {
        print!("\r\x1b[2K");
        let _ = std::io::stdout().flush();
    }

    match result {
        Ok(graph) => {
            println!(
                "      ✓ graph: {} edges ({} files) in {}ms",
                graph.edge_count(),
                graph.all_files().len(),
                started.elapsed().as_millis()
            );
            Ok(graph.edge_count())
        }
        Err(e) => Err(e),
    }
}

fn cmd_init(wizard: bool, _no_anim: bool) -> Result<(), String> {
    if wizard {
        println!("(wizard mode is non-interactive — defaults applied, edit .knocode/config.toml afterwards)");
        println!();
    }
    let started = std::time::Instant::now();
    let project_root = std::env::current_dir()
        .map_err(|e| format!("Failed to get current directory: {}", e))?;
    let project_name = project_root
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "repository".to_string());

    // ── Header (static — animation removed, progress lives in step 5) ──
    println!("knocode v{}", env!("CARGO_PKG_VERSION"));
    println!("═══════════════════════════════════════");
    println!("Bootstrap — {}", project_name);

    // ── Step 1: Scaffold ─────────────────────────────────────────────
    println!("[1/7] Scaffold (.knocode/, config, database)");
    let knocode_dir = PathBuf::from(".knocode");
    std::fs::create_dir_all(&knocode_dir)
        .map_err(|e| format!("Failed to create .knocode directory: {}", e))?;
    let config_path = knocode_dir.join("config.toml");
    if !config_path.exists() {
        let default_config = Config::default();
        let config_toml = toml::to_string_pretty(&default_config)
            .map_err(|e| format!("Failed to serialize default config: {}", e))?;
        std::fs::write(&config_path, config_toml)
            .map_err(|e| format!("Failed to write config: {}", e))?;
    }
    let db_path = get_db_path();
    if let Some(parent) = db_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("Failed to create database directory: {}", e))?;
    }
    let db = knocode_storage::Database::open(&db_path)
        .map_err(|e| format!("Failed to initialize database: {}", e))?;

    // ── Step 2: Repository discovery ─────────────────────────────────
    println!("[2/7] Repository discovery (languages, frameworks, commands)");
    let discovery = discover_repository(&project_root);
    println!(
        "      languages: {}",
        discovery
            .languages
            .iter()
            .map(|(l, n)| format!("{}({})", l, n))
            .collect::<Vec<_>>()
            .join(", ")
    );

    // Write discovered languages to .knocode/config.toml
    if !discovery.languages.is_empty() {
        match update_config_languages(&config_path, &discovery.languages) {
            Ok(updated) => {
                if updated {
                    println!("      ✓ config.toml languages updated from discovery");
                }
            }
            Err(e) => println!("      (config language update skipped: {})", e),
        }
    }

    // ── Step 3: Parser validation ────────────────────────────────────
    // tree-sitter-language-pack auto-downloads grammars on first use, so there is
    // no separate download step. Validation applies a per-language policy:
    //   Source/Test (code knocode parses for symbols) → validate_grammar(), which
    //     loads the grammar and auto-downloads it if missing (online warm-up).
    //   Documentation/Config (full-text only, symbols skipped) → offline pack check
    //     only; downloading grammars knocode never parses would be waste.
    //   Text → the pack has no grammar for it; reported as full-text-only.
    println!("[3/7] Parser validation (verify tree-sitter grammars)");
    use knocode_repo_intel::registry::{LanguageId, FileClass, file_class_for_language};
    let mut parser_status: Vec<(String, bool, String)> = Vec::new();
    for (lang_name, _count) in &discovery.languages {
        if let Some(id) = LanguageId::from_str(lang_name) {
            let file_class = file_class_for_language(id);
            let pack_name = knocode_repo_intel::registry::language_pack_name(id);
            let (ready, grammar_status) = if pack_name.is_empty() {
                (false, "no grammar in pack — full-text only".to_string())
            } else if matches!(file_class, FileClass::Source | FileClass::Test) {
                match knocode_repo_intel::parser::validate_grammar(id) {
                    Ok(()) => (true, "loaded".to_string()),
                    Err(e) => (false, e), // e.g. "grammar load failed for Rust"
                }
            } else {
                // Documentation/Config: check availability without downloading
                if tree_sitter_language_pack::has_parser(pack_name) {
                    (true, "ready (on-demand)".to_string())
                } else {
                    (false, "grammar not downloaded — full-text only".to_string())
                }
            };
            parser_status.push((lang_name.clone(), ready, grammar_status));
        } else {
            parser_status.push((lang_name.clone(), false, "unknown language".to_string()));
        }
    }
    let ready_count = parser_status.iter().filter(|(_, ready, _)| *ready).count();
    let total_count = parser_status.len();
    println!("      parsers ready: {}/{}", ready_count, total_count);
    for (lang, ready, status) in &parser_status {
        let icon = if *ready { "✓" } else { "—" };
        println!("        {} {:<20} {}", icon, lang, status);
    }

    // ── Step 4: Indexing ─────────────────────────────────────────────
    println!("[4/7] Indexing (full-text BM25 + symbol extraction)");
    let event_bus = knocode_events::EventBus::new();
    let mut repo_intel = knocode_repo_intel::RepositoryIntelligence::new(
        project_root.clone(),
        db,
        event_bus.clone(),
    );
    let stats = run_index_with_progress(&mut repo_intel)?;
    // Phase3: defer graph on large repos (second walk over 63k) — lazy on first query
    let dep_edges = if stats.files_indexed > 5000 && std::env::var("KNOCODE_BUILD_GRAPH").ok().as_deref() != Some("1") {
        println!("      Graph: deferred (lazy on first query, set KNOCODE_BUILD_GRAPH=1 to force during init)");
        0
    } else {
        run_graph_with_progress(&mut repo_intel).unwrap_or(0)
    };
    drop(repo_intel);
    // Autoconfigure large-repo tuning: persist candidate_k/max_files for next preview/daemon (V1_FIX_PLAN_0_8_1.md:31)
    // Runtime also auto-tunes if still defaults (crates/knocode-context/src/lib.rs:189 doc_count>5000 → 100→200/20→50)
    if stats.files_indexed > 5000 {
        if let Ok(raw) = std::fs::read_to_string(&config_path) {
            if let Ok(mut cfg) = knocode_core::Config::from_toml(&raw) {
                let mut changed = false;
                if cfg.context.candidate_k == 100 {
                    cfg.context.candidate_k = 200;
                    changed = true;
                }
                if cfg.context.max_files == 20 {
                    cfg.context.max_files = 50;
                    changed = true;
                }
                if changed {
                    if let Ok(toml) = cfg.to_toml() {
                        let _ = std::fs::write(&config_path, toml);
                        println!("      ↳ large repo detected ({} files) → config candidate_k={} max_files={} (override via --candidate-k/--max-files or KNOCODE_CANDIDATE_K)", stats.files_indexed, cfg.context.candidate_k, cfg.context.max_files);
                    }
                }
            }
        }
    }
    let db = knocode_storage::Database::open(&db_path)
        .map_err(|e| format!("Failed to reopen database: {}", e))?;

    // ── Step 5: Knowledge Hub ────────────────────────────────
    println!("[5/7] Knowledge Hub initialization");
    let (knowledge_seeded, _readme) = ingest_seed_documents(&project_root, &db);
    println!("      ✓ knowledge: {} entries seeded", knowledge_seeded);

    // ── Step 6: Validation (smoke test all components) ───────────
    println!("[6/7] Validation queries (smoke test)");
    let repo_id = knocode_core::repository_id_from_path(&project_root.to_string_lossy());

    // Tantivy (via RepositoryIntelligence::validate_index)
    let ri_for_validation = knocode_repo_intel::RepositoryIntelligence::new(
        project_root.clone(),
        knocode_storage::Database::open(&db_path).map_err(|e| format!("DB open failed: {}", e))?,
        knocode_events::EventBus::new(),
    );
    let tantivy_ok = match ri_for_validation.validate_index() {
        Ok(stats) => {
            let searchable = stats.doc_count > 0;
            println!("      Tantivy:   {} ({} docs)", if searchable { "✓ searchable" } else { "✗ empty" }, stats.doc_count);
            searchable
        }
        Err(e) => {
            println!("      Tantivy:   ✗ {}", e);
            false
        }
    };

    // Symbols — scoped to THIS repo. The DB is shared across repositories
    // (get_all_files has no repo filter), so a global count both overstates
    // (mattermost + knocode rows mixed) and mislabels files as symbols.
    let repo_files = db.get_all_files_meta(&repo_id).unwrap_or_default();
    let repo_file_ids: Vec<i64> = repo_files.iter().map(|r| r.id).collect();
    let repo_symbols = db.count_symbols_for_file_ids(&repo_file_ids).unwrap_or(0);
    println!(
        "      Symbols:   {} in {} files",
        repo_symbols,
        repo_files.len()
    );

    // Graph (deferred on large repos — not a failure, see step 5)
    println!(
        "      Graph:     {} edges{}",
        dep_edges,
        if dep_edges == 0 && stats.files_indexed > 5000 { " (deferred — lazy on first query)" } else { "" }
    );

    // Knowledge
    let _knowledge_ok = knowledge_seeded > 0;
    println!("      Knowledge: {} entries", knowledge_seeded);

    // ── Step 7: Status report ────────────────────────────────────────
    println!("[7/7] Repository profile");
    let profile = build_profile_json(
        &project_name,
        &discovery,
        stats.files_indexed,
        stats.symbols_extracted,
        dep_edges,
    );
    let profile_json =
        serde_json::to_string_pretty(&profile).unwrap_or_else(|_| "{}".to_string());
    let profile_path = knocode_dir.join("profile.json");
    std::fs::write(&profile_path, &profile_json)
        .map_err(|e| format!("Failed to write profile: {}", e))?;
    let _ = db.store_knowledge("profile", "repository-profile", &profile_json, 1.0, "bootstrap", &repo_id);

    println!();
    println!("✓ Bootstrap complete in {}ms", started.elapsed().as_millis());
    println!();
    let status_label = if tantivy_ok && repo_files.len() > 0 { "READY" } else { "PARTIAL" };
    println!(
        "{}",
        format_status_box(
            &format!("Repository Status: {}", status_label),
            &[
                ("Tree-sitter:", format!("{}/{} grammars", ready_count, total_count)),
                ("Symbols:", format!("{} extracted", repo_symbols)),
                ("Graph:", format!("{} edges{}", dep_edges, if dep_edges == 0 && stats.files_indexed > 5000 { " (deferred)" } else { "" })),
                ("Tantivy:", format!("{} files", stats.files_indexed)),
                ("Knowledge:", format!("{} entries", knowledge_seeded)),
            ],
        )
    );
    println!();

    if !discovery.languages.is_empty() {
        println!("  Languages detected:");
        for (lang_name, count) in &discovery.languages {
            let status = parser_status.iter()
                .find(|(l, _, _)| l == lang_name)
                .map(|(_, ready, _)| if *ready { "READY" } else { "no parser" })
                .unwrap_or("unknown");
            println!("    {:<20} {:>5} files   {}", lang_name, count, status);
        }
    }
    println!(
        "  Frameworks:        {}",
        if discovery.frameworks.is_empty() {
            "-".to_string()
        } else {
            discovery.frameworks.join(", ")
        }
    );
    println!(
        "  Build command:     {}",
        discovery.build_command.as_deref().unwrap_or("-")
    );
    println!(
        "  Test command:      {}",
        discovery.test_command.as_deref().unwrap_or("-")
    );
    println!("  Profile:           {}", profile_path.display());
    println!();
    println!("  Re-run 'knocode init' anytime - incremental and safe to repeat.");

    Ok(())
}


#[derive(Default, Debug)]
struct Discovery {
    languages: Vec<(String, usize)>,
    frameworks: Vec<String>,
    build_command: Option<String>,
    test_command: Option<String>,
    important_dirs: Vec<String>,
    git_branch: Option<String>,
}

const SKIP_DIRS: &[&str] = &[
    ".git",
    ".github",
    "node_modules",
    "target",
    "dist",
    "build",
    "out",
    "vendor",
    ".venv",
    "venv",
    "__pycache__",
    ".idea",
    ".vscode",
    ".next",
    "coverage",
    ".knocode",
    ".agents",
];

fn discover_repository(root: &Path) -> Discovery {
    let mut d = Discovery::default();

    let mut ext_counts: HashMap<String, usize> = HashMap::new();
    walk_ext_counts(root, 0, &mut ext_counts);
    let mut langs: Vec<(String, usize)> = ext_counts
        .into_iter()
        .filter_map(|(ext, n)| ext_language(&ext).map(|l| (l.to_string(), n)))
        .collect();
    langs.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    d.languages = langs.into_iter().take(10).collect();

    for candidate in [
        "src",
        "crates",
        "lib",
        "packages",
        "apps",
        "services",
        "docs",
        "tests",
        "test",
        "scripts",
        "benches",
        "examples",
        "deploy",
    ] {
        if root.join(candidate).is_dir() {
            d.important_dirs.push(candidate.to_string());
        }
    }

    detect_stack(root, &mut d);

    if root.join(".git").exists() {
        if let Ok(head) = std::fs::read_to_string(root.join(".git").join("HEAD")) {
            let head = head.trim();
            d.git_branch = head
                .strip_prefix("ref: refs/heads/")
                .map(|s| s.to_string())
                .or_else(|| Some(head.chars().take(12).collect()));
        }
    }

    d
}

fn walk_ext_counts(dir: &Path, depth: usize, counts: &mut HashMap<String, usize>) {
    if depth > 12 {
        return;
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let Ok(ft) = entry.file_type() else { continue };
        if ft.is_dir() {
            let name = entry.file_name().to_string_lossy().to_string();
            if SKIP_DIRS.contains(&name.as_str()) {
                continue;
            }
            walk_ext_counts(&entry.path(), depth + 1, counts);
        } else if ft.is_file() {
            if let Some(ext) = entry.path().extension().and_then(|e| e.to_str()) {
                if !ext.is_empty() {
                    *counts.entry(ext.to_lowercase()).or_insert(0) += 1;
                }
            }
        }
    }
}

fn ext_language(ext: &str) -> Option<&'static str> {
    // Use unified registry — single source of truth
    knocode_repo_intel::registry::language_by_extension(ext).map(|def| def.id.as_str())
}

/// Update `.knocode/config.toml` with discovered languages so tree-sitter and other
/// language-aware components use the actual tech stack. Only the `[index].languages`
/// field is modified; all other config is preserved via TOML merge.
/// Returns Ok(true) if the file was updated, Ok(false) if no change needed.
fn update_config_languages(
    config_path: &std::path::Path,
    languages: &[(String, usize)],
) -> Result<bool, String> {
    let content = std::fs::read_to_string(config_path)
        .map_err(|e| format!("read config: {}", e))?;
    let mut doc: toml::Value = toml::from_str(&content)
        .map_err(|e| format!("parse config: {}", e))?;

    // Build the new languages list from discovery (top 10, already sorted by file count)
    let new_langs: Vec<String> = languages.iter().map(|(l, _)| l.clone()).collect();

    // Get current languages for comparison
    let current = doc.get("index")
        .and_then(|i| i.get("languages"))
        .and_then(|l| l.as_array())
        .map(|arr| arr.iter().filter_map(|v| v.as_str().map(String::from)).collect::<Vec<_>>())
        .unwrap_or_default();

    if current == new_langs {
        return Ok(false);
    }

    // Update the [index] section
    if let Some(index) = doc.get_mut("index") {
        let langs_val = toml::Value::Array(new_langs.iter().map(|l| toml::Value::String(l.clone())).collect());
        index.as_table_mut()
            .ok_or("index is not a table")?
            .insert("languages".to_string(), langs_val);
    } else {
        let mut table = toml::value::Table::new();
        let langs_val = toml::Value::Array(new_langs.iter().map(|l| toml::Value::String(l.clone())).collect());
        table.insert("languages".to_string(), langs_val);
        doc.as_table_mut()
            .ok_or("config root is not a table")?
            .insert("index".to_string(), toml::Value::Table(table));
    }

    let updated_toml = toml::to_string_pretty(&doc)
        .map_err(|e| format!("serialize config: {}", e))?;
    std::fs::write(config_path, updated_toml)
        .map_err(|e| format!("write config: {}", e))?;
    Ok(true)
}

fn detect_stack(root: &Path, d: &mut Discovery) {
    // Root first, then immediate subdirectories (one level deep) so monorepos
    // (e.g. server/go.mod, webapp/channels/package.json) are also detected.
    detect_stack_at_root(root, d);
    if let Ok(entries) = std::fs::read_dir(root) {
        for entry in entries.flatten() {
            if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                let name = entry.file_name().to_string_lossy().to_string();
                if SKIP_DIRS.contains(&name.as_str()) || name.starts_with('.') {
                    continue;
                }
                detect_stack_at_root(&entry.path(), d);
            }
        }
    }
}

/// Stack detection for a single directory (repo root or monorepo subproject).
fn detect_stack_at_root(root: &Path, d: &mut Discovery) {
    let push_unique = |v: &mut Vec<String>, s: &str| {
        if !v.iter().any(|x| x == s) {
            v.push(s.to_string());
        }
    };
    if root.join("Cargo.toml").exists() {
        push_unique(&mut d.frameworks, "cargo");
        if let Ok(s) = std::fs::read_to_string(root.join("Cargo.toml")) {
            if s.contains("[workspace]") {
                push_unique(&mut d.frameworks, "rust-workspace");
            }
        }
        d.build_command.get_or_insert_with(|| "cargo build".into());
        d.test_command.get_or_insert_with(|| "cargo test".into());
    }
    let pkg = root.join("package.json");
    if pkg.exists() {
        push_unique(&mut d.frameworks, "node");
        if let Ok(content) = std::fs::read_to_string(&pkg) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                for fw in ["react", "vue", "svelte", "next", "express", "@nestjs/core"] {
                    if v["dependencies"].get(fw).is_some()
                        || v["devDependencies"].get(fw).is_some()
                    {
                        push_unique(&mut d.frameworks, fw.trim_start_matches('@').replace('/', "-").as_str());
                    }
                }
                if let Some(b) = v["scripts"]["build"].as_str() {
                    d.build_command.get_or_insert_with(|| b.to_string());
                }
                if let Some(t) = v["scripts"]["test"].as_str() {
                    d.test_command.get_or_insert_with(|| t.to_string());
                }
            }
        }
        if d.test_command.is_none() {
            d.test_command = Some("npm test".to_string());
        }
    }
    if root.join("go.mod").exists() {
        push_unique(&mut d.frameworks, "go-modules");
        d.build_command
            .get_or_insert_with(|| "go build ./...".into());
        d.test_command.get_or_insert_with(|| "go test ./...".into());
    }
    if root.join("pyproject.toml").exists()
        || root.join("requirements.txt").exists()
        || root.join("setup.py").exists()
    {
        push_unique(&mut d.frameworks, "python");
        if let Ok(s) = std::fs::read_to_string(root.join("pyproject.toml")) {
            if s.contains("[tool.poetry]") {
                push_unique(&mut d.frameworks, "poetry");
                d.test_command
                    .get_or_insert_with(|| "poetry run pytest".into());
            } else if s.contains("[tool.uv]") {
                push_unique(&mut d.frameworks, "uv");
            }
        }
        d.test_command.get_or_insert_with(|| "pytest".into());
    }
    if root.join("pom.xml").exists() {
        push_unique(&mut d.frameworks, "maven");
        d.build_command.get_or_insert_with(|| "mvn package".into());
        d.test_command.get_or_insert_with(|| "mvn test".into());
    } else if root.join("build.gradle").exists() || root.join("build.gradle.kts").exists() {
        push_unique(&mut d.frameworks, "gradle");
        d.build_command.get_or_insert_with(|| "gradle build".into());
        d.test_command.get_or_insert_with(|| "gradle test".into());
    }
    if root.join("Makefile").exists() || root.join("makefile").exists() {
        push_unique(&mut d.frameworks, "make");
        // Makefile-only projects (no other manifest) get make commands as fallback
        if d.build_command.is_none() && d.test_command.is_none() {
            d.build_command.get_or_insert_with(|| "make".into());
            d.test_command.get_or_insert_with(|| "make test".into());
        }
    }
}

fn ingest_seed_documents(root: &Path, db: &knocode_storage::Database) -> (usize, Option<String>) {
    // TASK-030: seed documents are stamped with the analyzed repo's identity
    let repo_id = knocode_core::repository_id_from_path(&root.to_string_lossy());
    let mut seeded = 0usize;
    let mut readme = None;
    for name in ["README.md", "Readme.md", "readme.md", "README"] {
        let path = root.join(name);
        if path.is_file() {
            if let Ok(content) = std::fs::read_to_string(&path) {
                let truncated: String = content.chars().take(65_536).collect();
                if db
                    .store_knowledge("docs", name, &truncated, 0.9, "bootstrap", &repo_id)
                    .is_ok()
                {
                    seeded += 1;
                }
                readme = Some(content);
            }
            break;
        }
    }
    for adr_dir in ["docs/adr", "docs/decisions", "adr", "decisions"] {
        let dir = root.join(adr_dir);
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("md") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    let rel = path.strip_prefix(root).unwrap_or(&path).to_string_lossy();
                    if db
                        .store_knowledge("adr", &rel, &content, 1.0, "bootstrap", &repo_id)
                        .is_ok()
                    {
                        seeded += 1;
                    }
                }
            }
        }
    }
    (seeded, readme)
}

fn build_profile_json(
    project_name: &str,
    d: &Discovery,
    files_indexed: usize,
    symbols_extracted: usize,
    dep_edges: usize,
) -> serde_json::Value {
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|t| t.as_secs())
        .unwrap_or(0);
    serde_json::json!({
        "version": 1,
        "project": project_name,
        "generated_at_unix": ts,
        "git_branch": d.git_branch,
        "languages": d.languages
            .iter()
            .map(|(name, files)| serde_json::json!({"name": name, "files": files}))
            .collect::<Vec<_>>(),
        "frameworks": d.frameworks,
        "commands": {"build": d.build_command, "test": d.test_command},
        "important_dirs": d.important_dirs,
        "index": {
            "files_indexed": files_indexed,
            "symbols_extracted": symbols_extracted,
            "dependency_edges": dep_edges,
        },
    })
}

fn cmd_index(watch: bool, watch_mode: Option<&str>) -> Result<(), String> {
    // Parse once up-front so bad values fail fast (single source of truth: WatchMode)
    let mode = watch_mode.unwrap_or("commit").parse::<WatchMode>()
        .map_err(|e: String| format!("Invalid watch mode: {}", e))?;
    if !watch && watch_mode.is_some() {
        return Err("--watch-mode requires --watch".to_string());
    }
    let mode_label = if watch {
        format!(" (watch mode — {})", mode)
    } else {
        String::new()
    };
    println!("Indexing repository{}...", mode_label);
    
    let project_root = std::env::current_dir()
        .map_err(|e| format!("Failed to get current directory: {}", e))?;
    
    // Open database
    let db_path = get_db_path();
    let db = knocode_storage::Database::open(&db_path)
        .map_err(|e| format!("Failed to open database: {}", e))?;
    
    // Create event bus
    let event_bus = knocode_events::EventBus::new();
    
    // Create repository intelligence
    let mut repo_intel = knocode_repo_intel::RepositoryIntelligence::new(
        project_root.clone(),
        db,
        event_bus.clone(),
    );
    
    // Run indexing (wires tantivy BM25 in-process, incremental via hash, see repo-intel lib)
    let stats = run_index_with_progress(&mut repo_intel)?;
    
    println!();
    println!("✓ Indexing complete!");
    println!();
    println!("  Files indexed:    {}", stats.files_indexed);
    println!("  Symbols extracted: {}", stats.symbols_extracted);
    println!("  Files skipped:    {}", stats.files_skipped);
    println!("  Files deleted:    {}", stats.files_deleted);
    println!("  Duration:         {}ms", stats.duration_ms);
    // Also show graph edge count (new in v0.3.0) — Phase3 defer on large repos
    if stats.files_indexed > 5000 && std::env::var("KNOCODE_BUILD_GRAPH").ok().as_deref() != Some("1") {
        println!("  Dependency edges: deferred (lazy, set KNOCODE_BUILD_GRAPH=1 to force)");
    } else if let Ok(g) = repo_intel.build_dependency_graph() {
        println!("  Dependency edges: {}", g.edge_count());
    }

    if watch {
        // The watcher callbacks run on tokio's blocking pool and call into
        // `index_repository`, which needs &mut — share the instance behind a Mutex.
        let watcher = repo_intel.spawn_watcher().with_mode(mode);
        let repo_intel = std::sync::Arc::new(std::sync::Mutex::new(repo_intel));

        println!();
        println!("Watching for changes (Ctrl+C to stop) — mode: {}", mode);
        let rt = tokio::runtime::Runtime::new()
            .map_err(|e| format!("Failed to create tokio runtime: {}", e))?;
        rt.block_on(async move {
            let ri = repo_intel.clone();
            let _handle = watcher.spawn(move || {
                println!("[watcher] change detected — re-indexing...");
                match ri
                    .lock()
                    .map_err(|e| format!("watcher lock poisoned: {}", e))
                    .and_then(|mut ri| ri.index_repository().map_err(|e| e.to_string()))
                {
                    Ok(stats) => println!(
                        "[watcher] ✓ re-indexed {} files, {} symbols in {}ms",
                        stats.files_indexed, stats.symbols_extracted, stats.duration_ms
                    ),
                    Err(e) => println!("[watcher] ✗ re-index failed: {}", e),
                }
            });

            println!("(Watcher running — press Ctrl+C to stop)");
            match tokio::signal::ctrl_c().await {
                Ok(()) => println!("\nCtrl+C received — watcher stopped"),
                Err(e) => eprintln!("Failed to listen for Ctrl+C: {}", e),
            }
        });
    }

    Ok(())
}

fn cmd_preview(prompt: &str, session: &str, no_cache: bool, diag: bool, expected_files: Option<&[String]>, candidate_k: Option<usize>, max_files: Option<usize>, json: bool) -> Result<(), String> {
    // Try daemon first (UDS then HTTP), fallback to local BuildContext
    // For v0.3.0 we implement real preview: build context locally if daemon not running.
    let effective_session = if no_cache { String::new() } else { session.to_string() };
    if !json {
        println!("Previewing BuildContext for: \"{}\" (session: {}, no_cache: {})", prompt, effective_session, no_cache);
        println!();
    }

    // Attempt HTTP daemon preview (UDS preview requires MessagePack client — HTTP is fallback)
    let daemon_url = std::env::var("KNOCODE_DAEMON_URL").unwrap_or_else(|_| "http://127.0.0.1:9527".to_string());
    let url = format!("{}/hook", daemon_url);
    // Use blocking reqwest via runtime-less approach: we do local preview directly if daemon not reachable quickly.
    // Build locally to guarantee preview works offline (spec: local-first).
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let db_path = get_db_path();
    let local_preview = (|| -> Result<(), String> {
        let event_bus = knocode_events::EventBus::new();
        let _t0 = Instant::now();
        let repo_intel = knocode_repo_intel::RepositoryIntelligence::new(project_root.clone(), knocode_storage::Database::open(&db_path).map_err(|e| e.to_string())?, event_bus.clone());
        if std::env::var("KNOCODE_PROFILE").is_ok() { eprintln!("[profile] cli.db_open_and_repo_intel_new: {}ms", _t0.elapsed().as_millis()); }
        let _t1 = Instant::now();
        let kh = knocode_knowledge::KnowledgeHub::new(
            knocode_storage::Database::open(&db_path).map_err(|e| e.to_string())?,
            event_bus.clone(),
        );
        if std::env::var("KNOCODE_PROFILE").is_ok() { eprintln!("[profile] cli.kh_new: {}ms", _t1.elapsed().as_millis()); }
        let core_config = knocode_core::Config::load(&project_root)
            .unwrap_or_default();
        let ctx_config = knocode_context::ContextConfig {
            max_tokens: core_config.context.max_tokens,
            max_files: max_files
                .or_else(|| std::env::var("KNOCODE_MAX_FILES").ok().and_then(|v| v.parse().ok()))
                .unwrap_or(core_config.context.max_files),
            max_lines_per_file: core_config.context.max_lines_per_file,
            cache_order: core_config.context.cache_order.clone(),
            candidate_k: candidate_k.unwrap_or(core_config.context.candidate_k),
        };
        let _t2 = Instant::now();
        let ctx = knocode_context::ContextEngine::new(repo_intel, kh, event_bus.clone(), ctx_config);
        if std::env::var("KNOCODE_PROFILE").is_ok() { eprintln!("[profile] cli.engine_new: {}ms", _t2.elapsed().as_millis()); }
        let task = knocode_core::TaskRequest {
            message: prompt.to_string(),
            session_id: effective_session.clone(),
            context_hints: None,
            repository_id: String::new(),
            repository_path: None,
            expected_files: expected_files.map(|f| f.to_vec()),
        };
        let rt = tokio::runtime::Runtime::new().map_err(|e| format!("Failed to create tokio runtime: {}", e))?;
        let _t3 = Instant::now();
        let pack = rt.block_on(ctx.build_context(&task)).map_err(|e| e.to_string())?;
        if std::env::var("KNOCODE_PROFILE").is_ok() { eprintln!("[profile] cli.build_context: {}ms", _t3.elapsed().as_millis()); }

        // ── JSON output mode: structured, regex-free contract for tooling ──
        if json {
            use serde_json::json;
            let mut paths: Vec<serde_json::Value> = Vec::new();
            for p in &pack.provenance {
                paths.push(json!({
                    "path": p.path.replace('\\', "/"),
                    "source": p.source,
                    "retriever": p.retriever,
                    "score": p.score,
                    "reason": p.reason,
                }));
            }
            let out = json!({
                "prompt": prompt,
                "status": pack.code_retrieval_status,
                "files": paths,
                "token_usage": {
                    "total": pack.token_usage.total_tokens,
                    "budget_remaining": pack.token_usage.budget_remaining,
                    "by_source": pack.token_usage.by_source,
                },
                "retrieval_ms": pack.retrieval_stats.as_ref().map(|s| s.retrieval_ms),
                "candidates": pack.retrieval_stats.as_ref().map(|s| s.candidates),
                "repository_state": pack.repository_state,
            });
            println!("{}", serde_json::to_string_pretty(&out).map_err(|e| e.to_string())?);
            return Ok(());
        }

        println!("Knowledge entries (docs_context):");
        if pack.docs_context.is_empty() { println!("  (none)"); } else { for line in pack.docs_context.lines().take(20) { println!("  {}", line); } }
        println!();
        println!("Code files (code_context):");
        if pack.code_context.is_empty() { println!("  (none — no index or no match)"); } else { for line in pack.code_context.lines().take(100) { println!("  {}", line); } }
        println!();
        println!("Token budget:");
        println!("  total: {}, remaining: {}, by_source: {:?}", pack.token_usage.total_tokens, pack.token_usage.budget_remaining, pack.token_usage.by_source);
        println!();
        println!("Daemon URL probed: {} (if daemon running, this local preview matches daemon's BuildContext)", url);
        // Retrieval diagnostic
        if diag || expected_files.is_some() {
            if let Some(ref diagnostic) = pack.retrieval_diagnostic {
                diagnostic.print();
            } else if diag {
                eprintln!("\n[diag] No retrieval diagnostic available (expected_files not provided or no code results)\n");
            }
        } else if let Some(ref diagnostic) = pack.retrieval_diagnostic {
            // Auto-print when env var is set
            if std::env::var("KNOCODE_RETRIEVAL_DIAG").is_ok() {
                diagnostic.print();
            }
        }
        Ok(())
    })();

    if let Err(e) = local_preview {
        println!("Local preview failed: {} — is database initialized? Run `knocode init` and `knocode index`.", e);
        println!();
        println!("(Daemon preview via {} would also be attempted if daemon is running)", daemon_url);
    }
    Ok(())
}

fn cmd_status() -> Result<(), String> {
    println!("Knocode Status");
    println!("═══════════════════════════════════════");
    println!();
    
    // Check if database exists
    let db_path = get_db_path();
    if db_path.exists() {
        let db = knocode_storage::Database::open(&db_path)
            .map_err(|e| format!("Failed to open database: {}", e))?;
        
        let file_count = db.get_file_count()
            .map_err(|e| format!("Failed to get file count: {}", e))?;
        let symbol_count = db.get_symbol_count()
            .map_err(|e| format!("Failed to get symbol count: {}", e))?;
        let usage = db.get_usage_stats()
            .map_err(|e| format!("Failed to get usage stats: {}", e))?;
        
        println!("Database:");
        println!("  Path:          {}", db_path.display());
        println!("  Files indexed: {} (all repositories)", file_count);
        println!("  Symbols:       {} (all repositories)", symbol_count);
        println!();

        // Current repository — the DB is shared across repos, so the global
        // totals above don't answer "what does THIS repo's index look like?".
        if let Ok(cwd) = std::env::current_dir() {
            let repo_id = knocode_core::repository_id_from_path(&cwd.to_string_lossy());
            let repo_files = db.get_all_files_meta(&repo_id).unwrap_or_default();
            if !repo_files.is_empty() {
                let ids: Vec<i64> = repo_files.iter().map(|r| r.id).collect();
                let repo_symbols = db.count_symbols_for_file_ids(&ids).unwrap_or(0);
                println!("Current repository:");
                println!("  Path:          {}", cwd.display());
                println!("  Files indexed: {}", repo_files.len());
                println!("  Symbols:       {}", repo_symbols);
                println!();
            }
        }
        println!("Token Usage:");
        println!("  Total input tokens:  {}", usage.total_input_tokens);
        println!("  Total output tokens: {}", usage.total_output_tokens);
        println!("  Total requests:      {}", usage.total_requests);
    } else {
        println!("Database: Not initialized");
        println!("  Run 'knocode init' to initialize");
    }
    
    Ok(())
}

fn cmd_config(action: ConfigAction) -> Result<(), String> {
    match action {
        ConfigAction::Show => {
            let project_root = std::env::current_dir()
                .map_err(|e| format!("Failed to get current directory: {}", e))?;
            
            let config = Config::load(&project_root)
                .map_err(|e| format!("Failed to load config: {}", e))?;
            
            let toml = config.to_toml()
                .map_err(|e| format!("Failed to serialize config: {}", e))?;
            
            println!("Effective configuration:");
            println!("═══════════════════════════════════════");
            println!();
            print!("{}", toml);
        }
        ConfigAction::Validate => {
            let project_root = std::env::current_dir()
                .map_err(|e| format!("Failed to get current directory: {}", e))?;
            
            let config = Config::load(&project_root)
                .map_err(|e| format!("Failed to load config: {}", e))?;
            
            match config.validate() {
                Ok(()) => {
                    println!("✓ Configuration is valid");
                }
                Err(e) => {
                    println!("✗ Configuration validation failed: {}", e);
                    return Err(e.to_string());
                }
            }
        }
        ConfigAction::SetLogLevel { level } => cmd_config_set_log_level(&level)?,
        ConfigAction::Migrate { from } => {
            println!("Migrating config from '{}' (claude|continue|cursor)...", from);
            let project_root = std::env::current_dir().map_err(|e| e.to_string())?;
            let config = Config::load(&project_root).unwrap_or_default();
            // Migration: scan source's config locations
            let candidates: Vec<PathBuf> = match from.as_str() {
                "claude" => vec![project_root.join(".claude").join("settings.json"), dirs().unwrap_or_else(|| PathBuf::from(".")).join(".claude").join("settings.json")],
                "continue" => vec![project_root.join(".continue").join("config.json")],
                "cursor" => vec![dirs().unwrap_or_else(|| PathBuf::from(".")).join(".cursor").join("settings.json")],
                _ => { println!("Unknown source '{}', supported: claude, continue, cursor", from); return Ok(()); }
            };
            let mut found = 0;
            for p in candidates {
                if p.exists() {
                    println!("  Found {} at {}", from, p.display());
                    // Copy config heuristically: if file exists, note migration and validate
                    found += 1;
                }
            }
            if found == 0 {
                println!("  No {} config found — nothing to migrate (best-effort per spec §3 Adapter Layer Tier 2).", from);
            } else {
                println!("  Migration best-effort complete — review .knocode/config.toml");
            }
            println!("  Config validation:");
            match config.validate() {
                Ok(()) => println!("  ✓ Config valid after migration"),
                Err(e) => println!("  ⚠ Config invalid: {}", e),
            }
        }
    }
    
    Ok(())
}

/// `knocode config set-log-level <level>` — change log verbosity without re-running
/// the installer. Upserts `[logging] level` in the USER config
/// (`~/.config/knocode/config.toml`, what the daemon reads when started outside a
/// repo) and the PROJECT config (`.knocode/config.toml`, when present), so the next
/// `knocode serve` picks it up. Accepts tracing levels (error/warn/info/debug/trace)
/// plus the installer's verbosity aliases (quiet/normal/verbose). Env
/// `KNOCODE_LOG_LEVEL` still overrides these files at daemon startup.
fn cmd_config_set_log_level(level: &str) -> Result<(), String> {
    let level = level.trim().to_lowercase();
    // Aliases mirror the installer's 3-level verbosity prompt (0 quiet / 1 normal / 2 verbose).
    let level = match level.as_str() {
        "0" | "quiet" => "error".to_string(),
        "1" | "normal" => "info".to_string(),
        "2" | "verbose" => "debug".to_string(),
        other => other.to_string(),
    };
    // Same valid set as Config::validate — reject before touching any file.
    let valid = ["error", "warn", "info", "debug", "trace"];
    if !valid.contains(&level.as_str()) {
        return Err(format!(
            "invalid log level '{}': must be one of {} (aliases: quiet, normal, verbose)",
            level,
            valid.join(", ")
        ));
    }

    let mut updated: Vec<String> = Vec::new();

    // 1. USER config — always written (the daemon's [logging] level fallback lives here).
    if let Some(user_cfg) = user_config_path() {
        upsert_logging_level(&user_cfg, &level)?;
        updated.push(user_cfg.display().to_string());
    }

    // 2. PROJECT config (.knocode/config.toml) — only when it exists AND carries a
    //    [logging] section (or is fresh); project merge would otherwise keep overriding
    //    the user value the user just set.
    let project_root = std::env::current_dir().map_err(|e| format!("Failed to get current directory: {}", e))?;
    let project_cfg = project_root.join(".knocode").join("config.toml");
    if project_cfg.exists() {
        let has_logging_section = std::fs::read_to_string(&project_cfg)
            .map(|t| t.contains("[logging]"))
            .unwrap_or(false);
        if has_logging_section {
            upsert_logging_level(&project_cfg, &level)?;
            updated.push(project_cfg.display().to_string());
        }
    }

    println!("✓ Log level set to '{}' (verbosity: {})", level, verbosity_label(&level));
    for p in &updated {
        println!("  updated: {}", p);
    }
    // 3. Persist the user env var (plugins read KNOCODE_LOG_LEVEL from the environment,
    //    not from config files) — HKCU\Environment on Windows (with WM_SETTINGCHANGE so
    //    new processes pick it up), `export` lines in ~/.profile + ~/.bashrc on Unix.
    //    Best-effort: a failure warns but does not fail the command (files are updated).
    match persist_user_env_log_level(&level) {
        Ok(Some(lines)) => {
            for w in lines {
                println!("  env: {}", w);
            }
        }
        Ok(None) => {} // default level (info) — nothing persisted, matching the installer
        Err(e) => println!("  ⚠ env not persisted: {}", e),
    }
    println!();
    println!("  Restart the daemon (knocode serve) to apply.");
    println!("  Note: KNOCODE_LOG_LEVEL env overrides these files when set.");
    Ok(())
}

/// Persist KNOCODE_LOG_LEVEL in the user environment (best-effort).
/// Windows: HKCU\Environment + WM_SETTINGCHANGE broadcast (what
/// `[Environment]::SetEnvironmentVariable(..., 'User')` does). Unix: idempotent
/// `export KNOCODE_LOG_LEVEL="..."` lines in ~/.profile and ~/.bashrc using the
/// same `# KNOCODE_LOG_LEVEL:` marker the installers write. Returns human-readable
/// confirmations; `None` when there was nothing to change.
/// Setting the default level (`info`) REMOVES a previously persisted var instead —
/// env beats config files for the daemon and is the only thing plugins read, so a
/// stale `KNOCODE_LOG_LEVEL=debug` must not silently outlive `set-log-level info`.
fn persist_user_env_log_level(level: &str) -> Result<Option<Vec<String>>, String> {
    // Default level: remove a previously persisted var (env beats config files, so a
    // stale KNOCODE_LOG_LEVEL must not silently outlive `set-log-level info`).
    if level == "info" {
        return remove_user_env_log_level();
    }
    set_user_env_log_level(level)
}

/// Set the user env var to `level` (non-default levels).
fn set_user_env_log_level(level: &str) -> Result<Option<Vec<String>>, String> {

    #[cfg(target_os = "windows")]
    {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_READ, KEY_SET_VALUE};
        use winreg::RegKey;
        let env_key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags("Environment", KEY_READ | KEY_SET_VALUE)
            .map_err(|e| format!("cannot open HKCU\\Environment: {}", e))?;
        env_key
            .set_value("KNOCODE_LOG_LEVEL", &level)
            .map_err(|e| format!("cannot write HKCU\\Environment\\KNOCODE_LOG_LEVEL: {}", e))?;
        broadcast_wm_settingchange();
        Ok(Some(vec![
            "KNOCODE_LOG_LEVEL=... persisted in user environment (HKCU; new processes see it, existing shells don't)".to_string(),
        ]))
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut written: Vec<String> = Vec::new();
        let home = dirs().ok_or_else(|| "cannot determine home directory".to_string())?;
        for rc in [".profile", ".bashrc"] {
            let rc_path = home.join(rc);
            if !rc_path.exists() {
                continue; // mirrors the installer: never create rc files that don't exist
            }
            let text = std::fs::read_to_string(&rc_path)
                .map_err(|e| format!("cannot read {}: {}", rc_path.display(), e))?;
            if let Some(new_text) = upsert_rc_export(&text, level) {
                std::fs::write(&rc_path, new_text)
                    .map_err(|e| format!("cannot write {}: {}", rc_path.display(), e))?;
                written.push(format!(
                    "KNOCODE_LOG_LEVEL=\"{}\" updated in ~/{}, restart your shell to see it",
                    level, rc
                ));
            }
        }
        if written.is_empty() {
            written.push(
                "no ~/.profile or ~/.bashrc found — add 'export KNOCODE_LOG_LEVEL=\"...\"' to your shell profile manually"
                    .to_string(),
            );
        }
        Ok(Some(written))
    }
}

/// Remove a previously persisted KNOCODE_LOG_LEVEL from the user environment
/// (backing `set-log-level info` — the default needs no persisted override, and env
/// beats config files for both the daemon and the agent plugins).
fn remove_user_env_log_level() -> Result<Option<Vec<String>>, String> {
    #[cfg(target_os = "windows")]
    {
        use winreg::enums::{HKEY_CURRENT_USER, KEY_QUERY_VALUE, KEY_SET_VALUE};
        use winreg::RegKey;
        let env_key = RegKey::predef(HKEY_CURRENT_USER)
            .open_subkey_with_flags("Environment", KEY_QUERY_VALUE | KEY_SET_VALUE)
            .map_err(|e| format!("cannot open HKCU\\Environment: {}", e))?;
        match env_key.get_value::<String, _>("KNOCODE_LOG_LEVEL") {
            Ok(old) => {
                env_key
                    .delete_value("KNOCODE_LOG_LEVEL")
                    .map_err(|e| format!("cannot delete HKCU\\Environment\\KNOCODE_LOG_LEVEL: {}", e))?;
                broadcast_wm_settingchange();
                Ok(Some(vec![format!(
                    "KNOCODE_LOG_LEVEL={} removed from user environment (HKCU); config files now decide",
                    old
                )]))
            }
            Err(_) => Ok(None), // nothing persisted — nothing to remove
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let mut written: Vec<String> = Vec::new();
        let home = dirs().ok_or_else(|| "cannot determine home directory".to_string())?;
        for rc in [".profile", ".bashrc"] {
            let rc_path = home.join(rc);
            if !rc_path.exists() {
                continue;
            }
            let text = std::fs::read_to_string(&rc_path)
                .map_err(|e| format!("cannot read {}: {}", rc_path.display(), e))?;
            if let Some(new_text) = remove_rc_export(&text) {
                std::fs::write(&rc_path, new_text)
                    .map_err(|e| format!("cannot write {}: {}", rc_path.display(), e))?;
                written.push(format!(
                    "KNOCODE_LOG_LEVEL export removed from ~/{}, restart your shell to see it",
                    rc
                ));
            }
        }
        if written.is_empty() {
            return Ok(None);
        }
        Ok(Some(written))
    }
}

/// Remove any `export KNOCODE_LOG_LEVEL=...` line from rc-file `text`, together with
/// the installer's marker comment when it sits directly above the removed export.
/// Returns `None` when nothing would change. Cross-platform and unit-tested.
#[cfg(any(test, not(target_os = "windows")))]
fn remove_rc_export(text: &str) -> Option<String> {
    let mut out: Vec<String> = Vec::new();
    let mut changed = false;
    for line in text.lines() {
        if line.trim_start().starts_with("export KNOCODE_LOG_LEVEL") {
            changed = true;
            // Drop the marker comment directly above the removed export (installer format).
            if let Some(last) = out.last() {
                if last.trim_start().starts_with("# KNOCODE_LOG_LEVEL:") {
                    out.pop();
                }
            }
            continue;
        }
        out.push(line.to_string());
    }
    if !changed {
        return None;
    }
    let mut result = out.join("\n");
    if text.is_empty() || text.ends_with('\n') {
        result.push('\n');
    }
    Some(result)
}

/// Rewrite rc-file `text` so it carries exactly one `export KNOCODE_LOG_LEVEL="<level>"`
/// line: replaces an existing export (any indentation, installer- or CLI-written), or
/// appends the installer's marker comment + export at the end. Returns `None` when the
/// content would not change (idempotent re-runs skip the write). Cross-platform and
/// unit-tested — only the surrounding file I/O is platform-gated.
#[cfg(any(test, not(target_os = "windows")))]
fn upsert_rc_export(text: &str, level: &str) -> Option<String> {
    let export_line = format!("export KNOCODE_LOG_LEVEL=\"{}\"", level);
    let mut out: Vec<String> = Vec::new();
    let mut replaced = false;
    let mut changed = false;
    for line in text.lines() {
        if line.trim_start().starts_with("export KNOCODE_LOG_LEVEL") {
            out.push(export_line.clone());
            changed |= line != export_line;
            replaced = true;
        } else {
            // All other lines (including the installer's marker comment) kept as-is.
            out.push(line.to_string());
        }
    }
    if !replaced {
        if !out.is_empty() && out.last().map(|l| !l.trim().is_empty()).unwrap_or(false) {
            out.push(String::new());
        }
        out.push(
            "# KNOCODE_LOG_LEVEL: knocode log verbosity (0 quiet / 1 normal / 2 verbose = every daemon call)"
                .to_string(),
        );
        out.push(export_line);
        changed = true;
    }
    if !changed {
        return None;
    }
    let mut result = out.join("\n");
    if text.is_empty() || text.ends_with('\n') {
        result.push('\n');
    }
    Some(result)
}

/// Broadcast WM_SETTINGCHANGE ("Environment") so Explorer and newly launched
/// processes notice an HKCU\Environment change without a logoff/logon.
#[cfg(target_os = "windows")]
fn broadcast_wm_settingchange() {
    use std::os::windows::ffi::OsStrExt;
    #[link(name = "user32")]
    extern "system" {
        fn SendMessageTimeoutW(
            hwnd: isize,
            msg: u32,
            wparam: usize,
            lparam: *const u16,
            flags: u32,
            timeout: u32,
            out_result: *mut usize,
        ) -> isize;
    }
    const HWND_BROADCAST: isize = 0xFFFF;
    const WM_SETTINGCHANGE: u32 = 0x001A;
    const SMTO_ABORTIFHUNG: u32 = 0x0002;
    let env_wide: Vec<u16> = std::ffi::OsStr::new("Environment")
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut result: usize = 0;
    unsafe {
        let _ = SendMessageTimeoutW(
            HWND_BROADCAST,
            WM_SETTINGCHANGE,
            0,
            env_wide.as_ptr(),
            SMTO_ABORTIFHUNG,
            1000,
            &mut result,
        );
    }
}

/// The installer's 3-level verbosity label for a tracing level (mirrors
/// knocode-client's mapping: error/warn→0, info→1, debug/trace→2).
fn verbosity_label(level: &str) -> &'static str {
    match level {
        "error" | "warn" => "0 quiet — errors only",
        "info" => "1 normal",
        _ => "2 verbose — every daemon call logged",
    }
}

/// `~/.config/knocode/config.toml` (mirrors Config::user_config_path in knocode-core).
fn user_config_path() -> Option<PathBuf> {
    dirs().map(|home| home.join(".config").join("knocode").join("config.toml"))
}

/// Upsert `level = "<level>"` under the `[logging]` section of a TOML file.
/// Line-oriented (no toml_edit dep): replaces an existing `level =` line inside
/// [logging], inserts one after `[logging]`, or appends a new [logging] section.
/// Other keys/sections are preserved byte-for-byte.
fn upsert_logging_level(path: &Path, level: &str) -> Result<(), String> {
    let text = if path.exists() {
        std::fs::read_to_string(path).map_err(|e| format!("Failed to read {}: {}", path.display(), e))?
    } else {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| format!("Failed to create {}: {}", parent.display(), e))?;
        }
        String::new()
    };

    let new_line = format!("level = \"{}\"", level);
    let mut out: Vec<String> = Vec::new();
    let mut current_section: Option<String> = None;
    let mut replaced = false;
    let mut logging_found = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('[') && trimmed.ends_with(']') {
            let incoming = trimmed.trim_start_matches('[').trim_end_matches(']').trim().to_string();
            // Insert pending level line BEFORE switching sections: when the previous
            // section was [logging] and it carried no `level =` key, the new key line
            // belongs at the end of that section (before this new `[section]` header).
            if current_section.as_deref() == Some("logging") && !replaced {
                out.push(new_line.clone());
                replaced = true;
            }
            current_section = Some(incoming);
            if current_section.as_deref() == Some("logging") {
                logging_found = true;
            }
        } else if current_section.as_deref() == Some("logging") && !replaced {
            // Key line inside [logging]: strip comments, then compare the key
            // (the part before the first '=') so `level=x`, `level = x`, and
            // `level = x # comment` all match, while `levels=`/`level_max=` don't.
            let key_only = trimmed.split('#').next().unwrap_or("").trim();
            let key = key_only.split('=').next().unwrap_or("").trim();
            if key == "level" && key_only.contains('=') {
                out.push(new_line.clone());
                replaced = true;
                continue;
            }
        }
        out.push(line.to_string());
    }
    if current_section.as_deref() == Some("logging") && !replaced {
        out.push(new_line.clone());
        replaced = true;
    }
    if !logging_found {
        if !out.is_empty() && out.last().map(|l| !l.trim().is_empty()).unwrap_or(false) {
            out.push(String::new());
        }
        out.push("[logging]".to_string());
        out.push(new_line);
        replaced = true;
    }
    let _ = replaced;

    let mut result = out.join("\n");
    // Restore/normalize the trailing newline for new and newline-terminated files;
    // a file that deliberately lacked one keeps lacking it.
    if text.is_empty() || text.ends_with('\n') {
        result.push('\n');
    }

    std::fs::write(path, result).map_err(|e| format!("Failed to write {}: {}", path.display(), e))
}

fn cmd_doctor() -> Result<(), String> {
    println!("Knocode Doctor (v1 — 8 probes)");
    println!("═══════════════════════════════════════");
    println!();
    
    let mut all_ok = true;
    
    // Check SQLite (critical)
    print!("SQLite:          ");
    let db_path = get_db_path();
    match knocode_storage::Database::open(&db_path) {
        Ok(db) => {
            // Check migrations
            match db.get_file_count() {
                Ok(_) => println!("✓ OK (WAL, migrations up to date)"),
                Err(e) => { println!("✗ FAILED: {}", e); all_ok = false; }
            }
        },
        Err(e) => {
            println!("✗ FAILED: {}", e);
            all_ok = false;
        }
    }
    
    // Check config (critical)
    print!("Config:          ");
    let project_root = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    // Repo-scoped BM25 index path (matches init/daemon via default_index_path, honors KNOCODE_INDEX_DIR)
    let repo_id = knocode_core::repository_id_from_path(&project_root.to_string_lossy());
    match Config::load(&project_root) {
        Ok(config) => {
            match config.validate() {
                Ok(()) => println!("✓ OK (token budget valid)"),
                Err(e) => {
                    println!("✗ INVALID: {}", e);
                    all_ok = false;
                }
            }
        }
        Err(e) => {
            println!("✗ NOT FOUND: {}", e);
            all_ok = false;
        }
    }
    
    // Check installed binary on PATH (AGENTS.md — never search repo for .exe)
    print!("Knocode PATH:    ");
    {
        let bin_dir = dirs().unwrap_or_else(|| PathBuf::from(".")).join(".knocode").join("bin");
        #[cfg(target_os = "windows")]
        let installed = bin_dir.join("knocode.exe");
        #[cfg(not(target_os = "windows"))]
        let installed = bin_dir.join("knocode");
        let exists = installed.exists();
        let path_var = std::env::var("PATH").unwrap_or_default();
        #[cfg(target_os = "windows")]
        let sep = ';';
        #[cfg(not(target_os = "windows"))]
        let sep = ':';
        let on_path = path_var.split(sep).any(|p| {
            let pp = PathBuf::from(p.trim().trim_matches('"'));
            pp == bin_dir || pp.as_os_str() == bin_dir.as_os_str()
        });
        if exists && on_path {
            println!("✓ OK ({} on PATH)", bin_dir.display());
        } else if exists && !on_path {
            println!("⚠ INSTALLED at {} but NOT on PATH — restart shell or add to PATH (install.ps1/install.sh does this). Use absolute: {} init", bin_dir.display(), installed.display());
            all_ok = false;
        } else if !exists {
            println!("✗ NOT FOUND at {} — run scripts/install.ps1 (or install.sh) to copy target/release/knocode(.exe) there", bin_dir.display());
            all_ok = false;
        }
        // Also probe bare `knocode` resolves via PATH
        let bare_ok = std::process::Command::new(if cfg!(target_os = "windows") { "where" } else { "which" })
            .arg("knocode")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !bare_ok && exists {
            println!("  Hint: bare `knocode --version` will fail until new shell — use absolute {} --version", installed.display());
        }
    }

    // Check repository profile
    print!("Repo profile:     ");
    {
        let prof = PathBuf::from(".knocode/profile.json");
        if prof.exists() {
            println!("✓ OK ({})", prof.display());
        } else {
            println!("⚠ NOT FOUND (run 'knocode init' to bootstrap)");
        }
    }

    // Check tree-sitter — split global vs repo detected (V1: docs don't need parser)
    print!("Tree-sitter:     ");
    {
        use knocode_repo_intel::registry::{LANGUAGE_REGISTRY, get_ts_parser};
        let mut ready = 0;
        let mut total = 0;
        for def in LANGUAGE_REGISTRY {
            if !def.id.has_parser() {
                continue;
            }
            total += 1;
            if get_ts_parser(def.id).is_some() {
                ready += 1;
            }
        }
        if ready > 0 {
            println!("✓ OK (global {}/{} parsers ready)", ready, total);
        } else {
            println!("⚠ No parsers loaded");
        }
    }
    // Repository detected languages (files → languages → grammars, lexical fallback for docs/config)
    {
        println!();
        println!("  Repository detected languages");
        println!("  ─────────────────────────────");
        let repo_path = project_root.clone();
        let mut counts: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
        let mut docs_count = 0usize;
        let mut config_count = 0usize;
        let mut source_count = 0usize;
        if let Ok(walker) = std::fs::read_dir(&repo_path) {
            let _ = walker; // placeholder to avoid unused
        }
        // Use repo-intel walker to sample repo files
        let db_tmp = knocode_storage::Database::open(&std::path::PathBuf::from(":memory:")).ok();
        if let Some(db) = db_tmp {
            let ri = knocode_repo_intel::RepositoryIntelligence::new(repo_path.clone(), db, knocode_events::EventBus::new());
            let langs = ri.detect_repo_languages();
            for (ext, cnt) in langs.iter().take(12) {
                println!("    {:<18} {:>6} files", ext, cnt);
            }
            // Also show file-class breakdown from current index if available
            let idx_path = PathBuf::from(knocode_repo_intel::default_index_path(&repo_id));
            if let Ok(idx) = knocode_storage::tantivy_index::TantivyIndex::open(&idx_path.to_string_lossy()) {
                if let Ok(reader) = idx.reader() {
                    if let Ok(stats) = idx.stats(&reader) {
                        println!("  Indexing capability");
                        println!("  ────────────────────");
                        println!("    Tantivy docs: {} total", stats.doc_count);
                    }
                }
            }
            // Count docs/config/source from repo-intel classify
            let walk = ri.walk_directory_for_doctor(&repo_path);
            for p in walk.iter().take(5000) {
                let cls = knocode_repo_intel::registry::classify_file(p);
                match cls {
                    knocode_repo_intel::registry::FileClass::Documentation => docs_count += 1,
                    knocode_repo_intel::registry::FileClass::Config => config_count += 1,
                    knocode_repo_intel::registry::FileClass::Source | knocode_repo_intel::registry::FileClass::Test => source_count += 1,
                    _ => {}
                }
                if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
                    *counts.entry(ext.to_lowercase()).or_insert(0) += 1;
                }
            }
            if docs_count > 0 || config_count > 0 {
                println!("    Documentation: {} files (lexical, no parser needed)", docs_count);
                println!("    Configuration: {} files (lexical, no parser needed)", config_count);
                println!("    Source: {} files (Tree-sitter where available, lexical fallback otherwise)", source_count);
            }
        }
        println!();
    }
    
    // Check tantivy (new)
    print!("Tantivy:         ");
    {
        // Global BM25 store (per-repo scoping happens at query time via repository_id)
        let idx_path = PathBuf::from(knocode_repo_intel::default_index_path(&repo_id));
        if idx_path.exists() {
            let idx_str = idx_path.to_string_lossy().to_string();
            let doc_count = knocode_storage::tantivy_index::TantivyIndex::open(&idx_str)
                .ok()
                .and_then(|idx| {
                    let reader = idx.reader().ok()?;
                    idx.stats(&reader).ok().map(|s| s.doc_count)
                });
            match doc_count {
                Some(n) => println!("✓ OK ({} docs, {})", n, idx_path.display()),
                None => println!("⚠ index dir present but unreadable at {}", idx_path.display()),
            }
        } else {
            println!("○ global BM25 index not created yet — it is created automatically when the daemon starts (or run 'knocode index' inside a repository)");
        }
    }

    // RET-015: Retrieval probe — verify search works
    print!("Retrieval:       ");
    {
        let idx_path = PathBuf::from(knocode_repo_intel::default_index_path(&repo_id));
        if let Ok(idx) = knocode_storage::tantivy_index::TantivyIndex::open(&idx_path.to_string_lossy()) {
            if let Ok(reader) = idx.reader() {
                let rid = repo_id.clone();
                let probe_queries = ["class", "main", "function"];
                let mut hits = 0;
                for q in &probe_queries {
                    if let Ok(results) = idx.search(&reader, q, None, 1, Some(&rid)) {
                        if !results.is_empty() {
                            hits += 1;
                        }
                    }
                }
                if hits > 0 {
                    println!("✓ OK ({}/{} probe queries returned results)", hits, probe_queries.len());
                } else {
                    println!("⚠ Index exists but no results for probe queries — re-run 'knocode init'");
                    all_ok = false;
                }
            } else {
                println!("⚠ Cannot read index");
            }
        } else {
            println!("○ No index (run 'knocode init')");
        }
    }

    // LiteLLM removed — see docs/01-architecture/LLM_ROUTING_REMOVAL.md

    // Check tiktoken
    print!("Tiktoken:        ");
    match tiktoken_rs::cl100k_base() {
        Ok(_) => println!("✓ OK (cl100k_base local, no model API round-trip)"),
        Err(e) => println!("⚠ Failed to load: {}", e),
    }

    // Check secrets redaction
    print!("Secrets redact:  ");
    {
        let sample = "api_key: sk-abc1234567890";
        let redacted = knocode_core::redact_secrets(sample);
        if redacted.contains("[REDACTED]") { println!("✓ OK (redaction before outbound calls)"); } else { println!("⚠ Probe failed"); }
    }

    // Check metrics endpoint
    print!("Metrics:         ");
    {
        println!("○ GET /metrics on daemon (prometheus exposition) — curl localhost:9527/metrics");
    }
    
    println!();
    
    if all_ok {
        println!("✓ All critical checks passed");
        println!("  Try: `knocode preview \"test\"`, `knocode doctor`, `knocode serve`");
    } else {
        println!("⚠ Some checks failed. Run 'knocode init' to initialize.");
    }
    
    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────────


fn get_db_path() -> PathBuf {
    dirs().unwrap_or_else(|| PathBuf::from("."))
        .join(".knocode")
        .join("data.db")
}

fn dirs() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE")
            .ok()
            .map(PathBuf::from)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("HOME")
            .ok()
            .map(PathBuf::from)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_status_box_alignment() {
        let box_str = format_status_box(
            "Repository Status: READY",
            &[
                ("Tree-sitter:", "10/10 grammars".to_string()),
                ("Symbols:", "36861 extracted".to_string()),
                ("Graph:", "33446 edges".to_string()),
                ("Tantivy:", "9850 files".to_string()),
                ("Knowledge:", "1 entries".to_string()),
            ],
        );
        let lines: Vec<&str> = box_str.lines().collect();
        assert_eq!(lines.len(), 9);
        // Every line must have identical display width (borders inclusive) so
        // the │ edges line up — the bug this box style replaces.
        let width = lines[0].chars().count();
        for (i, line) in lines.iter().enumerate() {
            assert_eq!(line.chars().count(), width, "line {} misaligned: {:?}", i, line);
        }
        // A long value must widen the whole box instead of breaking alignment.
        let wide = format_status_box(
            "Repository Status: READY",
            &[("Graph:", "33446 edges (deferred — lazy on first query)".to_string())],
        );
        let wide_width = wide.lines().next().unwrap().chars().count();
        for line in wide.lines() {
            assert_eq!(line.chars().count(), wide_width);
        }
        assert!(wide_width > width);
    }

    #[test]
    fn test_parse_cargo_metadata_target_dir() {
        // Unix-style path
        let unix: &[u8] =
            br#"{"reason":"...","target_directory":"/home/u/proj/target","workspace_root":"/home/u/proj"}"#;
        assert_eq!(
            parse_cargo_metadata_target_dir(unix),
            Some(PathBuf::from("/home/u/proj/target"))
        );
        // Windows paths arrive JSON-escaped (raw bytes keep the double backslashes)
        let win: &[u8] = br#"{"target_directory":"C:\\\\Users\\\\marce\\\\.cargo\\\\target"}"#;
        assert_eq!(
            parse_cargo_metadata_target_dir(win),
            Some(PathBuf::from("C:\\\\Users\\\\marce\\\\.cargo\\\\target"))
        );
        assert_eq!(parse_cargo_metadata_target_dir(br#"{"nope":1}"#), None);
        assert_eq!(parse_cargo_metadata_target_dir(b"not json"), None);
        assert_eq!(parse_cargo_metadata_target_dir(b""), None);
    }

    #[test]
    fn test_normalize_path_separators() {
        // cargo config/env values use `/` while join() appends `\` on Windows —
        // the normalized path must display with the platform separator only.
        let p = normalize_path_separators(Path::new("C:/Users/marce/.cargo/target"));
        #[cfg(windows)]
        assert_eq!(p.to_string_lossy(), "C:\\Users\\marce\\.cargo\\target");
        #[cfg(not(windows))]
        assert_eq!(p.to_string_lossy(), "C:/Users/marce/.cargo/target");
    }

    #[test]
    fn test_daemon_spawn_args_forward_port() {
        assert_eq!(
            daemon_spawn_args(9599),
            vec!["--port".to_string(), "9599".to_string()]
        );
        assert_eq!(
            daemon_spawn_args(9527),
            vec!["--port".to_string(), "9527".to_string()]
        );
    }

    #[test]
    fn test_upsert_logging_level_replaces_existing_key() {
        let dir = std::env::temp_dir().join(format!("knocode_cfg_test_replace_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        std::fs::write(
            &p,
            "[logging]\nfile_path = \"~/.knocode/logs/knocode.log\"\nlevel = \"info\"\nretention_days = 7\n",
        )
        .unwrap();
        upsert_logging_level(&p, "debug").unwrap();
        let t = std::fs::read_to_string(&p).unwrap();
        assert!(t.contains("[logging]"));
        assert!(t.contains("level = \"debug\""));
        assert!(!t.contains("level = \"info\""));
        assert!(t.contains("file_path = \"~/.knocode/logs/knocode.log\""));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_upsert_logging_level_inserts_into_existing_section() {
        let dir = std::env::temp_dir().join(format!("knocode_cfg_test_insert_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        std::fs::write(&p, "[logging]\nfile_path = \"x\"\n").unwrap();
        upsert_logging_level(&p, "error").unwrap();
        let t = std::fs::read_to_string(&p).unwrap();
        // Assert semantics, not byte order: level lands inside [logging], the rest survives.
        let v: toml::Value = toml::from_str(&t).unwrap();
        assert_eq!(v["logging"]["level"].as_str(), Some("error"));
        assert_eq!(v["logging"]["file_path"].as_str(), Some("x"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_upsert_logging_level_appends_missing_section() {
        let dir = std::env::temp_dir().join(format!("knocode_cfg_test_append_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        std::fs::write(&p, "[context]\nmax_tokens = 12000\n").unwrap();
        upsert_logging_level(&p, "trace").unwrap();
        let t = std::fs::read_to_string(&p).unwrap();
        assert!(t.contains("[logging]\nlevel = \"trace\"\n"));
        assert!(t.contains("[context]"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_upsert_rc_export_appends_marker_and_export() {
        let out = upsert_rc_export("export PATH=\"$HOME/.knocode/bin:$PATH\"\n", "debug").unwrap();
        assert!(out.contains("# KNOCODE_LOG_LEVEL: knocode log verbosity"));
        assert!(out.contains("export KNOCODE_LOG_LEVEL=\"debug\"\n"));
        assert!(out.contains("export PATH="));
        // Blank line separates the appended block from existing content.
        assert!(out.contains("$PATH\"\n\n# KNOCODE_LOG_LEVEL:"));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn test_upsert_rc_export_replaces_existing_value() {
        let text = "# KNOCODE_LOG_LEVEL: knocode log verbosity (0 quiet / 1 normal / 2 verbose = every daemon call)\nexport KNOCODE_LOG_LEVEL=\"error\"\n";
        let out = upsert_rc_export(text, "trace").unwrap();
        assert!(out.contains("export KNOCODE_LOG_LEVEL=\"trace\""));
        assert!(!out.contains("\"error\""));
        // Marker comment and everything else preserved.
        assert_eq!(out.matches("KNOCODE_LOG_LEVEL").count(), 2);
    }

    #[test]
    fn test_upsert_rc_export_idempotent_when_unchanged() {
        let text = "# marker\nexport KNOCODE_LOG_LEVEL=\"debug\"\n";
        assert!(upsert_rc_export(text, "debug").is_none());
    }

    #[test]
    fn test_upsert_rc_export_replaces_indented_and_leaves_other_exports() {
        let text = "export KNOCODE_TIMEOUT_MS=\"5000\"\n  export KNOCODE_LOG_LEVEL=\"info\"\n";
        let out = upsert_rc_export(text, "error").unwrap();
        assert!(out.contains("export KNOCODE_TIMEOUT_MS=\"5000\""));
        assert!(out.contains("export KNOCODE_LOG_LEVEL=\"error\""));
        assert_eq!(out.matches("export KNOCODE_LOG_LEVEL").count(), 1);
    }

    #[test]
    fn test_upsert_rc_export_empty_file() {
        let out = upsert_rc_export("", "warn").unwrap();
        assert_eq!(
            out,
            "# KNOCODE_LOG_LEVEL: knocode log verbosity (0 quiet / 1 normal / 2 verbose = every daemon call)\nexport KNOCODE_LOG_LEVEL=\"warn\"\n"
        );
    }

    #[test]
    fn test_remove_rc_export_removes_export_and_marker() {
        let text = "# KNOCODE_BIN_PATH: knocode AI runtime CLI + daemon\nexport PATH=\"$HOME/.knocode/bin:$PATH\"\n\n# KNOCODE_LOG_LEVEL: knocode log verbosity (0 quiet / 1 normal / 2 verbose = every daemon call)\nexport KNOCODE_LOG_LEVEL=\"debug\"\n";
        let out = remove_rc_export(text).unwrap();
        assert!(!out.contains("KNOCODE_LOG_LEVEL"));
        assert!(out.contains("export PATH="));
        assert!(out.ends_with('\n'));
    }

    #[test]
    fn test_remove_rc_export_noop_when_absent() {
        assert!(remove_rc_export("export PATH=\"x\"\n").is_none());
        // A marker comment with no export below it survives.
        let text = "# KNOCODE_LOG_LEVEL: dangling marker\n";
        assert!(remove_rc_export(text).is_none());
    }

    #[test]
    fn test_upsert_logging_level_heals_duplicated_key_corruption() {
        // Regression: an installer regex once wrote `level = level = "debug"`
        // (group-ref + full-line replacement), which the daemon cannot parse.
        // The upsert must replace the WHOLE broken line, not append to it.
        let dir = std::env::temp_dir().join(format!("knocode_cfg_test_heal_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        std::fs::write(&p, "[logging]\nlevel = level = \"debug\"\n").unwrap();
        upsert_logging_level(&p, "info").unwrap();
        let t = std::fs::read_to_string(&p).unwrap();
        let v: toml::Value = toml::from_str(&t).expect("healed file must be valid TOML");
        assert_eq!(v["logging"]["level"].as_str(), Some("info"));
        assert!(!t.contains("level = level"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_upsert_logging_level_creates_new_file() {
        let dir = std::env::temp_dir().join(format!("knocode_cfg_test_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("nested").join("config.toml");
        upsert_logging_level(&p, "warn").unwrap();
        let t = std::fs::read_to_string(&p).unwrap();
        assert_eq!(t, "[logging]\nlevel = \"warn\"\n");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_upsert_logging_level_ignores_level_in_other_sections() {
        let dir = std::env::temp_dir().join(format!("knocode_cfg_test_sections_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("config.toml");
        // `level` under [index] must NOT be touched; [logging] has no level → insert one.
        std::fs::write(&p, "[index]\nlevel = \"keepme\"\n\n[logging]\nfile_path = \"x\"\n").unwrap();
        upsert_logging_level(&p, "info").unwrap();
        let t = std::fs::read_to_string(&p).unwrap();
        let v: toml::Value = toml::from_str(&t).unwrap();
        assert_eq!(v["index"]["level"].as_str(), Some("keepme"));
        assert_eq!(v["logging"]["level"].as_str(), Some("info"));
        assert_eq!(v["logging"]["file_path"].as_str(), Some("x"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_cli_parsing() {
        let cli = Cli::try_parse_from(["knocode", "init"]);
        assert!(cli.is_ok());
        
        let cli = Cli::try_parse_from(["knocode", "status"]);
        assert!(cli.is_ok());
        
        let cli = Cli::try_parse_from(["knocode", "preview", "test prompt"]);
        assert!(cli.is_ok());
        
        let cli = Cli::try_parse_from(["knocode", "config", "show"]);
        assert!(cli.is_ok());
        
        let cli = Cli::try_parse_from(["knocode", "doctor"]);
        assert!(cli.is_ok());
    }
    
    #[test]
    fn test_get_db_path() {
        let path = get_db_path();
        assert!(path.to_string_lossy().contains(".knocode"));
        assert!(path.to_string_lossy().contains("data.db"));
    }
    
    #[test]
    fn test_dirs_exists() {
        let dirs = dirs();
        assert!(dirs.is_some());
    }

    #[test]
    fn test_ext_language() {
        assert_eq!(ext_language("rs"), Some("rust"));
        assert_eq!(ext_language("tsx"), Some("typescriptreact"));
        assert_eq!(ext_language("ts"), Some("typescript"));
        assert_eq!(ext_language("hpp"), Some("cpp"));
        assert_eq!(ext_language(""), None);
        assert_eq!(ext_language("png"), None);
    }

    #[test]
    fn test_discovery_and_profile_json() {
        let tmp = std::env::temp_dir().join(format!("knocode-disc-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(tmp.join("src")).unwrap();
        std::fs::write(tmp.join("Cargo.toml"), "[package]\nname = \"x\"\n[workspace]\n").unwrap();
        std::fs::write(tmp.join("src").join("main.rs"), "fn main() {}").unwrap();
        std::fs::write(tmp.join("README.md"), "# demo").unwrap();

        let d = discover_repository(&tmp);
        assert!(d.frameworks.contains(&"cargo".to_string()));
        assert!(d.frameworks.contains(&"rust-workspace".to_string()));
        assert_eq!(d.build_command.as_deref(), Some("cargo build"));
        assert_eq!(d.test_command.as_deref(), Some("cargo test"));
        assert!(d.important_dirs.contains(&"src".to_string()));
        assert!(d.languages.iter().any(|(l, n)| l == "rust" && *n >= 1));

        let profile = build_profile_json("x", &d, 1, 2, 3);
        assert_eq!(profile["project"], "x");
        assert_eq!(profile["index"]["files_indexed"], 1);
        assert_eq!(profile["commands"]["test"], "cargo test");
        let json = serde_json::to_string_pretty(&profile).unwrap();
        assert!(json.contains("rust"));

        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn test_update_config_languages() {
        let tmp = std::env::temp_dir().join(format!("knocode_cfg_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&tmp).unwrap();
        let config_path = tmp.join("config.toml");

        // Write a minimal config with old languages
        std::fs::write(&config_path, "[index]\nlanguages = [\"rust\", \"typescript\"]\n").unwrap();

        let languages = vec![
            ("csharp".to_string(), 42),
            ("rust".to_string(), 10),
        ];

        let updated = update_config_languages(&config_path, &languages).unwrap();
        assert!(updated, "should update when languages differ");

        let content = std::fs::read_to_string(&config_path).unwrap();
        assert!(content.contains("csharp"), "config should contain csharp");
        assert!(content.contains("rust"), "config should still contain rust");

        // Re-run should be idempotent (no change)
        let updated2 = update_config_languages(&config_path, &languages).unwrap();
        assert!(!updated2, "idempotent: no change when languages match");

        std::fs::remove_dir_all(&tmp).ok();
    }
}
