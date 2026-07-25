use super::*;

/// Direct swapchain composition. A model wallpaper inserts one depth-tested
/// pass between the 2D background and client canvas draws.
#[derive(Clone, Copy)]
pub(super) struct RenderGeometry {
    pub(super) logical_size: (u32, u32),
    pub(super) scale: f32,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_direct_desktop_scene(
    canvas: &flux::Canvas,
    device: &flux::Device,
    frame: &mut flux::Frame<'_>,
    wallpaper: &mut Option<aegis_wallpaper::Wallpaper>,
    renderer: &mut aegis_render::Renderer,
    server: &aegis_compositor::Server,
    geometry: RenderGeometry,
    overview: bool,
) -> Result<(), flux::Error> {
    let RenderGeometry {
        logical_size,
        scale,
    } = geometry;
    draw_wallpaper_background(canvas, device, wallpaper, logical_size, scale);
    if wallpaper
        .as_ref()
        .is_some_and(|wallpaper| wallpaper.has_model())
    {
        canvas.end();
        if let Some(wallpaper) = wallpaper.as_mut() {
            wallpaper.draw_model(device, frame);
        }
        canvas.begin(frame, None)?;
    }
    if overview {
        draw_overview_scene(canvas, device, renderer, server, logical_size, scale);
    } else {
        draw_client_scene(canvas, device, renderer, server, scale);
    }
    Ok(())
}

pub(super) fn physical_window_target(cmd: &aegis_ipc::Command) -> Option<aegis_core::window::WindowId> {
    use aegis_ipc::Command;
    match cmd {
        Command::Focus { id }
        | Command::Minimize { id }
        | Command::Close { id }
        | Command::Move { id }
        | Command::SetWindowGeometry { id, .. } => Some(*id),
        Command::MoveToWorkspace { window, .. } => Some(*window),
        _ => None,
    }
}

/// Dispatch an [`aegis_ipc::Command`] to the server and side-effect targets. Extracted
/// from the three mutation sources (IPC, keybindings, chrome) so both the
/// physical-seat authority check and journal chokepoint (ADR-0033) are shared.
pub(super) fn apply_command(
    server: &mut aegis_compositor::Server,
    notif_queue: &std::sync::Arc<std::sync::Mutex<aegis_core::notify::NotificationQueue>>,
    quit: &mut bool,
    cmd: &aegis_ipc::Command,
    ipc: &Option<aegis_ipc::Server>,
    ts_mono_ms: u64,
) -> Result<(), String> {
    if physical_window_target(cmd).is_some_and(|window| !server.human_controls_window(window)) {
        return Err("physical seat has observation-only authority for this window".into());
    }

    use aegis_ipc::Command;
    match cmd {
        Command::Focus { id } => server.focus_surface_by_id(*id),
        Command::Minimize { id } => server.minimize_toplevel(*id),
        Command::Close { id } => server.close_toplevel(*id),
        Command::Move { id } => server.start_interactive_move(*id),
        Command::SetWindowGeometry { id, rect } => {
            server.set_window_geometry(*id, *rect);
        }
        Command::InjectInput { .. } | Command::InjectRealmInput { .. } => {
            // Synthetic input needs shell-occlusion validation and is handled
            // beside the physical-input router in the main loop.
            debug_assert!(false, "InjectInput reached the generic command path");
        }
        Command::LaunchInRealm { .. } => {
            debug_assert!(false, "LaunchInRealm reached the generic command path");
        }
        Command::Screenshot { .. } => {
            // Screenshots need the GPU objects and are handled beside the
            // frame renderer in the main loop.
            debug_assert!(false, "Screenshot reached the generic command path");
        }
        Command::ToggleOverview => {
            // The overview is shell-owned; toggled beside the IPC drain.
            debug_assert!(false, "ToggleOverview reached the generic command path");
        }
        Command::Cycle { forward } => server.cycle_focus(*forward),
        Command::SwitchWorkspace { dir } => server.switch_workspace(*dir),
        Command::SwitchWorkspaceTo { id } => server.switch_workspace_to(*id),
        Command::MoveToWorkspace { window, workspace } => {
            server.move_to_workspace(*window, *workspace)
        }
        Command::ToggleTiling => server.set_tiling(!server.tiling()),
        Command::Notify {
            summary,
            body,
            app_id,
        } => {
            let n = notif_queue.lock().unwrap().push(
                summary.clone(),
                body.clone(),
                app_id.clone(),
                ts_mono_ms,
            );
            if let Some(s) = ipc.as_ref() {
                s.broadcast(aegis_ipc::Event::Notified { notification: n });
            }
        }
        Command::DismissNotification { id } => {
            notif_queue.lock().unwrap().dismiss(*id);
        }
        Command::Quit => *quit = true,
    }
    Ok(())
}

pub(super) fn apply_realm_action(
    server: &mut aegis_compositor::Server,
    action: aegis_ipc::RealmAction,
) -> Result<aegis_ipc::RealmActionResult, String> {
    match action {
        aegis_ipc::RealmAction::Create {
            label,
            capabilities,
            output,
        } => {
            let bundle = server
                .create_agent_realm(label, capabilities)
                .map_err(|error| error.to_string())?;
            if let Some(output) = output
                && let Err(error) = server.configure_realm_output(bundle.realm, output)
            {
                let _ = server.revoke_realm(bundle.realm, aegis_core::realm::HUMAN_REALM);
                return Err(error.to_string());
            }
            Ok(aegis_ipc::RealmActionResult::Created { bundle })
        }
        aegis_ipc::RealmAction::Transact {
            expected_revision,
            mutations,
        } => server
            .transact_realms(expected_revision, &mutations)
            .map(|receipt| aegis_ipc::RealmActionResult::TransactionCommitted { receipt })
            .map_err(|error| error.to_string()),
        aegis_ipc::RealmAction::Revoke {
            realm,
            fallback,
            expected_revision,
        } => {
            let actual = server.realm_snapshot().revision;
            if expected_revision.is_some_and(|expected| expected != actual) {
                return Err(format!(
                    "Realm revision conflict: expected {}, actual {actual}",
                    expected_revision.unwrap()
                ));
            }
            server
                .revoke_realm(realm, fallback)
                .map(|receipt| aegis_ipc::RealmActionResult::Revoked { receipt })
                .map_err(|error| error.to_string())
        }
    }
}

pub(super) fn realm_intent_to_action(intent: aegis_shell::RealmIntent) -> aegis_ipc::RealmAction {
    match intent {
        aegis_shell::RealmIntent::Create { label } => aegis_ipc::RealmAction::Create {
            label,
            capabilities: aegis_core::realm::SeatCapabilities::POINTER_KEYBOARD,
            output: Some(aegis_core::realm::VirtualOutput::DEFAULT_AGENT),
        },
        aegis_shell::RealmIntent::SetState {
            realm,
            state,
            expected_revision,
        } => aegis_ipc::RealmAction::Transact {
            expected_revision: Some(expected_revision),
            mutations: vec![aegis_core::realm::RealmMutation::SetState { realm, state }],
        },
        aegis_shell::RealmIntent::Revoke {
            realm,
            expected_revision,
        } => aegis_ipc::RealmAction::Revoke {
            realm,
            fallback: aegis_core::realm::HUMAN_REALM,
            expected_revision: Some(expected_revision),
        },
        aegis_shell::RealmIntent::TransferWindow {
            window,
            target,
            retain_source_as_observer,
            expected_revision,
        } => aegis_ipc::RealmAction::Transact {
            expected_revision: Some(expected_revision),
            mutations: vec![aegis_core::realm::RealmMutation::TransferWindow {
                window,
                target,
                retain_source_as_observer,
            }],
        },
    }
}

pub(super) fn realm_action_invalidates_capture(
    action: &aegis_ipc::RealmAction,
) -> std::collections::BTreeSet<aegis_core::realm::RealmId> {
    match action {
        aegis_ipc::RealmAction::Create { .. } => std::collections::BTreeSet::new(),
        aegis_ipc::RealmAction::Revoke { realm, .. } => std::collections::BTreeSet::from([*realm]),
        aegis_ipc::RealmAction::Transact { mutations, .. } => mutations
            .iter()
            .filter_map(|mutation| match mutation {
                aegis_core::realm::RealmMutation::SetState {
                    realm,
                    state:
                        aegis_core::realm::RealmState::Paused | aegis_core::realm::RealmState::Revoked,
                } => Some(*realm),
                _ => None,
            })
            .collect(),
    }
}

pub(super) fn realms_explicitly_stopped(
    action: &aegis_ipc::RealmAction,
) -> std::collections::BTreeSet<aegis_core::realm::RealmId> {
    match action {
        aegis_ipc::RealmAction::Revoke { realm, .. } => std::collections::BTreeSet::from([*realm]),
        aegis_ipc::RealmAction::Transact { mutations, .. } => mutations
            .iter()
            .filter_map(|mutation| match mutation {
                aegis_core::realm::RealmMutation::SetState {
                    realm,
                    state: aegis_core::realm::RealmState::Paused,
                } => Some(*realm),
                _ => None,
            })
            .collect(),
        aegis_ipc::RealmAction::Create { .. } => std::collections::BTreeSet::new(),
    }
}

/// Record a mutation in the journal and push it to journal subscribers
/// (ADR-0033).
pub(super) fn journal_and_broadcast(
    journal: &std::sync::Arc<std::sync::Mutex<aegis_ipc::Journal>>,
    ipc: &Option<aegis_ipc::Server>,
    ts_mono_ms: u64,
    origin: aegis_ipc::Origin,
    cmd: aegis_ipc::Command,
) {
    journal_effect_and_broadcast(
        journal,
        ipc,
        ts_mono_ms,
        origin,
        cmd,
        aegis_ipc::Effect::Applied,
    );
}

pub(super) fn journal_effect_and_broadcast(
    journal: &std::sync::Arc<std::sync::Mutex<aegis_ipc::Journal>>,
    ipc: &Option<aegis_ipc::Server>,
    ts_mono_ms: u64,
    origin: aegis_ipc::Origin,
    cmd: aegis_ipc::Command,
    effect: aegis_ipc::Effect,
) {
    journal_mutation_effect_and_broadcast(
        journal,
        ipc,
        ts_mono_ms,
        origin,
        aegis_ipc::JournalMutation::Command { cmd },
        effect,
    );
}

pub(super) fn journal_mutation_effect_and_broadcast(
    journal: &std::sync::Arc<std::sync::Mutex<aegis_ipc::Journal>>,
    ipc: &Option<aegis_ipc::Server>,
    ts_mono_ms: u64,
    origin: aegis_ipc::Origin,
    mutation: aegis_ipc::JournalMutation,
    effect: aegis_ipc::Effect,
) {
    let mut j = journal.lock().unwrap();
    let entry = j.append(ts_mono_ms, origin, mutation, effect);
    if let Some(s) = ipc.as_ref() {
        s.broadcast_journal(entry.clone());
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_command_and_journal(
    server: &mut aegis_compositor::Server,
    notifications: &std::sync::Arc<std::sync::Mutex<aegis_core::notify::NotificationQueue>>,
    quit: &mut bool,
    command: aegis_ipc::Command,
    ipc: &Option<aegis_ipc::Server>,
    journal: &std::sync::Arc<std::sync::Mutex<aegis_ipc::Journal>>,
    ts_mono_ms: u64,
    origin: aegis_ipc::Origin,
) {
    let effect = match apply_command(server, notifications, quit, &command, ipc, ts_mono_ms) {
        Ok(()) => aegis_ipc::Effect::Applied,
        Err(reason) => aegis_ipc::Effect::Refused { reason },
    };
    journal_effect_and_broadcast(journal, ipc, ts_mono_ms, origin, command, effect);
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_chrome_window_command(
    server: &mut aegis_compositor::Server,
    notifications: &std::sync::Arc<std::sync::Mutex<aegis_core::notify::NotificationQueue>>,
    quit: &mut bool,
    command: aegis_ipc::Command,
    ipc: &Option<aegis_ipc::Server>,
    journal: &std::sync::Arc<std::sync::Mutex<aegis_ipc::Journal>>,
    ts_mono_ms: u64,
) {
    debug_assert!(physical_window_target(&command).is_some());
    apply_command_and_journal(
        server,
        notifications,
        quit,
        command,
        ipc,
        journal,
        ts_mono_ms,
        aegis_ipc::Origin::Chrome,
    );
}

/// Apply one trusted Control Center mutation. Compositor-native layout changes
/// return an IPC command so they pass through the journal chokepoint; host
/// hardware controls are dispatched through their standard Linux tools.
pub(super) fn apply_system_action(
    server: &mut aegis_compositor::Server,
    notifications: &std::sync::Arc<std::sync::Mutex<aegis_core::notify::NotificationQueue>>,
    status: &mut aegis_shell::SystemStatus,
    action: aegis_shell::SystemAction,
) -> Option<aegis_ipc::Command> {
    use aegis_shell::SystemAction;

    match action {
        SystemAction::ToggleMute => {
            spawn_host_command("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]);
            status.muted = !status.muted;
        }
        SystemAction::StepVolume(delta) => {
            let amount = format!(
                "{}%{}",
                delta.unsigned_abs(),
                if delta >= 0 { "+" } else { "-" }
            );
            spawn_host_command(
                "wpctl",
                &["set-volume", "@DEFAULT_AUDIO_SINK@", &amount, "-l", "1.0"],
            );
            let current = status.volume.unwrap_or(0) as i16;
            status.volume = Some((current + i16::from(delta)).clamp(0, 100) as u8);
        }
        SystemAction::SetVolume(level) => {
            let level = level.min(100);
            let amount = format!("{level}%");
            spawn_host_command(
                "wpctl",
                &["set-volume", "@DEFAULT_AUDIO_SINK@", &amount, "-l", "1.0"],
            );
            status.volume = Some(level);
        }
        SystemAction::SetBrightness(level) => {
            let level = level.clamp(1, 100);
            let amount = format!("{level}%");
            spawn_host_command("brightnessctl", &["--class=backlight", "set", &amount]);
            status.brightness = Some(level);
        }
        SystemAction::SetWifi(enabled) => {
            spawn_host_command(
                "nmcli",
                &["radio", "wifi", if enabled { "on" } else { "off" }],
            );
            status.wifi_enabled = Some(enabled);
        }
        SystemAction::SetBluetooth(enabled) => {
            spawn_host_command(
                "rfkill",
                &[if enabled { "unblock" } else { "block" }, "bluetooth"],
            );
            status.bluetooth_enabled = Some(enabled);
        }
        SystemAction::SetDoNotDisturb(enabled) => {
            notifications.lock().unwrap().set_do_not_disturb(enabled);
            status.do_not_disturb = enabled;
        }
        SystemAction::SetTiling(enabled) => {
            status.tiled = enabled;
            if server.tiling() != enabled {
                return Some(aegis_ipc::Command::ToggleTiling);
            }
        }
        // Touchpad profiles are persisted and applied by the main loop, which
        // owns both the config file and the selected input backend.
        SystemAction::SetTouchpad(_) | SystemAction::SetDisplay(_) => {}
    }
    None
}

pub(super) fn spawn_host_command(program: &str, args: &[&str]) {
    let result = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(error) = result {
        log::warn!("control center: failed to start {program}: {error}");
    }
}
