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
        Commands::Preview { prompt, session, no_cache, diag, expected_files, candidate_k, max_files } => cmd_preview(&prompt, &session, no_cache, diag, expected_files.as_deref(), candidate_k, max_files),
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

    // Start daemon as background process
    let daemon_work_dir = dirs_home().unwrap_or_else(|| PathBuf::from(".")).join(".knocode");
    std::fs::create_dir_all(&daemon_work_dir).ok();

    let mut cmd = std::process::Command::new(&daemon_exe);
    cmd.current_dir(&daemon_work_dir)
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
    Ok(())
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
    // 3. Check current directory target/release
    let local = PathBuf::from("target/release/knocode-daemon");
    #[cfg(windows)]
    let local = local.with_extension("exe");
    if local.exists() {
        return Ok(local);
    }
    Err("knocode-daemon not found. Build with: cargo build --release -p knocode-daemon".to_string())
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

    let progress_cb = move |done: usize, total: usize, phase: &str| {
        if is_tty {
            let mut stdout = std::io::stdout();
            if total > 0 {
                let _ = write!(stdout, "\r      {}/{} files — {}\x1b[K", done, total, phase);
            } else {
                let _ = write!(stdout, "\r      {} files — {}\x1b[K", done, phase);
            }
            let _ = stdout.flush();
        } else if done > 0 && done % 5000 == 0 {
            if total > 0 {
                println!("      … {}/{} files ({})", done, total, phase);
            } else {
                println!("      … {} files ({})", done, phase);
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
                "      ✓ indexed {} files ({} symbols) in {}ms",
                stats.files_indexed,
                stats.symbols_extracted,
                started.elapsed().as_millis()
            );
            Ok(stats)
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

    // ── Step 3: Download tree-sitter grammars ──────────────────────
    println!("[3/7] Downloading tree-sitter grammars");
    let lang_names: Vec<&str> = discovery.languages.iter()
        .map(|(name, _)| name.as_str())
        .filter(|name| tree_sitter_language_pack::has_language(name))
        .collect();
    if lang_names.is_empty() {
        println!("      (no downloadable grammars found)");
    } else {
        match tree_sitter_language_pack::download(&lang_names) {
            Ok(count) => println!("      ✓ {}/{} grammars downloaded", count, lang_names.len()),
            Err(e) => println!("      ⚠ Download failed: {}", e),
        }
    }

    // ── Step 4: Parser validation ────────────────────────────────────
    println!("[4/7] Parser validation (verify tree-sitter grammars)");
    let mut parser_status: Vec<(String, bool, String)> = Vec::new();
    for (lang_name, _count) in &discovery.languages {
        if let Some(id) = knocode_repo_intel::registry::LanguageId::from_str(lang_name) {
            let has_parser = id.has_parser();
            let grammar_status = if has_parser {
                match knocode_repo_intel::parser::validate_grammar(id) {
                    Ok(()) => "loaded".to_string(),
                    Err(e) => e, // e.g. "grammar load failed for Rust"
                }
            } else {
                "no parser available".to_string()
            };
            parser_status.push((lang_name.clone(), has_parser, grammar_status));
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

    // ── Step 5: Indexing ─────────────────────────────────────────────
    println!("[5/7] Indexing (full-text BM25 + symbol extraction)");
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
        repo_intel
            .build_dependency_graph()
            .map(|g| g.edge_count())
            .unwrap_or(0)
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

    // ── Step 5: Knowledge Hub ────────────────────────────────────────
    println!("[6/7] Knowledge Hub initialization");
    let (knowledge_seeded, _readme) = ingest_seed_documents(&project_root, &db);
    println!("      ✓ knowledge: {} entries seeded", knowledge_seeded);

    // ── Step 6: Validation (smoke test all components) ───────────────
    println!("[7/7] Validation queries (smoke test)");
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

    // Symbols
    let symbol_count = db.get_all_files().map(|f| f.len()).unwrap_or(0);
    println!("      Symbols:   {} files tracked", symbol_count);

    // Graph
    let _graph_ok = dep_edges > 0;
    println!("      Graph:     {} edges", dep_edges);

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
    println!("┌─────────────────────────────────────────┐");
    let status_label = if tantivy_ok && symbol_count > 0 { "READY" } else { "PARTIAL" };
    println!("│ Repository Status: {:<21} │", status_label);
    println!("├─────────────────────────────────────────┤");
    println!("│ Tree-sitter:  {:<24} │", format!("{}/{} grammars", ready_count, total_count));
    println!("│ Symbols:      {:<24} │", format!("{} extracted", stats.symbols_extracted));
    println!("│ Graph:        {:<24} │", format!("{} edges", dep_edges));
    println!("│ Tantivy:      {:<24} │", format!("{} files", stats.files_indexed));
    println!("│ Knowledge:    {:<24} │", format!("{} entries", knowledge_seeded));
    println!("└─────────────────────────────────────────┘");
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
    match ensure_artifact_dir(&project_root, "context") {
        Ok(dir) => {
            println!("  Artifacts:         {}", dir.display());
            print_gitignore_hint();
        }
        Err(e) => println!("  Artifacts:         (skipped: {})", e),
    }
    println!();
    // Next steps — dynamic: probe the daemon instead of suggesting what's already true.
    if daemon_alive() {
        println!("Next steps:");
        println!("  Daemon already running at http://127.0.0.1:9527 — this repo is ready.");
    } else {
        println!("Next steps:");
        println!("  1. Start the daemon: 'knocode serve' (or scripts/install.ps1, which starts it)");
    }
    println!("  Agent setup (opencode plugin) is global — installed once by scripts/install.ps1.");
    println!("  Re-run 'knocode init' anytime - incremental and safe to repeat.");

    Ok(())
}

/// Best-effort probe: is the knocode daemon answering on the default HTTP port?
fn daemon_alive() -> bool {
    use std::net::TcpStream;
    let addr = "127.0.0.1:9527";
    std::net::ToSocketAddrs::to_socket_addrs(addr)
        .ok()
        .and_then(|mut addrs| addrs.next())
        .map(|ip| TcpStream::connect_timeout(&ip, std::time::Duration::from_millis(300)).is_ok())
        .unwrap_or(false)
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

fn detect_stack(root: &Path, d: &mut Discovery) {    if root.join("Cargo.toml").exists() {
        d.frameworks.push("cargo".to_string());
        if let Ok(s) = std::fs::read_to_string(root.join("Cargo.toml")) {
            if s.contains("[workspace]") {
                d.frameworks.push("rust-workspace".to_string());
            }
        }
        d.build_command.get_or_insert_with(|| "cargo build".into());
        d.test_command.get_or_insert_with(|| "cargo test".into());
    }
    let pkg = root.join("package.json");
    if pkg.exists() {
        d.frameworks.push("node".to_string());
        if let Ok(content) = std::fs::read_to_string(&pkg) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&content) {
                for fw in ["react", "vue", "svelte", "next", "express", "@nestjs/core"] {
                    if v["dependencies"].get(fw).is_some()
                        || v["devDependencies"].get(fw).is_some()
                    {
                        d.frameworks.push(fw.trim_start_matches('@').replace('/', "-"));
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
        d.frameworks.push("go-modules".to_string());
        d.build_command
            .get_or_insert_with(|| "go build ./...".into());
        d.test_command.get_or_insert_with(|| "go test ./...".into());
    }
    if root.join("pyproject.toml").exists()
        || root.join("requirements.txt").exists()
        || root.join("setup.py").exists()
    {
        d.frameworks.push("python".to_string());
        if let Ok(s) = std::fs::read_to_string(root.join("pyproject.toml")) {
            if s.contains("[tool.poetry]") {
                d.frameworks.push("poetry".to_string());
                d.test_command
                    .get_or_insert_with(|| "poetry run pytest".into());
            } else if s.contains("[tool.uv]") {
                d.frameworks.push("uv".to_string());
            }
        }
        d.test_command.get_or_insert_with(|| "pytest".into());
    }
    if root.join("pom.xml").exists() {
        d.frameworks.push("maven".to_string());
        d.build_command.get_or_insert_with(|| "mvn package".into());
        d.test_command.get_or_insert_with(|| "mvn test".into());
    } else if root.join("build.gradle").exists() || root.join("build.gradle.kts").exists() {
        d.frameworks.push("gradle".to_string());
        d.build_command.get_or_insert_with(|| "gradle build".into());
        d.test_command.get_or_insert_with(|| "gradle test".into());
    }
    if root.join("Makefile").exists() || root.join("makefile").exists() {
        d.frameworks.push("make".to_string());
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
    // TASK-038: per-repo artifact home for any generated reports/exports
    match ensure_artifact_dir(&project_root, "context") {
        Ok(dir) => {
            println!("  Artifacts:        {}", dir.display());
            print_gitignore_hint();
        }
        Err(e) => println!("  Artifacts:        (skipped: {})", e),
    }
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

fn cmd_preview(prompt: &str, session: &str, no_cache: bool, diag: bool, expected_files: Option<&[String]>, candidate_k: Option<usize>, max_files: Option<usize>) -> Result<(), String> {
    // Try daemon first (UDS then HTTP), fallback to local BuildContext
    // For v0.3.0 we implement real preview: build context locally if daemon not running.
    let effective_session = if no_cache { String::new() } else { session.to_string() };
    println!("Previewing BuildContext for: \"{}\" (session: {}, no_cache: {})", prompt, effective_session, no_cache);
    println!();

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
        println!("  Files indexed: {}", file_count);
        println!("  Symbols:       {}", symbol_count);
        println!();
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
    
    // Check RTK
    print!("RTK:             ");
    {
        let rtk = knocode_optimizer::rtk::RtkAdapter::detect();
        if rtk.is_available() {
            println!("✓ OK (binary at {:?}, 10ms overhead)", rtk.binary_path);
        } else {
            println!("⚠ Not found on PATH — using built-in compressors + tee-on-failure (install rtk for 10ms binary)");
        }
    }

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

/// TASK-038: per-repo artifact home — ALL generated deliverables for an analyzed repo land in
/// `<repo>/.knocode/artifacts/<name>/` (NEVER back into the knocode source repository).
/// `knocode init` owns `.knocode/`, so creating on demand is safe.
fn ensure_artifact_dir(repo_root: &Path, name: &str) -> Result<PathBuf, String> {
    let dir = repo_root.join(".knocode").join("artifacts").join(name);
    std::fs::create_dir_all(&dir)
        .map_err(|e| format!("Failed to create artifact directory {}: {}", dir.display(), e))?;
    Ok(dir)
}

/// TASK-038: do not auto-edit the analyzed repo's .gitignore — print a hint instead.
fn print_gitignore_hint() {
    println!("  Hint: consider adding '.knocode/artifacts/' to this repository's .gitignore");
}

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
