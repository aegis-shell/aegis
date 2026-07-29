//! Authentication and presentation state machine.

use std::time::{Duration, Instant};

use crate::Secret;

const AMBIENT_AFTER: Duration = Duration::from_secs(30);
const ERROR_VISIBLE_FOR: Duration = Duration::from_secs(3);
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
    Checking,
    Rejected { message: String, until: Instant },
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
        self.secret.char_count()
    }

    #[must_use]
    pub fn failed_attempts(&self) -> u32 {
        self.failed_attempts
    }

    #[must_use]
    pub fn checking(&self) -> bool {
        matches!(self.phase, AuthPhase::Checking)
    }

    #[must_use]
    pub fn accepted(&self) -> bool {
        matches!(self.phase, AuthPhase::Accepted)
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
        if self.checking() || self.accepted() || text.chars().any(char::is_control) {
            return false;
        }
        self.reveal(now);
        let changed = self.secret.push_str(text);
        if changed {
            if !matches!(self.phase, AuthPhase::Rejected { .. }) {
                self.phase = AuthPhase::Ready;
            }
            self.clear_at = Some(now + PASSWORD_CLEAR_AFTER);
        }
        changed
    }

    pub fn backspace(&mut self, now: Instant) -> bool {
        if self.checking() || self.accepted() {
            return false;
        }
        self.reveal(now);
        let changed = self.secret.backspace();
        if !matches!(self.phase, AuthPhase::Rejected { .. }) {
            self.phase = AuthPhase::Ready;
        }
        self.clear_at = (!self.secret.is_empty()).then_some(now + PASSWORD_CLEAR_AFTER);
        changed
    }

    pub fn clear(&mut self, now: Instant) {
        if self.checking() || self.accepted() {
            return;
        }
        self.reveal(now);
        self.secret.clear();
        if !matches!(self.phase, AuthPhase::Rejected { .. }) {
            self.phase = AuthPhase::Ready;
        }
        self.clear_at = None;
    }

    #[must_use]
    pub fn submit(&mut self, now: Instant) -> LockAction {
        self.reveal(now);
        if matches!(self.phase, AuthPhase::Rejected { until, .. } if now < until) {
            return LockAction::None;
        }
        if self.checking() || self.accepted() || self.secret.is_empty() {
            return LockAction::None;
        }
        self.phase = AuthPhase::Checking;
        self.clear_at = None;
        LockAction::Authenticate(self.secret.take())
    }

    #[must_use]
    pub fn authentication_finished(&mut self, result: AuthResult, now: Instant) -> LockAction {
        // Results are capabilities to cross the lock boundary. Accept one
        // only while this state owns the corresponding in-flight attempt;
        // late, duplicated, or otherwise stale messages must not unlock.
        if !self.checking() {
            return LockAction::None;
        }
        match result {
            AuthResult::Accepted => {
                self.phase = AuthPhase::Accepted;
                LockAction::Unlock
            }
            AuthResult::Rejected { message } => {
                self.failed_attempts = self.failed_attempts.saturating_add(1);
                let exponent = self.failed_attempts.saturating_sub(1).min(5);
                let delay = Duration::from_secs(1u64 << exponent).max(ERROR_VISIBLE_FOR);
                self.phase = AuthPhase::Rejected {
                    message,
                    until: now + delay.min(MAX_AUTH_BACKOFF),
                };
                LockAction::None
            }
            AuthResult::Unavailable { message } => {
                self.phase = AuthPhase::Unavailable { message };
                LockAction::None
            }
        }
    }

    /// Advance privacy and feedback deadlines. Returns whether presentation
    /// changed and a new frame is required.
    pub fn tick(&mut self, now: Instant) -> bool {
        let mut changed = false;
        if self.clear_at.is_some_and(|deadline| now >= deadline) {
            self.secret.clear();
            self.clear_at = None;
            changed = true;
        }
        if matches!(
            self.phase,
            AuthPhase::Rejected { until, .. } if now >= until
        ) {
            self.phase = AuthPhase::Ready;
            changed = true;
        }
        if self.presentation == PresentationMode::Engaged
            && now.duration_since(self.last_interaction) >= AMBIENT_AFTER
            && !self.checking()
            && self.secret.is_empty()
        {
            self.presentation = PresentationMode::Ambient;
            changed = true;
        }
        changed
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
        assert_eq!(lock.password_len(), 0);
        assert!(matches!(
            lock.authentication_finished(
                AuthResult::Rejected {
                    message: "Incorrect password".into()
                },
                now
            ),
            LockAction::None
        ));
        assert_eq!(lock.failed_attempts(), 1);
        assert!(lock.type_text("again", now));
        assert!(matches!(lock.submit(now), LockAction::None));
        assert!(matches!(
            lock.submit(now + ERROR_VISIBLE_FOR),
            LockAction::Authenticate(_)
        ));
    }

    #[test]
    fn idle_empty_ui_becomes_ambient_and_input_reveals_it() {
        let now = Instant::now();
        let mut lock = LockState::new(now);
        assert!(lock.tick(now + AMBIENT_AFTER));
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
}
