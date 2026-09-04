use crate::*;

/// How long a registered launch placement stays consumable. The launcher
/// registers before spawning, so a slow first map must still match; but a
/// stale entry must not intercept the user's later manual launch of the same
/// app forever.
pub(crate) const LAUNCH_PLACEMENT_TTL: std::time::Duration = std::time::Duration::from_secs(60);

/// One pending first-map workspace placement (ADR-0118), registered through
/// [`Server::register_launch_placement`]. Each
/// entry places exactly one root toplevel: the first map whose client pid or
/// app_id matches consumes it (FIFO), so a user's later manual launch of the
/// same app can only steal a stale entry within the TTL.
pub(crate) struct PendingLaunchPlacement {
    /// Match keys: the desktop entry's `startup_wm_class` and the desktop-id
    /// stem, matched case-sensitively against the mapped app_id.
    pub(crate) app_ids: Vec<String>,
    /// Spawned child pid, when the launcher reported one.
    pub(crate) pid: Option<u32>,
    pub(crate) placement: tessera_model::workspace::LaunchPlacement,
    pub(crate) registered_at: std::time::Instant,
}

/// Consume the pending placement for a mapping toplevel. An exact pid match
/// wins over app_id matching (a re-exec'd or portal-spawned app may not
/// match the spawn pid; the app_id FIFO is the fallback). Expired entries
/// are purged first.
pub(crate) fn take_pending_launch_placement(
    pending: &mut Vec<PendingLaunchPlacement>,
    app_id: Option<&str>,
    pid: Option<u32>,
    now: std::time::Instant,
) -> Option<tessera_model::workspace::LaunchPlacement> {
    // `checked_duration_since`: a `registered_at` in the future (clock
    // weirdness) must not panic; treat the entry as brand new.
    pending.retain(|entry| {
        now.checked_duration_since(entry.registered_at)
            .is_none_or(|age| age < LAUNCH_PLACEMENT_TTL)
    });
    if let Some(pid) = pid
        && let Some(pos) = pending.iter().position(|entry| entry.pid == Some(pid))
    {
        return Some(pending.remove(pos).placement);
    }
    if let Some(app_id) = app_id.filter(|app_id| !app_id.is_empty())
        && let Some(pos) = pending
            .iter()
            .position(|entry| entry.app_ids.iter().any(|key| key == app_id))
    {
        return Some(pending.remove(pos).placement);
    }
    None
}

impl Server {
    /// Register a pending first-map workspace placement for an app launch
    /// (`Command::LaunchApp`; ADR-0118). The first
    /// root toplevel to map with a matching pid or app_id consumes it;
    /// anything still pending after [`LAUNCH_PLACEMENT_TTL`] is ignored.
    pub fn register_launch_placement(
        &mut self,
        app_ids: Vec<String>,
        pid: Option<u32>,
        placement: tessera_model::workspace::LaunchPlacement,
    ) {
        // An entry with no match key can never be consumed deterministically.
        debug_assert!(
            !app_ids.is_empty() || pid.is_some(),
            "launch placement registered without pid or app_ids"
        );
        if app_ids.is_empty() && pid.is_none() {
            return;
        }
        self.state
            .pending_launch_placements
            .push(PendingLaunchPlacement {
                app_ids,
                pid,
                placement,
                registered_at: std::time::Instant::now(),
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(
        app_ids: &[&str],
        pid: Option<u32>,
        label: &str,
        registered_at: std::time::Instant,
    ) -> PendingLaunchPlacement {
        PendingLaunchPlacement {
            app_ids: app_ids.iter().map(|&id| id.to_owned()).collect(),
            pid,
            placement: tessera_model::workspace::LaunchPlacement::FreshWorkspace {
                label: Some(label.to_owned()),
            },
            registered_at,
        }
    }

    fn label_of(placement: Option<tessera_model::workspace::LaunchPlacement>) -> Option<String> {
        match placement {
            Some(tessera_model::workspace::LaunchPlacement::FreshWorkspace { label }) => label,
            other => panic!("expected FreshWorkspace, got {other:?}"),
        }
    }

    #[test]
    fn pid_match_wins_over_earlier_app_id_match() {
        let now = std::time::Instant::now();
        let mut pending = vec![
            entry(&["org.example.App"], None, "app-id", now),
            entry(&[], Some(4242), "pid", now),
        ];
        let taken =
            take_pending_launch_placement(&mut pending, Some("org.example.App"), Some(4242), now);
        assert_eq!(label_of(taken).as_deref(), Some("pid"));
        // The app_id entry stays queued for a later map.
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].app_ids, vec!["org.example.App".to_owned()]);
    }

    #[test]
    fn app_id_matches_in_fifo_order() {
        let now = std::time::Instant::now();
        let mut pending = vec![
            entry(&["org.example.App"], None, "first", now),
            entry(&["org.example.App"], None, "second", now),
        ];
        let first = take_pending_launch_placement(&mut pending, Some("org.example.App"), None, now);
        assert_eq!(label_of(first).as_deref(), Some("first"));
        let second =
            take_pending_launch_placement(&mut pending, Some("org.example.App"), None, now);
        assert_eq!(label_of(second).as_deref(), Some("second"));
        assert!(pending.is_empty());
    }

    #[test]
    fn app_id_match_is_case_sensitive() {
        let now = std::time::Instant::now();
        let mut pending = vec![entry(&["Org.Example.App"], None, "entry", now)];
        assert!(
            take_pending_launch_placement(&mut pending, Some("org.example.app"), None, now)
                .is_none()
        );
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn any_registered_app_id_key_matches() {
        let now = std::time::Instant::now();
        let mut pending = vec![entry(
            &["org.example.App", "example-app"],
            None,
            "entry",
            now,
        )];
        let taken = take_pending_launch_placement(&mut pending, Some("example-app"), None, now);
        assert_eq!(label_of(taken).as_deref(), Some("entry"));
        assert!(pending.is_empty());
    }

    #[test]
    fn expired_entries_are_purged_and_skipped() {
        let now = std::time::Instant::now();
        let stale = now - LAUNCH_PLACEMENT_TTL;
        let mut pending = vec![
            entry(&[], Some(4242), "stale", stale),
            entry(&[], Some(4242), "fresh", now),
        ];
        let taken = take_pending_launch_placement(&mut pending, None, Some(4242), now);
        assert_eq!(label_of(taken).as_deref(), Some("fresh"));
        // The stale entry was purged, not just skipped.
        assert!(pending.is_empty());
    }

    #[test]
    fn ttl_boundary_age_is_purged() {
        let now = std::time::Instant::now();
        let at_ttl = now - LAUNCH_PLACEMENT_TTL;
        let mut pending = vec![entry(&["org.example.App"], None, "entry", at_ttl)];
        assert!(
            take_pending_launch_placement(&mut pending, Some("org.example.App"), None, now)
                .is_none()
        );
        assert!(pending.is_empty());
    }

    #[test]
    fn future_registered_at_is_kept_without_panicking() {
        let now = std::time::Instant::now();
        let future = now + std::time::Duration::from_secs(1);
        let mut pending = vec![entry(&["org.example.App"], None, "entry", future)];
        let taken = take_pending_launch_placement(&mut pending, Some("org.example.App"), None, now);
        assert_eq!(label_of(taken).as_deref(), Some("entry"));
    }

    #[test]
    fn empty_app_id_never_matches() {
        let now = std::time::Instant::now();
        let mut pending = vec![entry(&[""], None, "entry", now)];
        assert!(take_pending_launch_placement(&mut pending, Some(""), None, now).is_none());
        assert!(take_pending_launch_placement(&mut pending, None, None, now).is_none());
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn consumption_removes_the_entry() {
        let now = std::time::Instant::now();
        let mut pending = vec![entry(&["org.example.App"], Some(4242), "entry", now)];
        assert!(
            take_pending_launch_placement(&mut pending, Some("org.example.App"), Some(4242), now)
                .is_some()
        );
        assert!(pending.is_empty());
        assert!(
            take_pending_launch_placement(&mut pending, Some("org.example.App"), Some(4242), now)
                .is_none()
        );
    }

    #[test]
    fn none_pid_does_not_match_pid_entries() {
        let now = std::time::Instant::now();
        let mut pending = vec![entry(&[], Some(4242), "entry", now)];
        assert!(take_pending_launch_placement(&mut pending, None, None, now).is_none());
        assert_eq!(pending.len(), 1);
    }
}
