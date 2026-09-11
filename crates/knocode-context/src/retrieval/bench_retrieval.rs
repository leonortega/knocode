//! Retrieval vs Grep Benchmark — 50 queries comparing retrieval engine against grep.
//!
//! Metrics: recall (grep finds → retrieval finds), novelty (retrieval finds → grep misses),
//! overlap, grep-only, latency comparison.
//!
//! Run with: `cargo test -p knocode-context -- --ignored bench_retrieval_50 --nocapture`

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

// ── Test Query Definition ────────────────────────────────────────────────────

#[derive(Debug, Clone)]
struct BenchQuery {
    text: &'static str,
    /// Grep pattern to run as baseline
    grep_pattern: &'static str,
    /// Intent category
    category: &'static str,
}

/// 50 test queries across 5 intent categories (10 each)
fn bench_queries() -> Vec<BenchQuery> {
    vec![
        // ── Procedural (10) ──
        BenchQuery { text: "how to add a new package", grep_pattern: "add.*package|new.*package|package.*add", category: "procedural" },
        BenchQuery { text: "how to load config", grep_pattern: "Config::load|load.*config|config.*load", category: "procedural" },
        BenchQuery { text: "steps to build the project from source", grep_pattern: "cargo build|build.*release|compile", category: "procedural" },
        BenchQuery { text: "how to run the tests", grep_pattern: "cargo test|test.*run|run.*test", category: "procedural" },
        BenchQuery { text: "how to install the daemon", grep_pattern: "install|daemon.*start|knocode serve", category: "procedural" },
        BenchQuery { text: "how to add a new skill", grep_pattern: "skill.*add|add.*skill|SKILL\\.md", category: "procedural" },
        BenchQuery { text: "how to enable LSP support", grep_pattern: "lsp|LSP|language.server", category: "procedural" },
        BenchQuery { text: "how to configure the model router", grep_pattern: "model.*router|router.*config|ModelRouter", category: "procedural" },
        BenchQuery { text: "how to start the watcher", grep_pattern: "watcher|file.*watch|FsWatcher", category: "procedural" },
        BenchQuery { text: "how to add custom tree-sitter grammars", grep_pattern: "tree.sitter.*grammar|grammar.*download|arborium", category: "procedural" },
        // ── Debugging (10) ──
        BenchQuery { text: "why does the system use fail-open design", grep_pattern: "fail.open|OriginalPassthrough|timeout.*30", category: "debugging" },
        BenchQuery { text: "what happens when the index is corrupted", grep_pattern: "corrupt|index.*error|tantivy.*error", category: "debugging" },
        BenchQuery { text: "why is tantivy indexing slow on large repos", grep_pattern: "tantivy.*slow|index.*perf|commit.*slow", category: "debugging" },
        BenchQuery { text: "what causes the database locked error", grep_pattern: "database.*locked|SQLITE_BUSY|db.*lock", category: "debugging" },
        BenchQuery { text: "why does the context engine return empty results", grep_pattern: "empty.*result|no.*match|ContextNotBuilt", category: "debugging" },
        BenchQuery { text: "what does the watcher miss when polling fails", grep_pattern: "watcher.*fail|poll.*error|notify.*error", category: "debugging" },
        BenchQuery { text: "why are some files not indexed", grep_pattern: "skip.*file|not.*index|file.*exclude", category: "debugging" },
        BenchQuery { text: "what causes the dependency graph to be empty", grep_pattern: "graph.*empty|no.*edge|DependencyGraph", category: "debugging" },
        BenchQuery { text: "why does skill matching fail silently", grep_pattern: "skill.*fail|skill.*match|skill.*skip", category: "debugging" },
        BenchQuery { text: "what happens when the daemon crashes", grep_pattern: "daemon.*crash|daemon.*panic|lifecycle", category: "debugging" },
        // ── Structural/Find (10) ──
        BenchQuery { text: "find all error types defined in the codebase", grep_pattern: "enum.*Error|thiserror|KnocodeError", category: "structural" },
        BenchQuery { text: "find all trait implementations for IContextBuilder", grep_pattern: "IContextBuilder|impl.*Context", category: "structural" },
        BenchQuery { text: "show me all the public APIs in knocode-core", grep_pattern: "pub fn|pub struct|pub enum|pub trait", category: "structural" },
        BenchQuery { text: "find all uses of the EventBus", grep_pattern: "EventBus|event_bus|RuntimeEvent", category: "structural" },
        BenchQuery { text: "find all database migration files", grep_pattern: "migration|migrate|CREATE TABLE", category: "structural" },
        BenchQuery { text: "show all configuration structs", grep_pattern: "struct.*Config|Config\\s*\\{|daemon.*config", category: "structural" },
        BenchQuery { text: "find all test files that test the retrieval engine", grep_pattern: "#\\[test\\]|mod tests|retrieval.*test", category: "structural" },
        BenchQuery { text: "find the token budget allocation logic", grep_pattern: "token.*budget|budget.*token|token_budget", category: "structural" },
        BenchQuery { text: "show all places where repository_id is used", grep_pattern: "repository_id|repo.*id", category: "structural" },
        BenchQuery { text: "find the file classification system", grep_pattern: "FileClass|classify_file|file_class", category: "structural" },
        // ── Informational (10) ──
        BenchQuery { text: "what is the architecture of the retrieval engine", grep_pattern: "retrieval.*engine|CombinedRetriever|Retriever.*trait", category: "informational" },
        BenchQuery { text: "what is the purpose of the knowledge hub", grep_pattern: "KnowledgeHub|knowledge.*hub|KnowledgeEntry", category: "informational" },
        BenchQuery { text: "what does the dependency graph track", grep_pattern: "DependencyGraph|graph.*edge|graph.*node", category: "informational" },
        BenchQuery { text: "what is the difference between docs_context and code_context", grep_pattern: "docs_context|code_context|is_documentation_path", category: "informational" },
        BenchQuery { text: "how the BM25 scoring works", grep_pattern: "BM25|bm25|file_class_boost|sanitize_code_query", category: "informational" },
        BenchQuery { text: "what is the token budget breakdown", grep_pattern: "skills_budget|docs_budget|code_budget|token_budget", category: "informational" },
        BenchQuery { text: "what skill matching strategy is used", grep_pattern: "skill.*match|match.*skill|SkillMatch|tag.*score", category: "informational" },
        BenchQuery { text: "what is the frozen prefix boundary for", grep_pattern: "FROZEN.*PREFIX|frozen.*boundary|FROZEN_BOUNDARY", category: "informational" },
        BenchQuery { text: "explain the incremental indexing strategy", grep_pattern: "incremental|mtime|is_file_unchanged|existing_hashes", category: "informational" },
        BenchQuery { text: "what metrics are exposed via Prometheus", grep_pattern: "prometheus|histogram|metrics.*expose|/metrics", category: "informational" },
        // ── Mixed/Ambiguous (10) ──
        BenchQuery { text: "where does the session fingerprint dedup happen", grep_pattern: "session.*fingerprint|dedup|fingerprint.*hash", category: "mixed" },
        BenchQuery { text: "what is the difference between v0.4 and v1", grep_pattern: "v0\\.4|v1.*minimal|V1_MINIMAL", category: "mixed" },
        BenchQuery { text: "how does the daemon expose context over MCP", grep_pattern: "POST /mcp|tools/call|knocode_context", category: "mixed" },
        BenchQuery { text: "which files are excluded from indexing", grep_pattern: "Binary|Vendor|Dependency|Generated|Stylesheet|skip.*file", category: "mixed" },
        BenchQuery { text: "what is the correlation ID used for", grep_pattern: "correlation_id|CorrelationId|req_id", category: "mixed" },
        BenchQuery { text: "how does the graph boost affect retrieval ranking", grep_pattern: "graph.*boost|apply_graph_boost|DependencyGraph", category: "mixed" },
        BenchQuery { text: "what happens during a warm re-index", grep_pattern: "warm.*re.?index|incremental|mtime.*size", category: "mixed" },
        BenchQuery { text: "where is the code-behind fallback implemented", grep_pattern: "code.behind|add_code_behind|CodeBehind", category: "mixed" },
        BenchQuery { text: "what is the vocabulary expansion for query terms", grep_pattern: "vocab|expand_terms|synonyms_for|vocabulary", category: "mixed" },
        BenchQuery { text: "how does the structural retriever complement BM25", grep_pattern: "structural.*retriev|ast.grep|StructuralRetriever", category: "mixed" },
    ]
}

// ── Grep Runner ──────────────────────────────────────────────────────────────

/// Run grep and return matching file paths (deduplicated)
fn run_grep(pattern: &str, repo_root: &std::path::Path) -> Vec<String> {
    let output = Command::new("grep")
        .args(["-rn", "--include=*.rs", "--include=*.md", "--include=*.toml", pattern])
        .current_dir(repo_root)
        .output()
        .expect("Failed to run grep");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let mut files: HashSet<String> = HashSet::new();
    for line in stdout.lines() {
        if let Some(pos) = line.find(':') {
            let file = &line[..pos];
            // Normalize path separators
            let normalized = file.replace('\\', "/");
            files.insert(normalized);
        }
    }
    let mut sorted: Vec<String> = files.into_iter().collect();
    sorted.sort();
    sorted
}

// ── Benchmark Metrics ────────────────────────────────────────────────────────

#[derive(Debug, Default)]
struct QueryResult {
    query: String,
    category: String,
    /// Retrieval latency
    retrieval_ms: u64,
    /// Grep latency
    grep_ms: u64,
    /// Files returned by retrieval engine
    retrieval_files: Vec<String>,
    /// Files returned by grep
    grep_files: Vec<String>,
    /// Files in both retrieval and grep (overlap)
    overlap: usize,
    /// Files in retrieval but NOT in grep (novelty — retrieval understands semantics)
    retrieval_only: usize,
    /// Files in grep but NOT in retrieval (grep-only — retrieval misses these)
    grep_only: usize,
}

impl QueryResult {
    /// recall: what fraction of grep results did retrieval find?
    fn recall(&self) -> f64 {
        if self.grep_files.is_empty() { 1.0 } else { self.overlap as f64 / self.grep_files.len() as f64 }
    }
    /// precision: what fraction of retrieval results are also in grep?
    fn precision(&self) -> f64 {
        if self.retrieval_files.is_empty() { 0.0 } else { self.overlap as f64 / self.retrieval_files.len() as f64 }
    }
    /// novelty: what fraction of retrieval results are unique (not in grep)?
    fn novelty(&self) -> f64 {
        if self.retrieval_files.is_empty() { 0.0 } else { self.retrieval_only as f64 / self.retrieval_files.len() as f64 }
    }
}

#[derive(Debug, Default)]
struct BenchResults {
    results: Vec<QueryResult>,
    total_duration_ms: u64,
}

impl BenchResults {
    fn latency_p50(&self, f: impl Fn(&QueryResult) -> u64) -> u64 {
        let mut sorted: Vec<u64> = self.results.iter().map(f).collect();
        sorted.sort();
        sorted[sorted.len() / 2]
    }
    fn latency_p95(&self, f: impl Fn(&QueryResult) -> u64) -> u64 {
        let mut sorted: Vec<u64> = self.results.iter().map(f).collect();
        sorted.sort();
        sorted[(sorted.len() as f64 * 0.95) as usize]
    }
    fn avg(&self, f: impl Fn(&QueryResult) -> f64) -> f64 {
        if self.results.is_empty() { 0.0 }
        else { self.results.iter().map(&f).sum::<f64>() / self.results.len() as f64 }
    }
    fn by_category(&self, f: impl Fn(&QueryResult) -> f64) -> Vec<(String, f64, usize)> {
        let mut cat: HashMap<String, Vec<f64>> = HashMap::new();
        for r in &self.results {
            cat.entry(r.category.clone()).or_default().push(f(r));
        }
        let mut out: Vec<_> = cat.into_iter().map(|(k, v)| {
            let avg = v.iter().sum::<f64>() / v.len() as f64;
            (k, avg, v.len())
        }).collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }
}

// ── Benchmark Runner ─────────────────────────────────────────────────────────

fn run_bench() -> BenchResults {
    let queries = bench_queries();
    let mut results = Vec::with_capacity(queries.len());
    let project_root = PathBuf::from("C:/LeonRepository/knocode");

    let db_path = home_dir().join(".knocode").join("data.db");
    let db = Database::open(&db_path).expect("Failed to open database");
    let event_bus = EventBus::new();
    let mut repo_intel = RepositoryIntelligence::new(project_root.clone(), db, event_bus.clone());

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

    let policy = RetrievalPolicy { candidate_k: 100, max_files: 20, ..Default::default() };
    let retriever = CombinedRetriever::default();

    let total_start = Instant::now();

    for q in &queries {
        let repository_id = repo_intel.repository_id().to_string();
        let query_obj = RetrievalQuery::new(q.text, &repository_id);

        // Run grep
        let grep_start = Instant::now();
        let grep_files = run_grep(q.grep_pattern, &project_root);
        let grep_ms = grep_start.elapsed().as_millis() as u64;

        // Run retrieval
        let ret_start = Instant::now();
        let retrieval = retriever.retrieve(&query_obj, &repo_intel, &policy);
        let retrieval_ms = ret_start.elapsed().as_millis() as u64;

        let retrieval_files: Vec<String> = retrieval.evidence.iter()
            .map(|ev| {
                let p = ev.path.to_string_lossy().to_string();
                p.replace('\\', "/")
            })
            .collect();

        // Compute overlap using substring matching on basenames
        let grep_basenames: HashSet<String> = grep_files.iter()
            .map(|f| std::path::Path::new(f).file_name().unwrap_or_default().to_string_lossy().to_string())
            .collect();
        let retrieval_basenames: HashSet<String> = retrieval_files.iter()
            .map(|f| std::path::Path::new(f).file_name().unwrap_or_default().to_string_lossy().to_string())
            .collect();

        let overlap = grep_basenames.iter().filter(|g| retrieval_basenames.contains(*g)).count();
        let retrieval_only = retrieval_basenames.iter().filter(|r| !grep_basenames.contains(*r)).count();
        let grep_only = grep_basenames.iter().filter(|g| !retrieval_basenames.contains(*g)).count();

        results.push(QueryResult {
            query: q.text.to_string(),
            category: q.category.to_string(),
            retrieval_ms,
            grep_ms,
            retrieval_files,
            grep_files,
            overlap,
            retrieval_only,
            grep_only,
        });
    }

    BenchResults { results, total_duration_ms: total_start.elapsed().as_millis() as u64 }
}

// ── Test Entry Point ─────────────────────────────────────────────────────────

#[test]
#[ignore]
fn bench_retrieval_50() {
    let r = run_bench();

    println!();
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!("  Retrieval vs Grep Benchmark — 50 Queries");
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!();

    // Per-query table
    println!("┌────┬─────────────────────────────────────────────────┬────────┬──────┬──────┬──────┬──────┬──────┬───────┐");
    println!("│ #  │ Query                                           │ Cat    │RetMs │GrpMs │ Ovlp │RetOn │GrpOn │ MRR   │");
    println!("├────┼─────────────────────────────────────────────────┼────────┼──────┼──────┼──────┼──────┼──────┼───────┤");
    for (i, q) in r.results.iter().enumerate() {
        let qtext = if q.query.len() > 47 { format!("{}...", &q.query[..44]) } else { format!("{:<47}", q.query) };
        // MRR: 1/rank of first overlap in retrieval
        let mrr = {
            let grep_basenames: HashSet<String> = q.grep_files.iter()
                .map(|f| std::path::Path::new(f).file_name().unwrap_or_default().to_string_lossy().to_string())
                .collect();
            let mut m = 0.0;
            for (rank, rf) in q.retrieval_files.iter().enumerate() {
                let bn = std::path::Path::new(rf).file_name().unwrap_or_default().to_string_lossy().to_string();
                if grep_basenames.contains(&bn) { m = 1.0 / (rank + 1) as f64; break; }
            }
            m
        };
        println!("│ {:2} │ {} │ {:6} │ {:4} │ {:4} │ {:4} │ {:4} │ {:4} │ {:.3}  │",
            i + 1, qtext, q.category, q.retrieval_ms, q.grep_ms,
            q.overlap, q.retrieval_only, q.grep_only, mrr);
    }
    println!("└────┴─────────────────────────────────────────────────┴────────┴──────┴──────┴──────┴──────┴──────┴───────┘");
    println!();

    // ── Aggregate ──
    let total_overlap: usize = r.results.iter().map(|q| q.overlap).sum();
    let total_retrieval_only: usize = r.results.iter().map(|q| q.retrieval_only).sum();
    let total_grep_only: usize = r.results.iter().map(|q| q.grep_only).sum();

    println!("┌──────────────────────────────────────────────────────────────────────────┐");
    println!("│  Aggregate Metrics                                                       │");
    println!("├─────────────────────────────┬────────────────────────────────────────────┤");
    println!("│  Total queries              │  {:<40}│", r.results.len());
    println!("│  Total wall time            │  {:<36} ms│", r.total_duration_ms);
    println!("├─ Retrieval Engine ──────────┼────────────────────────────────────────────┤");
    println!("│  Avg retrieval latency      │  {:<36} ms│", format!("{:.0}", r.avg(|q| q.retrieval_ms as f64)));
    println!("│  Retrieval p50              │  {:<36} ms│", r.latency_p50(|q| q.retrieval_ms));
    println!("│  Retrieval p95              │  {:<36} ms│", r.latency_p95(|q| q.retrieval_ms));
    println!("├─ Grep Baseline ─────────────┼────────────────────────────────────────────┤");
    println!("│  Avg grep latency           │  {:<36} ms│", format!("{:.0}", r.avg(|q| q.grep_ms as f64)));
    println!("│  Grep p50                   │  {:<36} ms│", r.latency_p50(|q| q.grep_ms));
    println!("│  Grep p95                   │  {:<36} ms│", r.latency_p95(|q| q.grep_ms));
    println!("├─ Quality ───────────────────┼────────────────────────────────────────────┤");
    println!("│  Avg recall (ret ∩ grep / grep)  │  {:<32}│", format!("{:.1}%", r.avg(|q| q.recall()) * 100.0));
    println!("│  Avg precision (ret ∩ grep / ret) │  {:<32}│", format!("{:.1}%", r.avg(|q| q.precision()) * 100.0));
    println!("│  Avg novelty (ret − grep) / ret   │  {:<32}│", format!("{:.1}%", r.avg(|q| q.novelty()) * 100.0));
    println!("├─ Volume ────────────────────┼────────────────────────────────────────────┤");
    println!("│  Total overlap              │  {:<40}│", total_overlap);
    println!("│  Total retrieval-only       │  {:<40}│", total_retrieval_only);
    println!("│  Total grep-only            │  {:<40}│", total_grep_only);
    println!("└─────────────────────────────┴────────────────────────────────────────────┘");
    println!();

    // ── Per-category ──
    println!("┌──────────────────┬───────┬──────────┬──────────┬──────────┬──────────┬──────────┐");
    println!("│ Category         │ Count │ Recall%  │Precis%   │ Novelty% │ Ret ms   │ Grep ms  │");
    println!("├──────────────────┼───────┼──────────┼──────────┼──────────┼──────────┼──────────┤");
    for (cat, avg, cnt) in r.by_category(|q| q.recall()) {
        let prec = r.by_category(|q| q.precision()).iter().find(|(c, _, _)| *c == cat).map(|v| v.1).unwrap_or(0.0);
        let novel = r.by_category(|q| q.novelty()).iter().find(|(c, _, _)| *c == cat).map(|v| v.1).unwrap_or(0.0);
        let ret_lat = r.by_category(|q| q.retrieval_ms as f64).iter().find(|(c, _, _)| *c == cat).map(|v| v.1).unwrap_or(0.0);
        let grp_lat = r.by_category(|q| q.grep_ms as f64).iter().find(|(c, _, _)| *c == cat).map(|v| v.1).unwrap_or(0.0);
        println!("│ {:<16} │ {:5} │ {:>7.1}%  │ {:>7.1}%  │ {:>7.1}%  │ {:>6.0}ms │ {:>6.0}ms │",
            cat, cnt, avg * 100.0, prec * 100.0, novel * 100.0, ret_lat, grp_lat);
    }
    println!("└──────────────────┴───────┴──────────┴──────────┴──────────┴──────────┴──────────┘");
    println!();

    // ── Speed comparison ──
    let ret_total: u64 = r.results.iter().map(|q| q.retrieval_ms).sum();
    let grp_total: u64 = r.results.iter().map(|q| q.grep_ms).sum();
    let speedup = if ret_total > 0 { grp_total as f64 / ret_total as f64 } else { 0.0 };
    println!("⚡ Speed: retrieval {}ms total vs grep {}ms total → retrieval is {:.1}× {}",
        ret_total, grp_total,
        if speedup > 1.0 { speedup } else { 1.0 / speedup.max(0.01) },
        if speedup > 1.0 { "faster" } else { "slower" });
    println!();

    // ── High-novelty queries (retrieval finds what grep misses) ──
    let mut by_novelty: Vec<&QueryResult> = r.results.iter().filter(|q| q.retrieval_only > 0).collect();
    by_novelty.sort_by(|a, b| b.retrieval_only.cmp(&a.retrieval_only));
    if !by_novelty.is_empty() {
        println!("🧠 Retrieval finds what grep misses (top 5 by novelty):");
        for q in by_novelty.iter().take(5) {
            let ret_only_names: Vec<&str> = q.retrieval_files.iter()
                .map(|f| f.rsplit(['/', '\\']).next().unwrap_or(f))
                .filter(|bn| !q.grep_files.iter().any(|gf| gf.ends_with(bn)))
                .take(3)
                .collect();
            println!("   novelty={}  grep=0  \"{}\"  → {:?}", q.retrieval_only, q.query, ret_only_names);
        }
        println!();
    }

    // ── High-grep-only (retrieval misses what grep finds) ──
    let mut by_grep_only: Vec<&QueryResult> = r.results.iter().filter(|q| q.grep_only > 0 && q.overlap == 0).collect();
    by_grep_only.sort_by(|a, b| b.grep_only.cmp(&a.grep_only));
    if !by_grep_only.is_empty() {
        println!("🔍 Grep finds what retrieval misses (zero-overlap queries):");
        for q in by_grep_only.iter().take(5) {
            println!("   grep_only={}  \"{}\"", q.grep_only, q.query);
        }
        println!();
    }

    println!("═══════════════════════════════════════════════════════════════════════════════");

    // Assertions
    assert!(r.avg(|q| q.recall()) >= 0.2, "Avg recall {:.1}% < 20%", r.avg(|q| q.recall()) * 100.0);
    assert!(r.avg(|q| q.novelty()) >= 0.1, "Avg novelty {:.1}% < 10%", r.avg(|q| q.novelty()) * 100.0);
    assert!(r.latency_p95(|q| q.retrieval_ms) < 100, "Retrieval p95 {}ms > 100ms", r.latency_p95(|q| q.retrieval_ms));
}
