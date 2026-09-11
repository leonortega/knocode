use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tracing::{info, warn};
#[cfg(feature = "fs-watcher")]
use tracing::debug;

/// Watch mode for auto-reindexing — defined in `knocode-core` so config, CLI and
/// daemon share one type (this module re-exports it for compatibility).
pub use knocode_core::WatchMode;

/// Debounce interval for filesystem events — avoids rapid-fire during git operations.
#[cfg(feature = "fs-watcher")]
const DEBOUNCE_MS: u64 = 500;

/// Max cache size for libgit2 (64 MB) — controls memory usage on large repos.
#[cfg(feature = "fs-watcher")]
const LIBGIT2_CACHE_MAX_BYTES: usize = 64 * 1024 * 1024;

/// Git-change-triggered incremental watcher with two modes:
/// - **Commit** (default): Polls the resolved HEAD commit, triggers re-index only on new commits.
/// - **Filesystem**: Uses `notify` crate to watch for any file changes (requires `fs-watcher` feature).
pub struct RepoWatcher {
    repo_path: PathBuf,
    running: Arc<AtomicBool>,
    poll_interval: Duration,
    mode: WatchMode,
}

impl RepoWatcher {
    pub fn new(repo_path: PathBuf) -> Self {
        Self {
            repo_path,
            running: Arc::new(AtomicBool::new(false)),
            poll_interval: Duration::from_secs(5),
            mode: WatchMode::default(),
        }
    }

    pub fn with_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    pub fn with_mode(mut self, mode: WatchMode) -> Self {
        self.mode = mode;
        self
    }

    pub fn mode(&self) -> WatchMode {
        self.mode
    }

    /// Spawn background watcher task that calls `on_change` when re-indexing is needed.
    /// Both modes run `on_change` on tokio's blocking pool, so the callback may do
    /// blocking work (e.g. a full `index_repository()` walk) without stalling async I/O.
    pub fn spawn<F>(&self, on_change: F) -> tokio::task::JoinHandle<()>
    where
        F: Fn() + Send + Sync + 'static,
    {
        let on_change = Arc::new(on_change);

        match self.mode {
            WatchMode::Filesystem => self.spawn_filesystem_watcher(on_change),
            WatchMode::Commit => self.spawn_commit_watcher(on_change),
        }
    }

    // ── Commit Mode (default) ──────────────────────────────────────────

    /// Watch for new git commits by polling the resolved HEAD commit OID.
    /// Reading `.git/HEAD` alone is NOT enough — on a branch checkout HEAD is the
    /// static string `ref: refs/heads/<branch>` and never changes on commit. We
    /// resolve the actual commit via git2 (handles symbolic refs, packed refs,
    /// detached HEAD and worktrees) and compare the OID instead.
    fn spawn_commit_watcher<F>(&self, on_change: Arc<F>) -> tokio::task::JoinHandle<()>
    where
        F: Fn() + Send + Sync + 'static,
    {
        let repo_path = self.repo_path.clone();
        let running = self.running.clone();
        let interval = self.poll_interval;

        // Commit mode needs a real git repo with at least one commit to watch.
        if resolve_head_oid(&repo_path).is_none() {
            warn!(
                path = %repo_path.display(),
                "Commit watch mode requires a git repository with commits — auto-reindex \
                 disabled (use 'filesystem' mode for non-git repos)"
            );
            return tokio::task::spawn(async {});
        }

        running.store(true, Ordering::Relaxed);

        tokio::task::spawn_blocking(move || {
            info!(
                path = %repo_path.display(),
                interval_secs = interval.as_secs(),
                "Repo watcher started (commit mode — polling HEAD commit)"
            );

            let mut last_oid = resolve_head_oid(&repo_path);
            while running.load(Ordering::Relaxed) {
                std::thread::sleep(interval);
                // A transient resolution failure (e.g. mid-checkout) is skipped — we
                // keep the last known OID so the watcher doesn't fire spuriously.
                let Some(cur_oid) = resolve_head_oid(&repo_path) else {
                    continue;
                };
                if Some(cur_oid.as_str()) != last_oid.as_deref() {
                    info!(
                        old = ?last_oid,
                        new = %cur_oid,
                        "New commit detected — triggering re-index"
                    );
                    last_oid = Some(cur_oid);
                    on_change();
                }
            }
        })
    }

    // ── Filesystem Mode ────────────────────────────────────────────────

    /// Watch file system changes via `notify` crate.
    /// Triggers re-index on any file modification (debounced).
    #[cfg(feature = "fs-watcher")]
    fn spawn_filesystem_watcher<F>(&self, on_change: Arc<F>) -> tokio::task::JoinHandle<()>
    where
        F: Fn() + Send + Sync + 'static,
    {
        use notify::{RecommendedWatcher, RecursiveMode, Watcher};

        // Configure libgit2 memory cache
        configure_git2_cache();

        // Create debounced notify watcher
        let (tx, rx) = std::sync::mpsc::channel();
        let mut debouncer = match RecommendedWatcher::new(
            tx,
            notify::Config::default().with_poll_interval(Duration::from_millis(200)),
        ) {
            Ok(w) => w,
            Err(e) => {
                warn!(error = %e, "Failed to create notify watcher — falling back to commit mode");
                return self.spawn_commit_watcher(on_change);
            }
        };

        // Watch the repo directory recursively
        if let Err(e) = debouncer.watch(&self.repo_path, RecursiveMode::Recursive) {
            warn!(
                error = %e,
                path = %self.repo_path.display(),
                "Failed to watch repo directory — falling back to commit mode"
            );
            return self.spawn_commit_watcher(on_change);
        }

        let repo_path = self.repo_path.clone();
        let running = self.running.clone();
        running.store(true, Ordering::Relaxed);

        let handle = tokio::task::spawn_blocking(move || {
            let _watcher = debouncer;

            info!(
                path = %repo_path.display(),
                debounce_ms = DEBOUNCE_MS,
                "Repo watcher started (filesystem mode — notify + git2 dirty check)"
            );

            // Debounce loop: reset timer on each event, fire after quiet period
            loop {
                if !running.load(Ordering::Relaxed) {
                    break;
                }

                match rx.recv_timeout(Duration::from_secs(1)) {
                    Ok(_event) => {
                        // Got an event — drain any queued events with short timeout
                        let debounce_deadline =
                            std::time::Instant::now() + Duration::from_millis(DEBOUNCE_MS);
                        loop {
                            let remaining = debounce_deadline
                                .checked_duration_since(std::time::Instant::now())
                                .unwrap_or_default();
                            if remaining.is_zero() {
                                break;
                            }
                            match rx.recv_timeout(remaining) {
                                Ok(_event) => continue,
                                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                                    warn!("Notify channel disconnected");
                                    return;
                                }
                            }
                        }

                        // Only re-index for real changes. When the repo isn't a git repo
                        // (or has no HEAD yet) we can't diff, so treat file events as changes.
                        match repo_has_changes(&repo_path) {
                            Some(true) => {
                                debug!(
                                    path = %repo_path.display(),
                                    "Repo dirty — triggering incremental index"
                                );
                                on_change();
                            }
                            Some(false) => {
                                debug!(
                                    path = %repo_path.display(),
                                    "Filesystem event but repo clean — skipping"
                                );
                            }
                            None => {
                                debug!(
                                    path = %repo_path.display(),
                                    "No git HEAD to diff — treating file event as a change"
                                );
                                on_change();
                            }
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        warn!("Notify channel disconnected");
                        break;
                    }
                }
            }
        });

        handle
    }

    /// Stub when `fs-watcher` feature is not compiled in — falls back to commit mode.
    #[cfg(not(feature = "fs-watcher"))]
    fn spawn_filesystem_watcher<F>(&self, on_change: Arc<F>) -> tokio::task::JoinHandle<()>
    where
        F: Fn() + Send + Sync + 'static,
    {
        warn!(
            "fs-watcher feature not enabled — falling back to commit mode"
        );
        self.spawn_commit_watcher(on_change)
    }

    pub fn stop(&self) {
        self.running.store(false, Ordering::Relaxed);
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::Relaxed)
    }
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Configure libgit2 memory cache (64 MB).
#[cfg(feature = "fs-watcher")]
fn configure_git2_cache() {
    // SAFETY: set_cache_max_size only modifies internal libgit2 cache limits.
    // Safe to call once at startup before any concurrent repo operations.
    unsafe {
        if let Err(e) = git2::opts::set_cache_max_size(LIBGIT2_CACHE_MAX_BYTES as isize) {
            warn!(error = %e, "Failed to set libgit2 cache size — using defaults");
        }
    }
}

/// Resolve the full commit OID that HEAD currently points to.
/// Handles symbolic refs, packed refs, detached HEAD and worktrees — anything
/// `.git/HEAD` file reads cannot. Returns `None` when the repo can't be opened
/// or HEAD has no commit yet (fresh/unborn branch, non-git directory).
fn resolve_head_oid(repo_path: &Path) -> Option<String> {
    let repo = git2::Repository::open(repo_path).ok()?;
    let head = repo.head().ok()?;
    head.target().map(|oid| oid.to_string())
}

/// Determine whether the repo has uncommitted changes relative to HEAD.
///
/// Returns:
/// - `Some(true)` — dirty (staged/unstaged/untracked changes present)
/// - `Some(false)` — clean
/// - `None` — repo can't be opened or has no HEAD (fresh repo / non-git dir);
///   callers should treat file events as changes.
#[cfg(feature = "fs-watcher")]
fn repo_has_changes(repo_path: &Path) -> Option<bool> {
    let repo = git2::Repository::open(repo_path).ok()?;
    let head = repo.head().ok()?;
    let tree = head.peel_to_tree().ok()?;

    // Include untracked files so new (unstaged) files are detected
    let mut opts = git2::DiffOptions::new();
    opts.include_untracked(true);
    let diff = repo.diff_tree_to_workdir(Some(&tree), Some(&mut opts)).ok()?;
    Some(diff.deltas().len() > 0)
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// Create a git repo in `dir` with one initial commit.
    fn init_repo(dir: &std::path::Path) {
        let repo = git2::Repository::init(dir).unwrap();
        commit_file(&repo, "initial.txt", "init", "initial commit");
    }

    /// Stage + commit a single file in the given repo.
    fn commit_file(repo: &git2::Repository, name: &str, content: &str, msg: &str) {
        let workdir = repo.workdir().expect("repo has a workdir");
        std::fs::write(workdir.join(name), content).unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(std::path::Path::new(name)).unwrap();
        index.write().unwrap();
        let tree_id = index.write_tree().unwrap();
        let tree = repo.find_tree(tree_id).unwrap();
        let sig = git2::Signature::now("knocode-test", "test@knocode.dev").unwrap();
        let parents: Vec<git2::Commit> = repo
            .head()
            .ok()
            .and_then(|h| h.peel_to_commit().ok())
            .into_iter()
            .collect();
        let parent_refs: Vec<&git2::Commit> = parents.iter().collect();
        repo.commit(Some("HEAD"), &sig, &sig, msg, &tree, &parent_refs)
            .unwrap();
    }

    #[test]
    fn test_watcher_new_default_mode() {
        let w = RepoWatcher::new(PathBuf::from("."));
        assert!(!w.is_running());
        assert_eq!(w.mode(), WatchMode::Commit);
    }

    #[test]
    fn test_watch_mode_from_str() {
        assert_eq!("commit".parse::<WatchMode>().unwrap(), WatchMode::Commit);
        assert_eq!("git".parse::<WatchMode>().unwrap(), WatchMode::Commit);
        assert_eq!(
            "filesystem".parse::<WatchMode>().unwrap(),
            WatchMode::Filesystem
        );
        assert_eq!("fs".parse::<WatchMode>().unwrap(), WatchMode::Filesystem);
        assert!("invalid".parse::<WatchMode>().is_err());
    }

    #[test]
    fn test_watch_mode_display() {
        assert_eq!(WatchMode::Commit.to_string(), "commit");
        assert_eq!(WatchMode::Filesystem.to_string(), "filesystem");
    }

    #[test]
    fn test_watch_mode_default_is_commit() {
        assert_eq!(WatchMode::default(), WatchMode::Commit);
    }

    #[tokio::test]
    async fn test_commit_watcher_detects_new_commit() {
        let dir = std::env::temp_dir().join(format!(
            "knocode_watcher_commit_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);

        let w = RepoWatcher::new(dir.clone())
            .with_mode(WatchMode::Commit)
            .with_interval(Duration::from_millis(50));

        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();

        let handle = w.spawn(move || {
            flag_clone.store(true, Ordering::Relaxed);
        });

        assert!(w.is_running());

        // Let the watcher read the initial HEAD commit
        tokio::time::sleep(Duration::from_millis(150)).await;
        assert!(!flag.load(Ordering::Relaxed), "should not fire on startup");

        // A real new commit (the common branch-based workflow — .git/HEAD text is
        // unchanged by this; only the resolved OID moves)
        let repo = git2::Repository::open(&dir).unwrap();
        commit_file(&repo, "second.txt", "more", "second commit");
        drop(repo);

        // Wait for the poll to detect the new OID
        tokio::time::sleep(Duration::from_millis(300)).await;

        assert!(
            flag.load(Ordering::Relaxed),
            "Commit watcher should detect a new commit on the current branch"
        );

        w.stop();
        handle.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_commit_watcher_ignores_file_changes() {
        let dir = std::env::temp_dir().join(format!(
            "knocode_watcher_commit_noop_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);

        let w = RepoWatcher::new(dir.clone())
            .with_mode(WatchMode::Commit)
            .with_interval(Duration::from_millis(50));

        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();

        let handle = w.spawn(move || {
            flag_clone.store(true, Ordering::Relaxed);
        });

        assert!(w.is_running());
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Touch a file (but don't commit)
        std::fs::write(dir.join("new_file.rs"), "fn main() {}").unwrap();

        // Wait — commit watcher should NOT trigger
        tokio::time::sleep(Duration::from_millis(250)).await;

        assert!(
            !flag.load(Ordering::Relaxed),
            "Commit watcher should NOT trigger on plain file changes"
        );

        w.stop();
        handle.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn test_commit_watcher_noop_on_non_git_dir() {
        let dir = std::env::temp_dir().join(format!(
            "knocode_watcher_nongit_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let w = RepoWatcher::new(dir.clone())
            .with_mode(WatchMode::Commit)
            .with_interval(Duration::from_millis(50));

        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();

        let handle = w.spawn(move || {
            flag_clone.store(true, Ordering::Relaxed);
        });

        // Not a git repo → watcher must not pretend to run
        assert!(!w.is_running());
        std::fs::write(dir.join("a.txt"), "v1").unwrap();
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(!flag.load(Ordering::Relaxed));

        handle.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "fs-watcher")]
    #[test]
    fn test_repo_has_changes() {
        let dir = std::env::temp_dir().join(format!(
            "knocode_watcher_dirty_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);

        // Clean repo → Some(false)
        assert_eq!(repo_has_changes(&dir), Some(false));

        // Uncommitted change → Some(true)
        std::fs::write(dir.join("new_file.rs"), "fn main() {}").unwrap();
        assert_eq!(repo_has_changes(&dir), Some(true));

        // Non-git dir → None (treat file events as changes)
        let plain = std::env::temp_dir().join(format!(
            "knocode_watcher_notgit_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&plain).unwrap();
        assert_eq!(repo_has_changes(&plain), None);

        let _ = std::fs::remove_dir_all(&dir);
        let _ = std::fs::remove_dir_all(&plain);
    }

    #[cfg(feature = "fs-watcher")]
    #[tokio::test]
    async fn test_filesystem_watcher_triggers_on_dirty_file() {
        let dir = std::env::temp_dir().join(format!(
            "knocode_watcher_fs_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        init_repo(&dir);

        let w = RepoWatcher::new(dir.clone())
            .with_mode(WatchMode::Filesystem)
            .with_interval(Duration::from_millis(50));

        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();

        let handle = w.spawn(move || {
            flag_clone.store(true, Ordering::Relaxed);
        });

        assert!(w.is_running());

        // Wait for watcher to start
        tokio::time::sleep(Duration::from_millis(150)).await;

        // Create a dirty file
        std::fs::write(dir.join("dirty.rs"), "fn main() {}").unwrap();

        // Wait for debounce + dirty check
        tokio::time::sleep(Duration::from_millis(800)).await;

        assert!(
            flag.load(Ordering::Relaxed),
            "Filesystem watcher should detect dirty repo"
        );

        w.stop();
        handle.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[cfg(feature = "fs-watcher")]
    #[tokio::test]
    async fn test_filesystem_watcher_triggers_on_non_git_dir() {
        let dir = std::env::temp_dir().join(format!(
            "knocode_watcher_fs_nongit_{}",
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let w = RepoWatcher::new(dir.clone())
            .with_mode(WatchMode::Filesystem)
            .with_interval(Duration::from_millis(50));

        let flag = Arc::new(AtomicBool::new(false));
        let flag_clone = flag.clone();

        let handle = w.spawn(move || {
            flag_clone.store(true, Ordering::Relaxed);
        });

        assert!(w.is_running());

        // Wait for watcher to start, then create a file — no git repo to diff, so
        // the file event itself must trigger a re-index.
        tokio::time::sleep(Duration::from_millis(150)).await;
        std::fs::write(dir.join("new.txt"), "content").unwrap();
        tokio::time::sleep(Duration::from_millis(800)).await;

        assert!(
            flag.load(Ordering::Relaxed),
            "Filesystem watcher should trigger on file events in a non-git dir"
        );

        w.stop();
        handle.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }
}
