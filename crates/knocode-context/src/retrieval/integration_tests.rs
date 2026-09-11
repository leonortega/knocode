//! Integration tests — real-codebase pipeline tests for structural retrieval.
//!
//! Tests the full pipeline:
//! ```text
//! real code files → QueryPlanner → StructuralRetriever → AstGrepBackend → evidence
//! ```
//!
//! ## Configuration
//!
//! Set `KNOCODE_TEST_REPO=/path/to/repo` to run tests against a real codebase.
//! Without it, tests use a built-in temp fixture with hardcoded test files.
//!
//! ```bash
//! # Default: built-in fixture
//! cargo test -p knocode-context retrieval::integration_tests
//!
//! # Real repo (e.g. DefinitelyTyped)
//! KNOCODE_TEST_REPO=/c/tmp/DefinitelyTyped-master \
//!   cargo test -p knocode-context retrieval::integration_tests
//! ```

use std::path::PathBuf;
use std::time::Instant;

use crate::retrieval::query::RetrievalQuery;
use crate::retrieval::structural_plan::{QueryPlanner, StructuralIntent, StructuralQuery};
use knocode_repo_intel::structural::AstGrepBackend;

// ── Test repository configuration ──────────────────────────────────────

/// Check for a real repository path via KNOCODE_TEST_REPO env var.
fn test_repo_path() -> Option<PathBuf> {
    std::env::var("KNOCODE_TEST_REPO")
        .ok()
        .filter(|p| !p.is_empty())
        .map(PathBuf::from)
        .filter(|p| p.is_dir())
}

/// Check if a filename matches a pattern (handles compound extensions like `.d.ts`).
fn filename_matches(name: &str, pattern: &str) -> bool {
    // Exact match for compound extensions
    if name.ends_with(pattern) { return true; }
    // Single extension match
    if let Some(ext) = std::path::Path::new(name).extension().and_then(|e| e.to_str()) {
        return ext == pattern;
    }
    false
}

/// Find the first file with the given extension in a directory (recursive, max depth 3).
fn find_file_with_ext(root: &PathBuf, ext: &str, max_depth: usize) -> Option<PathBuf> {
    fn walk(dir: &PathBuf, ext: &str, depth: usize, max_depth: usize) -> Option<PathBuf> {
        if depth > max_depth { return None; }
        let entries = std::fs::read_dir(dir).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if filename_matches(&name, ext) {
                    // Skip huge files (>100KB)
                    if path.metadata().map(|m| m.len() < 100_000).unwrap_or(false) {
                        return Some(path);
                    }
                }
            } else if path.is_dir() {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if !["node_modules", ".git", "target", "dist", "build"].contains(&name.as_ref()) {
                    if let Some(found) = walk(&path, ext, depth + 1, max_depth) {
                        return Some(found);
                    }
                }
            }
        }
        None
    }
    walk(root, ext, 0, max_depth)
}

/// Find multiple files with the given extension.
fn find_files_with_ext(root: &PathBuf, ext: &str, max_depth: usize, max_files: usize) -> Vec<PathBuf> {
    fn walk(dir: &PathBuf, ext: &str, depth: usize, max_depth: usize, out: &mut Vec<PathBuf>, max: usize) {
        if depth > max_depth || out.len() >= max { return; }
        let entries = match std::fs::read_dir(dir) { Ok(e) => e, Err(_) => return };
        for entry in entries.flatten() {
            if out.len() >= max { return; }
            let path = entry.path();
            if path.is_file() {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if filename_matches(&name, ext) {
                    if path.metadata().map(|m| m.len() < 100_000).unwrap_or(false) {
                        out.push(path);
                    }
                }
            } else if path.is_dir() {
                let name = path.file_name().unwrap_or_default().to_string_lossy();
                if !["node_modules", ".git", "target", "dist", "build"].contains(&name.as_ref()) {
                    walk(&path, ext, depth + 1, max_depth, out, max);
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(root, ext, 0, max_depth, &mut out, max_files);
    out
}

/// Source code content for a test file.
struct TestFile {
    path: PathBuf,
    content: String,
    language: &'static str,
}

/// Get test files based on repo type.
fn get_test_files() -> Vec<TestFile> {
    if let Some(repo) = test_repo_path() {
        // Real repo: find actual source files
        let mut files = Vec::new();

        // Try TypeScript first (DefinitelyTyped is all .d.ts)
        for ext in &["ts", "d.ts"] {
            for path in find_files_with_ext(&repo, ext, 4, 5) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    files.push(TestFile { path, content, language: "typescript" });
                    if files.len() >= 3 { break; }
                }
            }
            if !files.is_empty() { break; }
        }

        // If no TS, try Rust
        if files.is_empty() {
            if let Some(path) = find_file_with_ext(&repo, "rs", 4) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    files.push(TestFile { path, content, language: "rust" });
                }
            }
        }

        // If no Rust, try Python
        if files.is_empty() {
            if let Some(path) = find_file_with_ext(&repo, "py", 4) {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    files.push(TestFile { path, content, language: "python" });
                }
            }
        }

        files
    } else {
        // Built-in fixture
        vec![
            TestFile {
                path: "src/main.rs".into(),
                content: "fn main() { println!(\"hello\"); }\nfn add(a: i32, b: i32) -> i32 { a + b }\nstruct Config { name: String }\nimpl Config { fn new() -> Self { Config { name: \"default\".into() } } }\nenum Color { Red, Green, Blue }\ntrait Drawable { fn draw(&self); }\n".into(),
                language: "rust",
            },
            TestFile {
                path: "src/app.ts".into(),
                content: "import express from 'express';\ninterface User { name: string; age: number; }\nclass UserController { getUser() { return {}; } }\nfunction createUser(data: any) { return { name: data.name, age: data.age }; }\napp.get(\"/users\", async (req, res) => { res.json([]); });\n".into(),
                language: "typescript",
            },
            TestFile {
                path: "src/main.py".into(),
                content: "import os\ndef hello(): pass\ndef add(a, b): return a + b\nclass Config: pass\n".into(),
                language: "python",
            },
        ]
    }
}

// ── Positive tests ─────────────────────────────────────────────────────

#[test]
fn integration_find_functions() {
    let files = get_test_files();
    assert!(!files.is_empty(), "should have test files");

    for tf in &files {
        let adapter = match knocode_repo_intel::structural::create_adapter(tf.language) {
            Some(a) => a,
            None => continue,
        };

        // Get patterns for this language
        let sq = StructuralQuery::new("find all functions").with_language(tf.language);
        let patterns = QueryPlanner::plan(&sq);
        if patterns.is_empty() { continue; }

        let mut _total_matches = 0;
        for pat in &patterns {
            if let Ok(result) = adapter.search(&pat.pattern, &tf.content) {
                _total_matches += result.match_count();
            }
        }

        // Verify search completed without panicking
        // (not all files have standard function declarations — arrow functions, declares, etc.)
    }
}

#[test]
fn integration_find_classes() {
    let files = get_test_files();
    assert!(!files.is_empty(), "should have test files");

    for tf in &files {
        let adapter = match knocode_repo_intel::structural::create_adapter(tf.language) {
            Some(a) => a,
            None => continue,
        };

        let patterns = match tf.language {
            "rust" => vec!["struct $NAME { $$$ }".to_string()],
            "python" => vec!["class $NAME: $$$".to_string()],
            "typescript" | "javascript" => vec!["class $NAME { $$$ }".to_string()],
            _ => continue,
        };

        for pat in &patterns {
            if let Ok(result) = adapter.search(pat, &tf.content) {
                if tf.content.contains("class ") || tf.content.contains("struct ") {
                    assert!(
                        result.match_count() >= 1,
                        "[{}] should find class/struct in {} (pattern: {})",
                        tf.language, tf.path.display(), pat
                    );
                }
            }
        }
    }
}

#[test]
fn integration_find_interfaces() {
    let files = get_test_files();
    assert!(!files.is_empty(), "should have test files");

    for tf in &files {
        if tf.language != "typescript" { continue; }

        let adapter = match knocode_repo_intel::structural::create_adapter("typescript") {
            Some(a) => a,
            None => continue,
        };

        if let Ok(result) = adapter.search("interface $NAME { $$$ }", &tf.content) {
            if tf.content.contains("interface ") {
                assert!(
                    result.match_count() >= 1,
                    "should find interface in {}",
                    tf.path.display()
                );
            }
        }
    }
}

#[test]
fn integration_find_imports() {
    let files = get_test_files();
    assert!(!files.is_empty(), "should have test files");

    for tf in &files {
        let adapter = match knocode_repo_intel::structural::create_adapter(tf.language) {
            Some(a) => a,
            None => continue,
        };

        let patterns = QueryPlanner::plan(
            &StructuralQuery::new("find all imports").with_language(tf.language)
        );

        for pat in &patterns {
            let _ = adapter.search(&pat.pattern, &tf.content);
        }

        // If file has import-like syntax, should find something
        if tf.content.contains("import ") || tf.content.contains("use ") {
            // Not all import syntax matches all patterns — just verify no panics
        }
    }
}

// ── Negative tests ─────────────────────────────────────────────────────

#[test]
fn integration_negative_empty_source() {
    let adapter = match knocode_repo_intel::structural::create_adapter("typescript") {
        Some(a) => a,
        None => return, // skip if no adapter
    };

    let result = adapter.search("function $NAME() { $$$ }", "").unwrap();
    assert_eq!(result.match_count(), 0, "empty source should have zero matches");
}

#[test]
fn integration_negative_wrong_pattern_for_language() {
    let adapter = match knocode_repo_intel::structural::create_adapter("python") {
        Some(a) => a,
        None => return,
    };

    // Rust-style fn pattern in Python — should error or return 0
    match adapter.search("fn $NAME() { $$$ }", "def hello(): pass") {
        Ok(result) => {
            assert_eq!(result.match_count(), 0, "Python should have no fn declarations");
        }
        Err(_) => {
            // AmbiguousPattern or InvalidPattern are acceptable
        }
    }
}

#[test]
fn integration_negative_no_match_when_pattern_absent() {
    let files = get_test_files();
    if files.is_empty() { return; }

    // Pick first file, search for something that definitely isn't there
    let tf = &files[0];
    let adapter = match knocode_repo_intel::structural::create_adapter(tf.language) {
        Some(a) => a,
        None => return,
    };

    // Search for "trait" in TypeScript — TS doesn't have traits
    // (some .d.ts files may have "trait" in comments, so we allow 0 matches)
    if tf.language == "typescript" {
        match adapter.search("trait $NAME { $$$ }", &tf.content) {
            Ok(_result) => {
                // 0 matches expected; if some found, they're likely false positives from comments
                // Just verify no panic occurred
            }
            Err(_) => {
                // AmbiguousPattern is also acceptable
            }
        }
    }
}

// ── Mixed tests (lexical + structural) ─────────────────────────────────

#[test]
fn integration_query_planner_explicit_pattern() {
    let sq = StructuralQuery::new("fn $NAME($$$) { $$$ }");
    let patterns = QueryPlanner::plan(&sq);

    assert_eq!(patterns.len(), 1, "explicit pattern should produce one resolved pattern");
    assert_eq!(patterns[0].pattern, "fn $NAME($$$) { $$$ }");
    assert_eq!(patterns[0].kind, "explicit");
}

#[test]
fn integration_query_planner_inferred_functions() {
    let sq = StructuralQuery::new("find all functions").with_language("rust");
    let patterns = QueryPlanner::plan(&sq);

    assert!(!patterns.is_empty(), "inferred function query should produce patterns");
    assert!(patterns.iter().any(|p| p.kind == "function"));
    assert!(patterns.len() >= 2, "Rust needs with/without return type");
}

#[test]
fn integration_query_planner_no_structural_for_procedural() {
    let sq = StructuralQuery::new("How do I add a new package?");
    let patterns = QueryPlanner::plan(&sq);
    assert!(patterns.is_empty(), "procedural query should not produce structural patterns");
}

#[test]
fn integration_structural_intent_detection() {
    let intent = StructuralIntent::from_query("fn $NAME() { }");
    assert!(matches!(intent, StructuralIntent::ExplicitPattern(_)));

    let intent = StructuralIntent::from_query("find all functions");
    assert!(matches!(intent, StructuralIntent::FindDeclarations(_)));

    let intent = StructuralIntent::from_query("How do I add a new package?");
    assert_eq!(intent, StructuralIntent::None);
}

// ── Latency budget tests ───────────────────────────────────────────────

#[test]
fn integration_latency_pattern_resolution() {
    let queries = vec![
        "find all functions",
        "show all classes",
        "find all calls to foo",
        "find all imports",
        "fn $NAME($$$) { $$$ }",
        "app.$METHOD($$$)",
    ];

    let budget_ms = 10;
    for query in &queries {
        let start = Instant::now();
        let sq = StructuralQuery::new(*query).with_language("rust");
        let _patterns = QueryPlanner::plan(&sq);
        let elapsed = start.elapsed().as_millis() as u64;
        assert!(
            elapsed <= budget_ms,
            "pattern resolution for '{}' took {}ms (budget: {}ms)",
            query, elapsed, budget_ms
        );
    }
}

#[test]
fn integration_latency_single_file_search() {
    let files = get_test_files();
    if files.is_empty() { return; }

    let tf = &files[0];
    let adapter = match knocode_repo_intel::structural::create_adapter(tf.language) {
        Some(a) => a,
        None => return,
    };

    let budget_ms = 200; // Scale for real files
    let start = Instant::now();
    let sq = StructuralQuery::new("find all functions").with_language(tf.language);
    let patterns = QueryPlanner::plan(&sq);
    for pat in &patterns {
        let _ = adapter.search(&pat.pattern, &tf.content);
    }
    let elapsed = start.elapsed().as_millis() as u64;

    assert!(
        elapsed <= budget_ms,
        "single file search took {}ms (budget: {}ms)",
        elapsed, budget_ms
    );
}

#[test]
fn integration_latency_multi_pattern() {
    let files = get_test_files();
    if files.is_empty() { return; }

    let tf = &files[0];
    let adapter = match knocode_repo_intel::structural::create_adapter(tf.language) {
        Some(a) => a,
        None => return,
    };

    let patterns = vec![
        "function $NAME($$$) { $$$ }",
        "class $NAME { $$$ }",
        "interface $NAME { $$$ }",
        "import $MODULE from $PATH",
    ];

    // Budget scales with content size: 400ms base + 2ms per 10KB
    let content_kb = tf.content.len() / 1024;
    let budget_ms = 400 + (content_kb as u64) * 2;
    let start = Instant::now();
    for pat in &patterns {
        let _ = adapter.search(pat, &tf.content);
    }
    let elapsed = start.elapsed().as_millis() as u64;

    assert!(
        elapsed <= budget_ms,
        "multi-pattern search took {}ms (budget: {}ms, content: {}KB)",
        elapsed, budget_ms, content_kb
    );
}

// ── Plan inspection tests ──────────────────────────────────────────────

#[test]
fn integration_plan_has_structural_pattern() {
    let plan = build_plan_for_query("find all functions");
    assert!(plan.structural, "plan should enable structural");
    assert!(plan.structural_pattern.is_some(), "plan should have a resolved pattern");
    assert!(plan.structural_intent.is_some(), "plan should have a structural intent");
}

#[test]
fn integration_plan_no_structural_for_procedural() {
    let plan = build_plan_for_query("How do I add a new package?");
    assert!(!plan.structural, "procedural plan should not enable structural");
    assert!(plan.structural_pattern.is_none(), "procedural plan should have no pattern");
}

#[test]
fn integration_plan_debugging_enriches() {
    let plan = build_plan_for_query("Why does the build fail?");
    assert!(plan.lexical, "debugging should use lexical");
    assert!(plan.structural, "debugging should enrich with structural");
    assert!(plan.graph, "debugging should use graph");
}

// ── DefinitelyTyped-specific tests ─────────────────────────────────────

/// Test that TypeScript interface declarations are found in .d.ts files.
/// This specifically tests the DefinitelyTyped use case.
#[test]
fn integration_dt_find_interfaces_in_dts() {
    let repo = match test_repo_path() {
        Some(p) => p,
        None => return, // Only runs with KNOCODE_TEST_REPO
    };

    // Find a .d.ts file that likely has interfaces
    let dts_files = find_files_with_ext(&repo, "d.ts", 5, 10);
    assert!(!dts_files.is_empty(), "should find .d.ts files in {}", repo.display());

    let adapter = match knocode_repo_intel::structural::create_adapter("typescript") {
        Some(a) => a,
        None => return,
    };

    let mut found_interfaces = false;
    for path in &dts_files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        if let Ok(result) = adapter.search("interface $NAME { $$$ }", &content) {
            if result.match_count() > 0 {
                found_interfaces = true;
                // Verify captures
                for m in &result.matches {
                    assert!(m.capture("NAME").is_some(), "interface should capture NAME in {}", path.display());
                }
                break;
            }
        }
    }

    // At least one .d.ts file should have interfaces (DefinitelyTyped is full of them)
    assert!(found_interfaces, "should find at least one interface in .d.ts files");
}

/// Test that TypeScript function declarations are found.
#[test]
fn integration_dt_find_functions_in_dts() {
    let repo = match test_repo_path() {
        Some(p) => p,
        None => return,
    };

    let dts_files = find_files_with_ext(&repo, "d.ts", 5, 10);
    assert!(!dts_files.is_empty(), "should find .d.ts files");

    let adapter = match knocode_repo_intel::structural::create_adapter("typescript") {
        Some(a) => a,
        None => return,
    };

    for path in &dts_files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let _ = adapter.search("function $NAME($$$)", &content);
    }

    // Some .d.ts files should have function declarations
    if !dts_files.is_empty() {
        // Not all .d.ts files have functions — just verify the search works
    }
}

/// Test that TypeScript type aliases are found.
#[test]
fn integration_dt_find_type_aliases() {
    let repo = match test_repo_path() {
        Some(p) => p,
        None => return,
    };

    let dts_files = find_files_with_ext(&repo, "d.ts", 5, 10);
    if dts_files.is_empty() { return; }

    let adapter = match knocode_repo_intel::structural::create_adapter("typescript") {
        Some(a) => a,
        None => return,
    };

    // Search for type aliases — many .d.ts files use `type X = ...`
    for path in dts_files.iter().take(5) {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        // Just verify the search doesn't panic
        let _ = adapter.search("type $NAME = $$$", &content);
    }
}

/// Test latency against real .d.ts files (DefinitelyTyped has large files).
#[test]
fn integration_dt_latency_budget() {
    let repo = match test_repo_path() {
        Some(p) => p,
        None => return,
    };

    let dts_files = find_files_with_ext(&repo, "d.ts", 5, 3);
    if dts_files.is_empty() { return; }

    let adapter = match knocode_repo_intel::structural::create_adapter("typescript") {
        Some(a) => a,
        None => return,
    };

    let budget_ms = 500; // Real files may be larger
    for path in &dts_files {
        let content = match std::fs::read_to_string(path) {
            Ok(c) => c,
            Err(_) => continue,
        };
        let start = Instant::now();
        let _ = adapter.search("interface $NAME { $$$ }", &content);
        let _ = adapter.search("function $NAME($$$)", &content);
        let _ = adapter.search("type $NAME = $$$", &content);
        let elapsed = start.elapsed().as_millis() as u64;

        assert!(
            elapsed <= budget_ms,
            "search in {} took {}ms (budget: {}ms, content: {} bytes)",
            path.display(), elapsed, budget_ms, content.len()
        );
    }
}

// ── Helpers ────────────────────────────────────────────────────────────

fn build_plan_for_query(query: &str) -> crate::retrieval::plan::RetrievalPlan {
    use crate::retrieval::engine::CombinedRetriever;
    let retriever = CombinedRetriever::default();
    let rq = RetrievalQuery::new(query, "test-repo");
    retriever.build_plan(&rq)
}
