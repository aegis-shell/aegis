use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

const DEFAULT_MODEL: &str = "claude-sonnet-4-5";
const DEFAULT_MAX_TOKENS: u32 = 8192;
const DEFAULT_MAX_TURNS: u32 = 32;
const DEFAULT_ANTHROPIC_BASE_URL: &str = "https://api.anthropic.com";
const DEFAULT_OPENAI_BASE_URL: &str = "https://api.openai.com/v1";

/// fuji product configuration loaded from `$XDG_CONFIG_HOME/fuji/config.toml`.
///
/// Every section is optional; a missing file yields the documented defaults.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FujiConfig {
    pub provider: ProviderConfig,
    pub agent: AgentConfig,
    pub permissions: PermissionsConfig,
    pub mcp: BTreeMap<String, McpServerConfig>,
    pub skills: SkillsConfig,
}

impl FujiConfig {
    /// Configuration file path: `$FUJI_CONFIG`, else
    /// `$XDG_CONFIG_HOME/fuji/config.toml`.
    pub fn path() -> PathBuf {
        if let Some(path) = std::env::var_os("FUJI_CONFIG")
            && !path.is_empty()
        {
            return PathBuf::from(path);
        }
        dirs::config_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("fuji")
            .join("config.toml")
    }

    /// Session storage root: `$XDG_DATA_HOME/fuji`.
    pub fn data_dir() -> PathBuf {
        dirs::data_dir()
            .unwrap_or_else(|| PathBuf::from("."))
            .join("fuji")
    }

    /// Load [`FujiConfig::path`]; a missing file is not an error.
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(&Self::path())
    }

    pub fn load_from(path: &Path) -> Result<Self, ConfigError> {
        let text = match std::fs::read_to_string(path) {
            Ok(text) => text,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default());
            }
            Err(source) => {
                return Err(ConfigError::Read {
                    path: path.to_path_buf(),
                    source,
                });
            }
        };
        toml::from_str(&text).map_err(|source| ConfigError::Parse {
            path: path.to_path_buf(),
            source,
        })
    }
}

/// Model provider selection and request budget.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub kind: ProviderKind,
    pub model: String,
    pub api_key_env: Option<String>,
    pub base_url: Option<String>,
    pub max_tokens: u32,
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            kind: ProviderKind::default(),
            model: DEFAULT_MODEL.to_string(),
            api_key_env: None,
            base_url: None,
            max_tokens: DEFAULT_MAX_TOKENS,
        }
    }
}

impl ProviderConfig {
    /// Fill in per-kind defaults for the credential variable and endpoint.
    pub fn resolve(&self) -> ResolvedProvider {
        let (api_key_env, base_url) = match self.kind {
            ProviderKind::Anthropic => ("ANTHROPIC_API_KEY", DEFAULT_ANTHROPIC_BASE_URL),
            ProviderKind::OpenAiCompatible => ("OPENAI_API_KEY", DEFAULT_OPENAI_BASE_URL),
        };
        ResolvedProvider {
            kind: self.kind,
            model: self.model.clone(),
            api_key_env: self
                .api_key_env
                .clone()
                .unwrap_or_else(|| api_key_env.to_string()),
            base_url: self
                .base_url
                .clone()
                .unwrap_or_else(|| base_url.to_string()),
            max_tokens: self.max_tokens,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
pub enum ProviderKind {
    #[default]
    #[serde(rename = "anthropic")]
    Anthropic,
    #[serde(rename = "openai-compatible")]
    OpenAiCompatible,
}

/// Provider settings with every default applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedProvider {
    pub kind: ProviderKind,
    pub model: String,
    pub api_key_env: String,
    pub base_url: String,
    pub max_tokens: u32,
}

impl ResolvedProvider {
    /// Read the credential from the resolved environment variable.
    pub fn api_key(&self) -> Option<String> {
        std::env::var(&self.api_key_env)
            .ok()
            .filter(|value| !value.trim().is_empty())
    }
}

/// Agent loop limits and prompt customization.
#[derive(Debug, Clone, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub max_turns: u32,
    pub system_prompt_append: Option<String>,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: DEFAULT_MAX_TURNS,
            system_prompt_append: None,
        }
    }
}

/// One permission decision for a tool invocation.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PermissionMode {
    Allow,
    #[default]
    Ask,
    Deny,
}

/// Per-tool permission policy. Tools without an exact entry use `default`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct PermissionsConfig {
    pub default: PermissionMode,
    #[serde(flatten)]
    pub tools: BTreeMap<String, PermissionMode>,
}

impl PermissionsConfig {
    pub fn mode_for(&self, tool: &str) -> PermissionMode {
        self.tools.get(tool).copied().unwrap_or(self.default)
    }
}

/// One stdio MCP server entry from `[mcp.<name>]`.
#[derive(Debug, Clone, Deserialize)]
pub struct McpServerConfig {
    pub command: Vec<String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub read_only: bool,
    #[serde(default)]
    pub environment: BTreeMap<String, String>,
}

fn default_enabled() -> bool {
    true
}

/// Skill discovery roots; each immediate child may hold a `SKILL.md`.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct SkillsConfig {
    pub paths: Vec<PathBuf>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("cannot read config {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("cannot parse config {path}: {source}")]
    Parse {
        path: PathBuf,
        source: toml::de::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_sections_fall_back_to_documented_defaults() {
        let config: FujiConfig = toml::from_str("").expect("empty config");
        assert_eq!(config.provider.kind, ProviderKind::Anthropic);
        assert_eq!(config.provider.model, DEFAULT_MODEL);
        assert_eq!(config.provider.max_tokens, DEFAULT_MAX_TOKENS);
        assert_eq!(config.agent.max_turns, DEFAULT_MAX_TURNS);
        assert_eq!(config.permissions.default, PermissionMode::Ask);
        assert!(config.mcp.is_empty());
        assert!(config.skills.paths.is_empty());
    }

    #[test]
    fn full_config_parses() {
        let text = r#"
[provider]
kind = "openai-compatible"
model = "deepseek-chat"
api_key_env = "DEEPSEEK_API_KEY"
base_url = "https://api.deepseek.com/v1"
max_tokens = 4096

[agent]
max_turns = 12
system_prompt_append = "Be terse."

[permissions]
default = "deny"
bash = "ask"
"mcp__aegis__realm_input" = "allow"

[mcp.aegis]
command = ["aegis-fuji-mcp"]
environment = { AEGIS_FUJI_SCOPE = "fuji" }

[skills]
paths = ["/opt/skills"]
"#;
        let config: FujiConfig = toml::from_str(text).expect("config");
        assert_eq!(config.provider.kind, ProviderKind::OpenAiCompatible);
        assert_eq!(config.provider.model, "deepseek-chat");
        assert_eq!(config.agent.max_turns, 12);
        assert_eq!(config.permissions.mode_for("bash"), PermissionMode::Ask);
        assert_eq!(
            config.permissions.mode_for("mcp__aegis__realm_input"),
            PermissionMode::Allow
        );
        assert_eq!(config.permissions.mode_for("unknown"), PermissionMode::Deny);
        let aegis = config.mcp.get("aegis").expect("aegis server");
        assert!(aegis.enabled);
        assert!(!aegis.read_only);
        assert_eq!(aegis.command, vec!["aegis-fuji-mcp".to_string()]);
        assert_eq!(
            aegis
                .environment
                .get("AEGIS_FUJI_SCOPE")
                .map(String::as_str),
            Some("fuji")
        );
    }

    #[test]
    fn resolve_applies_per_kind_defaults() {
        let anthropic = ProviderConfig::default().resolve();
        assert_eq!(anthropic.api_key_env, "ANTHROPIC_API_KEY");
        assert_eq!(anthropic.base_url, DEFAULT_ANTHROPIC_BASE_URL);

        let openai = ProviderConfig {
            kind: ProviderKind::OpenAiCompatible,
            ..ProviderConfig::default()
        }
        .resolve();
        assert_eq!(openai.api_key_env, "OPENAI_API_KEY");
        assert_eq!(openai.base_url, DEFAULT_OPENAI_BASE_URL);

        let custom = ProviderConfig {
            kind: ProviderKind::OpenAiCompatible,
            model: "qwen-max".into(),
            api_key_env: Some("QWEN_KEY".into()),
            base_url: Some("https://dashscope.example/v1".into()),
            max_tokens: 1024,
        }
        .resolve();
        assert_eq!(custom.api_key_env, "QWEN_KEY");
        assert_eq!(custom.base_url, "https://dashscope.example/v1");
        assert_eq!(custom.max_tokens, 1024);
    }

    #[test]
    fn load_from_missing_file_yields_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("absent.toml");
        let config = FujiConfig::load_from(&path).expect("defaults");
        assert_eq!(config.provider.model, DEFAULT_MODEL);
        assert_eq!(config.agent.max_turns, DEFAULT_MAX_TURNS);
        assert!(config.mcp.is_empty());
    }

    #[test]
    fn load_from_reports_parse_errors_with_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("broken.toml");
        std::fs::write(&path, "[provider\n").expect("write");
        let error = FujiConfig::load_from(&path).expect_err("parse error");
        assert!(matches!(error, ConfigError::Parse { .. }));
    }
}
