//! Storage layer — SQLite persistence backbone for Knocode.
//!
//! This crate owns:
//! - **Database:** SQLite connection with WAL mode, migrations, and CRUD for files, symbols, knowledge, and sessions.
//! - **TantivyIndex:** Full-text BM25 search index (in-process Tantivy with MmapDirectory).
//!
//! SQLite stores all structured metadata (file hashes, AST symbols, knowledge entries, dependency edges,
//! token usage). Tantivy is the search index for fast full-text retrieval. Both are built from the
//! same source code walk during `knocode init` and kept in sync during incremental updates.

pub mod tantivy_index;

use std::path::Path;
use std::time::Instant;

use chrono::Utc;
use knocode_events::{EventBus, RuntimeEvent};
use rusqlite::{params, Connection};
use tracing::{debug, info, warn};

/// Database wrapper for SQLite operations
///
/// Note: Database is !Send and !Sync due to rusqlite::Connection.
/// This is fine for single-process daemon use. Each component that
/// needs database access opens its own connection.
pub struct Database {
    conn: Connection,
    /// Original file path of the backing store (None for in-memory `:memory:` databases).
    /// Lets other components re-open the same SQLite file from a different connection.
    db_path: Option<std::path::PathBuf>,
}

impl Database {
    /// Open or create a database at the given path
    pub fn open(path: &Path) -> Result<Self, String> {
        let conn = Connection::open(path).map_err(|e| format!("Failed to open database: {}", e))?;

        // Enable WAL mode for concurrent reads
        conn.execute_batch("PRAGMA journal_mode=WAL;")
            .map_err(|e| format!("Failed to set WAL mode: {}", e))?;

        // Wait for the write lock instead of failing instantly: `knocode init` and
        // the daemon both write this database, and without a busy timeout the two
        // SQLITE_BUSY-retry against each other — a reindex can crawl for tens of
        // minutes and then fail-open, discarding all its work. 10s covers lock
        // handoffs; batched writes here keep transactions short anyway.
        conn.execute_batch("PRAGMA busy_timeout=10000; PRAGMA synchronous=NORMAL;")
            .map_err(|e| format!("Failed to set PRAGMAs: {}", e))?;

        // `:memory:` is not a real file — record nothing so callers can tell the
        // store is ephemeral (retrieval-only instances, tests).
        let db_path = (path != std::path::Path::new(":memory:")).then(|| path.to_path_buf());
        let db = Self { conn, db_path };
        db.run_migrations()?;
        Ok(db)
    }

    /// Path of the backing SQLite file, if this connection is file-backed.
    /// Returns `None` for in-memory (`:memory:`) stores.
    pub fn path(&self) -> Option<&std::path::Path> {
        self.db_path.as_deref()
    }

    /// Run all pending migrations
    fn run_migrations(&self) -> Result<(), String> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS schema_migrations (
                    id INTEGER PRIMARY KEY,
                    name TEXT NOT NULL UNIQUE,
                    applied_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                );",
            )
            .map_err(|e| format!("Failed to create migration table: {}", e))?;

        // Migration 001
        let migration_001 = include_str!("migrations/001_initial.sql");
        self.apply_migration("001_initial", migration_001)?;

        // Migration 002: Knowledge Hub
        let migration_002 = include_str!("migrations/002_knowledge.sql");
        self.apply_migration("002_knowledge", migration_002)?;

        // Migration 003: Dependency graph + cost
        let migration_003 = include_str!("migrations/003_graph.sql");
        self.apply_migration("003_graph", migration_003)?;

        // Migration 006: repository-scoped knowledge (TASK-030) + dedup cleanup (TASK-032)
        let migration_006 = include_str!("migrations/006_knowledge_repo.sql");
        self.apply_migration("006_knowledge_repo", migration_006)?;

        // Migration 007: drop vestigial token_usage model/tier columns (Model Router removed)
        let migration_007 = include_str!("migrations/007_drop_vestigial.sql");
        self.apply_migration("007_drop_vestigial", migration_007)?;

        // Migration 008: scope the files table per repository — without it, every
        // re-index of repo A deleted repo B's rows (globally-unique `path`).
        let migration_008 = include_str!("migrations/008_files_repo_scope.sql");
        self.apply_migration("008_files_repo_scope", migration_008)?;

        // Migration 009: repository registry (repository_id ↔ path) — the multi-repo
        // daemon's lazy indexing + stale-repo GC need the path behind each hash.
        let migration_009 = include_str!("migrations/009_repo_registry.sql");
        self.apply_migration("009_repo_registry", migration_009)?;

        // Migration 010: index symbols.parent_id. FK enforcement is ON at runtime
        // (re-enabled after every migration), and symbols.parent_id REFERENCES
        // symbols(id) with NO index: every symbol DELETE triggered a full table scan
        // of symbols PER ROW to verify the self-FK — mass cleanups (stale-repo GC on
        // a 53k-file repo) took minutes-to-hours holding the SQLite write lock. The
        // index makes FK checks O(log n).
        let migration_010 = include_str!("migrations/010_symbols_parent_id_idx.sql");
        self.apply_migration("010_symbols_parent_id_idx", migration_010)?;

        // Migration 011: index files.repository_id — find_symbols_scoped joins
        // symbols→files filtered by repository_id; without the index every symbol
        // lookup scans the whole files table (63k rows across all repos).
        let migration_011 = include_str!("migrations/011_files_repo_idx.sql");
        self.apply_migration("011_files_repo_idx", migration_011)?;

        // v1: 004_events and 005_audits removed per TASK-002/TASK-001 — preserved in future/workflow/migrations/
        // Event persistence (ring buffer + SQLite) and workflow audits are NOT part of v1 hot path.
        // v1 keeps only tracing + metrics + correlation IDs (EventBus is in-memory only).

        Ok(())
    }

    fn apply_migration(&self, name: &str, sql: &str) -> Result<(), String> {
        let already_applied: bool = self
            .conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM schema_migrations WHERE name = ?1",
                params![name],
                |row| row.get(0),
            )
            .map_err(|e| format!("Failed to check migration status: {}", e))?;

        if already_applied {
            return Ok(());
        }

        // Migrations that REBUILD a FK-referenced table (008 drops `files` and
        // renames `files_v2` back while `symbols` still holds a FOREIGN KEY →
        // files.id) fail the DROP the moment FK enforcement is ON (rusqlite's
        // default). The rebuild copies `id`s verbatim, so every foreign key stays
        // valid AFTER the rebuild — we only need enforcement OFF across the
        // migration window. PRAGMA foreign_keys must run outside a transaction,
        // which holds here (the status SELECT above left none open).
        self.conn
            .execute_batch("PRAGMA foreign_keys=OFF;")
            .map_err(|e| format!("Migration '{}': could not suspend foreign_keys: {}", name, e))?;

        // Transactional: a migration killed mid-way (e.g. daemon killed during a
        // slow table rebuild) must roll back cleanly, or the rerun hits partial
        // state (e.g. duplicate rows in a rebuilt table) and fails forever.
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| format!("Migration '{}': begin failed: {}", name, e))?;
        if let Err(e) = self.conn.execute_batch(sql) {
            let _ = self.conn.execute_batch("ROLLBACK");
            let _ = self.conn.execute_batch("PRAGMA foreign_keys=ON;");
            return Err(format!("Migration '{}' failed: {}", name, e));
        }

        if let Err(e) = self.conn.execute(
            "INSERT INTO schema_migrations (name) VALUES (?1)",
            params![name],
        ) {
            let _ = self.conn.execute_batch("ROLLBACK");
            let _ = self.conn.execute_batch("PRAGMA foreign_keys=ON;");
            return Err(format!("Failed to record migration '{}': {}", name, e));
        }

        self.conn
            .execute_batch("COMMIT")
            .map_err(|e| format!("Migration '{}': commit failed: {}", name, e))?;
        self.conn
            .execute_batch("PRAGMA foreign_keys=ON;")
            .map_err(|e| format!("Migration '{}': could not re-enable foreign_keys: {}", name, e))?;

        info!(migration = name, "Applied migration");
        Ok(())
    }

    // ── Files ───────────────────────────────────────────────────────

    /// Insert a new file record
    pub fn insert_file(
        &self,
        path: &str,
        hash: &str,
        size: i64,
        language: Option<&str>,
        repository_id: &str,
    ) -> Result<i64, String> {
        let start = Instant::now();
        self.conn
            .execute(
                "INSERT INTO files (path, repository_id, hash, size, language, last_indexed_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![path, repository_id, hash, size, language, Utc::now().to_rfc3339()],
            )
            .map_err(|e| format!("Failed to insert file: {}", e))?;
        let id = self.conn.last_insert_rowid();
        log_slow("insert_file", start);
        Ok(id)
    }

    /// Update an existing file's hash and size
    pub fn update_file(&self, id: i64, hash: &str, size: i64) -> Result<(), String> {
        let start = Instant::now();
        self.conn
            .execute(
                "UPDATE files SET hash = ?1, size = ?2, last_indexed_at = ?3 WHERE id = ?4",
                params![hash, size, Utc::now().to_rfc3339(), id],
            )
            .map_err(|e| format!("Failed to update file: {}", e))?;
        log_slow("update_file", start);
        Ok(())
    }

    /// Delete a file by path within one repository — cascades to symbols
    /// (TASK-010: stale symbols must disappear). Repository-scoped: a path may
    /// exist in several repos, and re-indexing repo A must never touch repo B's rows.
    pub fn delete_file(&self, path: &str, repository_id: &str) -> Result<(), String> {
        let start = Instant::now();
        // Delete symbols first to avoid FOREIGN KEY constraint (symbols.file_id → files.id)
        let _ = self.conn.execute(
            "DELETE FROM symbols WHERE file_id IN (SELECT id FROM files WHERE path = ?1 AND repository_id = ?2)",
            params![path, repository_id],
        );
        self.conn
            .execute(
                "DELETE FROM files WHERE path = ?1 AND repository_id = ?2",
                params![path, repository_id],
            )
            .map_err(|e| format!("Failed to delete file: {}", e))?;
        log_slow("delete_file", start);
        Ok(())
    }

    /// Get all files as (path, hash) pairs
    pub fn get_all_files(&self) -> Result<Vec<(String, String)>, String> {
        let start = Instant::now();
        let mut stmt = self
            .conn
            .prepare("SELECT path, hash FROM files")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let files = stmt
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .map_err(|e| format!("Failed to query files: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect files: {}", e))?;

        log_slow("get_all_files", start);
        Ok(files)
    }

    /// Get all files with full meta for ONE repository (incremental mtime+size shortcut — Phase2)
    pub fn get_all_files_meta(&self, repository_id: &str) -> Result<Vec<FileRecord>, String> {
        let start = Instant::now();
        let mut stmt = self
            .conn
            .prepare("SELECT id, path, hash, size, language, last_indexed_at FROM files WHERE repository_id = ?1")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;
        let files = stmt
            .query_map(params![repository_id], |row| {
                Ok(FileRecord {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    hash: row.get(2)?,
                    size: row.get(3)?,
                    language: row.get(4)?,
                    last_indexed_at: row.get(5)?,
                })
            })
            .map_err(|e| format!("Failed to query files: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect files: {}", e))?;
        log_slow("get_all_files_meta", start);
        Ok(files)
    }

    /// Get a file by path
    pub fn get_file(&self, path: &str) -> Result<Option<FileRecord>, String> {
        let start = Instant::now();
        let result = self.conn.query_row(
            "SELECT id, path, hash, size, language, last_indexed_at FROM files WHERE path = ?1",
            params![path],
            |row| {
                Ok(FileRecord {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    hash: row.get(2)?,
                    size: row.get(3)?,
                    language: row.get(4)?,
                    last_indexed_at: row.get(5)?,
                })
            },
        );

        log_slow("get_file", start);
        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to get file: {}", e)),
        }
    }

    /// Get a file record by ID
    pub fn get_file_by_id(&self, id: i64) -> Result<Option<FileRecord>, String> {
        let start = Instant::now();
        let result = self.conn.query_row(
            "SELECT id, path, hash, size, language, last_indexed_at FROM files WHERE id = ?1",
            params![id],
            |row| {
                Ok(FileRecord {
                    id: row.get(0)?,
                    path: row.get(1)?,
                    hash: row.get(2)?,
                    size: row.get(3)?,
                    language: row.get(4)?,
                    last_indexed_at: row.get(5)?,
                })
            },
        );

        log_slow("get_file_by_id", start);
        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to get file by id: {}", e)),
        }
    }

    /// Get the count of indexed files (global — use `get_file_count_for_repo` for per-repo)
    pub fn get_file_count(&self) -> Result<usize, String> {
        self.conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get::<_, i64>(0))
            .map(|v| v as usize)
            .map_err(|e| format!("Failed to count files: {}", e))
    }

    /// Count indexed files for ONE repository (daemon readiness / index_files gauge)
    pub fn get_file_count_for_repo(&self, repository_id: &str) -> Result<usize, String> {
        self.conn
            .query_row(
                "SELECT COUNT(*) FROM files WHERE repository_id = ?1",
                params![repository_id],
                |row| row.get::<_, i64>(0),
            )
            .map(|v| v as usize)
            .map_err(|e| format!("Failed to count files for repo: {}", e))
    }

    // ── Repository registry (migration 009) ─────────────────────────

    /// Upsert the path behind a repository_id. Called whenever the daemon indexes
    /// (eagerly or lazily) a repository so stale-repo GC can identify the data.
    pub fn register_repo(&self, repository_id: &str, path: &str) -> Result<(), String> {
        self.conn
            .execute(
                "INSERT INTO repo_registry (repository_id, path) VALUES (?1, ?2)
                 ON CONFLICT(repository_id) DO UPDATE SET path = excluded.path",
                params![repository_id, path],
            )
            .map(|_| ())
            .map_err(|e| format!("Failed to register repository: {}", e))
    }

    /// All registered repositories as (repository_id, path), oldest first.
    pub fn list_registered_repos(&self) -> Result<Vec<(String, String)>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT repository_id, path FROM repo_registry ORDER BY registered_at")
            .map_err(|e| format!("Failed to list registered repos: {}", e))?;
        let rows = stmt
            .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
            .map_err(|e| format!("Failed to query registered repos: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read registered repos: {}", e))?;
        Ok(rows)
    }

    /// Distinct repository_ids present in the files table (registered or not —
    /// used to find legacy/orphan rows from pre-registry runs).
    pub fn list_file_repo_ids(&self) -> Result<Vec<String>, String> {
        let mut stmt = self
            .conn
            .prepare("SELECT DISTINCT repository_id FROM files WHERE repository_id != ''")
            .map_err(|e| format!("Failed to list file repo ids: {}", e))?;
        let rows = stmt
            .query_map([], |row| row.get::<_, String>(0))
            .map_err(|e| format!("Failed to query file repo ids: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to read file repo ids: {}", e))?;
        Ok(rows)
    }

    /// Delete all file + symbol rows for ONE repository. Returns the number of
    /// file rows removed (symbols go with them via the file_id join).
    pub fn drop_repo_data(&self, repository_id: &str) -> Result<usize, String> {
        self.conn
            .execute(
                "DELETE FROM symbols WHERE file_id IN (SELECT id FROM files WHERE repository_id = ?1)",
                params![repository_id],
            )
            .map_err(|e| format!("Failed to drop symbols for repo: {}", e))?;
        self.conn
            .execute(
                "DELETE FROM files WHERE repository_id = ?1",
                params![repository_id],
            )
            .map(|v| v as usize)
            .map_err(|e| format!("Failed to drop files for repo: {}", e))
    }

    /// Remove a registry entry (after its data has been dropped).
    pub fn delete_repo_registry_entry(&self, repository_id: &str) -> Result<(), String> {
        self.conn
            .execute(
                "DELETE FROM repo_registry WHERE repository_id = ?1",
                params![repository_id],
            )
            .map(|_| ())
            .map_err(|e| format!("Failed to delete registry entry: {}", e))
    }

    // ── Symbols ─────────────────────────────────────────────────────

    /// Insert a symbol
    pub fn insert_symbol(
        &self,
        file_id: i64,
        name: &str,
        kind: &str,
        line_start: i64,
        line_end: i64,
        parent_id: Option<i64>,
    ) -> Result<i64, String> {
        let start = Instant::now();
        self.conn
            .execute(
                "INSERT INTO symbols (file_id, name, kind, line_start, line_end, parent_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![file_id, name, kind, line_start, line_end, parent_id],
            )
            .map_err(|e| format!("Failed to insert symbol: {}", e))?;
        let id = self.conn.last_insert_rowid();
        log_slow("insert_symbol", start);
        Ok(id)
    }

    /// Batch insert symbols for a file — single prepared statement, faster than per-row insert_symbol.
    /// Uses a savepoint so it works inside an existing batch transaction.
    pub fn insert_symbols_batch(
        &self,
        file_id: i64,
        symbols: &[(String, String)], // [(name, kind)]
    ) -> Result<(), String> {
        if symbols.is_empty() {
            return Ok(());
        }
        let start = Instant::now();
        // Use savepoint (nested transaction) so we work inside begin_batch
        self.conn.execute_batch("SAVEPOINT batch_sym")
            .map_err(|e| format!("batch savepoint: {e}"))?;
        let result = (|| {
            let sql = "INSERT INTO symbols (file_id, name, kind, line_start, line_end, parent_id) VALUES (?1, ?2, ?3, 0, 0, NULL)";
            let mut stmt = self.conn.prepare(sql)
                .map_err(|e| format!("batch prepare: {e}"))?;
            for (name, kind) in symbols {
                stmt.execute(rusqlite::params![file_id, name, kind])
                    .map_err(|e| format!("batch insert: {e}"))?;
            }
            Ok::<(), String>(())
        })();
        match result {
            Ok(()) => {
                self.conn.execute_batch("RELEASE SAVEPOINT batch_sym")
                    .map_err(|e| format!("batch release: {e}"))?;
            }
            Err(e) => {
                let _ = self.conn.execute_batch("ROLLBACK TO SAVEPOINT batch_sym");
                return Err(e);
            }
        }
        log_slow(&format!("insert_symbols_batch ({} symbols)", symbols.len()), start);
        Ok(())
    }

    /// Get all symbols for a file
    pub fn get_symbols_for_file(&self, file_id: i64) -> Result<Vec<Symbol>, String> {
        let start = Instant::now();
        let mut stmt = self
            .conn
            .prepare("SELECT id, file_id, name, kind, line_start, line_end, parent_id FROM symbols WHERE file_id = ?1")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let symbols = stmt
            .query_map(params![file_id], |row| {
                Ok(Symbol {
                    id: row.get(0)?,
                    file_id: row.get(1)?,
                    name: row.get(2)?,
                    kind: row.get(3)?,
                    line_start: row.get(4)?,
                    line_end: row.get(5)?,
                    parent_id: row.get(6)?,
                })
            })
            .map_err(|e| format!("Failed to query symbols: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect symbols: {}", e))?;

        log_slow("get_symbols_for_file", start);
        Ok(symbols)
    }

    /// Find symbols by name (partial match)
    pub fn find_symbol(&self, name: &str) -> Result<Vec<Symbol>, String> {
        let start = Instant::now();
        let pattern = format!("%{}%", name);
        let mut stmt = self
            .conn
            .prepare("SELECT id, file_id, name, kind, line_start, line_end, parent_id FROM symbols WHERE name LIKE ?1")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let symbols = stmt
            .query_map(params![pattern], |row| {
                Ok(Symbol {
                    id: row.get(0)?,
                    file_id: row.get(1)?,
                    name: row.get(2)?,
                    kind: row.get(3)?,
                    line_start: row.get(4)?,
                    line_end: row.get(5)?,
                    parent_id: row.get(6)?,
                })
            })
            .map_err(|e| format!("Failed to query symbols: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect symbols: {}", e))?;

        log_slow("find_symbol", start);
        Ok(symbols)
    }

    /// Repository-scoped, bounded symbol lookup — hot path for retrieval.
    ///
    /// Fixes the v1 regression where `find_symbol("how to add error handling")`
    /// ran `LIKE '%<whole sentence>%'` over EVERY repository's symbols with no
    /// LIMIT (13-22ms per query against 236k symbols). This version:
    /// 1. Tokenizes the query and ORs per-token LIKE patterns (SQLite can only
    ///    use a range scan for the FIRST pattern; ORs let the optimizer pick one
    ///    token instead of a full scan on the whole sentence).
    /// 2. Restricts candidates to one repository via a files JOIN on an indexed
    ///    `repository_id` (migration 011) — no cross-repo pollution.
    /// 3. `LIMIT max_results` — no unbounded result materialization.
    ///
    /// Returns symbols whose name contains ANY query token (bm25-style broad
    /// recall; ranking happens in the retrieval layer).
    pub fn find_symbols_scoped(
        &self,
        query: &str,
        repository_id: &str,
        max_results: usize,
    ) -> Result<Vec<Symbol>, String> {
        let start = Instant::now();

        // Tokenize: alphanumeric runs ≥ 3 chars (2-char tokens like "db" match
        // thousands of rows and dominate the OR cost for no ranking value).
        let mut tokens: Vec<String> = Vec::new();
        let mut cur = String::new();
        let mut push_token = |t: &mut String| {
            if t.len() >= 3 && !tokens.contains(t) {
                tokens.push(t.clone());
            }
            t.clear();
        };
        for ch in query.chars() {
            if ch.is_alphanumeric() || ch == '_' {
                cur.push(ch);
            } else {
                push_token(&mut cur);
            }
        }
        push_token(&mut cur);
        // Cap OR arms — more terms can't help and each is a potential scan.
        tokens.truncate(6);

        let symbols = if tokens.is_empty() {
            // No usable token (e.g. numeric/symbol-only query): return a small
            // bounded slice rather than an unbounded scan.
            let symbols = {
                let mut stmt = self
                    .conn
                    .prepare(
                        "SELECT s.id, s.file_id, s.name, s.kind, s.line_start, s.line_end, s.parent_id
                         FROM symbols s
                         JOIN files f ON f.id = s.file_id
                         WHERE f.repository_id = ?1
                         LIMIT ?2",
                    )
                    .map_err(|e| format!("Failed to prepare scoped symbol query: {}", e))?;
                let mapped = stmt.query_map(params![repository_id, max_results as i64], |row| {
                    Ok(Symbol {
                        id: row.get(0)?,
                        file_id: row.get(1)?,
                        name: row.get(2)?,
                        kind: row.get(3)?,
                        line_start: row.get(4)?,
                        line_end: row.get(5)?,
                        parent_id: row.get(6)?,
                    })
                })
                .map_err(|e| format!("Failed to query scoped symbols: {}", e))?;
                let collected = mapped
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| format!("Failed to collect scoped symbols: {}", e))?;
                collected
            };
            symbols
        } else {
            // OR-join per-token LIKE patterns against the indexed symbols.name.
            // Placeholders are numbered explicitly (?1..?n, then repo, then limit):
            // mixing an explicit ?N BEFORE anonymous ? markers makes SQLite
            // auto-number the anonymous ones from N+1, breaking the binding order.
            let arms: Vec<String> = (1..=tokens.len())
                .map(|i| format!("s.name LIKE ?{i} ESCAPE '\\'"))
                .collect();
            let where_clause = arms.join(" OR ");
            let sql = format!(
                "SELECT s.id, s.file_id, s.name, s.kind, s.line_start, s.line_end, s.parent_id
                 FROM symbols s
                 JOIN files f ON f.id = s.file_id
                 WHERE f.repository_id = ?{repo} AND ({where_clause})
                 LIMIT ?{lim}",
                repo = tokens.len() + 1,
                lim = tokens.len() + 2,
                where_clause = where_clause,
            );
            // Bind: one escaped LIKE pattern per token, then repo_id, then limit.
            let pattern_for = |t: &str| format!("%{}%", t.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_"));
            let mut params_vec: Vec<String> = tokens.iter().map(|t| pattern_for(t)).collect();
            params_vec.push(repository_id.to_string());
            params_vec.push(max_results.to_string());
            let symbols = {
                let mut stmt = self
                    .conn
                    .prepare(&sql)
                    .map_err(|e| format!("Failed to prepare scoped symbol query: {}", e))?;
                let param_refs: Vec<&dyn rusqlite::ToSql> =
                    params_vec.iter().map(|s| s as &dyn rusqlite::ToSql).collect();
                let mapped = stmt
                    .query_map(param_refs.as_slice(), |row| {
                        Ok(Symbol {
                            id: row.get(0)?,
                            file_id: row.get(1)?,
                            name: row.get(2)?,
                            kind: row.get(3)?,
                            line_start: row.get(4)?,
                            line_end: row.get(5)?,
                            parent_id: row.get(6)?,
                        })
                    })
                    .map_err(|e| format!("Failed to query scoped symbols: {}", e))?;
                let collected = mapped
                    .collect::<Result<Vec<_>, _>>()
                    .map_err(|e| format!("Failed to collect scoped symbols: {}", e))?;
                collected
            };
            symbols
        };

        log_slow("find_symbols_scoped", start);
        Ok(symbols)
    }

    /// Get the count of symbols
    pub fn get_symbol_count(&self) -> Result<usize, String> {
        self.conn
            .query_row("SELECT COUNT(*) FROM symbols", [], |row| row.get::<_, i64>(0))
            .map(|v| v as usize)
            .map_err(|e| format!("Failed to count symbols: {}", e))
    }

    /// Count symbols belonging to a set of file IDs (chunked IN queries — the ids
    /// come from one repo's `files` rows). Used to detect a symbol-less index left
    /// by an interrupted run: Phase1 commits file records BEFORE extraction, so a
    /// killed run leaves rows whose files the mtime+size shortcut would then skip
    /// forever, keeping the index silently crippled at 0 symbols.
    pub fn count_symbols_for_file_ids(&self, file_ids: &[i64]) -> Result<usize, String> {
        let mut total = 0usize;
        for chunk in file_ids.chunks(500) {
            let placeholders = vec!["?"; chunk.len()].join(",");
            let sql = format!(
                "SELECT COUNT(*) FROM symbols WHERE file_id IN ({})",
                placeholders
            );
            let params: Vec<&dyn rusqlite::ToSql> =
                chunk.iter().map(|id| id as &dyn rusqlite::ToSql).collect();
            let n: i64 = self
                .conn
                .query_row(&sql, params.as_slice(), |row| row.get(0))
                .map_err(|e| format!("Failed to count symbols for file ids: {}", e))?;
            total += n as usize;
            if total > 0 {
                return Ok(total); // early exit — only zero-vs-nonzero matters
            }
        }
        Ok(total)
    }

    // ── Token Usage ─────────────────────────────────────────────────

    /// Record token usage
    pub fn insert_usage(
        &self,
        correlation_id: &str,
        request_type: &str,
        input_tokens: i64,
        output_tokens: i64,
    ) -> Result<(), String> {
        let start = Instant::now();
        self.conn
            .execute(
                "INSERT INTO token_usage (correlation_id, request_type, input_tokens, output_tokens) VALUES (?1, ?2, ?3, ?4)",
                params![correlation_id, request_type, input_tokens, output_tokens],
            )
            .map_err(|e| format!("Failed to insert usage: {}", e))?;
        log_slow("insert_usage", start);
        Ok(())
    }

    /// Get aggregated usage statistics
    pub fn get_usage_stats(&self) -> Result<UsageStats, String> {
        let start = Instant::now();
        let stats = self.conn.query_row(
            "SELECT COALESCE(SUM(input_tokens), 0), COALESCE(SUM(output_tokens), 0), COUNT(*) FROM token_usage",
            [],
            |row| {
                Ok(UsageStats {
                    total_input_tokens: row.get(0)?,
                    total_output_tokens: row.get(1)?,
                    total_requests: row.get(2)?,
                })
            },
        ).map_err(|e| format!("Failed to get usage stats: {}", e))?;
        log_slow("get_usage_stats", start);
        Ok(stats)
    }

    // ── Knowledge Operations ───────────────────────────────────────

    /// Store a knowledge entry — idempotent upsert on (category, key, repository_id) (TASK-032)
    pub fn store_knowledge(&self, category: &str, key: &str, value: &str, confidence: f64, source: &str, repository_id: &str) -> Result<i64, String> {
        let start = Instant::now();
        self.conn
            .execute(
                "INSERT INTO knowledge (category, key, value, confidence, source, repository_id, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                 ON CONFLICT(category, key, repository_id) DO UPDATE SET
                   value = excluded.value, confidence = excluded.confidence, source = excluded.source, updated_at = excluded.updated_at",
                params![category, key, value, confidence, source, repository_id, Utc::now().to_rfc3339()],
            )
            .map_err(|e| format!("Failed to store knowledge: {}", e))?;
        let id = self.conn.last_insert_rowid();
        log_slow("store_knowledge", start);
        Ok(id)
    }

    /// Get a knowledge entry by category and key
    pub fn get_knowledge(&self, category: &str, key: &str) -> Result<Option<KnowledgeRecord>, String> {
        let start = Instant::now();
        let result = self.conn.query_row(
            "SELECT id, category, key, value, confidence, source, created_at, updated_at, repository_id FROM knowledge WHERE category = ?1 AND key = ?2 ORDER BY confidence DESC LIMIT 1",
            params![category, key],
            |row| {
                Ok(KnowledgeRecord {
                    id: row.get(0)?,
                    category: row.get(1)?,
                    key: row.get(2)?,
                    value: row.get(3)?,
                    confidence: row.get(4)?,
                    source: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                    repository_id: row.get(8)?,
                })
            },
        );
        log_slow("get_knowledge", start);
        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to get knowledge: {}", e)),
        }
    }

    /// Get all knowledge entries
    pub fn get_all_knowledge(&self) -> Result<Vec<KnowledgeRecord>, String> {
        let start = Instant::now();
        let mut stmt = self
            .conn
            .prepare("SELECT id, category, key, value, confidence, source, created_at, updated_at, repository_id FROM knowledge ORDER BY confidence DESC")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let records = stmt
            .query_map([], |row| {
                Ok(KnowledgeRecord {
                    id: row.get(0)?,
                    category: row.get(1)?,
                    key: row.get(2)?,
                    value: row.get(3)?,
                    confidence: row.get(4)?,
                    source: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                    repository_id: row.get(8)?,
                })
            })
            .map_err(|e| format!("Failed to query knowledge: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect knowledge: {}", e))?;

        log_slow("get_all_knowledge", start);
        Ok(records)
    }

    /// Get knowledge entries by category
    pub fn get_knowledge_by_category(&self, category: &str) -> Result<Vec<KnowledgeRecord>, String> {
        let start = Instant::now();
        let mut stmt = self
            .conn
            .prepare("SELECT id, category, key, value, confidence, source, created_at, updated_at, repository_id FROM knowledge WHERE category = ?1 ORDER BY confidence DESC")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let records = stmt
            .query_map(params![category], |row| {
                Ok(KnowledgeRecord {
                    id: row.get(0)?,
                    category: row.get(1)?,
                    key: row.get(2)?,
                    value: row.get(3)?,
                    confidence: row.get(4)?,
                    source: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                    repository_id: row.get(8)?,
                })
            })
            .map_err(|e| format!("Failed to query knowledge: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect knowledge: {}", e))?;

        log_slow("get_knowledge_by_category", start);
        Ok(records)
    }

    /// Update confidence for a knowledge entry
    pub fn update_knowledge_confidence(&self, id: i64, confidence: f64) -> Result<(), String> {
        let start = Instant::now();
        self.conn
            .execute(
                "UPDATE knowledge SET confidence = ?1, updated_at = ?2 WHERE id = ?3",
                params![confidence, Utc::now().to_rfc3339(), id],
            )
            .map_err(|e| format!("Failed to update confidence: {}", e))?;
        log_slow("update_knowledge_confidence", start);
        Ok(())
    }

    /// Decay confidence for knowledge entries older than min_age_days
    pub fn decay_knowledge_confidence(&self, min_age_days: i64, decay_amount: f64) -> Result<usize, String> {
        let start = Instant::now();
        let cutoff = (Utc::now() - chrono::Duration::days(min_age_days)).to_rfc3339();
        let affected = self.conn
            .execute(
                "UPDATE knowledge SET confidence = MAX(0.1, confidence - ?1), updated_at = ?2 WHERE updated_at < ?3",
                params![decay_amount, Utc::now().to_rfc3339(), cutoff],
            )
            .map_err(|e| format!("Failed to decay confidence: {}", e))?;
        log_slow("decay_knowledge_confidence", start);
        Ok(affected)
    }

    /// Search knowledge by text (LIKE-based) — optionally scoped to a repository (TASK-030).
    /// `repository_filter: Some(id)` matches ONLY rows stamped with that id (strict — legacy ''
    /// rows never leak across repos). `None` returns everything.
    /// Count total knowledge entries (for hub initialization checks — P0 #3)
    pub fn count_knowledge(&self, repository_filter: Option<&str>) -> Result<usize, String> {
        let sql = if repository_filter.is_some() {
            "SELECT COUNT(*) FROM knowledge WHERE repository_id = ?1"
        } else {
            "SELECT COUNT(*) FROM knowledge"
        };
        let count: i64 = if let Some(repo) = repository_filter {
            self.conn.query_row(sql, params![repo], |row| row.get(0))
        } else {
            self.conn.query_row(sql, params![], |row| row.get(0))
        }
        .map_err(|e| format!("Failed to count knowledge: {}", e))?;
        Ok(count as usize)
    }

    pub fn search_knowledge(&self, query: &str, category_filter: Option<&str>, min_confidence: f64, max_results: usize, repository_filter: Option<&str>) -> Result<Vec<KnowledgeRecord>, String> {
        let start = Instant::now();
        let pattern = format!("%{}%", query);

        let mut records = Vec::new();

        // Build parameterized SQL manually to keep the existing filter shapes
        if let Some(cat) = category_filter {
            let sql_with_repo = "SELECT id, category, key, value, confidence, source, created_at, updated_at, repository_id FROM knowledge WHERE (key LIKE ?1 OR value LIKE ?1) AND category = ?2 AND confidence >= ?3 AND repository_id = ?4 ORDER BY confidence DESC LIMIT ?5";
            let sql_plain   = "SELECT id, category, key, value, confidence, source, created_at, updated_at, repository_id FROM knowledge WHERE (key LIKE ?1 OR value LIKE ?1) AND category = ?2 AND confidence >= ?3 ORDER BY confidence DESC LIMIT ?4";
            let mut stmt = self.conn.prepare(if repository_filter.is_some() { sql_with_repo } else { sql_plain })
                .map_err(|e| format!("Failed to prepare query: {}", e))?;
            let map_row = |row: &rusqlite::Row| -> Result<KnowledgeRecord, rusqlite::Error> {
                Ok(KnowledgeRecord {
                    id: row.get(0)?,
                    category: row.get(1)?,
                    key: row.get(2)?,
                    value: row.get(3)?,
                    confidence: row.get(4)?,
                    source: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                    repository_id: row.get(8)?,
                })
            };
            let rows = if let Some(repo) = repository_filter {
                stmt.query_map(params![pattern, cat, min_confidence, repo, max_results as i64], map_row)
            } else {
                stmt.query_map(params![pattern, cat, min_confidence, max_results as i64], map_row)
            }.map_err(|e| format!("Failed to query knowledge: {}", e))?;
            for row in rows {
                records.push(row.map_err(|e| format!("Failed to read row: {}", e))?);
            }
        } else {
            let sql_with_repo = "SELECT id, category, key, value, confidence, source, created_at, updated_at, repository_id FROM knowledge WHERE (key LIKE ?1 OR value LIKE ?1) AND confidence >= ?2 AND repository_id = ?3 ORDER BY confidence DESC LIMIT ?4";
            let sql_plain   = "SELECT id, category, key, value, confidence, source, created_at, updated_at, repository_id FROM knowledge WHERE (key LIKE ?1 OR value LIKE ?1) AND confidence >= ?2 ORDER BY confidence DESC LIMIT ?3";
            let mut stmt = self.conn.prepare(if repository_filter.is_some() { sql_with_repo } else { sql_plain })
                .map_err(|e| format!("Failed to prepare query: {}", e))?;
            let map_row = |row: &rusqlite::Row| -> Result<KnowledgeRecord, rusqlite::Error> {
                Ok(KnowledgeRecord {
                    id: row.get(0)?,
                    category: row.get(1)?,
                    key: row.get(2)?,
                    value: row.get(3)?,
                    confidence: row.get(4)?,
                    source: row.get(5)?,
                    created_at: row.get(6)?,
                    updated_at: row.get(7)?,
                    repository_id: row.get(8)?,
                })
            };
            let rows = if let Some(repo) = repository_filter {
                stmt.query_map(params![pattern, min_confidence, repo, max_results as i64], map_row)
            } else {
                stmt.query_map(params![pattern, min_confidence, max_results as i64], map_row)
            }.map_err(|e| format!("Failed to query knowledge: {}", e))?;
            for row in rows {
                records.push(row.map_err(|e| format!("Failed to read row: {}", e))?);
            }
        }

        log_slow("search_knowledge", start);
        Ok(records)
    }

    /// Delete a knowledge entry
    pub fn delete_knowledge(&self, category: &str, key: &str) -> Result<(), String> {
        let start = Instant::now();
        self.conn
            .execute(
                "DELETE FROM knowledge WHERE category = ?1 AND key = ?2",
                params![category, key],
            )
            .map_err(|e| format!("Failed to delete knowledge: {}", e))?;
        log_slow("delete_knowledge", start);
        Ok(())
    }

    // ── Memory Operations ──────────────────────────────────────────

    /// Save a memory entry
    pub fn save_memory(&self, namespace: &str, key: &str, value: &str) -> Result<i64, String> {
        let start = Instant::now();
        self.conn
            .execute(
                "INSERT OR REPLACE INTO memory (namespace, key, value) VALUES (?1, ?2, ?3)",
                params![namespace, key, value],
            )
            .map_err(|e| format!("Failed to save memory: {}", e))?;
        let id = self.conn.last_insert_rowid();
        log_slow("save_memory", start);
        Ok(id)
    }

    /// Get a memory entry
    pub fn get_memory(&self, namespace: &str, key: &str) -> Result<Option<MemoryRecord>, String> {
        let start = Instant::now();
        let result = self.conn.query_row(
            "SELECT id, namespace, key, value, created_at FROM memory WHERE namespace = ?1 AND key = ?2",
            params![namespace, key],
            |row| {
                Ok(MemoryRecord {
                    id: row.get(0)?,
                    namespace: row.get(1)?,
                    key: row.get(2)?,
                    value: row.get(3)?,
                    created_at: row.get(4)?,
                })
            },
        );
        log_slow("get_memory", start);
        match result {
            Ok(record) => Ok(Some(record)),
            Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
            Err(e) => Err(format!("Failed to get memory: {}", e)),
        }
    }

    /// Search memory by text
    pub fn search_memory(&self, namespace: &str, query: &str, max_results: usize) -> Result<Vec<MemoryRecord>, String> {
        let start = Instant::now();
        let pattern = format!("%{}%", query);
        let mut stmt = self
            .conn
            .prepare("SELECT id, namespace, key, value, created_at FROM memory WHERE namespace = ?1 AND (key LIKE ?2 OR value LIKE ?2) LIMIT ?3")
            .map_err(|e| format!("Failed to prepare query: {}", e))?;

        let records = stmt
            .query_map(params![namespace, pattern, max_results as i64], |row| {
                Ok(MemoryRecord {
                    id: row.get(0)?,
                    namespace: row.get(1)?,
                    key: row.get(2)?,
                    value: row.get(3)?,
                    created_at: row.get(4)?,
                })
            })
            .map_err(|e| format!("Failed to query memory: {}", e))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("Failed to collect memory: {}", e))?;

        log_slow("search_memory", start);
        Ok(records)
    }

    /// Delete a memory entry
    pub fn delete_memory(&self, namespace: &str, key: &str) -> Result<(), String> {
        let start = Instant::now();
        self.conn
            .execute(
                "DELETE FROM memory WHERE namespace = ?1 AND key = ?2",
                params![namespace, key],
            )
            .map_err(|e| format!("Failed to delete memory: {}", e))?;
        log_slow("delete_memory", start);
        Ok(())
    }

    // ── Audits & Workflows — future/workflow only (TASK-001, not v1) ─────────────────────
    #[cfg(feature = "workflow")]
    pub fn insert_audit(&self, workflow_id: Option<&str>, correlation_id: Option<&str>, actor: &str, task: &str, ctx_pack_hash: Option<&str>, payload: Option<&str>) -> Result<i64, String> {
        self.conn.execute(
            "INSERT INTO audits (workflow_id, correlation_id, actor, task, ctx_pack_hash, payload) VALUES (?1,?2,?3,?4,?5,?6)",
            params![workflow_id, correlation_id, actor, task, ctx_pack_hash, payload],
        ).map_err(|e| format!("Failed to insert audit: {e}"))?;
        Ok(self.conn.last_insert_rowid())
    }

    #[cfg(feature = "workflow")]
    pub fn list_audits(&self, limit: usize) -> Result<Vec<AuditRecord>, String> {
        let mut stmt = self.conn.prepare("SELECT id, workflow_id, correlation_id, actor, task, ctx_pack_hash, payload, created_at FROM audits ORDER BY id DESC LIMIT ?1").map_err(|e| format!("Failed to prepare: {e}"))?;
        let rows = stmt.query_map(params![limit as i64], |row| Ok(AuditRecord {
            id: row.get(0)?, workflow_id: row.get(1)?, correlation_id: row.get(2)?, actor: row.get(3)?, task: row.get(4)?, ctx_pack_hash: row.get(5)?, payload: row.get(6)?, created_at: row.get(7)?,
        })).map_err(|e| format!("Failed to query audits: {e}"))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("{e}"))
    }

    #[cfg(feature = "workflow")]
    pub fn upsert_workflow(&self, workflow_id: &str, status: &str, task: &str) -> Result<(), String> {
        self.conn.execute(
            "INSERT INTO workflows (workflow_id, status, task, updated_at) VALUES (?1,?2,?3,?4) ON CONFLICT(workflow_id) DO UPDATE SET status=excluded.status, updated_at=excluded.updated_at",
            params![workflow_id, status, task, Utc::now().to_rfc3339()],
        ).map_err(|e| format!("Failed to upsert workflow: {e}"))?;
        Ok(())
    }

    #[cfg(feature = "workflow")]
    pub fn get_workflow(&self, workflow_id: &str) -> Result<Option<WorkflowRecord>, String> {
        let res = self.conn.query_row("SELECT workflow_id, status, task, created_at, updated_at FROM workflows WHERE workflow_id=?1", params![workflow_id], |row| Ok(WorkflowRecord {
            workflow_id: row.get(0)?, status: row.get(1)?, task: row.get(2)?, created_at: row.get(3)?, updated_at: row.get(4)?,
        }));
        match res { Ok(r) => Ok(Some(r)), Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None), Err(e) => Err(format!("{e}")) }
    }

    #[cfg(feature = "workflow")]
    pub fn list_workflows(&self, limit: usize) -> Result<Vec<WorkflowRecord>, String> {
        let mut stmt = self.conn.prepare("SELECT workflow_id, status, task, created_at, updated_at FROM workflows ORDER BY created_at DESC LIMIT ?1").map_err(|e| format!("{e}"))?;
        let rows = stmt.query_map(params![limit as i64], |row| Ok(WorkflowRecord {
            workflow_id: row.get(0)?, status: row.get(1)?, task: row.get(2)?, created_at: row.get(3)?, updated_at: row.get(4)?,
        })).map_err(|e| format!("{e}"))?;
        rows.collect::<Result<Vec<_>, _>>().map_err(|e| format!("{e}"))
    }

    // ── Utility ─────────────────────────────────────────────────────

    /// Begin batched transaction for bulk indexing (Phase1 perf — avoids fsync per row)
    pub fn begin_batch(&self) -> Result<(), String> {
        self.conn
            .execute_batch("BEGIN IMMEDIATE")
            .map_err(|e| format!("Failed to begin batch: {}", e))
    }

    /// Commit batched transaction
    pub fn commit_batch(&self) -> Result<(), String> {
        self.conn
            .execute_batch("COMMIT")
            .map_err(|e| format!("Failed to commit batch: {}", e))
    }

    /// Rollback batched transaction (best-effort)
    pub fn rollback_batch(&self) -> Result<(), String> {
        let _ = self.conn.execute_batch("ROLLBACK");
        Ok(())
    }

    /// Emit indexing progress event via the event bus
    pub fn emit_indexing_progress(&self, event_bus: &EventBus, files_indexed: usize, symbols_extracted: usize, duration_ms: u64) {
        event_bus.emit(RuntimeEvent::RepositoryUpdated {
            files_indexed,
            symbols_extracted,
            duration_ms,
        });
    }
}

// ── Data Types ──────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct FileRecord {
    pub id: i64,
    pub path: String,
    pub hash: String,
    pub size: i64,
    pub language: Option<String>,
    pub last_indexed_at: String,
}

#[derive(Debug, Clone)]
pub struct Symbol {
    pub id: i64,
    pub file_id: i64,
    pub name: String,
    pub kind: String,
    pub line_start: i64,
    pub line_end: i64,
    pub parent_id: Option<i64>,
}

#[derive(Debug, Clone, Default)]
pub struct UsageStats {
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_requests: i64,
}

#[derive(Debug, Clone)]
pub struct KnowledgeRecord {
    pub id: i64,
    pub category: String,
    pub key: String,
    pub value: String,
    pub confidence: f64,
    pub source: String,
    pub created_at: String,
    pub updated_at: String,
    /// Owning repository scope (TASK-030) — '' marks legacy/global rows
    pub repository_id: String,
}

#[derive(Debug, Clone)]
pub struct MemoryRecord {
    pub id: i64,
    pub namespace: String,
    pub key: String,
    pub value: String,
    pub created_at: String,
}

#[cfg(feature = "workflow")]
#[derive(Debug, Clone)]
pub struct AuditRecord {
    pub id: i64,
    pub workflow_id: Option<String>,
    pub correlation_id: Option<String>,
    pub actor: String,
    pub task: String,
    pub ctx_pack_hash: Option<String>,
    pub payload: Option<String>,
    pub created_at: String,
}

#[cfg(feature = "workflow")]
#[derive(Debug, Clone)]
pub struct WorkflowRecord {
    pub workflow_id: String,
    pub status: String,
    pub task: String,
    pub created_at: String,
    pub updated_at: String,
}

// ── Helpers ─────────────────────────────────────────────────────────────

/// Log a warning if a database operation took more than 100ms
fn log_slow(operation: &str, start: Instant) {
    let elapsed = start.elapsed();
    if elapsed.as_millis() > 100 {
        warn!(
            operation = operation,
            duration_ms = elapsed.as_millis() as u64,
            "Slow database query"
        );
    } else {
        debug!(
            operation = operation,
            duration_ms = elapsed.as_millis() as u64,
            "Database query"
        );
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_db() -> Database {
        let path = PathBuf::from(":memory:");
        Database::open(&path).expect("Failed to create in-memory database")
    }

    #[test]
    fn test_database_creation() {
        let db = test_db();
        let count = db.get_file_count().unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_insert_and_get_file() {
        let db = test_db();
        let id = db
            .insert_file("src/main.rs", "abc123", 1024, Some("rust"), "test")
            .unwrap();
        assert!(id > 0);

        let file = db.get_file("src/main.rs").unwrap().unwrap();
        assert_eq!(file.path, "src/main.rs");
        assert_eq!(file.hash, "abc123");
        assert_eq!(file.size, 1024);
        assert_eq!(file.language.as_deref(), Some("rust"));
    }

    #[test]
    fn test_get_nonexistent_file() {
        let db = test_db();
        let file = db.get_file("nonexistent.rs").unwrap();
        assert!(file.is_none());
    }

    #[test]
    fn test_update_file() {
        let db = test_db();
        let id = db
            .insert_file("src/main.rs", "old_hash", 100, None, "test")
            .unwrap();
        db.update_file(id, "new_hash", 200).unwrap();

        let file = db.get_file("src/main.rs").unwrap().unwrap();
        assert_eq!(file.hash, "new_hash");
        assert_eq!(file.size, 200);
    }

    #[test]
    fn test_delete_file() {
        let db = test_db();
        db.insert_file("src/main.rs", "abc", 100, None, "test")
            .unwrap();
        db.delete_file("src/main.rs", "test").unwrap();
        let file = db.get_file("src/main.rs").unwrap();
        assert!(file.is_none());
    }

    #[test]
    fn test_get_all_files() {
        let db = test_db();
        db.insert_file("a.rs", "h1", 10, None, "test").unwrap();
        db.insert_file("b.rs", "h2", 20, None, "test").unwrap();
        let files = db.get_all_files().unwrap();
        assert_eq!(files.len(), 2);
    }

    #[test]
    fn test_file_count() {
        let db = test_db();
        assert_eq!(db.get_file_count().unwrap(), 0);
        db.insert_file("a.rs", "h", 10, None, "test").unwrap();
        assert_eq!(db.get_file_count().unwrap(), 1);
        db.insert_file("b.rs", "h", 10, None, "test").unwrap();
        assert_eq!(db.get_file_count().unwrap(), 2);
    }

    #[test]
    fn test_insert_and_find_symbols() {
        let db = test_db();
        let file_id = db
            .insert_file("src/main.rs", "h", 100, None, "test")
            .unwrap();

        let sym_id = db
            .insert_symbol(file_id, "main", "function", 1, 10, None)
            .unwrap();
        assert!(sym_id > 0);

        let symbols = db.get_symbols_for_file(file_id).unwrap();
        assert_eq!(symbols.len(), 1);
        assert_eq!(symbols[0].name, "main");
        assert_eq!(symbols[0].kind, "function");
    }

    #[test]
    fn test_find_symbol() {
        let db = test_db();
        let file_id = db
            .insert_file("a.rs", "h", 100, None, "test")
            .unwrap();
        db.insert_symbol(file_id, "UserService", "struct", 1, 20, None)
            .unwrap();
        db.insert_symbol(file_id, "handle_request", "function", 25, 50, None)
            .unwrap();

        let results = db.find_symbol("User").unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].name, "UserService");
    }

    #[test]
    fn test_find_symbols_scoped() {
        let db = test_db();
        // Repo A: two symbols
        let fa = db.insert_file("a.rs", "h", 100, None, "repo_a").unwrap();
        db.insert_symbol(fa, "UserService", "struct", 1, 20, None).unwrap();
        db.insert_symbol(fa, "handle_request", "function", 25, 50, None).unwrap();
        // Repo B: one symbol that would match a whole-sentence LIKE scan
        let fb = db.insert_file("b.rs", "h", 100, None, "repo_b").unwrap();
        db.insert_symbol(fb, "UserService", "struct", 1, 20, None).unwrap();

        // Tokenized match: "user service" → tokens user/service → UserService
        let results = db
            .find_symbols_scoped("how does the user service handler work", "repo_a", 50)
            .unwrap();
        assert_eq!(results.len(), 1, "UserService matches the 'user' token");
        let results_handle = db
            .find_symbols_scoped("the request handler setup", "repo_a", 50)
            .unwrap();
        assert_eq!(results_handle.len(), 1, "handle_request matches the 'request' token");

        // Repo scoping: repo_b never sees repo_a rows and vice versa
        let results_b = db
            .find_symbols_scoped("UserService", "repo_b", 50)
            .unwrap();
        assert_eq!(results_b.len(), 1);
        let results_none = db
            .find_symbols_scoped("handle_request", "repo_b", 50)
            .unwrap();
        assert_eq!(results_none.len(), 0, "no cross-repo pollution");

        // LIMIT is honored
        let limited = db.find_symbols_scoped("user service", "repo_a", 1).unwrap();
        assert_eq!(limited.len(), 1);

        // No usable tokens → bounded fallback slice (≤ max_results), never a
        // full-sentence LIKE scan and never unbounded.
        let empty_tokens = db.find_symbols_scoped("::", "repo_a", 1).unwrap();
        assert_eq!(empty_tokens.len(), 1, "no tokens → bounded fallback slice");
    }

    #[test]
    fn test_symbol_count() {
        let db = test_db();
        let file_id = db
            .insert_file("a.rs", "h", 100, None, "test")
            .unwrap();
        assert_eq!(db.get_symbol_count().unwrap(), 0);
        db.insert_symbol(file_id, "a", "function", 1, 5, None)
            .unwrap();
        db.insert_symbol(file_id, "b", "function", 6, 10, None)
            .unwrap();
        assert_eq!(db.get_symbol_count().unwrap(), 2);
    }

    #[test]
    fn test_token_usage() {
        let db = test_db();
        db.insert_usage("req_123", "pre_generation", 1000, 500)
            .unwrap();
        db.insert_usage("req_456", "pre_tool", 2000, 300)
            .unwrap();

        let stats = db.get_usage_stats().unwrap();
        assert_eq!(stats.total_input_tokens, 3000);
        assert_eq!(stats.total_output_tokens, 800);
        assert_eq!(stats.total_requests, 2);
    }

    #[test]
    fn test_migration_idempotency() {
        let dir = std::env::temp_dir().join(format!("knocode_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.db");

        {
            let db1 = Database::open(&path).unwrap();
            db1.insert_file("a.rs", "h", 10, None, "test").unwrap();
        }

        // Open again — migrations should not fail
        let db2 = Database::open(&path).unwrap();
        let count = db2.get_file_count().unwrap();
        assert_eq!(count, 1);

        // Cleanup
        let _ = std::fs::remove_dir_all(&dir);
    }

    // v1: 004/005 migrations removed — tests moved to future/workflow
    // Workflow audits/workflows are NOT part of v1 hot path (TASK-001/002)

    #[test]
    fn test_knowledge_repo_scoping_and_upsert() {
        // TASK-030/032: repository-scoped search, idempotent upsert, duplicate collapse
        let db = test_db();
        db.store_knowledge("docs", "guide.md", "v1 eshop checkout", 0.8, "mkdocs", "repo_a").unwrap();
        // Re-store same key → upsert, not growth (F-3)
        db.store_knowledge("docs", "guide.md", "v2 eshop checkout", 0.9, "mkdocs", "repo_a").unwrap();
        // Same key in another repo → separate row (cross-repo isolation)
        db.store_knowledge("docs", "guide.md", "other repo checkout content zzz", 0.7, "mkdocs", "repo_b").unwrap();

        let all = db.get_all_knowledge().unwrap();
        assert_eq!(all.len(), 2, "upsert must not grow the table");
        let a = db.get_knowledge("docs", "guide.md").unwrap().unwrap();
        assert!((a.confidence - 0.9).abs() < 1e-9, "highest-confidence write wins");

        let hits_a = db.search_knowledge("checkout", None, 0.0, 10, Some("repo_a")).unwrap();
        assert_eq!(hits_a.len(), 1);
        assert_eq!(hits_a[0].repository_id, "repo_a");
        let hits_b = db.search_knowledge("checkout", None, 0.0, 10, Some("repo_b")).unwrap();
        assert_eq!(hits_b.len(), 1);
        assert!(hits_b[0].value.contains("other repo"), "no cross-repo leakage (F-1)");
    }

    #[test]
    fn test_migration_006_collapses_legacy_duplicates() {
        // Simulate a legacy DB (pre-006): create schema without repository_id via manual SQL,
        // insert duplicates, then open through Database::open to trigger migration.
        let dir = std::env::temp_dir().join(format!("knocode_m006_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("legacy.db");
        {
            let conn = rusqlite::Connection::open(&path).unwrap();
            conn.execute_batch(
                "CREATE TABLE knowledge (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    category TEXT NOT NULL,
                    key TEXT NOT NULL,
                    value TEXT NOT NULL,
                    confidence REAL NOT NULL DEFAULT 0.5,
                    source TEXT NOT NULL DEFAULT 'manual',
                    created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP,
                    updated_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
                );",
            )
            .unwrap();
            for _ in 0..4 {
                conn.execute(
                    "INSERT INTO knowledge (category, key, value, confidence, source) VALUES ('docs', 'dup.md', 'dup content', 0.6, 'mkdocs')",
                    [],
                )
                .unwrap();
            }
        }
        let db = Database::open(&path).unwrap();
        let rows = db.search_knowledge("dup content", None, 0.0, 100, None).unwrap();
        assert_eq!(rows.len(), 1, "migration must collapse duplicate rows keeping max confidence");
        assert_eq!(rows[0].repository_id, "");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
