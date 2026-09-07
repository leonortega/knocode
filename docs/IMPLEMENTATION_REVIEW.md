# Implementation Review — Remaining Items

**Date:** 2026-09-04
**Status:** Pending review

This document lists all incomplete or partially-implemented items found during the
repository review. Each item is categorized and includes context so a decision can
be made on whether to complete, remove, or defer.

---

## Category A: Dead code that was kept (honest `#[allow(dead_code)]`)

These are struct fields that are set from config but never read. Removing them
requires constructor changes. Decide: wire them, remove them, or leave as-is.

### A1. `DaemonState.db` — unused database handle
- **File:** `crates/knocode-daemon/src/lifecycle.rs:22`
- **What:** `pub db: Arc<Database>` is stored in daemon state but never accessed.
- **Why it exists:** The database was intended for state persistence (caching, dedup).
- **Options:**
  - **Wire it:** Use the DB for query result caching, index metadata, or dedup.
  - **Remove it:** Delete the field + constructor arg. Simplifies state.
  - **Leave:** Harmless, but misleading.

### A2. `KnowledgeHub.config` — unused config
- **File:** `crates/knocode-knowledge/src/lib.rs:27`
- **What:** `config: KnowledgeConfig` stored but never read.
- **Why it exists:** Intended for tuning knowledge retrieval parameters.
- **Options:**
  - **Wire it:** Use config values for retrieval thresholds, ranking weights.
  - **Remove it:** Delete field + constructor param.
  - **Leave:** Harmless.

### A3. `RepositoryIntelligence.import_pattern` — unused regex
- **File:** `crates/knocode-repo-intel/src/lib.rs:54`
- **What:** Regex pattern for import statements, compiled but never used in search.
- **Why it exists:** Built for richer code intelligence (import graph).
- **Options:**
  - **Wire it:** Use in symbol extraction to build import dependency graph.
  - **Remove it:** Delete field + construction code.
  - **Leave:** Harmless.

### A4. `RepositoryIntelligence.file_hashes` — unused cache
- **File:** `crates/knocode-repo-intel/src/lib.rs:133`
- **What:** `HashMap<String, String>` for file change detection, always empty.
- **Why it exists:** Intended for incremental re-indexing (skip unchanged files).
- **Options:**
  - **Wire it:** Implement incremental indexing by comparing hashes.
  - **Remove it:** Delete field + initialization.
  - **Leave:** Harmless.

### A5. `IndexedFile.hash` — unused hash field
- **File:** `crates/knocode-repo-intel/src/lib.rs:242`
- **What:** Content hash stored per file but never compared.
- **Why it exists:** Pairs with A4 for incremental re-indexing.
- **Options:**
  - **Wire it:** Compute and store hashes, compare on re-index.
  - **Remove it:** Delete field from struct.
  - **Leave:** Harmless.

---

## Category B: Built but never wired into the runtime

These are complete implementations that were built but never connected to the
main request pipeline.

### B1. `expand_query_with_symbols` — query expansion (REMOVED in cleanup)
- **Status:** ✅ Removed during cleanup — was dead code.
- **Note:** If query quality needs improvement later, this can be re-implemented
  and wired into the tantivy search pipeline.

### B2. `is_valid_file_path` — context filtering (REMOVED in cleanup)
- **Status:** ✅ Removed during cleanup — was dead code.
- **Note:** If context quality needs improvement, re-implement and wire into
  the retrieval ranking step.

### B3. Adapter layer (UDS/MessagePack server) — RESOLVED: HTTP-only
- **What was considered:** A full UDS + MessagePack IPC server alongside the
  HTTP server.
- **Current state:** ✅ Resolved — the daemon is **HTTP-only**. `adapter.rs` was
  removed; `POST /hook` + `POST /mcp` on the axum listener (`http_server.rs`)
  are the single transport on every OS.
- **Decision:** HTTP only — no dual transport to maintain.

---

## Category C: Optional enhancements (built infrastructure, not wired)

### C1. Prometheus metrics + alerting
- **Files:** `crates/knocode-daemon/src/metrics.rs`
- **What:** The daemon exposes `GET /metrics` (Prometheus exposition: readiness,
  index_files, build duration histogram) and metrics via the `Probe` response.
  Prometheus alerting rules were written but the monitoring stack was never
  deployed.
- **Options:**
  - **Deploy alerting:** Set up Prometheus + Grafana (requires infrastructure).
  - **Skip:** Metrics are available via `GET /metrics`, `knocode doctor`, and the Probe endpoint.

### C2. Incremental re-indexing (file hash comparison)
- **Related to:** A4, A5
- **What:** The hash infrastructure was built but never used. On re-index,
  all files are re-processed even if unchanged.
- **Options:**
  - **Implement:** Compare file hashes, skip unchanged files. Faster re-index.
  - **Skip:** Full re-index is fast enough for now.

### C3. Import dependency graph
- **Related to:** A3
- **What:** The import pattern regex was built but never used. Could build a
  graph of which files import which, improving context ranking.
- **Options:**
  - **Implement:** Parse imports, build graph, use in retrieval ranking.
  - **Skip:** Current retrieval is good enough without it.

### C4. Gemini CLI adapter
- **What:** A bash hook adapter for Gemini CLI was prototyped but never connected.
- **Current state:** Gemini could be supported via MCP by adding it to the agent
  catalog in the installer (current catalog: `opencode`, `copilot`).
- **Options:**
  - **Add via MCP:** Add "gemini" to the agent catalog, wire `~/.gemini/settings.json`.
  - **Skip:** Gemini support not prioritized.

---

## Category D: Documentation / housekeeping

### D1. Completed V1 plan documents
- **Files:** `docs/00-project/V1_PLAN.md`, `V1_MINIMAL_STACK_PLAN.md`, `V1_FIX_PLAN_0_8_1.md`
- **What:** V1 is shipped. These plans are historical.
- **Options:**
  - **Archive:** Move to `docs/archive/`.
  - **Delete:** Remove entirely (CHANGELOG has the history).
  - **Leave:** Harmless, but adds noise to docs index.

### D2. Removal ADRs
- **Files:** `docs/01-architecture/ENGRAM_CBM_REMOVAL.md`, `FLASHRANK_REMOVAL.md`, `LLM_ROUTING_REMOVAL.md`, `REMOVED_TOOLS.md`
- **What:** Formal decision records for removed features.
- **Options:**
  - **Keep:** They document why things were removed (useful for new contributors).
  - **Consolidate:** Merge into a single `REMOVED.md`.
  - **Delete:** CHANGELOG has the history.

### D3. `docs/00-project/LLM_ROUTING_REMOVAL_PLAN.md`
- **What:** Plan for removing LLM routing. Already done.
- **Options:**
  - **Delete:** Superseded by `REMOVED_TOOLS.md`.
  - **Keep:** Historical record.

---

## Summary

| Category | Count | Effort | Impact |
|---|---|---|---|
| A: Dead code (keep/remove/wire) | 5 | Low | Low |
| B: Built but not wired | 1 (adapter) | Medium | Medium |
| C: Optional enhancements | 4 | Medium-High | Medium |
| D: Documentation | 3 | Low | Low |

**Recommendation:** Start with A (decide wire vs remove for each field), then D
(quick cleanup), then evaluate C based on user-facing needs.
