//! `org.freedesktop.impl.portal.ScreenCast` v3.
//!
//! The portal frontend supplies Request and Session object paths to the
//! backend. `CreateSession` exports the Session object, `SelectSources`
//! requires a user-confirmed compositor picker, and `Start` spawns the cast
//! thread (compositor frame stream → PipeWire producer, ADR-0052). Blocking
//! picker and PipeWire work stays on the screencast worker; D-Bus methods
//! asynchronously await the backend `(response, results)` tuple.
//!
//! Scope of this phase: source types `monitor` and `window`, one stream per
//! session, cursor mode Hidden only. Selection always runs the compositor's
//! picker (`PickTarget`, ADR-0054): click a window, press Enter (or click
//! empty desktop) for the whole output, Escape to cancel. A window stream
//! crops the window's visible region from the output frame — occluded parts
//! show the occluder, and the stream ends if the window closes or its size
//! changes. Persistence is not advertised: version 4's `restore_data` belongs
//! to the portal frontend's PermissionStore contract and is deferred until
//! Aegis can implement that ABI exactly.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use zbus::zvariant::{Array, Dict, ObjectPath, Structure, StructureBuilder, Value};

use crate::cast;
use crate::request::{PortalResponse, RequestTracker, ResponseSender};
use crate::session::{CastSource, SessionIface, SessionRegistry};

const SESSION_IFACE: &str = "org.freedesktop.impl.portal.Session";

/// Version 3 adds `source_type` stream properties. Persistence starts at v4
/// and is intentionally not advertised.
pub(crate) const SCREENCAST_VERSION: u32 = 3;
/// `AvailableSourceTypes` bit: monitor.
const SOURCE_TYPE_MONITOR: u32 = 1;
/// `AvailableSourceTypes` bit: window (ADR-0054).
const SOURCE_TYPE_WINDOW: u32 = 2;
/// `AvailableCursorModes`: Hidden only. No cursor metadata is produced.
const CURSOR_MODES: u32 = 1;
/// Waiting for the PipeWire negotiation longer than this is a failure.
const START_TIMEOUT: Duration = Duration::from_secs(10);
/// One job handed from the bus methods to the screencast worker.
pub(crate) enum CastJob {
    SelectSources {
        request_path: String,
        session_path: String,
        app_id: String,
        source_types: u32,
        cursor_mode: u32,
        reply: ResponseSender,
    },
    Start {
        request_path: String,
        session_path: String,
        app_id: String,
        reply: ResponseSender,
    },
    /// The client called `Session.Close`.
    CloseSession { session_path: String },
    /// The compositor ended the stream (scope revoked, lease lapsed,
    /// disconnect); reported by the cast thread.
    SessionEnded { session_path: String },
}

/// Options parsed out of the `SelectSources` argument.
pub(crate) struct SelectOptions {
    /// Requested source-type mask; must intersect what we offer.
    pub(crate) source_types: u32,
    pub(crate) cursor_mode: u32,
}

/// Parse `SelectSources` options. Unknown keys are ignored per spec.
pub(crate) fn parse_select_options(options: &HashMap<String, Value<'_>>) -> SelectOptions {
    let source_types = options
        .get("types")
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(SOURCE_TYPE_MONITOR);
    let cursor_mode = options
        .get("cursor_mode")
        .and_then(|value| u32::try_from(value).ok())
        .unwrap_or(1);
    SelectOptions {
        source_types,
        cursor_mode,
    }
}

/// Why a `SelectSources` option set cannot be served, as a D-Bus message.
fn validate_select(options: &SelectOptions) -> Result<(), String> {
    let supported_sources = SOURCE_TYPE_MONITOR | SOURCE_TYPE_WINDOW;
    if options.source_types == 0 || options.source_types & !supported_sources != 0 {
        return Err("only monitor and window sources are supported".to_string());
    }
    if options.cursor_mode != CURSOR_MODES {
        return Err(format!(
            "cursor_mode {} is not supported (Hidden only)",
            options.cursor_mode
        ));
    }
    Ok(())
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
        handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        app_id: &str,
        _options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<PortalResponse> {
        let request_path = handle.as_str().to_string();
        let session_path = session_handle.as_str().to_string();
        crate::request::register(&self.conn, &self.tracker, &request_path).await?;

        if self.tracker.lock().unwrap().was_closed(&request_path) {
            crate::request::finish(&self.conn, &self.tracker, &request_path).await;
            return Ok((1, HashMap::new()));
        }
        let insert_result = { self.sessions.lock().unwrap().insert(&session_path, app_id) };
        if let Err(error) = insert_result {
            log::warn!("portal: CreateSession refused: {error}");
            crate::request::finish(&self.conn, &self.tracker, &request_path).await;
            return Ok((2, HashMap::new()));
        }
        let inserted = self
            .conn
            .object_server()
            .at(
                session_path.as_str(),
                SessionIface {
                    path: session_path.clone(),
                    jobs: self.jobs.clone(),
                },
            )
            .await;
        let inserted = match inserted {
            Ok(inserted) => inserted,
            Err(error) => {
                let _ = self.sessions.lock().unwrap().remove(&session_path);
                crate::request::finish(&self.conn, &self.tracker, &request_path).await;
                return Err(zbus::fdo::Error::from(error));
            }
        };
        if !inserted {
            let _ = self.sessions.lock().unwrap().remove(&session_path);
            crate::request::finish(&self.conn, &self.tracker, &request_path).await;
            return Ok((2, HashMap::new()));
        }
        crate::request::finish(&self.conn, &self.tracker, &request_path).await;
        log::info!("portal: screencast session {session_path} created for '{app_id}'");
        Ok((0, HashMap::new()))
    }

    async fn select_sources(
        &self,
        handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        app_id: &str,
        options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<PortalResponse> {
        let session_path = session_handle.as_str().to_string();
        if !self.sessions.lock().unwrap().contains(&session_path) {
            return Err(zbus::fdo::Error::Failed(format!(
                "unknown session {session_path}"
            )));
        }
        let options = parse_select_options(&options);
        validate_select(&options).map_err(zbus::fdo::Error::InvalidArgs)?;

        let path = handle.as_str().to_string();
        log::debug!("portal: SelectSources for '{app_id}' on {session_path} at {path}");

        crate::request::register(&self.conn, &self.tracker, &path).await?;
        let (reply, response) = async_channel::bounded(1);
        let queued = self.enqueue(CastJob::SelectSources {
            request_path: path.clone(),
            session_path,
            app_id: app_id.to_string(),
            source_types: options.source_types,
            cursor_mode: options.cursor_mode,
            reply,
        });
        if let Err(error) = queued {
            crate::request::finish(&self.conn, &self.tracker, &path).await;
            return Err(error);
        }
        let result = response.recv().await.map_err(|_| {
            zbus::fdo::Error::Failed("screencast worker dropped its response".to_string())
        });
        crate::request::finish(&self.conn, &self.tracker, &path).await;
        result
    }

    async fn start(
        &self,
        handle: ObjectPath<'_>,
        session_handle: ObjectPath<'_>,
        app_id: &str,
        _parent_window: &str,
        _options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<PortalResponse> {
        let session_path = session_handle.as_str().to_string();
        if !self.sessions.lock().unwrap().contains(&session_path) {
            return Err(zbus::fdo::Error::Failed(format!(
                "unknown session {session_path}"
            )));
        }
        let path = handle.as_str().to_string();
        log::debug!("portal: Start for '{app_id}' on {session_path} at {path}");

        crate::request::register(&self.conn, &self.tracker, &path).await?;
        let (reply, response) = async_channel::bounded(1);
        let queued = self.enqueue(CastJob::Start {
            request_path: path.clone(),
            session_path,
            app_id: app_id.to_string(),
            reply,
        });
        if let Err(error) = queued {
            crate::request::finish(&self.conn, &self.tracker, &path).await;
            return Err(error);
        }
        let result = response.recv().await.map_err(|_| {
            zbus::fdo::Error::Failed("screencast worker dropped its response".to_string())
        });
        crate::request::finish(&self.conn, &self.tracker, &path).await;
        result
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

impl ScreenCastIface {
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
) {
    // The worker's own scoped client drives the interactive source picker.
    let mut picker = crate::ipc::PortalCapture::new(socket.clone());
    while let Ok(job) = rx.recv() {
        match job {
            CastJob::SelectSources {
                request_path,
                session_path,
                app_id,
                source_types,
                cursor_mode,
                reply,
            } => {
                let code = select_sources(
                    &tracker,
                    &sessions,
                    &mut picker,
                    &request_path,
                    &session_path,
                    &app_id,
                    source_types,
                    cursor_mode,
                );
                log::debug!("portal: SelectSources for '{app_id}' → response {code}");
                let _ = reply.send_blocking((code, HashMap::new()));
            }
            CastJob::Start {
                request_path,
                session_path,
                app_id,
                reply,
            } => {
                let response = start_cast(
                    &tracker,
                    &sessions,
                    &jobs,
                    &socket,
                    &mut picker,
                    &request_path,
                    &session_path,
                    &app_id,
                );
                log::debug!("portal: Start for '{app_id}' → response {}", response.0);
                let _ = reply.send_blocking(response);
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
#[allow(clippy::too_many_arguments)]
fn select_sources(
    tracker: &Arc<Mutex<RequestTracker>>,
    sessions: &Arc<Mutex<SessionRegistry>>,
    picker: &mut crate::ipc::PortalCapture,
    request_path: &str,
    session_path: &str,
    app_id: &str,
    source_types: u32,
    _cursor_mode: u32,
) -> u32 {
    if tracker.lock().unwrap().was_closed(request_path) {
        return 1;
    }
    let source = match pick_source(picker, source_types) {
        Ok(source) => source,
        Err(code) => return code,
    };
    if tracker.lock().unwrap().was_closed(request_path) {
        return 1;
    }
    match sessions
        .lock()
        .unwrap()
        .mark_sources_selected(session_path, app_id, source)
    {
        Ok(()) => 0,
        Err(error) => {
            log::warn!("portal: SelectSources refused: {error}");
            2
        }
    }
}

/// Decide the session's source through an explicit user action. The picker
/// returns a clicked window, Enter/empty-desktop for the whole output, or
/// Escape to cancel (ADR-0054). The selected source must be present in the
/// request mask.
fn pick_source(
    picker: &mut crate::ipc::PortalCapture,
    source_types: u32,
) -> Result<CastSource, u32> {
    match picker.pick(aegis_ipc::PickKind::Window) {
        Ok(aegis_ipc::PickResult::Window { id }) if source_types & SOURCE_TYPE_WINDOW != 0 => {
            Ok(CastSource::Window(id))
        }
        Ok(aegis_ipc::PickResult::Window { .. }) => {
            log::info!("portal: window picked for a monitor-only request; refusing");
            Err(2)
        }
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
    app_id: &str,
) -> (u32, HashMap<String, Value<'static>>) {
    if tracker.lock().unwrap().was_closed(request_path) {
        return (1, HashMap::new());
    }
    let (source, window_geometry) = {
        let sessions = sessions.lock().unwrap();
        let source = match sessions.source_for_start(session_path, app_id) {
            Ok(source) => source,
            Err(error) => {
                log::warn!("portal: Start refused: {error}");
                return (2, HashMap::new());
            }
        };
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
            let results = HashMap::from([(
                "streams".to_string(),
                streams_value(started.node_id, source_type, position, size),
            )]);
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
    let Some(_session) = sessions.lock().unwrap().remove(session_path) else {
        return;
    };
    log::debug!("portal: screencast session {session_path} closed");
    if let Err(error) = conn.emit_signal(None::<&str>, session_path, SESSION_IFACE, "Closed", &()) {
        log::warn!("portal: could not emit Closed for {session_path}: {error}");
    }
    if let Err(error) = conn.object_server().remove::<SessionIface, _>(session_path) {
        log::warn!("portal: could not remove {session_path}: {error}");
    }
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
    fn select_options_default_to_monitor_with_hidden_cursor() {
        let parsed = parse_select_options(&HashMap::new());
        assert_eq!(parsed.source_types, SOURCE_TYPE_MONITOR);
        assert_eq!(parsed.cursor_mode, 1);
        assert!(validate_select(&parsed).is_ok());
    }

    #[test]
    fn select_options_accept_monitor_and_window_mix() {
        // The picker allows the user to choose either advertised source.
        let parsed = parse_select_options(&options(&[("types", Value::from(0b11u32))]));
        assert!(validate_select(&parsed).is_ok());
    }

    #[test]
    fn select_options_accept_only_hidden_cursor_mode() {
        for unsupported in [0u32, 2, 4, 5] {
            let parsed =
                parse_select_options(&options(&[("cursor_mode", Value::from(unsupported))]));
            assert!(validate_select(&parsed).is_err());
        }
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
    fn screencast_version_is_3() {
        assert_eq!(SCREENCAST_VERSION, 3);
    }

    #[test]
    fn select_options_ignore_wrong_types() {
        let parsed = parse_select_options(&options(&[("types", Value::from("monitor"))]));
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
}
