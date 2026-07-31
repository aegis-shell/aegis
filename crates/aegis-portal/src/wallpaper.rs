//! `org.freedesktop.impl.portal.Wallpaper` v1: desktop wallpaper changes.
//!
//! `SetWallpaperURI` parks on a worker while the compositor's confirmation
//! dialog (`PickConfirm` IPC) asks whether the calling application may
//! change the wallpaper; an affirmative answer decodes and swaps the image
//! live on the compositor main loop (`SetWallpaper` IPC, version 17 — the
//! reply is the authoritative receipt, so decode failures surface as
//! response 2). Only `file://` URIs are accepted, per the spec.
//!
//! Accepted-but-ignored options: `show-preview` (no preview UI),
//! `background-color` (the image covers the whole output), and `set-on`
//! (Aegis has one wallpaper surface; `lockscreen`/`both` apply to it — the
//! lock screen reads the same compositor wallpaper).
//!
//! Response codes follow the portal specification: 0 applied, 1 declined
//! (or `Request.Close` raced in), 2 other error.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};

use zbus::zvariant::{ObjectPath, Value};

use crate::ipc::PortalCapture;
use crate::request::{PortalResponse, RequestTracker, ResponseSender};

/// The served interface version.
pub(crate) const WALLPAPER_VERSION: u32 = 1;

/// One wallpaper request handed from the bus method to the worker.
pub(crate) enum WallpaperJob {
    Set {
        request_path: String,
        app_id: String,
        path: PathBuf,
        reply: ResponseSender,
    },
}

/// The served wallpaper interface. The method only registers the request
/// object and enqueues; the consent prompt happens on the worker.
pub(crate) struct WallpaperIface {
    /// Async handle onto the same connection; only used inside served
    /// methods, which already run on zbus's executor (screenshot precedent).
    pub(crate) conn: zbus::Connection,
    pub(crate) tracker: Arc<Mutex<RequestTracker>>,
    pub(crate) jobs: mpsc::Sender<WallpaperJob>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Wallpaper")]
impl WallpaperIface {
    /// The spec spells the method `SetWallpaperURI`; zbus would
    /// auto-PascalCase `set_wallpaper_uri` to `SetWallpaperUri`.
    #[zbus(name = "SetWallpaperURI")]
    async fn set_wallpaper_uri(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        _parent_window: &str,
        uri: &str,
        _options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<PortalResponse> {
        let path = handle.as_str().to_string();
        log::info!("portal: SetWallpaperURI for '{app_id}' ({uri}) at {path}");

        let Some(path_buf) = uri_path(uri) else {
            return Err(zbus::fdo::Error::InvalidArgs(format!(
                "only file:// URIs are supported, got '{uri}'"
            )));
        };

        crate::request::register(&self.conn, &self.tracker, &path).await?;
        let (reply, response) = async_channel::bounded(1);
        let queued = self.jobs.send(WallpaperJob::Set {
            request_path: path.clone(),
            app_id: app_id.to_string(),
            path: path_buf,
            reply,
        });
        if queued.is_err() {
            crate::request::finish(&self.conn, &self.tracker, &path).await;
            return Err(zbus::fdo::Error::Failed(
                "wallpaper worker is gone".to_string(),
            ));
        }
        let result = response.recv().await.map_err(|_| {
            zbus::fdo::Error::Failed("wallpaper worker dropped its response".to_string())
        });
        crate::request::finish(&self.conn, &self.tracker, &path).await;
        result
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        WALLPAPER_VERSION
    }
}

/// Worker loop: one consent prompt at a time, serialized like the other
/// choosers (each blocks on user interaction).
pub(crate) fn wallpaper_worker(
    rx: mpsc::Receiver<WallpaperJob>,
    tracker: Arc<Mutex<RequestTracker>>,
    mut capture: PortalCapture,
) {
    while let Ok(WallpaperJob::Set {
        request_path,
        app_id,
        path,
        reply,
    }) = rx.recv()
    {
        let result = run_set(&mut capture, &tracker, &request_path, &app_id, path);
        let _ = reply.send_blocking(result);
    }
}

/// Execute one request: prompt for consent, then swap the wallpaper.
fn run_set(
    capture: &mut PortalCapture,
    tracker: &Arc<Mutex<RequestTracker>>,
    request_path: &str,
    app_id: &str,
    path: PathBuf,
) -> (u32, HashMap<String, Value<'static>>) {
    if tracker.lock().unwrap().was_closed(request_path) {
        return (1, HashMap::new());
    }
    let body = format!(
        "The application '{app_id}' wants to change the desktop wallpaper to {}.",
        path.display()
    );
    match capture.pick_confirm(
        "Change Wallpaper".to_string(),
        body,
        Some("Set Wallpaper".to_string()),
    ) {
        Ok(aegis_ipc::ConfirmPickResult::Confirmed) => {}
        Ok(aegis_ipc::ConfirmPickResult::Cancelled) => return (1, HashMap::new()),
        Err(error) => {
            log::warn!("portal: SetWallpaperURI consent for '{app_id}' failed: {error}");
            return (2, HashMap::new());
        }
    }
    if tracker.lock().unwrap().was_closed(request_path) {
        return (1, HashMap::new());
    }

    match capture.set_wallpaper(path.clone()) {
        Ok(()) => {
            log::info!(
                "portal: SetWallpaperURI for '{app_id}' applied {}",
                path.display()
            );
            (0, HashMap::new())
        }
        Err(error) => {
            log::warn!("portal: SetWallpaperURI for '{app_id}' failed: {error}");
            (2, HashMap::new())
        }
    }
}

/// Resolve a `file://` URI to a filesystem path: strips the scheme and an
/// optional `localhost` authority, percent-decodes the rest. Returns `None`
/// for any other scheme.
fn uri_path(uri: &str) -> Option<PathBuf> {
    let rest = uri
        .strip_prefix("file://localhost/")
        .map(|path| format!("/{path}"))
        .or_else(|| uri.strip_prefix("file://").map(str::to_string))?;
    let mut bytes = Vec::with_capacity(rest.len());
    let mut chars = rest.bytes();
    while let Some(byte) = chars.next() {
        if byte == b'%' {
            let hi = chars.next()?;
            let lo = chars.next()?;
            let hex = |b: u8| -> Option<u8> {
                match b {
                    b'0'..=b'9' => Some(b - b'0'),
                    b'a'..=b'f' => Some(b - b'a' + 10),
                    b'A'..=b'F' => Some(b - b'A' + 10),
                    _ => None,
                }
            };
            bytes.push(hex(hi)? * 16 + hex(lo)?);
        } else {
            bytes.push(byte);
        }
    }
    use std::os::unix::ffi::OsStringExt;
    Some(PathBuf::from(std::ffi::OsString::from_vec(bytes)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_1() {
        assert_eq!(WALLPAPER_VERSION, 1);
    }

    #[test]
    fn uri_path_decodes_and_strips_the_scheme() {
        assert_eq!(
            uri_path("file:///home/user/My%20Wallpapers/a%20b.png"),
            Some(PathBuf::from("/home/user/My Wallpapers/a b.png"))
        );
        assert_eq!(
            uri_path("file://localhost/tmp/x.png"),
            Some(PathBuf::from("/tmp/x.png"))
        );
        assert_eq!(uri_path("https://example.com/x.png"), None);
        assert_eq!(uri_path("file:///bad/%zz.png"), None);
    }
}
