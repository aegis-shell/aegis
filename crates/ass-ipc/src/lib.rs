//! Versioned IPC and introspection surface for ass.
//!
//! A unix-domain socket at `$XDG_RUNTIME_DIR/ass.sock` speaks a length-
//! framed, schema-versioned JSON protocol. It is the sole extension and
//! automation surface: the chrome, external tools, and the later agent layer
//! all read the same `ass_core` model the IPC serves — there is no separate
//! wire DTO and no in-process scripting.
//!
//! The crate is pure: it depends on [`ass_core`] (with its `serde` feature
//! so the `Window` it sends is the same `Window` the renderer reads) and on
//! `serde`/`serde_json`. A loopback server + client exercise the whole path
//! in tests with no Vulkan or Wayland dependency. See
//! [ADR-0027](../../docs/adr/0027-ipc-and-introspection.md).
//!
//! # Status
//!
//! Protocol version 3 supports Realm authority, connection-bound capability
//! leases, optimistic transactions, directed virtual-output capture, state
//! queries, typed commands, and event/journal subscriptions.

pub mod client;
pub mod codec;
pub mod journal;
pub mod schema;
pub mod server;

mod blob;

pub use client::{CapturedRealm, Client};
pub use journal::{
    Effect, Journal, JournalEntry, JournalMutation, JournalSnapshot, Origin, DEFAULT_CAPACITY,
};
pub use schema::{
    Capabilities, Command, Event, LeaseGrant, LeaseRequest, OpClass, RealmAction,
    RealmActionResult, RealmCapture, Request, Response, Scope, LOCAL_REALM_ADMIN_SCOPE,
    PROTOCOL_VERSION,
};
pub use server::{CaptureOutputPayload, CaptureRealmPayload, Handler, Server};
