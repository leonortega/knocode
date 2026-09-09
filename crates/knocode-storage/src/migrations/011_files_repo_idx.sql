-- Migration 011: index files.repository_id.
--
-- `Database::find_symbols_scoped` resolves candidate symbols via
--   SELECT ... FROM files f JOIN symbols s ON s.file_id = f.id
--   WHERE f.repository_id = ?1 ...
-- Without an index on files.repository_id that predicate degrades to a full
-- files scan (63k rows on this machine, one per repo ever indexed) for every
-- symbol lookup. The index makes the repo→files fan-out O(log n + k).

CREATE INDEX IF NOT EXISTS idx_files_repository_id ON files(repository_id);
