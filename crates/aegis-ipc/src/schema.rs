//! The schema for the aegis IPC.
//!
//! One major version ([`PROTOCOL_VERSION`]); a client offering a NEWER
//! major version is refused at the handshake, while an older client is
//! answered at its own version (the server speaks the minimum of the two).
//! Messages are internally
//! tagged (`{"type": "..."}`) so the wire is self-describing and new
//! variants add without renaming existing fields. See
//! [ADR-0027](../../docs/adr/0027-ipc-and-introspection.md).

use std::path::PathBuf;

use aegis_model::Rect;
use aegis_model::input::SyntheticInputAction;
use aegis_model::interaction_domain::{
    InteractionDomainBundle, InteractionDomainId, InteractionDomainMutation,
    InteractionDomainRevocation, InteractionDomainSnapshot, InteractionDomainTransactionReceipt,
    InteractionDomainWindowPlacement, SeatCapabilities, VirtualOutput,
};
use aegis_model::notify::Notification;
use aegis_model::output::OutputInfo;
#[cfg(test)]
use aegis_model::semantic::{SemanticAction, SemanticRole, SemanticSnapshot, SemanticState};
pub use aegis_model::settings::{SettingsAction, SettingsReceipt, SettingsSnapshot};
pub use aegis_model::system::{SystemAction, SystemStatus};
use aegis_model::window::{SpaceUse, Window, WindowId};
use aegis_model::workspace::{LaunchPlacement, Switch, WorkspaceId, WorkspaceSnapshot};
pub use aegis_security::authority::{
    ActorActionIntent, ActorActionReceipt, ActorCapability, ActorResource, AuthorizationDecision,
    ObservationToken, ResourceGrant, ResourceGrantId, SemanticObservation,
};

use crate::journal::{JournalEntry, JournalSnapshot};
pub use aegis_semantic::{
    AccessibilityTreeUpdate, AccessibilityWindowBinding, SemanticActionRequest,
};

/// The protocol major version this build speaks. A client offering a newer
/// major version is refused at the [`Request::Hello`] handshake; an older
/// client is answered at its own version. Version 27 adds workspace-directed
/// application launching (`LaunchApp` with an optional `LaunchPlacement` that
/// never switches the user's view) and the additive `reveal` flag on `Focus`
/// (ADR-0118). Version 26 adds per-window content
/// capture (`CaptureWindow` → `WindowCapture`): one authorized window's real
/// pixels, offscreen-rendered wherever the window lives. Version 25 adds the zero-copy
/// dmabuf slot streaming protocol (slot descriptors on
/// `StreamOutputStarted`, slot-referenced frames, `StreamBufferRelease`)
/// and the handshake downgrade. Version 24 binds
/// accessibility windows to kernel-authenticated Wayland process ids before
/// accepting AT-SPI trees. Version 23 adds
/// explicit Actor sessions, exact resource-grant handles, and the bounded
/// accessibility tree/action adapter protocol. Version 22 renamed the
/// compositor authority boundary from Realm to Interaction Domain and moved
/// observation-bound action contracts into the transport-neutral authority
/// kernel. Version 21 made
/// Interaction Domain input observation-bound and synchronous: semantic observations and
/// captures issue short-lived, connection-bound tokens consumed by
/// `ActInInteractionDomain`, which returns an authoritative main-loop receipt. Version 20
/// removes compositor filesystem selection (`PickFile`, `FilePicked`, and
/// their scope/types); FileChooser is now a portal-owned process boundary.
/// Version 19 binds
/// Agent Interaction Domains to authenticated subjects, reauthorizes live agent ceilings,
/// and separates owner, Interaction Domain, and Agent administration scopes. Version 18 adds
/// agent identity pairing (`Hello.agent`) and runtime-grantable `ask`
/// operations on scopes (ADR-0088). Version 17 adds
/// wallpaper mutation (`SetWallpaper` → `WallpaperSet`). Version 16 adds
/// user-consent yes/no confirmation (`PickConfirm` → `ConfirmPicked`).
/// Version 15 adds
/// user-consent secret prompting (`PromptSecret` → `SecretPrompted`).
/// Version 14 adds
/// user-consent application picking (`PickApp` → `AppPicked`). Version 13
/// adds user-consent file picking (`PickFile` → `FilePicked`). Version 12 adds
/// the staged idle-policy snapshot and transaction. Version 11 makes live
/// system-control replies authoritative main-loop receipts. Version 10 adds
/// the complete effective desktop-preferences snapshot and transaction.
/// Version 9 adds compositor maximize/restore commands. Version 8 adds explicit
/// output-space-use transition events. Version 7 adds
/// live system-status queries and immediate system-control commands. Version
/// 6 adds user-consent interactive picking (`PickTarget` → `Picked`, ADR-0054)
/// and the window target on `StreamOutputStart`. Version 5 adds continuous
/// physical-output frame streaming (`StreamOutputStart`,
/// `Event::StreamFrame`, `Event::StreamEnded`, `StreamOutputStop`,
/// ADR-0052). Version 4 adds revisioned desktop-settings snapshots,
/// subscriptions, and confirmed settings transactions.
pub const PROTOCOL_VERSION: u32 = 27;
/// Built-in owner-only scope used by native `aegis` commands for Interaction Domain
/// recovery and administration. The Unix socket remains user-private; naming
/// this scope opts the connection into the high-risk Interaction Domain operation allowlist
/// and its time-bounded lease.
pub const LOCAL_INTERACTION_DOMAIN_ADMIN_SCOPE: &str = "aegis-interaction-domain-admin";
/// Built-in owner-only scope dedicated to agent identity and grant
/// administration. Keeping this separate from ordinary query/control
/// connections prevents ambient local clients from enumerating credentials'
/// metadata or changing capability ceilings.
pub const LOCAL_AGENT_ADMIN_SCOPE: &str = "aegis-agent-admin";
/// Built-in scope for explicitly trusted owner tools such as native `aegis`
/// domain commands.
/// Anonymous connections are query-only when agent lockdown is enabled;
/// naming this scope opts a local owner tool into the ordinary desktop and
/// session capability surface.
pub const LOCAL_OWNER_ADMIN_SCOPE: &str = "aegis-owner-admin";
/// Built-in owner-only scope used by the xdg-desktop-portal backend
/// (`xdg-desktop-portal-aegis`, ADR-0095). It resolves to an explicit
/// allowlist covering
/// capture and streaming, idle inhibition, user-consent pickers and prompts,
/// notifications, and wallpaper changes — and nothing else. Like the Interaction Domain
/// administration
/// scope it does not weaken the socket's owner-only `0600` boundary; it opts
/// the connection into an explicit high-risk operation allowlist and its
/// time-bounded lease.
pub const LOCAL_PORTAL_SCOPE: &str = "aegis-portal";

/// The capability classes a client may hold (ADR-0027).
///
/// `query` is always granted (read state + subscribe); `control`, `input`,
/// and `session` require the server's policy to allow them. Serialized as an
/// object so tool authors read it without decoding a bitmask. New fields use
/// serde defaults so a version-2 peer that predates them negotiates them off.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ConnectionCapabilities {
    /// Read state and subscribe to events. Always granted.
    pub query: bool,
    /// Mutate windows, workspaces, and input focus.
    pub control: bool,
    /// Inject bounded, target-local input actions. Named scope required.
    #[serde(default)]
    pub input: bool,
    /// Session-level actions: quit, reload config, change outputs.
    pub session: bool,
    /// Create, configure, transfer, pause, and revoke Interaction Domain authority.
    #[serde(default)]
    pub interaction_domain: bool,
}

impl ConnectionCapabilities {
    /// Query only.
    pub const QUERY: Self = Self {
        query: true,
        control: false,
        input: false,
        session: false,
        interaction_domain: false,
    };

    /// Intersection of two capability sets. Used to fold the client's request
    /// against the server's policy.
    pub fn intersect(self, other: Self) -> Self {
        Self {
            query: self.query && other.query,
            control: self.control && other.control,
            input: self.input && other.input,
            session: self.session && other.session,
            interaction_domain: self.interaction_domain && other.interaction_domain,
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
        self.control || self.input || self.session || self.interaction_domain
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

/// IPC command-policy adapter for the transport-neutral [`Scope`].
/// Resource axes and capability decisions live in `aegis-security::authority`; only
/// knowledge of this protocol's concrete command vocabulary remains here.
pub trait CommandScopePolicy {
    fn permits_interaction_domain_action(&self, action: &InteractionDomainAction) -> bool;
    fn permits_interaction_domain_action_resources(&self, action: &InteractionDomainAction)
    -> bool;
    fn permits(&self, command: &Command) -> bool;
    fn permits_resources(&self, command: &Command) -> bool;
    fn decide_command(&self, command: &Command) -> AuthorizationDecision;
    fn decide_interaction_domain_action(
        &self,
        action: &InteractionDomainAction,
    ) -> AuthorizationDecision;
}

pub use aegis_security::authority::ActorScope as Scope;

impl CommandScopePolicy for Scope {
    fn permits_interaction_domain_action(&self, action: &InteractionDomainAction) -> bool {
        let op = action.op_class();
        if !self
            .ops
            .as_ref()
            .is_some_and(|operations| operations.contains(&op))
        {
            return false;
        }
        self.permits_interaction_domain_action_resources(action)
    }

    /// The resource-allowlist half of [`Self::permits_interaction_domain_action`],
    /// separated so the ask path enforces Interaction Domain and window allowlists
    /// independently of the operation lists (ADR-0088). This is also the
    /// check applied once a runtime grant has authorized the operation
    /// itself.
    fn permits_interaction_domain_action_resources(
        &self,
        action: &InteractionDomainAction,
    ) -> bool {
        match action {
            InteractionDomainAction::Create { .. } => true,
            InteractionDomainAction::Transact { mutations, .. } => {
                mutations.iter().all(|mutation| {
                    let interaction_domain = match mutation {
                        InteractionDomainMutation::TransferWindow { target, .. } => *target,
                        InteractionDomainMutation::SetObserver {
                            interaction_domain, ..
                        }
                        | InteractionDomainMutation::ConfigureOutput {
                            interaction_domain, ..
                        }
                        | InteractionDomainMutation::SetState {
                            interaction_domain, ..
                        } => *interaction_domain,
                    };
                    self.permits_interaction_domain(interaction_domain)
                        && match mutation {
                            InteractionDomainMutation::TransferWindow { window, .. } => {
                                self.permits_window(*window)
                            }
                            _ => true,
                        }
                })
            }
            InteractionDomainAction::Revoke {
                interaction_domain,
                fallback,
                ..
            } => {
                self.permits_interaction_domain(*interaction_domain)
                    && self.permits_interaction_domain(*fallback)
            }
        }
    }

    /// Whether this scope permits the given command (ADR-0034). Session
    /// commands bypass scope; control commands check ops + resources.
    fn permits(&self, cmd: &Command) -> bool {
        let need = cmd.required_cap();
        if need.session {
            return true;
        }
        if need.input || need.interaction_domain {
            // Input and Interaction Domain lifecycle are high-risk capabilities with no
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
        self.permits_resources(cmd)
    }

    /// The resource-allowlist half of [`Self::permits`], separated so the
    /// ask path enforces window/workspace/Interaction Domain allowlists independently of
    /// the operation lists (ADR-0088). This is also the check applied once
    /// a runtime grant has authorized the operation itself.
    fn permits_resources(&self, cmd: &Command) -> bool {
        match cmd {
            Command::Focus { id, .. }
            | Command::Minimize { id }
            | Command::SetMaximized { id, .. }
            | Command::SetAlwaysOnTop { id, .. }
            | Command::Close { id }
            | Command::Move { id }
            | Command::SetWindowGeometry { id, .. }
            | Command::InjectInput { id, .. } => self.permits_window(*id),
            Command::LaunchInInteractionDomain {
                interaction_domain, ..
            } => self.permits_interaction_domain(*interaction_domain),
            Command::MoveToWorkspace { window, workspace } => {
                self.permits_window(*window) && self.permits_workspace(*workspace)
            }
            Command::SwitchWorkspaceTo { id } => self.permits_workspace(*id),
            Command::LaunchApp {
                placement: Some(LaunchPlacement::Workspace { id }),
                ..
            } => self.permits_workspace(*id),
            Command::LaunchApp { .. } => true,
            _ => true,
        }
    }

    /// Three-way command decision (ADR-0088): a pre-granted command wins; a
    /// command named in `ask_ops` whose resource allowlists pass is
    /// requestable through an interactive grant; anything else is refused.
    fn decide_command(&self, cmd: &Command) -> AuthorizationDecision {
        if self.permits(cmd) {
            return AuthorizationDecision::Permit;
        }
        if !cmd.required_cap().session && self.asks(cmd.op_class()) && self.permits_resources(cmd) {
            AuthorizationDecision::Ask(cmd.op_class())
        } else {
            AuthorizationDecision::Deny
        }
    }

    /// Three-way Interaction Domain-action decision, mirroring [`Self::decide_command`].
    fn decide_interaction_domain_action(
        &self,
        action: &InteractionDomainAction,
    ) -> AuthorizationDecision {
        if self.permits_interaction_domain_action(action) {
            return AuthorizationDecision::Permit;
        }
        let op = action.op_class();
        if self.asks(op) && self.permits_interaction_domain_action_resources(action) {
            AuthorizationDecision::Ask(op)
        } else {
            AuthorizationDecision::Deny
        }
    }
}

/// Synchronous Interaction Domain lifecycle operation. Unlike ordinary compositor
/// commands, the response confirms commit and carries its authoritative
/// receipt.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum InteractionDomainAction {
    Create {
        label: String,
        capabilities: SeatCapabilities,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        output: Option<VirtualOutput>,
    },
    Transact {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<u64>,
        mutations: Vec<InteractionDomainMutation>,
    },
    Revoke {
        interaction_domain: InteractionDomainId,
        fallback: InteractionDomainId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<u64>,
    },
}

impl InteractionDomainAction {
    pub fn op_class(&self) -> ActorCapability {
        match self {
            Self::Create { .. } => ActorCapability::CreateInteractionDomain,
            Self::Transact { .. } => ActorCapability::TransactInteractionDomain,
            Self::Revoke { .. } => ActorCapability::RevokeInteractionDomain,
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
                    return Err("interaction_domain label length is out of range");
                }
                if !capabilities.pointer && !capabilities.keyboard && !capabilities.touch {
                    return Err("interaction_domain must expose at least one input capability");
                }
                if output.is_some_and(|output| !output.validate()) {
                    return Err("virtual output parameters are invalid");
                }
                Ok(())
            }
            Self::Transact { mutations, .. } if mutations.is_empty() || mutations.len() > 64 => {
                Err("interaction_domain transaction size is out of range")
            }
            Self::Transact { mutations, .. } => {
                if mutations.iter().any(|mutation| {
                    matches!(
                        mutation,
                        InteractionDomainMutation::SetState {
                            state: aegis_model::interaction_domain::InteractionDomainState::Revoked,
                            ..
                        }
                    )
                }) {
                    return Err("revocation is a separate lifecycle operation");
                }
                Ok(())
            }
            Self::Revoke {
                interaction_domain,
                fallback,
                ..
            } if interaction_domain == fallback => {
                Err("interaction_domain and fallback must differ")
            }
            Self::Revoke { .. } => Ok(()),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum InteractionDomainActionResult {
    Created {
        bundle: InteractionDomainBundle,
    },
    TransactionCommitted {
        receipt: InteractionDomainTransactionReceipt,
    },
    Revoked {
        receipt: InteractionDomainRevocation,
    },
}

/// A mutation the compositor applies on its main loop. Mirrors the operations
/// the chrome and the key bindings already perform. Serialized as a tagged
/// table so new commands add without renaming existing ones.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum Command {
    /// Focus (activate) a toplevel by id. `control`.
    /// `reveal` (additive since protocol 27, defaults to `true` for older
    /// peers): when `false` the compositor must not switch the view to the
    /// window's workspace; a hidden window is only raised within its own
    /// workspace and never steals the physical seat's keyboard focus.
    Focus {
        id: WindowId,
        #[serde(default = "default_reveal")]
        reveal: bool,
    },
    /// Minimize a toplevel by id while keeping it mapped. `control`.
    Minimize { id: WindowId },
    /// Set or clear compositor-managed maximization for a toplevel. `control`.
    SetMaximized { id: WindowId, maximized: bool },
    /// Set or clear the compositor-internal always-on-top flag for a
    /// toplevel. `control`.
    SetAlwaysOnTop { id: WindowId, on_top: bool },
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
    /// Launch a desktop entry through a private mount-scoped Wayland portal
    /// and Linux namespace sandbox.
    LaunchInInteractionDomain {
        interaction_domain: InteractionDomainId,
        desktop_id: String,
    },
    /// Launch a desktop entry on the desktop (no Interaction Domain sandbox),
    /// optionally directing its first toplevel to a workspace at map time.
    /// A placement never switches the user's current view; the window opens
    /// on the target workspace even while it is hidden. `control`.
    LaunchApp {
        desktop_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        placement: Option<LaunchPlacement>,
    },
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
    /// Apply one immediate live-system control. `control`.
    System { action: SystemAction },
    /// Post a notification (M9, delivered over the IPC). `control`.
    /// `external_id` carries the sender's own notification id (the
    /// Notification portal's per-application id) so a later withdrawal can
    /// be matched; additive since protocol 14, defaulted for older peers.
    Notify {
        summary: String,
        body: String,
        app_id: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        external_id: Option<String>,
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

/// Serde default for [`Command::Focus::reveal`]: peers predating protocol 27
/// expect the historical focus-and-reveal behavior.
fn default_reveal() -> bool {
    true
}

impl Command {
    /// The capability a client must hold to issue this command.
    pub fn required_cap(&self) -> ConnectionCapabilities {
        match self {
            Command::Quit => ConnectionCapabilities {
                query: false,
                control: false,
                input: false,
                session: true,
                interaction_domain: false,
            },
            Command::InjectInput { .. } => ConnectionCapabilities {
                query: false,
                control: false,
                input: true,
                session: false,
                interaction_domain: false,
            },
            Command::LaunchInInteractionDomain { .. } => ConnectionCapabilities {
                query: false,
                control: false,
                input: false,
                session: false,
                interaction_domain: true,
            },
            _ => ConnectionCapabilities {
                query: false,
                control: true,
                input: false,
                session: false,
                interaction_domain: false,
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
            Command::Notify {
                summary,
                body,
                app_id,
                external_id,
            } if summary.trim().is_empty()
                || summary.len() > 1_024
                || body.len() > 16_384
                || app_id.as_ref().is_some_and(|value| value.len() > 512)
                || external_id.as_ref().is_some_and(|value| value.len() > 512)
                || summary.contains('\0')
                || body.contains('\0')
                || app_id.as_ref().is_some_and(|value| value.contains('\0'))
                || external_id
                    .as_ref()
                    .is_some_and(|value| value.contains('\0')) =>
            {
                Err("notification fields are empty, oversized, or contain NUL")
            }
            Command::Screenshot { path, .. }
                if aegis_security::authority::ActorResource::FilesystemPath {
                    path: PathBuf::from(path),
                    access: aegis_security::authority::FilesystemAccess::Write,
                }
                .validate()
                .is_err() =>
            {
                Err("screenshot path must be bounded, absolute, and lexically normalized")
            }
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
            Command::InjectInput { actions, .. }
                if actions.is_empty() || actions.len() > MAX_INPUT_ACTIONS =>
            {
                Err("input action count is out of range")
            }
            Command::InjectInput { actions, .. } => {
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
            Command::LaunchInInteractionDomain { desktop_id, .. }
            | Command::LaunchApp { desktop_id, .. }
                if desktop_id.trim().is_empty()
                    || desktop_id.len() > 512
                    || desktop_id
                        .chars()
                        .any(|character| matches!(character, '\0' | '/' | '\\'))
                    || desktop_id == "."
                    || desktop_id == ".." =>
            {
                Err("desktop id is empty, oversized, or malformed")
            }
            Command::LaunchApp {
                placement: Some(LaunchPlacement::FreshWorkspace { label: Some(label) }),
                ..
            } if label.trim().is_empty() || label.len() > 128 || label.contains('\0') => {
                Err("workspace label is empty, oversized, or contains NUL")
            }
            Command::System { action } => action.validate(),
            _ => Ok(()),
        }
    }

    /// The [`ActorCapability`] this command belongs to, for scope checks (ADR-0034).
    /// Session commands (Quit) have no ActorCapability; scope does not apply to them.
    pub fn op_class(&self) -> ActorCapability {
        match self {
            Command::Focus { .. } => ActorCapability::Focus,
            Command::Minimize { .. } => ActorCapability::Minimize,
            Command::SetMaximized { .. } => ActorCapability::SetWindowGeometry,
            Command::SetAlwaysOnTop { .. } => ActorCapability::SetWindowGeometry,
            Command::Close { .. } => ActorCapability::Close,
            Command::Move { .. } => ActorCapability::Move,
            Command::SetWindowGeometry { .. } => ActorCapability::SetWindowGeometry,
            Command::InjectInput { .. } => ActorCapability::InjectInput,
            Command::LaunchInInteractionDomain { .. } => ActorCapability::LaunchInInteractionDomain,
            Command::LaunchApp { .. } => ActorCapability::LaunchApp,
            Command::Cycle { .. } => ActorCapability::Cycle,
            Command::SwitchWorkspace { .. } => ActorCapability::SwitchWorkspace,
            Command::SwitchWorkspaceTo { .. } => ActorCapability::SwitchWorkspaceTo,
            Command::MoveToWorkspace { .. } => ActorCapability::MoveToWorkspace,
            Command::ToggleTiling => ActorCapability::ToggleTiling,
            Command::System { .. } => ActorCapability::SystemControl,
            Command::Notify { .. } => ActorCapability::Notify,
            Command::DismissNotification { .. } => ActorCapability::DismissNotification,
            Command::Screenshot {
                region: Some(_), ..
            } => ActorCapability::ScreenshotRegion,
            Command::Screenshot { region: None, .. } => ActorCapability::Screenshot,
            Command::ToggleOverview => ActorCapability::ToggleOverview,
            Command::Quit => ActorCapability::ToggleTiling, // unreachable: scope skips session cmds
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
    /// The strongest visible output-space consumer changed. Maximized and
    /// fullscreen remain distinct so shell surfaces and external observers do
    /// not infer fullscreen behavior from a maximized window.
    SpaceUseChanged { state: SpaceUse },
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
    /// Interaction Domain authority, lifecycle, or presentation changed. Consumers re-query
    /// the snapshot and can use `revision` to discard stale state.
    InteractionDomainsChanged { revision: u64 },
    /// Persistent compositor settings changed. Consumers re-query with
    /// [`Request::GetSettings`] and discard snapshots older than `revision`.
    SettingsChanged { revision: u64 },
    /// Live host or compositor-owned session status changed. Consumers
    /// re-query with [`Request::GetSystemStatus`].
    SystemStatusChanged,
    /// An Interaction Domain-directed scene changed. Damage is expressed in that Interaction Domain's
    /// virtual-output logical coordinates and is conservative: every changed
    /// pixel is included, but topology changes may invalidate the full output.
    /// Pixels remain pull-based through `CaptureInteractionDomain`.
    InteractionDomainDamaged {
        interaction_domain: InteractionDomainId,
        sequence: u64,
        revision: u64,
        damage: Vec<Rect>,
    },
    /// One presented output frame for a stream opened with
    /// [`Request::StreamOutputStart`] (ADR-0052). For SHM streams the JSON
    /// event is followed immediately by one sealed memfd of `byte_len`
    /// tightly packed pixels transferred with `SCM_RIGHTS`, reusing the
    /// one-shot capture blob channel (ADR-0041). For dmabuf streams
    /// (version 25) `slot` names one of the descriptors transferred once at
    /// start and no blob follows; the slot stays owned by the consumer
    /// until [`Request::StreamBufferRelease`]. `dropped` is the cumulative
    /// count of frames the stream dropped to backpressure since it started.
    /// `damage` is conservative: the first version reports one full-frame
    /// rectangle.
    StreamFrame {
        stream_id: u64,
        sequence: u64,
        width: u32,
        height: u32,
        stride: u32,
        format: StreamPixelFormat,
        damage: Vec<Rect>,
        dropped: u64,
        byte_len: u64,
        /// dmabuf slot index (version 25); absent on SHM frames.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot: Option<u32>,
    },
    /// The server ended a stream: the connection's scope was revoked or
    /// narrowed, its lease expired, the output geometry changed, or the
    /// compositor is shutting down. Session-lock pauses delivery instead of
    /// ending the stream.
    StreamEnded { stream_id: u64, reason: String },
}

/// The memory byte order of one [`Event::StreamFrame`] pixel blob. Four
/// bytes per pixel, tightly packed rows of `stride` bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum StreamPixelFormat {
    /// Blue, green, red, alpha in memory order; alpha is always 255.
    /// Matches PipeWire's `SPA_VIDEO_FORMAT_BGRA`/`BGRx`.
    Bgra8,
    /// Red, green, blue, alpha in memory order; alpha is always 255.
    Rgba8,
    /// Direct GPU dmabuf zero-copy export (ADR-0055). `drm_format` is the
    /// DRM FOURCC format code; `modifier` is the DRM format modifier.
    Dmabuf { drm_format: u32, modifier: u64 },
}

/// What a [`Request::StreamOutputStart`] streams (ADR-0054). `Output` is the
/// version-5 behavior: the whole focused output. `Window` crops each
/// presented frame to one window's current visible region, following its
/// position; the stream ends when the window closes or its size changes
/// (PipeWire consumers negotiate a fixed size).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum StreamTarget {
    /// The whole focused output (version-5 default).
    #[default]
    Output,
    /// One toplevel's visible region, cropped from the output frame. The id
    /// comes from a user-consent [`Request::PickTarget`] window pick.
    Window { window: WindowId },
}

impl StreamTarget {
    /// Whether this target is the whole output (the serde-skipped default).
    pub fn is_output(&self) -> bool {
        matches!(self, StreamTarget::Output)
    }
}

/// The kind of interactive pick a [`Request::PickTarget`] asks the user for
/// (ADR-0054). The compositor freezes the screen and opens the matching
/// chrome selector; the user's choice (or cancellation) is the response.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum PickKind {
    /// Drag out a screen region (the Print-key selector's interaction).
    Region,
    /// Click one screen point; the compositor reads back its colour.
    Pixel,
    /// Click a window, press Enter (or click empty desktop) for the whole
    /// output, or Escape to cancel.
    Window,
}

/// The outcome of a [`Request::PickTarget`], delivered as
/// [`Response::Picked`]. The user's click is the authorization: a pick
/// returns exactly what the user pointed at and nothing more (ADR-0054).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum PickResult {
    /// A picked region in compositor logical pixels.
    Region { rect: Rect },
    /// A picked point in compositor logical pixels and the straight-alpha
    /// RGB of the presented frame at that point.
    Pixel {
        point: aegis_model::Point,
        rgb: [u8; 3],
    },
    /// A picked toplevel.
    Window { id: WindowId },
    /// The user declined a specific window and chose the whole output (a
    /// window-mode pick answered with Enter or a click on empty desktop).
    Output,
    /// The user dismissed the selector without picking (Escape, or Enter
    /// with no staged region).
    Cancelled,
}

/// The outcome of a [`Request::PickApp`], delivered as
/// [`Response::AppPicked`]. The user's confirmation is the authorization:
/// the result is exactly the one application id the user approved.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum AppPickResult {
    /// The confirmed application's desktop file id.
    App { id: String },
    /// The user dismissed the picker without confirming.
    Cancelled,
}

/// The outcome of a [`Request::PromptSecret`], delivered as
/// [`Response::SecretPrompted`]. The value is the user's typed secret (a
/// password, PIN, …). The transport zeroizes its serialization buffers and
/// the compositor zeroizes its response copy after sending; callers must
/// zeroize the returned value immediately after use.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum SecretPromptResult {
    /// The confirmed secret value.
    Secret { value: String },
    /// The user dismissed the prompt without confirming.
    Cancelled,
}

impl std::fmt::Debug for SecretPromptResult {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Secret { .. } => formatter.write_str("SecretPromptResult::Secret([REDACTED])"),
            Self::Cancelled => formatter.write_str("SecretPromptResult::Cancelled"),
        }
    }
}

impl SecretPromptResult {
    /// Erase a returned secret in place after its downstream consumer has
    /// completed.
    pub fn zeroize(&mut self) {
        use zeroize::Zeroize as _;
        if let Self::Secret { value } = self {
            value.zeroize();
        }
    }
}

impl Drop for SecretPromptResult {
    fn drop(&mut self) {
        self.zeroize();
    }
}

/// The outcome of a [`Request::PickConfirm`], delivered as
/// [`Response::ConfirmPicked`]: the user's yes/no decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum ConfirmPickResult {
    /// The user accepted.
    Confirmed,
    /// The user declined or dismissed the dialog.
    Cancelled,
}

/// One atomic observation of an Interaction Domain's directed virtual output.
///
/// `region` and every placement use virtual-output logical coordinates.
/// `width` and `height` are the encoded PNG's physical-pixel extent after
/// applying `scale_milli`. The placement snapshot, pixels, and authority
/// `revision` are captured together on the compositor thread, so callers can
/// safely map a pixel observation back to target-local input coordinates.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InteractionDomainCapture {
    pub interaction_domain: InteractionDomainId,
    pub width: u32,
    pub height: u32,
    pub scale_milli: u32,
    pub region: Rect,
    pub placements: Vec<InteractionDomainWindowPlacement>,
    /// Semantic state captured in the same compositor transaction as the
    /// directed pixels. Its token is bound to the authenticated connection,
    /// expires quickly, and is consumed by one [`Request::ActInInteractionDomain`].
    pub observation: SemanticObservation,
    /// Byte length of the sealed PNG memfd sent immediately after this JSON
    /// response through `SCM_RIGHTS`.
    pub png_bytes: u64,
    pub revision: u64,
}

/// One atomic pixel observation of a single window (version 26).
///
/// The window's real surface tree is rendered offscreen, so the capture
/// carries true content whether the window is visible, occluded, minimized,
/// or on another workspace. `rect` is the toplevel's logical rectangle at
/// capture time; the image's origin is the toplevel's origin, so popups
/// extending past the toplevel bounds are clipped (mirroring
/// [`StreamTarget::Window`] semantics). `width` and `height` are the encoded
/// PNG's physical-pixel extent after applying `scale_milli`.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct WindowCapture {
    pub window: WindowId,
    pub width: u32,
    pub height: u32,
    pub scale_milli: u32,
    pub rect: Rect,
    /// Byte length of the sealed PNG memfd sent immediately after this JSON
    /// response through `SCM_RIGHTS`.
    pub png_bytes: u64,
}

/// Agent self-declaration carried on [`Request::Hello`] (ADR-0088). The
/// declaration is a *request*, never a grant: the compositor sanitizes the
/// requested set, the user approves first contact through the pairing
/// prompt, and the approved ceiling is enforced server-side on every
/// request.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentHello {
    /// Display label for prompts and the permission manager. Cosmetic only:
    /// it authenticates nothing and the user may rename the principal at any
    /// time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// The operation families the agent wants to borrow. The compositor
    /// filters this against the agent-requestable set before prompting.
    #[serde(default)]
    pub requested: Vec<ActorCapability>,
    /// The credential issued at an earlier pairing, if this installation
    /// holds one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

impl std::fmt::Debug for AgentHello {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentHello")
            .field("label", &self.label)
            .field("requested", &self.requested)
            .field(
                "credential",
                &self.credential.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

impl Drop for AgentHello {
    fn drop(&mut self) {
        use zeroize::Zeroize as _;
        if let Some(credential) = &mut self.credential {
            credential.zeroize();
        }
    }
}

/// The compositor's pairing reply carried on [`Response::Hello`]
/// (ADR-0088). `credential` is present only when a new principal was issued
/// during this handshake; the agent must persist it in durable owner-only
/// storage and present it on every later connection.
#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentIssued {
    /// The opaque principal id this connection is bound to.
    pub principal: String,
    /// The newly issued credential; absent when the presented credential was
    /// recognized.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential: Option<String>,
}

impl std::fmt::Debug for AgentIssued {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("AgentIssued")
            .field("principal", &self.principal)
            .field(
                "credential",
                &self.credential.as_ref().map(|_| "[REDACTED]"),
            )
            .finish()
    }
}

impl Drop for AgentIssued {
    fn drop(&mut self) {
        use zeroize::Zeroize as _;
        if let Some(credential) = &mut self.credential {
            credential.zeroize();
        }
    }
}

/// A paired agent principal as reported by [`Response::AgentPrincipals`]
/// (ADR-0088).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentPrincipalInfo {
    /// The opaque principal id.
    pub principal: String,
    /// The display label (cosmetic, user-renameable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    /// Ceiling operations usable immediately.
    pub pregranted: Vec<ActorCapability>,
    /// Ceiling operations routed through the interactive runtime grant.
    pub gated: Vec<ActorCapability>,
    /// Pairing time, unix epoch seconds.
    pub created_at: u64,
}

/// The decision recorded for one runtime grant (ADR-0088).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum AgentGrantDecision {
    /// The operation is allowed.
    Allow,
    /// The operation is refused without prompting again.
    Deny,
}

/// One recorded runtime grant as reported by [`Response::AgentGrants`]
/// (ADR-0088).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct AgentGrantInfo {
    /// The principal the grant belongs to.
    pub principal: String,
    /// The operation family the grant covers.
    pub op: ActorCapability,
    /// The recorded decision.
    pub decision: AgentGrantDecision,
    /// Decision time, unix epoch seconds.
    pub granted_at: u64,
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
        caps: ConnectionCapabilities,
        /// Optional scope name (ADR-0034). Resolved by the server from its
        /// configuration; `None` means unscoped (back-compat).
        #[serde(default)]
        scope: Option<String>,
        /// Required to retain any privileged capability. Query-only
        /// connections may omit it.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lease: Option<LeaseRequest>,
        /// Agent self-declaration for capability borrowing (ADR-0088).
        /// `None` keeps the connection anonymous within its capability
        /// classes.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<AgentHello>,
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
    GetInteractionDomains,
    /// List paired agent principals (ADR-0088). Requires `query`.
    GetAgentPrincipals,
    /// List recorded runtime grants, optionally filtered to one principal
    /// (ADR-0088). Requires `query`.
    GetAgentGrants {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        principal: Option<String>,
    },
    /// Rename a principal's display label (`None` clears it). Requires
    /// `control` and a live lease.
    RenameAgentPrincipal {
        principal: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
    },
    /// Forget a principal: its credential dies immediately and its recorded
    /// grants are dropped. Requires `control` and a live lease.
    ForgetAgentPrincipal { principal: String },
    /// Replace a principal's approved ceiling. Requires `control` and a
    /// live lease.
    SetAgentCeiling {
        principal: String,
        pregranted: Vec<ActorCapability>,
        gated: Vec<ActorCapability>,
    },
    /// Register a principal ahead of time (administrator pre-provisioning).
    /// The reply carries the issued credential to plant in the agent's
    /// identity store. Requires `control` and a live lease.
    RegisterAgent {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        label: Option<String>,
        pregranted: Vec<ActorCapability>,
        gated: Vec<ActorCapability>,
    },
    /// Drop one recorded runtime grant. Requires `control` and a live lease.
    RevokeAgentGrant {
        principal: String,
        op: ActorCapability,
    },
    /// Ask the authority broker for a short-lived, exact resource handle.
    /// A capability ceiling alone never becomes filesystem, network,
    /// secret, or payment authority without this session-bound grant.
    RequestResourceGrant {
        resource: ActorResource,
        ttl_ms: u64,
        uses: u32,
    },
    /// Consume one use of an exact resource grant. The expected resource is
    /// echoed to prevent a leaked opaque id from becoming ambient authority.
    ConsumeResourceGrant {
        id: ResourceGrantId,
        resource: ActorResource,
    },
    /// Revoke one resource grant owned by this Actor session.
    RevokeResourceGrant { id: ResourceGrantId },
    /// Fetch process-bound windows for the trusted accessibility adapter.
    /// This is intentionally separate from `GetWindows`: process credentials
    /// are never general observation data.
    GetAccessibilityWindows,
    /// Publish a complete accessibility-tree revision. Requires a paired
    /// semantic-provider principal with `PublishAccessibilityTree`.
    PublishAccessibilityTree { update: AccessibilityTreeUpdate },
    /// Long-poll the next compositor-validated semantic action for this
    /// provider. D-Bus execution stays in the adapter process.
    NextAccessibilityAction { timeout_ms: u64 },
    /// Complete a previously delivered semantic action.
    CompleteAccessibilityAction {
        request_id: u64,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// Fetch the compositor-owned persistent settings snapshot.
    GetSettings,
    /// Fetch live host and compositor-owned session status.
    GetSystemStatus,
    /// Persist and apply one settings edit on the compositor main loop. The
    /// response confirms completion and carries the new revision.
    Settings {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        expected_revision: Option<u64>,
        action: SettingsAction,
    },
    /// Commit an Interaction Domain lifecycle operation and return its receipt.
    InteractionDomain { action: InteractionDomainAction },
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
    /// [`ActorCapability::CaptureOutput`] entry in a named scope's `ops` (never
    /// inherited), and is refused while the session is locked.
    /// When `region` is present, only that logical-pixel rectangle is captured.
    CaptureOutput {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<Rect>,
    },
    /// Capture one Interaction Domain's directed virtual output. Coordinates are Interaction Domain
    /// logical pixels and never include compositor chrome or another Interaction Domain.
    CaptureInteractionDomain {
        interaction_domain: InteractionDomainId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        region: Option<Rect>,
    },
    /// Capture one window's real content as a PNG (version 26). The window
    /// is rendered offscreen, so foreground, occluded, minimized, and
    /// foreign-workspace windows all capture; popups past the toplevel
    /// bounds are clipped. Authorization mirrors [`Request::CaptureOutput`]:
    /// `control`, a live lease, and an explicit
    /// [`ActorCapability::CaptureWindow`] scope decision for this window —
    /// never inherited — and it is refused while the session is locked.
    CaptureWindow { window: WindowId },
    /// Read semantic objects for one Interaction Domain without receiving framebuffer
    /// pixels. The returned observation is a short-lived precondition lease,
    /// not action authority.
    ObserveInteractionDomain {
        interaction_domain: InteractionDomainId,
    },
    /// Commit one observation-bound input intent. Unlike [`Request::Do`],
    /// this waits for main-loop validation and returns an authoritative
    /// receipt or a refusal; the observation token is single-use.
    ActInInteractionDomain { intent: ActorActionIntent },
    /// Start a continuous frame stream of the focused output (ADR-0052).
    /// Authorization mirrors [`Request::CaptureOutput`]: `control`, a live
    /// lease, and an explicit [`ActorCapability::StreamOutput`] entry in the
    /// connection's named scope — never inherited. Frames arrive as
    /// [`Event::StreamFrame`] until [`Request::StreamOutputStop`],
    /// disconnect, or [`Event::StreamEnded`]. `max_fps` throttles delivery;
    /// `None` means the server default (30 fps, clamped to 1–60).
    /// `target` (version 6) selects the whole output or one window's
    /// visible region (ADR-0054); a window id the compositor does not know
    /// is refused.
    StreamOutputStart {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        max_fps: Option<u32>,
        #[serde(default, skip_serializing_if = "StreamTarget::is_output")]
        target: StreamTarget,
        /// Opt in to a zero-copy dmabuf stream (version 25): the reply may
        /// announce [`StreamPixelFormat::Dmabuf`] and carry the slot table.
        /// Absent or `Some(false)` keeps the sealed-memfd SHM stream. A
        /// client that did not opt in never sees a dmabuf announcement, so
        /// its framing stays synchronized.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dmabuf: Option<bool>,
    },
    /// Stop a stream owned by this connection.
    StreamOutputStop { stream_id: u64 },
    /// Release a dmabuf stream slot (version 25) after the consumer finished
    /// reading it. The compositor must not reuse a slot between delivering
    /// its frame and receiving this release.
    StreamBufferRelease { stream_id: u64, slot: u32 },
    /// Set or clear this connection's global idle inhibitor (the Inhibit
    /// portal, ADR-0075). Authorization mirrors
    /// [`Request::StreamOutputStart`]: `control`, a live lease, and an
    /// explicit [`ActorCapability::IdleInhibit`] entry in the connection's named
    /// scope — never inherited. The inhibitor is surfaceless: while any
    /// connection holds one, idle notifications stay resumed. The server
    /// releases the inhibitor automatically when the owning connection
    /// disconnects.
    SetIdleInhibit { inhibit: bool },
    /// Ask the user to interactively pick a screen target through
    /// compositor chrome (ADR-0054). The screen freezes, the matching
    /// selector opens, and the connection blocks until the user confirms or
    /// cancels (or the compositor's interaction timeout elapses). The user's
    /// choice is the authorization, but the request is still fail-closed:
    /// `control`, a live lease, and an explicit [`ActorCapability::PickTarget`]
    /// entry in the connection's named scope — never inherited — and it is
    /// refused while the session is locked. One pick at a time compositor-
    /// wide; a concurrent request is refused.
    PickTarget { kind: PickKind },
    /// Ask the user to choose one application out of `choices` (desktop file
    /// ids) through compositor chrome (the AppChooser portal's compositor
    /// side). `subject` is a human-readable context line ("the file or
    /// content the app is chosen for"), `last_choice` pre-highlights the
    /// previously used app. It uses fail-closed authorization and permits
    /// only one compositor prompt at a time; no screen capture is involved.
    PickApp {
        choices: Vec<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subject: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        last_choice: Option<String>,
    },
    /// Ask the user for a secret (password, PIN, …) through a masked
    /// compositor prompt (the secret vault's password unlock). `title` and
    /// `reason` label the prompt; the typed value returns as
    /// [`SecretPromptResult::Secret`]. It uses fail-closed authorization and
    /// permits only one compositor prompt at a time; no capture is involved.
    PromptSecret {
        resource_grant: ResourceGrantId,
        title: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
    /// Ask the user a yes/no consent question through compositor chrome
    /// (portal consent dialogs: Account, Access, DynamicLauncher). `title`
    /// heads the dialog, `body` explains the request, `accept_label`
    /// overrides the affirmative button's label. It uses fail-closed
    /// authorization and permits only one compositor prompt at a time.
    PickConfirm {
        title: String,
        body: String,
        accept_label: Option<String>,
    },
    /// Replace the desktop wallpaper with the image at `path` (the
    /// Wallpaper portal). Decodes on the compositor main loop and swaps
    /// live; the reply is an authoritative receipt, not a queue
    /// acknowledgment. Fail-closed like the picks: `control`, a live lease,
    /// and an explicit [`ActorCapability::SetWallpaper`] entry in the connection's
    /// named scope — never inherited.
    SetWallpaper { path: PathBuf },
}

/// A server → client message.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum Response {
    /// Handshake reply. Carries the negotiated version and the capabilities
    /// the server actually granted.
    Hello {
        version: u32,
        caps: ConnectionCapabilities,
        /// The scope the server actually granted (ADR-0034). Unscoped if the
        /// client did not request a scope name.
        #[serde(default = "Scope::unscoped")]
        scope: Scope,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        lease: Option<LeaseGrant>,
        /// Explicit bounded Actor execution context. Unlike durable pairing,
        /// this session expires and is revoked on disconnect.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        session: Option<aegis_security::authority::ActorSessionSnapshot>,
        /// The pairing outcome when the client presented `Hello.agent`
        /// (ADR-0088). Carries a new credential only when one was issued.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        agent: Option<AgentIssued>,
    },
    /// Reply to [`Request::GetWindows`].
    Windows {
        windows: Vec<Window>,
    },
    /// Reply to [`Request::GetAccessibilityWindows`].
    AccessibilityWindows {
        windows: Vec<AccessibilityWindowBinding>,
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
    InteractionDomains {
        snapshot: InteractionDomainSnapshot,
    },
    /// Reply to [`Request::GetAgentPrincipals`].
    AgentPrincipals {
        principals: Vec<AgentPrincipalInfo>,
    },
    /// Reply to [`Request::GetAgentGrants`].
    AgentGrants {
        grants: Vec<AgentGrantInfo>,
    },
    /// Reply to [`Request::RegisterAgent`]: the issued credential to plant
    /// in the agent's identity store.
    AgentRegistered {
        principal: String,
        credential: String,
    },
    ResourceGranted {
        grant: ResourceGrant,
    },
    ResourceGrantConsumed {
        grant: ResourceGrant,
    },
    ResourceGrantRevoked {},
    AccessibilityTreePublished {},
    AccessibilityAction {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        request: Option<SemanticActionRequest>,
    },
    AccessibilityActionCompleted {},
    Settings {
        snapshot: SettingsSnapshot,
    },
    /// Reply to [`Request::GetSystemStatus`].
    SystemStatus {
        snapshot: SystemStatus,
    },
    SettingsApplied {
        receipt: SettingsReceipt,
    },
    InteractionDomain {
        result: InteractionDomainActionResult,
    },
    /// Reply to [`Request::CaptureOutput`]: the output's physical size and
    /// length of the sealed PNG memfd that immediately follows it.
    CaptureOutput {
        width: u32,
        height: u32,
        /// Byte length of the sealed PNG memfd that follows this metadata.
        png_bytes: u64,
    },
    CaptureInteractionDomain {
        capture: InteractionDomainCapture,
    },
    /// Reply to [`Request::CaptureWindow`] (version 26): the captured
    /// window's geometry metadata; the sealed PNG memfd follows immediately.
    CaptureWindow {
        capture: WindowCapture,
    },
    /// Reply to [`Request::ObserveInteractionDomain`].
    InteractionDomainObserved {
        observation: SemanticObservation,
    },
    /// Reply to [`Request::ActInInteractionDomain`].
    ActorActionCommitted {
        receipt: ActorActionReceipt,
    },
    /// Reply to [`Request::StreamOutputStart`]: the stream id and the
    /// output's physical size and pixel format at start time. When `format`
    /// is [`StreamPixelFormat::Dmabuf`] (version 25), the reply is followed
    /// immediately by `slots` fixed-size dmabuf descriptors on the blob
    /// channel (one `0xfd`-marked `SCM_RIGHTS` message per slot, each
    /// `slot_bytes` long with rows of `slot_stride` bytes); frames then
    /// reference slots by index and carry no descriptor.
    StreamOutputStarted {
        stream_id: u64,
        width: u32,
        height: u32,
        format: StreamPixelFormat,
        /// dmabuf slot count (version 25); absent on SHM streams.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slots: Option<u32>,
        /// dmabuf slot row stride in bytes (version 25).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot_stride: Option<u32>,
        /// dmabuf slot byte length (version 25).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        slot_bytes: Option<u64>,
    },
    /// Reply to [`Request::StreamOutputStop`].
    StreamOutputStopped {
        stream_id: u64,
    },
    /// Reply to [`Request::StreamBufferRelease`] (version 25).
    StreamBufferReleased {
        stream_id: u64,
        slot: u32,
    },
    /// Reply to [`Request::SetIdleInhibit`]: the inhibitor state this
    /// connection now holds.
    IdleInhibitSet {
        inhibited: bool,
    },
    /// Reply to [`Request::PickTarget`] (ADR-0054): what the user picked,
    /// or [`PickResult::Cancelled`].
    Picked {
        result: PickResult,
    },
    /// Reply to [`Request::PickApp`]: the application id the user confirmed,
    /// or [`AppPickResult::Cancelled`].
    AppPicked {
        result: AppPickResult,
    },
    /// Reply to [`Request::PromptSecret`]: the secret the user confirmed,
    /// or [`SecretPromptResult::Cancelled`].
    SecretPrompted {
        result: SecretPromptResult,
    },
    /// Reply to [`Request::PickConfirm`]: the user's yes/no decision.
    ConfirmPicked {
        result: ConfirmPickResult,
    },
    /// Reply to [`Request::SetWallpaper`]: the wallpaper was decoded and
    /// swapped (an authoritative main-loop receipt).
    WallpaperSet {},
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

impl std::fmt::Debug for Response {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let variant = match self {
            Self::Hello { .. } => "Hello",
            Self::Windows { .. } => "Windows",
            Self::AccessibilityWindows { .. } => "AccessibilityWindows",
            Self::Workspaces { .. } => "Workspaces",
            Self::Notifications { .. } => "Notifications",
            Self::Outputs { .. } => "Outputs",
            Self::Journal { .. } => "Journal",
            Self::InteractionDomains { .. } => "InteractionDomains",
            Self::AgentPrincipals { .. } => "AgentPrincipals",
            Self::AgentGrants { .. } => "AgentGrants",
            Self::AgentRegistered { .. } => "AgentRegistered",
            Self::ResourceGranted { .. } => "ResourceGranted",
            Self::ResourceGrantConsumed { .. } => "ResourceGrantConsumed",
            Self::ResourceGrantRevoked { .. } => "ResourceGrantRevoked",
            Self::AccessibilityTreePublished { .. } => "AccessibilityTreePublished",
            Self::AccessibilityAction { .. } => "AccessibilityAction",
            Self::AccessibilityActionCompleted { .. } => "AccessibilityActionCompleted",
            Self::Settings { .. } => "Settings",
            Self::SystemStatus { .. } => "SystemStatus",
            Self::SettingsApplied { .. } => "SettingsApplied",
            Self::InteractionDomain { .. } => "InteractionDomain",
            Self::CaptureOutput { .. } => "CaptureOutput",
            Self::CaptureInteractionDomain { .. } => "CaptureInteractionDomain",
            Self::CaptureWindow { .. } => "CaptureWindow",
            Self::InteractionDomainObserved { .. } => "InteractionDomainObserved",
            Self::ActorActionCommitted { .. } => "ActorActionCommitted",
            Self::StreamOutputStarted { .. } => "StreamOutputStarted",
            Self::StreamBufferReleased { .. } => "StreamBufferReleased",
            Self::StreamOutputStopped { .. } => "StreamOutputStopped",
            Self::IdleInhibitSet { .. } => "IdleInhibitSet",
            Self::Picked { .. } => "Picked",
            Self::AppPicked { .. } => "AppPicked",
            Self::SecretPrompted { .. } => "SecretPrompted",
            Self::ConfirmPicked { .. } => "ConfirmPicked",
            Self::WallpaperSet { .. } => "WallpaperSet",
            Self::LeaseRenewed { .. } => "LeaseRenewed",
            Self::Ok => "Ok",
            Self::Subscribed => "Subscribed",
            Self::Error { .. } => "Error",
        };
        formatter.write_str(variant)
    }
}

impl Response {
    /// Erase credentials and secrets retained by the sending side after the
    /// framed response has been serialized. Public clients still own any
    /// credential or secret they deliberately extract from a received
    /// response and must store or consume it under their own secret policy.
    pub fn zeroize_sensitive(&mut self) {
        use zeroize::Zeroize as _;
        match self {
            Self::Hello {
                agent:
                    Some(AgentIssued {
                        credential: Some(credential),
                        ..
                    }),
                ..
            }
            | Self::AgentRegistered { credential, .. } => credential.zeroize(),
            Self::SecretPrompted { result } => result.zeroize(),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;
