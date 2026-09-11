//! RetrievalPlan — decides which retrievers participate and with what weight.
//!
//! Intent selects **retrieval capabilities**, not exactly one retriever.
//! ```text
//! STRUCTURAL  → Tantivy (candidates) + Structural (pattern match)
//! PROCEDURAL  → Tantivy/docs (primary), structural optional
//! DEBUGGING   → Tantivy + symbols + structural
//! IMPLEMENTATION → Tantivy + structural enrichment
//! ALL OTHERS  → Tantivy (primary)
//! ```
//!
//! This avoids the "everything accumulates inside build_context()" problem
//! by making the plan explicit and testable.

use crate::retrieval::intent::QueryIntent;
use crate::retrieval::structural_plan::StructuralIntent;

/// Which retrieval backends participate and their relative weights.
///
/// The plan is **explicit and inspectable** — every decision is recorded
/// as a field, not hidden inside if-else chains.
#[derive(Debug, Clone)]
pub struct RetrievalPlan {
    /// Run Tantivy BM25 lexical retrieval.
    pub lexical: bool,
    /// Weight of lexical evidence in final merge (0.0–1.0).
    pub lexical_weight: f32,
    /// Run structural retrieval (tree-sitter pattern matching).
    pub structural: bool,
    /// Weight of structural evidence in final merge (0.0–1.0).
    pub structural_weight: f32,
    /// Run symbol search (currently inside TantivyRetriever).
    pub symbols: bool,
    /// Weight of symbol evidence.
    pub symbols_weight: f32,
    /// Run graph boost (dependency edges).
    pub graph: bool,

    // ── P2: Explicit structural plan fields ──
    /// Resolved structural pattern (if any). Either an explicit ast-grep pattern
    /// or an inferred pattern from natural language.
    pub structural_pattern: Option<String>,
    /// Structural intent that produced the pattern.
    pub structural_intent: Option<StructuralIntent>,
    /// Target languages for structural search (from query or file context).
    pub languages: Vec<String>,
    /// Maximum candidate files for structural search.
    pub candidate_limit: usize,
}

impl Default for RetrievalPlan {
    fn default() -> Self {
        Self {
            lexical: true,
            lexical_weight: 1.0,
            structural: false,
            structural_weight: 0.0,
            symbols: true,
            symbols_weight: 1.0,
            graph: false,
            structural_pattern: None,
            structural_intent: None,
            languages: Vec::new(),
            candidate_limit: 200,
        }
    }
}

impl RetrievalPlan {
    /// Build a plan from query intent and query text analysis.
    /// Intent is a **policy input**, not a hard router.
    pub fn from_intent(intent: QueryIntent, has_structural_pattern: bool) -> Self {
        match intent {
            QueryIntent::Structural if has_structural_pattern => {
                // Structural queries: pattern match is primary, lexical provides candidates
                Self {
                    lexical: true,
                    lexical_weight: 0.3,
                    structural: true,
                    structural_weight: 1.0,
                    symbols: true,
                    symbols_weight: 0.5,
                    graph: false,
                    ..Self::default()
                }
            }
            QueryIntent::Implementation | QueryIntent::Debugging => {
                // Code queries: lexical primary, structural enriches with AST evidence
                Self {
                    lexical: true,
                    lexical_weight: 1.0,
                    structural: true,
                    structural_weight: 0.4,
                    symbols: true,
                    symbols_weight: 1.0,
                    graph: intent == QueryIntent::Debugging,
                    ..Self::default()
                }
            }
            QueryIntent::Testing => {
                // Test queries: lexical primary, structural can help find test patterns
                Self {
                    lexical: true,
                    lexical_weight: 1.0,
                    structural: false,
                    structural_weight: 0.0,
                    symbols: true,
                    symbols_weight: 0.8,
                    graph: false,
                    ..Self::default()
                }
            }
            QueryIntent::Procedural | QueryIntent::Configuration => {
                // Documentation/procedural: lexical only (README, docs)
                Self {
                    lexical: true,
                    lexical_weight: 1.0,
                    structural: false,
                    structural_weight: 0.0,
                    symbols: false,
                    symbols_weight: 0.0,
                    graph: false,
                    ..Self::default()
                }
            }
            _ => {
                // Default: lexical primary only
                Self::default()
            }
        }
    }

    /// Whether any retrieval should happen at all.
    pub fn has_any_retrieval(&self) -> bool {
        self.lexical || self.structural
    }

    /// Maximum evidence count across all participating backends.
    pub fn max_evidence(&self, base_max: usize) -> usize {
        let mut count = 0;
        if self.lexical { count += base_max; }
        if self.structural { count += base_max / 2; }
        if count == 0 { base_max } else { count }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structural_pattern_plan_has_both() {
        let plan = RetrievalPlan::from_intent(QueryIntent::Structural, true);
        assert!(plan.lexical);
        assert!(plan.structural);
        assert!(plan.structural_weight > plan.lexical_weight);
    }

    #[test]
    fn procedural_plan_no_structural() {
        let plan = RetrievalPlan::from_intent(QueryIntent::Procedural, false);
        assert!(plan.lexical);
        assert!(!plan.structural);
    }

    #[test]
    fn debugging_plan_has_structural_enrichment() {
        let plan = RetrievalPlan::from_intent(QueryIntent::Debugging, false);
        assert!(plan.lexical);
        assert!(plan.structural);
        assert!(plan.graph);
    }

    #[test]
    fn implementation_plan_has_structural() {
        let plan = RetrievalPlan::from_intent(QueryIntent::Implementation, false);
        assert!(plan.structural);
        assert!(plan.structural_weight < 1.0); // enrichment, not primary
    }

    #[test]
    fn testing_plan_no_structural() {
        let plan = RetrievalPlan::from_intent(QueryIntent::Testing, false);
        assert!(!plan.structural);
    }

    #[test]
    fn informational_plan_default() {
        let plan = RetrievalPlan::from_intent(QueryIntent::Informational, false);
        assert!(plan.lexical);
        assert!(!plan.structural);
    }

    #[test]
    fn max_evidence_scales_with_participants() {
        let plan_all = RetrievalPlan {
            lexical: true,
            structural: true,
            ..Default::default()
        };
        let plan_lex = RetrievalPlan {
            lexical: true,
            structural: false,
            ..Default::default()
        };
        assert!(plan_all.max_evidence(20) > plan_lex.max_evidence(20));
    }
}
