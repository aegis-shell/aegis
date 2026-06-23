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
//! MVP: the `query` capability and a single command, [`Request::GetWindows`].
//! The capability model and handshake are in place so `control` and
//! `session` commands add without changing the wire.
//!
//! [`Request::GetWindows`]: schema::Request::GetWindows

pub mod client;
pub mod codec;
pub mod schema;
pub mod server;

pub use client::Client;
pub use schema::{Capabilities, Command, Event, PROTOCOL_VERSION, Request, Response};
pub use server::{Handler, Server};
