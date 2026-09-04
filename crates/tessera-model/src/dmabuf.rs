//! DRM dma-buf format metadata shared across the compositor and renderer.
//!
//! These are plain DRM fourcc / modifier constants — independent of flux or
//! Wayland — so a renderer (which knows flux and the Vulkan device) can
//! produce the advertised format/modifier set and hand it to the compositor
//! (which speaks `zwp_linux_dmabuf_v1`) without either crate depending on the
//! other.
//!
//! The modifier set advertised to clients must match what the render device
//! can actually sample and import as external memory. Advertising only
//! `DRM_FORMAT_MOD_LINEAR` (the historical fallback) forces clients onto
//! uncompressed, untiled buffers, which for a GPU-bound client such as a game
//! is a severe bandwidth regression — see the renderer's
//! `formats_with_modifiers`.

/// Little-endian 32 bpp: `[B, G, R, A]` in memory. The X-variant's alpha byte
/// is undefined; the server forces it opaque at commit time.
pub const DRM_FORMAT_ARGB8888: u32 = 0x3432_5241;
pub const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258;

/// Little-endian 32 bpp: `[R, G, B, A]` in memory (the byte-swapped pair).
pub const DRM_FORMAT_ABGR8888: u32 = 0x3432_4241;
pub const DRM_FORMAT_XBGR8888: u32 = 0x3432_4258;

/// Packed 10-bit-per-channel ("30"): `A`/`X` in bits 31..=30, then B, G, R
/// in descending 10-bit fields — the Vulkan `A2B10G10R10_UNORM_PACK32`
/// layout. This is the deep-color scanout and client HDR video format.
pub const DRM_FORMAT_ABGR2101010: u32 = 0x3033_4241;
pub const DRM_FORMAT_XBGR2101010: u32 = 0x3033_4258;

/// `WL_SHM_FORMAT_XRGB8888`. Wayland SHM formats overlap numerically with DRM
/// fourccs for the `A`-variants but use the legacy `1` enum value for the
/// `X`-variant, so SHM and DRM opaqueness must be tested separately. Kept here
/// so all alpha/opaque knowledge lives in one place.
pub const WL_SHM_FORMAT_XRGB8888: u32 = 1;

/// The single source of truth for "is this DRM fourcc alpha-free?" — a buffer
/// whose only undefined channel is a padding `X` byte can be composited with a
/// SRC-replace write (no destination read) and treated as fully opaque by the
/// occlusion pass. Every consumer (occlusion culling, renderer blit selection,
/// SHM ingestion) must go through this predicate instead of re-listing fourccs,
/// so adding a new alpha-free format can never silently diverge between them.
pub fn is_format_opaque(fourcc: u32) -> bool {
    matches!(
        fourcc,
        DRM_FORMAT_XRGB8888 | DRM_FORMAT_XBGR8888 | DRM_FORMAT_XBGR2101010
    )
}

/// Whether a Wayland SHM format code has an undefined (padding) alpha byte that
/// must be forced opaque at ingest time. SHM format codes are a separate
/// namespace from DRM fourccs (see [`WL_SHM_FORMAT_XRGB8888`]).
pub fn is_wl_shm_format_xrgb(shm_format: u32) -> bool {
    shm_format == WL_SHM_FORMAT_XRGB8888
}

/// `DRM_FORMAT_MOD_LINEAR` — the only layout a CPU can interpret directly. It
/// disables GPU compression and tiling, so advertising it alone is a
/// performance liability whenever the device supports better modifiers.
pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;
/// `DRM_FORMAT_MOD_INVALID` signals "no explicit layout"; some legacy clients
/// select it. Kept for reference; the compositor requires explicit modifiers.
pub const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// The fourccs the compositor can import, in advertisement order. The
/// 10-bit entries serve HDR and deep-color client content (decoded into
/// the working space at sample time; see the renderer's
/// `drm_format_to_flux`).
pub const ADVERTISED_FOURCCS: [u32; 6] = [
    DRM_FORMAT_ARGB8888,
    DRM_FORMAT_XRGB8888,
    DRM_FORMAT_ABGR8888,
    DRM_FORMAT_XBGR8888,
    DRM_FORMAT_ABGR2101010,
    DRM_FORMAT_XBGR2101010,
];

/// A fourcc paired with the device-supported modifiers a client may use for
/// it. The renderer builds these from the Vulkan device's capability
/// queries; the compositor advertises them verbatim over
/// `zwp_linux_dmabuf_v1`.
#[derive(Debug, Clone)]
pub struct DmabufFormat {
    pub fourcc: u32,
    pub modifiers: Vec<u64>,
}
