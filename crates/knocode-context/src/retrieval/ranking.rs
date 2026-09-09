//! Deterministic ranking — all scoring logic lives here.
//! Mirrors the ad-hoc steps in `lib.rs:268-393` + `tantivy_index.rs:86-130` + `647-652`.

use std::collections::{HashMap, HashSet};
use std::sync::LazyLock;

use crate::retrieval::evidence::{Evidence, EvidenceSource, RetrievalSignal};
use crate::retrieval::policy::RetrievalPolicy;

/// Stop words for symbol-match boosting — same list as `lib.rs:12-23`.
/// FIX #8: Use HashSet for O(1) lookup instead of O(n) slice scan.
static STOP_WORDS: LazyLock<HashSet<&'static str>> = LazyLock::new(|| {
    [
        "a", "an", "the", "is", "are", "was", "were", "be", "been", "being",
        "have", "has", "had", "do", "does", "did", "will", "would", "could",
        "should", "may", "might", "shall", "can", "to", "of", "in", "for",
        "on", "with", "at", "by", "from", "as", "into", "through", "during",
        "before", "after", "above", "below", "between", "and", "but", "or",
        "nor", "not", "so", "yet", "both", "either", "neither", "each",
        "every", "all", "any", "few", "more", "most", "other", "some",
        "such", "no", "only", "own", "same", "than", "too", "very",
        "just", "because", "if", "when", "where", "how", "what", "which",
        "who", "whom", "this", "that", "these", "those",
    ].into_iter().collect()
});

/// Extract query tokens for symbol-match boosting — mirrors `lib.rs:269-296`.
/// FIX #5: Reduced allocations by reusing buffers and avoiding unnecessary clones.
pub fn query_tokens(query: &str) -> Vec<String> {
    let mut result: Vec<String> = Vec::with_capacity(16);
    let mut seen: HashSet<String> = HashSet::with_capacity(16);
    let mut current = String::with_capacity(32);

    for t in query.split_whitespace() {
        current.clear();
        for c in t.chars() {
            if c.is_alphanumeric() || c == '_' {
                current.push(c.to_ascii_lowercase());
            }
        }
        if current.len() < 2 || STOP_WORDS.contains(current.as_str()) {
            continue;
        }
        // Add the full word
        if seen.insert(current.clone()) {
            result.push(current.clone());
        }
        // Split PascalCase: "UserProfile" → ["user", "profile"]
        let mut part_start = 0;
        for (i, c) in current.char_indices() {
            if i > 0 && c.is_uppercase() {
                let part = &current[part_start..i];
                if part.len() >= 2 && !STOP_WORDS.contains(part) && seen.insert(part.to_string()) {
                    result.push(part.to_string());
                }
                part_start = i;
            }
        }
        let last_part = &current[part_start..];
        if last_part != current.as_str() && last_part.len() >= 2 && !STOP_WORDS.contains(last_part) && seen.insert(last_part.to_string()) {
            result.push(last_part.to_string());
        }
    }
    result
}

/// Apply symbol-match boost to a candidate (content+path haystack).
/// Returns `(boost_factor, matched_count)` — mirrors `lib.rs:298-315`.
pub fn symbol_boost(query_tokens: &[String], path: &str, content: &str, policy: &RetrievalPolicy) -> (f32, usize) {
    if query_tokens.is_empty() {
        return (1.0, 0);
    }
    let haystack = format!("{} {}", content, path).to_lowercase();
    let matched = query_tokens.iter().filter(|t| haystack.contains(t.as_str())).count();
    if matched == 0 {
        return (1.0, 0);
    }
    let boost = 1.0 + (matched as f32 / query_tokens.len() as f32) * policy.symbol_match_weight;
    (boost, matched)
}

/// Apply file-class + directory + test-aware boosts to a single score.
/// Extracted from `tantivy_index.rs:742-746` + `86-130`.
pub fn apply_class_and_dir_boost(
    base_score: f32,
    path: &str,
    file_class: &str,
    query: &str,
    policy: &RetrievalPolicy,
    signals: &mut Vec<RetrievalSignal>,
) -> f32 {
    apply_class_and_dir_boost_with_query_tokens(
        base_score,
        path,
        file_class,
        &query_tokens(query),
        policy,
        signals,
    )
}

/// P0 eval-quality fix: documentation damping for top-of-list monopolization.
///
/// Generic meta-docs (README.md, ROADMAP.md, CHANGELOG.md, docs/*.md) match
/// every query through their CONTENT, then stack compounding priors — file
/// class ×1.4-2.5, directory ×1.2-1.3, authority ×1.5. On the golden 50-task
/// eval this put a .md file at rank 1 for 47/50 tasks, pushing the actual code
/// target to rank ~3 and capping MRR at ~0.28.
///
/// A doc file whose PATH shares no token with the query matched on content
/// generality, not on-topic evidence. The engine multiplies such docs' FULL
/// final score by `policy.doc_prior_damping` (see engine.rs). Code/Config/Test
/// files are never damped; a path-token match (query "indexing" vs
/// INDEXING_PERF_PLAN.md) keeps the full score — topical docs still win.

/// True when this file is documentation-class AND its path contains none of
/// the query tokens (i.e. it is a generic meta-doc for this query).
pub fn is_generic_doc(path: &str, file_class: &str, query_tokens: &[String]) -> bool {
    if file_class != "Documentation" || query_tokens.is_empty() {
        return false;
    }
    let path_lower = path.to_lowercase();
    !query_tokens.iter().any(|t| path_lower.contains(t.as_str()))
}

pub fn apply_class_and_dir_boost_with_query_tokens(
    base_score: f32,
    path: &str,
    file_class: &str,
    query_tokens: &[String],
    policy: &RetrievalPolicy,
    signals: &mut Vec<RetrievalSignal>,
) -> f32 {
    let class_boost = policy.file_class_weights.boost_for(file_class);
    if (class_boost - 1.0).abs() > f32::EPSILON {
        signals.push(RetrievalSignal::FileClassBoost { class: file_class.to_string(), boost: class_boost });
    }
    // Query-aware test multiplier from pre-tokenized query (mirrors
    // `test_multiplier`'s substring check on the lowercased query).
    let is_test_query = query_tokens.iter().any(|t| t.contains("test") || t.contains("spec") || t.contains("dtslint"));
    let test_mult = if file_class == "Test" {
        if is_test_query { policy.test_boost } else { policy.test_penalty }
    } else {
        1.0
    };
    if (test_mult - 1.0).abs() > f32::EPSILON {
        if test_mult < 1.0 {
            signals.push(RetrievalSignal::TestPenalty(test_mult));
        } else {
            signals.push(RetrievalSignal::TestBoost(test_mult));
        }
    }
    let dir_boost = policy.directory_weights.boost_for(path);
    if (dir_boost - 1.0).abs() > f32::EPSILON {
        signals.push(RetrievalSignal::DirectoryBoost(dir_boost));
    }
    let combined = class_boost * test_mult * dir_boost;

    base_score * combined
}

/// Merge BM25 + symbol results: keep max score per path.
/// Mirrors `lib.rs:318-329`.
pub fn merge_by_path(
    bm25: Vec<(String, f64, String, EvidenceSource)>,
    symbols: Vec<(String, f64, String, EvidenceSource)>,
) -> HashMap<String, (f64, String, EvidenceSource)> {
    let mut map: HashMap<String, (f64, String, EvidenceSource)> = HashMap::new();
    for (path, score, file_class, source) in bm25.into_iter().chain(symbols) {
        let entry = map.entry(path.clone()).or_insert((score, file_class.clone(), source.clone()));
        if score > entry.0 {
            *entry = (score, file_class, source);
        }
    }
    map
}

/// Add code-behind pairs (`.cshtml → .cshtml.cs`, `.razor → .razor.cs`).
/// Mirrors `lib.rs:331-355`.
/// FIX #3: O(n) with HashSet instead of O(n²) with repeated iter().any().
pub fn add_code_behind(
    merged: &mut Vec<(String, f64)>,
    policy: &RetrievalPolicy,
) -> Vec<RetrievalSignal> {
    // Build set of existing paths for O(1) lookup
    let existing: HashSet<&String> = merged.iter().map(|(p, _)| p).collect();

    let pairs: Vec<(String, String)> = merged.iter().filter_map(|(path, _)| {
        if path.ends_with(".cshtml") || path.ends_with(".razor") {
            Some((path.clone(), format!("{}.cs", path)))
        } else {
            None
        }
    }).collect();

    let mut signals = Vec::new();
    let mut additions: Vec<(String, f64)> = Vec::new();
    for (view_path, code_behind_path) in pairs {
        if existing.contains(&view_path) && !existing.contains(&code_behind_path) {
            if let Some(view_score) = merged.iter().find(|(p, _)| *p == view_path).map(|(_, s)| *s) {
                let new_score = view_score * policy.code_behind_multiplier as f64;
                additions.push((code_behind_path, new_score));
                signals.push(RetrievalSignal::CodeBehindPenalty(policy.code_behind_multiplier));
            }
        }
    }
    merged.extend(additions);
    signals
}

/// Graph boost: 20% for files connected to top-3 scoring files.
/// Mirrors `lib.rs:357-393`.
pub fn apply_graph_boost(
    merged: &mut [(String, f64)],
    graph: &knocode_repo_intel::graph::DependencyGraph,
    policy: &RetrievalPolicy,
) -> Vec<(String, RetrievalSignal)> {
    if merged.len() < 2 {
        return Vec::new();
    }
    let high: HashSet<String> = merged.iter().take(3).map(|(p, _)| p.clone()).collect();
    let mut boosted = Vec::new();
    for (path, score) in merged.iter_mut() {
        if high.contains(path) {
            continue;
        }
        let deps = graph.dependencies_of(path);
        let dependents = graph.dependents_of(path);
        let connected = deps.iter().any(|d| high.contains(d)) || dependents.iter().any(|d| high.contains(d));
        if connected {
            *score *= policy.graph_multiplier as f64;
            boosted.push((path.clone(), RetrievalSignal::GraphBoost(policy.graph_multiplier)));
        }
    }
    boosted
}

/// P1.5: Extract the title from markdown content.
/// Looks for the first `# Title` line and returns it.
pub fn extract_markdown_title(content: &str) -> Option<String> {
    for line in content.lines() {
        let trimmed = line.trim();
        if let Some(title) = trimmed.strip_prefix("# ") {
            let title = title.trim();
            if !title.is_empty() {
                return Some(title.to_string());
            }
        }
    }
    None
}

/// P1.5: Compute title-match score between a markdown title and query tokens.
/// Returns a boost factor: 1.0 for no match, up to 2.0 for strong match.
pub fn title_match_boost(title: &str, query_tokens: &[String]) -> f32 {
    if query_tokens.is_empty() || title.is_empty() {
        return 1.0;
    }
    let title_lower = title.to_lowercase();
    let matched = query_tokens.iter().filter(|t| title_lower.contains(t.as_str())).count();
    if matched == 0 {
        return 1.0;
    }
    let ratio = matched as f32 / query_tokens.len() as f32;
    1.0 + ratio // 1.0 to 2.0
}

/// Build ranked `Evidence` from merged scores + metadata.
/// Centralizes scoring so `ContextEngine` stops knowing how relevance is calculated.
pub fn build_evidence_from_merged(
    merged: Vec<(String, f64)>,
    by_path: &HashMap<String, (f64, String, EvidenceSource)>,
    signals_by_path: &HashMap<String, Vec<RetrievalSignal>>,
) -> Vec<Evidence> {
    let mut out = Vec::new();
    for (path, score) in merged {
        if let Some((_, file_class, source)) = by_path.get(&path) {
            let mut ev = Evidence::new(path.clone(), score as f32 * 1000.0, file_class.clone());
            ev.raw_score = score as f32;
            ev.source = source.clone();
            if let Some(sigs) = signals_by_path.get(&path) {
                ev.signals = sigs.clone();
            }
            out.push(ev);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::retrieval::policy::RetrievalPolicy;

    #[test]
    fn query_tokens_filters_stop_words() {
        let toks = query_tokens("the authentication middleware for the user");
        assert!(!toks.contains(&"the".to_string()));
        assert!(toks.contains(&"authentication".to_string()));
    }

    #[test]
    fn symbol_boost_scales_with_coverage() {
        let p = RetrievalPolicy::default();
        let toks = vec!["auth".to_string(), "middleware".to_string()];
        let (boost, matched) = symbol_boost(&toks, "src/auth/middleware.rs", "auth middleware", &p);
        assert_eq!(matched, 2);
        assert!((boost - (1.0 + 1.0 * 1.5)).abs() < 1e-6);
    }

    // ── P1.5: Markdown title extraction tests ──

    #[test]
    fn extract_markdown_title_basic() {
        let content = "# My Package\n\nThis is the readme.";
        assert_eq!(extract_markdown_title(content), Some("My Package".to_string()));
    }

    #[test]
    fn extract_markdown_title_with_whitespace() {
        let content = "  #   Spaced Title  \n\nBody";
        assert_eq!(extract_markdown_title(content), Some("Spaced Title".to_string()));
    }

    #[test]
    fn extract_markdown_title_no_title() {
        let content = "This is just plain text without a heading.";
        assert_eq!(extract_markdown_title(content), None);
    }

    #[test]
    fn extract_markdown_title_skips_h2() {
        let content = "## Not the title\n# Real Title";
        assert_eq!(extract_markdown_title(content), Some("Real Title".to_string()));
    }

    #[test]
    fn extract_markdown_title_empty() {
        assert_eq!(extract_markdown_title(""), None);
    }

    #[test]
    fn title_match_boost_no_tokens() {
        assert!((title_match_boost("My Package", &[]) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn title_match_boost_full_match() {
        let tokens = vec!["package".to_string()];
        let boost = title_match_boost("My Package", &tokens);
        assert!(boost > 1.0, "full match should boost: {}", boost);
        assert!(boost <= 2.0);
    }

    #[test]
    fn title_match_boost_partial_match() {
        let tokens = vec!["add".to_string(), "package".to_string()];
        let boost = title_match_boost("Adding a Package", &tokens);
        // "package" matches, "add" matches (in "adding")
        assert!(boost > 1.0, "partial match should boost: {}", boost);
    }

    #[test]
    fn title_match_boost_no_match() {
        let tokens = vec!["auth".to_string(), "middleware".to_string()];
        let boost = title_match_boost("My Package", &tokens);
        assert!((boost - 1.0).abs() < 1e-6, "no match should not boost: {}", boost);
    }

    // ── P1.5: Canonical README detection tests ──

    #[test]
    fn canonical_readme_prefers_root() {
        use crate::retrieval::policy::DocumentationAuthority;
        let paths = vec!["README.ja.md", "README.md", "docs/README.md"];
        let canonical = DocumentationAuthority::canonical_readme_path(&paths);
        assert_eq!(canonical, Some("README.md"));
    }

    #[test]
    fn canonical_readme_no_root() {
        use crate::retrieval::policy::DocumentationAuthority;
        let paths = vec!["docs/guide.md", "README.ja.md"];
        let canonical = DocumentationAuthority::canonical_readme_path(&paths);
        assert_eq!(canonical, Some("docs/guide.md"));
    }

    #[test]
    fn canonical_readme_empty() {
        use crate::retrieval::policy::DocumentationAuthority;
        assert!(DocumentationAuthority::canonical_readme_path(&[]).is_none());
    }
}
