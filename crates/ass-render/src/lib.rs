//! Compositing for ass, built on flux.
//!
//! Turns the surface tree into draw calls: each client buffer becomes a flux
//! texture (shm via CPU upload, dmabuf via zero-copy import), composited in
//! z-order into the output's frame.

use std::borrow::Cow;
use std::collections::HashMap;
use std::os::raw::c_int;

use ass_core::{SurfaceDmabuf, SurfacePixels, Transform};

unsafe extern "C" {
    fn dup(fd: c_int) -> c_int;
    fn close(fd: c_int) -> c_int;
}

/// DRM fourccs the compositor advertises, mapped to flux formats. The
/// 32-bit per-pixel layouts are little-endian: `ARGB8888` is `[B, G, R, A]` in
/// memory → flux `BGRA8_UNORM`; the byte-swapped `ABGR8888` is
/// `[R, G, B, A]` → flux `RGBA8_UNORM`. The `X*` variants carry an undefined
/// alpha that the server forces opaque at commit time.
const DRM_FORMAT_ARGB8888: u32 = 0x3432_5241;
const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258;
const DRM_FORMAT_ABGR8888: u32 = 0x3432_4241;
const DRM_FORMAT_XBGR8888: u32 = 0x3432_4258;

fn drm_format_to_flux(drm: u32) -> Option<flux::Format> {
    match drm {
        DRM_FORMAT_ARGB8888 | DRM_FORMAT_XRGB8888 => Some(flux::Format::FLUX_FORMAT_BGRA8_UNORM),
        DRM_FORMAT_ABGR8888 | DRM_FORMAT_XBGR8888 => Some(flux::Format::FLUX_FORMAT_RGBA8_UNORM),
        _ => None,
    }
}

/// Apply a `wl_surface.set_buffer_transform` to tightly packed BGRA8/RGBA8
/// pixels (4 bytes per pixel, no padding). Returns the transformed buffer
/// and its new dimensions (swapped for the 90°/270° rotations).
///
/// This is the CPU-staging fallback. A GPU-side transform (per-vertex UVs
/// in flux's image fragment shader) is the long-term path; until that lands,
/// the cost is one pixel copy per commit on transformed surfaces, which is
/// negligible for the static-image case and acceptable for video.
///
/// `Normal` returns the input as a borrowed `Cow`, so the common case of
/// "no transform" pays nothing.
fn transform_pixels<'a>(
    src: &'a [u8],
    width: usize,
    height: usize,
    transform: Transform,
) -> Cow<'a, [u8]> {
    const BPP: usize = 4;
    if transform == Transform::Normal || width == 0 || height == 0 {
        return Cow::Borrowed(src);
    }
    let n = width * height * BPP;
    if src.len() < n {
        return Cow::Borrowed(src);
    }
    let mut dst = vec![0u8; n];
    let swap_axes = transform.swap_axes();
    // Destination dimensions for axis-swapping transforms; identity for the
    // 180°/flip-180° cases that keep dimensions.
    let (new_w, new_h) = if swap_axes {
        (height, width)
    } else {
        (width, height)
    };
    // For each destination pixel (dy, dx) compute the source pixel (sy, sx)
    // and copy 4 bytes. The per-pixel math comes from inverting each
    // transform's "buffer was rotated X from upright" definition back to
    // "what source pixel lands at this destination pixel".
    for dy in 0..new_h {
        for dx in 0..new_w {
            let (sx, sy): (usize, usize);
            match transform {
                Transform::Normal => unreachable!(),
                Transform::Rotate90 => {
                    // Buffer was rotated 90° CCW from upright; display applies
                    // the inverse (90° CW). dest (dx, dy) maps to source
                    // (sx=dy, sy=(H-1)-dx).
                    sx = dy;
                    sy = (height - 1).saturating_sub(dx);
                }
                Transform::Rotate180 => {
                    sx = width - 1 - dx;
                    sy = height - 1 - dy;
                }
                Transform::Rotate270 => {
                    // 270° CCW = 90° CW from upright; display applies 90° CCW.
                    sx = (width - 1).saturating_sub(dy);
                    sy = dx;
                }
                Transform::FlipHorizontal => {
                    sx = width - 1 - dx;
                    sy = dy;
                }
                Transform::FlipRotate90 => {
                    // "90 CCW + flip horizontal": mirror then rotate; display
                    // undoes in reverse, equivalently dest = (sy, sx).
                    sx = dy;
                    sy = dx;
                }
                Transform::FlipRotate180 => {
                    // 180 + flip = flip vertical.
                    sx = dx;
                    sy = height - 1 - dy;
                }
                Transform::FlipRotate270 => {
                    sx = (width - 1).saturating_sub(dy);
                    sy = (height - 1).saturating_sub(dx);
                }
            }
            let src_off = (sy * width + sx) * BPP;
            let dst_off = (dy * new_w + dx) * BPP;
            dst[dst_off..dst_off + BPP].copy_from_slice(&src[src_off..src_off + BPP]);
        }
    }
    Cow::Owned(dst)
}

/// Per-surface transformed dimensions after applying a transform. Returns
/// `(width, height)` post-transform; axes are swapped for 90°/270°.
fn transformed_dims(width: i32, height: i32, transform: Transform) -> (i32, i32) {
    if transform.swap_axes() {
        (height, width)
    } else {
        (width, height)
    }
}

/// Compute the destination size (in logical pixels) at which a surface's
/// buffer is drawn, given its post-transform buffer dimensions, buffer
/// scale, and the optional `wp_viewport` source/destination pair.
///
/// Mirrors `weston_surface_update_size`:
/// - `viewport_dst` set: used as-is (already in logical pixels).
/// - `viewport_dst` unset, `viewport_src` set: source size (already in
///   post-buffer-scale surface coordinates).
/// - both unset: post-transform buffer dims / scale.
///
/// `scale <= 0` is treated as 1; the server validates `value >= 1` and
/// `SurfaceGeometry::default` sets 1, so this is defence in depth.
fn destination_size(
    post_transform_dims: (i32, i32),
    viewport_src: Option<ass_core::Rect>,
    viewport_dst: Option<ass_core::Size>,
    buffer_scale: i32,
) -> (f32, f32) {
    let scale = buffer_scale.max(1) as f32;
    match (viewport_dst, viewport_src) {
        (Some(dst), _) => (dst.w as f32, dst.h as f32),
        (None, Some(src)) => (src.size.w as f32, src.size.h as f32),
        (None, None) => {
            let (tw, th) = post_transform_dims;
            (tw as f32 / scale, th as f32 / scale)
        }
    }
}

fn viewport_uv(
    source: ass_core::Rect,
    post_transform_dims: (i32, i32),
    buffer_scale: i32,
) -> (f32, f32, f32, f32) {
    let scale = buffer_scale.max(1) as f32;
    let (width, height) = post_transform_dims;
    (
        source.origin.x as f32 * scale / width.max(1) as f32,
        source.origin.y as f32 * scale / height.max(1) as f32,
        source.size.w as f32 * scale / width.max(1) as f32,
        source.size.h as f32 * scale / height.max(1) as f32,
    )
}

/// Caches per-surface GPU textures so client contents are re-uploaded only when
/// they change (tracked by the surface's generation counter). Also tracks which
/// (surface, format, modifier) tuples have already logged an import failure so
/// the diagnostic is emitted once per surface rather than every frame.
pub struct Renderer {
    cache: HashMap<usize, (flux::Image, u64)>,
    failed_imports: HashMap<usize, ()>,
}

impl Renderer {
    pub fn new() -> Renderer {
        Renderer {
            cache: HashMap::new(),
            failed_imports: HashMap::new(),
        }
    }

    /// Drop cached textures for surfaces no longer present. Call once per frame
    /// with every live surface id (shm and dma-buf).
    pub fn gc(&mut self, live_ids: impl Iterator<Item = usize>) {
        let live: std::collections::HashSet<usize> = live_ids.collect();
        self.cache.retain(|k, _| live.contains(k));
        self.failed_imports.retain(|k, _| live.contains(k));
    }

    /// Composite toplevel surfaces into `canvas`. Each is uploaded to a cached
    /// flux texture (re-uploaded only when its generation changes) and drawn
    /// at the position declared in its `geometry`. `origin` is kept for API
    /// stability but no longer offsets the draw — surface positions are
    /// authoritative.
    pub fn draw_toplevels(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        frames: &[SurfacePixels<'_>],
        _origin: (f32, f32),
    ) {
        for f in frames.iter() {
            let stale = self
                .cache
                .get(&f.id)
                .is_none_or(|(_, g)| *g != f.generation);
            if stale {
                // Apply the buffer transform on the CPU at upload time. For
                // the common case (Normal) this is a borrowed Cow with no
                // allocation; axis-swapping rotations return an owned
                // staging buffer. GPU-side transforms in flux's image
                // shader are the long-term path.
                let transformed = transform_pixels(
                    f.pixels,
                    f.width as usize,
                    f.height as usize,
                    f.geometry.transform,
                );
                let (tex_w, tex_h) = transformed_dims(f.width, f.height, f.geometry.transform);
                if let Ok(img) = flux::Image::from_bytes(
                    device,
                    tex_w as u32,
                    tex_h as u32,
                    flux::Format::FLUX_FORMAT_BGRA8_UNORM,
                    &transformed,
                ) {
                    self.cache.insert(f.id, (img, f.generation));
                }
            } else if !f.damage.is_empty()
                && f.geometry.transform == Transform::Normal
                && f.geometry.buffer_scale <= 1
            {
                // Incremental update: re-upload only the damaged regions.
                // Skipped when a transform is active because damage rects
                // are in surface-local coords and would need transformation
                // to map to the staging buffer's layout. Also skipped when
                // buffer_scale > 1: surface-local damage rects would cover
                // only 1/scale^2 of the buffer, and damage_buffer rects
                // are interleaved into the same Vec without distinction.
                // The full-upload path runs on the next generation change.
                if let Some((img, _)) = self.cache.get(&f.id) {
                    for d in f.damage {
                        // Clamp to surface bounds; clients can send rects
                        // that extend past the buffer.
                        let x = d.origin.x.max(0).min(f.width - 1) as u32;
                        let y = d.origin.y.max(0).min(f.height - 1) as u32;
                        let max_w = (f.width as u32).saturating_sub(x);
                        let max_h = (f.height as u32).saturating_sub(y);
                        let w = (d.size.w as u32).min(max_w).max(1);
                        let h = (d.size.h as u32).min(max_h).max(1);
                        // Extract the sub-rect from the source buffer.
                        // Tightly packed BGRA8: stride = width * 4.
                        let bpp = 4usize;
                        let stride = f.width as usize * bpp;
                        let mut sub = vec![0u8; (w as usize) * (h as usize) * bpp];
                        for row in 0..h as usize {
                            let src_off = ((y as usize + row) * stride) + (x as usize * bpp);
                            let dst_off = row * (w as usize * bpp);
                            let len = w as usize * bpp;
                            // src_off + len must be within f.pixels; clamp to avoid panic.
                            let avail = f.pixels.len().saturating_sub(src_off);
                            let take = len.min(avail);
                            sub[dst_off..dst_off + take]
                                .copy_from_slice(&f.pixels[src_off..src_off + take]);
                        }
                        // Update failures are non-fatal: the cached texture
                        // stays at the previous frame's contents for that
                        // region until the next full upload.
                        let _ = img.update_region(x, y, w, h, &sub);
                    }
                    // Bump the generation so a subsequent stale check
                    // doesn't re-upload the whole texture.
                    if let Some((_, gen)) = self.cache.get_mut(&f.id) {
                        *gen = f.generation;
                    }
                }
            }
            if let Some((img, _)) = self.cache.get(&f.id) {
                let x = f.geometry.position.x as f32;
                let y = f.geometry.position.y as f32;
                // Apply wp_viewport and buffer_scale to compute the
                // destination rectangle. Viewport source coordinates are
                // after buffer transform *and scale*, so convert them back to
                // buffer-pixel UVs using buffer_scale.
                let (tw, th) = transformed_dims(f.width, f.height, f.geometry.transform);
                let (dst_w, dst_h) = destination_size(
                    (tw, th),
                    f.geometry.viewport_src,
                    f.geometry.viewport_dst,
                    f.geometry.buffer_scale,
                );
                match f.geometry.viewport_src {
                    Some(src) => {
                        let (su, sv, sw, sh) = viewport_uv(src, (tw, th), f.geometry.buffer_scale);
                        canvas.draw_image_sub(img, x, y, dst_w, dst_h, su, sv, sw, sh);
                    }
                    None => {
                        canvas.draw_image(img, x, y, dst_w, dst_h);
                    }
                }
            }
        }
    }

    /// Composite dma-buf-backed toplevels by zero-copy import. Each surface's
    /// fd is imported into a cached flux texture (re-imported only when the
    /// generation changes) and drawn at the position declared in its geometry.
    pub fn draw_dmabuf_toplevels(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        frames: &[SurfaceDmabuf],
        _origin: (f32, f32),
    ) {
        for f in frames.iter() {
            let stale = self
                .cache
                .get(&f.id)
                .is_none_or(|(_, g)| *g != f.generation);
            if stale {
                if let Some(fmt) = drm_format_to_flux(f.drm_format) {
                    // Flux consumes the descriptor fd on success. The frame's
                    // fd is borrowed from the server and must remain valid for
                    // later commits, so hand Flux a fresh duplicate.
                    let import_fd = unsafe { dup(f.fd) };
                    if import_fd < 0 {
                        if self.failed_imports.insert(f.id, ()).is_none() {
                            log::warn!(
                                "[render] failed to duplicate dma-buf fd {} for {}x{}",
                                f.fd,
                                f.width,
                                f.height,
                            );
                        }
                        continue;
                    }
                    let img = unsafe {
                        flux::Image::import_dmabuf(
                            device,
                            f.width as u32,
                            f.height as u32,
                            fmt,
                            f.modifier,
                            import_fd,
                            f.offset,
                            f.stride,
                        )
                    };
                    match img {
                        Ok(img) => {
                            if !self.cache.contains_key(&f.id) {
                                log::info!(
                                    "[render] dma-buf imported: {}x{} fourcc={:#x} mod={:#x}",
                                    f.width,
                                    f.height,
                                    f.drm_format,
                                    f.modifier
                                );
                            }
                            // A successful import after a prior failure: clear
                            // the suppression so a transient error is logged
                            // again if it recurs.
                            self.failed_imports.remove(&f.id);
                            self.cache.insert(f.id, (img, f.generation));
                        }
                        Err(e) => {
                            // Flux leaves ownership with the caller on error.
                            unsafe { close(import_fd) };
                            // Suppress repeated identical failures; otherwise a
                            // persistent import problem floods the log every
                            // frame, since `stale` keeps re-triggering the path.
                            // `HashMap::insert` returns None when the key is new.
                            if self.failed_imports.insert(f.id, ()).is_none() {
                                log::warn!(
                                    "[render] dma-buf import failed ({e}): {}x{} fourcc={:#x} mod={:#x} stride={} offset={} fd={}",
                                    f.width,
                                    f.height,
                                    f.drm_format,
                                    f.modifier,
                                    f.stride,
                                    f.offset,
                                    f.fd,
                                );
                            }
                        }
                    }
                }
            }
            if let Some((img, _)) = self.cache.get(&f.id) {
                let x = f.geometry.position.x as f32;
                let y = f.geometry.position.y as f32;
                // Destination size from viewport + buffer_scale, mirroring
                // the shm path. The dmabuf path does not CPU-stage the
                // buffer transform, so post-transform dims equal the raw
                // buffer dims here.
                let (dst_w, dst_h) = destination_size(
                    (f.width, f.height),
                    f.geometry.viewport_src,
                    f.geometry.viewport_dst,
                    f.geometry.buffer_scale,
                );
                match f.geometry.viewport_src {
                    Some(src) => {
                        let (su, sv, sw, sh) =
                            viewport_uv(src, (f.width, f.height), f.geometry.buffer_scale);
                        canvas.draw_image_sub(img, x, y, dst_w, dst_h, su, sv, sw, sh);
                    }
                    None => {
                        canvas.draw_image(img, x, y, dst_w, dst_h);
                    }
                }
            }
        }
    }
}

impl Default for Renderer {
    fn default() -> Renderer {
        Renderer::new()
    }
}

impl Renderer {
    /// Composite subsurfaces. Subsurfaces are drawn in z-order (caller passes
    /// them already sorted: below-parent first, then above-parent). Their
    /// `geometry.position` is absolute (parent position + subsurface offset),
    /// so the renderer uses it directly. The per-id texture cache is shared
    /// with the toplevel draw path.
    pub fn draw_subsurfaces(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        frames: &[SurfacePixels<'_>],
    ) {
        self.draw_toplevels(device, canvas, frames, (0.0, 0.0));
    }

    /// Composite dma-buf-backed subsurfaces. Mirrors `draw_subsurfaces` for
    /// the zero-copy import path.
    pub fn draw_dmabuf_subsurfaces(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        frames: &[SurfaceDmabuf],
    ) {
        self.draw_dmabuf_toplevels(device, canvas, frames, (0.0, 0.0));
    }
}

/// Create a flux device suitable for compositing.
///
/// `headless` skips swapchain/presentation requirements — used for smoke tests
/// and any logic that never presents. A windowed backend passes `false` plus
/// the surface extensions it needs.
pub fn create_device(
    headless: bool,
    instance_extensions: &[&std::ffi::CStr],
    device_extensions: &[&std::ffi::CStr],
) -> Result<flux::Device, flux::Error> {
    flux::Device::new(headless, instance_extensions, device_extensions)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// GC drops only the textures for ids not present in the live set. The
    /// cache itself is empty here (no flux device in unit tests), but the
    /// bookkeeping on `failed_imports` is exercised by the same path.
    #[test]
    fn gc_keeps_live_drops_dead() {
        let mut r = Renderer::new();
        // Simulate a prior failed import so we can verify GC drops its record.
        r.failed_imports.insert(7, ());
        r.failed_imports.insert(9, ());
        // Live set: 7 stays, 9 goes.
        r.gc([7usize].into_iter());
        assert!(
            r.failed_imports.contains_key(&7),
            "live id should be retained"
        );
        assert!(
            !r.failed_imports.contains_key(&9),
            "dead id should be collected"
        );
    }

    /// Subsurfaces re-use the same cache as toplevels (keyed by surface id).
    /// Verify that calling `gc` with both id sets keeps both alive.
    #[test]
    fn gc_keeps_both_toplevel_and_subsurface_ids() {
        let mut r = Renderer::new();
        r.failed_imports.insert(1, ());
        r.failed_imports.insert(2, ());
        r.gc([1usize, 2].into_iter());
        assert!(r.failed_imports.contains_key(&1));
        assert!(r.failed_imports.contains_key(&2));
    }

    /// `Transform::Normal` returns a borrowed Cow — no allocation, no copy.
    #[test]
    fn transform_normal_is_borrowed() {
        let pixels = vec![0u8; 8 * 4]; // 8 pixels, BGRA8
        let out = transform_pixels(&pixels, 8, 1, Transform::Normal);
        assert!(matches!(out, Cow::Borrowed(_)));
    }

    /// Rotate90 of a 2×2 grid swaps axes. Construct a grid with distinct
    /// pixels per cell and verify the rotation places each in the right
    /// destination cell.
    #[test]
    fn transform_rotate90_swaps_axes_and_places_pixels() {
        // 2×2 grid, each pixel is BGRA8 with a unique value in the R channel.
        // Layout:  A=0x10 at (0,0)   B=0x20 at (1,0)
        //          C=0x30 at (0,1)   D=0x40 at (1,1)
        let pixels = [
            0x10, 0, 0, 0xff, 0x20, 0, 0, 0xff, // row 0: A B
            0x30, 0, 0, 0xff, 0x40, 0, 0, 0xff, // row 1: C D
        ];
        let out = transform_pixels(&pixels, 2, 2, Transform::Rotate90);
        let owned = match out {
            Cow::Owned(v) => v,
            _ => panic!("rotate90 should own its output"),
        };
        // Rotate90 means buffer is 90° CCW from upright; display applies
        // 90° CW. Result of rotating  A B  by 90° CW:
        //                        C D
        //   C A
        //   D B
        let r = |x: usize, y: usize| owned[(y * 2 + x) * 4];
        assert_eq!(r(0, 0), 0x30, "top-left should be C");
        assert_eq!(r(1, 0), 0x10, "top-right should be A");
        assert_eq!(r(0, 1), 0x40, "bottom-left should be D");
        assert_eq!(r(1, 1), 0x20, "bottom-right should be B");
    }

    /// A 3×1 buffer rotated 90° becomes a 1×3 buffer (axes swap).
    #[test]
    fn transform_rotate90_swaps_axes_for_non_square() {
        let pixels = [
            0x10, 0, 0, 0xff, // (0,0)=A
            0x20, 0, 0, 0xff, // (1,0)=B
            0x30, 0, 0, 0xff, // (2,0)=C
        ];
        let (tw, th) = transformed_dims(3, 1, Transform::Rotate90);
        assert_eq!((tw, th), (1, 3));
        let out = transform_pixels(&pixels, 3, 1, Transform::Rotate90);
        let owned = match out {
            Cow::Owned(v) => v,
            _ => panic!("should own"),
        };
        // 1×3 layout.
        assert_eq!(owned.len(), 3 * 4);
    }

    /// Rotate180 reverses both axes.
    #[test]
    fn transform_rotate180_reverses_both_axes() {
        let pixels = [
            0x10, 0, 0, 0xff, 0x20, 0, 0, 0xff, // A B
            0x30, 0, 0, 0xff, 0x40, 0, 0, 0xff, // C D
        ];
        let out = transform_pixels(&pixels, 2, 2, Transform::Rotate180);
        let owned = match out {
            Cow::Owned(v) => v,
            _ => panic!("should own"),
        };
        // 180° of  A B  ->  D C
        //         C D      B A
        let r = |x: usize, y: usize| owned[(y * 2 + x) * 4];
        assert_eq!(r(0, 0), 0x40);
        assert_eq!(r(1, 0), 0x30);
        assert_eq!(r(0, 1), 0x20);
        assert_eq!(r(1, 1), 0x10);
    }

    /// FlipHorizontal mirrors within each row.
    #[test]
    fn transform_flip_horizontal_mirrors_row() {
        let pixels = [
            0x10, 0, 0, 0xff, 0x20, 0, 0, 0xff, // A B
            0x30, 0, 0, 0xff, 0x40, 0, 0, 0xff, // C D
        ];
        let out = transform_pixels(&pixels, 2, 2, Transform::FlipHorizontal);
        let owned = match out {
            Cow::Owned(v) => v,
            _ => panic!("should own"),
        };
        // Flip X:  A B  ->  B A
        //          C D      D C
        let r = |x: usize, y: usize| owned[(y * 2 + x) * 4];
        assert_eq!(r(0, 0), 0x20);
        assert_eq!(r(1, 0), 0x10);
        assert_eq!(r(0, 1), 0x40);
        assert_eq!(r(1, 1), 0x30);
    }

    /// `destination_size` covers the four viewport/scale combinations.
    #[test]
    fn destination_size_handles_viewport_and_scale() {
        use ass_core::{Rect, Size};

        // No viewport, scale 1: post-transform buffer dims as-is.
        let (w, h) = destination_size((100, 50), None, None, 1);
        assert_eq!((w, h), (100.0, 50.0));

        // No viewport, scale 2: client committed at 2× the logical size.
        let (w, h) = destination_size((100, 50), None, None, 2);
        assert_eq!((w, h), (50.0, 25.0));

        // Source-only: coordinates are already after buffer scale.
        let src = Rect::new(10, 10, 80, 40);
        let (w, h) = destination_size((100, 50), Some(src), None, 1);
        assert_eq!((w, h), (80.0, 40.0));
        let (w, h) = destination_size((100, 50), Some(src), None, 2);
        assert_eq!((w, h), (80.0, 40.0));

        // Destination wins regardless of scale (already logical px).
        let dst = Size { w: 30, h: 20 };
        let (w, h) = destination_size((100, 50), Some(src), Some(dst), 1);
        assert_eq!((w, h), (30.0, 20.0));
        let (w, h) = destination_size((100, 50), Some(src), Some(dst), 2);
        assert_eq!((w, h), (30.0, 20.0));

        // Destination-only with scale: dst taken as-is, source falls back
        // to whole buffer (UV math), scale ignored on dst.
        let (w, h) = destination_size((100, 50), None, Some(dst), 2);
        assert_eq!((w, h), (30.0, 20.0));
    }

    /// A malformed `buffer_scale <= 0` is clamped to 1 rather than producing
    /// a divide-by-zero or negative destination. The server and
    /// `SurfaceGeometry::default` both ensure `>= 1`, so this is defence in
    /// depth.
    #[test]
    fn destination_size_clamps_nonpositive_scale() {
        let (w, h) = destination_size((100, 50), None, None, 0);
        assert_eq!((w, h), (100.0, 50.0));
    }

    #[test]
    fn viewport_source_converts_post_scale_coordinates_to_buffer_uvs() {
        let src = ass_core::Rect::new(10, 5, 30, 20);
        assert_eq!(viewport_uv(src, (200, 100), 2), (0.1, 0.1, 0.3, 0.4));
    }
}
