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
//! Protocol version 2 supports state queries, typed control and session
//! commands, coarse event subscriptions, detailed mutation-journal streams,
//! and configuration-defined agent scopes. The capability handshake limits
//! each connection to its negotiated operations.

pub mod client;
pub mod codec;
pub mod journal;
pub mod schema;
pub mod server;

pub mod base64;

pub use client::Client;
pub use journal::{Effect, Journal, JournalEntry, JournalSnapshot, Origin, DEFAULT_CAPACITY};
pub use schema::{
    Capabilities, Command, Event, OpClass, Request, Response, Scope, PROTOCOL_VERSION,
};
pub use server::{Handler, Server};
