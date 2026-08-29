//! Host probes for the backend-neutral live system model.
//!
//! Probing remains outside `aegis-model` because it uses Linux files and host
//! commands. Mutations remain owned by the executable.

use std::fs;
use std::path::Path;
use std::process::{Command, Stdio};

pub use aegis_model::settings::{DisplaySettings, DisplayStatus};
pub use aegis_model::system::{
    BatteryStatus, ChassisKind, NetworkState, ResourceStats, SystemAction, SystemStatus,
};

/// Probe live host services through standard Linux interfaces.
///
/// Every source is optional so VMs, nested sessions, and desktops without a
/// given service still receive a coherent snapshot.
pub fn detect_system_status() -> SystemStatus {
    let (volume, muted, wifi_enabled, wifi_ssid) = detect_forked_status();
    let (network, network_interface) = detect_network();
    SystemStatus {
        volume,
        muted,
        network,
        network_interface,
        wifi_ssid,
        battery: detect_battery(),
        wifi_enabled,
        bluetooth_enabled: detect_bluetooth_radio(),
        brightness: detect_brightness(),
        do_not_disturb: false,
        tiled: false,
        input: aegis_model::input::InputStatus::default(),
        display: DisplayStatus::default(),
        idle_inhibited: false,
        power_mode: aegis_model::power::PowerMode::default(),
        // Compositor-owned; the runtime patches its live value onto every
        // host sample before publishing (ADR-0128).
        capture_streams: 0,
    }
}

/// Probe every status field whose source is a cheap `/sys` read, deferring the
/// fork+exec probes (`wpctl get-volume`, `nmcli radio wifi`, the SSID lookup)
/// to a separate full pass.
///
/// The status poller wakes on a short cadence to keep the HUD (battery,
/// brightness, charging, network link) fresh, but none of the forked
/// commands changes on that timescale — volume moves only on user action
/// (which already triggers an out-of-cycle full probe via the refresh signal)
/// and the Wi-Fi radio toggle is rare. Re-running them every few seconds
/// spawned one `wpctl` and one `nmcli` per cycle purely to re-discover an
/// unchanged answer. This light variant keeps the frequent poll off the fork
/// path entirely; `volume`, `muted`, `wifi_enabled`, and `wifi_ssid` are
/// filled in from the caller's last known values, so a snapshot built from
/// it only diverges where the sysfs-backed fields actually moved.
pub fn detect_system_status_lightweight(
    last_volume: Option<u8>,
    last_muted: bool,
    last_wifi_enabled: Option<bool>,
    last_wifi_ssid: Option<String>,
) -> SystemStatus {
    let (network, network_interface) = detect_network();
    SystemStatus {
        volume: last_volume,
        muted: last_muted,
        network,
        network_interface,
        wifi_ssid: last_wifi_ssid,
        battery: detect_battery(),
        wifi_enabled: last_wifi_enabled,
        bluetooth_enabled: detect_bluetooth_radio(),
        brightness: detect_brightness(),
        do_not_disturb: false,
        tiled: false,
        input: aegis_model::input::InputStatus::default(),
        display: DisplayStatus::default(),
        idle_inhibited: false,
        power_mode: aegis_model::power::PowerMode::default(),
        // Compositor-owned; the runtime patches its live value onto every
        // host sample before publishing (ADR-0128).
        capture_streams: 0,
    }
}

/// Run only the forked probes and return
/// `(volume, muted, wifi_enabled, wifi_ssid)`.
///
/// Called on the long-interval cadence and on out-of-cycle refresh requests,
/// so the cheap poll stays on the `/sys` path while the expensive commands run
/// rarely and only when their result may have changed.
pub fn detect_forked_status() -> (Option<u8>, bool, Option<bool>, Option<String>) {
    let (volume, muted) = detect_volume();
    (volume, muted, detect_wifi_radio(), detect_wifi_ssid())
}

fn detect_volume() -> (Option<u8>, bool) {
    let volume_output = command_output("wpctl", &["get-volume", "@DEFAULT_AUDIO_SINK@"]);
    volume_output
        .as_deref()
        .map(|output| {
            let value = output
                .split_whitespace()
                .find_map(|part| part.parse::<f32>().ok())
                .map(|value| (value * 100.0).round().clamp(0.0, 100.0) as u8);
            (value, output.contains("MUTED"))
        })
        .unwrap_or((None, false))
}

/// The default-route interface first (the one the panel should name),
/// falling back to any live link. Returns the state plus the sysfs name.
fn detect_network() -> (NetworkState, String) {
    let Ok(entries) = fs::read_dir("/sys/class/net") else {
        return (NetworkState::Offline, String::new());
    };
    let mut wired: Option<String> = None;
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
        let name = name.to_string_lossy().into_owned();
        if path.join("wireless").is_dir() || name.starts_with("wl") {
            return (NetworkState::Wifi, name);
        }
        wired = wired.or(Some(name));
    }
    match wired {
        Some(name) => (NetworkState::Wired, name),
        None => (NetworkState::Offline, String::new()),
    }
}

/// The associated Wi-Fi network name, daemon-neutral by construction.
///
/// `iwgetid -r` (wireless-tools) answers for any station because it reads
/// the association through the generic netlink/cfg80211 path the kernel
/// exposes, independent of whether iwd, wpa_supplicant, or NetworkManager
/// owns the radio. Where wireless-tools is absent, `iw dev <if> link`
/// serves the same purpose against the same kernel source. `None` when no
/// tool, radio, or association answers — the same "unknown, keep the last
/// value" contract as the other forked probes.
fn detect_wifi_ssid() -> Option<String> {
    if let Some(value) = command_output("iwgetid", &["-r"]) {
        let ssid = value.trim();
        if !ssid.is_empty() {
            return Some(ssid.to_owned());
        }
        // An empty answer from a present tool is authoritative: no
        // association. Do not fall through to `iw`, which would only
        // repeat it.
        return None;
    }
    let (_, interface) = detect_network();
    if interface.is_empty() {
        return None;
    }
    iw_link_ssid(&interface)
}

/// Extract the SSID from `iw dev <interface> link` output. The command
/// prints `SSID: <name>` on its own line when associated; a non-associated
/// link prints `Not connected.` and yields `None`.
fn iw_link_ssid(interface: &str) -> Option<String> {
    parse_iw_link_ssid(&command_output("iw", &["dev", interface, "link"])?)
}

/// Pure parser half of [`iw_link_ssid`], separated so the line protocol is
/// testable without a host wireless stack.
fn parse_iw_link_ssid(output: &str) -> Option<String> {
    output
        .lines()
        .find_map(|line| line.trim().strip_prefix("SSID:"))
        .map(|ssid| ssid.trim().to_owned())
        .filter(|ssid| !ssid.is_empty())
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

/// Stateful probe for host resource utilisation.
///
/// Rates (CPU, network) are computed against the previous sample, so the
/// first sample reports zero rates. Everything here is a cheap `/proc` /
/// `/sys` read or a `statvfs` call — no subprocesses — so it can poll on a
/// short cadence without the fork cost of [`detect_system_status`].
pub struct ResourceProbe {
    previous_cpu: Option<(u64, u64)>,
    previous_net: Option<(u64, u64)>,
    previous_sample: Option<std::time::Instant>,
    chassis: ChassisKind,
}

impl ResourceProbe {
    pub fn new() -> ResourceProbe {
        ResourceProbe {
            previous_cpu: None,
            previous_net: None,
            previous_sample: None,
            chassis: detect_chassis(),
        }
    }

    /// Sample the host's current resource usage.
    pub fn sample(&mut self) -> ResourceStats {
        let now = std::time::Instant::now();
        let elapsed = self
            .previous_sample
            .map(|previous| now.duration_since(previous).as_secs_f64())
            .filter(|elapsed| *elapsed > 0.0);
        self.previous_sample = Some(now);

        let cpu_percent = fs::read_to_string("/proc/stat")
            .ok()
            .and_then(|stat| parse_proc_stat_cpu(&stat))
            .map(|(idle, total)| {
                let usage = self
                    .previous_cpu
                    .map(|(previous_idle, previous_total)| {
                        let delta_idle = idle.saturating_sub(previous_idle);
                        let delta_total = total.saturating_sub(previous_total);
                        if delta_total == 0 {
                            0.0
                        } else {
                            ((1.0 - delta_idle as f64 / delta_total as f64) * 100.0) as f32
                        }
                    })
                    .unwrap_or(0.0);
                self.previous_cpu = Some((idle, total));
                usage.clamp(0.0, 100.0)
            })
            .unwrap_or(0.0);

        let (mem_used_bytes, mem_total_bytes) = fs::read_to_string("/proc/meminfo")
            .ok()
            .and_then(|meminfo| parse_meminfo(&meminfo))
            .map(|(total, available)| (total.saturating_sub(available), total))
            .unwrap_or((0, 0));

        let net = fs::read_to_string("/proc/net/dev")
            .ok()
            .map(|dev| parse_netdev(&dev))
            .map(|(rx, tx)| {
                let rates = self
                    .previous_net
                    .zip(elapsed)
                    .map(|((previous_rx, previous_tx), elapsed)| {
                        (
                            rx.saturating_sub(previous_rx) as f64 / elapsed,
                            tx.saturating_sub(previous_tx) as f64 / elapsed,
                        )
                    })
                    .unwrap_or((0.0, 0.0));
                self.previous_net = Some((rx, tx));
                rates
            })
            .unwrap_or((0.0, 0.0));

        let (disk_used_bytes, disk_total_bytes) = root_fs_usage().unwrap_or((0, 0));

        ResourceStats {
            cpu_percent,
            gpu_percent: detect_gpu_busy(),
            mem_used_bytes,
            mem_total_bytes,
            net_rx_bytes_per_sec: net.0,
            net_tx_bytes_per_sec: net.1,
            disk_used_bytes,
            disk_total_bytes,
            chassis: self.chassis,
        }
    }
}

impl Default for ResourceProbe {
    fn default() -> Self {
        Self::new()
    }
}

/// Parse the aggregate `cpu ` line of `/proc/stat` into `(idle, total)`
/// jiffies, where idle includes iowait.
fn parse_proc_stat_cpu(stat: &str) -> Option<(u64, u64)> {
    let line = stat.lines().find(|line| line.starts_with("cpu "))?;
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1)
        .filter_map(|field| field.parse().ok())
        .collect();
    if fields.len() < 5 {
        return None;
    }
    let idle = fields[3] + fields[4];
    let total = fields.iter().sum();
    Some((idle, total))
}

/// Parse `MemTotal` and `MemAvailable` from `/proc/meminfo`, in bytes.
fn parse_meminfo(meminfo: &str) -> Option<(u64, u64)> {
    let field = |name: &str| {
        meminfo.lines().find_map(|line| {
            line.strip_prefix(name)?
                .split_whitespace()
                .next()?
                .parse::<u64>()
                .ok()
                .map(|kib| kib * 1024)
        })
    };
    Some((field("MemTotal:")?, field("MemAvailable:")?))
}

/// Sum rx/tx byte counters across all `/proc/net/dev` interfaces except `lo`.
fn parse_netdev(dev: &str) -> (u64, u64) {
    let mut rx = 0;
    let mut tx = 0;
    for line in dev.lines().skip(2) {
        let Some((name, counters)) = line.split_once(':') else {
            continue;
        };
        if name.trim() == "lo" {
            continue;
        }
        let fields: Vec<u64> = counters
            .split_whitespace()
            .filter_map(|field| field.parse().ok())
            .collect();
        if fields.len() < 9 {
            continue;
        }
        rx += fields[0];
        tx += fields[8];
    }
    (rx, tx)
}

/// Used and total bytes of the filesystem mounted at `/`.
fn root_fs_usage() -> Option<(u64, u64)> {
    let path = std::ffi::CString::new("/").ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    if unsafe { libc::statvfs(path.as_ptr(), &mut stat) } != 0 {
        return None;
    }
    let total = stat.f_blocks * stat.f_frsize;
    let used = (stat.f_blocks - stat.f_bfree) * stat.f_frsize;
    Some((used, total))
}

/// Best-effort GPU busy percent from the DRM sysfs nodes. `None` when no
/// driver exposes `gpu_busy_percent`.
fn detect_gpu_busy() -> Option<f32> {
    let entries = fs::read_dir("/sys/class/drm").ok()?;
    let mut cards: Vec<_> = entries
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("card")
                && name["card".len()..]
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_ascii_digit())
        })
        .collect();
    cards.sort_by_key(|entry| entry.file_name());
    for card in cards {
        if let Some(percent) = read_u64(&card.path().join("device/gpu_busy_percent")) {
            return Some(percent.min(100) as f32);
        }
    }
    None
}

/// Map a DMI chassis type code to a form factor. Laptop codes follow
/// SMBIOS: notebook-class enclosures 8-12, 14, and 30-32.
fn chassis_from_type_code(code: &str) -> Option<ChassisKind> {
    match code.trim().parse::<u32>().ok()? {
        8..=12 | 14 | 30..=32 => Some(ChassisKind::Laptop),
        _ => Some(ChassisKind::Desktop),
    }
}

/// Detect the machine form factor once: DMI chassis type first, then the
/// presence of a battery as the fallback, finally Desktop.
fn detect_chassis() -> ChassisKind {
    if let Ok(code) = fs::read_to_string("/sys/class/dmi/id/chassis_type")
        && let Some(kind) = chassis_from_type_code(&code)
    {
        return kind;
    }
    let has_battery = fs::read_dir("/sys/class/power_supply")
        .map(|entries| {
            entries.flatten().any(|entry| {
                entry
                    .file_name()
                    .to_string_lossy()
                    .to_ascii_uppercase()
                    .starts_with("BAT")
            })
        })
        .unwrap_or(false);
    if has_battery {
        ChassisKind::Laptop
    } else {
        ChassisKind::Desktop
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
        assert_eq!(
            status.input.touchpad.config,
            aegis_model::input::TouchpadConfig::default()
        );
        assert_eq!(
            status.input.keyboard,
            aegis_model::input::KeyboardConfig::default()
        );
    }

    #[test]
    fn parses_aggregate_cpu_line() {
        let stat = "\
cpu  3357 0 4313 1362393 341 0 254 0 0 0
cpu0 1000 0 1000 100000 100 0 100 0 0 0
";
        let (idle, total) = parse_proc_stat_cpu(stat).unwrap();
        assert_eq!(idle, 1362393 + 341);
        assert_eq!(total, 3357 + 4313 + 1362393 + 341 + 254);
        assert!(parse_proc_stat_cpu("no cpu line here\n").is_none());
    }

    #[test]
    fn parses_meminfo_totals_in_bytes() {
        let meminfo = "\
MemTotal:       16384000 kB
MemFree:         1024000 kB
MemAvailable:    8192000 kB
";
        let (total, available) = parse_meminfo(meminfo).unwrap();
        assert_eq!(total, 16384000 * 1024);
        assert_eq!(available, 8192000 * 1024);
        assert!(parse_meminfo("MemTotal:       1024 kB\n").is_none());
    }

    #[test]
    fn parses_netdev_skipping_loopback() {
        let dev = "\
Inter-|   Receive                                                |  Transmit
 face |bytes    packets errs drop fifo frame compressed multicast|bytes    packets
  eth0: 1000000    1000    0    0    0     0          0         0  2000000    2000
    lo: 9999999    9999    0    0    0     0          0         0  9999999    9999
 wlan0:  300000    3000    0    0    0     0          0         0   400000    4000
";
        let (rx, tx) = parse_netdev(dev);
        assert_eq!(rx, 1000000 + 300000);
        assert_eq!(tx, 2000000 + 400000);
    }

    #[test]
    fn maps_chassis_type_codes() {
        for code in ["8", "9", "10", "11", "12", "14", "30", "31", "32"] {
            assert_eq!(chassis_from_type_code(code), Some(ChassisKind::Laptop));
        }
        assert_eq!(chassis_from_type_code("3"), Some(ChassisKind::Desktop));
        assert_eq!(chassis_from_type_code("not-a-number"), None);
    }

    #[test]
    fn parses_iw_link_ssid_for_associated_stations() {
        // Layout exactly as `iw dev wlan0 link` prints it: signal and SSID
        // lines indented on their own rows, preceded by the joined BSS.
        let associated = "\
Connected to 3c:84:6a:12:34:56 (on wlan0)
\tSSID: Homelab-5G
\tfreq: 5240
\tsignal: -47 dBm
";
        assert_eq!(
            parse_iw_link_ssid(associated).as_deref(),
            Some("Homelab-5G")
        );

        let unassociated = "Not connected.\n";
        assert_eq!(parse_iw_link_ssid(unassociated), None);

        let empty_ssid = "\tSSID: \n";
        assert_eq!(parse_iw_link_ssid(empty_ssid), None);

        // A hidden network reports the SSID as an octet string that can
        // still decode as text; a trailing carriage return must not leak
        // into the name.
        let crlf = "Connected to aa:bb:cc:dd:ee:ff (on wlan0)\r\n\tSSID: Café 5G\r\n";
        assert_eq!(parse_iw_link_ssid(crlf).as_deref(), Some("Café 5G"));
    }
}
