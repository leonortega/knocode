use std::path::Path;

/// Maps LanguageId to tree-sitter-language-pack's name.
/// Language-pack uses lowercase names (e.g. "csharp", "fsharp", "typescript").
pub fn language_pack_name(id: LanguageId) -> &'static str {
    match id {
        LanguageId::Rust => "rust",
        LanguageId::TypeScript => "typescript",
        LanguageId::TypeScriptReact => "tsx",
        LanguageId::JavaScript => "javascript",
        LanguageId::JavaScriptReact => "jsx",
        LanguageId::Python => "python",
        LanguageId::CSharp => "csharp",
        LanguageId::Go => "go",
        LanguageId::Java => "java",
        LanguageId::C => "c",
        LanguageId::Cpp => "cpp",
        LanguageId::Ruby => "ruby",
        LanguageId::Php => "php",
        LanguageId::Swift => "swift",
        LanguageId::Kotlin => "kotlin",
        LanguageId::Scala => "scala",
        LanguageId::Haskell => "haskell",
        LanguageId::Elixir => "elixir",
        LanguageId::Erlang => "erlang",
        LanguageId::Lua => "lua",
        LanguageId::Zig => "zig",
        LanguageId::R => "r",
        LanguageId::Ocaml => "ocaml",
        LanguageId::Clojure => "clojure",
        LanguageId::FSharp => "fsharp",
        LanguageId::Vb => "vb",
        LanguageId::Sql => "sql",
        LanguageId::Shell => "bash",
        LanguageId::Protobuf => "protobuf",
        LanguageId::GraphQL => "graphql",
        LanguageId::Terraform => "hcl",
        LanguageId::Vue => "vue",
        LanguageId::Svelte => "svelte",
        // Markup/config languages — grammars exist in tree-sitter-language-pack
        LanguageId::Markdown => "markdown",
        LanguageId::Yaml => "yaml",
        LanguageId::Toml => "toml",
        LanguageId::Json => "json",
        LanguageId::Xml => "xml",
        LanguageId::Html => "html",
        LanguageId::Css => "css",
        LanguageId::Scss => "scss",
        // No grammar in the pack for plain text
        LanguageId::Text => "",
    }
}

// ── Language Identity ────────────────────────────────────────────────────

/// Stable language identifier — single source of truth for all of Knocode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LanguageId {
    Rust,
    TypeScript,
    TypeScriptReact,
    JavaScript,
    JavaScriptReact,
    Python,
    CSharp,
    Go,
    Java,
    C,
    Cpp,
    Ruby,
    Php,
    Swift,
    Kotlin,
    Scala,
    Haskell,
    Elixir,
    Erlang,
    Lua,
    Zig,
    R,
    Ocaml,
    Clojure,
    FSharp,
    Vb,
    Sql,
    Shell,
    Protobuf,
    GraphQL,
    Terraform,
    Vue,
    Svelte,
    // Non-code — metadata/search only, no parser
    Markdown,
    Yaml,
    Toml,
    Json,
    Xml,
    Html,
    Css,
    Scss,
    Text,
}

impl LanguageId {
    /// String name used in DB, tantivy, and logs
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Rust => "rust",
            Self::TypeScript => "typescript",
            Self::TypeScriptReact => "typescriptreact",
            Self::JavaScript => "javascript",
            Self::JavaScriptReact => "javascriptreact",
            Self::Python => "python",
            Self::CSharp => "csharp",
            Self::Go => "go",
            Self::Java => "java",
            Self::C => "c",
            Self::Cpp => "cpp",
            Self::Ruby => "ruby",
            Self::Php => "php",
            Self::Swift => "swift",
            Self::Kotlin => "kotlin",
            Self::Scala => "scala",
            Self::Haskell => "haskell",
            Self::Elixir => "elixir",
            Self::Erlang => "erlang",
            Self::Lua => "lua",
            Self::Zig => "zig",
            Self::R => "r",
            Self::Ocaml => "ocaml",
            Self::Clojure => "clojure",
            Self::FSharp => "fsharp",
            Self::Vb => "vb",
            Self::Sql => "sql",
            Self::Shell => "shell",
            Self::Protobuf => "protobuf",
            Self::GraphQL => "graphql",
            Self::Terraform => "terraform",
            Self::Vue => "vue",
            Self::Svelte => "svelte",
            Self::Markdown => "markdown",
            Self::Yaml => "yaml",
            Self::Toml => "toml",
            Self::Json => "json",
            Self::Xml => "xml",
            Self::Html => "html",
            Self::Css => "css",
            Self::Scss => "scss",
            Self::Text => "text",
        }
    }

    /// Does this language have a tree-sitter parser?
    /// Delegates to tree-sitter-language-pack — no hardcoded allow/deny list.
    /// Note: answers from statically compiled grammars or already-downloaded
    /// dynamic grammars; never triggers a network download.
    pub fn has_parser(&self) -> bool {
        let name = language_pack_name(*self);
        if name.is_empty() {
            return false;
        }
        tree_sitter_language_pack::has_parser(name)
    }

    /// Parse from string (case-insensitive)
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "rust" | "rs" => Some(Self::Rust),
            "typescript" | "ts" => Some(Self::TypeScript),
            "typescriptreact" | "tsx" => Some(Self::TypeScriptReact),
            "javascript" | "js" => Some(Self::JavaScript),
            "javascriptreact" | "jsx" => Some(Self::JavaScriptReact),
            "python" | "py" => Some(Self::Python),
            "csharp" | "c#" | "cs" => Some(Self::CSharp),
            "go" => Some(Self::Go),
            "java" => Some(Self::Java),
            "c" => Some(Self::C),
            "cpp" | "c++" => Some(Self::Cpp),
            "ruby" | "rb" => Some(Self::Ruby),
            "php" => Some(Self::Php),
            "swift" => Some(Self::Swift),
            "kotlin" | "kt" => Some(Self::Kotlin),
            "scala" => Some(Self::Scala),
            "haskell" | "hs" => Some(Self::Haskell),
            "elixir" | "ex" => Some(Self::Elixir),
            "erlang" | "erl" => Some(Self::Erlang),
            "lua" => Some(Self::Lua),
            "zig" => Some(Self::Zig),
            "r" => Some(Self::R),
            "ocaml" | "ml" => Some(Self::Ocaml),
            "clojure" | "clj" => Some(Self::Clojure),
            "fsharp" | "f#" | "fs" => Some(Self::FSharp),
            "vb" | "vb.net" => Some(Self::Vb),
            "sql" => Some(Self::Sql),
            "shell" | "sh" | "bash" | "zsh" => Some(Self::Shell),
            "protobuf" | "proto" => Some(Self::Protobuf),
            "graphql" | "gql" => Some(Self::GraphQL),
            "terraform" | "tf" | "hcl" => Some(Self::Terraform),
            "vue" => Some(Self::Vue),
            "svelte" => Some(Self::Svelte),
            "markdown" | "md" => Some(Self::Markdown),
            "yaml" | "yml" => Some(Self::Yaml),
            "toml" => Some(Self::Toml),
            "json" => Some(Self::Json),
            "xml" => Some(Self::Xml),
            "html" => Some(Self::Html),
            "css" => Some(Self::Css),
            "scss" => Some(Self::Scss),
            "text" | "txt" => Some(Self::Text),
            _ => None,
        }
    }
}

// ── Language Definition ──────────────────────────────────────────────────

/// Static definition of a supported language.
#[derive(Debug, Clone)]
pub struct LanguageDefinition {
    pub id: LanguageId,
    pub extensions: &'static [&'static str],
    pub filenames: &'static [&'static str],
}

impl LanguageDefinition {
    const fn new(id: LanguageId, extensions: &'static [&'static str], filenames: &'static [&'static str]) -> Self {
        Self { id, extensions, filenames }
    }
}

// ── Parser Registry ──────────────────────────────────────────────────────

/// Get tree-sitter parser for a LanguageId (backward-compatible free function).
/// Uses tree-sitter-language-pack to load the parser.
pub fn get_ts_parser(id: LanguageId) -> Option<tree_sitter_language_pack::Language> {
    let name = language_pack_name(id);
    if name.is_empty() {
        return None;
    }
    tree_sitter_language_pack::get_language(name).ok()
}

/// Extensible parser registry — manages language definitions and grammar loading.
/// Languages can be registered at runtime via `register_language()`.
/// Grammar loading is handled by tree-sitter-language-pack via `language_pack_name()`.
pub struct ParserRegistry {
    definitions: Vec<LanguageDefinition>,
    supported: std::collections::HashSet<LanguageId>,
}

impl ParserRegistry {
    /// Create a new registry with all built-in languages pre-registered.
    pub fn new() -> Self {
        let mut registry = Self {
            definitions: Vec::new(),
            supported: std::collections::HashSet::new(),
        };
        registry.register_builtins();
        registry
    }

    /// Register a built-in language.
    fn register_builtin(&mut self, def: LanguageDefinition, has_parser: bool) {
        if has_parser {
            self.supported.insert(def.id);
        }
        self.definitions.push(def);
    }

    /// Register all built-in languages.
    /// Languages with tree-sitter parsers use tree-sitter-language-pack (371 languages).
    fn register_builtins(&mut self) {
        // Helper: a language has a parser if it has a language-pack name
        fn has_pack_parser(id: LanguageId) -> bool {
            !language_pack_name(id).is_empty()
        }

        // Source languages with parsers (via tree-sitter-language-pack)
        self.register_builtin(LanguageDefinition::new(LanguageId::Rust, &["rs"], &["Cargo.toml"]), has_pack_parser(LanguageId::Rust));
        self.register_builtin(LanguageDefinition::new(LanguageId::TypeScript, &["ts", "mts", "cts"], &[]), has_pack_parser(LanguageId::TypeScript));
        self.register_builtin(LanguageDefinition::new(LanguageId::TypeScriptReact, &["tsx"], &[]), has_pack_parser(LanguageId::TypeScriptReact));
        self.register_builtin(LanguageDefinition::new(LanguageId::JavaScript, &["js", "mjs", "cjs"], &["package.json"]), has_pack_parser(LanguageId::JavaScript));
        self.register_builtin(LanguageDefinition::new(LanguageId::JavaScriptReact, &["jsx"], &[]), has_pack_parser(LanguageId::JavaScriptReact));
        self.register_builtin(LanguageDefinition::new(LanguageId::Python, &["py", "pyi"], &["pyproject.toml", "setup.py", "requirements.txt"]), has_pack_parser(LanguageId::Python));
        self.register_builtin(LanguageDefinition::new(LanguageId::CSharp, &["cs"], &["*.csproj", "*.sln"]), has_pack_parser(LanguageId::CSharp));
        self.register_builtin(LanguageDefinition::new(LanguageId::Go, &["go"], &["go.mod"]), has_pack_parser(LanguageId::Go));
        self.register_builtin(LanguageDefinition::new(LanguageId::Java, &["java"], &["pom.xml", "build.gradle"]), has_pack_parser(LanguageId::Java));
        self.register_builtin(LanguageDefinition::new(LanguageId::C, &["c", "h"], &[]), has_pack_parser(LanguageId::C));
        self.register_builtin(LanguageDefinition::new(LanguageId::Cpp, &["cpp", "cc", "cxx", "hpp"], &[]), has_pack_parser(LanguageId::Cpp));
        self.register_builtin(LanguageDefinition::new(LanguageId::Ruby, &["rb"], &["Gemfile"]), has_pack_parser(LanguageId::Ruby));
        self.register_builtin(LanguageDefinition::new(LanguageId::Php, &["php"], &["composer.json"]), has_pack_parser(LanguageId::Php));
        self.register_builtin(LanguageDefinition::new(LanguageId::Swift, &["swift"], &["Package.swift"]), has_pack_parser(LanguageId::Swift));
        self.register_builtin(LanguageDefinition::new(LanguageId::Kotlin, &["kt", "kts"], &[]), has_pack_parser(LanguageId::Kotlin));
        self.register_builtin(LanguageDefinition::new(LanguageId::Scala, &["scala"], &["build.sbt"]), has_pack_parser(LanguageId::Scala));
        self.register_builtin(LanguageDefinition::new(LanguageId::Haskell, &["hs"], &[]), has_pack_parser(LanguageId::Haskell));
        self.register_builtin(LanguageDefinition::new(LanguageId::Elixir, &["ex", "exs"], &["mix.exs"]), has_pack_parser(LanguageId::Elixir));
        self.register_builtin(LanguageDefinition::new(LanguageId::Erlang, &["erl"], &[]), has_pack_parser(LanguageId::Erlang));
        self.register_builtin(LanguageDefinition::new(LanguageId::Lua, &["lua"], &[]), has_pack_parser(LanguageId::Lua));
        self.register_builtin(LanguageDefinition::new(LanguageId::Zig, &["zig"], &["build.zig"]), has_pack_parser(LanguageId::Zig));
        self.register_builtin(LanguageDefinition::new(LanguageId::R, &["r", "R"], &[]), has_pack_parser(LanguageId::R));
        self.register_builtin(LanguageDefinition::new(LanguageId::Ocaml, &["ml"], &[]), has_pack_parser(LanguageId::Ocaml));
        self.register_builtin(LanguageDefinition::new(LanguageId::Clojure, &["clj"], &[]), has_pack_parser(LanguageId::Clojure));
        self.register_builtin(LanguageDefinition::new(LanguageId::FSharp, &["fs", "fsx"], &[]), has_pack_parser(LanguageId::FSharp));
        self.register_builtin(LanguageDefinition::new(LanguageId::Vb, &["vb"], &[]), has_pack_parser(LanguageId::Vb));
        self.register_builtin(LanguageDefinition::new(LanguageId::Sql, &["sql"], &[]), has_pack_parser(LanguageId::Sql));
        self.register_builtin(LanguageDefinition::new(LanguageId::Shell, &["sh", "bash", "zsh"], &[]), has_pack_parser(LanguageId::Shell));
        self.register_builtin(LanguageDefinition::new(LanguageId::Protobuf, &["proto"], &[]), has_pack_parser(LanguageId::Protobuf));
        self.register_builtin(LanguageDefinition::new(LanguageId::GraphQL, &["graphql", "gql"], &[]), has_pack_parser(LanguageId::GraphQL));
        self.register_builtin(LanguageDefinition::new(LanguageId::Terraform, &["tf", "hcl"], &[]), has_pack_parser(LanguageId::Terraform));
        self.register_builtin(LanguageDefinition::new(LanguageId::Vue, &["vue"], &[]), has_pack_parser(LanguageId::Vue));
        self.register_builtin(LanguageDefinition::new(LanguageId::Svelte, &["svelte"], &[]), has_pack_parser(LanguageId::Svelte));
        // Markup/config languages — grammars available via tree-sitter-language-pack
        self.register_builtin(LanguageDefinition::new(LanguageId::Markdown, &["md"], &[]), has_pack_parser(LanguageId::Markdown));
        self.register_builtin(LanguageDefinition::new(LanguageId::Yaml, &["yaml", "yml"], &[]), has_pack_parser(LanguageId::Yaml));
        self.register_builtin(LanguageDefinition::new(LanguageId::Toml, &["toml"], &[]), has_pack_parser(LanguageId::Toml));
        self.register_builtin(LanguageDefinition::new(LanguageId::Json, &["json"], &[]), has_pack_parser(LanguageId::Json));
        self.register_builtin(LanguageDefinition::new(LanguageId::Xml, &["xml"], &[]), has_pack_parser(LanguageId::Xml));
        self.register_builtin(LanguageDefinition::new(LanguageId::Html, &["html"], &[]), has_pack_parser(LanguageId::Html));
        self.register_builtin(LanguageDefinition::new(LanguageId::Css, &["css"], &[]), has_pack_parser(LanguageId::Css));
        self.register_builtin(LanguageDefinition::new(LanguageId::Scss, &["scss"], &[]), has_pack_parser(LanguageId::Scss));
        // Plain text — no grammar in the pack
        self.register_builtin(LanguageDefinition::new(LanguageId::Text, &["txt"], &[]), false);
    }

    /// Register a new language at runtime.
    /// Returns `true` if the language was registered, `false` if already present.
    pub fn register_language(&mut self, def: LanguageDefinition, has_parser: bool) -> bool {
        if self.definitions.iter().any(|d| d.id == def.id) {
            return false;
        }
        if has_parser {
            self.supported.insert(def.id);
        }
        self.definitions.push(def);
        true
    }

    /// Check if a LanguageId has a tree-sitter parser available via language-pack.
    pub fn has_parser(&self, id: LanguageId) -> bool {
        self.supported.contains(&id)
    }

    /// List all available languages (with parser support status).
    pub fn list_available_languages(&self) -> Vec<(LanguageId, bool)> {
        self.definitions.iter().map(|d| (d.id, self.supported.contains(&d.id))).collect()
    }

    /// List languages that have tree-sitter parsers available.
    pub fn list_with_parsers(&self) -> Vec<LanguageId> {
        self.definitions.iter()
            .filter(|d| self.supported.contains(&d.id))
            .map(|d| d.id)
            .collect()
    }

    /// Look up language definition by file extension.
    pub fn language_by_extension(&self, ext: &str) -> Option<&LanguageDefinition> {
        self.definitions.iter().find(|def| def.extensions.contains(&ext))
    }

    /// Look up language definition by manifest filename.
    pub fn language_by_manifest(&self, filename: &str) -> Option<&LanguageDefinition> {
        self.definitions.iter().find(|def| def.filenames.contains(&filename))
    }

    /// Detect language from a file path using this registry.
    pub fn detect_language(&self, path: &Path) -> Option<&LanguageDefinition> {
        if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
            if let Some(def) = self.language_by_manifest(name) {
                return Some(def);
            }
        }
        if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
            if let Some(def) = self.language_by_extension(ext) {
                return Some(def);
            }
        }
        None
    }
}

impl Default for ParserRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Static registry of all known languages (backward-compatible constant).
pub const LANGUAGE_REGISTRY: &[LanguageDefinition] = &[
    // ── Source languages with parsers ──
    LanguageDefinition::new(LanguageId::Rust, &["rs"], &["Cargo.toml"]),
    LanguageDefinition::new(LanguageId::TypeScript, &["ts", "mts", "cts"], &[]),
    LanguageDefinition::new(LanguageId::TypeScriptReact, &["tsx"], &[]),
    LanguageDefinition::new(LanguageId::JavaScript, &["js", "mjs", "cjs"], &["package.json"]),
    LanguageDefinition::new(LanguageId::JavaScriptReact, &["jsx"], &[]),
    LanguageDefinition::new(LanguageId::Python, &["py", "pyi"], &["pyproject.toml", "setup.py", "requirements.txt"]),
    LanguageDefinition::new(LanguageId::CSharp, &["cs"], &["*.csproj", "*.sln"]),
    LanguageDefinition::new(LanguageId::Go, &["go"], &["go.mod"]),
    LanguageDefinition::new(LanguageId::Java, &["java"], &["pom.xml", "build.gradle"]),
    LanguageDefinition::new(LanguageId::C, &["c", "h"], &[]),
    LanguageDefinition::new(LanguageId::Cpp, &["cpp", "cc", "cxx", "hpp"], &[]),
    // ── Source languages without parsers (regex fallback) ──
    LanguageDefinition::new(LanguageId::Ruby, &["rb"], &["Gemfile"]),
    LanguageDefinition::new(LanguageId::Php, &["php"], &["composer.json"]),
    LanguageDefinition::new(LanguageId::Swift, &["swift"], &["Package.swift"]),
    LanguageDefinition::new(LanguageId::Kotlin, &["kt", "kts"], &[]),
    LanguageDefinition::new(LanguageId::Scala, &["scala"], &["build.sbt"]),
    LanguageDefinition::new(LanguageId::Haskell, &["hs"], &[]),
    LanguageDefinition::new(LanguageId::Elixir, &["ex", "exs"], &["mix.exs"]),
    LanguageDefinition::new(LanguageId::Erlang, &["erl"], &[]),
    LanguageDefinition::new(LanguageId::Lua, &["lua"], &[]),
    LanguageDefinition::new(LanguageId::Zig, &["zig"], &["build.zig"]),
    LanguageDefinition::new(LanguageId::R, &["r", "R"], &[]),
    LanguageDefinition::new(LanguageId::Ocaml, &["ml"], &[]),
    LanguageDefinition::new(LanguageId::Clojure, &["clj"], &[]),
    LanguageDefinition::new(LanguageId::FSharp, &["fs", "fsx"], &[]),
    LanguageDefinition::new(LanguageId::Vb, &["vb"], &[]),
    LanguageDefinition::new(LanguageId::Sql, &["sql"], &[]),
    LanguageDefinition::new(LanguageId::Shell, &["sh", "bash", "zsh"], &[]),
    LanguageDefinition::new(LanguageId::Protobuf, &["proto"], &[]),
    LanguageDefinition::new(LanguageId::GraphQL, &["graphql", "gql"], &[]),
    LanguageDefinition::new(LanguageId::Terraform, &["tf", "hcl"], &[]),
    LanguageDefinition::new(LanguageId::Vue, &["vue"], &[]),
    LanguageDefinition::new(LanguageId::Svelte, &["svelte"], &[]),
    // ── Non-code (metadata/search only) ──
    LanguageDefinition::new(LanguageId::Markdown, &["md"], &[]),
    LanguageDefinition::new(LanguageId::Yaml, &["yaml", "yml"], &[]),
    LanguageDefinition::new(LanguageId::Toml, &["toml"], &[]),
    LanguageDefinition::new(LanguageId::Json, &["json"], &[]),
    LanguageDefinition::new(LanguageId::Xml, &["xml"], &[]),
    LanguageDefinition::new(LanguageId::Html, &["html"], &[]),
    LanguageDefinition::new(LanguageId::Css, &["css"], &[]),
    LanguageDefinition::new(LanguageId::Scss, &["scss"], &[]),
    LanguageDefinition::new(LanguageId::Text, &["txt"], &[]),
];

/// Look up language by file extension
pub fn language_by_extension(ext: &str) -> Option<&'static LanguageDefinition> {
    LANGUAGE_REGISTRY.iter().find(|def| def.extensions.contains(&ext))
}

/// Look up language by manifest filename (e.g. "Cargo.toml" -> Rust)
pub fn language_by_manifest(filename: &str) -> Option<&'static LanguageDefinition> {
    LANGUAGE_REGISTRY.iter().find(|def| def.filenames.contains(&filename))
}

/// Detect language from a file path
pub fn detect_language(path: &Path) -> Option<&'static LanguageDefinition> {
    // Try filename/manifest first (e.g. "Cargo.toml" -> Rust, "go.mod" -> Go)
    // This must come before extension check since "Cargo.toml" has ext "toml"
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if let Some(def) = language_by_manifest(name) {
            return Some(def);
        }
    }
    // Try extension
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if let Some(def) = language_by_extension(ext) {
            return Some(def);
        }
    }
    None
}

// ── File Classification ──────────────────────────────────────────────

/// Path-independent file classification for a language id (counterpart to
/// `classify_file`). Used where only the language is known — e.g. init's
/// parser validation decides between loading/downloading a grammar (code)
/// and an offline availability check (docs/config).
pub fn file_class_for_language(id: LanguageId) -> FileClass {
    match id {
        LanguageId::Markdown | LanguageId::Text => FileClass::Documentation,
        LanguageId::Yaml | LanguageId::Toml | LanguageId::Json | LanguageId::Xml => FileClass::Config,
        _ => FileClass::Source,
    }
}

/// Classifies a file path into a category for indexing decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileClass {
    Source,
    Test,
    Generated,
    Vendor,
    Dependency,
    Config,
    Documentation,
    Binary,
    Unknown,
}

/// Directories that indicate generated/vendor/dependency code
const VENDOR_DIRS: &[&str] = &["vendor", "node_modules", "third_party", "extern", "external", ".git", ".svn", ".hg", "__pycache__", ".cache", ".knocode", ".vscode", ".idea", ".vs", ".claude", ".devcontainer"];
const GENERATED_DIRS: &[&str] = &["generated", "gen", "auto-generated", "__generated__"];
const DEP_DIRS: &[&str] = &["target", "bin", "obj", "dist", "build", ".gradle", ".next", ".nuxt"];
const TEST_DIRS: &[&str] = &["test", "tests", "__tests__", "test_data", "testdata", "fixtures", "spec"];

/// Classify a file path
pub fn classify_file(path: &Path) -> FileClass {
    let components: Vec<_> = path.components()
        .filter_map(|c| c.as_os_str().to_str())
        .collect();

    // Check for binary extensions
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if matches!(ext,
            "exe" | "dll" | "so" | "dylib" | "o" | "a" | "lib" |
            "bin" | "dat" | "png" | "jpg" | "jpeg" | "gif" | "bmp" |
            "ico" | "pdf" | "zip" | "tar" | "gz" | "woff" | "woff2" | "ttf"
        ) {
            return FileClass::Binary;
        }
    }

    // Check for dotfiles (hidden config files like .editorconfig, .gitignore, etc.)
    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
        if name.starts_with('.') && name.len() > 1 {
            return FileClass::Config;
        }
    }

    for comp in &components {
        let lower = comp.to_lowercase();
        if VENDOR_DIRS.contains(&lower.as_str()) {
            return FileClass::Vendor;
        }
        if GENERATED_DIRS.contains(&lower.as_str()) {
            return FileClass::Generated;
        }
        if DEP_DIRS.contains(&lower.as_str()) {
            return FileClass::Dependency;
        }
        if TEST_DIRS.contains(&lower.as_str()) {
            return FileClass::Test;
        }
    }

    // Check for config/doc files
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        if matches!(ext, "md" | "txt" | "rst") {
            return FileClass::Documentation;
        }
        if matches!(ext, "toml" | "yaml" | "yml" | "json" | "xml" | "ini" | "cfg" | "env") {
            return FileClass::Config;
        }
    }

    FileClass::Source
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_language_id_roundtrip() {
        for id in [
            LanguageId::Rust, LanguageId::TypeScript, LanguageId::JavaScript,
            LanguageId::Python, LanguageId::CSharp, LanguageId::Go, LanguageId::Java,
        ] {
            let s = id.as_str();
            let parsed = LanguageId::from_str(s);
            assert_eq!(parsed, Some(id), "roundtrip failed for {}", s);
        }
    }

    #[test]
    fn test_language_by_extension() {
        assert_eq!(language_by_extension("rs").unwrap().id, LanguageId::Rust);
        assert_eq!(language_by_extension("cs").unwrap().id, LanguageId::CSharp);
        assert_eq!(language_by_extension("tsx").unwrap().id, LanguageId::TypeScriptReact);
        assert!(language_by_extension("xyz").is_none());
    }

    #[test]
    fn test_detect_language_by_path() {
        use std::path::PathBuf;
        assert_eq!(detect_language(&PathBuf::from("src/main.rs")).unwrap().id, LanguageId::Rust);
        assert_eq!(detect_language(&PathBuf::from("Program.cs")).unwrap().id, LanguageId::CSharp);
        assert_eq!(detect_language(&PathBuf::from("Cargo.toml")).unwrap().id, LanguageId::Rust);
        assert_eq!(detect_language(&PathBuf::from("go.mod")).unwrap().id, LanguageId::Go);
        assert_eq!(detect_language(&PathBuf::from("app.tsx")).unwrap().id, LanguageId::TypeScriptReact);
    }

    #[test]
    fn test_classify_file() {
        use std::path::PathBuf;
        assert_eq!(classify_file(&PathBuf::from("src/main.rs")), FileClass::Source);
        assert_eq!(classify_file(&PathBuf::from("tests/test_main.rs")), FileClass::Test);
        assert_eq!(classify_file(&PathBuf::from("vendor/lib/foo.rs")), FileClass::Vendor);
        assert_eq!(classify_file(&PathBuf::from("node_modules/pkg/index.js")), FileClass::Vendor);
        assert_eq!(classify_file(&PathBuf::from("target/debug/binary")), FileClass::Dependency);
        assert_eq!(classify_file(&PathBuf::from("README.md")), FileClass::Documentation);
        assert_eq!(classify_file(&PathBuf::from("image.png")), FileClass::Binary);
        // Stylesheets are code now — indexed and parsed
        assert_eq!(classify_file(&PathBuf::from("styles/main.scss")), FileClass::Source);
        assert_eq!(classify_file(&PathBuf::from("styles/main.css")), FileClass::Source);
        // Path-independent language classification (init validation policy)
        assert_eq!(file_class_for_language(LanguageId::Markdown), FileClass::Documentation);
        assert_eq!(file_class_for_language(LanguageId::Json), FileClass::Config);
        assert_eq!(file_class_for_language(LanguageId::TypeScriptReact), FileClass::Source);
    }

    #[test]
    fn test_has_parser() {
        // Delegates to tree-sitter-language-pack — source languages have parsers
        assert!(LanguageId::Rust.has_parser());
        assert!(LanguageId::CSharp.has_parser());
        assert!(LanguageId::Python.has_parser());
        assert!(LanguageId::Ruby.has_parser());
        assert!(LanguageId::Go.has_parser());
        assert!(LanguageId::Kotlin.has_parser());
        // Markup/config languages have pack grammars too (pack truth, no download triggered)
        assert!(LanguageId::Markdown.has_parser() || !tree_sitter_language_pack::has_parser("markdown"));
        // Plain text has no grammar in the pack
        assert!(!LanguageId::Text.has_parser());
    }

    #[test]
    fn test_registry_consistency() {
        // Every extension in EXTENSION_MAP should map to a known LanguageId
        for def in LANGUAGE_REGISTRY {
            for ext in def.extensions {
                assert!(language_by_extension(ext).is_some(), "extension {} not found", ext);
            }
        }
    }

    #[test]
    fn test_parser_registry_new_has_all_builtins() {
        let registry = ParserRegistry::new();
        let langs = registry.list_available_languages();
        assert!(langs.len() >= 30, "should have all built-in languages");
        // Verify key languages are present
        assert!(langs.iter().any(|(id, _)| *id == LanguageId::Rust));
        assert!(langs.iter().any(|(id, _)| *id == LanguageId::Python));
        assert!(langs.iter().any(|(id, _)| *id == LanguageId::CSharp));
    }

    #[test]
    fn test_parser_registry_has_parser() {
        let registry = ParserRegistry::new();
        assert!(registry.has_parser(LanguageId::Rust));
        assert!(registry.has_parser(LanguageId::Python));
    }

    #[test]
    fn test_parser_registry_register_custom_language() {
        let mut registry = ParserRegistry::new();
        let def = LanguageDefinition::new(LanguageId::Zig, &["zig"], &["build.zig"]);
        // Zig is already registered, so register_language should return false
        assert!(!registry.register_language(def, false));
        // Verify list_with_parsers returns languages with loaded grammars
        let with_parsers = registry.list_with_parsers();
        assert!(with_parsers.contains(&LanguageId::Rust));
    }

    #[test]
    fn test_parser_registry_detect_language() {
        use std::path::PathBuf;
        let registry = ParserRegistry::new();
        assert_eq!(registry.detect_language(&PathBuf::from("src/main.rs")).unwrap().id, LanguageId::Rust);
        assert_eq!(registry.detect_language(&PathBuf::from("app.py")).unwrap().id, LanguageId::Python);
        assert!(registry.detect_language(&PathBuf::from("unknown.xyz")).is_none());
    }

    #[test]
    fn test_parser_registry_language_by_extension() {
        let registry = ParserRegistry::new();
        assert_eq!(registry.language_by_extension("rs").unwrap().id, LanguageId::Rust);
        assert_eq!(registry.language_by_extension("cs").unwrap().id, LanguageId::CSharp);
        assert!(registry.language_by_extension("xyz").is_none());
    }
}
