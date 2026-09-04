//! Agent capability-borrowing consent (ADR-0088): the pairing checklist
//! where the user approves a subset of the requested capability groups
//! instead of an all-or-nothing Allow/Deny.
//!
//! Modeled on the other picks (`confirm_pick.rs`): a `PairAgent` IPC
//! request arrives on a connection thread, is forwarded here, and parks its
//! reply channel while the compositor opens the checklist chrome
//! immediately — ordinary modal chrome over the live scene that captures no
//! screen content. One interactive pick at a time compositor-wide, shared
//! with the other picks: all are single modal overlays.

use super::*;

/// One control message from an IPC connection thread, applied on the main
/// loop. Mirrors [`ConfirmPickControlRequest`].
pub(super) struct CapabilityPickControlRequest {
    pub(super) conn_id: u64,
    pub(super) action: CapabilityPickControl,
}

pub(super) enum CapabilityPickControl {
    /// Open the checklist; the reply completes when the user allows the
    /// checked groups or denies.
    Start {
        params: tessera_shell::CapabilityPickParams,
        reply: std::sync::mpsc::Sender<Result<tessera_shell::CapabilityPickResult, String>>,
    },
    /// The IPC handler stopped waiting (interaction timeout): close the
    /// checklist if it is still open for this connection.
    Cancel,
}

/// A capability pick waiting for user interaction, owned by the main loop.
pub(super) struct PendingCapabilityPick {
    pub(super) conn_id: u64,
    pub(super) reply: std::sync::mpsc::Sender<Result<tessera_shell::CapabilityPickResult, String>>,
}

impl CompositorRuntime {
    /// Apply capability-pick controls from IPC connection threads. Like the
    /// other picks there is no freeze to arm: the checklist opens over the
    /// live scene as soon as the request is accepted.
    pub(super) fn drain_capability_pick_controls(&mut self) {
        while let Ok(request) = self.capability_pick_rx.try_recv() {
            match request.action {
                CapabilityPickControl::Start { params, reply } => {
                    if self.pending_capability_pick.is_some()
                        || self.pending_confirm_pick.is_some()
                        || self.pending_secret_prompt.is_some()
                        || self.pending_app_pick.is_some()
                        || self.pending_pick.is_some()
                    {
                        let _ =
                            reply.send(Err("another interactive pick is in progress".to_owned()));
                    } else if self.server.session_locked() || !self.host.is_active() {
                        let _ = reply.send(Err("session is locked or inactive".to_owned()));
                    } else {
                        self.pending_capability_pick = Some(PendingCapabilityPick {
                            conn_id: request.conn_id,
                            reply,
                        });
                        self.shell.start_capability_pick(params);
                    }
                }
                CapabilityPickControl::Cancel => {
                    if self
                        .pending_capability_pick
                        .as_ref()
                        .is_some_and(|pick| pick.conn_id == request.conn_id)
                    {
                        self.abandon_pending_capability_pick("capability pick timed out");
                    }
                }
            }
        }
    }

    /// Answer the pending capability pick with an error and close its
    /// chrome, if any. Used by the timeout path, and by the session-lock
    /// path before the lock screen covers the checklist.
    pub(super) fn abandon_pending_capability_pick(&mut self, reason: &str) {
        if let Some(pick) = self.pending_capability_pick.take() {
            let _ = pick.reply.send(Err(reason.to_owned()));
        }
        self.shell.cancel_capability_pick();
    }
}
