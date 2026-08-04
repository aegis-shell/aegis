use super::*;

/// Publish one normalized live-system snapshot to in-process chrome and IPC.
pub(super) fn publish_system_status_parts(
    status: &aegis_model::system::SystemStatus,
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
    host: &mut aegis_backend::host::Host,
    notifications: &std::sync::Arc<std::sync::Mutex<aegis_model::notify::NotificationQueue>>,
    status: &mut aegis_model::system::SystemStatus,
    idle_inhibits: &mut super::idle::IdleInhibits,
    action: aegis_model::system::SystemAction,
) -> Result<(), String> {
    use aegis_model::system::SystemAction;

    action.validate().map_err(str::to_owned)?;
    validate_session_boundary(server.session_lock_confirmed(), &action)?;
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
        SystemAction::SetOutputPower { powered } => {
            host.set_outputs_powered(powered)?;
        }
        SystemAction::SetIdleInhibit { inhibit } => {
            // The session-owned "always on" toggle: held in the same
            // registry as connection-scoped IPC inhibitors under a reserved
            // id, so both sources fold into one effective flag and a
            // disconnecting IPC client can never clear the panel's toggle.
            // The status snapshot mirrors the session toggle only, so the
            // command panel's checkbox tracks the user's own setting.
            let effective = idle_inhibits.set(super::idle::SESSION_IDLE_INHIBIT_ID, inhibit);
            server.set_ipc_idle_inhibit(effective);
            status.idle_inhibited = inhibit;
        }
    }
    Ok(())
}

fn validate_session_boundary(
    lock_confirmed: bool,
    action: &aegis_model::system::SystemAction,
) -> Result<(), String> {
    if matches!(
        action,
        aegis_model::system::SystemAction::SetOutputPower { powered: false }
    ) && !lock_confirmed
    {
        Err("output power-off requires a confirmed session lock".into())
    } else {
        Ok(())
    }
}

/// Spawn a short-lived host control command without blocking the compositor
/// main loop, while still reaping the child to avoid zombie accumulation.
///
/// `Command::spawn` returns a `Child` handle whose drop does **not** wait for
/// the process: dropping it orphan-style leaves the kernel with no `waitpid`
/// caller, so every `wpctl set-volume` / `brightnessctl` / `nmcli` the user
/// triggers becomes a `<defunct>` entry that lingers until the compositor
/// exits. (On a long session this leaked dozens of `wpctl` zombies.) Reaping
/// matters even though the commands themselves are milliseconds: an unbounded
/// zombie count eventually exhausts the per-process PID table.
///
/// Blocking in `Command::status` would reclaim the child but stalls the frame
/// loop on slow hosts (`nmcli radio` can take tens of milliseconds). Instead
/// the `Child` is handed to a single long-lived background reaper thread that
/// waits on each command in arrival order. The compositor returns immediately;
/// the zombies never form. The thread is lazily spawned on first use and dies
/// naturally when the sender side is dropped (process teardown).
fn spawn_host_command(program: &str, args: &[&str]) -> Result<(), String> {
    let child = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|error| format!("failed to start {program}: {error}"))?;
    host_command_reaper().reap(child);
    Ok(())
}

/// A lazily-initialized single worker that reaps fire-and-forget host
/// commands, keeping the compositor frame loop off the `waitpid` path.
struct HostCommandReaper {
    tx: std::sync::mpsc::Sender<std::process::Child>,
}

impl HostCommandReaper {
    fn reap(&self, child: std::process::Child) {
        // A send failure means the reaper thread exited (process teardown);
        // there is nothing useful to do with the handle then, so ignore it.
        let _ = self.tx.send(child);
    }
}

fn host_command_reaper() -> &'static HostCommandReaper {
    use std::sync::OnceLock;
    static REAPER: OnceLock<HostCommandReaper> = OnceLock::new();
    REAPER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<std::process::Child>();
        std::thread::Builder::new()
            .name("aegis-host-cmd-reaper".into())
            .spawn(move || {
                // waitpid each command in arrival order. `rx.recv` returns
                // `None` once every sender is gone (compositor shutting down),
                // so the thread drains anything still pending and exits.
                while let Ok(mut child) = rx.recv() {
                    // Ignore the status: these are fire-and-forget controls
                    // whose effect is reconciled by the status poller. We only
                    // need the kernel-side reaping.
                    let _ = child.wait();
                }
            })
            .expect("spawn host-command reaper");
        HostCommandReaper { tx }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use aegis_model::system::SystemAction;

    #[test]
    fn output_power_requires_confirmed_secure_presentation() {
        let action = SystemAction::SetOutputPower { powered: false };
        assert!(validate_session_boundary(false, &action).is_err());
        assert!(validate_session_boundary(true, &action).is_ok());
        assert!(
            validate_session_boundary(false, &SystemAction::SetOutputPower { powered: true })
                .is_ok()
        );
        assert!(validate_session_boundary(false, &SystemAction::ToggleMute).is_ok());
    }
}
