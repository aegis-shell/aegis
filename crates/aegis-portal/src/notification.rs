//! `org.freedesktop.impl.portal.Notification` v2: session notifications
//! posted by (sandboxed) applications.
//!
//! `AddNotification` forwards the title/body to the compositor's
//! notification queue over scoped IPC (`Command::Notify`, carrying the
//! application's own notification id as `external_id`); the queue's chrome
//! surfaces (toast strip, command panel, HUD count) render it like any
//! compositor notification. `RemoveNotification` resolves the `(app_id,
//! id)` pair against the live queue snapshot and dismisses the matching
//! entry; removing an unknown id is a no-op, as the spec expects.
//!
//! Only the baseline `title`/`body` notification keys are honored; icons,
//! priorities, buttons, and actions have no queue support yet (see
//! `SupportedOptions`, which advertises nothing beyond the baseline). No
//! `Request` object is involved: the spec's methods carry no handle and
//! return nothing.
//!
//! Calls are short (one fire-and-forget command, or one snapshot query plus
//! one dismiss), so they run inline on zbus's executor behind a mutex —
//! no worker thread.

use std::collections::HashMap;
use std::sync::Mutex;

use zbus::zvariant::Value;

use crate::ipc::PortalCapture;

/// The served interface version: 2 deprecated the icon `bytes` option.
pub(crate) const NOTIFICATION_VERSION: u32 = 2;

/// The served notification interface.
pub(crate) struct NotificationIface {
    /// Lazily connected scoped IPC client; calls are milliseconds, so they
    /// are taken inline under this lock (screenshot-worker discipline is
    /// unnecessary here).
    pub(crate) capture: Mutex<PortalCapture>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Notification")]
impl NotificationIface {
    async fn add_notification(
        &self,
        app_id: &str,
        id: &str,
        notification: HashMap<String, Value<'_>>,
    ) -> zbus::fdo::Result<()> {
        let get_string = |key: &str| {
            notification
                .get(key)
                .and_then(|value| String::try_from(value).ok())
        };
        let summary = get_string("title").unwrap_or_default();
        let body = get_string("body").unwrap_or_default();
        log::info!("portal: AddNotification for '{app_id}' (id '{id}'): {summary}");

        self.capture
            .lock()
            .unwrap()
            .notify_external(
                summary,
                body,
                Some(app_id.to_string()),
                Some(id.to_string()),
            )
            .map_err(|error| {
                log::warn!("portal: AddNotification for '{app_id}' failed: {error}");
                zbus::fdo::Error::Failed("notification IPC failed".to_string())
            })
    }

    async fn remove_notification(&self, app_id: &str, id: &str) -> zbus::fdo::Result<()> {
        log::info!("portal: RemoveNotification for '{app_id}' (id '{id}')");

        let mut capture = self.capture.lock().unwrap();
        let notifications = capture.notifications().map_err(|error| {
            log::warn!("portal: RemoveNotification snapshot for '{app_id}' failed: {error}");
            zbus::fdo::Error::Failed("notification IPC failed".to_string())
        })?;
        let match_id = notifications
            .iter()
            .find(|n| n.app_id.as_deref() == Some(app_id) && n.external_id.as_deref() == Some(id));
        if let Some(notification) = match_id {
            capture
                .dismiss_notification(notification.id)
                .map_err(|error| {
                    log::warn!("portal: RemoveNotification dismiss for '{app_id}' failed: {error}");
                    zbus::fdo::Error::Failed("notification IPC failed".to_string())
                })?;
        }
        // An unknown (or already expired) id is not an error.
        Ok(())
    }

    /// The notification keys honored beyond the baseline `title`/`body`:
    /// none yet (no icons, priorities, buttons, or actions in the queue).
    #[zbus(property, name = "SupportedOptions")]
    fn supported_options(&self) -> HashMap<String, Value<'static>> {
        HashMap::new()
    }

    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        NOTIFICATION_VERSION
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_is_2() {
        assert_eq!(NOTIFICATION_VERSION, 2);
    }
}
