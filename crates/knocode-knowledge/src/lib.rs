use knocode_core::KnowledgeEntry;
use knocode_events::{EventBus, RuntimeEvent};
use knocode_storage::Database;
use tracing::{debug, info};

// ── Knowledge Hub ───────────────────────────────────────────────────────

pub struct KnowledgeHub {
    db: Database,
    event_bus: EventBus,
}

impl KnowledgeHub {
    /// Create a new Knowledge Hub
    pub fn new(db: Database, event_bus: EventBus) -> Self {
        Self {
            db,
            event_bus,
        }
    }

    // ── Knowledge Storage ──────────────────────────────────────────

    /// Store a knowledge entry (unscoped — legacy/global '' repository_id)
    pub fn store_knowledge(&self, entry: &KnowledgeEntry) -> Result<i64, String> {
        self.store_knowledge_for_repo(entry, "")
    }

    /// Store a knowledge entry scoped to a repository (TASK-030) — idempotent upsert on
    /// `(category, key, repository_id)` so re-ingestion never grows the table (TASK-032)
    pub fn store_knowledge_for_repo(&self, entry: &KnowledgeEntry, repository_id: &str) -> Result<i64, String> {
        self.db.store_knowledge(
            &entry.category,
            &entry.key,
            &entry.value,
            entry.confidence,
            &entry.source,
            repository_id,
        )
    }

    /// Get a knowledge entry by category and key
    pub fn get_knowledge(&self, category: &str, key: &str) -> Result<Option<KnowledgeEntry>, String> {
        match self.db.get_knowledge(category, key)? {
            Some(record) => Ok(Some(KnowledgeEntry {
                id: Some(record.id),
                category: record.category,
                key: record.key,
                value: record.value,
                confidence: record.confidence,
                source: record.source,
                relevance_score: None,
            })),
            None => Ok(None),
        }
    }

    /// Get all knowledge entries
    pub fn get_all_knowledge(&self) -> Result<Vec<KnowledgeEntry>, String> {
        let records = self.db.get_all_knowledge()?;
        Ok(records
            .into_iter()
            .map(|r| KnowledgeEntry {
                id: Some(r.id),
                category: r.category,
                key: r.key,
                value: r.value,
                confidence: r.confidence,
                source: r.source,
                relevance_score: None,
            })
            .collect())
    }

    /// Update confidence for a knowledge entry
    pub fn update_confidence(&self, id: i64, confidence: f64) -> Result<(), String> {
        self.db.update_knowledge_confidence(id, confidence)
    }

    /// Decay confidence for old knowledge entries
    pub fn decay_confidence(&self, min_age_days: i64, decay_amount: f64) -> Result<usize, String> {
        self.db.decay_knowledge_confidence(min_age_days, decay_amount)
    }

    // ── Knowledge Retrieval (BM25 local, spec §3) ───────

    /// Retrieve knowledge matching a query — lexical BM25/tantivy local.
    /// `repository_id: Some(id)` scopes results to one repository (TASK-030/F-1).
    pub fn retrieve_knowledge(
        &self,
        query: &str,
        category_filter: Option<&str>,
        max_results: usize,
        repository_id: Option<&str>,
    ) -> Result<Vec<KnowledgeEntry>, String> {
        // lexical search (tantivy if available, else LIKE) — collect top 20, filter by confidence ≥0.3
        let lexical_limit = 20usize;
        let records = self.db.search_knowledge(query, category_filter, 0.3, lexical_limit, repository_id)?;

        let mut entries: Vec<KnowledgeEntry> = Vec::new();
        for rec in records.into_iter().take(max_results) {
            if rec.confidence >= 0.3 {
                entries.push(KnowledgeEntry {
                    id: Some(rec.id),
                    category: rec.category.clone(),
                    key: rec.key.clone(),
                    value: rec.value.clone(),
                    confidence: rec.confidence,
                    source: rec.source.clone(),
                    relevance_score: None,
                });
            }
        }

        debug!(query = query, results = entries.len(), "Knowledge retrieval (BM25 local)");

        Ok(entries)
    }

    /// Check if the Knowledge Hub has any seeded knowledge for this repository (P0 #3 proactive detection)
    pub fn is_initialized(&self, repository_id: Option<&str>) -> bool {
        self.db.count_knowledge(repository_id).unwrap_or(0) > 0
    }

    // ── Knowledge Extraction ───────────────────────────────────────

    /// Extract knowledge from indexed code analysis
    pub fn extract_knowledge(
        &self,
        symbols: &[(String, String)], // (name, kind) pairs
        file_paths: &[String],
    ) -> Result<usize, String> {
        let mut extracted = 0;

        // Detect naming patterns
        let naming_patterns = detect_naming_patterns(symbols);
        for (pattern, confidence) in naming_patterns {
            self.store_knowledge(&KnowledgeEntry {
                id: None,
                category: "convention".to_string(),
                key: format!("naming_{}", pattern),
                value: format!("Project uses {} naming convention", pattern),
                confidence,
                source: "auto_extract".to_string(),
                relevance_score: None,
            })?;
            extracted += 1;
        }

        // Detect architectural patterns
        let arch_patterns = detect_architectural_patterns(symbols, file_paths);
        for (pattern, confidence) in arch_patterns {
            self.store_knowledge(&KnowledgeEntry {
                id: None,
                category: "pattern".to_string(),
                key: format!("arch_{}", pattern),
                value: format!("Project follows {} architecture", pattern),
                confidence,
                source: "auto_extract".to_string(),
                relevance_score: None,
            })?;
            extracted += 1;
        }

        // Detect domain terms
        let domain_terms = detect_domain_terms(symbols);
        for (term, definition, confidence) in domain_terms {
            self.store_knowledge(&KnowledgeEntry {
                id: None,
                category: "domain".to_string(),
                key: term,
                value: definition,
                confidence,
                source: "auto_extract".to_string(),
                relevance_score: None,
            })?;
            extracted += 1;
        }

        info!(extracted = extracted, "Knowledge extraction complete");
        Ok(extracted)
    }

    // ── Memory Operations (SQLite local) ────────────────────

    /// Save to memory (SQLite local)
    pub fn memory_save(&self, namespace: &str, key: &str, value: &str) -> Result<i64, String> {
        let id = self.db.save_memory(namespace, key, value)?;
        
        self.event_bus.emit(RuntimeEvent::MemorySaved {
            entry_id: id.to_string(),
            namespace: namespace.to_string(),
            key: key.to_string(),
        });

        Ok(id)
    }

    /// Search memory (SQLite local)
    pub fn memory_search(&self, namespace: &str, query: &str, max_results: usize) -> Result<Vec<(String, String)>, String> {
        let records = self.db.search_memory(namespace, query, max_results)?;
        Ok(records
            .into_iter()
            .map(|r| (r.key, r.value))
            .collect())
    }
}

// ── Pattern Detection Helpers ───────────────────────────────────────────

/// Detect naming conventions used in the codebase
fn detect_naming_patterns(symbols: &[(String, String)]) -> Vec<(String, f64)> {
    let mut patterns = Vec::new();
    let mut snake_case_count = 0;
    let mut camel_case_count = 0;
    let mut pascal_case_count = 0;
    let total = symbols.len();

    if total == 0 {
        return patterns;
    }

    for (name, _kind) in symbols {
        if name.contains('_') {
            snake_case_count += 1;
        } else if name.chars().next().map(|c| c.is_lowercase()).unwrap_or(false)
            && name.chars().any(|c| c.is_uppercase())
        {
            camel_case_count += 1;
        } else if name.chars().next().map(|c| c.is_uppercase()).unwrap_or(false)
            && name.chars().any(|c| c.is_uppercase())
        {
            pascal_case_count += 1;
        }
    }

    if snake_case_count as f64 / total as f64 > 0.5 {
        patterns.push(("snake_case".to_string(), 0.8));
    }
    if camel_case_count as f64 / total as f64 > 0.5 {
        patterns.push(("camelCase".to_string(), 0.8));
    }
    if pascal_case_count as f64 / total as f64 > 0.5 {
        patterns.push(("PascalCase".to_string(), 0.8));
    }

    patterns
}

/// Detect architectural patterns
fn detect_architectural_patterns(
    symbols: &[(String, String)],
    file_paths: &[String],
) -> Vec<(String, f64)> {
    let mut patterns = Vec::new();

    // Check for controller-service-repo pattern
    let has_controller = symbols.iter().any(|(name, _)| name.to_lowercase().contains("controller"));
    let has_service = symbols.iter().any(|(name, _)| name.to_lowercase().contains("service"));
    let has_repository = symbols.iter().any(|(name, _)| name.to_lowercase().contains("repository") || name.to_lowercase().contains("repo"));

    if has_controller && has_service && has_repository {
        patterns.push(("controller-service-repository".to_string(), 0.9));
    }

    // Check for MVC pattern
    let has_model = file_paths.iter().any(|p| p.to_lowercase().contains("model"));
    let has_view = file_paths.iter().any(|p| p.to_lowercase().contains("view"));
    let has_controller_file = file_paths.iter().any(|p| p.to_lowercase().contains("controller"));

    if has_model && has_view && has_controller_file {
        patterns.push(("mvc".to_string(), 0.85));
    }

    // Check for handler pattern
    let has_handler = symbols.iter().any(|(name, _)| name.to_lowercase().contains("handler"));
    if has_handler {
        patterns.push(("handler".to_string(), 0.7));
    }

    // Check for middleware pattern
    let has_middleware = symbols.iter().any(|(name, _)| name.to_lowercase().contains("middleware"));
    if has_middleware {
        patterns.push(("middleware".to_string(), 0.7));
    }

    patterns
}

/// Detect domain-specific terms
fn detect_domain_terms(symbols: &[(String, String)]) -> Vec<(String, String, f64)> {
    let mut terms = Vec::new();

    // Common domain terms to look for
    let domain_keywords = [
        ("user", "A user of the system"),
        ("auth", "Authentication and authorization"),
        ("session", "User session management"),
        ("permission", "Access control permissions"),
        ("role", "User roles for access control"),
        ("token", "Authentication tokens"),
        ("api", "Application programming interface"),
        ("endpoint", "API endpoint"),
        ("route", "API route definition"),
        ("middleware", "Request/response middleware"),
        ("handler", "Request handler"),
        ("service", "Business logic service"),
        ("repository", "Data access layer"),
        ("model", "Data model"),
        ("schema", "Database schema"),
        ("migration", "Database migration"),
        ("config", "Configuration"),
        ("cache", "Caching layer"),
        ("queue", "Message queue"),
        ("event", "Event handling"),
    ];

    for (keyword, definition) in &domain_keywords {
        let count = symbols
            .iter()
            .filter(|(name, _)| name.to_lowercase().contains(keyword))
            .count();

        if count >= 2 {
            // At least 2 occurrences to be confident
            let confidence = (count as f64 / 10.0).min(0.9);
            terms.push((
                keyword.to_string(),
                definition.to_string(),
                confidence,
            ));
        }
    }

    terms
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

    fn test_hub() -> KnowledgeHub {
        let db = test_db();
        let event_bus = EventBus::new();
        KnowledgeHub::new(db, event_bus)
    }

    #[test]
    fn test_store_and_get_knowledge() {
        let hub = test_hub();
        let entry = KnowledgeEntry {
            id: None,
            category: "convention".to_string(),
            key: "naming".to_string(),
            value: "Use snake_case".to_string(),
            confidence: 0.8,
            source: "test".to_string(),
            relevance_score: None,
        };

        let id = hub.store_knowledge(&entry).unwrap();
        assert!(id > 0);

        let retrieved = hub.get_knowledge("convention", "naming").unwrap();
        assert!(retrieved.is_some());
        let retrieved = retrieved.unwrap();
        assert_eq!(retrieved.value, "Use snake_case");
        assert!((retrieved.confidence - 0.8).abs() < 0.01);
    }

    #[test]
    fn test_get_all_knowledge() {
        let hub = test_hub();
        
        hub.store_knowledge(&KnowledgeEntry {
            id: None,
            category: "convention".to_string(),
            key: "naming".to_string(),
            value: "Use snake_case".to_string(),
            confidence: 0.8,
            source: "test".to_string(),
            relevance_score: None,
        }).unwrap();

        hub.store_knowledge(&KnowledgeEntry {
            id: None,
            category: "pattern".to_string(),
            key: "arch".to_string(),
            value: "MVC pattern".to_string(),
            confidence: 0.7,
            source: "test".to_string(),
            relevance_score: None,
        }).unwrap();

        let all = hub.get_all_knowledge().unwrap();
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn test_search_knowledge() {
        let hub = test_hub();
        
        hub.store_knowledge(&KnowledgeEntry {
            id: None,
            category: "convention".to_string(),
            key: "naming".to_string(),
            value: "Use snake_case for variables".to_string(),
            confidence: 0.8,
            source: "test".to_string(),
            relevance_score: None,
        }).unwrap();

        let results = hub.retrieve_knowledge("snake", None, 10, None).unwrap();
        assert_eq!(results.len(), 1);
        assert!(results[0].value.contains("snake_case"));
    }

    #[test]
    fn test_update_confidence() {
        let hub = test_hub();
        
        let id = hub.store_knowledge(&KnowledgeEntry {
            id: None,
            category: "test".to_string(),
            key: "key".to_string(),
            value: "value".to_string(),
            confidence: 0.5,
            source: "test".to_string(),
            relevance_score: None,
        }).unwrap();

        hub.update_confidence(id, 0.9).unwrap();
        
        let entry = hub.get_knowledge("test", "key").unwrap().unwrap();
        assert!((entry.confidence - 0.9).abs() < 0.01);
    }

    #[test]
    fn test_memory_save_and_search() {
        let hub = test_hub();
        
        let id = hub.memory_save("conventions", "style", "Use rustfmt").unwrap();
        assert!(id > 0);

        let results = hub.memory_search("conventions", "rustfmt", 10).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].0, "style");
        assert_eq!(results[0].1, "Use rustfmt");
    }

    #[test]
    fn test_extract_knowledge() {
        let hub = test_hub();
        
        let symbols = vec![
            ("get_user".to_string(), "function".to_string()),
            ("set_user".to_string(), "function".to_string()),
            ("user_service".to_string(), "struct".to_string()),
            ("UserController".to_string(), "struct".to_string()),
            ("UserService".to_string(), "struct".to_string()),
            ("UserRepository".to_string(), "struct".to_string()),
        ];

        let file_paths = vec![
            "src/controllers/user_controller.rs".to_string(),
            "src/services/user_service.rs".to_string(),
            "src/repositories/user_repository.rs".to_string(),
        ];

        let extracted = hub.extract_knowledge(&symbols, &file_paths).unwrap();
        assert!(extracted > 0);
    }

    #[test]
    fn test_detect_naming_patterns() {
        let symbols = vec![
            ("get_user".to_string(), "function".to_string()),
            ("set_user".to_string(), "function".to_string()),
            ("is_valid".to_string(), "function".to_string()),
        ];

        let patterns = detect_naming_patterns(&symbols);
        assert!(patterns.iter().any(|(p, _)| p == "snake_case"));
    }

    #[test]
    fn test_detect_architectural_patterns() {
        let symbols = vec![
            ("UserController".to_string(), "struct".to_string()),
            ("UserService".to_string(), "struct".to_string()),
            ("UserRepository".to_string(), "struct".to_string()),
        ];

        let file_paths = vec![
            "src/controllers/user.rs".to_string(),
            "src/services/user.rs".to_string(),
            "src/repositories/user.rs".to_string(),
        ];

        let patterns = detect_architectural_patterns(&symbols, &file_paths);
        assert!(patterns.iter().any(|(p, _)| p == "controller-service-repository"));
    }

    #[test]
    fn test_detect_domain_terms() {
        let symbols = vec![
            ("get_user".to_string(), "function".to_string()),
            ("create_user".to_string(), "function".to_string()),
            ("delete_user".to_string(), "function".to_string()),
            ("find_user".to_string(), "function".to_string()),
        ];

        let terms = detect_domain_terms(&symbols);
        assert!(terms.iter().any(|(t, _, _)| t == "user"));
    }

    #[test]
    fn test_retrieve_knowledge_repo_scoped() {
        // TASK-030/F-1: retrieval must be repository-scoped
        let hub = test_hub();
        hub.store_knowledge_for_repo(&KnowledgeEntry { id: None, category: "docs".to_string(), key: "checkout.md".to_string(), value: "eshop basket checkout flow".to_string(), confidence: 0.9, source: "mkdocs".to_string(), relevance_score: None }, "repo_eshop").unwrap();
        hub.store_knowledge_for_repo(&KnowledgeEntry { id: None, category: "docs".to_string(), key: "router.md".to_string(), value: "knocode daemon router checkout".to_string(), confidence: 0.9, source: "mkdocs".to_string(), relevance_score: None }, "repo_knocode").unwrap();

        let eshop = hub.retrieve_knowledge("checkout", None, 10, Some("repo_eshop")).unwrap();
        assert_eq!(eshop.len(), 1);
        assert_eq!(eshop[0].key, "checkout.md", "only the requested repo's docs may surface");

        let other = hub.retrieve_knowledge("basket", None, 10, Some("repo_knocode")).unwrap();
        assert!(other.is_empty(), "no cross-repo leakage");
    }

    #[test]
    fn test_retrieve_knowledge_bm25_local() {
        let hub = test_hub();
        for i in 0..5 {
            hub.store_knowledge(&KnowledgeEntry { id: None, category: "docs".to_string(), key: format!("k{i}"), value: format!("rerank test doc {i} contains rust"), confidence: 0.6, source: "test".to_string(), relevance_score: None }).unwrap();
        }
        let res = hub.retrieve_knowledge("rust", Some("docs"), 3, None).unwrap();
        assert!(!res.is_empty());
        assert!(res.len() <= 3);
    }

}
