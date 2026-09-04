//! tessera-ipc-client — the rich client library for the Tessera capability
//! broker (ADR-0125).
//!
//! `tessera-ipc` ships the thin transport client (schema, codec, one blocking
//! request/response per call) and stays free of filesystem policy. Every
//! out-of-process consumer of the broker additionally performs the same
//! client discipline, and this crate owns it so each consumer keeps only its
//! own protocol surface:
//!
//! - the pairing handshake discipline: a generous handshake timeout for the
//!   interactive pairing prompt, then a short per-request I/O timeout;
//! - credential policies: a durable, atomically persisted pairing identity
//!   (`IdentityStore`) and launcher-injected ephemeral credentials
//!   (`read_credential_from_stdin`);
//! - per-instance state recovery: a `flock`-held recovery lock and the
//!   managed Interaction Domain record (`InteractionDomainSession`), with
//!   crash recovery proven by the authenticated principal subject;
//! - connection-bound observation lease retention (`ObservationLeases`) for
//!   one-shot-connection consumers;
//! - the connection policies: one-shot connections for
//!   least-privilege-in-time consumers, and `PersistentConnection` for
//!   long-lived consumers — one multiplexed connection with lazy lease
//!   renewal and transparent re-pairing after a transport failure.
//!
//! It is a client library, not a daemon: consumers keep their own processes,
//! supervision, and reconnection policies. Current consumers are `tessera-mcp`
//! (the MCP bridge) and `tessera-atspi` (the accessibility adapter). The
//! compositor never links it.

mod connect;
mod identity;
mod interaction_domain;
mod observation;
mod persistent;
mod state;

pub use connect::{
    ConnectError, ConnectParams, Connected, CredentialSource, connect, read_credential_from_stdin,
};
pub use identity::{IdentityError, IdentityStore};
pub use interaction_domain::{InteractionDomainSession, ManagedInteractionDomain};
pub use observation::ObservationLeases;
pub use persistent::{ClassifyError, PersistentConnection, PersistentError, is_transport_failure};
pub use state::{SessionError, scope_key};
