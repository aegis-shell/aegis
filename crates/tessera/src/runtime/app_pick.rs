//! User-consent application picking (the AppChooser portal's compositor
//! side).
//!
//! A `PickApp` IPC request
//! arrives on a connection thread, is forwarded here, and parks its reply
//! channel while the compositor opens the app-picker chrome immediately —
//! ordinary modal chrome over the live scene that captures no screen
//! content. One interactive compositor-owned pick at a time is shared with
//! target selection; both are single modal overlays.

use super::*;

/// One control message from an IPC connection thread, applied on the main
/// loop.
pub(super) struct AppPickControlRequest {
    pub(super) conn_id: u64,
    pub(super) action: AppPickControl,
}

pub(super) enum AppPickControl {
    /// Open the picker with the request's choices; the reply completes when
    /// the user confirms or cancels.
    Start {
        choices: Vec<String>,
        subject: Option<String>,
        last_choice: Option<String>,
        reply: std::sync::mpsc::Sender<Result<tessera_ipc::AppPickResult, String>>,
    },
    /// The IPC handler stopped waiting (interaction timeout): close the
    /// picker chrome if it is still open for this connection.
    Cancel,
}

/// An app pick waiting for user interaction, owned by the main loop.
pub(super) struct PendingAppPick {
    pub(super) conn_id: u64,
    pub(super) reply: std::sync::mpsc::Sender<Result<tessera_ipc::AppPickResult, String>>,
}

impl CompositorRuntime {
    /// Apply app-pick controls from IPC connection threads. There is no
    /// screenshot freeze to arm: the picker chrome opens over the live scene
    /// as soon as the request is accepted.
    pub(super) fn drain_app_pick_controls(&mut self) {
        while let Ok(request) = self.app_pick_rx.try_recv() {
            match request.action {
                AppPickControl::Start {
                    choices,
                    subject,
                    last_choice,
                    reply,
                } => {
                    if self.pending_app_pick.is_some() || self.pending_pick.is_some() {
                        let _ =
                            reply.send(Err("another interactive pick is in progress".to_owned()));
                    } else if self.server.session_locked() || !self.host.is_active() {
                        let _ = reply.send(Err("session is locked or inactive".to_owned()));
                    } else {
                        self.pending_app_pick = Some(PendingAppPick {
                            conn_id: request.conn_id,
                            reply,
                        });
                        self.shell.start_app_pick(tessera_shell::AppPickParams {
                            choices,
                            subject,
                            last_choice,
                        });
                    }
                }
                AppPickControl::Cancel => {
                    if self
                        .pending_app_pick
                        .as_ref()
                        .is_some_and(|pick| pick.conn_id == request.conn_id)
                    {
                        self.abandon_pending_app_pick("app pick timed out");
                    }
                }
            }
        }
    }

    /// Answer the pending app pick with an error and close its chrome, if
    /// any. Used by the timeout path, and by the session-lock path before
    /// the lock screen covers the picker.
    pub(super) fn abandon_pending_app_pick(&mut self, reason: &str) {
        if let Some(pick) = self.pending_app_pick.take() {
            let _ = pick.reply.send(Err(reason.to_owned()));
        }
        self.shell.cancel_app_pick();
    }
}

#[cfg(test)]
mod tests {
    // Behavior is covered by the shell-side component tests and IPC gates.
}
