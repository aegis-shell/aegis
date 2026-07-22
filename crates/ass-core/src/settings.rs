//! Versionable desktop-settings model shared by chrome, IPC, and clients.
//!
//! These types describe compositor-owned persistent settings. Instant system
//! controls such as volume and Wi-Fi are intentionally absent: their source
//! of truth is the corresponding system service, not the compositor config.

use crate::Point;
use crate::input::{TouchpadConfig, TouchpadStatus};
use crate::output::{ModeSpec, OutputInfo};

/// Live display capabilities and current effective configuration.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DisplayStatus {
    /// Whether this session owns physical display configuration.
    pub configurable: bool,
    pub outputs: Vec<OutputInfo>,
    /// Last persistence/application failure, cleared by a successful edit.
    pub error: Option<String>,
}

/// One complete, validated output edit.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq)]
pub struct DisplaySettings {
    pub connector: String,
    pub mode: ModeSpec,
    pub scale: f64,
    pub position: Point,
    pub primary: bool,
}

/// Coherent persistent-settings snapshot. `revision` changes after every
/// accepted mutation and lets clients reject stale drafts.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SettingsSnapshot {
    pub revision: u64,
    pub touchpad: TouchpadStatus,
    pub display: DisplayStatus,
}

/// One typed persistent-settings transaction.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type"))]
#[derive(Debug, Clone, PartialEq)]
pub enum SettingsAction {
    SetTouchpad { config: TouchpadConfig },
    SetDisplay { settings: DisplaySettings },
}

impl SettingsAction {
    /// Validate transport-level bounds. Hardware capability checks and mode
    /// membership remain authoritative main-loop work.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::SetTouchpad { config }
                if !config.pointer_speed.is_finite()
                    || !(-1.0..=1.0).contains(&config.pointer_speed) =>
            {
                Err("touchpad pointer speed is outside -1.0..=1.0")
            }
            Self::SetDisplay { settings }
                if settings.connector.trim().is_empty() || settings.connector.len() > 128 =>
            {
                Err("display connector is empty or too long")
            }
            Self::SetDisplay { settings }
                if !settings.scale.is_finite() || !(0.25..=4.0).contains(&settings.scale) =>
            {
                Err("display scale is outside 0.25..=4.0")
            }
            Self::SetDisplay { settings }
                if settings.mode.width <= 0
                    || settings.mode.height <= 0
                    || settings.mode.width > 32_768
                    || settings.mode.height > 32_768
                    || settings
                        .mode
                        .refresh_hz
                        .is_some_and(|hz| !(1..=1_000).contains(&hz)) =>
            {
                Err("display mode is outside the supported range")
            }
            _ => Ok(()),
        }
    }
}

/// Confirmation returned after the main loop persisted and applied a
/// settings action.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SettingsReceipt {
    pub revision: u64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_validation_rejects_unbounded_values() {
        let mut config = TouchpadConfig::default();
        config.pointer_speed = 1.5;
        assert!(SettingsAction::SetTouchpad { config }.validate().is_err());

        let display = DisplaySettings {
            connector: "DP-1".into(),
            mode: ModeSpec {
                width: 2560,
                height: 1440,
                refresh_hz: Some(144),
            },
            scale: 1.5,
            position: Point { x: 0, y: 0 },
            primary: true,
        };
        assert!(
            SettingsAction::SetDisplay { settings: display }
                .validate()
                .is_ok()
        );
    }
}
