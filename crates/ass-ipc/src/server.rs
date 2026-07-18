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
use crate::journal::JournalEntry;
use crate::schema::{Capabilities, Command, Event, Request, Response, Scope, PROTOCOL_VERSION};

/// Subscriber ids are globally unique across connections and subscription
/// types (coarse events vs. journal entries).
type SubId = u64;

/// What the writer thread sends on the wire. Both kinds go through one inbox
/// so the writer is the single owner of the write half.
#[derive(Debug, Clone)]
enum Outbound {
    Response(Response),
    Event(Event),
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
    /// Resolve a named scope from configuration (ADR-0034). Returns `None`
    /// if the name is unknown; the server then falls back to unscoped.
    fn resolve_scope(&self, _name: &str) -> Option<Scope> {
        None
    }
    /// Receive a control/session [`Command`]. Called from a connection
    /// thread; the implementation must forward to the compositor's main loop
    /// because the Wayland server state is not `Send`. Fire-and-forget: the
    /// caller acknowledges queuing, not completion.
    fn command(&self, cmd: Command);
    /// Capture the focused output as a PNG. Called from a connection thread;
    /// the implementation forwards to the main loop and blocks briefly for
    /// the reply. Returns `(width, height, png_base64)` or an error message
    /// (unsupported, locked session, capture failure). `region` is in
    /// compositor logical pixels; `None` captures the whole output.
    fn capture_output(
        &self,
        _region: Option<ass_core::Rect>,
    ) -> Result<(u32, u32, String), String> {
        Err("capture unsupported".into())
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
        let subs_for_accept = Arc::clone(&subs);
        let journal_subs_for_accept = Arc::clone(&journal_subs);
        let next_for_accept = Arc::clone(&next_sub);
        let accept = match thread::Builder::new()
            .name("ass-ipc-accept".into())
            .spawn(move || {
                accept_loop(
                    listener,
                    handler,
                    subs_for_accept,
                    journal_subs_for_accept,
                    next_for_accept,
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
) {
    for incoming in listener.incoming() {
        let Ok(stream) = incoming else {
            continue;
        };
        let h = Arc::clone(&handler);
        let s = Arc::clone(&subs);
        let js = Arc::clone(&journal_subs);
        let n = Arc::clone(&next_sub);
        let _ = thread::Builder::new()
            .name("ass-ipc-conn".into())
            .spawn(move || serve_connection(stream, h, s, js, n));
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
) {
    let mut read_half = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let (tx, rx) = mpsc::channel::<Outbound>();

    let writer = thread::Builder::new()
        .name("ass-ipc-writer".into())
        .spawn(move || {
            let mut w = stream;
            while let Ok(out) = rx.recv() {
                let res = match out {
                    Outbound::Response(r) => write_msg(&mut w, &r),
                    Outbound::Event(e) => write_msg(&mut w, &e),
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
fn drive_read_loop<H: Handler>(
    read: &mut UnixStream,
    tx: &Sender<Outbound>,
    handler: &H,
    subs: &Mutex<HashMap<SubId, Sender<Outbound>>>,
    journal_subs: &Mutex<HashMap<SubId, Sender<Outbound>>>,
    next_sub: &AtomicU64,
) -> (Option<SubId>, Option<SubId>) {
    let (granted, granted_scope, scope_name) = match read_msg::<_, Request>(read) {
        Ok(Request::Hello {
            version,
            caps,
            scope,
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
            (gc, gs, scope)
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
        }))
        .is_err()
    {
        return (None, None);
    }

    let mut sub_id: Option<SubId> = None;
    let mut journal_sub_id: Option<SubId> = None;
    while let Ok(req) = read_msg::<_, Request>(read) {
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
            Request::Do { cmd } => {
                let need = cmd.required_cap();
                let allowed = (need.control && granted.control)
                    || (need.input && granted.input)
                    || (need.session && granted.session);
                if !allowed {
                    Response::Error {
                        message: "command requires a capability not granted".into(),
                    }
                } else if !scope_name
                    .as_deref()
                    .map(|name| {
                        handler
                            .resolve_scope(name)
                            .is_some_and(|scope| scope.permits(&cmd))
                    })
                    .unwrap_or_else(|| granted_scope.permits(&cmd))
                {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else if let Err(message) = cmd.validate() {
                    Response::Error {
                        message: message.into(),
                    }
                } else {
                    handler.command(cmd);
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
            Request::CaptureOutput { region } => {
                // Pixel capture reads the screen back to the client, so it is
                // fail-closed like InjectInput: `control` plus an explicit
                // CaptureOutput op in the granted scope — never inherited
                // through None-means-all (ADR-0034).
                let op_allowed = granted_scope
                    .ops
                    .as_ref()
                    .is_some_and(|ops| ops.contains(&crate::schema::OpClass::CaptureOutput));
                if !granted.control {
                    Response::Error {
                        message: "CaptureOutput requires the control capability".into(),
                    }
                } else if !op_allowed {
                    Response::Error {
                        message: "out of scope".into(),
                    }
                } else {
                    match handler.capture_output(region) {
                        Ok((width, height, png_base64)) => Response::CaptureOutput {
                            width,
                            height,
                            png_base64,
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
