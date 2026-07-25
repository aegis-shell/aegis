//! `org.freedesktop.impl.portal.Request` objects and their cancellation
//! bookkeeping.
//!
//! One object per in-flight call, registered at the `handle_token`-derived
//! path and removed after the `Response` signal. `Close` records the path in
//! the shared tracker; the capture worker checks the tracker both before it
//! starts a job and after a capture completes, so a racing cancel answers
//! with response code 1 instead of a completed result.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

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
