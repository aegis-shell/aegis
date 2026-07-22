use super::*;

impl CompositorRuntime {
    pub(super) fn publish_settings(&mut self) {
        publish_settings_parts(
            self.settings_revision,
            &self.system_status,
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
        action: ass_ipc::SettingsAction,
    ) -> Result<ass_ipc::SettingsReceipt, String> {
        commit_settings_parts(
            expected_revision,
            action,
            &mut self.settings_revision,
            self.config_path.as_deref(),
            &mut self.config,
            &mut self.keymap,
            &mut self.server,
            &mut self.shell,
            &mut self.cursor_cache,
            &mut self.host,
            &mut self.reload,
            &self.live,
            &mut self.system_status,
            &mut self.input_acc,
            &self.ipc,
        )
    }
}

pub(super) fn publish_settings_parts(
    revision: u64,
    status: &ass_shell::SystemStatus,
    shell: &mut ass_shell::Shell,
    live: &std::sync::Arc<LiveState>,
    ipc: &Option<ass_ipc::Server>,
) {
    let snapshot = ass_ipc::SettingsSnapshot {
        revision,
        touchpad: status.touchpad.clone(),
        display: status.display.clone(),
    };
    live.set_settings(snapshot.clone());
    shell.set_system_status(status.clone());
    if let Some(ipc) = ipc {
        ipc.broadcast(ass_ipc::Event::SettingsChanged {
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
    action: ass_ipc::SettingsAction,
    revision: &mut u64,
    config_path: Option<&std::path::Path>,
    config: &mut Option<ass_config::Config>,
    keymap: &mut ass_core::keybind::Keymap,
    server: &mut ass_server::Server,
    shell: &mut ass_shell::Shell,
    cursor_cache: &mut crate::cursor::CursorCache,
    host: &mut Host,
    reload: &mut Option<ass_config::ReloadWatcher>,
    live: &std::sync::Arc<LiveState>,
    status: &mut ass_shell::SystemStatus,
    input_acc: &mut InputAccumulator,
    ipc: &Option<ass_ipc::Server>,
) -> Result<ass_ipc::SettingsReceipt, String> {
    if expected_revision.is_some_and(|expected| expected != *revision) {
        return Err(format!(
            "settings revision conflict: expected {}, actual {}",
            expected_revision.unwrap(),
            *revision
        ));
    }
    action.validate().map_err(str::to_owned)?;

    let result = match action {
        ass_ipc::SettingsAction::SetTouchpad { config: touchpad } => {
            let path = config_path
                .ok_or_else(|| "cannot persist touchpad settings: no config path".to_owned())?;
            ass_config::set_touchpad_config(path, &touchpad)
                .map_err(|error| format!("failed to persist touchpad settings: {error}"))?;
            if let Some(current) = config.as_mut() {
                current.input.touchpad = touchpad;
            }
            status.touchpad = host.set_touchpad_config(touchpad);
            Ok(())
        }
        ass_ipc::SettingsAction::SetDisplay { settings } => apply_display_settings(
            settings,
            config_path,
            config,
            keymap,
            server,
            shell,
            cursor_cache,
            host,
            reload,
            live,
            status,
            input_acc,
        ),
    };

    match result {
        Ok(()) => {
            *revision = revision.saturating_add(1);
            status.display.error = None;
            publish_settings_parts(*revision, status, shell, live, ipc);
            Ok(ass_ipc::SettingsReceipt {
                revision: *revision,
            })
        }
        Err(error) => {
            status.display.error = Some(error.clone());
            publish_settings_parts(*revision, status, shell, live, ipc);
            Err(error)
        }
    }
}
