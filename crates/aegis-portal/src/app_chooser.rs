//! `org.freedesktop.impl.portal.AppChooser` v2: pick an application to open
//! a piece of content with.
//!
//! `ChooseApplication` exports a `org.freedesktop.impl.portal.Request`
//! object at the caller's `handle`, then hands the candidate list to a
//! dedicated worker which runs the compositor's app-picker chrome over
//! scoped IPC (`PickApp`, IPC version 14). The picker is ordinary modal
//! chrome over the live scene; no screen content is captured.
//!
//! The `subject` line under the dialog title is the most specific of
//! `filename`, `uri`, `content_type`. `modal` and `activation_token` are
//! accepted and ignored (the chrome is always a session-modal overlay, and
//! aegis does not issue activation tokens). `UpdateChoices` is accepted and
//! logged but not forwarded yet: the picker does not support replacing its
//! candidate list mid-pick (rarely used by frontends).
//!
//! Response codes follow the portal specification: 0 success, 1 cancelled
//! (the client called `Request.Close` first, or the user dismissed the
//! picker), 2 other error.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};

use zbus::zvariant::{ObjectPath, Value};

use crate::ipc::PortalCapture;
use crate::request::{PortalResponse, RequestTracker, ResponseSender};

/// The served interface version: 2 added `activation_token` (accepted and
/// ignored).
pub(crate) const APP_CHOOSER_VERSION: u32 = 2;

/// One app-chooser request handed from the bus methods to the worker.
pub(crate) enum AppChooserJob {
    Choose {
        request_path: String,
        app_id: String,
        choices: Vec<String>,
        subject: Option<String>,
        last_choice: Option<String>,
        reply: ResponseSender,
    },
}

/// The served app-chooser interface. Methods only register the request
/// object and enqueue; the user-facing pick happens on the worker.
pub(crate) struct AppChooserIface {
    /// Async handle onto the same connection; only used inside served
    /// methods, which already run on zbus's executor (screenshot precedent).
    pub(crate) conn: zbus::Connection,
    pub(crate) tracker: Arc<Mutex<RequestTracker>>,
    pub(crate) jobs: mpsc::Sender<AppChooserJob>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.AppChooser")]
impl AppChooserIface {
    async fn choose_application(
        &self,
        handle: ObjectPath<'_>,
        app_id: &str,
        _parent_window: &str,
        choices: Vec<String>,
        options: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<PortalResponse> {
        let path = handle.as_str().to_string();
        log::info!(
            "portal: ChooseApplication for '{app_id}' ({} candidate(s)) at {path}",
            choices.len()
        );
        if choices.is_empty() {
            return Err(zbus::fdo::Error::InvalidArgs(
                "choices must not be empty".to_string(),
            ));
        }

        let get_string = |key: &str| {
            options
                .get(key)
                .and_then(|value| String::try_from(value).ok())
        };
        // The most specific context wins: a filename beats a URI beats a
        // bare content type.
        let subject = get_string("filename")
            .or_else(|| get_string("uri"))
            .or_else(|| get_string("content_type"));
        let last_choice = get_string("last_choice");

        crate::request::register(&self.conn, &self.tracker, &path).await?;
        let (reply, response) = async_channel::bounded(1);
        let queued = self.jobs.send(AppChooserJob::Choose {
            request_path: path.clone(),
            app_id: app_id.to_string(),
            choices,
            subject,
            last_choice,
            reply,
        });
        if queued.is_err() {
            crate::request::finish(&self.conn, &self.tracker, &path).await;
            return Err(zbus::fdo::Error::Failed(
                "app chooser worker is gone".to_string(),
            ));
        }
        let result = response.recv().await.map_err(|_| {
            zbus::fdo::Error::Failed("app chooser worker dropped its response".to_string())
        });
        crate::request::finish(&self.conn, &self.tracker, &path).await;
        result
    }

    /// The picker does not support replacing its candidate list mid-pick
    /// yet; the call is acknowledged so the frontend proceeds normally.
    async fn update_choices(&self, handle: ObjectPath<'_>, choices: Vec<String>) {
        log::info!(
            "portal: UpdateChoices ({} candidate(s)) for {handle} ignored: \
             mid-pick list updates are unsupported",
            choices.len()
        );
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        APP_CHOOSER_VERSION
    }
}

/// Worker loop: one pick at a time, serialized like the other choosers.
/// Each pick blocks on user interaction, so a dedicated thread keeps it off
/// both zbus's executor and the other workers.
pub(crate) fn app_chooser_worker(
    rx: mpsc::Receiver<AppChooserJob>,
    tracker: Arc<Mutex<RequestTracker>>,
    mut capture: PortalCapture,
) {
    while let Ok(AppChooserJob::Choose {
        request_path,
        app_id,
        choices,
        subject,
        last_choice,
        reply,
    }) = rx.recv()
    {
        let result = run_pick(
            &mut capture,
            &tracker,
            &request_path,
            &app_id,
            choices,
            subject,
            last_choice,
        );
        let _ = reply.send_blocking(result);
    }
}

/// Execute one pick and produce the `(response_code, results)` pair.
fn run_pick(
    capture: &mut PortalCapture,
    tracker: &Arc<Mutex<RequestTracker>>,
    request_path: &str,
    app_id: &str,
    choices: Vec<String>,
    subject: Option<String>,
    last_choice: Option<String>,
) -> (u32, HashMap<String, Value<'static>>) {
    if tracker.lock().unwrap().was_closed(request_path) {
        return (1, HashMap::new());
    }
    match capture.pick_app(choices, subject, last_choice) {
        Ok(aegis_ipc::AppPickResult::App { id }) => {
            // A Close racing the pick wins over a completed result.
            if tracker.lock().unwrap().was_closed(request_path) {
                return (1, HashMap::new());
            }
            log::info!("portal: ChooseApplication for '{app_id}' → {id}");
            (0, HashMap::from([("choice".to_string(), Value::from(id))]))
        }
        Ok(aegis_ipc::AppPickResult::Cancelled) => (1, HashMap::new()),
        Err(error) => {
            log::warn!("portal: ChooseApplication for '{app_id}' failed: {error}");
            (2, HashMap::new())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_2() {
        assert_eq!(APP_CHOOSER_VERSION, 2);
    }
}
