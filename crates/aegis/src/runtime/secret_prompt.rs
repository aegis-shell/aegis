//! User-consent secret prompting (the secret vault's password unlock).
//!
//! Modeled on the app pick (`app_pick.rs`): a `PromptSecret` IPC request
//! arrives on a connection thread, is forwarded here, and parks its reply
//! channel while the compositor opens the masked secret-prompt chrome
//! immediately — ordinary modal chrome over the live scene that captures no
//! screen content. One interactive pick at a time compositor-wide, shared
//! with the other picks: all are single modal overlays.

use super::*;

/// One control message from an IPC connection thread, applied on the main
/// loop. Mirrors [`AppPickControlRequest`].
pub(super) struct SecretPromptControlRequest {
    pub(super) conn_id: u64,
    pub(super) action: SecretPromptControl,
}

pub(super) enum SecretPromptControl {
    /// Open the prompt; the reply completes when the user confirms or
    /// cancels.
    Start {
        title: String,
        reason: Option<String>,
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::SecretPromptResult, String>>,
    },
    /// The IPC handler stopped waiting (interaction timeout): close the
    /// prompt chrome if it is still open for this connection.
    Cancel,
}

/// A secret prompt waiting for user interaction, owned by the main loop.
pub(super) struct PendingSecretPrompt {
    pub(super) conn_id: u64,
    pub(super) reply: std::sync::mpsc::Sender<Result<aegis_ipc::SecretPromptResult, String>>,
}

impl CompositorRuntime {
    /// Apply secret-prompt controls from IPC connection threads. Like the
    /// other picks there is no freeze to arm: the prompt chrome opens over
    /// the live scene as soon as the request is accepted.
    pub(super) fn drain_secret_prompt_controls(&mut self) {
        while let Ok(request) = self.secret_prompt_rx.try_recv() {
            match request.action {
                SecretPromptControl::Start {
                    title,
                    reason,
                    reply,
                } => {
                    if self.pending_secret_prompt.is_some()
                        || self.pending_app_pick.is_some()
                        || self.pending_file_pick.is_some()
                        || self.pending_pick.is_some()
                    {
                        let _ =
                            reply.send(Err("another interactive pick is in progress".to_owned()));
                    } else if self.server.session_locked() || !self.host.is_active() {
                        let _ = reply.send(Err("session is locked or inactive".to_owned()));
                    } else {
                        self.pending_secret_prompt = Some(PendingSecretPrompt {
                            conn_id: request.conn_id,
                            reply,
                        });
                        self.shell
                            .start_secret_prompt(aegis_shell::SecretPromptParams { title, reason });
                    }
                }
                SecretPromptControl::Cancel => {
                    if self
                        .pending_secret_prompt
                        .as_ref()
                        .is_some_and(|pick| pick.conn_id == request.conn_id)
                    {
                        self.abandon_pending_secret_prompt("secret prompt timed out");
                    }
                }
            }
        }
    }

    /// Answer the pending secret prompt with an error and close its chrome,
    /// if any. Used by the timeout path, and by the session-lock path before
    /// the lock screen covers the prompt.
    pub(super) fn abandon_pending_secret_prompt(&mut self, reason: &str) {
        if let Some(pick) = self.pending_secret_prompt.take() {
            let _ = pick.reply.send(Err(reason.to_owned()));
        }
        self.shell.cancel_secret_prompt();
    }
}
