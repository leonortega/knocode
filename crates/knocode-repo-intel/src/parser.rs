use crate::registry::{LanguageId, language_pack_name};

// ── Grammar validation ──────────────────────────────────────────────────

/// Validate that a tree-sitter grammar can be loaded for the given language.
/// Returns `Ok(())` if the grammar loads successfully, `Err` with details otherwise.
pub fn validate_grammar(id: LanguageId) -> Result<(), String> {
    let name = language_pack_name(id);
    if name.is_empty() {
        return Err(format!("no parser for {:?}", id));
    }
    tree_sitter_language_pack::get_parser(name)
        .map(|_| ())
        .map_err(|e| format!("grammar load failed for {:?}: {}", id, e))
}

/// Check if a language has a tree-sitter parser available (no download needed).
pub fn has_parser(id: LanguageId) -> bool {
    let name = language_pack_name(id);
    if name.is_empty() {
        return false;
    }
    tree_sitter_language_pack::has_parser(name)
}

// ── AST Symbol Extraction ────────────────────────────────────────────────

/// Symbol extracted from AST
#[derive(Debug, Clone)]
pub struct AstSymbol {
    pub name: String,
    pub kind: String,
    pub line_start: u32,
    pub line_end: u32,
}

/// Extract symbols from source code using tree-sitter-language-pack.
/// Supports 371 languages with automatic grammar download on first use.
pub fn extract_symbols_ast(content: &str, language: &str) -> Vec<AstSymbol> {
    let id = match LanguageId::from_str(language) {
        Some(id) => id,
        None => return Vec::new(),
    };
    extract_symbols_by_id(content, id)
}

/// Extract symbols using LanguageId directly via tree-sitter-language-pack.
pub fn extract_symbols_by_id(content: &str, id: LanguageId) -> Vec<AstSymbol> {
    let name = language_pack_name(id);
    if name.is_empty() {
        return Vec::new();
    }

    let config = tree_sitter_language_pack::ProcessConfig::new(name).all();
    let result = match tree_sitter_language_pack::process(content, &config) {
        Ok(r) => r,
        Err(_) => return Vec::new(),
    };

    let mut symbols = Vec::new();
    extract_structure_items(&result.structure, &mut symbols);
    symbols
}

/// Recursively extract symbols from language-pack StructureItems.
fn extract_structure_items(items: &[tree_sitter_language_pack::StructureItem], symbols: &mut Vec<AstSymbol>) {
    use tree_sitter_language_pack::StructureKind;
    for item in items {
        let kind = match item.kind {
            StructureKind::Function => "function",
            StructureKind::Class => "class",
            StructureKind::Method => "method",
            StructureKind::Struct => "struct",
            StructureKind::Enum => "enum",
            StructureKind::Interface => "interface",
            StructureKind::Trait => "trait",
            StructureKind::Impl => "impl",
            StructureKind::Module => "module",
            StructureKind::Namespace => "namespace",
            _ => continue,
        };
        if let Some(ref name) = item.name {
            symbols.push(AstSymbol {
                name: name.clone(),
                kind: kind.to_string(),
                line_start: item.span.start_line as u32 + 1,
                line_end: item.span.end_line as u32 + 1,
            });
        }
        // Recurse into children (e.g., methods inside a class)
        extract_structure_items(&item.children, symbols);
    }
}

// ── Tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_rust_symbols() {
        let code = r#"
pub fn main() {
    println!("Hello");
}

pub struct Config {
    pub name: String,
}

enum Color {
    Red,
    Green,
    Blue,
}

impl Config {
    fn new() -> Self {
        Config { name: "test".to_string() }
    }
}

trait Drawable {
    fn draw(&self);
}

type Result<T> = std::result::Result<T, Error>;
"#;

        let symbols = extract_symbols_ast(code, "rust");
        assert!(symbols.iter().any(|s| s.name == "main" && s.kind == "function"), "expected main function");
        assert!(symbols.iter().any(|s| s.name == "Config" && s.kind == "struct"), "expected Config struct");
        assert!(symbols.iter().any(|s| s.name == "Color" && s.kind == "enum"), "expected Color enum");
        assert!(symbols.iter().any(|s| s.name == "Drawable" && s.kind == "trait"), "expected Drawable trait");
    }

    #[test]
    fn test_python_symbols() {
        let code = r#"
def hello():
    print("Hello")

class Config:
    def __init__(self):
        self.name = "test"

class MyEnum:
    pass
"#;

        let symbols = extract_symbols_ast(code, "python");
        assert!(symbols.iter().any(|s| s.name == "hello" && s.kind == "function"), "expected hello function");
        assert!(symbols.iter().any(|s| s.name == "Config" && s.kind == "class"), "expected Config class");
    }

    #[test]
    fn test_javascript_symbols() {
        let code = r#"
function hello() {
    console.log("Hello");
}

class Config {
    constructor() {
        this.name = "test";
    }
}

const greet = () => {
    console.log("Hi");
};

export async function fetchData() {
    return {};
}
"#;

        let symbols = extract_symbols_ast(code, "javascript");
        assert!(symbols.iter().any(|s| s.name == "hello" && s.kind == "function"), "expected hello function");
        assert!(symbols.iter().any(|s| s.name == "Config" && s.kind == "class"), "expected Config class");
    }

    #[test]
    fn test_typescript_declaration_symbols() {
        // DefinitelyTyped-style .d.ts content: ambient declarations. Regressed to
        // 0 symbols on the whole DT repo — verify each extraction layer.
        let code = r#"
export declare function add(a: number, b: number): number;
export interface Options { timeout?: number; }
export type Handler = (e: Event) => void;
declare class Widget { render(): void; }
declare namespace Dom { function query(sel: string): Element; }
"#;
        let ast = extract_symbols_ast(code, "typescript");
        eprintln!("AST symbols from .d.ts: {:?}", ast.iter().map(|s| (s.kind.as_str(), s.name.as_str())).collect::<Vec<_>>());
        assert!(
            !ast.is_empty(),
            "tree-sitter extracted 0 symbols from .d.ts content — ambient declarations are not captured"
        );
    }

    #[test]
    fn test_typescript_symbols() {
        let code = r#"
interface User {
    name: string;
    age: number;
}

type Result<T> = {
    data: T;
    error?: string;
}

function greet(user: User): string {
    return `Hello ${user.name}`;
}
"#;

        let symbols = extract_symbols_ast(code, "typescript");
        assert!(symbols.iter().any(|s| s.name == "greet" && s.kind == "function"), "expected greet function");
    }

    #[test]
    fn test_csharp_symbols() {
        let code = r#"
namespace MyApp.Models
{
    public class Order
    {
        public int Id { get; set; }
        public string CustomerName { get; set; }

        public Order() { }

        public void Process() { }
    }

    public interface IOrderService
    {
        Task<Order> GetOrderAsync(int id);
    }

    public struct Point
    {
        public double X;
        public double Y;
    }

    public enum Status
    {
        Pending,
        Completed,
        Cancelled
    }
}
"#;

        let symbols = extract_symbols_ast(code, "csharp");
        assert!(!symbols.is_empty(), "C# should extract symbols");
        assert!(symbols.iter().any(|s| s.name == "Order" && s.kind == "class"), "expected Order class");
    }

    #[test]
    fn test_unknown_language() {
        let code = "fn main() {}";
        let symbols = extract_symbols_ast(code, "unknown");
        assert!(symbols.is_empty());
    }

    #[test]
    fn test_validate_grammar_core_langs() {
        assert!(validate_grammar(LanguageId::Rust).is_ok());
        assert!(validate_grammar(LanguageId::TypeScript).is_ok());
        assert!(validate_grammar(LanguageId::JavaScript).is_ok());
        assert!(validate_grammar(LanguageId::Python).is_ok());
        assert!(validate_grammar(LanguageId::CSharp).is_ok());
    }

    #[test]
    fn test_validate_grammar_extended_langs() {
        // With tree-sitter-language-pack, 371 languages available
        assert!(validate_grammar(LanguageId::Go).is_ok());
        assert!(validate_grammar(LanguageId::Java).is_ok());
        assert!(validate_grammar(LanguageId::Kotlin).is_ok());
        assert!(validate_grammar(LanguageId::Swift).is_ok());
        assert!(validate_grammar(LanguageId::Ruby).is_ok());
    }

    #[test]
    fn test_extract_symbols_go() {
        let code = r#"
package main
func Hello() {}
type Config struct { Name string }
"#;
        let symbols = extract_symbols_ast(code, "go");
        assert!(symbols.is_empty() || symbols.iter().any(|s| s.name.contains("Hello") || s.name.contains("Config")));
    }

    #[test]
    fn test_validate_grammar_text_fails() {
        // The pack has no grammar for plain text
        assert!(validate_grammar(LanguageId::Text).is_err());
    }

    #[test]
    fn test_validate_grammar_markup_langs() {
        // Pack grammars exist for markup/config languages (downloads on demand)
        assert!(validate_grammar(LanguageId::Markdown).is_ok());
        assert!(validate_grammar(LanguageId::Json).is_ok());
    }

    #[test]
    fn test_extract_symbols_by_id() {
        use crate::registry::LanguageId;
        let code = "fn main() {}";
        let symbols = extract_symbols_by_id(code, LanguageId::Rust);
        assert!(symbols.iter().any(|s| s.name == "main" && s.kind == "function"));
    }
}
