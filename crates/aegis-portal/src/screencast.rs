//! `org.freedesktop.impl.portal.ScreenCast` v2, monitor-only path.
//!
//! The three-call contract: `CreateSession` registers a Session object and
//! returns its handle; `SelectSources` validates the requested options and
//! arms the session; `Start` spawns the cast thread (compositor frame
//! stream → PipeWire producer, ADR-0052) and answers with the PipeWire
//! node id. `SelectSources` and `Start` are request/response style like
//! `Screenshot`; all blocking work happens on the screencast worker.
//!
//! Scope of this phase: source types `monitor` and `window`, one stream per
//! session, cursor modes Hidden | Metadata (no cursor is ever captured).
//! Without a window in the requested mask the focused output is cast as
//! before. When the mask offers a window, `SelectSources` runs the
//! compositor's window picker (`PickTarget`, ADR-0054): click a window,
//! press Enter (or click empty desktop) for the whole output, Escape to
//! cancel. A window stream crops the window's visible region from the
//! output frame — occluded parts show the occluder, and the stream ends if
//! the window closes or its size changes. Version 2 adds `persist_mode` 1
//! (token kept in memory — aegis has no application-exit tracking, so "until
//! the application exits" degrades to "until the portal restarts") and 2
//! (token persisted as `$XDG_DATA_HOME/aegis-portal/screencast-tokens.json`,
//! ADR-0053). A valid `restore_token` skips confirmation — the unguessable
//! token *is* the authorization credential — and `Start` returns the
//! session's `restore_token` in its results.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use zbus::zvariant::{
    Array, Dict, ObjectPath, OwnedObjectPath, Structure, StructureBuilder, Value,
};

use crate::cast;
use crate::request::{RequestIface, RequestTracker};
use crate::screenshot::{request_path, sanitize_token, sender_element};
use crate::session::{CastSource, SessionIface, SessionRegistry};

const REQUEST_IFACE: &str = "org.freedesktop.impl.portal.Request";
const SESSION_IFACE: &str = "org.freedesktop.impl.portal.Session";

/// The served interface version: 2 = `persist_mode` + `restore_token`.
pub(crate) const SCREENCAST_VERSION: u32 = 2;
/// `AvailableSourceTypes` bit: monitor.
const SOURCE_TYPE_MONITOR: u32 = 1;
/// `AvailableSourceTypes` bit: window (ADR-0054).
const SOURCE_TYPE_WINDOW: u32 = 2;
/// `AvailableCursorModes`: Hidden | Embedded is not captured; Metadata only.
const CURSOR_MODES: u32 = 1 | 4;
/// Waiting for the PipeWire negotiation longer than this is a failure.
const START_TIMEOUT: Duration = Duration::from_secs(10);
/// The state document holding persisted (mode 2) restore tokens.
const TOKENS_DOC: &str = "screencast-tokens.json";

/// One job handed from the bus methods to the screencast worker.
pub(crate) enum CastJob {
    SelectSources {
        request_path: String,
        session_path: String,
        app_id: String,
        source_types: u32,
        persist_mode: u32,
        cursor_mode: u32,
        restore_token: Option<String>,
    },
    Start {
        request_path: String,
        session_path: String,
        app_id: String,
    },
    /// The client called `Session.Close`.
    CloseSession { session_path: String },
    /// The compositor ended the stream (scope revoked, lease lapsed,
    /// disconnect); reported by the cast thread.
    SessionEnded { session_path: String },
}

/// Options parsed out of the `SelectSources` argument.
pub(crate) struct SelectOptions {
    pub(crate) handle_token: Option<String>,
    /// Requested source-type mask; must intersect what we offer.
    pub(crate) source_types: u32,
    pub(crate) cursor_mode: u32,
    pub(crate) persist_mode: u32,
    pub(crate) restore_token: Option<String>,
}

/// Parse `SelectSources` options. Unknown keys are ignored per spec.
pub(crate) fn parse_select_options(options: &HashMap<String, Value<'_>>) -> SelectOptions {
    let string = |key: &str| {
        options
            .get(key)
            .and_then(|value| match value {
                Value::Str(token) => Some(token.to_string()),
                _ => None,
            })
            .filter(|token| !token.is_empty() && sanitize_token(token) == *token)
    };
    let handle_token = string("handle_token");
    let restore_token = string("restore_token");
    let source_types = options
        .get("types")
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(SOURCE_TYPE_MONITOR);
    let cursor_mode = options
        .get("cursor_mode")
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0);
    let persist_mode = options
        .get("persist_mode")
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(0);
    SelectOptions {
        handle_token,
        source_types,
        cursor_mode,
        persist_mode,
        restore_token,
    }
}

/// Why a `SelectSources` option set cannot be served, as a D-Bus message.
fn validate_select(options: &SelectOptions) -> Result<(), String> {
    if options.source_types & (SOURCE_TYPE_MONITOR | SOURCE_TYPE_WINDOW) == 0 {
        return Err("only monitor and window sources are supported".to_string());
    }
    if options.cursor_mode & !CURSOR_MODES != 0 {
        return Err(format!(
            "cursor_mode {} is not supported (Hidden | Metadata only)",
            options.cursor_mode
        ));
    }
    if options.persist_mode > 2 {
        return Err(format!("unknown persist_mode {}", options.persist_mode));
    }
    Ok(())
}

/// Fallback request tokens mirror the screenshot path.
pub(crate) fn fallback_token(handle: &ObjectPath<'_>) -> String {
    handle
        .as_str()
        .rsplit('/')
        .next()
        .filter(|tail| !tail.is_empty())
        .map(sanitize_token)
        .unwrap_or_else(|| {
            static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            format!(
                "aegis{}",
                COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            )
        })
}

/// The served ScreenCast interface. Methods register request/session
/// objects and enqueue; the worker does everything blocking.
pub(crate) struct ScreenCastIface {
    pub(crate) conn: zbus::Connection,
    pub(crate) tracker: Arc<Mutex<RequestTracker>>,
    pub(crate) sessions: Arc<Mutex<SessionRegistry>>,
    pub(crate) jobs: mpsc::Sender<CastJob>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.ScreenCast")]
impl ScreenCastIface {
    async fn create_session(
        &self,
        options: HashMap<String, Value<'_>>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let token = options
            .get("session_handle_token")
            .and_then(|value| match value {
                Value::Str(token) => Some(token.to_string()),
                _ => None,
            })
            .filter(|token| !token.is_empty() && sanitize_token(token) == *token)
            .ok_or_else(|| {
                zbus::fdo::Error::InvalidArgs(
                    "CreateSession requires a valid session_handle_token".to_string(),
                )
            })?;
        let sender = header.sender().map(|name| name.as_str());
        let path = format!(
            "/org/freedesktop/portal/desktop/session/{}/{}",
            sender_element(sender),
            token
        );

        self.sessions
            .lock()
            .unwrap()
            .insert(&path, String::new())
            .map_err(zbus::fdo::Error::Failed)?;
        self.conn
            .object_server()
            .at(
                path.as_str(),
                SessionIface {
                    path: path.clone(),
                    jobs: self.jobs.clone(),
                },
            )
            .await
            .map_err(zbus::fdo::Error::from)?;
        log::info!("portal: screencast session {path} created");
        OwnedObjectPath::try_from(path).map_err(|error| zbus::fdo::Error::Failed(error.to_string()))
    }

    async fn select_sources(
        &self,
        handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        app_id: &str,
        options: HashMap<String, Value<'_>>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let session_path = session_handle.as_str().to_string();
        if !self.sessions.lock().unwrap().contains(&session_path) {
            return Err(zbus::fdo::Error::Failed(format!(
                "unknown session {session_path}"
            )));
        }
        let options = parse_select_options(&options);
        validate_select(&options).map_err(zbus::fdo::Error::InvalidArgs)?;

        let sender = header.sender().map(|name| name.as_str());
        let token = options
            .handle_token
            .unwrap_or_else(|| fallback_token(&handle));
        let path = request_path(sender, &token);
        log::info!("portal: SelectSources for '{app_id}' on {session_path} at {path}");

        self.register_request(&path).await?;
        self.enqueue(CastJob::SelectSources {
            request_path: path.clone(),
            session_path,
            app_id: app_id.to_string(),
            source_types: options.source_types,
            persist_mode: options.persist_mode,
            cursor_mode: options.cursor_mode,
            restore_token: options.restore_token.clone(),
        })?;
        OwnedObjectPath::try_from(path).map_err(|error| zbus::fdo::Error::Failed(error.to_string()))
    }

    async fn start(
        &self,
        handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        app_id: &str,
        _parent_window: &str,
        options: HashMap<String, Value<'_>>,
        #[zbus(header)] header: zbus::message::Header<'_>,
    ) -> zbus::fdo::Result<OwnedObjectPath> {
        let session_path = session_handle.as_str().to_string();
        if !self.sessions.lock().unwrap().contains(&session_path) {
            return Err(zbus::fdo::Error::Failed(format!(
                "unknown session {session_path}"
            )));
        }
        let sender = header.sender().map(|name| name.as_str());
        let handle_token = options
            .get("handle_token")
            .and_then(|value| match value {
                Value::Str(token) => Some(token.to_string()),
                _ => None,
            })
            .filter(|token| !token.is_empty() && sanitize_token(token) == *token);
        let token = handle_token.unwrap_or_else(|| fallback_token(&handle));
        let path = request_path(sender, &token);
        log::info!("portal: Start for '{app_id}' on {session_path} at {path}");

        self.register_request(&path).await?;
        self.enqueue(CastJob::Start {
            request_path: path.clone(),
            session_path,
            app_id: app_id.to_string(),
        })?;
        OwnedObjectPath::try_from(path).map_err(|error| zbus::fdo::Error::Failed(error.to_string()))
    }

    #[zbus(property, name = "AvailableSourceTypes")]
    fn available_source_types(&self) -> u32 {
        SOURCE_TYPE_MONITOR | SOURCE_TYPE_WINDOW
    }

    #[zbus(property, name = "AvailableCursorModes")]
    fn available_cursor_modes(&self) -> u32 {
        CURSOR_MODES
    }

    #[zbus(property)]
    fn version(&self) -> u32 {
        SCREENCAST_VERSION
    }
}

/// Non-D-Bus helpers, kept out of the `#[zbus::interface]` block so the
/// macro does not try to export them as methods.
impl ScreenCastIface {
    async fn register_request(&self, path: &str) -> zbus::fdo::Result<()> {
        self.conn
            .object_server()
            .at(
                path,
                RequestIface {
                    path: path.to_string(),
                    tracker: Arc::clone(&self.tracker),
                },
            )
            .await
            .map(|_| ())
            .map_err(zbus::fdo::Error::from)
    }

    fn enqueue(&self, job: CastJob) -> zbus::fdo::Result<()> {
        self.jobs
            .send(job)
            .map_err(|_| zbus::fdo::Error::Failed("screencast worker is gone".to_string()))
    }
}

/// Worker loop: one job at a time. Only `Start` blocks (IPC connect +
/// PipeWire negotiation, bounded by the cast thread's own failures and
/// [`START_TIMEOUT`]); serializing casts is the natural pacing.
pub(crate) fn cast_worker(
    rx: mpsc::Receiver<CastJob>,
    jobs: mpsc::Sender<CastJob>,
    conn: zbus::blocking::Connection,
    tracker: Arc<Mutex<RequestTracker>>,
    sessions: Arc<Mutex<SessionRegistry>>,
    socket: PathBuf,
    mut tokens: TokenStore,
) {
    // The worker's own scoped client drives the interactive window picker
    // and validates restored window ids against the live window list.
    let mut picker = crate::ipc::PortalCapture::new(socket.clone());
    while let Ok(job) = rx.recv() {
        match job {
            CastJob::SelectSources {
                request_path,
                session_path,
                app_id,
                source_types,
                persist_mode,
                cursor_mode,
                restore_token,
            } => {
                let code = select_sources(
                    &tracker,
                    &sessions,
                    &mut tokens,
                    &mut picker,
                    &request_path,
                    &session_path,
                    &app_id,
                    source_types,
                    persist_mode,
                    cursor_mode,
                    restore_token.as_deref(),
                );
                log::info!("portal: SelectSources for '{app_id}' → response {code}");
                finish_request(&conn, &tracker, &request_path, code, HashMap::new());
            }
            CastJob::Start {
                request_path,
                session_path,
                app_id,
            } => {
                let (code, results) = start_cast(
                    &tracker,
                    &sessions,
                    &jobs,
                    &socket,
                    &mut picker,
                    &request_path,
                    &session_path,
                );
                log::info!("portal: Start for '{app_id}' → response {code}");
                finish_request(&conn, &tracker, &request_path, code, results);
                // A failed Start leaves the session armed but idle; the
                // client may retry Start or close the session itself.
            }
            CastJob::CloseSession { session_path } | CastJob::SessionEnded { session_path } => {
                close_session(&conn, &sessions, &session_path);
            }
        }
    }
}

/// Arm the session. Response codes: 0 ok, 1 cancelled, 2 refused.
///
/// A valid `restore_token` (recorded source among the requested types, and
/// for a window the window still live) skips the picker entirely: the token
/// is the authorization credential. Otherwise the source comes from the
/// requested mask — monitor-only casts the focused output as before, a
/// window bit runs the compositor's window picker (ADR-0054). The persist
/// half of v2: for `persist_mode` 1/2 the session carries a restore token —
/// the caller's valid token when it presents one, a freshly minted one
/// otherwise. Mode 1 tokens are memory-only, mode 2 tokens persist in the
/// state directory.
#[allow(clippy::too_many_arguments)]
fn select_sources(
    tracker: &Arc<Mutex<RequestTracker>>,
    sessions: &Arc<Mutex<SessionRegistry>>,
    tokens: &mut TokenStore,
    picker: &mut crate::ipc::PortalCapture,
    request_path: &str,
    session_path: &str,
    app_id: &str,
    source_types: u32,
    persist_mode: u32,
    cursor_mode: u32,
    restore_token: Option<&str>,
) -> u32 {
    if tracker.lock().unwrap().was_closed(request_path) {
        return 1;
    }
    // A presented token restores only a source the caller still requests;
    // anything else is treated as no token (logged) and re-picked/re-minted.
    let restored = if persist_mode > 0 {
        restore_token.and_then(|token| {
            let record = tokens.lookup(token)?;
            match record.source {
                TokenSource::Monitor if source_types & SOURCE_TYPE_MONITOR != 0 => {
                    Some((token.to_string(), CastSource::Monitor))
                }
                TokenSource::Window { window } if source_types & SOURCE_TYPE_WINDOW != 0 => {
                    match window_live(picker, window) {
                        true => Some((
                            token.to_string(),
                            CastSource::Window(aegis_core::window::WindowId(window)),
                        )),
                        false => {
                            log::info!(
                                "portal: restored window {window} is gone; minting a fresh token"
                            );
                            None
                        }
                    }
                }
                _ => {
                    log::info!(
                        "portal: restore token's source is not among the requested types; minting a fresh one"
                    );
                    None
                }
            }
        })
    } else {
        None
    };
    let (token, source) = match restored {
        Some(restored) => (Some(restored.0), restored.1),
        None => {
            if persist_mode > 0 && restore_token.is_some() {
                log::info!("portal: unknown restore token from '{app_id}'; minting a fresh one");
            }
            let source = match pick_source(picker, source_types) {
                Ok(source) => source,
                Err(code) => return code,
            };
            // A Close racing the picker wins over the completed selection.
            if tracker.lock().unwrap().was_closed(request_path) {
                return 1;
            }
            let token = if persist_mode > 0 {
                match tokens.mint(app_id, cursor_mode, persist_mode, source) {
                    Ok(token) => Some(token),
                    Err(error) => {
                        log::warn!("portal: cannot mint a restore token: {error}");
                        None
                    }
                }
            } else {
                None
            };
            (token, source)
        }
    };
    match sessions
        .lock()
        .unwrap()
        .mark_sources_selected(session_path, token, source)
    {
        Ok(()) => 0,
        Err(error) => {
            log::warn!("portal: SelectSources refused: {error}");
            2
        }
    }
}

/// Decide the session's source from the requested mask. Monitor-only masks
/// skip interaction (the focused output is the only choice); a window bit
/// runs the compositor's picker: a clicked window, Enter/empty-desktop for
/// the whole output, or Escape to cancel (ADR-0054). `Err(code)` is the
/// SelectSources response code.
fn pick_source(
    picker: &mut crate::ipc::PortalCapture,
    source_types: u32,
) -> Result<CastSource, u32> {
    if source_types & SOURCE_TYPE_WINDOW == 0 {
        return Ok(CastSource::Monitor);
    }
    match picker.pick(aegis_ipc::PickKind::Window) {
        Ok(aegis_ipc::PickResult::Window { id }) => Ok(CastSource::Window(id)),
        Ok(aegis_ipc::PickResult::Output) if source_types & SOURCE_TYPE_MONITOR != 0 => {
            Ok(CastSource::Monitor)
        }
        // The picker offered the whole output but the client did not.
        Ok(aegis_ipc::PickResult::Output) => {
            log::info!("portal: whole-output pick on a window-only request; refusing");
            Err(2)
        }
        Ok(aegis_ipc::PickResult::Cancelled) => Err(1),
        Ok(other) => {
            log::warn!("portal: window pick answered with {other:?}");
            Err(2)
        }
        Err(error) => {
            log::warn!("portal: window pick failed: {error}");
            Err(2)
        }
    }
}

/// Whether a restored window id still names a live window.
fn window_live(picker: &mut crate::ipc::PortalCapture, window: u64) -> bool {
    picker
        .windows()
        .map(|windows| windows.iter().any(|candidate| candidate.id.0 == window))
        .unwrap_or(false)
}

/// Spawn the cast and report the negotiated stream. Response codes follow
/// the portal spec (0 ok, 1 cancelled, 2 error).
#[allow(clippy::too_many_arguments)]
fn start_cast(
    tracker: &Arc<Mutex<RequestTracker>>,
    sessions: &Arc<Mutex<SessionRegistry>>,
    jobs: &mpsc::Sender<CastJob>,
    socket: &std::path::Path,
    picker: &mut crate::ipc::PortalCapture,
    request_path: &str,
    session_path: &str,
) -> (u32, HashMap<String, Value<'static>>) {
    if tracker.lock().unwrap().was_closed(request_path) {
        return (1, HashMap::new());
    }
    let (source, window_geometry) = {
        let sessions = sessions.lock().unwrap();
        if let Err(error) = sessions.can_start(session_path) {
            log::warn!("portal: Start refused: {error}");
            return (2, HashMap::new());
        }
        let source = sessions.source(session_path).unwrap_or(CastSource::Monitor);
        drop(sessions);
        // A window source reports its compositor-logical geometry in the
        // stream properties; a window gone at Start time fails the Start.
        let geometry = match source {
            CastSource::Monitor => None,
            CastSource::Window(id) => match picker
                .windows()
                .ok()
                .and_then(|windows| windows.into_iter().find(|w| w.id == id))
            {
                Some(window) => Some((window.position, window.size)),
                None => {
                    log::warn!("portal: Start refused: window {} is gone", id.0);
                    return (2, HashMap::new());
                }
            },
        };
        (source, geometry)
    };
    let window = match source {
        CastSource::Monitor => None,
        CastSource::Window(id) => Some(id),
    };
    // The cast thread reports compositor-side stream ends back to this
    // worker through a clone of the worker's own job channel.
    let handle = match cast::spawn(
        socket.to_path_buf(),
        session_path.to_string(),
        jobs.clone(),
        window,
    ) {
        Ok(handle) => handle,
        Err(error) => {
            log::warn!("portal: could not spawn cast for {session_path}: {error}");
            return (2, HashMap::new());
        }
    };
    match handle.started.recv_timeout(START_TIMEOUT) {
        Ok(Ok(started)) => {
            // A Close racing the negotiation wins over a started cast.
            if tracker.lock().unwrap().was_closed(request_path) {
                drop(handle.stop);
                let _ = handle.thread.join();
                return (1, HashMap::new());
            }
            sessions
                .lock()
                .unwrap()
                .mark_started(session_path, handle.stop, handle.thread);
            log::info!(
                "portal: cast for {session_path} live as pipewire node {} ({}x{}, {:?})",
                started.node_id,
                started.width,
                started.height,
                source
            );
            let (source_type, position, size) = match window_geometry {
                Some((position, size)) => (
                    SOURCE_TYPE_WINDOW,
                    (position.x, position.y),
                    (size.w, size.h),
                ),
                None => (
                    SOURCE_TYPE_MONITOR,
                    (0, 0),
                    (started.width as i32, started.height as i32),
                ),
            };
            let mut results = HashMap::from([(
                "streams".to_string(),
                streams_value(started.node_id, source_type, position, size),
            )]);
            // v2: an armed session answers with its restore token, the
            // authorization credential a later SelectSources can present.
            if let Some(token) = sessions.lock().unwrap().restore_token(session_path) {
                results.insert("restore_token".to_string(), Value::from(token));
            }
            (0, results)
        }
        Ok(Err(error)) => {
            log::warn!("portal: cast for {session_path} failed: {error}");
            drop(handle.stop);
            let _ = handle.thread.join();
            (2, HashMap::new())
        }
        Err(_) => {
            log::warn!("portal: cast for {session_path} timed out during negotiation");
            drop(handle.stop);
            let _ = handle.thread.join();
            (2, HashMap::new())
        }
    }
}

/// Stop the cast (if any), emit `Closed`, and remove the session object.
fn close_session(
    conn: &zbus::blocking::Connection,
    sessions: &Arc<Mutex<SessionRegistry>>,
    session_path: &str,
) {
    if sessions.lock().unwrap().remove(session_path).is_none() {
        return;
    }
    log::info!("portal: screencast session {session_path} closed");
    if let Err(error) = conn.emit_signal(None::<&str>, session_path, SESSION_IFACE, "Closed", &()) {
        log::warn!("portal: could not emit Closed for {session_path}: {error}");
    }
    if let Err(error) = conn.object_server().remove::<SessionIface, _>(session_path) {
        log::warn!("portal: could not remove {session_path}: {error}");
    }
}

/// Emit `Response` on a request object, then remove it and forget its
/// cancellation state (same lifecycle as the screenshot worker).
fn finish_request(
    conn: &zbus::blocking::Connection,
    tracker: &Arc<Mutex<RequestTracker>>,
    request_path: &str,
    code: u32,
    results: HashMap<String, Value<'static>>,
) {
    if let Err(error) = conn.emit_signal(
        None::<&str>,
        request_path,
        REQUEST_IFACE,
        "Response",
        &(code, results),
    ) {
        log::warn!("portal: could not emit Response for {request_path}: {error}");
    }
    if let Err(error) = conn.object_server().remove::<RequestIface, _>(request_path) {
        log::warn!("portal: could not remove {request_path}: {error}");
    }
    tracker.lock().unwrap().forget(request_path);
}

/// The `streams` result: `a(ua{sv})` with the PipeWire node id, the
/// source's position and size in compositor coordinates, and its source
/// type (monitor or window, ADR-0054).
fn streams_value(
    node_id: u32,
    source_type: u32,
    position: (i32, i32),
    size: (i32, i32),
) -> Value<'static> {
    let properties: HashMap<String, Value> = HashMap::from([
        (
            "position".to_string(),
            Value::Structure(Structure::from(position)),
        ),
        ("size".to_string(), Value::Structure(Structure::from(size))),
        ("source_type".to_string(), Value::U32(source_type)),
    ]);
    // `append_field` keeps each field's dynamic signature, so the structure
    // types as `(ua{sv})`; `Structure::from` would route the fields through
    // `Value::new` and wrap them as variants (`(vv)`).
    let stream = StructureBuilder::new()
        .append_field(Value::U32(node_id))
        .append_field(Value::Dict(Dict::from(properties)))
        .build()
        .expect("non-empty structure");
    // The array must carry the element signature `(ua{sv})` — building it
    // from a `Vec<Value>` would type it as `av`, which the frontend cannot
    // deserialize as the spec's `a(ua{sv})`.
    let mut streams =
        Array::new(&zbus::zvariant::Signature::try_from("(ua{sv})").expect("valid signature"));
    streams
        .append(Value::Structure(stream))
        .expect("stream element matches");
    Value::Array(streams)
}

/// The source a restore token stands for (ADR-0054). Documents persisted
/// before window sources existed carry no `source` key and decode as
/// `Monitor`, their original meaning.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(tag = "type")]
pub(crate) enum TokenSource {
    #[default]
    Monitor,
    Window {
        window: u64,
    },
}

impl From<CastSource> for TokenSource {
    fn from(source: CastSource) -> Self {
        match source {
            CastSource::Monitor => TokenSource::Monitor,
            CastSource::Window(id) => TokenSource::Window { window: id.0 },
        }
    }
}

/// What a restore token stands for: the requesting application, the cursor
/// mode, and the source the grant was minted for. The record exists so a
/// presented token can be validated and so a future permission UI can list
/// or revoke grants.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct TokenRecord {
    pub(crate) app_id: String,
    pub(crate) cursor_mode: u32,
    #[serde(default)]
    pub(crate) source: TokenSource,
}

#[derive(Debug, Default, serde::Serialize, serde::Deserialize)]
struct TokenDoc {
    tokens: HashMap<String, TokenRecord>,
}

/// Restore-token store, owned by the screencast worker. Mode 2 tokens live
/// in `persistent` and are written through to the state directory on every
/// change; mode 1 tokens live in `transient` and die with the process (aegis
/// has no application-exit tracking, so "until the application exits"
/// degrades to "until the portal restarts").
pub(crate) struct TokenStore {
    dir: Option<PathBuf>,
    persistent: HashMap<String, TokenRecord>,
    transient: HashMap<String, TokenRecord>,
}

impl TokenStore {
    pub(crate) fn load(dir: Option<PathBuf>) -> Self {
        let doc: TokenDoc = dir
            .as_ref()
            .and_then(|dir| crate::state::read_json(dir, TOKENS_DOC))
            .unwrap_or_default();
        Self {
            dir,
            persistent: doc.tokens,
            transient: HashMap::new(),
        }
    }

    pub(crate) fn lookup(&self, token: &str) -> Option<&TokenRecord> {
        self.persistent
            .get(token)
            .or_else(|| self.transient.get(token))
    }

    /// Mint a fresh unguessable token for `app_id`. Mode 2 persists it;
    /// anything else keeps it in memory.
    pub(crate) fn mint(
        &mut self,
        app_id: &str,
        cursor_mode: u32,
        persist_mode: u32,
        source: CastSource,
    ) -> std::io::Result<String> {
        let record = TokenRecord {
            app_id: app_id.to_string(),
            cursor_mode,
            source: source.into(),
        };
        let mut token = crate::state::random_token()?;
        while self.lookup(&token).is_some() {
            token = crate::state::random_token()?;
        }
        if persist_mode == 2 {
            self.persistent.insert(token.clone(), record);
            self.save()?;
        } else {
            self.transient.insert(token.clone(), record);
        }
        Ok(token)
    }

    /// Drop a token (both maps), persisting the removal. Returns whether a
    /// token was found. Revocation is backend-internal today — a future
    /// PermissionStore integration or settings UI calls the same path.
    #[allow(dead_code)]
    pub(crate) fn revoke(&mut self, token: &str) -> bool {
        let found =
            self.transient.remove(token).is_some() | self.persistent.remove(token).is_some();
        if found {
            self.save().ok();
        }
        found
    }

    fn save(&self) -> std::io::Result<()> {
        let Some(dir) = &self.dir else {
            return Ok(());
        };
        let doc = TokenDoc {
            tokens: self.persistent.clone(),
        };
        crate::state::write_json(dir, TOKENS_DOC, &doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(pairs: &[(&str, Value<'static>)]) -> HashMap<String, Value<'static>> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect()
    }

    #[test]
    fn select_options_default_to_monitor_without_persist() {
        let parsed = parse_select_options(&HashMap::new());
        assert_eq!(parsed.source_types, SOURCE_TYPE_MONITOR);
        assert_eq!(parsed.persist_mode, 0);
        assert_eq!(parsed.cursor_mode, 0);
        assert_eq!(parsed.restore_token, None);
        assert!(validate_select(&parsed).is_ok());
    }

    #[test]
    fn select_options_accept_monitor_and_window_mix() {
        // types = monitor | window is servable: we deliver the monitor part.
        let parsed = parse_select_options(&options(&[("types", Value::from(0b11u32))]));
        assert!(validate_select(&parsed).is_ok());
    }

    #[test]
    fn select_options_accept_persist_modes_1_and_2() {
        for mode in [1u32, 2] {
            let parsed = parse_select_options(&options(&[("persist_mode", Value::from(mode))]));
            assert_eq!(parsed.persist_mode, mode);
            assert!(validate_select(&parsed).is_ok());
        }
        let beyond = parse_select_options(&options(&[("persist_mode", Value::from(3u32))]));
        assert!(validate_select(&beyond).is_err());
    }

    #[test]
    fn select_options_accept_restore_token_and_advertised_cursor_modes() {
        let parsed = parse_select_options(&options(&[
            ("restore_token", Value::from("abcdef0123456789")),
            ("cursor_mode", Value::from(4u32)),
            ("persist_mode", Value::from(2u32)),
        ]));
        assert_eq!(parsed.restore_token.as_deref(), Some("abcdef0123456789"));
        assert_eq!(parsed.cursor_mode, 4);
        assert!(validate_select(&parsed).is_ok());

        // Embedded cursor (2) is not offered: no cursor is ever captured.
        let embedded = parse_select_options(&options(&[("cursor_mode", Value::from(2u32))]));
        assert!(validate_select(&embedded).is_err());
    }

    #[test]
    fn select_options_accept_window_only_sources() {
        let window_only = parse_select_options(&options(&[("types", Value::from(0b10u32))]));
        assert_eq!(window_only.source_types, SOURCE_TYPE_WINDOW);
        assert!(validate_select(&window_only).is_ok());
    }

    #[test]
    fn select_options_refuse_unknown_source_bits() {
        let unknown = parse_select_options(&options(&[("types", Value::from(0b100u32))]));
        assert!(validate_select(&unknown).is_err());
    }

    #[test]
    fn screencast_version_is_2() {
        assert_eq!(SCREENCAST_VERSION, 2);
    }

    #[test]
    fn token_store_mint_lookup_revoke_round_trip() {
        let dir = std::env::temp_dir().join(format!(
            "aegis-portal-token-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let mut store = TokenStore::load(Some(dir.clone()));
        // Mode 2 persists; mode 1 stays in memory.
        let persisted = store
            .mint("org.example.App", 1, 2, CastSource::Monitor)
            .unwrap();
        let transient = store
            .mint(
                "org.example.App",
                4,
                1,
                CastSource::Window(aegis_core::window::WindowId(9)),
            )
            .unwrap();
        assert_ne!(persisted, transient);
        assert_eq!(
            store.lookup(&persisted),
            Some(&TokenRecord {
                app_id: "org.example.App".to_string(),
                cursor_mode: 1,
                source: TokenSource::Monitor,
            })
        );
        assert_eq!(
            store.lookup(&transient),
            Some(&TokenRecord {
                app_id: "org.example.App".to_string(),
                cursor_mode: 4,
                source: TokenSource::Window { window: 9 },
            })
        );

        // A reload sees only the mode 2 token.
        let reloaded = TokenStore::load(Some(dir.clone()));
        assert!(reloaded.lookup(&persisted).is_some());
        assert!(reloaded.lookup(&transient).is_none());

        // Revocation removes and persists the removal.
        let mut store = reloaded;
        assert!(!store.revoke("no-such-token"));
        assert!(store.revoke(&persisted));
        assert!(store.lookup(&persisted).is_none());
        let reloaded = TokenStore::load(Some(dir.clone()));
        assert!(reloaded.lookup(&persisted).is_none());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn token_store_degrades_to_memory_without_a_state_dir() {
        let mut store = TokenStore::load(None);
        let token = store
            .mint("org.example.App", 1, 2, CastSource::Monitor)
            .unwrap();
        assert!(store.lookup(&token).is_some());
    }

    #[test]
    fn select_options_ignore_wrong_types() {
        let parsed = parse_select_options(&options(&[
            ("types", Value::from("monitor")),
            ("persist_mode", Value::from(true)),
        ]));
        assert!(validate_select(&parsed).is_ok());
    }

    #[test]
    fn streams_value_has_portal_shape() {
        let value = streams_value(42, SOURCE_TYPE_MONITOR, (0, 0), (1920, 1080));
        let Value::Array(array) = &value else {
            panic!("streams must be an array");
        };
        assert_eq!(array.len(), 1);
        // Signature: a(ua{sv}) — the frontend's deserialize expects exactly
        // this shape, so pin it.
        assert_eq!(
            value.value_signature().to_string(),
            "a(ua{sv})",
            "streams signature"
        );
    }

    #[test]
    fn streams_value_reports_window_source_geometry() {
        let value = streams_value(7, SOURCE_TYPE_WINDOW, (40, 60), (800, 600));
        let Value::Array(array) = &value else {
            panic!("streams must be an array");
        };
        let stream: Value = array.get(0).expect("read").expect("one stream");
        let Value::Structure(stream) = stream else {
            panic!("stream element must be a structure");
        };
        let fields = stream.fields();
        let Value::Dict(properties) = &fields[1] else {
            panic!("stream properties must be a dict");
        };
        let get = |key: &str| {
            properties
                .iter()
                .find(|(k, _)| match k {
                    Value::Str(s) => s.as_str() == key,
                    _ => false,
                })
                .map(|(_, v)| match v {
                    Value::Value(inner) => (**inner).clone(),
                    other => other.clone(),
                })
        };
        assert_eq!(get("source_type"), Some(Value::U32(SOURCE_TYPE_WINDOW)));
        assert_eq!(
            get("position"),
            Some(Value::Structure(Structure::from((40i32, 60i32))))
        );
        assert_eq!(
            get("size"),
            Some(Value::Structure(Structure::from((800i32, 600i32))))
        );
    }

    #[test]
    fn persisted_tokens_without_a_source_decode_as_monitor() {
        let record: TokenRecord =
            serde_json::from_str(r#"{"app_id":"org.example.App","cursor_mode":1}"#)
                .expect("legacy token record decodes");
        assert_eq!(record.source, TokenSource::Monitor);

        let window = TokenRecord {
            app_id: "org.example.App".to_string(),
            cursor_mode: 4,
            source: TokenSource::Window { window: 42 },
        };
        let json = serde_json::to_string(&window).unwrap();
        assert_eq!(serde_json::from_str::<TokenRecord>(&json).unwrap(), window);
    }
}
