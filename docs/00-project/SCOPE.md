# Scope

## Purpose

Define what the AI Runtime for Coding Agents does, what it does not do, and who owns each responsibility. This is the authoritative boundary reference for all implementation work.

## In Scope (v1)

| Area | What Is Included |
|------|------------------|
| **Agent Interception** | Pre-generation hooks for Tier 1 agents (opencode, Claude Code, Cursor, Gemini CLI, Copilot, OpenClaw, Pi, Factory Droid). Tier 2 agents supported as best-effort via convention-based integration. |
| **Repository Intelligence** | Incremental AST parsing (tree-sitter), structural search (ast-grep), text search (ripgrep), git-change-triggered incremental updates, metadata storage. Optional LSP enrichment via agent's own language server. |
| **Knowledge Hub** | Unified organizational surface for docs, ADRs, templates, and memory. BM25/tantivy for lexical retrieval. FlashRank and engram removed (see REMOVED_TOOLS.md, REMOVED_TOOLS.md); memory is SQLite+tantivy local. The Skill Engine was removed — agents own skill discovery natively (see `docs/01-architecture/REMOVED_TOOLS.md`). |
| **Context Engine** | `BuildContext(task)` — the one public API. Retrieve → rank → deduplicate → compress → cache-order → token-budget → emit YAML Context Pack. Runs as a long-lived local daemon with Unix socket IPC. Local token counting via `tiktoken-rs`. |
| **Execution Optimizer** | ❌ removed — tool-output compression delegated to RTK (external binary); installers wire RTK's own integrations. See REMOVED_TOOLS.md. |
| **Event Bus** | Async-only observability events: ContextBuilt, RepositoryUpdated, ResponseGenerated, MemorySaved. Consumed by CLI inspection, metrics, and future orchestrators. |
| **Local Persistence** | SQLite for repository index, metadata, and memory (engram removed). Filesystem for configuration and logs. |
| **CLI** | Start daemon, initialize repository, preview BuildContext, health check. |
| **Configuration** | TOML-based configuration for token budgets, retrieval settings, daemon settings, and logging. |
| **Offline Evaluation** | Promptfoo configuration for CI regression and scheduled eval against real usage logs. |

## Out of Scope (v1)

| Area | Why It Is Excluded |
|------|---------------------|
| **Code editing** | The coding agent owns all code modification |
| **Shell execution** | The coding agent owns all shell commands |
| **Test execution** | The coding agent owns test running |
| **Git operations** | The coding agent owns commits, branches, and merges |
| **Deployment** | External tool responsibility |
| **CI/CD orchestration** | External tool responsibility |
| **Conversational state** | The coding agent owns conversation history |
| **User interaction** | The coding agent owns the user interface |
| **Multi-tenancy** | Single user, single repository per daemon |
| **Plugin marketplace** | Agents discover skills from their own ecosystem (`.claude/skills`, `.agents/skills`, ...); the runtime does not own a marketplace |
| **Workflow orchestration** | Removed — single tokio daemon (see `docs/01-architecture/REMOVED_TOOLS.md`) |
| **Distributed infrastructure** | Single local daemon process |
| **Web dashboard** | CLI-only interface |
| **Authentication** | Local daemon, no auth needed |
| **Rate limiting** | Provider rate limiting is the provider's concern; the runtime token-buckets its own request intake |
| **Model fine-tuning** | Routes to existing models only |
| **Data labeling** | No human-in-the-loop labeling |
| **Audit trail** | Logging and events only, no formal audit system |
| **Collaborative editing** | Single developer per session |
| **Background jobs** | All processing is request-response (daemon) or async (event bus) |
| **Human approval workflows** | Out of scope; added by external orchestrator if needed |
| **Governance dashboards** | Out of scope; added by external orchestrator if needed |
| **Mission management** | No mission/workflow decomposition in the runtime |
| **Quality gates** | Native per-language analyzers run externally; runtime does not enforce them |

## Responsibility Boundaries

### Runtime Owns

| Responsibility | Details |
|----------------|---------|
| Agent interception | Pre-generation hooks |
| Repository parsing | tree-sitter AST parsing, incremental updates on git change |
| Code indexing | Structural search (ast-grep), text search (ripgrep), metadata storage |
| Knowledge storage | Docs, ADRs, templates, memory (SQLite+tantivy local; engram removed) |
| Knowledge retrieval | BM25/tantivy lexical search (FlashRank removed) |
| Context assembly | Token-budgeted YAML Context Pack with cache-aware ordering |
| Tool-output compression | ❌ delegated to RTK (external binary) — not a daemon responsibility |
| Token accounting | Local token counting via tiktoken-rs |
| Observability | Event bus for async metrics and inspection |

### Coding Agent Owns

| Responsibility | Details |
|----------------|---------|
| User interaction | Prompts, responses, UI rendering |
| Conversation management | Multi-turn state, message history |
| Code editing | File creation, modification, deletion |
| Shell execution | Running commands, scripts, tools |
| Test execution | Running tests, parsing results |
| Git operations | Commits, branches, diffs, merges |
| Tool definitions | Defining available tools for the model |
| Error presentation | Showing errors to the developer |
| Retry logic | Deciding when and how to retry failed operations |
| Model API calls | The agent calls its model provider directly — the runtime is model-agnostic and never routes |

### External Tools Own

| Responsibility | Details |
|----------------|---------|
| LLM inference | Model providers (OpenAI, Anthropic, Google, etc.) |
| Provider authentication | API keys and credentials |
| Provider rate limiting | Quota management |
| Language servers | Optional LSP enrichment (agent's own processes) |
| Static analysis | Native per-language analyzers |
| External orchestration | Removed with the workflow engine (see `docs/01-architecture/REMOVED_TOOLS.md`) |

## v1 Boundaries

### Process Boundary

```
┌──────────────────────────────────────────────────────────┐
│                     Developer Machine                     │
│                                                          │
│  ┌─────────────────────┐    ┌──────────────────────────┐ │
│  │    Coding Agent     │    │   Knocode Daemon         │ │
│  │                     │    │                          │ │
│  │  - UI               │    │  ┌────────────────────┐  │ │
│  │  - Code editing     │◄──►│  │  Adapter Layer     │  │ │
│  │  - Shell exec       │ UDS│  │  (Tier 1/Tier 2)   │  │ │
│  │  - Git ops          │    │  └────────┬───────────┘  │ │
│  │  - Conversation     │    │           │              │ │
│  │                     │    │  ┌────────▼───────────┐  │ │
│  └─────────────────────┘    │  │  Context Engine    │  │ │
│                              │  │  (BuildContext)    │  │ │
│                              │  └────────┬───────────┘  │ │
│                              │           │              │ │
│           ┌──────────────────┼───────────────┐             │ │
│           │                  │               │             │ │
│  ┌────────▼──────┐  ┌───────▼────┐           │             │ │
│  │ Repo Intel    │  │Knowledge Hub│           │             │ │
│  │ (tree-sitter, │  │(BM25,      │           │             │ │
│  │  ast-grep,    │  │ local)     │           │             │ │
│  │  ripgrep)     │  │            │           │             │ │
│  └───────────────┘  └────────────┘           │             │ │
│                              │                               │ │
│  ┌───────────────────────────▼──────────────────────────────┐ │
│  │                    Event Bus (async)                      │   │
│  │  ContextBuilt, RepositoryUpdated,                        │   │
│  │  ResponseGenerated, MemorySaved                          │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │                 Local Storage                             │   │
│  │  - SQLite (index, metadata, memory)                       │   │
│  │  - Tantivy BM25 (index)                                   │   │
│  │  - Filesystem (config, logs)                              │   │
│  └──────────────────────────────────────────────────────────┘   │
│                                                                  │
└──────────────────────────────────────────────────────────────────┘
           │
           │  Agent's own connection (runtime is model-agnostic)
           ▼
  ┌─────────────────┐
  │  Model Provider │
  │  (OpenAI, etc.) │
  └─────────────────┘
```

### Communication Boundary

| Path | Protocol | Direction | Purpose |
|------|----------|-----------|---------|
| Agent → Daemon | Unix Domain Socket (MessagePack) | Bidirectional | Pre-generation hooks, readiness probes |
| Daemon → engram | *Removed* — memory is SQLite local (see REMOVED_TOOLS.md) | — |
| Daemon → SQLite | In-process (rusqlite) | Bidirectional | Index and metadata |
| Daemon → Event Bus | Internal async channel | Outbound only | Observability events |

### Data Boundary

| Data | Owner | Persistence |
|------|-------|-------------|
| Repository source code | Developer / Git | Filesystem (read-only by runtime) |
| Repository index | Runtime | SQLite database |
| Repository metadata | Runtime | SQLite database |
| Memory entries | Runtime | SQLite (local; engram removed) |
| Configuration | Developer | TOML files on filesystem |
| Conversation history | Coding Agent | Not stored by runtime |
| Token usage metrics | Runtime | SQLite database |
| Logs | Runtime | Log files on filesystem |

## Future Features (Must NOT Affect v1)

| Feature | Planned Version | v1 Impact |
|---------|-----------------|-----------|
| Multi-repository support | v2 | None. v1 uses single-repo schema |
| Conversation memory | v2 | None. v1 is stateless across requests |
| Plugin system | v2 | None. v1 is hook-based (no plugin surface) |
| Web dashboard | v2 | None. v1 is CLI-only |
| Distributed deployment | v2 | None. v1 is single daemon |
| Multi-agent coordination | v2 | None. v1 serves one agent |
| Collaborative editing | v3 | None. v1 is single-developer |
| Model fine-tuning | v3 | None. v1 is model-agnostic — the agent picks the model |
| CI/CD integration | v2 | None. v1 is request-response |
| Workflow engine | Removed | None. Single tokio daemon (see `docs/01-architecture/REMOVED_TOOLS.md`) |
| Enterprise governance | v3 | None. v1 has no auth or audit |
| Vector/semantic recall | Deferred | None. v1 uses FTS5 lexical recall only |
| Graph-based retrieval | Deferred | None. v1 uses BM25 + reranking only |
| External orchestration | v0.6.0 required (SQLite) | v1 separate product; v0.6.0 promoted to required runtime (single-node SQLite+Litestream) |
