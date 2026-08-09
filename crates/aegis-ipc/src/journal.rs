//! The mutation journal (ADR-0033).
//!
//! An in-memory, append-only ring buffer of [`JournalEntry`] records, one per
//! command or Interaction Domain authority action the compositor decides, regardless of
//! origin (chrome, keybinding, IPC, or internal cleanup). The journal records
//! the compositor's *decisions* — what it did, by whom, and with what outcome
//! — so the agent can reconstruct recent history without polling.
//!
//! The ring is bounded; oldest entries are evicted when full. `seq` is
//! monotonic across evictions, so a subscriber that falls behind detects the
//! gap and re-queries rather than reasoning over a partial history.
//!
//! See [ADR-0033](../../docs/adr/0033-mutation-journal.md).

use crate::schema::{ActorCapability, Command, InteractionDomainAction, Scope, SettingsAction};
use aegis_model::interaction_domain::InteractionDomainId;
use aegis_model::semantic::{SemanticActionIntent, SemanticObjectId};

/// Privacy-preserving semantic action shape retained in the durable audit.
/// User-entered text and values never enter the event store.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuditedSemanticAction {
    Invoke,
    Focus,
    SetValue {
        utf8_bytes: u32,
    },
    TypeText {
        utf8_bytes: u32,
    },
    Select {
        selected: bool,
    },
    Expand,
    Collapse,
    SyntheticInput {
        pointer_moves: u32,
        clicks: u32,
        scrolls: u32,
        key_presses: u32,
    },
}

/// Privacy-minimized command projection retained in the durable audit.
/// Resource ids needed for scope filtering remain visible, while free-form
/// text, filesystem paths, input coordinates, and key/button codes do not.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum AuditedCommand {
    Focus {
        id: aegis_model::window::WindowId,
    },
    Minimize {
        id: aegis_model::window::WindowId,
    },
    SetMaximized {
        id: aegis_model::window::WindowId,
        maximized: bool,
    },
    SetAlwaysOnTop {
        id: aegis_model::window::WindowId,
        on_top: bool,
    },
    Close {
        id: aegis_model::window::WindowId,
    },
    Move {
        id: aegis_model::window::WindowId,
    },
    SetWindowGeometry {
        id: aegis_model::window::WindowId,
    },
    InjectInput {
        id: aegis_model::window::WindowId,
        pointer_moves: u32,
        clicks: u32,
        scrolls: u32,
        key_presses: u32,
    },
    LaunchInInteractionDomain {
        interaction_domain: InteractionDomainId,
        desktop_id: String,
    },
    LaunchApp {
        desktop_id: String,
        /// The explicit target workspace, when placement named one (`FreshWorkspace` records `None`).
        workspace: Option<aegis_model::workspace::WorkspaceId>,
    },
    Cycle {
        forward: bool,
    },
    SwitchWorkspace {
        dir: aegis_model::workspace::Switch,
    },
    SwitchWorkspaceTo {
        id: aegis_model::workspace::WorkspaceId,
    },
    MoveToWorkspace {
        window: aegis_model::window::WindowId,
        workspace: aegis_model::workspace::WorkspaceId,
    },
    ToggleTiling,
    System {
        action: crate::schema::SystemAction,
    },
    Notify {
        summary_utf8_bytes: u32,
        body_utf8_bytes: u32,
        app_id: Option<String>,
    },
    DismissNotification {
        id: u64,
    },
    Screenshot {
        region: bool,
    },
    ToggleOverview,
    Quit,
}

impl AuditedCommand {
    pub fn permitted_by(&self, scope: &Scope) -> bool {
        match self {
            Self::Focus { id }
            | Self::Minimize { id }
            | Self::SetMaximized { id, .. }
            | Self::SetAlwaysOnTop { id, .. }
            | Self::Close { id }
            | Self::Move { id }
            | Self::SetWindowGeometry { id }
            | Self::InjectInput { id, .. } => scope.permits_window(*id),
            Self::LaunchInInteractionDomain {
                interaction_domain, ..
            } => scope.permits_interaction_domain(*interaction_domain),
            Self::LaunchApp { workspace, .. } => workspace
                .map(|id| scope.permits_workspace(id))
                .unwrap_or(true),
            Self::SwitchWorkspaceTo { id } => scope.permits_workspace(*id),
            Self::MoveToWorkspace { window, workspace } => {
                scope.permits_window(*window) && scope.permits_workspace(*workspace)
            }
            Self::Cycle { .. }
            | Self::SwitchWorkspace { .. }
            | Self::ToggleTiling
            | Self::System { .. }
            | Self::Notify { .. }
            | Self::DismissNotification { .. }
            | Self::Screenshot { .. }
            | Self::ToggleOverview
            | Self::Quit => true,
        }
    }
}

impl From<&Command> for AuditedCommand {
    fn from(command: &Command) -> Self {
        match command {
            Command::Focus { id, .. } => Self::Focus { id: *id },
            Command::Minimize { id } => Self::Minimize { id: *id },
            Command::SetMaximized { id, maximized } => Self::SetMaximized {
                id: *id,
                maximized: *maximized,
            },
            Command::SetAlwaysOnTop { id, on_top } => Self::SetAlwaysOnTop {
                id: *id,
                on_top: *on_top,
            },
            Command::Close { id } => Self::Close { id: *id },
            Command::Move { id } => Self::Move { id: *id },
            Command::SetWindowGeometry { id, .. } => Self::SetWindowGeometry { id: *id },
            Command::InjectInput { id, actions } => {
                let (mut pointer_moves, mut clicks, mut scrolls, mut key_presses) = (0, 0, 0, 0);
                for action in actions {
                    match action {
                        aegis_model::input::SyntheticInputAction::PointerMove { .. } => {
                            pointer_moves += 1;
                        }
                        aegis_model::input::SyntheticInputAction::Click { .. } => clicks += 1,
                        aegis_model::input::SyntheticInputAction::Scroll { .. } => scrolls += 1,
                        aegis_model::input::SyntheticInputAction::KeyPress { .. } => {
                            key_presses += 1
                        }
                    }
                }
                Self::InjectInput {
                    id: *id,
                    pointer_moves,
                    clicks,
                    scrolls,
                    key_presses,
                }
            }
            Command::LaunchInInteractionDomain {
                interaction_domain,
                desktop_id,
            } => Self::LaunchInInteractionDomain {
                interaction_domain: *interaction_domain,
                desktop_id: desktop_id.clone(),
            },
            Command::LaunchApp {
                desktop_id,
                placement,
            } => Self::LaunchApp {
                desktop_id: desktop_id.clone(),
                workspace: match placement {
                    Some(aegis_model::workspace::LaunchPlacement::Workspace { id }) => Some(*id),
                    _ => None,
                },
            },
            Command::Cycle { forward } => Self::Cycle { forward: *forward },
            Command::SwitchWorkspace { dir } => Self::SwitchWorkspace { dir: *dir },
            Command::SwitchWorkspaceTo { id } => Self::SwitchWorkspaceTo { id: *id },
            Command::MoveToWorkspace { window, workspace } => Self::MoveToWorkspace {
                window: *window,
                workspace: *workspace,
            },
            Command::ToggleTiling => Self::ToggleTiling,
            Command::System { action } => Self::System {
                action: action.clone(),
            },
            Command::Notify {
                summary,
                body,
                app_id,
                ..
            } => Self::Notify {
                summary_utf8_bytes: summary.len().min(u32::MAX as usize) as u32,
                body_utf8_bytes: body.len().min(u32::MAX as usize) as u32,
                app_id: app_id.clone(),
            },
            Command::DismissNotification { id } => Self::DismissNotification { id: *id },
            Command::Screenshot { region, .. } => Self::Screenshot {
                region: region.is_some(),
            },
            Command::ToggleOverview => Self::ToggleOverview,
            Command::Quit => Self::Quit,
        }
    }
}

impl From<&SemanticActionIntent> for AuditedSemanticAction {
    fn from(action: &SemanticActionIntent) -> Self {
        match action {
            SemanticActionIntent::Invoke => Self::Invoke,
            SemanticActionIntent::Focus => Self::Focus,
            SemanticActionIntent::SetValue { value } => Self::SetValue {
                utf8_bytes: value.len().min(u32::MAX as usize) as u32,
            },
            SemanticActionIntent::TypeText { text } => Self::TypeText {
                utf8_bytes: text.len().min(u32::MAX as usize) as u32,
            },
            SemanticActionIntent::Select { selected } => Self::Select {
                selected: *selected,
            },
            SemanticActionIntent::Expand => Self::Expand,
            SemanticActionIntent::Collapse => Self::Collapse,
            SemanticActionIntent::SyntheticInput { actions } => {
                let mut pointer_moves = 0u32;
                let mut clicks = 0u32;
                let mut scrolls = 0u32;
                let mut key_presses = 0u32;
                for action in actions {
                    match action {
                        aegis_model::input::SyntheticInputAction::PointerMove { .. } => {
                            pointer_moves = pointer_moves.saturating_add(1);
                        }
                        aegis_model::input::SyntheticInputAction::Click { .. } => {
                            clicks = clicks.saturating_add(1);
                        }
                        aegis_model::input::SyntheticInputAction::Scroll { .. } => {
                            scrolls = scrolls.saturating_add(1);
                        }
                        aegis_model::input::SyntheticInputAction::KeyPress { .. } => {
                            key_presses = key_presses.saturating_add(1);
                        }
                    }
                }
                Self::SyntheticInput {
                    pointer_moves,
                    clicks,
                    scrolls,
                    key_presses,
                }
            }
        }
    }
}

pub fn audit_semantic_actions(actions: &[SemanticActionIntent]) -> Vec<AuditedSemanticAction> {
    actions.iter().map(AuditedSemanticAction::from).collect()
}

/// Who caused a mutation. The agent filters its own echoes and models user
/// intent from the origin.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum Origin {
    /// A chrome component (dock, decorations, launcher, workspace bar).
    Chrome,
    /// A keybinding match.
    Keybinding,
    /// A compositor-owned touchpad gesture (e.g., the three-finger
    /// navigation swipe).
    Gesture,
    /// An IPC `Do` request from connection `conn_id`.
    Ipc { conn_id: u64 },
    /// An authenticated Actor using the IPC seam. `principal` is the opaque
    /// compositor-issued identity, not a self-asserted display label.
    Actor { conn_id: u64, principal: String },
    /// Internal compositor cleanup (e.g., closing a window whose client
    /// vanished).
    Internal,
}

impl Origin {
    /// Construct an IPC origin without losing authenticated Actor identity.
    pub fn ipc(conn_id: u64, principal: Option<&str>) -> Self {
        match principal {
            Some(principal) => Self::Actor {
                conn_id,
                principal: principal.to_owned(),
            },
            None => Self::Ipc { conn_id },
        }
    }

    pub fn conn_id(&self) -> Option<u64> {
        match self {
            Self::Ipc { conn_id } | Self::Actor { conn_id, .. } => Some(*conn_id),
            _ => None,
        }
    }
}

/// What happened when the compositor applied a command.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum Effect {
    /// The command was applied to the model.
    Applied,
    /// The command was refused (scope violation, target gone, etc.).
    Refused { reason: String },
    /// The command was a no-op (nothing changed).
    NoOp,
}

/// The exact mutation the compositor decided.
///
/// Interaction Domain actions carry both authority revisions so an observer can correlate
/// a transfer, observer change, lifecycle transition, or output reconfigure
/// with snapshots and captured pixels without racing a later action.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum JournalMutation {
    Command {
        cmd: AuditedCommand,
    },
    InteractionDomain {
        action: InteractionDomainAction,
        before_revision: u64,
        after_revision: u64,
    },
    Settings {
        action: SettingsAction,
        before_revision: u64,
        after_revision: u64,
    },
    /// One observation-bound Actor action decision. The bearer observation
    /// token is deliberately excluded from the journal. `action_id` and
    /// `authority_revision` are present only after the main loop validated
    /// the preconditions and committed the complete batch.
    ActorAction {
        action_id: Option<u64>,
        interaction_domain: InteractionDomainId,
        target: SemanticObjectId,
        window: Option<aegis_model::window::WindowId>,
        actions: Vec<AuditedSemanticAction>,
        /// True when an invalid oversized request was bounded to the first
        /// 64 actions for audit retention.
        actions_truncated: bool,
        authority_revision: Option<u64>,
    },
    /// Agent authorization lifecycle (ADR-0088): pairing, runtime grants,
    /// and principal management.
    AgentAuth {
        principal: String,
        action: AgentAuthAction,
    },
    ActorSession {
        session: aegis_security::authority::ActorSessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        principal: Option<aegis_security::authority::ActorPrincipal>,
        action: ActorSessionAuditAction,
    },
    ResourceGrant {
        session: aegis_security::authority::ActorSessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        principal: Option<aegis_security::authority::ActorPrincipal>,
        capability: ActorCapability,
        resource_kind: ResourceKind,
        action: ResourceGrantAuditAction,
    },
    /// A refused resource-handle operation. Bearer identifiers and exact
    /// resource values are deliberately excluded: the event proves which
    /// capability family was attempted without retaining paths, origins,
    /// secret purposes, or payment details.
    ResourceGrantAttempt {
        session: aegis_security::authority::ActorSessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        principal: Option<aegis_security::authority::ActorPrincipal>,
        action: ResourceGrantAttemptAction,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capability: Option<ActorCapability>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resource_kind: Option<ResourceKind>,
    },
    /// Privacy-minimized decision for an explicit capability endpoint that
    /// is not already represented by a richer command/action mutation.
    CapabilityUse {
        session: aegis_security::authority::ActorSessionId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        principal: Option<aegis_security::authority::ActorPrincipal>,
        capability: ActorCapability,
        action: CapabilityUseAction,
    },
}

impl JournalMutation {
    /// Replace an arbitrary downstream refusal string with a stable,
    /// payload-free category before durable persistence. Runtime errors can
    /// contain paths, titles, toolkit text, or other values deliberately
    /// omitted from the audited mutation shape.
    pub fn privacy_minimize_effect(&self, mut effect: Effect) -> Effect {
        let Effect::Refused { reason } = &mut effect else {
            return effect;
        };
        use zeroize::Zeroize as _;
        reason.zeroize();
        *reason = match self {
            Self::Command { .. } => "command refused",
            Self::InteractionDomain { .. } => "Interaction Domain mutation refused",
            Self::Settings { .. } => "settings mutation refused",
            Self::ActorAction { .. } => "Actor action refused",
            Self::AgentAuth { .. } => "Agent authorization refused",
            Self::ActorSession { .. } => "Actor session transition refused",
            Self::ResourceGrant { .. } | Self::ResourceGrantAttempt { .. } => {
                "resource grant operation refused"
            }
            Self::CapabilityUse { .. } => "capability use refused",
        }
        .into();
        effect
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityUseAction {
    Observe,
    Publish,
    Await,
    Complete,
    Capture,
    Start,
    Stop,
    Enable,
    Disable,
    Pick,
    Prompt,
    Apply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActorSessionAuditAction {
    Started,
    Disconnected,
    PrincipalRevoked,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceGrantAuditAction {
    Issued,
    Consumed,
    Revoked,
    Expired,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceGrantAttemptAction {
    Issue,
    Consume,
    Revoke,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    FilesystemPath,
    NetworkOrigin,
    SecretPrompt,
    PaymentRequest,
}

impl From<&aegis_security::authority::ActorResource> for ResourceKind {
    fn from(resource: &aegis_security::authority::ActorResource) -> Self {
        match resource {
            aegis_security::authority::ActorResource::FilesystemPath { .. } => Self::FilesystemPath,
            aegis_security::authority::ActorResource::NetworkOrigin { .. } => Self::NetworkOrigin,
            aegis_security::authority::ActorResource::SecretPrompt { .. } => Self::SecretPrompt,
            aegis_security::authority::ActorResource::PaymentRequest { .. } => Self::PaymentRequest,
        }
    }
}

/// One agent-authorization lifecycle event (ADR-0088), carried by
/// [`JournalMutation::AgentAuth`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum AgentAuthAction {
    /// A principal was issued (interactive pairing or pre-provisioning).
    Paired,
    /// A runtime grant was answered interactively; `persistence` records
    /// how long the decision lives.
    Granted {
        op: ActorCapability,
        persistence: GrantPersistence,
    },
    /// A recorded runtime grant was revoked by the user.
    GrantRevoked { op: ActorCapability },
    /// A principal was forgotten; its credential and grants died with it.
    Forgotten,
    /// A principal's display label was changed.
    Renamed,
    /// A principal's approved capability ceiling was replaced.
    CeilingChanged,
}

/// How long an interactively answered runtime grant lives (ADR-0088).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub enum GrantPersistence {
    /// Allowed for this one operation only; nothing was recorded.
    Once,
    /// Allowed until the compositor exits; recorded in memory only.
    Session,
    /// Allowed durably; recorded on disk.
    Always,
    /// Refused until the compositor exits; recorded in memory only.
    DeniedSession,
}

/// One ordered mutation event. Storage and bounded projection mechanics live
/// in `aegis-security::audit`; IPC owns only this wire-visible event vocabulary.
pub type JournalEntry = aegis_security::audit::AuditEntry<Origin, JournalMutation, Effect>;

/// A bounded live projection with explicit sequence bounds.
pub type JournalSnapshot = aegis_security::audit::AuditSnapshot<JournalEntry>;

/// In-memory projection of the durable event stream.
pub type Journal = aegis_security::audit::AuditLog<JournalEntry>;

pub use aegis_security::audit::DEFAULT_CAPACITY;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::Command;

    fn cmd(n: u64) -> Command {
        Command::Focus {
            id: aegis_model::window::WindowId(n),
            reveal: true,
        }
    }

    fn command(n: u64) -> JournalMutation {
        let command = cmd(n);
        JournalMutation::Command {
            cmd: AuditedCommand::from(&command),
        }
    }

    #[test]
    fn append_assigns_monotonic_seq() {
        let mut j = Journal::new(8);
        let e1 = j.append(0, Origin::Chrome, command(1), Effect::Applied);
        let s1 = e1.seq;
        let e2 = j.append(1, Origin::Keybinding, command(2), Effect::Applied);
        assert!(e2.seq > s1);
        assert_eq!(j.latest_seq(), e2.seq);
    }

    #[test]
    fn ring_evicts_oldest_when_full() {
        let mut j = Journal::new(3);
        j.append(0, Origin::Chrome, command(1), Effect::Applied);
        j.append(1, Origin::Chrome, command(2), Effect::Applied);
        j.append(2, Origin::Chrome, command(3), Effect::Applied);
        assert_eq!(j.len(), 3);
        // Fourth append evicts the first.
        j.append(3, Origin::Chrome, command(4), Effect::Applied);
        assert_eq!(j.len(), 3);
        let snap = j.since(0);
        assert_eq!(snap.entries.len(), 3);
        // The first entry (seq=1) was evicted; oldest is now seq=2.
        assert_eq!(snap.oldest_seq, 2);
    }

    #[test]
    fn since_filters_by_seq() {
        let mut j = Journal::new(8);
        j.append(0, Origin::Chrome, command(1), Effect::Applied);
        let mid = j.latest_seq();
        j.append(1, Origin::Ipc { conn_id: 7 }, command(2), Effect::Applied);
        j.append(2, Origin::Keybinding, command(3), Effect::Applied);
        let snap = j.since(mid);
        assert_eq!(snap.entries.len(), 2);
        assert!(snap.entries.iter().all(|e| e.seq > mid));
    }

    #[test]
    fn gap_detection_via_oldest_seq() {
        let mut j = Journal::new(2);
        j.append(0, Origin::Chrome, command(1), Effect::Applied);
        j.append(1, Origin::Chrome, command(2), Effect::Applied);
        j.append(2, Origin::Chrome, command(3), Effect::Applied);
        // Client asks for everything since seq=0, but seq=1 was evicted.
        let snap = j.since(0);
        assert_eq!(snap.oldest_seq, 2, "oldest in ring is seq=2");
        assert!(snap.entries.iter().all(|e| e.seq >= 2));
    }

    #[test]
    fn empty_journal_since_returns_empty_with_bounds() {
        let j = Journal::new(8);
        let snap = j.since(0);
        assert!(snap.entries.is_empty());
        assert_eq!(snap.latest_seq, 0);
    }

    #[test]
    fn entry_round_trips_through_serde() {
        let entry = JournalEntry {
            seq: 42,
            ts_mono_ms: 12345,
            origin: Origin::Ipc { conn_id: 7 },
            mutation: command(99),
            effect: Effect::Refused {
                reason: "out of scope".into(),
            },
        };
        let json = serde_json::to_string(&entry).unwrap();
        let back: JournalEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(entry, back);
    }

    #[test]
    fn interaction_domain_action_round_trips_with_authority_revisions() {
        let mutation = JournalMutation::InteractionDomain {
            action: InteractionDomainAction::Create {
                label: "research".into(),
                capabilities: aegis_model::interaction_domain::SeatCapabilities::POINTER_KEYBOARD,
                output: Some(aegis_model::interaction_domain::VirtualOutput::DEFAULT_AGENT),
            },
            before_revision: 7,
            after_revision: 8,
        };
        let mut journal = Journal::new(4);
        let entry = journal.append(
            5,
            Origin::Ipc { conn_id: 9 },
            mutation.clone(),
            Effect::Applied,
        );
        assert_eq!(entry.mutation, mutation);
        let encoded = serde_json::to_string(&entry).unwrap();
        let decoded: JournalEntry = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, entry);
    }

    #[test]
    fn origin_tags_distinguish_sources() {
        let mut j = Journal::new(8);
        j.append(0, Origin::Chrome, command(1), Effect::Applied);
        j.append(1, Origin::Keybinding, command(2), Effect::Applied);
        j.append(2, Origin::Ipc { conn_id: 3 }, command(3), Effect::Applied);
        j.append(3, Origin::Internal, command(4), Effect::Applied);
        let snap = j.since(0);
        assert_eq!(snap.entries.len(), 4);
        assert!(matches!(snap.entries[0].origin, Origin::Chrome));
        assert!(matches!(snap.entries[1].origin, Origin::Keybinding));
        assert!(matches!(snap.entries[2].origin, Origin::Ipc { conn_id: 3 }));
        assert!(matches!(snap.entries[3].origin, Origin::Internal));
    }

    #[test]
    fn effect_variants_round_trip() {
        let effects = [
            Effect::Applied,
            Effect::Refused {
                reason: "test".into(),
            },
            Effect::NoOp,
        ];
        for e in &effects {
            let json = serde_json::to_string(e).unwrap();
            let back: Effect = serde_json::from_str(&json).unwrap();
            assert_eq!(&back, e);
        }
    }

    #[test]
    fn agent_auth_mutation_round_trips() {
        let mutation = JournalMutation::AgentAuth {
            principal: "prin_ab12".into(),
            action: AgentAuthAction::Granted {
                op: ActorCapability::Close,
                persistence: GrantPersistence::Always,
            },
        };
        let mut journal = Journal::new(4);
        let entry = journal.append(
            3,
            Origin::Ipc { conn_id: 5 },
            mutation.clone(),
            Effect::Applied,
        );
        assert_eq!(entry.mutation, mutation);
        let encoded = serde_json::to_string(&entry).unwrap();
        let decoded: JournalEntry = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, entry);
    }

    #[test]
    fn resource_grant_refusal_shape_excludes_bearers_and_exact_resources() {
        let mutation = JournalMutation::ResourceGrantAttempt {
            session: aegis_security::authority::ActorSessionId(7),
            principal: Some(aegis_security::authority::ActorPrincipal::new("prin_actor").unwrap()),
            action: ResourceGrantAttemptAction::Consume,
            capability: Some(ActorCapability::ReadFile),
            resource_kind: Some(ResourceKind::FilesystemPath),
        };
        let entry = Journal::new(4).append(
            3,
            Origin::Actor {
                conn_id: 5,
                principal: "prin_actor".into(),
            },
            mutation,
            Effect::Refused {
                reason: "resource grant consume refused".into(),
            },
        );
        let encoded = serde_json::to_string(&entry).unwrap();
        assert!(encoded.contains("filesystem_path"));
        for secret in ["/private/customer.db", "rg_secret"] {
            assert!(
                !encoded.contains(secret),
                "audit entry leaked {secret}: {encoded}"
            );
        }
        let decoded: JournalEntry = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, entry);
    }

    #[test]
    fn capability_use_round_trips_without_endpoint_payload() {
        let mutation = JournalMutation::CapabilityUse {
            session: aegis_security::authority::ActorSessionId(9),
            principal: Some(aegis_security::authority::ActorPrincipal::new("prin_actor").unwrap()),
            capability: ActorCapability::PromptSecret,
            action: CapabilityUseAction::Prompt,
        };
        let encoded = serde_json::to_string(&mutation).unwrap();
        assert!(encoded.contains("PromptSecret"));
        assert!(!encoded.contains("password"));
        let decoded: JournalMutation = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, mutation);
    }

    #[test]
    fn actor_action_round_trips_without_bearer_token() {
        let mutation = JournalMutation::ActorAction {
            action_id: Some(17),
            interaction_domain: aegis_model::interaction_domain::InteractionDomainId(4),
            target: aegis_model::semantic::SemanticObjectId::for_window(
                aegis_model::window::WindowId(9),
            ),
            window: Some(aegis_model::window::WindowId(9)),
            actions: vec![AuditedSemanticAction::SyntheticInput {
                pointer_moves: 0,
                clicks: 1,
                scrolls: 0,
                key_presses: 0,
            }],
            actions_truncated: false,
            authority_revision: Some(22),
        };
        let entry = Journal::new(4).append(
            3,
            Origin::Actor {
                conn_id: 5,
                principal: "prin_actor".into(),
            },
            mutation,
            Effect::Applied,
        );
        let encoded = serde_json::to_string(&entry).unwrap();
        assert!(!encoded.contains("observation"));
        let decoded: JournalEntry = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, entry);
    }

    #[test]
    fn audited_actions_never_retain_text_keys_or_coordinates() {
        let actions = vec![
            SemanticActionIntent::TypeText {
                text: "private-password".into(),
            },
            SemanticActionIntent::SetValue {
                value: "private-value".into(),
            },
            SemanticActionIntent::SyntheticInput {
                actions: vec![
                    aegis_model::input::SyntheticInputAction::PointerMove {
                        position: aegis_model::Point { x: 123, y: 456 },
                    },
                    aegis_model::input::SyntheticInputAction::KeyPress { code: 777 },
                ],
            },
        ];
        let encoded = serde_json::to_string(&audit_semantic_actions(&actions)).unwrap();
        for secret in ["private-password", "private-value", "123", "456", "777"] {
            assert!(
                !encoded.contains(secret),
                "audit leaked {secret}: {encoded}"
            );
        }
        assert!(encoded.contains("utf8_bytes"));
        assert!(encoded.contains("key_presses"));
    }

    #[test]
    fn audited_commands_exclude_text_paths_coordinates_and_input_codes() {
        let commands = [
            Command::Notify {
                summary: "private-summary".into(),
                body: "private-body".into(),
                app_id: Some("org.example.SafeId".into()),
                external_id: Some("private-external-id".into()),
            },
            Command::Screenshot {
                path: "/private/customer/screenshot.png".into(),
                region: Some(aegis_model::Rect::new(123_456, 234_567, 10, 20)),
            },
            Command::InjectInput {
                id: aegis_model::window::WindowId(4),
                actions: vec![
                    aegis_model::input::SyntheticInputAction::PointerMove {
                        position: aegis_model::Point {
                            x: 345_678,
                            y: 456_789,
                        },
                    },
                    aegis_model::input::SyntheticInputAction::KeyPress { code: 777 },
                ],
            },
        ];
        let audited = commands
            .iter()
            .map(AuditedCommand::from)
            .collect::<Vec<_>>();
        let encoded = serde_json::to_string(&audited).unwrap();
        for secret in [
            "private-summary",
            "private-body",
            "private-external-id",
            "/private/customer/screenshot.png",
            "123456",
            "234567",
            "345678",
            "456789",
            "777",
        ] {
            assert!(
                !encoded.contains(secret),
                "audited command leaked {secret}: {encoded}"
            );
        }
        assert!(encoded.contains("summary_utf8_bytes"));
        assert!(encoded.contains("key_presses"));
        assert!(encoded.contains("org.example.SafeId"));
    }

    #[test]
    fn refusal_effects_are_reduced_to_payload_free_categories() {
        let mutation = JournalMutation::Command {
            cmd: AuditedCommand::Screenshot { region: false },
        };
        let effect = mutation.privacy_minimize_effect(Effect::Refused {
            reason: "write /home/alice/private/customer.png: secret failure".into(),
        });
        assert_eq!(
            effect,
            Effect::Refused {
                reason: "command refused".into()
            }
        );
        let encoded = serde_json::to_string(&effect).unwrap();
        assert!(!encoded.contains("alice"));
        assert!(!encoded.contains("customer"));
        assert!(!encoded.contains("secret failure"));
    }
}
