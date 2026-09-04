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
        action: tessera_ipc::SettingsAction,
    ) -> Result<tessera_ipc::SettingsReceipt, String> {
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
    status: &tessera_shell::SystemStatus,
    config: Option<&tessera_config::Config>,
    shell: &mut tessera_shell::Shell,
    live: &std::sync::Arc<LiveState>,
    ipc: &Option<tessera_ipc::Server>,
) {
    let snapshot = tessera_ipc::SettingsSnapshot {
        revision,
        input: status.input.clone(),
        display: status.display.clone(),
        preferences: effective_desktop_preferences(config),
        idle: config.map(|config| config.idle).unwrap_or_default(),
        dock: config
            .map(|config| tessera_model::settings::DockSettings {
                minimize_animation: config.dock.minimize_animation,
            })
            .unwrap_or_default(),
    };
    live.set_settings(snapshot.clone());
    shell.set_settings(snapshot.clone());
    publish_system_status_parts(status, shell, live, ipc);
    if let Some(ipc) = ipc {
        ipc.broadcast(tessera_ipc::Event::SettingsChanged {
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
    action: tessera_ipc::SettingsAction,
    revision: &mut u64,
    config_path: Option<&std::path::Path>,
    config_writer: &ConfigWriter,
    config: &mut Option<tessera_config::Config>,
    keymap: &mut tessera_model::keybind::Keymap,
    gesture_map: &mut tessera_model::gesture::GestureMap,
    server: &mut tessera_compositor::Server,
    shell: &mut tessera_shell::Shell,
    cursor_cache: &mut crate::cursor::CursorCache,
    host: &mut Host,
    reload: &mut Option<tessera_config::ReloadWatcher>,
    idle_process: &mut session::IdleProcess,
    live: &std::sync::Arc<LiveState>,
    status: &mut tessera_shell::SystemStatus,
    input_acc: &mut InputAccumulator,
    ipc: &Option<tessera_ipc::Server>,
) -> Result<tessera_ipc::SettingsReceipt, String> {
    if expected_revision.is_some_and(|expected| expected != *revision) {
        return Err(format!(
            "settings revision conflict: expected {}, actual {}",
            expected_revision.unwrap(),
            *revision
        ));
    }
    action.validate().map_err(str::to_owned)?;
    let display_action = matches!(&action, tessera_ipc::SettingsAction::SetDisplay { .. });

    let result = match action {
        tessera_ipc::SettingsAction::SetInput { config: input } => {
            // The TOML rewrite runs on the serialized config-write worker so
            // it cannot interleave with dock-pin writes; the receipt keeps
            // the IPC reply synchronous.
            config_writer
                .apply_and_wait(tessera_config::ConfigEdit::SetInput {
                    touchpad: input.touchpad,
                    mouse: input.mouse,
                    keyboard: input.keyboard,
                })
                .map_err(|error| format!("failed to persist input settings: {error}"))?;
            if let Some(current) = config.as_mut() {
                current.input = input;
            }
            // Keyboard repeat is a server-side advertisement; push it to the
            // compositor so bound clients re-learn the rate.
            server.set_keyboard_repeat(input.keyboard);
            status.input = host.set_input_config(input);
            // Backends without a keyboard device model cannot know the
            // persisted profile; the runtime is authoritative for it.
            status.input.keyboard = input.keyboard;
            Ok(())
        }
        tessera_ipc::SettingsAction::SetDisplay { settings } => apply_display_settings(
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
        tessera_ipc::SettingsAction::SetDesktopPreferences { preferences } => {
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
        tessera_ipc::SettingsAction::SetIdle { settings } => {
            config_writer
                .apply_and_wait(tessera_config::ConfigEdit::SetIdle { settings })
                .map_err(|error| format!("failed to persist idle settings: {error}"))?;
            if let Some(current) = config.as_mut() {
                current.idle = settings;
            } else if let Some(path) = config_path {
                *config = tessera_config::load(path)
                    .map_err(|error| format!("failed to reload idle settings: {error}"))?;
            }
            idle_process.reconfigure(settings);
            Ok(())
        }
        tessera_ipc::SettingsAction::SetDock { settings } => {
            config_writer
                .apply_and_wait(tessera_config::ConfigEdit::SetDockMinimizeAnimation {
                    style: settings.minimize_animation,
                })
                .map_err(|error| format!("failed to persist dock settings: {error}"))?;
            if let Some(current) = config.as_mut() {
                current.dock.minimize_animation = settings.minimize_animation;
            }
            server.set_minimize_animation(settings.minimize_animation);
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
            Ok(tessera_ipc::SettingsReceipt {
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
