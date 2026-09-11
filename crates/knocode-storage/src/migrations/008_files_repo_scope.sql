-- Migration 008: scope the `files` table per repository.
--
-- `path` alone was globally UNIQUE, so every re-index of repo A treated repo B's
-- rows as "files deleted from A" and erased them one delete_file() at a time
-- (observed live: a mattermost daemon deleting 53k DefinitelyTyped rows, and 87
-- knocode-home rows earlier). Rebuild the table with (path, repository_id)
-- uniqueness so each repo owns its own rows.
--
-- Existing rows get repository_id = '' (legacy/global bucket); they are re-owned
-- by the next repo that indexes them under the new scoping.

-- Drop leftovers from a partially-applied earlier attempt (pre-transactional
-- runs could die between statements): always rebuild from the intact source table.
DROP TABLE IF EXISTS files_v2;

CREATE TABLE IF NOT EXISTS files_v2 (
    id INTEGER PRIMARY KEY AUTOINCREMENT,
    path TEXT NOT NULL,
    repository_id TEXT NOT NULL DEFAULT '',
    hash TEXT NOT NULL,
    size INTEGER NOT NULL,
    language TEXT,
    last_indexed_at TEXT NOT NULL,
    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
    UNIQUE(path, repository_id)
);

INSERT INTO files_v2 (id, path, repository_id, hash, size, language, last_indexed_at, created_at)
    SELECT id, path, '', hash, size, language, last_indexed_at, created_at FROM files;

DROP TABLE files;
ALTER TABLE files_v2 RENAME TO files;

CREATE INDEX IF NOT EXISTS idx_files_path ON files(path);
CREATE INDEX IF NOT EXISTS idx_files_hash ON files(hash);
CREATE INDEX IF NOT EXISTS idx_files_repo ON files(repository_id);
