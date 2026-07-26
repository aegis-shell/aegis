//! Host probes for the backend-neutral live system model.
//!
//! Probing remains outside `aegis-core` because it uses Linux files and host
//! commands. Mutations remain owned by the executable.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

pub use aegis_core::settings::{DisplaySettings, DisplayStatus};
pub use aegis_core::system::{BatteryStatus, NetworkState, SystemAction, SystemStatus};

/// Probe live host services through standard Linux interfaces.
///
/// Every source is optional so VMs, nested sessions, and desktops without a
/// given service still receive a coherent snapshot.
pub fn detect_system_status() -> SystemStatus {
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
        touchpad: aegis_core::input::TouchpadStatus::default(),
        display: DisplayStatus::default(),
    }
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
            aegis_core::input::TouchpadConfig::default()
        );
    }
}
