//! User-consent interactive picking (ADR-0054).
//!
//! A `PickTarget` IPC request arrives on a connection thread, is forwarded
//! here like the stream/idle controls, and parks its reply channel while the
//! compositor freezes the screen and opens the matching selector chrome
//! (reusing the Print-key component). The user's confirm/cancel drains back
//! through the shell events and answers the parked request. One pick at a
//! time compositor-wide: the chrome is a single modal overlay.

use super::*;

/// Bounds the IPC handler's wait for a user pick. On expiry the handler
/// sends [`PickControl::Cancel`] so the picker chrome never lingers.
pub(super) const PICK_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(300);

/// One control message from an IPC connection thread, applied on the main
/// loop. Mirrors the stream/idle-control request pattern.
pub(super) struct PickControlRequest {
    pub(super) conn_id: u64,
    pub(super) action: PickControl,
}

pub(super) enum PickControl {
    /// Open the picker for `kind`; the reply completes when the user
    /// confirms or cancels.
    Start {
        kind: aegis_ipc::PickKind,
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::PickResult, String>>,
    },
    /// The IPC handler stopped waiting (interaction timeout): close the
    /// picker chrome if it is still open for this connection.
    Cancel,
}

/// A pick waiting for user interaction, owned by the main loop.
pub(super) struct PendingPick {
    pub(super) conn_id: u64,
    pub(super) kind: aegis_ipc::PickKind,
    pub(super) reply: std::sync::mpsc::Sender<Result<aegis_ipc::PickResult, String>>,
}

/// Map an IPC pick kind onto the selector's interaction mode.
pub(super) fn picker_mode(kind: aegis_ipc::PickKind) -> aegis_shell::PickerMode {
    match kind {
        aegis_ipc::PickKind::Region => aegis_shell::PickerMode::Region,
        aegis_ipc::PickKind::Pixel => aegis_shell::PickerMode::Pixel,
        aegis_ipc::PickKind::Window => aegis_shell::PickerMode::Window,
        aegis_ipc::PickKind::Output => aegis_shell::PickerMode::Output,
    }
}

impl CompositorRuntime {
    /// Apply pick controls from IPC connection threads. A new pick freezes
    /// the screen; the picker itself opens once the freeze holds (the same
    /// trigger-frame pipeline as the Print key, routed in presentation).
    pub(super) fn drain_pick_controls(&mut self) {
        while let Ok(request) = self.pick_rx.try_recv() {
            match request.action {
                PickControl::Start { kind, reply } => {
                    if self.pending_pick.is_some() || self.pending_app_pick.is_some() {
                        let _ =
                            reply.send(Err("another interactive pick is in progress".to_owned()));
                    } else if self.server.session_locked() || !self.host.is_active() {
                        let _ = reply.send(Err("session is locked or inactive".to_owned()));
                    } else if self.shell.screenshot_active() {
                        let _ = reply.send(Err("the selector is already open".to_owned()));
                    } else {
                        self.pending_pick = Some(PendingPick {
                            conn_id: request.conn_id,
                            kind,
                            reply,
                        });
                        self.pending_pick_open = Some(kind);
                        // Portal pickers intentionally retain their cursor-free
                        // capture contract.
                        self.screenshot_freeze.request_open(None);
                    }
                }
                PickControl::Cancel => {
                    if self
                        .pending_pick
                        .as_ref()
                        .is_some_and(|pick| pick.conn_id == request.conn_id)
                    {
                        self.abandon_pending_pick("interactive pick timed out");
                    }
                }
            }
        }
    }

    /// Answer the pending pick with an error and close its chrome, if any.
    /// Used by the timeout path, and by the session-lock path before the
    /// lock screen covers the picker.
    pub(super) fn abandon_pending_pick(&mut self, reason: &str) {
        if let Some(pick) = self.pending_pick.take() {
            let _ = pick.reply.send(Err(reason.to_owned()));
        }
        self.pending_pick_open = None;
        self.screenshot_freeze.disarm();
        self.shell.set_screenshot_freeze(false);
        self.shell.cancel_pick();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pick_kinds_map_to_picker_modes() {
        assert_eq!(
            picker_mode(aegis_ipc::PickKind::Region),
            aegis_shell::PickerMode::Region
        );
        assert_eq!(
            picker_mode(aegis_ipc::PickKind::Pixel),
            aegis_shell::PickerMode::Pixel
        );
        assert_eq!(
            picker_mode(aegis_ipc::PickKind::Window),
            aegis_shell::PickerMode::Window
        );
        assert_eq!(
            picker_mode(aegis_ipc::PickKind::Output),
            aegis_shell::PickerMode::Output
        );
    }
}
