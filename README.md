# Knocode — AI Runtime

**Knocode is a local AI runtime that makes coding agents 20-27× faster at finding relevant code.** It runs as a local daemon, intercepting agent requests and enriching them with repository context — knowledge and code files — using a retrieval engine that understands *what you mean*, not just *what you typed*.

### Why This Matters

When you ask an AI coding agent "how to add error handling", it needs to find the right files in your codebase. Today, most agents use `grep` — a 1970s text-matching tool that finds literal string patterns. Knocode's retrieval engine replaces grep with **semantic search**: it understands intent, expands queries with synonyms, and finds files that grep completely misses.

```
Traditional (grep):                        Knocode:
  "how to add error handling"                "how to add error handling"
  → finds files with literal "error"         → finds error types, try/catch patterns,
  → misses documentation, test files,          documentation, test files, config,
    related components, config files            related components
  → 4.8 seconds on 53k files                 → 45ms on 53k files (106× faster at P50)
```

## Features

- **Retrieval Engine** — Semantic code search that replaces grep. Intent detection → query expansion → BM25 + structural search → graph boost → ranking. Finds files grep can't.
- **Repository Intelligence** — Incremental indexing: tree-sitter AST (**111 languages**) + tantivy BM25 + structural search (ast-grep) + dependency graph. `mtime+size` shortcut for fast warm re-indexes.
- **Context Engine** — Assembles contextual information from your codebase for better AI responses (`BuildContext` — docs → code, 30s budget, fail-open).
- **Execution Optimizer** — RTK adapter + built-in compressors + `tiktoken-rs` savings reporting.
- **Event Bus** — Async in-memory observability with `tracing`/`metrics`/`correlation_id`.
- **Metrics** — Prometheus exposition at `GET /metrics`, Grafana dashboard.
- **Fail-Open Design** — Always returns a response on hot path, never blocks the agent (30s timeout → `OriginalPassthrough`).
- **Two Auto-Index Modes** — `commit` (default, polls git HEAD) or `filesystem` (real-time via notify). Configurable via `[index].watch_mode`.

---

## Benchmark: Knocode vs Grep

We benchmarked our retrieval engine against `grep -rE` across three real-world codebases. The results: **Knocode is 27-106× faster than grep while finding semantically relevant files that grep completely misses.**

### Speed

| Codebase | Knocode (P50) | grep -rE (P50) | Speedup |
|----------|--------------|----------------|---------|
| Mattermost (9k files) | 27ms | 971ms | **27×** |
| DefinitelyTyped (53k files) | 49ms | 4,836ms | **106×** |
| Knocode repo (158 files) | ~10ms | ~20ms | 2× |

At 27-49ms, Knocode is fast enough to run on every keystroke in an AI coding assistant. Grep's 4.8 seconds makes it unusable for real-time interaction.

### Quality

| Codebase | Recall | Precision | Novelty | What novelty means |
|----------|--------|-----------|---------|-------------------|
| Mattermost (9k files) | 13.1% | 32.8% | 53.0% | Half our results grep CAN'T find |
| DefinitelyTyped (53k files) | 14.2% | 1.6% | 89.2% | 89% of our results grep CAN'T find |

- **Recall** (13-14%): We find a curated subset of grep's results — the *best* files, not *all* files.
- **Precision** (2-33%): Our results are targeted to what the query actually needs.
- **Novelty** (53-89%): The magic — files that grep's pattern matching completely misses.

### What We Find That Grep Can't

| Query | Grep Finds | Knocode Finds | Why |
|-------|-----------|---------------|-----|
| "how to add error handling" | Files with literal "error" | Error types, try/catch patterns, docs, tests | Semantic understanding of "error handling" |
| "find all API endpoints" | Files with literal "API" + "endpoint" | Route definitions, handler registrations, API docs | Understands "endpoints" means route handlers |
| "why does the auth fail" | Files with literal "auth" + "fail" | Auth middleware, session handling, permission checks | Understands "fail" means debugging context |

### Component Impact

| Component | Latency Cost | Recall Improvement | Verdict |
|-----------|-------------|-------------------|---------|
| Graph Boost | -2ms | +0.0% | ⚠️ Neutral |
| Candidate K (50→500) | +3ms | +249% | ✅ Strongly recommended |
| Query Expansion | +3ms | +6.5-18% | ✅ Recommended |

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

The installers ask which agents to wire up — pick one or more of **OpenCode**, **Codex**, **Copilot (VS Code)**, and **Cursor** (default: all in the developer installers; none in the release one-liner).

```powershell
# Windows one-liner with agent selection
powershell -ExecutionPolicy Bypass -c "irm https://leonortega.github.io/knocode/install.ps1 | iex" -Agents opencode,codex
```

```bash
# Developer installers (source checkout)
powershell -ExecutionPolicy Bypass -File scripts/install.ps1 -Agents opencode,codex
bash scripts/install.sh --agents opencode,cursor
```

- **OpenCode** — plugin `opencode-knocode` + agent skill in `~/.config/opencode`
- **Codex** — MCP server `knocode-mcp` in `~/.codex/config.toml`
- **Copilot / Cursor** — the same `knocode-mcp` server in the VS Code / Cursor user-level `mcp.json`

The integration bundles (`opencode-knocode`, `knocode-mcp`) ship inside every release zip — no npm registry needed; they only require Node.js, which the installer installs automatically if missing (along with Git). Pass `-AllAgents`/`--all-agents` to skip the prompt, `-NoAgents`/`--no-agents` to wire nothing, or `-SkipPrereqs`/`--skip-prereqs` to disable automatic prerequisite installs. Agent configs are written idempotently (re-running updates them).

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
│   ├── knocode-daemon/           # Daemon — UDS/MessagePack + HTTP fallback + /metrics
│   ├── knocode-cli/              # CLI — init/index/serve/preview/doctor
│   ├── knocode-repo-intel/       # Repository Intelligence — tree-sitter + tantivy + graph + watcher
│   ├── knocode-context/          # Context Engine — retrieval engine + BuildContext
│   ├── knocode-knowledge/        # Knowledge Hub — SQLite+tantivy local BM25
│   ├── knocode-optimizer/        # Execution Optimizer — RTK adapter + compressors
│   ├── knocode-events/           # Event Bus — in-memory broadcast + tracing
│   ├── knocode-storage/          # Local Storage — SQLite WAL + tantivy
├── .knocode/
│   └── config.toml               # Default configuration
├── adapters/
│   ├── cursor/extension.ts       # Cursor integration
│   ├── gemini/hooks.sh           # Gemini CLI integration
│   └── tier2/README.md           # Best-effort adapters
├── .opencode/plugins/            # OpenCode plugin (TypeScript)
├── .claude/hooks/                # Claude Code hooks (shell scripts)
├── benches/                      # criterion benchmarks
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

Start the daemon server (UDS primary on `/tmp/knocode.sock` + HTTP fallback on `127.0.0.1:9527`).

```bash
knocode serve
knocode serve --socket /tmp/knocode.sock --port 9527
```

The daemon will:
1. Load configuration
2. Initialize logging + metrics (`/metrics` endpoint)
3. Open database (SQLite WAL) + tantivy index (MmapDirectory)
4. Run initial repository indexing (readiness-gated — `GET /health` reports `state: indexing` and `POST /hook` returns `503 daemon_indexing` until it completes; the UDS/MessagePack adapter binds only after)
5. Start auto-reindex watcher (commit/filesystem), then UDS+MessagePack primary and HTTP fallback
6. Wait for shutdown signal (Ctrl+C)

#### Health & Readiness

Clients should wait until the daemon is ready before sending requests — during the initial index (and any auto-reindex) the engine lock is held, so `/hook` rejects fast instead of queueing:

- `GET /health` → `{"status": "ok", "version": "...", "state": "indexing" | "ready", "index_files": N}` — poll until `state` is `ready`
- `GET /metrics` → `knocode_daemon_ready 0|1` gauge (plus `knocode_index_files`)
- `POST /hook` → HTTP `503` with `reason: "daemon_indexing"` while not ready — retry with backoff
- UDS/MessagePack `Probe` payload (`{"type":"Probe"}`) → `{"type":"Probe","state":"ready","index_files":N,"version":"..."}` — same signal as `/health` over the primary transport; answered before rate-limiting with no engine lock
- **Bundled adapters poll automatically** — the OpenCode plugin, Claude Code hooks (`.claude/hooks/knocode-ready.sh`), Gemini CLI hooks, and Cursor extension each wait for `state: "ready"` before their first request (bounded + fail-open: an unreachable daemon bails immediately, a successful check is cached for 30s). Budget via `KNOCODE_READY_TIMEOUT_MS` (default 10000)

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

### `knocode doctor`

Health check for all dependencies.

```bash
knocode doctor
```

Output:
```
Knocode Doctor
═══════════════════════════════════════

SQLite:          ✓ OK (WAL, migrations up to date)
Config:          ✓ OK (4 default langs, 111 available via arborium)
Socket path:     ✓ OK (/tmp/knocode.sock)
Tree-sitter:     ✓ OK (111 languages via arborium)
Tantivy:         ✓ OK (MmapDirectory)
Knowledge Hub:   ✓ OK (SQLite+tantivy local)
RTK:             ⚠ Not found — using built-in compressors
Tiktoken:        ✓ OK (cl100k_base LazyLock)
Secrets redact:  ✓ OK (HMAC via hmac crate)
Retrieval:       ✓ OK (tantivy BM25 + ast-grep structural)
Metrics:         ○ GET /metrics on daemon

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
| `[daemon]` | Socket path, concurrency, timeout, `metrics_port`, `rate_limit_per_session` |
| `[database]` | SQLite path, connection pool |
| `[index]` | Tantivy path, languages, `watch_mode` (`"commit"` or `"filesystem"`) |
| `[retrieval]` | Retrieval engine settings (`candidate_k`, `max_files`, `enable_graph`, `enable_expansion`) |
| `[knowledge]` | Knowledge settings (`max_knowledge_entries`) |
| `[context]` | Token budget, file limits, `cache_order` |
| `[rtk]` | Enabled, max tokens, compression level |
| `[logging]` | Level, file path, retention |

### Environment Variables

| Variable | Overrides | Default |
|----------|-----------|---------|
| `KNOCODE_DAEMON_SOCKET` | daemon.socket_path | /tmp/knocode.sock |
| `KNOCODE_DATABASE_PATH` | database.path | ~/.knocode/data.db |
| `KNOCODE_LOG_LEVEL` | logging.level | info |
| `KNOCODE_CONTEXT_MAX_TOKENS` | context.max_tokens | 12000 |
| `KNOCODE_CANDIDATE_K` | retrieval.candidate_k | 100 |
| `KNOCODE_WATCH_MODE` | index.watch_mode ("commit" or "filesystem") | commit |
| `KNOCODE_SYMBOLS_ENABLED` | Enable/disable tree-sitter symbol extraction | true |
| `KNOCODE_READY_TIMEOUT_MS` | Client-adapter readiness wait budget (poll `GET /health` before first request) | 10000 |

## Agent Integration

### OpenCode

1. Start the daemon: `knocode serve`
2. Copy the plugin: `cp .opencode/plugins/knocode.ts .opencode/plugins/`
3. Restart OpenCode

### Claude Code

1. Start the daemon: `knocode serve`
2. Hooks are configured in `.claude/settings.json`
3. Make hooks executable: `chmod +x .claude/hooks/*.sh`
4. Restart Claude Code

### Cursor

1. Start the daemon: `knocode serve`
2. Install extension from `adapters/cursor/extension.ts`
3. Extension calls `POST /hook` (UDS/MessagePack primary, HTTP fallback, 30s fail-open)

### Gemini CLI

1. Start the daemon: `knocode serve`
2. Hooks at `adapters/gemini/hooks.sh`
3. `chmod +x adapters/gemini/hooks.sh`

## Architecture

### Component Overview

```
┌─────────────────────────────────────────────────────────────┐
│                      Coding Agent                           │
│  (opencode, Claude Code, Cursor, Gemini CLI, etc.)          │
└─────────────────────────┬───────────────────────────────────┘
                           │ UDS/MessagePack primary, HTTP fallback
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                    Adapter Layer                            │
│  • Request validation + rate-limit (per session)            │
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
        ├──► Repository Intelligence (tree-sitter 111 langs + tantivy + graph + watcher)
        └──► Knowledge Hub (tantivy local)
```

### Request Flow

1. Agent sends request via UDS/MessagePack (HTTP/JSON fallback on Windows)
2. Adapter Layer validates, rate-limits (per `session_id`), generates correlation ID
3. Context Engine assembles context pack (`RwLock` read, concurrent sessions):
   - **Retrieval Engine** finds relevant code files (intent → expansion → BM25 + structural → graph → ranking)
   - Knowledge Hub retrieves entries (BM25 local)
   - Orders: `docs_context` → `code_context`
   - Reversible truncation
4. Response returned (or `OriginalPassthrough` on error/timeout), metrics recorded

## IPC Protocol

### Primary: UDS + MessagePack

`rmp-serde` encode of `AgentRequest` → `4-byte BE len` → body. HTTP/JSON `POST /hook` as fallback.

### Request Format (JSON fallback)

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

### Response Format (JSON fallback)

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
| Repository Intelligence | ✅ Complete | tree-sitter 111 langs + tantivy + graph + watcher |
| Context Engine | ✅ Complete | BuildContext + RwLock concurrency, fail-open |
| Knowledge Hub | ✅ Complete | BM25 local (SQLite+tantivy) |
| Execution Optimizer | ✅ Complete | RTK adapter + compressors + tiktoken-rs |
| Adapter Layer | ✅ Complete | UDS/MessagePack primary + HTTP fallback, 30s fail-open |
| CLI Commands | ✅ Complete | init, index, serve, preview, doctor, config |
| Agent Adapters | ✅ Complete | OpenCode, Cursor, Gemini CLI |
| Metrics | ✅ Complete | Prometheus /metrics + Grafana dashboard |
| Benchmarks | ✅ Complete | Component Eval, Mattermost (9k), DefinitelyTyped (53k) |

### External Tool Integration

| Tool | Purpose | Status |
|------|---------|--------|
| tree-sitter | AST parsing (111 languages via arborium) | ✅ |
| tantivy | BM25 full-text search (MmapDirectory) | ✅ |
| ast-grep | Structural code search | ✅ |
| tiktoken-rs | Token counting (cl100k_base) | ✅ |
| RTK | Tool-output compression | ✅ |
| Prometheus | /metrics exposition + Grafana | ✅ |

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

See [docs/ROADMAP.md](docs/ROADMAP.md) for the full release history and future plans. Benchmarks: [docs/BENCHMARKS_V1.md](docs/BENCHMARKS_V1.md).

## License

MIT
