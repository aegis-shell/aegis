//! The schema for the ass IPC.
//!
//! One major version ([`PROTOCOL_VERSION`]); a client offering any other
//! major version is refused at the handshake. Messages are internally
//! tagged (`{"type": "..."}`) so the wire is self-describing and new
//! variants add without renaming existing fields. See
//! [ADR-0027](../../docs/adr/0027-ipc-and-introspection.md).

use ass_core::Rect;
use ass_core::input::SyntheticInputAction;
use ass_core::notify::Notification;
use ass_core::output::OutputInfo;
use ass_core::realm::{
    RealmBundle, RealmId, RealmMutation, RealmRevocation, RealmSnapshot, RealmTransactionReceipt,
    RealmWindowPlacement, SeatCapabilities, VirtualOutput,
};
use ass_core::window::{Window, WindowId};
use ass_core::workspace::{OutputId, Switch, WorkspaceId, WorkspaceSnapshot};

use crate::journal::{JournalEntry, JournalSnapshot};

/// The protocol major version this build speaks. A client must offer the
/// same major version at the [`Request::Hello`] handshake. Version 3 adds
/// Realm authority, explicit time-bounded leases, optimistic transactions,
/// and directed virtual-output capture.
pub const PROTOCOL_VERSION: u32 = 3;
/// Built-in owner-only scope used by the compositor's reference CLI for
/// Realm recovery and administration. The Unix socket remains user-private;
/// naming this scope opts the connection into the high-risk Realm operation
/// allowlist and its time-bounded lease.
pub const LOCAL_REALM_ADMIN_SCOPE: &str = "ass-control-realm-admin";

/// The capability classes a client may hold (ADR-0027).
///
/// `query` is always granted (read state + subscribe); `control`, `input`,
/// and `session` require the server's policy to allow them. Serialized as an
/// object so tool authors read it without decoding a bitmask. New fields use
/// serde defaults so a version-2 peer that predates them negotiates them off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Capabilities {
    /// Read state and subscribe to events. Always granted.
    pub query: bool,
    /// Mutate windows, workspaces, and input focus.
    pub control: bool,
    /// Inject bounded, target-local input actions. Named scope required.
    #[serde(default)]
    pub input: bool,
    /// Session-level actions: quit, reload config, change outputs.
    pub session: bool,
    /// Create, configure, transfer, pause, and revoke Realm authority.
    #[serde(default)]
    pub realm: bool,
}

impl Capabilities {
    /// Query only.
    pub const QUERY: Self = Self {
        query: true,
        control: false,
        input: false,
        session: false,
        realm: false,
    };

    /// Intersection of two capability sets. Used to fold the client's request
    /// against the server's policy.
    pub fn intersect(self, other: Self) -> Self {
        Self {
            query: self.query && other.query,
            control: self.control && other.control,
            input: self.input && other.input,
            session: self.session && other.session,
            realm: self.realm && other.realm,
        }
    }

    /// Force `query` on, per the ADR's "always allowed" rule.
    pub fn with_query_always(self) -> Self {
        Self {
            query: true,
            ..self
        }
    }

    pub fn privileged(self) -> bool {
        self.control || self.input || self.session || self.realm
    }
}

/// Requested duration for the connection's privileged capability lease.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LeaseRequest {
    pub ttl_ms: u64,
}

impl Default for LeaseRequest {
    fn default() -> Self {
        Self { ttl_ms: 900_000 }
    }
}

/// Server-issued, connection-bound lease. The id is audit metadata; clients
/// do not echo it on each request, so it cannot be replayed on another
/// connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct LeaseGrant {
    pub id: u64,
    pub ttl_ms: u64,
    pub renewable: bool,
}

/// One operation family, used by [`Scope`] to enumerate which commands a
/// scoped client may issue (ADR-0034). One variant per scoped `Command`;
/// session commands (Quit) are governed by caps alone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum OpClass {
    Focus,
    Minimize,
    Close,
    Move,
    SetWindowGeometry,
    InjectInput,
    InjectRealmInput,
    Cycle,
    SwitchWorkspace,
    SwitchWorkspaceTo,
    MoveToWorkspace,
    ToggleTiling,
    Notify,
    DismissNotification,
    Screenshot,
    ScreenshotRegion,
    ToggleOverview,
    CaptureOutput,
    CreateRealm,
    TransactRealm,
    RevokeRealm,
    CaptureRealm,
    LaunchInRealm,
}

/// A resource-and-operation allowlist layered on top of capabilities
/// (ADR-0034). `None` at any field means "unrestricted at this axis". The
/// default (all fields `None`) is the unscoped back-compat behavior for
/// ordinary operations. High-risk input is the exception: `InjectInput` must
/// be explicitly present in `ops`.
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
    /// Allowed Realm ids. `None` = all existing realms. Creating a new Realm
    /// is governed only by the operation allowlist because its id does not
    /// exist yet.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub realms: Option<Vec<RealmId>>,
    /// Allowed operation families. `None` = all ordinary operations, but no
    /// synthetic input.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ops: Option<Vec<OpClass>>,
}

impl Scope {
    /// The unscoped compatibility default. Synthetic input still requires an
    /// explicit operation allowlist entry.
    pub fn unscoped() -> Self {
        Scope::default()
    }

    /// Whether `val` is allowed by the `None`-means-all allowlist.
    fn allows<T: PartialEq + Copy>(opt: &Option<Vec<T>>, val: T) -> bool {
        opt.as_ref().is_none_or(|v| v.contains(&val))
    }

    pub fn permits_window(&self, window: WindowId) -> bool {
        Self::allows(&self.windows, window)
    }

    pub fn permits_realm(&self, realm: RealmId) -> bool {
        Self::allows(&self.realms, realm)
    }

    pub fn permits_realm_action(&self, action: &RealmAction) -> bool {
        let op = action.op_class();
        if !self
            .ops
            .as_ref()
            .is_some_and(|operations| operations.contains(&op))
        {
            return false;
        }
        match action {
            RealmAction::Create { .. } => true,
            RealmAction::Transact { mutations, .. } => mutations.iter().all(|mutation| {
                let realm = match mutation {
                    RealmMutation::TransferWindow { target, .. } => *target,
                    RealmMutation::SetObserver { realm, .. }
                    | RealmMutation::ConfigureOutput { realm, .. }
                    | RealmMutation::SetState { realm, .. } => *realm,
                };
                Self::allows(&self.realms, realm)
                    && match mutation {
                        RealmMutation::TransferWindow { window, .. } => {
                            Self::allows(&self.windows, *window)
                        }
                        _ => true,
                    }
            }),
            RealmAction::Revoke {
                realm, fallback, ..
            } => Self::allows(&self.realms, *realm) && Self::allows(&self.realms, *fallback),
        }
    }

    pub fn permits_realm_capture(&self, realm: RealmId) -> bool {
        self.ops
            .as_ref()
            .is_some_and(|operations| operations.contains(&OpClass::CaptureRealm))
            && Self::allows(&self.realms, realm)
    }

    /// Whether this scope permits the given command (ADR-0034). Session
    /// commands bypass scope; control commands check ops + resources.
    pub fn permits(&self, cmd: &Command) -> bool {
        let need = cmd.required_cap();
        if need.session {
            return true;
        }
        if need.input || need.realm {
            // Input and Realm lifecycle are high-risk capabilities with no
            // compatibility caller: a named scope must opt in explicitly.
            if !self
                .ops
                .as_ref()
                .is_some_and(|ops| ops.contains(&cmd.op_class()))
            {
                return false;
            }
        } else if let Some(ops) = &self.ops
            && !ops.contains(&cmd.op_class())
        {
            return false;
        }
        match cmd {
            Command::Focus { id }
            | Command::Minimize { id }
            | Command::Close { id }
            | Command::Move { id }
            | Command::SetWindowGeometry { id, .. }
            | Command::InjectInput { id, .. } => Self::allows(&self.windows, *id),
            Command::InjectRealmInput { realm, id, .. } => {
                Self::allows(&self.realms, *realm) && Self::allows(&self.windows, *id)
            }
            Command::LaunchInRealm { realm, .. } => Self::allows(&self.realms, *realm),
            Command::MoveToWorkspace { window, workspace } => {
                Self::allows(&self.windows, *window) && Self::allows(&self.workspaces, *workspace)
            }
            Command::SwitchWorkspaceTo { id } => Self::allows(&self.workspaces, *id),
            _ => true,
        }
    }
}

/// Synchronous Realm lifecycle operation. Unlike ordinary compositor
/// commands, the response confirms commit and carries its authoritative
/// receipt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum RealmAction {
    Create {
        label: String,
        capabilities: SeatCapabilities,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<VirtualOutput>,
    },
    Transact {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<u64>,
        mutations: Vec<RealmMutation>,
    },
    Revoke {
        realm: RealmId,
        fallback: RealmId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<u64>,
    },
}

impl RealmAction {
    pub fn op_class(&self) -> OpClass {
        match self {
            Self::Create { .. } => OpClass::CreateRealm,
            Self::Transact { .. } => OpClass::TransactRealm,
            Self::Revoke { .. } => OpClass::RevokeRealm,
        }
    }

    pub fn validate(&self) -> Result<(), &'static str> {
        match self {
            Self::Create {
                label,
                capabilities,
                output,
            } => {
                if label.trim().is_empty() || label.len() > 128 {
                    return Err("realm label length is out of range");
                }
                if !capabilities.pointer && !capabilities.keyboard && !capabilities.touch {
                    return Err("realm must expose at least one input capability");
                }
                if output.is_some_and(|output| !output.validate()) {
                    return Err("virtual output parameters are invalid");
                }
                Ok(())
            }
            Self::Transact { mutations, .. } if mutations.is_empty() || mutations.len() > 64 => {
                Err("realm transaction size is out of range")
            }
            Self::Transact { mutations, .. } => {
                if mutations.iter().any(|mutation| {
                    matches!(
                        mutation,
                        RealmMutation::SetState {
                            state: ass_core::realm::RealmState::Revoked,
                            ..
                        }
                    )
                }) {
                    return Err("revocation is a separate lifecycle operation");
                }
                Ok(())
            }
            Self::Revoke {
                realm, fallback, ..
            } if realm == fallback => Err("realm and fallback must differ"),
            Self::Revoke { .. } => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum RealmActionResult {
    Created { bundle: RealmBundle },
    TransactionCommitted { receipt: RealmTransactionReceipt },
    Revoked { receipt: RealmRevocation },
}

/// A mutation the compositor applies on its main loop. Mirrors the operations
/// the chrome and the key bindings already perform. Serialized as a tagged
/// table so new commands add without renaming existing ones.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum Command {
    /// Focus (activate) a toplevel by id. `control`.
    Focus { id: WindowId },
    /// Minimize a toplevel by id while keeping it mapped. `control`.
    Minimize { id: WindowId },
    /// Close a toplevel by id. `control`.
    Close { id: WindowId },
    /// Begin an interactive move of a toplevel by id. `control`.
    Move { id: WindowId },
    /// Set a floating toplevel's geometry in compositor logical coordinates.
    /// The server validates and clamps the requested size to client hints.
    /// `control`.
    SetWindowGeometry { id: WindowId, rect: Rect },
    /// Deliver self-contained input actions in target-window-local logical
    /// coordinates. Requires the separate `input` capability and a named
    /// scope that grants both this operation and the target window.
    InjectInput {
        id: WindowId,
        actions: Vec<SyntheticInputAction>,
    },
    /// Deliver target-local input through the independent seat owned by a
    /// Realm. The compositor resolves the live seat and rechecks authority at
    /// apply time; physical desktop focus and chrome are never consulted.
    InjectRealmInput {
        realm: RealmId,
        id: WindowId,
        actions: Vec<SyntheticInputAction>,
    },
    /// Launch a desktop entry through a private mount-scoped Wayland portal
    /// and Linux namespace sandbox.
    LaunchInRealm { realm: RealmId, desktop_id: String },
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
    /// Capture the focused output and write it as a PNG file (M9 screenshot
    /// path). The compositor refuses while the session is locked. `control`.
    /// When `region` is present, only that logical-pixel rectangle is captured.
    Screenshot {
        path: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<Rect>,
    },
    /// Toggle the window/workspace overview (M9). `control`.
    ToggleOverview,
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
                input: false,
                session: true,
                realm: false,
            },
            Command::InjectInput { .. } | Command::InjectRealmInput { .. } => Capabilities {
                query: false,
                control: false,
                input: true,
                session: false,
                realm: false,
            },
            Command::LaunchInRealm { .. } => Capabilities {
                query: false,
                control: false,
                input: false,
                session: false,
                realm: true,
            },
            _ => Capabilities {
                query: false,
                control: true,
                input: false,
                session: false,
                realm: false,
            },
        }
    }

    /// Validate transport-level bounds before a command reaches the main
    /// loop. Resource state (window existence, visibility, and local pointer
    /// bounds) remains the compositor's responsibility at apply time.
    pub fn validate(&self) -> Result<(), &'static str> {
        const MAX_GEOMETRY_EXTENT: i32 = 32_768;
        const MAX_INPUT_ACTIONS: usize = 64;
        const MAX_SCROLL_DELTA: f32 = 1_000.0;
        match self {
            Command::SetWindowGeometry { rect, .. }
            | Command::Screenshot {
                region: Some(rect), ..
            } if !(1..=MAX_GEOMETRY_EXTENT).contains(&rect.size.w)
                || !(1..=MAX_GEOMETRY_EXTENT).contains(&rect.size.h)
                || rect.origin.x.checked_add(rect.size.w).is_none()
                || rect.origin.y.checked_add(rect.size.h).is_none() =>
            {
                Err("geometry size is out of range")
            }
            Command::InjectInput { actions, .. } | Command::InjectRealmInput { actions, .. }
                if actions.is_empty() || actions.len() > MAX_INPUT_ACTIONS =>
            {
                Err("input action count is out of range")
            }
            Command::InjectInput { actions, .. } | Command::InjectRealmInput { actions, .. } => {
                for action in actions {
                    match *action {
                        SyntheticInputAction::Click { button, .. }
                            if !(0x110..=0x117).contains(&button) =>
                        {
                            return Err("input button code is out of range");
                        }
                        SyntheticInputAction::Scroll { dx, dy, .. }
                            if !dx.is_finite()
                                || !dy.is_finite()
                                || dx.abs() > MAX_SCROLL_DELTA
                                || dy.abs() > MAX_SCROLL_DELTA =>
                        {
                            return Err("input scroll delta is out of range");
                        }
                        SyntheticInputAction::KeyPress { code } if code > 0x2ff => {
                            return Err("input key code is out of range");
                        }
                        _ => {}
                    }
                }
                Ok(())
            }
            Command::LaunchInRealm { desktop_id, .. }
                if desktop_id.trim().is_empty() || desktop_id.len() > 512 =>
            {
                Err("desktop id length is out of range")
            }
            _ => Ok(()),
        }
    }

    /// The [`OpClass`] this command belongs to, for scope checks (ADR-0034).
    /// Session commands (Quit) have no OpClass; scope does not apply to them.
    pub fn op_class(&self) -> OpClass {
        match self {
            Command::Focus { .. } => OpClass::Focus,
            Command::Minimize { .. } => OpClass::Minimize,
            Command::Close { .. } => OpClass::Close,
            Command::Move { .. } => OpClass::Move,
            Command::SetWindowGeometry { .. } => OpClass::SetWindowGeometry,
            Command::InjectInput { .. } => OpClass::InjectInput,
            Command::InjectRealmInput { .. } => OpClass::InjectRealmInput,
            Command::LaunchInRealm { .. } => OpClass::LaunchInRealm,
            Command::Cycle { .. } => OpClass::Cycle,
            Command::SwitchWorkspace { .. } => OpClass::SwitchWorkspace,
            Command::SwitchWorkspaceTo { .. } => OpClass::SwitchWorkspaceTo,
            Command::MoveToWorkspace { .. } => OpClass::MoveToWorkspace,
            Command::ToggleTiling => OpClass::ToggleTiling,
            Command::Notify { .. } => OpClass::Notify,
            Command::DismissNotification { .. } => OpClass::DismissNotification,
            Command::Screenshot {
                region: Some(_), ..
            } => OpClass::ScreenshotRegion,
            Command::Screenshot { region: None, .. } => OpClass::Screenshot,
            Command::ToggleOverview => OpClass::ToggleOverview,
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
    /// Realm authority, lifecycle, or presentation changed. Consumers re-query
    /// the snapshot and can use `revision` to discard stale state.
    RealmsChanged { revision: u64 },
    /// A Realm-directed scene changed. Damage is expressed in that Realm's
    /// virtual-output logical coordinates and is conservative: every changed
    /// pixel is included, but topology changes may invalidate the full output.
    /// Pixels remain pull-based through `CaptureRealm`.
    RealmDamaged {
        realm: RealmId,
        sequence: u64,
        revision: u64,
        damage: Vec<Rect>,
    },
}

/// One atomic observation of a Realm's directed virtual output.
///
/// `region` and every placement use virtual-output logical coordinates.
/// `width` and `height` are the encoded PNG's physical-pixel extent after
/// applying `scale_milli`. The placement snapshot, pixels, and authority
/// `revision` are captured together on the compositor thread, so callers can
/// safely map a pixel observation back to target-local input coordinates.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RealmCapture {
    pub realm: RealmId,
    pub width: u32,
    pub height: u32,
    pub scale_milli: u32,
    pub region: Rect,
    pub placements: Vec<RealmWindowPlacement>,
    /// Byte length of the sealed PNG memfd sent immediately after this JSON
    /// response through `SCM_RIGHTS`.
    pub png_bytes: u64,
    pub revision: u64,
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
        /// Required to retain any privileged capability. Query-only
        /// connections may omit it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lease: Option<LeaseRequest>,
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
    /// Fetch the complete authority snapshot.
    GetRealms,
    /// Commit a Realm lifecycle operation and return its receipt.
    Realm { action: RealmAction },
    /// Submit a [`Command`]. Fire-and-forget: the server acknowledges queuing
    /// with [`Response::Ok`], not completion. Requires the command's capability.
    Do { cmd: Command },
    /// Opt into server-pushed [`Event`]s on this connection. Idempotent.
    Subscribe,
    /// Opt into server-pushed [`Event::Journal`] entries (ADR-0033). Separate
    /// from [`Subscribe`](Self::Subscribe) so status bars that only need the
    /// coarse re-query signal are not flooded with per-command entries.
    SubscribeJournal,
    /// Renew the connection-bound privileged lease. A lease cannot outlive
    /// this connection and is capped by server policy.
    RenewLease { ttl_ms: u64 },
    /// Capture the focused output as a PNG (M10 pixel capture). The response
    /// metadata is followed by a sealed memfd sent with `SCM_RIGHTS`.
    /// Privacy-sensitive: requires `control`, an explicit
    /// [`OpClass::CaptureOutput`] entry in a named scope's `ops` (never
    /// inherited), and is refused while the session is locked.
    /// When `region` is present, only that logical-pixel rectangle is captured.
    CaptureOutput {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<Rect>,
    },
    /// Capture one Realm's directed virtual output. Coordinates are Realm
    /// logical pixels and never include compositor chrome or another Realm.
    CaptureRealm {
        realm: RealmId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<Rect>,
    },
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
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lease: Option<LeaseGrant>,
    },
    /// Reply to [`Request::GetWindows`].
    Windows {
        windows: Vec<Window>,
    },
    /// Reply to [`Request::GetWorkspaces`].
    Workspaces {
        snapshot: WorkspaceSnapshot,
    },
    /// Reply to [`Request::GetNotifications`].
    Notifications {
        notifications: Vec<Notification>,
    },
    /// Reply to [`Request::GetOutputs`].
    Outputs {
        outputs: Vec<OutputInfo>,
    },
    /// Reply to [`Request::GetJournal`] (ADR-0033).
    Journal {
        snapshot: JournalSnapshot,
    },
    Realms {
        snapshot: RealmSnapshot,
    },
    Realm {
        result: RealmActionResult,
    },
    /// Reply to [`Request::CaptureOutput`]: the output's physical size and
    /// length of the sealed PNG memfd that immediately follows it.
    CaptureOutput {
        width: u32,
        height: u32,
        /// Byte length of the sealed PNG memfd that follows this metadata.
        png_bytes: u64,
    },
    CaptureRealm {
        capture: RealmCapture,
    },
    LeaseRenewed {
        lease: LeaseGrant,
    },
    /// Acknowledgment of a queued [`Request::Do`].
    Ok,
    /// Reply to [`Request::Subscribe`]: events will now be pushed.
    Subscribed,
    /// An error servicing a request. The connection stays open unless the
    /// error is a protocol violation (wrong version, missing handshake).
    Error {
        message: String,
    },
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
                input: false,
                session: true,
                realm: false,
            },
            scope: None,
            lease: Some(LeaseRequest::default()),
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
            input: true,
            session: true,
            realm: true,
        };
        let policy = Capabilities::QUERY; // query only
        let granted = policy.intersect(client).with_query_always();
        assert!(granted.query);
        assert!(!granted.control);
        assert!(!granted.input);
        assert!(!granted.session);
    }

    #[test]
    fn capabilities_from_older_v2_peer_default_input_off() {
        let caps: Capabilities =
            serde_json::from_str(r#"{"query":true,"control":true,"session":false}"#).unwrap();
        assert!(caps.query);
        assert!(caps.control);
        assert!(!caps.input);
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
    fn minimize_command_round_trips_and_is_window_scoped() {
        let cmd = Command::Minimize { id: WindowId(9) };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(json, r#"{"type":"Minimize","id":9}"#);
        assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd);

        let scope = Scope {
            windows: Some(vec![WindowId(9)]),
            ops: Some(vec![OpClass::Minimize]),
            ..Scope::default()
        };
        assert!(scope.permits(&cmd));
        assert!(!scope.permits(&Command::Minimize { id: WindowId(10) }));
    }

    #[test]
    fn geometry_command_is_control_scoped_and_validated() {
        let cmd = Command::SetWindowGeometry {
            id: WindowId(9),
            rect: Rect::new(10, 20, 800, 600),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd);
        assert!(cmd.required_cap().control);
        assert!(cmd.validate().is_ok());
        assert!(
            Command::SetWindowGeometry {
                id: WindowId(9),
                rect: Rect::new(0, 0, 0, 600),
            }
            .validate()
            .is_err()
        );

        let scope = Scope {
            windows: Some(vec![WindowId(9)]),
            ops: Some(vec![OpClass::SetWindowGeometry]),
            ..Scope::default()
        };
        assert!(scope.permits(&cmd));
    }

    #[test]
    fn synthetic_input_is_separately_capability_and_window_scoped() {
        let cmd = Command::InjectInput {
            id: WindowId(9),
            actions: vec![SyntheticInputAction::Click {
                position: ass_core::Point { x: 20, y: 30 },
                button: 0x110,
            }],
        };
        let json = serde_json::to_string(&cmd).unwrap();
        assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd);
        let cap = cmd.required_cap();
        assert!(cap.input);
        assert!(!cap.control);
        assert!(cmd.validate().is_ok());

        let scope = Scope {
            windows: Some(vec![WindowId(9)]),
            ops: Some(vec![OpClass::InjectInput]),
            ..Scope::default()
        };
        assert!(scope.permits(&cmd));
        assert!(!scope.permits(&Command::InjectInput {
            id: WindowId(10),
            actions: vec![SyntheticInputAction::KeyPress { code: 30 }],
        }));
        assert!(
            Command::InjectInput {
                id: WindowId(9),
                actions: vec![],
            }
            .validate()
            .is_err()
        );
    }

    #[test]
    fn required_cap_separates_control_and_session() {
        assert!(Command::Focus { id: WindowId(1) }.required_cap().control);
        assert!(Command::Cycle { forward: true }.required_cap().control);
        assert!(Command::Quit.required_cap().session);
        assert!(!Command::Quit.required_cap().control);
        assert!(
            Command::InjectInput {
                id: WindowId(1),
                actions: vec![SyntheticInputAction::KeyPress { code: 30 }],
            }
            .required_cap()
            .input
        );
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
        assert!(!s.permits(&Command::InjectInput {
            id: WindowId(1),
            actions: vec![SyntheticInputAction::KeyPress { code: 30 }],
        }));
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
    fn screenshot_command_op_class_depends_on_region_presence() {
        let full = Command::Screenshot {
            path: "a.png".into(),
            region: None,
        };
        let region = Command::Screenshot {
            path: "b.png".into(),
            region: Some(Rect::new(10, 20, 100, 80)),
        };
        assert_eq!(full.op_class(), OpClass::Screenshot);
        assert_eq!(region.op_class(), OpClass::ScreenshotRegion);

        let full_scope = Scope {
            ops: Some(vec![OpClass::Screenshot]),
            ..Scope::default()
        };
        let region_scope = Scope {
            ops: Some(vec![OpClass::ScreenshotRegion]),
            ..Scope::default()
        };
        assert!(full_scope.permits(&full));
        assert!(!full_scope.permits(&region));
        assert!(region_scope.permits(&region));
        assert!(!region_scope.permits(&full));
    }

    #[test]
    fn capture_output_request_round_trips_with_optional_region() {
        let req = Request::CaptureOutput {
            region: Some(Rect::new(10, 20, 100, 80)),
        };
        let json = serde_json::to_string(&req).unwrap();
        assert!(json.contains("\"region\""), "{json}");
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);

        let default: Request = serde_json::from_str(r#"{"type":"CaptureOutput"}"#).unwrap();
        assert_eq!(default, Request::CaptureOutput { region: None });
    }

    #[test]
    fn realm_capture_response_round_trips_correlated_layout_metadata() {
        let capture = RealmCapture {
            realm: RealmId(7),
            width: 500,
            height: 250,
            scale_milli: 1250,
            region: Rect::new(100, 50, 400, 200),
            placements: vec![RealmWindowPlacement {
                window: WindowId(42),
                output_rect: Rect::new(120, 70, 300, 150),
                surface_size: ass_core::Size { w: 900, h: 450 },
            }],
            png_bytes: 3,
            revision: 19,
        };
        let json =
            serde_json::to_string(&Response::CaptureRealm { capture }).expect("serialize capture");
        let decoded: Response = serde_json::from_str(&json).expect("deserialize capture");
        let Response::CaptureRealm { capture } = decoded else {
            panic!("expected Realm capture response");
        };
        assert_eq!(capture.realm, RealmId(7));
        assert_eq!(capture.region, Rect::new(100, 50, 400, 200));
        assert_eq!(capture.placements[0].window, WindowId(42));
        assert_eq!(
            capture.placements[0].surface_size,
            ass_core::Size { w: 900, h: 450 }
        );
        assert_eq!(capture.revision, 19);
    }

    #[test]
    fn hello_with_scope_name_round_trips() {
        let req = Request::Hello {
            version: PROTOCOL_VERSION,
            caps: Capabilities::QUERY,
            scope: Some("browser-helper".into()),
            lease: None,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: Request = serde_json::from_str(&json).unwrap();
        assert_eq!(back, req);
    }
}
