//! Versioned IPC and introspection surface for aegis.
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
//! Protocol version 12 adds the staged inactivity-policy snapshot and
//! transaction. Earlier settings and live-system revisions are recorded in
//! [`schema::PROTOCOL_VERSION`].

pub mod client;
pub mod codec;
pub mod journal;
pub mod schema;
pub mod server;

mod blob;

pub use client::{CapturedRealm, Client, StreamFrame, StreamMessage, StreamStarted};
pub use journal::{
    AgentAuthAction, DEFAULT_CAPACITY, Effect, GrantPersistence, Journal, JournalEntry,
    JournalMutation, JournalSnapshot, Origin,
};
pub use schema::{
    AgentGrantDecision, AgentGrantInfo, AgentHello, AgentIssued, AgentPrincipalInfo, AppPickResult,
    Capabilities, Command, ConfirmPickResult, Event, FileFilter, FilePickMode, FilePickOptions,
    FilePickResult, LOCAL_AGENT_ADMIN_SCOPE, LOCAL_OWNER_ADMIN_SCOPE, LOCAL_PORTAL_SCOPE,
    LOCAL_REALM_ADMIN_SCOPE, LeaseGrant, LeaseRequest, OpClass, PROTOCOL_VERSION, PickKind,
    PickResult, RealmAction, RealmActionResult, RealmCapture, Request, Response, Scope,
    ScopeDecision, SecretPromptResult, SettingsAction, SettingsReceipt, SettingsSnapshot,
    StreamPixelFormat, StreamTarget, SystemAction, SystemStatus,
};
pub use server::{
    AgentIdentity, CaptureOutputPayload, CaptureRealmPayload, Handler, PairedAgent, Server,
    StreamFramePayload, StreamInfo,
};
