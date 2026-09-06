# Removed Tools & Components — Decision Index

Status: **active, authoritative** — supersedes scattered removal notes in ROADMAP/CHANGELOG as the
single index of what was removed from the runtime, when, and why. Each formal removal links its
dedicated ADR where one exists. Code is the source of truth for what exists today; this document
explains why the rest is gone.

## Quick Reference

| Tool / component | Removed (version) | What it was | Why removed (one line) | ADR / evidence |
|---|---|---|---|---|
| LLM Model Router + LiteLLM | v0.8.6 | Heuristic tier routing (`capable→balanced→fast`) with a LiteLLM HTTP fallback gateway; whole `knocode-router` crate | Routing couldn't be shown to beat the agent/provider/user choice, and model capabilities move too fast to encode | — |
| FlashRank reranker (`ort` int8) | v0.7.6 | Second-stage retrieval reranker, TF-IDF fallback on model-load failure | Measured **MRR degradation** on benchmark eval — ranking got worse, not better | — |
| Engram (cross-session memory) | v0.7.6 | Single Go binary, SQLite+FTS5 memory with MCP-native HTTP/CLI API | Zero recall gain; another external binary to install | — |
| codebase-memory-mcp (graph probe) | v0.7.6 | Node dependency-graph extractor probed via `npx` as a first-class graph source | **0pp R@5 gain** over local AST+regex; fragile Node/npm dependency | — |
| MkDocs integration | v0.9.0 | Ingested docs through the MkDocs build/ingestion path | Docs are plain Markdown — indexed via a first-class path instead | — |
| DBOS workflow engine | v0.9.0 (code cleanup now) | Workflow/orchestration dependency (v0.6.0 made it "required") | Not required for V1; runtime is a single tokio daemon | — |
| `knocode replay` CLI | v0.9.0 | Event-replay command over the event log | Replay off the hot path; `tracing` + metrics retained | — |
| Skill Engine (runtime) | **current change** | `knocode-skills` crate: tag-based skill matching + full-instruction injection (`.knocode/skills`, `~/.knocode/skills`) | Agents already have native skill discovery (`.claude/skills`, `.agents/skills`, `.cursor/rules`); a second discovery+conflict system added complexity with no demonstrated outcome lift | V1_RUNTIME_SPEC.md §2 |
| Model map / tier config | **current change** | `ModelConfig { default_tier, routing_enabled }`, "fast/balanced/capable" validation, `KNOCODE_MODEL_DEFAULT`, metrics request counter keyed `hook+tier`, `token_usage.model`/`tier` columns | Vestigial after the Model Router removal — nothing reads a tier anymore | — |
| Built-in tool-output compression (Execution Optimizer, MCP `knocode_compress`, `/hook` `ToolOutput`) | **current change** | `ExecutionOptimizer` daemon state, `/hook` `ToolOutput`/`CompressedOutput` contract, MCP `knocode_compress` tool, `tokens_saved` metric, `.claude/hooks/knocode-pretool.sh` | Compression is RTK's job — the installer wires RTK's own integrations (`rtk init`) instead of the daemon reimplementing it | — |

## Why things were removed — the reasoning

A component is removed when it fails one of two tests:

1. **No demonstrated value.** Knocode's bar for keeping a capability is a *measured* improvement
   (recall, MRR, latency, tokens). FlashRank, engram, and codebase-memory-mcp were all evaluated
   head-to-head against the local baseline and lost — see the benchmark numbers below.
2. **The runtime shouldn't own the problem.** Model selection belongs to the agent/provider/user;
   skill discovery belongs to the agent's own ecosystem. Capabilities an agent "consistently benefits
   from but shouldn't have to implement" (retrieval, context, observability, compression) stay in the
   runtime; everything else is out of scope regardless of how attractive it looks architecturally.

### LLM Model Router + LiteLLM (v0.8.6)

`task → model selection` sounds valuable but the runtime could not be shown to choose better than the
agent, the provider, the user, or a configured model — and model capabilities change extremely quickly.
Encoding them in the daemon would have made the runtime a moving target. The router crate was deleted
(~900 lines, one fewer workspace crate), `[model]`/`[routing]`/`[litellm]` config and env were removed,
and `doctor` stopped probing `LITELLM_URL`. BuildContext is deterministic retrieval only — the daemon
makes **zero external LLM calls**. The MCP surface is retained (it is an interface, not a model
decision).

### FlashRank reranker (v0.7.6)

Benchmark evidence  showed the reranker *degraded* results: adding FlashRank
was worse than the BM25 + symbol/path baseline on the metrics that matter (MRR). The `ort` int8 model
was replaced by a passthrough, then the passthrough scaffolding itself was removed (current change) —
`rerank_enabled` and the `rerank.rs` module are gone from `knocode-knowledge`.

### Engram + codebase-memory-mcp (v0.7.6)

Both were external-process integrations (a Go binary and a Node CLI via `npx`) that contradicted the
Local-First in-process stack. Benchmarks measured **zero** gain: `+ codebase-memory-mcp` = 0pp R@5 over
BM25+symbol/path, and engram reads added nothing over SQLite+tantivy local. Cross-session memory is now
fully local (SQLite + tantivy).

### DBOS workflow (v0.9.0 → current cleanup)

The workflow engine was isolated out of the runtime in v0.9.0 (v1 is a single tokio daemon). With the
daemon's `workflow` cargo feature, its cfg-gated HTTP routes, and the archived `crates/knocode-workflow`
crate now deleted, no DBOS code remains anywhere in the repository. (The feature had been default-off;
nothing depended on it.)

### Skill Engine (current change)

Skills were first demoted in v0.8.0 to an optional, opt-in integration, then removed entirely because
the runtime should not own the concept:

- Modern agents already discover skills natively (`.claude/skills`, `.agents/skills`, `.cursor/rules`).
  A parallel Knocode skill-selection system (its own discovery + conflict resolution + priority rules)
  is pure added complexity on top of that.
- No evidence that runtime skill matching improves outcomes over the agent's own tooling — unlike
  retrieval and context, which are measured.
- Removal is complete: the `knocode-skills` crate, `ContextPack.behavioral_skills`, KnowledgeHub skill
  load/match, the daemon's skill loading, the CLI `skills` subcommand + `--community-skills` install,
  `[skills]` config, and the bundled skill library (`.knocode/skills`, `~/.knocode/skills`) are all
  gone. Context packs now carry docs + code context only.
- **Retained — the agent-facing `knocode` skill** (`.knocode/skills/knocode/SKILL.md`). Skills live in
  the agent's own ecosystem, and knocode ships one skill that teaches the agent how to use the
  runtime (binary location, init/doctor, MCP tools). The installers copy it to the agent's global
  skills directory (`~/.config/opencode/skills/knocode/`) per-agent — opencode today, others as
  adapters land — and the uninstallers remove that copy. This is an agent-native integration, not a
  runtime-owned concept: the runtime never matches or injects skills itself.

### Model map / tier config (current change)

After the Model Router removal nothing read `default_tier` / `routing_enabled`, yet the config struct,
env var, validation, a hard-coded `"balanced"` tier in the metrics request counter, and
`token_usage.model`/`tier` columns survived. This change deletes the model map entirely:

- `ModelConfig` and `Config.model` removed from `knocode-core` config (with `[model]`/`[routing]`/
  `[litellm]` TOML samples and `KNOCODE_MODEL_DEFAULT`).
- Metrics request counter keyed by hook type only (`knocode_requests_total`), no tier dimension.
- Migration `007` drops the vestigial `token_usage.model` / `token_usage.tier` columns.

## Never shipped (scoped out in planning, not deleted code)

The following were removed from the *architecture* during V1 planning and never existed as runtime
code: Capability Scheduler, Mission Manager, standalone Workflow Engine (beyond the DBOS removal
above), durable Event Store as a product surface, autonomous SDLC, multi-agent orchestration, and
Skill Engine as a V1 primitive. See `V1_RUNTIME_SPEC.md`.

## What remains

| Surface | Status |
|---|---|
| Repository Intelligence (tree-sitter + tantivy + graph + watcher) | ✅ core |
| Retrieval + Context Engine (deterministic, local) | ✅ core |
| Knowledge (SQLite + tantivy local; docs + code context) | ✅ core |
| Tool-output compression | ❌ delegated to RTK (external binary, wired by installers) |
| Observability (`/metrics`, `/health`, trace logs) | ✅ core |
| Daemon IPC: HTTP `/hook` + **MCP `POST /mcp`** | ✅ core |
| Readiness (`/health`, `/metrics` gauge, HTTP probe, MCP `-32001`) | ✅ core |
| Agent adapters (opencode plugin via MCP, Claude/Gemini/Cursor hooks) | ✅ |
| MCP server package (`packages/knocode-mcp`, for Codex/Copilot/others) | ✅ |

## Re-entry rule

A removed component may return only with a **demonstrated, measured improvement** (the same bar that
removed Model Router, FlashRank, and codebase-memory-mcp) — not because it looks architecturally
attractive. Any re-introduction needs a new ADR with benchmark evidence, per `V1_RUNTIME_SPEC.md` §6.
