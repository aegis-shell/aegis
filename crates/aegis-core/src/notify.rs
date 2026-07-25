//! Notifications (M9 polish, delivered early over the IPC).
//!
//! A pure queue of dismissible, time-expiring notifications. The IPC `Notify`
//! command and external sources push here; a chrome toast component renders
//! the live entries; the IPC queries them. No flux, lens, or Wayland
//! dependency, so the queue and its expiry are unit-tested in isolation.

/// One notification. `id` is stable for the notification's life; `at_ms` is
/// the compositor-relative millisecond timestamp used for expiry ordering.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: u64,
    /// Short title (the freedesktop.org "summary").
    pub summary: String,
    /// Longer body text, possibly multi-line.
    pub body: String,
    /// Originating application id, if known.
    pub app_id: Option<String>,
    /// Compositor-relative timestamp (ms) the notification was posted.
    pub at_ms: u64,
}

/// A time-expiring queue of notifications. Entries older than `ttl_ms` are
/// dropped by [`Self::expire`]; the chrome calls that each frame before
/// reading [`Self::recent`].
#[derive(Debug)]
pub struct NotificationQueue {
    entries: Vec<Notification>,
    next_id: u64,
    ttl_ms: u64,
    do_not_disturb: bool,
}

impl NotificationQueue {
    /// An empty queue that expires entries `ttl_ms` after they were posted.
    pub fn new(ttl_ms: u64) -> NotificationQueue {
        NotificationQueue {
            entries: Vec::new(),
            next_id: 0,
            ttl_ms,
            do_not_disturb: false,
        }
    }

    /// Post a notification timestamped `now_ms`. Returns the posted
    /// notification (with its assigned id) so the caller can forward it as
    /// an event.
    pub fn push(
        &mut self,
        summary: impl Into<String>,
        body: impl Into<String>,
        app_id: Option<String>,
        now_ms: u64,
    ) -> Notification {
        let n = Notification {
            id: self.next_id,
            summary: summary.into(),
            body: body.into(),
            app_id,
            at_ms: now_ms,
        };
        self.next_id += 1;
        self.entries.push(n.clone());
        n
    }

    /// Drop entries older than `ttl_ms` relative to `now_ms`.
    pub fn expire(&mut self, now_ms: u64) {
        self.entries
            .retain(|n| now_ms.saturating_sub(n.at_ms) <= self.ttl_ms);
    }

    /// Dismiss a notification by id. Returns `true` if it was present and
    /// removed. Mirrors a user "dismiss" action before the TTL elapses.
    pub fn dismiss(&mut self, id: u64) -> bool {
        let before = self.entries.len();
        self.entries.retain(|n| n.id != id);
        self.entries.len() != before
    }

    /// The live entries, oldest first. Call [`Self::expire`] first to age
    /// out expired ones.
    pub fn recent(&self) -> &[Notification] {
        &self.entries
    }

    /// A cloned snapshot of the live entries (for the IPC).
    pub fn snapshot(&self) -> Vec<Notification> {
        self.entries.clone()
    }

    /// Number of live entries.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the queue is empty.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Suppress transient toast presentation while keeping notifications in
    /// the queue for the control center and IPC history.
    pub fn set_do_not_disturb(&mut self, enabled: bool) {
        self.do_not_disturb = enabled;
    }

    /// Whether transient notification presentation is currently suppressed.
    pub fn do_not_disturb(&self) -> bool {
        self.do_not_disturb
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_assigns_increasing_ids_and_timestamps() {
        let mut q = NotificationQueue::new(1000);
        let a = q.push("First", "body a", None, 10);
        let b = q.push("Second", "body b", Some("app".into()), 20);
        assert_eq!(a.id, 0);
        assert_eq!(b.id, 1);
        assert_eq!(a.at_ms, 10);
        assert_eq!(b.app_id.as_deref(), Some("app"));
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn expire_drops_only_old_entries() {
        let mut q = NotificationQueue::new(1000);
        q.push("old", "", None, 0);
        q.push("new", "", None, 1500);
        q.expire(2000); // old is 2000ms old (>1000) → dropped; new is 500ms old → kept
        let live: Vec<&str> = q.recent().iter().map(|n| n.summary.as_str()).collect();
        assert_eq!(live, vec!["new"]);
    }

    #[test]
    fn expire_keeps_entries_within_ttl() {
        let mut q = NotificationQueue::new(500);
        q.push("a", "", None, 100);
        q.push("b", "", None, 300);
        q.expire(400); // a is 300ms old, b is 100ms old — both within 500
        assert_eq!(q.len(), 2);
        q.expire(700); // a is 600ms old → dropped; b is 400ms old → kept
        assert_eq!(q.len(), 1);
    }

    #[test]
    fn snapshot_is_independent_of_the_queue() {
        let mut q = NotificationQueue::new(1000);
        q.push("a", "", None, 0);
        let snap = q.snapshot();
        q.push("b", "", None, 1);
        assert_eq!(snap.len(), 1, "snapshot does not see later pushes");
        assert_eq!(q.len(), 2);
    }

    #[test]
    fn dismiss_removes_by_id_and_reports_presence() {
        let mut q = NotificationQueue::new(1000);
        let a = q.push("a", "", None, 0);
        let b = q.push("b", "", None, 1);
        assert_eq!(q.len(), 2);
        assert!(q.dismiss(a.id), "existing id is dismissed");
        assert!(!q.dismiss(999), "unknown id reports false");
        assert_eq!(q.len(), 1);
        assert_eq!(q.recent()[0].id, b.id);
    }

    #[test]
    fn do_not_disturb_suppresses_presentation_without_dropping_history() {
        let mut queue = NotificationQueue::new(1000);
        queue.push("kept", "", None, 0);
        queue.set_do_not_disturb(true);
        assert!(queue.do_not_disturb());
        assert_eq!(queue.len(), 1);
        queue.set_do_not_disturb(false);
        assert!(!queue.do_not_disturb());
        assert_eq!(queue.len(), 1);
    }
}
