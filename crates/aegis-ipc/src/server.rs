//! The compositor side of the IPC.
//!
//! [`Server::start`] binds a unix socket and spawns an accept thread; each
//! connection runs two helper threads — a reader that drives the protocol and
//! a writer that is the sole owner of the write half (so responses and
//! pushed events never contend). Mutations arrive as [`Command`]s through
//! [`Handler::command`], which the compositor forwards to its main loop:
//! the Wayland server state is not `Send`, so a connection thread must not
//! touch it directly. See ADR-0027.
//!
//! The mutation journal (ADR-0033) has a separate subscriber set
//! ([`Server::broadcast_journal`]) so status bars receiving the coarse event
//! stream are not flooded with per-command entries.

use std::collections::HashMap;
use std::io;
use std::net::Shutdown;
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::codec::{read_msg, write_msg};
use crate::journal::{JournalEntry, JournalMutation};
use crate::schema::{
    ActorActionIntent, ActorActionReceipt, ActorCapability, AgentGrantInfo, AgentIssued,
    AgentPrincipalInfo, AuthorizationDecision, Command, CommandScopePolicy, ConnectionCapabilities,
    Event, InteractionDomainAction, InteractionDomainActionResult, InteractionDomainCapture,
    LOCAL_AGENT_ADMIN_SCOPE, LOCAL_INTERACTION_DOMAIN_ADMIN_SCOPE, LOCAL_OWNER_ADMIN_SCOPE,
    LOCAL_PORTAL_SCOPE, LeaseGrant, PROTOCOL_VERSION, Request, Response, Scope,
    SemanticObservation, SettingsAction, SettingsReceipt, SettingsSnapshot, StreamPixelFormat,
};
pub use aegis_authority::{AgentIdentity, PairedAgent};

/// Large output-capture payload transferred as a sealed memfd by the IPC
/// writer. It intentionally is not part of the JSON schema.
#[derive(Debug, Clone)]
pub struct CaptureOutputPayload {
    pub width: u32,
    pub height: u32,
    pub png: Vec<u8>,
}

/// Correlated Interaction Domain metadata and its PNG. The writer serializes only
/// `capture` and passes `png` as a sealed memfd.
#[derive(Debug, Clone)]
pub struct CaptureInteractionDomainPayload {
    pub capture: InteractionDomainCapture,
    pub png: Vec<u8>,
}

/// Geometry and format of a stream started through
/// [`Handler::stream_output_start`] (ADR-0052).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamInfo {
    pub stream_id: u64,
    pub width: u32,
    pub height: u32,
    pub format: StreamPixelFormat,
}

/// One presented frame pushed to a stream's connection. `pixels` are raw,
/// tightly packed (row stride `stride`) and transferred as a sealed memfd
/// after the JSON [`Event::StreamFrame`] metadata; the blob intentionally is
/// not part of the JSON schema. Shared cheaply between streams fanning out
/// from one readback.
#[derive(Debug, Clone)]
pub struct StreamFramePayload {
    pub stream_id: u64,
    pub sequence: u64,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub format: StreamPixelFormat,
    pub damage: Vec<aegis_core::Rect>,
    pub dropped: u64,
    pub pixels: Arc<[u8]>,
}

/// Frames a single stream may have queued to its connection writer before
/// the producer starts dropping (ADR-0052: bounded lanes, drop never block).
const STREAM_LANE_DEPTH: u32 = 2;

/// Responses, pushed events, and detached payloads share one bounded writer
/// inbox per connection. Request threads may backpressure on it; compositor
/// producers always use `try_send` and never wait for a slow peer.
const OUTBOUND_QUEUE_DEPTH: usize = 64;
const MAX_CONNECTIONS: u32 = 256;
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);
const WRITE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

struct ConnectionPermit {
    active: Arc<AtomicU32>,
}

impl ConnectionPermit {
    fn acquire(active: &Arc<AtomicU32>) -> Option<Self> {
        active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < MAX_CONNECTIONS).then_some(count + 1)
            })
            .ok()
            .map(|_| Self {
                active: Arc::clone(active),
            })
    }
}

impl Drop for ConnectionPermit {
    fn drop(&mut self) {
        self.active.fetch_sub(1, Ordering::AcqRel);
    }
}

/// Re-resolvable authority for work that can outlive the request that
/// started it. Named scopes and authenticated principal ceilings are read
/// again at the final effect boundary; anonymous compatibility clients use
/// only their handshake-time ceiling.
#[derive(Debug, Clone)]
struct LiveScopeBinding {
    connection_id: u64,
    session: aegis_authority::ActorSessionId,
    name: Option<String>,
    principal: Option<aegis_authority::ActorPrincipal>,
    fallback: Scope,
}

impl LiveScopeBinding {
    fn resolve<H: Handler>(&self, handler: &H) -> Option<Scope> {
        effective_scope(
            handler,
            self.name.as_deref(),
            &self.fallback,
            self.principal.as_ref().map(AsRef::as_ref),
        )
    }
}

/// One live stream's delivery lane into its connection's writer thread.
struct StreamLane {
    conn_id: u64,
    tx: SyncSender<Outbound>,
    scope: LiveScopeBinding,
    target: crate::schema::StreamTarget,
    lease_deadline: Arc<Mutex<std::time::Instant>>,
    /// Frames handed to the writer but not yet written. The producer
    /// (`Server::push_stream_frame`) increments and the writer decrements, so
    /// a slow consumer's backlog is bounded at [`STREAM_LANE_DEPTH`].
    queued: Arc<AtomicU32>,
}

/// One event subscription plus a connection shutdown handle. If its bounded
/// writer queue fills, the broadcaster closes the socket: silently dropping
/// a journal or invalidation event would leave the client with an
/// unknowingly stale projection.
#[derive(Clone)]
struct SubscriptionLane {
    tx: SyncSender<Outbound>,
    shutdown: Option<Arc<UnixStream>>,
}

impl SubscriptionLane {
    fn try_send(&self, outbound: Outbound) -> bool {
        if self.tx.try_send(outbound).is_ok() {
            return true;
        }
        if let Some(stream) = &self.shutdown {
            let _ = stream.shutdown(Shutdown::Both);
        }
        false
    }
}

/// Subscriber ids are globally unique across connections and subscription
/// types (coarse events vs. journal entries).
type SubId = u64;

/// What the writer thread sends on the wire. Both kinds go through one inbox
/// so the writer is the single owner of the write half.
#[derive(Debug, Clone)]
enum Outbound {
    Response(Response),
    Event(Event),
    CaptureOutput {
        payload: CaptureOutputPayload,
        lease_deadline: std::time::Instant,
        scope: LiveScopeBinding,
    },
    CaptureInteractionDomain {
        payload: CaptureInteractionDomainPayload,
        lease_deadline: std::time::Instant,
        scope: LiveScopeBinding,
        /// The request was authorized through a runtime grant rather than
        /// the scope's pregranted operations (ADR-0088).
        via_grant: bool,
    },
    StreamFrame {
        payload: StreamFramePayload,
        lease_deadline: Arc<Mutex<std::time::Instant>>,
        scope: LiveScopeBinding,
        target: crate::schema::StreamTarget,
        queued: Arc<AtomicU32>,
    },
}

mod handler;
pub use handler::Handler;
#[derive(Clone, Default)]
pub struct JournalBroadcaster {
    subscribers: Arc<Mutex<HashMap<SubId, SubscriptionLane>>>,
}

impl JournalBroadcaster {
    /// Push one already-durable entry to every journal subscriber.
    pub fn broadcast(&self, entry: JournalEntry) {
        self.subscribers.lock().unwrap().retain(|_, sender| {
            sender.try_send(Outbound::Event(Event::Journal {
                entry: entry.clone(),
            }))
        });
    }
}

/// A bound IPC server. The accept thread runs until the handle is dropped
/// (process exit) or the listener errors. `Drop` removes the socket file
/// best-effort so a restart rebinds cleanly.
pub struct Server {
    _accept: thread::JoinHandle<()>,
    socket: PathBuf,
    subs: Arc<Mutex<HashMap<SubId, SubscriptionLane>>>,
    journal_broadcaster: JournalBroadcaster,
    streams: Arc<Mutex<HashMap<u64, StreamLane>>>,
}

impl Server {
    /// Bind `path`, remove a stale socket first, and start serving. The
    /// bind happens synchronously before the accept thread spawns, so a
    /// caller connecting immediately after `start` returns does not race.
    pub fn start<H: Handler + 'static>(path: &Path, handler: Arc<H>) -> io::Result<Server> {
        Self::start_with_journal_broadcaster(path, handler, JournalBroadcaster::default())
    }

    /// Start with a pre-created broadcaster. Production uses this form so
    /// every journal producer can serialize durable append plus broadcast
    /// under the same journal lock before the socket accepts a client.
    pub fn start_with_journal_broadcaster<H: Handler + 'static>(
        path: &Path,
        handler: Arc<H>,
        journal_broadcaster: JournalBroadcaster,
    ) -> io::Result<Server> {
        match std::fs::symlink_metadata(path) {
            Ok(metadata) if metadata.file_type().is_socket() => match UnixStream::connect(path) {
                Ok(_) => {
                    return Err(io::Error::new(
                        io::ErrorKind::AddrInUse,
                        format!("IPC socket {} is already serving", path.display()),
                    ));
                }
                Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                    std::fs::remove_file(path)?;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(error),
            },
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    format!("refusing to replace non-socket path {}", path.display()),
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        let listener = UnixListener::bind(path)?;
        if let Err(error) = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)) {
            let _ = std::fs::remove_file(path);
            return Err(error);
        }
        let socket = path.to_path_buf();
        let subs = Arc::new(Mutex::new(HashMap::new()));
        let journal_subs = Arc::clone(&journal_broadcaster.subscribers);
        let streams = Arc::new(Mutex::new(HashMap::new()));
        let next_sub = Arc::new(AtomicU64::new(0));
        let next_lease = Arc::new(AtomicU64::new(1));
        let next_conn = Arc::new(AtomicU64::new(1));
        let active_connections = Arc::new(AtomicU32::new(0));
        let subs_for_accept = Arc::clone(&subs);
        let journal_subs_for_accept = Arc::clone(&journal_subs);
        let streams_for_accept = Arc::clone(&streams);
        let next_for_accept = Arc::clone(&next_sub);
        let lease_for_accept = Arc::clone(&next_lease);
        let conn_for_accept = Arc::clone(&next_conn);
        let active_for_accept = Arc::clone(&active_connections);
        let accept = match thread::Builder::new()
            .name("aegis-ipc-accept".into())
            .spawn(move || {
                accept_loop(
                    listener,
                    handler,
                    subs_for_accept,
                    journal_subs_for_accept,
                    streams_for_accept,
                    next_for_accept,
                    lease_for_accept,
                    conn_for_accept,
                    active_for_accept,
                )
            }) {
            Ok(accept) => accept,
            Err(error) => {
                let _ = std::fs::remove_file(path);
                return Err(error);
            }
        };
        Ok(Server {
            _accept: accept,
            socket,
            subs,
            journal_broadcaster,
            streams,
        })
    }

    /// Push a coarse event to every subscribed connection (ADR-0027).
    ///
    /// Delivery is bounded and fail-closed: a full or disconnected lane is
    /// removed and its socket is shut down, forcing the client to reconnect
    /// and obtain a fresh authoritative snapshot instead of continuing after
    /// a silently missed invalidation.
    pub fn broadcast(&self, ev: Event) {
        self.subs
            .lock()
            .unwrap()
            .retain(|_, lane| lane.try_send(Outbound::Event(ev.clone())));
    }

    /// Push a journal entry to every journal-subscribed connection
    /// (ADR-0033). Separate from [`broadcast`](Self::broadcast) so coarse
    /// subscribers are not flooded with per-command entries.
    pub fn broadcast_journal(&self, entry: JournalEntry) {
        self.journal_broadcaster.broadcast(entry);
    }

    /// Queue one presented frame for delivery to a stream (ADR-0052). Called
    /// from the compositor main loop after a readback completes. Bounded:
    /// returns `false` without queueing when the stream is unknown or already
    /// has two frames in flight, so a slow consumer can
    /// never stall the producer or grow the queue; the caller counts the drop.
    pub fn push_stream_frame(&self, payload: StreamFramePayload) -> bool {
        let (tx, scope, target, lease_deadline, queued) = {
            let streams = self.streams.lock().unwrap();
            let Some(lane) = streams.get(&payload.stream_id) else {
                return false;
            };
            if lane.queued.load(Ordering::Acquire) >= STREAM_LANE_DEPTH {
                return false;
            }
            (
                lane.tx.clone(),
                lane.scope.clone(),
                lane.target,
                Arc::clone(&lane.lease_deadline),
                Arc::clone(&lane.queued),
            )
        };
        queued.fetch_add(1, Ordering::AcqRel);
        if tx
            .try_send(Outbound::StreamFrame {
                payload,
                lease_deadline,
                scope,
                target,
                queued: Arc::clone(&queued),
            })
            .is_err()
        {
            queued.fetch_sub(1, Ordering::AcqRel);
            return false;
        }
        true
    }

    /// End a stream from the server side (output geometry change, compositor
    /// shutdown): unregister its lane and notify the client. The caller drops
    /// its own stream state separately.
    pub fn end_stream(&self, stream_id: u64, reason: &str) {
        let lane = self.streams.lock().unwrap().remove(&stream_id);
        if let Some(lane) = lane {
            let _ = lane.tx.try_send(Outbound::Event(Event::StreamEnded {
                stream_id,
                reason: reason.to_owned(),
            }));
        }
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.socket)
            .is_ok_and(|metadata| metadata.file_type().is_socket())
        {
            let _ = std::fs::remove_file(&self.socket);
        }
    }
}

mod connection;
use connection::accept_loop;
mod authorization;
use authorization::*;
mod dispatch;
use dispatch::drive_read_loop;
mod writer;
use writer::{write_interaction_domain_capture, write_output_capture, write_stream_frame};
#[cfg(test)]
mod tests;
