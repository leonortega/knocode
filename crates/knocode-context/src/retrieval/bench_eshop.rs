//! eShopOnWeb Benchmark — 50 hard queries on 225-file ASP.NET/C# e-commerce app.
//!
//! Run with: `cargo test --release -p knocode-context -- --ignored bench_eshop_50 --nocapture`

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

/// 50 hard queries designed for eShopOnWeb — a 225-file ASP.NET/C# e-commerce app.
/// Tests: cross-layer understanding (Domain ↔ Infrastructure ↔ Web), C# idioms,
/// DDD patterns, authentication, basket/checkout flow, catalog browsing, Blazor admin.
fn bench_queries() -> Vec<BenchQuery> {
    vec![
        // ── Procedural (10) — "how to" patterns for C#/.NET ──
        BenchQuery { text: "how to add a new catalog item", grep_pattern: "CatalogItem|AddCatalogItem|CreateCatalogItem", category: "procedural" },
        BenchQuery { text: "how to add a new API endpoint", grep_pattern: "MapGet|MapPost|MapPut|MapDelete|EndpointRouteBuilder", category: "procedural" },
        BenchQuery { text: "how to add a new database migration", grep_pattern: "Migration|Up\\(|Down\\(|MigrationBuilder", category: "procedural" },
        BenchQuery { text: "how to add a new Blazor admin page", grep_pattern: "@page|@inject|ComponentBase|Blazor", category: "procedural" },
        BenchQuery { text: "how to configure dependency injection", grep_pattern: "AddScoped|AddSingleton|AddTransient|services\\.Add", category: "procedural" },
        BenchQuery { text: "how to add a new specification", grep_pattern: "Specification|ISpecification|And\\(|Or\\(|Take\\(", category: "procedural" },
        BenchQuery { text: "how to add a new exception type", grep_pattern: "Exception|: Exception|throw new", category: "procedural" },
        BenchQuery { text: "how to add health check endpoint", grep_pattern: "HealthCheck|MapHealthChecks|IHealthCheck|AddHealthChecks", category: "procedural" },
        BenchQuery { text: "how to add a new order status", grep_pattern: "OrderStatus|status.*Order|enum.*Status", category: "procedural" },
        BenchQuery { text: "how to add a new payment method", grep_pattern: "PaymentMethod|Payment|Checkout|Buyer", category: "procedural" },

        // ── Debugging (10) — "why" patterns ──
        BenchQuery { text: "why does the basket merge fail on login", grep_pattern: "MergeBasket|BasketMerge|anonymous.*basket|logged.*in", category: "debugging" },
        BenchQuery { text: "why is the catalog item price not updating", grep_pattern: "Price|CatalogItem.*price|UpdatePrice|SetPrice", category: "debugging" },
        BenchQuery { text: "why does the order fail to save", grep_pattern: "SaveChanges|Order.*save|CreateOrder|IOrderService", category: "debugging" },
        BenchQuery { text: "why is the identity token not being issued", grep_pattern: "Token|JWT|SignInManager|IdentityToken|TokenClaimsService", category: "debugging" },
        BenchQuery { text: "why does the Blazor admin page not load", grep_pattern: "OnInitialized|OnParametersSet|ComponentBase|blazor.*error", category: "debugging" },
        BenchQuery { text: "why is the catalog search returning wrong results", grep_pattern: "SearchCatalog|FilterItems|CatalogFilter|specification", category: "debugging" },
        BenchQuery { text: "why does the checkout process timeout", grep_pattern: "Checkout|OrderService|CreateOrderAsync|timeout", category: "debugging" },
        BenchQuery { text: "why is the image URL not resolving", grep_pattern: "UriComposer|ImageUrl|ImagePlaceholder|catalog.*image", category: "debugging" },
        BenchQuery { text: "why does the admin authentication redirect loop", grep_pattern: "Authorize|AllowAnonymous|SignIn|Redirect|Identity.*auth", category: "debugging" },
        BenchQuery { text: "why is the EF Core query generating N+1", grep_pattern: "Include|ThenInclude|AsNoTracking|Eager|Lazy.*load", category: "debugging" },

        // ── Structural (10) — find patterns ──
        BenchQuery { text: "find all domain entities", grep_pattern: "class.*: BaseEntity|IAggregateRoot|Entity|Aggregate", category: "structural" },
        BenchQuery { text: "find all repository interfaces", grep_pattern: "interface.*Repository|IRepository|IReadRepository", category: "structural" },
        BenchQuery { text: "find all service implementations", grep_pattern: "class.*Service|: I.*Service|ServiceBase", category: "structural" },
        BenchQuery { text: "find all controller actions", grep_pattern: "HttpGet|HttpPost|HttpPut|HttpDelete|ApiController", category: "structural" },
        BenchQuery { text: "find all Blazor components", grep_pattern: "@page|@component|ComponentBase|inherits.*Component", category: "structural" },
        BenchQuery { text: "find all specifications", grep_pattern: "class.*Specification|: Specification|ISpecification", category: "structural" },
        BenchQuery { text: "find all exception types", grep_pattern: "class.*Exception|: Exception|ExceptionBase", category: "structural" },
        BenchQuery { text: "find all database configurations", grep_pattern: "EntityTypeConfiguration|OnModelCreating|modelBuilder|HasData", category: "structural" },
        BenchQuery { text: "find all authorization policies", grep_pattern: "Authorize|Policy|AuthorizationConstants|Claims|Roles", category: "structural" },
        BenchQuery { text: "find all dependency injection registrations", grep_pattern: "AddScoped|AddSingleton|AddTransient|services\\.Add|builder\\.Services", category: "structural" },

        // ── Informational (10) — "what is" patterns ──
        BenchQuery { text: "what is the overall architecture", grep_pattern: "Clean Architecture|Onion|DDD|Aggregate|Entity|ValueObject", category: "informational" },
        BenchQuery { text: "what is the basket checkout flow", grep_pattern: "Basket|Checkout|Order|Transfer.*basket|MergeBasket", category: "informational" },
        BenchQuery { text: "what is the catalog browsing system", grep_pattern: "CatalogItem|CatalogBrand|CatalogType|ListItems|Filter", category: "informational" },
        BenchQuery { text: "what is the authentication mechanism", grep_pattern: "Identity|SignIn|Token|Cookie|Authorize|AllowAnonymous", category: "informational" },
        BenchQuery { text: "what is the order management system", grep_pattern: "Order|OrderItem|OrderService|IOrderService|OrderStatus", category: "informational" },
        BenchQuery { text: "what is the admin panel structure", grep_pattern: "BlazorAdmin|Pages.*Admin|CatalogItemPage|Admin.*Service", category: "informational" },
        BenchQuery { text: "what is the API endpoint structure", grep_pattern: "PublicApi|Endpoint|MapGet|MapPost|MinimalApi", category: "informational" },
        BenchQuery { text: "what is the caching strategy", grep_pattern: "Cache|IMemoryCache|DistributedCache|OutputCache|ResponseCache", category: "informational" },
        BenchQuery { text: "what is the logging approach", grep_pattern: "ILogger|LogInformation|LogWarning|LogError|Serilog", category: "informational" },
        BenchQuery { text: "what is the deployment configuration", grep_pattern: "docker|Dockerfile|docker-compose|appsettings|launchSettings", category: "informational" },

        // ── Mixed (10) — complex intent ──
        BenchQuery { text: "how does the basket-to-order conversion work end to end", grep_pattern: "Basket.*Order|TransferToOrder|CreateOrder|OrderService", category: "mixed" },
        BenchQuery { text: "find where the price is calculated and displayed", grep_pattern: "Price|CalculatePrice|GetPrice|UnitPrice|atalogItem.*Price", category: "mixed" },
        BenchQuery { text: "how is the catalog filtered by brand and type", grep_pattern: "CatalogFilter|FilterByBrand|FilterByType|BrandId|TypeId", category: "mixed" },
        BenchQuery { text: "what happens when a user logs in with items in anonymous basket", grep_pattern: "MergeBasket|anonymous|LoggedIn|TransferBasket|Cookie", category: "mixed" },
        BenchQuery { text: "how does the admin edit a catalog item", grep_pattern: "EditItem|UpdateItem|CatalogItem.*Edit|SaveChanges|AdminService", category: "mixed" },
        BenchQuery { text: "find the complete checkout pipeline from basket to order", grep_pattern: "Checkout|PlaceOrder|CreateOrder|OrderService|BasketService", category: "mixed" },
        BenchQuery { text: "how is the catalog image URL composed", grep_pattern: "UriComposer|ImageUrl|ComposeUri|catalog.*image|ImagePlaceholder", category: "mixed" },
        BenchQuery { text: "what authorization is required for admin operations", grep_pattern: "Authorize|Policy|Admin|Roles|Claims|AuthorizeAttribute", category: "mixed" },
        BenchQuery { text: "how does the specification pattern filter catalog items", grep_pattern: "Specification|And\\(|Or\\(|Take\\(|FilterSpecification", category: "mixed" },
        BenchQuery { text: "trace the data flow from HTTP request to database query", grep_pattern: "DbContext|OnModelCreating|DbSet|SaveChanges|Repository", category: "mixed" },
    ]
}

fn run_grep(pattern: &str, repo_root: &std::path::Path) -> Vec<String> {
    let output = Command::new("grep")
        .args(["-rEn", "--include=*.cs", "--include=*.razor", "--include=*.json", "--include=*.cshtml", pattern])
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

    let policy = RetrievalPolicy { candidate_k: 200, max_files: 50, max_files_explicit: true, ..Default::default() };
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
fn bench_eshop_50() {
    let eshop_root = PathBuf::from("C:/tmp/eShopOnWeb");
    if !eshop_root.exists() {
        eprintln!("eShopOnWeb not found at {:?} — skipping", eshop_root);
        return;
    }

    eprintln!("Running benchmark on eShopOnWeb (225 C# files)...");
    let r = run_bench(&eshop_root);

    println!();
    println!("═══════════════════════════════════════════════════════════════════════════════");
    println!("  eShopOnWeb Benchmark — 50 Hard Queries (225 C# files)");
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
        let slow = if q.retrieval_ms > 100 { " ⚠" } else { "" };
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
    println!("│  Aggregate Metrics — eShopOnWeb (225 C# files)                           │");
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
    let mut by_novel: Vec<&QueryResult> = r.results.iter().filter(|q| q.retrieval_only > 3).collect();
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

    // High-grep-only (retrieval misses what grep finds)
    let mut by_grep_only: Vec<&QueryResult> = r.results.iter().filter(|q| q.grep_only > 3 && q.overlap == 0).collect();
    by_grep_only.sort_by(|a, b| b.grep_only.cmp(&a.grep_only));
    if !by_grep_only.is_empty() {
        println!("🔍 Grep finds what retrieval misses (zero-overlap queries):");
        for q in by_grep_only.iter().take(5) {
            println!("   grep_only={}  \"{}\"", q.grep_only, q.query);
        }
        println!();
    }

    println!("═══════════════════════════════════════════════════════════════════════════════");
}
