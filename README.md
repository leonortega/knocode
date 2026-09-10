# Knocode — AI Runtime

**Knocode is a local AI runtime that makes coding agents 37-67× faster at finding relevant code.** It runs as a local daemon, intercepting agent requests and enriching them with repository context — knowledge and code files — using a retrieval engine that understands *what you mean*, not just *what you typed*.

### Why This Matters

When you ask an AI coding agent "how to add error handling", it needs to find the right files in your codebase. Today, most agents use `grep` — a 1970s text-matching tool that finds literal string patterns. Knocode's retrieval engine replaces grep with **semantic search**: it understands intent, expands queries with synonyms, and finds files that grep completely misses.

```
Traditional (grep):                        Knocode:
  "how to add error handling"                "how to add error handling"
  → finds files with literal "error"         → finds error types, try/catch patterns,
  → misses documentation, test files,          documentation, test files, config,
    related components, config files            related components
  → 5.1 seconds on 53k files                 → 42ms on 53k files (37-67× faster at P50)
```

## Features

- **Retrieval Engine** — Semantic code search that replaces grep. Intent detection → query expansion → BM25 + structural search → graph boost → ranking. Finds files grep can't.
- **Repository Intelligence** — Incremental indexing: tree-sitter AST (**371 grammars** available via `tree-sitter-language-pack`; 42-language registry, 33 with parsers) + tantivy BM25 + structural search (ast-grep) + dependency graph. `mtime+size` shortcut for fast warm re-indexes.
- **Context Engine** — Assembles contextual information from your codebase for better AI responses (`BuildContext` — docs → code, 30s budget, fail-open).
- **Execution Optimization** — Tool-output compression is delegated to [RTK](https://github.com/rtk-ai/rtk) (external binary, opt-in via the installer); local token accounting via `tiktoken-rs`.
- **Event Bus** — Async in-memory observability with `tracing`/`metrics`/`correlation_id`.
- **Metrics** — Prometheus exposition at `GET /metrics`.
- **Fail-Open Design** — Always returns a response on hot path, never blocks the agent (30s timeout → `OriginalPassthrough`).
- **Two Auto-Index Modes** — `commit` (default, polls git HEAD) or `filesystem` (real-time via notify). Configurable via `[index].watch_mode`.

---

## Benchmark: Knocode vs Grep

We benchmarked our retrieval engine against `grep -rE` across three real-world codebases. The results: **Knocode is 37-67× faster than grep while finding semantically relevant files that grep completely misses.**

> Full report with methodology, per-query breakdowns, and component ablations:
> [docs/BENCHMARKS_V1.md](docs/BENCHMARKS_V1.md). A fresh validation run
> (2026-09-06, v0.9.11) is recorded at the top of that file: BuildContext 15.2ms
> mean (criterion), Recall@5 0.573 on the 50-task eval (dataset refreshed 2026-09-07).

### Speed *(historical run — see [BENCHMARKS_V1.md · Benchmark 1](docs/BENCHMARKS_V1.md#-benchmark-1-definitelytyped-53000-typescript-files) and [Benchmark 2](docs/BENCHMARKS_V1.md#-benchmark-2-mattermost-9000-go--react-files))*

| Codebase | Knocode (P50) | grep -rE (P50) | Speedup |
|----------|--------------|----------------|---------|
| Mattermost (9k files) | 7ms | 819ms | **67.2×** |
| DefinitelyTyped (53k files) | 42ms | 5,132ms | **36.5×** |
| Knocode repo (158 files) | 1ms | 110ms | **55.1×** |

At 7-42ms P50, Knocode is fast enough to run on every keystroke in an AI coding assistant. Grep's 5.1 seconds makes it unusable for real-time interaction.

### Quality *(historical run — details in [BENCHMARKS_V1.md](docs/BENCHMARKS_V1.md))*

| Codebase | Recall | Precision | Novelty | What novelty means |
|----------|--------|-----------|---------|-------------------|
| Mattermost (9k files) | 13.0% | 32.2% | 53.3% | Over half our results grep CAN'T find |
| DefinitelyTyped (53k files) | 17.3% | 9.3% | 38.6% | 39% of our results grep CAN'T find |

- **Recall** (13-17%): We find a curated subset of grep's results — the *best* files, not *all* files.
- **Precision** (9-32%): Our results are targeted to what the query actually needs.
- **Novelty** (39-53%): The magic — files that grep's pattern matching completely misses.

### What We Find That Grep Can't *(examples from the historical run — [BENCHMARKS_V1.md](docs/BENCHMARKS_V1.md))*

| Query | Grep Finds | Knocode Finds | Why |
|-------|-----------|---------------|-----|
| "how to add error handling" | Files with literal "error" | Error types, try/catch patterns, docs, tests | Semantic understanding of "error handling" |
| "find all API endpoints" | Files with literal "API" + "endpoint" | Route definitions, handler registrations, API docs | Understands "endpoints" means route handlers |
| "why does the auth fail" | Files with literal "auth" + "fail" | Auth middleware, session handling, permission checks | Understands "fail" means debugging context |

### Component Impact *(ablation on the historical run — [BENCHMARKS_V1.md · Benchmark 3](docs/BENCHMARKS_V1.md#-benchmark-3-component-evaluation-knocode-repo))*

| Component | Latency Cost | Recall Improvement | Verdict |
|-----------|-------------|-------------------|---------|
| Graph Boost | ~0ms | +0.0% | ⚠️ Neutral |
| Candidate K (50→500) | ~0ms | +81.3% | ✅ Strongly recommended |
| Query Expansion | ~0ms | +25.7% | ✅ Recommended |

---

## Install (end users)

Prebuilt Windows x64 binaries are published to every [GitHub Release](https://github.com/leonortega/knocode/releases).

**One-liner install** (GitHub Pages — auto-downloads the latest release):

```powershell
# Windows
powershell -ExecutionPolicy Bypass -c "irm https://leonortega.github.io/knocode/install.ps1 | iex"
```

```bash
# Linux / macOS
curl -fsSL https://leonortega.github.io/knocode/install.sh | bash
```

**Scoop** (Windows — per-user, no admin; installs Git as a dependency):

```powershell
scoop bucket add knocode https://github.com/leonortega/knocode
scoop install knocode
scoop update knocode
```

**Winget** (once merged in [microsoft/winget-pkgs](https://github.com/microsoft/winget-pkgs)):

```powershell
winget install Knocode.knocode
winget upgrade Knocode.knocode
```

**Direct download** — grab `knocode-<ver>-x86_64-pc-windows-msvc.zip` from the Release and unzip it; add the folder to your PATH.

### Agent integrations

The installers ask which agents to wire up — pick one or both of **OpenCode** and **Copilot (VS Code)** (default: all in the developer installers; none in the release one-liner).

```bash
# Developer installers (source checkout)
powershell -ExecutionPolicy Bypass -File scripts/install.ps1 -Agents opencode,copilot
bash scripts/install.sh --agents opencode,copilot
```

- **OpenCode** — npm plugin `opencode-knocode` (V2 prompt hook) + agent skill in `~/.config/opencode`
- **Copilot (VS Code)** — user-level agent hooks in `~/.copilot/hooks/knocode-context.json` (SessionStart + UserPromptSubmit); the same `knocode-mcp` server is also available for user-level `mcp.json`

The integration bundles (`opencode-knocode`, `knocode-mcp`) ship inside every release zip — no npm registry needed; they only require Node.js, which the installer installs automatically if missing (along with Git). Pass `-AllAgents`/`--all-agents` to skip the prompt, `-NoAgents`/`--no-agents` to wire nothing, or `-SkipPrereqs`/`--skip-prereqs` to disable automatic prerequisite installs. Agent configs are written idempotently (re-running updates them).

### Uninstall

```powershell
# Windows (one-liner, latest release)
powershell -ExecutionPolicy Bypass -c "irm https://github.com/leonortega/knocode/releases/latest/download/uninstall.ps1 | iex"
```

```bash
# Linux / macOS (one-liner, latest release)
curl -fsSL https://github.com/leonortega/knocode/releases/latest/download/uninstall.sh | bash -s -- --force
```

```bash
# Developer uninstallers (source checkout)
powershell -ExecutionPolicy Bypass -File scripts/uninstall.ps1 -Force  # add -RemoveRepo to also delete repo artifacts
bash scripts/uninstall.sh --force                                       # add --remove-repo to also delete repo plugin files
```

By default the uninstaller removes binaries, agent integrations (OpenCode plugin + skill, Copilot hooks + skill, RTK integrations), external tools and global data (`~/.knocode`). Pass `-KeepExternal`/`--keep-external` or `-KeepData`/`--keep-data` to preserve tools or data, `-DryRun`/`--dry-run` to preview.

## Quick Start

```bash
# 1. Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# 2. Build knocode
cargo build --release

# 3. Install to user bin
powershell -ExecutionPolicy Bypass -File scripts/install.ps1  # or: bash scripts/install.sh

# 4. Initialize your project
~/.knocode/bin/knocode init            # Unix
# or: %USERPROFILE%\.knocode\bin\knocode.exe init  # Windows

# 5. Index your repository
~/.knocode/bin/knocode index

# 6. Start the daemon
~/.knocode/bin/knocode serve
```

> **Agents**: always use the installed absolute path for `init`/`index`/`doctor`. Do **not** search `target/release` — the binary is already on PATH after install.

## Prerequisites

- **Rust 1.75+** — Install via [rustup](https://rustup.rs/)
- **SQLite** — Bundled via `rusqlite` (no system dependency needed)
- **Node.js** — For OpenCode plugin (optional)

## Build

```bash
cargo build                # Debug
cargo build --release      # Release (recommended)
cargo build -p knocode-core # Specific crate
```

## Test

```bash
cargo test                              # All tests (~400)
cargo test -p knocode-repo-intel       # Repo intelligence
cargo test -p knocode-context          # Context engine + benchmarks
cargo test -- --nocapture               # With output
```

## Lint

```bash
cargo clippy       # Additional lint checks
cargo fmt          # Format code
cargo fmt --check  # Check formatting
```

## Project Structure

```
knocode/
├── Cargo.toml                    # Workspace root
├── crates/
│   ├── knocode-core/             # Shared types, errors, config
│   ├── knocode-daemon/           # Daemon — HTTP/MCP server (POST /hook, POST /mcp, /health, /metrics)
│   ├── knocode-cli/              # CLI — init/index/serve/preview/status/config/doctor
│   ├── knocode-repo-intel/       # Repository Intelligence — tree-sitter + tantivy + graph + watcher
│   ├── knocode-context/          # Context Engine — retrieval engine + BuildContext
│   ├── knocode-knowledge/        # Knowledge Hub — SQLite+tantivy local BM25
│   ├── knocode-optimizer/        # RTK adapter helpers (doctor probe; compression itself is RTK's)
│   ├── knocode-events/           # Event Bus — in-memory broadcast + tracing
│   ├── knocode-storage/          # Local Storage — SQLite WAL + tantivy
│   └── knocode-bench/            # Criterion benchmarks
├── .knocode/
│   ├── config.toml               # Default configuration
│   └── skills/knocode/           # Agent-facing knocode skill (SKILL.md)
├── packages/
│   ├── knocode-client/           # Shared daemon client — single source of truth, vendored into the plugins
│   ├── opencode-knocode/         # OpenCode plugin (TypeScript, npm)
│   ├── knocode-mcp/              # stdio MCP pass-through proxy (Codex, Copilot, Claude)
│   ├── knocode-copilot-plugin/   # Copilot Agent Plugin (hooks + bundled MCP)
│   └── vscode-copilot-knocode/   # VS Code @knocode chat participant extension
├── .opencode/                    # OpenCode agent skill (in-repo copy)
├── .claude/hooks/                # Claude Code hooks (shell scripts)
└── docs/                         # Architecture, benchmarks, and specification docs
```

## CLI Commands

### `knocode init`

Initialize knocode for the current repository.

```bash
~/.knocode/bin/knocode init              # Unix
%USERPROFILE%\.knocode\bin\knocode.exe init  # Windows
```

Creates:
- `.knocode/` directory
- `.knocode/config.toml` with default configuration
- `.knocode/profile.json` with the repository profile
- SQLite database at `~/.knocode/data.db`

### `knocode index`

Index the repository for search and context building.

```bash
~/.knocode/bin/knocode index
```

Output:
```
✓ Indexing complete!

  Files indexed:    142
  Symbols extracted: 89
  Files skipped:    23
  Duration:         1234ms
```

### `knocode serve`

Start the daemon server (HTTP + MCP on `127.0.0.1:9527`).

```bash
knocode serve
knocode serve --port 9527
```

The daemon will:
1. Load configuration
2. Initialize logging + metrics (`/metrics` endpoint)
3. Open database (SQLite WAL) + tantivy index (MmapDirectory)
4. Run initial repository indexing (readiness-gated — `GET /health` reports `state: indexing` and `POST /hook` returns `503 daemon_indexing` until it completes)
5. Start auto-reindex watcher (commit/filesystem), then serve HTTP + MCP
6. Wait for shutdown signal (Ctrl+C)

#### Health & Readiness

Clients should wait until the daemon is ready before sending requests — during the initial index (and any auto-reindex) the engine lock is held, so `/hook` rejects fast instead of queueing:

- `GET /health` → `{"status": "ok", "version": "...", "state": "indexing" | "ready", "index_files": N}` — poll until `state` is `ready`
- `GET /metrics` → `knocode_daemon_ready 0|1` gauge (plus `knocode_index_files`)
- `POST /hook` → HTTP `503` with `reason: "daemon_indexing"` while not ready — retry with backoff
- HTTP `Probe` payload — `POST /hook` with `{"type":"Probe"}` → `{"type":"Probe","state":"ready","index_files":N,"version":"..."}` — same signal as `/health`; answered before rate-limiting with no engine lock
- **Bundled clients poll automatically** — the OpenCode plugin, `knocode-mcp`, and Claude Code hooks (`.claude/hooks/knocode-ready.sh`) each wait for `state: "ready"` before their first request (bounded + fail-open: an unreachable daemon bails immediately, a successful check is cached for 30s). Budget via `KNOCODE_READY_TIMEOUT_MS` (default 10000)

#### Daemon MCP (`POST /mcp`)

The daemon hosts an MCP (Model Context Protocol) surface on the same HTTP listener: JSON-RPC 2.0 at `POST /mcp`, identical on Windows and Unix, no extra socket or process. It is the "no-conversion" path for client plugins: typed tools in, natural text + structured metadata out — prompts and answers never get reshaped into internal wire payloads.

- **Methods**: `initialize`, `ping`, `tools/list`, `tools/call` (plus `notifications/initialized` → HTTP `202`). Stateless, tools-only subset — no sampling/prompts/resources, no batches.
- **`knocode_context(prompt, repository_path?)`** → the enriched context answer for a prompt (text + `provenance` in `structuredContent`). Same engine as the `/hook` rewrite — no message-shape conversion.
  *(Tool-output compression is not an MCP tool: RTK (github.com/rtk-ai/rtk) owns that layer — the knocode installer wires RTK's own integrations on request.)*
- **Readiness**: `tools/list`/`initialize`/`ping` always answer; `tools/call` while indexing returns JSON-RPC error `-32001 daemon_indexing` (HTTP stays `200`) — parity with the `/hook` 503 gate, so clients retry instead of queueing on the engine lock.
- **Clients**: the OpenCode plugin drives prompt enrichment through `knocode_context` (`chat.message`). Daemons that predate `/mcp` are still served via the automatic `/hook` fallback.

### `knocode preview <prompt>`

Preview what BuildContext would produce for a prompt.

```bash
knocode preview "implement a new API endpoint"
knocode preview "fix auth" --session my-sess --no-cache
```

Shows:
- Knowledge entries (BM25 local) that would be included
- Code files (ripgrep/tantivy/graph) that would be included
- Token budget by source

### `knocode status`

Show daemon status and metrics.

```bash
knocode status
```

### `knocode config show`

Display the effective configuration.

### `knocode config validate`

Validate the configuration file.

### `knocode config set-log-level`

Change log verbosity without re-running the installer. Upserts `[logging] level` in the
user config (`~/.config/knocode/config.toml`) and the project config
(`.knocode/config.toml`, when it has a `[logging]` section), and persists the user
`KNOCODE_LOG_LEVEL` env var exactly like the installer (HKCU Environment on Windows,
`export` in `~/.profile`/`~/.bashrc` on Unix) so agent plugins pick it up too. Accepts
`error`, `warn`, `info`, `debug`, `trace`, plus the installer's verbosity aliases
(`quiet`, `normal`, `verbose` = every daemon call). Setting `info` (the default)
removes a previously persisted env var instead of writing one, since env beats files.

```bash
knocode config set-log-level verbose   # = debug; daemon logs every MCP call
knocode config set-log-level info      # back to normal
```

### `knocode doctor`

Health check for all dependencies.

```bash
knocode doctor
```

Output:
```
Knocode Doctor (v1 — 8 probes)
═══════════════════════════════════════

SQLite:          ✓ OK (WAL, migrations up to date)
Config:          ✓ OK (token budget valid)
Knocode PATH:    ✓ OK (~/.knocode/bin on PATH)
Repo profile:    ✓ OK (.knocode/profile.json)
Tree-sitter:     ✓ OK (global 33/33 parsers ready)

  Repository detected languages
  ─────────────────────────────
    rust             214 files
    typescript       187 files
    ...

Tantivy:         ✓ OK (142 docs, ~/.knocode/index/<repo-id>)
Retrieval:       ✓ OK (3/3 probe queries returned results)
RTK:             ⚠ Not found on PATH — using built-in compressors + tee-on-failure (install rtk for 10ms binary)
Tiktoken:        ✓ OK (cl100k_base local, no model API round-trip)
Secrets redact:  ✓ OK (redaction before outbound calls)
Metrics:         ○ GET /metrics on daemon (prometheus exposition) — curl localhost:9527/metrics

✓ All critical checks passed
```

## Configuration

Configuration is loaded in order of priority (highest wins):

1. **Environment variables**: `KNOCODE_*`
2. **Project config**: `.knocode/config.toml`
3. **User config**: `~/.config/knocode/config.toml`
4. **Defaults**: Built-in defaults

### Configuration Sections

| Section | Purpose |
|---------|---------|
| `[database]` | SQLite path, max connections |
| `[index]` | BM25 index path, languages, `watch_mode` (`"commit"` or `"filesystem"`) |
| `[knowledge]` | Knowledge settings (`max_knowledge_entries`) |
| `[context]` | Token budget, file limits, `cache_order`, `candidate_k` |
| `[rtk]` | Enabled, max tokens, compression level |
| `[logging]` | Level, file path, retention |

### Environment Variables

| Variable | Overrides | Default |
|----------|-----------|---------|
| `KNOCODE_DATABASE_PATH` | database.path | ~/.knocode/data.db |
| `KNOCODE_LOG_LEVEL` | logging.level — daemon filter AND agent-plugin verbosity: `error`/`warn` = quiet (errors only), `info` = normal (default), `debug`/`trace` = verbose (log every daemon call). Set by the installer's log-verbosity prompt (0/1/2). | info |
| `KNOCODE_CONTEXT_MAX_TOKENS` | context.max_tokens | 12000 |
| `KNOCODE_CANDIDATE_K` | context.candidate_k | 100 |
| `KNOCODE_MAX_FILES` | context.max_files | 20 |
| `KNOCODE_WATCH_MODE` | index.watch_mode ("commit" or "filesystem") | commit |
| `KNOCODE_SYMBOLS_ENABLED` | Set to `false` to disable tree-sitter symbol extraction (BM25 only) | true |
| `KNOCODE_DAEMON_URL` | Daemon URL used by `knocode status`/`preview` and the JS clients | http://127.0.0.1:9527 |
| `KNOCODE_READY_TIMEOUT_MS` | Client-adapter readiness wait budget (poll `GET /health` before first request) | 10000 |

## Agent Integration

### OpenCode

1. Start the daemon: `knocode serve`
2. Install the npm plugin (`opencode-knocode`) + agent skill — done automatically by `scripts/install.ps1 -Agents opencode` (or `install.sh --agents opencode`)
3. Restart OpenCode

### Copilot (VS Code)

1. Start the daemon: `knocode serve`
2. User-level agent hooks are written to `~/.copilot/hooks/` by the installer (`-Agents copilot`)

### Claude Code

1. Start the daemon: `knocode serve`
2. Hooks live in `.claude/hooks/` (`knocode-pregeneration.sh`, `knocode-ready.sh`)
3. Make hooks executable: `chmod +x .claude/hooks/*.sh`
4. Restart Claude Code

## Architecture

### Component Overview

```
┌─────────────────────────────────────────────────────────────┐
│                      Coding Agent                           │
│  (OpenCode, Copilot, Claude Code, etc.)                     │
└─────────────────────────┬───────────────────────────────────┘
                           │ HTTP JSON — POST /hook, POST /mcp
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                    HTTP API Layer                           │
│  • Request validation + rate-limit (token bucket)           │
│  • Fail-open (30s → OriginalPassthrough)                    │
│  • Prometheus /metrics                                      │
└─────────────────────────┬───────────────────────────────────┘
                           │
           ┌───────────────┼───────────────┐
           ▼               ▼               ▼
┌─────────────┐  ┌─────────────┐  ┌─────────────┐
│   Context   │  │  Execution  │  │    Event    │
│   Engine    │  │  Optimizer  │  │     Bus     │
│ (RwLock)    │  │ (RTK)       │  │ (in-memory) │
└──────┬──────┘  └─────────────┘  └─────────────┘
        │
        ├──► Retrieval Engine (intent → expansion → BM25 + structural → graph → ranking)
        ├──► Repository Intelligence (tree-sitter + tantivy + graph + watcher)
        └──► Knowledge Hub (tantivy local)
```

### Request Flow

1. Agent sends an HTTP JSON request to `POST /hook` (or MCP `tools/call` via `POST /mcp`)
2. HTTP layer validates input, rate-limits, generates correlation ID
3. Context Engine assembles context pack (`RwLock` read, concurrent sessions):
   - **Retrieval Engine** finds relevant code files (intent → expansion → BM25 + structural → graph → ranking)
   - Knowledge Hub retrieves entries (BM25 local)
   - Orders: `docs_context` → `code_context`
   - Reversible truncation
4. Response returned (or `OriginalPassthrough` on error/timeout), metrics recorded

## HTTP API

The daemon exposes a single HTTP listener (default `127.0.0.1:9527`): `POST /hook` for prompt enrichment, `POST /mcp` for MCP clients, `GET /health` and `GET /metrics` for observability. There is no socket/MessagePack transport.

### Request Format (`POST /hook`)

```json
{
  "hook_type": "PreGeneration",
  "payload": {
    "type": "MessageRewrite",
    "session_id": "test",
    "message": "fix a typo in README"
  }
}
```

### Response Format

```json
{
  "correlation_id": "req_abc123",
  "hook_type": "PreGeneration",
  "payload": {
    "type": "RewrittenMessage",
    "original": "fix a typo in README",
    "rewritten": "fix a typo in README\n\n---\n\nContext:\n..."
  },
  "latency_ms": 100,
  "error": null
}
```

### Fail-Open Behavior

On any error or timeout, the daemon returns `OriginalPassthrough` with the original message unchanged. The agent always gets a response.

| Condition | Response | Reason |
|-----------|----------|--------|
| Timeout (>30s) | OriginalPassthrough | "timeout" |
| Context build error | OriginalPassthrough | "error" |
| Any internal error | OriginalPassthrough | "fail-open" |

## Implementation Status (v0.9.11)

| Component | Status | Notes |
|-----------|--------|-------|
| Retrieval Engine | ✅ Complete | Intent → expansion → BM25 + structural → graph → ranking |
| Repository Intelligence | ✅ Complete | tree-sitter (42-lang registry) + tantivy + graph + watcher |
| Context Engine | ✅ Complete | BuildContext + RwLock concurrency, fail-open |
| Knowledge Hub | ✅ Complete | BM25 local (SQLite+tantivy) |
| Execution Optimizer | ✅ Complete | RTK (external, opt-in) + built-in compressor fallback + tiktoken-rs |
| Adapter Layer | ✅ Complete | HTTP `/hook` + `/mcp`, rate limiting, 30s fail-open |
| CLI Commands | ✅ Complete | init, index, serve, preview, doctor, config |
| Agent Adapters | ✅ Complete | OpenCode, Copilot (VS Code), Claude Code hooks |
| Metrics | ✅ Complete | Prometheus /metrics exposition |
| Benchmarks | ✅ Complete | Component Eval, Mattermost (9k), DefinitelyTyped (53k) |

### External Tool Integration

| Tool | Purpose | Status |
|------|---------|--------|
| tree-sitter | AST parsing (371 languages via tree-sitter-language-pack) | ✅ |
| tantivy | BM25 full-text search (MmapDirectory) | ✅ |
| ast-grep | Structural code search | ✅ |
| tiktoken-rs | Token counting (cl100k_base) | ✅ |
| RTK | Tool-output compression (external binary, opt-in via installer) | ⚠ optional |
| Prometheus | /metrics exposition | ✅ |

## Development

### Adding a New Crate

1. Create `crates/knocode-<name>/Cargo.toml`
2. Add to workspace `Cargo.toml` members
3. Add shared dependencies to `[workspace.dependencies]`
4. Create `src/lib.rs` with module code
5. Add tests in `#[cfg(test)] mod tests`

### Running Specific Tests

```bash
cargo test -p knocode-core
cargo test test_config_load
cargo test -- --nocapture
```

## Roadmap

See [docs/ROADMAP.md](docs/ROADMAP.md) for the full release history and future plans. Benchmarks: [docs/BENCHMARKS_V1.md](docs/BENCHMARKS_V1.md) (includes the 2026-09-06 fresh validation run).

## License

MIT
