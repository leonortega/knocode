//! DefinitelyTyped Benchmark — 50 hard queries on 53k-file TypeScript type definitions repo.
//!
//! Run with: `cargo test -p knocode-context -- --ignored bench_dt_50 --nocapture`

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

/// 50 hard queries designed for DefinitelyTyped — a 53k-file TypeScript type definitions repo.
/// Queries test: exact type lookup, cross-package relationships, API surface understanding,
/// generic type patterns, and ambiguous/intent-heavy queries.
fn bench_queries() -> Vec<BenchQuery> {
    vec![
        // ── Procedural (10) — "how to" type patterns ──
        BenchQuery { text: "how to type a React functional component with children", grep_pattern: "PropsWithChildren|ReactNode.*children|FC.*children", category: "procedural" },
        BenchQuery { text: "how to type an Express middleware that modifies the request", grep_pattern: "Request.*Props|augment.*Request|declare.*namespace.*Express", category: "procedural" },
        BenchQuery { text: "how to create a type-safe event emitter", grep_pattern: "EventEmitter|TypedEvent|EventMap|on\\(.*string", category: "procedural" },
        BenchQuery { text: "how to type a reducer with discriminated union actions", grep_pattern: "Reducer|Action.*type|discriminated|Action.*payload", category: "procedural" },
        BenchQuery { text: "how to type a database query builder", grep_pattern: "QueryBuilder|Chainable|\\.where\\(|\\.select\\(", category: "procedural" },
        BenchQuery { text: "how to type a configuration object with optional nested fields", grep_pattern: "DeepPartial|RecursivePartial|Options.*\\{|Config.*\\{", category: "procedural" },
        BenchQuery { text: "how to type a plugin system with registration", grep_pattern: "Plugin|register|use\\(|middleware.*use", category: "procedural" },
        BenchQuery { text: "how to type a Redux store with middleware", grep_pattern: "Store|Middleware|Dispatch|Enhancer", category: "procedural" },
        BenchQuery { text: "how to type a GraphQL resolver with context", grep_pattern: "Resolver|Context.*type|IResolver|ResolveFn", category: "procedural" },
        BenchQuery { text: "how to type a WebSocket message protocol", grep_pattern: "WebSocket.*Message|WS.*Data|Socket.*Send|Message.*type", category: "procedural" },
        // ── Debugging (10) — "why" and "what went wrong" ──
        BenchQuery { text: "why does the React hooks type inference fail with generic components", grep_pattern: "useHooks|Hook.*Generic|Generic.*Component.*hook", category: "debugging" },
        BenchQuery { text: "what is the correct type for a Node.js ReadableStream in browser vs server", grep_pattern: "ReadableStream|Readable.*Web|NodeJS.*Readable|stream\\.Readable", category: "debugging" },
        BenchQuery { text: "why is the Mongoose document type not matching the schema", grep_pattern: "Document.*Schema|Model.*Document|InferSchemaType|SchemaType", category: "debugging" },
        BenchQuery { text: "what type should I use for a function that returns Promise or value", grep_pattern: "PromiseOrValue|Awaitable|MaybePromise|T \\| Promise", category: "debugging" },
        BenchQuery { text: "why does TypeScript complain about this conditional type", grep_pattern: "Conditional.*Type|infer |extends.*\\?|分布式条件", category: "debugging" },
        BenchQuery { text: "what is the correct type for a React ref callback", grep_pattern: "RefCallback|RefObject|React\\.ref|forwardRef", category: "debugging" },
        BenchQuery { text: "why is the Express response type missing json method", grep_pattern: "Response.*json|res\\.json|Json.*method|send.*json", category: "debugging" },
        BenchQuery { text: "what type should I use for a JWT that may be expired", grep_pattern: "JWT|jwt.*expir|token.*verify|JwtPayload", category: "debugging" },
        BenchQuery { text: "why does the generic constraint prevent this assignment", grep_pattern: "extends.*Error|constraint.*fail|generic.*assign|not assignable", category: "debugging" },
        BenchQuery { text: "what is the correct type for a callback that receives an error or result", grep_pattern: "Callback.*Error|ErrorFirst|node.*callback|Err.*Result", category: "debugging" },
        // ── Structural/Find (10) — locate specific type patterns ──
        BenchQuery { text: "find all type definitions that extend Error", grep_pattern: "extends Error|class.*Error.*{|Error.*class", category: "structural" },
        BenchQuery { text: "find all interface definitions with index signatures", grep_pattern: "\\[key.*string\\]|\\[key.*number\\]|\\[index.*\\]|Record<", category: "structural" },
        BenchQuery { text: "find all type definitions using template literal types", grep_pattern: "Template.*Literal|`\\$\\{|\\`.*\\$\\{", category: "structural" },
        BenchQuery { text: "find all generic type definitions with multiple type parameters", grep_pattern: "<[A-Z],\\s*[A-Z]>|<T,\\s*U>|<K,\\s*V>", category: "structural" },
        BenchQuery { text: "find all React component type definitions with defaultProps", grep_pattern: "defaultProps|DefaultProps|static.*default", category: "structural" },
        BenchQuery { text: "find all Express route handler type definitions", grep_pattern: "RouteHandler|RequestHandler|Handler.*Request|router\\.", category: "structural" },
        BenchQuery { text: "find all database model type definitions", grep_pattern: "interface.*Model|type.*Model|Model<|Schema.*type", category: "structural" },
        BenchQuery { text: "find all configuration type definitions with nested objects", grep_pattern: "interface.*Config|Config.*\\{|Options.*\\{|Settings.*\\{", category: "structural" },
        BenchQuery { text: "find all enum definitions with string values", grep_pattern: "enum.*=.*\"|enum.*string|const enum", category: "structural" },
        BenchQuery { text: "find all utility type definitions (Partial, Pick, Omit)", grep_pattern: "type.*Partial|type.*Pick|type.*Omit|type.*Record", category: "structural" },
        // ── Informational (10) — "what is" and "how does" ──
        BenchQuery { text: "what is the type definition for the React useState hook", grep_pattern: "useState|UseState|StateHook|SetState", category: "informational" },
        BenchQuery { text: "how is the Express application type structured", grep_pattern: "Express.*Application|Application.*type|express\\.Application", category: "informational" },
        BenchQuery { text: "what types does the Node.js fs module expose", grep_pattern: "fs\\.|Filesystem|ReadFile|WriteFile|StatResult", category: "informational" },
        BenchQuery { text: "how is the Axios response type structured", grep_pattern: "AxiosResponse|Response.*data|AxiosError|AxiosInstance", category: "informational" },
        BenchQuery { text: "what types does Socket.IO expose for events", grep_pattern: "Socket.*Event|Server.*Event|io\\(|Socket\\.IO", category: "informational" },
        BenchQuery { text: "how is the Next.js page component typed", grep_pattern: "NextPage|GetServerSideProps|PageProps|NextComponent", category: "informational" },
        BenchQuery { text: "what types does the Jest test framework provide", grep_pattern: "jest|Describe|It.*fn|Expect.* matcher|Mock.*fn", category: "informational" },
        BenchQuery { text: "how is the MongoDB collection type structured", grep_pattern: "Collection.*type|MongoDB.*Collection|Db.*collection|Aggregate", category: "informational" },
        BenchQuery { text: "what types does the webpack configuration use", grep_pattern: "webpack.*Config|Module.*Rule|Plugin.*type|Loader", category: "informational" },
        BenchQuery { text: "how is the Electron IPC type system structured", grep_pattern: "ipcRenderer|ipcMain|Electron.*Event|IpcRenderer", category: "informational" },
        // ── Mixed/Ambiguous (10) — hard queries requiring semantic understanding ──
        BenchQuery { text: "where is the type definition for a cancelable promise", grep_pattern: "Cancelable|Abort.*Promise|Cancel.*token|AbortController", category: "mixed" },
        BenchQuery { text: "what type should I use for a deeply nested object path", grep_pattern: "DeepPath|PathValue|Get\\.|NestedKey|PropertyPath", category: "mixed" },
        BenchQuery { text: "find the type definition for a retry mechanism with backoff", grep_pattern: "retry|backoff|exponential|RetryOptions", category: "mixed" },
        BenchQuery { text: "what is the type for a React context provider with default value", grep_pattern: "createContext|Provider.*value|Context.*default|useContext", category: "mixed" },
        BenchQuery { text: "how to type a function that accepts either a string or object", grep_pattern: "string \\| object|StringOr|string.*\\|.*\\{|Overload", category: "mixed" },
        BenchQuery { text: "find the type for a rate limiter configuration", grep_pattern: "rate.*limit|throttle|RateLimit|tokens.*bucket", category: "mixed" },
        BenchQuery { text: "what type should I use for a lazy-loaded component", grep_pattern: "Lazy|Suspense|lazy\\(|React\\.lazy", category: "mixed" },
        BenchQuery { text: "find the type definition for a dependency injection container", grep_pattern: "Container|inject|Inject|IoC|Dependency.*inject", category: "mixed" },
        BenchQuery { text: "what is the type for a serialized/deserialized object", grep_pattern: "Serializable|Serialize|Deserialize|JSON.*type|FromJSON", category: "mixed" },
        BenchQuery { text: "find the type for a connection pool with health checks", grep_pattern: "Pool|health.*check|Connection.*pool|Pool.*options", category: "mixed" },
    ]
}

// ── Grep Runner ──────────────────────────────────────────────────────────────

fn run_grep(pattern: &str, repo_root: &std::path::Path) -> Vec<String> {
    let output = Command::new("grep")
        .args(["-rEn", "--include=*.d.ts", "--include=*.ts", "--include=*.tsx", pattern])
        .current_dir(repo_root)
        .output()
        .expect("Failed to run grep");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut files: HashSet<String> = HashSet::new();
    for line in stdout.lines() {
        if let Some(pos) = line.find(':') {
            let file = &line[..pos];
            files.insert(file.replace('\\', "/"));
        }
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
            eprintln!("Index built: {} files indexed, {} symbols extracted, {}ms",
                stats.files_indexed, stats.symbols_extracted, stats.duration_ms);
        }
        Err(e) => {
            eprintln!("WARNING: index build failed: {} — queries may return empty results", e);
        }
    }

    // Result-set size knob for experiments (KNOCODE_BENCH_MAX_FILES, default 50 = historical baseline).
    // A pinned value is an EXPLICIT pin (the repo-size auto-tune must not override it);
    // unset → default 50, and the bench run honestly reflects production defaults.
    let bench_max_files_env = std::env::var("KNOCODE_BENCH_MAX_FILES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok());
    let policy = RetrievalPolicy {
        candidate_k: 200,
        max_files: bench_max_files_env.unwrap_or(50),
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

        results.push(QueryResult {
            query: q.text.to_string(), category: q.category.to_string(),
            retrieval_ms, grep_ms, retrieval_files, grep_files, overlap, retrieval_only, grep_only,
        });
    }
    BenchResults { results, total_duration_ms: total_start.elapsed().as_millis() as u64 }
}

#[test]
#[ignore]
fn bench_dt_50() {
    let dt_root = PathBuf::from("C:/tmp/DefinitelyTyped-master");
    if !dt_root.exists() {
        eprintln!("DefinitelyTyped not found at {:?} — skipping", dt_root);
        return;
    }

    eprintln!("Running benchmark on DefinitelyTyped (53k files)...");
    let r = run_bench(&dt_root);

    println!();
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!("  DefinitelyTyped Benchmark — 50 Hard Queries (53k .d.ts files)");
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!();

    // Per-query table
    println!("┌────┬─────────────────────────────────────────────────┬────────┬──────┬──────┬──────┬──────┬──────┬───────┐");
    println!("│ #  │ Query                                           │ Cat    │RetMs │GrpMs │ Ovlp │RetOn │GrpOn │ MRR   │");
    println!("├────┼─────────────────────────────────────────────────┼────────┼──────┼──────┼──────┼──────┼──────┼───────┤");
    for (i, q) in r.results.iter().enumerate() {
        let qtext = if q.query.len() > 47 { format!("{}...", &q.query[..44]) } else { format!("{:<47}", q.query) };
        let mrr = {
            let gb: HashSet<String> = q.grep_files.iter().map(|f| std::path::Path::new(f).file_name().unwrap_or_default().to_string_lossy().to_string()).collect();
            let mut m = 0.0;
            for (rank, rf) in q.retrieval_files.iter().enumerate() {
                let bn = std::path::Path::new(rf).file_name().unwrap_or_default().to_string_lossy().to_string();
                if gb.contains(&bn) { m = 1.0 / (rank + 1) as f64; break; }
            }
            m
        };
        // Flag slow queries
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
    println!("│  Aggregate Metrics — DefinitelyTyped (53k files)                         │");
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

    // Speed
    let ret_total: u64 = r.results.iter().map(|q| q.retrieval_ms).sum();
    let grp_total: u64 = r.results.iter().map(|q| q.grep_ms).sum();
    let speedup = if ret_total > 0 { grp_total as f64 / ret_total as f64 } else { 0.0 };
    println!("⚡ Speed: retrieval {}ms total vs grep {}ms total → retrieval is {:.1}× {}",
        ret_total, grp_total, if speedup > 1.0 { speedup } else { 1.0 / speedup.max(0.01) },
        if speedup > 1.0 { "faster" } else { "slower" });
    println!();

    // Slowest queries
    let mut by_lat: Vec<&QueryResult> = r.results.iter().filter(|q| q.retrieval_ms > 100).collect();
    by_lat.sort_by(|a, b| b.retrieval_ms.cmp(&a.retrieval_ms));
    if !by_lat.is_empty() {
        println!("⚠  Slow queries (>100ms):");
        for q in by_lat.iter().take(5) {
            println!("   {}ms  [{}] \"{}\"", q.retrieval_ms, q.category, q.query);
        }
        println!();
    }

    // High-novelty (retrieval finds what grep misses)
    let mut by_novel: Vec<&QueryResult> = r.results.iter().filter(|q| q.retrieval_only > 10).collect();
    by_novel.sort_by(|a, b| b.retrieval_only.cmp(&a.retrieval_only));
    if !by_novel.is_empty() {
        println!("🧠 Retrieval finds what grep misses (top 5 by novelty):");
        for q in by_novel.iter().take(5) {
            let sample: Vec<&str> = q.retrieval_files.iter()
                .map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f))
                .filter(|bn| !q.grep_files.iter().any(|gf| gf.ends_with(bn)))
                .take(3).collect();
            println!("   novelty={}  grep=0  \"{}\"  → {:?}", q.retrieval_only, q.query, sample);
        }
        println!();
    }

    println!("═══════════════════════════════════════════════════════════════════════════════");
}
