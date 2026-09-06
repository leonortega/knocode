# Knocode Roadmap

> ⚠️ **Partially superseded by [V1_RUNTIME_SPEC.md](01-architecture/V1_RUNTIME_SPEC.md)** —
> the V1 product definition (Runtime vs Knocode boundaries, out-of-scope list).
> Where the sections below still show Model Router / Skill Engine as part of the
> runtime, they reflect removed capabilities (Model Router v0.8.6; Skill Engine and
> workflow — see `01-architecture/REMOVED_TOOLS.md`); the V1 spec wins where they
> conflict.

## Current Version: v0.9.11

**Released:** September 3, 2026
**Status:** Active
**Crates:** 12 workspace members (+ `knocode-workflow` excluded, in `future/workflow/`)
**Tests:** ~400

---

## Release History

### v0.1.0 — Initial Release ✅

**Released:** August 24, 2026

- 11 Rust crates, 108 unit tests
- HTTP server for agent integration
- OpenCode and Claude Code adapters
- Promptfoo evaluation framework
- Custom implementations for all components (regex, SQLite LIKE, heuristic scoring, built-in compressors)

### v0.2.0 — External Tool Integration ✅

**Released:** August 24, 2026 | **Tests:** 128

- tree-sitter AST parsing (Rust, Python, JavaScript, TypeScript)
- ripgrep text search with .gitignore support
- tantivy BM25 full-text indexing and search
- *engram HTTP client for cross-session memory — removed in v0.7.6 (see REMOVED_TOOLS.md, replaced by SQLite+tantivy local)*
- *FlashRank reranker with TF-IDF fallback — removed in v0.7.6 (see REMOVED_TOOLS.md, SQLite+tantivy local)*
- LiteLLM client for multi-provider model routing

### v0.3.0 — Spec-Compliance ✅

**Released:** August 24, 2026 | **Tests:** 147

- UDS + MessagePack IPC with 30s fail-open
- tiktoken-rs local token counting (cl100k_base)
- Cache-aware pack: SHA-256 dedup, frozen-prefix boundary, reversible compression
- Repository Intelligence: structural search, full-text tantivy, dependency graph, git watcher
- Knowledge Hub: BM25 → FlashRank pipeline *(FlashRank removed in v0.7.6 — see REMOVED_TOOLS.md, reranker is passthrough)*, *engram deterministic reads — removed (see REMOVED_TOOLS.md)*
- LiteLLM gateway with fallback chains
- RTK adoption with tee-on-failure
- Event bus + `knocode preview`/`replay` CLI
- Interface contracts: `IContextBuilder`, `IModelGateway`, `IWorkflowEngine`

### v0.4.0 — Production Hardening + DBOS ✅

**Released:** August 24, 2026 | **Tests:** 165


- Prometheus metrics (`GET /metrics`), Grafana dashboard
- Token-bucket rate limiting, HMAC-SHA256 request signing
- Distribution: Dockerfile, Homebrew Formula, docker-compose
- Multi-agent: Cursor + Gemini CLI promoted to Tier 1
- Concurrency: `RwLock<ContextEngine>`, per-session isolation, soak 20×100
- Benchmarks: `criterion` context bench (p95 <50ms target)

### v0.5.0 — First-Class Tools ✅

**Released:** August 24, 2026 | **Tests:** 166

- Structural search: `AstGrepBackend` (ast-grep-core) + `StructuralRetriever` + `CombinedRetriever` intent routing + `QueryPlanner`
- *engram deterministic reads — removed (see REMOVED_TOOLS.md, SQLite local)*
- *FlashRank via `ort` int8, TF-IDF fallback only on model load fail — removed in v0.7.6 (see REMOVED_TOOLS.md, offline eval only)*
- *codebase-memory-mcp probe — removed (see REMOVED_TOOLS.md, local AST+regex)*
- LiteLLM `IModelGateway` with `capable→balanced→fast` cascade
- RTK vendored crate primary, built-in compressors fallback only
- Git `notify`+`git2` incremental watcher, polling fallback only
- MkDocs → Knowledge Hub ingestion (`category="docs"`)
- Promptfoo UDS custom provider

### v0.6.0 — DBOS Required + Spec Compliance ✅

**Released:** August 24, 2026 | **Tests:** 193

- DBOS promoted to **required** (`enabled:true` default), native async `#[async_trait]`
- Real `Hmac<Sha256>` (was `sha256(secret+body)`)
- OpenSpec hook compat: `chat.message` primary + `message.updated` shim
- Extended languages feature (Go, Java, C, C++ behind `extended-languages` flag)
- Duplicate collapse: single HMAC, single UDS listener

### v0.7.0 — Single-Command Bootstrap ✅

**Released:** August 25, 2026

- `knocode init` full bootstrap: scaffold → discovery → indexing → knowledge → engram → profile
- Repository discovery: language census by extension, framework detection from manifests
- Knowledge seeding at init: README + ADRs → `store_knowledge(category="docs")`
- *Engram bootstrap — removed (see REMOVED_TOOLS.md)*
- Repository profile artifact: `.knocode/profile.json`
- ast-grep via npm prebuilt, RTK prebuilt binary installers

### v0.7.5 ✅

**Released:** August 27, 2026

- 12 workspace crates (`knocode-workflow` excluded to `future/workflow/`)
- Event persistence removed from hot path (in-memory ring buffer only)
- DBOS isolated to `future/workflow/` — not required for v1
- `knocode doctor` works without DBOS

### v0.8.0 — Minimal v1 Stack ✅

**Released:** August 28, 2026

- Minimal v1 stack: Tree-sitter + Tantivy + SQLite(metadata) + Git (RTK/LiteLLM optional)
- Removed MkDocs ingestion (docs remain as plain markdown), Knowledge Hub collapsed to Repository Context
- Retrieval latency: global INDEX_CACHE + cached_reader, graph gated for doc_count>5000

### v0.8.5 — Docs + Retrieval ✅

**Released:** August 29, 2026

- First-class docs indexing without tree-sitter
- Generic ranking + docs/code split, removed domain-specific eShop code
- Version bumped to 0.8.5

### v0.8.6 — Retrieval Refactor ✅

**Released:** August 30, 2026

- Retrieval engine refactor + in-process ast-grep structural search
- Removed LLM Model Router / LiteLLM — BuildContext now deterministic (MCP retained)
- candidateK/maxFiles sweep, sanitization fix, large-repo auto-tune, MCP stdio

### v0.9.0 — Retrieval Engine v1 ✅

**Released:** September 3, 2026 | **Tests:** ~400

- **Retrieval Engine:** Intent detection → query expansion → BM25 + structural search (ast-grep) → graph boost → ranking
- **CombinedRetriever** orchestrates the full pipeline with configurable `RetrievalPolicy`
- **Benchmark suite:** 4 benchmarks (components, mattermost, dt, retrieval) — 27-106× faster than grep
- **Watch mode:** Two auto-index modes — `commit` (default, polls git HEAD) and `filesystem` (real-time via notify)
- **Dependency updates:** tantivy 0.26.1, git2 0.21, tantivy-tokenizer-api 0.7, tree-sitter-language-pack 1.16.1
- **Cleanup:** Removed engram, FlashRank, LiteLLM, MkDocs, DBOS workflow, `knocode replay`
- **Benchmark report:** `docs/BENCHMARKS_V1.md`

---

## Current Architecture

See [Architecture](01-architecture/ARCHITECTURE.md), [Components](01-architecture/COMPONENTS.md), and the V1 product definition in [V1 Runtime Spec](01-architecture/V1_RUNTIME_SPEC.md).

### Core Pipeline

```
Coding Agent → Adapter Layer (UDS/MessagePack) → Context Engine → Context Pack (YAML)
                                                       ↓
                                               Repository Intel
                                               Knowledge Hub
                                               Retrieval Engine
                                        (intent → expansion → BM25 + structural → graph → ranking)

Tool-output compression: delegated to RTK (external binary, wired by installers)
```

### Workspace Crates

| Crate | Purpose |
|-------|---------|
| `knocode-core` | Shared types, config, IPC, traits |
| `knocode-daemon` | HTTP/UDS server, adapter, metrics |
| `knocode-cli` | CLI commands (init, index, preview, doctor, etc.) |
| `knocode-context` | BuildContext pipeline, token budgeting |
| `knocode-repo-intel` | tree-sitter, ripgrep, tantivy, graph, watcher |
| `knocode-knowledge` | Knowledge Hub, retrieval |
| `knocode-optimizer` | ❌ removed — RTK compression, tool output optimization (see REMOVED_TOOLS.md) |
| `knocode-events` | Event bus (in-memory ring buffer) |
| `knocode-storage` | SQLite + tantivy persistence |
| `knocode-bench` | Criterion benchmarks |

### External Integrations

| Tool | Role | Status |
|------|------|--------|
| tree-sitter | AST parsing | ✅ First-class |
| ripgrep | Text search | ✅ First-class |
| ast-grep | Structural search (in-process `AstGrepBackend`) | ✅ First-class |
| tantivy | BM25 full-text index | ✅ First-class |
| tiktoken-rs | Local token counting | ✅ First-class |
| RTK | Tool output compression | ✅ External binary, installed + wired by installers (not embedded) |
| git2 | Commit-based auto-indexing | ✅ First-class (non-optional) |
| notify | Filesystem watcher (real-time mode) | ✅ Optional |

---

## Agent Support

| Agent | Tier | Status |
|-------|------|--------|
| OpenCode | 1 | ✅ Canonical integration |
| Claude Code | 1 | ✅ Supported |
| Cursor | 1 | ✅ Supported |
| Gemini CLI | 1 | ✅ Supported |
| Continue | 1 | ✅ Supported |
| Copilot / Factory Droid / OpenClaw / Pi | 2 | ⏳ Scaffold |
| Codex / Windsurf / Cline / Kilo / Antigravity / Kimi | 2 | ⚠️ Best-effort |

---

## Future Plans

### v0.10.0 — Graph Boost + Cache Warming

- Enable graph boost for cross-layer queries (Go ↔ React)
- Pre-compute common query patterns, warm trie on repo open
- Sub-10ms for cached queries

### v1.1 — Retrieval Quality

- Recall@5 target: 0.4 on 50-task eval dataset (current: ~0.29)
- Structural query improvement for exhaustive "find all X" patterns
- Tantivy phrase query panic fix (waiting for upstream tantivy 0.26.2+)

### v2.0 — Platform Extensions

- Multi-repository support
- Conversation memory
- Plugin system
- Web dashboard
- Distributed deployment

---

## Contributing

See [CONTRIBUTING.md](../CONTRIBUTING.md) for development guidelines.

### Priority Areas

1. **Graph boost** — Cross-layer queries (Go ↔ React, backend ↔ frontend)
2. **Cache warming** — Pre-compute common query patterns
3. **Structural recall** — Improve exhaustive "find all X" queries
4. **Benchmarks** — Context build p95, indexing throughput

---

## Success Metrics

| Metric | v0.1.0 | v0.3.0 | v0.5.0 | v0.7.5 | v0.9.0 |
|--------|--------|--------|--------|--------|------------------|
| Tests | 108 | 147 | 166 | ~184 | ~400 |
| Languages | 4 | 10+ | 111 | 111 | 111 |
| Latency P50 | <100ms | <50ms | <50ms | <50ms | 27-49ms |
| Speedup vs grep | — | — | — | — | 27-106× |
| Novelty | — | — | — | — | 53-89% |
| Workflow | — | Noop | DBOS (opt) | DBOS → future/ | Removed |
| Tool compliance | 58% | 90%+ | 15/16 | 15/16 | 15/16 |
