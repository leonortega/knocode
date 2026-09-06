# AI Runtime V1 Specification

> **Status:** Canonical V1 definition. Supersedes the V1 claims in
> `ARCHITECTURE.md`, `ROADMAP.md`, `RUNTIME.md`, `REQUEST_LIFECYCLE.md`, and
> `COMPONENTS.md` **where they conflict with this document** (primarily: model
> routing, skills, LiteLLM, and the durable event store as a product surface).
> Those files remain as historical / implementation detail.
> Where this spec and the code disagree, **the code is the source of truth** —
> file a bug if you find a conflict.

## 1. Product Definition

The **AI Runtime** is the product: a local layer between coding agents and their
models/providers that provides capabilities agents consistently benefit from but
should not have to implement independently.

**Knocode is a subsystem** of the Runtime — its repository/context intelligence
— not the whole product. This keeps the Runtime model-agnostic and agent-agnostic
and leaves room for capabilities beyond Knocode to be added later without turning
Knocode into a monolith.

The Runtime **improves existing coding agents**. It does not replace them, and it
is not an autonomous SDLC orchestrator.

### V1 Pillars

| Pillar | Purpose | Confidence | Primary crates |
|--------|---------|------------|----------------|
| **Observability** | See what the agent + runtime are doing (traces, metrics, costs) | 🟢🟢 | `knocode-daemon` (metrics), `knocode-events`, `knocode-storage` |
| **Intelligence** | Understand the repository and build token-efficient context | 🟢🟢 | `knocode-repo-intel`, `knocode-context`, `knocode-knowledge` |
| **Execution Optimization** | Reduce useless tool output (token waste) | 🟢 | `knocode-optimizer` |
| **Local Runtime** | Fast, fail-open infrastructure (IPC, lifecycle, readiness) | 🟢🟢 | `knocode-daemon`, `knocode-core`, `knocode-cli` |

## 2. Ownership Boundaries

### 2.1 What the Runtime owns

- The **Runtime API** (UDS/MessagePack primary, HTTP fallback), the daemon
  lifecycle, and fail-open guarantees.
- **Readiness**: `GET /health` (`state: indexing|ready`), `GET /metrics`
  (`knocode_daemon_ready`), HTTP `503 daemon_indexing`, and the UDS `Probe`
  payload. Clients wait on readiness before sending requests.
- **Observability**: per-request traces, metrics, and (V1 gap) cost attribution.
- **Execution optimization**: tool-output compression/filtering.
- The **Intelligence pillar**, implemented by Knocode (§2.2).

### 2.2 What Knocode owns (the Intelligence subsystem)

- **Repository Intelligence** — index → symbols → structural understanding →
  retrieval → dependency relationships. Incremental indexing (tree-sitter +
  tantivy BM25 + ast-grep structural + dependency graph), auto-reindex watcher
  (`commit` / `filesystem` modes), cached-reader invalidation so queries serve
  fresh commits immediately.
- **Retrieval** — intent detection → query expansion → BM25 + structural search →
  graph boost → ranking. The runtime answers: *"given what the agent is doing,
  what repository information is worth giving it?"* — different from exposing grep.
- **Context Construction** — token-efficient assembly (docs → code, frozen-prefix
  boundary, session fingerprint dedup, reversible truncation).
- **Knowledge** — local SQLite+tantivy knowledge, repository profile, ADR/docs.

### 2.3 What the agent owns

- Task interpretation and conversation policy.
- **Model / provider selection** (the Runtime is model-agnostic; it does not route).
- Tool execution and its own **skill discovery/selection** mechanisms
  (`.claude/skills`, `.agents/skills`, `.cursor/rules`, ...).

### 2.4 Explicitly out of scope for V1

| Capability | Status in this repo | Rationale |
|------------|--------------------|-----------|
| Skill Engine | Removed (see `REMOVED_TOOLS.md`) | Modern agents have their own discovery; a second selection system adds complexity without demonstrated outcome gains. Runtime does not *own* the skill concept. |
| Model Router | Removed (v0.8.6; `knocode-router` no longer in the workspace) | Cannot prove the runtime consistently beats agent/provider/user choice; capabilities change fast. Runtime stays model-agnostic. |
| Workflow Engine | Removed (`knocode-workflow` no longer in the workspace) | Not required to make the Runtime valuable. |
| Durable Event Store as a product surface | In-memory ring buffer only (`knocode-events`) | Traces/events remain — they are the observability substrate — but persistence/replay is not a V1 product feature. |
| Capability Scheduler / Mission Manager | Never built | Out of scope. |
| Autonomous SDLC / multi-agent orchestration | Never built | Contradicts the V1 constraint: improve agents, don't replace them. |

## 3. Pillar Specifications

### 3.1 Observability — agent-trace-first

Today the repo is **metrics-first** (Prometheus exposition: requests, build
duration histogram, fail-open count, indexed files, tokens saved, context tokens,
retrieval recall, daemon readiness) plus in-memory events and `tracing` with
`correlation_id`. V1 elevates this to a **per-request trace model**:

```
Agent
 ├── tool call
 │      ├── command
 │      ├── duration
 │      ├── output tokens
 │      └── compressed tokens
 ├── repository retrieval
 │      ├── query
 │      ├── candidates
 │      ├── selected files
 │      ├── latency
 │      └── context tokens
 ├── context construction
 │      ├── sources
 │      ├── budget
 │      ├── truncation
 │      └── final tokens
 └── model interaction
        ├── model
        ├── input tokens
        ├── output tokens
        ├── latency
        └── cost
```

V1 trace questions the Runtime must answer:
- Why did this agent consume N tokens? (per-request breakdown by source)
- Which tools are producing useless output?
- Which repository searches are slow?
- How much context did Knocode actually add, and did retrieval change the outcome?
- Which operations are failing, and where is the latency?

**V1 gaps (build work, not relabeling):**
1. **Cost attribution** — removed with LiteLLM; per-request cost requires an
   agent-supplied model + price table or provider reporting (agent-owned model
   field feeds the trace).
2. **Per-agent trace aggregation + dashboard** — today traces are per-process
   logs; a queryable per-session/per-agent view is missing.
3. **Outcome hooks** — correlating "context injected" with "result improved"
   needs evaluation scaffolding beyond current benchmarks.

### 3.2 Intelligence (Knocode)

- **Repository Intelligence** — incremental indexing; tree-sitter (111 langs),
  tantivy BM25, ast-grep structural, dependency graph; `commit`/`filesystem`
  auto-reindex; index freshness guarantees (cached-handle invalidation).
- **Retrieval** — intent → expansion → BM25 + structural → graph → ranking;
  configurable `RetrievalPolicy` (`candidate_k`, `max_files`, graph/expansion
  toggles); first-class docs indexing with docs/code split.
- **Context Engine** — BuildContext with token budget, cache-order
  `docs → code`, frozen-prefix boundary, session fingerprinting,
  reversible truncation; single index path through `ContextEngine::reindex_repository`.
- **Knowledge** — local SQLite+tantivy entries; repository profile; docs/ADR
  seeding at `init`.

### 3.3 Execution Optimization

- **Delegated to RTK** (github.com/rtk-ai/rtk): the runtime does not compress
  tool outputs. Installers offer RTK as an opt-in external resource and wire
  RTK's own agent integrations (`rtk init`).

### 3.4 Local Runtime

- Single daemon process, tokio; UDS/MessagePack primary + HTTP/JSON fallback.
- Fail-open always: 30s hard timeout → `OriginalPassthrough`; readiness-gated
  startup (listeners/UDS bind only after the initial index); graceful shutdown
  with watcher stop.
- Security: input validation, secrets redaction, HMAC request signing, token
  bucket rate limiting.

## 4. V1 Acceptance Criteria

| # | Criterion | Current | Target |
|---|-----------|---------|--------|
| A1 | Retrieval P50 on a 9k-file repo | 27ms | ≤ 50ms |
| A2 | Speedup vs `grep -rE` | 27–106× | ≥ 20× |
| A3 | BuildContext total overhead (target budget) | — | < 160ms typical; 30s hard fail-open |
| A4 | Retrieval recall@5 on 50-task eval | ~0.29 | ≥ 0.4 |
| A5 | ~~Compression ratio on compressible tool output~~ — delegated to RTK | — | — |
| A6 | Fail-open: agent always gets a response (never blocks) | ✅ | invariant |
| A7 | Every request emits a complete trace (tool/retrieval/context/model sections) | partial (metrics-first) | full |
| A8 | Per-request cost attribution | ❌ (gap) | ✅ |
| A9 | Readiness contract: clients can wait on `state` before sending requests | ✅ (HTTP + UDS Probe) | invariant |
| A10 | Queries serve fresh commits immediately after reindex | ✅ (cached-handle invalidation) | invariant |

## 5. Runtime API (V1 surface)

| Transport | Endpoint / message | Purpose |
|-----------|--------------------|---------|
| UDS/MessagePack | `RequestPayload::MessageRewrite` | Pre-generation: enrich message with context |
| UDS/MessagePack | `RequestPayload::Probe` | Readiness: `state`, `index_files`, `version` |
| HTTP | `GET /health` | Readiness + version + index count |
| HTTP | `GET /metrics` | Prometheus exposition incl. `knocode_daemon_ready` |
| HTTP | `POST /hook` | JSON fallback for the pre-generation hook; `503 daemon_indexing` while not ready |

## 6. Evolution Path

A capability enters the V1 architecture only with **demonstrated measurable
value** (benchmark, eval, or production trace evidence) — the bar that removed
Model Router, FlashRank, and the Skill Engine. New capabilities land as optional
integrations first; they are promoted to pillars only when the evidence exists.