//! Shared compositor model for aegis.
//!
//! This crate holds the backend- and renderer-agnostic state: geometry, the
//! surface tree, output and focus models, and the input-event types backends
//! emit. It is deliberately free of flux and Wayland types so it can later
//! grow the semantic introspection surface the AI-adaptation phase needs.

pub mod app;
pub mod dmabuf;
pub mod gesture;
pub mod input;
pub mod keybind;
pub mod launcher;
pub mod layout;
pub mod notify;
pub mod output;
pub mod overview;
pub mod realm;
pub mod settings;
pub mod system;
pub mod transition;
pub mod window;
pub mod window_rule;
pub mod window_state_store;
pub mod window_switcher;
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

    /// A degenerate (zero-area) rectangle covers no pixels.
    pub fn is_empty(&self) -> bool {
        self.size.w <= 0 || self.size.h <= 0
    }

    /// The largest rectangle shared with `other`, or `None` if they do not
    /// overlap. Used by occlusion culling to intersect coverage regions.
    pub fn intersect(&self, other: Rect) -> Option<Rect> {
        let x0 = self.origin.x.max(other.origin.x);
        let y0 = self.origin.y.max(other.origin.y);
        let x1 = (self.origin.x + self.size.w).min(other.origin.x + other.size.w);
        let y1 = (self.origin.y + self.size.h).min(other.origin.y + other.size.h);
        if x1 <= x0 || y1 <= y0 {
            None
        } else {
            Some(Rect::new(x0, y0, x1 - x0, y1 - y0))
        }
    }

    /// Remove `hole` from this rectangle, returning the (up to four) disjoint
    /// rectangles that remain. An empty result means `hole` fully covered this
    /// rectangle. This is the geometric core of occlusion culling: subtracting
    /// every occluder's coverage from a target window leaves a non-empty set
    /// exactly when some part of the target is still visible.
    pub fn subtract(&self, hole: Rect) -> Vec<Rect> {
        if self.is_empty() || hole.is_empty() {
            return vec![*self];
        }
        let Some(clip) = self.intersect(hole) else {
            // No overlap: nothing removed.
            return vec![*self];
        };
        let mut out = Vec::with_capacity(4);
        let Self {
            origin: Point { x: sx, y: sy },
            size: Size { w: sw, h: sh },
        } = *self;
        let left = clip.origin.x - sx;
        let top = clip.origin.y - sy;
        let right = (clip.origin.x + clip.size.w) - (sx + sw);
        let bottom = (clip.origin.y + clip.size.h) - (sy + sh);
        if left > 0 {
            out.push(Rect::new(sx, sy, left, sh));
        }
        if right < 0 {
            let x = clip.origin.x + clip.size.w;
            out.push(Rect::new(x, sy, -right, sh));
        }
        if top > 0 {
            out.push(Rect::new(clip.origin.x, sy, clip.size.w, top));
        }
        if bottom < 0 {
            let y = clip.origin.y + clip.size.h;
            out.push(Rect::new(clip.origin.x, y, clip.size.w, -bottom));
        }
        out
    }

    /// Whether `self` is entirely covered by the union of `occluders`. A
    /// conservative, exact test built on [`Rect::subtract`]: subtracting every
    /// occluder and exhausting all fragments means no pixel of `self` remains
    /// uncovered. Occluders may overlap; the subtraction handles that.
    pub fn fully_covered_by(self, occluders: &[Rect]) -> bool {
        if self.is_empty() {
            return true;
        }
        let mut fragments = vec![self];
        for &occluder in occluders {
            if occluder.is_empty() {
                continue;
            }
            let mut next = Vec::new();
            for fragment in fragments {
                next.extend(fragment.subtract(occluder));
            }
            fragments = next;
            if fragments.is_empty() {
                return true;
            }
        }
        fragments.is_empty()
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

    /// Map a damage rectangle from **buffer pixel coordinates** to
    /// **surface-local buffer pixel coordinates** (i.e. the coordinate space the
    /// surface occupies *after* this transform but *before* division by
    /// `buffer_scale`). All eight Wayland transforms are pure 90°-multiple
    /// rotations and/or axis flips, so an axis-aligned buffer rectangle maps to
    /// another axis-aligned rectangle; the result is the tight bounding box of
    /// the four transformed corners, rounded outward so no edge pixel is lost.
    ///
    /// `buffer_dims` is the buffer's own `(width, height)` in pixels. Callers
    /// then divide by `buffer_scale` (with outward rounding) to reach
    /// surface-local logical pixels — exactly what `buffer_damage_to_surface`
    /// already does for the identity case.
    ///
    /// This closes the historical "transform ⇒ unmappable ⇒ full damage"
    /// fallback: a rotated/flipped client's `wl_surface.damage_buffer` no longer
    /// forces a whole-output repaint.
    pub fn map_buffer_rect_to_surface(self, rect: Rect, buffer_dims: (i32, i32)) -> Rect {
        let (bw, bh) = buffer_dims;
        // Buffer rectangle corners (inclusive-exclusive spans).
        let (bx0, by0, bx1, by1) = (
            i64::from(rect.origin.x),
            i64::from(rect.origin.y),
            i64::from(rect.origin.x + rect.size.w),
            i64::from(rect.origin.y + rect.size.h),
        );
        let bwi = i64::from(bw.max(1));
        let bhi = i64::from(bh.max(1));
        // Point transform for each of the 8 cases, accumulated as the min/max
        // of the four corner images.
        let mut sx0 = i64::MAX;
        let mut sy0 = i64::MAX;
        let mut sx1 = i64::MIN;
        let mut sy1 = i64::MIN;
        for (px, py) in [(bx0, by0), (bx1, by0), (bx0, by1), (bx1, by1)] {
            let (ux, uy) = match self {
                Transform::Normal => (px, py),
                Transform::Rotate90 => (bhi - py, px),
                Transform::Rotate180 => (bwi - px, bhi - py),
                Transform::Rotate270 => (py, bwi - px),
                Transform::FlipHorizontal => (bwi - px, py),
                Transform::FlipRotate90 => (py, px),
                Transform::FlipRotate180 => (px, bhi - py),
                Transform::FlipRotate270 => (bhi - py, bwi - px),
            };
            sx0 = sx0.min(ux);
            sy0 = sy0.min(uy);
            sx1 = sx1.max(ux);
            sy1 = sy1.max(uy);
        }
        let clamp = |v: i64| v.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
        Rect::new(
            clamp(sx0),
            clamp(sy0),
            clamp(sx1.saturating_sub(sx0)),
            clamp(sy1.saturating_sub(sy0)),
        )
    }

    /// The canonical lowercase name (hyphenated) used in configuration and
    /// documentation. [`Transform::from_name`] accepts these names plus
    /// underscore aliases.
    pub fn name(self) -> &'static str {
        match self {
            Transform::Normal => "normal",
            Transform::Rotate90 => "90",
            Transform::Rotate180 => "180",
            Transform::Rotate270 => "270",
            Transform::FlipHorizontal => "flipped",
            Transform::FlipRotate90 => "flipped-90",
            Transform::FlipRotate180 => "flipped-180",
            Transform::FlipRotate270 => "flipped-270",
        }
    }

    /// Resolve a transform by its config name: the canonical
    /// [`Transform::name`] forms plus underscore aliases (`flipped_90` and
    /// friends). Matching is exact and lowercase; unknown names return
    /// `None` so the caller can diagnose them.
    pub fn from_name(s: &str) -> Option<Transform> {
        Some(match s {
            "normal" => Transform::Normal,
            "90" => Transform::Rotate90,
            "180" => Transform::Rotate180,
            "270" => Transform::Rotate270,
            "flipped" => Transform::FlipHorizontal,
            "flipped-90" | "flipped_90" => Transform::FlipRotate90,
            "flipped-180" | "flipped_180" => Transform::FlipRotate180,
            "flipped-270" | "flipped_270" => Transform::FlipRotate270,
            _ => return None,
        })
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

    #[test]
    fn transform_maps_buffer_damage_to_the_renderer_surface_orientation() {
        let rect = Rect::new(10, 15, 20, 10);
        let dims = (100, 60);
        let expected = [
            (Transform::Normal, Rect::new(10, 15, 20, 10)),
            (Transform::Rotate90, Rect::new(35, 10, 10, 20)),
            (Transform::Rotate180, Rect::new(70, 35, 20, 10)),
            (Transform::Rotate270, Rect::new(15, 70, 10, 20)),
            (Transform::FlipHorizontal, Rect::new(70, 15, 20, 10)),
            (Transform::FlipRotate90, Rect::new(15, 10, 10, 20)),
            (Transform::FlipRotate180, Rect::new(10, 35, 20, 10)),
            (Transform::FlipRotate270, Rect::new(35, 70, 10, 20)),
        ];

        for (transform, mapped) in expected {
            assert_eq!(
                transform.map_buffer_rect_to_surface(rect, dims),
                mapped,
                "{transform:?}"
            );
        }
    }

    #[test]
    fn transform_maps_a_full_buffer_to_the_full_transformed_extent() {
        let full = Rect::new(0, 0, 100, 60);
        for transform in [
            Transform::Normal,
            Transform::Rotate180,
            Transform::FlipHorizontal,
            Transform::FlipRotate180,
        ] {
            assert_eq!(
                transform.map_buffer_rect_to_surface(full, (100, 60)),
                Rect::new(0, 0, 100, 60),
                "{transform:?}"
            );
        }
        for transform in [
            Transform::Rotate90,
            Transform::Rotate270,
            Transform::FlipRotate90,
            Transform::FlipRotate270,
        ] {
            assert_eq!(
                transform.map_buffer_rect_to_surface(full, (100, 60)),
                Rect::new(0, 0, 60, 100),
                "{transform:?}"
            );
        }
    }

    #[test]
    fn transform_name_round_trips_through_from_name() {
        let all = [
            Transform::Normal,
            Transform::Rotate90,
            Transform::Rotate180,
            Transform::Rotate270,
            Transform::FlipHorizontal,
            Transform::FlipRotate90,
            Transform::FlipRotate180,
            Transform::FlipRotate270,
        ];
        for t in all {
            assert_eq!(Transform::from_name(t.name()), Some(t), "{t:?}");
        }
    }

    #[test]
    fn transform_from_name_accepts_underscore_aliases_only() {
        assert_eq!(
            Transform::from_name("flipped_90"),
            Some(Transform::FlipRotate90)
        );
        assert_eq!(
            Transform::from_name("flipped_270"),
            Some(Transform::FlipRotate270)
        );
        // Exact lowercase matching: no case folding, no invented synonyms.
        assert_eq!(Transform::from_name("Normal"), None);
        assert_eq!(Transform::from_name("rotate90"), None);
        assert_eq!(Transform::from_name("upside-down"), None);
        assert_eq!(Transform::from_name(""), None);
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
    /// local logical coordinates. Empty means "no damage info; renderer may
    /// choose to skip the incremental update and re-upload the whole
    /// texture on the next generation change." Bounded to the surface's
    /// width/height by the server before being surfaced.
    pub damage: &'a [Rect],
    /// Opaque region declared via `wl_surface.set_opaque_region`, in surface-
    /// local logical coordinates, or `None` when the client declared no opaque
    /// region (the surface is treated as fully translucent). Lets the renderer
    /// split the blit so pixels under an opaque sub-rect use SRC-replace (no
    /// framebuffer readback) even for ARGB buffers, matching the occlusion
    /// pass's notion of opaqueness. Empty slice == no opaque region.
    pub opaque_region: Option<&'a [Rect]>,
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
    /// Damage accumulated since the previous presented frame, in
    /// surface-local logical coordinates. Empty means the damage is unknown
    /// and consumers must conservatively treat the whole surface as changed.
    pub damage: Vec<Rect>,
    /// Stable monotonic identity of the backing `wl_buffer`, or 0 when
    /// unknown. Unlike `generation` — which is bumped on every commit — this
    /// stays constant across frames that reuse the same buffer and is not
    /// vulnerable to allocator reuse of a destroyed `wl_resource` address.
    /// `fd` cannot serve that role: it is re-`dup`ed per commit.
    pub buffer_id: u64,
    pub fd: i32,
    pub drm_format: u32,
    pub modifier: u64,
    pub offset: u32,
    pub stride: u32,
    /// Borrowed Linux sync_file fd for this generation, or -1 when implicit
    /// synchronization applies. The renderer duplicates it before import.
    pub acquire_fence: i32,
    pub geometry: SurfaceGeometry,
    /// Opaque region declared via `wl_surface.set_opaque_region`, in surface-
    /// local logical coordinates, or `None` when the client declared no opaque
    /// region. Owned (like `damage`) because the dma-buf frame may outlive the
    /// server borrow. Lets the renderer split the blit into SRC-replace opaque
    /// sub-rects and source-over remainder, matching the occlusion pass.
    pub opaque_region: Option<Vec<Rect>>,
}
