//! Transport-neutral Actor authority primitives.
//!
//! This crate owns identities bound to live broker sessions, semantic
//! observation leases, and optimistic action-transaction validation. It has
//! no IPC, Wayland, renderer, shell, or agent-runtime dependency. Transport
//! adapters serialize these values; the compositor main loop remains the
//! only place that may commit validated GUI actions.

mod capability;
mod identity;
mod observation;
mod resource;
mod scope;
mod session;

pub use capability::{ActorCapability, AuthorizationDecision};
pub use identity::{ActorPrincipal, AgentIdentity, PairedAgent};
pub use observation::{
    ActorActionIntent, ActorActionReceipt, ActorBinding, ObservationLeaseRegistry,
    ObservationToken, SemanticObservation, ValidatedActorAction,
};
pub use resource::{
    ActorResource, FilesystemAccess, ResourceGrant, ResourceGrantId, ResourceGrantRegistry,
};
pub use scope::ActorScope;
pub use session::{
    ActorSessionId, ActorSessionPolicy, ActorSessionRegistry, ActorSessionSnapshot,
    ActorSessionState,
};
