use std::time::{Duration, Instant};

const PRESENTATION_WATCHDOG: Duration = Duration::from_secs(1);

/// Redraw lifecycle for one presentation domain.
///
/// Aegis currently submits one atomic KMS batch spanning every active CRTC,
/// so the domain is the host rather than an individual connector. Keeping
/// this state in the runtime makes the ownership rule explicit: input and
/// Wayland traffic may continue while a commit is in flight, but a second
/// render cannot start until the backend retires that batch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PresentationState {
    /// No visible work is known.
    Idle,
    /// A redraw may start immediately.
    Queued,
    /// The runtime has consumed a queued redraw and is synchronously building
    /// its outcome. `estimated_deadline` preserves a no-damage callback cycle
    /// when real damage queued an early assessment inside that cycle.
    Rendering { estimated_deadline: Option<Instant> },
    /// A frame is owned by the presentation backend until vblank.
    WaitingForVblank {
        /// Visible state changed while the submitted frame was in flight.
        redraw_queued: bool,
        submitted_at: Instant,
        stall_reported: bool,
    },
    /// No backend vblank will arrive. The estimated boundary throttles
    /// no-damage frame callbacks and nested animation frames without hiding
    /// newly damaged content: `redraw_queued` makes the state renderable.
    WaitingForEstimatedVblank {
        deadline: Instant,
        redraw_queued: bool,
    },
    /// A transient backend rejection (EBUSY: KMS still owns the previous
    /// batch) parks the retry until the next estimated vblank. Retrying
    /// immediately would spin the loop on zero-timeout waits, rendering
    /// full frames back-to-back for a commit that cannot land yet.
    Retrying { not_before: Instant },
    /// Presentation is unavailable, with the reason retained so losing the
    /// backend after output power-off still invalidates the input epoch.
    Suspended(PresentationAvailability),
}

/// State-machine facade used by the compositor loop.
pub(super) struct PresentationScheduler {
    state: PresentationState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum PresentationAvailability {
    Available,
    /// Scanout is deliberately dark, but the active input epoch remains valid
    /// so physical activity can wake a locked session.
    OutputsOff,
    /// No renderable output target currently exists, but the backend and
    /// active input epoch remain valid (for example, all connectors unplugged).
    TargetUnavailable,
    /// The backend/device epoch is gone (VT loss, seat revoke, or backend
    /// failure).
    BackendUnavailable,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum ActivationChange {
    None,
    Suspended(PresentationAvailability),
    /// The backend epoch changed after presentation was already suspended for
    /// another reason, so input ownership still needs invalidation.
    BackendEpochInvalidated,
    Resumed,
}

impl PresentationScheduler {
    pub(super) fn new() -> Self {
        Self {
            // The first frame is compositor-owned work and must not wait for
            // an input event to make the desktop visible.
            state: PresentationState::Queued,
        }
    }

    pub(super) fn set_availability(
        &mut self,
        availability: PresentationAvailability,
    ) -> ActivationChange {
        if availability == PresentationAvailability::Available {
            if matches!(self.state, PresentationState::Suspended(_)) {
                self.state = PresentationState::Queued;
                return ActivationChange::Resumed;
            }
            return ActivationChange::None;
        }

        if let PresentationState::Suspended(previous) = self.state {
            self.state = PresentationState::Suspended(availability);
            if previous != PresentationAvailability::BackendUnavailable
                && availability == PresentationAvailability::BackendUnavailable
            {
                return ActivationChange::BackendEpochInvalidated;
            }
            return ActivationChange::None;
        }

        self.state = PresentationState::Suspended(availability);
        ActivationChange::Suspended(availability)
    }

    pub(super) fn reconcile_backend(&mut self, presentation_pending: bool) {
        if presentation_pending {
            return;
        }
        if let PresentationState::WaitingForVblank { redraw_queued, .. } = self.state {
            self.state = if redraw_queued {
                PresentationState::Queued
            } else {
                PresentationState::Idle
            };
        }
    }

    /// Advance an estimated-vblank timer.
    ///
    /// Returns `true` only when the boundary elapsed without a redraw waiting.
    /// The runtime can then complete callbacks directly instead of performing
    /// a no-damage render assessment solely to reopen the callback cycle.
    pub(super) fn tick(&mut self, now: Instant) -> bool {
        if let PresentationState::Retrying { not_before } = self.state
            && now >= not_before
        {
            // A parked retry promotes to a fresh render; this is not an
            // estimated-vblank callback boundary.
            self.state = PresentationState::Queued;
            return false;
        }
        if let PresentationState::WaitingForEstimatedVblank {
            deadline,
            redraw_queued,
        } = self.state
            && now >= deadline
        {
            self.state = if redraw_queued {
                PresentationState::Queued
            } else {
                PresentationState::Idle
            };
            return !redraw_queued;
        }
        false
    }

    pub(super) fn queue_redraw(&mut self) {
        self.state = match self.state {
            PresentationState::Idle => PresentationState::Queued,
            PresentationState::Queued => PresentationState::Queued,
            PresentationState::Rendering { .. } => {
                panic!("redraw queued during a synchronous render transaction")
            }
            PresentationState::WaitingForVblank {
                submitted_at,
                stall_reported,
                ..
            } => PresentationState::WaitingForVblank {
                redraw_queued: true,
                submitted_at,
                stall_reported,
            },
            PresentationState::WaitingForEstimatedVblank { deadline, .. } => {
                PresentationState::WaitingForEstimatedVblank {
                    deadline,
                    redraw_queued: true,
                }
            }
            // A parked retry already implies a redraw; fresh damage simply
            // waits for the same deadline.
            PresentationState::Retrying { not_before } => {
                PresentationState::Retrying { not_before }
            }
            PresentationState::Suspended(availability) => {
                PresentationState::Suspended(availability)
            }
        };
    }

    pub(super) fn can_redraw(&self) -> bool {
        matches!(
            self.state,
            PresentationState::Queued
                | PresentationState::WaitingForEstimatedVblank {
                    redraw_queued: true,
                    ..
                }
        )
    }

    /// No-damage callbacks are allowed once per estimated refresh cycle.
    pub(super) fn frame_callbacks_allowed(&self) -> bool {
        !matches!(
            self.state,
            PresentationState::WaitingForEstimatedVblank { .. }
                | PresentationState::Rendering {
                    estimated_deadline: Some(_)
                }
        )
    }

    /// Consume the queued edge and enter the synchronous render transaction.
    pub(super) fn begin_redraw(&mut self) {
        self.state = match self.state {
            PresentationState::Queued => PresentationState::Rendering {
                estimated_deadline: None,
            },
            PresentationState::WaitingForEstimatedVblank {
                deadline,
                redraw_queued: true,
            } => PresentationState::Rendering {
                estimated_deadline: Some(deadline),
            },
            _ => panic!("redraw began outside a queued presentation state"),
        };
    }

    pub(super) fn submitted(
        &mut self,
        presentation_pending: bool,
        redraw_after_present: bool,
        pacing_anchor: Instant,
        refresh_interval: Duration,
    ) {
        assert!(
            matches!(self.state, PresentationState::Rendering { .. }),
            "submission completed outside a render transaction"
        );
        self.state = if presentation_pending {
            PresentationState::WaitingForVblank {
                redraw_queued: redraw_after_present,
                submitted_at: pacing_anchor,
                stall_reported: false,
            }
        } else if redraw_after_present {
            Self::estimated(pacing_anchor, refresh_interval)
        } else {
            PresentationState::Idle
        };
    }

    pub(super) fn no_damage(
        &mut self,
        callbacks_sent: bool,
        redraw_after_cycle: bool,
        now: Instant,
        refresh_interval: Duration,
    ) {
        let PresentationState::Rendering { estimated_deadline } = self.state else {
            panic!("no-damage outcome completed outside a render transaction");
        };
        if let Some(deadline) = estimated_deadline {
            // A redraw queued during the closed callback cycle found no new
            // output damage. Keep the original deadline; replacing it would
            // let a frame-only client postpone the boundary forever.
            self.state = PresentationState::WaitingForEstimatedVblank {
                deadline,
                redraw_queued: false,
            };
        } else if callbacks_sent || redraw_after_cycle {
            self.state = Self::estimated(now, refresh_interval);
        } else {
            self.state = PresentationState::Idle;
        }
    }

    /// Close a callback cycle that was completed directly at an estimated
    /// boundary, without entering the renderer.
    pub(super) fn callbacks_sent_at_estimated_vblank(
        &mut self,
        callbacks_sent: bool,
        now: Instant,
        refresh_interval: Duration,
    ) {
        assert!(
            matches!(self.state, PresentationState::Idle),
            "estimated-vblank callback completed outside the idle boundary"
        );
        if callbacks_sent {
            self.state = Self::estimated(now, refresh_interval);
        }
    }

    /// How long a vblank wait may outlive its stall warning before the flip
    /// event is declared lost and scanout ownership is reclaimed.
    const RECOVERY_TIMEOUT: Duration = Duration::from_secs(3);

    pub(super) fn retry_at(&mut self, not_before: Instant) {
        assert!(
            matches!(self.state, PresentationState::Rendering { .. }),
            "retry completed outside a render transaction"
        );
        self.state = PresentationState::Retrying { not_before };
    }

    /// Report a page flip that exceeded the ownership watchdog once per
    /// submission. The state remains blocked at this tier: a late flip is
    /// not permission to reuse a scanout buffer that KMS may still own.
    /// `take_recovery_due` provides the second tier for a flip whose event
    /// never arrives at all.
    pub(super) fn take_stall_warning(&mut self, now: Instant) -> Option<Duration> {
        let PresentationState::WaitingForVblank {
            submitted_at,
            stall_reported,
            ..
        } = &mut self.state
        else {
            return None;
        };
        let elapsed = now.saturating_duration_since(*submitted_at);
        if !*stall_reported && elapsed >= PRESENTATION_WATCHDOG {
            *stall_reported = true;
            Some(elapsed)
        } else {
            None
        }
    }

    /// Second watchdog tier: a flip event still missing
    /// `RECOVERY_TIMEOUT` after submission is treated as lost. Remaining
    /// blocked would freeze presentation forever, so the caller reclaims
    /// backend ownership and forces a full redraw. If KMS genuinely still
    /// owns the batch, the next commit comes back EBUSY and the paced retry
    /// keeps the loop alive instead. Fires once per submission.
    pub(super) fn take_recovery_due(&mut self, now: Instant) -> bool {
        let due = matches!(
            self.state,
            PresentationState::WaitingForVblank {
                submitted_at,
                stall_reported: true,
                ..
            } if now.saturating_duration_since(submitted_at) >= Self::RECOVERY_TIMEOUT
        );
        if due {
            self.state = PresentationState::Queued;
        }
        due
    }

    pub(super) fn wait_timeout(&self, idle_timeout: Duration, now: Instant) -> Duration {
        match self.state {
            PresentationState::Queued
            | PresentationState::WaitingForEstimatedVblank {
                redraw_queued: true,
                ..
            } => Duration::ZERO,
            PresentationState::Rendering { .. } => {
                panic!("event wait requested during a render transaction")
            }
            PresentationState::Retrying { not_before } => {
                not_before.saturating_duration_since(now).min(idle_timeout)
            }
            PresentationState::WaitingForEstimatedVblank { deadline, .. } => {
                deadline.saturating_duration_since(now).min(idle_timeout)
            }
            // Visual timers cannot make progress while KMS owns a submitted
            // batch. Ignoring them prevents expired wallpaper timers from
            // turning the vblank wait into a busy poll.
            PresentationState::WaitingForVblank {
                submitted_at,
                stall_reported,
                ..
            } => {
                if stall_reported {
                    PRESENTATION_WATCHDOG
                } else {
                    (submitted_at + PRESENTATION_WATCHDOG).saturating_duration_since(now)
                }
            }
            PresentationState::Idle => idle_timeout,
            // The seat or output is unavailable, so visual timers are also
            // irrelevant. Backend fds still wake this bounded maintenance wait.
            PresentationState::Suspended(_) => PRESENTATION_WATCHDOG,
        }
    }

    fn estimated(now: Instant, refresh_interval: Duration) -> PresentationState {
        PresentationState::WaitingForEstimatedVblank {
            deadline: now + refresh_interval,
            redraw_queued: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FRAME: Duration = Duration::from_millis(16);

    #[test]
    fn damage_coalesces_while_waiting_for_vblank() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.submitted(true, false, now, FRAME);
        assert!(!state.can_redraw());

        state.queue_redraw();
        assert!(!state.can_redraw());
        state.reconcile_backend(false);
        assert!(state.can_redraw());
    }

    #[test]
    fn completed_frame_without_new_damage_returns_idle() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.submitted(true, false, now, FRAME);
        state.reconcile_backend(false);
        assert!(!state.can_redraw());
        assert_eq!(
            state.wait_timeout(Duration::from_secs(1), now),
            Duration::from_secs(1)
        );
    }

    #[test]
    fn no_damage_callback_closes_cycle_until_estimated_vblank() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.no_damage(true, false, now, FRAME);
        assert!(!state.frame_callbacks_allowed());

        state.queue_redraw();
        assert!(state.can_redraw());
        state.begin_redraw();
        state.no_damage(false, false, now + Duration::from_millis(1), FRAME);
        assert!(!state.can_redraw());

        assert!(state.tick(now + FRAME));
        assert!(!state.can_redraw());
        assert!(state.frame_callbacks_allowed());
    }

    #[test]
    fn nested_animation_uses_estimated_boundary_but_input_can_redraw_early() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.submitted(false, true, now, FRAME);
        assert!(!state.can_redraw());

        state.queue_redraw();
        assert!(state.can_redraw());
    }

    #[test]
    fn suspend_blocks_and_resume_forces_one_redraw() {
        let mut state = PresentationScheduler::new();
        assert_eq!(
            state.set_availability(PresentationAvailability::OutputsOff),
            ActivationChange::Suspended(PresentationAvailability::OutputsOff)
        );
        assert_eq!(
            state.set_availability(PresentationAvailability::OutputsOff),
            ActivationChange::None
        );
        state.queue_redraw();
        assert!(!state.can_redraw());
        assert_eq!(
            state.set_availability(PresentationAvailability::Available),
            ActivationChange::Resumed
        );
        assert_eq!(
            state.set_availability(PresentationAvailability::Available),
            ActivationChange::None
        );
        assert!(state.can_redraw());
    }

    #[test]
    fn backend_loss_after_input_preserving_suspension_invalidates_the_epoch() {
        for reason in [
            PresentationAvailability::OutputsOff,
            PresentationAvailability::TargetUnavailable,
        ] {
            let mut state = PresentationScheduler::new();
            state.set_availability(reason);
            assert_eq!(
                state.set_availability(PresentationAvailability::BackendUnavailable),
                ActivationChange::BackendEpochInvalidated
            );
            assert_eq!(
                state.set_availability(PresentationAvailability::BackendUnavailable),
                ActivationChange::None
            );
        }
    }

    #[test]
    fn estimated_boundary_promotes_only_an_accumulated_redraw() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.no_damage(true, false, now, FRAME);
        state.queue_redraw();

        // Queued real work may render before the estimated boundary; if the
        // loop has not consumed it yet, the boundary keeps it queued.
        assert!(state.can_redraw());
        assert!(!state.tick(now + FRAME - Duration::from_nanos(1)));
        assert!(state.can_redraw());
        assert!(!state.tick(now + FRAME));
        assert!(state.can_redraw());
    }

    #[test]
    fn callback_sent_at_estimated_boundary_starts_the_next_cycle() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.no_damage(true, false, now, FRAME);
        assert!(state.tick(now + FRAME));

        state.callbacks_sent_at_estimated_vblank(true, now + FRAME, FRAME);
        assert!(!state.frame_callbacks_allowed());
        assert_eq!(
            state.wait_timeout(Duration::from_secs(1), now + FRAME),
            FRAME
        );
    }

    #[test]
    fn a_transient_rejection_parks_the_retry_until_the_next_vblank() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.retry_at(now + FRAME);

        // No zero-timeout spin: the loop waits out the estimated vblank,
        // and fresh damage is absorbed by the parked retry.
        assert!(!state.can_redraw());
        assert_eq!(state.wait_timeout(Duration::from_secs(1), now), FRAME);
        state.queue_redraw();
        assert!(!state.can_redraw());

        assert!(!state.tick(now + FRAME - Duration::from_nanos(1)));
        assert!(!state.can_redraw());
        assert!(!state.tick(now + FRAME));
        assert!(state.can_redraw());
        state.begin_redraw();
    }

    #[test]
    #[should_panic(expected = "redraw began outside a queued presentation state")]
    fn a_render_transaction_cannot_begin_twice() {
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.begin_redraw();
    }

    #[test]
    fn real_vblank_wait_ignores_an_expired_visual_timer() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.submitted(true, false, now, FRAME);

        assert_eq!(
            state.wait_timeout(Duration::ZERO, now),
            PRESENTATION_WATCHDOG
        );
    }

    #[test]
    fn stalled_flip_warns_once_without_releasing_ownership() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.submitted(true, false, now, FRAME);

        assert_eq!(
            state.take_stall_warning(now + PRESENTATION_WATCHDOG),
            Some(PRESENTATION_WATCHDOG)
        );
        assert_eq!(
            state.take_stall_warning(now + PRESENTATION_WATCHDOG * 2),
            None
        );
        assert!(!state.can_redraw());
        assert_eq!(
            state.wait_timeout(Duration::ZERO, now + PRESENTATION_WATCHDOG),
            PRESENTATION_WATCHDOG
        );
    }

    #[test]
    fn a_lost_flip_event_recovers_instead_of_freezing() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.begin_redraw();
        state.submitted(true, false, now, FRAME);

        // First tier warns once and stays blocked.
        assert!(
            state
                .take_stall_warning(now + PRESENTATION_WATCHDOG)
                .is_some()
        );
        assert!(!state.can_redraw());
        assert!(!state.take_recovery_due(now + PRESENTATION_WATCHDOG));

        // Second tier reclaims ownership and requeues exactly once.
        assert!(state.take_recovery_due(now + PresentationScheduler::RECOVERY_TIMEOUT));
        assert!(state.can_redraw());
        assert!(!state.take_recovery_due(now + PresentationScheduler::RECOVERY_TIMEOUT));
    }

    #[test]
    fn suspended_state_ignores_visual_timers() {
        let now = Instant::now();
        let mut state = PresentationScheduler::new();
        state.set_availability(PresentationAvailability::BackendUnavailable);
        assert_eq!(
            state.wait_timeout(Duration::ZERO, now),
            PRESENTATION_WATCHDOG
        );
    }
}
