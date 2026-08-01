//! Authority and presentation domains for concurrent human and agent use.
//!
//! A [`RealmModel`] is the protocol- and renderer-independent source of truth
//! for the identities and invariants introduced by
//! [ADR-0040](../../docs/adr/0040-realms-seats-and-transferable-interaction-authority.md).
//! It deliberately does not contain Wayland resources, input events, or render
//! targets. The server binds those mechanisms to these durable identities.

use crate::window::WindowId;
use crate::{Rect, Size};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

macro_rules! durable_id {
    ($name:ident, $doc:literal) => {
        #[doc = $doc]
        #[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
        #[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
        pub struct $name(pub u64);

        impl $name {
            /// Identifier zero is reserved as the invalid/unassigned value.
            pub fn is_valid(self) -> bool {
                self.0 != 0
            }
        }
    };
}

durable_id!(
    PrincipalId,
    "Durable identity of a human, agent, or system principal."
);
durable_id!(
    RealmId,
    "Durable identity of one authority and presentation domain."
);
durable_id!(
    SeatId,
    "Durable identity of one independent logical input seat."
);
durable_id!(
    ClientId,
    "Durable identity of one Wayland client connection."
);
durable_id!(
    InteractionGroupId,
    "Durable identity of one atomically transferable interactive surface group."
);

/// The bootstrap human principal exists for the compositor lifetime.
pub const HUMAN_PRINCIPAL: PrincipalId = PrincipalId(1);
/// The physical desktop's authority domain.
pub const HUMAN_REALM: RealmId = RealmId(1);
/// The physical user's logical seat.
pub const HUMAN_SEAT: SeatId = SeatId(1);

/// What kind of subject a principal represents.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalKind {
    Human,
    Agent,
    System,
}

/// The role a realm plays in the compositor.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealmKind {
    Human,
    Agent,
    Secure,
}

/// Whether a realm currently accepts interaction.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RealmState {
    Active,
    Paused,
    Revoked,
}

/// Offscreen output advertised by an agent realm. Dimensions are logical
/// pixels; scale is fixed-point thousandths so snapshots remain deterministic.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VirtualOutput {
    pub width: u32,
    pub height: u32,
    pub scale_milli: u32,
    pub refresh_mhz: u32,
}

impl VirtualOutput {
    pub const DEFAULT_AGENT: Self = Self {
        width: 1920,
        height: 1080,
        scale_milli: 1000,
        refresh_mhz: 60_000,
    };

    pub fn validate(self) -> bool {
        let physical_width = u64::from(self.width)
            .saturating_mul(u64::from(self.scale_milli))
            .div_ceil(1000);
        let physical_height = u64::from(self.height)
            .saturating_mul(u64::from(self.scale_milli))
            .div_ceil(1000);
        (1..=16_384).contains(&self.width)
            && (1..=16_384).contains(&self.height)
            && (250..=8000).contains(&self.scale_milli)
            && (1_000..=1_000_000).contains(&self.refresh_mhz)
            // Bound one RGBA frame to 256 MiB. This is checked at the pure
            // model edge so an IPC request cannot force an oversized GPU
            // allocation even when logical dimensions are individually valid.
            && physical_width.saturating_mul(physical_height) <= 67_108_864
    }
}

/// Where a realm is presented.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type", rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationTarget {
    Physical,
    Virtual { output: VirtualOutput },
    Secure,
}

/// Input device classes exposed by a logical seat.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeatCapabilities {
    pub pointer: bool,
    pub keyboard: bool,
    pub touch: bool,
}

impl SeatCapabilities {
    pub const POINTER_KEYBOARD: Self = Self {
        pointer: true,
        keyboard: true,
        touch: false,
    };

    pub const ALL: Self = Self {
        pointer: true,
        keyboard: true,
        touch: true,
    };
}

impl Default for SeatCapabilities {
    fn default() -> Self {
        Self::POINTER_KEYBOARD
    }
}

/// Whether a client has demonstrated support for more than one `wl_seat`.
///
/// This is observed protocol compatibility, not an application allowlist.
/// `Unknown` remains conservative until the server sees resources bound for
/// more than one advertised seat.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum MultiSeatSupport {
    #[default]
    Unknown,
    Supported,
    Unsupported,
}

/// One subject that may receive a capability lease.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Principal {
    pub id: PrincipalId,
    pub kind: PrincipalKind,
    pub label: String,
    /// Authenticated agent-registry subject bound to this authority.
    ///
    /// `None` is reserved for compositor-local principals such as the human
    /// desktop and legacy/admin-created Realms. Agent IPC connections bind
    /// their opaque authenticated principal here when creating a Realm, so
    /// later operations can be authorized independently of display labels.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub subject: Option<String>,
}

/// One authority and presentation domain.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Realm {
    pub id: RealmId,
    pub kind: RealmKind,
    pub label: String,
    /// The principal allowed to own seats in this realm.
    pub controller: PrincipalId,
    pub state: RealmState,
    pub presentation: PresentationTarget,
}

/// Stable metadata for a Wayland logical seat.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seat {
    pub id: SeatId,
    /// Stable `wl_seat.name`; unique within the compositor lifetime.
    pub name: String,
    pub principal: PrincipalId,
    pub realm: RealmId,
    pub capabilities: SeatCapabilities,
    /// Pausing or revoking a realm disables all of its seats fail-closed.
    pub enabled: bool,
}

/// One Wayland client connection known to the authority model.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Client {
    pub id: ClientId,
    /// Sandbox/security context supplied by the connection accept path.
    pub security_context: Option<String>,
    pub multi_seat: MultiSeatSupport,
    pub connected: bool,
}

/// The smallest window set whose interaction authority transfers atomically.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionGroup {
    pub id: InteractionGroupId,
    pub client: ClientId,
    /// Complete toplevel roots in this group. Surface descendants follow
    /// their root and are not listed separately.
    pub windows: BTreeSet<WindowId>,
    /// Exactly one realm may deliver client input to this group.
    pub control_realm: RealmId,
    /// Realms allowed to draw read-only mirrors. The control realm is never
    /// repeated here.
    pub observer_realms: BTreeSet<RealmId>,
}

/// Result of creating an agent principal, realm, and seat atomically.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RealmBundle {
    pub principal: PrincipalId,
    pub realm: RealmId,
    pub seat: SeatId,
    pub revision: u64,
}

/// Options for an atomic interaction-authority transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferOptions {
    /// Keep a read-only mirror in the source realm after control moves.
    pub retain_source_as_observer: bool,
}

impl Default for TransferOptions {
    fn default() -> Self {
        Self {
            retain_source_as_observer: true,
        }
    }
}

/// Auditable result of one successful authority transfer.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthorityTransfer {
    pub group: InteractionGroupId,
    pub windows: Vec<WindowId>,
    pub from: RealmId,
    pub to: RealmId,
    pub source_retained_as_observer: bool,
    pub revision: u64,
}

/// Auditable result of fail-closed realm revocation.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealmRevocation {
    pub realm: RealmId,
    pub fallback: RealmId,
    pub transferred_groups: Vec<InteractionGroupId>,
    pub revision: u64,
}

/// Owned point-in-time authority model for IPC and shell consumers.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealmSnapshot {
    pub revision: u64,
    pub principals: Vec<Principal>,
    pub realms: Vec<Realm>,
    pub seats: Vec<Seat>,
    pub clients: Vec<Client>,
    pub interaction_groups: Vec<InteractionGroup>,
}

/// Placement of one observed window on a Realm's directed virtual output.
///
/// `output_rect` is in virtual-output logical coordinates. `surface_size` is
/// the target-local logical extent accepted by `InjectRealmInput`; together
/// they provide an unambiguous affine mapping from captured pixels to input.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RealmWindowPlacement {
    pub window: WindowId,
    pub output_rect: Rect,
    pub surface_size: Size,
}

/// One mutation in an optimistic, all-or-nothing Realm transaction.
///
/// Creation and permanent revocation are intentionally separate lifecycle
/// operations because they allocate/destroy protocol globals. Transactions
/// cover the live authority and presentation changes an agent orchestrator
/// commonly needs to commit together.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type", rename_all = "snake_case"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealmMutation {
    TransferWindow {
        window: WindowId,
        target: RealmId,
        retain_source_as_observer: bool,
    },
    SetObserver {
        group: InteractionGroupId,
        realm: RealmId,
        observe: bool,
    },
    ConfigureOutput {
        realm: RealmId,
        output: VirtualOutput,
    },
    SetState {
        realm: RealmId,
        state: RealmState,
    },
}

/// Per-operation result returned in transaction order.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type", rename_all = "snake_case"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealmMutationResult {
    Transferred {
        receipt: AuthorityTransfer,
    },
    ObserverChanged {
        group: InteractionGroupId,
        realm: RealmId,
        observe: bool,
        revision: u64,
    },
    OutputConfigured {
        realm: RealmId,
        output: VirtualOutput,
        revision: u64,
    },
    StateChanged {
        realm: RealmId,
        state: RealmState,
        revision: u64,
    },
}

/// Receipt for a committed optimistic Realm transaction.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealmTransactionReceipt {
    pub before_revision: u64,
    pub after_revision: u64,
    pub results: Vec<RealmMutationResult>,
}

/// A rejected model mutation. Every error leaves the model revision and all
/// collections unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealmError {
    InvalidId,
    UnknownPrincipal(PrincipalId),
    UnknownRealm(RealmId),
    UnknownSeat(SeatId),
    UnknownClient(ClientId),
    UnknownInteractionGroup(InteractionGroupId),
    UnknownWindow(WindowId),
    DuplicateWindow(WindowId),
    EmptyInteractionGroup,
    SeatNameInUse(String),
    PrincipalDoesNotControlRealm {
        principal: PrincipalId,
        realm: RealmId,
    },
    RealmNotActive(RealmId),
    ControlRealmCannotObserve(RealmId),
    AlreadyControlledBy(RealmId),
    CannotRevokeHumanRealm,
    InvalidFallbackRealm(RealmId),
    RealmHasNoVirtualOutput(RealmId),
    InvalidVirtualOutput,
    RevisionConflict {
        expected: u64,
        actual: u64,
    },
    EmptyTransaction,
    TransactionTooLarge,
    InvalidTransactionalState(RealmState),
}

impl fmt::Display for RealmError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            RealmError::InvalidId => write!(f, "identifier zero is reserved"),
            RealmError::UnknownPrincipal(id) => write!(f, "unknown principal {}", id.0),
            RealmError::UnknownRealm(id) => write!(f, "unknown realm {}", id.0),
            RealmError::UnknownSeat(id) => write!(f, "unknown seat {}", id.0),
            RealmError::UnknownClient(id) => write!(f, "unknown client {}", id.0),
            RealmError::UnknownInteractionGroup(id) => {
                write!(f, "unknown interaction group {}", id.0)
            }
            RealmError::UnknownWindow(id) => write!(f, "unknown window {}", id.0),
            RealmError::DuplicateWindow(id) => {
                write!(f, "window {} already belongs to an interaction group", id.0)
            }
            RealmError::EmptyInteractionGroup => {
                write!(f, "an interaction group must contain at least one window")
            }
            RealmError::SeatNameInUse(name) => write!(f, "seat name {name:?} is already in use"),
            RealmError::PrincipalDoesNotControlRealm { principal, realm } => write!(
                f,
                "principal {} does not control realm {}",
                principal.0, realm.0
            ),
            RealmError::RealmNotActive(id) => write!(f, "realm {} is not active", id.0),
            RealmError::ControlRealmCannotObserve(id) => {
                write!(f, "control realm {} cannot also be an observer", id.0)
            }
            RealmError::AlreadyControlledBy(id) => {
                write!(
                    f,
                    "interaction group is already controlled by realm {}",
                    id.0
                )
            }
            RealmError::CannotRevokeHumanRealm => write!(f, "the human realm cannot be revoked"),
            RealmError::InvalidFallbackRealm(id) => {
                write!(f, "realm {} is not a valid revocation fallback", id.0)
            }
            RealmError::RealmHasNoVirtualOutput(id) => {
                write!(f, "realm {} has no virtual output", id.0)
            }
            RealmError::InvalidVirtualOutput => write!(f, "virtual output parameters are invalid"),
            RealmError::RevisionConflict { expected, actual } => write!(
                f,
                "realm revision conflict: expected {expected}, current revision is {actual}"
            ),
            RealmError::EmptyTransaction => write!(f, "realm transaction is empty"),
            RealmError::TransactionTooLarge => {
                write!(f, "realm transaction exceeds the 64-operation limit")
            }
            RealmError::InvalidTransactionalState(state) => {
                write!(f, "realm state {state:?} is not transactional")
            }
        }
    }
}

impl std::error::Error for RealmError {}

/// Pure authority model. Identifiers are monotonically allocated and never
/// reused within the compositor lifetime.
#[derive(Debug, Clone)]
pub struct RealmModel {
    revision: u64,
    principals: BTreeMap<PrincipalId, Principal>,
    realms: BTreeMap<RealmId, Realm>,
    seats: BTreeMap<SeatId, Seat>,
    clients: BTreeMap<ClientId, Client>,
    interaction_groups: BTreeMap<InteractionGroupId, InteractionGroup>,
    window_groups: BTreeMap<WindowId, InteractionGroupId>,
    next_principal_id: u64,
    next_realm_id: u64,
    next_seat_id: u64,
    next_client_id: u64,
    next_group_id: u64,
}

impl Default for RealmModel {
    fn default() -> Self {
        Self::new()
    }
}

impl RealmModel {
    /// Bootstrap a compositor with the physical human principal, realm, and
    /// pointer/keyboard seat. The initial snapshot is revision 1.
    pub fn new() -> Self {
        let human = Principal {
            id: HUMAN_PRINCIPAL,
            kind: PrincipalKind::Human,
            label: "Human".into(),
            subject: None,
        };
        let realm = Realm {
            id: HUMAN_REALM,
            kind: RealmKind::Human,
            label: "Desktop".into(),
            controller: HUMAN_PRINCIPAL,
            state: RealmState::Active,
            presentation: PresentationTarget::Physical,
        };
        let seat = Seat {
            id: HUMAN_SEAT,
            name: "human".into(),
            principal: HUMAN_PRINCIPAL,
            realm: HUMAN_REALM,
            capabilities: SeatCapabilities::ALL,
            enabled: true,
        };
        Self {
            revision: 1,
            principals: BTreeMap::from([(HUMAN_PRINCIPAL, human)]),
            realms: BTreeMap::from([(HUMAN_REALM, realm)]),
            seats: BTreeMap::from([(HUMAN_SEAT, seat)]),
            clients: BTreeMap::new(),
            interaction_groups: BTreeMap::new(),
            window_groups: BTreeMap::new(),
            next_principal_id: 2,
            next_realm_id: 2,
            next_seat_id: 2,
            next_client_id: 1,
            next_group_id: 1,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn principal(&self, id: PrincipalId) -> Option<&Principal> {
        self.principals.get(&id)
    }

    pub fn realm(&self, id: RealmId) -> Option<&Realm> {
        self.realms.get(&id)
    }

    pub fn seat(&self, id: SeatId) -> Option<&Seat> {
        self.seats.get(&id)
    }

    pub fn client(&self, id: ClientId) -> Option<&Client> {
        self.clients.get(&id)
    }

    pub fn interaction_group(&self, id: InteractionGroupId) -> Option<&InteractionGroup> {
        self.interaction_groups.get(&id)
    }

    pub fn interaction_group_for_window(&self, window: WindowId) -> Option<&InteractionGroup> {
        self.window_groups
            .get(&window)
            .and_then(|id| self.interaction_groups.get(id))
    }

    pub fn interaction_groups_for_client(
        &self,
        client: ClientId,
    ) -> impl Iterator<Item = &InteractionGroup> {
        self.interaction_groups
            .values()
            .filter(move |group| group.client == client)
    }

    /// Create a principal, an active agent realm, and its seat as one model
    /// revision. No partially created authority can become externally visible.
    pub fn create_agent_realm(
        &mut self,
        label: impl Into<String>,
        capabilities: SeatCapabilities,
    ) -> RealmBundle {
        self.create_agent_realm_for_subject(label, capabilities, None)
    }

    /// Create an agent Realm owned by an authenticated registry subject.
    ///
    /// The subject is distinct from the cosmetic Realm label and is copied
    /// into the Realm's controlling principal as part of the same model
    /// revision. It is never accepted from an untrusted wire field; the IPC
    /// server supplies it from the credential-bound connection context.
    pub fn create_agent_realm_for_subject(
        &mut self,
        label: impl Into<String>,
        capabilities: SeatCapabilities,
        subject: Option<String>,
    ) -> RealmBundle {
        let label = label.into();
        let principal = self.alloc_principal();
        let realm = self.alloc_realm();
        let seat = self.alloc_seat();
        self.principals.insert(
            principal,
            Principal {
                id: principal,
                kind: PrincipalKind::Agent,
                label: label.clone(),
                subject,
            },
        );
        self.realms.insert(
            realm,
            Realm {
                id: realm,
                kind: RealmKind::Agent,
                label,
                controller: principal,
                state: RealmState::Active,
                presentation: PresentationTarget::Virtual {
                    output: VirtualOutput::DEFAULT_AGENT,
                },
            },
        );
        self.seats.insert(
            seat,
            Seat {
                id: seat,
                name: format!("agent-{}", realm.0),
                principal,
                realm,
                capabilities,
                enabled: true,
            },
        );
        let revision = self.bump_revision();
        RealmBundle {
            principal,
            realm,
            seat,
            revision,
        }
    }

    pub fn create_principal(
        &mut self,
        kind: PrincipalKind,
        label: impl Into<String>,
    ) -> PrincipalId {
        let id = self.alloc_principal();
        self.principals.insert(
            id,
            Principal {
                id,
                kind,
                label: label.into(),
                subject: None,
            },
        );
        self.bump_revision();
        id
    }

    pub fn create_realm(
        &mut self,
        kind: RealmKind,
        label: impl Into<String>,
        controller: PrincipalId,
    ) -> Result<RealmId, RealmError> {
        if !controller.is_valid() {
            return Err(RealmError::InvalidId);
        }
        if !self.principals.contains_key(&controller) {
            return Err(RealmError::UnknownPrincipal(controller));
        }
        let id = self.alloc_realm();
        self.realms.insert(
            id,
            Realm {
                id,
                kind,
                label: label.into(),
                controller,
                state: RealmState::Active,
                presentation: match kind {
                    RealmKind::Human => PresentationTarget::Physical,
                    RealmKind::Agent => PresentationTarget::Virtual {
                        output: VirtualOutput::DEFAULT_AGENT,
                    },
                    RealmKind::Secure => PresentationTarget::Secure,
                },
            },
        );
        self.bump_revision();
        Ok(id)
    }

    pub fn create_seat(
        &mut self,
        name: impl Into<String>,
        principal: PrincipalId,
        realm: RealmId,
        capabilities: SeatCapabilities,
    ) -> Result<SeatId, RealmError> {
        let name = name.into();
        if self.seats.values().any(|seat| seat.name == name) {
            return Err(RealmError::SeatNameInUse(name));
        }
        let target = self
            .realms
            .get(&realm)
            .ok_or(RealmError::UnknownRealm(realm))?;
        if target.state != RealmState::Active {
            return Err(RealmError::RealmNotActive(realm));
        }
        if !self.principals.contains_key(&principal) {
            return Err(RealmError::UnknownPrincipal(principal));
        }
        if target.controller != principal {
            return Err(RealmError::PrincipalDoesNotControlRealm { principal, realm });
        }
        let id = self.alloc_seat();
        self.seats.insert(
            id,
            Seat {
                id,
                name,
                principal,
                realm,
                capabilities,
                enabled: true,
            },
        );
        self.bump_revision();
        Ok(id)
    }

    pub fn configure_virtual_output(
        &mut self,
        realm: RealmId,
        output: VirtualOutput,
    ) -> Result<(), RealmError> {
        if !output.validate() {
            return Err(RealmError::InvalidVirtualOutput);
        }
        let target = self
            .realms
            .get_mut(&realm)
            .ok_or(RealmError::UnknownRealm(realm))?;
        if !matches!(target.presentation, PresentationTarget::Virtual { .. }) {
            return Err(RealmError::RealmHasNoVirtualOutput(realm));
        }
        if target.presentation != (PresentationTarget::Virtual { output }) {
            target.presentation = PresentationTarget::Virtual { output };
            self.bump_revision();
        }
        Ok(())
    }

    /// Apply a bounded optimistic transaction atomically. All operations run
    /// against a private clone; `self` changes only after every validation and
    /// mutation succeeds.
    pub fn transact(
        &mut self,
        expected_revision: Option<u64>,
        mutations: &[RealmMutation],
    ) -> Result<RealmTransactionReceipt, RealmError> {
        if let Some(expected) = expected_revision
            && expected != self.revision
        {
            return Err(RealmError::RevisionConflict {
                expected,
                actual: self.revision,
            });
        }
        if mutations.is_empty() {
            return Err(RealmError::EmptyTransaction);
        }
        if mutations.len() > 64 {
            return Err(RealmError::TransactionTooLarge);
        }

        let before_revision = self.revision;
        let mut staged = self.clone();
        let mut results = Vec::with_capacity(mutations.len());
        for mutation in mutations {
            let result = match *mutation {
                RealmMutation::TransferWindow {
                    window,
                    target,
                    retain_source_as_observer,
                } => {
                    let group = staged
                        .interaction_group_for_window(window)
                        .map(|group| group.id)
                        .ok_or(RealmError::UnknownWindow(window))?;
                    RealmMutationResult::Transferred {
                        receipt: staged.transfer_control(
                            group,
                            target,
                            TransferOptions {
                                retain_source_as_observer,
                            },
                        )?,
                    }
                }
                RealmMutation::SetObserver {
                    group,
                    realm,
                    observe,
                } => {
                    staged.set_observer(group, realm, observe)?;
                    RealmMutationResult::ObserverChanged {
                        group,
                        realm,
                        observe,
                        revision: staged.revision,
                    }
                }
                RealmMutation::ConfigureOutput { realm, output } => {
                    staged.configure_virtual_output(realm, output)?;
                    RealmMutationResult::OutputConfigured {
                        realm,
                        output,
                        revision: staged.revision,
                    }
                }
                RealmMutation::SetState { realm, state } => {
                    match state {
                        RealmState::Active => staged.resume_realm(realm)?,
                        RealmState::Paused => staged.pause_realm(realm)?,
                        RealmState::Revoked => {
                            return Err(RealmError::InvalidTransactionalState(state));
                        }
                    }
                    RealmMutationResult::StateChanged {
                        realm,
                        state,
                        revision: staged.revision,
                    }
                }
            };
            results.push(result);
        }
        staged.validate()?;
        let after_revision = staged.revision;
        *self = staged;
        Ok(RealmTransactionReceipt {
            before_revision,
            after_revision,
            results,
        })
    }

    /// Register a newly accepted Wayland client connection.
    pub fn register_client(&mut self, security_context: Option<String>) -> ClientId {
        let id = self.alloc_client();
        self.clients.insert(
            id,
            Client {
                id,
                security_context,
                multi_seat: MultiSeatSupport::Unknown,
                connected: true,
            },
        );
        self.bump_revision();
        id
    }

    /// Mark a Wayland connection as gone. Durable identity and metadata stay
    /// available for audit; live windows are removed separately by their
    /// resource destroy callbacks.
    pub fn disconnect_client(&mut self, client: ClientId) -> Result<(), RealmError> {
        let record = self
            .clients
            .get_mut(&client)
            .ok_or(RealmError::UnknownClient(client))?;
        if record.connected {
            record.connected = false;
            self.bump_revision();
        }
        Ok(())
    }

    /// Record observed client protocol compatibility.
    pub fn set_client_multi_seat(
        &mut self,
        client: ClientId,
        support: MultiSeatSupport,
    ) -> Result<(), RealmError> {
        let record = self
            .clients
            .get_mut(&client)
            .ok_or(RealmError::UnknownClient(client))?;
        if record.multi_seat != support {
            record.multi_seat = support;
            self.bump_revision();
        }
        Ok(())
    }

    /// Create a non-empty interaction group. Validation completes before an
    /// identifier is allocated or any collection changes.
    pub fn create_interaction_group(
        &mut self,
        client: ClientId,
        windows: &[WindowId],
        control_realm: RealmId,
    ) -> Result<InteractionGroupId, RealmError> {
        if !self.clients.contains_key(&client) {
            return Err(RealmError::UnknownClient(client));
        }
        if !self
            .clients
            .get(&client)
            .is_some_and(|client| client.connected)
        {
            return Err(RealmError::UnknownClient(client));
        }
        self.require_active_realm(control_realm)?;
        if windows.is_empty() {
            return Err(RealmError::EmptyInteractionGroup);
        }
        let mut members = BTreeSet::new();
        for &window in windows {
            if window.0 == 0 {
                return Err(RealmError::InvalidId);
            }
            if self.window_groups.contains_key(&window) || !members.insert(window) {
                return Err(RealmError::DuplicateWindow(window));
            }
        }
        let id = self.alloc_group();
        for &window in &members {
            self.window_groups.insert(window, id);
        }
        self.interaction_groups.insert(
            id,
            InteractionGroup {
                id,
                client,
                windows: members,
                control_realm,
                observer_realms: BTreeSet::new(),
            },
        );
        self.bump_revision();
        Ok(id)
    }

    pub fn add_window_to_group(
        &mut self,
        group: InteractionGroupId,
        window: WindowId,
    ) -> Result<(), RealmError> {
        if window.0 == 0 {
            return Err(RealmError::InvalidId);
        }
        if self.window_groups.contains_key(&window) {
            return Err(RealmError::DuplicateWindow(window));
        }
        let target = self
            .interaction_groups
            .get_mut(&group)
            .ok_or(RealmError::UnknownInteractionGroup(group))?;
        target.windows.insert(window);
        self.window_groups.insert(window, group);
        self.bump_revision();
        Ok(())
    }

    /// Remove a retired window. The group is removed when its last toplevel
    /// disappears. Identifiers remain consumed and are never reused.
    pub fn remove_window(&mut self, window: WindowId) -> Result<(), RealmError> {
        let group = self
            .window_groups
            .remove(&window)
            .ok_or(RealmError::UnknownWindow(window))?;
        let empty = if let Some(target) = self.interaction_groups.get_mut(&group) {
            target.windows.remove(&window);
            target.windows.is_empty()
        } else {
            false
        };
        if empty {
            self.interaction_groups.remove(&group);
        }
        self.bump_revision();
        Ok(())
    }

    /// Add or remove a read-only mirror. A group cannot observe itself in its
    /// controlling realm because that would blur the input boundary.
    pub fn set_observer(
        &mut self,
        group: InteractionGroupId,
        realm: RealmId,
        observe: bool,
    ) -> Result<(), RealmError> {
        let realm_state = self
            .realms
            .get(&realm)
            .ok_or(RealmError::UnknownRealm(realm))?
            .state;
        if realm_state == RealmState::Revoked {
            return Err(RealmError::RealmNotActive(realm));
        }
        let target = self
            .interaction_groups
            .get_mut(&group)
            .ok_or(RealmError::UnknownInteractionGroup(group))?;
        if observe && target.control_realm == realm {
            return Err(RealmError::ControlRealmCannotObserve(realm));
        }
        let changed = if observe {
            target.observer_realms.insert(realm)
        } else {
            target.observer_realms.remove(&realm)
        };
        if changed {
            self.bump_revision();
        }
        Ok(())
    }

    /// Atomically transfer every window in an interaction group to a new
    /// controlling realm.
    pub fn transfer_control(
        &mut self,
        group: InteractionGroupId,
        target_realm: RealmId,
        options: TransferOptions,
    ) -> Result<AuthorityTransfer, RealmError> {
        self.require_active_realm(target_realm)?;
        let current = self
            .interaction_groups
            .get(&group)
            .ok_or(RealmError::UnknownInteractionGroup(group))?;
        let source_realm = current.control_realm;
        if source_realm == target_realm {
            return Err(RealmError::AlreadyControlledBy(target_realm));
        }
        let windows = current.windows.iter().copied().collect::<Vec<_>>();
        let target = self
            .interaction_groups
            .get_mut(&group)
            .expect("validated interaction group disappeared");
        target.control_realm = target_realm;
        target.observer_realms.remove(&target_realm);
        let source_retained_as_observer = if options.retain_source_as_observer {
            target.observer_realms.insert(source_realm);
            true
        } else {
            target.observer_realms.remove(&source_realm);
            false
        };
        let revision = self.bump_revision();
        Ok(AuthorityTransfer {
            group,
            windows,
            from: source_realm,
            to: target_realm,
            source_retained_as_observer,
            revision,
        })
    }

    /// Pause a realm and disable all seats attached to it.
    pub fn pause_realm(&mut self, realm: RealmId) -> Result<(), RealmError> {
        self.set_realm_running_state(realm, RealmState::Paused, false)
    }

    /// Resume a paused realm and re-enable its seats.
    pub fn resume_realm(&mut self, realm: RealmId) -> Result<(), RealmError> {
        let current = self
            .realms
            .get(&realm)
            .ok_or(RealmError::UnknownRealm(realm))?
            .state;
        if current == RealmState::Revoked {
            return Err(RealmError::RealmNotActive(realm));
        }
        self.set_realm_running_state(realm, RealmState::Active, true)
    }

    /// Permanently revoke a non-human realm. Controlled groups move to an
    /// active fallback in the same revision, all mirrors into the revoked
    /// realm disappear, and its seats are disabled.
    pub fn revoke_realm(
        &mut self,
        realm: RealmId,
        fallback: RealmId,
    ) -> Result<RealmRevocation, RealmError> {
        if realm == HUMAN_REALM {
            return Err(RealmError::CannotRevokeHumanRealm);
        }
        if realm == fallback {
            return Err(RealmError::InvalidFallbackRealm(fallback));
        }
        if !self.realms.contains_key(&realm) {
            return Err(RealmError::UnknownRealm(realm));
        }
        if self
            .realms
            .get(&realm)
            .is_some_and(|record| record.state == RealmState::Revoked)
        {
            return Err(RealmError::RealmNotActive(realm));
        }
        self.require_active_realm(fallback)
            .map_err(|_| RealmError::InvalidFallbackRealm(fallback))?;

        let transferred_groups = self
            .interaction_groups
            .values()
            .filter(|group| group.control_realm == realm)
            .map(|group| group.id)
            .collect::<Vec<_>>();
        for group in self.interaction_groups.values_mut() {
            group.observer_realms.remove(&realm);
            if group.control_realm == realm {
                group.control_realm = fallback;
                group.observer_realms.remove(&fallback);
            }
        }
        self.realms
            .get_mut(&realm)
            .expect("validated realm disappeared")
            .state = RealmState::Revoked;
        for seat in self.seats.values_mut().filter(|seat| seat.realm == realm) {
            seat.enabled = false;
        }
        let revision = self.bump_revision();
        Ok(RealmRevocation {
            realm,
            fallback,
            transferred_groups,
            revision,
        })
    }

    /// Whether a seat may currently deliver input to a window.
    pub fn seat_controls_window(&self, seat: SeatId, window: WindowId) -> bool {
        let Some(seat) = self.seats.get(&seat) else {
            return false;
        };
        if !seat.enabled
            || self
                .realms
                .get(&seat.realm)
                .is_none_or(|realm| realm.state != RealmState::Active)
        {
            return false;
        }
        self.interaction_group_for_window(window)
            .is_some_and(|group| group.control_realm == seat.realm)
    }

    /// Whether a realm may render a window, either as controller or observer.
    pub fn realm_observes_window(&self, realm: RealmId, window: WindowId) -> bool {
        self.interaction_group_for_window(window)
            .is_some_and(|group| {
                group.control_realm == realm || group.observer_realms.contains(&realm)
            })
    }

    pub fn snapshot(&self) -> RealmSnapshot {
        RealmSnapshot {
            revision: self.revision,
            principals: self.principals.values().cloned().collect(),
            realms: self.realms.values().cloned().collect(),
            seats: self.seats.values().cloned().collect(),
            clients: self.clients.values().cloned().collect(),
            interaction_groups: self.interaction_groups.values().cloned().collect(),
        }
    }

    /// Validate the complete live model. Intended for tests, debug assertions,
    /// and production health checks.
    pub fn validate(&self) -> Result<(), RealmError> {
        let mut names = BTreeSet::new();
        for seat in self.seats.values() {
            if !names.insert(seat.name.as_str()) {
                return Err(RealmError::SeatNameInUse(seat.name.clone()));
            }
            let realm = self
                .realms
                .get(&seat.realm)
                .ok_or(RealmError::UnknownRealm(seat.realm))?;
            if realm.controller != seat.principal {
                return Err(RealmError::PrincipalDoesNotControlRealm {
                    principal: seat.principal,
                    realm: seat.realm,
                });
            }
            if !self.principals.contains_key(&seat.principal) {
                return Err(RealmError::UnknownPrincipal(seat.principal));
            }
        }
        let mut seen_windows = BTreeSet::new();
        for group in self.interaction_groups.values() {
            if group.windows.is_empty() {
                return Err(RealmError::EmptyInteractionGroup);
            }
            if !self.clients.contains_key(&group.client) {
                return Err(RealmError::UnknownClient(group.client));
            }
            if !self.realms.contains_key(&group.control_realm) {
                return Err(RealmError::UnknownRealm(group.control_realm));
            }
            if group.observer_realms.contains(&group.control_realm) {
                return Err(RealmError::ControlRealmCannotObserve(group.control_realm));
            }
            for observer in &group.observer_realms {
                if !self.realms.contains_key(observer) {
                    return Err(RealmError::UnknownRealm(*observer));
                }
            }
            for &window in &group.windows {
                if !seen_windows.insert(window) {
                    return Err(RealmError::DuplicateWindow(window));
                }
                if self.window_groups.get(&window) != Some(&group.id) {
                    return Err(RealmError::UnknownWindow(window));
                }
            }
        }
        if seen_windows.len() != self.window_groups.len() {
            let unknown = self
                .window_groups
                .keys()
                .find(|window| !seen_windows.contains(window))
                .copied()
                .unwrap_or_default();
            return Err(RealmError::UnknownWindow(unknown));
        }
        Ok(())
    }

    fn set_realm_running_state(
        &mut self,
        realm: RealmId,
        state: RealmState,
        seats_enabled: bool,
    ) -> Result<(), RealmError> {
        let target = self
            .realms
            .get_mut(&realm)
            .ok_or(RealmError::UnknownRealm(realm))?;
        if target.state == RealmState::Revoked {
            return Err(RealmError::RealmNotActive(realm));
        }
        let mut changed = target.state != state;
        target.state = state;
        for seat in self.seats.values_mut().filter(|seat| seat.realm == realm) {
            changed |= seat.enabled != seats_enabled;
            seat.enabled = seats_enabled;
        }
        if changed {
            self.bump_revision();
        }
        Ok(())
    }

    fn require_active_realm(&self, id: RealmId) -> Result<(), RealmError> {
        let realm = self.realms.get(&id).ok_or(RealmError::UnknownRealm(id))?;
        if realm.state != RealmState::Active {
            return Err(RealmError::RealmNotActive(id));
        }
        Ok(())
    }

    fn bump_revision(&mut self) -> u64 {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("realm model revision exhausted");
        self.revision
    }

    fn alloc_principal(&mut self) -> PrincipalId {
        let id = PrincipalId(self.next_principal_id);
        self.next_principal_id = self
            .next_principal_id
            .checked_add(1)
            .expect("principal id exhausted");
        id
    }

    fn alloc_realm(&mut self) -> RealmId {
        let id = RealmId(self.next_realm_id);
        self.next_realm_id = self
            .next_realm_id
            .checked_add(1)
            .expect("realm id exhausted");
        id
    }

    fn alloc_seat(&mut self) -> SeatId {
        let id = SeatId(self.next_seat_id);
        self.next_seat_id = self.next_seat_id.checked_add(1).expect("seat id exhausted");
        id
    }

    fn alloc_client(&mut self) -> ClientId {
        let id = ClientId(self.next_client_id);
        self.next_client_id = self
            .next_client_id
            .checked_add(1)
            .expect("client id exhausted");
        id
    }

    fn alloc_group(&mut self) -> InteractionGroupId {
        let id = InteractionGroupId(self.next_group_id);
        self.next_group_id = self
            .next_group_id
            .checked_add(1)
            .expect("interaction group id exhausted");
        id
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model_with_window() -> (RealmModel, ClientId, InteractionGroupId) {
        let mut model = RealmModel::new();
        let client = model.register_client(Some("test.client".into()));
        let group = model
            .create_interaction_group(client, &[WindowId(10)], HUMAN_REALM)
            .unwrap();
        (model, client, group)
    }

    #[test]
    fn bootstrap_has_one_active_human_authority_chain() {
        let model = RealmModel::new();
        assert_eq!(model.revision(), 1);
        assert_eq!(
            model.realm(HUMAN_REALM).map(|realm| realm.controller),
            Some(HUMAN_PRINCIPAL)
        );
        assert_eq!(
            model
                .seat(HUMAN_SEAT)
                .map(|seat| (seat.realm, seat.enabled)),
            Some((HUMAN_REALM, true))
        );
        assert!(model.validate().is_ok());
    }

    #[test]
    fn agent_bundle_is_created_in_one_revision() {
        let mut model = RealmModel::new();
        let bundle = model.create_agent_realm("Research", SeatCapabilities::POINTER_KEYBOARD);
        assert_eq!(bundle.revision, 2);
        assert_eq!(
            model.realm(bundle.realm).map(|realm| realm.controller),
            Some(bundle.principal)
        );
        assert_eq!(
            model.seat(bundle.seat).map(|seat| seat.realm),
            Some(bundle.realm)
        );
        assert!(model.validate().is_ok());
    }

    #[test]
    fn authenticated_subject_is_bound_to_the_controlling_principal() {
        let mut model = RealmModel::new();
        let bundle = model.create_agent_realm_for_subject(
            "Agent",
            SeatCapabilities::POINTER_KEYBOARD,
            Some("prin_test".into()),
        );
        let realm = model.realm(bundle.realm).expect("realm");
        let principal = model.principal(realm.controller).expect("controller");
        assert_eq!(principal.subject.as_deref(), Some("prin_test"));
        assert_eq!(
            model.snapshot().principals[1].subject.as_deref(),
            Some("prin_test")
        );
    }

    #[test]
    fn transfer_moves_control_and_retains_read_only_human_observation() {
        let (mut model, _, group) = model_with_window();
        let agent = model.create_agent_realm("Agent", SeatCapabilities::default());
        let receipt = model
            .transfer_control(group, agent.realm, TransferOptions::default())
            .unwrap();
        assert_eq!(receipt.from, HUMAN_REALM);
        assert_eq!(receipt.to, agent.realm);
        assert_eq!(receipt.windows, vec![WindowId(10)]);
        assert!(model.seat_controls_window(agent.seat, WindowId(10)));
        assert!(!model.seat_controls_window(HUMAN_SEAT, WindowId(10)));
        assert!(model.realm_observes_window(HUMAN_REALM, WindowId(10)));
        assert!(model.realm_observes_window(agent.realm, WindowId(10)));
        assert!(model.validate().is_ok());
    }

    #[test]
    fn interaction_group_transfers_all_member_windows_atomically() {
        let mut model = RealmModel::new();
        let client = model.register_client(None);
        let group = model
            .create_interaction_group(
                client,
                &[WindowId(3), WindowId(4), WindowId(5)],
                HUMAN_REALM,
            )
            .unwrap();
        let agent = model.create_agent_realm("Agent", SeatCapabilities::default());
        let receipt = model
            .transfer_control(group, agent.realm, TransferOptions::default())
            .unwrap();
        assert_eq!(receipt.windows, vec![WindowId(3), WindowId(4), WindowId(5)]);
        for id in receipt.windows {
            assert!(model.seat_controls_window(agent.seat, id));
        }
    }

    #[test]
    fn rejected_transfer_is_atomic() {
        let (mut model, _, group) = model_with_window();
        let agent = model.create_agent_realm("Agent", SeatCapabilities::default());
        model.pause_realm(agent.realm).unwrap();
        let before = model.snapshot();
        assert_eq!(
            model.transfer_control(group, agent.realm, TransferOptions::default()),
            Err(RealmError::RealmNotActive(agent.realm))
        );
        assert_eq!(model.snapshot(), before);
    }

    #[test]
    fn paused_realm_cannot_control_until_resumed() {
        let (mut model, _, group) = model_with_window();
        let agent = model.create_agent_realm("Agent", SeatCapabilities::default());
        model
            .transfer_control(group, agent.realm, TransferOptions::default())
            .unwrap();
        model.pause_realm(agent.realm).unwrap();
        assert!(!model.seat_controls_window(agent.seat, WindowId(10)));
        assert!(!model.seat(agent.seat).unwrap().enabled);
        model.resume_realm(agent.realm).unwrap();
        assert!(model.seat_controls_window(agent.seat, WindowId(10)));
    }

    #[test]
    fn revocation_drains_control_and_removes_observation() {
        let (mut model, _, group) = model_with_window();
        let agent = model.create_agent_realm("Agent", SeatCapabilities::default());
        model
            .transfer_control(group, agent.realm, TransferOptions::default())
            .unwrap();
        let second_client = model.register_client(None);
        let second_group = model
            .create_interaction_group(second_client, &[WindowId(20)], HUMAN_REALM)
            .unwrap();
        model.set_observer(second_group, agent.realm, true).unwrap();

        let receipt = model.revoke_realm(agent.realm, HUMAN_REALM).unwrap();
        assert_eq!(receipt.transferred_groups, vec![group]);
        assert!(model.seat_controls_window(HUMAN_SEAT, WindowId(10)));
        assert!(!model.seat(agent.seat).unwrap().enabled);
        assert!(!model.realm_observes_window(agent.realm, WindowId(20)));
        assert_eq!(
            model.realm(agent.realm).map(|realm| realm.state),
            Some(RealmState::Revoked)
        );
        assert!(model.validate().is_ok());
    }

    #[test]
    fn window_can_belong_to_exactly_one_interaction_group() {
        let (mut model, client, _) = model_with_window();
        let before = model.snapshot();
        assert_eq!(
            model.create_interaction_group(client, &[WindowId(10)], HUMAN_REALM),
            Err(RealmError::DuplicateWindow(WindowId(10)))
        );
        assert_eq!(model.snapshot(), before);
    }

    #[test]
    fn seat_names_are_unique_and_controller_must_match() {
        let mut model = RealmModel::new();
        let agent = model.create_agent_realm("Agent", SeatCapabilities::default());
        let before = model.snapshot();
        assert_eq!(
            model.create_seat(
                "human",
                agent.principal,
                agent.realm,
                SeatCapabilities::default()
            ),
            Err(RealmError::SeatNameInUse("human".into()))
        );
        assert_eq!(model.snapshot(), before);

        assert_eq!(
            model.create_seat(
                "wrong-controller",
                HUMAN_PRINCIPAL,
                agent.realm,
                SeatCapabilities::default()
            ),
            Err(RealmError::PrincipalDoesNotControlRealm {
                principal: HUMAN_PRINCIPAL,
                realm: agent.realm,
            })
        );
    }

    #[test]
    fn retired_identifiers_are_not_reused() {
        let (mut model, _, first_group) = model_with_window();
        model.remove_window(WindowId(10)).unwrap();
        let client = model.register_client(None);
        let second_group = model
            .create_interaction_group(client, &[WindowId(11)], HUMAN_REALM)
            .unwrap();
        assert!(second_group > first_group);
    }

    #[test]
    fn transaction_commits_transfer_output_and_pause_together() {
        let (mut model, _, _) = model_with_window();
        let agent = model.create_agent_realm("Agent", SeatCapabilities::ALL);
        let before = model.revision();
        let output = VirtualOutput {
            width: 2560,
            height: 1440,
            scale_milli: 1250,
            refresh_mhz: 90_000,
        };
        let receipt = model
            .transact(
                Some(before),
                &[
                    RealmMutation::TransferWindow {
                        window: WindowId(10),
                        target: agent.realm,
                        retain_source_as_observer: true,
                    },
                    RealmMutation::ConfigureOutput {
                        realm: agent.realm,
                        output,
                    },
                    RealmMutation::SetState {
                        realm: agent.realm,
                        state: RealmState::Paused,
                    },
                ],
            )
            .unwrap();
        assert_eq!(receipt.before_revision, before);
        assert_eq!(receipt.results.len(), 3);
        assert_eq!(
            model.realm(agent.realm).map(|realm| realm.presentation),
            Some(PresentationTarget::Virtual { output })
        );
        assert!(!model.seat_controls_window(agent.seat, WindowId(10)));
        assert!(model.realm_observes_window(HUMAN_REALM, WindowId(10)));
        assert!(model.validate().is_ok());
    }

    #[test]
    fn failed_transaction_and_revision_conflict_leave_snapshot_unchanged() {
        let (mut model, _, _) = model_with_window();
        let agent = model.create_agent_realm("Agent", SeatCapabilities::ALL);
        let before = model.snapshot();
        assert_eq!(
            model.transact(
                Some(before.revision),
                &[
                    RealmMutation::TransferWindow {
                        window: WindowId(10),
                        target: agent.realm,
                        retain_source_as_observer: true,
                    },
                    RealmMutation::ConfigureOutput {
                        realm: HUMAN_REALM,
                        output: VirtualOutput::DEFAULT_AGENT,
                    },
                ],
            ),
            Err(RealmError::RealmHasNoVirtualOutput(HUMAN_REALM))
        );
        assert_eq!(model.snapshot(), before);
        assert_eq!(
            model.transact(
                Some(before.revision - 1),
                &[RealmMutation::SetState {
                    realm: agent.realm,
                    state: RealmState::Paused,
                }],
            ),
            Err(RealmError::RevisionConflict {
                expected: before.revision - 1,
                actual: before.revision,
            })
        );
        assert_eq!(model.snapshot(), before);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn snapshot_round_trips_through_json() {
        let (mut model, _, group) = model_with_window();
        let agent = model.create_agent_realm("Agent", SeatCapabilities::default());
        model
            .transfer_control(group, agent.realm, TransferOptions::default())
            .unwrap();
        let snapshot = model.snapshot();
        let json = serde_json::to_string(&snapshot).unwrap();
        let decoded: RealmSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, snapshot);
    }
}
