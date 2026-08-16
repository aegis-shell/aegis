//! Versioned IPC and introspection surface for aegis.
//!
//! A unix-domain socket at `$XDG_RUNTIME_DIR/aegis.sock` speaks a length-
//! framed, schema-versioned JSON protocol. It is the sole extension and
//! automation surface: the chrome, external tools, and the later agent layer
//! all read the same `aegis_model` model the IPC serves — there is no separate
//! wire DTO and no in-process scripting.
//!
//! The crate is pure: it depends on [`aegis_model`] (with its `serde` feature
//! so the `Window` it sends is the same `Window` the renderer reads) and on
//! `serde`/`serde_json`. A loopback server + client exercise the whole path
//! in tests with no Vulkan or Wayland dependency. See
//! [ADR-0027](../../docs/adr/0027-ipc-and-introspection.md).
//!
//! # Status
//!
//! Protocol version 28 adds the agent primitive families (ADR-0125):
//! `Observe`, `Transact`, and scope-filtered agent subscriptions. Version 24
//! added kernel-process-bound accessibility window
//! correlation. Version 23 introduced explicit Actor sessions, exact
//! resource handles, and the bounded accessibility adapter transport. It retains the
//! Interaction Domain vocabulary and transport-neutral authority contracts
//! introduced in version 22. Earlier revisions are recorded in
//! [`schema::PROTOCOL_VERSION`] and the project changelog.

pub mod client;
pub mod codec;
pub mod journal;
pub mod schema;
pub mod server;

pub use aegis_security::authority::{
    ActorPrincipal, ActorResource, ActorSessionId, ActorSessionPolicy, ActorSessionSnapshot,
    ActorSessionState, FilesystemAccess, ResourceGrant, ResourceGrantId,
};

mod blob;

pub use client::{
    CapturedInteractionDomain, CapturedWindow, Client, StreamFrame, StreamMessage, StreamStarted,
};
pub use journal::{
    ActorSessionAuditAction, AgentAuthAction, AuditedCommand, AuditedSemanticAction,
    CapabilityUseAction, DEFAULT_CAPACITY, Effect, GrantPersistence, Journal, JournalEntry,
    JournalMutation, JournalSnapshot, Origin, ResourceGrantAttemptAction, ResourceGrantAuditAction,
    ResourceKind, audit_semantic_actions,
};
pub use schema::{
    AccessibilityTreeUpdate, AccessibilityWindowBinding, ActorActionIntent, ActorActionReceipt,
    ActorCapability, AgentGrantDecision, AgentGrantInfo, AgentHello, AgentIssued,
    AgentPrincipalInfo, AppPickResult, AuthorizationDecision, Command, CommandScopePolicy,
    ConfirmPickResult, ConnectionCapabilities, Event, InteractionDomainAction,
    InteractionDomainActionResult, InteractionDomainCapture, JournalCursor,
    LOCAL_AGENT_ADMIN_SCOPE, LOCAL_INTERACTION_DOMAIN_ADMIN_SCOPE, LOCAL_OWNER_ADMIN_SCOPE,
    LOCAL_PORTAL_SCOPE, LeaseGrant, LeaseRequest, MAX_TRANSACT_OPS, ObservationToken,
    ObserveSnapshot, OutputInfo, PROTOCOL_VERSION, PickKind, PickResult, Request, Response, Scope,
    SecretPromptResult, SemanticActionRequest, SemanticObservation, SettingsAction,
    SettingsReceipt, SettingsSnapshot, StreamCursorMode, StreamPixelFormat, StreamTarget,
    SystemAction, SystemStatus, TransactOp, TransactOpResult, TransactPrecondition,
    TransactReceipt, TransactResult, WindowCapture,
};
pub use server::{
    AgentIdentity, CaptureInteractionDomainPayload, CaptureOutputPayload, CaptureWindowPayload,
    Handler, JournalBroadcaster, PairedAgent, Server, StreamFramePayload, StreamInfo,
    StreamPixelFrame, StreamSlotFrame, StreamSlotTable,
};
