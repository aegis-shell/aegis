//! Authority and presentation domains for concurrent human and agent use.
//!
//! An [`InteractionDomainModel`] is the protocol- and renderer-independent source of truth
//! for the identities and invariants introduced by
//! [ADR-0103](../../docs/adr/0103-actor-authority-and-interaction-domain-architecture.md).
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
    InteractionPrincipalId,
    "Durable compositor-model identity of a human, agent, or system principal. Distinct from an authenticated ActorPrincipal."
);
durable_id!(
    InteractionDomainId,
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
pub const HUMAN_PRINCIPAL: InteractionPrincipalId = InteractionPrincipalId(1);
/// The physical desktop's authority domain.
pub const HUMAN_INTERACTION_DOMAIN: InteractionDomainId = InteractionDomainId(1);
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

/// The role an interaction domain plays in the compositor.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionDomainKind {
    Human,
    Agent,
    Secure,
}

/// Whether an interaction domain currently accepts interaction.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "snake_case"))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InteractionDomainState {
    Active,
    Paused,
    Revoked,
}

/// Offscreen output advertised by an agent interaction domain. Dimensions are logical
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

/// Where an interaction domain is presented.
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
    pub id: InteractionPrincipalId,
    pub kind: PrincipalKind,
    pub label: String,
    /// Authenticated agent-registry subject bound to this authority.
    ///
    /// `None` is reserved for compositor-local principals such as the human
    /// desktop and legacy/admin-created Interaction Domains. Agent IPC connections bind
    /// their opaque authenticated principal here when creating an Interaction Domain, so
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
pub struct InteractionDomain {
    pub id: InteractionDomainId,
    pub kind: InteractionDomainKind,
    pub label: String,
    /// The principal allowed to own seats in this interaction domain.
    pub controller: InteractionPrincipalId,
    pub state: InteractionDomainState,
    pub presentation: PresentationTarget,
}

/// Stable metadata for a Wayland logical seat.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seat {
    pub id: SeatId,
    /// Stable `wl_seat.name`; unique within the compositor lifetime.
    pub name: String,
    pub principal: InteractionPrincipalId,
    pub interaction_domain: InteractionDomainId,
    pub capabilities: SeatCapabilities,
    /// Pausing or revoking an interaction domain disables all of its seats fail-closed.
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
    /// Exactly one interaction domain may deliver client input to this group.
    pub control_interaction_domain: InteractionDomainId,
    /// Interaction Domains allowed to draw read-only mirrors. The control domain is never
    /// repeated here.
    pub observer_interaction_domains: BTreeSet<InteractionDomainId>,
}

/// Result of creating an agent principal, interaction domain, and seat atomically.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractionDomainBundle {
    pub principal: InteractionPrincipalId,
    pub interaction_domain: InteractionDomainId,
    pub seat: SeatId,
    pub revision: u64,
}

/// Options for an atomic interaction-authority transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TransferOptions {
    /// Keep a read-only mirror in the source interaction domain after control moves.
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
    pub from: InteractionDomainId,
    pub to: InteractionDomainId,
    pub source_retained_as_observer: bool,
    pub revision: u64,
}

/// Auditable result of fail-closed interaction domain revocation.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionDomainRevocation {
    pub interaction_domain: InteractionDomainId,
    pub fallback: InteractionDomainId,
    pub transferred_groups: Vec<InteractionGroupId>,
    pub revision: u64,
}

/// Owned point-in-time authority model for IPC and shell consumers.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionDomainSnapshot {
    pub revision: u64,
    pub principals: Vec<Principal>,
    pub interaction_domains: Vec<InteractionDomain>,
    pub seats: Vec<Seat>,
    pub clients: Vec<Client>,
    pub interaction_groups: Vec<InteractionGroup>,
}

/// Placement of one observed window on an Interaction Domain's directed virtual output.
///
/// `output_rect` is in virtual-output logical coordinates. `surface_size` is
/// the target-local logical extent accepted by observation-bound
/// `ActInInteractionDomain`; together they correlate pixel compatibility captures with
/// compositor-owned semantic window roots.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InteractionDomainWindowPlacement {
    pub window: WindowId,
    pub output_rect: Rect,
    pub surface_size: Size,
}

/// One mutation in an optimistic, all-or-nothing Interaction Domain transaction.
///
/// Creation and permanent revocation are intentionally separate lifecycle
/// operations because they allocate/destroy protocol globals. Transactions
/// cover the live authority and presentation changes an agent orchestrator
/// commonly needs to commit together.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type", rename_all = "snake_case"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionDomainMutation {
    TransferWindow {
        window: WindowId,
        target: InteractionDomainId,
        retain_source_as_observer: bool,
    },
    SetObserver {
        group: InteractionGroupId,
        interaction_domain: InteractionDomainId,
        observe: bool,
    },
    ConfigureOutput {
        interaction_domain: InteractionDomainId,
        output: VirtualOutput,
    },
    SetState {
        interaction_domain: InteractionDomainId,
        state: InteractionDomainState,
    },
}

/// Per-operation result returned in transaction order.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(tag = "type", rename_all = "snake_case"))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionDomainMutationResult {
    Transferred {
        receipt: AuthorityTransfer,
    },
    ObserverChanged {
        group: InteractionGroupId,
        interaction_domain: InteractionDomainId,
        observe: bool,
        revision: u64,
    },
    OutputConfigured {
        interaction_domain: InteractionDomainId,
        output: VirtualOutput,
        revision: u64,
    },
    StateChanged {
        interaction_domain: InteractionDomainId,
        state: InteractionDomainState,
        revision: u64,
    },
}

/// Receipt for a committed optimistic Interaction Domain transaction.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InteractionDomainTransactionReceipt {
    pub before_revision: u64,
    pub after_revision: u64,
    pub results: Vec<InteractionDomainMutationResult>,
}

/// A rejected model mutation. Every error leaves the model revision and all
/// collections unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionDomainError {
    InvalidId,
    UnknownPrincipal(InteractionPrincipalId),
    UnknownInteractionDomain(InteractionDomainId),
    UnknownSeat(SeatId),
    UnknownClient(ClientId),
    UnknownInteractionGroup(InteractionGroupId),
    UnknownWindow(WindowId),
    DuplicateWindow(WindowId),
    EmptyInteractionGroup,
    SeatNameInUse(String),
    PrincipalDoesNotControlInteractionDomain {
        principal: InteractionPrincipalId,
        interaction_domain: InteractionDomainId,
    },
    InteractionDomainNotActive(InteractionDomainId),
    ControlInteractionDomainCannotObserve(InteractionDomainId),
    AlreadyControlledBy(InteractionDomainId),
    CannotRevokeHumanInteractionDomain,
    InvalidFallbackInteractionDomain(InteractionDomainId),
    InteractionDomainHasNoVirtualOutput(InteractionDomainId),
    InvalidVirtualOutput,
    RevisionConflict {
        expected: u64,
        actual: u64,
    },
    EmptyTransaction,
    TransactionTooLarge,
    InvalidTransactionalState(InteractionDomainState),
}

impl fmt::Display for InteractionDomainError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            InteractionDomainError::InvalidId => write!(f, "identifier zero is reserved"),
            InteractionDomainError::UnknownPrincipal(id) => write!(f, "unknown principal {}", id.0),
            InteractionDomainError::UnknownInteractionDomain(id) => {
                write!(f, "unknown interaction_domain {}", id.0)
            }
            InteractionDomainError::UnknownSeat(id) => write!(f, "unknown seat {}", id.0),
            InteractionDomainError::UnknownClient(id) => write!(f, "unknown client {}", id.0),
            InteractionDomainError::UnknownInteractionGroup(id) => {
                write!(f, "unknown interaction group {}", id.0)
            }
            InteractionDomainError::UnknownWindow(id) => write!(f, "unknown window {}", id.0),
            InteractionDomainError::DuplicateWindow(id) => {
                write!(f, "window {} already belongs to an interaction group", id.0)
            }
            InteractionDomainError::EmptyInteractionGroup => {
                write!(f, "an interaction group must contain at least one window")
            }
            InteractionDomainError::SeatNameInUse(name) => {
                write!(f, "seat name {name:?} is already in use")
            }
            InteractionDomainError::PrincipalDoesNotControlInteractionDomain {
                principal,
                interaction_domain,
            } => write!(
                f,
                "principal {} does not control interaction_domain {}",
                principal.0, interaction_domain.0
            ),
            InteractionDomainError::InteractionDomainNotActive(id) => {
                write!(f, "interaction_domain {} is not active", id.0)
            }
            InteractionDomainError::ControlInteractionDomainCannotObserve(id) => {
                write!(
                    f,
                    "control interaction_domain {} cannot also be an observer",
                    id.0
                )
            }
            InteractionDomainError::AlreadyControlledBy(id) => {
                write!(
                    f,
                    "interaction group is already controlled by interaction_domain {}",
                    id.0
                )
            }
            InteractionDomainError::CannotRevokeHumanInteractionDomain => {
                write!(f, "the human interaction_domain cannot be revoked")
            }
            InteractionDomainError::InvalidFallbackInteractionDomain(id) => {
                write!(
                    f,
                    "interaction_domain {} is not a valid revocation fallback",
                    id.0
                )
            }
            InteractionDomainError::InteractionDomainHasNoVirtualOutput(id) => {
                write!(f, "interaction_domain {} has no virtual output", id.0)
            }
            InteractionDomainError::InvalidVirtualOutput => {
                write!(f, "virtual output parameters are invalid")
            }
            InteractionDomainError::RevisionConflict { expected, actual } => write!(
                f,
                "interaction_domain revision conflict: expected {expected}, current revision is {actual}"
            ),
            InteractionDomainError::EmptyTransaction => {
                write!(f, "interaction_domain transaction is empty")
            }
            InteractionDomainError::TransactionTooLarge => {
                write!(
                    f,
                    "interaction_domain transaction exceeds the 64-operation limit"
                )
            }
            InteractionDomainError::InvalidTransactionalState(state) => {
                write!(f, "interaction_domain state {state:?} is not transactional")
            }
        }
    }
}

impl std::error::Error for InteractionDomainError {}

/// Pure authority model. Identifiers are monotonically allocated and never
/// reused within the compositor lifetime.
#[derive(Debug, Clone)]
pub struct InteractionDomainModel {
    revision: u64,
    principals: BTreeMap<InteractionPrincipalId, Principal>,
    interaction_domains: BTreeMap<InteractionDomainId, InteractionDomain>,
    seats: BTreeMap<SeatId, Seat>,
    clients: BTreeMap<ClientId, Client>,
    interaction_groups: BTreeMap<InteractionGroupId, InteractionGroup>,
    window_groups: BTreeMap<WindowId, InteractionGroupId>,
    next_principal_id: u64,
    next_interaction_domain_id: u64,
    next_seat_id: u64,
    next_client_id: u64,
    next_group_id: u64,
}

impl Default for InteractionDomainModel {
    fn default() -> Self {
        Self::new()
    }
}

impl InteractionDomainModel {
    /// Bootstrap a compositor with the physical human principal, interaction domain, and
    /// pointer/keyboard seat. The initial snapshot is revision 1.
    pub fn new() -> Self {
        let human = Principal {
            id: HUMAN_PRINCIPAL,
            kind: PrincipalKind::Human,
            label: "Human".into(),
            subject: None,
        };
        let interaction_domain = InteractionDomain {
            id: HUMAN_INTERACTION_DOMAIN,
            kind: InteractionDomainKind::Human,
            label: "Desktop".into(),
            controller: HUMAN_PRINCIPAL,
            state: InteractionDomainState::Active,
            presentation: PresentationTarget::Physical,
        };
        let seat = Seat {
            id: HUMAN_SEAT,
            name: "human".into(),
            principal: HUMAN_PRINCIPAL,
            interaction_domain: HUMAN_INTERACTION_DOMAIN,
            capabilities: SeatCapabilities::ALL,
            enabled: true,
        };
        Self {
            revision: 1,
            principals: BTreeMap::from([(HUMAN_PRINCIPAL, human)]),
            interaction_domains: BTreeMap::from([(HUMAN_INTERACTION_DOMAIN, interaction_domain)]),
            seats: BTreeMap::from([(HUMAN_SEAT, seat)]),
            clients: BTreeMap::new(),
            interaction_groups: BTreeMap::new(),
            window_groups: BTreeMap::new(),
            next_principal_id: 2,
            next_interaction_domain_id: 2,
            next_seat_id: 2,
            next_client_id: 1,
            next_group_id: 1,
        }
    }

    pub fn revision(&self) -> u64 {
        self.revision
    }

    pub fn principal(&self, id: InteractionPrincipalId) -> Option<&Principal> {
        self.principals.get(&id)
    }

    pub fn interaction_domain(&self, id: InteractionDomainId) -> Option<&InteractionDomain> {
        self.interaction_domains.get(&id)
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

    /// Create a principal, an active agent interaction domain, and its seat as one model
    /// revision. No partially created authority can become externally visible.
    pub fn create_agent_interaction_domain(
        &mut self,
        label: impl Into<String>,
        capabilities: SeatCapabilities,
    ) -> InteractionDomainBundle {
        self.create_agent_interaction_domain_for_subject(label, capabilities, None)
    }

    /// Create an agent Interaction Domain owned by an authenticated registry subject.
    ///
    /// The subject is distinct from the cosmetic Interaction Domain label and is copied
    /// into the Interaction Domain's controlling principal as part of the same model
    /// revision. It is never accepted from an untrusted wire field; the IPC
    /// server supplies it from the credential-bound connection context.
    pub fn create_agent_interaction_domain_for_subject(
        &mut self,
        label: impl Into<String>,
        capabilities: SeatCapabilities,
        subject: Option<String>,
    ) -> InteractionDomainBundle {
        let label = label.into();
        let principal = self.alloc_principal();
        let interaction_domain = self.alloc_interaction_domain();
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
        self.interaction_domains.insert(
            interaction_domain,
            InteractionDomain {
                id: interaction_domain,
                kind: InteractionDomainKind::Agent,
                label,
                controller: principal,
                state: InteractionDomainState::Active,
                presentation: PresentationTarget::Virtual {
                    output: VirtualOutput::DEFAULT_AGENT,
                },
            },
        );
        self.seats.insert(
            seat,
            Seat {
                id: seat,
                name: format!("agent-{}", interaction_domain.0),
                principal,
                interaction_domain,
                capabilities,
                enabled: true,
            },
        );
        let revision = self.bump_revision();
        InteractionDomainBundle {
            principal,
            interaction_domain,
            seat,
            revision,
        }
    }

    pub fn create_principal(
        &mut self,
        kind: PrincipalKind,
        label: impl Into<String>,
    ) -> InteractionPrincipalId {
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

    pub fn create_interaction_domain(
        &mut self,
        kind: InteractionDomainKind,
        label: impl Into<String>,
        controller: InteractionPrincipalId,
    ) -> Result<InteractionDomainId, InteractionDomainError> {
        if !controller.is_valid() {
            return Err(InteractionDomainError::InvalidId);
        }
        if !self.principals.contains_key(&controller) {
            return Err(InteractionDomainError::UnknownPrincipal(controller));
        }
        let id = self.alloc_interaction_domain();
        self.interaction_domains.insert(
            id,
            InteractionDomain {
                id,
                kind,
                label: label.into(),
                controller,
                state: InteractionDomainState::Active,
                presentation: match kind {
                    InteractionDomainKind::Human => PresentationTarget::Physical,
                    InteractionDomainKind::Agent => PresentationTarget::Virtual {
                        output: VirtualOutput::DEFAULT_AGENT,
                    },
                    InteractionDomainKind::Secure => PresentationTarget::Secure,
                },
            },
        );
        self.bump_revision();
        Ok(id)
    }

    pub fn create_seat(
        &mut self,
        name: impl Into<String>,
        principal: InteractionPrincipalId,
        interaction_domain: InteractionDomainId,
        capabilities: SeatCapabilities,
    ) -> Result<SeatId, InteractionDomainError> {
        let name = name.into();
        if self.seats.values().any(|seat| seat.name == name) {
            return Err(InteractionDomainError::SeatNameInUse(name));
        }
        let target = self.interaction_domains.get(&interaction_domain).ok_or(
            InteractionDomainError::UnknownInteractionDomain(interaction_domain),
        )?;
        if target.state != InteractionDomainState::Active {
            return Err(InteractionDomainError::InteractionDomainNotActive(
                interaction_domain,
            ));
        }
        if !self.principals.contains_key(&principal) {
            return Err(InteractionDomainError::UnknownPrincipal(principal));
        }
        if target.controller != principal {
            return Err(
                InteractionDomainError::PrincipalDoesNotControlInteractionDomain {
                    principal,
                    interaction_domain,
                },
            );
        }
        let id = self.alloc_seat();
        self.seats.insert(
            id,
            Seat {
                id,
                name,
                principal,
                interaction_domain,
                capabilities,
                enabled: true,
            },
        );
        self.bump_revision();
        Ok(id)
    }

    pub fn configure_virtual_output(
        &mut self,
        interaction_domain: InteractionDomainId,
        output: VirtualOutput,
    ) -> Result<(), InteractionDomainError> {
        if !output.validate() {
            return Err(InteractionDomainError::InvalidVirtualOutput);
        }
        let target = self
            .interaction_domains
            .get_mut(&interaction_domain)
            .ok_or(InteractionDomainError::UnknownInteractionDomain(
                interaction_domain,
            ))?;
        if !matches!(target.presentation, PresentationTarget::Virtual { .. }) {
            return Err(InteractionDomainError::InteractionDomainHasNoVirtualOutput(
                interaction_domain,
            ));
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
        mutations: &[InteractionDomainMutation],
    ) -> Result<InteractionDomainTransactionReceipt, InteractionDomainError> {
        if let Some(expected) = expected_revision
            && expected != self.revision
        {
            return Err(InteractionDomainError::RevisionConflict {
                expected,
                actual: self.revision,
            });
        }
        if mutations.is_empty() {
            return Err(InteractionDomainError::EmptyTransaction);
        }
        if mutations.len() > 64 {
            return Err(InteractionDomainError::TransactionTooLarge);
        }

        let before_revision = self.revision;
        let mut staged = self.clone();
        let mut results = Vec::with_capacity(mutations.len());
        for mutation in mutations {
            let result = match *mutation {
                InteractionDomainMutation::TransferWindow {
                    window,
                    target,
                    retain_source_as_observer,
                } => {
                    let group = staged
                        .interaction_group_for_window(window)
                        .map(|group| group.id)
                        .ok_or(InteractionDomainError::UnknownWindow(window))?;
                    InteractionDomainMutationResult::Transferred {
                        receipt: staged.transfer_control(
                            group,
                            target,
                            TransferOptions {
                                retain_source_as_observer,
                            },
                        )?,
                    }
                }
                InteractionDomainMutation::SetObserver {
                    group,
                    interaction_domain,
                    observe,
                } => {
                    staged.set_observer(group, interaction_domain, observe)?;
                    InteractionDomainMutationResult::ObserverChanged {
                        group,
                        interaction_domain,
                        observe,
                        revision: staged.revision,
                    }
                }
                InteractionDomainMutation::ConfigureOutput {
                    interaction_domain,
                    output,
                } => {
                    staged.configure_virtual_output(interaction_domain, output)?;
                    InteractionDomainMutationResult::OutputConfigured {
                        interaction_domain,
                        output,
                        revision: staged.revision,
                    }
                }
                InteractionDomainMutation::SetState {
                    interaction_domain,
                    state,
                } => {
                    match state {
                        InteractionDomainState::Active => {
                            staged.resume_interaction_domain(interaction_domain)?
                        }
                        InteractionDomainState::Paused => {
                            staged.pause_interaction_domain(interaction_domain)?
                        }
                        InteractionDomainState::Revoked => {
                            return Err(InteractionDomainError::InvalidTransactionalState(state));
                        }
                    }
                    InteractionDomainMutationResult::StateChanged {
                        interaction_domain,
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
        Ok(InteractionDomainTransactionReceipt {
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
    pub fn disconnect_client(&mut self, client: ClientId) -> Result<(), InteractionDomainError> {
        let record = self
            .clients
            .get_mut(&client)
            .ok_or(InteractionDomainError::UnknownClient(client))?;
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
    ) -> Result<(), InteractionDomainError> {
        let record = self
            .clients
            .get_mut(&client)
            .ok_or(InteractionDomainError::UnknownClient(client))?;
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
        control_interaction_domain: InteractionDomainId,
    ) -> Result<InteractionGroupId, InteractionDomainError> {
        if !self.clients.contains_key(&client) {
            return Err(InteractionDomainError::UnknownClient(client));
        }
        if !self
            .clients
            .get(&client)
            .is_some_and(|client| client.connected)
        {
            return Err(InteractionDomainError::UnknownClient(client));
        }
        self.require_active_interaction_domain(control_interaction_domain)?;
        if windows.is_empty() {
            return Err(InteractionDomainError::EmptyInteractionGroup);
        }
        let mut members = BTreeSet::new();
        for &window in windows {
            if window.0 == 0 {
                return Err(InteractionDomainError::InvalidId);
            }
            if self.window_groups.contains_key(&window) || !members.insert(window) {
                return Err(InteractionDomainError::DuplicateWindow(window));
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
                control_interaction_domain,
                observer_interaction_domains: BTreeSet::new(),
            },
        );
        self.bump_revision();
        Ok(id)
    }

    pub fn add_window_to_group(
        &mut self,
        group: InteractionGroupId,
        window: WindowId,
    ) -> Result<(), InteractionDomainError> {
        if window.0 == 0 {
            return Err(InteractionDomainError::InvalidId);
        }
        if self.window_groups.contains_key(&window) {
            return Err(InteractionDomainError::DuplicateWindow(window));
        }
        let target = self
            .interaction_groups
            .get_mut(&group)
            .ok_or(InteractionDomainError::UnknownInteractionGroup(group))?;
        target.windows.insert(window);
        self.window_groups.insert(window, group);
        self.bump_revision();
        Ok(())
    }

    /// Remove a retired window. The group is removed when its last toplevel
    /// disappears. Identifiers remain consumed and are never reused.
    pub fn remove_window(&mut self, window: WindowId) -> Result<(), InteractionDomainError> {
        let group = self
            .window_groups
            .remove(&window)
            .ok_or(InteractionDomainError::UnknownWindow(window))?;
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
    /// controlling interaction domain because that would blur the input boundary.
    pub fn set_observer(
        &mut self,
        group: InteractionGroupId,
        interaction_domain: InteractionDomainId,
        observe: bool,
    ) -> Result<(), InteractionDomainError> {
        let interaction_domain_state = self
            .interaction_domains
            .get(&interaction_domain)
            .ok_or(InteractionDomainError::UnknownInteractionDomain(
                interaction_domain,
            ))?
            .state;
        if interaction_domain_state == InteractionDomainState::Revoked {
            return Err(InteractionDomainError::InteractionDomainNotActive(
                interaction_domain,
            ));
        }
        let target = self
            .interaction_groups
            .get_mut(&group)
            .ok_or(InteractionDomainError::UnknownInteractionGroup(group))?;
        if observe && target.control_interaction_domain == interaction_domain {
            return Err(
                InteractionDomainError::ControlInteractionDomainCannotObserve(interaction_domain),
            );
        }
        let changed = if observe {
            target
                .observer_interaction_domains
                .insert(interaction_domain)
        } else {
            target
                .observer_interaction_domains
                .remove(&interaction_domain)
        };
        if changed {
            self.bump_revision();
        }
        Ok(())
    }

    /// Atomically transfer every window in an interaction group to a new
    /// controlling interaction domain.
    pub fn transfer_control(
        &mut self,
        group: InteractionGroupId,
        target_interaction_domain: InteractionDomainId,
        options: TransferOptions,
    ) -> Result<AuthorityTransfer, InteractionDomainError> {
        self.require_active_interaction_domain(target_interaction_domain)?;
        let current = self
            .interaction_groups
            .get(&group)
            .ok_or(InteractionDomainError::UnknownInteractionGroup(group))?;
        let source_interaction_domain = current.control_interaction_domain;
        if source_interaction_domain == target_interaction_domain {
            return Err(InteractionDomainError::AlreadyControlledBy(
                target_interaction_domain,
            ));
        }
        let windows = current.windows.iter().copied().collect::<Vec<_>>();
        let target = self
            .interaction_groups
            .get_mut(&group)
            .expect("validated interaction group disappeared");
        target.control_interaction_domain = target_interaction_domain;
        target
            .observer_interaction_domains
            .remove(&target_interaction_domain);
        let source_retained_as_observer = if options.retain_source_as_observer {
            target
                .observer_interaction_domains
                .insert(source_interaction_domain);
            true
        } else {
            target
                .observer_interaction_domains
                .remove(&source_interaction_domain);
            false
        };
        let revision = self.bump_revision();
        Ok(AuthorityTransfer {
            group,
            windows,
            from: source_interaction_domain,
            to: target_interaction_domain,
            source_retained_as_observer,
            revision,
        })
    }

    /// Pause an interaction domain and disable all seats attached to it.
    pub fn pause_interaction_domain(
        &mut self,
        interaction_domain: InteractionDomainId,
    ) -> Result<(), InteractionDomainError> {
        self.set_interaction_domain_running_state(
            interaction_domain,
            InteractionDomainState::Paused,
            false,
        )
    }

    /// Resume a paused interaction domain and re-enable its seats.
    pub fn resume_interaction_domain(
        &mut self,
        interaction_domain: InteractionDomainId,
    ) -> Result<(), InteractionDomainError> {
        let current = self
            .interaction_domains
            .get(&interaction_domain)
            .ok_or(InteractionDomainError::UnknownInteractionDomain(
                interaction_domain,
            ))?
            .state;
        if current == InteractionDomainState::Revoked {
            return Err(InteractionDomainError::InteractionDomainNotActive(
                interaction_domain,
            ));
        }
        self.set_interaction_domain_running_state(
            interaction_domain,
            InteractionDomainState::Active,
            true,
        )
    }

    /// Permanently revoke a non-human interaction domain. Controlled groups move to an
    /// active fallback in the same revision, all mirrors into the revoked
    /// interaction domain disappear, and its seats are disabled.
    pub fn revoke_interaction_domain(
        &mut self,
        interaction_domain: InteractionDomainId,
        fallback: InteractionDomainId,
    ) -> Result<InteractionDomainRevocation, InteractionDomainError> {
        if interaction_domain == HUMAN_INTERACTION_DOMAIN {
            return Err(InteractionDomainError::CannotRevokeHumanInteractionDomain);
        }
        if interaction_domain == fallback {
            return Err(InteractionDomainError::InvalidFallbackInteractionDomain(
                fallback,
            ));
        }
        if !self.interaction_domains.contains_key(&interaction_domain) {
            return Err(InteractionDomainError::UnknownInteractionDomain(
                interaction_domain,
            ));
        }
        if self
            .interaction_domains
            .get(&interaction_domain)
            .is_some_and(|record| record.state == InteractionDomainState::Revoked)
        {
            return Err(InteractionDomainError::InteractionDomainNotActive(
                interaction_domain,
            ));
        }
        self.require_active_interaction_domain(fallback)
            .map_err(|_| InteractionDomainError::InvalidFallbackInteractionDomain(fallback))?;

        let transferred_groups = self
            .interaction_groups
            .values()
            .filter(|group| group.control_interaction_domain == interaction_domain)
            .map(|group| group.id)
            .collect::<Vec<_>>();
        for group in self.interaction_groups.values_mut() {
            group
                .observer_interaction_domains
                .remove(&interaction_domain);
            if group.control_interaction_domain == interaction_domain {
                group.control_interaction_domain = fallback;
                group.observer_interaction_domains.remove(&fallback);
            }
        }
        self.interaction_domains
            .get_mut(&interaction_domain)
            .expect("validated interaction_domain disappeared")
            .state = InteractionDomainState::Revoked;
        for seat in self
            .seats
            .values_mut()
            .filter(|seat| seat.interaction_domain == interaction_domain)
        {
            seat.enabled = false;
        }
        let revision = self.bump_revision();
        Ok(InteractionDomainRevocation {
            interaction_domain,
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
                .interaction_domains
                .get(&seat.interaction_domain)
                .is_none_or(|interaction_domain| {
                    interaction_domain.state != InteractionDomainState::Active
                })
        {
            return false;
        }
        self.interaction_group_for_window(window)
            .is_some_and(|group| group.control_interaction_domain == seat.interaction_domain)
    }

    /// Whether an interaction domain may render a window, either as controller or observer.
    pub fn interaction_domain_observes_window(
        &self,
        interaction_domain: InteractionDomainId,
        window: WindowId,
    ) -> bool {
        self.interaction_group_for_window(window)
            .is_some_and(|group| {
                group.control_interaction_domain == interaction_domain
                    || group
                        .observer_interaction_domains
                        .contains(&interaction_domain)
            })
    }

    pub fn snapshot(&self) -> InteractionDomainSnapshot {
        // Durable client records accumulate for the process lifetime (audit
        // identity), but a snapshot is the live published view: disconnected
        // clients with no remaining interaction groups contribute nothing to
        // authorization decisions and are excluded so per-frame fan-out cost
        // does not grow with session window churn. The IPC scope filter keeps
        // only group-referenced clients anyway.
        let group_clients: BTreeSet<ClientId> = self
            .interaction_groups
            .values()
            .map(|group| group.client)
            .collect();
        let clients = self
            .clients
            .values()
            .filter(|client| client.connected || group_clients.contains(&client.id))
            .cloned()
            .collect();
        InteractionDomainSnapshot {
            revision: self.revision,
            principals: self.principals.values().cloned().collect(),
            interaction_domains: self.interaction_domains.values().cloned().collect(),
            seats: self.seats.values().cloned().collect(),
            clients,
            interaction_groups: self.interaction_groups.values().cloned().collect(),
        }
    }

    /// Validate the complete live model. Intended for tests, debug assertions,
    /// and production health checks.
    pub fn validate(&self) -> Result<(), InteractionDomainError> {
        let mut names = BTreeSet::new();
        for seat in self.seats.values() {
            if !names.insert(seat.name.as_str()) {
                return Err(InteractionDomainError::SeatNameInUse(seat.name.clone()));
            }
            let interaction_domain = self
                .interaction_domains
                .get(&seat.interaction_domain)
                .ok_or(InteractionDomainError::UnknownInteractionDomain(
                    seat.interaction_domain,
                ))?;
            if interaction_domain.controller != seat.principal {
                return Err(
                    InteractionDomainError::PrincipalDoesNotControlInteractionDomain {
                        principal: seat.principal,
                        interaction_domain: seat.interaction_domain,
                    },
                );
            }
            if !self.principals.contains_key(&seat.principal) {
                return Err(InteractionDomainError::UnknownPrincipal(seat.principal));
            }
        }
        let mut seen_windows = BTreeSet::new();
        for group in self.interaction_groups.values() {
            if group.windows.is_empty() {
                return Err(InteractionDomainError::EmptyInteractionGroup);
            }
            if !self.clients.contains_key(&group.client) {
                return Err(InteractionDomainError::UnknownClient(group.client));
            }
            if !self
                .interaction_domains
                .contains_key(&group.control_interaction_domain)
            {
                return Err(InteractionDomainError::UnknownInteractionDomain(
                    group.control_interaction_domain,
                ));
            }
            if group
                .observer_interaction_domains
                .contains(&group.control_interaction_domain)
            {
                return Err(
                    InteractionDomainError::ControlInteractionDomainCannotObserve(
                        group.control_interaction_domain,
                    ),
                );
            }
            for observer in &group.observer_interaction_domains {
                if !self.interaction_domains.contains_key(observer) {
                    return Err(InteractionDomainError::UnknownInteractionDomain(*observer));
                }
            }
            for &window in &group.windows {
                if !seen_windows.insert(window) {
                    return Err(InteractionDomainError::DuplicateWindow(window));
                }
                if self.window_groups.get(&window) != Some(&group.id) {
                    return Err(InteractionDomainError::UnknownWindow(window));
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
            return Err(InteractionDomainError::UnknownWindow(unknown));
        }
        Ok(())
    }

    fn set_interaction_domain_running_state(
        &mut self,
        interaction_domain: InteractionDomainId,
        state: InteractionDomainState,
        seats_enabled: bool,
    ) -> Result<(), InteractionDomainError> {
        let target = self
            .interaction_domains
            .get_mut(&interaction_domain)
            .ok_or(InteractionDomainError::UnknownInteractionDomain(
                interaction_domain,
            ))?;
        if target.state == InteractionDomainState::Revoked {
            return Err(InteractionDomainError::InteractionDomainNotActive(
                interaction_domain,
            ));
        }
        let mut changed = target.state != state;
        target.state = state;
        for seat in self
            .seats
            .values_mut()
            .filter(|seat| seat.interaction_domain == interaction_domain)
        {
            changed |= seat.enabled != seats_enabled;
            seat.enabled = seats_enabled;
        }
        if changed {
            self.bump_revision();
        }
        Ok(())
    }

    fn require_active_interaction_domain(
        &self,
        id: InteractionDomainId,
    ) -> Result<(), InteractionDomainError> {
        let interaction_domain = self
            .interaction_domains
            .get(&id)
            .ok_or(InteractionDomainError::UnknownInteractionDomain(id))?;
        if interaction_domain.state != InteractionDomainState::Active {
            return Err(InteractionDomainError::InteractionDomainNotActive(id));
        }
        Ok(())
    }

    fn bump_revision(&mut self) -> u64 {
        self.revision = self
            .revision
            .checked_add(1)
            .expect("interaction_domain model revision exhausted");
        self.revision
    }

    fn alloc_principal(&mut self) -> InteractionPrincipalId {
        let id = InteractionPrincipalId(self.next_principal_id);
        self.next_principal_id = self
            .next_principal_id
            .checked_add(1)
            .expect("principal id exhausted");
        id
    }

    fn alloc_interaction_domain(&mut self) -> InteractionDomainId {
        let id = InteractionDomainId(self.next_interaction_domain_id);
        self.next_interaction_domain_id = self
            .next_interaction_domain_id
            .checked_add(1)
            .expect("interaction_domain id exhausted");
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

    fn model_with_window() -> (InteractionDomainModel, ClientId, InteractionGroupId) {
        let mut model = InteractionDomainModel::new();
        let client = model.register_client(Some("test.client".into()));
        let group = model
            .create_interaction_group(client, &[WindowId(10)], HUMAN_INTERACTION_DOMAIN)
            .unwrap();
        (model, client, group)
    }

    #[test]
    fn bootstrap_has_one_active_human_authority_chain() {
        let model = InteractionDomainModel::new();
        assert_eq!(model.revision(), 1);
        assert_eq!(
            model
                .interaction_domain(HUMAN_INTERACTION_DOMAIN)
                .map(|interaction_domain| interaction_domain.controller),
            Some(HUMAN_PRINCIPAL)
        );
        assert_eq!(
            model
                .seat(HUMAN_SEAT)
                .map(|seat| (seat.interaction_domain, seat.enabled)),
            Some((HUMAN_INTERACTION_DOMAIN, true))
        );
        assert!(model.validate().is_ok());
    }

    #[test]
    fn agent_bundle_is_created_in_one_revision() {
        let mut model = InteractionDomainModel::new();
        let bundle =
            model.create_agent_interaction_domain("Research", SeatCapabilities::POINTER_KEYBOARD);
        assert_eq!(bundle.revision, 2);
        assert_eq!(
            model
                .interaction_domain(bundle.interaction_domain)
                .map(|interaction_domain| interaction_domain.controller),
            Some(bundle.principal)
        );
        assert_eq!(
            model.seat(bundle.seat).map(|seat| seat.interaction_domain),
            Some(bundle.interaction_domain)
        );
        assert!(model.validate().is_ok());
    }

    #[test]
    fn authenticated_subject_is_bound_to_the_controlling_principal() {
        let mut model = InteractionDomainModel::new();
        let bundle = model.create_agent_interaction_domain_for_subject(
            "Agent",
            SeatCapabilities::POINTER_KEYBOARD,
            Some("prin_test".into()),
        );
        let interaction_domain = model
            .interaction_domain(bundle.interaction_domain)
            .expect("interaction_domain");
        let principal = model
            .principal(interaction_domain.controller)
            .expect("controller");
        assert_eq!(principal.subject.as_deref(), Some("prin_test"));
        assert_eq!(
            model.snapshot().principals[1].subject.as_deref(),
            Some("prin_test")
        );
    }

    #[test]
    fn transfer_moves_control_and_retains_read_only_human_observation() {
        let (mut model, _, group) = model_with_window();
        let agent = model.create_agent_interaction_domain("Agent", SeatCapabilities::default());
        let receipt = model
            .transfer_control(group, agent.interaction_domain, TransferOptions::default())
            .unwrap();
        assert_eq!(receipt.from, HUMAN_INTERACTION_DOMAIN);
        assert_eq!(receipt.to, agent.interaction_domain);
        assert_eq!(receipt.windows, vec![WindowId(10)]);
        assert!(model.seat_controls_window(agent.seat, WindowId(10)));
        assert!(!model.seat_controls_window(HUMAN_SEAT, WindowId(10)));
        assert!(model.interaction_domain_observes_window(HUMAN_INTERACTION_DOMAIN, WindowId(10)));
        assert!(model.interaction_domain_observes_window(agent.interaction_domain, WindowId(10)));
        assert!(model.validate().is_ok());
    }

    #[test]
    fn interaction_group_transfers_all_member_windows_atomically() {
        let mut model = InteractionDomainModel::new();
        let client = model.register_client(None);
        let group = model
            .create_interaction_group(
                client,
                &[WindowId(3), WindowId(4), WindowId(5)],
                HUMAN_INTERACTION_DOMAIN,
            )
            .unwrap();
        let agent = model.create_agent_interaction_domain("Agent", SeatCapabilities::default());
        let receipt = model
            .transfer_control(group, agent.interaction_domain, TransferOptions::default())
            .unwrap();
        assert_eq!(receipt.windows, vec![WindowId(3), WindowId(4), WindowId(5)]);
        for id in receipt.windows {
            assert!(model.seat_controls_window(agent.seat, id));
        }
    }

    #[test]
    fn rejected_transfer_is_atomic() {
        let (mut model, _, group) = model_with_window();
        let agent = model.create_agent_interaction_domain("Agent", SeatCapabilities::default());
        model
            .pause_interaction_domain(agent.interaction_domain)
            .unwrap();
        let before = model.snapshot();
        assert_eq!(
            model.transfer_control(group, agent.interaction_domain, TransferOptions::default()),
            Err(InteractionDomainError::InteractionDomainNotActive(
                agent.interaction_domain
            ))
        );
        assert_eq!(model.snapshot(), before);
    }

    #[test]
    fn paused_interaction_domain_cannot_control_until_resumed() {
        let (mut model, _, group) = model_with_window();
        let agent = model.create_agent_interaction_domain("Agent", SeatCapabilities::default());
        model
            .transfer_control(group, agent.interaction_domain, TransferOptions::default())
            .unwrap();
        model
            .pause_interaction_domain(agent.interaction_domain)
            .unwrap();
        assert!(!model.seat_controls_window(agent.seat, WindowId(10)));
        assert!(!model.seat(agent.seat).unwrap().enabled);
        model
            .resume_interaction_domain(agent.interaction_domain)
            .unwrap();
        assert!(model.seat_controls_window(agent.seat, WindowId(10)));
    }

    #[test]
    fn revocation_drains_control_and_removes_observation() {
        let (mut model, _, group) = model_with_window();
        let agent = model.create_agent_interaction_domain("Agent", SeatCapabilities::default());
        model
            .transfer_control(group, agent.interaction_domain, TransferOptions::default())
            .unwrap();
        let second_client = model.register_client(None);
        let second_group = model
            .create_interaction_group(second_client, &[WindowId(20)], HUMAN_INTERACTION_DOMAIN)
            .unwrap();
        model
            .set_observer(second_group, agent.interaction_domain, true)
            .unwrap();

        let receipt = model
            .revoke_interaction_domain(agent.interaction_domain, HUMAN_INTERACTION_DOMAIN)
            .unwrap();
        assert_eq!(receipt.transferred_groups, vec![group]);
        assert!(model.seat_controls_window(HUMAN_SEAT, WindowId(10)));
        assert!(!model.seat(agent.seat).unwrap().enabled);
        assert!(!model.interaction_domain_observes_window(agent.interaction_domain, WindowId(20)));
        assert_eq!(
            model
                .interaction_domain(agent.interaction_domain)
                .map(|interaction_domain| interaction_domain.state),
            Some(InteractionDomainState::Revoked)
        );
        assert!(model.validate().is_ok());
    }

    #[test]
    fn window_can_belong_to_exactly_one_interaction_group() {
        let (mut model, client, _) = model_with_window();
        let before = model.snapshot();
        assert_eq!(
            model.create_interaction_group(client, &[WindowId(10)], HUMAN_INTERACTION_DOMAIN),
            Err(InteractionDomainError::DuplicateWindow(WindowId(10)))
        );
        assert_eq!(model.snapshot(), before);
    }

    #[test]
    fn seat_names_are_unique_and_controller_must_match() {
        let mut model = InteractionDomainModel::new();
        let agent = model.create_agent_interaction_domain("Agent", SeatCapabilities::default());
        let before = model.snapshot();
        assert_eq!(
            model.create_seat(
                "human",
                agent.principal,
                agent.interaction_domain,
                SeatCapabilities::default()
            ),
            Err(InteractionDomainError::SeatNameInUse("human".into()))
        );
        assert_eq!(model.snapshot(), before);

        assert_eq!(
            model.create_seat(
                "wrong-controller",
                HUMAN_PRINCIPAL,
                agent.interaction_domain,
                SeatCapabilities::default()
            ),
            Err(
                InteractionDomainError::PrincipalDoesNotControlInteractionDomain {
                    principal: HUMAN_PRINCIPAL,
                    interaction_domain: agent.interaction_domain,
                }
            )
        );
    }

    #[test]
    fn retired_identifiers_are_not_reused() {
        let (mut model, _, first_group) = model_with_window();
        model.remove_window(WindowId(10)).unwrap();
        let client = model.register_client(None);
        let second_group = model
            .create_interaction_group(client, &[WindowId(11)], HUMAN_INTERACTION_DOMAIN)
            .unwrap();
        assert!(second_group > first_group);
    }

    #[test]
    fn transaction_commits_transfer_output_and_pause_together() {
        let (mut model, _, _) = model_with_window();
        let agent = model.create_agent_interaction_domain("Agent", SeatCapabilities::ALL);
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
                    InteractionDomainMutation::TransferWindow {
                        window: WindowId(10),
                        target: agent.interaction_domain,
                        retain_source_as_observer: true,
                    },
                    InteractionDomainMutation::ConfigureOutput {
                        interaction_domain: agent.interaction_domain,
                        output,
                    },
                    InteractionDomainMutation::SetState {
                        interaction_domain: agent.interaction_domain,
                        state: InteractionDomainState::Paused,
                    },
                ],
            )
            .unwrap();
        assert_eq!(receipt.before_revision, before);
        assert_eq!(receipt.results.len(), 3);
        assert_eq!(
            model
                .interaction_domain(agent.interaction_domain)
                .map(|interaction_domain| interaction_domain.presentation),
            Some(PresentationTarget::Virtual { output })
        );
        assert!(!model.seat_controls_window(agent.seat, WindowId(10)));
        assert!(model.interaction_domain_observes_window(HUMAN_INTERACTION_DOMAIN, WindowId(10)));
        assert!(model.validate().is_ok());
    }

    #[test]
    fn failed_transaction_and_revision_conflict_leave_snapshot_unchanged() {
        let (mut model, _, _) = model_with_window();
        let agent = model.create_agent_interaction_domain("Agent", SeatCapabilities::ALL);
        let before = model.snapshot();
        assert_eq!(
            model.transact(
                Some(before.revision),
                &[
                    InteractionDomainMutation::TransferWindow {
                        window: WindowId(10),
                        target: agent.interaction_domain,
                        retain_source_as_observer: true,
                    },
                    InteractionDomainMutation::ConfigureOutput {
                        interaction_domain: HUMAN_INTERACTION_DOMAIN,
                        output: VirtualOutput::DEFAULT_AGENT,
                    },
                ],
            ),
            Err(InteractionDomainError::InteractionDomainHasNoVirtualOutput(
                HUMAN_INTERACTION_DOMAIN
            ))
        );
        assert_eq!(model.snapshot(), before);
        assert_eq!(
            model.transact(
                Some(before.revision - 1),
                &[InteractionDomainMutation::SetState {
                    interaction_domain: agent.interaction_domain,
                    state: InteractionDomainState::Paused,
                }],
            ),
            Err(InteractionDomainError::RevisionConflict {
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
        let agent = model.create_agent_interaction_domain("Agent", SeatCapabilities::default());
        model
            .transfer_control(group, agent.interaction_domain, TransferOptions::default())
            .unwrap();
        let snapshot = model.snapshot();
        let json = serde_json::to_string(&snapshot).unwrap();
        let decoded: InteractionDomainSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, snapshot);
    }
}
