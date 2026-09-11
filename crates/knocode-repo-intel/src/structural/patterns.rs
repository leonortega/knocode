//! Structural patterns — reusable, per-language pattern definitions.
//!
//! ## Architecture
//!
//! ```text
//! structural_patterns/
//! ├── declarations  (function, class, interface, type, enum, trait, module, impl)
//! ├── calls         (function_call, method_call)
//! ├── dependencies  (import, require)
//! └── relationships (extends, implements)
//! ```
//!
//! Each category provides patterns for multiple languages. Patterns use
//! ast-grep metavariable syntax (`$NAME`, `$$$ARGS`).

/// A structural pattern for a specific language.
#[derive(Debug, Clone)]
pub struct PatternDef {
    /// The ast-grep pattern string (e.g., `"fn $NAME($$$) { $$$ }"`).
    pub pattern: String,
    /// Human-readable description.
    pub description: String,
    /// Priority: lower = tried first (for multi-pattern languages).
    pub priority: u32,
}

/// Get all patterns for a structural category and language.
///
/// Categories: "function", "class", "method", "impl", "trait"/"interface",
/// "enum", "module", "call", "import", "extends"/"implements".
pub fn patterns_for(category: &str, lang: &str) -> Vec<PatternDef> {
    match category {
        "function" => function_patterns(lang),
        "class" => class_patterns(lang),
        "method" => method_patterns(lang),
        "impl" => impl_patterns(lang),
        "trait" | "interface" => interface_patterns(lang),
        "enum" => enum_patterns(lang),
        "module" => module_patterns(lang),
        "call" => call_patterns(lang),
        "import" => import_patterns(lang),
        "extends" | "implements" => relationship_patterns(lang),
        _ => vec![],
    }
}

// ── Declarations ──────────────────────────────────────────────────────

fn function_patterns(lang: &str) -> Vec<PatternDef> {
    let raw = match lang {
        "rust" => vec![
            ("fn $NAME($$$) { $$$ }", "function without return type"),
            ("fn $NAME($$$) -> $RET { $$$ }", "function with return type"),
        ],
        "python" => vec![
            ("def $NAME($$$): $$$", "function definition"),
        ],
        "javascript" | "typescript" | "tsx" => vec![
            ("function $NAME($$$) { $$$ }", "function declaration"),
        ],
        "go" => vec![
            ("func $NAME($$$) { $$$ }", "function declaration"),
        ],
        "java" | "csharp" => vec![
            ("public $RET $NAME($$$) { $$$ }", "public method"),
        ],
        "kotlin" => vec![
            ("fun $NAME($$$) { $$$ }", "function declaration"),
        ],
        "ruby" => vec![
            ("def $NAME; $$$", "method definition"),
        ],
        "swift" => vec![
            ("func $NAME($$$) $$$", "function declaration"),
        ],
        "scala" => vec![
            ("def $NAME($$$) = $$$", "function definition"),
        ],
        "haskell" => vec![
            ("$NAME $$$ = $$$", "function definition"),
        ],
        "lua" => vec![
            ("function $NAME($$$) $$$ end", "function definition"),
        ],
        "c" | "cpp" => vec![
            ("$RET $NAME($$$) { $$$ }", "function definition"),
        ],
        _ => vec![
            ("function $NAME($$$) { $$$ }", "function declaration"),
        ],
    };

    raw.into_iter().enumerate().map(|(i, (pat, desc))| {
        PatternDef {
            pattern: pat.to_string(),
            description: desc.to_string(),
            priority: i as u32,
        }
    }).collect()
}

fn class_patterns(lang: &str) -> Vec<PatternDef> {
    let raw = match lang {
        "rust" => vec![("struct $NAME { $$$ }", "struct definition")],
        "python" => vec![("class $NAME: $$$", "class definition")],
        "javascript" | "typescript" | "tsx" => vec![("class $NAME { $$$ }", "class definition")],
        "go" => vec![("type $NAME struct { $$$ }", "struct type definition")],
        "java" | "kotlin" | "csharp" => vec![("class $NAME { $$$ }", "class definition")],
        _ => vec![("class $NAME { $$$ }", "class definition")],
    };

    raw.into_iter().enumerate().map(|(i, (pat, desc))| {
        PatternDef { pattern: pat.to_string(), description: desc.to_string(), priority: i as u32 }
    }).collect()
}

fn method_patterns(lang: &str) -> Vec<PatternDef> {
    let raw = match lang {
        "rust" => vec![
            ("impl $TYPE { fn $NAME($$$) { $$$ } }", "method without return type"),
            ("impl $TYPE { fn $NAME($$$) -> $RET { $$$ } }", "method with return type"),
        ],
        _ => vec![("function $NAME($$$) { $$$ }", "method definition")],
    };

    raw.into_iter().enumerate().map(|(i, (pat, desc))| {
        PatternDef { pattern: pat.to_string(), description: desc.to_string(), priority: i as u32 }
    }).collect()
}

fn impl_patterns(lang: &str) -> Vec<PatternDef> {
    let raw = match lang {
        "rust" => vec![("impl $TYPE { $$$ }", "impl block")],
        _ => vec![],
    };

    raw.into_iter().enumerate().map(|(i, (pat, desc))| {
        PatternDef { pattern: pat.to_string(), description: desc.to_string(), priority: i as u32 }
    }).collect()
}

fn interface_patterns(lang: &str) -> Vec<PatternDef> {
    let raw = match lang {
        "rust" => vec![("trait $NAME { $$$ }", "trait definition")],
        "typescript" | "tsx" => vec![("interface $NAME { $$$ }", "interface definition")],
        "java" | "kotlin" | "csharp" => vec![("interface $NAME { $$$ }", "interface definition")],
        _ => vec![("interface $NAME { $$$ }", "interface definition")],
    };

    raw.into_iter().enumerate().map(|(i, (pat, desc))| {
        PatternDef { pattern: pat.to_string(), description: desc.to_string(), priority: i as u32 }
    }).collect()
}

fn enum_patterns(lang: &str) -> Vec<PatternDef> {
    let raw = match lang {
        "rust" => vec![("enum $NAME { $$$ }", "enum definition")],
        "typescript" | "tsx" | "java" | "kotlin" | "csharp" => {
            vec![("enum $NAME { $$$ }", "enum definition")]
        }
        _ => vec![("enum $NAME { $$$ }", "enum definition")],
    };

    raw.into_iter().enumerate().map(|(i, (pat, desc))| {
        PatternDef { pattern: pat.to_string(), description: desc.to_string(), priority: i as u32 }
    }).collect()
}

fn module_patterns(lang: &str) -> Vec<PatternDef> {
    let raw = match lang {
        "rust" => vec![("mod $NAME { $$$ }", "module definition")],
        _ => vec![],
    };

    raw.into_iter().enumerate().map(|(i, (pat, desc))| {
        PatternDef { pattern: pat.to_string(), description: desc.to_string(), priority: i as u32 }
    }).collect()
}

// ── Calls ──────────────────────────────────────────────────────────────

fn call_patterns(lang: &str) -> Vec<PatternDef> {
    let raw = match lang {
        "rust" | "python" | "javascript" | "typescript" | "tsx" | "go" | "java" | "kotlin" | "csharp" => {
            vec![
                ("$OBJ.$METHOD($$$)", "method call"),
                ("$FUNC($$$)", "function call"),
            ]
        }
        _ => vec![
            ("$OBJ.$METHOD($$$)", "method call"),
            ("$FUNC($$$)", "function call"),
        ],
    };

    raw.into_iter().enumerate().map(|(i, (pat, desc))| {
        PatternDef { pattern: pat.to_string(), description: desc.to_string(), priority: i as u32 }
    }).collect()
}

// ── Dependencies ──────────────────────────────────────────────────────

fn import_patterns(lang: &str) -> Vec<PatternDef> {
    let raw = match lang {
        "rust" => vec![
            ("use $MODULE;", "use statement"),
            ("use $MODULE::$$$;", "use with paths"),
        ],
        "python" => vec![
            ("import $MODULE", "import statement"),
            ("from $MODULE import $$$", "from import"),
        ],
        "javascript" | "typescript" | "tsx" => vec![
            ("import $MODULE from $PATH", "default import"),
            ("import { $$$ } from $PATH", "named import"),
        ],
        "go" => vec![("import \"$PATH\"", "import statement")],
        "java" => vec![("import $MODULE;", "import statement")],
        "kotlin" => vec![("import $MODULE", "import statement")],
        "csharp" => vec![("using $MODULE;", "using statement")],
        _ => vec![],
    };

    raw.into_iter().enumerate().map(|(i, (pat, desc))| {
        PatternDef { pattern: pat.to_string(), description: desc.to_string(), priority: i as u32 }
    }).collect()
}

// ── Relationships ─────────────────────────────────────────────────────

fn relationship_patterns(lang: &str) -> Vec<PatternDef> {
    let raw = match lang {
        "rust" => vec![], // Rust uses trait impl, not extends
        "python" => vec![("class $NAME($BASE): $$$", "class inheritance")],
        "javascript" | "typescript" | "tsx" => {
            vec![("class $NAME extends $BASE { $$$ }", "class extends")]
        }
        "java" | "kotlin" => vec![("class $NAME extends $BASE { $$$ }", "class extends")],
        "csharp" => vec![("class $NAME : $BASE { $$$ }", "class inherits")],
        _ => vec![],
    };

    raw.into_iter().enumerate().map(|(i, (pat, desc))| {
        PatternDef { pattern: pat.to_string(), description: desc.to_string(), priority: i as u32 }
    }).collect()
}

// ── Tests ──────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn function_patterns_rust() {
        let p = patterns_for("function", "rust");
        assert!(p.len() >= 2, "Rust should have with/without return type");
        assert!(p[0].pattern.contains("$NAME"));
        assert!(p.iter().any(|p| p.pattern.contains("-> $RET")));
    }

    #[test]
    fn function_patterns_python() {
        let p = patterns_for("function", "python");
        assert_eq!(p.len(), 1);
        assert!(p[0].pattern.contains("def"));
    }

    #[test]
    fn function_patterns_typescript() {
        let p = patterns_for("function", "typescript");
        assert_eq!(p.len(), 1);
        assert!(p[0].pattern.contains("function"));
    }

    #[test]
    fn class_patterns_rust() {
        let p = patterns_for("class", "rust");
        assert!(p[0].pattern.contains("struct"));
    }

    #[test]
    fn class_patterns_python() {
        let p = patterns_for("class", "python");
        assert!(p[0].pattern.contains("class"));
    }

    #[test]
    fn call_patterns_universal() {
        let langs = ["rust", "python", "javascript", "typescript", "go", "java"];
        for lang in &langs {
            let p = patterns_for("call", lang);
            assert!(!p.is_empty(), "{} should have call patterns", lang);
            assert!(p.iter().any(|pat| pat.pattern.contains("$METHOD")));
        }
    }

    #[test]
    fn import_patterns_per_language() {
        let rust = patterns_for("import", "rust");
        assert!(rust.iter().any(|p| p.pattern.contains("use")));

        let py = patterns_for("import", "python");
        assert!(py.iter().any(|p| p.pattern.contains("import")));

        let ts = patterns_for("import", "typescript");
        assert!(ts.iter().any(|p| p.pattern.contains("import")));
    }

    #[test]
    fn relationship_patterns_typescript() {
        let p = patterns_for("extends", "typescript");
        assert!(!p.is_empty());
        assert!(p[0].pattern.contains("extends"));
    }

    #[test]
    fn relationship_patterns_rust_empty() {
        let p = patterns_for("extends", "rust");
        assert!(p.is_empty(), "Rust has no extends patterns");
    }

    #[test]
    fn interface_patterns_rust_is_trait() {
        let p = patterns_for("trait", "rust");
        assert_eq!(p.len(), 1);
        assert!(p[0].pattern.contains("trait"));
    }

    #[test]
    fn unknown_category_returns_empty() {
        let p = patterns_for("nonexistent", "rust");
        assert!(p.is_empty());
    }

    #[test]
    fn pattern_metadata() {
        let p = patterns_for("function", "rust");
        for pat in &p {
            assert!(!pat.description.is_empty());
            assert!(!pat.pattern.is_empty());
        }
    }
}
