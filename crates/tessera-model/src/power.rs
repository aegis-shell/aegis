//! Session power modes: the user-facing policy layered over the staged idle
//! pipeline (ADR-0140).
//!
//! The idle pipeline stays mechanism: four optional stages (dim, lock,
//! display-off, suspend) whose timers the compositor evaluates and whose
//! secure actions the [`crate::system`] boundary gates. A power mode is pure
//! policy: a named set of stages the user has asked to keep **armed** for the
//! rest of the session. Selectively not arming a stage's
//! `ext_idle_notification_v1` object expresses "inhibit" without touching the
//! compositor's inhibitor evaluation, which continues to serve per-surface
//! and connection-scoped application inhibitors unchanged.
//!
//! Modes are session-scoped runtime state, never persisted (the same contract
//! as the session idle inhibitor they generalize). Manual locking
//! (`Super+L`) and lock-before-sleep are outside every mode by construction:
//! they enter the pipeline through `require_lock()`, not through a stage
//! notification, so no mask can reach them.

/// One stage of the idle pipeline a mode can choose to keep armed.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdleStageSelector {
    Dim,
    Lock,
    DisplayOff,
    Suspend,
}

/// A session power mode: which idle stages stay armed.
///
/// The mask is derived, never stored: each mode names the stages it keeps so
/// the mapping is reviewable in one place and impossible to configure into a
/// state the security boundary rejects. Three modes cover the product's
/// four-quadrant toggle space: the fourth quadrant ("never blank, never
/// lock") is unreachable by policy — blanking or suspending an unlocked
/// session is forbidden — so it projects onto [`PowerMode::Awake`] with the
/// dim stage as its only idle response.
#[cfg_attr(
    feature = "serde",
    derive(serde::Serialize, serde::Deserialize),
    serde(rename_all = "snake_case")
)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash)]
pub enum PowerMode {
    /// Full staged policy: dim, lock, display-off, suspend.
    #[default]
    Balanced,
    /// Keep the session awake and unlocked: only dimming remains armed.
    /// The screen still dims (burn-in and sanity protection); nothing locks,
    /// powers off, or suspends.
    Awake,
    /// Lock on schedule but never blank: the lock stage stays armed while
    /// display-off and suspend are disarmed, so the session secures itself
    /// without ever blanking.
    Secure,
}

impl PowerMode {
    pub const ALL: [PowerMode; 3] = [PowerMode::Balanced, PowerMode::Awake, PowerMode::Secure];

    pub fn as_str(self) -> &'static str {
        match self {
            PowerMode::Balanced => "balanced",
            PowerMode::Awake => "awake",
            PowerMode::Secure => "secure",
        }
    }

    /// Parse the wire/config name. Unknown names are rejected so a typo can
    /// never silently select a weaker mode.
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "balanced" => PowerMode::Balanced,
            "awake" => PowerMode::Awake,
            "secure" => PowerMode::Secure,
            _ => return None,
        })
    }

    /// The stages this mode keeps armed. Order follows the pipeline; the
    /// daemon arms one notification per returned stage.
    pub fn armed_stages(self) -> &'static [IdleStageSelector] {
        match self {
            PowerMode::Balanced => &[
                IdleStageSelector::Dim,
                IdleStageSelector::Lock,
                IdleStageSelector::DisplayOff,
                IdleStageSelector::Suspend,
            ],
            PowerMode::Awake => &[IdleStageSelector::Dim],
            PowerMode::Secure => &[IdleStageSelector::Dim, IdleStageSelector::Lock],
        }
    }

    /// Whether this mode still arms the automatic lock stage. This is the
    /// legacy `idle_inhibited` semantics: modes that disarm the lock stage
    /// read as "idle inhibited" to chrome built against the old single-bit
    /// status.
    pub fn locks_automatically(self) -> bool {
        self.armed_stages().contains(&IdleStageSelector::Lock)
    }

    /// Whether the display can power off in this mode.
    pub fn blanks_display(self) -> bool {
        self.armed_stages().contains(&IdleStageSelector::DisplayOff)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn mode_names_round_trip() {
        for mode in PowerMode::ALL {
            assert_eq!(PowerMode::from_name(mode.as_str()), Some(mode));
        }
        assert_eq!(PowerMode::from_name("cinematic"), None);
    }

    #[test]
    fn every_mode_arms_an_ordered_unique_stage_set() {
        for mode in PowerMode::ALL {
            let stages = mode.armed_stages();
            assert!(!stages.is_empty(), "{mode:?} arms nothing");
            let unique: HashSet<_> = stages.iter().collect();
            assert_eq!(unique.len(), stages.len(), "{mode:?} repeats a stage");
            // Dim is always first when armed: dimming never follows a
            // stronger stage in the pipeline order.
            if let Some(first) = stages.first() {
                assert!(matches!(first, IdleStageSelector::Dim));
            }
        }
    }

    #[test]
    fn power_stages_never_outrun_the_lock_stage() {
        // The pipeline invariant (power stages only behind a confirmed lock)
        // is preserved mode-side: no mode arms display-off or suspend without
        // also arming lock.
        for mode in PowerMode::ALL {
            let stages = mode.armed_stages();
            let has_power = stages.contains(&IdleStageSelector::DisplayOff)
                || stages.contains(&IdleStageSelector::Suspend);
            let has_lock = stages.contains(&IdleStageSelector::Lock);
            assert!(
                !has_power || has_lock,
                "{mode:?} arms power stages without the lock stage"
            );
        }
    }

    #[test]
    fn legacy_idle_inhibited_mirror_matches_lock_disarm() {
        assert!(PowerMode::Balanced.locks_automatically());
        assert!(!PowerMode::Awake.locks_automatically());
        assert!(PowerMode::Secure.locks_automatically());
    }
}
