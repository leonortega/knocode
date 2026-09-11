//! Structural search — ast-grep pattern matching backed by tree-sitter-language-pack.
//!
//! ## Architecture
//!
//! ```text
//! tree-sitter-language-pack (single grammar source)
//!         │
//!    Knocode Language Registry
//!         │
//!    ┌────┴────┐
//!    ▼         ▼
//! Symbol      TsLangAdapter
//! Extraction  (this module)
//!     │             │
//!     │             ▼
//!     │      ast-grep-core
//!     │             │
//!     │             ▼
//!     │    Structural Matching
//!     │             │
//!     └──────┬──────┘
//!            ▼
//!         Evidence
//! ```
//!
//! **One grammar source. Two consumers.**
//!
//! The `TsLangAdapter` wraps a `tree_sitter::Language` from `tree-sitter-language-pack`
//! and implements ast-grep's `Language` + `LanguageExt` traits. This avoids grammar
//! duplication, version drift, and binary size bloat.

pub mod ast_grep_adapter;
pub mod backend;
pub mod patterns;

pub use ast_grep_adapter::{TsLangAdapter, create_adapter, cached_adapter, clear_adapter_cache, adapter_cache_size, ext_to_lang_pack_name, lang_patterns_for};
pub use backend::{AstGrepBackend, AstMatch, AstSearchError, AstSearchResult};
pub use patterns::{PatternDef, patterns_for};
