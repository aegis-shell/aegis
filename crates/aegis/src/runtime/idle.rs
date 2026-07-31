//! Connection-scoped global idle inhibition over IPC.
//!
//! An IPC client without a Wayland surface can hold a connection-scoped
//! global inhibitor through `Request::SetIdleInhibit`. The registry
//! lives on the compositor main loop, keyed by IPC connection id; the
//! effective flag fans into the Wayland server's
//! `Server::set_ipc_idle_inhibit`, and a connection disconnect releases
//! its entry fail-closed — the same shape as the output-stream registry.

use super::*;

/// One control message from an IPC connection thread, applied on the main
/// loop. Mirrors the stream-control request pattern.
pub(super) struct IdleControlRequest {
    pub(super) conn_id: u64,
    pub(super) action: IdleControl,
}

pub(super) enum IdleControl {
    Set {
        inhibit: bool,
        reply: std::sync::mpsc::Sender<Result<bool, String>>,
    },
    /// The connection disconnected while holding an inhibitor.
    Disconnect,
}

/// Reserved registry id for the session-owned "always on" toggle
/// (`SystemAction::SetIdleInhibit`, the command panel's Always On row).
/// Real IPC connection ids count up from 1, so `u64::MAX` can never
/// collide; holding the toggle in the same registry keeps the two
/// inhibitor sources folded into one effective flag — a disconnecting
/// IPC client can never clear the panel's toggle, nor vice versa.
pub(super) const SESSION_IDLE_INHIBIT_ID: u64 = u64::MAX;

/// IPC connections currently holding a global idle inhibitor.
#[derive(Default)]
pub(super) struct IdleInhibits {
    connections: std::collections::BTreeSet<u64>,
}

impl IdleInhibits {
    /// Record one connection's inhibitor state, returning the effective
    /// flag (any connection inhibiting).
    pub(super) fn set(&mut self, conn_id: u64, inhibit: bool) -> bool {
        if inhibit {
            self.connections.insert(conn_id);
        } else {
            self.connections.remove(&conn_id);
        }
        !self.connections.is_empty()
    }

    /// Drop a dead connection's entry, returning the new effective flag
    /// only when the connection actually held an inhibitor.
    pub(super) fn disconnect(&mut self, conn_id: u64) -> Option<bool> {
        if self.connections.remove(&conn_id) {
            Some(!self.connections.is_empty())
        } else {
            None
        }
    }
}

impl CompositorRuntime {
    /// Drain queued idle-inhibit controls and apply them to the Wayland
    /// server. Drained next to the stream controls each iteration.
    pub(super) fn drain_idle_controls(&mut self) {
        while let Ok(request) = self.idle_control_rx.try_recv() {
            match request.action {
                IdleControl::Set { inhibit, reply } => {
                    let effective = self.ipc_idle_inhibits.set(request.conn_id, inhibit);
                    self.server.set_ipc_idle_inhibit(effective);
                    let _ = reply.send(Ok(inhibit));
                }
                IdleControl::Disconnect => {
                    if let Some(effective) = self.ipc_idle_inhibits.disconnect(request.conn_id) {
                        log::info!(
                            "idle: IPC connection {} gone; releasing its inhibitor",
                            request.conn_id
                        );
                        self.server.set_ipc_idle_inhibit(effective);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn per_connection_counts_fold_into_one_flag() {
        let mut inhibits = IdleInhibits::default();
        assert!(inhibits.set(1, true));
        assert!(inhibits.set(2, true));
        // One release leaves the other connection's inhibitor effective.
        assert!(inhibits.set(1, false));
        assert!(!inhibits.set(2, false));
        // Idempotent clears keep the flag stable.
        assert!(!inhibits.set(2, false));
    }

    #[test]
    fn disconnect_releases_only_held_inhibitors() {
        let mut inhibits = IdleInhibits::default();
        // A connection that never inhibited reports no change.
        assert_eq!(inhibits.disconnect(9), None);
        inhibits.set(1, true);
        inhibits.set(2, true);
        assert_eq!(inhibits.disconnect(1), Some(true));
        assert_eq!(inhibits.disconnect(2), Some(false));
        assert_eq!(inhibits.disconnect(2), None);
    }
}
