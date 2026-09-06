# Engineering Principles

## Purpose

Define the engineering principles that govern every implementation decision. When a design choice arises, these principles resolve it.

## Principles

### 1. Deterministic Before AI

**Rule:** No LLM call decides whether to retrieve or compress. All such decisions use heuristics or deterministic algorithms. LLM calls are reserved for doing the actual work the user asked for.

**Implications:**
- Repository structure analysis is deterministic (tree-sitter parsing)
- Context selection uses structural relevance (file relationships, imports, BM25 scores)
- Tool-output compression uses RTK's deterministic rules
- The only non-deterministic element is the LLM response itself

### 2. Interception Before the Model

**Rule:** Context injection happens before the model sees anything, via each agent's own native hooks — not a reverse proxy, not an MCP tool the agent can choose to skip. Tool-output optimization is delegated to RTK (external binary), not the runtime.

**Implications:**
- Use opencode's `chat.message` hook; output compression via RTK's own plugin
- Use Claude Code's `UserPromptSubmit` hook; output compression via RTK's own hooks
- Hooks fire unconditionally on every message/tool call
- The agent cannot bypass the runtime's context injection
- Fail-open on timeout or error: pass the raw message through unmodified

### 3. Fail-Open is Mandatory

**Rule:** On timeout or error, pass the raw message through unmodified rather than silently losing context injection with no signal. The runtime must never block or break the agent.

**Implications:**
- `UserPromptSubmit` has a hard 30-second timeout (Claude Code) that silently discards output if exceeded
- Target well under 30 seconds, ideally low single digits
- On any error in the Context Engine, return the original message unchanged
- Log the failure for debugging, but do not block the agent
- The agent always gets a response, even if it's the unmodified original

### 4. Cache-Awareness is First-Class

**Rule:** Prompt caching is the single biggest available cost lever. The Context Pack is ordered for maximum cache stability: docs → code (most to least cache-stable). An explicit frozen-prefix boundary ensures only content after that boundary changes between calls.

**Implications:**
- Context Pack YAML has two sections in fixed order: `docs_context`, `code_context`
- Docs are byte-identical across many tasks (most cache-stable)
- Code changes frequently (least stable)
- Frozen-prefix boundary marks where stable content ends
- Compression should be reversible by default — the model can request originals

### 5. Local-First

**Rule:** All core processing runs on the developer's machine. No external service is required for the runtime to function, except the LLM provider the agent talks to directly.

**Implications:**
- SQLite is the primary database, not a remote database
- SQLite+tantivy runs in-process (engram removed — see REMOVED_TOOLS.md)
- BM25/tantivy runs in-process
- FlashRank runs in-process via `ort` (Rust ONNX Runtime bindings) — removed, offline only
- tree-sitter, ast-grep, ripgrep are embedded as native Rust crates, not shelled out
- Repository data never leaves the machine unless sent to an LLM provider
- Network failures (other than LLM provider) do not break the runtime

### 6. Reuse Existing Tools

**Rule:** Use established, mature tools for every capability they provide. Do not reimplement functionality that exists in a well-maintained library. Custom code is reserved for the things genuinely specific to this problem: the Context Pack's ranking/schema logic and the retrieval/compression heuristics.

**Implications:**
- tree-sitter for all AST parsing — embedded as native Rust crate
- ast-grep for structural code search — embedded as native Rust crate
- ripgrep for text search — embedded as native Rust crate
- BM25/tantivy for full-text indexing and search
- FlashRank for reranking — via `ort` (ONNX Runtime) — removed (see REMOVED_TOOLS.md / REMOVED_TOOLS.md)
- engram for memory — SQLite+FTS5, MCP-native — removed (see REMOVED_TOOLS.md; SQLite+tantivy local)
- RTK for tool-output compression — delegated entirely (external binary, not embedded in the daemon)
- tiktoken-rs for local token counting — never via model API round-trip

### 7. Minimal Runtime

**Rule:** The runtime does the minimum necessary to improve the coding agent. It does not replicate agent capabilities. It does not accumulate orchestration, governance, or SDLC features.

**Implications:**
- No code editing logic
- No shell execution
- No conversation state management
- No user interface beyond CLI
- No workflow orchestration
- No task scheduling
- No event-driven architecture for the hot path
- Components communicate through direct function calls, not message passing
- The event bus is strictly async/observability, never in the `BuildContext` call path

### 8. Concrete v1

**Rule:** Every design decision must resolve to a single concrete implementation for v1. No abstract interfaces without a concrete reason. No "TBD" fields. No generic plugin points.

**Implications:**
- One database schema, not a schema versioning system
- One configuration format (TOML)
- One IPC protocol (Unix domain socket with MessagePack)
- One context format (YAML with fixed sections)
- Direct struct usage, not trait objects for component interfaces
- Concrete error types, not generic error enums
- The runtime is model-agnostic: no model gateway or router (removed — see `docs/01-architecture/REMOVED_TOOLS.md`)

### 9. Incremental Repository Intelligence

**Rule:** Repository knowledge builds up incrementally, triggered by git changes — exactly like a language server's own indexing. Never rebuilt on every prompt.

**Implications:**
- tree-sitter's native incremental parsing is used
- Git change triggers incremental reparse
- Stored repository model is updated, not rebuilt
- The Context Engine consumes stored metadata, not live parsing
- Indexing runs in the background, not on the hot path

### 10. Token Efficiency

**Rule:** Every token sent to a model must earn its place. Reduce token count at every stage without losing information the model needs.

**Implications:**
- Context packs have hard token budgets enforced by the Context Engine
- Tool outputs are compressed via RTK before inclusion in context
- Only relevant files are included, not entire directories
- Duplicated content is deduplicated across context sources
- Token counts are tracked locally via tiktoken-rs (never via model API)

### 11. Observable Behavior

**Rule:** Every significant operation produces structured logs. The runtime's behavior can be understood from logs alone. The event bus provides async observability without impacting the hot path.

**Implications:**
- Every component uses structured logging
- Log levels: ERROR for failures, WARN for recoverable issues, INFO for request lifecycle, DEBUG for component decisions
- Each request gets a unique correlation ID propagated across all components
- Token counts are logged at every stage
- Event bus events: ContextBuilt, RepositoryUpdated, ResponseGenerated, MemorySaved
- CLI inspection command can preview what a prompt would build

### 12. Portability via Interfaces

**Rule:** Define `IContextBuilder` (in-process / daemon / remote) as an explicit contract; the reference implementation is the Rust Context Engine and stays swappable behind it.

**Implications:**
- The Context Engine implements `IContextBuilder`
- The model gateway (`IModelGateway`) and workflow engine (`IWorkflowEngine`) interfaces were removed with the Model Router and workflow engine (see `docs/01-architecture/REMOVED_TOOLS.md`)
- Swapping implementations requires only re-implementing the interface, not modifying the runtime

### 13. Knowledge is Retrieval-Only (skills removed)

**Rule:** Docs, ADRs, and long-term memory live behind one Knowledge Hub API using BM25 lexical retrieval. The Skill Engine was removed — agents own skill discovery natively (see `docs/01-architecture/REMOVED_TOOLS.md`). Do not re-introduce a parallel skill-selection system without measured outcome gains.

**Implications:**
- Knowledge Hub has one API surface for storage and retrieval
- Doc/code retrieval uses BM25 lexical search (FlashRank removed — see REMOVED_TOOLS.md, reranker is passthrough)
- Memory uses SQLite+tantivy local (engram removed — see REMOVED_TOOLS.md)
- Knowledge subsystems are composed, not unified into one algorithm

### 14. Report Savings Honestly

**Rule:** Separate "reduction in the specific thing measured" (e.g., bash output size) from "reduction in your bill" (diluted by system prompt, history, and output tokens). Do not oversell.

**Implications:**
- Report tool-output compression ratio separately from total cost reduction (RTK owns compression; knocode owns context)
- Report token reduction per request separately from bill impact
- Include caveats about system prompt, history, and output tokens in cost claims
- Use Promptfoo for objective evaluation, not cherry-picked examples

## Principle Hierarchy

When principles conflict, resolve in this order:

1. **Fail-Open is Mandatory** — The agent must never be blocked or broken
2. **Deterministic Before AI** — Predictable behavior builds trust
3. **Cache-Awareness is First-Class** — Prompt caching is the biggest cost lever
4. **Local-First** — Privacy and offline capability are non-negotiable
5. **Concrete v1** — Ship a working system, not a framework
6. **Minimal Runtime** — Do less, do it well
7. **Token Efficiency** — Every token has a cost
8. **Reuse Existing Tools** — Leverage the ecosystem
9. **Interception Before the Model** — Native hooks, not proxies
10. **Observable Behavior** — If you can't see it, you can't fix it
11. **Incremental Repository Intelligence** — Never rebuild on every prompt
12. **Portability via Interfaces** — Swappable implementations
13. **Knowledge is Organizationally Unified** — One API, different retrieval strategies
14. **Report Savings Honestly** — Measure and report accurately
