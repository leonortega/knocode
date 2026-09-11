//! Mattermost Benchmark — 50 hard queries on 9k-file Go + React codebase.
//!
//! Run with: `cargo test -p knocode-context -- --ignored bench_mattermost_50 --nocapture`

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

use crate::retrieval::policy::RetrievalPolicy;
use crate::retrieval::query::RetrievalQuery;
use crate::retrieval::{CombinedRetriever, Retriever};
use knocode_events::EventBus;
use knocode_repo_intel::RepositoryIntelligence;
use knocode_storage::Database;

fn home_dir() -> PathBuf {
    std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(PathBuf::from))
        .unwrap_or_else(|_| PathBuf::from("."))
}

#[derive(Debug, Clone)]
struct BenchQuery {
    text: &'static str,
    grep_pattern: &'static str,
    category: &'static str,
}

/// 50 hard queries designed for Mattermost — a 9k-file Go + React codebase.
/// Tests: API understanding, cross-layer (Go ↔ React), config, auth, WebSocket, channels.
fn bench_queries() -> Vec<BenchQuery> {
    vec![
        // ── Procedural (10) — "how to" patterns ──
        BenchQuery { text: "how to add a new API endpoint", grep_pattern: "HandleFunc|router\\.|mux\\.", category: "procedural" },
        BenchQuery { text: "how to create a new React component", grep_pattern: "export.*function|export.*const.*=.*\\(|React\\.FC", category: "procedural" },
        BenchQuery { text: "how to add a new WebSocket event", grep_pattern: "WebSocket|ws\\.|websock|HandleWebSocket", category: "procedural" },
        BenchQuery { text: "how to add a database migration", grep_pattern: "migrate|Migration|ALTER TABLE|CREATE TABLE", category: "procedural" },
        BenchQuery { text: "how to add a new plugin hook", grep_pattern: "Plugin|Hook|OnActivate|MessageWillBePosted", category: "procedural" },
        BenchQuery { text: "how to add configuration option", grep_pattern: "Config\\{|config\\.go|ConfigSettings|EnvironmentConfig", category: "procedural" },
        BenchQuery { text: "how to add a new notification type", grep_pattern: "Notification|notify|PushNotification|emailNotification", category: "procedural" },
        BenchQuery { text: "how to add permissions check", grep_pattern: "Permission|HasPermissionTo|CheckPermission|Permissions", category: "procedural" },
        BenchQuery { text: "how to add rate limiting", grep_pattern: "RateLimit|rateLimiter|throttle|Limit", category: "procedural" },
        BenchQuery { text: "how to add a new scheduled job", grep_pattern: "Job|Scheduler|cron|periodicJobs", category: "procedural" },
        // ── Debugging (10) — "why" patterns ──
        BenchQuery { text: "why does the WebSocket connection drop", grep_pattern: "websocket.*close|disconnect|connection.*lost|ws.*error", category: "debugging" },
        BenchQuery { text: "why is the channel not loading", grep_pattern: "channel.*load|GetChannel|fetchChannel|channel.*error", category: "debugging" },
        BenchQuery { text: "why does the user session expire", grep_pattern: "Session|session.*expire|TokenExpire|session.*timeout", category: "debugging" },
        BenchQuery { text: "why is the message not being delivered", grep_pattern: "SendMessage|postMessage|delivery|message.*fail", category: "debugging" },
        BenchQuery { text: "why does the file upload fail", grep_pattern: "upload.*fail|FileUpload|MultipartForm|file.*error", category: "debugging" },
        BenchQuery { text: "why is the search returning wrong results", grep_pattern: "SearchPost|searchPosts|FullTextSearch|search.*index", category: "debugging" },
        BenchQuery { text: "why does the email notification not send", grep_pattern: "EmailNotification|sendEmail|SMTP|email.*send", category: "debugging" },
        BenchQuery { text: "why is the plugin failing to load", grep_pattern: "Plugin.*error|plugin.*activate|PluginAPI|manifest", category: "debugging" },
        BenchQuery { text: "why does the team invite fail", grep_pattern: "Invite|joinTeam|team.*invite|AddMembers", category: "debugging" },
        BenchQuery { text: "why is the permission denied error", grep_pattern: "PermissionDenied|Forbidden|403|not.*authorized", category: "debugging" },
        // ── Structural (10) — find patterns ──
        BenchQuery { text: "find all REST API handlers", grep_pattern: "HandleFunc|api\\.|Handler\\(|ServeHTTP", category: "structural" },
        BenchQuery { text: "find all WebSocket message types", grep_pattern: "WebSocketResponse|wsResponse|wss\\.|websocket\\.Message", category: "structural" },
        BenchQuery { text: "find all database models", grep_pattern: "type.*struct|model\\.Team|model\\.Channel|model\\.Post", category: "structural" },
        BenchQuery { text: "find all middleware functions", grep_pattern: "Middleware|middleware|Next\\(\\)|ChainHandler", category: "structural" },
        BenchQuery { text: "find all configuration structs", grep_pattern: "Config\\s*struct|ConfigSettings|config\\s+struct", category: "structural" },
        BenchQuery { text: "find all error types", grep_pattern: "AppError|StatusCode|error\\(|ErrNotFound", category: "structural" },
        BenchQuery { text: "find all hooks in the plugin system", grep_pattern: "OnActivate|OnDeactivate|MessageWillBePosted|ServeHTTP", category: "structural" },
        BenchQuery { text: "find all scheduled tasks", grep_pattern: "Job|Scheduler|cron|schedule|periodic", category: "structural" },
        BenchQuery { text: "find all Redux actions", grep_pattern: "dispatch|Action|ActionCreators|useDispatch", category: "structural" },
        BenchQuery { text: "find all test files", grep_pattern: "_test\\.go|\\.test\\.|\\.spec\\.|__tests__", category: "structural" },
        // ── Informational (10) — "what" patterns ──
        BenchQuery { text: "what is the overall architecture", grep_pattern: "architecture|overview|design|structure", category: "informational" },
        BenchQuery { text: "what is the authentication flow", grep_pattern: "Login|Authenticate|Session|Token.*auth", category: "informational" },
        BenchQuery { text: "what is the channel system", grep_pattern: "Channel|channel_type|public.*channel|private.*channel", category: "informational" },
        BenchQuery { text: "what is the plugin architecture", grep_pattern: "Plugin|plugin.*api|hook.*system|manifest", category: "informational" },
        BenchQuery { text: "what is the message format", grep_pattern: "Post\\{|MessageFormat|markdown|message.*struct", category: "informational" },
        BenchQuery { text: "what is the user management system", grep_pattern: "User|user.*model|UserAuth|login.*method", category: "informational" },
        BenchQuery { text: "what is the team structure", grep_pattern: "Team|TeamMember|team.*invite|join.*team", category: "informational" },
        BenchQuery { text: "what is the file storage backend", grep_pattern: "FileStore|S3Store|LocalStore|filestore|Storage", category: "informational" },
        BenchQuery { text: "what is the compliance and audit system", grep_pattern: "Compliance|Audit|audit.*log|retention|eDiscovery", category: "informational" },
        BenchQuery { text: "what is the clustering setup", grep_pattern: "Cluster|cluster.*node|gossip|peer|discovery", category: "informational" },
        // ── Mixed (10) — complex intent ──
        BenchQuery { text: "how does the channel member system work end to end", grep_pattern: "ChannelMember|AddMember|join.*channel|member.*count", category: "mixed" },
        BenchQuery { text: "find where permissions are checked for post creation", grep_pattern: "CreatePost|CanPost|permission.*post|draft.*permission", category: "mixed" },
        BenchQuery { text: "how is the notification preference stored and applied", grep_pattern: "NotificationPreference|NotifyProps|notification.*setting", category: "mixed" },
        BenchQuery { text: "what happens when a user is deactivated", grep_pattern: "Deactivate|deactivateUser|user.*deactivate|status.*offline", category: "mixed" },
        BenchQuery { text: "how does the real-time typing indicator work", grep_pattern: "typing|TypingIndicator|websocket.*typing|UserTyping", category: "mixed" },
        BenchQuery { text: "find the code path for importing slack channels", grep_pattern: "SlackImport|import.*slack|SlackConverter|import.*channel", category: "mixed" },
        BenchQuery { text: "how is the emoji system implemented", grep_pattern: "Emoji|emoji.*image|custom.*emoji|EmojiAlias", category: "mixed" },
        BenchQuery { text: "how does rate limiting interact with the API", grep_pattern: "RateLimit|rateLimiter|api.*throttle|limit.*per", category: "mixed" },
        BenchQuery { text: "what is the lifecycle of a slash command", grep_pattern: "SlashCommand|command.*execute|CommandArgs|openDialogURL", category: "mixed" },
        BenchQuery { text: "how does the search indexing pipeline work", grep_pattern: "SearchIndex|indexPost|FullText|searchEngine", category: "mixed" },
    ]
}

fn run_grep(pattern: &str, repo_root: &std::path::Path) -> Vec<String> {
    let output = Command::new("grep")
        .arg("-rE")
        .arg("--include=*.go")
        .arg("--include=*.ts")
        .arg("--include=*.tsx")
        .arg("--include=*.jsx")
        .arg("-l")
        .arg(pattern)
        .arg(repo_root)
        .output()
        .expect("Failed to run grep");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut files: HashSet<String> = HashSet::new();
    for line in stdout.lines() {
        if line.is_empty() { continue; }
        // -l flag: each line is a filename (no ':')
        files.insert(line.replace('\\', "/"));
    }
    let mut sorted: Vec<String> = files.into_iter().collect();
    sorted.sort();
    sorted
}

// ── Metrics ──────────────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct QueryResult {
    query: String,
    category: String,
    retrieval_ms: u64,
    grep_ms: u64,
    retrieval_files: Vec<String>,
    grep_files: Vec<String>,
    overlap: usize,
    retrieval_only: usize,
    grep_only: usize,
    /// First reciprocal rank of a grep-matching (basename) file — 0.0 when none.
    mrr: f64,
}

impl QueryResult {
    fn recall(&self) -> f64 { if self.grep_files.is_empty() { 1.0 } else { self.overlap as f64 / self.grep_files.len() as f64 } }
    fn precision(&self) -> f64 { if self.retrieval_files.is_empty() { 0.0 } else { self.overlap as f64 / self.retrieval_files.len() as f64 } }
    fn novelty(&self) -> f64 { if self.retrieval_files.is_empty() { 0.0 } else { self.retrieval_only as f64 / self.retrieval_files.len() as f64 } }
}

#[derive(Debug, Default)]
struct BenchResults { results: Vec<QueryResult>, total_duration_ms: u64 }

impl BenchResults {
    fn p50(&self, f: impl Fn(&QueryResult) -> u64) -> u64 { let mut s: Vec<u64> = self.results.iter().map(&f).collect(); s.sort(); s[s.len() / 2] }
    fn p95(&self, f: impl Fn(&QueryResult) -> u64) -> u64 { let mut s: Vec<u64> = self.results.iter().map(&f).collect(); s.sort(); s[(s.len() as f64 * 0.95) as usize] }
    fn avg(&self, f: impl Fn(&QueryResult) -> f64) -> f64 { if self.results.is_empty() { 0.0 } else { self.results.iter().map(&f).sum::<f64>() / self.results.len() as f64 } }
    fn by_cat(&self, f: impl Fn(&QueryResult) -> f64) -> Vec<(String, f64, usize)> {
        let mut c: HashMap<String, Vec<f64>> = HashMap::new();
        for r in &self.results { c.entry(r.category.clone()).or_default().push(f(r)); }
        let mut o: Vec<_> = c.into_iter().map(|(k, v)| (k, v.iter().sum::<f64>() / v.len() as f64, v.len())).collect();
        o.sort_by(|a, b| a.0.cmp(&b.0)); o
    }
}

// ── Runner ───────────────────────────────────────────────────────────────────

fn run_bench(repo_root: &std::path::Path) -> BenchResults {
    let queries = bench_queries();
    let mut results = Vec::with_capacity(queries.len());
    let db_path = home_dir().join(".knocode").join("data.db");
    let db = Database::open(&db_path).expect("Failed to open database");
    let event_bus = EventBus::new();
    let mut repo_intel = RepositoryIntelligence::new(repo_root.to_path_buf(), db, event_bus.clone());

    // Build index — without this, every query returns empty results.
    match repo_intel.index_repository() {
        Ok(stats) => {
            let mode = if stats.healed {
                "HEAL (forced re-extraction)"
            } else if stats.files_extracted > 0 {
                "COLD/INCREMENTAL"
            } else {
                "WARM (mtime shortcut)"
            };
            eprintln!(
                "Index built: {} files ({} shortcut, {} extracted, {} skipped), {} symbols, {}ms [{} mode] walk={}ms extract={}ms write={}ms",
                stats.files_indexed, stats.files_shortcut, stats.files_extracted, stats.files_skipped,
                stats.symbols_extracted, stats.duration_ms, mode,
                stats.walk_ms, stats.extract_ms, stats.write_ms
            );
        }
        Err(e) => {
            eprintln!("WARNING: index build failed: {} — queries may return empty results", e);
        }
    }

    // Result-set size knob for experiments (KNOCODE_BENCH_MAX_FILES, default 50).
    // A pinned value is an EXPLICIT pin (the repo-size auto-tune must not override it);
    // unset → default 50, and the bench run honestly reflects production defaults.
    let bench_max_files_env = std::env::var("KNOCODE_BENCH_MAX_FILES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok());
    let bench_max_files = bench_max_files_env.unwrap_or(50);
    let policy = RetrievalPolicy {
        candidate_k: 200,
        max_files: bench_max_files,
        max_files_explicit: bench_max_files_env.is_some(),
        ..Default::default()
    };
    let retriever = CombinedRetriever::default();
    let total_start = Instant::now();

    for q in &queries {
        let repository_id = repo_intel.repository_id().to_string();
        let query_obj = RetrievalQuery::new(q.text, &repository_id);

        let grep_start = Instant::now();
        let grep_files = run_grep(q.grep_pattern, repo_root);
        let grep_ms = grep_start.elapsed().as_millis() as u64;

        let ret_start = Instant::now();
        let retrieval = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            retriever.retrieve(&query_obj, &repo_intel, &policy)
        })).unwrap_or_else(|_| crate::retrieval::evidence::RetrievalResult::empty(
            knocode_core::RetrievalStatus::NoMatch));
        let retrieval_ms = ret_start.elapsed().as_millis() as u64;

        let retrieval_files: Vec<String> = retrieval.evidence.iter()
            .map(|ev| ev.path.to_string_lossy().to_string().replace('\\', "/"))
            .collect();

        let grep_basenames: HashSet<String> = grep_files.iter()
            .map(|f| std::path::Path::new(f).file_name().unwrap_or_default().to_string_lossy().to_string()).collect();
        let retrieval_basenames: HashSet<String> = retrieval_files.iter()
            .map(|f| std::path::Path::new(f).file_name().unwrap_or_default().to_string_lossy().to_string()).collect();

        let overlap = grep_basenames.iter().filter(|g| retrieval_basenames.contains(*g)).count();
        let retrieval_only = retrieval_basenames.iter().filter(|r| !grep_basenames.contains(*r)).count();
        let grep_only = grep_basenames.iter().filter(|g| !retrieval_basenames.contains(*g)).count();

        // MRR against basename matches (first reciprocal rank of a grep-hit file)
        let mrr = retrieval_files.iter()
            .map(|f| std::path::Path::new(f).file_name().unwrap_or_default().to_string_lossy().to_string())
            .enumerate()
            .find(|(_, bn)| grep_basenames.contains(bn))
            .map(|(rank, _)| 1.0 / (rank + 1) as f64)
            .unwrap_or(0.0);

        results.push(QueryResult {
            query: q.text.to_string(), category: q.category.to_string(),
            retrieval_ms, grep_ms, retrieval_files, grep_files, overlap, retrieval_only, grep_only, mrr,
        });
    }
    BenchResults { results, total_duration_ms: total_start.elapsed().as_millis() as u64 }
}

#[test]
#[ignore]
fn bench_mattermost_50() {
    let mm_root = PathBuf::from("C:/tmp/mattermost-master");
    if !mm_root.exists() {
        eprintln!("Mattermost not found at {:?} — skipping", mm_root);
        return;
    }

    eprintln!("Running benchmark on Mattermost (9k files)...");
    let r = run_bench(&mm_root);

    println!();
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!("  Mattermost Benchmark — 50 Hard Queries (9k Go + React files)");
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!();

    // Per-query table
    println!("┌────┬─────────────────────────────────────────────────┬────────┬──────┬──────┬──────┬──────┬──────┬───────┐");
    println!("│ #  │ Query                                           │ Cat    │RetMs │GrpMs │ Ovlp │RetOn │GrpOn │ MRR   │");
    println!("├────┼─────────────────────────────────────────────────┼────────┼──────┼──────┼──────┼──────┼──────┼───────┤");
    for (i, q) in r.results.iter().enumerate() {
        let qtext = if q.query.len() > 47 { format!("{}...", &q.query[..44]) } else { format!("{:<47}", q.query) };
        let mrr = q.mrr;
        let slow = if q.retrieval_ms > 1000 { " ⚠" } else { "" };
        println!("│ {:2} │ {} │ {:6} │ {:4} │ {:4} │ {:4} │ {:4} │ {:4} │ {:.3} {}│",
            i + 1, qtext, q.category, q.retrieval_ms, q.grep_ms, q.overlap, q.retrieval_only, q.grep_only, mrr, slow);
    }
    println!("└────┴─────────────────────────────────────────────────┴────────┴──────┴──────┴──────┴──────┴──────┴───────┘");
    println!();

    // Aggregate
    let tot_ov: usize = r.results.iter().map(|q| q.overlap).sum();
    let tot_ret: usize = r.results.iter().map(|q| q.retrieval_only).sum();
    let tot_grp: usize = r.results.iter().map(|q| q.grep_only).sum();

    println!("┌──────────────────────────────────────────────────────────────────────────┐");
    println!("│  Aggregate Metrics — Mattermost (9k Go + React files)                   │");
    println!("├─────────────────────────────┬────────────────────────────────────────────┤");
    println!("│  Total queries              │  {:<40}│", r.results.len());
    println!("│  Total wall time            │  {:<36} ms│", r.total_duration_ms);
    println!("├─ Retrieval Engine ──────────┼────────────────────────────────────────────┤");
    println!("│  Avg retrieval latency      │  {:<36} ms│", format!("{:.0}", r.avg(|q| q.retrieval_ms as f64)));
    println!("│  Retrieval p50              │  {:<36} ms│", r.p50(|q| q.retrieval_ms));
    println!("│  Retrieval p95              │  {:<36} ms│", r.p95(|q| q.retrieval_ms));
    println!("├─ Grep Baseline ─────────────┼────────────────────────────────────────────┤");
    println!("│  Avg grep latency           │  {:<36} ms│", format!("{:.0}", r.avg(|q| q.grep_ms as f64)));
    println!("│  Grep p50                   │  {:<36} ms│", r.p50(|q| q.grep_ms));
    println!("│  Grep p95                   │  {:<36} ms│", r.p95(|q| q.grep_ms));
    println!("├─ Quality ───────────────────┼────────────────────────────────────────────┤");
    println!("│  Avg recall                 │  {:<37}│", format!("{:.1}%", r.avg(|q| q.recall()) * 100.0));
    println!("│  Avg precision              │  {:<37}│", format!("{:.1}%", r.avg(|q| q.precision()) * 100.0));
    println!("│  Avg novelty                │  {:<37}│", format!("{:.1}%", r.avg(|q| q.novelty()) * 100.0));
    println!("│  Avg MRR (basename)         │  {:<37}│", format!("{:.4}", r.avg(|q| q.mrr)));
    println!("├─ Volume ────────────────────┼────────────────────────────────────────────┤");
    println!("│  Total overlap              │  {:<40}│", tot_ov);
    println!("│  Total retrieval-only       │  {:<40}│", tot_ret);
    println!("│  Total grep-only            │  {:<40}│", tot_grp);
    println!("└─────────────────────────────┴────────────────────────────────────────────┘");
    println!();

    // Per-category
    println!("┌──────────────────┬───────┬──────────┬──────────┬──────────┬──────────┬──────────┐");
    println!("│ Category         │ Count │ Recall%  │Precis%   │ Novelty% │ Ret ms   │ Grep ms  │");
    println!("├──────────────────┼───────┼──────────┼──────────┼──────────┼──────────┼──────────┤");
    for (cat, avg, cnt) in r.by_cat(|q| q.recall()) {
        let prec = r.by_cat(|q| q.precision()).iter().find(|(c, _, _)| *c == cat).map(|v| v.1).unwrap_or(0.0);
        let novel = r.by_cat(|q| q.novelty()).iter().find(|(c, _, _)| *c == cat).map(|v| v.1).unwrap_or(0.0);
        let ret_lat = r.by_cat(|q| q.retrieval_ms as f64).iter().find(|(c, _, _)| *c == cat).map(|v| v.1).unwrap_or(0.0);
        let grp_lat = r.by_cat(|q| q.grep_ms as f64).iter().find(|(c, _, _)| *c == cat).map(|v| v.1).unwrap_or(0.0);
        println!("│ {:<16} │ {:5} │ {:>7.1}%  │ {:>7.1}%  │ {:>7.1}%  │ {:>6.0}ms │ {:>6.0}ms │",
            cat, cnt, avg * 100.0, prec * 100.0, novel * 100.0, ret_lat, grp_lat);
    }
    println!("└──────────────────┴───────┴──────────┴──────────┴──────────┴──────────┴──────────┘");
    println!();

    // Speed comparison
    let total_ret_ms: u64 = r.results.iter().map(|q| q.retrieval_ms).sum();
    let total_grp_ms: u64 = r.results.iter().map(|q| q.grep_ms).sum();
    let speedup = if total_ret_ms > 0 { total_grp_ms as f64 / total_ret_ms as f64 } else { 0.0 };
    println!("⚡ Speed: retrieval {}ms total vs grep {}ms total → retrieval is {:.1}× faster",
        total_ret_ms, total_grp_ms, speedup);
    println!();

    // Slow queries
    let slow: Vec<_> = r.results.iter().filter(|q| q.retrieval_ms > 100).collect();
    if !slow.is_empty() {
        println!("⚠  Slow queries (>100ms):");
        for q in slow {
            println!("   {}ms  [{}] \"{}\"", q.retrieval_ms, q.category, q.query);
        }
    }

    // Novelty highlights
    let mut novelty_ranked: Vec<_> = r.results.iter().collect();
    novelty_ranked.sort_by(|a, b| b.retrieval_only.cmp(&a.retrieval_only));
    println!();
    println!("🧠 Retrieval finds what grep misses (top 5 by novelty):");
    for q in novelty_ranked.iter().take(5) {
        let top3: Vec<String> = q.retrieval_files.iter().take(3)
            .map(|f| std::path::Path::new(f).file_name().unwrap_or_default().to_string_lossy().to_string())
            .collect();
        println!("   novelty={} grep={} \"{}\"  → [\"{}\"]",
            q.retrieval_only, q.grep_files.len(), q.query, top3.join("\", \""));
    }
    println!();
    println!("═══════════════════════════════════════════════════════════════════════════════");
}
