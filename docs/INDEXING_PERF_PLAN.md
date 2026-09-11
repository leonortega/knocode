# Indexing Performance Plan — Step [5/7] Full-text BM25 + Symbol Extraction + Dependency Graph

> **Historical plan (v0.9.0).** `codebase-memory-mcp`, `engram`, and the Skill Engine were removed in v0.8.6–v0.9.0 (see `docs/01-architecture/REMOVED_TOOLS.md`). This document is retained for reference only.
>
> Scope: `knocode init` Step [5/7] for 63k-file repos. Focus is `RepositoryIntelligence::index_repository()` + `build_dependency_graph()` hot path in `crates/knocode-repo-intel/src/lib.rs:164`.

## 1. Background

`knocode init` 7-step pipeline `crates/knocode-cli/src/main.rs:300` + `docs/01-architecture/ARCHITECTURE.md:278`:

```
[5/7] Indexing (full-text BM25 + symbol extraction + dependency graph)
  -> RepositoryIntelligence::index_repository()  // BM25 + symbols + tantivy
  -> RepositoryIntelligence::build_dependency_graph() // second walk + extract_imports
```

At ~300 files/sec, 63k files = ~210s. User reports timeout on this step. Daemon fail-open is 30s for `BuildContext`, so init slowness blocks validation `[7/7]` too.

## 2. Root Causes (verified)

| Cause | Location | Cost at 63k |
|-------|----------|-------------|
| Sequential `WalkBuilder::build()` + per-file `read_to_string` + `is_likely_binary` extra 512B `read` + `sha256` | `lib.rs:186,218,225,234,898` | I/O bound, no `threads(N)` |
| `extract_symbols()` twice per file (DB + tantivy) | `lib.rs:259,280` -> `parser.rs:54` `tree_sitter_language_pack::process()` | ~60% CPU, duplicated |
| No transaction batching — per-row `INSERT/UPDATE` + `get_file` | `crates/knocode-storage/src/lib.rs:111` | fsync per row |
| No mtime shortcut — reads unchanged files fully to hash-compare | `lib.rs:237` | 63k reads on warm re-index |
| Tantivy per-doc `delete_query(BooleanQuery)` + `add_document(preprocess_code_content+tokenize_path)` + heap 50MB | `crates/knocode-storage/src/tantivy_index.rs:411,452` | delete+preprocess heavy |
| Second full walk for graph | `cli/main.rs:310` -> `lib.rs:768` `walk_directory` + `graph.rs:22` `extract_imports` | doubles I/O |
| Progress log every 100 `info!` | `lib.rs:301` | log pressure |


## 3. Goals / Non-Goals

**Goals:**
- Cold init 63k < 90s, warm incremental ( <1% changed) < 8s on NVMe.
- No timeout on `[5/7]`; fail-open still respected but not triggered.
- No schema change; WAL `storage/src/lib.rs:36` preserved.

**Non-Goals:**
- Changing retrieval ranking; only indexing write path.

## 4. Plan — Phases (fail-open, small diffs)

### Phase 1 — Zero-risk dedup & batching (est. 40% win, 1 PR)

1. **Dedup symbol extraction** `lib.rs:253-296`: compute `symbols = extract_symbols(&content, &patterns, lang)` once, reuse `sym_names/sym_kinds` for DB + tantivy. Delete second call at `lib.rs:280`. Test: `test_extract_symbols_*` still pass; add `test_index_reuses_symbol_once`.
2. **SQLite transaction batching** `storage/src/lib.rs:26`: add `Database::transaction(|tx|)` (rusqlite `Connection::execute_batch("BEGIN IMMEDIATE")`), wrap `index_repository` loop commit every 1000 files + final commit. Same for symbol inserts, delete loop. Measure via `log_slow:100ms`.
3. **Tantivy config** `tantivy_index.rs:414`: bump `writer(150_000_000)` for 63k, `commit` every 1000 docs (not per doc). Keep final `commit`. MmapDirectory unchanged.

### Phase 2 — Incremental fast-path (est. +30% warm win, 1 PR)

4. **mtime+size pre-filter** before `read_to_string`: add `std::fs::metadata` check; `existing_files` map stores `(id, hash, size, mtime)` via `get_all_files_with_meta` (new query `SELECT path,hash,size` already has size; add `last_indexed_at` compare). If `size == recorded && mtime <= last_indexed_at` -> skip read/hash. Only on miss do `read_to_string + compute_hash`. On hash mismatch, `update_file`. Keeps hash as source of truth, avoids 63k reads on warm.
5. **Avoid `is_likely_binary` double read**: merge ext check + null-byte check on already-read `content.as_bytes()[..512]` instead of `std::fs::read(path)` at `lib.rs:912`.

### Phase 3 — Parallelism & I/O (est. 2-3x cold win, 1 PR)

6. **Parallel walk** `lib.rs:794` `WalkBuilder::threads(4)` (keep `git_ignore(true)`, `hidden(false)`).Producer-consumer: walk thread pushes `PathBuf` to `crossbeam::channel`, 4 workers do `classify_file + detect_language + metadata filter` before read. Keep single SQLite/tantivy writer thread to respect `!Send` `Connection`.
7. **Defer graph off init hot path** `cli/main.rs:310`: remove blocking `build_dependency_graph()` from `[5/7]`. Compute lazily: daemon `ContextEngine` caches `DependencyGraph` on first `build_context` with `tokio::task::spawn_blocking` + `timeout 2s`, or `knocode graph --warm` explicit command. `init` prints `Dependency edges: deferred (warm on first query / run 'knocode graph')`. Saves second 63k walk during init.
8. **Reduce `is_indexable_text_file` + `classify_file` alloc**: use `path.extension()` once, pass `&str` not `String`.

### Phase 4 — Observability & guardrails (1 PR)

9. **Progress & metrics**: keep `info! every 100` but add `every 1000` `duration_ms/files_per_sec` + `metrics::global().set_index_files`. Add `KNOCODE_PROFILE=1` timing already at `lib.rs:362`.
10. **Large-repo hint**: if `walk_count > 50000` log `hint: set KNOCODE_SYMBOLS_ENABLED=false for ~20% faster indexing (BM25 only), symbols still available via query-time fallback`.

## 5. Files to Touch

- `crates/knocode-repo-intel/src/lib.rs:164,253,280,768,794,898` (core)
- `crates/knocode-repo-intel/src/parser.rs:48` (no change, just reused)
- `crates/knocode-storage/src/lib.rs:30,110` (transaction helpers, `get_all_files_with_meta`)
- `crates/knocode-storage/src/tantivy_index.rs:411` (writer heap)
- `crates/knocode-cli/src/main.rs:300,1059` (defer graph, progress)
- `crates/knocode-core/src/config.rs:340` (optional `KNOCODE_INDEX_THREADS` env, not required)


## 6. Risks & Mitigations

- **Parallelism + rusqlite `!Send`**: mitigate with single writer channel; workers send `FileJob` structs.
- **mtime shortcut false negative**: hash remains canonical; mtime only skips read, never skips hash mismatch update.
- **Tantivy larger heap OOM on small machines**: gate `150MB` behind `if files > 20000` else `50MB`.

## 7. Verification

- **Bench**: `cargo bench --bench context_bench` + new `benches/indexing.rs` (63k synthetic via `tempdir` 60k empty `.rs` + 3k real). Assert cold <90s, warm <8s on CI NVMe.
- **Tests**: existing `test_incremental_indexing:1360`, `test_mkdocs_ingestion_is_idempotent`, plus new `test_index_mtime_skip` and `test_symbol_dedup`.
- **Manual**: `KNOCODE_PROFILE=1 cargo run --bin knocode -- init` on 63k clone; check `Duration` log `lib.rs:362`.
- **Rollback**: each phase independent, revert single commit.

## 8. Rollout

1 PR per phase, behind no feature flag (behavior identical). Phase 1 can ship immediately; Phase 3 behind `KNOCODE_INDEX_THREADS=4` default, `1` to revert.

---
*Created for 63k-file stress timeout (historical plan, v0.9.0).*
