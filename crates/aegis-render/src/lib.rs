//! Compositing for aegis, built on flux.
//!
//! Turns the surface tree into draw calls: each client buffer becomes a flux
//! texture (shm via CPU upload, dmabuf via zero-copy import), composited in
//! z-order into the output's frame.

/// Generic offscreen DAG planning from Optics, re-exported at the renderer
/// boundary so compositor code does not depend on the package topology.
pub use flux_composition_graph as composition_graph;

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};
use std::os::fd::BorrowedFd;

use aegis_model::{SurfaceDmabuf, SurfacePixels, Transform, dmabuf as drm_fmt};

/// Convenience aliases for the shared DRM fourccs in [`aegis_model::dmabuf`].
const DRM_FORMAT_ARGB8888: u32 = drm_fmt::DRM_FORMAT_ARGB8888;
const DRM_FORMAT_XRGB8888: u32 = drm_fmt::DRM_FORMAT_XRGB8888;
const DRM_FORMAT_ABGR8888: u32 = drm_fmt::DRM_FORMAT_ABGR8888;
const DRM_FORMAT_XBGR8888: u32 = drm_fmt::DRM_FORMAT_XBGR8888;
const DRM_FORMAT_ABGR2101010: u32 = drm_fmt::DRM_FORMAT_ABGR2101010;
const DRM_FORMAT_XBGR2101010: u32 = drm_fmt::DRM_FORMAT_XBGR2101010;

fn drm_format_to_flux(drm: u32) -> Option<flux::Format> {
    match drm {
        DRM_FORMAT_ARGB8888 | DRM_FORMAT_XRGB8888 => Some(flux::Format::Bgra8Unorm),
        DRM_FORMAT_ABGR8888 | DRM_FORMAT_XBGR8888 => Some(flux::Format::Rgba8Unorm),
        DRM_FORMAT_ABGR2101010 | DRM_FORMAT_XBGR2101010 => {
            Some(flux::Format::Rgb10a2Unorm)
        }
        _ => None,
    }
}

/// Map a `wp_color_management_v1` parametric image description onto a flux
/// color space. `None` when the pair is not representable (flux validates:
/// sane primaries, gamma > 0 for power curves).
fn flux_color_space(color: &aegis_model::color::ParametricColor) -> Option<flux::ColorSpace> {
    use aegis_model::color::{ContentPrimaries, ContentTransfer, NamedPrimaries, NamedTransfer};
    let (transfer, gamma) = match color.transfer {
        ContentTransfer::Named(NamedTransfer::Linear) => (flux::TransferFunction::Linear, 0.0),
        ContentTransfer::Named(NamedTransfer::Gamma22) => (flux::TransferFunction::Gamma, 2.2),
        ContentTransfer::Named(NamedTransfer::Srgb) => (flux::TransferFunction::Srgb, 0.0),
        ContentTransfer::Named(NamedTransfer::Pq) => (flux::TransferFunction::Pq, 0.0),
        ContentTransfer::Named(NamedTransfer::Hlg) => (flux::TransferFunction::Hlg, 0.0),
        ContentTransfer::Gamma(g) => (flux::TransferFunction::Gamma, g),
    };
    let space = match color.primaries {
        ContentPrimaries::Named(named) => {
            let primaries = match named {
                NamedPrimaries::Srgb => flux::ColorPrimaries::Bt709,
                NamedPrimaries::Bt2020 => flux::ColorPrimaries::Bt2020,
                NamedPrimaries::DisplayP3 => flux::ColorPrimaries::DisplayP3,
                NamedPrimaries::AdobeRgb => flux::ColorPrimaries::AdobeRgb,
            };
            flux::ColorSpace::new(primaries, transfer).with_gamma(gamma)
        }
        ContentPrimaries::Custom(xy) => flux::ColorSpace::custom(
            flux::PrimariesXy {
                rx: xy.rx,
                ry: xy.ry,
                gx: xy.gx,
                gy: xy.gy,
                bx: xy.bx,
                by: xy.by,
                wx: xy.wx,
                wy: xy.wy,
            },
            transfer,
        )
        .with_gamma(gamma),
    };
    space.is_valid().then_some(space)
}

fn prefer_native_modifiers(modifiers: &mut [u64]) {
    // `sort_by_key` is stable: preserve the driver's preference order inside
    // each class while moving the bandwidth-heavy linear fallback behind all
    // native tiled/compressed layouts.
    modifiers.sort_by_key(|modifier| u8::from(*modifier == drm_fmt::DRM_FORMAT_MOD_LINEAR));
}

/// Build the `(fourcc, modifiers)` set the compositor should advertise over
/// `zwp_linux_dmabuf_v1`, by querying the render device for the modifiers it
/// can both sample and import per fourcc.
///
/// Each advertised fourcc is paired only with modifiers Flux proves both
/// sampleable and externally importable on the selected device. Unsupported
/// formats are omitted; in particular, LINEAR is never synthesized when the
/// driver does not report it.
///
/// Call this once at startup, after the flux device is created, and pass the
/// result to `aegis_compositor::Server::new_with_render_caps`.
pub fn formats_with_modifiers(device: &flux::Device) -> Vec<drm_fmt::DmabufFormat> {
    use drm_fmt::DmabufFormat;
    if !flux::dmabuf_supported(device) {
        return Vec::new();
    }
    ADVERTISED_FOURCCS
        .iter()
        .filter_map(|&fourcc| {
            let format = drm_format_to_flux(fourcc)?;
            let mut modifiers = flux::dmabuf_format_modifiers(device, format);
            if modifiers.is_empty() {
                return None;
            }
            // Vulkan does not promise a preference order for format
            // modifiers. Wayland clients generally choose from the leading
            // preference tranche, so keep native tiled/compressed layouts
            // ahead of a driver-confirmed LINEAR option.
            prefer_native_modifiers(&mut modifiers);
            Some(DmabufFormat { fourcc, modifiers })
        })
        .collect()
}

/// The advertised fourccs in order, re-exported for callers that iterate the
/// format table (e.g. to wire it into the compositor's modifier feedback).
const ADVERTISED_FOURCCS: [u32; 6] = drm_fmt::ADVERTISED_FOURCCS;

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
    viewport_src: Option<aegis_model::Rect>,
    viewport_dst: Option<aegis_model::Size>,
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
    source: aegis_model::Rect,
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

/// Number of horizontal slices the genie minimize warp draws per frame:
/// enough that the pinch curve reads as smooth, few enough to stay
/// negligible next to the texture upload the flight already pays for.
const GENIE_STRIPS: usize = 24;

/// A sub-rectangle of the destination blit in framebuffer pixels, paired with
/// the matching normalised source-UV box so a single texture can be sampled
/// into an arbitrary destination rectangle.
#[derive(Clone, Copy, Debug, PartialEq)]
struct BlitRect {
    dst_x: f32,
    dst_y: f32,
    dst_w: f32,
    dst_h: f32,
    src_u: f32,
    src_v: f32,
    src_du: f32,
    src_dv: f32,
}

/// Horizontal-slice blits for the genie minimize warp (ADR-0029
/// `TransitionEffect::Minimize`, genie style). The destination rect is cut
/// into horizontal strips and each strip is pinched horizontally toward the
/// dock icon's centre by `progress * strip_v`, so the lower edge funnels
/// into the icon first while the top edge follows. The window's overall rect
/// still interpolates toward the icon; `progress` only shapes the funnel.
/// Source UVs span the full texture; callers with a `wp_viewport` crop remap
/// them into the cropped region.
fn genie_strips(
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    target: aegis_model::Point,
    progress: f32,
) -> Vec<BlitRect> {
    let n = GENIE_STRIPS as f32;
    let cx = x + w * 0.5;
    let tx = target.x as f32;
    (0..GENIE_STRIPS)
        .map(|i| {
            let v0 = i as f32 / n;
            let v1 = (i + 1) as f32 / n;
            let strip_v = (i as f32 + 0.5) / n;
            let pinch = (progress * strip_v).clamp(0.0, 1.0);
            let half = w * 0.5 * (1.0 - pinch);
            let centre = cx + (tx - cx) * pinch;
            BlitRect {
                dst_x: centre - half,
                dst_y: y + h * v0,
                dst_w: (half * 2.0).max(0.0),
                dst_h: h * (v1 - v0),
                src_u: 0.0,
                src_v: v0,
                src_du: 1.0,
                src_dv: v1 - v0,
            }
        })
        .collect()
}

/// Split a full-surface blit into opaque and blended destination pieces using
/// the client's `opaque_region` (surface-local logical coordinates).
///
/// When the region is usable — the surface has no buffer transform and no
/// `wp_viewport` source-crop, the two cases where surface-local logical
/// coordinates do not map 1:1 onto destination pixels — every pixel under an
/// opaque rect is drawn with SRC-replace (no framebuffer readback) and only
/// the genuinely translucent remainder is composited source-over. Each
/// destination pixel is blitted exactly once: the non-opaque remainder is the
/// set difference of the whole destination rect and the opaque rectangles.
///
/// `surface_logical` is the unscaled surface size used to turn surface-local
/// coordinates into normalised UVs; it equals `dst` when scale/viewport are
/// identity. An opaque rect that falls outside the surface is clipped to it;
/// degenerate rects are dropped.
fn split_opaque_blit(
    dst: (f32, f32, f32, f32),
    surface_logical: (f32, f32),
    opaque_region: Option<&[aegis_model::Rect]>,
    can_split: bool,
) -> (Vec<BlitRect>, Vec<BlitRect>) {
    let (dx, dy, dw, dh) = dst;
    let (sw, sh) = surface_logical;
    if !can_split || sw < 1.0 || sh < 1.0 || dw < 1.0 || dh < 1.0 {
        return (
            Vec::new(),
            vec![BlitRect {
                dst_x: dx,
                dst_y: dy,
                dst_w: dw,
                dst_h: dh,
                src_u: 0.0,
                src_v: 0.0,
                src_du: 1.0,
                src_dv: 1.0,
            }],
        );
    }
    let region = match opaque_region {
        Some(r) if !r.is_empty() => r,
        _ => {
            return (
                Vec::new(),
                vec![BlitRect {
                    dst_x: dx,
                    dst_y: dy,
                    dst_w: dw,
                    dst_h: dh,
                    src_u: 0.0,
                    src_v: 0.0,
                    src_du: 1.0,
                    src_dv: 1.0,
                }],
            );
        }
    };
    // Opaque regions use integer surface-local logical coordinates. Avoid
    // guessing when the computed surface extent is fractional: drawing the
    // complete surface source-over is always correct, whereas rounding the
    // region domain can duplicate or omit source texels.
    let logical_w = sw.round() as i32;
    let logical_h = sh.round() as i32;
    if (sw - logical_w as f32).abs() > f32::EPSILON
        || (sh - logical_h as f32).abs() > f32::EPSILON
        || logical_w < 1
        || logical_h < 1
    {
        return (
            Vec::new(),
            vec![BlitRect {
                dst_x: dx,
                dst_y: dy,
                dst_w: dw,
                dst_h: dh,
                src_u: 0.0,
                src_v: 0.0,
                src_du: 1.0,
                src_dv: 1.0,
            }],
        );
    }

    let whole_logical = aegis_model::Rect::new(0, 0, logical_w, logical_h);

    // Build a non-overlapping union of the opaque region in *surface logical*
    // coordinates. This is the coordinate space used by wl_surface's
    // opaque_region. Keeping both the union and the remainder in this domain
    // is essential at buffer_scale > 1: buffer pixels and surface pixels are
    // not interchangeable.
    let mut opaque_logical: Vec<aegis_model::Rect> = Vec::with_capacity(region.len());
    for r in region {
        let cx0 = r.origin.x.max(0).min(logical_w);
        let cy0 = r.origin.y.max(0).min(logical_h);
        // Regions are client supplied. Saturate the far edge before clipping
        // so an extreme origin/extent cannot wrap (or panic in debug builds)
        // before it reaches the surface bounds.
        let cx1 = r.origin.x.saturating_add(r.size.w).max(0).min(logical_w);
        let cy1 = r.origin.y.saturating_add(r.size.h).max(0).min(logical_h);
        if cx1 <= cx0 || cy1 <= cy0 {
            continue;
        }
        let clipped = aegis_model::Rect::new(cx0, cy0, cx1 - cx0, cy1 - cy0);
        let mut additions = vec![clipped];
        for existing in &opaque_logical {
            let mut next = Vec::new();
            for piece in additions.drain(..) {
                next.extend(piece.subtract(*existing));
            }
            additions = next;
        }
        opaque_logical.extend(additions);
    }
    if opaque_logical.is_empty() {
        return (
            Vec::new(),
            vec![BlitRect {
                dst_x: dx,
                dst_y: dy,
                dst_w: dw,
                dst_h: dh,
                src_u: 0.0,
                src_v: 0.0,
                src_du: 1.0,
                src_dv: 1.0,
            }],
        );
    }
    // Remainder = whole logical surface minus the opaque union. Mapping both
    // sets through the same normalised transform makes them cover the
    // destination exactly once, including transitions that rescale `dst`.
    let mut remainder = vec![whole_logical];
    for o in &opaque_logical {
        let mut next = Vec::new();
        for piece in remainder.drain(..) {
            next.extend(piece.subtract(*o));
        }
        remainder = next;
    }

    let map_piece = |p: aegis_model::Rect| {
        let u = p.origin.x as f32 / sw;
        let v = p.origin.y as f32 / sh;
        let du = p.size.w as f32 / sw;
        let dv = p.size.h as f32 / sh;
        BlitRect {
            dst_x: dx + u * dw,
            dst_y: dy + v * dh,
            dst_w: du * dw,
            dst_h: dv * dh,
            src_u: u,
            src_v: v,
            src_du: du,
            src_dv: dv,
        }
    };
    let opaque = opaque_logical.into_iter().map(map_piece).collect();
    let blended = remainder.into_iter().map(map_piece).collect();
    (opaque, blended)
}

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
    /// Scratch buffers reused across [`Renderer::gc`] calls so the live-id
    /// set and dead-id lists are never freshly heap-allocated each frame.
    gc_live: std::collections::HashSet<usize>,
    gc_dead_shm: Vec<usize>,
    gc_dead_dmabuf: Vec<(usize, u64)>,
    /// Scratch staging buffer for incremental SHM damage uploads, grown to
    /// the session's high-water damage-box size and reused across frames so
    /// the partial-refresh path does not allocate (and zero) per frame.
    /// Mirrors the `gc_*` scratch-buffer policy above.
    shm_staging: Vec<u8>,
    /// Parsed ICC profiles (flux) keyed by a hash of the profile bytes, so a
    /// color-tagged surface does not re-parse per texture creation. The
    /// value's second field is the frame epoch of the last lookup (LRU).
    ///
    /// Bounded: profile blobs are client-supplied (up to the protocol's
    /// 16 MiB cap each), so an unbounded map is a remote memory-exhaustion
    /// vector. Eviction only costs a re-parse on the next texture
    /// recreation; an already-imported texture keeps its baked color tag.
    icc_profiles: HashMap<u64, (flux::IccProfile, u64)>,
    /// ICC byte-hashes that failed to parse; suppresses per-frame retries
    /// and log floods (the content falls back to sRGB interpretation).
    /// Cleared wholesale when it exceeds [`MAX_ICC_FAILED`] — the worst case
    /// is one repeat warn log per distinct blob.
    icc_failed: std::collections::HashSet<u64>,
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
/// `color` pins the color-space tag the texture was created with: a tag
/// change on the same buffer forces a fresh upload/import.
struct CachedImage {
    image: flux::Image,
    generation: u64,
    modifier: u64,
    width: u32,
    height: u32,
    color: Option<aegis_model::color::ContentColor>,
    last_used_epoch: u64,
}

const MAX_DMABUF_BUFFERS_PER_SURFACE: usize = 8;

/// Parsed-ICC-profile cache ceiling. Profiles are client-supplied blobs (up
/// to 16 MiB each through `wp_color_management_v1`), so the cache must be
/// bounded even though hits are far more common than distinct profiles: a
/// desktop realistically presents a handful (per-monitor EDID-derived tags).
/// Eviction only costs a re-parse at the next texture recreation.
const MAX_ICC_PROFILES: usize = 32;

/// Ceiling for remembered ICC parse failures. Each entry is a u64 hash, so
/// the set is small in absolute terms; the cap exists so a hostile client
/// cannot grow it without bound by streaming garbage blobs. On overflow the
/// set clears wholesale — the worst case is one repeated warn log.
const MAX_ICC_FAILED: usize = 256;

/// A sync_file belongs to one producer commit, not to every draw of the same
/// imported image. Backdrop capture and final output may reference one buffer
/// several times; only a new surface generation needs another acquire wait.
fn reusable_acquire_wait_required(
    cached_generation: u64,
    incoming_generation: u64,
    acquire_fence: i32,
) -> bool {
    acquire_fence >= 0 && cached_generation != incoming_generation
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OrderedSurfaceSource {
    Shm(usize),
    Dmabuf(usize),
}

type WindowMap<'a> =
    dyn Fn(Option<aegis_model::window::WindowId>, aegis_model::Rect) -> aegis_model::Rect + 'a;

/// Appearance applied to every image in one mapped surface subtree.
///
/// `rounded_clip` is independent of each surface destination. Reusing it for
/// the toplevel, subsurfaces, and popups produces one preview silhouette.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MappedSurfaceStyle {
    pub opacity: f32,
    pub brightness: f32,
    pub rounded_clip: aegis_model::Rect,
    pub corner_radius: f32,
}

#[derive(Default)]
struct OrderedSurfaceOptions<'a> {
    map: Option<&'a WindowMap<'a>>,
    window_shadows: Option<&'a [aegis_model::window::Window]>,
    window_filter: Option<&'a HashSet<aegis_model::window::WindowId>>,
    mapped_style: Option<MappedSurfaceStyle>,
    /// Per-window blurred shadow images, pre-rendered by the composition
    /// root through the Optics shadow operator (ADR-0139). Keyed by window
    /// id; each entry carries the physical-pixel rect the image covers so
    /// this layer can place it under the window tree without owning any
    /// effect state.
    soft_shadows: Option<&'a SoftShadowLayer<'a>>,
    /// Policy selecting which shadow style to draw (ADR-0139 data).
    shadow_style: aegis_model::window::WindowShadowStyle,
}

/// Pre-rendered per-window shadow images plus their placement rects,
/// produced at a pass boundary by the composition root and consumed here.
pub struct SoftShadowLayer<'a> {
    /// (window id, image, physical-space placement) triples in draw order.
    pub entries: &'a [SoftShadowEntry<'a>],
}

/// One pre-rendered blurred shadow and where it lands, in physical output
/// pixels. The image is a borrowed Optics effect output (premultiplied);
/// drawing it straight composites correctly.
pub struct SoftShadowEntry<'a> {
    pub window: aegis_model::window::WindowId,
    /// Raw `flux_image` borrowed from the composition root's shadow filter
    /// for this frame slot (ADR-0074 frame-slot lifetime). The renderer only
    /// reads it inside this frame's canvas passes.
    pub raw: *mut flux::sys::flux_image,
    /// Marker tying the borrow to the layer's lifetime without taking a
    /// reference the filter owns (the filter is re-applied only on the next
    /// frame rotation, after this frame submits).
    pub _borrow: std::marker::PhantomData<&'a flux::Image>,
    /// Placement in physical pixels, already accounting for the blur margin.
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

fn mapped_surface_modulation(style: MappedSurfaceStyle) -> (u8, u8) {
    let opacity = if style.opacity.is_finite() {
        style.opacity.clamp(0.0, 1.0)
    } else {
        1.0
    };
    let brightness = if style.brightness.is_finite() {
        style.brightness.clamp(0.0, 1.0)
    } else {
        1.0
    };
    (
        (brightness * 255.0).round() as u8,
        (opacity * 255.0).round() as u8,
    )
}

/// Window metadata and membership for one independently composited workspace
/// page. Keeping them together prevents callers from accidentally filtering
/// surfaces while sourcing shadows from a different scene.
pub struct WorkspaceSurfaceLayer<'a> {
    windows: &'a [aegis_model::window::Window],
    window_filter: &'a HashSet<aegis_model::window::WindowId>,
}

impl<'a> WorkspaceSurfaceLayer<'a> {
    pub fn new(
        windows: &'a [aegis_model::window::Window],
        window_filter: &'a HashSet<aegis_model::window::WindowId>,
    ) -> Self {
        Self {
            windows,
            window_filter,
        }
    }
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

fn surface_passes_window_filter(
    window: Option<aegis_model::window::WindowId>,
    filter: Option<&HashSet<aegis_model::window::WindowId>>,
) -> bool {
    filter.is_none_or(|filter| window.is_some_and(|window| filter.contains(&window)))
}

/// Whether this window gets a compositor-owned shadow: floating, mapped,
/// interactive, not maximized/fullscreen, with a non-empty extent. Shared
/// by the inline stroke path and the Optics blurred-shadow path (ADR-0139).
pub fn window_casts_shadow(window: &aegis_model::window::Window) -> bool {
    window_casts_resize_shadow(window)
}

fn window_casts_resize_shadow(window: &aegis_model::window::Window) -> bool {
    !window.read_only
        && !window.minimized
        && !window.state.maximized
        && !window.state.fullscreen
        && window.layout_role == aegis_model::layout::LayoutRole::Floating
        && window.size.w > 0
        && window.size.h > 0
}

/// Paint a subtle four-logical-pixel compositor shadow immediately below a
/// floating window. Its visual extent is intentionally independent from the
/// larger direct-resize hit target.
fn draw_window_resize_shadow(canvas: &flux::Canvas, window: &aegis_model::window::Window) {
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
            gc_live: std::collections::HashSet::new(),
            gc_dead_shm: Vec::new(),
            gc_dead_dmabuf: Vec::new(),
            shm_staging: Vec::new(),
            icc_profiles: HashMap::new(),
            icc_failed: std::collections::HashSet::new(),
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

    /// Build the flux image color tag for a surface's content description
    /// (`wp_color_management_v1`), parsing and caching ICC profiles on
    /// demand. An empty/default tag selects the format-derived color space
    /// (sRGB for 8-bit content).
    fn image_color_tag(
        &mut self,
        color: Option<&aegis_model::color::ContentColor>,
    ) -> flux::ImageColorSpace<'_> {
        match color {
            Some(aegis_model::color::ContentColor::Parametric(parametric)) => {
                flux::ImageColorSpace {
                    space: flux_color_space(parametric),
                    icc: None,
                }
            }
            Some(aegis_model::color::ContentColor::Icc(bytes)) => flux::ImageColorSpace {
                space: None,
                icc: self.icc_profile_for(bytes),
            },
            None => flux::ImageColorSpace::default(),
        }
    }

    fn icc_profile_for(&mut self, bytes: &[u8]) -> Option<&flux::IccProfile> {
        use std::hash::{Hash, Hasher};
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        bytes.hash(&mut hasher);
        let key = hasher.finish();
        if self.icc_profiles.contains_key(&key) {
            if let Some(entry) = self.icc_profiles.get_mut(&key) {
                entry.1 = self.frame_epoch;
            }
            return self.icc_profiles.get(&key).map(|(profile, _)| profile);
        }
        if self.icc_failed.contains(&key) {
            return None;
        }
        let profile = match flux::IccProfile::new(bytes) {
            Ok(profile) => profile,
            Err(error) => {
                log::warn!("[render] ICC profile parse failed ({error}); content renders as sRGB");
                self.icc_failed.insert(key);
                if self.icc_failed.len() > MAX_ICC_FAILED {
                    self.icc_failed.clear();
                }
                return None;
            }
        };
        if self.icc_profiles.len() >= MAX_ICC_PROFILES {
            // Evict the least-recently-used profile. Profiles are only
            // consulted at texture (re)creation, so a live texture is never
            // affected; a re-tagged surface simply re-parses its blob.
            if let Some(oldest) = self
                .icc_profiles
                .iter()
                .min_by_key(|(_, (_, last_used))| *last_used)
                .map(|(hash, _)| *hash)
            {
                self.icc_profiles.remove(&oldest);
            }
        }
        self.icc_profiles.insert(key, (profile, self.frame_epoch));
        self.icc_profiles.get(&key).map(|(profile, _)| profile)
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

    /// Immediately evict all cached textures (SHM and DMA-BUF) for a surface
    /// that has been destroyed, releasing GPU resources without waiting for GC.
    pub fn evict_surface(&mut self, surface_id: usize) {
        self.retire_cached(surface_id);
        self.retire_dmabuf_surface(surface_id);
        self.failed_imports.remove(&surface_id);
    }

    /// Drop cached textures for surfaces no longer present. Call once per frame
    /// with every live surface id (shm and dma-buf). Reuses internal scratch
    /// buffers so no set or list is heap-allocated on the steady-state path.
    pub fn gc(&mut self, live_ids: impl Iterator<Item = usize>) {
        self.gc_live.clear();
        self.gc_live.extend(live_ids);
        // Collect dead ids into the scratch Vec, then move it out so the loop
        // body can mutate the cache/dmabuf_cache without holding a borrow on
        // the scratch field.
        self.gc_dead_shm.clear();
        self.gc_dead_shm.extend(
            self.cache
                .keys()
                .copied()
                .filter(|id| !self.gc_live.contains(id)),
        );
        let dead_shm = std::mem::take(&mut self.gc_dead_shm);
        for id in &dead_shm {
            if let Some(entry) = self.cache.remove(id) {
                self.retired.push((entry.image, self.frame_epoch));
            }
        }
        self.gc_dead_shm = dead_shm;

        self.gc_dead_dmabuf.clear();
        self.gc_dead_dmabuf.extend(
            self.dmabuf_cache
                .keys()
                .copied()
                .filter(|(id, _)| !self.gc_live.contains(id)),
        );
        let dead_dmabuf = std::mem::take(&mut self.gc_dead_dmabuf);
        for key in &dead_dmabuf {
            self.retire_dmabuf(*key);
        }
        self.gc_dead_dmabuf = dead_dmabuf;

        self.failed_imports.retain(|k, _| self.gc_live.contains(k));
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
        self.draw_toplevels_impl(device, canvas, frames, None, None);
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
        map: &dyn Fn(Option<aegis_model::window::WindowId>, aegis_model::Rect) -> aegis_model::Rect,
    ) {
        self.draw_toplevels_impl(device, canvas, frames, Some(map), None);
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

    /// Ordered mixed-backing drawing with one compositor-owned shadow
    /// inserted beneath each floating window tree. The first ordered surface
    /// for a window may be a below-parent subsurface, so inserting here (not
    /// in a separate global pass) preserves both subtree and window z-order.
    /// The style selects between the inline stroke shadow, the pre-rendered
    /// Optics blurred shadow (`soft_shadows`), and none (ADR-0139).
    #[allow(clippy::too_many_arguments)]
    pub fn draw_surfaces_ordered_with_window_shadows(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        order: &[usize],
        shm: &[SurfacePixels<'_>],
        dmabuf: &[SurfaceDmabuf],
        windows: &[aegis_model::window::Window],
        soft_shadows: Option<&SoftShadowLayer<'_>>,
        shadow_style: aegis_model::window::WindowShadowStyle,
    ) {
        self.draw_surfaces_ordered_impl(
            device,
            canvas,
            order,
            shm,
            dmabuf,
            OrderedSurfaceOptions {
                window_shadows: Some(windows),
                soft_shadows,
                shadow_style,
                ..Default::default()
            },
        );
    }

    /// Draw one workspace as an independent scene layer. The compositor may
    /// collect surfaces from several workspaces for an animated transition,
    /// but each call retains only this page's complete window trees and their
    /// internal mixed-backing Z-order.
    pub fn draw_workspace_surface_layer(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        order: &[usize],
        shm: &[SurfacePixels<'_>],
        dmabuf: &[SurfaceDmabuf],
        layer: WorkspaceSurfaceLayer<'_>,
    ) {
        self.draw_surfaces_ordered_impl(
            device,
            canvas,
            order,
            shm,
            dmabuf,
            OrderedSurfaceOptions {
                window_shadows: Some(layer.windows),
                window_filter: Some(layer.window_filter),
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
        map: &dyn Fn(Option<aegis_model::window::WindowId>, aegis_model::Rect) -> aegis_model::Rect,
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

    /// Ordered mapped drawing with one shared appearance for every remapped
    /// surface. This is used by transient scene previews whose complete
    /// surface tree needs one rounded silhouette and color-preserving focus.
    #[allow(clippy::too_many_arguments)]
    pub fn draw_surfaces_ordered_mapped_with_style(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        order: &[usize],
        shm: &[SurfacePixels<'_>],
        dmabuf: &[SurfaceDmabuf],
        map: &dyn Fn(Option<aegis_model::window::WindowId>, aegis_model::Rect) -> aegis_model::Rect,
        style: MappedSurfaceStyle,
    ) {
        self.draw_surfaces_ordered_impl(
            device,
            canvas,
            order,
            shm,
            dmabuf,
            OrderedSurfaceOptions {
                map: Some(map),
                mapped_style: Some(style),
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
        map: &dyn Fn(Option<aegis_model::window::WindowId>, aegis_model::Rect) -> aegis_model::Rect,
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
        let mapped_style = options.mapped_style;
        // One upload batch per composite: every SHM texture refresh and
        // recreation recorded in this pass reaches the queue through a
        // single vkQueueSubmit instead of one submit (plus command-pool
        // recycle) per surface. See `Device::uploads_begin`.
        let _uploads = device.uploads_begin();
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
            if !surface_passes_window_filter(window_id, options.window_filter) {
                continue;
            }
            if let Some(window_id) = window_id
                && shadowed.insert(window_id)
                && let Some(window) = shadow_windows
                    .as_ref()
                    .and_then(|windows| windows.get(&window_id))
            {
                match options.shadow_style {
                    aegis_model::window::WindowShadowStyle::None => {}
                    aegis_model::window::WindowShadowStyle::Resize => {
                        if window_casts_resize_shadow(window) {
                            draw_window_resize_shadow(canvas, window);
                        }
                    }
                    aegis_model::window::WindowShadowStyle::Soft => {
                        if window_casts_resize_shadow(window)
                            && let Some(entry) = options.soft_shadows.and_then(|layer| {
                                layer.entries.iter().find(|entry| entry.window == window_id)
                            })
                        {
                            // SAFETY: the entry's raw pointer is the shadow
                            // filter's slot output for this frame; the
                            // composition root guarantees it stays valid
                            // through this frame's passes (ADR-0074
                            // frame-slot lifetime). One draw call, read-only,
                            // inside this canvas pass.
                            let destination = flux::sys::flux_rect {
                                x: entry.x,
                                y: entry.y,
                                w: entry.w,
                                h: entry.h,
                            };
                            unsafe {
                                flux::sys::flux_canvas_draw_image(
                                    canvas.as_raw(),
                                    entry.raw,
                                    destination,
                                    std::ptr::null(),
                                )
                            };
                        }
                    }
                }
            }
            match source {
                OrderedSurfaceSource::Shm(index) => self.draw_toplevels_impl(
                    device,
                    canvas,
                    std::slice::from_ref(&shm[index]),
                    map,
                    mapped_style,
                ),
                OrderedSurfaceSource::Dmabuf(index) => self.draw_dmabuf_toplevels_impl(
                    device,
                    canvas,
                    std::slice::from_ref(&dmabuf[index]),
                    map,
                    mapped_style,
                ),
            }
        }
    }

    /// Draw one frame of every closing-window ghost (ADR-0029 close
    /// transition). Ghosts have no protocol identity left — the model window
    /// is gone — so each view carries its own never-reused id, its
    /// interpolated rect, and its fading opacity. The texture is cached under
    /// that id and dies with the ghost's cache retirement below.
    pub fn draw_closing_frames(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        ghosts: &[aegis_model::ClosingGhostView],
    ) {
        // One upload batch for all ghost uploads; see
        // `draw_surfaces_ordered_impl`.
        let _uploads = device.uploads_begin();
        for ghost in ghosts {
            let alpha = (ghost.opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
            if alpha == 0 {
                continue;
            }
            let paint = flux::Paint::solid(flux::rgba(255, 255, 255, alpha));
            if ghost.dmabuf_fd >= 0
                && let Some(fmt) = drm_format_to_flux(ghost.drm_format)
            {
                let key = (ghost.id, ghost.dmabuf_fd as u64);
                if !self.dmabuf_cache.contains_key(&key) {
                    // Flux owns and closes the plane fd on a successful
                    // import (see flux/dmabuf.h). The ghost's fd is owned by
                    // its `DmabufBuffer`, which still closes it when the
                    // closing frame settles — handing flux that same
                    // descriptor would double-close it and could take an
                    // unrelated fd down with it. Hand flux a fresh duplicate,
                    // exactly as the live path does. The duplicate is an
                    // `OwnedFd` from the start: a failed import closes it via
                    // its own drop, and success hands it to flux.
                    let import_fd = unsafe { BorrowedFd::borrow_raw(ghost.dmabuf_fd) }
                        .try_clone_to_owned();
                    let import_fd = match import_fd {
                        Ok(fd) => fd,
                        Err(_) => continue,
                    };
                    // SAFETY: the duplicate descriptor and the ghost's layout
                    // describe the snapshotted dma-buf exactly as the live
                    // path did. The import takes the `OwnedFd` by value, so
                    // failure closes the duplicate via its own drop — no
                    // manual `libc::close` needed anymore.
                    let imported = unsafe {
                        let tag = self.image_color_tag(ghost.color.as_ref());
                        flux::Image::import_dmabuf_with_color_space(
                            device,
                            ghost.buffer_width as u32,
                            ghost.buffer_height as u32,
                            fmt,
                            ghost.modifier,
                            import_fd,
                            ghost.offset,
                            ghost.stride,
                            None,
                            tag,
                        )
                    };
                    match imported {
                        Ok(img) => {
                            self.cache_dmabuf_image(
                                key,
                                CachedImage {
                                    image: img,
                                    generation: 0,
                                    modifier: ghost.modifier,
                                    width: ghost.buffer_width as u32,
                                    height: ghost.buffer_height as u32,
                                    color: ghost.color.clone(),
                                    last_used_epoch: self.frame_epoch,
                                },
                            );
                        }
                        Err(_) => continue,
                    }
                }
                if let Some(entry) = self.dmabuf_cache.get(&key) {
                    let img = &entry.image;
                    // Scale the whole buffer into the interpolated ghost
                    // rect: the logical draw origin plus the scale factor
                    // from logical size to rect keeps CSD insets intact.
                    canvas.draw_image_with_paint(
                        img,
                        ghost.rect.origin.x as f32,
                        ghost.rect.origin.y as f32,
                        ghost.rect.size.w as f32,
                        ghost.rect.size.h as f32,
                        &paint,
                    );
                }
            } else if !ghost.pixels.is_empty() && !self.cache.contains_key(&ghost.id) {
                let uploaded = {
                    let tag = self.image_color_tag(ghost.color.as_ref());
                    flux::Image::from_bytes_with_color_space(
                        device,
                        ghost.buffer_width as u32,
                        ghost.buffer_height as u32,
                        flux::Format::Bgra8Unorm,
                        ghost.pixels,
                        tag,
                    )
                };
                if let Ok(img) = uploaded {
                    self.cache_image(
                        ghost.id,
                        CachedImage {
                            image: img,
                            generation: 0,
                            modifier: 0,
                            width: ghost.buffer_width as u32,
                            height: ghost.buffer_height as u32,
                            color: ghost.color.clone(),
                            last_used_epoch: self.frame_epoch,
                        },
                    );
                }
            }
            if let Some(entry) = self.cache.get(&ghost.id) {
                let img = &entry.image;
                canvas.draw_image_with_paint(
                    img,
                    ghost.rect.origin.x as f32,
                    ghost.rect.origin.y as f32,
                    ghost.rect.size.w as f32,
                    ghost.rect.size.h as f32,
                    &paint,
                );
            }
        }
    }

    fn draw_toplevels_impl(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        frames: &[SurfacePixels<'_>],
        map: Option<
            &dyn Fn(Option<aegis_model::window::WindowId>, aegis_model::Rect) -> aegis_model::Rect,
        >,
        mapped_style: Option<MappedSurfaceStyle>,
    ) {
        // One upload batch per composite; see `draw_surfaces_ordered_impl`.
        let _uploads = device.uploads_begin();
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
            //   frame, resize, transform, or empty damage
            //   ("no information") — replaces the whole texture.
            let (tex_w, tex_h) = transformed_dims(f.width, f.height, f.geometry.transform);
            let dims_match = self
                .cache
                .get(&f.id)
                .is_some_and(|c| c.image.size() == (tex_w as u32, tex_h as u32));
            // A color-tag change leaves the pixels identical but requires a
            // fresh, tagged texture — never the incremental path.
            let tag_changed = self.cache.get(&f.id).is_some_and(|c| c.color != f.color);
            let new_contents = tag_changed
                || self
                    .cache
                    .get(&f.id)
                    .is_none_or(|c| c.generation != f.generation);
            if !dims_match || new_contents {
                // Refresh strategy, cheapest first:
                //
                // 1. An upright, same-tag texture whose cached size still
                //    matches can be refreshed **in place** with a full
                //    `update_region`. The pixel copy is the same staging
                //    upload `flux_image_create` performs, but it skips
                //    re-allocating the VkImage, re-registering it in the
                //    bindless heap, retiring the old texture through the
                //    deferred-release queue, and the per-upload queue
                //    submit churn that dominated CPU profiles for
                //    continuously-updating SHM clients (terminals, browser
                //    tabs) on HiDPI outputs. This also covers the
                //    "no information" commit (empty damage) — the whole
                //    texture is rewritten either way.
                // 2. Same conditions *plus* usable surface-local damage:
                //    upload only the damage bounding box.
                // 3. Anything else (first frame, resize, color-tag change,
                //    rotated buffer): recreate through `flux_image_create`.
                let upright = f.geometry.transform == Transform::Normal;
                let in_place =
                    dims_match && !tag_changed && upright && self.cache.contains_key(&f.id);
                // Incremental refresh additionally requires the server to
                // have supplied usable surface-local damage. Unlike the
                // historical guard, a buffer_scale > 1 is NOT a barrier: the
                // server already normalises buffer damage to surface-local
                // logical coordinates at commit, so we only have to scale
                // those coordinates into buffer pixels (buffer_scale, or the
                // viewport-implied factor for fractional-scale clients).
                let incremental = in_place && !f.damage.is_empty();
                if in_place || incremental {
                    // Union of the damage rects mapped from surface-local
                    // logical coordinates to buffer pixels (rounded outward so
                    // no edge pixel is dropped), then clamped to the buffer
                    // extent. Uploaded in a single update_region: pixels
                    // outside every damaged rect are identical to the previous
                    // frame by the damage protocol, so refreshing a bounding
                    // superset is always correct. The factor is the effective
                    // logical→buffer scale — fractional-scale clients keep
                    // buffer_scale at 1 and carry the density in a wp_viewport
                    // destination, which buffer_scale alone under-covers.
                    let (x0, y0, x1, y1) = if incremental {
                        let (scale_x, scale_y) =
                            f.geometry.logical_to_buffer_scale(f.width, f.height);
                        let mut x0 = i32::MAX;
                        let mut y0 = i32::MAX;
                        let mut x1 = i32::MIN;
                        let mut y1 = i32::MIN;
                        for d in f.damage {
                            // Outward rounding so a partially-covered buffer
                            // pixel is always included (mirrors the server's
                            // buffer_damage_to_surface division).
                            let sx0 = ((d.origin.x as f32 * scale_x).floor() as i32)
                                .max(0)
                                .min(f.width);
                            let sy0 = ((d.origin.y as f32 * scale_y).floor() as i32)
                                .max(0)
                                .min(f.height);
                            let sx1 = (((d.origin.x + d.size.w) as f32 * scale_x).ceil() as i32)
                                .max(0)
                                .min(f.width);
                            let sy1 = (((d.origin.y + d.size.h) as f32 * scale_y).ceil() as i32)
                                .max(0)
                                .min(f.height);
                            if sx1 <= sx0 || sy1 <= sy0 {
                                continue;
                            }
                            x0 = x0.min(sx0);
                            y0 = y0.min(sy0);
                            x1 = x1.max(sx1);
                            y1 = y1.max(sy1);
                        }
                        (x0, y0, x1, y1)
                    } else {
                        // No damage information: rewrite the whole texture.
                        (0, 0, f.width, f.height)
                    };
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
                            // Tightly packed BGRA8: stride = width * 4. The
                            // staging scratch is reused across frames: a
                            // terminal cursor blink or a scrolling browser
                            // hits this path at commit rate, and a fresh
                            // allocation per frame (up to several MiB for a
                            // large damage box) showed up as avoidable heap
                            // churn in the CPU profile.
                            let bpp = 4usize;
                            let stride = f.width as usize * bpp;
                            let row_bytes = bw as usize * bpp;
                            self.shm_staging.clear();
                            self.shm_staging.resize(bh as usize * row_bytes, 0);
                            for row in 0..bh as usize {
                                let src_off = (y0 as usize + row) * stride + x0 as usize * bpp;
                                let dst_off = row * row_bytes;
                                // src_off + len must stay within f.pixels.
                                let avail = f.pixels.len().saturating_sub(src_off);
                                let take = row_bytes.min(avail);
                                self.shm_staging[dst_off..dst_off + take]
                                    .copy_from_slice(&f.pixels[src_off..src_off + take]);
                            }
                            img.update_region(x0 as u32, y0 as u32, bw, bh, &self.shm_staging)
                        };
                        match updated {
                            Ok(()) => {
                                if let Some(cached) = self.cache.get_mut(&f.id) {
                                    cached.generation = f.generation;
                                    cached.last_used_epoch = self.frame_epoch;
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
                    let uploaded = {
                        let tag = self.image_color_tag(f.color.as_ref());
                        flux::Image::from_bytes_with_color_space(
                            device,
                            tex_w as u32,
                            tex_h as u32,
                            flux::Format::Bgra8Unorm,
                            &transformed,
                            tag,
                        )
                    };
                    if let Ok(img) = uploaded {
                        self.cache_image(
                            f.id,
                            CachedImage {
                                image: img,
                                generation: f.generation,
                                modifier: 0,
                                width: f.width as u32,
                                height: f.height as u32,
                                color: f.color.clone(),
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
                // wl_surface opaque regions are expressed in the surface's
                // untransformed logical coordinate space. Preserve that
                // extent before a window transition temporarily rescales the
                // destination rectangle.
                let surface_logical = (dst_w, dst_h);
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
                // ADR-0029 open/close fade: modulate the whole blit through a
                // translucent solid paint. The opaque-region split is skipped
                // while fading — SRC-replace ignores paint alpha and would
                // punch opaque holes in the fade.
                let fade_paint = if map.is_none() {
                    f.geometry.transition_opacity.map(|opacity| {
                        let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
                        flux::Paint::solid(flux::rgba(255, 255, 255, alpha))
                    })
                } else {
                    None
                };
                if let Some(map) = map {
                    let natural = aegis_model::Rect::new(
                        x as i32,
                        y as i32,
                        dst_w.max(1.0) as i32,
                        dst_h.max(1.0) as i32,
                    );
                    let mapped = map(f.window, natural);
                    if let Some(style) = mapped_style {
                        let (brightness, alpha) = mapped_surface_modulation(style);
                        let paint = flux::Paint::solid(flux::rgba(
                            brightness, brightness, brightness, alpha,
                        ));
                        let clip = style.rounded_clip;
                        canvas.draw_image_clipped_rrect_with_paint(
                            img,
                            mapped.origin.x as f32,
                            mapped.origin.y as f32,
                            mapped.size.w as f32,
                            mapped.size.h as f32,
                            clip.origin.x as f32,
                            clip.origin.y as f32,
                            clip.size.w as f32,
                            clip.size.h as f32,
                            style.corner_radius,
                            &paint,
                        );
                    } else {
                        canvas.draw_image(
                            img,
                            mapped.origin.x as f32,
                            mapped.origin.y as f32,
                            mapped.size.w as f32,
                            mapped.size.h as f32,
                        );
                    }
                    continue;
                }
                if let Some(paint) = &fade_paint {
                    // Fading (open/close): one translucent blit of the whole
                    // surface. draw_image_with_paint source-overs the tinted
                    // texture, so a white tint with the fade alpha both fades
                    // the window and keeps per-pixel alpha intact.
                    canvas.draw_image_with_paint(img, x, y, dst_w, dst_h, paint);
                    continue;
                }
                match f.geometry.viewport_src {
                    _ if map.is_none() && f.geometry.minimize_warp.is_some() => {
                        // Genie minimize flight: slice the window into
                        // horizontal strips pinched toward the dock icon. The
                        // opaque-region split is skipped for the few hundred
                        // milliseconds the warp lasts.
                        let warp = f.geometry.minimize_warp.expect("checked above");
                        let (su, sv, sw, sh) = match f.geometry.viewport_src {
                            Some(src) => viewport_uv(src, (tw, th), f.geometry.buffer_scale),
                            None => (0.0, 0.0, 1.0, 1.0),
                        };
                        for strip in genie_strips(x, y, dst_w, dst_h, warp.target, warp.progress) {
                            canvas.draw_image_sub(
                                img,
                                strip.dst_x,
                                strip.dst_y,
                                strip.dst_w,
                                strip.dst_h,
                                su + sw * strip.src_u,
                                sv + sh * strip.src_v,
                                sw * strip.src_du,
                                sh * strip.src_dv,
                            );
                        }
                    }
                    Some(src) => {
                        let (su, sv, sw, sh) = viewport_uv(src, (tw, th), f.geometry.buffer_scale);
                        canvas.draw_image_sub(img, x, y, dst_w, dst_h, su, sv, sw, sh);
                    }
                    None => {
                        // Split on the client's opaque_region so the pixels it
                        // declares opaque use SRC-replace (no destination
                        // readback) and only the translucent remainder pays the
                        // source-over merge. SHM XRGB alpha is already forced
                        // opaque at ingest, so SRC-replace is pixel-correct here
                        // too. Opaque-region mapping is only well-defined without
                        // a buffer transform.
                        let can_split = f.geometry.transform == aegis_model::Transform::Normal;
                        let (opaque, blended) = split_opaque_blit(
                            (x, y, dst_w, dst_h),
                            surface_logical,
                            f.opaque_region,
                            can_split,
                        );
                        for b in &opaque {
                            canvas.draw_image_opaque_sub(
                                img, b.dst_x, b.dst_y, b.dst_w, b.dst_h, b.src_u, b.src_v,
                                b.src_du, b.src_dv,
                            );
                        }
                        for b in &blended {
                            canvas.draw_image_sub(
                                img, b.dst_x, b.dst_y, b.dst_w, b.dst_h, b.src_u, b.src_v,
                                b.src_du, b.src_dv,
                            );
                        }
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
        self.draw_dmabuf_toplevels_impl(device, canvas, frames, None, None);
    }

    /// As [`draw_dmabuf_toplevels`](Self::draw_dmabuf_toplevels), but each
    /// frame's natural destination rect is passed through `map` first (the
    /// overview's grid placement, M9).
    pub fn draw_dmabuf_toplevels_mapped(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        frames: &[SurfaceDmabuf],
        map: &dyn Fn(Option<aegis_model::window::WindowId>, aegis_model::Rect) -> aegis_model::Rect,
    ) {
        self.draw_dmabuf_toplevels_impl(device, canvas, frames, Some(map), None);
    }

    fn draw_dmabuf_toplevels_impl(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        frames: &[SurfaceDmabuf],
        map: Option<
            &dyn Fn(Option<aegis_model::window::WindowId>, aegis_model::Rect) -> aegis_model::Rect,
        >,
        mapped_style: Option<MappedSurfaceStyle>,
    ) {
        // One upload batch per composite; see `draw_surfaces_ordered_impl`.
        let _uploads = device.uploads_begin();
        for f in frames.iter() {
            self.retire_cached(f.id);
            let key = (f.id, f.buffer_id);
            let reusable = f.buffer_id != 0
                && self.dmabuf_cache.get(&key).is_some_and(|cached| {
                    cached.modifier == f.modifier
                        && cached.width == f.width as u32
                        && cached.height == f.height as u32
                        && cached.color == f.color
                });

            if reusable {
                // Explicit-sync fences describe each new producer commit, not
                // the lifetime of the backing buffer. Attach the new fence to
                // this frame's use of the existing VkImage; Flux waits it on
                // the GPU before the FOREIGN -> graphics ownership acquire.
                let needs_acquire_wait = self.dmabuf_cache.get(&key).is_some_and(|cached| {
                    reusable_acquire_wait_required(cached.generation, f.generation, f.acquire_fence)
                });
                if needs_acquire_wait {
                    let fence = match unsafe { BorrowedFd::borrow_raw(f.acquire_fence) }
                        .try_clone_to_owned()
                    {
                        Ok(fd) => fd,
                        Err(_) => {
                            if self.failed_imports.insert(f.id, ()).is_none() {
                                log::warn!(
                                    "[render] failed to duplicate reusable dma-buf acquire fence fd {}",
                                    f.acquire_fence
                                );
                            }
                            continue;
                        }
                    };
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
                    // later commits, so hand Flux a fresh duplicate. Wrapping
                    // the duplicate in `OwnedFd` right away means every early
                    // `continue` below drops (and closes) it.
                    let import_fd =
                        match unsafe { BorrowedFd::borrow_raw(f.fd) }.try_clone_to_owned() {
                            Ok(fd) => fd,
                            Err(error) => {
                                if self.failed_imports.insert(f.id, ()).is_none() {
                                    log::warn!(
                                        "[render] failed to duplicate dma-buf fd {} for {}x{}: {error}",
                                        f.fd,
                                        f.width,
                                        f.height,
                                    );
                                }
                                continue;
                            }
                        };
                    let acquire_fence = if f.acquire_fence >= 0 {
                        match unsafe { BorrowedFd::borrow_raw(f.acquire_fence) }
                            .try_clone_to_owned()
                        {
                            Ok(dup) => Some(dup),
                            Err(error) => {
                                if self.failed_imports.insert(f.id, ()).is_none() {
                                    log::warn!(
                                        "[render] failed to duplicate acquire fence fd {}: {error}",
                                        f.acquire_fence
                                    );
                                }
                                continue;
                            }
                        }
                    } else {
                        None
                    };
                    let img = unsafe {
                        let tag = self.image_color_tag(f.color.as_ref());
                        flux::Image::import_dmabuf_with_color_space(
                            device,
                            f.width as u32,
                            f.height as u32,
                            fmt,
                            f.modifier,
                            import_fd,
                            f.offset,
                            f.stride,
                            acquire_fence,
                            tag,
                        )
                    };
                    match img {
                        Ok(img) => {
                            if !self
                                .dmabuf_cache
                                .keys()
                                .any(|candidate| candidate.0 == f.id)
                            {
                                log::trace!(
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
                                    color: f.color.clone(),
                                    last_used_epoch: self.frame_epoch,
                                },
                            );
                        }
                        Err(e) => {
                            // The `OwnedFd` drops close the duplicates; flux
                            // only consumes them on a successful import.
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
                let surface_logical = (dst_w, dst_h);
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
                // ADR-0029 open/close fade (see the shm path).
                let fade_paint = if map.is_none() {
                    f.geometry.transition_opacity.map(|opacity| {
                        let alpha = (opacity.clamp(0.0, 1.0) * 255.0).round() as u8;
                        flux::Paint::solid(flux::rgba(255, 255, 255, alpha))
                    })
                } else {
                    None
                };
                if let Some(map) = map {
                    let natural = aegis_model::Rect::new(
                        x as i32,
                        y as i32,
                        dst_w.max(1.0) as i32,
                        dst_h.max(1.0) as i32,
                    );
                    let mapped = map(f.window, natural);
                    if let Some(style) = mapped_style {
                        let (brightness, alpha) = mapped_surface_modulation(style);
                        let paint = flux::Paint::solid(flux::rgba(
                            brightness, brightness, brightness, alpha,
                        ));
                        let clip = style.rounded_clip;
                        canvas.draw_image_clipped_rrect_with_paint(
                            img,
                            mapped.origin.x as f32,
                            mapped.origin.y as f32,
                            mapped.size.w as f32,
                            mapped.size.h as f32,
                            clip.origin.x as f32,
                            clip.origin.y as f32,
                            clip.size.w as f32,
                            clip.size.h as f32,
                            style.corner_radius,
                            &paint,
                        );
                    } else if drm_fmt::is_format_opaque(f.drm_format)
                        && f.geometry.viewport_src.is_none()
                    {
                        canvas.draw_image_opaque(
                            img,
                            mapped.origin.x as f32,
                            mapped.origin.y as f32,
                            mapped.size.w as f32,
                            mapped.size.h as f32,
                        );
                    } else {
                        canvas.draw_image(
                            img,
                            mapped.origin.x as f32,
                            mapped.origin.y as f32,
                            mapped.size.w as f32,
                            mapped.size.h as f32,
                        );
                    }
                    continue;
                }
                if let Some(paint) = &fade_paint {
                    // Fading (open/close), mirroring the shm path.
                    canvas.draw_image_with_paint(img, x, y, dst_w, dst_h, paint);
                    continue;
                }
                match f.geometry.viewport_src {
                    _ if map.is_none() && f.geometry.minimize_warp.is_some() => {
                        // Genie minimize flight, mirroring the shm path.
                        let warp = f.geometry.minimize_warp.expect("checked above");
                        let (su, sv, sw, sh) = match f.geometry.viewport_src {
                            Some(src) => {
                                viewport_uv(src, (f.width, f.height), f.geometry.buffer_scale)
                            }
                            None => (0.0, 0.0, 1.0, 1.0),
                        };
                        for strip in genie_strips(x, y, dst_w, dst_h, warp.target, warp.progress) {
                            canvas.draw_image_sub(
                                img,
                                strip.dst_x,
                                strip.dst_y,
                                strip.dst_w,
                                strip.dst_h,
                                su + sw * strip.src_u,
                                sv + sh * strip.src_v,
                                sw * strip.src_du,
                                sh * strip.src_dv,
                            );
                        }
                    }
                    Some(src) => {
                        let (su, sv, sw, sh) =
                            viewport_uv(src, (f.width, f.height), f.geometry.buffer_scale);
                        if drm_fmt::is_format_opaque(f.drm_format) {
                            // Alpha-free buffer with source-crop: still SRC-replace
                            // and alpha-forced-opaque, so the undefined X bits are
                            // never read as alpha and no framebuffer readback occurs.
                            canvas.draw_image_opaque_sub(img, x, y, dst_w, dst_h, su, sv, sw, sh);
                        } else {
                            canvas.draw_image_sub(img, x, y, dst_w, dst_h, su, sv, sw, sh);
                        }
                    }
                    None => {
                        if drm_fmt::is_format_opaque(f.drm_format) {
                            // Alpha-free buffer: the whole surface is SRC-replace and
                            // alpha-forced-opaque, so the undefined X bits are never
                            // read as alpha and no framebuffer readback occurs.
                            canvas.draw_image_opaque(img, x, y, dst_w, dst_h);
                        } else {
                            // ARGB buffer: split on the client's opaque_region so the
                            // pixels it declares opaque use SRC-replace (no
                            // destination readback) and only the translucent remainder
                            // pays the source-over merge. Opaque-region mapping is only
                            // well-defined without a buffer transform.
                            let can_split = f.geometry.transform == aegis_model::Transform::Normal;
                            let (opaque, blended) = split_opaque_blit(
                                (x, y, dst_w, dst_h),
                                surface_logical,
                                f.opaque_region.as_deref(),
                                can_split,
                            );
                            for b in &opaque {
                                canvas.draw_image_opaque_sub(
                                    img, b.dst_x, b.dst_y, b.dst_w, b.dst_h, b.src_u, b.src_v,
                                    b.src_du, b.src_dv,
                                );
                            }
                            for b in &blended {
                                canvas.draw_image_sub(
                                    img, b.dst_x, b.dst_y, b.dst_w, b.dst_h, b.src_u, b.src_v,
                                    b.src_du, b.src_dv,
                                );
                            }
                        }
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
        map: &dyn Fn(Option<aegis_model::window::WindowId>, aegis_model::Rect) -> aegis_model::Rect,
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
        map: &dyn Fn(Option<aegis_model::window::WindowId>, aegis_model::Rect) -> aegis_model::Rect,
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

    #[test]
    fn modifier_preference_is_stable_and_keeps_linear_last() {
        let tiled_a = 0x100;
        let tiled_b = 0x200;
        let mut modifiers = vec![
            drm_fmt::DRM_FORMAT_MOD_LINEAR,
            tiled_a,
            drm_fmt::DRM_FORMAT_MOD_LINEAR,
            tiled_b,
        ];
        prefer_native_modifiers(&mut modifiers);
        assert_eq!(
            modifiers,
            vec![
                tiled_a,
                tiled_b,
                drm_fmt::DRM_FORMAT_MOD_LINEAR,
                drm_fmt::DRM_FORMAT_MOD_LINEAR,
            ]
        );
    }

    #[test]
    fn reusable_dmabuf_wait_is_once_per_commit_generation() {
        assert!(reusable_acquire_wait_required(7, 8, 12));
        assert!(!reusable_acquire_wait_required(8, 8, 12));
        assert!(!reusable_acquire_wait_required(7, 8, -1));
    }

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

    /// Genie strips leave the destination untouched at progress zero.
    #[test]
    fn genie_strips_are_undeformed_at_progress_zero() {
        let target = aegis_model::Point { x: 500, y: 1000 };
        let strips = genie_strips(100.0, 50.0, 400.0, 240.0, target, 0.0);
        assert_eq!(strips.len(), GENIE_STRIPS);
        let n = GENIE_STRIPS as f32;
        for (i, strip) in strips.iter().enumerate() {
            assert_eq!(strip.dst_x, 100.0, "strip {i} x");
            assert_eq!(strip.dst_w, 400.0, "strip {i} w");
            assert!((strip.dst_y - (50.0 + 240.0 * i as f32 / n)).abs() < 1e-4);
            assert!((strip.src_v - i as f32 / n).abs() < 1e-6);
            assert!((strip.src_dv - 1.0 / n).abs() < 1e-6);
        }
    }

    /// The pinch strengthens toward the bottom edge and funnels the lowest
    /// strip onto the icon's centre as progress completes.
    #[test]
    fn genie_strips_funnel_the_bottom_edge_into_the_icon() {
        let target = aegis_model::Point { x: 700, y: 1000 };
        let strips = genie_strips(100.0, 50.0, 400.0, 240.0, target, 1.0);
        let widths: Vec<f32> = strips.iter().map(|strip| strip.dst_w).collect();
        assert!(
            widths.windows(2).all(|pair| pair[0] >= pair[1]),
            "pinch widens downward: {widths:?}"
        );
        let top = strips.first().unwrap();
        assert!(top.dst_w > 300.0, "top edge barely pinches: {top:?}");
        let bottom = strips.last().unwrap();
        let bottom_centre = bottom.dst_x + bottom.dst_w * 0.5;
        assert!(
            (bottom_centre - 700.0).abs() < 30.0,
            "bottom edge converges on the icon: {bottom:?}"
        );
        assert!(
            bottom.dst_w < 200.0,
            "bottom edge nearly closes: {bottom:?}"
        );
        // Vertical coverage stays intact: the funnel is horizontal only.
        let covered = strips.iter().map(|strip| strip.dst_h).sum::<f32>();
        assert!((covered - 240.0).abs() < 1e-3);
    }

    /// `destination_size` covers the four viewport/scale combinations.
    #[test]
    fn destination_size_handles_viewport_and_scale() {
        use aegis_model::{Rect, Size};

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

    /// A scale-2 buffer is 3072x1920 pixels but its opaque region is still
    /// expressed in the 1536x960 surface coordinate space. A full-surface
    /// region must therefore produce exactly one full opaque blit, not a
    /// half-sized opaque piece plus a second enlarged copy of the texture.
    #[test]
    fn opaque_split_uses_surface_coordinates_at_hidpi_scale() {
        let region = [aegis_model::Rect::new(0, 0, 1536, 960)];
        let (opaque, blended) = split_opaque_blit(
            (0.0, 0.0, 1536.0, 960.0),
            (1536.0, 960.0),
            Some(&region),
            true,
        );

        assert_eq!(
            opaque,
            vec![BlitRect {
                dst_x: 0.0,
                dst_y: 0.0,
                dst_w: 1536.0,
                dst_h: 960.0,
                src_u: 0.0,
                src_v: 0.0,
                src_du: 1.0,
                src_dv: 1.0,
            }]
        );
        assert!(blended.is_empty());
    }

    /// Client-owned region coordinates may approach the i32 limits. Clipping
    /// must reject an entirely out-of-bounds rectangle without overflowing its
    /// far-edge addition or disturbing the full blended fallback.
    #[test]
    fn opaque_split_saturates_extreme_client_rect_edges() {
        let region = [aegis_model::Rect::new(i32::MAX - 4, i32::MAX - 4, 100, 100)];
        let (opaque, blended) =
            split_opaque_blit((0.0, 0.0, 128.0, 64.0), (128.0, 64.0), Some(&region), true);

        assert!(opaque.is_empty());
        assert_eq!(
            blended,
            vec![BlitRect {
                dst_x: 0.0,
                dst_y: 0.0,
                dst_w: 128.0,
                dst_h: 64.0,
                src_u: 0.0,
                src_v: 0.0,
                src_du: 1.0,
                src_dv: 1.0,
            }]
        );
    }

    /// Transition scaling changes only the destination mapping; the source UV
    /// remains normalised against the original surface logical extent.
    #[test]
    fn opaque_split_scales_destination_without_rescaling_source_uv() {
        let region = [aegis_model::Rect::new(0, 0, 1536, 120)];
        let (opaque, blended) = split_opaque_blit(
            (10.0, 20.0, 3072.0, 1920.0),
            (1536.0, 960.0),
            Some(&region),
            true,
        );

        assert_eq!(opaque.len(), 1);
        assert_eq!(opaque[0].dst_x, 10.0);
        assert_eq!(opaque[0].dst_y, 20.0);
        assert_eq!(opaque[0].dst_w, 3072.0);
        assert_eq!(opaque[0].dst_h, 240.0);
        assert_eq!(opaque[0].src_u, 0.0);
        assert_eq!(opaque[0].src_v, 0.0);
        assert_eq!(opaque[0].src_du, 1.0);
        assert_eq!(opaque[0].src_dv, 0.125);

        assert_eq!(blended.len(), 1);
        assert_eq!(blended[0].dst_y, 260.0);
        assert_eq!(blended[0].dst_h, 1680.0);
        assert_eq!(blended[0].src_v, 0.125);
        assert_eq!(blended[0].src_dv, 0.875);
    }

    #[test]
    fn viewport_source_converts_post_scale_coordinates_to_buffer_uvs() {
        let src = aegis_model::Rect::new(10, 5, 30, 20);
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
    fn workspace_filter_is_closed_to_foreign_and_unowned_surfaces() {
        use aegis_model::window::WindowId;

        let filter = HashSet::from([WindowId(2)]);
        assert!(surface_passes_window_filter(
            Some(WindowId(2)),
            Some(&filter)
        ));
        assert!(!surface_passes_window_filter(
            Some(WindowId(1)),
            Some(&filter)
        ));
        assert!(!surface_passes_window_filter(None, Some(&filter)));
        assert!(surface_passes_window_filter(None, None));
    }

    #[test]
    fn mapped_surface_modulation_separates_brightness_from_opacity() {
        let style = MappedSurfaceStyle {
            opacity: 0.5,
            brightness: 0.75,
            rounded_clip: aegis_model::Rect::new(0, 0, 100, 80),
            corner_radius: 12.0,
        };
        assert_eq!(mapped_surface_modulation(style), (191, 128));
        assert_eq!(
            mapped_surface_modulation(MappedSurfaceStyle {
                opacity: f32::NAN,
                brightness: f32::NAN,
                ..style
            }),
            (255, 255)
        );
    }

    #[test]
    fn resize_shadow_follows_direct_resize_eligibility() {
        let mut window = aegis_model::window::Window::new(aegis_model::window::WindowId(1));
        window.size = aegis_model::Size { w: 640, h: 480 };
        window.layout_role = aegis_model::layout::LayoutRole::Floating;
        assert!(window_casts_resize_shadow(&window));

        window.state.maximized = true;
        assert!(!window_casts_resize_shadow(&window));
        window.state.maximized = false;
        window.read_only = true;
        assert!(!window_casts_resize_shadow(&window));
        window.read_only = false;
        window.layout_role = aegis_model::layout::LayoutRole::Tiled;
        assert!(!window_casts_resize_shadow(&window));
    }

    #[test]
    fn evict_surface_purges_cached_textures_immediately() {
        let mut renderer = Renderer::new();
        // Insert dummy failed import and verify evict clears it
        renderer.failed_imports.insert(42, ());
        assert!(renderer.failed_imports.contains_key(&42));
        renderer.evict_surface(42);
        assert!(!renderer.failed_imports.contains_key(&42));
    }
}
