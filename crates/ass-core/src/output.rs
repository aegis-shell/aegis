//! Output and monitor geometry (ADR-0028).
//!
//! Pure, backend- and renderer-agnostic model of one output's physical
//! mode, scale, and transform, plus the derivation of the logical size the
//! chrome and clients see. This is the foundation the multi-output milestone
//! (M7) and the tiling work-area build on; the workspace model's
//! [`Output`](crate::workspace::Output) gains a geometry reference when M7
//! wires real hotplug.

use crate::{Point, Rect, Size, Transform};

/// A display mode: physical resolution and refresh rate.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputMode {
    /// Physical width in device pixels.
    pub width: i32,
    /// Physical height in device pixels.
    pub height: i32,
    /// Refresh rate in millihertz (e.g. 60000 for 60 Hz), matching the
    /// DRM/KMS and `wl_output.mode` convention.
    pub refresh_mhz: u32,
}

/// A scale factor. Carries a fractional value so HiDPI hardware that prefers
/// a non-integer scale (1.5, 1.25) is representable, beyond the integer-only
/// `wl_surface.set_buffer_scale`. Maps to `wp_fractional_scale_v1`.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Scale(pub f32);

impl Scale {
    /// No scaling — logical and physical pixels coincide.
    pub const IDENTITY: Scale = Scale(1.0);

    /// The scale as an `f32`.
    pub fn as_f32(self) -> f32 {
        self.0
    }
}

impl Default for Scale {
    fn default() -> Scale {
        Scale::IDENTITY
    }
}

/// Per-output geometry (ADR-0028): the physical mode, scale, transform, and
/// the output's top-left in the global logical layout. From these the
/// [`logical_size`](Self::logical_size) — the size the chrome and clients
/// operate in — is derived.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct OutputGeometry {
    pub mode: OutputMode,
    pub scale: Scale,
    pub transform: Transform,
    /// Top-left of this output in the global logical coordinate space. The
    /// primary output sits at (0, 0); others are placed relative to it.
    pub logical_origin: Point,
}

impl Default for OutputGeometry {
    fn default() -> OutputGeometry {
        OutputGeometry {
            mode: OutputMode {
                width: 0,
                height: 0,
                refresh_mhz: 0,
            },
            scale: Scale::IDENTITY,
            transform: Transform::Normal,
            logical_origin: Point::default(),
        }
    }
}

impl OutputGeometry {
    /// The logical size the chrome and clients see: the physical mode, axes
    /// swapped for a 90°/270° transform, divided by the scale. Rounded to the
    /// nearest logical pixel.
    pub fn logical_size(&self) -> Size {
        let (w, h) = if self.transform.swap_axes() {
            (self.mode.height, self.mode.width)
        } else {
            (self.mode.width, self.mode.height)
        };
        let s = self.scale.0;
        if s <= 0.0 {
            // A non-positive scale is nonsensical; avoid divide-by-zero.
            return Size { w, h };
        }
        Size {
            w: ((w as f32) / s).round() as i32,
            h: ((h as f32) / s).round() as i32,
        }
    }

    /// The output's rect in the global logical layout.
    pub fn logical_rect(&self) -> Rect {
        Rect {
            origin: self.logical_origin,
            size: self.logical_size(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mode(w: i32, h: i32) -> OutputMode {
        OutputMode {
            width: w,
            height: h,
            refresh_mhz: 60000,
        }
    }

    #[test]
    fn identity_scale_and_transform_keeps_physical_size() {
        let g = OutputGeometry {
            mode: mode(1920, 1080),
            ..Default::default()
        };
        assert_eq!(g.logical_size(), Size { w: 1920, h: 1080 });
    }

    #[test]
    fn integer_scale_halves_a_hidpi_mode() {
        let g = OutputGeometry {
            mode: mode(3840, 2160),
            scale: Scale(2.0),
            ..Default::default()
        };
        assert_eq!(g.logical_size(), Size { w: 1920, h: 1080 });
    }

    #[test]
    fn fractional_scale_supports_non_integer() {
        let g = OutputGeometry {
            mode: mode(3000, 2000),
            scale: Scale(1.5),
            ..Default::default()
        };
        // 3000/1.5 = 2000, 2000/1.5 ≈ 1333.33 → 1333.
        assert_eq!(g.logical_size(), Size { w: 2000, h: 1333 });
    }

    #[test]
    fn rotate_90_swaps_axes() {
        let g = OutputGeometry {
            mode: mode(1920, 1080), // landscape panel
            transform: Transform::Rotate90,
            ..Default::default()
        };
        // Rotated portrait: logical width = physical height and vice versa.
        assert_eq!(g.logical_size(), Size { w: 1080, h: 1920 });
    }

    #[test]
    fn rotate_90_and_scale_compose() {
        let g = OutputGeometry {
            mode: mode(3840, 2160),
            scale: Scale(2.0),
            transform: Transform::Rotate90,
            ..Default::default()
        };
        // Swap → (2160, 3840); /2 → (1080, 1920).
        assert_eq!(g.logical_size(), Size { w: 1080, h: 1920 });
    }

    #[test]
    fn flip_variants_swap_axes_only_for_90_270() {
        let normal = OutputGeometry {
            mode: mode(1920, 1080),
            ..Default::default()
        };
        let flip_h = OutputGeometry {
            mode: mode(1920, 1080),
            transform: Transform::FlipHorizontal,
            ..normal
        };
        // Pure flips (no rotation) do not swap axes.
        assert_eq!(flip_h.logical_size(), normal.logical_size());
    }

    #[test]
    fn logical_rect_combines_origin_and_size() {
        let g = OutputGeometry {
            mode: mode(2560, 1440),
            scale: Scale(2.0),
            logical_origin: Point { x: 960, y: 0 },
            ..Default::default()
        };
        assert_eq!(
            g.logical_rect(),
            Rect {
                origin: Point { x: 960, y: 0 },
                size: Size { w: 1280, h: 720 },
            }
        );
    }

    #[test]
    fn non_positive_scale_falls_back_to_physical() {
        let g = OutputGeometry {
            mode: mode(100, 50),
            scale: Scale(0.0),
            ..Default::default()
        };
        assert_eq!(g.logical_size(), Size { w: 100, h: 50 });
    }
}
