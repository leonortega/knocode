//! Bounded deterministic vocabulary — fixes lexical mismatch without giant OR.
//! Example: `add package` ↔ `create package` for `README.md: #### Create a new package`.
//!
//! The synonym table lives in `knocode_core::ranking::synonyms_for` (shared
//! with storage-side query sanitization — formerly two drifted hardcoded
//! match arms). Expansion is WEIGHTED: the un-expanded query runs at full
//! weight and the expanded query at `EXPANSION_WEIGHT` (0.5), so a synonym
//! hit can lift a file into the pool but can never outrank an original-term
//! hit — see `weighted_combined_query`.

use std::collections::HashSet;

/// Weight of the expanded (synonym) branch relative to the original query.
/// Original terms stay at 1.0 — expansion broadens recall without letting
/// synonyms dilute exact matches.
pub const EXPANSION_WEIGHT: f32 = 0.5;

/// Expand a single term into bounded synonyms (including itself) using the
/// canonical table. Lookup is stem-canonicalized ("Packages" → "package").
pub fn synonyms_for(term: &str) -> Vec<String> {
    let canonical = knocode_core::ranking::canonicalize_term(term);
    knocode_core::ranking::synonyms_for(&canonical)
        .iter()
        .map(|s| s.to_string())
        .collect()
}

/// Expand query terms with bounded synonyms.
/// `tokens` are already lowercased, stop-word filtered.
/// Returns expanded set (original + synonyms) deduped.
pub fn expand_terms(tokens: &[String]) -> Vec<String> {
    let mut out: HashSet<String> = HashSet::new();
    for t in tokens {
        out.insert(t.clone());
        for syn in synonyms_for(t) {
            if syn.len() >= 2 {
                out.insert(syn);
            }
        }
    }
    // Keep bounded: at most original 2x + synonyms, but cap 20 terms
    let mut v: Vec<String> = out.into_iter().collect();
    v.sort();
    v.truncate(20);
    v
}

/// Build an OR-joined expanded query string for Tantivy.
/// Uses original query + synonym expansion, but keeps it bounded (not huge OR).
///
/// OR-join ownership note (verified): returning "a OR b" here is safe even
/// though `sanitize_code_query` re-joins terms with OR — the sanitizer tokenizes
/// on whitespace and drops "or" as a stop word (`knocode_core::ranking::STOP_WORDS`),
/// so the ORs we emit are stripped and the term set is re-joined exactly once.
pub fn expanded_query_string(original: &str, tokens: &[String]) -> String {
    if tokens.is_empty() {
        return original.to_string();
    }
    let expanded = expand_terms(tokens);
    if expanded.len() <= tokens.len() {
        original.to_string()
    } else {
        expanded.join(" OR ")
    }
}

/// Weighted two-branch query: original terms at full weight, expansion at
/// `EXPANSION_WEIGHT`. Run both branches through retrieval and combine per
/// file as `max(original_score, EXPANSION_WEIGHT * expanded_score)` — max-combine
/// (not sum) preserves BM25 comparability while guaranteeing an original-term
/// hit always outranks a synonym-only hit of the same base score.
///
/// Returns `(original, expanded)` — `expanded` is `None` when expansion adds
/// nothing (then the caller runs the original query only).
pub fn weighted_query_pair(original: &str, tokens: &[String]) -> (String, Option<String>) {
    if tokens.is_empty() {
        return (original.to_string(), None);
    }
    let expanded = expanded_query_string(original, tokens);
    if expanded == original {
        (original.to_string(), None)
    } else {
        (original.to_string(), Some(expanded))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_package_expands_to_create_new() {
        let toks = vec!["add".to_string(), "package".to_string()];
        let exp = expand_terms(&toks);
        assert!(exp.contains(&"add".to_string()));
        assert!(exp.contains(&"create".to_string()));
        assert!(exp.contains(&"new".to_string()));
        assert!(exp.contains(&"package".to_string()));
        assert!(exp.contains(&"workspace".to_string()));
    }

    #[test]
    fn bounded_no_explosion() {
        let toks = vec!["how".to_string(), "do".to_string(), "i".to_string(), "add".to_string(), "package".to_string()];
        let exp = expand_terms(&toks);
        assert!(exp.len() <= 20);
    }

    #[test]
    fn non_synonym_unchanged() {
        // "authentication" DOES expand (auth ↔ authentication is in the table);
        // use a term with no table entry to verify pass-through.
        let toks = vec!["pagination".to_string()];
        let exp = expand_terms(&toks);
        assert_eq!(exp, vec!["pagination".to_string()]);
    }

    #[test]
    fn authentication_expands_to_auth() {
        let toks = vec!["authentication".to_string()];
        let exp = expand_terms(&toks);
        assert!(exp.contains(&"auth".to_string()));
    }

    #[test]
    fn plural_hits_table_via_stem() {
        // "packages" (plural) must reach the "package" table entry
        let syn = synonyms_for("packages");
        assert!(syn.contains(&"package".to_string()), "plural stem should hit table: {:?}", syn);
        assert!(syn.contains(&"workspace".to_string()));
    }

    #[test]
    fn table_covers_common_engineering_terms() {
        // previously the table had only 8 entries; these were added in the shared table
        for term in ["auth", "config", "db", "error", "test", "deploy", "install"] {
            assert!(!synonyms_for(term).is_empty(), "table missing term '{term}'");
        }
    }

    #[test]
    fn weighted_pair_originals_stay_full_weight() {
        let toks = vec!["add".to_string(), "package".to_string()];
        let (orig, expanded) = weighted_query_pair("add package", &toks);
        assert_eq!(orig, "add package");
        let e = expanded.expect("expansion expected for 'add package'");
        assert!(e.contains("create"));
        // expansion weight is exported and < 1.0 by design
        assert!(EXPANSION_WEIGHT < 1.0 && EXPANSION_WEIGHT > 0.0);
    }

    #[test]
    fn weighted_pair_none_when_no_expansion() {
        let toks = vec!["pagination".to_string()];
        let (orig, expanded) = weighted_query_pair("pagination", &toks);
        assert_eq!(orig, "pagination");
        assert!(expanded.is_none());
    }
}
