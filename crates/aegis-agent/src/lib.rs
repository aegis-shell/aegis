//! aegis-agent — the shared agent client layer for the Aegis platform
//! (ADR-0125).
//!
//! Every out-of-process agent that operates the desktop performs the same
//! client discipline: present or pair a credential, split the pairing
//! handshake timeout from per-request I/O, recover per-instance state across
//! crashes, and retain connection-bound observation leases. This crate owns
//! that discipline so each agent surface — the MCP bridge, accessibility
//! adapters, future bridges — keeps only its own protocol translation.
//!
//! The crate is a client library, not a daemon: consumers keep their own
//! processes, supervision, and reconnection policies.

mod connect;
mod identity;
mod interaction_domain;
mod observation;
mod state;

pub use connect::{
    ConnectError, ConnectParams, Connected, CredentialSource, connect, read_credential_from_stdin,
};
pub use identity::{IdentityError, IdentityStore};
pub use interaction_domain::{InteractionDomainSession, ManagedInteractionDomain};
pub use observation::ObservationLeases;
pub use state::{SessionError, scope_key};
