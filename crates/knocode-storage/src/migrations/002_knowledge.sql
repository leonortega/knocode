-- Migration 002: Knowledge Hub tables
-- Knowledge entries for project docs, patterns, conventions, decisions
CREATE TABLE IF NOT EXISTS knowledge (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    category TEXT NOT NULL,           -- convention, pattern, domain, decision
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    confidence REAL NOT NULL DEFAULT 0.5,
    source TEXT NOT NULL DEFAULT 'manual',
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(category, key)
);

-- Memory entries (SQLite local)
CREATE TABLE IF NOT EXISTS memory (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    namespace TEXT NOT NULL,
    key TEXT NOT NULL,
    value TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(namespace, key)
);

-- Indexes for knowledge
CREATE INDEX IF NOT EXISTS idx_knowledge_category ON knowledge(category);
CREATE INDEX IF NOT EXISTS idx_knowledge_key ON knowledge(key);
CREATE INDEX IF NOT EXISTS idx_knowledge_confidence ON knowledge(confidence);
CREATE INDEX IF NOT EXISTS idx_memory_namespace ON memory(namespace);
