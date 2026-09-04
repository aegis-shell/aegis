//! Night light: color-temperature scheduling and gamma math.
//!
//! Pure model — no backend, no clock I/O. The runtime evaluates
//! [`NightLight::step`] on a ~1 Hz cadence and hands the resulting channel
//! gains to the backend's CRTC `GAMMA_LUT` programming.

/// Neutral daylight temperature: no tint, night light effectively off.
pub const NEUTRAL_KELVIN: f32 = 6500.0;

/// Clamp accepted for configured temperatures.
pub const MIN_KELVIN: f32 = 1000.0;
pub const MAX_KELVIN: f32 = 10000.0;

/// White-point channel gains for a color temperature in Kelvin, using the
/// Tanner Helland approximation (the same curve wlsunset and redshift
/// use). Returns linear RGB multipliers in 0..=1; 6500K is neutral.
pub fn temperature_to_gains(kelvin: f32) -> [f32; 3] {
    let t = (kelvin.clamp(MIN_KELVIN, MAX_KELVIN) / 100.0).max(f32::EPSILON);
    let red = if t <= 66.0 {
        1.0
    } else {
        (329.698_73_f32 * (t - 60.0).powf(-0.133_204_74)) / 255.0
    };
    let green = if t <= 66.0 {
        (99.470_8_f32 * t.ln() - 161.119_57) / 255.0
    } else {
        (288.122_16_f32 * (t - 60.0).powf(-0.075_514_846)) / 255.0
    };
    let blue = if t >= 66.0 {
        1.0
    } else if t <= 19.0 {
        0.0
    } else {
        (138.517_73_f32 * (t - 10.0).ln() - 305.044_8) / 255.0
    };
    [
        red.clamp(0.0, 1.0),
        green.clamp(0.0, 1.0),
        blue.clamp(0.0, 1.0),
    ]
}

/// A wall-clock time of day in minutes since midnight.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClockTime {
    pub minutes: u32,
}

impl ClockTime {
    /// Parse `"HH:MM"` (24-hour). Rejects out-of-range fields and garbage.
    pub fn from_hhmm(s: &str) -> Option<ClockTime> {
        let (hours, minutes) = s.split_once(':')?;
        if hours.len() != 2 || minutes.len() != 2 {
            return None;
        }
        let hours: u32 = hours.parse().ok()?;
        let minutes: u32 = minutes.parse().ok()?;
        if hours >= 24 || minutes >= 60 {
            return None;
        }
        Some(ClockTime {
            minutes: hours * 60 + minutes,
        })
    }
}

/// Whether `now` falls inside the [`start`, `end`) window. Windows that
/// cross midnight (`start > end`) match late evening and early morning.
/// An empty window (`start == end`) means "always active".
pub fn schedule_active(start: ClockTime, end: ClockTime, now: ClockTime) -> bool {
    if start == end {
        return true;
    }
    if start.minutes < end.minutes {
        (start.minutes..end.minutes).contains(&now.minutes)
    } else {
        now.minutes >= start.minutes || now.minutes < end.minutes
    }
}

/// The live night-light state: the currently applied temperature and its
/// drift toward the target. Temperature (not gains) interpolates so the
/// fade reads linearly to the eye.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct NightLight {
    /// The temperature currently programmed on the outputs.
    pub current_kelvin: f32,
    /// Whether the outputs carry a night-light gamma table right now.
    pub active: bool,
}

impl Default for NightLight {
    fn default() -> NightLight {
        NightLight {
            current_kelvin: NEUTRAL_KELVIN,
            active: false,
        }
    }
}

impl NightLight {
    /// One evaluation step. `target` is `Some(kelvin)` when night light
    /// should be on, `None` when off; `step_kelvin` is the fade rate per
    /// call (the caller paces calls at 1 Hz with `range / fade_seconds`).
    ///
    /// Returns the gains to program when the applied state changed, or
    /// `None` when nothing needs reprogramming. A `Some([1.0, 1.0, 1.0])`
    /// result signals "restore the neutral table" (the backend clears its
    /// gamma LUT); it is only produced on the final approach to neutral.
    pub fn step(&mut self, target: Option<f32>, step_kelvin: f32) -> Option<[f32; 3]> {
        let target_kelvin = target
            .unwrap_or(NEUTRAL_KELVIN)
            .clamp(MIN_KELVIN, NEUTRAL_KELVIN);
        let step = step_kelvin.max(1.0);
        let previous = self.current_kelvin;
        if (self.current_kelvin - target_kelvin).abs() <= step {
            self.current_kelvin = target_kelvin;
        } else if self.current_kelvin < target_kelvin {
            self.current_kelvin += step;
        } else {
            self.current_kelvin -= step;
        }
        let want_active = self.current_kelvin < NEUTRAL_KELVIN - 1.0;
        let changed = self.current_kelvin != previous || want_active != self.active;
        self.active = want_active;
        if !changed {
            return None;
        }
        Some(temperature_to_gains(self.current_kelvin))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_temperature_is_unity() {
        let [r, g, b] = temperature_to_gains(NEUTRAL_KELVIN);
        assert!(r == 1.0 && g > 0.99 && b > 0.98, "{r} {g} {b}");
    }

    #[test]
    fn warm_temperature_suppresses_blue_most() {
        let [r, g, b] = temperature_to_gains(3400.0);
        assert!(r > g && g > b, "{r} {g} {b}");
        assert!(b < 0.75, "blue should drop well below neutral: {b}");
    }

    #[test]
    fn temperature_clamps_to_documented_range() {
        assert_eq!(
            temperature_to_gains(200.0),
            temperature_to_gains(MIN_KELVIN)
        );
        assert_eq!(
            temperature_to_gains(50000.0),
            temperature_to_gains(MAX_KELVIN)
        );
    }

    #[test]
    fn clock_time_parses_hhmm_only() {
        assert_eq!(ClockTime::from_hhmm("19:30").unwrap().minutes, 19 * 60 + 30);
        assert!(ClockTime::from_hhmm("24:00").is_none());
        assert!(ClockTime::from_hhmm("9:00").is_none()); // strict HH:MM
        assert!(ClockTime::from_hhmm("19:60").is_none());
        assert!(ClockTime::from_hhmm("garbage").is_none());
    }

    #[test]
    fn schedule_windows_including_overnight() {
        let start = ClockTime::from_hhmm("19:00").unwrap();
        let end = ClockTime::from_hhmm("07:00").unwrap();
        assert!(schedule_active(
            start,
            end,
            ClockTime::from_hhmm("22:00").unwrap()
        ));
        assert!(schedule_active(
            start,
            end,
            ClockTime::from_hhmm("02:00").unwrap()
        ));
        assert!(!schedule_active(
            start,
            end,
            ClockTime::from_hhmm("12:00").unwrap()
        ));
        assert!(!schedule_active(
            start,
            end,
            ClockTime::from_hhmm("07:00").unwrap()
        ));
        // Same start/end = always on.
        assert!(schedule_active(
            start,
            start,
            ClockTime::from_hhmm("12:00").unwrap()
        ));
    }

    #[test]
    fn step_fades_to_target_then_still() {
        let mut nl = NightLight::default();
        let first = nl.step(Some(3400.0), 500.0).expect("starts fading");
        assert!(nl.active);
        assert_eq!(first, temperature_to_gains(6000.0));
        // Eventually reaches the target and stays put.
        let mut last = None;
        for _ in 0..16 {
            if let Some(gains) = nl.step(Some(3400.0), 500.0) {
                last = Some(gains);
            }
        }
        assert_eq!(last, Some(temperature_to_gains(3400.0)));
        assert!(
            nl.step(Some(3400.0), 500.0).is_none(),
            "steady state is quiet"
        );
    }

    #[test]
    fn step_fades_back_to_neutral_and_disengages() {
        let mut nl = NightLight {
            current_kelvin: 3400.0,
            active: true,
        };
        let mut cleared = false;
        for _ in 0..16 {
            if nl.step(None, 500.0).is_some() {
                cleared = !nl.active;
            }
        }
        assert!(cleared, "reaching neutral disengages the LUT");
        assert!((nl.current_kelvin - NEUTRAL_KELVIN).abs() < f32::EPSILON);
    }
}
