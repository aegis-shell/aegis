//! Shared compositor model for ass.
//!
//! This crate holds the backend- and renderer-agnostic state: geometry, the
//! surface tree, output and focus models, and the input-event types backends
//! emit. It is deliberately free of flux and Wayland types so it can later
//! grow the semantic introspection surface the AI-adaptation phase needs.

pub mod app;
pub mod input;
pub mod keybind;
pub mod launcher;
pub mod layout;
pub mod notify;
pub mod output;
pub mod overview;
pub mod transition;
pub mod window;
pub mod window_rule;
pub mod workspace;

/// An integer point in compositor (logical) coordinate space.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

/// An integer size in logical pixels.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Size {
    pub w: i32,
    pub h: i32,
}

/// An axis-aligned rectangle in logical coordinate space.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub origin: Point,
    pub size: Size,
}

impl Rect {
    pub fn new(x: i32, y: i32, w: i32, h: i32) -> Rect {
        Rect {
            origin: Point { x, y },
            size: Size { w, h },
        }
    }

    pub fn contains(&self, p: Point) -> bool {
        p.x >= self.origin.x
            && p.y >= self.origin.y
            && p.x < self.origin.x + self.size.w
            && p.y < self.origin.y + self.size.h
    }
}

/// Buffer transform, mirroring `wl_surface.set_buffer_transform`. The eight
/// 90-degree rotations + reflections cover everything the Wayland core
/// protocol defines. Compositing must apply this to the buffer's UVs (or
/// pre-transform into a staging texture) before drawing.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u32)]
pub enum Transform {
    /// Identity. The buffer's first row is its top row, first column its left.
    #[default]
    Normal,
    /// Rotated 90 degrees counter-clockwise.
    Rotate90,
    /// Rotated 180 degrees.
    Rotate180,
    /// Rotated 270 degrees counter-clockwise.
    Rotate270,
    /// Mirrored along the vertical axis, no rotation.
    FlipHorizontal,
    /// Mirrored then rotated 90 degrees counter-clockwise.
    FlipRotate90,
    /// Mirrored then rotated 180 degrees.
    FlipRotate180,
    /// Mirrored then rotated 270 degrees counter-clockwise.
    FlipRotate270,
}

impl Transform {
    /// Apply this transform to a (width, height) pair, swapping axes for the
    /// odd rotations. Used by the renderer to compute destination dimensions
    /// that match a transformed source buffer.
    pub fn swap_axes(self) -> bool {
        matches!(
            self,
            Transform::Rotate90
                | Transform::Rotate270
                | Transform::FlipRotate90
                | Transform::FlipRotate270
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_contains_inclusive_origin_exclusive_far_corner() {
        let r = Rect::new(10, 20, 100, 50);
        assert!(r.contains(Point { x: 10, y: 20 })); // top-left inclusive
        assert!(r.contains(Point { x: 109, y: 69 })); // bottom-right - 1
        assert!(!r.contains(Point { x: 110, y: 20 })); // right edge exclusive
        assert!(!r.contains(Point { x: 10, y: 70 })); // bottom edge exclusive
        assert!(!r.contains(Point { x: 9, y: 20 })); // left of origin
        assert!(!r.contains(Point { x: 10, y: 19 })); // above origin
    }

    #[test]
    fn transform_swap_axes_matches_90_and_270_only() {
        let should_swap = [
            Transform::Rotate90,
            Transform::Rotate270,
            Transform::FlipRotate90,
            Transform::FlipRotate270,
        ];
        let should_not_swap = [
            Transform::Normal,
            Transform::Rotate180,
            Transform::FlipHorizontal,
            Transform::FlipRotate180,
        ];
        for t in should_swap {
            assert!(t.swap_axes(), "{t:?} should swap axes");
        }
        for t in should_not_swap {
            assert!(!t.swap_axes(), "{t:?} should not swap axes");
        }
    }
}

/// Common geometry/metadata carried alongside every surface's pixels, whether
/// the backing store is shm CPU memory or a dma-buf fd. Fields default to
/// "no transform, scale 1, no offset, no clipping" so existing call sites
/// that only populate the buffer itself still produce a visible result.
#[derive(Debug, Clone, Copy)]
pub struct SurfaceGeometry {
    /// Logical position of the surface's top-left corner in the output.
    pub position: Point,
    /// Logical extent the client declared via `xdg_surface.set_window_geometry`.
    /// None means the buffer's own w/h is the geometry.
    pub window_geometry: Option<Rect>,
    /// Buffer transform (rotation/reflection) to apply at composite time.
    pub transform: Transform,
    /// Buffer scale factor (1 = unscaled). HiDPI clients commit at scale N and
    /// expect the compositor to divide destination dimensions by N.
    pub buffer_scale: i32,
    /// `wp_viewport.set_source` rectangle in surface-local pixel coords, or
    /// None for "whole buffer".
    pub viewport_src: Option<Rect>,
    /// `wp_viewport.set_destination` size in logical pixels, or None for
    /// "buffer or source size".
    pub viewport_dst: Option<Size>,
    /// Interpolated size while a window transition (ADR-0029) is in flight.
    /// The renderer draws the root texture scaled to this instead of the
    /// buffer-implied size. `None` outside transitions and for subsurfaces.
    pub transition_size: Option<Size>,
}

impl Default for SurfaceGeometry {
    /// `buffer_scale` defaults to 1 (unscaled), not the i32 default of 0,
    /// so a partially populated `SurfaceGeometry` produces a visible surface
    /// rather than a divide-by-zero in the renderer's destination-size
    /// calculation.
    fn default() -> Self {
        SurfaceGeometry {
            position: Point::default(),
            window_geometry: None,
            transform: Transform::Normal,
            buffer_scale: 1,
            viewport_src: None,
            viewport_dst: None,
            transition_size: None,
        }
    }
}

/// A borrowed view of a surface's current contents, handed from the server to
/// the renderer. Pixels are tightly packed BGRA8 (`stride == width * 4`).
/// `generation` increments on every new commit so the renderer can cache the
/// uploaded texture and re-upload only when the contents change.
pub struct SurfacePixels<'a> {
    /// Stable identifier for the surface (its record address).
    pub id: usize,
    /// Toplevel the surface belongs to (its root's window id), or `None`
    /// for compositor-owned overlays. Lets the renderer's mapped drawing
    /// (overview) group frames per window.
    pub window: Option<crate::window::WindowId>,
    pub width: i32,
    pub height: i32,
    pub generation: u64,
    pub pixels: &'a [u8],
    pub geometry: SurfaceGeometry,
    /// Damage rectangles accumulated since the last commit, in surface-
    /// local pixel coords. Empty means "no damage info; renderer may
    /// choose to skip the incremental update and re-upload the whole
    /// texture on the next generation change." Bounded to the surface's
    /// width/height by the server before being surfaced.
    pub damage: &'a [Rect],
}

/// A single-plane dma-buf-backed surface, handed from the server to the
/// renderer for zero-copy import. The `fd` is borrowed: the renderer (flux)
/// duplicates it before import; the server keeps ownership and closes it when
/// the surface backing is replaced or destroyed. `drm_format` is a DRM fourcc.
pub struct SurfaceDmabuf {
    pub id: usize,
    /// Toplevel the surface belongs to (its root's window id), or `None`
    /// for compositor-owned overlays. Lets the renderer's mapped drawing
    /// (overview) group frames per window.
    pub window: Option<crate::window::WindowId>,
    pub width: i32,
    pub height: i32,
    pub generation: u64,
    pub fd: i32,
    pub drm_format: u32,
    pub modifier: u64,
    pub offset: u32,
    pub stride: u32,
    /// Borrowed Linux sync_file fd for this generation, or -1 when implicit
    /// synchronization applies. The renderer duplicates it before import.
    pub acquire_fence: i32,
    pub geometry: SurfaceGeometry,
}
