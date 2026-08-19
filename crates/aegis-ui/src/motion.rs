//! Motion curves, stagger choreography, and easing primitives for Aegis chrome.

/// Standard cubic ease-out curve ($f(t) = 1 - (1 - t)^3$).
///
/// Produces a smooth decelerating motion profile aligned with the Liquid Glass
/// physical spring response. Clamps input `t` to `[0.0, 1.0]`.
#[inline]
pub fn ease_out_cubic(value: f32) -> f32 {
    let inverse = 1.0 - value.clamp(0.0, 1.0);
    1.0 - inverse * inverse * inverse
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

/// Linear interpolation between `a` and `b` by factor `t` in `[0.0, 1.0]`.
#[inline]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
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
    }
}
