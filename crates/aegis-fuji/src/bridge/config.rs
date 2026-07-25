use std::ffi::OsString;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

const DEFAULT_SCOPE: &str = "fuji";
const DEFAULT_REALM_LABEL: &str = "Fuji";
const DEFAULT_IPC_TIMEOUT_SECS: u64 = 5;

/// Runtime policy for one fuji MCP bridge process.
#[derive(Clone, PartialEq, Eq)]
pub struct BridgeConfig {
    pub socket_path: PathBuf,
    pub runtime_dir: PathBuf,
    pub scope: String,
    pub realm_label: String,
    pub io_timeout: Duration,
    /// Revoke the managed Realm on a graceful stdio shutdown. A process killed
    /// during connector refresh leaves a recovery record for the successor.
    pub revoke_on_exit: bool,
}

impl fmt::Debug for BridgeConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("BridgeConfig")
            .field("socket_path", &self.socket_path)
            .field("runtime_dir", &self.runtime_dir)
            .field("scope", &self.scope)
            .field("realm_label", &self.realm_label)
            .field("io_timeout", &self.io_timeout)
            .field("revoke_on_exit", &self.revoke_on_exit)
            .finish()
    }
}

impl BridgeConfig {
    pub fn new(
        socket_path: impl Into<PathBuf>,
        runtime_dir: impl Into<PathBuf>,
        scope: impl Into<String>,
        realm_label: impl Into<String>,
    ) -> Result<Self, ConfigError> {
        let config = Self {
            socket_path: socket_path.into(),
            runtime_dir: runtime_dir.into(),
            scope: scope.into(),
            realm_label: realm_label.into(),
            io_timeout: Duration::from_secs(DEFAULT_IPC_TIMEOUT_SECS),
            revoke_on_exit: true,
        };
        config.validate()?;
        Ok(config)
    }

    /// Load the bridge configuration from `ASS_FUJI_*` variables.
    pub fn from_env() -> Result<Self, ConfigError> {
        Self::from_lookup(|name| std::env::var_os(name))
    }

    fn from_lookup(mut get: impl FnMut(&str) -> Option<OsString>) -> Result<Self, ConfigError> {
        let runtime_dir = PathBuf::from(required_os(&mut get, "XDG_RUNTIME_DIR")?);
        let socket_path = get("ASS_FUJI_SOCKET")
            .map(PathBuf::from)
            .unwrap_or_else(|| runtime_dir.join("aegis.sock"));
        let scope = optional_string(&mut get, "ASS_FUJI_SCOPE")?
            .unwrap_or_else(|| DEFAULT_SCOPE.to_string());
        let realm_label = optional_string(&mut get, "ASS_FUJI_REALM_LABEL")?
            .unwrap_or_else(|| DEFAULT_REALM_LABEL.to_string());
        let io_timeout = Duration::from_secs(parse_number(
            &mut get,
            "ASS_FUJI_IPC_TIMEOUT_SECS",
            DEFAULT_IPC_TIMEOUT_SECS,
            1,
            60,
        )?);
        let revoke_on_exit = parse_bool(&mut get, "ASS_FUJI_REVOKE_ON_EXIT", true)?;
        let config = Self {
            socket_path,
            runtime_dir,
            scope,
            realm_label,
            io_timeout,
            revoke_on_exit,
        };
        config.validate()?;
        Ok(config)
    }

    pub(crate) fn validate(&self) -> Result<(), ConfigError> {
        if self.socket_path.as_os_str().is_empty() {
            return Err(invalid("ASS_FUJI_SOCKET", "path must not be empty"));
        }
        if !self.runtime_dir.is_absolute() {
            return Err(invalid(
                "XDG_RUNTIME_DIR",
                "path must be absolute so Realm recovery state is private and unambiguous",
            ));
        }
        if self.scope.trim().is_empty() || self.scope.len() > 128 {
            return Err(invalid(
                "ASS_FUJI_SCOPE",
                "length must be from 1 through 128 bytes",
            ));
        }
        let label = self.realm_label.trim();
        if label.is_empty() || label.len() > 128 {
            return Err(invalid(
                "ASS_FUJI_REALM_LABEL",
                "length must be from 1 through 128 bytes",
            ));
        }
        if !(Duration::from_secs(1)..=Duration::from_secs(60)).contains(&self.io_timeout) {
            return Err(invalid(
                "ASS_FUJI_IPC_TIMEOUT_SECS",
                "expected a duration from 1 through 60 seconds",
            ));
        }
        Ok(())
    }

    pub(crate) fn state_dir(&self) -> PathBuf {
        self.runtime_dir.join("aegis-fuji")
    }
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
    fn defaults_are_product_scoped_and_have_no_provider_credentials() {
        let config = load(&[("XDG_RUNTIME_DIR", "/run/user/1000")]).expect("config");
        assert_eq!(config.scope, "fuji");
        assert_eq!(config.realm_label, "Fuji");
        assert_eq!(config.socket_path, PathBuf::from("/run/user/1000/aegis.sock"));
        assert!(config.revoke_on_exit);
    }

    #[test]
    fn rejects_relative_runtime_directory() {
        let error = load(&[("XDG_RUNTIME_DIR", "relative")]).expect_err("invalid");
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
            ("ASS_FUJI_REVOKE_ON_EXIT", "off"),
        ])
        .expect("config");
        assert!(!config.revoke_on_exit);
    }

    #[test]
    fn bounds_scope_name_used_for_recovery_identity() {
        let long = "x".repeat(129);
        let values = HashMap::from([
            (
                "XDG_RUNTIME_DIR".to_string(),
                OsString::from("/tmp/runtime"),
            ),
            ("ASS_FUJI_SCOPE".to_string(), OsString::from(long)),
        ]);
        let error =
            BridgeConfig::from_lookup(|name| values.get(name).cloned()).expect_err("long scope");
        assert!(matches!(
            error,
            ConfigError::Invalid {
                name: "ASS_FUJI_SCOPE",
                ..
            }
        ));
    }
}
