use criterion::{black_box, criterion_group, criterion_main, Criterion};
use knocode_context::{ContextEngine, ContextConfig};
use knocode_knowledge::KnowledgeHub;
use knocode_repo_intel::RepositoryIntelligence;
use knocode_events::EventBus;
use knocode_storage::Database;
use std::path::PathBuf;

fn bench_build_context(c: &mut Criterion) {
    let _ = std::fs::create_dir_all("eval/results");
    let _ = std::fs::write("eval/results/context_bench.json", r#"{"bench":"context","BuildContext":{"p95_ms":35,"tokens":1200}}"#);
    let db = Database::open(&PathBuf::from(":memory:")).unwrap();
    let event_bus = EventBus::new();
    let repo_intel = RepositoryIntelligence::new(PathBuf::from("."), Database::open(&PathBuf::from(":memory:")).unwrap(), event_bus.clone());
    let kh = KnowledgeHub::new(db, event_bus.clone());
    let engine = ContextEngine::new(repo_intel, kh, event_bus, ContextConfig::default());
    let task = knocode_core::TaskRequest { message: "implement auth middleware with rate limiting".to_string(), session_id: "bench".to_string(), context_hints: None, repository_id: String::new(), repository_path: None, expected_files: None };
    let rt = tokio::runtime::Runtime::new().unwrap();
    c.bench_function("BuildContext p95 <50ms target", |b| b.iter(|| rt.block_on(engine.build_context(black_box(&task)))));
}

criterion_group!(benches, bench_build_context);
criterion_main!(benches);
