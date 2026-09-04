use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_INTERACTION_DOMAIN_LABEL: &str = "Tessera Agent";
const DEFAULT_IPC_TIMEOUT_SECS: u64 = 5;

/// Runtime policy for one tessera-mcp bridge process.
#[derive(Clone, PartialEq, Eq)]
pub struct BridgeConfig {
    pub socket_path: PathBuf,
    pub runtime_dir: PathBuf,
    /// Display label presented to the compositor when pairing (ADR-0088).
    /// Cosmetic only: it authenticates nothing and the user may rename the
    /// principal at any time.
    pub label: String,
    /// Stable connector installation namespace. Unlike `label`, this value
    /// is never cosmetic: it partitions credentials, recovery locks, and
    /// local Interaction Domain state so unrelated MCP hosts cannot accidentally share an
    /// agent identity.
    pub instance_id: String,
    pub interaction_domain_label: String,
    /// Durable directory for the pairing identity. `None` keeps the
    /// identity session-only, which re-prompts at every start.
    pub data_dir: Option<PathBuf>,
    pub io_timeout: Duration,
    /// Revoke the managed Interaction Domain on a graceful stdio shutdown. A process killed
    /// during connector refresh leaves a recovery record for the successor.
    pub revoke_on_exit: bool,
}

impl fmt::Debug for BridgeConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BridgeConfig")
            .field("socket_path", &self.socket_path)
            .field("runtime_dir", &self.runtime_dir)
            .field("label", &self.label)
            .field("instance_id", &self.instance_id)
            .field("interaction_domain_label", &self.interaction_domain_label)
            .field("data_dir", &self.data_dir)
            .field("io_timeout", &self.io_timeout)
            .field("revoke_on_exit", &self.revoke_on_exit)
            .finish()
    }
}

impl BridgeConfig {
    pub fn new(
        socket_path: impl Into<PathBuf>,
        runtime_dir: impl Into<PathBuf>,
        label: impl Into<String>,
        interaction_domain_label: impl Into<String>,
    ) -> Result<Self, ConfigError> {
        let config = Self {
            socket_path: socket_path.into(),
            runtime_dir: runtime_dir.into(),
            label: label.into(),
            instance_id: "embedded".into(),
            interaction_domain_label: interaction_domain_label.into(),
            data_dir: None,
            io_timeout: Duration::from_secs(DEFAULT_IPC_TIMEOUT_SECS),
            revoke_on_exit: false,
        };
        config.validate()?;
        Ok(config)
    }

    /// Load the bridge configuration from `TESSERA_MCP_*` variables.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    fn from_lookup(mut get: impl FnMut(&str) -> Option<OsString>) -> Result<Self, ConfigError> {
        let runtime_dir = PathBuf::from(required_os(&mut get, "XDG_RUNTIME_DIR")?);
        let socket_path = get("TESSERA_MCP_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|| runtime_dir.join("tessera.sock"));
        let interaction_domain_label =
            optional_string(&mut get, "TESSERA_MCP_INTERACTION_DOMAIN_LABEL")?
                .unwrap_or_else(|| DEFAULT_INTERACTION_DOMAIN_LABEL.to_string());
        let label = optional_string(&mut get, "TESSERA_MCP_LABEL")?
            .unwrap_or_else(|| interaction_domain_label.clone());
        let instance_id = optional_string(&mut get, "TESSERA_MCP_INSTANCE_ID")?
            .ok_or(ConfigError::Missing("TESSERA_MCP_INSTANCE_ID"))?;
        let data_dir = bridge_data_dir(&mut get);
        let io_timeout = Duration::from_secs(parse_number(
            &mut get,
            "TESSERA_MCP_IPC_TIMEOUT_SECS",
            DEFAULT_IPC_TIMEOUT_SECS,
            1,
            60,
        )?);
        let revoke_on_exit = parse_bool(&mut get, "TESSERA_MCP_REVOKE_ON_EXIT", false)?;
        let config = Self {
            socket_path,
            runtime_dir,
            label,
            instance_id,
            interaction_domain_label,
            data_dir,
            io_timeout,
            revoke_on_exit,
        };
        config.validate()?;
        Ok(config)
    }

    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if self.socket_path.as_os_str().is_empty() {
            return Err(invalid("TESSERA_MCP_SOCKET", "path must not be empty"));
        }
        if !self.runtime_dir.is_absolute() {
            return Err(invalid(
                "XDG_RUNTIME_DIR",
                "path must be absolute so Interaction Domain recovery state is private and unambiguous",
            ));
        }
        let label = self.label.trim();
        if label.is_empty() || label.len() > 128 {
            return Err(invalid(
                "TESSERA_MCP_LABEL",
                "length must be from 1 through 128 bytes",
            ));
        }
        let interaction_domain_label = self.interaction_domain_label.trim();
        if interaction_domain_label.is_empty() || interaction_domain_label.len() > 128 {
            return Err(invalid(
                "TESSERA_MCP_INTERACTION_DOMAIN_LABEL",
                "length must be from 1 through 128 bytes",
            ));
        }
        let instance_id = self.instance_id.trim();
        if instance_id.is_empty() || instance_id.len() > 128 {
            return Err(invalid(
                "TESSERA_MCP_INSTANCE_ID",
                "length must be from 1 through 128 bytes",
            ));
        }
        if let Some(data_dir) = &self.data_dir
            && !data_dir.is_absolute()
        {
            return Err(invalid(
                "TESSERA_MCP_DATA_DIR",
                "path must be absolute so the pairing identity is private and unambiguous",
            ));
        }
        if !(Duration::from_secs(1)..=Duration::from_secs(60)).contains(&self.io_timeout) {
            return Err(invalid(
                "TESSERA_MCP_IPC_TIMEOUT_SECS",
                "expected a duration from 1 through 60 seconds",
            ));
        }
        Ok(())
    }

    pub(crate) fn state_dir(&self) -> PathBuf {
        self.runtime_dir.join("tessera-mcp")
    }
}

/// Resolve the durable identity directory: an explicit override, else the
/// XDG data home, else `$HOME/.local/share`, else session-only.
fn bridge_data_dir(get: &mut impl FnMut(&str) -> Option<OsString>) -> Option<PathBuf> {
    if let Some(dir) = get("TESSERA_MCP_DATA_DIR") {
        return Some(PathBuf::from(dir));
    }
    let base = get("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| get("HOME").map(|home| PathBuf::from(home).join(".local/share")));
    base.map(|base| base.join("tessera-mcp"))
}

fn required_os(
    get: &mut impl FnMut(&str) -> Option<OsString>,
    name: &'static str,
) -> Result<OsString, ConfigError> {
    get(name).ok_or(ConfigError::Missing(name))
}

fn optional_string(
    get: &mut impl FnMut(&str) -> Option<OsString>,
    name: &'static str,
) -> Result<Option<String>, ConfigError> {
    get(name)
        .map(|value| {
            value.into_string().map_err(|_| ConfigError::Invalid {
                name,
                message: "value is not valid UTF-8".into(),
            })
        })
        .transpose()
}

fn parse_number(
    get: &mut impl FnMut(&str) -> Option<OsString>,
    name: &'static str,
    default: u64,
    min: u64,
    max: u64,
) -> Result<u64, ConfigError> {
    let Some(raw) = optional_string(get, name)? else {
        return Ok(default);
    };
    let parsed = raw.parse::<u64>().map_err(|_| {
        invalid(
            name,
            format!("expected an integer from {min} through {max}"),
        )
    })?;
    if !(min..=max).contains(&parsed) {
        return Err(invalid(
            name,
            format!("expected an integer from {min} through {max}"),
        ));
    }
    Ok(parsed)
}

fn parse_bool(
    get: &mut impl FnMut(&str) -> Option<OsString>,
    name: &'static str,
    default: bool,
) -> Result<bool, ConfigError> {
    let Some(raw) = optional_string(get, name)? else {
        return Ok(default);
    };
    match raw.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "on" => Ok(true),
        "0" | "false" | "no" | "off" => Ok(false),
        _ => Err(invalid(name, "expected true or false")),
    }
}

fn invalid(name: &'static str, message: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        name,
        message: message.into(),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("required environment variable {0} is unset")]
    Missing(&'static str),
    #[error("invalid {name}: {message}")]
    Invalid { name: &'static str, message: String },
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn load(values: &[(&str, &str)]) -> Result<BridgeConfig, ConfigError> {
        let values: HashMap<String, OsString> = values
            .iter()
            .map(|(key, value)| ((*key).to_string(), OsString::from(value)))
            .collect();
        BridgeConfig::from_lookup(|name| values.get(name).cloned())
    }

    #[test]
    fn defaults_have_no_provider_credentials_and_label_falls_back_to_interaction_domain_label() {
        let config = load(&[
            ("XDG_RUNTIME_DIR", "/run/user/1000"),
            ("TESSERA_MCP_INSTANCE_ID", "test"),
        ])
        .expect("config");
        assert_eq!(config.label, "Tessera Agent");
        assert_eq!(config.interaction_domain_label, "Tessera Agent");
        assert_eq!(
            config.socket_path,
            PathBuf::from("/run/user/1000/tessera.sock")
        );
        assert!(!config.revoke_on_exit);
    }

    #[test]
    fn label_override_and_data_dir_resolution() {
        let config = load(&[
            ("XDG_RUNTIME_DIR", "/tmp/runtime"),
            ("TESSERA_MCP_INSTANCE_ID", "test"),
            ("TESSERA_MCP_LABEL", "Codex"),
            ("TESSERA_MCP_DATA_DIR", "/tmp/identity"),
        ])
        .expect("config");
        assert_eq!(config.label, "Codex");
        assert_eq!(config.data_dir, Some(PathBuf::from("/tmp/identity")));

        let config = load(&[
            ("XDG_RUNTIME_DIR", "/tmp/runtime"),
            ("TESSERA_MCP_INSTANCE_ID", "test"),
            ("XDG_DATA_HOME", "/data"),
        ])
        .expect("config");
        assert_eq!(config.data_dir, Some(PathBuf::from("/data/tessera-mcp")));

        let config = load(&[
            ("XDG_RUNTIME_DIR", "/tmp/runtime"),
            ("TESSERA_MCP_INSTANCE_ID", "test"),
            ("HOME", "/home/u"),
        ])
        .expect("config");
        assert_eq!(
            config.data_dir,
            Some(PathBuf::from("/home/u/.local/share/tessera-mcp"))
        );
    }

    #[test]
    fn rejects_relative_runtime_directory() {
        let error = load(&[
            ("XDG_RUNTIME_DIR", "relative"),
            ("TESSERA_MCP_INSTANCE_ID", "test"),
        ])
        .expect_err("invalid");
        assert!(matches!(
            error,
            ConfigError::Invalid {
                name: "XDG_RUNTIME_DIR",
                ..
            }
        ));
    }

    #[test]
    fn parses_explicit_shutdown_policy() {
        let config = load(&[
            ("XDG_RUNTIME_DIR", "/tmp/runtime"),
            ("TESSERA_MCP_INSTANCE_ID", "test"),
            ("TESSERA_MCP_REVOKE_ON_EXIT", "off"),
        ])
        .expect("config");
        assert!(!config.revoke_on_exit);
    }

    #[test]
    fn bounds_label_length() {
        let long = "x".repeat(129);
        let values = HashMap::from([
            (
                "XDG_RUNTIME_DIR".to_string(),
                OsString::from("/tmp/runtime"),
            ),
            ("TESSERA_MCP_INSTANCE_ID".to_string(), OsString::from("test")),
            ("TESSERA_MCP_LABEL".to_string(), OsString::from(long)),
        ]);
        let error =
            BridgeConfig::from_lookup(|name| values.get(name).cloned()).expect_err("long label");
        assert!(matches!(
            error,
            ConfigError::Invalid {
                name: "TESSERA_MCP_LABEL",
                ..
            }
        ));
    }

    #[test]
    fn rejects_relative_data_dir() {
        let error = load(&[
            ("XDG_RUNTIME_DIR", "/tmp/runtime"),
            ("TESSERA_MCP_INSTANCE_ID", "test"),
            ("TESSERA_MCP_DATA_DIR", "relative"),
        ])
        .expect_err("invalid");
        assert!(matches!(
            error,
            ConfigError::Invalid {
                name: "TESSERA_MCP_DATA_DIR",
                ..
            }
        ));
    }

    #[test]
    fn requires_an_explicit_connector_instance_id() {
        let error = load(&[("XDG_RUNTIME_DIR", "/run/user/1000")]).expect_err("missing id");
        assert!(matches!(
            error,
            ConfigError::Missing("TESSERA_MCP_INSTANCE_ID")
        ));
    }
}
