//! Window layout policies (ADR-0024).
//!
//! Floating is the universal base; an optional, policy-driven tiling layer
//! applies on top. A tiling policy is pure geometry: given a work area and a
//! count of tiled windows, it emits one [`Rect`] per window in order. The
//! window manager assigns rectangle *i* to the *i*-th tiled toplevel and
//! applies it as the window's position and size — exactly as an interactive
//! move does. There is no separate tiling-container type; a tiled window is
//! still a [`Window`](crate::window::Window) with a position and size.
//!
//! This module is pure: no flux, lens, or Wayland dependency, so a policy is
//! unit-tested in isolation. The server-side application (reconfigure clients
//! when the layout changes) is a follow-up.

use crate::{Rect, Size};

/// Whether a toplevel is laid out by the floating policy (free placement,
/// the default) or by a tiling policy. A tiled window still carries a
/// position and size like a floating one; the tiling policy simply sets them.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[cfg_attr(feature = "serde", serde(rename_all = "lowercase"))]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum LayoutRole {
    /// Free placement. The user (or the application) chooses position and
    /// size; the compositor does not override them.
    #[default]
    Floating,
    /// The active tiling policy computes position and size each time the set
    /// of tiled windows changes.
    Tiled,
}

/// Tunable parameters a [`Layout`] policy reads. Per-workspace in the full
/// design (ADR-0024); the pure policy just receives them.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LayoutParams {
    /// Gap in logical pixels, applied around the work-area edge and between
    /// adjacent tiles.
    pub gaps: i32,
    /// Fraction of the work-area width the master column takes (0.0..=1.0).
    /// Used by policies that distinguish a master (e.g. [`MasterStack`]).
    pub master_ratio: f32,
}

impl Default for LayoutParams {
    fn default() -> LayoutParams {
        LayoutParams {
            gaps: 8,
            master_ratio: 0.5,
        }
    }
}

/// A tiling policy. Given a work area and a count of tiled windows, emit one
/// rectangle per window in order. Pure geometry; the caller assigns
/// rectangle *i* to the *i*-th tiled toplevel.
pub trait Layout: Send + Sync {
    fn layout(&self, work_area: Rect, n_tiled: usize, params: &LayoutParams) -> Vec<Rect>;
}

/// Master-stack: the first window is the master, filling the left column at
/// `master_ratio` of the width; the rest share the right column as equal
/// horizontal rows. The canonical simple tiling policy (dwm/awesome/i3
/// master-stack lineage).
pub struct MasterStack;

impl Layout for MasterStack {
    fn layout(&self, work_area: Rect, n_tiled: usize, params: &LayoutParams) -> Vec<Rect> {
        if n_tiled == 0 {
            return Vec::new();
        }
        let inner = inset(work_area, params.gaps);
        if n_tiled == 1 {
            return vec![inner];
        }
        // Two columns separated by a gap. The master column takes
        // `master_ratio` of the width *minus* the separating gap.
        let avail = (inner.size.w - params.gaps).max(0) as f32;
        let master_w = (avail * params.master_ratio.clamp(0.0, 1.0)) as i32;
        let stack_x = inner.origin.x + master_w + params.gaps;
        let stack_w = (inner.size.w - master_w - params.gaps).max(0);
        let master = Rect {
            origin: inner.origin,
            size: Size {
                w: master_w,
                h: inner.size.h,
            },
        };
        // `n_tiled - 1` stack windows share the right column as equal rows
        // separated by gaps. With m = n_tiled - 1 rows there are m-1 internal
        // gaps, so each row is (height - (m-1)*gaps) / m.
        let m = (n_tiled - 1) as i32;
        let row_h = ((inner.size.h - (m - 1) * params.gaps).max(0)) / m;
        let mut out = Vec::with_capacity(n_tiled);
        out.push(master);
        for k in 0..m {
            let y = inner.origin.y + k * (row_h + params.gaps);
            out.push(Rect {
                origin: crate::Point { x: stack_x, y },
                size: Size {
                    w: stack_w,
                    h: row_h,
                },
            });
        }
        out
    }
}

/// Inset a rectangle by `g` on every side, clamped so size never goes
/// negative. The tiling work area is the output minus chrome; the policy
/// adds its own inner gap before tiling.
fn inset(r: Rect, g: i32) -> Rect {
    let g = g.max(0);
    let w = (r.size.w - 2 * g).max(0);
    let h = (r.size.h - 2 * g).max(0);
    Rect {
        origin: crate::Point {
            x: r.origin.x + g,
            y: r.origin.y + g,
        },
        size: Size { w, h },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Point;

    fn area(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect {
            origin: Point { x, y },
            size: Size { w, h },
        }
    }

    #[test]
    fn zero_windows_is_empty() {
        let out = MasterStack.layout(area(0, 0, 1000, 800), 0, &LayoutParams::default());
        assert!(out.is_empty());
    }

    #[test]
    fn one_window_fills_the_inner_area() {
        let p = LayoutParams {
            gaps: 10,
            ..Default::default()
        };
        let out = MasterStack.layout(area(0, 0, 1000, 800), 1, &p);
        assert_eq!(out.len(), 1);
        // Inner = inset by 10 → (10,10,980,780).
        assert_eq!(out[0], area(10, 10, 980, 780));
    }

    #[test]
    fn two_windows_master_left_stack_right_full_height() {
        let p = LayoutParams {
            gaps: 8,
            master_ratio: 0.5,
        };
        // Inner = (8,8,984,784). avail = 984-8 = 976; master_w = 488.
        let out = MasterStack.layout(area(0, 0, 1000, 800), 2, &p);
        assert_eq!(out.len(), 2);
        // Master: (8,8,488,784).
        assert_eq!(out[0], area(8, 8, 488, 784));
        // Stack: x = 8+488+8 = 504; w = 984-488-8 = 488; full height.
        assert_eq!(out[1], area(504, 8, 488, 784));
    }

    #[test]
    fn three_windows_two_equal_stack_rows() {
        let p = LayoutParams {
            gaps: 10,
            master_ratio: 0.5,
        };
        // Inner = (10,10,980,780). avail = 970; master_w = 485.
        // stack_w = 980-485-10 = 485. m=2 rows: row_h = (780 - 10)/2 = 385.
        let out = MasterStack.layout(area(0, 0, 1000, 800), 3, &p);
        assert_eq!(out.len(), 3);
        assert_eq!(out[0], area(10, 10, 485, 780)); // master
        assert_eq!(out[1], area(485 + 10 + 10, 10, 485, 385)); // first stack row
                                                               // Second row starts at y = 10 + 385 + 10 = 405.
        assert_eq!(out[2], area(505, 405, 485, 385));
    }

    #[test]
    fn count_always_matches_input() {
        let p = LayoutParams::default();
        for n in 0..=6usize {
            let out = MasterStack.layout(area(0, 0, 1200, 900), n, &p);
            assert_eq!(out.len(), n, "n={n}");
        }
    }

    #[test]
    fn tiles_stay_within_the_inner_area() {
        let p = LayoutParams {
            gaps: 12,
            master_ratio: 0.6,
        };
        let work = area(0, 0, 1280, 720);
        let inner = inset(work, p.gaps);
        let out = MasterStack.layout(work, 4, &p);
        assert_eq!(out.len(), 4);
        for r in &out {
            // Every tile lies within the inner rect and is non-negative.
            assert!(r.origin.x >= inner.origin.x, "{r:?}");
            assert!(r.origin.y >= inner.origin.y, "{r:?}");
            assert!(
                r.origin.x + r.size.w <= inner.origin.x + inner.size.w,
                "{r:?}"
            );
            assert!(
                r.origin.y + r.size.h <= inner.origin.y + inner.size.h,
                "{r:?}"
            );
            assert!(r.size.w >= 0 && r.size.h >= 0);
        }
    }

    #[test]
    fn master_ratio_shifts_the_split() {
        let p_small = LayoutParams {
            gaps: 0,
            master_ratio: 0.25,
        };
        let p_big = LayoutParams {
            gaps: 0,
            master_ratio: 0.75,
        };
        let work = area(0, 0, 1000, 800);
        let small = MasterStack.layout(work, 2, &p_small);
        let big = MasterStack.layout(work, 2, &p_big);
        assert!(
            small[0].size.w < big[0].size.w,
            "bigger ratio → wider master"
        );
        assert_eq!(small[0].size.w, 250);
        assert_eq!(big[0].size.w, 750);
    }

    #[test]
    fn tiny_work_area_never_produces_negative_sizes() {
        let p = LayoutParams {
            gaps: 50,
            master_ratio: 0.5,
        };
        let out = MasterStack.layout(area(0, 0, 10, 10), 3, &p);
        assert_eq!(out.len(), 3);
        for r in &out {
            assert!(r.size.w >= 0 && r.size.h >= 0, "{r:?}");
        }
    }

    #[test]
    fn layout_role_defaults_to_floating() {
        assert_eq!(LayoutRole::default(), LayoutRole::Floating);
    }
}
