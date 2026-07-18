//! Read-only host system status and typed intents emitted by trusted shell
//! applications.
//!
//! Probing lives here so the HUD and control center consume one normalized
//! snapshot. Mutations remain owned by the executable: chrome emits a
//! [`SystemAction`], and the compositor main loop decides how to apply it.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

use ass_core::Point;
use ass_core::output::{ModeSpec, OutputInfo};

/// Coarse connectivity state shown in compact status surfaces.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum NetworkState {
    #[default]
    Offline,
    Wifi,
    Wired,
}

/// Battery state read from Linux power-supply sysfs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatteryStatus {
    pub percent: u8,
    pub charging: bool,
}

/// Live display state exposed to compositor-owned system UI.
///
/// Direct DRM sessions can persist and apply output policy. Nested sessions
/// deliberately expose the outer compositor's single host surface as
/// read-only because modesetting remains owned by that compositor.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DisplayStatus {
    /// Whether this session owns display configuration.
    pub configurable: bool,
    /// Connected outputs after the active per-connector policy is applied.
    pub outputs: Vec<OutputInfo>,
    /// Last persistence/application failure, cleared by a successful edit.
    pub error: Option<String>,
}

/// One complete, validated output edit emitted by Control Center.
///
/// The UI only constructs this value from modes advertised by the selected
/// connector. Persistence and backend application remain main-loop work.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplaySettings {
    pub connector: String,
    pub mode: ModeSpec,
    pub scale: f64,
    pub position: Point,
    pub primary: bool,
}

/// One coherent snapshot consumed by compositor-owned system UI.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct SystemStatus {
    pub volume: Option<u8>,
    pub muted: bool,
    pub network: NetworkState,
    pub battery: Option<BatteryStatus>,
    /// `None` means NetworkManager's radio state is unavailable.
    pub wifi_enabled: Option<bool>,
    /// `None` means no Bluetooth rfkill device was found.
    pub bluetooth_enabled: Option<bool>,
    /// Backlight level in percent, or `None` on displays without a backlight.
    pub brightness: Option<u8>,
    /// Compositor-owned state filled by the executable after host probing.
    pub do_not_disturb: bool,
    /// Current workspace layout state filled by the executable.
    pub tiled: bool,
    /// Physical touchpad capabilities and the selected device profile.
    pub touchpad: ass_core::input::TouchpadStatus,
    /// Connected displays and whether this session may configure them.
    pub display: DisplayStatus,
}

impl SystemStatus {
    /// Probe the host using standard Linux interfaces. Every source is
    /// optional so a desktop, VM, or nested compositor without that subsystem
    /// still gets a usable control center.
    pub fn detect() -> SystemStatus {
        let volume_output = command_output("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]);
        let (volume, muted) = volume_output
            .as_deref()
            .map(|output| {
                let value = output
                    .split_whitespace()
                    .find_map(|part| part.parse::<f32>().ok())
                    .map(|value| (value * 100.0).round().clamp(0.0, 100.0) as u8);
                (value, output.contains("MUTED"))
            })
            .unwrap_or((None, false));

        SystemStatus {
            volume,
            muted,
            network: detect_network(),
            battery: detect_battery(),
            wifi_enabled: detect_wifi_radio(),
            bluetooth_enabled: detect_bluetooth_radio(),
            brightness: detect_brightness(),
            do_not_disturb: false,
            tiled: false,
            touchpad: ass_core::input::TouchpadStatus::default(),
            display: DisplayStatus::default(),
        }
    }
}

/// Trusted system mutation requested by the control center or compact HUD.
#[derive(Debug, Clone, PartialEq)]
pub enum SystemAction {
    ToggleMute,
    StepVolume(i8),
    SetVolume(u8),
    SetBrightness(u8),
    SetWifi(bool),
    SetBluetooth(bool),
    SetDoNotDisturb(bool),
    SetTiling(bool),
    SetTouchpad(ass_core::input::TouchpadConfig),
    SetDisplay(DisplaySettings),
}

fn detect_network() -> NetworkState {
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return NetworkState::Offline;
    };
    let mut wired = false;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name == "lo" {
            continue;
        }
        let path = entry.path();
        let up = fs::read_to_string(path.join("operstate"))
            .map(|state| matches!(state.trim(), "up" | "unknown"))
            .unwrap_or(false);
        if !up {
            continue;
        }
        if path.join("wireless").is_dir() || name.to_string_lossy().starts_with("wl") {
            return NetworkState::Wifi;
        }
        wired = true;
    }
    if wired {
        NetworkState::Wired
    } else {
        NetworkState::Offline
    }
}

fn detect_battery() -> Option<BatteryStatus> {
    let entries = fs::read_dir("/sys/class/power_supply").ok()?;
    for entry in entries.flatten() {
        if !entry
            .file_name()
            .to_string_lossy()
            .to_ascii_uppercase()
            .starts_with("BAT")
        {
            continue;
        }
        let Some(percent) = read_u64(&entry.path().join("capacity")) else {
            continue;
        };
        let percent = percent.min(100) as u8;
        let charging = fs::read_to_string(entry.path().join("status"))
            .map(|status| status.trim().eq_ignore_ascii_case("charging"))
            .unwrap_or(false);
        return Some(BatteryStatus { percent, charging });
    }
    None
}

fn detect_wifi_radio() -> Option<bool> {
    command_output("nmcli", &["radio", "wifi"]).and_then(|value| match value.trim() {
        "enabled" => Some(true),
        "disabled" => Some(false),
        _ => None,
    })
}

fn detect_bluetooth_radio() -> Option<bool> {
    let entries = fs::read_dir("/sys/class/rfkill").ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(kind) = fs::read_to_string(path.join("type")) else {
            continue;
        };
        if kind.trim() != "bluetooth" {
            continue;
        }
        if let Some(state) = read_u64(&path.join("state")) {
            return Some(state != 0);
        }
    }
    None
}

fn detect_brightness() -> Option<u8> {
    let entries = fs::read_dir("/sys/class/backlight").ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        let Some(current) = read_u64(&path.join("brightness")) else {
            continue;
        };
        let Some(maximum) = read_u64(&path.join("max_brightness")) else {
            continue;
        };
        if maximum == 0 {
            continue;
        }
        return Some(((current.saturating_mul(100) + maximum / 2) / maximum).min(100) as u8);
    }
    None
}

fn read_u64(path: &Path) -> Option<u64> {
    fs::read_to_string(path).ok()?.trim().parse().ok()
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
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
        assert_eq!(
            status.touchpad.config,
            ass_core::input::TouchpadConfig::default()
        );
    }
}
