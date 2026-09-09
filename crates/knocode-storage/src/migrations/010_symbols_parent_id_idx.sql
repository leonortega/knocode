-- Migration 010: index symbols.parent_id.
--
-- FK enforcement is ON at runtime (re-enabled after every migration), and
-- symbols.parent_id REFERENCES symbols(id) with NO index: every symbol DELETE
-- triggered a full table scan of symbols PER ROW to verify the self-FK — mass
-- cleanups (stale-repo GC on a 53k-file repo) took minutes-to-hours holding
-- the SQLite write lock. The index makes FK checks O(log n).

CREATE INDEX IF NOT EXISTS idx_symbols_parent_id ON symbols(parent_id);
