# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed
- **Copilot integration registered via user-level hooks, not plugin discovery** — VS Code/Copilot never scans `~/.knocode/copilot-plugin` (only marketplaces, `Chat: Install Plugin From Source`, or `~/.copilot/installed-plugins/`), so the deployed bundle was invisible. All four installers now write `~/.copilot/hooks/knocode-context.json` (SessionStart + UserPromptSubmit, absolute script path, forward slashes) — the same registration mechanism RTK uses. The `~/.knocode/copilot-plugin` bundle remains the hook-script home; its bundled stdio MCP server is no longer auto-registered (context flows through the hooks).
- **`knocode-copilot-plugin` hook surface moved to `UserPromptSubmit`** — the `PreToolUse` hook (context for read/search tools) is removed: RTK owns Copilot's `PreToolUse` layer for command rewriting, and a second PreToolUse duplicated daemon calls per tool. `UserPromptSubmit` now injects repository context retrieved from the **user's actual prompt** (the faithful analog of the OpenCode plugin's `session.prompt` admission hook); `SessionStart` keeps seeding a warm repository overview. Hook handler, `hooks.json`, smoke test, and README updated; smoke test passes (`session-start` + `user-prompt-submit`).

### Fixed
- **Installers now remove a non-RTK `rtk` name-collision binary instead of just warning** — when the identity probe (`rtk init --help`) fails (e.g. the unrelated crates.io "Rust Type Kit" `rtk` in `~/.cargo/bin`), all four installers (`installers/knocode-install.{ps1,sh}`, `scripts/install.{ps1,sh}`) now `cargo uninstall rtk`/delete the impostor before downloading the real [rtk-ai/rtk](https://github.com/rtk-ai/rtk) release, so the wrong binary can no longer shadow the real one on PATH. If removal fails, an explicit manual-removal warning is shown.

### Added
- **Lightweight integration-boundary metrics** — daemon `ContextPack` now carries `retrieval_stats` (code-search duration + candidate/result counts from the engine where the data lives), observed as `knocode_context_files`, `knocode_retrieval_candidates`, and `knocode_retrieval_duration_seconds` histograms/counters on `GET /metrics`; no I/O or serialization on the hot path. The plugin measures its integration boundary with a single `Date.now()` pair and logs one INFO line per prompt: `[knocode] context latency=<ms> tokens=<n> files=<n>` (plus a `context passthrough latency=<ms>ms` line on fail-open), reading `total_tokens`/`provenance` from the existing MCP `structuredContent` — no metrics pipeline, no network, no duplicate daemon metrics.

### Changed
- **`opencode-knocode` ported to the OpenCode V2 plugin spec** — `Plugin.define({ id, setup })` + `ctx.session.hook("prompt")` replaces the V1 `chat.message` hook: enrichment now runs during prompt admission, mutating the owned `event.prompt.text` draft (edits become the canonical persisted user input). Adds an idempotency guard against hook replays and clears stale attachment-mention offsets when a rewrite breaks the original-text prefix. Dependency moved to `@opencode-ai/plugin` `beta` (V2 API is beta). The plugin's legacy `POST /hook` client (`callKnocodeDaemon`, `KnocodeRequest`/`KnocodeResponse`, `hashRepositoryId`) was removed — MCP `knocode_context` is the only enrichment path.
- **Legacy `/hook` ToolOutput contract and `ExecutionOptimizer` removed from the daemon** — `POST /hook` now serves the pre-generation hook only; `PreToolCall` payloads answer HTTP `400`. `RequestPayload::ToolOutput`, `ResponsePayload::CompressedOutput`, `HookType::PreToolCall`, the `tokens_saved` metric, and the orphaned `.claude/hooks/knocode-pretool.sh` were deleted. Tool-output compression lives exclusively in [RTK](https://github.com/rtk-ai/rtk).
- **`knocode_compress` removed from the daemon MCP surface** — `POST /mcp` now exposes a single tool, `knocode_context`; tool-output compression lives exclusively in [RTK](https://github.com/rtk-ai/rtk) (installed/wired by the knocode installers on request). Calling `knocode_compress` answers the standard JSON-RPC `-32602` unknown-tool error.
- **OpenCode plugin is context-only** — `tool.execute.after` (compression) removed from `opencode-knocode`; the Copilot agent plugin's `PostToolUse` compression hook removed likewise. RTK ships its own OpenCode/Copilot integrations and the knocode installers now offer RTK as an opt-in external resource (`--with-rtk`), wiring it via `rtk init -g` for each selected agent.
- **Docs updated** — RUNTIME / ARCHITECTURE / COMPONENTS / DATA_FLOW / REQUEST_LIFECYCLE / V1_RUNTIME_SPEC / REMOVED_TOOLS / ROADMAP and `00-project` docs now reflect the daemon being context-only.

## [0.9.11] - 2026-09-04 — Doctor index-path fix

### Fixed
- **`knocode doctor` reported `0 docs` / "no results — re-run init"** — the doctor probes opened the global `~/.knocode/index` container instead of the repo-scoped `~/.knocode/index/<repository_id>/` directory that `init` and the daemon write to. Doctor now resolves the index via `default_index_path(repository_id)` (honoring `KNOCODE_INDEX_DIR`), so `Tantivy:` and `Retrieval:` report the real in-repo counts.

### Changed
- **Version strings synced** across `knocode.json` (Scoop), `Formula/knocode.rb` (Homebrew), `packages/opencode-knocode/package.json`, `Cargo.lock` to match workspace `0.9.11`

---

## [0.9.10] - 2026-09-04 — Housekeeping + Cleanup

### Fixed
- **Undefined `$ocGlobalDir` in `scripts/uninstall.ps1`** — npm plugin cleanup silently did nothing on Windows; variable now defined before use

### Changed
- **Version strings synced** across `knocode.json` (Scoop), `Formula/knocode.rb` (Homebrew), `packages/opencode-knocode/package.json` to match workspace `0.9.10`
- **Removed stale docs** — `ADAPTERS.md` (references deleted adapter files), `EVALUATION.md` (references removed Model Router/FlashRank), `docs/dashboards/knocode.json` (stale Grafana dashboard)
- **Cleaned `.opencode/.gitignore`** — removed `engram/` entry (engram removed in v0.7.6)
- **Cleaned `docs/INDEXING_PERF_PLAN.md`** — removed references to deleted `codebase-memory-mcp` and `engram`

---

## [0.9.5] - 2026-09-03 — GitHub Releases + Lean Installers

### Added — Release Pipeline
- **GitHub Actions release workflow** `.github/workflows/release.yml` — on `v*` tag push: verifies the tag matches `workspace.package.version` in `Cargo.toml` (aborts on mismatch so releases can't be mislabeled), builds `knocode` + `knocode-daemon` (Windows x64, `--release`), packages `knocode-<ver>-x86_64-pc-windows-msvc.zip`, and publishes it to the tag's GitHub Release (auto-created if missing, updated on re-tag) together with the end-user installer
- **End-user installer** `installers/knocode-install.ps1` — ships in every Release: downloads the matching prebuilt archive (latest by default, or pinned via `-Version`), installs `knocode.exe` + `knocode-daemon.exe` to `~/.knocode/bin`, persists that dir on the USER PATH (idempotent), and verifies with `knocode --version`. One-liner: `powershell -ExecutionPolicy Bypass -c "irm https://github.com/leonortega/knocode/releases/latest/download/knocode-install.ps1 | iex"`

### Changed — Installers (prebuilt-only, no Rust)
- **Removed Rust/rustup + clippy from `scripts/install.ps1`/`install.sh`** — the installers no longer compile (they consume the `target/release/` prebuilt binaries; source builds use `scripts/compile.*` or CI), so the rustup install/update, rustc checks, and `rustup component add clippy` are gone
- **Removed ast-grep CLI** (`npm @ast-grep/cli`) — structural search is the embedded `ast-grep-core` crate, no external binary required
- **Removed eslint** global install — the `cargo clippy`/`eslint` analyzer gates have no runtime call sites (reserved for the excluded workflow crate)
- **`-SkipExternal` now skips RTK + promptfoo too** in `install.ps1` (brace placement fixed) — matches `install.sh` and the documented semantics; promptfoo/Python remain developer-only eval tooling
- **Coderun → Knocode rename** in uninstall messaging and related configuration

### Changed — CI
- **GitHub Actions on the Node 24 runtime** — `actions/checkout@v5` + `softprops/action-gh-release@v3` (v2.6.2 was the last Node 20 release), clearing the Node 20 deprecation warnings

### Changed
- Version `0.9.0 → 0.9.5` in `Cargo.toml` (`workspace.package`) and `release.toml`

### Docs
- AI Runtime V1 Specification added; roadmap updated

---

## [0.9.0] - 2026-09-03 — Retrieval Engine v1 + Benchmarks

### Added — Retrieval Engine
- **Intent detection** `crates/knocode-context/src/retrieval/intent.rs` classifies queries into categories (procedural, structural, debugging, informational, mixed) to route to the best search strategy
- **Query expansion** `crates/knocode-context/src/retrieval/query.rs` expands queries with synonyms and related terms (e.g., "error handling" → [error, handling, try, catch, exception]) for higher recall
- **BM25 full-text search** via tantivy `MmapDirectory` for fast lexical matching across indexed files
- **Structural search** `crates/knocode-context/src/retrieval/structural.rs` in-process ast-grep backend for code-aware search (find function definitions, class hierarchies, import chains)
- **Graph boost** `crates/knocode-context/src/retrieval/ranking.rs` boosts files related to top candidates via the code dependency graph
- **Retrieval policy** `crates/knocode-context/src/retrieval/policy.rs` configurable tuning: `candidate_k` (50→500), `max_files`, `enable_graph`, `enable_expansion`
- **Candidate pool sweep** — `candidate_k` increased from 50 to 500 with +249% recall improvement at only +3ms latency cost
- **CombinedRetriever** `crates/knocode-context/src/retrieval/engine.rs` orchestrates the full pipeline: intent → expansion → BM25 + structural → graph → ranking

### Added — Benchmark Suite
- **bench_components** — Component evaluation on knocode repo (20 queries, measures impact of graph boost, candidate_k, query expansion)
- **bench_mattermost_50** — 50 queries against Mattermost (9k Go + React files) vs `grep -rE`
- **bench_dt_50** — 50 queries against DefinitelyTyped (53k TypeScript files) vs `grep -rE`
- **bench_retrieval_50** — 50 queries on knocode repo with `index_repository()` cold/warm comparison
- **benchmark.rs** — Unit tests for recall@k, MRR, keyword coverage, intent detection metrics
- Full benchmark report: `docs/BENCHMARKS_V1.md`

### Added — Repository Intelligence Improvements
- **First-class docs indexing** — Markdown files indexed without tree-sitter via dedicated path
- **Docs/code split** — Separated documentation and code retrieval paths for better precision
- **PascalCase splitting** — Symbol extraction splits `MyFunctionName` → [My, Function, Name] for better BM25 matching
- **Path tokenization** — File paths split into searchable tokens (e.g., `src/components/Header.tsx` → [src, components, Header, tsx])
- **Incremental symbol extraction** — `mtime+size` shortcut skips unchanged files on warm re-index (cold: 1,455 symbols, warm: 0-298)

### Added — Watch Mode Configuration
- **Two auto-index modes** `crates/knocode-repo-intel/src/watcher.rs`:
  - `commit` (default) — Polls the resolved HEAD commit (git2, handles branch refs/packed refs/detached HEAD) every 5s, re-indexes only on new commits
  - `filesystem` — Real-time file change detection via `notify` crate
- **Configurable** via `[index].watch_mode` in `.knocode/config.toml` or `KNOCODE_WATCH_MODE` env var
- **git2 now non-optional** — Required for commit-based watching (always available)

### Added — Daemon Readiness Endpoint
- **`GET /health` + `GET /metrics` readiness** — `GET /health` now reports `state: "indexing" | "ready"` (plus `index_files`), `GET /metrics` exposes a `knocode_daemon_ready` gauge, and `POST /hook` returns HTTP `503` with `reason: "daemon_indexing"` until the initial index completes. The HTTP health/metrics listener binds *before* indexing (only request serving is readiness-gated), so clients can poll readiness and wait instead of queueing on the engine lock mid-index.
- **UDS/MessagePack `Probe` payload** — new `RequestPayload::Probe` / `ResponsePayload::Probe` (plus `HookType::Probe`) over the primary transport: send `{"type":"Probe"}` and get `state`, `index_files`, and `version` back. Answered before rate-limiting/gating with no engine lock — the same readiness signal as `GET /health`, so UDS clients can wait before sending real requests.
- **Client adapters poll readiness before their first request** — the OpenCode plugin, Claude Code hooks (`.claude/hooks/knocode-ready.sh`), Gemini CLI hooks, and Cursor extension now wait for `state: "ready"` (polling `GET /health`, bounded + fail-open) before their first real request, so a cold-starting daemon enriches the first prompt instead of 503-passthroughing it. Unreachable daemons bail immediately (hooks never stall on a missing daemon); successful checks are cached for 30s. `KNOCODE_READY_TIMEOUT_MS` controls the wait budget (default 10s).


### Added— Daemon MCP Surface (`POST /mcp`)
- **Daemon-hosted MCP**— JSON-RPC 2.0 at `POST /mcp` on the existing `127.0.0.1:9527` axum listener (identical on Windows and Unix; no extra socket or process). Stateless tools-only subset: `initialize`, `ping`, `tools/list`, `tools/call`, `notifications/initialized` (HTTP 202).
- **Two tools**— `knocode_context(prompt, repository_path?)` (enriched context answer + provenance `structuredContent`) and `knocode_compress(content, tool_name, output_type?, context?)` (compressed output + token counts) - the same engine/optimizer paths `/hook` uses.
- **No-conversion client path**— the opencode plugin now drives both hooks through the MCP tools (`chat.message` -> `knocode_context`, `tool.execute.before` -> `knocode_compress`); daemons that predate `/mcp` are served via the automatic `/hook` fallback (wire behavior unchanged).
- **Readiness over MCP**— `tools/list`/`initialize` answer while indexing; `tools/call` returns JSON-RPC `-32001 daemon_indexing` (HTTP `200`) until the index is ready - parity with the `/hook` 503 gate.
- **`knocode-mcp` rewritten as a pass-through proxy** — `packages/knocode-mcp` (stdio MCP for Codex, VS Code Copilot, Claude) no longer keeps its own half-implemented tool list: `knocode_search` was an alias of `knocode_preview`, `knocode_symbols` dumped the rewrite JSON, and `knocode_read` never read a file. It now relays stdio JSON-RPC verbatim to the daemon’s `POST /mcp`, so those agents see the identical canonical tools (`knocode_context`, `knocode_compress`) with no surface drift; MCP notifications are forwarded but never answered, and an unreachable daemon answers a JSON-RPC `-32000` so clients fail open.

### Changed — Dependency Updates
- **tantivy** updated to `0.26.1` (latest stable)
- **git2** updated from `0.19` to `0.21` (fewer transitive deps: libssh2, openssl removed)
- **tantivy-tokenizer-api** updated from `0.2` to `0.7`
- **tree-sitter-language-pack** updated to `1.16.1`
- **Version** bumped from `0.8.6` to `0.9.0`

### Changed — Cleanup
- **Removed engram** — Cross-session memory now handled by SQLite+tantivy local
- **Removed FlashRank** — Reranker removed per benchmark evaluation (MRR degradation)
- **Removed LLM Model Router / LiteLLM** — BuildContext now deterministic (MCP retained)
- **Removed MkDocs integration** — Docs indexed via first-class path instead
- **Removed DBOS workflow** — `knocode-workflow` crate, the daemon `workflow` feature and its routes, and the CLI `workflow` command deleted (see `docs/01-architecture/REMOVED_TOOLS.md`)
- **Removed `knocode replay`** — Event replay removed from hot path, `tracing` + `metrics` retained
- **Removed the Skill Engine** — `knocode-skills` crate, `ContextPack.behavioral_skills`, Knowledge Hub skill load/match, the CLI `skills` subcommand + `--community-skills`, `[skills]` config, and repo skill content deleted (see `docs/01-architecture/REMOVED_TOOLS.md`)
- **Removed the model map** — `[model]` config + `KNOCODE_MODEL_DEFAULT`, metrics tier dimension, and `token_usage.model`/`tier` columns (migration 007)
- **Bench files gated behind `#[cfg(test)]`** — Zero warnings in release builds

### Changed — Installers
- **RTK downloaded from GitHub releases** — both installers now fetch the matching prebuilt asset for the platform (`rtk-x86_64-pc-windows-msvc.zip`, `rtk-{aarch64,x86_64}-apple-darwin.tar.gz`, `rtk-{x86_64-musl,aarch64-gnu}-unknown-linux*.tar.gz`) from `rtk-ai/rtk/releases/latest/download/<asset>`, extract it to `~/.knocode/bin/rtk(.exe)`, and clean up the temp download. The repo-shipped `.knocode/rtk/` prebuilt and the cargo build fallback (git + crates.io) are removed — crates.io `rtk` is an unrelated crate, so no external binaries are kept in the repo. `uninstall.ps1` now also removes the current `~/.knocode/bin/rtk.exe` path.
- **Knocode agent skill restored (per-agent, optional)** — `.knocode/skills/knocode/SKILL.md` — the skill that teaches an agent how to use the runtime (binary location, `init`/`doctor`, MCP tools) — is kept even though the runtime Skill Engine is removed. Installers now copy it to the agent’s global skills directory (`~/.config/opencode/skills/knocode/`), opencode being the only supported agent today, and uninstallers remove that copy. Agent-native discovery only: the runtime never matches or injects skills.
### Fixed
- **bench_retrieval_50 empty results** — Added missing `index_repository()` call before queries
- **Unnecessary parentheses warnings** in `bench_components.rs` (3 instances)
- **Dead code warnings** in `knocode-repo-intel` — Gated `DEBOUNCE_MS`, `LIBGIT2_CACHE_MAX_BYTES`, `repo_has_changes` behind `#[cfg(feature = "fs-watcher")]` and `#[cfg(test)]`
- **Auto-reindex goes through the ContextEngine** — the daemon watcher reindexes via `ContextEngine::reindex_repository`, and indexing now evicts the repo's cached tantivy handle on completion so the next query serves the fresh commit immediately instead of from a stale pre-reindex reader

### Performance

| Metric | Mattermost (9k) | DefinitelyTyped (53k) |
|--------|----------------|----------------------|
| Retrieval P50 | 27ms | 47ms |
| Grep P50 | 971ms | 4,819ms |
| Speedup | **27×** | **106×** (P50) |
| Novelty | 53.0% | 89.2% |
| Recall | 13.1% | 14.2% |

---

## [0.7.0] - 2026-08-25 — Single-Command Bootstrap

### Added — One-Command Repository Bootstrap
- **`knocode init` full bootstrap** `crates/knocode-cli/src/main.rs:cmd_init` 6 phases in one command (was scaffold-only + manual `index`): `[1/6]` scaffold `.knocode/`, config, skills, database → `[2/6]` repository discovery → `[3/6]` indexing (tree-sitter symbols + tantivy BM25 + dependency graph) → `[4/6]` knowledge initialization → `[5/6]` engram memory initialization → `[6/6]` repository profile; incremental and safe to re-run
- **Repository discovery** `discover_repository()`/`walk_ext_counts()`/`ext_language()` language census by extension (16 mappings, skip-list for `.git/node_modules/target/dist/build/vendor/...`), `detect_stack()` frameworks + build/test commands from manifests (`Cargo.toml` + workspace detection, `package.json` scripts + deps react/vue/svelte/next/express/nest, `go.mod`, `pyproject.toml` poetry/uv, `requirements.txt`, `pom.xml`, `build.gradle(.kts)`, `Makefile`), git branch from `.git/HEAD`
- **Knowledge seeding at init** `ingest_seed_documents()` README (+`docs/adr|decisions/*.md`) → `store_knowledge(category="docs"/"adr")` so retrieval works before first task
- **Engram bootstrap seed** `init_engram()` health-checks `knowledge.memory_endpoint`, seeds `repository-profile` + `readme` entries in project namespace via real `EngramClient` HTTP (fail-open with status message when endpoint unreachable/disabled)
- **Repository Profile artifact** `build_profile_json()` → `.knocode/profile.json` (languages+counts, frameworks, commands, important dirs, git branch, index stats) + stored as knowledge entry `category="profile"`, committable per repo
- **Doctor probe** `Repo profile:` check added to `cmd_doctor`

### Changed — Installers
- **ast-grep via npm** prebuilt `@ast-grep/cli` (was cargo compile), PATH fixup + graceful WARN fallback
- **RTK prebuilt binary** installers copy `.knocode/rtk/rtk.exe` → `~/.knocode/bin/rtk.exe` (unified bin, legacy `~/bin/rtk.exe` migrated; no compile), live-streamed cargo build output as last resort (PS5.1 EAP fixes)

### Changed
- Version `0.6.0 → 0.7.0` `Cargo.toml` + `release.toml`

## [0.6.0] - 2026-08-24 — DBOS Required + Spec Compliance

### Changed — DBOS Promoted to Required (SQLite + Litestream native async)
- **DBOS required** `crates/knocode-core/src/config.rs:WorkflowConfig` `enabled:true` `engine:dbos` default (was `false/noop`), single-node SQLite `sqlite://~/.knocode/dbos.db` + `sqlite://~/.knocode/dbos_system.db` + Litestream replica `DBOS_LITESTREAM_REPLICA_URL`
- **Native async** `crates/knocode-core/src/traits.rs:IWorkflowEngine` → `#[async_trait]` `async fn start_workflow/get_status/is_available`, `crates/knocode-workflow/src/dbos.rs` deleted `block_on_in_thread` hack, direct `tokio::time::timeout(5s/3s/1s)` via shared `reqwest::Client`; `NoopWorkflowEngine` kept only `#[cfg(test)]`; CLI `crates/knocode-cli/src/main.rs:cmd_workflow` now `rt.block_on`; fail-closed when DBOS down (was fail-open)
- **HMAC canonical** `crates/knocode-core/src/secrets.rs:verify_hmac/hmac_hex` single impl via `hmac = "0.12"` `Hmac<Sha256>` `LazyLock<Regex>` for `redact_secrets` (was `sha256(secret+body)` ×2 in `workflow/dbos.rs` + `daemon/ratelimit.rs`); `daemon/ratelimit.rs:verify_hmac` delegates to core
- **Sidecar native** `workflow/dbos/src/main.ts` + `workflow/dbos/src/workflows/governed.ts` now import `dbos-transact` `DBOS.workflow/communicator/transaction/sleep/signal`, `workflow/dbos/package.json` `0.4.0→0.6.0` + `dbos-transact = "^1.2.0"`

### Added — Duplicate Collapse + Extended Languages
- **Skill scorer single** `crates/knocode-skills/src/lib.rs:SkillEngine::from_skills` + `crates/knocode-knowledge/src/lib.rs:match_skills` delegates to canonical `SkillEngine::match_skills` (deleted `simple_tag_match` divergent `0.3` scorer)
- **Extended languages feature B** `crates/knocode-repo-intel/Cargo.toml` `extended-languages = [tree-sitter-go/java/c/cpp]` optional, `crates/knocode-repo-intel/src/parser.rs:get_language` `#[cfg(feature)]` arms for `go,java,c,cpp` (`cpp` also `c++` alias), `crates/knocode-core/src/config.rs:IndexConfig` default `4` langs (was `8`), `validate()` warns if `go/java/c/cpp` without feature
- **Hook compat (OpenSpec)** `.opencode/plugins/knocode.ts` dual registration `chat.message` primary + `message.updated` compat shim `WARN` + metric placeholder (see `docs/V0_6_0_PLAN.md:2.1`)
- **Workspace deps** `async-trait = "0.1"` `hmac = "0.12"` `sha2` deduplicated in `Cargo.toml:18`; `crates/knocode-core:Cargo.toml` + `knocode-workflow` add `hmac,async-trait`

### Changed
- Version `0.5.0 → 0.6.0` `Cargo.toml:18` + `release.toml:39`, 193 tests (was 166; +27 via extended-languages + async DBOS + HMAC core)

## [0.5.0] - 2026-08-24 — First-Class Tools

### Added — First-Class Fixes (fallbacks kept only inside Err/warn)
- **ast-grep** `search_structural()` first-class `sg-core` gated `repo-intel/src/lib.rs:348` (heuristic deleted as primary, kept only in `search_structural_fallback()`)
- **engram** deterministic reads `knowledge/src/lib.rs:248` `EngramClient::search_memory()` `2s timeout` primary `block_on_in_thread`, `db.search_memory()` LIKE only on Err
- **FlashRank via ort** `knowledge/src/rerank.rs:1` `ort=2.0.0-rc.13` optional feature int8 `~/.knocode/models/flashrank.onnx` primary, `rerank_tfidf()` fallback with WARN, `Default enabled:true`
- **codebase-memory-mcp** `repo-intel/src/graph.rs:20` `try_codebase_memory_mcp()` via `npx` probe primary, regex `extract_imports()` fallback with WARN
- **LiteLLM** `LiteLLMGateway` `router/src/lib.rs:222` `complete_with_fallback()` `capable→balanced→fast` cascade + `cost_usd` `003_graph.sql:15`
- **RTK** vendored crate primary `optimizer/src/lib.rs:66` `RtkAdapter::compress()` first, built-ins `WARN` fallback, `tee` `~/.knocode/logs/tool-failures/`
- **Git** `notify+git2` `repo-intel/src/watcher.rs:7` `try_notify_git2_watcher()` primary `notify::RecommendedWatcher`+`git2::diff`, polling fallback
- **MkDocs** ingestion `repo-intel/src/lib.rs:290` walk `docs/**/*.md` → `store_knowledge(category="docs")` + tantivy on `index_repository()`
- **Promptfoo** UDS `eval/providers/context-quality.js:1` `net.createConnection("/tmp/knocode.sock")`+`msgpack-lite` length-prefix+`rmp-serde` primary, mock fallback
- **Native analyzers** `optimizer/src/analyzers.rs` `run_gate()` `cargo clippy -D warnings` post-DBOS gate
- **Workspace** `notify="6"`, `git2="0.19"`, `ort` per-crate optional `knowledge/Cargo.toml:20` feature `ort`

### Changed
- Version `0.4.0 → 0.5.0` `Cargo.toml:18`, 166 tests (15 knowledge due to `RerankerConfig` `enabled:true`)

## [0.4.0] - 2026-08-24 — Production Hardening + DBOS

### Added
- **DBOS Transact** durable workflows: `crates/knocode-workflow` (`DBOSWorkflowEngine: IWorkflowEngine`), Node sidecar `workflow/dbos/` (SQLite WAL + Litestream, approval gates, audit), `005_audits.sql` (`audits` + `workflows`), CLI `knocode workflow start/status/approve/list`, `WorkflowConfig` (`KNOCODE_WORKFLOW_ENABLED`, `KNOCODE_DBOS_SECRET`)
- **Observability:** `daemon/src/metrics.rs` Prometheus exposition (`GET /metrics` `knocode_requests_total`, `knocode_build_context_duration_seconds` histogram, `knocode_fail_open_total`), Grafana `docs/dashboards/knocode.json`, alerts `deploy/prometheus/alerts.yml`
- **Security:** `daemon/src/ratelimit.rs` token-bucket (10/s burst 20 per `session_id`), HMAC-SHA256 `X-Knocode-Signature`, structured audit log off hot path
- **Concurrency:** `AdapterLayer` `Mutex→RwLock` (`daemon/src/adapter.rs:44`), session-isolated memory namespace, soak test 20×100
- **Distribution:** `Dockerfile` (multi-stage distroless), `Formula/knocode.rb` (brew tap with service), `deploy/docker-compose.yml` wiring DBOS sidecar
- **Multi-agent:** Cursor + Gemini CLI promoted to Tier 1 `ADAPTERS.md:10` (RwLock session isolation proof), Continue promoted, Copilot/Factory Droid scaffolds
- **Benchmarks:** `benches/context_bench.rs` (`criterion` p95 <50ms target)

### Changed
- Version `0.3.0 → 0.4.0` `Cargo.toml:18`
- `http_server.rs:93` adds `/metrics`, `/workflow/*` routes; `doctor` now 8 probes (DBOS)

## [0.2.0] - 2026-08-24

### Added

#### External Tool Integration
- **tree-sitter** for AST parsing (Rust, Python, JavaScript, TypeScript)
- **ripgrep** (grep-searcher) for fast text search with .gitignore support
- **tantivy** for BM25 full-text indexing and search
- **engram** HTTP client for cross-session memory
- **FlashRank** reranker with TF-IDF fallback
- **LiteLLM** client for multi-provider model routing

#### New Modules
- `knocode-repo-intel/src/parser.rs` — tree-sitter AST parsing
- `knocode-storage/src/tantivy_index.rs` — tantivy BM25 index
- `knocode-knowledge/src/engram.rs` — engram HTTP client
- `knocode-knowledge/src/rerank.rs` — reranking module
- `knocode-router/src/litellm.rs` — LiteLLM client

### Changed
- Repository Intelligence now uses ripgrep for text search
- Repository Intelligence uses ignore crate for .gitignore support
- Symbol extraction uses tree-sitter AST when available, falls back to regex
- Storage module includes tantivy index for full-text search
- Knowledge Hub includes engram client for cross-session memory
- Router includes LiteLLM client for multi-provider routing

### Test Results
- **128 tests passing** (up from 108 in v0.1.0)
- 20 new tests for external tool integration

## [0.3.0] - 2026-08-24

### Added

#### P0 — Non-Negotiable Spec Compliance

- **UDS + MessagePack IPC + 30s fail-open** — dual transport: UDS/MessagePack primary, HTTP/JSON fallback (`crates/knocode-daemon/src/adapter.rs:70-193`, `crates/knocode-daemon/src/lifecycle.rs:158-280`, `crates/knocode-daemon/src/http_server.rs:129-145` secret redaction + input validation 100KB/1MB)
- **`tiktoken-rs` token counting** — local `cl100k_base` in `crates/knocode-context/src/lib.rs:388-413` and `crates/knocode-optimizer/src/lib.rs:264-302`, fallback heuristic only on load failure
- **Cache-aware pack hardening** — dedup via SHA-256 `session_fingerprints` `crates/knocode-context/src/lib.rs:70-118`, frozen-prefix `FROZEN PREFIX END` `lib.rs:153-170`, reversible truncation `~/.knocode/cache/originals/{hash}` `lib.rs:415-462`
- **Repository Intelligence completion** — `search_structural` (tree-sitter+regex) `crates/knocode-repo-intel/src/lib.rs:328-410`, `search_fulltext` (tantivy BM25) `lib.rs:412-453`, tantivy upsert in `index_repository` `lib.rs:176-320`, `graph.rs` dependency graph + `lsp.rs` optional + `watcher.rs` git polling `crates/knocode-repo-intel/src/*.rs`, migrations `003_graph.sql`+`004_events.sql`

#### P1 — Integrations

- **Knowledge Hub unification** — BM25→FlashRank adaptive K `crates/knocode-knowledge/src/lib.rs:160-230`, deterministic engram hot reads `lib.rs:232-252` (2s timeout, fail-open local)
- **LiteLLM gateway + fallback** — `IModelGateway` `crates/knocode-core/src/traits.rs:11-22` + `crates/knocode-router/src/lib.rs:329-365` `fallback_chain`, `cost_usd` in `003_graph.sql`
- **RTK adoption** — `crates/knocode-optimizer/src/rtk.rs:1-120` adapter (binary detection, tee-on-failure `~/.knocode/logs/tool-failures/`) + in-process fallback
- **Event bus + inspection** — real `knocode preview`/`replay` `crates/knocode-cli/src/main.rs:234-400`, SQLite spill `004_events.sql`, async-only invariant preserved

#### P2 — Packaging / Docs / Security

- **Interfaces as contracts** — `IContextBuilder`/`IModelGateway`/`IWorkflowEngine` `crates/knocode-core/src/traits.rs:1-51` + `secrets.rs` redaction before outbound calls
- **Packaging & hardening** — `knocode init --wizard`, expanded `knocode doctor` (9 probes incl. tiktoken+tantivy+redaction) `crates/knocode-cli/src/main.rs:489-640`, `knocode migrate --from claude|continue|cursor`
- **MkDocs → Knowledge Hub** — `mkdocs.yml` + ingest docs into Knowledge Hub (`category="docs"`)
- **Security** — input validation `http_server.rs:validate_input_len`, secrets redaction `crates/knocode-core/src/secrets.rs:1-35`, token-bucket stub (rate limit)
- **Benchmarks** — `benches/context_bench.rs` micro-benches (BuildContext p95, tiktoken 10KB, compression)
- **Multi-agent** — Cursor (`adapters/cursor/extension.ts`) + Gemini (`adapters/gemini/hooks.sh`) Tier 1, Tier 2 best-effort `adapters/tier2/README.md`, `docs/ADAPTERS.md` updated

### Test Results

- **147 tests passing** (up from 128 in v0.2.0)
- Zero warnings, zero clippy warnings
- Migrations 001-004 idempotent

### Changed

- Version bump 0.1.0 → 0.3.0 (`Cargo.toml:17`, `release.toml:39`)

## [0.1.0] - 2026-08-24

### Added

#### Core System
- Configuration system with TOML loading, env overrides, and validation
- Core types with error enums, IPC message types, and serde support
- Event Bus with broadcast channel and in-memory buffer
- Local Storage with SQLite, WAL mode, and migrations

#### Intelligence Components
- Repository Intelligence with incremental indexing and regex-based search
- Skill Engine with Markdown/TOML/YAML parsing and tag-based matching
- Knowledge Hub with SQLite storage, search, and pattern extraction
- Model Router with heuristic complexity scoring and tier selection
- Execution Optimizer with type-specific compression (file, search, shell)

#### Runtime
- Context Engine with pipeline assembly, cache ordering, and token budget
- Adapter Layer with HTTP server (JSON) and fail-open behavior
- Daemon Lifecycle with startup, shutdown, and signal handling
- CLI Commands (init, index, preview, status, skills, config, doctor)

#### Agent Integration
- OpenCode plugin (TypeScript) with pre-generation and pre-tool hooks
- Claude Code hooks (shell scripts) for UserPromptSubmit and PreToolUse

#### Evaluation
- Promptfoo evaluation framework with model routing and context quality tests
- 20 evaluation tests (11 model routing, 9 context quality)

#### Documentation
- README with full usage guide
- Architecture documentation
- Adapter integration guide
- Evaluation framework documentation
- Contributing guidelines
- Changelog

### Implementation Notes

v0.1.0 uses **custom, self-contained implementations** for all components:

| Component | Implementation |
|-----------|----------------|
| Repository Intelligence | Regex-based symbol extraction |
| Knowledge Hub | SQLite LIKE queries |
| Model Router | Heuristic scoring (no external API) |
| Execution Optimizer | Built-in compressors |
| Storage | SQLite with WAL mode |

This approach minimizes external dependencies and ensures the project builds and runs without Python/Node at runtime.

### Test Results

- **108 unit tests passing** across 11 crates
- **Zero compiler warnings**
- **Zero clippy warnings**
- **Zero security vulnerabilities** (cargo audit)
- **20 evaluation tests** (100% pass rate)

### Metrics

| Metric | Value |
|--------|-------|
| Crates | 11 |
| Lines of code | ~5,000+ |
| Test coverage | 108 tests |
| Build time | <30s (release) |
| Binary size | ~6MB |
| Startup time | <100ms |
| Indexing speed | ~300 files/sec |

### Known Limitations

1. **Regex-based extraction** — Misses nested structures and complex syntax
2. **SQLite LIKE queries** — Don't scale for large codebases
3. **Heuristic routing** — Can't route to multiple providers
4. **No cross-session memory** — Each session starts fresh
5. **No structural search** — Can't find similar code patterns

### Security

- Passed cargo audit with zero vulnerabilities
- No external network calls (except optional engram/LiteLLM)
- Input validation on all endpoints
- Timeout protection on all operations

[0.1.0]: https://github.com/leonortega/knocode/releases/tag/v0.1.0
