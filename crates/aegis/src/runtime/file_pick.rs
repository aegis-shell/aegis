//! User-consent file picking (the FileChooser portal's compositor side).
//!
//! Modeled on the interactive target pick (ADR-0054) without the
//! screenshot freeze: a `PickFile` IPC request arrives on a connection
//! thread, is forwarded here like the pick/stream/idle controls, and parks
//! its reply channel while the compositor opens the file-picker chrome
//! immediately — ordinary modal chrome over the live scene that captures
//! no screen content. The user's confirm/cancel drains back through the
//! shell events and answers the parked request. One interactive pick at a
//! time compositor-wide, shared with the target pick: both are single
//! modal overlays.

use super::*;

/// One control message from an IPC connection thread, applied on the main
/// loop. Mirrors [`PickControlRequest`].
pub(super) struct FilePickControlRequest {
    pub(super) conn_id: u64,
    pub(super) action: FilePickControl,
}

pub(super) enum FilePickControl {
    /// Open the picker with `options`; the reply completes when the user
    /// confirms or cancels.
    Start {
        options: aegis_ipc::FilePickOptions,
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::FilePickResult, String>>,
    },
    /// The IPC handler stopped waiting (interaction timeout): close the
    /// picker chrome if it is still open for this connection.
    Cancel,
}

/// A file pick waiting for user interaction, owned by the main loop.
pub(super) struct PendingFilePick {
    pub(super) conn_id: u64,
    pub(super) reply: std::sync::mpsc::Sender<Result<aegis_ipc::FilePickResult, String>>,
}

/// Map IPC file-pick options onto the picker chrome's parameters. Without
/// a requested folder the picker opens in the user's home directory.
pub(super) fn file_pick_params(
    options: &aegis_ipc::FilePickOptions,
) -> aegis_shell::FilePickParams {
    let mode = match options.mode {
        aegis_ipc::FilePickMode::Open => aegis_core::file_picker::FilePickMode::Open,
        aegis_ipc::FilePickMode::Save => aegis_core::file_picker::FilePickMode::Save,
        aegis_ipc::FilePickMode::ChooseDir => aegis_core::file_picker::FilePickMode::ChooseDir,
    };
    aegis_shell::FilePickParams {
        mode,
        multiple: options.multiple,
        directory: options.directory,
        title: options.title.clone(),
        accept_label: options.accept_label.clone(),
        start_dir: options
            .current_folder
            .clone()
            .or_else(|| std::env::var_os("HOME").map(std::path::PathBuf::from))
            .unwrap_or_else(|| std::path::PathBuf::from("/")),
        suggested_name: options.current_name.clone(),
        filters: options
            .filters
            .iter()
            .map(|filter| (filter.label.clone(), filter.patterns.clone()))
            .collect(),
    }
}

impl CompositorRuntime {
    /// Apply file-pick controls from IPC connection threads. Unlike the
    /// target pick there is no freeze to arm: the picker chrome opens over
    /// the live scene as soon as the request is accepted.
    pub(super) fn drain_file_pick_controls(&mut self) {
        while let Ok(request) = self.file_pick_rx.try_recv() {
            match request.action {
                FilePickControl::Start { options, reply } => {
                    if self.pending_file_pick.is_some()
                        || self.pending_app_pick.is_some()
                        || self.pending_pick.is_some()
                    {
                        let _ =
                            reply.send(Err("another interactive pick is in progress".to_owned()));
                    } else if self.server.session_locked() || !self.host.is_active() {
                        let _ = reply.send(Err("session is locked or inactive".to_owned()));
                    } else {
                        self.pending_file_pick = Some(PendingFilePick {
                            conn_id: request.conn_id,
                            reply,
                        });
                        // No screenshot freeze: the picker is ordinary modal
                        // chrome over the live scene.
                        self.shell.start_file_pick(file_pick_params(&options));
                    }
                }
                FilePickControl::Cancel => {
                    if self
                        .pending_file_pick
                        .as_ref()
                        .is_some_and(|pick| pick.conn_id == request.conn_id)
                    {
                        self.abandon_pending_file_pick("file pick timed out");
                    }
                }
            }
        }
    }

    /// Answer the pending file pick with an error and close its chrome, if
    /// any. Used by the timeout path, and by the session-lock path before
    /// the lock screen covers the picker.
    pub(super) fn abandon_pending_file_pick(&mut self, reason: &str) {
        if let Some(pick) = self.pending_file_pick.take() {
            let _ = pick.reply.send(Err(reason.to_owned()));
        }
        self.shell.cancel_file_pick();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_options_map_to_picker_params() {
        let options = aegis_ipc::FilePickOptions {
            mode: aegis_ipc::FilePickMode::Save,
            multiple: true,
            directory: true,
            title: Some("Export".into()),
            accept_label: Some("Export".into()),
            current_folder: Some(std::path::PathBuf::from("/srv/data")),
            current_name: Some("report.pdf".into()),
            filters: vec![aegis_ipc::FileFilter {
                label: "Images".into(),
                patterns: vec!["*.png".into(), "image/jpeg".into()],
            }],
        };
        let params = file_pick_params(&options);
        assert_eq!(params.mode, aegis_core::file_picker::FilePickMode::Save);
        assert!(params.multiple);
        assert!(params.directory);
        assert_eq!(params.title.as_deref(), Some("Export"));
        assert_eq!(params.accept_label.as_deref(), Some("Export"));
        assert_eq!(params.start_dir, std::path::PathBuf::from("/srv/data"));
        assert_eq!(params.suggested_name.as_deref(), Some("report.pdf"));
        assert_eq!(
            params.filters,
            vec![(
                "Images".to_owned(),
                vec!["*.png".to_owned(), "image/jpeg".to_owned()]
            )]
        );
    }

    #[test]
    fn default_options_fall_back_to_a_start_dir() {
        let params = file_pick_params(&aegis_ipc::FilePickOptions::default());
        assert_eq!(params.mode, aegis_core::file_picker::FilePickMode::Open);
        assert!(params.start_dir.is_absolute());
        assert!(params.filters.is_empty());
        assert!(params.title.is_none());
    }
}
