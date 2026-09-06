# Glossary

## Purpose

Define all terms used across the AI Runtime for Coding Agents specification documents. This is the authoritative terminology reference.

## Terms

### Runtime

**Definition:** The AI Runtime system itself — a local application that improves coding agents by providing repository intelligence and context optimization. Tool-output compression is delegated to RTK.

**Scope:** Everything that runs as the daemon process. Excludes the coding agent and model providers.

### Daemon

**Definition:** The long-lived local process that hosts the Context Engine and all runtime modules. Communicates with the Adapter Layer over a Unix domain socket using MessagePack.

**Scope:** The process lifecycle is: start → initialize → listen → serve requests → shutdown.

### Agent

**Definition:** A coding agent that interacts with a developer to write, modify, and understand code. Examples: opencode, Claude Code, Cursor, Gemini CLI.

**Scope:** External to the runtime. The runtime improves the agent but does not become the agent.

### Tier 1 Agent

**Definition:** An agent with true programmatic hooks that fire unconditionally on every message/tool call. The runtime can intercept and rewrite messages with guaranteed execution.

**Agents:** opencode, Claude Code, Cursor, Gemini CLI, GitHub Copilot, OpenClaw, Pi, Factory Droid.

### Tier 2 Agent

**Definition:** An agent that only exposes convention-based integration (a rules file the agent may or may not follow). Support is best-effort, never with the same guarantee as Tier 1.

**Agents:** Codex, Windsurf, Cline, Kilo Code, Antigravity, Kimi.

### Adapter Layer

**Definition:** One thin adapter per agent CLI, implementing two operations: intercept-before-generation (rewrite the message) and intercept-before-tool-call (allow/deny/modify). Adapters translate between agent-specific hooks and the runtime's internal format.

**Scope:** The entry point of the runtime. One adapter type per supported agent.

### Pre-Generation Hook

**Definition:** A hook that fires before the model generates a response. The runtime intercepts the message, builds context, and rewrites the message with injected context.

**Examples:** opencode `chat.message`, Claude Code `UserPromptSubmit`.

### Pre-Tool Hook

**Definition:** [REMOVED from the runtime] A hook that fires before a tool executes. The daemon no longer compresses tool outputs — compression is delegated to RTK (external binary), which installs its own hooks. See REMOVED_TOOLS.md.

**Examples:** opencode `tool.execute.before`, Claude Code `PreToolUse` (now owned by RTK, not the knocode daemon).

### Fail-Open

**Definition:** The behavior where, on timeout or error, the runtime passes the raw message through unmodified rather than blocking the agent or silently losing context injection.

**Scope:** Mandatory for all hook implementations. The agent always gets a response.

### Context Engine

**Definition:** The central component that implements `BuildContext(task)`. Retrieves relevant information, ranks and reranks results, deduplicates, compresses, orders for cache stability, enforces token budgets, and emits a Context Pack as YAML.

**Scope:** The one public entry point of the runtime. Runs as a long-lived daemon.

### BuildContext

**Definition:** The single public API of the Context Engine. Takes a task description and returns a Context Pack containing all context needed for the model to complete the task.

**Signature:** `BuildContext(task: TaskRequest) -> ContextPack`

### Context Pack

**Definition:** The final, token-budgeted package of context assembled for a single LLM request. Emitted as YAML with two sections in fixed order: `docs_context`, `code_context`.

**Scope:** Output of the Context Engine. Input to the model.

### Frozen-Prefix Boundary

**Definition:** An explicit boundary in the Context Pack that marks where cache-stable content ends and variable content begins. Content before the boundary is byte-identical across many tasks; content after changes between calls.

**Scope:** Part of the cache-awareness strategy. Maximizes prompt cache hit rates.

### Task

**Definition:** A description of work provided by the developer to the coding agent. The runtime receives this as input to `BuildContext`.

**Scope:** The runtime does not decompose tasks. Task decomposition is the agent's responsibility.

### Repository Intelligence

**Definition:** The system that parses, indexes, and understands a codebase incrementally. Uses tree-sitter for AST parsing, ast-grep for structural search, ripgrep for text search. Updated on git changes, not per-request.

**Scope:** Owns: incremental parsing, structural search, text search, metadata storage.

### Knowledge Hub

**Definition:** One organizational surface for project docs, ADRs, templates, and long-term memory. BM25/tantivy lexical search over stored knowledge (FlashRank and engram removed — see REMOVED_TOOLS.md and REMOVED_TOOLS.md).

**Scope:** Owns: storage and retrieval of all knowledge types.

### Skill

**Definition:** A named, reusable instruction set that teaches a coding agent how to perform a specific type of task. Skills live in community formats (Claude, Cursor, Continue, agentskills.io) and are discovered by the agent's own tooling — the runtime does not load or match them (removed — see `docs/01-architecture/REMOVED_TOOLS.md`).

**Scope:** Owned by the coding agent's ecosystem, not the runtime.

### Skill Engine

**Definition:** [REMOVED] Knocode's deterministic tag-based skill matching component was removed — agents own skill discovery natively (see `docs/01-architecture/REMOVED_TOOLS.md`).

### Context

**Definition:** Information provided to the LLM to help it understand and complete a task. Includes: documentation, code snippets, repository metadata.

**Scope:** Built by the Context Engine. Managed within token budgets.

### Model Router / Tier / Gateway

**Definition:** [REMOVED v0.8.6] Heuristic tier routing and the LiteLLM gateway were deleted — the runtime is model-agnostic and the agent / provider / user chooses the model (see REMOVED_TOOLS.md).

### Execution Optimizer

**Definition:** [REMOVED from the runtime] Formerly the component that compressed tool outputs. Tool-output compression is now delegated entirely to RTK (see REMOVED_TOOLS.md).

**Scope:** None — removed from the daemon; RTK's own integrations handle compression.

### RTK

**Definition:** RunTime Kit — a Rust binary for tool-output compression. Zero dependencies, <10ms overhead. Intercepts tool/command output and rewrites it to compact form.

**Scope:** Adopted directly, not reimplemented.

### Tool Output

**Definition:** The result of a tool execution by the coding agent. Includes: file contents read by the agent, search results, shell command output, and any other structured output returned to the model.

**Scope:** Compressed by RTK (external binary) — the runtime no longer touches tool outputs.

### Event Bus

**Definition:** An async-only system for observability events. Events: ContextBuilt, RepositoryUpdated, ResponseGenerated, MemorySaved. Never in the `BuildContext` call path.

**Scope:** Consumed by CLI inspection, metrics, and future orchestrators.

### Memory

**Definition:** Long-term storage of information across sessions. Historically via engram (single Go binary, SQLite+FTS5, MCP-native); removed — now SQLite+tantivy local (see REMOVED_TOOLS.md). No LLM/embedding dependency for its core save/search path.

**Scope:** Used by the Knowledge Hub for cross-session knowledge persistence.

### engram

**Definition:** A memory system (`Gentleman-Programming/engram`): single Go binary, SQLite+FTS5, MCP-native. Provides save and search capabilities without LLM or embedding dependencies. **Removed** from Knocode v1 — see REMOVED_TOOLS.md (replaced by SQLite+tantivy local).

**Scope:** Used for persistent memory in the Knowledge Hub.

### BM25

**Definition:** A lexical scoring algorithm used for full-text search. Computes relevance based on term frequency and inverse document frequency. Used by tantivy for code and documentation retrieval.

**Scope:** Part of the Knowledge Hub's retrieval pipeline.

### FlashRank

**Definition:** A reranking model that reorders search results using a cross-encoder. Runs in-process via `ort` (Rust ONNX Runtime bindings). Applied after BM25 scoring to improve result quality.

**Scope:** Part of the Knowledge Hub's retrieval pipeline.

### Token Budget

**Definition:** The maximum number of tokens allocated for a Context Pack. Configured in TOML. The Context Engine enforces this budget by selecting and truncating content to fit.

**Scope:** Enforced by the Context Engine.

### Token Counting

**Definition:** The process of counting tokens locally using `tiktoken-rs`. Never via a model API round-trip. Provides accurate token counts for budget enforcement.

**Scope:** Used by the Context Engine throughout context construction.

### Correlation ID

**Definition:** A unique identifier assigned to each request, propagated across all components and included in log entries. Enables tracing a single request through the entire runtime.

**Scope:** Generated by the Adapter Layer. Used by all components for logging.

### Configuration

**Definition:** Runtime settings defined in TOML format. Includes: token budgets, daemon settings, retrieval settings, logging levels, and database paths.

**Scope:** Loaded at daemon startup.

### Index

**Definition:** The repository metadata store (SQLite) and search indices (BM25/tantivy) that collectively represent the runtime's understanding of a repository.

**Scope:** Owned by Repository Intelligence. Persistent across daemon restarts.

### IContextBuilder

**Definition:** The interface contract for context building. Supports in-process, daemon, and remote implementations. The Context Engine implements this interface.

**Scope:** Defined as a contract for portability. Concrete implementation is the Rust daemon.

### IModelGateway

**Definition:** [REMOVED v0.8.6] The model gateway interface and its LiteLLM implementation were deleted with the Model Router — the runtime is model-agnostic (see REMOVED_TOOLS.md).

### IWorkflowEngine

**Definition:** [REMOVED] The workflow-engine interface (and DBOS) were removed — the runtime is a single tokio daemon (see `docs/01-architecture/REMOVED_TOOLS.md`).

**Scope:** Removed with the workflow engine.

### Prompt Caching

**Definition:** A cost optimization where the model provider caches a prefix of the prompt and charges less for cached tokens. The runtime maximizes cache hits by ordering context for stability.

**Scope:** First-class concern in the Context Engine's pack ordering.

### Preview Command

**Definition:** A CLI command that previews what a given prompt would build via BuildContext (event replay was removed — see `docs/01-architecture/REMOVED_TOOLS.md`).

**Scope:** Runs BuildContext locally (or via the daemon when running). Part of the CLI.
