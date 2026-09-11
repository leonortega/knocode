//! Retrieval Engine — deterministic, independently testable.
//! Invariant: **Retrieval determines relevance; Context determines inclusion.**
//! Scoring: `Final = lexical_relevance × intent_authority × document_authority × structural`

use std::collections::{HashMap, HashSet};
use std::time::Instant;

use knocode_repo_intel::RepositoryIntelligence;

use crate::retrieval::evidence::{BackendMetrics, Evidence, EvidenceSource, RetrievalDiagnostics, RetrievalResult, RetrievalSignal};
use crate::retrieval::intent::QueryIntent;
use crate::retrieval::plan::RetrievalPlan;
use crate::retrieval::policy::RetrievalPolicy;
use crate::retrieval::query::RetrievalQuery;
use crate::retrieval::structural::StructuralRetriever;
use crate::retrieval::{ranking, vocab};

/// Trait for retrieval — stable contract between retrieval and context.
pub trait Retriever {
    fn retrieve(
        &self,
        query: &RetrievalQuery,
        repo_intel: &RepositoryIntelligence,
        policy: &RetrievalPolicy,
    ) -> RetrievalResult;
}

/// Combined retriever that uses a RetrievalPlan to weight capabilities.
///
/// Intent is a **policy input**, not a hard router. The plan decides
/// which backends participate and with what weight, then evidence is
/// merged and re-ranked.
///
/// ```text
/// Query
///   → detect_intent() → RetrievalPlan
///     ├── lexical_weight > 0 → TantivyRetriever
///     ├── structural_weight > 0 → StructuralRetriever
///     └── merge normalized Evidence
/// ```
pub struct CombinedRetriever {
    pub tantivy: TantivyRetriever,
    pub structural: StructuralRetriever,
}

impl Default for CombinedRetriever {
    fn default() -> Self {
        Self {
            tantivy: TantivyRetriever,
            structural: StructuralRetriever,
        }
    }
}

impl CombinedRetriever {
    /// Retrieve with an explicit plan (for testing and policy override).
    pub fn retrieve_with_plan(
        &self,
        query: &RetrievalQuery,
        repo_intel: &RepositoryIntelligence,
        policy: &RetrievalPolicy,
        plan: &RetrievalPlan,
    ) -> RetrievalResult {
        if !plan.has_any_retrieval() {
            return RetrievalResult::empty(knocode_core::RetrievalStatus::NoMatch);
        }

        let t0 = Instant::now();
        let mut lexical_result = None;
        let mut structural_result = None;

        // Phase 1: Run participating backends
        if plan.lexical {
            let result = self.tantivy.retrieve(query, repo_intel, policy);
            // P2: Log backend metrics
            for m in &result.diagnostics.backends {
                tracing::info!(
                    backend = %m.backend,
                    query = %m.query,
                    language = ?m.language,
                    candidates = m.candidate_count,
                    matches = m.match_count,
                    duration_ms = m.duration_ms,
                    status = %m.status,
                    "retrieval backend"
                );
            }
            if !result.evidence.is_empty() {
                lexical_result = Some(result);
            }
        }

        if plan.structural {
            let result = self.structural.retrieve(query, repo_intel, policy);
            // P2: Log backend metrics
            for m in &result.diagnostics.backends {
                tracing::info!(
                    backend = %m.backend,
                    query = %m.query,
                    language = ?m.language,
                    candidates = m.candidate_count,
                    matches = m.match_count,
                    duration_ms = m.duration_ms,
                    status = %m.status,
                    "retrieval backend"
                );
            }
            if !result.evidence.is_empty() {
                structural_result = Some(result);
            }
        }

        // Phase 2: Merge evidence with plan weights
        let result = match (lexical_result, structural_result) {
            (Some(lex), Some(struc)) => {
                merge_evidence(lex, struc, plan, policy)
            }
            (Some(lex), None) => lex,
            (None, Some(struc)) => struc,
            (None, None) => RetrievalResult::empty(knocode_core::RetrievalStatus::NoMatch),
        };

        // P2: Log combined retrieval summary
        let total_ms = t0.elapsed().as_millis() as u64;
        let lexical_ms = result.diagnostics.tantivy_ms;
        let structural_ms = result.diagnostics.structural_ms;
        let symbolic_ms = result.diagnostics.ranking_ms;
        tracing::info!(
            retrieval_total_ms = total_ms,
            lexical_ms = lexical_ms,
            symbolic_ms = symbolic_ms,
            structural_ms = structural_ms,
            evidence_count = result.evidence.len(),
            status = ?result.status,
            "retrieval summary"
        );

        result
    }
}

impl Retriever for CombinedRetriever {
    fn retrieve(
        &self,
        query: &RetrievalQuery,
        repo_intel: &RepositoryIntelligence,
        policy: &RetrievalPolicy,
    ) -> RetrievalResult {
        let plan = self.build_plan(query);
        self.retrieve_with_plan(query, repo_intel, policy, &plan)
    }
}

impl CombinedRetriever {
    /// Build an explicit, inspectable RetrievalPlan from a query.
    ///
    /// Uses QueryPlanner to resolve structural patterns, then maps
    /// intent → plan with all fields populated.
    pub fn build_plan(&self, query: &RetrievalQuery) -> RetrievalPlan {
        use crate::retrieval::structural_plan::{QueryPlanner, StructuralQuery};

        let intent = query.intent();
        let has_structural = crate::retrieval::structural::parse_structural_query(&query.text).is_some();
        let mut plan = RetrievalPlan::from_intent(intent, has_structural);

        // Use QueryPlanner to resolve structural patterns
        let sq = StructuralQuery::new(query.text.clone())
            .with_language(query.language.clone().unwrap_or_default());
        let resolved = QueryPlanner::plan(&sq);

        if !resolved.is_empty() {
            // Take the highest-priority (first) pattern
            plan.structural_pattern = Some(resolved[0].pattern.clone());
            plan.structural_intent = Some(sq.intent);
            if let Some(ref lang) = query.language {
                plan.languages = vec![lang.clone()];
            }
        }

        plan
    }
}

/// Merge evidence from multiple backends using plan weights.
fn merge_evidence(
    mut primary: RetrievalResult,
    secondary: RetrievalResult,
    plan: &RetrievalPlan,
    policy: &RetrievalPolicy,
) -> RetrievalResult {
    // Normalize scores by plan weights
    for ev in &mut primary.evidence {
        ev.score *= plan.lexical_weight;
    }

    // Build index of primary paths
    let primary_paths: HashMap<String, usize> = primary.evidence.iter().enumerate()
        .map(|(i, ev)| (ev.path.to_string_lossy().to_string(), i))
        .collect();

    // Add secondary evidence
    for mut ev in secondary.evidence {
        let path_str = ev.path.to_string_lossy().to_string();
        ev.score *= plan.structural_weight;

        if let Some(&idx) = primary_paths.get(&path_str) {
            // Path exists in primary — take max score
            if ev.score > primary.evidence[idx].score {
                primary.evidence[idx].score = ev.score;
                primary.evidence[idx].source = ev.source;
                primary.evidence[idx].signals.extend(ev.signals);
            }
        } else {
            // New path from secondary
            primary.evidence.push(ev);
        }
    }

    // Re-rank and truncate (deterministic: score desc, path asc)
    ranking::sort_evidence_by_score_desc(&mut primary.evidence);
    let max = plan.max_evidence(policy.max_files);
    // Docs-slot reservation: promote Documentation evidence into the kept
    // region's tail before truncation (promote-only — see policy.rs
    // reserve_doc_slots).
    let docs_reserve = policy.effective_docs_reserve().min(max);
    policy.reserve_doc_slots(
        &mut primary.evidence,
        max,
        docs_reserve,
        |ev| ev.file_class == "Documentation",
        |ev| ev.file_class.clone(),
    );
    primary.evidence.truncate(max);

    // Combine diagnostics
    primary.diagnostics.candidate_count += secondary.diagnostics.candidate_count;
    primary.diagnostics.tantivy_ms += secondary.diagnostics.tantivy_ms;
    primary.diagnostics.ranking_ms += secondary.diagnostics.ranking_ms;
    // Merge backend metrics
    primary.diagnostics.backends.extend(secondary.diagnostics.backends);

    primary
}

// ── TantivyRetriever (unchanged) ────────────────────────────────────────

/// Tantivy-backed retriever — BM25 lexical + symbol search.
pub struct TantivyRetriever;

/// Panic-guarded fulltext search — single home for the catch_unwind that
/// shields against Tantivy 0.26.1 phrase-query panics ("target should be >= doc")
/// on large indices. Returns `Err(panic_message)` on panic so callers fall back
/// to `search_text` (ripgrep) instead of crashing.
fn search_fulltext_guarded(
    repo_intel: &RepositoryIntelligence,
    q: &str,
    language: Option<&str>,
    candidate_k: usize,
    repository_id: &str,
) -> Result<knocode_core::SearchResults, String> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        repo_intel.search_fulltext(q, language, candidate_k, Some(repository_id))
    }))
    .unwrap_or_else(|panic_info| {
        let msg = if let Some(s) = panic_info.downcast_ref::<&str>() {
            s.to_string()
        } else if let Some(s) = panic_info.downcast_ref::<String>() {
            s.clone()
        } else {
            "unknown panic".to_string()
        };
        tracing::warn!(query = %q, panic = %msg, "Tantivy phrase query panicked — falling back to ripgrep");
        Err(msg)
    })
}

impl Retriever for TantivyRetriever {
    fn retrieve(
        &self,
        query: &RetrievalQuery,
        repo_intel: &RepositoryIntelligence,
        policy: &RetrievalPolicy,
    ) -> RetrievalResult {
        let t0 = Instant::now();

        // ── Query understanding: deterministic intent + bounded vocabulary ──
        let intent = query.intent();
        let q_tokens = ranking::query_tokens(&query.text);
        let expanded_query = if q_tokens.is_empty() {
            query.text.clone()
        } else {
            let expanded_terms = vocab::expand_terms(&q_tokens);
            if expanded_terms.len() > q_tokens.len() {
                vocab::expanded_query_string(&query.text, &q_tokens)
            } else {
                query.text.clone()
            }
        };
        let has_expansion = expanded_query != query.text;

        // Validate index
        let doc_count = match repo_intel.validate_index() {
            Ok(s) => s.doc_count,
            Err(ref e) if e == "index not built" => {
                return RetrievalResult::empty(knocode_core::RetrievalStatus::IndexNotBuilt);
            }
            Err(ref e) if e == "index is empty" => 0,
            Err(_) => {
                return RetrievalResult::empty(knocode_core::RetrievalStatus::IndexUnavailable);
            }
        };        // Structural mode: "find all X" queries need exhaustive results, not top-50.
        // Detect structural intent and increase limits significantly.
        // FIX #5: compute to_lowercase() once instead of 11 times.
        let q_lower = query.text.to_lowercase();
        let is_structural_exhaustive = matches!(intent,
            QueryIntent::Structural
        ) && (q_lower.starts_with("find all")
            || q_lower.starts_with("show all")
            || q_lower.starts_with("list all")
            || q_lower.contains("all error")
            || q_lower.contains("all config")
            || q_lower.contains("all test")
            || q_lower.contains("all api")
            || q_lower.contains("all database")
            || q_lower.contains("all enum")
            || q_lower.contains("all interface")
            || q_lower.contains("all type"));

        let effective_max = if is_structural_exhaustive {
            // FIX #4: Use configurable structural settings from policy
            policy.structural_max_files.min(doc_count)
        } else {
            policy.effective_max_files(doc_count)
        };
        let candidate_k = if is_structural_exhaustive {
            policy.structural_candidate_k
        } else {
            policy.effective_candidate_k_for(doc_count)
        };

        // Build retrieval plan from intent (auto-enables graph for debugging/implementation)
        let has_structural = crate::retrieval::structural::parse_structural_query(&query.text).is_some();
        let plan = RetrievalPlan::from_intent(intent, has_structural);

        let mut used_fallback = false;
        let mut status = knocode_core::RetrievalStatus::NoMatch;

        // Phrase query fallback: Tantivy 0.26.1 panics on phrase queries with large indices
        // ("target should be >= doc"). Catch the panic and fall back to individual term search.
        //
        // Weighted expansion (see vocab::weighted_query_pair): the ORIGINAL
        // query runs at full weight; the synonym-expanded query is a gap-filler
        // branch scaled by EXPANSION_WEIGHT. Previously the expanded OR-query
        // was the primary search, diluting original-term BM25 weights.
        let search_results = match search_fulltext_guarded(repo_intel, &query.text, query.language.as_deref(), candidate_k, &query.repository_id) {
            // Normal success path
            Ok(sr) if sr.total_count > 0 => {
                status = knocode_core::RetrievalStatus::Found(sr.total_count);
                sr
            }
            // Success but empty — try fallback
            Ok(_) => {
                used_fallback = true;
                let fallback_q = if has_expansion { expanded_query.clone() } else { query.text.clone() };
                match repo_intel.search_text(&fallback_q, query.language.as_deref(), candidate_k) {
                    Ok(sr) if sr.total_count > 0 => {
                        status = knocode_core::RetrievalStatus::Found(sr.total_count);
                        sr
                    }
                    Ok(_) => knocode_core::SearchResults { results: vec![], total_count: 0 },
                    Err(e) => return RetrievalResult::empty(knocode_core::RetrievalStatus::RetrievalFailed(e)),
                }
            }
            // PANIC from Tantivy phrase query — helper already logged; fall back to ripgrep
            Err(_panic_msg) => {
                used_fallback = true;
                let fallback_q = if has_expansion { expanded_query.clone() } else { query.text.clone() };
                match repo_intel.search_text(&fallback_q, query.language.as_deref(), candidate_k) {
                    Ok(sr) if sr.total_count > 0 => {
                        status = knocode_core::RetrievalStatus::FallbackUsed("tantivy-panic→ripgrep".to_string());
                        sr
                    }
                    Ok(_) => knocode_core::SearchResults { results: vec![], total_count: 0 },
                    Err(e) => return RetrievalResult::empty(knocode_core::RetrievalStatus::RetrievalFailed(e)),
                }
            }
        };

        if search_results.total_count > 0 && used_fallback {
            status = knocode_core::RetrievalStatus::FallbackUsed("tantivy→ripgrep".to_string());
        } else if search_results.total_count == 0 {
            status = knocode_core::RetrievalStatus::NoMatch;
        }

        // ── Weighted expansion branch (max-combine) ──
        // Only when the full-weight original query under-fills the candidate
        // pool: add synonym-branch hits not already present, scaled by
        // EXPANSION_WEIGHT so an original-term hit always outranks a
        // synonym-only hit of the same base score.
        let mut search_results = search_results;
        if has_expansion && search_results.total_count < effective_max {
            let expanded_run = search_fulltext_guarded(repo_intel, &expanded_query, query.language.as_deref(), candidate_k, &query.repository_id);
            if let Ok(mut exp_sr) = expanded_run {
                let seen: HashSet<String> = search_results.results.iter().map(|r| r.path.clone()).collect();
                let before = search_results.results.len();
                for r in exp_sr.results.iter_mut() {
                    if !seen.contains(&r.path) {
                        r.score *= vocab::EXPANSION_WEIGHT as f64;
                        search_results.results.push(r.clone());
                    }
                }
                search_results.total_count = search_results.results.len();
                if search_results.results.len() > before && status == knocode_core::RetrievalStatus::NoMatch {
                    status = knocode_core::RetrievalStatus::Found(search_results.total_count);
                }
            }
        }

        let tantivy_ms = t0.elapsed().as_millis() as u64;

        // Dedup BM25 hits + valid file paths
        let mut seen_bm25 = HashSet::new();
        let mut bm25_scored: Vec<(String, f64, String, EvidenceSource)> = Vec::new();
        for r in &search_results.results {
            if seen_bm25.insert(r.path.clone()) && is_valid_file_path(&r.path) {
                let fc = infer_file_class(&r.path);
                bm25_scored.push((r.path.clone(), r.score, fc, EvidenceSource::Tantivy));
            }
        }

        // Symbol results
        let mut seen_sym = HashSet::new();
        let mut symbol_scored: Vec<(String, f64, String, EvidenceSource)> = Vec::new();
        let symbol_query = if has_expansion { &expanded_query } else { &query.text };
        if let Ok(syms) = repo_intel.search_symbols(symbol_query, effective_max * 2) {
            for sym in syms {
                if seen_sym.insert(sym.path.clone()) && is_valid_file_path(&sym.path) {
                    let fc = infer_file_class(&sym.path);
                    symbol_scored.push((sym.path.clone(), sym.score, fc, EvidenceSource::Symbol));
                }
            }
        }

        // Symbol-match boost
        let t_rank = Instant::now();
        let expanded_tokens = if has_expansion {
            vocab::expand_terms(&q_tokens)
        } else {
            q_tokens.clone()
        };
        if !expanded_tokens.is_empty() {
            for (path, score, _fc, _src) in bm25_scored.iter_mut() {
                let haystack = path.to_lowercase(); // FIX #1: removed useless format!("{} {}", "", path)
                let matched = ranking::count_path_token_matches(&haystack, &expanded_tokens);
                if matched > 0 {
                    let boost = 1.0 + (matched as f32 / expanded_tokens.len() as f32) * policy.symbol_match_weight;
                    *score *= boost as f64;
                }
            }
        }

        // Merge BM25 + symbol
        let by_path = ranking::merge_by_path(bm25_scored, symbol_scored);
        let mut merged: Vec<(String, f64)> = by_path.iter().map(|(p, (s, _, _))| (p.clone(), *s)).collect();
        ranking::sort_by_score_desc(&mut merged);

        // Code-behind
        let _cb_signals = ranking::add_code_behind(&mut merged, policy);
        merged.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));

        // Graph boost — auto-enabled by intent (debugging/implementation) on medium repos,
        // or forced via KNOCODE_BUILD_GRAPH=1. Skipped on large repos (>5k) and type-defs-heavy repos.
        // Fast path: if global doc_count > 10k, skip expensive file_count walk (repo is definitely large)
        let force_graph = std::env::var("KNOCODE_BUILD_GRAPH").ok().as_deref() == Some("1");
        let graph_enabled = (plan.graph || policy.enable_graph || force_graph)
            && (doc_count <= 5000 || repo_intel.file_count() <= 5000);
        let mut graph_ms = 0u64;
        let mut graph_signals: HashMap<String, RetrievalSignal> = HashMap::new();
        if merged.len() >= 2 && graph_enabled {
            let tg = Instant::now();
            // FIX #3: build_dependency_graph() uses self.repo_path internally,
            // no need to canonicalize separately — the old code dropped the path.
            if let Ok(graph) = repo_intel.build_dependency_graph() {
                let boosted = ranking::apply_graph_boost(&mut merged, &graph, policy);
                for (path, sig) in boosted {
                    graph_signals.insert(path, sig);
                }
                ranking::sort_by_score_desc(&mut merged);
            }
            graph_ms = tg.elapsed().as_millis() as u64;
        }

        // Intent-aware authority scoring
        let generic_weights = crate::retrieval::policy::FileClassWeights::default();
        let mut intent_signals: HashMap<String, Vec<RetrievalSignal>> = HashMap::new();
        for (path, score) in merged.iter_mut() {
            if let Some((_, file_class, _)) = by_path.get(path) {
                let generic = generic_weights.boost_for(file_class);
                let intent_boost = policy.intent_file_class_boost(file_class, intent);
                let doc_auth = policy.doc_authority_boost(path);
                let intent_delta = if generic > 0.01 { intent_boost / generic } else { intent_boost };
                let mut extra = 1.0;
                let mut sigs = Vec::new();
                if (intent_delta - 1.0).abs() > 0.01 {
                    extra *= intent_delta;
                    sigs.push(RetrievalSignal::IntentBoost { intent: intent.to_string(), boost: intent_delta });
                }
                if (doc_auth - 1.0).abs() > 0.01 {
                    extra *= doc_auth;
                    sigs.push(RetrievalSignal::DocAuthority(doc_auth));
                }
                if has_expansion {
                    sigs.push(RetrievalSignal::QueryExpansion(1.05));
                    extra *= 1.05;
                }
                if extra != 1.0 {
                    *score *= extra as f64;
                }
                if !sigs.is_empty() {
                    intent_signals.insert(path.clone(), sigs);
                }
            }
        }
        ranking::sort_by_score_desc(&mut merged);

        // P0 eval-quality fix: damp generic meta-docs. Documentation files whose
        // PATH shares no token with the query matched via content generality and
        // stacked class×dir×authority×intent priors — putting a .md at rank 1 for
        // 47/50 golden-eval tasks and pushing the actual code target to rank ~3.
        //
        // Prior-only damping proved insufficient (explain output: README's pure
        // BM25 can be 3× the target code file's), so the damping multiplies the
        // FULL final score of generic docs. Genuinely on-topic docs are carried
        // by their raw BM25 and survive; generic meta-docs fall below the code
        // files they used to bury. Code/Config/Test files are never damped.
        let damped_paths: HashSet<String> = {
            let mut damped_paths: HashSet<String> = HashSet::new();
            for (path, score) in merged.iter_mut() {
                if let Some((_, file_class, _)) = by_path.get(path.as_str()) {
                    if ranking::is_generic_doc(path, file_class, &q_tokens) {
                        *score *= policy.doc_prior_damping as f64;
                        damped_paths.insert(path.clone());
                    }
                }
            }
            ranking::sort_by_score_desc(&mut merged);
            damped_paths
        };

        // Build Evidence with signals
        let mut signals_by_path: HashMap<String, Vec<RetrievalSignal>> = HashMap::new();
        for (path, score) in merged.iter() {
            let mut sigs = Vec::new();
            sigs.push(RetrievalSignal::TantivyScore(*score as f32));
            if damped_paths.contains(path.as_str()) {
                sigs.push(RetrievalSignal::DocPriorDamping { factor: policy.doc_prior_damping });
            }
            if let Some(gs) = graph_signals.get(path) {
                sigs.push(gs.clone());
            }
            if let Some(is) = intent_signals.get(path) {
                sigs.extend(is.clone());
            }
            signals_by_path.insert(path.clone(), sigs);
        }

        // Docs-slot reservation (floor of the documented 45% docs budget,
        // REQUEST_LIFECYCLE.md:312): promote the top-scoring Documentation
        // candidates into the kept region's tail slots AFTER damping
        // re-ranking, BEFORE truncation. Promote-only — scores are never
        // rescaled and the leading code-first order is untouched, so the P0
        // damping eval result holds.
        let docs_reserve = policy.effective_docs_reserve().min(effective_max);
        {
            let is_doc = |e: &(String, f64)| {
                by_path.get(&e.0).map(|(_, fc, _)| fc.as_str() == "Documentation").unwrap_or(false)
            };
            let class_of = |e: &(String, f64)| {
                by_path.get(&e.0).map(|(_, fc, _)| fc.clone()).unwrap_or_default()
            };
            policy.reserve_doc_slots(&mut merged, effective_max, docs_reserve, is_doc, class_of);
        }

        let merged_len = merged.len();
        let mut evidence: Vec<Evidence> = Vec::new();
        // Reserved slots sit at [kept_len - docs_reserve, kept_len) — kept_len
        // may be less than effective_max when the repo has few candidates.
        let kept_len = merged_len.min(effective_max);
        let docs_tail_start = kept_len.saturating_sub(docs_reserve);
        for (rank_pos, (path, score)) in merged.into_iter().take(effective_max).enumerate() {
            if let Some((_, file_class, source)) = by_path.get(&path) {
                let mut ev = Evidence::new(path.clone(), score as f32 * knocode_core::ranking::SCORE_SCALE, file_class.clone());
                ev.raw_score = score as f32;
                ev.source = source.clone();
                ev.matched_terms = expanded_tokens.clone();
                if let Some(sigs) = signals_by_path.get(&path) {
                    ev.signals = sigs.clone();
                }
                if rank_pos >= docs_tail_start && file_class == "Documentation" {
                    ev.signals.push(RetrievalSignal::DocsQuota { slot: rank_pos - docs_tail_start });
                }
                let mut extra = Vec::new();
                ranking::apply_class_and_dir_boost_with_query_tokens(1.0, &path, file_class, &q_tokens, policy, &mut extra);
                ev.signals.extend(extra);
                evidence.push(ev);
            }
        }

        ranking::sort_evidence_by_score_desc(&mut evidence);
        let ranking_ms = t_rank.elapsed().as_millis() as u64;
        let evidence_count = evidence.len();

        if std::env::var("KNOCODE_RETRIEVAL_EXPLAIN").is_ok() {
            eprintln!("[retrieval] intent={} expanded={} has_expansion={}", intent, expanded_query, has_expansion);
        }

        RetrievalResult {
            evidence,
            status,
            diagnostics: RetrievalDiagnostics {
                candidate_count: by_path.len(),
                filtered_count: 0,
                tantivy_ms,
                ranking_ms,
                graph_ms,
                structural_ms: 0,
                doc_count,
                candidate_k,
                max_files: effective_max,
                backends: vec![
                    BackendMetrics {
                        backend: "tantivy".into(),
                        query: expanded_query.clone(),
                        language: query.language.clone(),
                        candidate_count: by_path.len(),
                        match_count: evidence_count,
                        duration_ms: tantivy_ms,
                        status: if used_fallback { "fallback".into() } else { "ok".into() },
                    },
                    BackendMetrics {
                        backend: "ranking".into(),
                        query: query.text.clone(),
                        language: None,
                        candidate_count: merged_len,
                        match_count: evidence_count,
                        duration_ms: ranking_ms,
                        status: "ok".into(),
                    },
                ],
            },
        }
    }
}

fn is_valid_file_path(path: &str) -> bool {
    if path.len() < 5 { return false; }
    let lower = path.to_lowercase();
    if lower == "todo" || lower == "no" || lower == "this" || lower == "true" || lower == "false" { return false; }
    if path.contains('/') || path.contains('\\') { return true; }
    if path.contains('.') { return true; }
    false
}

fn infer_file_class(path: &str) -> String {
    use knocode_repo_intel::registry::{classify_file, FileClass};
    let p = std::path::Path::new(path);
    match classify_file(p) {
        FileClass::Source => "Source".to_string(),
        FileClass::Documentation => "Documentation".to_string(),
        FileClass::Config => "Config".to_string(),
        FileClass::Test => "Test".to_string(),
        FileClass::Generated => "Generated".to_string(),
        FileClass::Vendor => "Vendor".to_string(),
        FileClass::Dependency => "Dependency".to_string(),
        FileClass::Binary => "Binary".to_string(),
        FileClass::Unknown => "Source".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::query::RetrievalQuery;

    use crate::retrieval::intent::QueryIntent;

    #[test]
    fn plan_from_structural_intent() {
        let plan = RetrievalPlan::from_intent(QueryIntent::Structural, true);
        assert!(plan.lexical);
        assert!(plan.structural);
        assert!(plan.structural_weight > plan.lexical_weight);
    }

    #[test]
    fn plan_from_procedural_intent() {
        let plan = RetrievalPlan::from_intent(QueryIntent::Procedural, false);
        assert!(plan.lexical);
        assert!(!plan.structural);
    }

    #[test]
    fn plan_debugging_enriches_with_structural() {
        let plan = RetrievalPlan::from_intent(QueryIntent::Debugging, false);
        assert!(plan.structural);
        assert!(plan.graph);
    }

    // ── Regression tests for known queries ──
    // Ensure query classification doesn't break existing behavior.

    #[test]
    fn regression_structural_pattern_query() {
        // Explicit pattern should be Structural with both lexical + structural
        let intent = RetrievalQuery::new("fn $FUNC($$$) { $$$ }", "test").intent();
        let has_structural = crate::retrieval::structural::parse_structural_query("fn $FUNC($$$) { $$$ }").is_some();
        let plan = RetrievalPlan::from_intent(intent, has_structural);
        assert!(plan.structural, "structural pattern should activate structural retriever");
        assert!(plan.lexical, "structural should still get lexical candidates");
    }

    #[test]
    fn regression_procedural_does_not_invoke_structural() {
        // README/package queries must NOT invoke structural search
        let intent = RetrievalQuery::new("How do I add a new package?", "test").intent();
        assert_eq!(intent, QueryIntent::Procedural);
        let plan = RetrievalPlan::from_intent(intent, false);
        assert!(!plan.structural, "procedural query should not invoke structural search");
        assert!(plan.lexical, "procedural query should use lexical search");
    }

    #[test]
    fn regression_find_functions_invokes_structural() {
        let intent = RetrievalQuery::new("find all functions in the codebase", "test").intent();
        assert_eq!(intent, QueryIntent::Structural);
        let plan = RetrievalPlan::from_intent(intent, true);
        assert!(plan.structural, "find all functions should invoke structural search");
    }

    #[test]
    fn regression_where_is_uses_lexical() {
        // Navigation should use lexical, not structural
        let intent = RetrievalQuery::new("where is Foo implemented?", "test").intent();
        assert_eq!(intent, QueryIntent::Navigation);
        let plan = RetrievalPlan::from_intent(intent, false);
        assert!(!plan.structural, "navigation should not invoke structural search");
        assert!(plan.lexical, "navigation should use lexical search");
    }

    #[test]
    fn regression_debugging_enriches_with_structural() {
        let intent = RetrievalQuery::new("Why does the build fail?", "test").intent();
        assert_eq!(intent, QueryIntent::Debugging);
        let plan = RetrievalPlan::from_intent(intent, false);
        assert!(plan.structural, "debugging should enrich with structural");
        assert!(plan.graph, "debugging should use graph boost");
    }

    #[test]
    fn regression_show_all_classes_is_structural() {
        let intent = RetrievalQuery::new("show all classes", "test").intent();
        assert_eq!(intent, QueryIntent::Structural);
    }

    #[test]
    fn regression_what_is_pnpm_is_informational() {
        let intent = RetrievalQuery::new("what is pnpm", "test").intent();
        assert_eq!(intent, QueryIntent::Informational);
    }
}
