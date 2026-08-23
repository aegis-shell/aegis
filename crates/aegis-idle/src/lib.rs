//! Pure staged-idle policy used by the daemon and tests.

#![forbid(unsafe_code)]

use aegis_model::power::{IdleStageSelector, PowerMode};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdlePolicy {
    pub dim_after: Option<Duration>,
    pub lock_after: Option<Duration>,
    pub display_off_after: Option<Duration>,
    pub suspend_after: Option<Duration>,
    pub dim_percent: u8,
    /// Session power mode (ADR-0140): which stages stay armed. The mode
    /// filters the armed set at [`IdlePolicy::stages`]; timeouts and the
    /// security validation remain the mode-free product policy.
    pub mode: PowerMode,
}

impl Default for IdlePolicy {
    fn default() -> Self {
        Self {
            dim_after: Some(Duration::from_secs(5 * 60)),
            lock_after: Some(Duration::from_secs(10 * 60)),
            display_off_after: Some(Duration::from_secs(11 * 60)),
            suspend_after: Some(Duration::from_secs(30 * 60)),
            dim_percent: 30,
            mode: PowerMode::Balanced,
        }
    }
}

impl IdlePolicy {
    pub fn validate(self) -> Result<Self, &'static str> {
        if !(1..=100).contains(&self.dim_percent) {
            return Err("idle dim percentage must be inside 1..=100");
        }
        let maximum = Duration::from_secs(u64::from(
            aegis_model::settings::IdleSettings::MAX_TIMEOUT_SECONDS,
        ));
        if [
            self.dim_after,
            self.lock_after,
            self.display_off_after,
            self.suspend_after,
        ]
        .into_iter()
        .flatten()
        .any(|timeout| timeout > maximum)
        {
            return Err("idle timeout is longer than seven days");
        }
        if (self.display_off_after.is_some() || self.suspend_after.is_some())
            && self.lock_after.is_none()
        {
            return Err("locking must be enabled before output power-off or suspend");
        }
        let stages = [
            self.dim_after,
            self.lock_after,
            self.display_off_after,
            self.suspend_after,
        ];
        let ordered = stages
            .into_iter()
            .flatten()
            .try_fold(Duration::ZERO, |previous, current| {
                (current > previous).then_some(current)
            })
            .is_some();
        if ordered {
            Ok(self)
        } else {
            Err("enabled idle stages must be strictly increasing")
        }
    }

    pub fn stages(self) -> impl Iterator<Item = (IdleStage, Duration)> {
        self.stages_unfiltered()
            .filter(|(stage, _)| self.mode_armed(*stage))
            .collect::<Vec<_>>()
            .into_iter()
    }

    /// Every stage whose product timeout is enabled, ignoring the session
    /// power mode. Validation and re-arm decisions use this set so a mode can
    /// never weaken the security ordering of the underlying policy.
    pub fn stages_unfiltered(self) -> impl Iterator<Item = (IdleStage, Duration)> {
        [
            self.dim_after.map(|timeout| (IdleStage::Dim, timeout)),
            self.lock_after.map(|timeout| (IdleStage::Lock, timeout)),
            self.display_off_after
                .map(|timeout| (IdleStage::DisplayOff, timeout)),
            self.suspend_after
                .map(|timeout| (IdleStage::Suspend, timeout)),
        ]
        .into_iter()
        .flatten()
    }

    /// Whether the session power mode keeps `stage` armed (ADR-0140). The
    /// product timeouts are unchanged: a disarmed stage simply never arms a
    /// notification, so its timer never starts.
    fn mode_armed(self, stage: IdleStage) -> bool {
        let selector = match stage {
            IdleStage::Dim => IdleStageSelector::Dim,
            IdleStage::Lock => IdleStageSelector::Lock,
            IdleStage::DisplayOff => IdleStageSelector::DisplayOff,
            IdleStage::Suspend => IdleStageSelector::Suspend,
        };
        self.mode.armed_stages().contains(&selector)
    }

    /// The stages this policy arms, as a count for diagnostics.
    pub fn armed_stage_count(self) -> usize {
        self.stages().count()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum IdleStage {
    Dim,
    Lock,
    DisplayOff,
    Suspend,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_are_security_ordered() {
        IdlePolicy::default().validate().unwrap();
    }

    #[test]
    fn refuses_power_off_before_lock() {
        let policy = IdlePolicy {
            lock_after: Some(Duration::from_secs(60)),
            display_off_after: Some(Duration::from_secs(30)),
            ..IdlePolicy::default()
        };
        assert!(policy.validate().is_err());
    }

    #[test]
    fn refuses_power_stages_without_a_lock_boundary() {
        let policy = IdlePolicy {
            lock_after: None,
            ..IdlePolicy::default()
        };
        assert!(policy.validate().is_err());
    }

    #[test]
    fn refuses_timeouts_outside_the_shared_product_policy() {
        let policy = IdlePolicy {
            suspend_after: Some(Duration::from_secs(
                u64::from(aegis_model::settings::IdleSettings::MAX_TIMEOUT_SECONDS) + 1,
            )),
            ..IdlePolicy::default()
        };
        assert!(policy.validate().is_err());
    }

    #[test]
    fn awake_mode_disarms_every_security_stage_but_dim() {
        let policy = IdlePolicy {
            mode: aegis_model::power::PowerMode::Awake,
            ..IdlePolicy::default()
        };
        let stages: Vec<_> = policy.stages().map(|(stage, _)| stage).collect();
        assert_eq!(stages, vec![IdleStage::Dim]);
        // The underlying policy still validates: a mode filters, it never
        // rewrites the product timeouts or their ordering.
        assert!(policy.validate().is_ok());
        assert_eq!(policy.stages_unfiltered().count(), 4);
    }

    #[test]
    fn secure_mode_arms_lock_and_dim_only() {
        let policy = IdlePolicy {
            mode: aegis_model::power::PowerMode::Secure,
            ..IdlePolicy::default()
        };
        let stages: Vec<_> = policy.stages().map(|(stage, _)| stage).collect();
        assert_eq!(stages, vec![IdleStage::Dim, IdleStage::Lock]);
    }

    #[test]
    fn modes_cannot_disable_dim_when_it_is_the_only_armed_stage() {
        // Awake keeps Dim armed: the display always has *some* idle
        // response, so a forgotten mode cannot burn a static image in.
        for mode in aegis_model::power::PowerMode::ALL {
            let policy = IdlePolicy {
                mode,
                ..IdlePolicy::default()
            };
            assert!(policy.stages().count() >= 1, "{mode:?} arms nothing");
        }
    }
}
