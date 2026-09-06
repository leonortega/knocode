# Data Flow

> **V1 framing:** see [V1_RUNTIME_SPEC.md](V1_RUNTIME_SPEC.md). Model Router /
> LiteLLM were deleted in v0.8.6 (see REMOVED_TOOLS.md) — Flow 7 (Model Routing) is removed.
> Flow 5 (Skill Selection) is removed with the Skill Engine (see `REMOVED_TOOLS.md`).
> Where this file conflicts with the V1 spec or the code, the V1 spec / code win.

## Purpose

Describe all important data flows through the AI Runtime. Each flow shows the sequence of operations, data transformations, and module interactions.

## Flow 1: Repository Indexing (Incremental)

### Trigger

- Git change detected (file system watcher or manual trigger)
- `knocode index` command
- SIGHUP or SIGUSR1 signal

### Sequence

```mermaid
sequenceDiagram
    participant GI as Git Change
    participant RI as Repository Intelligence
    participant TS as tree-sitter
    participant RG as ripgrep
    participant AG as ast-grep
    participant TV as BM25/Tantivy
    participant DB as SQLite
    participant KH as Knowledge Hub
    participant EB as Event Bus

    GI->>RI: RepositoryUpdated (file change)
    RI->>DB: SELECT path, hash FROM files
    DB-->>RI: known_files

    RI->>RI: walk_directory_tree()
    RI->>RI: diff(current, known)

    loop For each new/changed file
        RI->>TS: parse(content, language) [incremental]
        TS-->>RI: AST
        RI->>RI: extract_symbols(AST)
        RI->>TV: add_document(content, metadata)
        RI->>DB: insert/update file metadata
        RI->>DB: insert/update symbols
    end

    loop For each deleted file
        RI->>TV: delete_document(path)
        RI->>DB: DELETE WHERE path = ?
    end

    RI->>TV: commit_index()
    RI->>KH: extract_knowledge(index_results)
    KH->>KH: detect_patterns(index_results)
    KH->>DB: insert_knowledge(entries)
    RI->>EB: emit(RepositoryUpdated)
    RI-->>RI: IndexResult(statistics)
```

### Delta Detection

1. Query SQLite for all known file paths and hashes
2. Walk current filesystem
3. Compute three sets:
   - **New files**: in filesystem, not in database
   - **Changed files**: in both, but hash differs
   - **Deleted files**: in database, not in filesystem
4. Process each set accordingly

---

## Flow 2: Pre-Generation (BuildContext)

### Trigger

- Agent's pre-generation hook fires (e.g., `chat.message`, `UserPromptSubmit`)

### Sequence

```mermaid
sequenceDiagram
    participant Agent as Coding Agent
    participant AD as Adapter Layer
    participant CE as Context Engine
    participant RI as Repository Intelligence
    participant KH as Knowledge Hub
    participant TC as tiktoken-rs
    participant EB as Event Bus

    Agent->>AD: PreGeneration(message, session_id)
    AD->>AD: Validate request
    AD->>AD: Generate correlation ID

    AD->>CE: BuildContext(task)

    CE->>RI: search_code(query)
    RI->>TV: BM25 search + ripgrep
    TV-->>RI: ranked_results
    RI-->>CE: SearchResults

    CE->>KH: retrieve_knowledge(query)
    KH->>TV: BM25 search
    TV-->>KH: candidates
    KH-->>CE: Vec<KnowledgeEntry>

    CE->>TC: count_tokens(content)
    TC-->>CE: token_counts

    CE->>CE: Order: docs → code
    CE->>CE: Apply frozen-prefix boundary
    CE->>CE: Deduplicate against fingerprint
    CE->>CE: Enforce token budget
    CE->>CE: Emit Context Pack as YAML

    CE->>EB: emit(ContextBuilt)

    CE-->>AD: ContextPack
    AD-->>Agent: RewrittenMessage(with context)
```

---

## Flow 3: Pre-Tool (Tool Output Compression)

> **Removed:** the daemon no longer compresses tool outputs. Compression lives
> entirely in RTK (external binary, wired by the installers via `rtk init`); the
> `PreToolCall`/`ToolOutput`/`CompressedOutput` IPC variants were deleted — see
> `REMOVED_TOOLS.md`.

### Trigger

- None (historical). The `PreToolCall` flow was removed from the daemon.

---

## Flow 4: Knowledge Retrieval

### Trigger

- Part of BuildContext pipeline (Flow 2)

### Sequence

```mermaid
sequenceDiagram
    participant CE as Context Engine
    participant KH as Knowledge Hub
    participant TV as BM25/Tantivy
    participant DB as SQLite

    CE->>KH: retrieve_knowledge(query, category_filter)
    KH->>TV: search(query, max_results=20)
    TV-->>KH: ranked_results

    KH->>DB: SELECT confidence WHERE id IN (...)
    DB-->>KH: confidence_scores

    KH->>KH: filter_by_confidence(min=0.3)
    KH->>KH: take(top_10)

    KH->>DB: search_memory(query)  -- local LIKE (engram removed)
    DB-->>KH: memory_entries

    KH->>KH: merge(knowledge, memory)
    KH-->>CE: Vec<KnowledgeEntry>
```

### KnowledgeEntry Structure

```json
{
  "id": 42,
  "category": "convention",
  "key": "naming_functions",
  "value": "Functions use camelCase naming convention",
  "confidence": 0.95,
  "source": "detected from 15 function definitions",
  "relevance_score": 0.87
}
```

---

## Flow 5: Skill Selection — [REMOVED]

> Skill matching ran inside BuildContext as an optional path. The Skill Engine was
> removed — agents own skill discovery natively (see `REMOVED_TOOLS.md`).


---

## Flow 6: Context Construction (Cache-Aware)

### Trigger

- Part of BuildContext pipeline (Flow 2)
- Orchestrates Flow 4

### Sequence

```mermaid
flowchart TD
    A[Receive TaskRequest] --> B[Initialize token budget: 12000]
    B --> C[Search code: ~6600 tokens]
    C --> D[Retrieve knowledge: ~45% budget]
    D --> E[Order for cache stability]

    E --> F[Section 1: docs_context - 45%]
    F --> G[Frozen-prefix boundary]
    G --> H[Section 2: code_context - 55%]

    H --> I[Deduplicate against session fingerprint]
    I --> J[Enforce token budget]
    J --> K[Emit Context Pack as YAML]

    style F fill:#f3e5f5
    style G fill:#fff3e0
    style H fill:#e1f5fe
```

### Cache-Aware Ordering Detail

```
Context Pack Structure (YAML):

┌─────────────────────────────────────────────────┐
│ docs_context                       45%  5,400 tok│
│ ████████████████████████████████████████         │
│ (Most cache-stable: byte-identical across tasks)│
├───────── FROZEN-PREFIX BOUNDARY ────────────────┤
│ code_context                       55%  6,600 tok│
│ ████████████████████████████████████████         │
│ ████████████████████████████████████████         │
│ ████████████████████████████████████████         │
│ (Least stable: changes frequently)               │
└─────────────────────────────────────────────────┘
```

### Code File Selection

1. Search Repository Intelligence with task description
2. Get top 20 candidate files
3. Score each file by:
   - Text match relevance (0.0–1.0)
   - Structural relevance (imports, function calls) (0.0–1.0)
   - File proximity (same directory = higher) (0.0–1.0)
4. Sort by composite score
5. Add files to context until code budget is exhausted
6. For each file, truncate to `max_lines_per_file` if needed

---

## Flow 7: Model Routing — [REMOVED v0.8.6]

Model routing / LiteLLM were deleted from the v1 runtime (see REMOVED_TOOLS.md).
The runtime is model-agnostic — the agent / provider / user chooses the model
(V1_RUNTIME_SPEC.md §2.3). Flow 2 (BuildContext) ends at `ContextPack`.

---

## Flow 8: Memory Operations (SQLite+tantivy local — engram removed, see REMOVED_TOOLS.md)

### Read (In Hot Path)

```mermaid
sequenceDiagram
    participant CE as Context Engine
    participant KH as Knowledge Hub
    participant DB as SQLite

    CE->>KH: retrieve_knowledge(query)
    KH->>DB: search_memory(query)
    DB-->>KH: memory_entries
    KH->>KH: merge with knowledge entries
    KH-->>CE: Vec<KnowledgeEntry>
```

### Write (Agent-Invoked, Async)

```mermaid
sequenceDiagram
    participant Agent as Coding Agent
    participant AD as Adapter Layer
    participant KH as Knowledge Hub
    participant DB as SQLite
    participant EB as Event Bus

    Agent->>AD: MemorySave(namespace, key, value)
    AD->>KH: memory_save(entry)
    KH->>DB: save(entry)
    DB-->>KH: confirmation
    KH->>EB: emit(MemorySaved)
    KH-->>AD: SaveResult
    AD-->>Agent: confirmation
```

---

## Flow 9: Event Bus (Async Observability)

### Sequence

```mermaid
sequenceDiagram
    participant CE as Context Engine
    participant RI as Repository Intelligence
    participant EB as Event Bus
    participant CLI as CLI Inspection
    participant MET as Metrics

    CE->>EB: emit(ContextBuilt {correlation_id, tokens, ...})
    RI->>EB: emit(RepositoryUpdated {files_indexed, ...})

    EB->>CLI: dispatch(event)
    EB->>MET: dispatch(event)

    CLI->>CLI: Store in in-memory buffer
    MET->>MET: Aggregate metrics
```

---

## Flow 10: Fail-Open (Timeout/Error)

### Trigger

- BuildContext exceeds 30s timeout
- Any unrecoverable error in the pipeline

### Sequence

```mermaid
sequenceDiagram
    participant Agent as Coding Agent
    participant AD as Adapter Layer
    participant CE as Context Engine

    Agent->>AD: PreGeneration(message, session_id)
    AD->>AD: Validate request
    AD->>AD: Generate correlation ID

    AD->>CE: BuildContext(task)

    Note over CE: Processing...

    alt Timeout (> 30s)
        AD->>AD: Timeout fired
        AD->>AD: Log warning with correlation_id
        AD-->>Agent: OriginalPassthrough {reason: "timeout"}
    else Error
        CE-->>AD: Error
        AD->>AD: Log error with correlation_id
        AD-->>Agent: OriginalPassthrough {reason: "fail-open"}
    end

    Note over Agent: Agent continues with unmodified message
```

### Fail-Open Guarantees

| Condition | Response | Agent Impact |
|-----------|----------|--------------|
| BuildContext timeout | OriginalPassthrough | None — original message used |
| BuildContext error | OriginalPassthrough | None — original message used |
| Repository not indexed | OriginalPassthrough | None — original message used |
| Any internal error | OriginalPassthrough | None — original message used |

The agent always gets a response. The runtime never blocks or breaks the agent.
