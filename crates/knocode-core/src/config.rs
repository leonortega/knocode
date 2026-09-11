use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tracing::info;

use crate::error::{ConfigError, Result};

// ── Top-level Config ────────────────────────────────────────────────────

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub daemon: DaemonConfig,
    pub database: DatabaseConfig,
    pub index: IndexConfig,
    pub knowledge: KnowledgeConfig,
    pub context: ContextConfig,
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct DaemonConfig {}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DatabaseConfig {
    pub path: String,
    pub max_connections: usize,
}

/// Auto-reindex watch mode — single source of truth shared by config, CLI and daemon.
/// Possible values: `commit` (default) or `filesystem` (aliases `git`/`fs` accepted).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum WatchMode {
    /// Re-index only when a new git commit lands on the current HEAD.
    #[serde(alias = "git")]
    #[default]
    Commit,
    /// Re-index on any repository file change (debounced, via `notify`).
    #[serde(alias = "fs")]
    Filesystem,
}

impl std::fmt::Display for WatchMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Commit => write!(f, "commit"),
            Self::Filesystem => write!(f, "filesystem"),
        }
    }
}

impl std::str::FromStr for WatchMode {
    type Err = String;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "commit" | "git" => Ok(Self::Commit),
            "filesystem" | "fs" => Ok(Self::Filesystem),
            _ => Err(format!(
                "unknown watch mode '{}': expected 'commit' or 'filesystem'",
                s
            )),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IndexConfig {
    pub path: String,
    pub languages: Vec<String>,
    /// Auto-reindex watch mode: `commit` (default) or `filesystem`.
    pub watch_mode: WatchMode,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KnowledgeConfig {
    pub max_knowledge_entries: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ContextConfig {
    pub max_tokens: usize,
    pub max_files: usize,
    pub max_lines_per_file: usize,
    pub cache_order: Vec<String>,
    /// Candidate pool size before deterministic ranking (P1 sweep 20/50/100/200, default 100 → Top 20)
    /// Env KNOCODE_CANDIDATE_K overrides; CLI --candidate-k overrides config.
    pub candidate_k: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub file_path: String,
    pub max_size_mb: usize,
    pub retention_days: usize,
}

// ── Defaults ────────────────────────────────────────────────────────────

// Default is derived since all fields implement Default


impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            path: "~/.knocode/data.db".to_string(),
            max_connections: 5,
        }
    }
}

impl Default for IndexConfig {
    fn default() -> Self {
        // Default 4 langs; 371 languages available via tree-sitter-language-pack (no feature gate needed)
        Self {
            path: "~/.knocode/index/".to_string(),
            languages: vec![
                "rust".to_string(),
                "typescript".to_string(),
                "javascript".to_string(),
                "python".to_string(),
            ],
            watch_mode: WatchMode::Commit,
        }
    }
}

impl Default for KnowledgeConfig {
    fn default() -> Self {
        Self {
            max_knowledge_entries: 10000,
        }
    }
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            max_tokens: 12000,
            max_files: 20,
            max_lines_per_file: 500,
            cache_order: vec![
                "docs_context".to_string(),
                "code_context".to_string(),
            ],
            candidate_k: 100,
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            file_path: "~/.knocode/logs/knocode.log".to_string(),
            max_size_mb: 100,
            retention_days: 7,
        }
    }
}

// ── Config Loading ──────────────────────────────────────────────────────

impl Config {
    /// Load configuration by merging user, project, and environment configs.
    ///
    /// Priority (highest wins): environment > project > user > defaults
    pub fn load(project_root: &Path) -> Result<Self> {
        let mut config = Self::default();

        // 1. Load user config (~/.config/knocode/config.toml)
        let user_config_path = Self::user_config_path();
        if user_config_path.exists() {
            info!(path = %user_config_path.display(), "Loading user config");
            let user_config = Self::from_file(&user_config_path)?;
            config.merge(user_config);
        }

        // 2. Load project config (.knocode/config.toml)
        let project_config_path = project_root.join(".knocode/config.toml");
        if project_config_path.exists() {
            info!(path = %project_config_path.display(), "Loading project config");
            let project_config = Self::from_file(&project_config_path)?;
            config.merge(project_config);
        }

        // 3. Apply environment variable overrides
        config.apply_env_overrides();

        Ok(config)
    }

    /// Load config from a TOML file
    pub fn from_file(path: &Path) -> Result<Self> {
        let content = std::fs::read_to_string(path).map_err(|e| {
            ConfigError::FileReadError {
                path: path.display().to_string(),
                source: e,
            }
        })?;
        Self::from_toml(&content)
    }

    /// Load config from a TOML string
    pub fn from_toml(content: &str) -> Result<Self> {
        toml::from_str(content).map_err(|e| ConfigError::ParseError(e.to_string()).into())
    }

    /// Get the user config path (~/.config/knocode/config.toml)
    fn user_config_path() -> PathBuf {
        if let Some(home) = dirs() {
            home.join(".config").join("knocode").join("config.toml")
        } else {
            PathBuf::from("~/.config/knocode/config.toml")
        }
    }

    /// Merge another config into this one (non-default values override)
    pub fn merge(&mut self, other: Config) {
        self.daemon = other.daemon;
        self.database = other.database;
        self.index = other.index;
        self.knowledge = other.knowledge;
        self.context = other.context;
        self.logging = other.logging;
    }

    /// Apply environment variable overrides (KNOCODE_*)
    pub fn apply_env_overrides(&mut self) {
        if let Ok(val) = std::env::var("KNOCODE_DATABASE_PATH") {
            self.database.path = val;
        }
        if let Ok(val) = std::env::var("KNOCODE_LOG_LEVEL") {
            self.logging.level = val;
        }
        if let Ok(val) = std::env::var("KNOCODE_CONTEXT_MAX_TOKENS") {
            if let Ok(n) = val.parse() {
                self.context.max_tokens = n;
            }
        }
        if let Ok(val) = std::env::var("KNOCODE_CANDIDATE_K") {
            if let Ok(n) = val.parse() {
                self.context.candidate_k = n;
            }
        }
        if let Ok(val) = std::env::var("KNOCODE_MAX_FILES") {
            if let Ok(n) = val.parse() {
                self.context.max_files = n;
            }
        }
        if let Ok(val) = std::env::var("KNOCODE_WATCH_MODE") {
            match val.parse::<WatchMode>() {
                Ok(m) => self.index.watch_mode = m,
                Err(e) => tracing::warn!(value = %val, error = %e, "Ignoring invalid KNOCODE_WATCH_MODE"),
            }
        }
        // KNOCODE_LITELLM_URL removed with LiteLLM — see LLM_ROUTING_REMOVAL.md
    }

    /// Validate the configuration
    pub fn validate(&self) -> Result<()> {


        // Database
        if self.database.max_connections == 0 {
            return Err(ConfigError::InvalidValue {
                field: "database.max_connections".to_string(),
                message: "must be greater than 0".to_string(),
            }
            .into());
        }

        // Context
        if self.context.max_tokens == 0 {
            return Err(ConfigError::InvalidValue {
                field: "context.max_tokens".to_string(),
                message: "must be greater than 0".to_string(),
            }
            .into());
        }
        if self.context.max_files == 0 {
            return Err(ConfigError::InvalidValue {
                field: "context.max_files".to_string(),
                message: "must be greater than 0".to_string(),
            }
            .into());
        }
        if self.context.max_lines_per_file == 0 {
            return Err(ConfigError::InvalidValue {
                field: "context.max_lines_per_file".to_string(),
                message: "must be greater than 0".to_string(),
            }
            .into());
        }
        if self.context.candidate_k == 0 {
            return Err(ConfigError::InvalidValue {
                field: "context.candidate_k".to_string(),
                message: "must be greater than 0".to_string(),
            }
            .into());
        }

        // Logging
        let valid_levels = ["error", "warn", "info", "debug", "trace"];
        if !valid_levels.contains(&self.logging.level.as_str()) {
            return Err(ConfigError::InvalidValue {
                field: "logging.level".to_string(),
                message: format!(
                    "must be one of: {}",
                    valid_levels.join(", ")
                ),
            }
            .into());
        }

        Ok(())
    }

    /// Show the effective configuration as TOML string
    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|e| ConfigError::SerializeError(e.to_string()).into())
    }
}

/// Get the user's home directory
fn dirs() -> Option<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        std::env::var("USERPROFILE")
            .ok()
            .map(PathBuf::from)
    }
    #[cfg(not(target_os = "windows"))]
    {
        std::env::var("HOME").ok().map(PathBuf::from)
    }
}

// ── Tests ───────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_config_is_valid() {
        let config = Config::default();
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_from_str() {
        let toml = r#"
[database]
path = "/tmp/test.db"
max_connections = 2

[index]
path = "/tmp/index"
languages = ["rust", "python"]

[knowledge]
max_knowledge_entries = 5000

[context]
max_tokens = 8000
max_files = 10
max_lines_per_file = 200
cache_order = ["code_context"]

[logging]
level = "debug"
file_path = "/tmp/test.log"
max_size_mb = 50
retention_days = 3
"#;
        let config = Config::from_toml(toml).unwrap();
        assert_eq!(config.database.path, "/tmp/test.db");
        assert_eq!(config.context.max_tokens, 8000);
        assert!(config.validate().is_ok());
    }

    #[test]
    fn test_config_merge() {
        let mut base = Config::default();
        let mut override_config = Config::default();
        override_config.context.max_tokens = 5000;

        base.merge(override_config);
        assert_eq!(base.context.max_tokens, 5000);
    }

    #[test]
    fn test_validation_fails_invalid_log_level() {
        let mut config = Config::default();
        config.logging.level = "verbose".to_string();
        let err = config.validate().unwrap_err();
        assert!(err.to_string().contains("logging.level"));
    }

    #[test]
    fn test_config_roundtrip() {
        let config = Config::default();
        let toml = config.to_toml().unwrap();
        let loaded = Config::from_toml(&toml).unwrap();
        assert_eq!(config.context.max_tokens, loaded.context.max_tokens);
    }

    #[test]
    fn test_watch_mode_default_is_commit() {
        assert_eq!(WatchMode::default(), WatchMode::Commit);
        let config = Config::default();
        assert_eq!(config.index.watch_mode, WatchMode::Commit);
        // Serializes back to the canonical "commit" string
        let toml = config.to_toml().unwrap();
        assert!(toml.contains("watch_mode = \"commit\""));
    }

    #[test]
    fn test_watch_mode_parsing_and_aliases() {
        assert_eq!("commit".parse::<WatchMode>().unwrap(), WatchMode::Commit);
        assert_eq!("git".parse::<WatchMode>().unwrap(), WatchMode::Commit);
        assert_eq!("filesystem".parse::<WatchMode>().unwrap(), WatchMode::Filesystem);
        assert_eq!("fs".parse::<WatchMode>().unwrap(), WatchMode::Filesystem);
        assert!("invalid".parse::<WatchMode>().is_err());
        assert_eq!(WatchMode::Commit.to_string(), "commit");
        assert_eq!(WatchMode::Filesystem.to_string(), "filesystem");
    }

    #[test]
    fn test_watch_mode_serde_from_toml() {
        let config = Config::from_toml("[index]\nwatch_mode = \"filesystem\"\n").unwrap();
        assert_eq!(config.index.watch_mode, WatchMode::Filesystem);
        // Old configs without the key fall back to the default
        let config = Config::from_toml("[index]\nlanguages = [\"rust\"]\n").unwrap();
        assert_eq!(config.index.watch_mode, WatchMode::Commit);
    }

    #[test]
    fn test_partial_toml_uses_defaults() {
        let toml = r#"
[database]
path = "/tmp/test.db"
"#;
        let config = Config::from_toml(toml).unwrap();
        assert_eq!(config.database.path, "/tmp/test.db");
        assert_eq!(config.context.max_tokens, 12000);
    }

    #[test]
    fn test_env_overrides() {
        // Save and restore env
        let original_log = std::env::var("KNOCODE_LOG_LEVEL").ok();
        let original_watch = std::env::var("KNOCODE_WATCH_MODE").ok();

        std::env::set_var("KNOCODE_LOG_LEVEL", "trace");
        std::env::set_var("KNOCODE_WATCH_MODE", "filesystem");

        let mut config = Config::default();
        config.apply_env_overrides();

        assert_eq!(config.logging.level, "trace");
        assert_eq!(config.index.watch_mode, WatchMode::Filesystem);

        // Invalid values are ignored (keep default)
        std::env::set_var("KNOCODE_WATCH_MODE", "bogus");
        config.apply_env_overrides();
        assert_eq!(config.index.watch_mode, WatchMode::Filesystem);

        // Restore
        match original_log {
            Some(v) => std::env::set_var("KNOCODE_LOG_LEVEL", v),
            None => std::env::remove_var("KNOCODE_LOG_LEVEL"),
        }
        match original_watch {
            Some(v) => std::env::set_var("KNOCODE_WATCH_MODE", v),
            None => std::env::remove_var("KNOCODE_WATCH_MODE"),
        }
    }

    #[test]
    fn test_v060_defaults_index_four_languages() {
        let config = Config::default();
        assert_eq!(config.index.languages.len(), 4);
        assert_eq!(config.index.languages, vec!["rust", "typescript", "javascript", "python"]);
        // With tree-sitter-language-pack, all languages are available — no feature gate needed
        let mut with_all_langs = Config::default();
        with_all_langs.index.languages.push("go".to_string());
        with_all_langs.index.languages.push("java".to_string());
        with_all_langs.index.languages.push("kotlin".to_string());
        with_all_langs.index.languages.push("swift".to_string());
        assert!(with_all_langs.validate().is_ok(), "all languages should be valid with tree-sitter-language-pack");
    }
}
