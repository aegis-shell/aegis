//! `org.freedesktop.impl.portal.DynamicLauncher` v1: launcher installation
//! consent.
//!
//! The frontend writes the actual `.desktop` files; the backend's job is
//! the user-facing consent. `PrepareInstall` parks on a worker while the
//! compositor's confirmation dialog (`PickConfirm` IPC, version 16) asks
//! whether the calling application may install a launcher; an affirmative
//! answer echoes the proposed name and icon and returns a fresh install
//! token. There is no name/icon editing UI yet: the proposal returns
//! unchanged (`editable` is accepted and ignored).
//!
//! `RequestInstallToken` always answers nonzero: non-interactive
//! installation is never allowed here, so every install goes through the
//! consent dialog above.
//!
//! Response codes follow the portal specification: 0 success, 1 declined
//! (or `Request.Close` raced in), 2 other error.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use rand::RngCore;
use rand::rngs::OsRng;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

use crate::ipc::PortalCapture;
use crate::request::{PortalResponse, RequestTracker, ResponseSender};

/// The served interface version.
pub(crate) const DYNAMIC_LAUNCHER_VERSION: u32 = 1;
/// The `SupportedLauncherTypes` bitmask: 1 = Application (the only type
/// with an Aegis launch surface).
const LAUNCHER_TYPE_APPLICATION: u32 = 1;

/// One prepare-install request handed from the bus method to the worker.
pub(crate) enum DynamicLauncherJob {
    PrepareInstall {
        request_path: String,
        app_id: String,
        name: String,
        icon: OwnedValue,
        reply: ResponseSender,
    },
}

/// The served dynamic-launcher interface. Methods only register the
/// request object and enqueue; the consent prompt happens on the worker.
pub(crate) struct DynamicLauncherIface {
    /// Async handle onto the same connection; only used inside served
    /// methods, which already run on zbus's executor (screenshot precedent).
    pub(crate) conn: zbus::Connection,
    pub(crate) tracker: Arc<Mutex<RequestTracker>>,
    pub(crate) jobs: mpsc::Sender<DynamicLauncherJob>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.DynamicLauncher")]
impl DynamicLauncherIface {
    async fn prepare_install(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        _parent_window: &str,
        name: &str,
        icon_v: Value<'_>,
        _options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<PortalResponse> {
        let path = handle.as_str().to_string();
        log::info!("portal: PrepareInstall for '{app_id}' ('{name}') at {path}");

        crate::request::register(&self.conn, &self.tracker, &path).await?;
        let (reply, response) = async_channel::bounded(1);
        let queued = self.jobs.send(DynamicLauncherJob::PrepareInstall {
            request_path: path.clone(),
            app_id: app_id.to_string(),
            name: name.to_string(),
            icon: icon_v
                .try_to_owned()
                .map_err(|e| zbus::fdo::Error::InvalidArgs(e.to_string()))?,
            reply,
        });
        if queued.is_err() {
            crate::request::finish(&self.conn, &self.tracker, &path).await;
            return Err(zbus::fdo::Error::Failed(
                "dynamic launcher worker is gone".to_string(),
            ));
        }
        let result = response.recv().await.map_err(|_| {
            zbus::fdo::Error::Failed("dynamic launcher worker dropped its response".to_string())
        });
        crate::request::finish(&self.conn, &self.tracker, &path).await;
        result
    }

    /// Non-interactive installation is never allowed: every install goes
    /// through `PrepareInstall`'s consent dialog.
    async fn request_install_token(
        &self,
        app_id: &str,
        _options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<u32> {
        log::info!("portal: RequestInstallToken for '{app_id}' refused (consent is mandatory)");
        Ok(1)
    }

    #[zbus(property, name = "SupportedLauncherTypes")]
    fn supported_launcher_types(&self) -> u32 {
        LAUNCHER_TYPE_APPLICATION
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        DYNAMIC_LAUNCHER_VERSION
    }
}

/// Worker loop: one consent prompt at a time, serialized like the other
/// choosers (each blocks on user interaction).
pub(crate) fn dynamic_launcher_worker(
    rx: mpsc::Receiver<DynamicLauncherJob>,
    tracker: Arc<Mutex<RequestTracker>>,
    mut capture: PortalCapture,
) {
    while let Ok(DynamicLauncherJob::PrepareInstall {
        request_path,
        app_id,
        name,
        icon,
        reply,
    }) = rx.recv()
    {
        let result = run_prepare(&mut capture, &tracker, &request_path, &app_id, name, icon);
        let _ = reply.send_blocking(result);
    }
}

/// Execute one request: prompt for consent, then echo the proposal plus a
/// fresh token.
fn run_prepare(
    capture: &mut PortalCapture,
    tracker: &Arc<Mutex<RequestTracker>>,
    request_path: &str,
    app_id: &str,
    name: String,
    icon: OwnedValue,
) -> (u32, HashMap<String, Value<'static>>) {
    if tracker.lock().unwrap().was_closed(request_path) {
        return (1, HashMap::new());
    }
    let body = format!(
        "The application '{app_id}' wants to install a launcher named '{name}' \
         on this desktop."
    );
    let confirmed = capture.pick_confirm(
        "Install Launcher".to_string(),
        body,
        Some("Install".to_string()),
    );
    match confirmed {
        Ok(aegis_ipc::ConfirmPickResult::Confirmed) => {}
        Ok(aegis_ipc::ConfirmPickResult::Cancelled) => return (1, HashMap::new()),
        Err(error) => {
            log::warn!("portal: PrepareInstall consent for '{app_id}' failed: {error}");
            return (2, HashMap::new());
        }
    }
    if tracker.lock().unwrap().was_closed(request_path) {
        return (1, HashMap::new());
    }

    let mut token_bytes = [0u8; 16];
    OsRng.fill_bytes(&mut token_bytes);
    let token: String = token_bytes.iter().map(|b| format!("{b:02x}")).collect();
    log::info!("portal: PrepareInstall for '{app_id}' consented ('{name}')");
    (
        0,
        HashMap::from([
            ("name".to_string(), Value::from(name)),
            ("icon".to_string(), icon.into()),
            ("token".to_string(), Value::from(token)),
        ]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_and_launcher_types() {
        assert_eq!(DYNAMIC_LAUNCHER_VERSION, 1);
        assert_eq!(LAUNCHER_TYPE_APPLICATION, 1);
    }
}
