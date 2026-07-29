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

/// `DRM_FORMAT_MOD_LINEAR` — the only layout a CPU can interpret directly. It
/// disables GPU compression and tiling, so advertising it alone is a
/// performance liability whenever the device supports better modifiers.
pub const DRM_FORMAT_MOD_LINEAR: u64 = 0;
/// `DRM_FORMAT_MOD_INVALID` signals "no explicit layout"; some legacy clients
/// select it. Kept for reference; the compositor requires explicit modifiers.
pub const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

/// The 32-bit-per-pixel fourccs the compositor can import, in advertisement
/// order.
pub const ADVERTISED_FOURCCS: [u32; 4] = [
    DRM_FORMAT_ARGB8888,
    DRM_FORMAT_XRGB8888,
    DRM_FORMAT_ABGR8888,
    DRM_FORMAT_XBGR8888,
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
