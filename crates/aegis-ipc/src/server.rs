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
use std::os::unix::fs::{FileTypeExt, PermissionsExt};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::codec::{read_msg, write_msg};
use crate::journal::{JournalEntry, JournalMutation};
use crate::schema::{
    Capabilities, Command, Event, LeaseGrant, OpClass, PROTOCOL_VERSION, RealmAction,
    RealmActionResult, RealmCapture, Request, Response, Scope, SettingsAction, SettingsReceipt,
    SettingsSnapshot, StreamPixelFormat,
};

/// Large output-capture payload transferred as a sealed memfd by the IPC
/// writer. It intentionally is not part of the JSON schema.
#[derive(Debug, Clone)]
pub struct CaptureOutputPayload {
    pub width: u32,
    pub height: u32,
    pub png: Vec<u8>,
}

/// Correlated Realm metadata and its PNG. The writer serializes only
/// `capture` and passes `png` as a sealed memfd.
#[derive(Debug, Clone)]
pub struct CaptureRealmPayload {
    pub capture: RealmCapture,
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

/// One live stream's delivery lane into its connection's writer thread.
struct StreamLane {
    conn_id: u64,
    tx: Sender<Outbound>,
    scope_name: Option<String>,
    lease_deadline: Arc<Mutex<std::time::Instant>>,
    /// Frames handed to the writer but not yet written. The producer
    /// (`Server::push_stream_frame`) increments and the writer decrements, so
    /// a slow consumer's backlog is bounded at [`STREAM_LANE_DEPTH`].
    queued: Arc<AtomicU32>,
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
        scope_name: Option<String>,
    },
    CaptureRealm {
        payload: CaptureRealmPayload,
        lease_deadline: std::time::Instant,
        scope_name: Option<String>,
    },
    StreamFrame {
        payload: StreamFramePayload,
        lease_deadline: Arc<Mutex<std::time::Instant>>,
        scope_name: Option<String>,
        queued: Arc<AtomicU32>,
    },
}

/// Live compositor state the IPC serves. The main loop updates the snapshot
/// each frame; connection threads read it. `Send + Sync + 'static` so it can
/// live behind an `Arc` shared across threads.
pub trait Handler: Send + Sync {
    /// Capabilities this server grants by policy before per-client
    /// intersection. `query` is added back unconditionally (ADR-0027).
    fn policy_caps(&self) -> Capabilities {
        Capabilities::QUERY
    }
    /// Snapshot of live toplevels, in z-order — the same `Window` the
    /// renderer and chrome read.
    fn windows(&self) -> Vec<aegis_core::window::Window>;
    /// Snapshot of the workspace/output model. Same shape the chrome and the
    /// agent read.
    fn workspaces(&self) -> aegis_core::workspace::WorkspaceSnapshot;
    /// Snapshot of the live notification queue.
    fn notifications(&self) -> Vec<aegis_core::notify::Notification>;
    /// Snapshot of the live outputs (connector + geometry).
    fn outputs(&self) -> Vec<aegis_core::output::OutputInfo>;
    /// Snapshot of journal entries with `seq > since` (ADR-0033).
    fn journal_since(&self, since: u64) -> crate::journal::JournalSnapshot;
    /// Complete Realm authority snapshot.
    fn realms(&self) -> aegis_core::realm::RealmSnapshot {
        aegis_core::realm::RealmModel::new().snapshot()
    }
    /// Compositor-owned persistent settings snapshot.
    fn settings(&self) -> SettingsSnapshot {
        SettingsSnapshot::default()
    }
    /// Live host and compositor-owned session status.
    fn system_status(&self) -> aegis_core::system::SystemStatus {
        aegis_core::system::SystemStatus::default()
    }
    /// Resolve a named scope from configuration (ADR-0034). Returns `None`
    /// if the name is unknown; an explicitly named connection is refused.
    fn resolve_scope(&self, _name: &str) -> Option<Scope> {
        None
    }
    /// Reauthorize one Realm action against live interaction-group state.
    ///
    /// The default performs the schema-level scope check. Compositors should
    /// additionally expand group-level mutations to every affected window so
    /// an allowlisted member cannot smuggle sibling windows across realms.
    fn authorize_realm_action(&self, scope: &Scope, action: &RealmAction) -> Result<(), String> {
        scope
            .permits_realm_action(action)
            .then_some(())
            .ok_or_else(|| "out of scope".into())
    }
    /// Record a mutation rejected by the IPC capability/scope/lease layer.
    ///
    /// Implementations should enqueue this onto the compositor's single
    /// journal owner rather than mutating the journal from the connection
    /// thread. The default keeps embedders source-compatible without an audit
    /// sink.
    fn audit_refusal(&self, _conn_id: u64, _mutation: JournalMutation, _reason: String) {}
    /// Live lock/VT security gate checked by the writer immediately before it
    /// attaches a capture memfd.
    fn capture_security_active(&self) -> bool {
        true
    }
    /// Receive a control/session [`Command`]. Called from a connection
    /// thread; the implementation must forward to the compositor's main loop
    /// because the Wayland server state is not `Send`. Fire-and-forget: the
    /// caller acknowledges queuing, not completion.
    fn command(&self, conn_id: u64, cmd: Command);
    /// Commit one synchronous Realm lifecycle request on the compositor main
    /// thread and return its authoritative receipt.
    fn realm_action(
        &self,
        _conn_id: u64,
        _action: RealmAction,
    ) -> Result<RealmActionResult, String> {
        Err("realm control unsupported".into())
    }
    /// Persist and apply one settings transaction on the compositor main
    /// thread, returning only after the authoritative state is updated.
    fn settings_action(
        &self,
        _conn_id: u64,
        _expected_revision: Option<u64>,
        _action: SettingsAction,
    ) -> Result<SettingsReceipt, String> {
        Err("settings control unsupported".into())
    }
    /// Capture the focused output as a PNG. Called from a connection thread;
    /// the implementation forwards to the main loop and blocks briefly for
    /// the reply. The writer transfers the PNG through a sealed memfd after
    /// the small JSON response. `region` is in compositor logical pixels;
    /// `None` captures the whole output.
    fn capture_output(
        &self,
        _region: Option<aegis_core::Rect>,
    ) -> Result<CaptureOutputPayload, String> {
        Err("capture unsupported".into())
    }
    /// Capture one directed virtual output.
    fn capture_realm(
        &self,
        _realm: aegis_core::realm::RealmId,
        _region: Option<aegis_core::Rect>,
    ) -> Result<CaptureRealmPayload, String> {
        Err("realm capture unsupported".into())
    }
    /// Start a continuous frame stream of the focused output (ADR-0052).
    /// Called from a connection thread after capability, lease, and scope
    /// checks; the implementation forwards to the main loop and blocks
    /// briefly for the reply. Frames are pushed back through
    /// [`Server::push_stream_frame`]. `target` (version 6) selects the whole
    /// output or one window's visible region (ADR-0054); an unknown window
    /// id is an error.
    fn stream_output_start(
        &self,
        _conn_id: u64,
        _max_fps: Option<u32>,
        _target: crate::schema::StreamTarget,
    ) -> Result<StreamInfo, String> {
        Err("streaming unsupported".into())
    }
    /// Notification that a stream was torn down — a `StreamOutputStop`
    /// request, a per-frame authorization failure in the writer, or a
    /// disconnect. Fire-and-forget; the main loop drops its stream state.
    fn stream_output_stop(&self, _stream_id: u64) {}
    /// Notification that a connection owning one or more streams
    /// disconnected. The server has already unregistered the delivery lanes;
    /// the main loop drops its stream state.
    fn streams_disconnected(&self, _conn_id: u64) {}
    /// Set or clear the calling connection's global idle inhibitor
    /// (ADR-0053). Called from a connection thread after capability, lease,
    /// and scope checks; the implementation forwards to the main loop and
    /// blocks briefly for the reply, which carries the inhibitor state the
    /// connection now holds.
    fn set_idle_inhibit(&self, _conn_id: u64, _inhibit: bool) -> Result<bool, String> {
        Err("idle inhibit unsupported".into())
    }
    /// Notification that a connection holding an idle inhibitor
    /// disconnected. The server releases the inhibitor; the main loop drops
    /// its per-connection state.
    fn idle_inhibit_disconnected(&self, _conn_id: u64) {}
    /// Run one user-consent interactive pick (ADR-0054). Called from a
    /// connection thread after capability, lease, scope, and lock/VT-gate
    /// checks; the implementation forwards to the compositor main loop,
    /// which freezes the screen, opens the matching selector chrome, and
    /// answers when the user confirms or cancels. The reply may therefore
    /// block for as long as the user takes, bounded by the compositor's
    /// interaction timeout.
    fn pick_target(
        &self,
        _conn_id: u64,
        _kind: crate::schema::PickKind,
    ) -> Result<crate::schema::PickResult, String> {
        Err("interactive picking unsupported".into())
    }
}

/// A bound IPC server. The accept thread runs until the handle is dropped
/// (process exit) or the listener errors. `Drop` removes the socket file
/// best-effort so a restart rebinds cleanly.
pub struct Server {
    _accept: thread::JoinHandle<()>,
    socket: PathBuf,
    subs: Arc<Mutex<HashMap<SubId, Sender<Outbound>>>>,
    journal_subs: Arc<Mutex<HashMap<SubId, Sender<Outbound>>>>,
    streams: Arc<Mutex<HashMap<u64, StreamLane>>>,
}

impl Server {
    /// Bind `path`, remove a stale socket first, and start serving. The
    /// bind happens synchronously before the accept thread spawns, so a
    /// caller connecting immediately after `start` returns does not race.
    pub fn start<H: Handler + 'static>(path: &Path, handler: Arc<H>) -> io::Result<Server> {
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
        let journal_subs = Arc::new(Mutex::new(HashMap::new()));
        let streams = Arc::new(Mutex::new(HashMap::new()));
        let next_sub = Arc::new(AtomicU64::new(0));
        let next_lease = Arc::new(AtomicU64::new(1));
        let next_conn = Arc::new(AtomicU64::new(1));
        let subs_for_accept = Arc::clone(&subs);
        let journal_subs_for_accept = Arc::clone(&journal_subs);
        let streams_for_accept = Arc::clone(&streams);
        let next_for_accept = Arc::clone(&next_sub);
        let lease_for_accept = Arc::clone(&next_lease);
        let conn_for_accept = Arc::clone(&next_conn);
        let accept = match thread::Builder::new()
            .name("ass-ipc-accept".into())
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
            journal_subs,
            streams,
        })
    }

    /// Push a coarse event to every subscribed connection (ADR-0027).
    /// Best-effort: a dead subscriber is reaped on disconnect.
    pub fn broadcast(&self, ev: Event) {
        let subs = self.subs.lock().unwrap();
        for tx in subs.values() {
            let _ = tx.send(Outbound::Event(ev.clone()));
        }
    }

    /// Push a journal entry to every journal-subscribed connection
    /// (ADR-0033). Separate from [`broadcast`](Self::broadcast) so coarse
    /// subscribers are not flooded with per-command entries.
    pub fn broadcast_journal(&self, entry: JournalEntry) {
        let subs = self.journal_subs.lock().unwrap();
        for tx in subs.values() {
            let _ = tx.send(Outbound::Event(Event::Journal {
                entry: entry.clone(),
            }));
        }
    }

    /// Queue one presented frame for delivery to a stream (ADR-0052). Called
    /// from the compositor main loop after a readback completes. Bounded:
    /// returns `false` without queueing when the stream is unknown or already
    /// has two frames in flight, so a slow consumer can
    /// never stall the producer or grow the queue; the caller counts the drop.
    pub fn push_stream_frame(&self, payload: StreamFramePayload) -> bool {
        let (tx, scope_name, lease_deadline, queued) = {
            let streams = self.streams.lock().unwrap();
            let Some(lane) = streams.get(&payload.stream_id) else {
                return false;
            };
            if lane.queued.load(Ordering::Acquire) >= STREAM_LANE_DEPTH {
                return false;
            }
            (
                lane.tx.clone(),
                lane.scope_name.clone(),
                Arc::clone(&lane.lease_deadline),
                Arc::clone(&lane.queued),
            )
        };
        queued.fetch_add(1, Ordering::AcqRel);
        if tx
            .send(Outbound::StreamFrame {
                payload,
                lease_deadline,
                scope_name,
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
            let _ = lane.tx.send(Outbound::Event(Event::StreamEnded {
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

#[allow(clippy::too_many_arguments)]
fn accept_loop<H: Handler + 'static>(
    listener: UnixListener,
    handler: Arc<H>,
    subs: Arc<Mutex<HashMap<SubId, Sender<Outbound>>>>,
    journal_subs: Arc<Mutex<HashMap<SubId, Sender<Outbound>>>>,
    streams: Arc<Mutex<HashMap<u64, StreamLane>>>,
    next_sub: Arc<AtomicU64>,
    next_lease: Arc<AtomicU64>,
    next_conn: Arc<AtomicU64>,
) {
    for incoming in listener.incoming() {
        let Ok(stream) = incoming else {
            continue;
        };
        let h = Arc::clone(&handler);
        let s = Arc::clone(&subs);
        let js = Arc::clone(&journal_subs);
        let st = Arc::clone(&streams);
        let n = Arc::clone(&next_sub);
        let l = Arc::clone(&next_lease);
        let conn_id = next_conn.fetch_add(1, Ordering::Relaxed);
        let _ = thread::Builder::new()
            .name("ass-ipc-conn".into())
            .spawn(move || serve_connection(stream, h, s, js, st, n, l, conn_id));
    }
}

/// Run one connection: a writer thread owns the write half; the current
/// thread drives the read half and pushes responses/events through the
/// writer's inbox. On read close the reader removes both its coarse and
/// journal subscription entries so the writer sees its last sender
/// disappear and exits promptly. Any stream the connection owned is
/// unregistered and the handler is notified once for the connection.
#[allow(clippy::too_many_arguments)]
fn serve_connection<H: Handler + 'static>(
    stream: UnixStream,
    handler: Arc<H>,
    subs: Arc<Mutex<HashMap<SubId, Sender<Outbound>>>>,
    journal_subs: Arc<Mutex<HashMap<SubId, Sender<Outbound>>>>,
    streams: Arc<Mutex<HashMap<u64, StreamLane>>>,
    next_sub: Arc<AtomicU64>,
    next_lease: Arc<AtomicU64>,
    conn_id: u64,
) {
    let mut read_half = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let (tx, rx) = mpsc::channel::<Outbound>();
    let writer_handler = Arc::clone(&handler);
    let writer_streams = Arc::clone(&streams);

    let writer = thread::Builder::new()
        .name("ass-ipc-writer".into())
        .spawn(move || {
            let mut w = stream;
            while let Ok(out) = rx.recv() {
                let res = match out {
                    Outbound::Response(r) => write_msg(&mut w, &r),
                    Outbound::Event(e) => write_msg(&mut w, &e),
                    Outbound::CaptureOutput {
                        payload,
                        lease_deadline,
                        scope_name,
                    } => write_output_capture(
                        &mut w,
                        payload,
                        lease_deadline,
                        &*writer_handler,
                        scope_name.as_deref(),
                    ),
                    Outbound::CaptureRealm {
                        payload,
                        lease_deadline,
                        scope_name,
                    } => write_realm_capture(
                        &mut w,
                        payload,
                        lease_deadline,
                        &*writer_handler,
                        scope_name.as_deref(),
                    ),
                    Outbound::StreamFrame {
                        payload,
                        lease_deadline,
                        scope_name,
                        queued,
                    } => {
                        let res = write_stream_frame(
                            &mut w,
                            payload,
                            &*writer_handler,
                            scope_name.as_deref(),
                            &lease_deadline,
                            &writer_streams,
                        );
                        queued.fetch_sub(1, Ordering::AcqRel);
                        res
                    }
                };
                if res.is_err() {
                    break;
                }
            }
        });

    let (sub_id, journal_sub_id, idle_inhibited) = drive_read_loop(
        &mut read_half,
        &tx,
        &*handler,
        &subs,
        &journal_subs,
        &streams,
        &next_sub,
        &next_lease,
        conn_id,
    );
    if let Some(id) = sub_id {
        subs.lock().unwrap().remove(&id);
    }
    if let Some(id) = journal_sub_id {
        journal_subs.lock().unwrap().remove(&id);
    }
    // A disconnect releases the connection's idle inhibitor fail-closed,
    // exactly like its streams (ADR-0053).
    if idle_inhibited {
        handler.idle_inhibit_disconnected(conn_id);
    }
    let owned_streams: Vec<u64> = {
        let mut streams = streams.lock().unwrap();
        let owned: Vec<u64> = streams
            .iter()
            .filter(|(_, lane)| lane.conn_id == conn_id)
            .map(|(id, _)| *id)
            .collect();
        for id in &owned {
            streams.remove(id);
        }
        owned
    };
    if !owned_streams.is_empty() {
        handler.streams_disconnected(conn_id);
    }
    drop(tx);
    if let Ok(handle) = writer {
        let _ = handle.join();
    }
}

/// Drive the protocol on the read half. Returns `(coarse_sub_id,
/// journal_sub_id, idle_inhibited)` for cleanup; any may be absent/false.
#[allow(clippy::too_many_arguments)]
fn drive_read_loop<H: Handler>(
    read: &mut UnixStream,
    tx: &Sender<Outbound>,
    handler: &H,
    subs: &Mutex<HashMap<SubId, Sender<Outbound>>>,
    journal_subs: &Mutex<HashMap<SubId, Sender<Outbound>>>,
    streams: &Mutex<HashMap<u64, StreamLane>>,
    next_sub: &AtomicU64,
    next_lease: &AtomicU64,
    conn_id: u64,
) -> (Option<SubId>, Option<SubId>, bool) {
    const MIN_LEASE_MS: u64 = 1_000;
    const MAX_LEASE_MS: u64 = 86_400_000;
    let (granted, granted_scope, scope_name, mut active_lease) = match read_msg::<_, Request>(read)
    {
        Ok(Request::Hello {
            version,
            caps,
            scope,
            lease,
        }) => {
            if version != PROTOCOL_VERSION {
                let _ = tx.send(Outbound::Response(Response::Error {
                    message: format!(
                        "unsupported protocol version {version} (server supports {PROTOCOL_VERSION})"
                    ),
                }));
                return (None, None, false);
            }
            let mut gc = handler.policy_caps().intersect(caps).with_query_always();
            // Synthetic input is intentionally unavailable to unscoped
            // compatibility clients. A caller must name a compositor-owned
            // scope so every injected action has a revocable resource and
            // operation bound (ADR-0035).
            if scope.is_none() {
                gc.input = false;
            }
            if lease.is_none() {
                gc.control = false;
                gc.input = false;
                gc.session = false;
                gc.realm = false;
            }
            let gs = match scope.as_deref() {
                Some(name) => match handler.resolve_scope(name) {
                    Some(scope) => scope,
                    None => {
                        let _ = tx.send(Outbound::Response(Response::Error {
                            message: format!("unknown scope '{name}'"),
                        }));
                        return (None, None, false);
                    }
                },
                None => Scope::unscoped(),
            };
            let active_lease = if gc.privileged() {
                let requested = lease.expect("privileged capabilities require a lease");
                if !(MIN_LEASE_MS..=MAX_LEASE_MS).contains(&requested.ttl_ms) {
                    let _ = tx.send(Outbound::Response(Response::Error {
                        message: format!(
                            "lease ttl must be between {MIN_LEASE_MS} and {MAX_LEASE_MS} ms"
                        ),
                    }));
                    return (None, None, false);
                }
                let grant = LeaseGrant {
                    id: next_lease.fetch_add(1, Ordering::Relaxed),
                    ttl_ms: requested.ttl_ms,
                    renewable: true,
                };
                let deadline = std::time::Instant::now()
                    .checked_add(std::time::Duration::from_millis(grant.ttl_ms))
                    .expect("bounded lease duration overflowed");
                Some((grant, deadline))
            } else {
                None
            };
            (gc, gs, scope, active_lease)
        }
        Ok(_) => {
            let _ = tx.send(Outbound::Response(Response::Error {
                message: "expected Hello first".into(),
            }));
            return (None, None, false);
        }
        Err(_) => return (None, None, false),
    };
    if tx
        .send(Outbound::Response(Response::Hello {
            version: PROTOCOL_VERSION,
            caps: granted,
            scope: granted_scope.clone(),
            lease: active_lease.as_ref().map(|(grant, _)| *grant),
        }))
        .is_err()
    {
        return (None, None, false);
    }

    // Streams outlive individual requests, so their delivery-time lease
    // check reads a deadline shared with lease renewals (ADR-0052).
    let lease_deadline_shared = Arc::new(Mutex::new(
        active_lease
            .as_ref()
            .map(|(_, deadline)| *deadline)
            .unwrap_or_else(std::time::Instant::now),
    ));

    let mut sub_id: Option<SubId> = None;
    let mut journal_sub_id: Option<SubId> = None;
    // Whether this connection currently holds a global idle inhibitor;
    // released through the handler on disconnect (ADR-0053).
    let mut idle_inhibited = false;
    while let Ok(req) = read_msg::<_, Request>(read) {
        let lease_alive = active_lease
            .as_ref()
            .is_some_and(|(_, deadline)| std::time::Instant::now() < *deadline);
        let resp = match req {
            Request::Hello { .. } => Response::Error {
                message: "Hello already exchanged".into(),
            },
            Request::GetWindows => {
                if granted.query {
                    Response::Windows {
                        windows: handler.windows(),
                    }
                } else {
                    Response::Error {
                        message: "GetWindows requires the query capability".into(),
                    }
                }
            }
            Request::GetWorkspaces => {
                if granted.query {
                    Response::Workspaces {
                        snapshot: handler.workspaces(),
                    }
                } else {
                    Response::Error {
                        message: "GetWorkspaces requires the query capability".into(),
                    }
                }
            }
            Request::GetNotifications => {
                if granted.query {
                    Response::Notifications {
                        notifications: handler.notifications(),
                    }
                } else {
                    Response::Error {
                        message: "GetNotifications requires the query capability".into(),
                    }
                }
            }
            Request::GetOutputs => {
                if granted.query {
                    Response::Outputs {
                        outputs: handler.outputs(),
                    }
                } else {
                    Response::Error {
                        message: "GetOutputs requires the query capability".into(),
                    }
                }
            }
            Request::GetJournal { since } => {
                if granted.query {
                    Response::Journal {
                        snapshot: handler.journal_since(since),
                    }
                } else {
                    Response::Error {
                        message: "GetJournal requires the query capability".into(),
                    }
                }
            }
            Request::GetRealms => {
                if granted.query {
                    Response::Realms {
                        snapshot: handler.realms(),
                    }
                } else {
                    Response::Error {
                        message: "GetRealms requires the query capability".into(),
                    }
                }
            }
            Request::GetSettings => {
                if granted.query {
                    Response::Settings {
                        snapshot: handler.settings(),
                    }
                } else {
                    Response::Error {
                        message: "GetSettings requires the query capability".into(),
                    }
                }
            }
            Request::GetSystemStatus => {
                if granted.query {
                    Response::SystemStatus {
                        snapshot: handler.system_status(),
                    }
                } else {
                    Response::Error {
                        message: "GetSystemStatus requires the query capability".into(),
                    }
                }
            }
            Request::Settings {
                expected_revision,
                action,
            } => {
                let before_revision = handler.settings().revision;
                let rejection = if !granted.session {
                    Some("Settings requires the session capability".to_owned())
                } else if !lease_alive {
                    Some("privileged capability lease expired".to_owned())
                } else {
                    action.validate().err().map(str::to_owned)
                };
                if let Some(message) = rejection {
                    handler.audit_refusal(
                        conn_id,
                        JournalMutation::Settings {
                            action,
                            before_revision,
                            after_revision: before_revision,
                        },
                        message.clone(),
                    );
                    Response::Error { message }
                } else {
                    match handler.settings_action(conn_id, expected_revision, action) {
                        Ok(receipt) => Response::SettingsApplied { receipt },
                        Err(message) => Response::Error { message },
                    }
                }
            }
            Request::Realm { action } => {
                let current_scope = match scope_name.as_deref() {
                    Some(name) => handler.resolve_scope(name),
                    None => Some(granted_scope.clone()),
                };
                let rejection = if !granted.realm {
                    Some("Realm requires the realm capability".to_owned())
                } else if !lease_alive {
                    Some("privileged capability lease expired".to_owned())
                } else {
                    match current_scope.as_ref() {
                        None => Some("out of scope: named scope was revoked".into()),
                        Some(scope) => match handler.authorize_realm_action(scope, &action) {
                            Err(message) => Some(message),
                            Ok(()) => action.validate().err().map(str::to_owned),
                        },
                    }
                };
                if let Some(message) = rejection {
                    let revision = handler.realms().revision;
                    handler.audit_refusal(
                        conn_id,
                        JournalMutation::Realm {
                            action,
                            before_revision: revision,
                            after_revision: revision,
                        },
                        message.clone(),
                    );
                    Response::Error { message }
                } else {
                    match handler.realm_action(conn_id, action) {
                        Ok(result) => Response::Realm { result },
                        Err(message) => Response::Error { message },
                    }
                }
            }
            Request::Do { cmd } => {
                let need = cmd.required_cap();
                let allowed = (need.control && granted.control)
                    || (need.input && granted.input)
                    || (need.session && granted.session)
                    || (need.realm && granted.realm);
                let rejection = if !allowed {
                    Some("command requires a capability not granted".to_owned())
                } else if !lease_alive {
                    Some("privileged capability lease expired".to_owned())
                } else if !scope_name
                    .as_deref()
                    .map(|name| {
                        handler
                            .resolve_scope(name)
                            .is_some_and(|scope| scope.permits(&cmd))
                    })
                    .unwrap_or_else(|| granted_scope.permits(&cmd))
                {
                    Some("out of scope".to_owned())
                } else if let Err(message) = cmd.validate() {
                    Some(message.into())
                } else {
                    None
                };
                if let Some(message) = rejection {
                    handler.audit_refusal(
                        conn_id,
                        JournalMutation::Command { cmd },
                        message.clone(),
                    );
                    Response::Error { message }
                } else {
                    handler.command(conn_id, cmd);
                    Response::Ok
                }
            }
            Request::Subscribe => {
                if sub_id.is_none() {
                    let id = next_sub.fetch_add(1, Ordering::Relaxed);
                    subs.lock().unwrap().insert(id, tx.clone());
                    sub_id = Some(id);
                }
                Response::Subscribed
            }
            Request::SubscribeJournal => {
                if journal_sub_id.is_none() {
                    let id = next_sub.fetch_add(1, Ordering::Relaxed);
                    journal_subs.lock().unwrap().insert(id, tx.clone());
                    journal_sub_id = Some(id);
                }
                Response::Subscribed
            }
            Request::RenewLease { ttl_ms } => {
                if !(MIN_LEASE_MS..=MAX_LEASE_MS).contains(&ttl_ms) {
                    Response::Error {
                        message: format!(
                            "lease ttl must be between {MIN_LEASE_MS} and {MAX_LEASE_MS} ms"
                        ),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "lease is absent or already expired".into(),
                    }
                } else {
                    let (grant, deadline) =
                        active_lease.as_mut().expect("lease_alive checked presence");
                    grant.ttl_ms = ttl_ms;
                    *deadline = std::time::Instant::now()
                        .checked_add(std::time::Duration::from_millis(ttl_ms))
                        .expect("bounded lease duration overflowed");
                    *lease_deadline_shared.lock().unwrap() = *deadline;
                    Response::LeaseRenewed { lease: *grant }
                }
            }
            Request::CaptureOutput { region } => {
                // Pixel capture reads the screen back to the client, so it is
                // fail-closed like InjectInput: `control` plus an explicit
                // CaptureOutput op in the granted scope — never inherited
                // through None-means-all (ADR-0034).
                let current_scope = match scope_name.as_deref() {
                    Some(name) => handler.resolve_scope(name),
                    None => Some(granted_scope.clone()),
                };
                let op_allowed = current_scope.as_ref().is_some_and(|scope| {
                    scope
                        .ops
                        .as_ref()
                        .is_some_and(|ops| ops.contains(&crate::schema::OpClass::CaptureOutput))
                });
                if !granted.control {
                    Response::Error {
                        message: "CaptureOutput requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !op_allowed {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else {
                    match handler.capture_output(region) {
                        Ok(payload) => {
                            let scope_still_allows = match scope_name.as_deref() {
                                Some(name) => handler.resolve_scope(name),
                                None => Some(granted_scope.clone()),
                            }
                            .is_some_and(|scope| {
                                scope.ops.as_ref().is_some_and(|ops| {
                                    ops.contains(&crate::schema::OpClass::CaptureOutput)
                                })
                            });
                            let lease_deadline = active_lease
                                .as_ref()
                                .map(|(_, deadline)| *deadline)
                                .expect("granted control has an active lease");
                            if !scope_still_allows {
                                Response::Error {
                                    message: "out of scope before capture delivery".into(),
                                }
                            } else if std::time::Instant::now() >= lease_deadline {
                                Response::Error {
                                    message: "privileged capability lease expired".into(),
                                }
                            } else {
                                if tx
                                    .send(Outbound::CaptureOutput {
                                        payload,
                                        lease_deadline,
                                        scope_name: scope_name.clone(),
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                                continue;
                            }
                        }
                        Err(message) => Response::Error { message },
                    }
                }
            }
            Request::CaptureRealm { realm, region } => {
                let current_scope = match scope_name.as_deref() {
                    Some(name) => handler.resolve_scope(name),
                    None => Some(granted_scope.clone()),
                };
                if !granted.realm {
                    Response::Error {
                        message: "CaptureRealm requires the realm capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !current_scope
                    .as_ref()
                    .is_some_and(|scope| scope.permits_realm_capture(realm))
                {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else {
                    match handler.capture_realm(realm, region) {
                        Ok(payload) if payload.capture.realm == realm => {
                            let scope_still_allows = match scope_name.as_deref() {
                                Some(name) => handler.resolve_scope(name),
                                None => Some(granted_scope.clone()),
                            }
                            .is_some_and(|scope| scope.permits_realm_capture(realm));
                            let lease_deadline = active_lease
                                .as_ref()
                                .map(|(_, deadline)| *deadline)
                                .expect("granted Realm capability has an active lease");
                            if !scope_still_allows {
                                Response::Error {
                                    message: "out of scope before capture delivery".into(),
                                }
                            } else if std::time::Instant::now() >= lease_deadline {
                                Response::Error {
                                    message: "privileged capability lease expired".into(),
                                }
                            } else {
                                if tx
                                    .send(Outbound::CaptureRealm {
                                        payload,
                                        lease_deadline,
                                        scope_name: scope_name.clone(),
                                    })
                                    .is_err()
                                {
                                    break;
                                }
                                continue;
                            }
                        }
                        Ok(payload) => Response::Error {
                            message: format!(
                                "capture handler returned Realm {} for requested Realm {}",
                                payload.capture.realm.0, realm.0
                            ),
                        },
                        Err(message) => Response::Error { message },
                    }
                }
            }
            Request::StreamOutputStart { max_fps, target } => {
                // Fail-closed exactly like CaptureOutput: `control`, a live
                // lease, and an explicit StreamOutput op in the granted
                // scope — never inherited through None-means-all (ADR-0052).
                let current_scope = match scope_name.as_deref() {
                    Some(name) => handler.resolve_scope(name),
                    None => Some(granted_scope.clone()),
                };
                let op_allowed = current_scope.as_ref().is_some_and(|scope| {
                    scope
                        .ops
                        .as_ref()
                        .is_some_and(|ops| ops.contains(&OpClass::StreamOutput))
                });
                if !granted.control {
                    Response::Error {
                        message: "StreamOutputStart requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !op_allowed {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else if !handler.capture_security_active() {
                    Response::Error {
                        message: "session is locked or inactive".into(),
                    }
                } else {
                    match handler.stream_output_start(conn_id, max_fps, target) {
                        Ok(info) => {
                            streams.lock().unwrap().insert(
                                info.stream_id,
                                StreamLane {
                                    conn_id,
                                    tx: tx.clone(),
                                    scope_name: scope_name.clone(),
                                    lease_deadline: Arc::clone(&lease_deadline_shared),
                                    queued: Arc::new(AtomicU32::new(0)),
                                },
                            );
                            Response::StreamOutputStarted {
                                stream_id: info.stream_id,
                                width: info.width,
                                height: info.height,
                                format: info.format,
                            }
                        }
                        Err(message) => Response::Error { message },
                    }
                }
            }
            Request::StreamOutputStop { stream_id } => {
                // A connection may stop only a stream it owns.
                let owned = streams
                    .lock()
                    .unwrap()
                    .get(&stream_id)
                    .is_some_and(|lane| lane.conn_id == conn_id);
                if owned {
                    streams.lock().unwrap().remove(&stream_id);
                    handler.stream_output_stop(stream_id);
                    Response::StreamOutputStopped { stream_id }
                } else {
                    Response::Error {
                        message: format!("unknown stream {stream_id}"),
                    }
                }
            }
            Request::SetIdleInhibit { inhibit } => {
                // Fail-closed exactly like StreamOutputStart: `control`, a
                // live lease, and an explicit IdleInhibit op in the granted
                // scope — never inherited through None-means-all (ADR-0053).
                let current_scope = match scope_name.as_deref() {
                    Some(name) => handler.resolve_scope(name),
                    None => Some(granted_scope.clone()),
                };
                let op_allowed = current_scope.as_ref().is_some_and(|scope| {
                    scope
                        .ops
                        .as_ref()
                        .is_some_and(|ops| ops.contains(&OpClass::IdleInhibit))
                });
                if !granted.control {
                    Response::Error {
                        message: "SetIdleInhibit requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !op_allowed {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else {
                    match handler.set_idle_inhibit(conn_id, inhibit) {
                        Ok(inhibited) => {
                            idle_inhibited = inhibited;
                            Response::IdleInhibitSet { inhibited }
                        }
                        Err(message) => Response::Error { message },
                    }
                }
            }
            Request::PickTarget { kind } => {
                // Fail-closed exactly like StreamOutputStart (ADR-0054):
                // `control`, a live lease, and an explicit PickTarget op in
                // the granted scope — never inherited — plus the lock/VT
                // gate, since a pick presents and reads screen content. The
                // user's click is the interactive half of the authorization.
                let current_scope = match scope_name.as_deref() {
                    Some(name) => handler.resolve_scope(name),
                    None => Some(granted_scope.clone()),
                };
                let op_allowed = current_scope.as_ref().is_some_and(|scope| {
                    scope
                        .ops
                        .as_ref()
                        .is_some_and(|ops| ops.contains(&OpClass::PickTarget))
                });
                if !granted.control {
                    Response::Error {
                        message: "PickTarget requires the control capability".into(),
                    }
                } else if !lease_alive {
                    Response::Error {
                        message: "privileged capability lease expired".into(),
                    }
                } else if !op_allowed {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else if !handler.capture_security_active() {
                    Response::Error {
                        message: "session is locked or inactive".into(),
                    }
                } else {
                    match handler.pick_target(conn_id, kind) {
                        Ok(result) => {
                            // The pick blocked on user interaction; policy may
                            // have changed meanwhile, so re-check before
                            // delivering the picked content (ADR-0054).
                            let scope_still_allows = match scope_name.as_deref() {
                                Some(name) => handler.resolve_scope(name),
                                None => Some(granted_scope.clone()),
                            }
                            .is_some_and(|scope| {
                                scope
                                    .ops
                                    .as_ref()
                                    .is_some_and(|ops| ops.contains(&OpClass::PickTarget))
                            });
                            if !scope_still_allows {
                                Response::Error {
                                    message: "out of scope before pick delivery".into(),
                                }
                            } else if active_lease
                                .as_ref()
                                .is_some_and(|(_, deadline)| std::time::Instant::now() >= *deadline)
                            {
                                Response::Error {
                                    message: "privileged capability lease expired".into(),
                                }
                            } else {
                                Response::Picked { result }
                            }
                        }
                        Err(message) => Response::Error { message },
                    }
                }
            }
        };
        if tx.send(Outbound::Response(resp)).is_err() {
            break;
        }
    }
    (sub_id, journal_sub_id, idle_inhibited)
}

fn write_output_capture<H: Handler>(
    stream: &mut UnixStream,
    payload: CaptureOutputPayload,
    lease_deadline: std::time::Instant,
    handler: &H,
    scope_name: Option<&str>,
) -> io::Result<()> {
    match crate::blob::SealedBlob::new(&payload.png) {
        Ok(_blob) if std::time::Instant::now() >= lease_deadline => write_msg(
            stream,
            &Response::Error {
                message: "privileged capability lease expired before capture delivery".into(),
            },
        ),
        Ok(_blob)
            if !handler.capture_security_active()
                || !scope_name
                    .and_then(|name| handler.resolve_scope(name))
                    .is_some_and(|scope| {
                        scope
                            .ops
                            .as_ref()
                            .is_some_and(|ops| ops.contains(&crate::schema::OpClass::CaptureOutput))
                    }) =>
        {
            write_msg(
                stream,
                &Response::Error {
                    message: "capture authorization changed before final delivery".into(),
                },
            )
        }
        Ok(blob) => {
            write_msg(
                stream,
                &Response::CaptureOutput {
                    width: payload.width,
                    height: payload.height,
                    png_bytes: blob.len(),
                },
            )?;
            blob.send(stream)
        }
        Err(error) => write_msg(
            stream,
            &Response::Error {
                message: format!("prepare output capture transfer: {error}"),
            },
        ),
    }
}

fn write_realm_capture<H: Handler>(
    stream: &mut UnixStream,
    mut payload: CaptureRealmPayload,
    lease_deadline: std::time::Instant,
    handler: &H,
    scope_name: Option<&str>,
) -> io::Result<()> {
    match crate::blob::SealedBlob::new(&payload.png) {
        Ok(_blob) if std::time::Instant::now() >= lease_deadline => write_msg(
            stream,
            &Response::Error {
                message: "privileged capability lease expired before Realm capture delivery".into(),
            },
        ),
        Ok(blob) => {
            let snapshot = handler.realms();
            let authorized = handler.capture_security_active()
                && scope_name
                    .and_then(|name| handler.resolve_scope(name))
                    .is_some_and(|scope| scope.permits_realm_capture(payload.capture.realm))
                && snapshot.revision == payload.capture.revision
                && snapshot.realms.iter().any(|realm| {
                    realm.id == payload.capture.realm
                        && realm.state == aegis_core::realm::RealmState::Active
                });
            if !authorized {
                return write_msg(
                    stream,
                    &Response::Error {
                        message: "Realm capture authorization changed before final delivery".into(),
                    },
                );
            }
            payload.capture.png_bytes = blob.len();
            write_msg(
                stream,
                &Response::CaptureRealm {
                    capture: payload.capture,
                },
            )?;
            blob.send(stream)
        }
        Err(error) => write_msg(
            stream,
            &Response::Error {
                message: format!("prepare Realm capture transfer: {error}"),
            },
        ),
    }
}

/// Write one stream frame: the JSON [`Event::StreamFrame`] metadata followed
/// by its sealed pixel memfd. Live policy is re-checked per frame
/// (ADR-0052): an expired lease or a revoked/narrowed scope ends the stream
/// (`StreamEnded`, lane unregistered, handler notified); an inactive
/// lock/VT gate drops the frame silently and the stream survives.
#[allow(clippy::too_many_arguments)]
fn write_stream_frame<H: Handler>(
    stream: &mut UnixStream,
    payload: StreamFramePayload,
    handler: &H,
    scope_name: Option<&str>,
    lease_deadline: &Mutex<std::time::Instant>,
    streams: &Mutex<HashMap<u64, StreamLane>>,
) -> io::Result<()> {
    let lease_alive = std::time::Instant::now() < *lease_deadline.lock().unwrap();
    let scope_allows = scope_name
        .and_then(|name| handler.resolve_scope(name))
        .is_some_and(|scope| {
            scope
                .ops
                .as_ref()
                .is_some_and(|ops| ops.contains(&OpClass::StreamOutput))
        });
    if !lease_alive || !scope_allows {
        let reason = if !lease_alive {
            "privileged capability lease expired"
        } else {
            "stream scope was revoked or narrowed"
        };
        let lane = streams.lock().unwrap().remove(&payload.stream_id);
        if lane.is_some() {
            handler.stream_output_stop(payload.stream_id);
        }
        return write_msg(
            stream,
            &Event::StreamEnded {
                stream_id: payload.stream_id,
                reason: reason.to_owned(),
            },
        );
    }
    if !handler.capture_security_active() {
        // Session locked or seat inactive: pause delivery, keep the stream.
        return Ok(());
    }
    let blob = match crate::blob::SealedBlob::new(&payload.pixels) {
        Ok(blob) => blob,
        // A malformed frame (size overflow) is dropped, not fatal.
        Err(_) => return Ok(()),
    };
    write_msg(
        stream,
        &Event::StreamFrame {
            stream_id: payload.stream_id,
            sequence: payload.sequence,
            width: payload.width,
            height: payload.height,
            stride: payload.stride,
            format: payload.format,
            damage: payload.damage,
            dropped: payload.dropped,
            byte_len: blob.len(),
        },
    )?;
    blob.send(stream)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(stream_id: u64) -> StreamFramePayload {
        StreamFramePayload {
            stream_id,
            sequence: 1,
            width: 2,
            height: 2,
            stride: 8,
            format: StreamPixelFormat::Bgra8,
            damage: vec![aegis_core::Rect::new(0, 0, 2, 2)],
            dropped: 0,
            pixels: Arc::from(&[7u8; 16][..]),
        }
    }

    /// A `Server` without a listener: the lane bookkeeping under test lives
    /// behind `push_stream_frame`, which never touches the accept thread.
    fn bare_server() -> Server {
        Server {
            _accept: thread::spawn(|| {}),
            socket: PathBuf::new(),
            subs: Arc::new(Mutex::new(HashMap::new())),
            journal_subs: Arc::new(Mutex::new(HashMap::new())),
            streams: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn add_lane(server: &Server, stream_id: u64, tx: Sender<Outbound>) {
        server.streams.lock().unwrap().insert(
            stream_id,
            StreamLane {
                conn_id: 1,
                tx,
                scope_name: None,
                lease_deadline: Arc::new(Mutex::new(std::time::Instant::now())),
                queued: Arc::new(AtomicU32::new(0)),
            },
        );
    }

    #[test]
    fn stream_lane_bounds_queued_frames_at_lane_depth() {
        let server = bare_server();
        // Nobody drains the receiver, so the writer never decrements
        // `queued`: the lane fills exactly at STREAM_LANE_DEPTH and further
        // pushes drop instead of queueing (ADR-0052 backpressure).
        let (tx, _rx) = mpsc::channel();
        add_lane(&server, 1, tx);
        for _ in 0..STREAM_LANE_DEPTH {
            assert!(server.push_stream_frame(frame(1)));
        }
        assert!(!server.push_stream_frame(frame(1)));
        // Unknown streams refuse without queueing.
        assert!(!server.push_stream_frame(frame(99)));
    }

    #[test]
    fn stream_lane_refills_as_the_writer_drains() {
        let server = bare_server();
        let (tx, rx) = mpsc::channel();
        add_lane(&server, 1, tx);
        for _ in 0..STREAM_LANE_DEPTH {
            assert!(server.push_stream_frame(frame(1)));
        }
        // Simulate the writer consuming and decrementing.
        let mut drained = 0;
        while let Ok(Outbound::StreamFrame { queued, .. }) = rx.try_recv() {
            queued.fetch_sub(1, Ordering::AcqRel);
            drained += 1;
        }
        assert_eq!(drained, STREAM_LANE_DEPTH);
        assert!(server.push_stream_frame(frame(1)));
    }

    #[test]
    fn end_stream_unregisters_and_notifies_the_client() {
        let server = bare_server();
        let (tx, rx) = mpsc::channel();
        add_lane(&server, 7, tx);
        server.end_stream(7, "output geometry changed");
        assert!(!server.push_stream_frame(frame(7)));
        match rx.recv().unwrap() {
            Outbound::Event(Event::StreamEnded { stream_id, reason }) => {
                assert_eq!(stream_id, 7);
                assert_eq!(reason, "output geometry changed");
            }
            other => panic!("expected StreamEnded, got {other:?}"),
        }
        // Ending an unknown stream is a no-op.
        server.end_stream(7, "again");
        assert!(rx.try_recv().is_err());
    }
}
