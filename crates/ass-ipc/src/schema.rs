//! The schema for the ass IPC.
//!
//! One major version ([`PROTOCOL_VERSION`]); a client offering any other
//! major version is refused at the handshake. Messages are internally
//! tagged (`{"type": "..."}`) so the wire is self-describing and new
//! variants add without renaming existing fields. See
//! [ADR-0027](../../docs/adr/0027-ipc-and-introspection.md).

use ass_core::notify::Notification;
use ass_core::output::OutputInfo;
use ass_core::window::{Window, WindowId};
use ass_core::workspace::{OutputId, Switch, WorkspaceId, WorkspaceSnapshot};

use crate::journal::{JournalEntry, JournalSnapshot};

/// The protocol major version this build speaks. A client must offer the
/// same major version at the [`Request::Hello`] handshake. Bumped to 2 by
/// ADR-0032 (durable `WindowId` replacing the recycled surface-address id);
/// the wire encoding is unchanged, the non-reuse guarantee is new.
pub const PROTOCOL_VERSION: u32 = 2;

/// The capability classes a client may hold (ADR-0027).
///
/// `query` is always granted (read state + subscribe); `control` and
/// `session` require the server's policy to allow them. Serialized as an
/// object so tool authors read it without decoding a bitmask.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Capabilities {
    /// Read state and subscribe to events. Always granted.
    pub query: bool,
    /// Mutate windows, workspaces, and input focus.
    pub control: bool,
    /// Session-level actions: quit, reload config, change outputs.
    pub session: bool,
}

impl Capabilities {
    /// Query only.
    pub const QUERY: Self = Self {
        query: true,
        control: false,
        session: false,
    };

    /// Intersection of two capability sets. Used to fold the client's request
    /// against the server's policy.
    pub fn intersect(self, other: Self) -> Self {
        Self {
            query: self.query && other.query,
            control: self.control && other.control,
            session: self.session && other.session,
        }
    }

    /// Force `query` on, per the ADR's "always allowed" rule.
    pub fn with_query_always(self) -> Self {
        Self {
            query: true,
            ..self
        }
    }
}

/// One operation family, used by [`Scope`] to enumerate which commands a
/// scoped client may issue (ADR-0034). One variant per `control`-class
/// `Command`; session commands (Quit) are governed by caps alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum OpClass {
    Focus,
    Close,
    Move,
    Cycle,
    SwitchWorkspace,
    SwitchWorkspaceTo,
    MoveToWorkspace,
    ToggleTiling,
    Notify,
    DismissNotification,
}

/// A resource-and-operation allowlist layered on top of capabilities
/// (ADR-0034). `None` at any field means "unrestricted at this axis". The
/// default (all fields `None`) is the unscoped back-compat behavior: all
/// resources, all operations within the granted caps.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Scope {
    /// Allowed window ids. `None` = all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub windows: Option<Vec<WindowId>>,
    /// Allowed workspace ids. `None` = all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspaces: Option<Vec<WorkspaceId>>,
    /// Allowed output ids. `None` = all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outputs: Option<Vec<OutputId>>,
    /// Allowed operation families. `None` = all.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops: Option<Vec<OpClass>>,
}

impl Scope {
    /// The unscoped default: all resources, all operations.
    pub fn unscoped() -> Self {
        Scope::default()
    }

    /// Whether `val` is allowed by the `None`-means-all allowlist.
    fn allows<T: PartialEq + Copy>(opt: &Option<Vec<T>>, val: T) -> bool {
        opt.as_ref().map_or(true, |v| v.contains(&val))
    }

    /// Whether this scope permits the given command (ADR-0034). Session
    /// commands bypass scope; control commands check ops + resources.
    pub fn permits(&self, cmd: &Command) -> bool {
        let need = cmd.required_cap();
        if need.session {
            return true;
        }
        if let Some(ops) = &self.ops {
            if !ops.contains(&cmd.op_class()) {
                return false;
            }
        }
        match cmd {
            Command::Focus { id } | Command::Close { id } | Command::Move { id } => {
                Self::allows(&self.windows, *id)
            }
            Command::MoveToWorkspace { window, workspace } => {
                Self::allows(&self.windows, *window) && Self::allows(&self.workspaces, *workspace)
            }
            Command::SwitchWorkspaceTo { id } => Self::allows(&self.workspaces, *id),
            _ => true,
        }
    }
}

/// A mutation the compositor applies on its main loop. Mirrors the operations
/// the chrome and the key bindings already perform. Serialized as a tagged
/// table so new commands add without renaming existing ones.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum Command {
    /// Focus (activate) a toplevel by id. `control`.
    Focus { id: WindowId },
    /// Close a toplevel by id. `control`.
    Close { id: WindowId },
    /// Begin an interactive move of a toplevel by id. `control`.
    Move { id: WindowId },
    /// Cycle keyboard focus. `forward = true` for next, `false` for previous. `control`.
    Cycle { forward: bool },
    /// Switch to an adjacent workspace on the focused output. `control`.
    SwitchWorkspace { dir: Switch },
    /// Switch directly to a workspace by id (ADR-0025). `control`.
    SwitchWorkspaceTo { id: WorkspaceId },
    /// Move a toplevel to a workspace (ADR-0025). `control`.
    MoveToWorkspace {
        window: WindowId,
        workspace: WorkspaceId,
    },
    /// Toggle the current workspace between tiled and floating (ADR-0024). `control`.
    ToggleTiling,
    /// Post a notification (M9, delivered over the IPC). `control`.
    Notify {
        summary: String,
        body: String,
        app_id: Option<String>,
    },
    /// Dismiss a notification by id before its TTL elapses. `control`.
    DismissNotification { id: u64 },
    /// Quit the compositor. `session`.
    Quit,
}

impl Command {
    /// The capability a client must hold to issue this command.
    pub fn required_cap(&self) -> Capabilities {
        match self {
            Command::Quit => Capabilities {
                query: false,
                control: false,
                session: true,
            },
            _ => Capabilities {
                query: false,
                control: true,
                session: false,
            },
        }
    }

    /// The [`OpClass`] this command belongs to, for scope checks (ADR-0034).
    /// Session commands (Quit) have no OpClass; scope does not apply to them.
    pub fn op_class(&self) -> OpClass {
        match self {
            Command::Focus { .. } => OpClass::Focus,
            Command::Close { .. } => OpClass::Close,
            Command::Move { .. } => OpClass::Move,
            Command::Cycle { .. } => OpClass::Cycle,
            Command::SwitchWorkspace { .. } => OpClass::SwitchWorkspace,
            Command::SwitchWorkspaceTo { .. } => OpClass::SwitchWorkspaceTo,
            Command::MoveToWorkspace { .. } => OpClass::MoveToWorkspace,
            Command::ToggleTiling => OpClass::ToggleTiling,
            Command::Notify { .. } => OpClass::Notify,
            Command::DismissNotification { .. } => OpClass::DismissNotification,
            Command::Quit => OpClass::ToggleTiling, // unreachable: scope skips session cmds
        }
    }
}

/// A server-pushed event, delivered to connections that sent [`Request::Subscribe`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum Event {
    /// The set of visible toplevels changed (a window mapped, unmapped,
    /// closed, focused, or retitled, or the current workspace switched).
    /// The client re-queries with [`Request::GetWindows`] for the new snapshot.
    WindowsChanged,
    /// The workspace model changed (switch, a toplevel placed on or removed
    /// from a workspace, a workspace created or reaped). Re-query with
    /// [`Request::GetWorkspaces`].
    WorkspaceChanged,
    /// A notification was posted (via [`Request::Do`] / `Notify`). Carries
    /// the notification itself; the queue is also queryable with
    /// [`Request::GetNotifications`].
    Notified { notification: Notification },
    /// A mutation was applied and recorded in the journal (ADR-0033). Pushed
    /// only to connections that sent [`Request::SubscribeJournal`].
    Journal { entry: JournalEntry },
}

/// A client → server message.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum Request {
    /// Handshake opener. Sent exactly once, before any other request.
    Hello {
        /// The major protocol version the client speaks.
        version: u32,
        /// The capabilities the client wants.
        caps: Capabilities,
        /// Optional scope name (ADR-0034). Resolved by the server from its
        /// configuration; `None` means unscoped (back-compat).
        #[serde(default)]
        scope: Option<String>,
    },
    /// Fetch the live toplevel snapshot, in z-order. Requires `query`.
    GetWindows,
    /// Fetch the live workspace/output snapshot. Requires `query`.
    GetWorkspaces,
    /// Fetch the live notification queue. Requires `query`.
    GetNotifications,
    /// Fetch the live output list (connector + geometry). Requires `query`.
    GetOutputs,
    /// Fetch journal entries with `seq > since` (ADR-0033). Requires `query`.
    /// The response carries the ring's `oldest_seq` and `latest_seq` so the
    /// client detects gaps from evicted entries.
    GetJournal { since: u64 },
    /// Submit a [`Command`]. Fire-and-forget: the server acknowledges queuing
    /// with [`Response::Ok`], not completion. Requires the command's capability.
    Do { cmd: Command },
    /// Opt into server-pushed [`Event`]s on this connection. Idempotent.
    Subscribe,
    /// Opt into server-pushed [`Event::Journal`] entries (ADR-0033). Separate
    /// from [`Subscribe`](Self::Subscribe) so status bars that only need the
    /// coarse re-query signal are not flooded with per-command entries.
    SubscribeJournal,
}

/// A server → client message.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum Response {
    /// Handshake reply. Carries the negotiated version and the capabilities
    /// the server actually granted.
    Hello {
        version: u32,
        caps: Capabilities,
        /// The scope the server actually granted (ADR-0034). Unscoped if the
        /// client did not request a scope name.
        #[serde(default = "Scope::unscoped")]
        scope: Scope,
    },
    /// Reply to [`Request::GetWindows`].
    Windows { windows: Vec<Window> },
    /// Reply to [`Request::GetWorkspaces`].
    Workspaces { snapshot: WorkspaceSnapshot },
    /// Reply to [`Request::GetNotifications`].
    Notifications { notifications: Vec<Notification> },
    /// Reply to [`Request::GetOutputs`].
    Outputs { outputs: Vec<OutputInfo> },
    /// Reply to [`Request::GetJournal`] (ADR-0033).
    Journal { snapshot: JournalSnapshot },
    /// Acknowledgment of a queued [`Request::Do`].
    Ok,
    /// Reply to [`Request::Subscribe`]: events will now be pushed.
    Subscribed,
    /// An error servicing a request. The connection stays open unless the
    /// error is a protocol violation (wrong version, missing handshake).
    Error { message: String },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_getwindows_serializes_as_tagged_unit() {
        let json = serde_json::to_string(&Request::GetWindows).unwrap();
        assert_eq!(json, r#"{"type":"GetWindows"}"#);
    }

    #[test]
    fn hello_round_trips() {
        let req = Request::Hello {
            version: PROTOCOL_VERSION,
            caps: Capabilities {
                query: true,
                control: false,
                session: true,
            },
            scope: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn caps_intersect_and_force_query() {
        let client = Capabilities {
            query: true,
            control: true,
            session: true,
        };
        let policy = Capabilities::QUERY; // query only
        let granted = policy.intersect(client).with_query_always();
        assert!(granted.query);
        assert!(!granted.control);
        assert!(!granted.session);
    }

    #[test]
    fn windows_response_round_trips_with_a_window() {
        let mut w = Window::new(WindowId(42));
        w.title = Some("demo".into());
        w.app_id = Some("org.example.app".into());
        w.state.activated = true;
        let resp = Response::Windows { windows: vec![w] };
        let json = serde_json::to_string(&resp).unwrap();
        let back: Response = serde_json::from_str(&json).unwrap();
        match back {
            Response::Windows { windows } => {
                assert_eq!(windows.len(), 1);
                assert_eq!(windows[0].id, WindowId(42));
                assert_eq!(windows[0].title.as_deref(), Some("demo"));
                assert!(windows[0].state.activated);
            }
            _ => panic!("expected Windows"),
        }
    }

    #[test]
    fn command_round_trips_and_tags() {
        let cmd = Command::Close { id: WindowId(7) };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""type":"Close""#), "{json}");
        assert!(json.contains(r#""id":7"#), "{json}");
        let back: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cmd);
    }

    #[test]
    fn required_cap_separates_control_and_session() {
        assert!(Command::Focus { id: WindowId(1) }.required_cap().control);
        assert!(Command::Cycle { forward: true }.required_cap().control);
        assert!(Command::Quit.required_cap().session);
        assert!(!Command::Quit.required_cap().control);
    }

    #[test]
    fn event_serializes_as_tagged_unit() {
        let json = serde_json::to_string(&Event::WindowsChanged).unwrap();
        assert_eq!(json, r#"{"type":"WindowsChanged"}"#);
        let back: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(back, Event::WindowsChanged);
    }

    #[test]
    fn switch_workspace_command_round_trips() {
        // A nested internally-tagged enum (Command variant carrying `Switch`).
        let cmd = Command::SwitchWorkspace { dir: Switch::Next };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""type":"SwitchWorkspace""#), "{json}");
        assert!(json.contains(r#""dir":{"type":"Next"}"#), "{json}");
        let back: Command = serde_json::from_str(&json).unwrap();
        assert_eq!(back, cmd);
    }

    #[test]
    fn toggle_tiling_command_round_trips() {
        let json = serde_json::to_string(&Command::ToggleTiling).unwrap();
        assert_eq!(json, r#"{"type":"ToggleTiling"}"#);
        assert_eq!(
            serde_json::from_str::<Command>(&json).unwrap(),
            Command::ToggleTiling
        );
        assert!(Command::ToggleTiling.required_cap().control);
    }

    #[test]
    fn move_to_workspace_command_round_trips() {
        let cmd = Command::MoveToWorkspace {
            window: WindowId(42),
            workspace: WorkspaceId(3),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert!(json.contains(r#""type":"MoveToWorkspace""#), "{json}");
        assert!(
            json.contains(r#""window":42"#) && json.contains(r#""workspace":3"#),
            "{json}"
        );
        assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd);
        assert!(cmd.required_cap().control);
    }

    #[test]
    fn dismiss_notification_command_round_trips() {
        let cmd = Command::DismissNotification { id: 7 };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(json, r#"{"type":"DismissNotification","id":7}"#);
        assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd);
        assert!(cmd.required_cap().control);
    }

    #[test]
    fn unscoped_scope_permits_everything() {
        let s = Scope::unscoped();
        assert!(s.permits(&Command::Focus { id: WindowId(1) }));
        assert!(s.permits(&Command::Close { id: WindowId(99) }));
        assert!(s.permits(&Command::Quit));
    }

    #[test]
    fn scoped_ops_reject_unlisted_commands() {
        let s = Scope {
            ops: Some(vec![OpClass::Focus]),
            ..Scope::default()
        };
        assert!(s.permits(&Command::Focus { id: WindowId(1) }));
        assert!(!s.permits(&Command::Close { id: WindowId(1) }));
    }

    #[test]
    fn scoped_windows_enforce_allowlist() {
        let s = Scope {
            windows: Some(vec![WindowId(1), WindowId(2)]),
            ..Scope::default()
        };
        assert!(s.permits(&Command::Focus { id: WindowId(1) }));
        assert!(s.permits(&Command::Focus { id: WindowId(2) }));
        assert!(!s.permits(&Command::Focus { id: WindowId(3) }));
    }

    #[test]
    fn session_commands_bypass_scope() {
        let s = Scope {
            ops: Some(vec![]),
            ..Scope::default()
        };
        assert!(s.permits(&Command::Quit), "Quit is session-level");
    }

    #[test]
    fn move_to_workspace_checks_both_window_and_workspace() {
        let s = Scope {
            windows: Some(vec![WindowId(1)]),
            workspaces: Some(vec![WorkspaceId(2)]),
            ..Scope::default()
        };
        assert!(s.permits(&Command::MoveToWorkspace {
            window: WindowId(1),
            workspace: WorkspaceId(2)
        }));
        assert!(!s.permits(&Command::MoveToWorkspace {
            window: WindowId(1),
            workspace: WorkspaceId(3)
        }));
        assert!(!s.permits(&Command::MoveToWorkspace {
            window: WindowId(9),
            workspace: WorkspaceId(2)
        }));
    }

    #[test]
    fn hello_with_scope_name_round_trips() {
        let req = Request::Hello {
            version: PROTOCOL_VERSION,
            caps: Capabilities::QUERY,
            scope: Some("browser-helper".into()),
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }
}
