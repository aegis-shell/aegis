//! Declarative window transitions (ADR-0029).
//!
//! When the window manager changes a toplevel's rectangle non-interactively
//! (tiling, maximize, snap, IPC geometry), it records the previous and
//! target rectangles and a start time. The model always reports the target;
//! the server interpolates the rectangle for rendering only, so a transition
//! never mutates the state the chrome, the IPC, or the agent read. The
//! reduced-motion policy resolves every transition in at most one frame.

use crate::{Point, Rect, Size};

/// Default transition length. Short enough to feel instant, long enough to
/// read as motion.
pub const DEFAULT_DURATION_MS: u64 = 180;

/// One in-flight geometry transition. `to` is always the window's model
/// rect, so it is not stored here; [`WindowTransition::rect_at`] takes it as
/// an argument.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WindowTransition {
    /// The rect the window is animating away from.
    pub from: Rect,
    /// Compositor-relative millisecond timestamp the transition started at.
    pub started_ms: u64,
    /// How long the transition runs, in milliseconds.
    pub duration_ms: u64,
}

impl WindowTransition {
    /// Start a transition from `from` at `now_ms` with the default duration.
    pub fn new(from: Rect, now_ms: u64) -> WindowTransition {
        WindowTransition {
            from,
            started_ms: now_ms,
            duration_ms: DEFAULT_DURATION_MS,
        }
    }

    /// Whether this transition is still in flight at `now_ms`.
    ///
    /// Keep the lifetime predicate independent of the current target rect so
    /// every consumer (scene visibility, occlusion, callbacks, and rendering)
    /// agrees on the exact instant a transition settles.
    pub fn is_active_at(&self, now_ms: u64) -> bool {
        self.duration_ms > 0 && now_ms.saturating_sub(self.started_ms) < self.duration_ms
    }

    /// The interpolated rect at `now_ms` (ease-out cubic), or `None` when
    /// the transition has settled and the window renders at `target`.
    pub fn rect_at(&self, target: Rect, now_ms: u64) -> Option<Rect> {
        if !self.is_active_at(now_ms) {
            return None;
        }
        let elapsed = now_ms.saturating_sub(self.started_ms);
        let t = ease_out_cubic(elapsed as f32 / self.duration_ms as f32);
        Some(lerp_rect(self.from, target, t))
    }
}

/// `1 - (1 - t)³`: fast start, gentle settle — the standard compositor curve.
pub fn ease_out_cubic(t: f32) -> f32 {
    let u = (1.0 - t.clamp(0.0, 1.0)).powi(3);
    1.0 - u
}

/// Interpolate two rects component-wise, rounding to the nearest pixel.
pub fn lerp_rect(from: Rect, to: Rect, t: f32) -> Rect {
    let lerp = |a: i32, b: i32| (a as f32 + (b - a) as f32 * t).round() as i32;
    Rect {
        origin: Point {
            x: lerp(from.origin.x, to.origin.x),
            y: lerp(from.origin.y, to.origin.y),
        },
        size: Size {
            w: lerp(from.size.w, to.size.w).max(1),
            h: lerp(from.size.h, to.size.h).max(1),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ease_out_cubic_endpoints_and_shape() {
        assert_eq!(ease_out_cubic(0.0), 0.0);
        assert_eq!(ease_out_cubic(1.0), 1.0);
        // Ease-out: ahead of linear early, catching up late.
        assert!(ease_out_cubic(0.25) > 0.25);
        assert!(ease_out_cubic(0.75) < 0.875 + 0.125);
        assert!(ease_out_cubic(0.75) > 0.75 - 0.25);
        // Monotonic.
        assert!(ease_out_cubic(0.3) < ease_out_cubic(0.6));
    }

    #[test]
    fn rect_at_interpolates_then_settles() {
        let from = Rect::new(0, 0, 100, 100);
        let target = Rect::new(100, 50, 200, 150);
        let tr = WindowTransition::new(from, 1000);

        // t=0 is still in flight, exactly at `from`.
        assert_eq!(tr.rect_at(target, 1000), Some(from));

        let mid = tr.rect_at(target, 1075).expect("mid-flight");
        assert!(mid.origin.x > 0 && mid.origin.x < 100, "mid x: {mid:?}");
        assert!(mid.size.w > 100 && mid.size.w < 200, "mid w: {mid:?}");

        assert!(tr.rect_at(target, 1180).is_none(), "settled at duration");
        assert!(tr.rect_at(target, 5000).is_none(), "settled long after");
        // A zero-duration transition resolves immediately (reduced motion).
        let instant = WindowTransition {
            duration_ms: 0,
            ..tr
        };
        assert!(instant.rect_at(target, 1000).is_none());
    }

    #[test]
    fn lerp_rect_hits_endpoints() {
        let from = Rect::new(10, 20, 100, 100);
        let to = Rect::new(110, 120, 300, 200);
        assert_eq!(lerp_rect(from, to, 0.0), from);
        assert_eq!(lerp_rect(from, to, 1.0), to);
        let mid = lerp_rect(from, to, 0.5);
        assert_eq!(mid.origin, Point { x: 60, y: 70 });
        assert_eq!(mid.size, Size { w: 200, h: 150 });
    }
}
