//! User-consent yes/no confirmation (portal consent dialogs: Account,
//! Access, DynamicLauncher).
//!
//! Modeled on the other picks (`app_pick.rs`): a `PickConfirm` IPC request
//! arrives on a connection thread, is forwarded here, and parks its reply
//! channel while the compositor opens the confirmation chrome immediately —
//! ordinary modal chrome over the live scene that captures no screen
//! content. One interactive pick at a time compositor-wide, shared with the
//! other picks: all are single modal overlays.

use super::*;

/// One control message from an IPC connection thread, applied on the main
/// loop. Mirrors [`AppPickControlRequest`].
pub(super) struct ConfirmPickControlRequest {
    pub(super) conn_id: u64,
    pub(super) action: ConfirmPickControl,
}

pub(super) enum ConfirmPickControl {
    /// Open the dialog; the reply completes when the user confirms or
    /// cancels.
    Start {
        title: String,
        body: String,
        accept_label: Option<String>,
        style: aegis_shell::ConfirmPickStyle,
        reply: std::sync::mpsc::Sender<Result<aegis_shell::ConfirmAnswer, String>>,
    },
    /// The IPC handler stopped waiting (interaction timeout): close the
    /// dialog if it is still open for this connection.
    Cancel,
}

/// A confirmation waiting for user interaction, owned by the main loop.
pub(super) struct PendingConfirmPick {
    pub(super) conn_id: u64,
    pub(super) reply: std::sync::mpsc::Sender<Result<aegis_shell::ConfirmAnswer, String>>,
}

impl CompositorRuntime {
    /// Apply confirmation controls from IPC connection threads. Like the
    /// other picks there is no freeze to arm: the dialog opens over the
    /// live scene as soon as the request is accepted.
    pub(super) fn drain_confirm_pick_controls(&mut self) {
        while let Ok(request) = self.confirm_pick_rx.try_recv() {
            match request.action {
                ConfirmPickControl::Start {
                    title,
                    body,
                    accept_label,
                    style,
                    reply,
                } => {
                    if self.pending_confirm_pick.is_some()
                        || self.pending_secret_prompt.is_some()
                        || self.pending_app_pick.is_some()
                        || self.pending_pick.is_some()
                    {
                        let _ =
                            reply.send(Err("another interactive pick is in progress".to_owned()));
                    } else if self.server.session_locked() || !self.host.is_active() {
                        let _ = reply.send(Err("session is locked or inactive".to_owned()));
                    } else {
                        self.pending_confirm_pick = Some(PendingConfirmPick {
                            conn_id: request.conn_id,
                            reply,
                        });
                        self.shell
                            .start_confirm_pick(aegis_shell::ConfirmPickParams {
                                title,
                                body,
                                accept_label,
                                style,
                            });
                    }
                }
                ConfirmPickControl::Cancel => {
                    if self
                        .pending_confirm_pick
                        .as_ref()
                        .is_some_and(|pick| pick.conn_id == request.conn_id)
                    {
                        self.abandon_pending_confirm_pick("confirmation timed out");
                    }
                }
            }
        }
    }

    /// Answer the pending confirmation with an error and close its chrome,
    /// if any. Used by the timeout path, and by the session-lock path
    /// before the lock screen covers the dialog.
    pub(super) fn abandon_pending_confirm_pick(&mut self, reason: &str) {
        if let Some(pick) = self.pending_confirm_pick.take() {
            let _ = pick.reply.send(Err(reason.to_owned()));
        }
        self.shell.cancel_confirm_pick();
    }

    /// Park a destructive session action behind the system-level
    /// confirmation dialog. There is no IPC requester here — the
    /// compositor's own chrome asked for the transition — so unlike the
    /// portal picks nothing parks a reply channel; the parked action in
    /// `pending_system_action` is the whole handshake, and the answer
    /// delivery in the presentation loop applies or drops it.
    fn request_system_action_confirmation(&mut self, action: aegis_model::system::SystemAction) {
        use aegis_model::system::SystemAction;
        if self.server.session_locked() || !self.host.is_active() {
            // Locked/inactive sessions never open consent chrome; the
            // request simply dies here.
            return;
        }
        let (title, body) = match action {
            SystemAction::Reboot => ("Restart?", "The session ends and the computer restarts."),
            SystemAction::Suspend => ("Suspend?", "The computer sleeps until you wake it."),
            _ => (
                "Power off?",
                "The session ends and the computer powers off.",
            ),
        };
        self.pending_system_action = Some(action);
        self.shell
            .start_confirm_pick(aegis_shell::ConfirmPickParams {
                title: title.to_owned(),
                body: body.to_owned(),
                accept_label: None,
                style: aegis_shell::ConfirmPickStyle::YesNo,
            });
    }

    /// Destructive session actions queued by chrome during the last frame,
    /// each opening the system-level confirmation dialog. Only one dialog
    /// exists at a time; later requests in the same batch drop while one
    /// is pending, matching the picks' single-modal rule. The consent
    /// dialog is exclusive chrome, so an open command panel closes first —
    /// exactly like the battery alert taking over the band.
    pub(super) fn drain_system_confirm_requests(&mut self) {
        let requests = std::mem::take(&mut self.system_confirm_requests);
        if requests.is_empty() {
            return;
        }
        if self.shell.command_panel_active() {
            self.shell.toggle_command_panel();
        }
        for action in requests {
            if self.pending_system_action.is_none() {
                self.request_system_action_confirmation(action);
            }
        }
    }
}
