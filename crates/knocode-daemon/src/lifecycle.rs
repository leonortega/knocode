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

// ── Daemon State ────────────────────────────────────────────────────────

#[allow(clippy::arc_with_non_send_sync)]
pub struct DaemonState {
    pub config: Config,
    pub event_bus: EventBus,
    pub context_engine: Arc<tokio::sync::Mutex<knocode_context::ContextEngine>>,
    /// Auto-reindex watcher — owned by the state so graceful shutdown can call
    /// `stop()` and end its poll loops instead of letting them run until process exit.
    pub watcher: RepoWatcher,
    pub shutdown_flag: Arc<AtomicBool>,
    pub force_shutdown_flag: Arc<AtomicBool>,
}

impl DaemonState {
    /// Initialize daemon state from config
    pub fn initialize(config: Config) -> Result<Self, String> {
        // Initialize logging
        initialize_logging(&config.logging.level);

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

        // Initialize repository intelligence
        let repo_path = std::env::current_dir()
            .map_err(|e| format!("Failed to get current directory: {}", e))?;

        // Auto-reindex watcher (started in serve(); owned here so shutdown can stop it)
        let watcher = RepoWatcher::new(repo_path.clone()).with_mode(config.index.watch_mode);

        let repo_db_path = expand_path(&config.database.path);
        let repo_db = Database::open(&repo_db_path)
            .map_err(|e| format!("Failed to open database for repo-intel: {}", e))?;

        let repo_intel = RepositoryIntelligence::new(
            repo_path,
            repo_db,
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
        );

        Ok(Self {
            config,
            event_bus,
            context_engine: Arc::new(tokio::sync::Mutex::new(context_engine)),
            watcher,
            shutdown_flag,
            force_shutdown_flag,
        })
    }

    /// Start the daemon
    pub async fn serve(&self) -> Result<(), String> {
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
        let http_port = 9527;
        let http_handle = tokio::spawn(async move {
            if let Err(e) = crate::http_server::start_http_server(http_port, http_state).await {
                error!(error = %e, "HTTP server error");
            }
        });

        // ── Startup indexing — READINESS GATED ──────────────────────────────
        // The UDS/MessagePack adapter does NOT bind until the initial index
        // finishes (success or failure), so no request on the primary transport can
        // ever wait on the engine lock mid-index or race a half-built index.
        // Indexing goes through the ContextEngine — the SAME single index path the
        // auto-reindex watcher uses below — and runs on tokio's blocking pool.
        // (A Ctrl+C/SIGTERM during this phase terminates the process; only the
        // health/metrics HTTP listener is up, so there is nothing to drain.)
        let indexing_db_path = expand_path(&self.config.database.path);
        let indexing_engine = self.context_engine.clone();
        info!("Running initial repository indexing (UDS adapter binds when it completes; HTTP /hook returns 503 until ready)...");
        // blocking_lock is safe here: spawn_blocking thread, never an async task.
        let result = tokio::task::spawn_blocking(move || {
            indexing_engine.blocking_lock().reindex_repository(None)
        })
        .await;
        match result {
            Ok(Ok(stats)) => {
                // TASK-034/F-5: report the REAL indexed-file count from SQLite —
                // `files_indexed` is per-run (incremental runs are small); the store
                // handle used by reindex_repository is already dropped.
                let file_count = Database::open(&indexing_db_path).and_then(|db| db.get_file_count());
                match file_count {
                    Ok(count) => crate::metrics::global().set_index_files(count),
                    Err(_) => crate::metrics::global().set_index_files(stats.files_indexed),
                }
                info!(
                    files = stats.files_indexed,
                    symbols = stats.symbols_extracted,
                    duration_ms = stats.duration_ms,
                    "Initial indexing complete — binding listeners"
                );
            }
            Ok(Err(e)) => {
                // Bind anyway: retrieval degrades to the ripgrep fallback and the
                // watcher below keeps retrying on repo changes.
                crate::metrics::global().inc_fail_open();
                error!(error = %e, "Initial indexing failed — binding listeners anyway");
            }
            Err(join_err) => {
                crate::metrics::global().inc_fail_open();
                error!(error = %join_err, "Initial indexing panicked — binding listeners anyway");
            }
        }

        // Initial index done (success or failure — we serve either way): flip
        // readiness so /hook accepts requests and clients stop polling.
        crate::metrics::global().set_readiness(crate::metrics::Readiness::Ready);

        // ── Auto-reindex watcher ────────────────────────────────────────────
        // Re-indexes when the repo changes after startup (mode from `[index].watch_mode`:
        // "commit" on new git commits, "filesystem" on any file change). The initial
        // index above already finished (readiness gate), so the watcher starts here and
        // the two never write to the SQLite/tantivy stores concurrently. `self.watcher`
        // is owned by DaemonState so graceful shutdown can call `stop()` — the poll
        // loop exits instead of running until process exit.
        let watcher_db_path = expand_path(&self.config.database.path);
        let watcher_engine = self.context_engine.clone();
        info!(mode = %self.watcher.mode(), "Starting auto-reindex watcher");

        // Callback runs on tokio's blocking pool — safe to do a full index walk.
        // Re-indexing goes THROUGH the shared ContextEngine (not an ad-hoc
        // RepositoryIntelligence), so the engine's cached tantivy handles are
        // invalidated on completion and queries issued right after a change serve
        // the fresh commit immediately instead of from a stale reader.
        // `blocking_lock` is deliberate: this runs on a blocking thread, never
        // inside an async task, so it cannot deadlock the runtime.
        // The JoinHandle is retained so graceful shutdown can await the loop's exit.
        let watch_handle = self.watcher.spawn(move || {
            // Flip to indexing so /health + /metrics report the reindex and /hook
            // 503s — clients back off instead of queueing on the engine lock.
            crate::metrics::global().set_readiness(crate::metrics::Readiness::Indexing);
            let t0 = std::time::Instant::now();
            let engine = watcher_engine.blocking_lock();
            match engine.reindex_repository(None) {
                Ok(stats) => {
                    // Report the REAL indexed-file count from SQLite (the reindex
                    // wrote through the engine's DB connection to the same store).
                    let file_count = Database::open(&watcher_db_path)
                        .and_then(|db| db.get_file_count())
                        .unwrap_or(stats.files_indexed);
                    crate::metrics::global().set_index_files(file_count);
                    info!(
                        files = stats.files_indexed,
                        symbols = stats.symbols_extracted,
                        duration_ms = stats.duration_ms,
                        took_ms = t0.elapsed().as_millis(),
                        "Auto-reindex complete"
                    );
                }
                Err(e) => {
                    crate::metrics::global().inc_fail_open();
                    error!(error = %e, "Auto-reindex failed");
                }
            }
            // Reindex done (success or failure) — serve again.
            crate::metrics::global().set_readiness(crate::metrics::Readiness::Ready);
        });



        // Wait for shutdown signal
        info!("Daemon ready. Press Ctrl+C to shutdown.");
        wait_for_shutdown(self.shutdown_flag.clone(), self.force_shutdown_flag.clone()).await;

        // Graceful shutdown
        info!("Shutting down gracefully...");

        // Wait for HTTP server to finish (timeout)
        let _ = tokio::time::timeout(Duration::from_secs(5), http_handle).await;

        // Stop the auto-reindex watcher and wait for its poll loop to exit
        // (commit mode: within one poll interval; filesystem: within ~1s + debounce).
        self.watcher.stop();
        let _ = tokio::time::timeout(Duration::from_secs(10), watch_handle).await;

        info!("Daemon shutdown complete");
        Ok(())
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

fn initialize_logging(level: &str) {
    use tracing_subscriber::EnvFilter;

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(level));

    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_target(true)
        .with_thread_ids(true)
        .init();
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

}
