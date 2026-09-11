-- Migration 003: Dependency graph + cost column (v0.3.0 spec §3, ROADMAP.md:81)
-- Dependency graph edges (AST-derived, local)
CREATE TABLE IF NOT EXISTS edges (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    from_path TEXT NOT NULL,
    to_path TEXT NOT NULL,
    kind TEXT NOT NULL DEFAULT 'import',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(from_path, to_path)
);
CREATE INDEX IF NOT EXISTS idx_edges_from ON edges(from_path);
CREATE INDEX IF NOT EXISTS idx_edges_to ON edges(to_path);

-- Token usage cost tracking (LiteLLM per-key budgets)
ALTER TABLE token_usage ADD COLUMN cost_usd REAL NOT NULL DEFAULT 0.0;
