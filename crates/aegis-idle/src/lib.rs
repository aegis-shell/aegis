//! Pure staged-idle policy used by the daemon and tests.

#![forbid(unsafe_code)]

use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IdlePolicy {
    pub dim_after: Option<Duration>,
    pub lock_after: Option<Duration>,
    pub display_off_after: Option<Duration>,
    pub suspend_after: Option<Duration>,
    pub dim_percent: u8,
}

impl Default for IdlePolicy {
    fn default() -> Self {
        Self {
            dim_after: Some(Duration::from_secs(5 * 60)),
            lock_after: Some(Duration::from_secs(10 * 60)),
            display_off_after: Some(Duration::from_secs(11 * 60)),
            suspend_after: Some(Duration::from_secs(30 * 60)),
            dim_percent: 30,
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
}
