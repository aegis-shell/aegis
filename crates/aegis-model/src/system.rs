//! Backend-neutral live system status and immediate control intents.
//!
//! These types describe state whose authority remains with a live service or
//! the current compositor session. They are deliberately separate from
//! [`crate::settings`], whose transactions persist configuration.

use crate::input::InputStatus;
use crate::power::PowerMode;
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

/// Per-threshold "already warned" latches for the low-battery alert.
///
/// The compositor runtime feeds every applied battery sample through
/// [`BatteryWarningLatches::poll`]. A threshold fires once per discharge
/// cycle and rearms only after the level recovers above it; charging clears
/// every latch, so the next discharge warns again.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BatteryWarningLatches {
    fired: Vec<u8>,
}

impl BatteryWarningLatches {
    /// Evaluate one battery sample against the configured warning thresholds
    /// (1..=99, strictly descending — see
    /// [`crate::settings::BatterySettings`]; an empty slice disables the
    /// feature). Returns the threshold to alert on, if any.
    pub fn poll(&mut self, percent: u8, charging: bool, thresholds: &[u8]) -> Option<u8> {
        if charging {
            self.fired.clear();
            return None;
        }
        // A threshold whose level recovered above it can fire again.
        self.fired.retain(|latched| percent <= *latched);
        let crossed: Vec<u8> = thresholds
            .iter()
            .copied()
            .filter(|threshold| percent <= *threshold && !self.fired.contains(threshold))
            .collect();
        if crossed.is_empty() {
            return None;
        }
        // One alert even when a fast drop crosses several thresholds at
        // once; the lowest is the most severe.
        let lowest = crossed.iter().min().copied();
        self.fired.extend(crossed);
        lowest
    }
}

/// One coherent observation of live host and compositor-owned session state.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SystemStatus {
    pub volume: Option<u8>,
    pub muted: bool,
    pub network: NetworkState,
    /// The sysfs name of the interface behind `network` (e.g. `wlan0`,
    /// `enp3s0`), empty when offline. Presentation shows it beside the
    /// link state so the panel reads as "which NIC".
    #[cfg_attr(feature = "serde", serde(default))]
    pub network_interface: String,
    /// The associated Wi-Fi network name when `network` is a live wireless
    /// link, `None` otherwise (wired, offline, or the forked probe has not
    /// answered yet).
    #[cfg_attr(feature = "serde", serde(default))]
    pub wifi_ssid: Option<String>,
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
    pub input: InputStatus,
    /// Included so one host snapshot can feed both status and settings surfaces.
    pub display: DisplayStatus,
    /// Session-owned "always on" idle inhibition — the derived legacy view
    /// of the session power mode (ADR-0140): true exactly when the mode
    /// disarms the automatic lock stage. Unlike the connection-scoped IPC
    /// inhibitors it survives client disconnects; the runtime owns and
    /// reconciles it.
    pub idle_inhibited: bool,
    /// Session power mode (ADR-0140): which idle stages stay armed. Like
    /// `idle_inhibited` this is compositor-owned runtime state; the host
    /// status poller must not clear it.
    #[cfg_attr(feature = "serde", serde(default))]
    pub power_mode: PowerMode,
    /// Live compositor-owned capture streams (`StreamOutputStart`,
    /// ADR-0052). Chrome shows a persistent, non-interactive recording
    /// indicator while this is non-zero (ADR-0128). Compositor-owned like
    /// `idle_inhibited`: the host status poller must not clear it.
    #[cfg_attr(feature = "serde", serde(default))]
    pub capture_streams: u32,
}

/// Machine form factor, for chrome that adapts its presentation.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ChassisKind {
    #[default]
    Desktop,
    Laptop,
}

/// One sample of host resource utilisation (polled separately from
/// [`SystemStatus`]).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ResourceStats {
    /// Aggregate CPU usage in percent, 0..=100.
    pub cpu_percent: f32,
    /// Best-effort DRM busy percent; `None` when the driver exposes none.
    pub gpu_percent: Option<f32>,
    pub mem_used_bytes: u64,
    pub mem_total_bytes: u64,
    pub net_rx_bytes_per_sec: f64,
    pub net_tx_bytes_per_sec: f64,
    /// Usage of the filesystem mounted at `/`.
    pub disk_used_bytes: u64,
    pub disk_total_bytes: u64,
    /// Static in practice; carried so one channel feeds the panel.
    pub chassis: ChassisKind,
}

impl Default for ResourceStats {
    fn default() -> Self {
        ResourceStats {
            cpu_percent: 0.0,
            gpu_percent: None,
            mem_used_bytes: 0,
            mem_total_bytes: 0,
            net_rx_bytes_per_sec: 0.0,
            net_tx_bytes_per_sec: 0.0,
            disk_used_bytes: 0,
            disk_total_bytes: 0,
            chassis: ChassisKind::default(),
        }
    }
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
    /// Hold or release the session-owned "always on" idle inhibitor (the
    /// command panel toggle). While held, idle notifications stay resumed:
    /// no automatic dimming, locking, or display power-off.
    ///
    /// Superseded by [`SystemAction::SetPowerMode`] (ADR-0140), which covers
    /// this shape as `Awake`/`Balanced`. Kept on the wire for older clients;
    /// the runtime maps it onto the mode.
    SetIdleInhibit {
        inhibit: bool,
    },
    /// Select the session power mode (ADR-0140): which idle stages stay
    /// armed. Session-scoped runtime state, not persisted; manual locking
    /// and lock-before-sleep are unaffected by every mode.
    SetPowerMode {
        mode: PowerMode,
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

    #[test]
    fn a_threshold_fires_once_per_discharge_cycle() {
        let mut latches = BatteryWarningLatches::default();
        let thresholds = [20, 5];
        assert_eq!(latches.poll(21, false, &thresholds), None);
        assert_eq!(latches.poll(20, false, &thresholds), Some(20));
        assert_eq!(latches.poll(20, false, &thresholds), None);
        assert_eq!(latches.poll(19, false, &thresholds), None);
    }

    #[test]
    fn one_alert_covers_a_fast_drop_across_several_thresholds() {
        let mut latches = BatteryWarningLatches::default();
        let thresholds = [20, 5];
        assert_eq!(latches.poll(4, false, &thresholds), Some(5));
        // Both crossed thresholds latched: neither refires afterwards.
        assert_eq!(latches.poll(3, false, &thresholds), None);
        assert_eq!(latches.poll(1, false, &thresholds), None);
    }

    #[test]
    fn charging_clears_the_latches_for_the_next_cycle() {
        let mut latches = BatteryWarningLatches::default();
        let thresholds = [20, 5];
        assert_eq!(latches.poll(19, false, &thresholds), Some(20));
        assert_eq!(latches.poll(30, true, &thresholds), None);
        assert_eq!(latches.poll(19, false, &thresholds), Some(20));
    }

    #[test]
    fn recovery_rearms_only_the_recovered_thresholds() {
        let mut latches = BatteryWarningLatches::default();
        let thresholds = [20, 5];
        assert_eq!(latches.poll(4, false, &thresholds), Some(5));
        // Recovering to 10% rearms only the 5% threshold: the 20% one stays
        // latched until the level climbs back above 20%.
        assert_eq!(latches.poll(10, false, &thresholds), None);
        assert_eq!(latches.poll(15, false, &thresholds), None);
        assert_eq!(latches.poll(4, false, &thresholds), Some(5));
        assert_eq!(latches.poll(25, false, &thresholds), None);
        assert_eq!(latches.poll(19, false, &thresholds), Some(20));
    }

    #[test]
    fn no_alert_while_charging_even_below_every_threshold() {
        let mut latches = BatteryWarningLatches::default();
        let thresholds = [20, 5];
        assert_eq!(latches.poll(3, true, &thresholds), None);
        // The charging sample cleared nothing and latched nothing: the next
        // discharge sample alerts normally.
        assert_eq!(latches.poll(3, false, &thresholds), Some(5));
    }

    #[test]
    fn empty_thresholds_disable_the_feature() {
        let mut latches = BatteryWarningLatches::default();
        assert_eq!(latches.poll(3, false, &[]), None);
        assert_eq!(latches.poll(1, false, &[]), None);
    }
}
