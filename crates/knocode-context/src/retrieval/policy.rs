//! RetrievalPolicy — named, testable ranking policy.
//! All magic numbers from `tantivy_index.rs` + `lib.rs` are centralized here.

use std::collections::HashSet;

use crate::retrieval::intent::QueryIntent;

/// Field weights for Tantivy BM25 query parser.
/// Maps directly to the `set_field_boost` calls in `tantivy_index.rs:647-652`.
#[derive(Debug, Clone)]
pub struct FieldWeights {
    pub symbol_name: f32,
    pub title: f32,
    pub path: f32,
    pub symbols: f32,
    pub filename: f32,
    pub content: f32,
}

impl Default for FieldWeights {
    fn default() -> Self {
        Self {
            symbol_name: 3.0,
            title: 2.5,
            path: 2.5,
            symbols: 2.0,
            filename: 2.0,
            content: 1.0,
        }
    }
}

/// Per-file-class boost factors.
/// Mirrors `CodeIndexSchema::file_class_boost` in `crates/knocode-storage/src/tantivy_index.rs:86-99`.
#[derive(Debug, Clone)]
pub struct FileClassWeights {
    pub documentation: f32,
    pub config: f32,
    pub source: f32,
    pub test: f32,
    pub generated: f32,
    pub stylesheet: f32,
    pub binary: f32,
    pub vendor: f32,
    pub dependency: f32,
}

impl Default for FileClassWeights {
    /// Parity: values must equal `knocode_core::ranking::file_class_boost`
    /// defaults (see `file_class_weights_parity_with_core` test below).
    fn default() -> Self {
        Self {
            documentation: knocode_core::ranking::file_class_boost("Documentation"),
            config: knocode_core::ranking::file_class_boost("Config"),
            source: knocode_core::ranking::file_class_boost("Source"),
            test: knocode_core::ranking::file_class_boost("Test"),
            generated: knocode_core::ranking::file_class_boost("Generated"),
            stylesheet: knocode_core::ranking::file_class_boost("Stylesheet"),
            binary: knocode_core::ranking::file_class_boost("Binary"),
            vendor: knocode_core::ranking::file_class_boost("Vendor"),
            dependency: knocode_core::ranking::file_class_boost("Dependency"),
        }
    }
}

impl FileClassWeights {
    pub fn boost_for(&self, file_class: &str) -> f32 {
        match file_class {
            "Documentation" => self.documentation,
            "Config" => self.config,
            "Source" => self.source,
            "Test" => self.test,
            "Generated" => self.generated,
            "Stylesheet" => self.stylesheet,
            "Binary" => self.binary,
            "Vendor" => self.vendor,
            "Dependency" => self.dependency,
            _ => 1.0,
        }
    }

    /// Intent-aware override — Procedural should upweight docs, Debugging upweights source/tests, etc.
    /// FIX #8: Uses pre-computed static lookup to avoid allocating a new struct per call.
    /// ORDER MUST MATCH QueryIntent enum variant order (intent.rs).
    pub fn for_intent(intent: QueryIntent) -> &'static Self {
        // QueryIntent order: Procedural=0, Informational=1, Implementation=2,
        // Debugging=3, Testing=4, Configuration=5, Architecture=6, Navigation=7, Structural=8
        static INTENT_WEIGHTS: [FileClassWeights; 9] = [
            // Procedural (0)
            FileClassWeights { documentation: 2.5, config: 1.4, source: 0.8, test: 0.25, generated: 0.5, stylesheet: 0.0, binary: 0.0, vendor: 0.0, dependency: 0.0 },
            // Informational (1)
            FileClassWeights { documentation: 1.4, config: 1.1, source: 1.0, test: 0.6, generated: 0.5, stylesheet: 0.0, binary: 0.0, vendor: 0.0, dependency: 0.0 },
            // Implementation (2)
            FileClassWeights { documentation: 1.0, config: 1.0, source: 1.5, test: 0.7, generated: 0.5, stylesheet: 0.0, binary: 0.0, vendor: 0.0, dependency: 0.0 },
            // Debugging (3)
            FileClassWeights { documentation: 1.0, config: 1.2, source: 1.5, test: 1.3, generated: 0.5, stylesheet: 0.0, binary: 0.0, vendor: 0.0, dependency: 0.0 },
            // Testing (4)
            FileClassWeights { documentation: 1.6, config: 1.3, source: 1.0, test: 1.5, generated: 0.5, stylesheet: 0.0, binary: 0.0, vendor: 0.0, dependency: 0.0 },
            // Configuration (5)
            FileClassWeights { documentation: 1.1, config: 1.8, source: 1.0, test: 0.5, generated: 0.5, stylesheet: 0.0, binary: 0.0, vendor: 0.0, dependency: 0.0 },
            // Architecture (6)
            FileClassWeights { documentation: 1.6, config: 1.5, source: 1.1, test: 0.6, generated: 0.5, stylesheet: 0.0, binary: 0.0, vendor: 0.0, dependency: 0.0 },
            // Navigation (7)
            FileClassWeights { documentation: 0.9, config: 0.9, source: 1.3, test: 0.7, generated: 0.5, stylesheet: 0.0, binary: 0.0, vendor: 0.0, dependency: 0.0 },
            // Structural (8)
            FileClassWeights { documentation: 0.8, config: 0.8, source: 1.5, test: 0.7, generated: 0.5, stylesheet: 0.0, binary: 0.0, vendor: 0.0, dependency: 0.0 },
        ];
        let idx = intent as usize;
        &INTENT_WEIGHTS[idx]
    }
}

/// Directory/location boosts.
/// Mirrors `CodeIndexSchema::directory_boost` in `crates/knocode-storage/src/tantivy_index.rs:113-130`.
#[derive(Debug, Clone)]
pub struct DirectoryWeights {
    /// README/CONTRIBUTING/CLAUDE.md/AGENTS.md at any path
    pub readme: f32,
    /// /docs/, /.github/, /.knocode/
    pub docs: f32,
    /// types/* monorepo workspace
    pub types: f32,
    /// pnpm-workspace.yaml / lerna.json / nx.json
    pub workspace: f32,
    pub default: f32,
}

impl Default for DirectoryWeights {
    /// Parity: values must equal the canonical defaults in
    /// `knocode_core::ranking` (see `directory_weights_parity_with_core`).
    fn default() -> Self {
        Self {
            readme: knocode_core::ranking::DIRECTORY_README,
            docs: knocode_core::ranking::DIRECTORY_DOCS,
            types: knocode_core::ranking::DIRECTORY_TYPES,
            workspace: knocode_core::ranking::DIRECTORY_WORKSPACE,
            default: knocode_core::ranking::DIRECTORY_DEFAULT,
        }
    }
}

impl DirectoryWeights {
    /// Delegate to the canonical logic in `knocode_core::ranking` with this
    /// struct's values — one logic copy, configurable numbers.
    pub fn boost_for(&self, path: &str) -> f32 {
        knocode_core::ranking::directory_boost_with(
            path,
            self.readme,
            self.docs,
            self.types,
            self.workspace,
            self.default,
        )
    }
}

/// Documentation authority prior — relevance × authority.
/// Fixes `README.md` vs `README.ja.md` canonicality.
///
#[derive(Debug, Clone)]
pub struct DocumentationAuthority {
    /// Repository root `README.md` — canonical
    pub canonical_readme: f32,
    /// Localized `README.*.md` (ja, fr, …)
    pub localized_readme: f32,
    /// `docs/index.md`
    pub docs_index: f32,
    /// `docs/*.md`
    pub docs_generic: f32,
    /// `.github/*.md`
    pub github: f32,
    /// Other `*.md`
    pub other: f32,
}

impl Default for DocumentationAuthority {
    fn default() -> Self {
        Self {
            canonical_readme: 1.50,
            localized_readme: 1.10,
            docs_index: 1.30,
            docs_generic: 1.20,
            github: 1.10,
            other: 1.00,
        }
    }
}

impl DocumentationAuthority {
    pub fn boost_for(&self, path: &str) -> f32 {
        let lower = path.to_lowercase();
        let filename = lower.rsplit('/').next().unwrap_or(&lower);
        // Canonical README.md at root or any directory ending exactly readme.md not readme.*.md
        if filename == "readme.md" {
            return self.canonical_readme;
        }
        if filename.starts_with("readme.") && filename.ends_with(".md") {
            return self.localized_readme;
        }
        if lower.ends_with("docs/index.md") {
            return self.docs_index;
        }
        // docs/ subdirectory markdown files (P1.5: stronger docs/ boost)
        if (lower.contains("/docs/") || lower.starts_with("docs/")) && lower.ends_with(".md") {
            return self.docs_generic;
        }
        if (lower.contains("/.github/") || lower.starts_with(".github/")) && lower.ends_with(".md") {
            return self.github;
        }
        // P1.5: Boost non-README docs at root (CONTRIBUTING.md, CHANGELOG.md, etc.)
        if !lower.contains('/') && lower.ends_with(".md") {
            return self.docs_generic;
        }
        if lower.ends_with(".md") {
            return self.other;
        }
        1.0
    }

    /// P1.5: Detect canonical README path for a repository.
    /// Returns the highest-authority README path from a list of candidates.
    pub fn canonical_readme_path<'a>(candidates: &[&'a str]) -> Option<&'a str> {
        // Prefer root README.md, then any */README.md, then README.*.md
        for path in candidates {
            let lower = path.to_lowercase();
            let filename = lower.rsplit('/').next().unwrap_or(&lower);
            if filename == "readme.md" {
                return Some(path);
            }
        }
        // No canonical README found — return first localized
        candidates.first().copied()
    }
}

/// Central retrieval policy — every ranking constant is named here.
///
#[derive(Debug, Clone)]
pub struct RetrievalPolicy {
    /// Tantivy candidate pool size before ranking (default 100 → Top 20)
    pub candidate_k: usize,
    /// Final file limit (default 20, 50 for large repos)
    pub max_files: usize,

    pub field_weights: FieldWeights,
    pub file_class_weights: FileClassWeights,
    pub directory_weights: DirectoryWeights,
    pub doc_authority: DocumentationAuthority,

    /// Symbol-match boost factor: `1.0 + (matched/total)*weight` — current 1.5
    /// Mirrors `lib.rs:312` `1.0 + (count/len)*1.5`
    pub symbol_match_weight: f32,
    /// Query-aware Test multiplier
    pub test_penalty: f32, // 0.6 when query not test-related
    pub test_boost: f32,   // 1.4 when query is test-related

    /// Code-behind pairing multiplier (`.cshtml → .cshtml.cs`) — 0.8
    pub code_behind_multiplier: f32,
    /// Graph connectivity boost — 1.2
    pub graph_multiplier: f32,

    pub enable_graph: bool,

    // ── Performance budgets (P1.4) ──
    /// Maximum time in ms for lexical retrieval.
    pub lexical_budget_ms: u64,
    /// Maximum time in ms for structural retrieval.
    pub structural_budget_ms: u64,
    /// Maximum time in ms for total retrieval (all backends combined).
    pub total_budget_ms: u64,
    /// Maximum matches per backend before early termination.
    pub max_matches_per_backend: usize,

    // ── FIX #4: Structural exhaustive mode settings ──
    /// Candidate pool size for structural exhaustive queries ("find all X").
    pub structural_candidate_k: usize,
    /// Max files for structural exhaustive queries ("find all X").
    pub structural_max_files: usize,

    /// P0 eval fix: score multiplier for documentation-class files whose path
    /// matches no query token (generic meta-docs like README/ROADMAP matching
    /// via content generality). `1.0` disables damping; `0.10` drops generic
    /// meta-docs below the on-topic code files they used to bury while letting
    /// genuinely BM25-strong docs stay listed (Recall@10 unchanged in eval).
    /// Code/Config/Test never damped; path-token match ("indexing" vs
    /// INDEXING_PERF_PLAN.md) keeps full score.
    pub doc_prior_damping: f32,

    /// Docs-slot reservation (REQUEST_LIFECYCLE.md:312 documents a 45% docs
    /// budget; this enforces a minimal floor of it). The N highest-scoring
    /// Documentation-class candidates are PROMOTED into the final evidence
    /// list's tail slots when they would otherwise fall below the top-K cut —
    /// promote-only: scores are never rescaled and leading code order is
    /// untouched, so the P0 damping eval result is preserved. `0` disables.
    /// Env override: `KNOCODE_DOCS_RESERVE`.
    pub docs_reserve_slots: usize,
}

impl RetrievalPolicy {
    /// Effective docs-slot reservation (`KNOCODE_DOCS_RESERVE` env override wins).
    pub fn effective_docs_reserve(&self) -> usize {
        if let Ok(v) = std::env::var("KNOCODE_DOCS_RESERVE") {
            if let Ok(n) = v.parse::<usize>() {
                return n;
            }
        }
        self.docs_reserve_slots
    }

    /// Promote the highest-scoring Documentation-class entries into the last
    /// `slots` positions of the KEPT region — the first `keep` entries of an
    /// already-ranked list (candidates ranked below `keep` may be swapped in,
    /// displacing non-docs outward past the cut). PROMOTE-ONLY: scores are
    /// never rescaled and the relative order of the leading code entries is
    /// preserved, so the P0 doc-damping eval result holds. Returns the file
    /// classes of the entries occupying the reserved slots (docs already in
    /// the tail keep their slot without being moved).
    ///
    /// Precondition: `ranked` is sorted best-first. Postcondition: the first
    /// `min(keep, len)` entries contain at least `min(slots, len)` doc slots
    /// at positions `[keep-slots, keep)`, provided that many docs exist.
    pub fn reserve_doc_slots<E>(
        &self,
        ranked: &mut [E],
        keep: usize,
        slots: usize,
        is_doc: impl Fn(&E) -> bool,
        class_of: impl Fn(&E) -> String,
    ) -> Vec<String> {
        let keep = keep.min(ranked.len());
        if slots == 0 || keep <= slots {
            return Vec::new();
        }
        let tail_start = keep - slots;

        // Docs already inside the reserved tail keep their slots.
        let mut promoted: Vec<String> = Vec::new();
        let mut reserved_tail: HashSet<usize> = HashSet::new();
        for idx in tail_start..keep {
            if is_doc(&ranked[idx]) {
                reserved_tail.insert(idx);
                promoted.push(class_of(&ranked[idx]));
            }
        }
        if promoted.len() >= slots {
            return promoted;
        }

        // Highest-scoring doc candidates outside the reserved tail, in ranked
        // order (indices < tail_start first, then >= keep — both best-first).
        let candidates: Vec<usize> = (0..ranked.len())
            .filter(|&i| i < tail_start || i >= keep)
            .filter(|&i| is_doc(&ranked[i]))
            .take(slots - promoted.len())
            .collect();

        // Free tail slots (kept-tail positions not already held by a doc).
        let free_tail: Vec<usize> = (tail_start..keep)
            .filter(|i| !reserved_tail.contains(i))
            .collect();

        // Swap each chosen candidate into a free tail slot. Candidate and tail
        // index ranges are disjoint, so swaps never collide with each other.
        for (&src, &dst) in candidates.iter().zip(free_tail.iter()) {
            if src != dst {
                ranked.swap(src, dst);
            }
            promoted.push(class_of(&ranked[dst]));
        }
        promoted
    }
}

impl Default for RetrievalPolicy {
    fn default() -> Self {
        Self {
            candidate_k: 100,
            max_files: 20,
            field_weights: FieldWeights::default(),
            file_class_weights: FileClassWeights::default(),
            directory_weights: DirectoryWeights::default(),
            doc_authority: DocumentationAuthority::default(),
            symbol_match_weight: 1.5,
            test_penalty: 0.6,
            test_boost: 1.4,
            code_behind_multiplier: 0.8,
            graph_multiplier: 1.2,
            enable_graph: false,

            lexical_budget_ms: 2000,
            structural_budget_ms: 3000,
            total_budget_ms: 5000,
            max_matches_per_backend: 200,
            structural_candidate_k: 500,
            structural_max_files: 500,
            doc_prior_damping: 0.10,
            docs_reserve_slots: 2,
        }
    }
}

impl RetrievalPolicy {
    /// Query-aware Test multiplier — delegates to the canonical logic in
    /// `knocode_core::ranking` with this policy's values.
    pub fn test_multiplier(&self, query: &str, file_class: &str) -> f32 {
        knocode_core::ranking::query_aware_test_multiplier_with(query, file_class, self.test_penalty, self.test_boost)
    }

    /// Intent-aware file-class boost: relevance × intent authority.
    pub fn intent_file_class_boost(&self, file_class: &str, intent: QueryIntent) -> f32 {
        let weights = FileClassWeights::for_intent(intent);
        weights.boost_for(file_class)
    }

    /// Document authority prior — separate from relevance.
    pub fn doc_authority_boost(&self, path: &str) -> f32 {
        self.doc_authority.boost_for(path)
    }

    /// Combined scoring factor for final ranking:
    /// `lexical × intent_class × directory × doc_authority × test_multiplier`
    /// Authority is kept conceptually separate from relevance.
    pub fn combined_boost(&self, path: &str, file_class: &str, query: &str, intent: QueryIntent) -> f32 {
        let intent_boost = self.intent_file_class_boost(file_class, intent);
        let dir_boost = self.directory_weights.boost_for(path);
        let auth_boost = self.doc_authority_boost(path);
        let test_mult = self.test_multiplier(query, file_class);
        intent_boost * dir_boost * auth_boost * test_mult
    }

    /// Effective candidate_k (env override `KNOCODE_CANDIDATE_K` wins).
    /// P0: clamp raised 200→1000 — the old clamp silently capped the documented
    /// structural candidate_k of 500-1000 AND made bench_components' 50 vs 500
    /// comparison measure 50 vs 200, masking the component's real effect.
    pub fn effective_candidate_k(&self) -> usize {
        if let Ok(v) = std::env::var("KNOCODE_CANDIDATE_K") {
            if let Ok(n) = v.parse::<usize>() {
                return n.min(1000);
            }
        }
        self.candidate_k
    }

    /// Effective max_files with large-repo auto-tune (matches `lib.rs:209-214`)
    pub fn effective_max_files(&self, doc_count: usize) -> usize {
        if doc_count > 5000 && self.max_files == 20 {
            50
        } else {
            self.max_files
        }
    }

    /// Effective candidate_k with large-repo auto-tune
    pub fn effective_candidate_k_for(&self, doc_count: usize) -> usize {
        let mut k = self.effective_candidate_k();
        if k == 100 && doc_count > 5000 {
            k = 200;
        }
        k
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::intent::QueryIntent;

    #[test]
    fn defaults_match_legacy_constants() {
        let p = RetrievalPolicy::default();
        assert!((p.field_weights.symbol_name - 3.0).abs() < 1e-6);
        assert!((p.field_weights.title - 2.5).abs() < 1e-6);
        assert!((p.field_weights.path - 2.5).abs() < 1e-6);
        assert!((p.field_weights.symbols - 2.0).abs() < 1e-6);
        assert!((p.field_weights.filename - 2.0).abs() < 1e-6);
        assert!((p.field_weights.content - 1.0).abs() < 1e-6);
        assert!((p.file_class_weights.documentation - 1.4).abs() < 1e-6);
        assert!((p.file_class_weights.config - 1.2).abs() < 1e-6);
        assert!((p.file_class_weights.source - 1.0).abs() < 1e-6);
        assert!((p.file_class_weights.test - 0.7).abs() < 1e-6);
        assert!((p.directory_weights.readme - 1.3).abs() < 1e-6);
        assert!((p.directory_weights.docs - 1.2).abs() < 1e-6);
        assert!((p.symbol_match_weight - 1.5).abs() < 1e-6);
        assert!((p.code_behind_multiplier - 0.8).abs() < 1e-6);
        assert!((p.graph_multiplier - 1.2).abs() < 1e-6);
    }

    #[test]
    fn file_class_boost_matches_legacy() {
        let p = FileClassWeights::default();
        assert!((p.boost_for("Documentation") - 1.4).abs() < 1e-6);
        assert!((p.boost_for("Source") - 1.0).abs() < 1e-6);
        assert!((p.boost_for("Binary") - 0.0).abs() < 1e-6);
    }

    /// Parity test: context weights must stay equal to the canonical
    /// `knocode_core::ranking` table (the same table storage uses for BM25
    /// hit scoring). Drift in either direction fails the build.
    #[test]
    fn file_class_weights_parity_with_core() {
        let p = FileClassWeights::default();
        for class in ["Documentation", "Config", "Source", "Test", "Generated", "Stylesheet", "Binary", "Vendor", "Dependency", "SomethingElse"] {
            assert_eq!(
                p.boost_for(class),
                knocode_core::ranking::file_class_boost(class),
                "FileClassWeights drifted from canonical table for class {class}"
            );
        }
    }

    /// Parity test: directory weights must stay equal to the canonical
    /// `knocode_core::ranking` defaults used by storage-side BM25 scoring.
    #[test]
    fn directory_weights_parity_with_core() {
        let d = DirectoryWeights::default();
        for path in ["README.md", "CONTRIBUTING.md", "docs/guide.md", "types/foo/index.d.ts", "pnpm-workspace.yaml", "src/main.rs"] {
            assert_eq!(
                d.boost_for(path),
                knocode_core::ranking::directory_boost(path),
                "DirectoryWeights drifted from canonical table for path {path}"
            );
        }
    }

    #[test]
    fn directory_boost_matches_legacy() {
        let d = DirectoryWeights::default();
        assert!((d.boost_for("README.md") - 1.3).abs() < 1e-6);
        assert!((d.boost_for("a/docs/api.md") - 1.2).abs() < 1e-6);
        assert!((d.boost_for("types/foo/index.d.ts") - 1.15).abs() < 1e-6);
        assert!((d.boost_for("src/main.rs") - 1.0).abs() < 1e-6);
    }

    #[test]
    fn test_multiplier_query_aware() {
        let p = RetrievalPolicy::default();
        assert!((p.test_multiplier("fix the test suite", "Test") - 1.4).abs() < 1e-6);
        assert!((p.test_multiplier("authentication middleware", "Test") - 0.6).abs() < 1e-6);
        assert!((p.test_multiplier("anything", "Source") - 1.0).abs() < 1e-6);
    }

    /// Parity: canonical STOP_WORDS must cover every word the old context-side
    /// lists filtered (a sample of former entries; the canonical list is their
    /// documented union).
    #[test]
    fn stop_words_parity_with_former_context_list() {
        let former = [
            "a","an","the","is","are","was","were","be","been","being",
            "have","has","had","do","does","did","will","would","could",
            "should","may","might","shall","can","to","of","in","for",
            "on","with","at","by","from","as","into","through","during",
            "before","after","above","below","between","and","but","or",
            "nor","not","so","yet","both","either","neither","each",
            "every","all","any","few","more","most","other","some",
            "such","no","only","own","same","than","too","very",
            "just","because","if","when","where","how","what","which",
            "who","whom","this","that","these","those",
        ];
        for w in former {
            assert!(
                knocode_core::ranking::STOP_WORDS.contains(&w),
                "canonical STOP_WORDS lost former entry '{w}' — query-token behavior would change"
            );
        }
    }

    #[test]
    fn intent_aware_procedural_upweights_docs() {
        let p = RetrievalPolicy::default();
        let proc_docs = p.intent_file_class_boost("Documentation", QueryIntent::Procedural);
        let proc_test = p.intent_file_class_boost("Test", QueryIntent::Procedural);
        assert!((proc_docs - 2.5).abs() < 1e-6);
        assert!((proc_test - 0.25).abs() < 1e-6);
        // debugging flips
        let dbg_src = p.intent_file_class_boost("Source", QueryIntent::Debugging);
        let dbg_test = p.intent_file_class_boost("Test", QueryIntent::Debugging);
        assert!(dbg_src > 1.0);
        assert!(dbg_test > 1.0);
        // testing upweights test
        let t_test = p.intent_file_class_boost("Test", QueryIntent::Testing);
        assert!((t_test - 1.5).abs() < 1e-6);
    }

    #[test]
    fn authority_canonical_readme_highest() {
        let p = RetrievalPolicy::default();
        assert!((p.doc_authority_boost("README.md") - 1.50).abs() < 1e-6);
        assert!((p.doc_authority_boost("README.ja.md") - 1.10).abs() < 1e-6);
        assert!((p.doc_authority_boost("docs/index.md") - 1.30).abs() < 1e-6);
        assert!((p.doc_authority_boost("docs/api.md") - 1.20).abs() < 1e-6);
        assert!((p.doc_authority_boost(".github/CONTRIBUTING.md") - 1.10).abs() < 1e-6);
        assert!((p.doc_authority_boost("src/main.md") - 1.00).abs() < 1e-6);
        assert!(p.doc_authority_boost("README.md") > p.doc_authority_boost("README.ja.md"));
    }

    #[test]
    fn combined_boost_procedural_readme_wins() {
        let p = RetrievalPolicy::default();
        let readme = p.combined_boost("README.md", "Documentation", "How do I add a new package?", QueryIntent::Procedural);
        let test_file = p.combined_boost("types/foo/test/run.ts", "Test", "How do I add a new package?", QueryIntent::Procedural);
        // README should dominate for procedural even though test might have lexical relevance
        assert!(readme > test_file, "readme {readme} should beat test {test_file} for procedural");
    }

    // ── Docs-slot reservation (promote-only) ──

    fn doc_fixture() -> Vec<(String, &'static str)> {
        vec![
            ("src/a.rs".into(), "Source"),
            ("src/b.rs".into(), "Source"),
            ("src/c.rs".into(), "Source"),
            ("src/d.rs".into(), "Source"),
            ("src/e.rs".into(), "Source"),
            ("docs/guide.md".into(), "Documentation"),
            ("docs/other.md".into(), "Documentation"),
            ("README.md".into(), "Documentation"),
        ]
    }

    #[test]
    fn reserve_promotes_docs_into_kept_tail() {
        let p = RetrievalPolicy::default();
        // 5 code (kept) + 3 docs (all below the cut, keep=5)
        let mut ranked = doc_fixture();
        p.reserve_doc_slots(&mut ranked, 5, 2,
            |e| e.1 == "Documentation", |e| e.1.to_string());
        // First 3 code entries untouched, last 2 kept slots are docs.
        assert_eq!(ranked[0].0, "src/a.rs");
        assert_eq!(ranked[1].0, "src/b.rs");
        assert_eq!(ranked[2].0, "src/c.rs");
        assert_eq!(ranked[3].0, "docs/guide.md");
        assert_eq!(ranked[4].0, "docs/other.md");
    }

    #[test]
    fn reserve_keeps_docs_already_in_tail() {
        let p = RetrievalPolicy::default();
        let mut ranked = doc_fixture();
        ranked.reverse(); // README first, code last — docs own the tail
        p.reserve_doc_slots(&mut ranked, 5, 2,
            |e| e.1 == "Documentation", |e| e.1.to_string());
        assert_eq!(ranked[3].1, "Documentation");
        assert_eq!(ranked[4].1, "Documentation");
    }

    #[test]
    fn reserve_noop_when_no_docs_below_cut() {
        let p = RetrievalPolicy::default();
        let mut ranked: Vec<(String, &str)> = (0..8)
            .map(|i| (format!("src/f{i}.rs"), if i < 5 { "Source" } else { "Documentation" }))
            .collect();
        // Only code in kept region, no docs anywhere → nothing to promote.
        ranked[5].1 = "Source";
        ranked[6].1 = "Source";
        ranked[7].1 = "Source";
        p.reserve_doc_slots(&mut ranked, 5, 2,
            |e| e.1 == "Documentation", |e| e.1.to_string());
        assert!(ranked.iter().take(5).all(|e| e.1 == "Source"));
    }

    #[test]
    fn reserve_disabled_when_slots_zero() {
        let p = RetrievalPolicy::default();
        let mut ranked = doc_fixture();
        p.reserve_doc_slots(&mut ranked, 5, 0,
            |e| e.1 == "Documentation", |e| e.1.to_string());
        assert_eq!(ranked[3].0, "src/d.rs");
        assert_eq!(ranked[4].0, "src/e.rs");
    }

    #[test]
    fn reserve_uses_first_docs_in_ranked_order() {
        let p = RetrievalPolicy::default();
        let mut ranked = doc_fixture();
        // Three docs below the cut, only 2 slots → the two best-ranked docs win.
        p.reserve_doc_slots(&mut ranked, 5, 2,
            |e| e.1 == "Documentation", |e| e.1.to_string());
        assert_eq!(ranked[3].0, "docs/guide.md");
        assert_eq!(ranked[4].0, "docs/other.md");
        assert_eq!(ranked[5].0, "src/d.rs"); // displaced below the cut
        assert_eq!(ranked[6].0, "src/e.rs"); // displaced below the cut
        assert_eq!(ranked[7].0, "README.md"); // lowest-ranked doc, still outside
    }

    #[test]
    fn reserve_partial_tail_doc_counts_toward_quota() {
        let p = RetrievalPolicy::default();
        let mut ranked = doc_fixture();
        // One doc already in the kept tail (index 4); only 1 promotion needed.
        ranked.swap(4, 5); // src/e.rs ↔ docs/guide.md
        p.reserve_doc_slots(&mut ranked, 5, 2,
            |e| e.1 == "Documentation", |e| e.1.to_string());
        assert_eq!(ranked[4].0, "docs/guide.md"); // kept in place
        assert_eq!(ranked[3].0, "docs/other.md"); // promoted to fill the other slot
    }

    // ── P1.4: Performance Budget Tests ──

    #[test]
    fn default_performance_budgets() {
        let p = RetrievalPolicy::default();
        assert_eq!(p.lexical_budget_ms, 2000);
        assert_eq!(p.structural_budget_ms, 3000);
        assert_eq!(p.total_budget_ms, 5000);
        assert_eq!(p.max_matches_per_backend, 200);
    }

    #[test]
    fn budget_values_are_reasonable() {
        let p = RetrievalPolicy::default();
        assert!(p.lexical_budget_ms > 0);
        assert!(p.structural_budget_ms > 0);
        assert!(p.total_budget_ms >= p.lexical_budget_ms);
        assert!(p.total_budget_ms >= p.structural_budget_ms);
        assert!(p.max_matches_per_backend > 0);
        assert!(p.max_matches_per_backend <= 1000);
    }

    // ── P1.5: Root .md boost tests ──

    #[test]
    fn root_md_files_get_docs_boost() {
        let _d = DirectoryWeights::default();
        // Root-level .md files (CONTRIBUTING.md, CHANGELOG.md, etc.) should get docs_generic
        // This is tested via DocumentationAuthority
        let auth = DocumentationAuthority::default();
        assert!(auth.boost_for("CONTRIBUTING.md") >= 1.2, "CONTRIBUTING.md should get docs_generic boost");
        assert!(auth.boost_for("CHANGELOG.md") >= 1.2, "CHANGELOG.md should get docs_generic boost");
        assert!(auth.boost_for("AUTHORS.md") >= 1.2, "AUTHORS.md should get docs_generic boost");
        // Non-md files should not get doc boost
        assert!((auth.boost_for("src/main.rs") - 1.0).abs() < 1e-6);
    }
}
