use super::*;

/// Publish one normalized live-system snapshot to in-process chrome and IPC.
pub(super) fn publish_system_status_parts(
    status: &aegis_core::system::SystemStatus,
    shell: &mut aegis_shell::Shell,
    live: &std::sync::Arc<LiveState>,
    ipc: &Option<aegis_ipc::Server>,
) {
    shell.set_system_status(status.clone());
    live.set_system_status(status.clone());
    if let Some(ipc) = ipc {
        ipc.broadcast(aegis_ipc::Event::SystemStatusChanged);
    }
}

/// Apply one immediate system control through the authoritative runtime path.
///
/// Host commands are spawned without blocking the compositor. The status
/// snapshot is updated optimistically and the status poller reconciles it
/// against the host service immediately afterwards.
pub(super) fn apply_system_action(
    server: &mut aegis_compositor::Server,
    notifications: &std::sync::Arc<std::sync::Mutex<aegis_core::notify::NotificationQueue>>,
    status: &mut aegis_core::system::SystemStatus,
    action: aegis_core::system::SystemAction,
) -> Result<(), String> {
    use aegis_core::system::SystemAction;

    action.validate().map_err(str::to_owned)?;
    match action {
        SystemAction::ToggleMute => {
            spawn_host_command("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"])?;
            status.muted = !status.muted;
        }
        SystemAction::StepVolume { delta } => {
            let amount = format!(
                "{}%{}",
                delta.unsigned_abs(),
                if delta >= 0 { "+" } else { "-" }
            );
            spawn_host_command(
                "wpctl",
                &["set-volume", "@DEFAULT_AUDIO_SINK@", &amount, "-l", "1.0"],
            )?;
            let current = status.volume.unwrap_or(0) as i16;
            status.volume = Some((current + i16::from(delta)).clamp(0, 100) as u8);
        }
        SystemAction::SetVolume { level } => {
            let amount = format!("{level}%");
            spawn_host_command(
                "wpctl",
                &["set-volume", "@DEFAULT_AUDIO_SINK@", &amount, "-l", "1.0"],
            )?;
            status.volume = Some(level);
        }
        SystemAction::SetBrightness { level } => {
            let amount = format!("{level}%");
            spawn_host_command("brightnessctl", &["--class=backlight", "set", &amount])?;
            status.brightness = Some(level);
        }
        SystemAction::SetWifi { enabled } => {
            spawn_host_command(
                "nmcli",
                &["radio", "wifi", if enabled { "on" } else { "off" }],
            )?;
            status.wifi_enabled = Some(enabled);
        }
        SystemAction::SetBluetooth { enabled } => {
            spawn_host_command(
                "rfkill",
                &[if enabled { "unblock" } else { "block" }, "bluetooth"],
            )?;
            status.bluetooth_enabled = Some(enabled);
        }
        SystemAction::SetDoNotDisturb { enabled } => {
            notifications.lock().unwrap().set_do_not_disturb(enabled);
            status.do_not_disturb = enabled;
        }
        SystemAction::SetTiling { enabled } => {
            server.set_tiling(enabled);
            status.tiled = enabled;
        }
    }
    Ok(())
}

fn spawn_host_command(program: &str, args: &[&str]) -> Result<(), String> {
    std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("failed to start {program}: {error}"))
}
