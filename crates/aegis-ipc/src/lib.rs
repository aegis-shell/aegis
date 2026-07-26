//! Versioned IPC and introspection surface for ass.
//!
//! A unix-domain socket at `$XDG_RUNTIME_DIR/aegis.sock` speaks a length-
//! framed, schema-versioned JSON protocol. It is the sole extension and
//! automation surface: the chrome, external tools, and the later agent layer
//! all read the same `aegis_core` model the IPC serves — there is no separate
//! wire DTO and no in-process scripting.
//!
//! The crate is pure: it depends on [`aegis_core`] (with its `serde` feature
//! so the `Window` it sends is the same `Window` the renderer reads) and on
//! `serde`/`serde_json`. A loopback server + client exercise the whole path
//! in tests with no Vulkan or Wayland dependency. See
//! [ADR-0027](../../docs/adr/0027-ipc-and-introspection.md).
//!
//! # Status
//!
//! Protocol version 7 adds live system-status queries and immediate
//! system-control commands on top of version 6's user-consent interactive
//! picking (`PickTarget`, ADR-0054) and window stream target, version 5's
//! continuous physical-output frame streaming (ADR-0052), and version 4's
//! revisioned desktop-settings transactions.

pub mod client;
pub mod codec;
pub mod journal;
pub mod schema;
pub mod server;

mod blob;

pub use client::{CapturedRealm, Client, StreamFrame, StreamMessage, StreamStarted};
pub use journal::{
    DEFAULT_CAPACITY, Effect, Journal, JournalEntry, JournalMutation, JournalSnapshot, Origin,
};
pub use schema::{
    Capabilities, Command, Event, LOCAL_PORTAL_SCOPE, LOCAL_REALM_ADMIN_SCOPE, LeaseGrant,
    LeaseRequest, OpClass, PROTOCOL_VERSION, PickKind, PickResult, RealmAction, RealmActionResult,
    RealmCapture, Request, Response, Scope, SettingsAction, SettingsReceipt, SettingsSnapshot,
    StreamPixelFormat, StreamTarget, SystemAction, SystemStatus,
};
pub use server::{
    CaptureOutputPayload, CaptureRealmPayload, Handler, Server, StreamFramePayload, StreamInfo,
};
