-- Migration 009: repository registry — maps repository_id ↔ on-disk path.
--
-- The multi-repo daemon indexes repos lazily on first request; the registry is
-- how the runtime (and `prune_stale_repos`) knows WHICH path a repository_id
-- belongs to. repository_id is a one-way hash, so without this table a stale
-- repo's data can never be identified for cleanup.

CREATE TABLE IF NOT EXISTS repo_registry (
    repository_id TEXT PRIMARY KEY,
    path TEXT NOT NULL,
    registered_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
);
