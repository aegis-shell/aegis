//! Authentication and presentation state machine.

use std::time::{Duration, Instant};

use crate::Secret;

const AMBIENT_AFTER: Duration = Duration::from_secs(30);
const ERROR_VISIBLE_FOR: Duration = Duration::from_secs(3);
const REJECTION_SHAKE_FOR: Duration = Duration::from_millis(420);
const VALIDATION_SWEEP_CYCLE: Duration = Duration::from_millis(1100);
const PASSWORD_CLEAR_AFTER: Duration = Duration::from_secs(30);
const MAX_AUTH_BACKOFF: Duration = Duration::from_secs(30);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthResult {
    Accepted,
    Rejected { message: String },
    Unavailable { message: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentationMode {
    Engaged,
    Ambient,
}

#[derive(Debug)]
enum AuthPhase {
    Ready,
    Waiting { started: Instant },
    Checking { started: Instant },
    Rejected { message: String, started: Instant },
    Unavailable { message: String },
    Accepted,
}

#[derive(Debug)]
pub enum LockAction {
    None,
    Authenticate(Secret),
    Unlock,
}

/// Pure lock-screen interaction state. Wayland, rendering, and PAM are hosts.
#[derive(Debug)]
pub struct LockState {
    secret: Secret,
    phase: AuthPhase,
    presentation: PresentationMode,
    failed_attempts: u32,
    caps_lock: bool,
    keyboard_layout: Option<String>,
    credential_revision: u64,
    locked_credential_len: Option<usize>,
    retry_not_before: Option<Instant>,
    last_interaction: Instant,
    clear_at: Option<Instant>,
}

impl LockState {
    #[must_use]
    pub fn new(now: Instant) -> Self {
        Self {
            secret: Secret::default(),
            phase: AuthPhase::Ready,
            presentation: PresentationMode::Engaged,
            failed_attempts: 0,
            caps_lock: false,
            keyboard_layout: None,
            credential_revision: 0,
            locked_credential_len: None,
            retry_not_before: None,
            last_interaction: now,
            clear_at: None,
        }
    }

    #[must_use]
    pub fn presentation(&self) -> PresentationMode {
        self.presentation
    }

    #[must_use]
    pub fn password_len(&self) -> usize {
        self.locked_credential_len
            .unwrap_or_else(|| self.secret.char_count())
    }

    /// Monotonic identity for the currently rendered credential value.
    ///
    /// Password contents remain secret; renderers use only this revision to
    /// invalidate retained text when an edit returns to a previously seen
    /// length/value (most visibly during Backspace).
    #[must_use]
    pub fn credential_revision(&self) -> u64 {
        self.credential_revision
    }

    #[must_use]
    pub fn failed_attempts(&self) -> u32 {
        self.failed_attempts
    }

    #[must_use]
    pub fn checking(&self) -> bool {
        matches!(self.phase, AuthPhase::Checking { .. })
    }

    #[must_use]
    pub fn validation_pending(&self) -> bool {
        matches!(
            self.phase,
            AuthPhase::Waiting { .. } | AuthPhase::Checking { .. }
        )
    }

    /// Elapsed time in the current waiting or checking phase.
    ///
    /// Feedback that derives from this (such as the stop-screen percentage
    /// counter) stays honest about a stalled authenticator instead of
    /// cycling forever.
    #[must_use]
    pub fn validation_elapsed(&self, now: Instant) -> Option<Duration> {
        match self.phase {
            AuthPhase::Waiting { started } | AuthPhase::Checking { started } => {
                Some(now.saturating_duration_since(started))
            }
            _ => None,
        }
    }

    /// Progress for the cinematic validation sweep.
    ///
    /// Waiting and active authentication share one fixed-rate clock. Its speed
    /// is independent of credential length and authentication backoff, and the
    /// phase transition does not restart the sweep.
    #[must_use]
    pub fn validation_feedback_progress(&self, now: Instant) -> Option<f32> {
        match self.phase {
            AuthPhase::Waiting { started } | AuthPhase::Checking { started } => {
                let cycle = VALIDATION_SWEEP_CYCLE.as_secs_f32();
                Some(now.saturating_duration_since(started).as_secs_f32() % cycle / cycle)
            }
            _ => None,
        }
        .map(|progress| progress.clamp(0.0, 1.0))
    }

    #[must_use]
    pub fn accepted(&self) -> bool {
        matches!(self.phase, AuthPhase::Accepted)
    }

    #[must_use]
    pub fn rejected(&self) -> bool {
        matches!(self.phase, AuthPhase::Rejected { .. })
    }

    /// Normalized progress for the short rejection shake. The error phase can
    /// remain active longer for authentication backoff, while motion ends
    /// quickly and deterministically.
    #[must_use]
    pub fn rejection_feedback_progress(&self, now: Instant) -> Option<f32> {
        let AuthPhase::Rejected { started, .. } = &self.phase else {
            return None;
        };
        let elapsed = now.saturating_duration_since(*started);
        (elapsed < REJECTION_SHAKE_FOR)
            .then(|| elapsed.as_secs_f32() / REJECTION_SHAKE_FOR.as_secs_f32())
    }

    #[must_use]
    pub fn message(&self) -> Option<&str> {
        match &self.phase {
            AuthPhase::Rejected { message, .. } | AuthPhase::Unavailable { message } => {
                Some(message)
            }
            _ => None,
        }
    }

    #[must_use]
    pub fn caps_lock(&self) -> bool {
        self.caps_lock
    }

    #[must_use]
    pub fn keyboard_layout(&self) -> Option<&str> {
        self.keyboard_layout.as_deref()
    }

    pub fn set_keyboard_status(&mut self, caps_lock: bool, layout: Option<String>) {
        self.caps_lock = caps_lock;
        self.keyboard_layout = layout.filter(|name| !name.is_empty());
    }

    /// Pointer/touch activity reveals the authentication controls without
    /// manufacturing password input.
    /// Reveal the authentication controls and refresh the privacy deadline.
    ///
    /// Returns `true` only when presentation changed, allowing motion-heavy
    /// input paths to avoid submitting identical frames.
    pub fn reveal(&mut self, now: Instant) -> bool {
        let changed = self.presentation != PresentationMode::Engaged;
        self.presentation = PresentationMode::Engaged;
        self.last_interaction = now;
        changed
    }

    pub fn type_text(&mut self, text: &str, now: Instant) -> bool {
        if self.validation_pending()
            || self.accepted()
            || text.is_empty()
            || text.chars().any(char::is_control)
        {
            return false;
        }
        self.reveal(now);
        let changed = self.secret.push_str(text);
        if changed {
            self.bump_credential_revision();
            // The first character of a new attempt ends the previous visual
            // rejection immediately. Authentication throttling is tracked
            // independently in `retry_not_before` and remains enforced.
            self.phase = AuthPhase::Ready;
            self.clear_at = Some(now + PASSWORD_CLEAR_AFTER);
        }
        changed
    }

    pub fn backspace(&mut self, now: Instant) -> bool {
        if self.validation_pending() || self.accepted() {
            return false;
        }
        self.reveal(now);
        let changed = self.secret.backspace();
        if changed {
            self.bump_credential_revision();
            self.phase = AuthPhase::Ready;
        }
        self.clear_at = (!self.secret.is_empty()).then_some(now + PASSWORD_CLEAR_AFTER);
        changed
    }

    pub fn clear(&mut self, now: Instant) {
        if self.validation_pending() || self.accepted() {
            return;
        }
        self.reveal(now);
        let changed = !self.secret.is_empty();
        self.secret.clear();
        if changed {
            self.bump_credential_revision();
        }
        self.phase = AuthPhase::Ready;
        self.clear_at = None;
    }

    #[must_use]
    pub fn submit(&mut self, now: Instant) -> LockAction {
        self.reveal(now);
        if self.validation_pending() || self.accepted() || self.secret.is_empty() {
            return LockAction::None;
        }
        if let Some(deadline) = self.retry_not_before
            && now < deadline
        {
            self.phase = AuthPhase::Waiting { started: now };
            self.clear_at = None;
            return LockAction::None;
        }
        self.begin_checking(now)
    }

    #[must_use]
    pub fn authentication_finished(&mut self, result: AuthResult, now: Instant) -> LockAction {
        // Results are capabilities to cross the lock boundary. Accept one
        // only while this state owns the corresponding in-flight attempt;
        // late, duplicated, or otherwise stale messages must not unlock.
        if !self.checking() {
            return LockAction::None;
        }
        self.locked_credential_len = None;
        self.bump_credential_revision();
        match result {
            AuthResult::Accepted => {
                self.phase = AuthPhase::Accepted;
                LockAction::Unlock
            }
            AuthResult::Rejected { message } => {
                self.failed_attempts = self.failed_attempts.saturating_add(1);
                let exponent = self.failed_attempts.saturating_sub(1).min(5);
                let delay = Duration::from_secs(1u64 << exponent).max(ERROR_VISIBLE_FOR);
                let retry_not_before = now + delay.min(MAX_AUTH_BACKOFF);
                self.phase = AuthPhase::Rejected {
                    message,
                    started: now,
                };
                self.retry_not_before = Some(retry_not_before);
                LockAction::None
            }
            AuthResult::Unavailable { message } => {
                self.phase = AuthPhase::Unavailable { message };
                LockAction::None
            }
        }
    }

    /// Advance privacy and feedback deadlines.
    ///
    /// The boolean reports whether presentation changed. The action starts a
    /// submission that was queued by Enter during authentication backoff.
    pub fn tick(&mut self, now: Instant) -> (bool, LockAction) {
        let mut changed = false;
        let mut action = LockAction::None;
        if self.clear_at.is_some_and(|deadline| now >= deadline) {
            self.secret.clear();
            self.bump_credential_revision();
            self.clear_at = None;
            changed = true;
        }
        if self
            .retry_not_before
            .is_some_and(|deadline| now >= deadline)
        {
            self.retry_not_before = None;
            let waiting_started = match self.phase {
                AuthPhase::Waiting { started } => Some(started),
                _ => None,
            };
            let was_waiting = waiting_started.is_some();
            let was_rejected = matches!(self.phase, AuthPhase::Rejected { .. });
            match (was_waiting, was_rejected, self.secret.is_empty()) {
                (true, _, false) => {
                    action = self.begin_checking(waiting_started.unwrap_or(now));
                    changed = true;
                }
                (true, _, true) | (_, true, _) => {
                    self.phase = AuthPhase::Ready;
                    changed = true;
                }
                _ => {}
            }
        }
        if self.presentation == PresentationMode::Engaged
            && now.duration_since(self.last_interaction) >= AMBIENT_AFTER
            && !self.checking()
            && self.secret.is_empty()
        {
            self.presentation = PresentationMode::Ambient;
            changed = true;
        }
        (changed, action)
    }

    fn bump_credential_revision(&mut self) {
        self.credential_revision = self.credential_revision.wrapping_add(1);
    }

    fn begin_checking(&mut self, feedback_started: Instant) -> LockAction {
        debug_assert!(!self.secret.is_empty());
        self.retry_not_before = None;
        self.locked_credential_len = Some(self.secret.char_count());
        self.phase = AuthPhase::Checking {
            started: feedback_started,
        };
        self.clear_at = None;
        self.bump_credential_revision();
        LockAction::Authenticate(self.secret.take())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_submission_never_reaches_authenticator() {
        let now = Instant::now();
        let mut lock = LockState::new(now);
        assert!(matches!(lock.submit(now), LockAction::None));
    }

    #[test]
    fn password_moves_once_and_rejection_counts() {
        let now = Instant::now();
        let mut lock = LockState::new(now);
        assert!(lock.type_text("secret", now));
        let LockAction::Authenticate(secret) = lock.submit(now) else {
            panic!("credential was not submitted");
        };
        assert_eq!(secret.len(), 6);
        assert_eq!(lock.password_len(), 6);
        assert!(matches!(
            lock.authentication_finished(
                AuthResult::Rejected {
                    message: "Incorrect password".into()
                },
                now
            ),
            LockAction::None
        ));
        assert_eq!(lock.password_len(), 0);
        assert_eq!(lock.failed_attempts(), 1);
        assert!(lock.type_text("again", now));
        assert!(matches!(lock.submit(now), LockAction::None));
        assert!(lock.validation_pending());
        let (changed, action) = lock.tick(now + ERROR_VISIBLE_FOR);
        assert!(changed);
        assert!(matches!(action, LockAction::Authenticate(_)));
    }

    #[test]
    fn idle_empty_ui_becomes_ambient_and_input_reveals_it() {
        let now = Instant::now();
        let mut lock = LockState::new(now);
        assert!(lock.tick(now + AMBIENT_AFTER).0);
        assert_eq!(lock.presentation(), PresentationMode::Ambient);
        assert!(lock.reveal(now + AMBIENT_AFTER));
        assert_eq!(lock.presentation(), PresentationMode::Engaged);
        assert!(!lock.reveal(now + AMBIENT_AFTER + Duration::from_secs(1)));
    }

    #[test]
    fn stale_authentication_results_cannot_unlock() {
        let now = Instant::now();
        let mut lock = LockState::new(now);
        assert!(matches!(
            lock.authentication_finished(AuthResult::Accepted, now),
            LockAction::None
        ));
        assert!(!lock.accepted());
    }

    #[test]
    fn rejection_feedback_is_brief_even_while_backoff_remains_active() {
        let now = Instant::now();
        let mut lock = LockState::new(now);
        assert!(lock.type_text("secret", now));
        assert!(matches!(lock.submit(now), LockAction::Authenticate(_)));
        assert!(matches!(
            lock.authentication_finished(
                AuthResult::Rejected {
                    message: "Incorrect password".into(),
                },
                now,
            ),
            LockAction::None
        ));

        assert!(lock.rejected());
        assert_eq!(lock.rejection_feedback_progress(now), Some(0.0));
        assert!(
            lock.rejection_feedback_progress(now + REJECTION_SHAKE_FOR / 2)
                .is_some_and(|progress| progress > 0.0 && progress < 1.0)
        );
        assert_eq!(
            lock.rejection_feedback_progress(now + REJECTION_SHAKE_FOR),
            None
        );
        assert!(lock.rejected());
    }

    #[test]
    fn new_attempt_clears_rejection_visual_without_bypassing_backoff() {
        let now = Instant::now();
        let mut lock = LockState::new(now);
        assert!(lock.type_text("wrong", now));
        assert!(matches!(lock.submit(now), LockAction::Authenticate(_)));
        assert!(matches!(
            lock.authentication_finished(
                AuthResult::Rejected {
                    message: "Incorrect password".into(),
                },
                now,
            ),
            LockAction::None
        ));

        assert!(lock.rejected());
        assert!(lock.type_text("n", now + Duration::from_millis(500)));
        assert!(!lock.rejected());
        assert_eq!(lock.rejection_feedback_progress(now), None);
        assert!(matches!(
            lock.submit(now + Duration::from_millis(500)),
            LockAction::None
        ));
        assert!(lock.validation_pending());
        let (changed, action) = lock.tick(now + ERROR_VISIBLE_FOR);
        assert!(changed);
        assert!(matches!(action, LockAction::Authenticate(_)));
    }

    #[test]
    fn validation_feedback_keeps_fixed_speed_across_waiting_and_checking() {
        let now = Instant::now();
        let mut lock = LockState::new(now);
        assert!(lock.type_text("wrong", now));
        assert!(matches!(lock.submit(now), LockAction::Authenticate(_)));
        assert!(matches!(
            lock.authentication_finished(
                AuthResult::Rejected {
                    message: "Incorrect password".into(),
                },
                now,
            ),
            LockAction::None
        ));
        assert!(lock.type_text("next", now + Duration::from_millis(250)));
        assert!(matches!(
            lock.submit(now + Duration::from_millis(250)),
            LockAction::None
        ));
        assert!(lock.validation_pending());
        assert_eq!(
            lock.validation_feedback_progress(now + Duration::from_millis(250)),
            Some(0.0)
        );
        assert_progress_near(
            lock.validation_feedback_progress(now + Duration::from_millis(525)),
            0.25,
        );

        let before_transition = lock.validation_feedback_progress(now + ERROR_VISIBLE_FOR);
        assert_progress_near(before_transition, 0.5);

        let (changed, action) = lock.tick(now + ERROR_VISIBLE_FOR);
        assert!(changed);
        assert!(matches!(action, LockAction::Authenticate(_)));
        assert!(lock.checking());
        assert_progress_near(
            lock.validation_feedback_progress(now + ERROR_VISIBLE_FOR),
            0.5,
        );
    }

    #[test]
    fn validation_feedback_is_independent_of_credential_length() {
        let now = Instant::now();
        let mut short = LockState::new(now);
        let mut long = LockState::new(now);
        assert!(short.type_text("x", now));
        assert!(long.type_text("a much longer credential", now));
        assert!(matches!(short.submit(now), LockAction::Authenticate(_)));
        assert!(matches!(long.submit(now), LockAction::Authenticate(_)));

        let sample = now + Duration::from_millis(275);
        assert_eq!(
            short.validation_feedback_progress(sample),
            long.validation_feedback_progress(sample)
        );
        assert_progress_near(short.validation_feedback_progress(sample), 0.25);
    }

    #[test]
    fn validation_locks_credential_during_waiting_and_checking() {
        let now = Instant::now();
        let mut lock = LockState::new(now);
        assert!(lock.type_text("wrong", now));
        assert!(matches!(lock.submit(now), LockAction::Authenticate(_)));
        assert_eq!(lock.password_len(), 5);
        assert!(!lock.type_text("ignored", now));
        assert!(!lock.backspace(now));
        lock.clear(now);
        assert!(matches!(lock.submit(now), LockAction::None));
        assert_eq!(lock.password_len(), 5);
        assert!(matches!(
            lock.authentication_finished(
                AuthResult::Rejected {
                    message: "Incorrect password".into(),
                },
                now,
            ),
            LockAction::None
        ));

        let queued_at = now + Duration::from_millis(250);
        assert!(lock.type_text("next", queued_at));
        assert!(matches!(lock.submit(queued_at), LockAction::None));
        assert!(lock.validation_pending());
        assert!(!lock.type_text("ignored", queued_at));
        assert!(!lock.backspace(queued_at));
        lock.clear(queued_at);
        assert!(matches!(lock.submit(queued_at), LockAction::None));
        assert_eq!(lock.password_len(), 4);

        let (_, action) = lock.tick(now + ERROR_VISIBLE_FOR);
        let LockAction::Authenticate(secret) = action else {
            panic!("queued credential was not submitted");
        };
        assert_eq!(secret.len(), 4);
        assert_eq!(lock.password_len(), 4);
        assert!(!lock.type_text("ignored", now + ERROR_VISIBLE_FOR));
        assert!(!lock.backspace(now + ERROR_VISIBLE_FOR));
        lock.clear(now + ERROR_VISIBLE_FOR);
        assert!(matches!(
            lock.submit(now + ERROR_VISIBLE_FOR),
            LockAction::None
        ));
        assert_eq!(lock.password_len(), 4);
    }

    #[test]
    fn credential_revision_tracks_insert_delete_clear_and_submit() {
        let now = Instant::now();
        let mut lock = LockState::new(now);
        assert_eq!(lock.credential_revision(), 0);

        assert!(lock.type_text("ab", now));
        assert_eq!(lock.credential_revision(), 1);
        assert!(lock.backspace(now));
        assert_eq!(lock.password_len(), 1);
        assert_eq!(lock.credential_revision(), 2);
        lock.clear(now);
        assert_eq!(lock.password_len(), 0);
        assert_eq!(lock.credential_revision(), 3);
        assert!(!lock.backspace(now));
        assert_eq!(lock.credential_revision(), 3);

        assert!(lock.type_text("c", now));
        assert!(matches!(lock.submit(now), LockAction::Authenticate(_)));
        assert_eq!(lock.password_len(), 1);
        assert_eq!(lock.credential_revision(), 5);
        assert!(matches!(
            lock.authentication_finished(
                AuthResult::Rejected {
                    message: "Incorrect password".into(),
                },
                now,
            ),
            LockAction::None
        ));
        assert_eq!(lock.password_len(), 0);
        assert_eq!(lock.credential_revision(), 6);
    }

    fn assert_progress_near(actual: Option<f32>, expected: f32) {
        let actual = actual.expect("validation feedback should be active");
        assert!((actual - expected).abs() < 0.001, "{actual} != {expected}");
    }
}
