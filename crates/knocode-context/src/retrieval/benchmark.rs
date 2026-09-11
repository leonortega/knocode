//! Retrieval benchmark — permanent evaluation data for Recall@k and MRR.
//!
//! ## Metrics
//!
//! - **Recall@k**: Did the relevant file appear in the top-k results?
//! - **MRR** (Mean Reciprocal Rank): What is the reciprocal rank of the first relevant result?
//!
//! ## Usage
//!
//! ```text
//! cargo test retrieval::benchmark -- --nocapture
//! ```

use crate::retrieval::intent::QueryIntent;
#[cfg(test)]
use crate::retrieval::intent::detect_intent;
#[cfg(test)]
use crate::retrieval::policy::RetrievalPolicy;

// ── Evaluation Queries ────────────────────────────────────────────────

/// A single evaluation query with expected relevant files.
#[derive(Debug, Clone)]
pub struct EvalQuery {
    /// The query text.
    pub query: &'static str,
    /// Expected intent.
    pub expected_intent: QueryIntent,
    /// Relevant file paths (ground truth).
    pub relevant_files: Vec<&'static str>,
    /// Keywords that should appear in at least one result.
    pub expected_keywords: Vec<&'static str>,
}

/// Permanent evaluation dataset.
pub fn eval_queries() -> Vec<EvalQuery> {
    vec![
        // ── Procedural ──
        EvalQuery {
            query: "How do I add a new package?",
            expected_intent: QueryIntent::Procedural,
            relevant_files: vec!["README.md", "CONTRIBUTING.md"],
            expected_keywords: vec!["package", "add"],
        },
        EvalQuery {
            query: "how to create a new package",
            expected_intent: QueryIntent::Procedural,
            relevant_files: vec!["README.md"],
            expected_keywords: vec!["create", "package"],
        },
        // ── Debugging ──
        EvalQuery {
            query: "Why does the build fail?",
            expected_intent: QueryIntent::Debugging,
            relevant_files: vec!["Cargo.toml", "package.json"],
            expected_keywords: vec!["build", "error"],
        },
        // ── Structural ──
        EvalQuery {
            query: "find all functions in the codebase",
            expected_intent: QueryIntent::Structural,
            relevant_files: vec!["src/main.rs", "src/lib.rs"],
            expected_keywords: vec!["fn", "function"],
        },
        EvalQuery {
            query: "show all classes",
            expected_intent: QueryIntent::Structural,
            relevant_files: vec!["src/lib.rs"],
            expected_keywords: vec!["class", "struct"],
        },
        // ── Navigation ──
        EvalQuery {
            query: "where is the auth middleware",
            expected_intent: QueryIntent::Navigation,
            relevant_files: vec!["src/auth.rs", "src/middleware.rs"],
            expected_keywords: vec!["auth", "middleware"],
        },
        // ── Informational ──
        EvalQuery {
            query: "what is pnpm",
            expected_intent: QueryIntent::Informational,
            relevant_files: vec!["package.json", "pnpm-workspace.yaml"],
            expected_keywords: vec!["pnpm", "workspace"],
        },
        // ── Configuration ──
        EvalQuery {
            query: "configure the workspace",
            expected_intent: QueryIntent::Configuration,
            relevant_files: vec!["pnpm-workspace.yaml", "tsconfig.json"],
            expected_keywords: vec!["workspace", "config"],
        },
    ]
}

// ── Metrics ───────────────────────────────────────────────────────────

/// Compute Recall@k: fraction of relevant files found in top-k results.
pub fn recall_at_k(retrieved: &[&str], relevant: &[&str], k: usize) -> f64 {
    if relevant.is_empty() {
        return 1.0;
    }
    let top_k: std::collections::HashSet<&str> = retrieved.iter().take(k).copied().collect();
    let found = relevant.iter().filter(|r| top_k.contains(*r)).count();
    found as f64 / relevant.len() as f64
}

/// Compute MRR (Mean Reciprocal Rank): reciprocal rank of first relevant result.
pub fn mrr(retrieved: &[&str], relevant: &[&str]) -> f64 {
    for (i, file) in retrieved.iter().enumerate() {
        if relevant.contains(file) {
            return 1.0 / (i + 1) as f64;
        }
    }
    0.0
}

/// Check if any expected keyword appears in the results.
pub fn keyword_coverage(results: &[&str], keywords: &[&str]) -> f64 {
    if keywords.is_empty() {
        return 1.0;
    }
    let results_text = results.join(" ").to_lowercase();
    let found = keywords.iter().filter(|k| results_text.contains(&k.to_lowercase())).count();
    found as f64 / keywords.len() as f64
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Intent Detection Regression ──

    #[test]
    fn eval_intent_detection_all_correct() {
        let queries = eval_queries();
        let mut passed = 0;
        let mut failed = Vec::new();

        for eq in &queries {
            let detected = detect_intent(eq.query);
            if detected == eq.expected_intent {
                passed += 1;
            } else {
                failed.push(format!(
                    "'{}' expected {:?}, got {:?}",
                    eq.query, eq.expected_intent, detected
                ));
            }
        }

        if !failed.is_empty() {
            eprintln!("Intent detection failures:");
            for f in &failed {
                eprintln!("  {}", f);
            }
        }
        assert!(failed.is_empty(), "{} intent detection failures", failed.len());
        eprintln!("Intent detection: {}/{} passed", passed, queries.len());
    }

    // ── Metric Unit Tests ──

    #[test]
    fn recall_at_k_perfect() {
        let retrieved = vec!["a.rs", "b.rs", "c.rs"];
        let relevant = vec!["a.rs", "b.rs"];
        assert!((recall_at_k(&retrieved, &relevant, 3) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn recall_at_k_partial() {
        let retrieved = vec!["a.rs", "x.rs", "y.rs"];
        let relevant = vec!["a.rs", "b.rs"];
        assert!((recall_at_k(&retrieved, &relevant, 3) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn recall_at_k_none() {
        let retrieved = vec!["x.rs", "y.rs"];
        let relevant = vec!["a.rs", "b.rs"];
        assert!((recall_at_k(&retrieved, &relevant, 2) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn recall_at_k_empty_relevant() {
        let retrieved = vec!["a.rs"];
        let relevant: Vec<&str> = vec![];
        assert!((recall_at_k(&retrieved, &relevant, 1) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn mrr_first_rank() {
        let retrieved = vec!["a.rs", "b.rs"];
        let relevant = vec!["a.rs"];
        assert!((mrr(&retrieved, &relevant) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn mrr_second_rank() {
        let retrieved = vec!["x.rs", "a.rs"];
        let relevant = vec!["a.rs"];
        assert!((mrr(&retrieved, &relevant) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn mrr_not_found() {
        let retrieved = vec!["x.rs", "y.rs"];
        let relevant = vec!["a.rs"];
        assert!((mrr(&retrieved, &relevant) - 0.0).abs() < 1e-6);
    }

    #[test]
    fn keyword_coverage_full() {
        let results = vec!["src/auth.rs", "src/middleware.rs"];
        let keywords = vec!["auth", "middleware"];
        assert!((keyword_coverage(&results, &keywords) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn keyword_coverage_partial() {
        let results = vec!["src/auth.rs"];
        let keywords = vec!["auth", "middleware"];
        assert!((keyword_coverage(&results, &keywords) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn keyword_coverage_empty_keywords() {
        let results = vec!["anything"];
        let keywords: Vec<&str> = vec![];
        assert!((keyword_coverage(&results, &keywords) - 1.0).abs() < 1e-6);
    }

    // ── Policy Consistency ──

    #[test]
    fn policy_budgets_are_set() {
        let p = RetrievalPolicy::default();
        assert!(p.lexical_budget_ms > 0);
        assert!(p.structural_budget_ms > 0);
        assert!(p.total_budget_ms > 0);
    }

    // ── Regression: eval queries are well-formed ──

    #[test]
    fn eval_queries_have_relevant_files() {
        for eq in eval_queries() {
            assert!(!eq.relevant_files.is_empty(), "query '{}' has no relevant files", eq.query);
            assert!(!eq.expected_keywords.is_empty(), "query '{}' has no expected keywords", eq.query);
        }
    }

    #[test]
    fn eval_queries_intent_matches_structural_detection() {
        for eq in eval_queries() {
            if eq.expected_intent == QueryIntent::Structural {
                // Structural queries should be detectable by parse_structural_query
                let pattern = crate::retrieval::structural::parse_structural_query(eq.query);
                assert!(pattern.is_some(), "structural query '{}' should be detected", eq.query);
            }
        }
    }
}
