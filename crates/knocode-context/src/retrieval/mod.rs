//! Retrieval Engine boundary.
//! Invariants:
//! - Retrieval determines relevance; Context determines inclusion.
//! - Repository evidence, project knowledge, and procedural skills are different evidence types.
//! - Retrieval must remain deterministic and independently testable without an LLM.

pub mod evidence;
pub mod intent;
pub mod vocab;
pub mod policy;
pub mod plan;
pub mod query;
pub mod ranking;
pub mod engine;
pub mod structural;
pub mod structural_plan;
pub mod benchmark;
#[cfg(test)]
pub mod bench_retrieval;
#[cfg(test)]
pub mod bench_dt;
#[cfg(test)]
pub mod bench_mattermost;
#[cfg(test)]
pub mod bench_components;
#[cfg(test)]
pub mod bench_eshop;
#[cfg(test)]
mod integration_tests;

pub use evidence::{Evidence, EvidenceKind, EvidenceSource, RetrievalDiagnostics, RetrievalResult, RetrievalSignal};
pub use intent::{QueryIntent, detect_intent};
pub use policy::{DirectoryWeights, DocumentationAuthority, FieldWeights, FileClassWeights, RetrievalPolicy};
pub use plan::RetrievalPlan;
pub use query::RetrievalQuery;
pub use engine::{Retriever, TantivyRetriever, CombinedRetriever};
pub use structural::{StructuralRetriever, StructuralPattern, parse_structural_query};
pub use structural_plan::{StructuralIntent, StructuralQuery, QueryPlanner, ResolvedPattern};
pub use vocab::{expand_terms, expanded_query_string};
