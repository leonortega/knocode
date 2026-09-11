//! Component Evaluation Benchmark — measures impact of graph boost, reranker, and query expansion.
//!
//! Run with: `cargo test -p knocode-context -- --ignored bench_components --nocapture`

use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

use crate::retrieval::policy::RetrievalPolicy;
use crate::retrieval::query::RetrievalQuery;
use crate::retrieval::{CombinedRetriever, Retriever};
use knocode_repo_intel::RepositoryIntelligence;
use knocode_events::EventBus;
use knocode_storage::Database;

fn home_dir() -> PathBuf {
    std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(PathBuf::from))
        .unwrap_or_else(|_| PathBuf::from("."))
}

/// Result-set size knob for experiments (KNOCODE_BENCH_MAX_FILES, default 50 = historical baseline).
fn bench_max_files_value() -> usize {
    std::env::var("KNOCODE_BENCH_MAX_FILES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok())
        .unwrap_or(50)
}

/// True when KNOCODE_BENCH_MAX_FILES is set — i.e. the bench run is an EXPLICIT
/// pin that the repo-size auto-tune must not override. Unset → default run,
/// which honestly reflects production defaults (auto-tune applies).
fn bench_max_files_is_explicit() -> bool {
    std::env::var("KNOCODE_BENCH_MAX_FILES").is_ok()
}

#[derive(Debug, Clone)]
struct EvalQuery {
    text: &'static str,
    category: &'static str,
}

/// 20 queries for component evaluation — covers all intent types
fn eval_queries() -> Vec<EvalQuery> {
    vec![
        // Procedural
        EvalQuery { text: "how to add a new package", category: "procedural" },
        EvalQuery { text: "how to configure the build system", category: "procedural" },
        EvalQuery { text: "how to run tests", category: "procedural" },
        EvalQuery { text: "how to deploy to production", category: "procedural" },
        EvalQuery { text: "how to add error handling", category: "procedural" },
        // Debugging
        EvalQuery { text: "why does the auth middleware fail", category: "debugging" },
        EvalQuery { text: "why is the config not loading", category: "debugging" },
        EvalQuery { text: "why does the database connection timeout", category: "debugging" },
        EvalQuery { text: "why is the test failing", category: "debugging" },
        EvalQuery { text: "why is the build slow", category: "debugging" },
        // Structural
        EvalQuery { text: "find all error types", category: "structural" },
        EvalQuery { text: "find all config files", category: "structural" },
        EvalQuery { text: "find all test files", category: "structural" },
        EvalQuery { text: "find all API endpoints", category: "structural" },
        EvalQuery { text: "find all database models", category: "structural" },
        // Informational
        EvalQuery { text: "what is the architecture", category: "informational" },
        EvalQuery { text: "what dependencies does this use", category: "informational" },
        EvalQuery { text: "what is the testing strategy", category: "informational" },
        EvalQuery { text: "what is the deployment process", category: "informational" },
        EvalQuery { text: "what is the error handling strategy", category: "informational" },
    ]
}

// ── Metrics ──────────────────────────────────────────────────────────────────

#[derive(Debug, Default, Clone)]
struct ComponentResult {
    query: String,
    category: String,
    // Baseline (no component)
    baseline_ms: u64,
    baseline_files: Vec<String>,
    // With component
    with_ms: u64,
    with_files: Vec<String>,
    // Comparison
    files_added: usize,
    files_removed: usize,
    files_kept: usize,
}

impl ComponentResult {
    fn recall_change(&self) -> f64 {
        // Positive = improved recall
        let baseline_set: HashSet<&String> = self.baseline_files.iter().collect();
        let with_set: HashSet<&String> = self.with_files.iter().collect();
        let added = with_set.iter().filter(|f| !baseline_set.contains(*f)).count();
        let removed = baseline_set.iter().filter(|f| !with_set.contains(*f)).count();
        (added as f64 - removed as f64) / self.baseline_files.len().max(1) as f64
    }
}

// ── Component Evaluators ─────────────────────────────────────────────────────

/// Evaluate graph boost impact
fn eval_graph(
    repo_intel: &RepositoryIntelligence,
    retriever: &CombinedRetriever,
) -> Vec<ComponentResult> {
    let queries = eval_queries();
    let mut results = Vec::with_capacity(queries.len());

    // Result-set size knob for experiments (KNOCODE_BENCH_MAX_FILES, default 50 = historical baseline).
    // A pinned value is an EXPLICIT pin (the repo-size auto-tune must not override it);
    // unset → default 50, and the bench run honestly reflects production defaults.
    let bench_max_files_env = std::env::var("KNOCODE_BENCH_MAX_FILES")
        .ok()
        .and_then(|v| v.parse::<usize>().ok());
    let bench_max_files = bench_max_files_env.unwrap_or(50);
    let bench_max_files_explicit = bench_max_files_env.is_some();

    // Policy without graph
    let policy_no_graph = RetrievalPolicy {
        candidate_k: 200,
        max_files: bench_max_files,
        max_files_explicit: bench_max_files_explicit,
        enable_graph: false,
        ..Default::default()
    };

    // Policy with graph (forced)
    let policy_with_graph = RetrievalPolicy {
        candidate_k: 200,
        max_files: bench_max_files,
        max_files_explicit: bench_max_files_explicit,
        enable_graph: true,
        ..Default::default()
    };

    // Warm up the dependency-graph cache (mem + disk) before timing.
    // The first-ever build walks the whole repo (~hundreds of ms one-time
    // cost); without warmup it lands on query 1 and smears the mean.
    if let Err(e) = repo_intel.build_dependency_graph() {
        eprintln!("WARNING: graph warmup failed: {} — with-graph timings include cold build", e);
    }

    for q in &queries {
        let repository_id = repo_intel.repository_id().to_string();
        let query_obj = RetrievalQuery::new(q.text, &repository_id);

        // Baseline (no graph)
        let baseline_start = Instant::now();
        let baseline = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            retriever.retrieve(&query_obj, repo_intel, &policy_no_graph)
        }))
        .unwrap_or_else(|_| {
            crate::retrieval::evidence::RetrievalResult::empty(knocode_core::RetrievalStatus::NoMatch)
        });
        let baseline_ms = baseline_start.elapsed().as_millis() as u64;

        // With graph
        let with_start = Instant::now();
        let with = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            retriever.retrieve(&query_obj, repo_intel, &policy_with_graph)
        }))
        .unwrap_or_else(|_| {
            crate::retrieval::evidence::RetrievalResult::empty(knocode_core::RetrievalStatus::NoMatch)
        });
        let with_ms = with_start.elapsed().as_millis() as u64;

        let baseline_files: Vec<String> = baseline
            .evidence
            .iter()
            .map(|ev| ev.path.to_string_lossy().to_string().replace('\\', "/"))
            .collect();
        let with_files: Vec<String> = with
            .evidence
            .iter()
            .map(|ev| ev.path.to_string_lossy().to_string().replace('\\', "/"))
            .collect();

        let baseline_set: HashSet<&String> = baseline_files.iter().collect();
        let with_set: HashSet<&String> = with_files.iter().collect();

        let files_added = with_set.iter().filter(|f| !baseline_set.contains(*f)).count();
        let files_removed = baseline_set.iter().filter(|f| !with_set.contains(*f)).count();
        let files_kept = with_set.iter().filter(|f| baseline_set.contains(*f)).count();

        results.push(ComponentResult {
            query: q.text.to_string(),
            category: q.category.to_string(),
            baseline_ms,
            baseline_files,
            with_ms,
            with_files,
            files_added,
            files_removed,
            files_kept,
        });
    }

    results
}

/// Evaluate candidate_k impact (how many candidates to consider before ranking)
///
/// NOTE on interpreting ~0% deltas: this metric diffs the truncated top-50
/// file SETS (50 vs 500), not recall against golden relevance. Since the
/// doc-prior damping + docs-slot reservation stabilize the top-50, both pool
/// sizes can resolve to the identical set on small repos — that reads as
/// "+0.0% NEUTRAL" but means "same good 50", not "pool size is worthless".
/// A large pool still matters for structural-exhaustive queries (500 vs 500
/// by construction there) and for recall measured against grep/golden sets.
fn eval_candidate_k(
    repo_intel: &RepositoryIntelligence,
    retriever: &CombinedRetriever,
) -> Vec<ComponentResult> {
    let queries = eval_queries();
    let mut results = Vec::with_capacity(queries.len());

    // Policy with small candidate pool
    let policy_small = RetrievalPolicy {
        candidate_k: 50,
        max_files: bench_max_files_value(),
        max_files_explicit: bench_max_files_is_explicit(),
        ..Default::default()
    };

    // Policy with large candidate pool
    let policy_large = RetrievalPolicy {
        candidate_k: 500,
        max_files: bench_max_files_value(),
        max_files_explicit: bench_max_files_is_explicit(),
        ..Default::default()
    };

    for q in &queries {
        let repository_id = repo_intel.repository_id().to_string();
        let query_obj = RetrievalQuery::new(q.text, &repository_id);

        // Baseline (small pool)
        let baseline_start = Instant::now();
        let baseline = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            retriever.retrieve(&query_obj, repo_intel, &policy_small)
        }))
        .unwrap_or_else(|_| {
            crate::retrieval::evidence::RetrievalResult::empty(knocode_core::RetrievalStatus::NoMatch)
        });
        let baseline_ms = baseline_start.elapsed().as_millis() as u64;

        // With large pool
        let with_start = Instant::now();
        let with = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            retriever.retrieve(&query_obj, repo_intel, &policy_large)
        }))
        .unwrap_or_else(|_| {
            crate::retrieval::evidence::RetrievalResult::empty(knocode_core::RetrievalStatus::NoMatch)
        });
        let with_ms = with_start.elapsed().as_millis() as u64;

        let baseline_files: Vec<String> = baseline
            .evidence
            .iter()
            .map(|ev| ev.path.to_string_lossy().to_string().replace('\\', "/"))
            .collect();
        let with_files: Vec<String> = with
            .evidence
            .iter()
            .map(|ev| ev.path.to_string_lossy().to_string().replace('\\', "/"))
            .collect();

        let baseline_set: HashSet<&String> = baseline_files.iter().collect();
        let with_set: HashSet<&String> = with_files.iter().collect();

        let files_added = with_set.iter().filter(|f| !baseline_set.contains(*f)).count();
        let files_removed = baseline_set.iter().filter(|f| !with_set.contains(*f)).count();
        let files_kept = with_set.iter().filter(|f| baseline_set.contains(*f)).count();

        results.push(ComponentResult {
            query: q.text.to_string(),
            category: q.category.to_string(),
            baseline_ms,
            baseline_files,
            with_ms,
            with_files,
            files_added,
            files_removed,
            files_kept,
        });
    }

    results
}

/// Evaluate query expansion impact
fn eval_expansion(
    repo_intel: &RepositoryIntelligence,
    retriever: &CombinedRetriever,
) -> Vec<ComponentResult> {
    let queries = eval_queries();
    let mut results = Vec::with_capacity(queries.len());

    let policy = RetrievalPolicy {
        candidate_k: 200,
        max_files: bench_max_files_value(),
        max_files_explicit: bench_max_files_is_explicit(),
        ..Default::default()
    };

    for q in &queries {
        let repository_id = repo_intel.repository_id().to_string();

        // Baseline (original query)
        let baseline_query = RetrievalQuery::new(q.text, &repository_id);
        let baseline_start = Instant::now();
        let baseline = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            retriever.retrieve(&baseline_query, repo_intel, &policy)
        }))
        .unwrap_or_else(|_| {
            crate::retrieval::evidence::RetrievalResult::empty(knocode_core::RetrievalStatus::NoMatch)
        });
        let baseline_ms = baseline_start.elapsed().as_millis() as u64;

        // With expansion (manually expanded query)
        let expanded = expand_query(q.text);
        let expanded_query = RetrievalQuery::new(&expanded, &repository_id);
        let with_start = Instant::now();
        let with = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            retriever.retrieve(&expanded_query, repo_intel, &policy)
        }))
        .unwrap_or_else(|_| {
            crate::retrieval::evidence::RetrievalResult::empty(knocode_core::RetrievalStatus::NoMatch)
        });
        let with_ms = with_start.elapsed().as_millis() as u64;

        let baseline_files: Vec<String> = baseline
            .evidence
            .iter()
            .map(|ev| ev.path.to_string_lossy().to_string().replace('\\', "/"))
            .collect();
        let with_files: Vec<String> = with
            .evidence
            .iter()
            .map(|ev| ev.path.to_string_lossy().to_string().replace('\\', "/"))
            .collect();

        let baseline_set: HashSet<&String> = baseline_files.iter().collect();
        let with_set: HashSet<&String> = with_files.iter().collect();

        let files_added = with_set.iter().filter(|f| !baseline_set.contains(*f)).count();
        let files_removed = baseline_set.iter().filter(|f| !with_set.contains(*f)).count();
        let files_kept = with_set.iter().filter(|f| baseline_set.contains(*f)).count();

        results.push(ComponentResult {
            query: q.text.to_string(),
            category: q.category.to_string(),
            baseline_ms,
            baseline_files,
            with_ms,
            with_files,
            files_added,
            files_removed,
            files_kept,
        });
    }

    results
}

/// Simple query expansion for evaluation
fn expand_query(query: &str) -> String {
    let mut expanded = query.to_string();
    // Add synonyms for common terms
    if query.contains("how to") {
        expanded.push_str(" guide tutorial example");
    }
    if query.contains("error") {
        expanded.push_str(" exception fault failure");
    }
    if query.contains("config") {
        expanded.push_str(" configuration settings options");
    }
    if query.contains("test") {
        expanded.push_str(" spec unit integration");
    }
    if query.contains("deploy") {
        expanded.push_str(" release publish production");
    }
    expanded
}

// ── Display ──────────────────────────────────────────────────────────────────

fn print_eval_results(component: &str, results: &[ComponentResult]) {
    println!();
    println!(
        "═══════════════════════════════════════════════════════════════════════════════"
    );
    println!("  {} Evaluation — 20 Queries", component);
    println!(
        "═══════════════════════════════════════════════════════════════════════════════"
    );
    println!();

    // Per-query table
    println!("┌────┬─────────────────────────────────────────────────┬────────┬──────┬──────┬──────┬──────┬──────┐");
    println!("│ #  │ Query                                           │ Cat    │BaseMs│WithMs│ Added│Remov │ Kept │");
    println!("├────┼─────────────────────────────────────────────────┼────────┼──────┼──────┼──────┼──────┼──────┤");
    for (i, r) in results.iter().enumerate() {
        let qtext = if r.query.len() > 47 {
            format!("{}...", &r.query[..44])
        } else {
            format!("{:<47}", r.query)
        };
        println!(
            "│ {:2} │ {} │ {:6} │ {:4} │ {:4} │ {:4} │ {:4} │ {:4} │",
            i + 1,
            qtext,
            r.category,
            r.baseline_ms,
            r.with_ms,
            r.files_added,
            r.files_removed,
            r.files_kept
        );
    }
    println!("└────┴─────────────────────────────────────────────────┴────────┴──────┴──────┴──────┴──────┴──────┘");
    println!();

    // Aggregate
    let total_added: usize = results.iter().map(|r| r.files_added).sum();
    let total_removed: usize = results.iter().map(|r| r.files_removed).sum();
    let total_kept: usize = results.iter().map(|r| r.files_kept).sum();
    let avg_baseline_ms: f64 =
        results.iter().map(|r| r.baseline_ms as f64).sum::<f64>() / results.len() as f64;
    let avg_with_ms: f64 =
        results.iter().map(|r| r.with_ms as f64).sum::<f64>() / results.len() as f64;
    let avg_recall_change: f64 =
        results.iter().map(|r| r.recall_change()).sum::<f64>() / results.len() as f64;

    println!("┌──────────────────────────────────────────────────────────────────────────┐");
    println!("│  Aggregate — {} Evaluation                        │", component);
    println!("├─────────────────────────────┬────────────────────────────────────────────┤");
    println!(
        "│  Total queries              │  {:<40}│",
        results.len()
    );
    println!(
        "│  Avg baseline latency       │  {:<36} ms│",
        format!("{:.0}", avg_baseline_ms)
    );
    println!(
        "│  Avg with-component latency │  {:<36} ms│",
        format!("{:.0}", avg_with_ms)
    );
    println!(
        "│  Latency overhead           │  {:<36} ms│",
        format!("{:.0}", avg_with_ms - avg_baseline_ms)
    );
    println!("├─────────────────────────────┼────────────────────────────────────────────┤");
    println!(
        "│  Files added (improved)     │  {:<40}│",
        total_added
    );
    println!(
        "│  Files removed (worse)      │  {:<40}│",
        total_removed
    );
    println!(
        "│  Files kept (unchanged)     │  {:<40}│",
        total_kept
    );
    println!(
        "│  Avg recall change          │  {:<37}│",
        format!("{:+.1}%", avg_recall_change * 100.0)
    );
    println!("└─────────────────────────────┴────────────────────────────────────────────┘");
}

// ── Test ─────────────────────────────────────────────────────────────────────

#[test]
#[ignore]
fn bench_components() {
    let knocode_root = PathBuf::from("C:/LeonRepository/knocode");
    if !knocode_root.exists() {
        eprintln!("knocode repo not found at {:?} — skipping", knocode_root);
        return;
    }

    eprintln!("Running component evaluation on knocode repo...");
    let db_path = home_dir().join(".knocode").join("data.db");
    let db = Database::open(&db_path).expect("Failed to open database");
    let event_bus = EventBus::new();
    let mut repo_intel =
        RepositoryIntelligence::new(knocode_root.to_path_buf(), db, event_bus.clone());

    // Ensure index is built — without this, validate_index returns "index not built"
    // and every query returns empty results (the bug that caused all-zeros in v1).
    match repo_intel.index_repository() {
        Ok(stats) => {
            eprintln!("Index built: {} files indexed, {} symbols extracted, {}ms",
                stats.files_indexed, stats.symbols_extracted, stats.duration_ms);
        }
        Err(e) => {
            eprintln!("WARNING: index build failed: {} — queries may return empty results", e);
        }
    }

    let retriever = CombinedRetriever::default();

    // 1. Graph boost evaluation
    eprintln!("Evaluating graph boost...");
    let graph_results = eval_graph(&repo_intel, &retriever);
    print_eval_results("Graph Boost", &graph_results);

    // 2. Candidate_k evaluation (replaces reranker — reranker is passthrough in v1)
    eprintln!("Evaluating candidate_k...");
    let candidate_k_results = eval_candidate_k(&repo_intel, &retriever);
    print_eval_results("Candidate K", &candidate_k_results);

    // 3. Query expansion evaluation
    eprintln!("Evaluating query expansion...");
    let expansion_results = eval_expansion(&repo_intel, &retriever);
    print_eval_results("Query Expansion", &expansion_results);

    // Summary
    println!();
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!("  Component Impact Summary");
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!();
    println!(
        "┌──────────────────┬────────────┬────────────┬────────────┬────────────────┐"
    );
    println!(
        "│ Component        │ Latency Δ  │ Files Δ    │ Recall Δ   │ Recommendation │"
    );
    println!(
        "├──────────────────┼────────────┼────────────┼────────────┼────────────────┤"
    );

    let graph_avg_ms: f64 = graph_results.iter().map(|r| r.with_ms as f64 - r.baseline_ms as f64).sum::<f64>() / graph_results.len() as f64;
    let graph_files_delta: i64 = graph_results.iter().map(|r| r.files_added as i64 - r.files_removed as i64).sum::<i64>();
    let graph_recall_delta: f64 = graph_results.iter().map(|r| r.recall_change()).sum::<f64>() / graph_results.len() as f64;
    let graph_rec = if graph_recall_delta > 0.05 { "✅ USE" } else if graph_recall_delta < -0.05 { "❌ SKIP" } else { "⚠️ NEUTRAL" };

    let ck_avg_ms: f64 = candidate_k_results.iter().map(|r| r.with_ms as f64 - r.baseline_ms as f64).sum::<f64>() / candidate_k_results.len() as f64;
    let ck_files_delta: i64 = candidate_k_results.iter().map(|r| r.files_added as i64 - r.files_removed as i64).sum::<i64>();
    let ck_recall_delta: f64 = candidate_k_results.iter().map(|r| r.recall_change()).sum::<f64>() / candidate_k_results.len() as f64;
    let ck_rec = if ck_recall_delta > 0.05 { "✅ USE" } else if ck_recall_delta < -0.05 { "❌ SKIP" } else { "⚠️ NEUTRAL" };

    let exp_avg_ms: f64 = expansion_results.iter().map(|r| r.with_ms as f64 - r.baseline_ms as f64).sum::<f64>() / expansion_results.len() as f64;
    let exp_files_delta: i64 = expansion_results.iter().map(|r| r.files_added as i64 - r.files_removed as i64).sum::<i64>();
    let exp_recall_delta: f64 = expansion_results.iter().map(|r| r.recall_change()).sum::<f64>() / expansion_results.len() as f64;
    let exp_rec = if exp_recall_delta > 0.05 { "✅ USE" } else if exp_recall_delta < -0.05 { "❌ SKIP" } else { "⚠️ NEUTRAL" };

    println!(
        "│ {:<16} │ {:>+8.0} ms │ {:>+8}    │ {:>+7.1}%   │ {:<14} │",
        "Graph Boost", graph_avg_ms, graph_files_delta, graph_recall_delta * 100.0, graph_rec
    );
    println!(
        "│ {:<16} │ {:>+8.0} ms │ {:>+8}    │ {:>+7.1}%   │ {:<14} │",
        "Candidate K", ck_avg_ms, ck_files_delta, ck_recall_delta * 100.0, ck_rec
    );
    println!(
        "│ {:<16} │ {:>+8.0} ms │ {:>+8}    │ {:>+7.1}%   │ {:<14} │",
        "Query Expansion", exp_avg_ms, exp_files_delta, exp_recall_delta * 100.0, exp_rec
    );
    println!(
        "└──────────────────┴────────────┴────────────┴────────────┴────────────────┘"
    );
}
