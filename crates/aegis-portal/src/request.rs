//! `org.freedesktop.impl.portal.Request` objects and their cancellation
//! bookkeeping.
//!
//! The portal frontend passes the exact object path in each backend method's
//! `handle` argument. The backend exports one object at that path for the
//! duration of the method call and removes it before returning the
//! `(response, results)` reply. `Close` records the path in the shared
//! tracker; workers check it before and after interactive work so a racing
//! cancellation answers with response code 1.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use zbus::zvariant::Value;

pub(crate) type PortalResults = std::collections::HashMap<String, Value<'static>>;
pub(crate) type PortalResponse = (u32, PortalResults);
pub(crate) type ResponseSender = async_channel::Sender<PortalResponse>;

/// Cancellation state shared between every served `Request` object and the
/// capture worker.
#[derive(Default)]
pub(crate) struct RequestTracker {
    closed: HashSet<String>,
}

impl RequestTracker {
    /// Whether `Close` arrived for this request path.
    pub(crate) fn was_closed(&self, path: &str) -> bool {
        self.closed.contains(path)
    }

    /// Drop all state for a finished request.
    pub(crate) fn forget(&mut self, path: &str) {
        self.closed.remove(path);
    }
}

/// The served request object. The portal spec gives it only `Close`.
pub(crate) struct RequestIface {
    pub(crate) path: String,
    pub(crate) tracker: Arc<Mutex<RequestTracker>>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Request")]
impl RequestIface {
    async fn close(&self) -> zbus::fdo::Result<()> {
        log::info!("portal: request {} closed by client", self.path);
        self.tracker
            .lock()
            .unwrap()
            .closed
            .insert(self.path.clone());
        Ok(())
    }
}

/// Export the backend request object at the exact path supplied by the portal
/// frontend. A duplicate handle is a protocol error rather than an
/// opportunity to share cancellation state between calls.
pub(crate) async fn register(
    conn: &zbus::Connection,
    tracker: &Arc<Mutex<RequestTracker>>,
    path: &str,
) -> zbus::fdo::Result<()> {
    let inserted = conn
        .object_server()
        .at(
            path,
            RequestIface {
                path: path.to_string(),
                tracker: Arc::clone(tracker),
            },
        )
        .await
        .map_err(zbus::fdo::Error::from)?;
    if !inserted {
        return Err(zbus::fdo::Error::Failed(format!(
            "request handle {path} is already active"
        )));
    }
    Ok(())
}

/// Remove a finished request object and its cancellation marker.
pub(crate) async fn finish(
    conn: &zbus::Connection,
    tracker: &Arc<Mutex<RequestTracker>>,
    path: &str,
) {
    if let Err(error) = conn.object_server().remove::<RequestIface, _>(path).await {
        log::warn!("portal: could not remove request {path}: {error}");
    }
    tracker.lock().unwrap().forget(path);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracker_records_and_forgets_closes() {
        let mut tracker = RequestTracker::default();
        assert!(!tracker.was_closed("/r/1"));
        tracker.closed.insert("/r/1".to_string());
        assert!(tracker.was_closed("/r/1"));
        tracker.forget("/r/1");
        assert!(!tracker.was_closed("/r/1"));
    }
}
