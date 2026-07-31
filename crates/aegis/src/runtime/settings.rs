use super::*;

impl CompositorRuntime {
    pub(super) fn publish_settings(&mut self) {
        publish_settings_parts(
            self.settings_revision,
            &self.system_status,
            self.config.as_ref(),
            &mut self.shell,
            &self.live,
            &self.ipc,
        );
    }

    /// Persist and apply one settings transaction. The main loop is the only
    /// writer, so checking and advancing the revision here is atomic with the
    /// actual backend/config mutation.
    pub(super) fn commit_settings(
        &mut self,
        expected_revision: Option<u64>,
        action: aegis_ipc::SettingsAction,
    ) -> Result<aegis_ipc::SettingsReceipt, String> {
        commit_settings_parts(
            expected_revision,
            action,
            &mut self.settings_revision,
            self.config_path.as_deref(),
            &self.config_writer,
            &mut self.config,
            &mut self.keymap,
            &mut self.gesture_map,
            &mut self.server,
            &mut self.shell,
            &mut self.cursor_cache,
            &mut self.host,
            &mut self.reload,
            &mut self.idle_process,
            &self.live,
            &mut self.system_status,
            &mut self.input_acc,
            &self.ipc,
        )
    }
}

pub(super) fn publish_settings_parts(
    revision: u64,
    status: &aegis_shell::SystemStatus,
    config: Option<&aegis_config::Config>,
    shell: &mut aegis_shell::Shell,
    live: &std::sync::Arc<LiveState>,
    ipc: &Option<aegis_ipc::Server>,
) {
    let snapshot = aegis_ipc::SettingsSnapshot {
        revision,
        touchpad: status.touchpad.clone(),
        display: status.display.clone(),
        preferences: effective_desktop_preferences(config),
        idle: config.map(|config| config.idle).unwrap_or_default(),
    };
    live.set_settings(snapshot.clone());
    publish_system_status_parts(status, shell, live, ipc);
    if let Some(ipc) = ipc {
        ipc.broadcast(aegis_ipc::Event::SettingsChanged {
            revision: snapshot.revision,
        });
    }
}

/// Disjoint-field form used while a Flux frame borrows `surface`. Keeping the
/// arguments explicit lets the renderer borrow and settings mutation coexist
/// without turning the whole runtime into one mutable borrow.
#[allow(clippy::too_many_arguments)]
pub(super) fn commit_settings_parts(
    expected_revision: Option<u64>,
    action: aegis_ipc::SettingsAction,
    revision: &mut u64,
    config_path: Option<&std::path::Path>,
    config_writer: &ConfigWriter,
    config: &mut Option<aegis_config::Config>,
    keymap: &mut aegis_core::keybind::Keymap,
    gesture_map: &mut aegis_core::gesture::GestureMap,
    server: &mut aegis_compositor::Server,
    shell: &mut aegis_shell::Shell,
    cursor_cache: &mut crate::cursor::CursorCache,
    host: &mut Host,
    reload: &mut Option<aegis_config::ReloadWatcher>,
    idle_process: &mut session::IdleProcess,
    live: &std::sync::Arc<LiveState>,
    status: &mut aegis_shell::SystemStatus,
    input_acc: &mut InputAccumulator,
    ipc: &Option<aegis_ipc::Server>,
) -> Result<aegis_ipc::SettingsReceipt, String> {
    if expected_revision.is_some_and(|expected| expected != *revision) {
        return Err(format!(
            "settings revision conflict: expected {}, actual {}",
            expected_revision.unwrap(),
            *revision
        ));
    }
    action.validate().map_err(str::to_owned)?;
    let display_action = matches!(&action, aegis_ipc::SettingsAction::SetDisplay { .. });

    let result = match action {
        aegis_ipc::SettingsAction::SetTouchpad { config: touchpad } => {
            // The TOML rewrite runs on the serialized config-write worker so
            // it cannot interleave with dock-pin writes; the receipt keeps
            // the IPC reply synchronous.
            config_writer
                .apply_and_wait(aegis_config::ConfigEdit::SetTouchpad { config: touchpad })
                .map_err(|error| format!("failed to persist touchpad settings: {error}"))?;
            if let Some(current) = config.as_mut() {
                current.input.touchpad = touchpad;
            }
            status.touchpad = host.set_touchpad_config(touchpad);
            Ok(())
        }
        aegis_ipc::SettingsAction::SetDisplay { settings } => apply_display_settings(
            settings,
            config_path,
            config_writer,
            config,
            keymap,
            gesture_map,
            server,
            shell,
            cursor_cache,
            host,
            reload,
            live,
            status,
            input_acc,
        ),
        aegis_ipc::SettingsAction::SetDesktopPreferences { preferences } => {
            apply_desktop_preferences(
                preferences,
                config_path,
                config_writer,
                config,
                keymap,
                gesture_map,
                server,
                shell,
                cursor_cache,
                reload,
            )
        }
        aegis_ipc::SettingsAction::SetIdle { settings } => {
            config_writer
                .apply_and_wait(aegis_config::ConfigEdit::SetIdle { settings })
                .map_err(|error| format!("failed to persist idle settings: {error}"))?;
            if let Some(current) = config.as_mut() {
                current.idle = settings;
            } else if let Some(path) = config_path {
                *config = aegis_config::load(path)
                    .map_err(|error| format!("failed to reload idle settings: {error}"))?;
            }
            idle_process.reconfigure(settings);
            Ok(())
        }
    };

    match result {
        Ok(()) => {
            *revision = revision.saturating_add(1);
            if display_action {
                status.display.error = None;
            }
            publish_settings_parts(*revision, status, config.as_ref(), shell, live, ipc);
            Ok(aegis_ipc::SettingsReceipt {
                revision: *revision,
            })
        }
        Err(error) => {
            if display_action {
                status.display.error = Some(error.clone());
            }
            publish_settings_parts(*revision, status, config.as_ref(), shell, live, ipc);
            Err(error)
        }
    }
}
