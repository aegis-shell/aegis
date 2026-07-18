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
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

use crate::codec::{read_msg, write_msg};
use crate::journal::{JournalEntry, JournalMutation};
use crate::schema::{
    Capabilities, Command, Event, LeaseGrant, PROTOCOL_VERSION, RealmAction, RealmActionResult,
    RealmCapture, Request, Response, Scope,
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
    fn windows(&self) -> Vec<ass_core::window::Window>;
    /// Snapshot of the workspace/output model. Same shape the chrome and the
    /// agent read.
    fn workspaces(&self) -> ass_core::workspace::WorkspaceSnapshot;
    /// Snapshot of the live notification queue.
    fn notifications(&self) -> Vec<ass_core::notify::Notification>;
    /// Snapshot of the live outputs (connector + geometry).
    fn outputs(&self) -> Vec<ass_core::output::OutputInfo>;
    /// Snapshot of journal entries with `seq > since` (ADR-0033).
    fn journal_since(&self, since: u64) -> crate::journal::JournalSnapshot;
    /// Complete Realm authority snapshot.
    fn realms(&self) -> ass_core::realm::RealmSnapshot {
        ass_core::realm::RealmModel::new().snapshot()
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
    /// Capture the focused output as a PNG. Called from a connection thread;
    /// the implementation forwards to the main loop and blocks briefly for
    /// the reply. The writer transfers the PNG through a sealed memfd after
    /// the small JSON response. `region` is in compositor logical pixels;
    /// `None` captures the whole output.
    fn capture_output(
        &self,
        _region: Option<ass_core::Rect>,
    ) -> Result<CaptureOutputPayload, String> {
        Err("capture unsupported".into())
    }
    /// Capture one directed virtual output.
    fn capture_realm(
        &self,
        _realm: ass_core::realm::RealmId,
        _region: Option<ass_core::Rect>,
    ) -> Result<CaptureRealmPayload, String> {
        Err("realm capture unsupported".into())
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
        let next_sub = Arc::new(AtomicU64::new(0));
        let next_lease = Arc::new(AtomicU64::new(1));
        let next_conn = Arc::new(AtomicU64::new(1));
        let subs_for_accept = Arc::clone(&subs);
        let journal_subs_for_accept = Arc::clone(&journal_subs);
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

fn accept_loop<H: Handler + 'static>(
    listener: UnixListener,
    handler: Arc<H>,
    subs: Arc<Mutex<HashMap<SubId, Sender<Outbound>>>>,
    journal_subs: Arc<Mutex<HashMap<SubId, Sender<Outbound>>>>,
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
        let n = Arc::clone(&next_sub);
        let l = Arc::clone(&next_lease);
        let conn_id = next_conn.fetch_add(1, Ordering::Relaxed);
        let _ = thread::Builder::new()
            .name("ass-ipc-conn".into())
            .spawn(move || serve_connection(stream, h, s, js, n, l, conn_id));
    }
}

/// Run one connection: a writer thread owns the write half; the current
/// thread drives the read half and pushes responses/events through the
/// writer's inbox. On read close the reader removes both its coarse and
/// journal subscription entries so the writer sees its last sender
/// disappear and exits promptly.
fn serve_connection<H: Handler + 'static>(
    stream: UnixStream,
    handler: Arc<H>,
    subs: Arc<Mutex<HashMap<SubId, Sender<Outbound>>>>,
    journal_subs: Arc<Mutex<HashMap<SubId, Sender<Outbound>>>>,
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
                };
                if res.is_err() {
                    break;
                }
            }
        });

    let (sub_id, journal_sub_id) = drive_read_loop(
        &mut read_half,
        &tx,
        &*handler,
        &subs,
        &journal_subs,
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
    drop(tx);
    if let Ok(handle) = writer {
        let _ = handle.join();
    }
}

/// Drive the protocol on the read half. Returns `(coarse_sub_id,
/// journal_sub_id)` for cleanup; either or both may be `None`.
#[allow(clippy::too_many_arguments)]
fn drive_read_loop<H: Handler>(
    read: &mut UnixStream,
    tx: &Sender<Outbound>,
    handler: &H,
    subs: &Mutex<HashMap<SubId, Sender<Outbound>>>,
    journal_subs: &Mutex<HashMap<SubId, Sender<Outbound>>>,
    next_sub: &AtomicU64,
    next_lease: &AtomicU64,
    conn_id: u64,
) -> (Option<SubId>, Option<SubId>) {
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
                return (None, None);
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
                        return (None, None);
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
                    return (None, None);
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
            return (None, None);
        }
        Err(_) => return (None, None),
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
        return (None, None);
    }

    let mut sub_id: Option<SubId> = None;
    let mut journal_sub_id: Option<SubId> = None;
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
        };
        if tx.send(Outbound::Response(resp)).is_err() {
            break;
        }
    }
    (sub_id, journal_sub_id)
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
                        && realm.state == ass_core::realm::RealmState::Active
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
