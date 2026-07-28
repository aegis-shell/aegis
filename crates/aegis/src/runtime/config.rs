use super::*;

/// Resolve `--backend auto|drm|nested`, falling back to `$ASS_BACKEND` and
/// then `auto`. X11/XWayland are intentionally not accepted backends.
pub(super) fn requested_backend() -> Result<BackendKind, Box<dyn std::error::Error>> {
    let mut selected = std::env::var("ASS_BACKEND").unwrap_or_else(|_| "auto".to_owned());
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        if let Some(value) = argument.strip_prefix("--backend=") {
            selected = value.to_owned();
        } else if argument == "--backend" {
            selected = args.next().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "--backend requires auto, drm, or nested",
                )
            })?;
        } else if argument == "--help" || argument == "-h" {
            println!("Usage: ass [--backend auto|drm|nested]");
            std::process::exit(0);
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unknown option {argument:?}; try --help"),
            )
            .into());
        }
    }
    Ok(selected.parse()?)
}

/// `[[output]]` mode requests as the backend's connector → `ModeSpec` map
/// (ADR-0028). Entries without a `mode` use the connector's highest-pixel
/// mode at its highest refresh rate.
pub(super) fn configured_output_modes(
    config: Option<&aegis_config::Config>,
) -> std::collections::HashMap<String, aegis_core::output::ModeSpec> {
    config
        .map(|c| c.output_policies())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(connector, policy)| policy.mode.map(|mode| (connector, mode)))
        .collect()
}

/// Generate a timestamped screenshot filename inside `dir`, creating the
/// directory if it does not exist.
pub(super) fn screenshot_path(dir: &std::path::Path) -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let _ = std::fs::create_dir_all(dir);
    dir.join(format!("ass-{ms}.png"))
        .to_string_lossy()
        .into_owned()
}

/// Load the configuration from `path`, logging diagnostics on failure.
/// `None` (no path, or a file that does not exist) means "use built-in
/// defaults" and is not an error.
pub(super) fn load_config(path: Option<&std::path::Path>) -> Option<aegis_config::Config> {
    let path = path?;
    match aegis_config::load(path) {
        Ok(Some(c)) => {
            log::info!("config: loaded {}", path.display());
            Some(c)
        }
        Ok(None) => None,
        Err(e) => {
            match &e {
                aegis_config::LoadError::Invalid { diagnostics, .. } => {
                    for d in diagnostics {
                        log::warn!("config: {d}");
                    }
                }
                _ => log::warn!("config: {e}"),
            }
            log::warn!("config: using built-in defaults");
            None
        }
    }
}

/// Re-load `path` and, on success, swap in the new config and rebuild the
/// keymap. On failure, keep the previous config and keymap.
pub(super) fn reload_config(
    path: &std::path::Path,
    config: &mut Option<aegis_config::Config>,
    keymap: &mut aegis_core::keybind::Keymap,
    server: &mut aegis_compositor::Server,
    shell: &mut aegis_shell::Shell,
    cursor_cache: &mut cursor::CursorCache,
) -> bool {
    let apply = |config: &Option<aegis_config::Config>,
                 server: &mut aegis_compositor::Server,
                 shell: &mut aegis_shell::Shell,
                 cursor_cache: &mut cursor::CursorCache| {
        server.set_window_rules(
            config
                .as_ref()
                .map(|c| c.window_rules.clone())
                .unwrap_or_default(),
        );
        if let Some(c) = config.as_ref() {
            server.set_layout_params(c.layout.clone().into());
            server.set_tiling_default(c.layout.default_tiled);
            server.set_remember_window_positions(c.layout.remember_window_positions);
            shell.set_reduced_motion(c.ui.reduced_motion);
            server.set_reduced_motion(c.ui.reduced_motion);
            server.set_decoration_policy(c.ui.window_decorations);
            cursor_cache.set_config(c.ui.cursor_theme.clone(), c.ui.cursor_size);
            server.set_output_policies(c.output_policies());
        } else {
            server.set_layout_params(aegis_core::layout::LayoutParams::default());
            server.set_tiling_default(false);
            server.set_remember_window_positions(true);
            shell.set_reduced_motion(false);
            server.set_reduced_motion(false);
            server.set_decoration_policy(aegis_core::window::DecorationPolicy::default());
            cursor_cache.set_config(None, None);
            server.set_output_policies(std::collections::HashMap::new());
        }
    };
    match aegis_config::load(path) {
        Ok(Some(new_cfg)) => {
            log::info!("config: reloaded {}", path.display());
            *config = Some(new_cfg);
            *keymap = build_keymap(config.as_ref());
            apply(config, server, shell, cursor_cache);
            true
        }
        Ok(None) => {
            log::warn!("config: {} removed; reverting to defaults", path.display());
            *config = None;
            *keymap = build_keymap(config.as_ref());
            apply(config, server, shell, cursor_cache);
            true
        }
        Err(e) => {
            match &e {
                aegis_config::LoadError::Invalid { diagnostics, .. } => {
                    for d in diagnostics {
                        log::warn!("config: {d}");
                    }
                }
                _ => log::warn!("config: {e}"),
            }
            log::warn!("config: reload failed; keeping previous configuration");
            false
        }
    }
}

/// Persist and apply one validated System Settings display edit through the
/// same configuration path used by startup and external file changes.
/// Explicit field borrows let this run after chrome rendering while the
/// current Flux frame still borrows the unrelated presentation surface.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_display_settings(
    settings: aegis_shell::DisplaySettings,
    config_path: Option<&std::path::Path>,
    config_writer: &ConfigWriter,
    config: &mut Option<aegis_config::Config>,
    keymap: &mut aegis_core::keybind::Keymap,
    server: &mut aegis_compositor::Server,
    shell: &mut aegis_shell::Shell,
    cursor_cache: &mut cursor::CursorCache,
    host: &mut Host,
    reload: &mut Option<aegis_config::ReloadWatcher>,
    live: &std::sync::Arc<LiveState>,
    system_status: &mut aegis_shell::SystemStatus,
    input_acc: &mut InputAccumulator,
) -> Result<(), String> {
    if host.name() != "drm" {
        return Err("the outer compositor owns display settings in a nested session".into());
    }
    let path = config_path
        .map(std::path::Path::to_path_buf)
        .ok_or_else(|| "no writable configuration path is available".to_owned())?;
    // Persist through the serialized config-write worker (same queue as dock
    // pins and touchpad) and block on the receipt, so this read-modify-write
    // cannot lose a concurrent edit and the settings reply stays synchronous.
    config_writer.apply_and_wait(aegis_config::ConfigEdit::SetOutput { settings })?;

    if !reload_config(&path, config, keymap, server, shell, cursor_cache) {
        return Err("the saved display configuration could not be reloaded".into());
    }
    // Reset the watcher baseline after our own atomic replacement so it does
    // not apply the same edit again on the next frame.
    *reload = Some(aegis_config::ReloadWatcher::at(&path));
    host.set_configured_modes(configured_output_modes(config.as_ref()));
    server.set_outputs(host.output_infos());
    let outputs = server.output_infos();
    live.set_outputs(outputs.clone());
    if let Some(logical) = outputs.first().map(|output| output.geometry.logical_size()) {
        input_acc.display_size = (logical.w.max(1) as f32, logical.h.max(1) as f32);
    }
    system_status.display = aegis_shell::DisplayStatus {
        configurable: true,
        outputs,
        error: None,
    };
    Ok(())
}

/// Build the nested output geometry from its logical surface size and the
/// host's preferred render scale. `wl_output.mode` is expressed in physical
/// pixels while xdg-output derives the original logical size by dividing by
/// `scale`; keeping both in one constructor prevents the two coordinate spaces
/// from silently drifting apart.
#[cfg(test)]
pub(super) fn output_geometry_from_host(
    logical_w: i32,
    logical_h: i32,
    scale: f32,
) -> aegis_core::output::OutputGeometry {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    aegis_core::output::OutputGeometry {
        mode: aegis_core::output::OutputMode {
            width: (logical_w.max(1) as f32 * scale).round() as i32,
            height: (logical_h.max(1) as f32 * scale).round() as i32,
            refresh_mhz: 0,
        },
        scale: aegis_core::output::Scale(scale),
        transform: aegis_core::Transform::Normal,
        logical_origin: aegis_core::Point::default(),
    }
}

/// Build the active keymap from the config file's `[[keybind]]` entries,
/// layered over the built-in defaults. The deprecated `$ASS_KEYBINDS` env
/// var is honored as a transitional override that takes precedence over the
/// file (ADR-0026); it is logged and removed before the desktop phase
/// closes.
pub(super) fn build_keymap(config: Option<&aegis_config::Config>) -> aegis_core::keybind::Keymap {
    let mut overrides: Vec<aegis_core::keybind::Keybind> = Vec::new();

    // Deprecated env override — highest precedence so existing setups keep
    // working during the transition.
    if let Ok(s) = std::env::var("ASS_KEYBINDS")
        && !s.trim().is_empty()
    {
        log::warn!(
            "keybind: $ASS_KEYBINDS is deprecated; move it to the \
                 `[[keybind]]` section of the config file"
        );
        let (env_binds, errs) = aegis_core::keybind::Keymap::parse_overrides(&s);
        for e in &errs {
            log::warn!("keybind: {e}");
        }
        overrides.extend(env_binds);
    }

    // Config-file overrides — below the env override.
    if let Some(cfg) = config {
        let (cfg_binds, errs) = cfg.resolve_keybinds();
        for e in &errs {
            log::warn!("config: {e}");
        }
        overrides.extend(cfg_binds);
    }

    if overrides.is_empty() {
        aegis_core::keybind::Keymap::defaults()
    } else {
        log::info!("keybinds: {} override(s) applied", overrides.len());
        aegis_core::keybind::Keymap::defaults().with_overrides(overrides)
    }
}

/// Compile the trusted named IPC scopes from configuration. Invalid operation
/// names are ignored inside an explicit allowlist (therefore granting nothing
/// for that entry) and logged; they never turn into an unrestricted scope.
pub(super) fn build_ipc_scopes(
    config: Option<&aegis_config::Config>,
) -> std::collections::HashMap<String, aegis_ipc::Scope> {
    let mut scopes = std::collections::HashMap::from([
        (
            aegis_ipc::LOCAL_REALM_ADMIN_SCOPE.to_string(),
            aegis_ipc::Scope {
                windows: None,
                workspaces: None,
                outputs: None,
                realms: None,
                ops: Some(vec![
                    aegis_ipc::OpClass::InjectRealmInput,
                    aegis_ipc::OpClass::CreateRealm,
                    aegis_ipc::OpClass::TransactRealm,
                    aegis_ipc::OpClass::RevokeRealm,
                    aegis_ipc::OpClass::CaptureRealm,
                    aegis_ipc::OpClass::LaunchInRealm,
                ]),
            },
        ),
        // The portal backend (ADR-0051/0052/0053/0054) serves Screenshot,
        // ScreenCast, the Inhibit portal's global idle inhibitor, and the
        // interactive picker round-trips through exactly four fail-closed
        // operations; `None`-means-all would grant it nothing, so the ops
        // must be listed explicitly here.
        (
            aegis_ipc::LOCAL_PORTAL_SCOPE.to_string(),
            aegis_ipc::Scope {
                windows: None,
                workspaces: None,
                outputs: None,
                realms: None,
                ops: Some(vec![
                    aegis_ipc::OpClass::CaptureOutput,
                    aegis_ipc::OpClass::StreamOutput,
                    aegis_ipc::OpClass::IdleInhibit,
                    aegis_ipc::OpClass::PickTarget,
                ]),
            },
        ),
    ]);
    let Some(config) = config else {
        return scopes;
    };

    for declared in &config.agent.scopes {
        let name = declared.name.trim();
        if name.is_empty() {
            log::warn!("config: ignoring agent scope with an empty name");
            continue;
        }
        if scopes.contains_key(name) {
            log::warn!("config: duplicate agent scope '{name}' ignored");
            continue;
        }

        let ops = if declared.ops.is_empty() {
            None
        } else {
            Some(
                declared
                    .ops
                    .iter()
                    .filter_map(|op| match ipc_op_class(op) {
                        Some(op) => Some(op),
                        None => {
                            log::warn!("config: agent scope '{name}' has unknown operation '{op}'");
                            None
                        }
                    })
                    .collect(),
            )
        };
        let windows = (!declared.windows.is_empty()).then(|| {
            declared
                .windows
                .iter()
                .copied()
                .map(aegis_core::window::WindowId)
                .collect()
        });
        let workspaces = (!declared.workspaces.is_empty()).then(|| {
            declared
                .workspaces
                .iter()
                .copied()
                .map(aegis_core::workspace::WorkspaceId)
                .collect()
        });
        let realms = (!declared.realms.is_empty()).then(|| {
            declared
                .realms
                .iter()
                .copied()
                .map(aegis_core::realm::RealmId)
                .collect()
        });
        scopes.insert(
            name.to_string(),
            aegis_ipc::Scope {
                windows,
                workspaces,
                outputs: None,
                realms,
                ops,
            },
        );
    }
    scopes
}

pub(super) fn authorize_realm_action_against_snapshot(
    scope: &aegis_ipc::Scope,
    action: &aegis_ipc::RealmAction,
    snapshot: &aegis_core::realm::RealmSnapshot,
) -> Result<(), String> {
    if !scope.permits_realm_action(action) {
        return Err("out of scope".into());
    }
    let aegis_ipc::RealmAction::Transact { mutations, .. } = action else {
        return Ok(());
    };
    for mutation in mutations {
        let group = match mutation {
            aegis_core::realm::RealmMutation::TransferWindow { window, .. } => snapshot
                .interaction_groups
                .iter()
                .find(|group| group.windows.contains(window)),
            aegis_core::realm::RealmMutation::SetObserver { group, .. } => snapshot
                .interaction_groups
                .iter()
                .find(|candidate| candidate.id == *group),
            aegis_core::realm::RealmMutation::ConfigureOutput { .. }
            | aegis_core::realm::RealmMutation::SetState { .. } => None,
        };
        if group.is_some_and(|group| {
            group
                .windows
                .iter()
                .any(|window| !scope.permits_window(*window))
        }) {
            return Err(
                "out of scope: Realm mutation affects another interaction-group window".into(),
            );
        }
    }
    Ok(())
}

pub(super) fn ipc_op_class(name: &str) -> Option<aegis_ipc::OpClass> {
    use aegis_ipc::OpClass;
    match name.trim().to_ascii_lowercase().as_str() {
        "focus" => Some(OpClass::Focus),
        "minimize" => Some(OpClass::Minimize),
        "close" => Some(OpClass::Close),
        "move" => Some(OpClass::Move),
        "setwindowgeometry" | "set_window_geometry" => Some(OpClass::SetWindowGeometry),
        "injectinput" | "inject_input" => Some(OpClass::InjectInput),
        "injectrealminput" | "inject_realm_input" => Some(OpClass::InjectRealmInput),
        "createrealm" | "create_realm" => Some(OpClass::CreateRealm),
        "transactrealm" | "transact_realm" => Some(OpClass::TransactRealm),
        "revokerealm" | "revoke_realm" => Some(OpClass::RevokeRealm),
        "capturerealm" | "capture_realm" => Some(OpClass::CaptureRealm),
        "launchinrealm" | "launch_in_realm" => Some(OpClass::LaunchInRealm),
        "cycle" => Some(OpClass::Cycle),
        "switchworkspace" | "switch_workspace" => Some(OpClass::SwitchWorkspace),
        "switchworkspaceto" | "switch_workspace_to" => Some(OpClass::SwitchWorkspaceTo),
        "movetoworkspace" | "move_to_workspace" => Some(OpClass::MoveToWorkspace),
        "toggletiling" | "toggle_tiling" => Some(OpClass::ToggleTiling),
        "systemcontrol" | "system_control" => Some(OpClass::SystemControl),
        "notify" => Some(OpClass::Notify),
        "dismissnotification" | "dismiss_notification" => Some(OpClass::DismissNotification),
        "streamoutput" | "stream_output" => Some(OpClass::StreamOutput),
        "idleinhibit" | "idle_inhibit" => Some(OpClass::IdleInhibit),
        "picktarget" | "pick_target" => Some(OpClass::PickTarget),
        _ => None,
    }
}
