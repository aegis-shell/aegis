//! Backend-neutral live system status and immediate control intents.
//!
//! These types describe state whose authority remains with a live service or
//! the current compositor session. They are deliberately separate from
//! [`crate::settings`], whose transactions persist configuration.

use crate::input::TouchpadStatus;
use crate::settings::DisplayStatus;

/// Coarse connectivity state shown by desktop status surfaces.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NetworkState {
    #[default]
    Offline,
    Wifi,
    Wired,
}

/// Battery state read from the host power service.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryStatus {
    pub percent: u8,
    pub charging: bool,
}

/// One coherent observation of live host and compositor-owned session state.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SystemStatus {
    pub volume: Option<u8>,
    pub muted: bool,
    pub network: NetworkState,
    pub battery: Option<BatteryStatus>,
    /// `None` means the Wi-Fi radio service is unavailable.
    pub wifi_enabled: Option<bool>,
    /// `None` means no Bluetooth radio service is available.
    pub bluetooth_enabled: Option<bool>,
    /// Backlight level in percent, or `None` without a controllable backlight.
    pub brightness: Option<u8>,
    pub do_not_disturb: bool,
    /// Layout mode for the current workspace.
    pub tiled: bool,
    /// Included so one host probe can feed both status and settings surfaces.
    pub touchpad: TouchpadStatus,
    /// Included so one host snapshot can feed both status and settings surfaces.
    pub display: DisplayStatus,
}

/// An immediate live-system mutation.
///
/// Unlike [`crate::settings::SettingsAction`], applying one of these actions
/// does not imply that configuration was persisted.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SystemAction {
    ToggleMute,
    StepVolume {
        delta: i8,
    },
    SetVolume {
        level: u8,
    },
    SetBrightness {
        level: u8,
    },
    SetWifi {
        enabled: bool,
    },
    SetBluetooth {
        enabled: bool,
    },
    SetDoNotDisturb {
        enabled: bool,
    },
    SetTiling {
        enabled: bool,
    },
    /// Enable or disable physical scanout without changing the output
    /// topology. Used by the trusted idle policy after the session lock has
    /// been compositor-confirmed.
    SetOutputPower {
        powered: bool,
    },
}

impl SystemAction {
    /// Validate bounds shared by IPC and in-process callers.
    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::StepVolume { delta } if !(-100..=100).contains(delta) => {
                Err("volume step is outside -100..=100")
            }
            Self::SetVolume { level } if *level > 100 => Err("volume is outside 0..=100"),
            Self::SetBrightness { level } if !(1..=100).contains(level) => {
                Err("brightness is outside 1..=100")
            }
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_status_marks_optional_devices_unavailable() {
        let status = SystemStatus::default();
        assert_eq!(status.network, NetworkState::Offline);
        assert_eq!(status.volume, None);
        assert_eq!(status.brightness, None);
        assert_eq!(status.wifi_enabled, None);
        assert_eq!(status.bluetooth_enabled, None);
    }

    #[test]
    fn action_validation_rejects_out_of_range_levels() {
        assert!(SystemAction::SetVolume { level: 101 }.validate().is_err());
        assert!(SystemAction::SetBrightness { level: 0 }.validate().is_err());
        assert!(
            SystemAction::SetBrightness { level: 100 }
                .validate()
                .is_ok()
        );
    }
}
