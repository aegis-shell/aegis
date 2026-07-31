//! `org.freedesktop.impl.portal.Screenshot` v2.
//!
//! `Screenshot` exports an `org.freedesktop.impl.portal.Request` object at the
//! exact `handle` supplied by the portal frontend, then awaits a dedicated
//! capture worker without blocking zbus's executor. The worker pulls the
//! focused output's PNG over scoped IPC and returns the backend contract's
//! `(response, results)` tuple. With `interactive = true` it first runs the
//! compositor's region picker (`PickTarget`, ADR-0054).
//!
//! Version 2 adds `PickColor`: the compositor's crosshair picker returns the
//! clicked point's RGB, which the response reports as the spec's `color`
//! `(ddd)` triple (0–1 doubles).
//!
//! Response codes follow the portal specification: 0 success, 1 cancelled
//! (the client called `Request.Close` first, or the user dismissed the
//! picker), 2 other error.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use zbus::zvariant::{ObjectPath, Value};

use crate::files;
use crate::ipc::PortalCapture;
use crate::request::{PortalResponse, RequestTracker, ResponseSender};

/// The served interface version: 2 adds `PickColor`.
pub(crate) const SCREENSHOT_VERSION: u32 = 2;

/// One screenshot/color request handed from the bus methods to the capture
/// worker.
pub(crate) enum CaptureJob {
    Screenshot {
        request_path: String,
        token: String,
        app_id: String,
        interactive: bool,
        reply: ResponseSender,
    },
    PickColor {
        request_path: String,
        app_id: String,
        reply: ResponseSender,
    },
}

/// Options parsed out of the `a{sv}` argument.
pub(crate) struct ScreenshotOptions {
    pub(crate) interactive: bool,
}

/// Parse the backend `Screenshot` options dict. Request-token handling belongs
/// to the portal frontend; backend options contain only request policy.
pub(crate) fn parse_options(options: &HashMap<String, Value<'_>>) -> ScreenshotOptions {
    let interactive = options
        .get("interactive")
        .and_then(|value| bool::try_from(value).ok())
        .unwrap_or(false);
    ScreenshotOptions { interactive }
}

/// Object-path elements allow `[A-Za-z0-9_]`; the filename and the bus path
/// share one sanitized token so neither can be escaped through the other.
pub(crate) fn sanitize_token(token: &str) -> String {
    token
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

fn capture_token(handle: &ObjectPath<'_>) -> String {
    handle
        .as_str()
        .rsplit('/')
        .next()
        .filter(|tail| !tail.is_empty())
        .map(sanitize_token)
        .unwrap_or_else(|| "capture".to_string())
}

/// The served screenshot interface. Methods only register the request
/// object and enqueue; all blocking work happens on the capture worker.
pub(crate) struct ScreenshotIface {
    /// Async handle onto the same connection; only used inside served
    /// methods, which already run on zbus's executor (tray precedent).
    pub(crate) conn: zbus::Connection,
    pub(crate) tracker: Arc<Mutex<RequestTracker>>,
    pub(crate) jobs: mpsc::Sender<CaptureJob>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Screenshot")]
impl ScreenshotIface {
    async fn screenshot(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        _parent_window: &str,
        options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<PortalResponse> {
        let options = parse_options(&options);
        let path = handle.as_str().to_string();
        let token = capture_token(&handle);
        log::info!(
            "portal: Screenshot for '{app_id}' (interactive={}) at {path}",
            options.interactive
        );

        crate::request::register(&self.conn, &self.tracker, &path).await?;
        let (reply, response) = async_channel::bounded(1);
        let queued = self.jobs.send(CaptureJob::Screenshot {
            request_path: path.clone(),
            token,
            app_id: app_id.to_string(),
            interactive: options.interactive,
            reply,
        });
        if queued.is_err() {
            crate::request::finish(&self.conn, &self.tracker, &path).await;
            return Err(zbus::fdo::Error::Failed(
                "capture worker is gone".to_string(),
            ));
        }
        let result = response.recv().await.map_err(|_| {
            zbus::fdo::Error::Failed("capture worker dropped its response".to_string())
        });
        crate::request::finish(&self.conn, &self.tracker, &path).await;
        result
    }

    async fn pick_color(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        _parent_window: &str,
        _options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<PortalResponse> {
        let path = handle.as_str().to_string();
        log::info!("portal: PickColor for '{app_id}' at {path}");

        crate::request::register(&self.conn, &self.tracker, &path).await?;
        let (reply, response) = async_channel::bounded(1);
        let queued = self.jobs.send(CaptureJob::PickColor {
            request_path: path.clone(),
            app_id: app_id.to_string(),
            reply,
        });
        if queued.is_err() {
            crate::request::finish(&self.conn, &self.tracker, &path).await;
            return Err(zbus::fdo::Error::Failed(
                "capture worker is gone".to_string(),
            ));
        }
        let result = response.recv().await.map_err(|_| {
            zbus::fdo::Error::Failed("capture worker dropped its response".to_string())
        });
        crate::request::finish(&self.conn, &self.tracker, &path).await;
        result
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        SCREENSHOT_VERSION
    }
}

/// Worker loop: one job at a time. Captures are frame-triggered on the
/// compositor side, so serializing them is the natural pacing.
pub(crate) fn capture_worker(
    rx: mpsc::Receiver<CaptureJob>,
    tracker: Arc<Mutex<RequestTracker>>,
    mut capture: PortalCapture,
) {
    while let Ok(job) = rx.recv() {
        let result = run_job(&mut capture, &tracker, &job);
        let reply = match &job {
            CaptureJob::Screenshot { reply, .. } | CaptureJob::PickColor { reply, .. } => reply,
        };
        let _ = reply.send_blocking(result);
    }
}

/// Execute one job and produce the `(response_code, results)` pair.
fn run_job(
    capture: &mut PortalCapture,
    tracker: &Arc<Mutex<RequestTracker>>,
    job: &CaptureJob,
) -> (u32, HashMap<String, Value<'static>>) {
    match job {
        CaptureJob::Screenshot {
            request_path,
            token,
            app_id,
            interactive,
            ..
        } => {
            if tracker.lock().unwrap().was_closed(request_path) {
                return (1, HashMap::new());
            }
            // Interactive: the compositor's region picker decides what to
            // capture (ADR-0054). A dismissed picker answers 1 (cancelled),
            // exactly like a client Close.
            let region = if *interactive {
                match capture.pick(aegis_ipc::PickKind::Region) {
                    Ok(aegis_ipc::PickResult::Region { rect }) => Some(rect),
                    Ok(aegis_ipc::PickResult::Cancelled) => return (1, HashMap::new()),
                    Ok(other) => {
                        log::warn!("portal: region pick answered with {other:?}");
                        return (2, HashMap::new());
                    }
                    Err(error) => {
                        log::warn!("portal: region pick for '{app_id}' failed: {error}");
                        return (2, HashMap::new());
                    }
                }
            } else {
                None
            };
            if tracker.lock().unwrap().was_closed(request_path) {
                return (1, HashMap::new());
            }
            match capture_and_write(capture, token, region) {
                Ok(uri) => {
                    // A Close racing the capture wins over a completed result.
                    if tracker.lock().unwrap().was_closed(request_path) {
                        return (1, HashMap::new());
                    }
                    log::info!("portal: screenshot for '{app_id}' → {uri}");
                    (0, HashMap::from([("uri".to_string(), Value::from(uri))]))
                }
                Err(error) => {
                    log::warn!("portal: screenshot for '{app_id}' failed: {error}");
                    (2, HashMap::new())
                }
            }
        }
        CaptureJob::PickColor {
            request_path,
            app_id,
            ..
        } => {
            if tracker.lock().unwrap().was_closed(request_path) {
                return (1, HashMap::new());
            }
            match capture.pick(aegis_ipc::PickKind::Pixel) {
                Ok(aegis_ipc::PickResult::Pixel { rgb, .. }) => {
                    if tracker.lock().unwrap().was_closed(request_path) {
                        return (1, HashMap::new());
                    }
                    log::info!(
                        "portal: PickColor for '{app_id}' → #{:02x}{:02x}{:02x}",
                        rgb[0],
                        rgb[1],
                        rgb[2]
                    );
                    (0, HashMap::from([("color".to_string(), color_value(rgb))]))
                }
                Ok(aegis_ipc::PickResult::Cancelled) => (1, HashMap::new()),
                Ok(other) => {
                    log::warn!("portal: pixel pick answered with {other:?}");
                    (2, HashMap::new())
                }
                Err(error) => {
                    log::warn!("portal: PickColor for '{app_id}' failed: {error}");
                    (2, HashMap::new())
                }
            }
        }
    }
}

/// The PickColor `color` result: `(ddd)` red/green/blue as 0–1 doubles.
fn color_value(rgb: [u8; 3]) -> Value<'static> {
    Value::Structure(zbus::zvariant::Structure::from((
        f64::from(rgb[0]) / 255.0,
        f64::from(rgb[1]) / 255.0,
        f64::from(rgb[2]) / 255.0,
    )))
}

/// Capture the focused output (or one region of it) and persist the PNG
/// under the portal cache directory, returning the `file://` URI the portal
/// contract expects.
fn capture_and_write(
    capture: &mut PortalCapture,
    token: &str,
    region: Option<aegis_core::Rect>,
) -> std::io::Result<String> {
    let dir = files::cache_dir().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            "neither $XDG_CACHE_HOME nor $XDG_RUNTIME_DIR is set",
        )
    })?;
    let png = match region {
        Some(region) => capture.capture_region_png(region)?,
        None => capture.capture_png()?,
    };
    let path = files::write_capture(&dir, token, &png)?;
    Ok(files::file_uri(&path))
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
    fn options_default_to_non_interactive() {
        let parsed = parse_options(&HashMap::new());
        assert!(!parsed.interactive);
    }

    #[test]
    fn options_parse_interactive() {
        let parsed = parse_options(&options(&[("interactive", Value::from(true))]));
        assert!(parsed.interactive);
    }

    #[test]
    fn wrong_typed_options_are_ignored() {
        let parsed = parse_options(&options(&[("interactive", Value::from("yes"))]));
        assert!(!parsed.interactive);
    }

    #[test]
    fn token_sanitization_replaces_path_separators() {
        assert_eq!(sanitize_token("a/b-c.d"), "a_b_c_d");
    }

    #[test]
    fn screenshot_version_is_2() {
        assert_eq!(SCREENSHOT_VERSION, 2);
    }

    #[test]
    fn color_result_is_a_ddd_structure_of_unit_doubles() {
        let value = color_value([255, 128, 0]);
        assert_eq!(
            value.value_signature().to_string(),
            "(ddd)",
            "the spec's color result is a (ddd) structure"
        );
        let Value::Structure(structure) = &value else {
            panic!("color must be a structure");
        };
        let fields = structure.fields();
        assert_eq!(fields.len(), 3);
        let channels: Vec<f64> = fields
            .iter()
            .map(|field| f64::try_from(field).expect("double channel"))
            .collect();
        assert_eq!(channels[0], 1.0);
        assert!((channels[1] - 128.0 / 255.0).abs() < f64::EPSILON);
        assert_eq!(channels[2], 0.0);
    }
}
