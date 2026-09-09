use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

// Pre-compiled regex patterns — avoids Regex::new per call on hot path
static FN_PATTERNS: LazyLock<Vec<regex::Regex>> = LazyLock::new(|| {
    vec![
        regex::Regex::new(r"(?:pub\s+)?(?:async\s+)?fn\s+(\w+)\s*\(").unwrap(),
        regex::Regex::new(r"def\s+(\w+)\s*\(").unwrap(),
        regex::Regex::new(r"(?:export\s+)?(?:async\s+)?function\s+(\w+)\s*\(").unwrap(),
        regex::Regex::new(r"(?:public|private|protected|internal)\s+(?:static\s+)?(?:async\s+)?(?:void|bool|int|long|float|double|string|var|IEnumerable|Task|ValueTask|IActionResult|ActionResult|ObjectResult)\s+(\w+)\s*\(").unwrap(),
        regex::Regex::new(r"(?:public|private|protected)\s+(?:static\s+)?(?:final\s+)?(?:void|boolean|int|long|float|double|String|var)\s+(\w+)\s*\(").unwrap(),
    ]
});

static CALL_PATTERN: LazyLock<regex::Regex> = LazyLock::new(|| {
    regex::Regex::new(r"\b([a-zA-Z_]\w*)\s*\(").unwrap()
});

/// Dependency graph derived from imports (local AST + regex)
/// Spec §3 Repository Intelligence — produce symbols, dependency graph, entry points.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct DependencyGraph {
    /// adjacency: file -> Vec<dep_file>
    edges: HashMap<String, Vec<String>>,
    /// reverse edges for impact analysis
    reverse: HashMap<String, Vec<String>>,
}

impl DependencyGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build graph from file list — local AST+regex
    pub fn build_from_files(repo_root: &Path, files: &[PathBuf]) -> Self {
        Self::build_from_files_with_progress(repo_root, files, &mut |_, _| {})
    }

    /// Build graph from file list with progress reporting.
    /// Callback receives (files_done, files_total); called once with (0, total) up front
    /// and once after each file is processed.
    pub fn build_from_files_with_progress(
        repo_root: &Path,
        files: &[PathBuf],
        progress: &mut dyn FnMut(usize, usize),
    ) -> Self {
        let mut graph = Self::new();
        let file_index = FileIndex::new(repo_root, files);
        let total = files.len();
        progress(0, total);
        for (done, file) in files.iter().enumerate() {
            let rel = file.strip_prefix(repo_root).unwrap_or(file).to_string_lossy().replace('\\', "/");
            if let Ok(content) = std::fs::read_to_string(file) {
                let language = tree_sitter_language_pack::detect_language_from_path(&rel);
                let deps = extract_imports(&content, language, &file_index);
                for dep in deps {
                    graph.add_edge(rel.clone(), dep);
                }
            }
            progress(done + 1, total);
        }
        graph
    }

    pub fn add_edge(&mut self, from: String, to: String) {
        self.edges.entry(from.clone()).or_default().push(to.clone());
        self.reverse.entry(to).or_default().push(from);
    }

    pub fn dependencies_of(&self, file: &str) -> Vec<String> {
        self.edges.get(file).cloned().unwrap_or_default()
    }

    pub fn dependents_of(&self, file: &str) -> Vec<String> {
        self.reverse.get(file).cloned().unwrap_or_default()
    }

    /// Impact analysis: transitive dependents of changed file
    pub fn impact_analysis(&self, changed: &str) -> Vec<String> {
        let mut visited = HashSet::new();
        let mut queue = vec![changed.to_string()];
        let mut result = Vec::new();
        while let Some(cur) = queue.pop() {
            for dep in self.dependents_of(&cur) {
                if visited.insert(dep.clone()) {
                    result.push(dep.clone());
                    queue.push(dep);
                }
            }
        }
        result
    }

    pub fn edge_count(&self) -> usize {
        self.edges.values().map(|v| v.len()).sum()
    }

    /// Get all files in the graph (both as sources and targets)
    pub fn all_files(&self) -> Vec<&String> {
        let mut files: HashSet<&String> = HashSet::new();
        for file in self.edges.keys() {
            files.insert(file);
        }
        for deps in self.edges.values() {
            for dep in deps {
                files.insert(dep);
            }
        }
        files.into_iter().collect()
    }
}

/// Call graph tracking function calls between files (P2 #8)
/// Uses deeper AST analysis to track function calls across file boundaries
#[derive(Debug, Default)]
pub struct CallGraph {
    /// Function call edges: (caller_file, caller_func) -> Vec<(callee_file, callee_func)>
    edges: HashMap<(String, String), Vec<(String, String)>>,
    /// Reverse edges for impact analysis
    reverse: HashMap<(String, String), Vec<(String, String)>>,
    /// Function definitions: file -> Vec<(func_name, line_start, line_end)>
    definitions: HashMap<String, Vec<(String, usize, usize)>>,
}

impl CallGraph {
    pub fn new() -> Self {
        Self::default()
    }

    /// Build call graph from file list using AST analysis
    pub fn build_from_files(repo_root: &Path, files: &[PathBuf]) -> Self {
        let mut graph = Self::new();
        
        // First pass: extract function definitions from all files
        for file in files {
            let rel = file.strip_prefix(repo_root).unwrap_or(file).to_string_lossy().to_string();
            if let Ok(content) = std::fs::read_to_string(file) {
                let defs = extract_function_definitions(&content, &rel);
                graph.definitions.insert(rel, defs);
            }
        }
        
        // Second pass: extract function calls and build edges
        for file in files {
            let rel = file.strip_prefix(repo_root).unwrap_or(file).to_string_lossy().to_string();
            if let Ok(content) = std::fs::read_to_string(file) {
                let calls = extract_function_calls(&content, &rel);
                let caller_defs = graph.definitions.get(&rel).cloned().unwrap_or_default();
                
                // For each call, find which function it's in and link to callee
                for (call_line, call_name) in calls {
                    // Find the function containing this call
                    let caller_func = find_containing_function(&caller_defs, call_line);
                    
                    // Try to find the callee definition across all files
                    if let Some((callee_file, _callee_line)) = find_function_definition(&graph.definitions, &call_name) {
                        let caller = (rel.clone(), caller_func);
                        let callee = (callee_file, call_name);
                        
                        graph.edges.entry(caller.clone()).or_default().push(callee.clone());
                        graph.reverse.entry(callee).or_default().push(caller);
                    }
                }
            }
        }
        
        graph
    }

    /// Get all functions called by a specific function
    pub fn callees_of(&self, file: &str, function: &str) -> Vec<(String, String)> {
        self.edges.get(&(file.to_string(), function.to_string())).cloned().unwrap_or_default()
    }

    /// Get all functions that call a specific function
    pub fn callers_of(&self, file: &str, function: &str) -> Vec<(String, String)> {
        self.reverse.get(&(file.to_string(), function.to_string())).cloned().unwrap_or_default()
    }

    /// Get all function definitions in a file
    pub fn definitions_in(&self, file: &str) -> Vec<(String, usize, usize)> {
        self.definitions.get(file).cloned().unwrap_or_default()
    }

    /// Get all files in the call graph
    pub fn all_files(&self) -> Vec<&String> {
        let mut files: HashSet<&String> = HashSet::new();
        for file in self.definitions.keys() {
            files.insert(file);
        }
        files.into_iter().collect()
    }

    /// Get edge count
    pub fn edge_count(&self) -> usize {
        self.edges.values().map(|v| v.len()).sum()
    }
}

/// Extract function definitions from source code using regex patterns
fn extract_function_definitions(content: &str, _file: &str) -> Vec<(String, usize, usize)> {
    let mut defs = Vec::new();
    
    // Rust: fn name(...) { ... }
    // Python: def name(...): ...
    // JS/TS: function name(...) { ... } or const name = (...) => { ... }
    // C#: public/private/static ReturnType Name(...) { ... }
    
    for (line_num, line) in content.lines().enumerate() {
        for pattern in FN_PATTERNS.iter() {
            if let Some(caps) = pattern.captures(line) {
                if let Some(name_match) = caps.get(1) {
                    let name = name_match.as_str().to_string();
                    // FIX #2: Count braces to find function end instead of fixed 50-line placeholder.
                    let line_start = line_num + 1;
                    let mut brace_depth = 0i32;
                    let mut found_open = false;
                    let mut line_end = line_start;
                    for subsequent_line in content.lines().skip(line_num) {
                        line_end += 1;
                        for ch in subsequent_line.chars() {
                            match ch {
                                '{' => { brace_depth += 1; found_open = true; }
                                '}' => {
                                    brace_depth -= 1;
                                    if found_open && brace_depth == 0 {
                                        // Found matching close brace — function ends here
                                        break;
                                    }
                                }
                                _ => {}
                            }
                        }
                        if found_open && brace_depth == 0 {
                            break;
                        }
                    }
                    // Safety: cap at 2000 lines to avoid runaway on malformed code
                    let line_end = line_end.min(line_start + 2000);
                    defs.push((name, line_start, line_end));
                }
            }
        }
    }
    
    defs
}

/// Extract function calls from source code
fn extract_function_calls(content: &str, _file: &str) -> Vec<(usize, String)> {
    let mut calls = Vec::new();
    
    // Keywords to exclude (not function calls)
    let keywords: HashSet<&str> = [
        "if", "else", "for", "while", "loop", "match", "return", "break", "continue",
        "fn", "struct", "enum", "impl", "trait", "type", "pub", "mod", "use",
        "class", "interface", "extends", "implements", "new", "this", "super",
        "function", "const", "let", "var", "import", "from", "export",
        "def", "self", "async", "await", "move",
    ].iter().cloned().collect();
    
    for (line_num, line) in content.lines().enumerate() {
        for caps in CALL_PATTERN.captures_iter(line) {
            if let Some(name_match) = caps.get(1) {
                let name = name_match.as_str();
                if !keywords.contains(name) && name.len() > 1 {
                    calls.push((line_num + 1, name.to_string()));
                }
            }
        }
    }
    
    calls
}

/// Find which function contains a given line number
fn find_containing_function(defs: &[(String, usize, usize)], line: usize) -> String {
    for (name, start, end) in defs {
        if line >= *start && line <= *end {
            return name.clone();
        }
    }
    "unknown".to_string()
}

/// Find function definition across all files
fn find_function_definition(definitions: &HashMap<String, Vec<(String, usize, usize)>>, func_name: &str) -> Option<(String, usize)> {
    for (file, defs) in definitions {
        for (name, line_start, _) in defs {
            if name == func_name {
                return Some((file.clone(), *line_start));
            }
        }
    }
    None
}

/// Extract imports from source code using tree-sitter-language-pack.
/// Returns resolved file paths relative to the repo root.
/// Falls back to regex extraction when tree-sitter is unavailable.
fn extract_imports(content: &str, language: Option<&str>, file_index: &FileIndex) -> Vec<String> {
    let lang = language.unwrap_or("");

    // Try tree-sitter extraction (grammar may not be downloaded — handle gracefully)
    if !lang.is_empty() {
        let mut config = tree_sitter_language_pack::ProcessConfig::new(lang).minimal();
        config.imports = true;
        if let Ok(result) = tree_sitter_language_pack::process(content, &config) {
            let mut deps = Vec::new();
            for import in &result.imports {
                if let Some(resolved) = file_index.resolve_import(&import.source, lang) {
                    deps.push(resolved);
                }
            }
            // Also extract `mod` declarations (Rust-specific, not always in imports)
            if lang == "rust" {
                for line in content.lines() {
                    let trimmed = line.trim();
                    if let Some(rest) = trimmed.strip_prefix("mod ") {
                        let name = rest.split(|c: char| c == ';' || c == '{' || c.is_whitespace()).next().unwrap_or("").trim();
                        if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                            if let Some(path) = file_index.resolve_rust_mod(name) {
                                deps.push(path);
                            }
                        }
                    }
                }
            }
            // Only use tree-sitter result if it found imports; otherwise grammar may be incomplete
            if !deps.is_empty() {
                deps.sort();
                deps.dedup();
                return deps;
            }
        }
        // Tree-sitter failed or returned empty — fall through to regex
    }

    // Fallback: regex-based extraction (no tree-sitter available)
    extract_imports_regex(content)
}

/// Regex-based fallback for when tree-sitter is unavailable.
fn extract_imports_regex(content: &str) -> Vec<String> {
    let mut deps = Vec::new();
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("use ") || trimmed.starts_with("mod ") {
            if let Some(rest) = trimmed.strip_prefix("mod ") {
                let name = rest.split(|c: char| c == ';' || c == '{' || c.is_whitespace()).next().unwrap_or("").trim();
                if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                    deps.push(format!("{}.rs", name));
                    continue;
                }
            }
            if let Some(rest) = trimmed.strip_prefix("use ") {
                let clean = rest.split(';').next().unwrap_or("").trim().split(" as ").next().unwrap_or("").trim().to_string();
                let parts: Vec<&str> = clean.split("::").collect();
                let mut dep_part = "";
                for p in &parts {
                    if !["crate", "self", "super", "std", "core"].contains(p) && !p.is_empty() {
                        dep_part = p;
                        break;
                    }
                }
                if dep_part.is_empty() { dep_part = parts.first().copied().unwrap_or(""); }
                if !dep_part.is_empty() && !["std","core"].contains(&dep_part) {
                    deps.push(format!("{}.rs", dep_part));
                    let full = parts.join("/");
                    if full.contains('/') && !deps.contains(&format!("{}.rs", full)) {
                        deps.push(full);
                    }
                }
            }
        } else if trimmed.starts_with("import ") || trimmed.contains(" from ") {
            if let Some(start) = trimmed.find('"').or_else(|| trimmed.find('\'')) {
                let quote = trimmed.chars().nth(trimmed.find('"').unwrap_or(trimmed.find('\'').unwrap_or(0))).unwrap_or('"');
                let rest = &trimmed[start+1..];
                if let Some(end) = rest.find(quote) {
                    let path = &rest[..end];
                    if !path.is_empty() {
                        deps.push(path.to_string());
                    }
                }
            }
        }
    }
    deps.sort();
    deps.dedup();
    deps
}

/// Index of all files in the repository, used for import path resolution.
struct FileIndex {
    /// Set of relative file paths (normalized with `/` separators)
    paths: HashSet<String>,
}

impl FileIndex {
    fn new(repo_root: &Path, files: &[PathBuf]) -> Self {
        let mut paths = HashSet::new();
        for file in files {
            let rel = file.strip_prefix(repo_root).unwrap_or(file).to_string_lossy().replace('\\', "/");
            paths.insert(rel);
        }
        Self { paths }
    }

    /// Resolve a Rust `mod name` declaration to a file path.
    /// Checks `name.rs` and `name/mod.rs` (Rust module conventions).
    fn resolve_rust_mod(&self, name: &str) -> Option<String> {
        let direct = format!("{}.rs", name);
        if self.paths.contains(&direct) {
            return Some(direct);
        }
        let nested = format!("{}/mod.rs", name);
        if self.paths.contains(&nested) {
            return Some(nested);
        }
        None
    }

    /// Resolve an import source path to a repository file path.
    /// Handles Rust `use`, JS/TS `import from`, Python `from ... import`.
    fn resolve_import(&self, source: &str, language: &str) -> Option<String> {
        match language {
            "rust" => self.resolve_rust_import(source),
            "javascript" | "typescript" | "tsx" | "jsx" => self.resolve_js_import(source),
            "python" => self.resolve_python_import(source),
            _ => None,
        }
    }

    /// Resolve Rust `use crate::foo::bar` or `use foo::bar`.
    /// Strategy: try `foo/bar.rs`, `foo/bar/mod.rs`, then just `foo.rs`.
    fn resolve_rust_import(&self, source: &str) -> Option<String> {
        // Skip std, core, external crate prefixes
        let first = source.split("::").next().unwrap_or("");
        if ["std", "core", "alloc"].contains(&first) {
            return None;
        }
        // Strip crate/self/super prefix
        let path_part = source
            .trim_start_matches("crate::")
            .trim_start_matches("self::")
            .trim_start_matches("super::");
        if path_part.is_empty() { return None; }

        let segments: Vec<&str> = path_part.split("::").collect();

        // Try progressively shorter paths: a::b::c → a/b/c.rs, a/b.rs, a.rs
        for i in (1..=segments.len()).rev() {
            let candidate = segments[..i].join("/");
            // Try as file
            let as_file = format!("{}.rs", candidate);
            if self.paths.contains(&as_file) {
                return Some(as_file);
            }
            // Try as mod
            let as_mod = format!("{}/mod.rs", candidate);
            if self.paths.contains(&as_mod) {
                return Some(as_mod);
            }
        }
        None
    }

    /// Resolve JS/TS `import ... from './foo'` or `require('./foo')`.
    fn resolve_js_import(&self, source: &str) -> Option<String> {
        // Skip bare specifiers (npm packages — no `.` or `/` prefix)
        if !source.starts_with('.') && !source.starts_with('/') {
            return None;
        }
        // Strip leading `./` or `../` for matching against repo paths
        let base = source.trim_start_matches("./").trim_start_matches("../");
        // Try exact match
        if self.paths.contains(base) {
            return Some(base.to_string());
        }
        // Try with extensions
        for ext in &[".ts", ".tsx", ".js", ".jsx", ".mts", ".cts"] {
            let candidate = format!("{}{}", base, ext);
            if self.paths.contains(&candidate) {
                return Some(candidate);
            }
        }
        // Try as index file
        for name in &["index.ts", "index.tsx", "index.js", "index.jsx"] {
            let candidate = format!("{}/{}", base, name);
            if self.paths.contains(&candidate) {
                return Some(candidate);
            }
        }
        None
    }

    /// Resolve Python `from foo import bar` or `import foo`.
    fn resolve_python_import(&self, source: &str) -> Option<String> {
        let parts: Vec<&str> = source.split('.').collect();
        // Try progressively: foo/bar/__init__.py, foo/bar.py, foo/__init__.py, foo.py
        for i in (1..=parts.len()).rev() {
            let candidate = parts[..i].join("/");
            // Try as package __init__.py
            let as_init = format!("{}/__init__.py", candidate);
            if self.paths.contains(&as_init) {
                return Some(as_init);
            }
            // Try as module
            let as_module = format!("{}.py", candidate);
            if self.paths.contains(&as_module) {
                return Some(as_module);
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_extract_imports_rust_regex_fallback() {
        let content = "use crate::graph::DependencyGraph;\nuse std::collections::HashMap;\nmod foo;";
        let index = FileIndex::new(Path::new("."), &[]);
        let deps = extract_imports(content, None, &index);
        // Regex fallback: extracts "graph.rs" from `use crate::graph::DependencyGraph`
        assert!(deps.iter().any(|d| d.contains("graph")));
    }

    #[test]
    fn test_extract_imports_rust_tree_sitter() {
        let dir = std::env::temp_dir().join(format!("knocode_graph_ts_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("graph.rs"), "pub struct DependencyGraph;").unwrap();
        std::fs::write(dir.join("main.rs"), "use crate::graph::DependencyGraph;\nfn main() {}").unwrap();
        let files = vec![dir.join("graph.rs"), dir.join("main.rs")];
        let index = FileIndex::new(&dir, &files);
        let content = std::fs::read_to_string(dir.join("main.rs")).unwrap();
        let deps = extract_imports(&content, Some("rust"), &index);
        assert!(deps.contains(&"graph.rs".to_string()), "expected graph.rs in deps, got {:?}", deps);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_resolve_rust_mod() {
        let files: Vec<PathBuf> = vec!["a.rs".into(), "b/mod.rs".into()];
        let index = FileIndex::new(Path::new("."), &files);
        assert_eq!(index.resolve_rust_mod("a"), Some("a.rs".to_string()));
        assert_eq!(index.resolve_rust_mod("b"), Some("b/mod.rs".to_string()));
        assert_eq!(index.resolve_rust_mod("nonexistent"), None);
    }

    #[test]
    fn test_resolve_rust_import() {
        let files: Vec<PathBuf> = vec!["graph.rs".into(), "foo/bar.rs".into(), "foo/bar/mod.rs".into()];
        let index = FileIndex::new(Path::new("."), &files);
        assert_eq!(index.resolve_rust_import("crate::graph"), Some("graph.rs".to_string()));
        assert_eq!(index.resolve_rust_import("crate::foo::bar"), Some("foo/bar.rs".to_string()));
        assert_eq!(index.resolve_rust_import("std::collections::HashMap"), None);
    }

    #[test]
    fn test_resolve_js_import() {
        let files: Vec<PathBuf> = vec!["utils.ts".into(), "components/Button.tsx".into(), "lib/index.ts".into()];
        let index = FileIndex::new(Path::new("."), &files);
        assert_eq!(index.resolve_js_import("./utils"), Some("utils.ts".to_string()));
        assert_eq!(index.resolve_js_import("./components/Button"), Some("components/Button.tsx".to_string()));
        assert_eq!(index.resolve_js_import("./lib"), Some("lib/index.ts".to_string()));
        assert_eq!(index.resolve_js_import("react"), None); // bare specifier
    }

    #[test]
    fn test_resolve_python_import() {
        let files: Vec<PathBuf> = vec!["utils.py".into(), "pkg/__init__.py".into(), "pkg/module.py".into()];
        let index = FileIndex::new(Path::new("."), &files);
        assert_eq!(index.resolve_python_import("utils"), Some("utils.py".to_string()));
        assert_eq!(index.resolve_python_import("pkg"), Some("pkg/__init__.py".to_string()));
        assert_eq!(index.resolve_python_import("pkg.module"), Some("pkg/module.py".to_string()));
    }

    #[test]
    fn test_graph_edges_and_impact() {
        let mut g = DependencyGraph::new();
        g.add_edge("a.rs".to_string(), "b.rs".to_string());
        g.add_edge("b.rs".to_string(), "c.rs".to_string());
        assert_eq!(g.edge_count(), 2);
        assert_eq!(g.dependencies_of("a.rs"), vec!["b.rs"]);
        let impact = g.impact_analysis("c.rs");
        assert!(impact.contains(&"b.rs".to_string()));
        assert!(impact.contains(&"a.rs".to_string()));
    }

    #[test]
    fn test_build_from_files_tmp() {
        let dir = std::env::temp_dir().join(format!("knocode_graph_test_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file_a = dir.join("a.rs");
        let file_b = dir.join("b.rs");
        std::fs::write(&file_a, "use crate::b::Foo;\nfn main() {}").unwrap();
        std::fs::write(&file_b, "pub struct Foo;").unwrap();
        let graph = DependencyGraph::build_from_files(&dir, &[file_a.clone(), file_b.clone()]);
        assert!(graph.edge_count() >= 1);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn test_extract_imports_js_from_quoted() {
        let content = r#"import foo from "bar/baz"; const x = require('qux');"#;
        let index = FileIndex::new(Path::new("."), &[]);
        let deps = extract_imports(content, None, &index);
        assert!(deps.iter().any(|d| d.contains("bar/baz")));
    }
}
