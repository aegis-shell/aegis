//! Geometry, hit-testing, and layout utility primitives.

use lens::{Align, LayoutOpts, Rect};

/// Return true if the rectangle contains the point `(x, y)`.
#[inline]
pub fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && x < rect.x + rect.w && y >= rect.y && y < rect.y + rect.h
}

/// A zero-padding, centered layout stretching to the rectangle's dimensions.
#[inline]
pub fn stretch(rect: Rect) -> LayoutOpts {
    LayoutOpts {
        width: rect.w,
        height: rect.h,
        pad: 0.0,
        gap: 0.0,
        cross: Align::Center,
        ..Default::default()
    }
}

/// A zero-padding layout stretching horizontally and aligned to the start/top.
#[inline]
pub fn stretch_top(rect: Rect) -> LayoutOpts {
    LayoutOpts {
        width: rect.w,
        height: rect.h,
        pad: 0.0,
        gap: 0.0,
        cross: Align::Start,
        ..Default::default()
    }
}

/// A centered layout stretching to the rectangle with explicit padding.
#[inline]
pub fn stretch_pad(rect: Rect, pad: f32) -> LayoutOpts {
    LayoutOpts {
        width: rect.w,
        height: rect.h,
        pad,
        gap: 0.0,
        cross: Align::Center,
        ..Default::default()
    }
}

/// A centered layout stretching to the rectangle with explicit item gap.
#[inline]
pub fn stretch_gap(rect: Rect, gap: f32) -> LayoutOpts {
    LayoutOpts {
        width: rect.w,
        height: rect.h,
        pad: 0.0,
        gap,
        cross: Align::Center,
        ..Default::default()
    }
}

/// Center an inner rectangle of size `(w, h)` within a display of size `(display.0, display.1)`
/// accounting for edge insets (such as reserved bars/docks).
pub fn center_rect(
    display: (f32, f32),
    inset_left: f32,
    inset_top: f32,
    inset_right: f32,
    inset_bottom: f32,
    target_w: f32,
    target_h: f32,
) -> Rect {
    let left = inset_left.max(0.0);
    let top = inset_top.max(0.0);
    let usable_w = (display.0 - left - inset_right.max(0.0)).max(1.0);
    let usable_h = (display.1 - top - inset_bottom.max(0.0)).max(1.0);

    let panel_w = target_w.min((usable_w - 32.0).max(240.0));
    let panel_h = target_h.min((usable_h - 32.0).max(120.0));

    Rect {
        x: left + ((usable_w - panel_w) * 0.5).max(0.0),
        y: top + ((usable_h - panel_h) * 0.5).max(0.0),
        w: panel_w,
        h: panel_h,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_contains() {
        let r = Rect {
            x: 10.0,
            y: 20.0,
            w: 100.0,
            h: 50.0,
        };
        assert!(contains(r, 10.0, 20.0));
        assert!(contains(r, 50.0, 45.0));
        assert!(contains(r, 109.9, 69.9));
        assert!(!contains(r, 9.9, 20.0));
        assert!(!contains(r, 110.0, 20.0));
        assert!(!contains(r, 50.0, 70.0));
    }

    #[test]
    fn test_center_rect() {
        let rect = center_rect((1920.0, 1080.0), 0.0, 32.0, 0.0, 48.0, 460.0, 200.0);
        assert_eq!(rect.w, 460.0);
        assert_eq!(rect.h, 200.0);
        assert_eq!(rect.x, (1920.0 - 460.0) * 0.5);
        assert_eq!(rect.y, 32.0 + (1080.0 - 32.0 - 48.0 - 200.0) * 0.5);
    }
}
