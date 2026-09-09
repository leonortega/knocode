use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tracing::{error, info, warn};


use knocode_core::Config;
use knocode_events::EventBus;
use knocode_knowledge::KnowledgeHub;
use knocode_repo_intel::watcher::RepoWatcher;
use knocode_repo_intel::RepositoryIntelligence;
use knocode_storage::Database;

/// Default HTTP bind port — shared by `serve()`, the daemon binary's `--port`
/// default, and the CLI's `knocode serve` default (9527).
pub const DEFAULT_HTTP_PORT: u16 = 9527;

// ── Daemon State ────────────────────────────────────────────────────────

/// One auto-reindex watcher per known repository (multi-repo daemon).
struct WatcherEntry {
    watcher: RepoWatcher,
    handle: tokio::task::JoinHandle<()>,
}

#[allow(clippy::arc_with_non_send_sync)]
pub struct DaemonState {
    pub config: Config,
    /// Shared SQLite store (migration 008: rows are per-repository) — used by the
    /// context engine for lazy per-repo indexing and by the watcher registry.
    pub db_path: PathBuf,
    /// Repos to index + watch eagerly at startup (from `--repo` flags). A multi-repo
    /// daemon starts repo-neutral: repos WITHOUT a `--repo` flag are indexed lazily
    /// on their first request and watched from then on.
    pub eager_repos: Vec<PathBuf>,
    pub event_bus: EventBus,
    pub context_engine: Arc<tokio::sync::Mutex<knocode_context::ContextEngine>>,
    /// Auto-reindex watcher registry — one RepoWatcher per known repository, keyed by
    /// canonical path string. Arc'd so the context engine's `on_repo_registered`
    /// callback can spawn watchers for lazily-resolved repos. Graceful shutdown
    /// stops every entry's poll loop.
    watchers: Arc<std::sync::Mutex<HashMap<String, WatcherEntry>>>,
    pub shutdown_flag: Arc<AtomicBool>,
    pub force_shutdown_flag: Arc<AtomicBool>,
}

impl DaemonState {
    /// Initialize daemon state from config.
    ///
    /// Multi-repo design: the daemon does NOT bind to a single repository. The
    /// default repo intelligence (used only as the fallback when a request carries
    /// no `repository_path`) resolves from the process CWD; every other repo is
    /// resolved, lazily indexed and watched per request. `eager_repos` (from
    /// `--repo` flags) are indexed + watched at startup instead.
    pub fn initialize(config: Config, eager_repos: Vec<PathBuf>) -> Result<Self, String> {
        // Initialize logging — stdout AND the configured file (`logging.file_path`).
        // The file layer is the only durable one: `knocode serve` spawns this process
        // with null stdio, so without it every freeze/fail-open was invisible.
        initialize_logging(&config.logging.level, &config.logging.file_path);

        info!("Initializing daemon...");

        // Open database
        let db_path = expand_path(&config.database.path);
        let db_dir = db_path.parent().unwrap_or(&db_path);
        std::fs::create_dir_all(db_dir)
            .map_err(|e| format!("Failed to create database directory: {}", e))?;

        info!(path = %db_path.display(), "Database directory ready");

        // Initialize event bus
        let event_bus = EventBus::new();

        let shutdown_flag = Arc::new(AtomicBool::new(false));
        let force_shutdown_flag = Arc::new(AtomicBool::new(false));

        // Default (fallback) repository intelligence — process CWD. NOT indexed at
        // startup: without `--repo` the daemon is repo-neutral and indexes repos
        // lazily on first request via the context engine.
        let cwd = std::env::current_dir()
            .map_err(|e| format!("Failed to get current directory: {e}"))?;
        info!(fallback_repo = %cwd.display(), eager_repos = eager_repos.len(), "Daemon initialized (multi-repo mode)");

        let default_db = Database::open(&db_path)
            .map_err(|e| format!("Failed to open database for repo-intel: {}", e))?;
        let repo_intel = RepositoryIntelligence::new(
            cwd,
            default_db,
            event_bus.clone(),
        );

        // Initialize knowledge hub
        let knowledge_db_path = expand_path(&config.database.path);
        let knowledge_db = Database::open(&knowledge_db_path)
            .map_err(|e| format!("Failed to open database for knowledge: {}", e))?;

        let knowledge_hub = KnowledgeHub::new(
            knowledge_db,
            event_bus.clone(),
        );

        // Initialize context engine
        let context_config = knocode_context::ContextConfig {
            max_tokens: config.context.max_tokens,
            max_files: config.context.max_files,
            max_lines_per_file: config.context.max_lines_per_file,
            cache_order: config.context.cache_order.clone(),
            candidate_k: config.context.candidate_k,
        };
        let context_engine = knocode_context::ContextEngine::new(
            repo_intel,
            knowledge_hub,
            event_bus.clone(),
            context_config,
        )
        .with_db_path(db_path.clone());

        Ok(Self {
            config,
            db_path,
            eager_repos,
            event_bus,
            context_engine: Arc::new(tokio::sync::Mutex::new(context_engine)),
            watchers: Arc::new(std::sync::Mutex::new(HashMap::new())),
            shutdown_flag,
            force_shutdown_flag,
        })
    }

    /// Start the daemon
    /// Start the daemon on the default HTTP port.
    pub async fn serve(&self) -> Result<(), String> {
        self.serve_on(DEFAULT_HTTP_PORT).await
    }

    /// Start the daemon on an explicit HTTP port (`knocode-daemon --port <N>`,
    /// forwarded by `knocode serve --port <N>`).
    pub async fn serve_on(&self, http_port: u16) -> Result<(), String> {
        info!("Starting knocode daemon...");

        // Print startup banner
        print_banner(&self.config);

        // ── Readiness signal ────────────────────────────────────────────────
        // Default is already Indexing, but set it explicitly so the state machine
        // is obvious: indexing → ready → (indexing during auto-reindexes) → ready.
        crate::metrics::global().set_readiness(crate::metrics::Readiness::Indexing);

        // ── HTTP health/metrics listener binds FIRST ────────────────────────
        // GET /health and GET /metrics are reachable while the initial index runs
        // so clients can poll readiness; POST /hook returns 503 (daemon_indexing)
        // until the index completes instead of queueing on the engine lock.
        let http_state = crate::http_server::HttpServerState {
            context_engine: self.context_engine.clone(),
        };
        let http_handle = tokio::spawn(async move {
            if let Err(e) = crate::http_server::start_http_server(http_port, http_state).await {
                error!(error = %e, "HTTP server error");
            }
        });

        // ── Multi-repo: GC, watcher-callback wiring, eager `--repo` indexes ──
        // The daemon is repo-neutral at startup — it does NOT index its CWD. Only
        // explicitly passed `--repo` paths are indexed eagerly (readiness-gated);
        // every other repository is indexed lazily on its first request
        // (ContextEngine::resolve_repo_intel) and watched from then on.
        // Stale-repo GC — fire-and-forget on a blocking thread. Mass DELETEs plus
        // removal of multi-GB tantivy index dirs can take MINUTES; gating readiness
        // on it would leave /health "indexing" and /hook 503 for the whole run
        // (observed: 3m37s). The daemon serves immediately; GC finishes in background.
        let gc_db_path = self.db_path.clone();
        tokio::task::spawn_blocking(move || Self::prune_stale_repos_in(&gc_db_path));

        // Fire the watcher factory when the engine resolves a NEW repository, so
        // lazily-indexed repos are auto-reindexed on change too (not just --repo).
        // Registered on a blocking thread BEFORE the HTTP server binds, so no request
        // can resolve a repo before the callback exists. (blocking_lock is illegal on
        // the async runtime thread — hence spawn_blocking.)
        {
            let engine = self.context_engine.clone();
            let registry = self.watchers.clone();
            let db_path = self.db_path.clone();
            let watch_mode = self.config.index.watch_mode.clone();
            let _ = tokio::task::spawn_blocking(move || {
                let mut eng = engine.blocking_lock();
                let engine_cb = engine.clone();
                eng.set_on_repo_registered(Arc::new(move |path| {
                    Self::spawn_watcher_for(
                        &registry,
                        &engine_cb,
                        &db_path,
                        watch_mode.clone(),
                        std::path::Path::new(path),
                    );
                }));
            })
            .await;
        }

        // ── Readiness signal + HTTP health/metrics listener ─────────────────
        crate::metrics::global().set_readiness(crate::metrics::Readiness::Indexing);
        for repo in &self.eager_repos {
            let repo_str = repo.to_string_lossy().to_string();
            let engine = self.context_engine.clone();
            let path_for_index = repo_str.clone();
            let result = tokio::task::spawn_blocking(move || {
                engine.blocking_lock().reindex_repository(Some(&path_for_index))
            })
            .await;
            match result {
                Ok(Ok(stats)) => {
                    let repo_id = knocode_core::repository_id_from_path(&repo_str);
                    let file_count = Database::open(&self.db_path)
                        .and_then(|db| {
                            let _ = db.register_repo(&repo_id, &repo_str);
                            db.get_file_count_for_repo(&repo_id)
                        })
                        .unwrap_or(stats.files_indexed);
                    crate::metrics::global().set_index_files(file_count);
                    crate::metrics::global().set_index_age(0.0);
                    info!(
                        repo = %repo_str,
                        files = stats.files_indexed,
                        symbols = stats.symbols_extracted,
                        duration_ms = stats.duration_ms,
                        "Eager repository indexing complete"
                    );
                }
                Ok(Err(e)) => {
                    crate::metrics::global().inc_fail_open();
                    error!(repo = %repo_str, error = %e, "Eager indexing failed — repo will be lazily indexed on first request");
                }
                Err(join_err) => {
                    crate::metrics::global().inc_fail_open();
                    error!(repo = %repo_str, error = %join_err, "Eager indexing panicked");
                }
            }
            self.spawn_repo_watcher(repo);
        }
        // Eager work done (or nothing to do — repo-neutral startup): serve.
        crate::metrics::global().set_readiness(crate::metrics::Readiness::Ready);

        // Wait for shutdown signal
        info!("Daemon ready. Press Ctrl+C to shutdown.");
        wait_for_shutdown(self.shutdown_flag.clone(), self.force_shutdown_flag.clone()).await;

        // Graceful shutdown
        info!("Shutting down gracefully...");

        // Wait for HTTP server to finish (timeout)
        let _ = tokio::time::timeout(Duration::from_secs(5), http_handle).await;

        // Stop every registered auto-reindex watcher and wait for its poll loop
        // (commit mode: within one poll interval; filesystem: ~1s + debounce).
        let entries: Vec<WatcherEntry> = {
            let mut registry = self.watchers.lock().unwrap_or_else(|e| e.into_inner());
            registry.drain().map(|(_, e)| e).collect()
        };
        for entry in entries {
            entry.watcher.stop();
            let _ = tokio::time::timeout(Duration::from_secs(10), entry.handle).await;
        }

        info!("Daemon shutdown complete");
        Ok(())
    }

    /// Register + spawn an auto-reindex watcher for ONE repository (multi-repo mode).
    /// Delegates to the shared factory with this daemon's state.
    fn spawn_repo_watcher(&self, repo_path: &std::path::Path) {
        Self::spawn_watcher_for(
            &self.watchers,
            &self.context_engine,
            &self.db_path,
            self.config.index.watch_mode.clone(),
            repo_path,
        );
    }

    /// Shared watcher factory — used both for eager `--repo` repos at startup and
    /// (via the context engine's `on_repo_registered` callback) for repos resolved
    /// lazily from requests, so EVERY known repo gets auto-reindexed on change.
    fn spawn_watcher_for(
        registry: &Arc<std::sync::Mutex<HashMap<String, WatcherEntry>>>,
        engine: &Arc<tokio::sync::Mutex<knocode_context::ContextEngine>>,
        db_path: &std::path::Path,
        watch_mode: knocode_core::WatchMode,
        repo_path: &std::path::Path,
    ) {
        let key = dunce::canonicalize(repo_path)
            .unwrap_or_else(|_| repo_path.to_path_buf())
            .to_string_lossy()
            .to_string();
        {
            let registry = registry.lock().unwrap_or_else(|e| e.into_inner());
            if registry.contains_key(&key) {
                return;
            }
        }

        let watcher = RepoWatcher::new(repo_path.to_path_buf()).with_mode(watch_mode);
        let engine = engine.clone();
        let repo_str = key.clone();
        let db_path = db_path.to_path_buf();
        info!(repo = %repo_str, mode = %watcher.mode(), "Starting auto-reindex watcher");

        let handle = watcher.spawn(move || {
            let repo_id = knocode_core::repository_id_from_path(&repo_str);
            // Flip to indexing so /health + /metrics report the reindex and /hook
            // 503s — clients back off instead of queueing on the engine lock.
            crate::metrics::global().set_readiness(crate::metrics::Readiness::Indexing);
            crate::metrics::global().clear_index_age();
            let t0 = std::time::Instant::now();
            let reindex = engine.blocking_lock().reindex_repository(Some(&repo_str));
            match reindex {
                Ok(stats) => {
                    // Report the REAL indexed-file count from SQLite (per-repo rows,
                    // migration 008) for THIS watcher's repository.
                    let file_count = Database::open(&db_path)
                        .and_then(|db| db.get_file_count_for_repo(&repo_id))
                        .unwrap_or(stats.files_indexed);
                    crate::metrics::global().set_index_files(file_count);
                    crate::metrics::global().set_index_age(0.0);
                    info!(
                        repo = %repo_str,
                        files = stats.files_indexed,
                        symbols = stats.symbols_extracted,
                        duration_ms = stats.duration_ms,
                        took_ms = t0.elapsed().as_millis(),
                        "Auto-reindex complete"
                    );
                }
                Err(e) => {
                    crate::metrics::global().inc_fail_open();
                    error!(repo = %repo_str, error = %e, "Auto-reindex failed");
                }
            }
            // Reindex done (success or failure) — serve again.
            crate::metrics::global().set_readiness(crate::metrics::Readiness::Ready);
        });

        let mut registry = registry.lock().unwrap_or_else(|e| e.into_inner());
        registry.insert(key, WatcherEntry { watcher, handle });
    }

    /// Startup GC (background — see serve_on): drop DB rows + tantivy indices for
    /// repositories whose path no longer exists (repo deleted/moved), and legacy
    /// orphan rows from pre-registry runs (including the old daemon's `~\.knocode`
    /// self-indexing bug). Deleted repos are simply re-indexed lazily if a request
    /// for them ever arrives again.
    fn prune_stale_repos_in(db_path: &std::path::Path) {
        let db = match Database::open(db_path) {
            Ok(db) => db,
            Err(e) => {
                warn!(error = %e, "Stale-repo GC skipped: could not open database");
                return;
            }
        };

        let registered = match db.list_registered_repos() {
            Ok(r) => r,
            Err(e) => {
                warn!(error = %e, "Stale-repo GC skipped: could not list registry");
                return;
            }
        };

        let mut pruned_files = 0usize;
        // 1. Registered repos whose path vanished.
        for (repo_id, path) in &registered {
            if std::path::Path::new(path).is_dir() {
                continue;
            }
            match db.drop_repo_data(repo_id) {
                Ok(n) => pruned_files += n,
                Err(e) => warn!(repository_id = %repo_id, error = %e, "Failed to drop stale repo rows"),
            }
            let _ = std::fs::remove_dir_all(knocode_repo_intel::default_index_path(repo_id));
            let _ = db.delete_repo_registry_entry(repo_id);
            info!(repo = %path, repository_id = %repo_id, "Pruned stale repository (path no longer exists)");
        }

        // 2. Orphan file rows with no registry entry (legacy pre-registry runs), plus
        // the pre-migration-008 global bucket (repository_id='') which per-repo
        // retrieval never reads. Errors are LOGGED (not swallowed) so a transient
        // SQLite busy/lock at startup is visible and retried on the next restart.
        let registered_ids: std::collections::HashSet<&String> =
            registered.iter().map(|(id, _)| id).collect();
        let mut orphan_ids: Vec<String> = match db.list_file_repo_ids() {
            Ok(ids) => ids,
            Err(e) => {
                warn!(error = %e, "Stale-repo GC: could not list orphan repo ids");
                Vec::new()
            }
        };
        orphan_ids.push(String::new()); // legacy global bucket from migration 008
        for repo_id in &orphan_ids {
            if registered_ids.contains(repo_id) {
                continue;
            }
            match db.drop_repo_data(repo_id) {
                Ok(n) if n > 0 => {
                    pruned_files += n;
                    if !repo_id.is_empty() {
                        let _ = std::fs::remove_dir_all(knocode_repo_intel::default_index_path(repo_id));
                        info!(repository_id = %repo_id, files = n, "Pruned orphan repository rows (no registry entry)");
                    }
                }
                Ok(_) => {}
                Err(e) => warn!(repository_id = %repo_id, error = %e, "Stale-repo GC: failed to drop orphan rows — will retry on next restart"),
            }
        }

        if pruned_files > 0 {
            info!(files = pruned_files, "Stale-repo GC complete");
        }
    }
}

// ── Signal Handling ─────────────────────────────────────────────────────

async fn wait_for_shutdown(shutdown_flag: Arc<AtomicBool>, force_flag: Arc<AtomicBool>) {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};

        let mut sigint = signal(SignalKind::interrupt()).expect("Failed to register SIGINT handler");
        let mut sigterm = signal(SignalKind::terminate()).expect("Failed to register SIGTERM handler");
        let mut sighup = signal(SignalKind::hangup()).expect("Failed to register SIGHUP handler");

        loop {
            tokio::select! {
                _ = sigint.recv() => {
                    if shutdown_flag.load(Ordering::Relaxed) {
                        warn!("Second signal received, forcing shutdown");
                        force_flag.store(true, Ordering::Relaxed);
                        break;
                    }
                    info!("SIGINT received, initiating graceful shutdown");
                    shutdown_flag.store(true, Ordering::Relaxed);
                }
                _ = sigterm.recv() => {
                    if shutdown_flag.load(Ordering::Relaxed) {
                        warn!("Second signal received, forcing shutdown");
                        force_flag.store(true, Ordering::Relaxed);
                        break;
                    }
                    info!("SIGTERM received, initiating graceful shutdown");
                    shutdown_flag.store(true, Ordering::Relaxed);
                }
                _ = sighup.recv() => {
                    info!("SIGHUP received, reloading configuration");
                    // TODO: Implement config reload
                }
            }
        }
    }

    #[cfg(not(unix))]
    {
        // Windows: use tokio's ctrl_c
        match tokio::signal::ctrl_c().await {
            Ok(()) => {
                if shutdown_flag.load(Ordering::Relaxed) {
                    warn!("Second signal received, forcing shutdown");
                    force_flag.store(true, Ordering::Relaxed);
                } else {
                    info!("Ctrl+C received, initiating graceful shutdown");
                    shutdown_flag.store(true, Ordering::Relaxed);
                }
            }
            Err(e) => error!(error = %e, "Failed to listen for shutdown signal"),
        }
    }
}

// ── Helper Functions ────────────────────────────────────────────────────

/// Translate `[logging] level` into an `EnvFilter` directive.
///
/// Verbose levels (`debug`/`trace` = installer/CLI verbosity 2) are scoped to
/// FIRST-PARTY crates only: `info,knocode=<level>` prefix-matches every `knocode_*`
/// target (`knocode_daemon`, `knocode_context`, `knocode_storage`, …) so per-call
/// detail is kept while third-party debug noise (tantivy mmap opens, HTTP
/// handshakes, SQLite internals) stays capped at info. Quiet/normal levels pass
/// through unchanged. `RUST_LOG`, when set in the environment, still overrides
/// everything (`EnvFilter::try_from_default_env` is tried first).
fn filter_directive(level: &str) -> String {
    match level {
        "debug" | "trace" => format!("info,knocode={level}"),
        other => other.to_string(),
    }
}

fn initialize_logging(level: &str, file_path: &str) {
    use tracing_subscriber::{EnvFilter, layer::SubscriberExt, util::SubscriberInitExt};

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(filter_directive(level)));

    let log_path = expand_path(file_path);
    let (dir, name) = (
        log_path.parent().map(|p| p.to_path_buf()),
        log_path.file_name().map(|n| n.to_os_string()),
    );

    match (dir, name) {
        (Some(dir), Some(name)) if std::fs::create_dir_all(&dir).is_ok() => {
            // `rolling::never` = a plain single file in append mode (no rotation yet).
            // Sync (non-non-blocking) writes on purpose — a daemon crash must not
            // lose the log lines that explain the crash. The EnvFilter layer is
            // registered first so it filters BOTH the console and file layers.
            let file_writer = tracing_appender::rolling::never(&dir, name);
            tracing_subscriber::registry()
                .with(filter)
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_target(true)
                        .with_thread_ids(true)
                        .with_ansi(false),
                )
                .with(
                    tracing_subscriber::fmt::layer()
                        .with_writer(file_writer)
                        .with_target(true)
                        .with_thread_ids(true)
                        .with_ansi(false),
                )
                .init();
        }
        _ => {
            // Log file/dir couldn't be created — fall back to stdout-only.
            tracing_subscriber::fmt()
                .with_env_filter(filter)
                .with_target(true)
                .with_thread_ids(true)
                .init();
        }
    }
}

fn expand_path(path: &str) -> PathBuf {
    if path.starts_with("~/") || path.starts_with("~\\") {
        if let Some(home) = dirs() {
            return home.join(&path[2..]);
        }
    }
    PathBuf::from(path)
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

fn print_banner(config: &Config) {
    // Version single source of truth: root Cargo.toml [workspace.package].version
    let label = format!("Knocode AI Runtime v{}", env!("CARGO_PKG_VERSION"));
    let inner = 42usize;
    let pad_total = inner.saturating_sub(label.len());
    let pad_left = pad_total / 2;
    let pad_right = pad_total - pad_left;
    println!("╔══════════════════════════════════════════╗");
    println!("║{}{}{}║", " ".repeat(pad_left), label, " ".repeat(pad_right));
    println!("╚══════════════════════════════════════════╝");
    println!();
    println!("  Database: {}", config.database.path);
    println!("  Log level: {}", config.logging.level);
    println!();
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_expand_path_home() {
        let path = expand_path("~/test/path");
        assert!(path.to_string_lossy().contains("test/path"));
        assert!(!path.to_string_lossy().starts_with("~/"));
    }

    #[test]
    fn test_expand_path_absolute() {
        let path = expand_path("/absolute/path");
        assert_eq!(path, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_expand_path_relative() {
        let path = expand_path("relative/path");
        assert_eq!(path, PathBuf::from("relative/path"));
    }

    #[test]
    fn test_filter_directive_scopes_verbose_levels_to_first_party() {
        assert_eq!(filter_directive("debug"), "info,knocode=debug");
        assert_eq!(filter_directive("trace"), "info,knocode=trace");
        // Quiet/normal levels stay global (there is nothing to scope).
        assert_eq!(filter_directive("info"), "info");
        assert_eq!(filter_directive("warn"), "warn");
        assert_eq!(filter_directive("error"), "error");
    }

}
