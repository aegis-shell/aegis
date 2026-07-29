//! Compositing for aegis, built on flux.
//!
//! Turns the surface tree into draw calls: each client buffer becomes a flux
//! texture (shm via CPU upload, dmabuf via zero-copy import), composited in
//! z-order into the output's frame.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::os::fd::{FromRawFd, OwnedFd};

use aegis_core::{SurfaceDmabuf, SurfacePixels, Transform, dmabuf as drm_fmt};

/// Convenience aliases for the shared DRM fourccs in [`aegis_core::dmabuf`].
const DRM_FORMAT_ARGB8888: u32 = drm_fmt::DRM_FORMAT_ARGB8888;
const DRM_FORMAT_XRGB8888: u32 = drm_fmt::DRM_FORMAT_XRGB8888;
const DRM_FORMAT_ABGR8888: u32 = drm_fmt::DRM_FORMAT_ABGR8888;
const DRM_FORMAT_XBGR8888: u32 = drm_fmt::DRM_FORMAT_XBGR8888;

fn drm_format_to_flux(drm: u32) -> Option<flux::Format> {
    match drm {
        DRM_FORMAT_ARGB8888 | DRM_FORMAT_XRGB8888 => Some(flux::Format::FLUX_FORMAT_BGRA8_UNORM),
        DRM_FORMAT_ABGR8888 | DRM_FORMAT_XBGR8888 => Some(flux::Format::FLUX_FORMAT_RGBA8_UNORM),
        _ => None,
    }
}

/// Build the `(fourcc, modifiers)` set the compositor should advertise over
/// `zwp_linux_dmabuf_v1`, by querying the render device for the modifiers it
/// can both sample and import per fourcc.
///
/// Each advertised fourcc is paired with its real, device-supported modifier
/// list so clients allocate GPU-optimal (tiled/compressed) buffers. A format
/// whose modifiers the device cannot honor still degrades to the historical
/// `[DRM_FORMAT_MOD_LINEAR]` fallback rather than being dropped entirely, so
/// every client that previously worked keeps working.
///
/// Call this once at startup, after the flux device is created, and pass the
/// result to `aegis_compositor::Server::new_with_render_caps`.
pub fn formats_with_modifiers(device: &flux::Device) -> Vec<drm_fmt::DmabufFormat> {
    use drm_fmt::DmabufFormat;
    ADVERTISED_FOURCCS
        .iter()
        .map(|&fourcc| {
            let modifiers = drm_format_to_flux(fourcc)
                .filter(|_| flux::dmabuf_supported(device))
                .map(|fmt| {
                    let mut mods = flux::dmabuf_format_modifiers(device, fmt);
                    // Always include LINEAR: it is the universal fallback and
                    // keeps the format usable on devices/pathologies where no
                    // tiled modifier is sampleable. Dedup in case the device
                    // already reports it.
                    if !mods.contains(&drm_fmt::DRM_FORMAT_MOD_LINEAR) {
                        mods.push(drm_fmt::DRM_FORMAT_MOD_LINEAR);
                    }
                    mods
                })
                .unwrap_or_else(|| vec![drm_fmt::DRM_FORMAT_MOD_LINEAR]);
            DmabufFormat { fourcc, modifiers }
        })
        .collect()
}

/// The advertised fourccs in order, re-exported for callers that iterate the
/// format table (e.g. to wire it into the compositor's modifier feedback).
const ADVERTISED_FOURCCS: [u32; 4] = drm_fmt::ADVERTISED_FOURCCS;

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
    viewport_src: Option<aegis_core::Rect>,
    viewport_dst: Option<aegis_core::Size>,
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
    source: aegis_core::Rect,
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
/// they change. Whole-texture uploads happen on first sight, size changes, and
/// frames without usable damage; same-size frames with damage refresh only the
/// damage bounding box (mirroring the server's incremental snapshot copy).
/// Also tracks which (surface, format, modifier) tuples have already logged an
/// import failure so the diagnostic is emitted once per surface rather than
/// every frame.
pub struct Renderer {
    /// CPU-uploaded SHM textures have one current backing per surface.
    cache: HashMap<usize, CachedImage>,
    /// A Wayland dma-buf client normally cycles through a 2–4 buffer
    /// swapchain. Cache each backing buffer independently so A→B→C→A does
    /// not rebuild a VkImage on every commit.
    dmabuf_cache: HashMap<(usize, u64), CachedImage>,
    failed_imports: HashMap<usize, ()>,
    /// Images removed from the live cache stay alive beyond Flux's maximum
    /// three in-flight frame slots. Vulkan resources must not be destroyed
    /// while an older command buffer may still sample them.
    retired: Vec<(flux::Image, u64)>,
    frame_epoch: u64,
}

/// A cached texture. SHM entries are keyed by surface; dma-buf entries by
/// `(surface, stable buffer identity)`, so every member of a client swapchain
/// can retain its own imported VkImage.
///
/// The identity used for the skip is the backing `wl_buffer` (`buffer_id`),
/// not `generation` — the latter is bumped on every commit, even when the
/// client reuses the same wl_buffer, so it is useless for buffer-reuse
/// detection. The `modifier`, `width`, and `height` pin the layout too, so a
/// reallocation that changes the tile/compression mode forces a fresh import.
struct CachedImage {
    image: flux::Image,
    generation: u64,
    modifier: u64,
    width: u32,
    height: u32,
    last_used_epoch: u64,
}

const MAX_DMABUF_BUFFERS_PER_SURFACE: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OrderedSurfaceSource {
    Shm(usize),
    Dmabuf(usize),
}

type WindowMap<'a> =
    dyn Fn(Option<aegis_core::window::WindowId>, aegis_core::Rect) -> aegis_core::Rect + 'a;

#[derive(Default)]
struct OrderedSurfaceOptions<'a> {
    map: Option<&'a WindowMap<'a>>,
    window_shadows: Option<&'a [aegis_core::window::Window]>,
}

fn ordered_surface_sources(
    order: &[usize],
    shm_ids: &[usize],
    dmabuf_ids: &[usize],
) -> Vec<OrderedSurfaceSource> {
    let mut by_id = HashMap::with_capacity(shm_ids.len() + dmabuf_ids.len());
    for (index, id) in shm_ids.iter().copied().enumerate() {
        by_id.insert(id, OrderedSurfaceSource::Shm(index));
    }
    for (index, id) in dmabuf_ids.iter().copied().enumerate() {
        by_id.insert(id, OrderedSurfaceSource::Dmabuf(index));
    }

    let mut sources = Vec::with_capacity(by_id.len());
    for id in order {
        if let Some(source) = by_id.remove(id) {
            sources.push(source);
        }
    }

    // Frames absent from the authoritative order stay hidden. Appending them
    // in backing-type batches would silently invent a z-order and can expose
    // a lower window above a foreground window.
    sources
}

fn window_casts_resize_shadow(window: &aegis_core::window::Window) -> bool {
    !window.read_only
        && !window.minimized
        && !window.state.maximized
        && !window.state.fullscreen
        && window.layout_role == aegis_core::layout::LayoutRole::Floating
        && window.size.w > 0
        && window.size.h > 0
}

/// Paint a subtle four-logical-pixel compositor shadow immediately below a
/// floating window. Its visual extent is intentionally independent from the
/// larger direct-resize hit target.
fn draw_window_resize_shadow(canvas: &flux::Canvas, window: &aegis_core::window::Window) {
    const SHADOW_MARGIN: u32 = 4;

    let x = window.position.x as f32;
    let y = window.position.y as f32;
    let w = window.size.w as f32;
    let h = window.size.h as f32;
    let margin = SHADOW_MARGIN;
    for extent in (1..=margin).rev() {
        let inset = extent as f32 - 0.5;
        let distance_from_outer = margin - extent;
        let alpha = if window.state.activated {
            20 + distance_from_outer * 15
        } else {
            13 + distance_from_outer * 11
        };
        canvas.stroke_rrect(
            x - inset,
            y - inset,
            w + inset * 2.0,
            h + inset * 2.0,
            extent as f32 + 2.0,
            flux::rgba(0, 0, 0, alpha as u8),
            1.0,
        );
    }
}

impl Renderer {
    pub fn new() -> Renderer {
        Renderer {
            cache: HashMap::new(),
            dmabuf_cache: HashMap::new(),
            failed_imports: HashMap::new(),
            retired: Vec::new(),
            frame_epoch: 0,
        }
    }

    /// Advance GPU-resource retirement after `Surface::begin_frame` has
    /// waited the slot fence. Call exactly once for each rendered frame.
    pub fn begin_frame(&mut self) {
        self.frame_epoch = self.frame_epoch.wrapping_add(1);
        let epoch = self.frame_epoch;
        self.retired
            .retain(|(_, retired_at)| epoch.wrapping_sub(*retired_at) <= 4);
    }

    fn cache_image(&mut self, id: usize, entry: CachedImage) {
        if let Some(old) = self.cache.insert(id, entry) {
            self.retired.push((old.image, self.frame_epoch));
        }
    }

    fn retire_cached(&mut self, id: usize) {
        if let Some(entry) = self.cache.remove(&id) {
            self.retired.push((entry.image, self.frame_epoch));
        }
    }

    fn cache_dmabuf_image(&mut self, key: (usize, u64), entry: CachedImage) {
        if let Some(old) = self.dmabuf_cache.insert(key, entry) {
            self.retired.push((old.image, self.frame_epoch));
        }

        let surface_id = key.0;
        let mut keys = self
            .dmabuf_cache
            .iter()
            .filter_map(|(candidate, entry)| {
                (candidate.0 == surface_id && *candidate != key)
                    .then_some((*candidate, entry.last_used_epoch))
            })
            .collect::<Vec<_>>();
        let keep_others = MAX_DMABUF_BUFFERS_PER_SURFACE.saturating_sub(1);
        if keys.len() <= keep_others {
            return;
        }
        keys.sort_unstable_by_key(|(_, last_used)| *last_used);
        let evict = keys.len() - keep_others;
        for (old_key, _) in keys.into_iter().take(evict) {
            self.retire_dmabuf(old_key);
        }
    }

    fn retire_dmabuf(&mut self, key: (usize, u64)) {
        if let Some(entry) = self.dmabuf_cache.remove(&key) {
            self.retired.push((entry.image, self.frame_epoch));
        }
    }

    fn retire_dmabuf_surface(&mut self, id: usize) {
        let keys = self
            .dmabuf_cache
            .keys()
            .copied()
            .filter(|key| key.0 == id)
            .collect::<Vec<_>>();
        for key in keys {
            self.retire_dmabuf(key);
        }
    }

    /// Drop cached textures for surfaces no longer present. Call once per frame
    /// with every live surface id (shm and dma-buf).
    pub fn gc(&mut self, live_ids: impl Iterator<Item = usize>) {
        let live: std::collections::HashSet<usize> = live_ids.collect();
        let dead = self
            .cache
            .keys()
            .copied()
            .filter(|id| !live.contains(id))
            .collect::<Vec<_>>();
        for id in dead {
            if let Some(entry) = self.cache.remove(&id) {
                self.retired.push((entry.image, self.frame_epoch));
            }
        }
        let dead_dmabufs = self
            .dmabuf_cache
            .keys()
            .copied()
            .filter(|(id, _)| !live.contains(id))
            .collect::<Vec<_>>();
        for key in dead_dmabufs {
            self.retire_dmabuf(key);
        }
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
        self.draw_toplevels_impl(device, canvas, frames, None);
    }

    /// As [`draw_toplevels`](Self::draw_toplevels), but each frame's natural
    /// destination rect is passed through `map` before drawing — the
    /// overview's grid placement (M9). Window transitions do not apply in
    /// mapped mode; `map` fully owns placement.
    pub fn draw_toplevels_mapped(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        frames: &[SurfacePixels<'_>],
        map: &dyn Fn(Option<aegis_core::window::WindowId>, aegis_core::Rect) -> aegis_core::Rect,
    ) {
        self.draw_toplevels_impl(device, canvas, frames, Some(map));
    }

    /// Composite shm and dma-buf surfaces in one compositor-provided stacking
    /// order. The order may interleave toplevels, popups, and subsurfaces;
    /// painting any surface class or backing type in a separate global batch
    /// can violate window-tree occlusion.
    pub fn draw_surfaces_ordered(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        order: &[usize],
        shm: &[SurfacePixels<'_>],
        dmabuf: &[SurfaceDmabuf],
    ) {
        self.draw_surfaces_ordered_impl(
            device,
            canvas,
            order,
            shm,
            dmabuf,
            OrderedSurfaceOptions::default(),
        );
    }

    /// Ordered mixed-backing drawing with one compositor-owned resize shadow
    /// inserted beneath each floating window tree. The first ordered surface
    /// for a window may be a below-parent subsurface, so inserting here (not
    /// in a separate global pass) preserves both subtree and window z-order.
    pub fn draw_surfaces_ordered_with_window_shadows(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        order: &[usize],
        shm: &[SurfacePixels<'_>],
        dmabuf: &[SurfaceDmabuf],
        windows: &[aegis_core::window::Window],
    ) {
        self.draw_surfaces_ordered_impl(
            device,
            canvas,
            order,
            shm,
            dmabuf,
            OrderedSurfaceOptions {
                window_shadows: Some(windows),
                ..Default::default()
            },
        );
    }

    /// Ordered mixed-backing surface drawing with overview placement applied.
    pub fn draw_surfaces_ordered_mapped(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        order: &[usize],
        shm: &[SurfacePixels<'_>],
        dmabuf: &[SurfaceDmabuf],
        map: &dyn Fn(Option<aegis_core::window::WindowId>, aegis_core::Rect) -> aegis_core::Rect,
    ) {
        self.draw_surfaces_ordered_impl(
            device,
            canvas,
            order,
            shm,
            dmabuf,
            OrderedSurfaceOptions {
                map: Some(map),
                ..Default::default()
            },
        );
    }

    /// Compatibility entry point for callers that only supply xdg-role
    /// surfaces. New scene composition should use
    /// [`draw_surfaces_ordered`](Self::draw_surfaces_ordered).
    pub fn draw_toplevels_ordered(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        order: &[usize],
        shm: &[SurfacePixels<'_>],
        dmabuf: &[SurfaceDmabuf],
    ) {
        self.draw_surfaces_ordered(device, canvas, order, shm, dmabuf);
    }

    /// Compatibility entry point for mapped xdg-role-only drawing.
    pub fn draw_toplevels_ordered_mapped(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        order: &[usize],
        shm: &[SurfacePixels<'_>],
        dmabuf: &[SurfaceDmabuf],
        map: &dyn Fn(Option<aegis_core::window::WindowId>, aegis_core::Rect) -> aegis_core::Rect,
    ) {
        self.draw_surfaces_ordered_mapped(device, canvas, order, shm, dmabuf, map);
    }

    fn draw_surfaces_ordered_impl(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        order: &[usize],
        shm: &[SurfacePixels<'_>],
        dmabuf: &[SurfaceDmabuf],
        options: OrderedSurfaceOptions<'_>,
    ) {
        let map = options.map;
        let shm_ids = shm.iter().map(|frame| frame.id).collect::<Vec<_>>();
        let dmabuf_ids = dmabuf.iter().map(|frame| frame.id).collect::<Vec<_>>();
        let shadow_windows = options.window_shadows.map(|windows| {
            windows
                .iter()
                .map(|window| (window.id, window))
                .collect::<HashMap<_, _>>()
        });
        let mut shadowed = HashSet::new();
        for source in ordered_surface_sources(order, &shm_ids, &dmabuf_ids) {
            let window_id = match source {
                OrderedSurfaceSource::Shm(index) => shm[index].window,
                OrderedSurfaceSource::Dmabuf(index) => dmabuf[index].window,
            };
            if let Some(window_id) = window_id
                && shadowed.insert(window_id)
                && let Some(window) = shadow_windows
                    .as_ref()
                    .and_then(|windows| windows.get(&window_id))
                && window_casts_resize_shadow(window)
            {
                draw_window_resize_shadow(canvas, window);
            }
            match source {
                OrderedSurfaceSource::Shm(index) => {
                    self.draw_toplevels_impl(device, canvas, std::slice::from_ref(&shm[index]), map)
                }
                OrderedSurfaceSource::Dmabuf(index) => self.draw_dmabuf_toplevels_impl(
                    device,
                    canvas,
                    std::slice::from_ref(&dmabuf[index]),
                    map,
                ),
            }
        }
    }

    fn draw_toplevels_impl(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        frames: &[SurfacePixels<'_>],
        map: Option<
            &dyn Fn(Option<aegis_core::window::WindowId>, aegis_core::Rect) -> aegis_core::Rect,
        >,
    ) {
        for f in frames.iter() {
            self.retire_dmabuf_surface(f.id);
            // Upload gating has two layers:
            //
            // * WHEN to upload: only when the surface committed new
            //   contents — the server bumps the generation on every
            //   buffer attach — or when the cached texture's dimensions
            //   no longer match the frame's. Damage rects persist until
            //   the next commit, so they must never trigger an upload on
            //   their own (a static surface would otherwise re-upload
            //   every rendered frame).
            //
            // * HOW to upload (mirrors the server's incremental shm
            //   copy): a same-size commit with usable damage refreshes
            //   only the damage bounding box; anything else — first
            //   frame, resize, transform, buffer scale, or empty damage
            //   ("no information") — replaces the whole texture.
            let (tex_w, tex_h) = transformed_dims(f.width, f.height, f.geometry.transform);
            let dims_match = self
                .cache
                .get(&f.id)
                .is_some_and(|c| c.image.size() == (tex_w as u32, tex_h as u32));
            let new_contents = self
                .cache
                .get(&f.id)
                .is_none_or(|c| c.generation != f.generation);
            if !dims_match || new_contents {
                let incremental = dims_match
                    && f.geometry.transform == Transform::Normal
                    && f.geometry.buffer_scale <= 1
                    && !f.damage.is_empty();
                if incremental {
                    // Union of the (clamped) damage rects, uploaded in a
                    // single update_region. Pixels outside every damaged
                    // rect are identical to the previous frame by the
                    // damage protocol, so refreshing a bounding superset
                    // of the damage is always correct.
                    let mut x0 = i32::MAX;
                    let mut y0 = i32::MAX;
                    let mut x1 = i32::MIN;
                    let mut y1 = i32::MIN;
                    for d in f.damage {
                        let x = d.origin.x.max(0).min(f.width - 1);
                        let y = d.origin.y.max(0).min(f.height - 1);
                        let w = (d.size.w.max(0)).min(f.width - x);
                        let h = (d.size.h.max(0)).min(f.height - y);
                        if w == 0 || h == 0 {
                            continue;
                        }
                        x0 = x0.min(x);
                        y0 = y0.min(y);
                        x1 = x1.max(x + w);
                        y1 = y1.max(y + h);
                    }
                    if x0 < x1
                        && y0 < y1
                        && let Some(entry) = self.cache.get(&f.id)
                    {
                        let img = &entry.image;
                        let (bw, bh) = ((x1 - x0) as u32, (y1 - y0) as u32);
                        let updated = if x0 == 0
                            && y0 == 0
                            && bw == f.width as u32
                            && bh == f.height as u32
                        {
                            // Full-surface box: upload straight from the
                            // snapshot, no extraction copy.
                            img.update_region(0, 0, bw, bh, f.pixels)
                        } else {
                            // Tightly packed BGRA8: stride = width * 4.
                            let bpp = 4usize;
                            let stride = f.width as usize * bpp;
                            let mut sub = vec![0u8; bw as usize * bh as usize * bpp];
                            for row in 0..bh as usize {
                                let src_off = (y0 as usize + row) * stride + x0 as usize * bpp;
                                let dst_off = row * (bw as usize * bpp);
                                let len = bw as usize * bpp;
                                // src_off + len must stay within f.pixels.
                                let avail = f.pixels.len().saturating_sub(src_off);
                                let take = len.min(avail);
                                sub[dst_off..dst_off + take]
                                    .copy_from_slice(&f.pixels[src_off..src_off + take]);
                            }
                            img.update_region(x0 as u32, y0 as u32, bw, bh, &sub)
                        };
                        match updated {
                            Ok(()) => {
                                if let Some(cached) = self.cache.get_mut(&f.id) {
                                    cached.generation = f.generation;
                                }
                            }
                            Err(_) => {
                                // A partial refresh that failed leaves
                                // the texture a frame behind the
                                // snapshot with no guaranteed full
                                // upload coming. Retire the entry so
                                // the next frame re-uploads the whole
                                // (always accurate) snapshot instead
                                // of tearing indefinitely.
                                self.retire_cached(f.id);
                            }
                        }
                    }
                } else {
                    // Apply the buffer transform on the CPU at upload time.
                    // For the common case (Normal) this is a borrowed Cow
                    // with no allocation; axis-swapping rotations return an
                    // owned staging buffer. GPU-side transforms in flux's
                    // image shader are the long-term path.
                    let transformed = transform_pixels(
                        f.pixels,
                        f.width as usize,
                        f.height as usize,
                        f.geometry.transform,
                    );
                    if let Ok(img) = flux::Image::from_bytes(
                        device,
                        tex_w as u32,
                        tex_h as u32,
                        flux::Format::FLUX_FORMAT_BGRA8_UNORM,
                        &transformed,
                    ) {
                        self.cache_image(
                            f.id,
                            CachedImage {
                                image: img,
                                generation: f.generation,
                                modifier: 0,
                                width: f.width as u32,
                                height: f.height as u32,
                                last_used_epoch: self.frame_epoch,
                            },
                        );
                    }
                }
            }
            if let Some(entry) = self.cache.get(&f.id) {
                let img = &entry.image;
                let x = f.geometry.position.x as f32;
                let y = f.geometry.position.y as f32;
                // Apply wp_viewport and buffer_scale to compute the
                // destination rectangle. Viewport source coordinates are
                // after buffer transform *and scale*, so convert them back to
                // buffer-pixel UVs using buffer_scale.
                let (tw, th) = transformed_dims(f.width, f.height, f.geometry.transform);
                let (mut dst_w, mut dst_h) = destination_size(
                    (tw, th),
                    f.geometry.viewport_src,
                    f.geometry.viewport_dst,
                    f.geometry.buffer_scale,
                );
                // ADR-0029: an in-flight window transition draws the texture
                // scaled to the interpolated size instead of the
                // buffer-implied size. Mapped mode (overview) ignores it;
                // the map owns placement.
                if map.is_none()
                    && let Some(size) = f.geometry.transition_size
                {
                    dst_w = size.w as f32;
                    dst_h = size.h as f32;
                }
                if let Some(map) = map {
                    let natural = aegis_core::Rect::new(
                        x as i32,
                        y as i32,
                        dst_w.max(1.0) as i32,
                        dst_h.max(1.0) as i32,
                    );
                    let mapped = map(f.window, natural);
                    canvas.draw_image(
                        img,
                        mapped.origin.x as f32,
                        mapped.origin.y as f32,
                        mapped.size.w as f32,
                        mapped.size.h as f32,
                    );
                    continue;
                }
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
        self.draw_dmabuf_toplevels_impl(device, canvas, frames, None);
    }

    /// As [`draw_dmabuf_toplevels`](Self::draw_dmabuf_toplevels), but each
    /// frame's natural destination rect is passed through `map` first (the
    /// overview's grid placement, M9).
    pub fn draw_dmabuf_toplevels_mapped(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        frames: &[SurfaceDmabuf],
        map: &dyn Fn(Option<aegis_core::window::WindowId>, aegis_core::Rect) -> aegis_core::Rect,
    ) {
        self.draw_dmabuf_toplevels_impl(device, canvas, frames, Some(map));
    }

    fn draw_dmabuf_toplevels_impl(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        frames: &[SurfaceDmabuf],
        map: Option<
            &dyn Fn(Option<aegis_core::window::WindowId>, aegis_core::Rect) -> aegis_core::Rect,
        >,
    ) {
        for f in frames.iter() {
            self.retire_cached(f.id);
            let key = (f.id, f.buffer_id);
            let reusable = f.buffer_id != 0
                && self.dmabuf_cache.get(&key).is_some_and(|cached| {
                    cached.modifier == f.modifier
                        && cached.width == f.width as u32
                        && cached.height == f.height as u32
                });

            if reusable {
                // Explicit-sync fences describe each new producer commit, not
                // the lifetime of the backing buffer. Attach the new fence to
                // this frame's use of the existing VkImage; Flux waits it on
                // the GPU before the FOREIGN -> graphics ownership acquire.
                if f.acquire_fence >= 0 {
                    let fd = unsafe { libc::dup(f.acquire_fence) };
                    if fd < 0 {
                        if self.failed_imports.insert(f.id, ()).is_none() {
                            log::warn!(
                                "[render] failed to duplicate reusable dma-buf acquire fence fd {}",
                                f.acquire_fence
                            );
                        }
                        continue;
                    }
                    // SAFETY: dup returned a fresh descriptor owned by this
                    // scope and OwnedFd closes it unless Flux consumes it.
                    let fence = unsafe { OwnedFd::from_raw_fd(fd) };
                    let wait = {
                        let cached = self.dmabuf_cache.get(&key).expect("reusable cache entry");
                        canvas.wait_dmabuf_acquire(&cached.image, fence)
                    };
                    if let Err(e) = wait {
                        if self.failed_imports.insert(f.id, ()).is_none() {
                            log::warn!(
                                "[render] reusable dma-buf acquire wait failed ({e}): buffer={} fourcc={:#x} mod={:#x}",
                                f.buffer_id,
                                f.drm_format,
                                f.modifier,
                            );
                        }
                        continue;
                    }
                }
                if let Some(cached) = self.dmabuf_cache.get_mut(&key) {
                    cached.generation = f.generation;
                    cached.last_used_epoch = self.frame_epoch;
                }
                self.failed_imports.remove(&f.id);
            } else {
                self.retire_dmabuf(key);
                if let Some(fmt) = drm_format_to_flux(f.drm_format) {
                    // Flux consumes the descriptor fd on success. The frame's
                    // fd is borrowed from the server and must remain valid for
                    // later commits, so hand Flux a fresh duplicate.
                    let import_fd = unsafe { libc::dup(f.fd) };
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
                    let acquire_fence = if f.acquire_fence >= 0 {
                        let fd = unsafe { libc::dup(f.acquire_fence) };
                        if fd < 0 {
                            unsafe { libc::close(import_fd) };
                            if self.failed_imports.insert(f.id, ()).is_none() {
                                log::warn!(
                                    "[render] failed to duplicate acquire fence fd {}",
                                    f.acquire_fence
                                );
                            }
                            continue;
                        }
                        Some(fd)
                    } else {
                        None
                    };
                    let img = unsafe {
                        if let Some(acquire_fence) = acquire_fence {
                            flux::Image::import_dmabuf_with_acquire_fence(
                                device,
                                f.width as u32,
                                f.height as u32,
                                fmt,
                                f.modifier,
                                import_fd,
                                f.offset,
                                f.stride,
                                acquire_fence,
                            )
                        } else {
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
                        }
                    };
                    match img {
                        Ok(img) => {
                            if !self
                                .dmabuf_cache
                                .keys()
                                .any(|candidate| candidate.0 == f.id)
                            {
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
                            self.cache_dmabuf_image(
                                key,
                                CachedImage {
                                    image: img,
                                    generation: f.generation,
                                    modifier: f.modifier,
                                    width: f.width as u32,
                                    height: f.height as u32,
                                    last_used_epoch: self.frame_epoch,
                                },
                            );
                        }
                        Err(e) => {
                            // Flux leaves ownership with the caller on error.
                            unsafe { libc::close(import_fd) };
                            if let Some(acquire_fence) = acquire_fence {
                                unsafe { libc::close(acquire_fence) };
                            }
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
                } else {
                    self.retire_dmabuf(key);
                }
            }
            if let Some(entry) = self.dmabuf_cache.get(&key) {
                let img = &entry.image;
                let x = f.geometry.position.x as f32;
                let y = f.geometry.position.y as f32;
                // Destination size from viewport + buffer_scale, mirroring
                // the shm path. The dmabuf path does not CPU-stage the
                // buffer transform, so post-transform dims equal the raw
                // buffer dims here.
                let (mut dst_w, mut dst_h) = destination_size(
                    (f.width, f.height),
                    f.geometry.viewport_src,
                    f.geometry.viewport_dst,
                    f.geometry.buffer_scale,
                );
                // ADR-0029: an in-flight window transition draws the texture
                // scaled to the interpolated size instead of the
                // buffer-implied size. Mapped mode (overview) ignores it;
                // the map owns placement.
                if map.is_none()
                    && let Some(size) = f.geometry.transition_size
                {
                    dst_w = size.w as f32;
                    dst_h = size.h as f32;
                }
                if let Some(map) = map {
                    let natural = aegis_core::Rect::new(
                        x as i32,
                        y as i32,
                        dst_w.max(1.0) as i32,
                        dst_h.max(1.0) as i32,
                    );
                    let mapped = map(f.window, natural);
                    canvas.draw_image(
                        img,
                        mapped.origin.x as f32,
                        mapped.origin.y as f32,
                        mapped.size.w as f32,
                        mapped.size.h as f32,
                    );
                    continue;
                }
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

    /// As [`draw_subsurfaces`](Self::draw_subsurfaces), with the overview's
    /// grid mapping applied to every frame's natural rect (M9).
    pub fn draw_subsurfaces_mapped(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        frames: &[SurfacePixels<'_>],
        map: &dyn Fn(Option<aegis_core::window::WindowId>, aegis_core::Rect) -> aegis_core::Rect,
    ) {
        self.draw_toplevels_mapped(device, canvas, frames, map);
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

    /// As [`draw_dmabuf_subsurfaces`](Self::draw_dmabuf_subsurfaces), with
    /// the overview's grid mapping applied (M9).
    pub fn draw_dmabuf_subsurfaces_mapped(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        frames: &[SurfaceDmabuf],
        map: &dyn Fn(Option<aegis_core::window::WindowId>, aegis_core::Rect) -> aegis_core::Rect,
    ) {
        self.draw_dmabuf_toplevels_mapped(device, canvas, frames, map);
    }
}

/// Create a flux device suitable for compositing.
///
/// `headless` skips swapchain/presentation requirements — used for smoke tests
/// and any logic that never presents. A windowed backend passes `false` plus
/// the surface extensions it needs. Frame slots stay at the flux default (0);
/// hosts that present choose their own count (see `Host::create_device`).
pub fn create_device(
    headless: bool,
    instance_extensions: &[&std::ffi::CStr],
    device_extensions: &[&std::ffi::CStr],
) -> Result<flux::Device, flux::Error> {
    flux::Device::new(headless, instance_extensions, device_extensions, 0)
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
        use aegis_core::{Rect, Size};

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
        let src = aegis_core::Rect::new(10, 5, 30, 20);
        assert_eq!(viewport_uv(src, (200, 100), 2), (0.1, 0.1, 0.3, 0.4));
    }

    #[test]
    fn mixed_backing_sources_follow_compositor_order() {
        let sources = ordered_surface_sources(&[10, 20, 30], &[10, 30], &[20]);
        assert_eq!(
            sources,
            vec![
                OrderedSurfaceSource::Shm(0),
                OrderedSurfaceSource::Dmabuf(0),
                OrderedSurfaceSource::Shm(1),
            ]
        );
    }

    #[test]
    fn mixed_backing_window_chrome_does_not_escape_its_tree_order() {
        // Window A's dma-buf chrome must be drawn before the shm root of
        // foreground window B. A global dma-buf pass would reverse them.
        let sources = ordered_surface_sources(&[10, 11, 20, 21], &[10, 20], &[11, 21]);
        assert_eq!(
            sources,
            vec![
                OrderedSurfaceSource::Shm(0),
                OrderedSurfaceSource::Dmabuf(0),
                OrderedSurfaceSource::Shm(1),
                OrderedSurfaceSource::Dmabuf(1),
            ]
        );
    }

    #[test]
    fn a_frame_missing_from_the_authoritative_order_stays_hidden() {
        let sources = ordered_surface_sources(&[10], &[10, 20], &[]);
        assert_eq!(sources, vec![OrderedSurfaceSource::Shm(0)]);
    }

    #[test]
    fn resize_shadow_follows_direct_resize_eligibility() {
        let mut window = aegis_core::window::Window::new(aegis_core::window::WindowId(1));
        window.size = aegis_core::Size { w: 640, h: 480 };
        window.layout_role = aegis_core::layout::LayoutRole::Floating;
        assert!(window_casts_resize_shadow(&window));

        window.state.maximized = true;
        assert!(!window_casts_resize_shadow(&window));
        window.state.maximized = false;
        window.read_only = true;
        assert!(!window_casts_resize_shadow(&window));
        window.read_only = false;
        window.layout_role = aegis_core::layout::LayoutRole::Tiled;
        assert!(!window_casts_resize_shadow(&window));
    }
}
