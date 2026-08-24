//! Motion curves, stagger choreography, and easing primitives for Aegis chrome.
//!
//! This module is the shared motion vocabulary every chrome component draws
//! from (ADR-0139): springs, decays, easing curves, and the reduced-motion
//! rule. It is pure math on caller-owned state — mechanism without a
//! timeline. A component decides *what* moves and *when* (policy); these
//! functions say *how a scalar travels between two values* (mechanism).
//! Nothing here schedules frames, owns a clock, or draws: components keep
//! driving `dt` from the frame input and retain their own animation state.
//!
//! # Reduced motion
//!
//! The ADR-0029 rule is one global switch, not a per-effect one: when
//! reduced motion is on, every animation resolves to its end state in at
//! most one frame. The helpers here make that a one-line concern — call
//! [`Spring::snap_to`] or [`decay::toward_zero`] with a `reduced_motion`
//! flag instead of writing a third local variant of the same rule.

/// One frame's delta time clamped to the range an animation may integrate
/// over. A long frame stall is absorbed across subsequent frames instead of
/// producing a teleport or a divergence; a zero delta (a duplicated frame)
/// integrates nothing.
#[inline]
pub fn frame_dt(dt_seconds: f32) -> f32 {
    dt_seconds.clamp(0.0, 1.0 / 30.0)
}

/// Linear interpolation between `a` and `b` by factor `t` in `[0.0, 1.0]`.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// Standard cubic ease-out curve ($f(t) = 1 - (1 - t)^3$).
///
/// Produces a smooth decelerating motion profile aligned with the Liquid Glass
/// physical spring response. Clamps input `t` to `[0.0, 1.0]`.
#[inline]
pub fn ease_out_cubic(value: f32) -> f32 {
    let inverse = 1.0 - value.clamp(0.0, 1.0);
    1.0 - inverse * inverse * inverse
}

/// Cubic ease-in curve ($f(t) = t^3$): slow start, accelerating exit.
#[inline]
pub fn ease_in_cubic(value: f32) -> f32 {
    let t = value.clamp(0.0, 1.0);
    t * t * t
}

/// Hermite smoothstep ($f(t) = t^2(3 - 2t)$): zero velocity at both ends.
///
/// This is the shape a dock-family morph applies to a reveal spring's
/// output — the spring decides *when* the value travels, smoothstep gives
/// the visible geometry a soft start and a soft landing so the surface
/// never arrives at either end of the morph with a hard stop.
#[inline]
pub fn smoothstep(value: f32) -> f32 {
    let t = value.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Calculate a staggered reveal progress: returns 0.0 until `reveal` exceeds `delay`,
/// then linearly interpolates to 1.0 as `reveal` reaches 1.0.
#[inline]
pub fn stagger(reveal: f32, delay: f32) -> f32 {
    if delay >= 1.0 {
        return if reveal >= 1.0 { 1.0 } else { 0.0 };
    }
    ((reveal - delay) / (1.0 - delay)).clamp(0.0, 1.0)
}

/// A damped harmonic oscillator state for one animated scalar.
///
/// Integrated from the closed-form analytic solution, so it is stable across
/// the whole accepted frame-interval range (`dt` is clamped by [`frame_dt`])
/// and reproduces the macOS-style slight overshoot exactly. The
/// semi-implicit Euler springs the chrome used before ADR-0139 could
/// diverge on a long frame stall; this form cannot (see
/// `spring_is_dt_stable`).
///
/// The state is intentionally `Copy + Default`: components hold one per
/// animated scalar (a tile edge length, a reveal progress) and advance it
/// inside their render pass.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Spring {
    /// Current eased value in caller units (logical px, 0..=1 progress, …).
    pub value: f32,
    /// Current velocity in value units per second.
    pub velocity: f32,
}

impl Spring {
    /// A spring at `value` with zero velocity.
    #[inline]
    pub fn at(value: f32) -> Spring {
        Spring {
            value,
            velocity: 0.0,
        }
    }

    /// True when the spring rests on `target` within the given tolerances
    /// and can be dropped from the frame-cadence decision. Pixel-scaled
    /// scalars want looser epsilons than progress-like scalars.
    #[inline]
    pub fn settled_on(&self, target: f32, value_epsilon: f32, velocity_epsilon: f32) -> bool {
        (self.value - target).abs() <= value_epsilon && self.velocity.abs() <= velocity_epsilon
    }

    /// Advance toward `target` by `dt_seconds` (clamped via [`frame_dt`])
    /// and return the new value.
    ///
    /// `stiffness` is ω₀² (rad/s)² — larger shortens the period (snappier).
    /// `damping` is the damping ratio ζ: 1.0 is critically damped, values
    /// just under 1.0 give the slight bounce-back that reads as physical.
    /// ζ ≥ 1 falls back to the smooth critically-damped response.
    pub fn advance(&mut self, target: f32, stiffness: f32, damping: f32, dt_seconds: f32) -> f32 {
        let dt = frame_dt(dt_seconds);
        if dt <= 0.0 {
            return self.value;
        }
        let omega0 = stiffness.max(0.0).sqrt();
        if omega0 <= 0.0 {
            return self.snap_to(target);
        }
        let zeta = damping.clamp(0.0, 1.0);
        let displacement = self.value - target;

        if zeta < 1.0 {
            // Under-damped: damped oscillation, the analytic solution the
            // dock magnification wave used since ADR-0019.
            let decay_rate = zeta * omega0;
            let omega_d = omega0 * (1.0 - zeta * zeta).sqrt();
            let decay = (-decay_rate * dt).exp();
            let sin = (omega_d * dt).sin();
            let cos = (omega_d * dt).cos();
            let velocity_term = (self.velocity + decay_rate * displacement) / omega_d;
            let value = target + decay * (displacement * cos + velocity_term * sin);
            self.velocity = decay
                * (self.velocity * cos
                    - (decay_rate * self.velocity + omega0 * omega0 * displacement) / omega_d
                        * sin);
            self.value = value;
        } else {
            // Critically damped (ζ ≥ 1 clamped): smooth approach, no
            // overshoot.
            let decay = (-omega0 * dt).exp();
            let velocity_term = self.velocity + omega0 * displacement;
            let value = target + decay * (displacement + velocity_term * dt);
            self.velocity = decay * (self.velocity - omega0 * velocity_term * dt);
            self.value = value;
        }
        self.value
    }

    /// ADR-0029 reduced-motion rule: resolve to the end state in one frame.
    ///
    /// Snaps value and velocity to the target and returns it. Choosing this
    /// over [`Spring::advance`] is the component's entire reduced-motion
    /// contract.
    #[inline]
    pub fn snap_to(&mut self, target: f32) -> f32 {
        self.value = target;
        self.velocity = 0.0;
        target
    }
}

/// Frame-rate-independent exponential approach of `current` toward
/// `target` at `rate` (a per-second decay constant; higher is faster).
/// A zero or negative `dt` returns `current` unchanged.
#[inline]
pub fn approach(current: f32, target: f32, rate: f32, dt_seconds: f32) -> f32 {
    let dt = dt_seconds.max(0.0);
    if dt <= 0.0 {
        return current;
    }
    lerp(current, target, 1.0 - (-rate * dt).exp())
}

/// Exponential-decay helpers for scalars that relax toward zero (a page
/// slide offset, a tooltip alpha).
pub mod decay {
    /// Frame-rate-independent exponential decay of `value` toward zero at
    /// `rate` per second. A zero or negative `dt` returns `value`
    /// unchanged.
    #[inline]
    pub fn toward_zero(value: f32, rate: f32, dt_seconds: f32) -> f32 {
        let dt = dt_seconds.max(0.0);
        if dt <= 0.0 {
            return value;
        }
        value * (-rate * dt).exp()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ease_out_cubic() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        assert!((ease_out_cubic(0.5) - 0.875).abs() < 1e-6);
        // Clamping checks
        assert_eq!(ease_out_cubic(-0.5), 0.0);
        assert_eq!(ease_out_cubic(1.5), 1.0);
    }

    #[test]
    fn test_ease_in_cubic() {
        assert_eq!(ease_in_cubic(0.0), 0.0);
        assert_eq!(ease_in_cubic(1.0), 1.0);
        assert!((ease_in_cubic(0.5) - 0.125).abs() < 1e-6);
        assert_eq!(ease_in_cubic(-1.0), 0.0);
        assert_eq!(ease_in_cubic(2.0), 1.0);
    }

    #[test]
    fn test_smoothstep() {
        assert_eq!(smoothstep(0.0), 0.0);
        assert_eq!(smoothstep(1.0), 1.0);
        assert!((smoothstep(0.5) - 0.5).abs() < 1e-6);
        // Symmetric about the midpoint.
        assert!((smoothstep(0.2) + smoothstep(0.8) - 1.0).abs() < 1e-6);
        // Clamping and monotonicity.
        assert_eq!(smoothstep(-0.5), 0.0);
        assert_eq!(smoothstep(1.5), 1.0);
        assert!(smoothstep(0.3) < smoothstep(0.4));
    }

    #[test]
    fn test_stagger() {
        assert_eq!(stagger(0.0, 0.2), 0.0);
        assert_eq!(stagger(0.2, 0.2), 0.0);
        assert!((stagger(0.6, 0.2) - 0.5).abs() < 1e-6);
        assert_eq!(stagger(1.0, 0.2), 1.0);
    }

    #[test]
    fn test_lerp() {
        assert_eq!(lerp(10.0, 20.0, 0.0), 10.0);
        assert_eq!(lerp(10.0, 20.0, 0.5), 15.0);
        assert_eq!(lerp(10.0, 20.0, 1.0), 20.0);
        assert_eq!(lerp(10.0, 20.0, -1.0), 10.0);
        assert_eq!(lerp(10.0, 20.0, 2.0), 20.0);
    }

    #[test]
    fn test_frame_dt_clamps() {
        assert_eq!(frame_dt(0.0), 0.0);
        assert_eq!(frame_dt(-1.0), 0.0);
        assert!((frame_dt(1.0 / 60.0) - 1.0 / 60.0).abs() < 1e-9);
        assert_eq!(frame_dt(1.0), 1.0 / 30.0);
    }

    #[test]
    fn test_approach() {
        // Zero dt is a no-op.
        assert_eq!(approach(10.0, 20.0, 12.0, 0.0), 10.0);
        assert_eq!(approach(10.0, 20.0, 12.0, -1.0), 10.0);
        // Moves toward the target, never past it.
        let half = approach(0.0, 100.0, 12.0, 1.0 / 12.0);
        assert!((half - 63.212).abs() < 0.01, "half = {half}");
        // Converges over many frames.
        let mut value = 0.0;
        for _ in 0..600 {
            value = approach(value, 100.0, 12.0, 1.0 / 60.0);
        }
        assert!((value - 100.0).abs() < 0.01, "value = {value}");
    }

    #[test]
    fn test_decay_toward_zero() {
        assert_eq!(decay::toward_zero(30.0, 18.0, 0.0), 30.0);
        assert_eq!(decay::toward_zero(30.0, 18.0, -1.0), 30.0);
        let decayed = decay::toward_zero(30.0, 18.0, 1.0 / 18.0);
        assert!(
            (decayed - 30.0 / std::f32::consts::E).abs() < 0.001,
            "{decayed}"
        );
        let mut value = 30.0;
        for _ in 0..600 {
            value = decay::toward_zero(value, 18.0, 1.0 / 60.0);
        }
        assert!(value.abs() < 0.01, "value = {value}");
    }

    #[test]
    fn spring_no_time_elapses_nothing_moves() {
        let mut spring = Spring::at(10.0);
        assert_eq!(spring.advance(20.0, 900.0, 0.85, 0.0), 10.0);
        assert_eq!(spring.velocity, 0.0);
    }

    #[test]
    fn spring_settles_on_target() {
        let mut spring = Spring::at(10.0);
        for _ in 0..2000 {
            spring.advance(20.0, 900.0, 0.85, 1.0 / 120.0);
        }
        assert!(
            (spring.value - 20.0).abs() < 0.01,
            "settled at {}",
            spring.value
        );
        assert!(spring.settled_on(20.0, 0.15, 0.5));
    }

    #[test]
    fn spring_overshoots_then_settles() {
        // Under-damped from rest: crosses the target at least once before
        // settling (the macOS lift-and-bounce).
        let mut spring = Spring::at(0.0);
        let mut overshot = false;
        for _ in 0..2000 {
            spring.advance(100.0, 900.0, 0.85, 1.0 / 120.0);
            if spring.value > 100.0 {
                overshot = true;
            }
        }
        assert!(overshot, "spring never overshot the target");
        assert!((spring.value - 100.0).abs() < 0.01);
    }

    #[test]
    fn spring_critical_damping_has_no_overshoot() {
        let mut spring = Spring::at(0.0);
        let mut overshot = false;
        for _ in 0..2000 {
            spring.advance(100.0, 360.0, 1.0, 1.0 / 120.0);
            if spring.value > 100.0 + 1e-3 {
                overshot = true;
            }
        }
        assert!(!overshot, "critically damped spring overshot");
        assert!((spring.value - 100.0).abs() < 0.01);
    }

    #[test]
    fn spring_is_dt_stable() {
        // A single large step (a long frame stall) must not blow up.
        let mut spring = Spring::at(0.0);
        let value = spring.advance(100.0, 900.0, 0.85, 1.0 / 5.0);
        assert!(value.is_finite(), "value diverged: {value}");
        assert!(
            spring.velocity.is_finite(),
            "velocity diverged: {}",
            spring.velocity
        );
        // dt is clamped, so the value also stays inside a plausible band.
        assert!(value > 0.0 && value < 200.0, "value escaped range: {value}");
    }

    #[test]
    fn spring_remains_bounded_and_settles_at_thirty_fps() {
        let mut spring = Spring::at(56.0);
        for _ in 0..300 {
            spring.advance(84.0, 900.0, 0.85, 1.0 / 30.0);
            assert!(
                spring.value >= 0.0 && spring.value <= 84.0 * 2.0,
                "spring escaped its visual range: {}",
                spring.value
            );
        }
        assert!((spring.value - 84.0).abs() < 0.01);
        assert!(spring.velocity.abs() < 0.01);
    }

    #[test]
    fn spring_snap_to_resolves_in_one_frame() {
        let mut spring = Spring::at(0.3);
        assert_eq!(spring.snap_to(1.0), 1.0);
        assert_eq!(spring.value, 1.0);
        assert_eq!(spring.velocity, 0.0);
        assert!(spring.settled_on(1.0, 0.002, 0.02));
    }
}
