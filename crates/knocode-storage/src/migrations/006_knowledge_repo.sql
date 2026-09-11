-- Migration 006: repository-scoped knowledge (TASK-030 / F-1) + idempotent ingestion cleanup (TASK-032 / F-3)
-- 1. Stamp every existing row as legacy/global ('') — new ingestion always stamps the owning repo.
ALTER TABLE knowledge ADD COLUMN repository_id TEXT NOT NULL DEFAULT '';

-- 2. Rebuild with (category, key, repository_id) uniqueness so re-indexing never grows the table.
CREATE TABLE knowledge_v2 (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    category TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 0.5,
    source TEXT NOT NULL DEFAULT 'manual',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    repository_id TEXT NOT NULL DEFAULT '',
    UNIQUE(category, key, repository_id)
);

-- Collapse pre-existing duplicate (category,key) rows keeping the highest-confidence copy
-- (SQLite bare-column rule: non-aggregated columns come from the MAX(confidence) row).
INSERT INTO knowledge_v2 (category, key, value, confidence, source, created_at, updated_at, repository_id)
SELECT category, key, value, MAX(confidence), source, created_at, updated_at, repository_id
FROM knowledge
GROUP BY category, key, repository_id;

DROP TABLE knowledge;
ALTER TABLE knowledge_v2 RENAME TO knowledge;

CREATE INDEX IF NOT EXISTS idx_knowledge_category ON knowledge(category);
CREATE INDEX IF NOT EXISTS idx_knowledge_key ON knowledge(key);
CREATE INDEX IF NOT EXISTS idx_knowledge_confidence ON knowledge(confidence);
CREATE INDEX IF NOT EXISTS idx_knowledge_repo ON knowledge(repository_id);
