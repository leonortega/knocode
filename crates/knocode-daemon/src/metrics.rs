use std::sync::OnceLock;
use std::time::Instant;

/// Lightweight metrics — no `prometheus` crate dependency for v0.4.0
/// Exposes `GET /metrics` as Prometheus exposition format.
/// Keeps p95 histogram in-memory without external crate; swap to `prometheus` crate later.
use std::sync::{Arc, Mutex};
use std::collections::HashMap;

#[derive(Default)]
struct Histogram {
    buckets: Vec<(f64, usize)>, // (upper_bound, count)
    sum: f64,
    count: usize,
}

impl Histogram {
    fn new(bounds: Vec<f64>) -> Self {
        Self { buckets: bounds.into_iter().map(|b| (b, 0)).collect(), sum: 0.0, count: 0 }
    }
    fn observe(&mut self, v: f64) {
        self.count += 1;
        self.sum += v;
        for (bound, cnt) in &mut self.buckets {
            if v <= *bound { *cnt += 1; }
        }
    }
    fn exposition(&self, name: &str, help: &str) -> String {
        let mut out = format!("# HELP {} {}\n# TYPE {} histogram\n", name, help, name);
        for (bound, cnt) in &self.buckets {
            out.push_str(&format!("{}{{le=\"{}\"}} {}\n", name, bound, cnt));
        }
        out.push_str(&format!("{}{{le=\"+Inf\"}} {}\n", name, self.count));
        out.push_str(&format!("{}_sum {}\n", name, self.sum));
        out.push_str(&format!("{}_count {}\n", name, self.count));
        out
    }
}

/// Daemon readiness for clients that poll before sending requests.
/// `Indexing` while the initial index (or an auto-reindex) is running,
/// `Ready` once requests can be served without waiting on the engine lock.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Readiness {
    #[default]
    Indexing,
    Ready,
}

// TASK-022: metrics should answer How fast? How many tokens saved? How much context? Which model? How often fail-open? How good is retrieval?
#[derive(Default)]
pub struct Metrics {
    requests_total: Mutex<HashMap<String, usize>>,
    build_duration: Mutex<Histogram>,
    fail_open_total: Mutex<usize>,
    index_files: Mutex<usize>,
    context_tokens: Mutex<Histogram>,
    context_files: Mutex<Histogram>,
    retrieval_duration: Mutex<Histogram>,
    retrieval_candidates: Mutex<Histogram>,
    retrieval_recall: Mutex<f64>,
    readiness: Mutex<Readiness>,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            requests_total: Mutex::new(HashMap::new()),
            build_duration: Mutex::new(Histogram::new(vec![0.01, 0.05, 0.1, 0.5, 1.0, 5.0, 30.0])),
            fail_open_total: Mutex::new(0),
            index_files: Mutex::new(0),
            context_tokens: Mutex::new(Histogram::new(vec![100.0, 1000.0, 5000.0, 12000.0, 30000.0])),
            context_files: Mutex::new(Histogram::new(vec![1.0, 3.0, 5.0, 10.0, 20.0, 50.0])),
            retrieval_duration: Mutex::new(Histogram::new(vec![0.001, 0.005, 0.01, 0.05, 0.1, 0.5, 1.0])),
            retrieval_candidates: Mutex::new(Histogram::new(vec![0.0, 5.0, 10.0, 25.0, 50.0, 100.0, 500.0])),
            retrieval_recall: Mutex::new(0.0),
            readiness: Mutex::new(Readiness::Indexing),
        }
    }

    pub fn inc_requests(&self, hook_type: &str) {
        if let Ok(mut m) = self.requests_total.lock() { *m.entry(hook_type.to_string()).or_insert(0) += 1; }
    }

    pub fn observe_build_duration(&self, secs: f64) {
        if let Ok(mut h) = self.build_duration.lock() { h.observe(secs); }
    }

    pub fn inc_fail_open(&self) {
        if let Ok(mut c) = self.fail_open_total.lock() { *c += 1; }
    }

    pub fn set_index_files(&self, n: usize) {
        if let Ok(mut g) = self.index_files.lock() { *g = n; }
    }

    // TASK-022: context size + retrieval quality (token savings moved to RTK)
    pub fn observe_context_tokens(&self, n: usize) {
        if let Ok(mut h) = self.context_tokens.lock() { h.observe(n as f64); }
    }
    /// Files included in the context pack per request (retrieval breadth).
    pub fn observe_context_files(&self, n: usize) {
        if let Ok(mut h) = self.context_files.lock() { h.observe(n as f64); }
    }
    /// Retrieval-stage latency (search only, excluding context packing).
    pub fn observe_retrieval_duration(&self, secs: f64) {
        if let Ok(mut h) = self.retrieval_duration.lock() { h.observe(secs); }
    }
    /// Candidate results the retrieval pipeline returned before packing.
    pub fn observe_retrieval_candidates(&self, n: usize) {
        if let Ok(mut h) = self.retrieval_candidates.lock() { h.observe(n as f64); }
    }
    pub fn set_retrieval_recall(&self, recall: f64) {
        if let Ok(mut r) = self.retrieval_recall.lock() { *r = recall; }
    }

    /// Daemon readiness — `Indexing` while the initial index / an auto-reindex runs,
    /// `Ready` once requests can be served without waiting on the engine lock.
    pub fn set_readiness(&self, readiness: Readiness) {
        if let Ok(mut r) = self.readiness.lock() { *r = readiness; }
    }

    pub fn readiness(&self) -> Readiness {
        self.readiness.lock().map(|r| *r).unwrap_or(Readiness::Indexing)
    }

    pub fn is_ready(&self) -> bool {
        self.readiness() == Readiness::Ready
    }

    /// Stable wire string for the probe payloads (`/health`, UDS `Probe`) — "indexing" | "ready".
    pub fn readiness_str(&self) -> &'static str {
        match self.readiness() {
            Readiness::Ready => "ready",
            Readiness::Indexing => "indexing",
        }
    }

    pub fn index_files(&self) -> usize {
        self.index_files.lock().map(|g| *g).unwrap_or(0)
    }

    pub fn exposition(&self) -> String {
        let mut out = String::new();
        out.push_str("# HELP knocode_requests_total Total requests by hook\n# TYPE knocode_requests_total counter\n");
        if let Ok(m) = self.requests_total.lock() {
            for (k, v) in m.iter() {
                out.push_str(&format!("knocode_requests_total{{key=\"{}\"}} {}\n", k, v));
            }
        }
        if let Ok(h) = self.build_duration.lock() {
            out.push_str(&h.exposition("knocode_build_context_duration_seconds", "BuildContext duration"));
        }
        if let Ok(c) = self.fail_open_total.lock() {
            out.push_str(&format!("# HELP knocode_fail_open_total Fail-open count\n# TYPE knocode_fail_open_total counter\nknocode_fail_open_total {}\n", *c));
        }
        if let Ok(g) = self.index_files.lock() {
            out.push_str(&format!("# HELP knocode_index_files Indexed files\n# TYPE knocode_index_files gauge\nknocode_index_files {}\n", *g));
        }
        {
            let ready = self.is_ready();
            out.push_str(&format!("# HELP knocode_daemon_ready Daemon readiness (1 = ready, 0 = indexing)\n# TYPE knocode_daemon_ready gauge\nknocode_daemon_ready {}\n", if ready { 1 } else { 0 }));
        }
        if let Ok(h) = self.context_tokens.lock() {
            out.push_str(&h.exposition("knocode_context_tokens", "Context tokens per request"));
        }
        if let Ok(h) = self.context_files.lock() {
            out.push_str(&h.exposition("knocode_context_files", "Files included in the context pack per request"));
        }
        if let Ok(h) = self.retrieval_duration.lock() {
            out.push_str(&h.exposition("knocode_retrieval_duration_seconds", "Retrieval-stage duration (search only, excluding packing)"));
        }
        if let Ok(h) = self.retrieval_candidates.lock() {
            out.push_str(&h.exposition("knocode_retrieval_candidates", "Candidate results returned by retrieval before packing"));
        }
        if let Ok(r) = self.retrieval_recall.lock() {
            out.push_str(&format!("# HELP knocode_retrieval_recall Retrieval recall@5\n# TYPE knocode_retrieval_recall gauge\nknocode_retrieval_recall {}\n", *r));
        }
        out
    }
}

static GLOBAL: OnceLock<Arc<Metrics>> = OnceLock::new();

pub fn global() -> Arc<Metrics> {
    GLOBAL.get_or_init(|| Arc::new(Metrics::new())).clone()
}

/// RAII timer for BuildContext
pub struct Timer { start: Instant, metrics: Arc<Metrics> }
impl Timer {
    pub fn start() -> Self { Self { start: Instant::now(), metrics: global() } }
}
impl Drop for Timer {
    fn drop(&mut self) {
        let secs = self.start.elapsed().as_secs_f64();
        self.metrics.observe_build_duration(secs);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_metrics_exposition() {
        let m = Metrics::new();
        m.inc_requests("PreGeneration");
        m.observe_build_duration(0.03);
        m.inc_fail_open();
        m.set_index_files(42);
        m.observe_context_files(7);
        m.observe_retrieval_duration(0.012);
        m.observe_retrieval_candidates(14);
        let exp = m.exposition();
        assert!(exp.contains("knocode_requests_total"));
        assert!(exp.contains("knocode_build_context_duration_seconds"));
        assert!(exp.contains("knocode_fail_open_total"));
        assert!(exp.contains("knocode_index_files 42"));
        assert!(exp.contains("knocode_context_files"));
        assert!(exp.contains("knocode_retrieval_duration_seconds"));
        assert!(exp.contains("knocode_retrieval_candidates"));
    }

    #[test]
    fn test_readiness_default_indexing() {
        let m = Metrics::new();
        assert_eq!(m.readiness(), Readiness::Indexing);
        assert!(!m.is_ready());
        assert!(m.exposition().contains("knocode_daemon_ready 0"));
    }

    #[test]
    fn test_readiness_transitions() {
        let m = Metrics::new();
        m.set_readiness(Readiness::Ready);
        assert!(m.is_ready());
        assert!(m.exposition().contains("knocode_daemon_ready 1"));
        m.set_readiness(Readiness::Indexing);
        assert!(!m.is_ready());
        assert!(m.exposition().contains("knocode_daemon_ready 0"));
    }

    #[test]
    fn test_global_singleton() {
        let a = global();
        let b = global();
        assert!(Arc::ptr_eq(&a, &b));
    }
}
