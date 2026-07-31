//! `org.freedesktop.impl.portal.Account` v1: user identity sharing.
//!
//! `GetUserInformation` never answers silently: the request parks on a
//! worker while the compositor's yes/no confirmation dialog (`PickConfirm`
//! IPC, version 16) asks the user whether to share their name and avatar
//! with the calling application. Only an affirmative answer releases the
//! identity: the account name and GECOS real name from `getpwuid`, plus the
//! first existing avatar from the canonical candidates
//! (`$XDG_DATA_HOME/aegis/avatars/face.*`, then the freedesktop `~/.face`
//! conventions — the same precedence `aegis-avatar` resolves).
//!
//! Response codes follow the portal specification: 0 shared, 1 declined
//! (or `Request.Close` raced in), 2 other error.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use zbus::zvariant::{ObjectPath, Value};

use crate::files;
use crate::ipc::PortalCapture;
use crate::request::{PortalResponse, RequestTracker, ResponseSender};

/// The served interface version.
pub(crate) const ACCOUNT_VERSION: u32 = 1;

/// One account request handed from the bus method to the worker.
pub(crate) enum AccountJob {
    GetUserInformation {
        request_path: String,
        app_id: String,
        reason: Option<String>,
        reply: ResponseSender,
    },
}

/// The served account interface. The method only registers the request
/// object and enqueues; the consent prompt happens on the worker.
pub(crate) struct AccountIface {
    /// Async handle onto the same connection; only used inside served
    /// methods, which already run on zbus's executor (screenshot precedent).
    pub(crate) conn: zbus::Connection,
    pub(crate) tracker: Arc<Mutex<RequestTracker>>,
    pub(crate) jobs: mpsc::Sender<AccountJob>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Account")]
impl AccountIface {
    async fn get_user_information(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        _window: &str,
        options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<PortalResponse> {
        let path = handle.as_str().to_string();
        log::info!("portal: GetUserInformation for '{app_id}' at {path}");

        let reason = options
            .get("reason")
            .and_then(|value| String::try_from(value).ok());

        crate::request::register(&self.conn, &self.tracker, &path).await?;
        let (reply, response) = async_channel::bounded(1);
        let queued = self.jobs.send(AccountJob::GetUserInformation {
            request_path: path.clone(),
            app_id: app_id.to_string(),
            reason,
            reply,
        });
        if queued.is_err() {
            crate::request::finish(&self.conn, &self.tracker, &path).await;
            return Err(zbus::fdo::Error::Failed(
                "account worker is gone".to_string(),
            ));
        }
        let result = response.recv().await.map_err(|_| {
            zbus::fdo::Error::Failed("account worker dropped its response".to_string())
        });
        crate::request::finish(&self.conn, &self.tracker, &path).await;
        result
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        ACCOUNT_VERSION
    }
}

/// Worker loop: one consent prompt at a time, serialized like the other
/// choosers (each blocks on user interaction).
pub(crate) fn account_worker(
    rx: mpsc::Receiver<AccountJob>,
    tracker: Arc<Mutex<RequestTracker>>,
    mut capture: PortalCapture,
) {
    while let Ok(AccountJob::GetUserInformation {
        request_path,
        app_id,
        reason,
        reply,
    }) = rx.recv()
    {
        let result = run_request(&mut capture, &tracker, &request_path, &app_id, reason);
        let _ = reply.send_blocking(result);
    }
}

/// Execute one request: prompt for consent, then release the identity.
fn run_request(
    capture: &mut PortalCapture,
    tracker: &Arc<Mutex<RequestTracker>>,
    request_path: &str,
    app_id: &str,
    reason: Option<String>,
) -> (u32, HashMap<String, Value<'static>>) {
    if tracker.lock().unwrap().was_closed(request_path) {
        return (1, HashMap::new());
    }

    let mut body = format!(
        "The application '{app_id}' requests access to your personal information \
         (name and avatar photo)."
    );
    if let Some(reason) = reason {
        body.push(' ');
        body.push_str(&reason);
    }
    let confirmed = capture.pick_confirm(
        "Share Personal Information".to_string(),
        body,
        Some("Share".to_string()),
    );
    match confirmed {
        Ok(aegis_ipc::ConfirmPickResult::Confirmed) => {}
        Ok(aegis_ipc::ConfirmPickResult::Cancelled) => return (1, HashMap::new()),
        Err(error) => {
            log::warn!("portal: GetUserInformation consent for '{app_id}' failed: {error}");
            return (2, HashMap::new());
        }
    }
    if tracker.lock().unwrap().was_closed(request_path) {
        return (1, HashMap::new());
    }

    let identity = Identity::current();
    log::info!("portal: GetUserInformation for '{app_id}' shared (consent given)");
    let mut results = HashMap::from([
        ("id".to_string(), Value::from(identity.id)),
        ("name".to_string(), Value::from(identity.name)),
    ]);
    if let Some(image) = identity.avatar_uri {
        results.insert("image".to_string(), Value::from(image));
    }
    (0, results)
}

/// The local user's account id, real name, and avatar URI.
struct Identity {
    id: String,
    name: String,
    avatar_uri: Option<String>,
}

impl Identity {
    fn current() -> Identity {
        let (id, name) = passwd_identity();
        Identity {
            id,
            name,
            avatar_uri: avatar_path().map(|path| files::file_uri(&path)),
        }
    }
}

/// `(account id, real name)` from `getpwuid`: the GECOS full name up to
/// the first comma, falling back to the account name.
fn passwd_identity() -> (String, String) {
    // SAFETY: getpwuid's returned pointer is valid for the process lifetime
    // and only read here.
    let passwd = unsafe { libc::getpwuid(libc::getuid()) };
    if passwd.is_null() {
        let fallback = std::env::var("USER").unwrap_or_else(|_| "unknown".to_string());
        return (fallback.clone(), fallback);
    }
    let read = |field: *const std::ffi::c_char| {
        if field.is_null() {
            return String::new();
        }
        // SAFETY: passwd string fields are NUL-terminated.
        unsafe { std::ffi::CStr::from_ptr(field) }
            .to_string_lossy()
            .into_owned()
    };
    // SAFETY: the passwd struct outlives these reads.
    let (pw_name, pw_gecos) = unsafe { ((*passwd).pw_name, (*passwd).pw_gecos) };
    let id = read(pw_name);
    let gecos = read(pw_gecos);
    let name = gecos
        .split(',')
        .next()
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| id.clone());
    (id, name)
}

/// The first existing avatar candidate, in `aegis-avatar`'s precedence:
/// the canonical Aegis data location, then the freedesktop `~/.face`
/// conventions.
fn avatar_path() -> Option<std::path::PathBuf> {
    let mut candidates: Vec<std::path::PathBuf> = Vec::new();
    if let Some(data) = dirs::data_dir() {
        let dir = data.join("aegis").join("avatars");
        for name in ["face.png", "face.jpg", "face.webp", "face"] {
            candidates.push(dir.join(name));
        }
    }
    if let Some(home) = dirs::home_dir() {
        candidates.push(home.join(".face"));
        candidates.push(home.join(".face.icon"));
    }
    candidates.into_iter().find(|path| path.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_1() {
        assert_eq!(ACCOUNT_VERSION, 1);
    }

    #[test]
    fn identity_is_never_empty() {
        let (id, name) = passwd_identity();
        assert!(!id.is_empty());
        assert!(!name.is_empty());
    }
}
