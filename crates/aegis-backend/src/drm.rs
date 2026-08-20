//! Direct-display backend built on atomic DRM/KMS, libseat, and libinput.
//!
//! Flux renders into exportable offscreen Vulkan images. Each completed image
//! is imported as a DRM framebuffer and attached to the primary plane with an
//! atomic commit. The device runs three in-flight frame slots (see
//! `host::FRAMES_IN_FLIGHT`), so the slot a new frame renders into was retired
//! from scanout one flip earlier — rendering never aliases the image the CRTC
//! is still scanning. `pending_flips` exposes that ownership boundary to the
//! runtime presentation state machine: event/input dispatch remains live, but
//! a second atomic batch cannot start until every CRTC retires the first one.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::RangeBounds;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

mod device;
mod events;
mod output;

use device::PresentedComposite;
use output::*;

use aegis_model::Size;
use aegis_model::input::{
    ButtonState, InputEvent, PointerAxis, PointerAxisFrame, PointerAxisRelativeDirection,
    PointerAxisSource, PointerGestureEvent, TabletEvent, TabletToolInfo, TouchpadCapabilities,
    TouchpadConfig, TouchpadScrollMethod, TouchpadStatus,
};
use aegis_model::output::{
    ColorPolicy, ModeSpec, OutputKind, OutputMode, Scale, automatic_scale, physical_ppi,
};
use drm::buffer::{Buffer, DrmFourcc, DrmModifier, Handle as BufferHandle, PlanarBuffer};
use drm::control::{
    self, AtomicCommitFlags, Device as ControlDevice, FbCmd2Flags, Mode, ModeTypeFlags,
    ResourceHandle, atomic, connector, crtc, plane, property,
};
use drm::{Device as BasicDevice, DriverCapability};
use input::event::EventTrait;
use input::event::device::DeviceEvent;
use input::event::gesture::{
    GestureEndEvent, GestureEvent, GestureEventCoordinates, GestureEventTrait, GestureHoldEvent,
    GesturePinchEvent, GesturePinchEventTrait, GestureSwipeEvent,
};
use input::event::keyboard::{KeyboardEvent, KeyboardEventTrait};
#[allow(deprecated)]
use input::event::pointer::{
    Axis, AxisSource, PointerAxisEvent, PointerEvent, PointerEventTrait, PointerScrollEvent,
    PointerScrollWheelEvent,
};
use input::event::tablet_tool::{
    ProximityState, TabletToolButtonEvent, TabletToolEvent, TabletToolEventTrait, TabletToolType,
    TipState,
};
use input::event::touch::{TouchEvent, TouchEventPosition, TouchEventSlot};
use input::{
    Device, DeviceCapability, DeviceConfigResult, DragLockState, Event, Libinput,
    LibinputInterface, ScrollMethod,
};

use crate::Backend;

/// Errors that prevent direct-display operation.
#[derive(Debug, thiserror::Error)]
pub enum DrmError {
    #[error("libseat: {0}")]
    Seat(String),
    #[error("no usable DRM card was found (tried {0})")]
    NoCard(String),
    #[error("DRM/KMS: {0}")]
    Io(#[from] std::io::Error),
    #[error("no connected DRM connector with a display mode")]
    NoConnector,
    #[error("connector has no compatible CRTC")]
    NoCrtc,
    #[error("CRTC has no compatible primary plane supporting XRGB8888/ARGB8888 dma-buf scanout")]
    NoPlane,
    #[error("DRM IN_FORMATS blob is malformed: {0}")]
    MalformedFormats(&'static str),
    #[error("combined output framebuffer {0}x{1} exceeds DRM limits")]
    DesktopTooLarge(u32, u32),
    #[error("DRM object is missing required atomic property {0}")]
    MissingProperty(&'static str),
    #[error("libinput could not bind seat {0}")]
    InputSeat(String),
    #[error("Flux: {0}")]
    Flux(#[from] flux::Error),
    #[error("Flux could not create exportable dma-buf render targets")]
    DmabufUnsupported,
    #[error("DRM session is inactive")]
    Inactive,
    #[error("previous DRM atomic commit is still being cleaned up")]
    Busy,
    #[error("timed out waiting for a KMS page flip")]
    FlipTimeout,
    #[error("display set changed during presentation; frame skipped")]
    Reconfigured,
    #[error("client buffer cannot be scanned out directly")]
    ScanoutUnsupported,
    #[error("hardware cursor unsupported: {0}")]
    CursorUnsupported(&'static str),
    #[error("hardware cursor atomic commit was rejected; retry with software composition")]
    CursorFallback,
}

/// Map an atomic-commit failure to a backend error. EBUSY is an allowed result
/// for a non-blocking atomic update while an earlier update is still pending;
/// a page-flip event marks `flip_done`, which can precede the kernel's terminal
/// `cleanup_done`. EACCES/EPERM means the session lost DRM master to a VT
/// switch whose seat Disable event has not been dispatched yet. All three are
/// transient frame-skip conditions and must never kill the compositor.
fn commit_error(error: std::io::Error) -> DrmError {
    match error.raw_os_error() {
        Some(libc::EBUSY) => DrmError::Busy,
        Some(libc::EACCES) | Some(libc::EPERM) => {
            log::warn!("drm: commit while masterless (VT switch in flight); skipping frame");
            DrmError::Inactive
        }
        _ => DrmError::Io(error),
    }
}

fn commit_error_is_transient(error: &std::io::Error) -> bool {
    matches!(
        error.raw_os_error(),
        Some(libc::EBUSY | libc::EACCES | libc::EPERM)
    )
}

/// Clip desktop-wide damage rectangles (physical framebuffer pixels) to one
/// output's scanout rectangle, yielding the `drm_mode_rect` fields
/// `(x1, y1, x2, y2)` for each `FB_DAMAGE_CLIPS` entry. `None` damage means
/// "unknown" and covers the whole output; an empty result slice means the
/// output is untouched by this frame's damage. Each input rect is clipped
/// independently so disjoint dirty regions (e.g. a video window plus a clock
/// tick) survive as separate clips instead of being unioned into one spanning
/// rect — which would defeat PSR2 / panel self-refresh.
fn damage_clip_for_output(
    damage: Option<&[aegis_model::Rect]>,
    output_x: u32,
    output_y: u32,
    width: u32,
    height: u32,
) -> Vec<[i32; 4]> {
    let (ox, oy) = (output_x as i32, output_y as i32);
    let full = [ox, oy, ox + width as i32, oy + height as i32];
    let Some(rects) = damage else {
        return vec![full];
    };
    let mut out = Vec::with_capacity(rects.len());
    for damage in rects {
        let x1 = damage.origin.x.max(ox);
        let y1 = damage.origin.y.max(oy);
        let x2 = (damage.origin.x.saturating_add(damage.size.w)).min(full[2]);
        let y2 = (damage.origin.y.saturating_add(damage.size.h)).min(full[3]);
        if x2 > x1 && y2 > y1 {
            out.push([x1, y1, x2, y2]);
        }
    }
    out
}

/// Place one cursor sprite on an output when their rectangles intersect. The
/// input position is the global physical-pixel hotspot and destination
/// coordinates are CRTC-local. Cursor planes conventionally require the full
/// source rectangle (Smithay/Niri follow the same rule); negative or
/// edge-crossing destination coordinates let KMS clip it. Returning a
/// placement for every intersected CRTC lets a cursor straddle outputs.
fn cursor_plane_rect(
    position: (i32, i32),
    hotspot: (u32, u32),
    image_size: (u32, u32),
    output_origin: (u32, u32),
    output_size: (u32, u32),
) -> Option<CursorPlaneRect> {
    let left = i64::from(position.0) - i64::from(hotspot.0);
    let top = i64::from(position.1) - i64::from(hotspot.1);
    let right = left + i64::from(image_size.0);
    let bottom = top + i64::from(image_size.1);
    let output_left = i64::from(output_origin.0);
    let output_top = i64::from(output_origin.1);
    let output_right = output_left + i64::from(output_size.0);
    let output_bottom = output_top + i64::from(output_size.1);
    if right <= output_left || bottom <= output_top || left >= output_right || top >= output_bottom
    {
        return None;
    }
    Some(CursorPlaneRect {
        src: (0, 0, image_size.0, image_size.1),
        dst: (
            (left - output_left) as i32,
            (top - output_top) as i32,
            image_size.0,
            image_size.1,
        ),
    })
}

fn add_cursor_plane_to_commit(
    request: &mut atomic::AtomicModeReq,
    output: &Output,
    state: Option<CursorState>,
    buffers: &[CursorBuffer],
) {
    let Some(cursor) = &output.cursor else {
        return;
    };
    let placement = state.and_then(|state| {
        let buffer = buffers.get(state.buffer)?;
        let (width, height) = output.mode.size();
        let rect = cursor_plane_rect(
            state.position,
            state.hotspot,
            buffer.dumb.size(),
            (output.x, output.y),
            (u32::from(width), u32::from(height)),
        )?;
        Some((buffer.framebuffer, rect))
    });
    let Some((framebuffer, rect)) = placement else {
        request.add_property(
            cursor.handle,
            cursor.props.plane_fb_id,
            property::Value::Framebuffer(None),
        );
        request.add_property(
            cursor.handle,
            cursor.props.plane_crtc_id,
            property::Value::CRTC(None),
        );
        return;
    };
    request.add_property(
        cursor.handle,
        cursor.props.plane_fb_id,
        property::Value::Framebuffer(Some(framebuffer)),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_crtc_id,
        property::Value::CRTC(Some(output.crtc)),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_src_x,
        property::Value::UnsignedRange(u64::from(rect.src.0) << 16),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_src_y,
        property::Value::UnsignedRange(u64::from(rect.src.1) << 16),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_src_w,
        property::Value::UnsignedRange(u64::from(rect.src.2) << 16),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_src_h,
        property::Value::UnsignedRange(u64::from(rect.src.3) << 16),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_crtc_x,
        property::Value::SignedRange(i64::from(rect.dst.0)),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_crtc_y,
        property::Value::SignedRange(i64::from(rect.dst.1)),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_crtc_w,
        property::Value::UnsignedRange(u64::from(rect.dst.2)),
    );
    request.add_property(
        cursor.handle,
        cursor.props.plane_crtc_h,
        property::Value::UnsignedRange(u64::from(rect.dst.3)),
    );
}

#[derive(Debug)]
struct Card {
    device: libseat::Device,
    path: PathBuf,
}

impl AsFd for Card {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.device.as_fd()
    }
}

impl BasicDevice for Card {}
impl ControlDevice for Card {}

#[derive(Debug, Clone, Copy)]
struct OutputAtomicProperties {
    connector_crtc_id: property::Handle,
    crtc_mode_id: property::Handle,
    crtc_active: property::Handle,
    /// `OUT_FENCE_PTR`: produces a sync_file for completion of an atomic
    /// commit. Direct scanout uses it for correct Wayland buffer release.
    crtc_out_fence_ptr: Option<property::Handle>,
    /// Connector `Colorspace` property plus the harvested enum values for
    /// `Default` and `BT2020_RGB` (None when the driver exposes neither the
    /// property nor those values). Set on every commit — the property is
    /// sticky, so an SDR commit after HDR must reset it explicitly.
    connector_colorspace: Option<(property::Handle, u64, u64)>,
    /// Connector `HDR_OUTPUT_METADATA` blob property (CTA-861.3 static
    /// metadata type 1). Set for HDR commits, cleared (blob 0) for SDR.
    connector_hdr_metadata: Option<property::Handle>,
    /// Connector `max bpc` range property. Driven to 10 for deep-color/HDR
    /// modes, 8 for SDR — drivers otherwise clamp it on their own.
    connector_max_bpc: Option<property::Handle>,
    /// CRTC `GAMMA_LUT` blob property and the driver's table size
    /// (`GAMMA_LUT_SIZE`, typically 256 or 1024 entries). Night light
    /// programs a per-channel gain ramp here; absent on legacy drivers.
    crtc_gamma_lut: Option<(property::Handle, u32)>,
}

#[derive(Debug, Clone, Copy)]
struct PrimaryPlaneProperties {
    fb_id: property::Handle,
    crtc_id: property::Handle,
    src_x: property::Handle,
    src_y: property::Handle,
    src_w: property::Handle,
    src_h: property::Handle,
    crtc_x: property::Handle,
    crtc_y: property::Handle,
    crtc_w: property::Handle,
    crtc_h: property::Handle,
    in_fence_fd: Option<property::Handle>,
    /// `FB_DAMAGE_CLIPS`: per-commit damage hint consumed by PSR-style
    /// drivers. Absent on planes/kernels without damage tracking; commits
    /// then carry no hint at all.
    fb_damage_clips: Option<property::Handle>,
}

#[derive(Debug)]
struct PrimaryPlane {
    handle: plane::Handle,
    props: PrimaryPlaneProperties,
    /// Pre-created `FB_DAMAGE_CLIPS` blob covering the output's whole
    /// framebuffer rectangle, reused by full/unknown-damage commits.
    full_damage_blob: Option<(property::Value<'static>, u64)>,
}

#[derive(Debug, Clone, Copy)]
struct CursorPlaneProperties {
    plane_fb_id: property::Handle,
    plane_crtc_id: property::Handle,
    plane_src_x: property::Handle,
    plane_src_y: property::Handle,
    plane_src_w: property::Handle,
    plane_src_h: property::Handle,
    plane_crtc_x: property::Handle,
    plane_crtc_y: property::Handle,
    plane_crtc_w: property::Handle,
    plane_crtc_h: property::Handle,
}

#[derive(Debug, Clone, Copy)]
struct CursorPlane {
    handle: plane::Handle,
    props: CursorPlaneProperties,
}

#[derive(Debug)]
struct Output {
    connector: connector::Handle,
    name: String,
    crtc: crtc::Handle,
    /// Exactly one primary plane owns this CRTC. Its framebuffer is either a
    /// compositor output or one eligible client dma-buf; those sources are
    /// mutually exclusive within an atomic commit.
    primary: PrimaryPlane,
    mode: Mode,
    mode_blob: property::Value<'static>,
    mode_blob_id: u64,
    x: u32,
    y: u32,
    /// Physical display dimensions reported by the DRM connector/EDID.
    physical_size_mm: Option<(u32, u32)>,
    /// Validated diagonal density for diagnostics. `None` means the physical
    /// dimensions were absent or failed the plausibility checks.
    ppi: Option<f32>,
    kind: OutputKind,
    /// Hardware-derived default. A `[[output]] scale` policy can still
    /// override this after the backend snapshot reaches the compositor.
    scale: Scale,
    props: OutputAtomicProperties,
    /// Dedicated ARGB8888 KMS cursor plane for this CRTC. Direct scanout may
    /// keep running with a visible cursor only when every output has one.
    cursor: Option<CursorPlane>,
    /// The connector's advertised modes at selection time (deduplicated,
    /// highest resolution first), surfaced through `output_infos`.
    available_modes: Vec<OutputMode>,
    /// HDR/wide-gamut capabilities parsed from the connector's EDID.
    color_caps: aegis_model::edid::EdidColorCapabilities,
}

/// The session-wide color pipeline mode. The compositor renders one shared
/// framebuffer for every output, so the pixel encoding is uniform: HDR
/// engages only when all active outputs allow and support it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum DisplayColorMode {
    /// 8-bit sRGB (the default).
    Sdr,
    /// 10-bit sRGB in an RGB10A2-class container — deep color for banding.
    SdrDeepColor,
    /// BT.2020 PQ (HDR10-class) in an RGB10A2-class container.
    Hdr,
}

impl DisplayColorMode {
    /// The framebuffer fourcc candidates in preference order.
    fn fb_candidates(self) -> &'static [DrmFourcc] {
        match self {
            DisplayColorMode::Sdr => &[DrmFourcc::Xrgb8888, DrmFourcc::Argb8888],
            DisplayColorMode::SdrDeepColor | DisplayColorMode::Hdr => &[
                DrmFourcc::Xbgr2101010,
                DrmFourcc::Abgr2101010,
                DrmFourcc::Xrgb8888,
                DrmFourcc::Argb8888,
            ],
        }
    }
}

#[derive(Debug)]
struct DisplaySet {
    outputs: Vec<Output>,
    size: (u32, u32),
    format: DrmFourcc,
    modifiers: Vec<u64>,
    /// Exact format/modifier intersections accepted by every selected
    /// primary plane. This is broader than the compositor render-target
    /// format above and governs client direct scanout.
    scanout_formats: HashMap<u32, Vec<u64>>,
    overlay: OverlayPlaneInventory,
    /// The negotiated session color pipeline. Derived from `format` and the
    /// requested policy: an 8-bit format means SDR regardless of intent.
    color_mode: DisplayColorMode,
    /// The ICC profile driving the framebuffer's content space (SDR modes
    /// only): the first connected connector with an `icc_profile` config
    /// entry, in connector order.
    icc_profile: Option<String>,
}

/// Current overlay allocation contract. Discovery is useful for diagnostics,
/// but arbitrary desktop layers stay compositor-owned until a future plane
/// planner proves every relevant capability in one atomic TEST_ONLY request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OverlayPlanePolicy {
    CompositorOnly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct OverlayPlaneInventory {
    available: usize,
    policy: OverlayPlanePolicy,
}

#[derive(Debug, Clone)]
struct AvailablePlane {
    handle: plane::Handle,
    possible_crtcs: Vec<crtc::Handle>,
    formats: Vec<u32>,
}

#[derive(Debug, Default)]
struct PlaneInventory {
    primary: Vec<AvailablePlane>,
    cursor: Vec<AvailablePlane>,
    overlay: Vec<AvailablePlane>,
}

type OutputSignature = (String, u32, u32, u32, u32, u32, u32, bool);
type DisplaySignature = (
    DrmFourcc,
    Vec<u64>,
    Vec<(u32, Vec<u64>)>,
    DisplayColorMode,
    Option<String>,
    Vec<OutputSignature>,
    OverlayPlaneInventory,
);

#[derive(Debug)]
struct Scanout {
    framebuffer: control::framebuffer::Handle,
    gem: BufferHandle,
    slot: u32,
    acquire_fence: Option<OwnedFd>,
    ownership: ScanoutOwnership,
}

/// Semantic source selected for the primary plane in this atomic commit.
/// Keeping this explicit prevents a boolean parameter from silently mixing
/// compositor framebuffer lifetime with client `wl_buffer` release rules.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PrimaryPlaneFrame {
    Composited,
    DirectClient,
}

impl PrimaryPlaneFrame {
    fn needs_kms_completion_fence(self) -> bool {
        matches!(self, Self::DirectClient)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ScanoutOwnership {
    /// An uncacheable compositor output import. It is destroyed when the
    /// scanout retires but does not use client `wl_buffer` release semantics.
    TransientCompositor,
    /// A client direct-scanout import belongs to this one presentation and is
    /// destroyed when the scanout retires or its atomic commit fails.
    TransientClient,
    /// A compositor-output import is owned by the backend cache. Scanout
    /// retirement only drops this reference; the cache destroys the DRM
    /// framebuffer and GEM handle after invalidation and the last reference.
    Cached(CompositeFbKey),
}

impl ScanoutOwnership {
    fn matches_frame(self, frame: PrimaryPlaneFrame) -> bool {
        matches!(
            (self, frame),
            (
                Self::TransientCompositor | Self::Cached(_),
                PrimaryPlaneFrame::Composited
            ) | (Self::TransientClient, PrimaryPlaneFrame::DirectClient)
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct DmabufIdentity {
    device: u64,
    inode: u64,
}

/// Complete identity of one Flux compositor-output swapchain image.
///
/// A slot number alone is insufficient: resize or surface recreation reuses
/// slot numbers for new dma-buf objects. The inode identity distinguishes the
/// backing object, while the layout fields prevent reuse after a format or
/// modifier reconfigure. `epoch` makes invalidation explicit even on drivers
/// whose dma-buf inode identity is unexpectedly recycled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct CompositeFbKey {
    epoch: u64,
    slot: u32,
    identity: DmabufIdentity,
    width: u32,
    height: u32,
    stride: u32,
    fourcc: u32,
    modifier: u64,
}

#[derive(Debug)]
struct CachedCompositeFb {
    framebuffer: control::framebuffer::Handle,
    gem: BufferHandle,
    /// Number of submitted/imported `Scanout` values that still reference
    /// this entry. This includes failed commits until their error path releases
    /// the scanout, current scanout, and page-flip-retiring scanout.
    users: u32,
    /// False once another dma-buf replaces the same slot or the surface epoch
    /// is invalidated. Non-reusable entries are destroyed at users == 0.
    reusable: bool,
}

#[derive(Debug)]
struct CursorBuffer {
    framebuffer: control::framebuffer::Handle,
    dumb: control::dumbbuffer::DumbBuffer,
    pixels: Vec<u8>,
    /// Size of the actual cursor art inside the fixed-size transparent KMS
    /// buffer. Plane programming uses the full driver-advertised dumb-buffer
    /// extent, while this value keeps cache lookup exact.
    content_size: (u32, u32),
    /// Cheap pre-filter for the pixel comparison below: sprites with
    /// different lengths or sizes are rejected without touching memory.
    len: usize,
}

impl CursorBuffer {
    fn matches(&self, size: (u32, u32), pixels: &[u8]) -> bool {
        self.content_size == size && self.len == pixels.len() && self.pixels.as_slice() == pixels
    }
}

/// Upper bound on distinct cursor sprites retained as KMS dumb buffers.
/// The set of distinct (shape, quantized-scale) pairs is bounded by the
/// theme (~tens) times active scales; entries beyond this bound are evicted
/// oldest-first so the cache cannot drift upward over a long session.
const MAX_CURSOR_BUFFERS: usize = 64;

#[derive(Debug, Clone, Copy)]
struct CursorState {
    buffer: usize,
    position: (i32, i32),
    hotspot: (u32, u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CursorPlaneRect {
    src: (u32, u32, u32, u32),
    dst: (i32, i32, u32, u32),
}

#[derive(Debug)]
struct ImportedBuffer {
    size: (u32, u32),
    stride: u32,
    modifier: DrmModifier,
    format: DrmFourcc,
    /// Byte offset of the first pixel within the dma-buf object. Zero for the
    /// compositor's own export; a directly-scanned-out client buffer may carry
    /// a non-zero plane offset.
    offset: u32,
    gem: BufferHandle,
}

/// Layout descriptor for a directly-scanned-out client dma-buf. Groups the
/// fields `import_scanout_client` needs beyond the duplicated fd, so the
/// import call stays readable.
struct ClientScanoutDesc {
    width: u32,
    height: u32,
    stride: u32,
    offset: u32,
    format: DrmFourcc,
    modifier: DrmModifier,
}

impl PlanarBuffer for ImportedBuffer {
    fn size(&self) -> (u32, u32) {
        self.size
    }

    fn format(&self) -> DrmFourcc {
        // VK_FORMAT_B8G8R8A8_UNORM has X/ARGB8888 byte layout on little-endian
        // Linux systems. The selected plane format decides whether alpha is
        // ignored or interpreted.
        self.format
    }

    fn modifier(&self) -> Option<DrmModifier> {
        (self.modifier != DrmModifier::Invalid).then_some(self.modifier)
    }

    fn pitches(&self) -> [u32; 4] {
        [self.stride, 0, 0, 0]
    }

    fn handles(&self) -> [Option<BufferHandle>; 4] {
        [Some(self.gem), None, None, None]
    }

    fn offsets(&self) -> [u32; 4] {
        [self.offset, 0, 0, 0]
    }
}

#[derive(Debug, Clone, Copy)]
enum PendingSeatEvent {
    Enable,
    Disable,
}

#[derive(Clone)]
struct SeatInputInterface {
    seat: Rc<RefCell<libseat::Seat>>,
    devices: Rc<RefCell<HashMap<RawFd, libseat::Device>>>,
}

impl LibinputInterface for SeatInputInterface {
    fn open_restricted(&mut self, path: &Path, _flags: i32) -> Result<OwnedFd, i32> {
        let device = self
            .seat
            .borrow_mut()
            .open_device(&path)
            .map_err(|error| error.0)?;
        let raw = device.as_fd().as_raw_fd();
        self.devices.borrow_mut().insert(raw, device);
        // SAFETY: libinput immediately consumes this OwnedFd into its C
        // context. The matching close callback removes the libseat device and
        // hands closure back to libseat without allowing Rust to close it too.
        Ok(unsafe { OwnedFd::from_raw_fd(raw) })
    }

    fn close_restricted(&mut self, fd: OwnedFd) {
        let raw = fd.into_raw_fd();
        if let Some(device) = self.devices.borrow_mut().remove(&raw) {
            if let Err(error) = self.seat.borrow_mut().close_device(device) {
                log::warn!("libseat: failed to close input device fd {raw}: {error:?}");
            }
        } else {
            // Defensive fallback for a descriptor not created by this
            // interface. Ownership arrived through OwnedFd and must be closed.
            unsafe { libc::close(raw) };
        }
    }
}

/// Atomic KMS presentation and libinput event source for all connected outputs
/// on one DRM card. Outputs form one horizontal logical desktop and scan out
/// disjoint source rectangles of a shared dma-buf framebuffer in one atomic
/// commit, so a frame is visually coherent across CRTCs.
pub struct DrmBackend {
    seat: Rc<RefCell<libseat::Seat>>,
    seat_event: Rc<Cell<Option<PendingSeatEvent>>>,
    input_devices: Rc<RefCell<HashMap<RawFd, libseat::Device>>>,
    input: Option<Libinput>,
    hotplug: Option<udev::MonitorSocket>,
    card: Option<Card>,
    displays: DisplaySet,
    active: bool,
    failed: bool,
    modeset_done: bool,
    pending_flips: HashSet<crtc::Handle>,
    current: Option<Scanout>,
    retiring: Option<Scanout>,
    /// KMS framebuffer imports for Flux compositor-output swapchain images.
    /// Direct client scanout deliberately bypasses this cache.
    composite_fb_cache: HashMap<CompositeFbKey, CachedCompositeFb>,
    /// Generation of the live Flux surface/storage. Resize and recreation
    /// advance it before the next exported frame can be imported.
    composite_fb_epoch: u64,
    /// Reused scratch storage for cache reaping; invalidation is rare, but it
    /// should not require a fresh allocation every time.
    composite_fb_reap: Vec<CompositeFbKey>,
    cursor_buffers: Vec<CursorBuffer>,
    cursor_state: Option<CursorState>,
    /// Whether the last successful atomic commit left any cursor plane
    /// enabled. This intentionally survives a software-fallback decision
    /// until a later commit has actually disabled the kernel plane.
    cursor_plane_active: bool,
    cursor_extent: (u32, u32),
    hardware_cursor_failed: bool,
    /// When a disabled cursor plane may be probed again. Cursor commit
    /// rejections are usually transient (a modeset or hotplug in flight), so
    /// the software fallback is not terminal: each consecutive failure
    /// doubles the backoff, and the first successful cursor-active commit
    /// resets the count.
    hardware_cursor_retry_at: Option<std::time::Instant>,
    hardware_cursor_failures: u32,
    input_events: Vec<InputEvent>,
    gesture_events: Vec<PointerGestureEvent>,
    pointer: (f32, f32),
    explicit_sync: bool,
    sync_capable: bool,
    render_ready: bool,
    /// KMS scanout power requested by the session idle policy. Input and
    /// Wayland dispatch remain active while this is false.
    outputs_powered: bool,
    /// Descriptor duplicate of the most recently presented composited
    /// frame, retained so zero-copy stream consumers (IPC protocol 25) can
    /// import exactly what is on screen. Cleared when a direct client
    /// scanout owns the primary plane and on surface recreation.
    presented_composite: Option<PresentedComposite>,
    hotplug_pending: bool,
    pending_resize: Option<Size>,
    /// Modifier intersection the live Flux surface was created with.
    surface_modifiers: Vec<u64>,
    /// Color pipeline mode the live Flux surface was created with.
    surface_color_mode: DisplayColorMode,
    /// ICC profile path the live Flux surface's content space came from.
    surface_icc: Option<String>,
    /// Live `GAMMA_LUT` blob id (night light). Replaced on each new table;
    /// destroyed when neutral restores the driver default ramp.
    gamma_blob: Option<u64>,
    /// Pre-created `HDR_OUTPUT_METADATA` blob (CTA-861.3 static metadata
    /// type 1, BT.2020 primaries, PQ EOTF) shared by every connector in
    /// HDR mode; created/destroyed with the HDR surface.
    hdr_metadata_blob: Option<(property::Value<'static>, u64)>,
    /// Set when a hotplug changed that intersection; the surface must be
    /// recreated (resize alone cannot change a surface's modifier).
    surface_stale: bool,
    /// Per-connector display-mode requests from the config's `[[output]]`
    /// entries (ADR-0028). Consulted on every output (re)selection: startup,
    /// hotplug, and session resume.
    configured_modes: HashMap<String, ModeSpec>,
    /// Per-connector color policy (`hdr` / `deep_color`) from the config's
    /// `[[output]]` entries. Same consultation cadence as `configured_modes`.
    configured_color: HashMap<String, ColorPolicy>,
    /// Per-connector ICC profile paths from the config's `[[output]]`
    /// entries. The chosen profile drives the framebuffer's content space
    /// in SDR modes.
    configured_icc: HashMap<String, String>,
    /// Retained libinput handles for touchpads currently on the seat.
    touchpads: HashMap<String, Device>,
    touchpad_config: TouchpadConfig,
    /// The compositor's Wayland server event-loop fd, registered via
    /// `Backend::set_wakeup_fd`. Polled for readability only — the main loop
    /// dispatches the server itself once the wait wakes.
    wakeup_fd: Option<RawFd>,
}

impl DrmBackend {
    /// Format/modifier pairs accepted by every active primary plane.
    ///
    /// This is the conservative set suitable for the linux-dmabuf feedback
    /// SCANOUT tranche: a client choosing one of these pairs can still be
    /// scanned out when the desktop spans more than one selected output.
    pub fn dmabuf_scanout_formats(&self) -> Vec<aegis_model::dmabuf::DmabufFormat> {
        let mut formats = self
            .displays
            .scanout_formats
            .iter()
            .map(|(&fourcc, modifiers)| aegis_model::dmabuf::DmabufFormat {
                fourcc,
                modifiers: modifiers.clone(),
            })
            .collect::<Vec<_>>();
        formats.sort_by_key(|format| format.fourcc);
        formats
    }

    /// Path and character-device identity of the libseat-owned KMS node.
    ///
    /// The identity comes from the live descriptor rather than a second path
    /// lookup, so Vulkan selection is bound to the exact device libseat
    /// granted even if `/dev/dri` changes concurrently.
    pub fn kms_device(&self) -> Result<(PathBuf, flux::DrmNode), DrmError> {
        let card = self.card.as_ref().ok_or(DrmError::Inactive)?;
        let mut stat = std::mem::MaybeUninit::<libc::stat>::uninit();
        if unsafe { libc::fstat(card.as_fd().as_raw_fd(), stat.as_mut_ptr()) } < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        let stat = unsafe { stat.assume_init() };
        if stat.st_mode & libc::S_IFMT != libc::S_IFCHR {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("{} is not a character device", card.path.display()),
            )
            .into());
        }
        Ok((
            card.path.clone(),
            flux::DrmNode {
                major: libc::major(stat.st_rdev),
                minor: libc::minor(stat.st_rdev),
            },
        ))
    }

    /// Linux `dev_t` for the DRM node that owns the active KMS resources.
    ///
    /// linux-dmabuf feedback accepts either a primary or render node. The
    /// primary node is the strongest identity available from this backend and
    /// is sufficient for Mesa to resolve the matching render node. Returning
    /// `None` keeps the compositor on the legacy v3 protocol rather than
    /// sending an invalid device identity.
    pub fn dmabuf_feedback_device(&self) -> Option<u64> {
        let path = &self.card.as_ref()?.path;
        match self.kms_device() {
            Ok((_, node)) => Some(libc::makedev(node.major, node.minor)),
            Err(error) => {
                log::warn!(
                    "drm: cannot identify live KMS device {} for linux-dmabuf feedback: {error}",
                    path.display()
                );
                None
            }
        }
    }
}

fn wheel_axis(event: &PointerScrollWheelEvent, axis: Axis) -> PointerAxis {
    if !event.has_axis(axis) {
        return PointerAxis::default();
    }
    let value120 = event.scroll_value_v120(axis).round() as i32;
    if value120 == 0 {
        return PointerAxis::default();
    }
    PointerAxis {
        value: Some(value120 as f32 / 12.0),
        discrete: (value120 % 120 == 0).then_some(value120 / 120),
        value120: Some(value120),
        ..PointerAxis::default()
    }
}

fn sequence_axis<T: PointerScrollEvent>(event: &T, axis: Axis, inverted: bool) -> PointerAxis {
    if !event.has_axis(axis) {
        return PointerAxis::default();
    }
    let value = event.scroll_value(axis) as f32;
    PointerAxis {
        value: (value != 0.0).then_some(value),
        stop: value == 0.0,
        relative_direction: (value != 0.0).then_some(if inverted {
            PointerAxisRelativeDirection::Inverted
        } else {
            PointerAxisRelativeDirection::Identical
        }),
        ..PointerAxis::default()
    }
}

#[allow(deprecated)]
fn legacy_axis(
    event: &PointerAxisEvent,
    axis: Axis,
    source: PointerAxisSource,
    inverted: bool,
) -> PointerAxis {
    if !event.has_axis(axis) {
        return PointerAxis::default();
    }
    let raw = event.axis_value(axis) as f32;
    let discrete = event
        .axis_value_discrete(axis)
        .map(|value| value.round() as i32)
        .filter(|value| *value != 0);
    let value = match (source, discrete) {
        (PointerAxisSource::Wheel | PointerAxisSource::WheelTilt, Some(steps)) => {
            steps as f32 * 10.0
        }
        _ => raw,
    };
    PointerAxis {
        value: (value != 0.0).then_some(value),
        discrete,
        stop: value == 0.0
            && matches!(
                source,
                PointerAxisSource::Finger | PointerAxisSource::Continuous
            ),
        relative_direction: (value != 0.0).then_some(if inverted {
            PointerAxisRelativeDirection::Inverted
        } else {
            PointerAxisRelativeDirection::Identical
        }),
        ..PointerAxis::default()
    }
}

impl Backend for DrmBackend {
    fn size(&self) -> Size {
        let (width, height) = self.displays.size;
        Size {
            w: width as i32,
            h: height as i32,
        }
    }

    fn output_infos(&self) -> Vec<aegis_model::output::OutputInfo> {
        self.displays
            .outputs
            .iter()
            .map(|output| {
                let (width, height) = output.mode.size();
                aegis_model::output::OutputInfo {
                    connector: output.name.clone(),
                    geometry: aegis_model::output::OutputGeometry {
                        mode: aegis_model::output::OutputMode {
                            width: width as i32,
                            height: height as i32,
                            refresh_mhz: output.mode.vrefresh().saturating_mul(1_000),
                        },
                        scale: output.scale,
                        transform: aegis_model::Transform::Normal,
                        logical_origin: aegis_model::Point {
                            x: output.x as i32,
                            y: output.y as i32,
                        },
                    },
                    available_modes: output.available_modes.clone(),
                    color_caps: output.color_caps,
                }
            })
            .collect()
    }

    fn set_touchpad_config(&mut self, config: TouchpadConfig) -> TouchpadStatus {
        self.touchpad_config = config;
        for device in self.touchpads.values_mut() {
            Self::apply_touchpad_profile(device, config);
        }
        self.current_touchpad_status()
    }

    fn touchpad_status(&self) -> TouchpadStatus {
        self.current_touchpad_status()
    }

    fn set_configured_modes(&mut self, modes: HashMap<String, ModeSpec>) {
        if self.configured_modes == modes {
            return;
        }
        self.configured_modes = modes;
        // Re-use the hotplug reconciliation path: it waits until every
        // in-flight page flip has retired, probes only advertised modes, and
        // hands a size/modifier change back to the main loop before another
        // frame is acquired. This makes a System Settings mode edit live
        // without racing scanout ownership.
        self.hotplug_pending = true;
    }

    fn set_configured_color_policies(&mut self, policies: HashMap<String, ColorPolicy>) {
        if self.configured_color == policies {
            return;
        }
        self.configured_color = policies;
        // Same reconciliation path as mode changes: re-selection re-runs the
        // color-mode negotiation, and a framebuffer format change flags the
        // Flux surface stale for recreation.
        self.hotplug_pending = true;
    }

    fn color_pipeline(&self) -> aegis_model::output::ColorPipeline {
        match self.displays.color_mode {
            DisplayColorMode::Sdr => aegis_model::output::ColorPipeline::Sdr,
            DisplayColorMode::SdrDeepColor => aegis_model::output::ColorPipeline::SdrDeepColor,
            DisplayColorMode::Hdr => aegis_model::output::ColorPipeline::Hdr,
        }
    }

    fn set_configured_icc_profiles(&mut self, profiles: HashMap<String, String>) {
        if self.configured_icc == profiles {
            return;
        }
        self.configured_icc = profiles;
        // A content-space change needs a surface rebuild, same as a
        // color-mode change.
        self.hotplug_pending = true;
    }

    fn set_gamma_gains(&mut self, gains: Option<[f32; 3]>) {
        let mut request = atomic::AtomicModeReq::new();
        let mut touched = false;
        for output in &self.displays.outputs {
            let Some((prop, size)) = output.props.crtc_gamma_lut else {
                continue;
            };
            let value = match gains {
                Some([red, green, blue]) => {
                    // drm_color_lut { red, green, blue, __reserved } per
                    // entry, linear ramp scaled by the channel gains.
                    let entries = size.max(2) as usize;
                    let mut lut = Vec::with_capacity(entries * 8);
                    for index in 0..entries {
                        let base = index as f32 / (entries - 1) as f32;
                        for gain in [red, green, blue] {
                            let scaled = (base * gain.clamp(0.0, 1.0) * 65535.0).round() as u16;
                            lut.extend_from_slice(&scaled.to_ne_bytes());
                        }
                        lut.extend_from_slice(&0u16.to_ne_bytes());
                    }
                    match self.card().create_property_blob(&lut[..]) {
                        Ok(value @ property::Value::Blob(id)) => {
                            if let Some(previous) = self.gamma_blob.replace(id) {
                                let _ = self.card().destroy_property_blob(previous);
                            }
                            value
                        }
                        other => {
                            log::warn!("drm: GAMMA_LUT blob allocation failed: {other:?}");
                            continue;
                        }
                    }
                }
                // NULL blob restores the driver's default (linear) ramp.
                None => property::Value::Blob(0),
            };
            request.add_property(output.crtc, prop, value);
            touched = true;
        }
        if !touched {
            return;
        }
        // Property-only commit: no modeset, no page-flip event.
        if let Err(error) = self
            .card()
            .atomic_commit(AtomicCommitFlags::empty(), request)
        {
            log::warn!("drm: GAMMA_LUT commit failed: {error}");
        }
        if gains.is_none()
            && let Some(previous) = self.gamma_blob.take()
        {
            let _ = self.card().destroy_property_blob(previous);
        }
    }

    fn dispatch(&mut self) -> bool {
        // Presentation ownership is exposed through `presentation_pending`.
        // Ordinary dispatch returns for *any* event so input and client
        // commits remain responsive while a page flip is in flight.
        self.pump(None, false)
    }

    fn dispatch_nonblocking(&mut self) -> bool {
        self.pump(Some(Duration::ZERO), false)
    }

    fn dispatch_timeout(&mut self, timeout: Duration) -> bool {
        self.pump(Some(timeout), false)
    }

    fn set_wakeup_fd(&mut self, fd: RawFd) {
        self.wakeup_fd = Some(fd);
    }

    fn take_input(&mut self) -> Vec<InputEvent> {
        std::mem::take(&mut self.input_events)
    }

    fn take_resize(&mut self) -> Option<Size> {
        let resize = self.pending_resize.take();
        if resize.is_some() {
            // Runtime will resize or recreate the Flux surface immediately
            // after consuming this event. Prevent same-slot reuse across that
            // storage boundary while retaining any still-scanned framebuffer.
            self.invalidate_composite_fb_cache();
        }
        resize
    }

    fn take_pointer_gestures(&mut self) -> Vec<PointerGestureEvent> {
        std::mem::take(&mut self.gesture_events)
    }

    fn is_active(&self) -> bool {
        DrmBackend::is_active(self)
    }

    fn outputs_powered(&self) -> bool {
        self.outputs_powered
    }

    fn presentation_target_ready(&self) -> bool {
        self.render_ready
    }

    fn set_outputs_powered(&mut self, powered: bool) -> Result<(), String> {
        self.set_outputs_powered(powered)
            .map_err(|error| error.to_string())
    }

    fn presentation_pending(&self) -> bool {
        !self.pending_flips.is_empty()
    }

    fn switch_vt(&mut self, vt: i32) {
        // libseat asks the session manager (logind/seatd) to activate the
        // target VT; our own Disable/Enable cycle follows on the seat fd.
        if let Err(error) = self.seat.borrow_mut().switch_session(vt) {
            log::warn!("drm: VT switch to {vt} failed: {error}");
        }
    }

    fn surface_needs_recreate(&self) -> bool {
        self.surface_stale
    }
}

impl Drop for DrmBackend {
    fn drop(&mut self) {
        if self.active
            && !self.pending_flips.is_empty()
            && let Err(error) = self.wait_for_flip(Duration::from_secs(1))
        {
            log::warn!("drm: page flip did not settle during shutdown: {error}");
        }
        if self.active
            && self.modeset_done
            && let Err(error) = self.disable_outputs()
        {
            log::warn!("drm: failed to disable output during shutdown: {error}");
        }

        // Dropping/suspending libinput invokes close_restricted; the seat must
        // still exist while those libseat device IDs are returned.
        if let Some(input) = self.input.take() {
            input.suspend();
            drop(input);
        }
        debug_assert!(self.input_devices.borrow().is_empty());

        if self.active {
            if let Some(scanout) = self.retiring.take() {
                self.release_scanout(scanout);
            }
            if let Some(scanout) = self.current.take() {
                self.release_scanout(scanout);
            }
            // Cache owns compositor-output framebuffer/GEM resources; release
            // all of them exactly once while the originating card fd is live.
            if self.card.is_some() {
                self.destroy_composite_fb_cache();
            }
        } else {
            // libseat has revoked this fd. Kernel resources died with it, and
            // cleanup ioctls must not be sent to a future/reused handle.
            self.retiring = None;
            self.current = None;
            self.forget_composite_fb_cache();
        }
        if self.active
            && let Some(card) = self.card.as_ref()
        {
            for cursor in self.cursor_buffers.drain(..) {
                if let Err(error) = card.destroy_framebuffer(cursor.framebuffer) {
                    log::warn!("DRM: failed to destroy cursor framebuffer: {error}");
                }
                if let Err(error) = card.destroy_dumb_buffer(cursor.dumb) {
                    log::warn!("DRM: failed to destroy cursor buffer: {error}");
                }
            }
        } else {
            // Revoked card: the kernel destroyed these records with the fd.
            self.cursor_buffers.clear();
        }
        if let Some(card) = self.card.take() {
            if self.active {
                for output in &self.displays.outputs {
                    let _ = card.destroy_property_blob(output.mode_blob_id);
                    if let Some((_, blob_id)) = output.primary.full_damage_blob.as_ref() {
                        let _ = card.destroy_property_blob(*blob_id);
                    }
                }
            }
            if let Err(error) = self.seat.borrow_mut().close_device(card.device) {
                log::warn!(
                    "libseat: failed to close {}: {error:?}",
                    card.path.display()
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scanout_ownership_must_match_primary_plane_source() {
        assert!(ScanoutOwnership::TransientCompositor.matches_frame(PrimaryPlaneFrame::Composited));
        assert!(
            !ScanoutOwnership::TransientCompositor.matches_frame(PrimaryPlaneFrame::DirectClient)
        );
        assert!(ScanoutOwnership::TransientClient.matches_frame(PrimaryPlaneFrame::DirectClient));
        assert!(!ScanoutOwnership::TransientClient.matches_frame(PrimaryPlaneFrame::Composited));
    }

    #[test]
    fn damage_clip_intersects_output_rect() {
        use aegis_model::Rect;
        // Unknown damage covers the whole output.
        assert_eq!(
            damage_clip_for_output(None, 1920, 0, 1920, 1080),
            vec![[1920, 0, 3840, 1080]]
        );
        // A box spanning both outputs of a side-by-side desktop clips to each.
        let damage_rects = [Rect::new(1900, 100, 100, 200)];
        let damage = Some(&damage_rects[..]);
        assert_eq!(
            damage_clip_for_output(damage, 0, 0, 1920, 1080),
            vec![[1900, 100, 1920, 300]]
        );
        assert_eq!(
            damage_clip_for_output(damage, 1920, 0, 1920, 1080),
            vec![[1920, 100, 2000, 300]]
        );
        // Disjoint damage leaves the output untouched.
        let other_rects = [Rect::new(0, 0, 100, 100)];
        let other = Some(&other_rects[..]);
        assert_eq!(
            damage_clip_for_output(other, 1920, 0, 1920, 1080),
            Vec::<[i32; 4]>::new()
        );
        // Damage fully outside the framebuffer on the negative side.
        let negative_rects = [Rect::new(-50, -50, 40, 40)];
        let negative = Some(&negative_rects[..]);
        assert_eq!(
            damage_clip_for_output(negative, 0, 0, 1920, 1080),
            Vec::<[i32; 4]>::new()
        );
        // Disjoint dirty regions are preserved as separate clips.
        let disjoint_rects = [Rect::new(10, 10, 5, 5), Rect::new(500, 400, 8, 8)];
        let disjoint = Some(&disjoint_rects[..]);
        assert_eq!(
            damage_clip_for_output(disjoint, 0, 0, 1920, 1080),
            vec![[10, 10, 15, 15], [500, 400, 508, 408]]
        );
    }

    #[test]
    fn cursor_plane_rect_clips_hotspot_and_output_boundaries() {
        assert_eq!(
            cursor_plane_rect((100, 80), (4, 6), (24, 24), (0, 0), (1920, 1080)),
            Some(CursorPlaneRect {
                src: (0, 0, 24, 24),
                dst: (96, 74, 24, 24),
            })
        );
        // Hotspot near the top-left keeps the full source and lets KMS clip a
        // negative destination, as cursor planes conventionally require.
        assert_eq!(
            cursor_plane_rect((2, 3), (6, 7), (24, 24), (0, 0), (1920, 1080)),
            Some(CursorPlaneRect {
                src: (0, 0, 24, 24),
                dst: (-4, -4, 24, 24),
            })
        );
        // The same sprite can be committed to two cursor planes while it
        // straddles side-by-side outputs.
        assert_eq!(
            cursor_plane_rect((1920, 100), (12, 12), (24, 24), (0, 0), (1920, 1080)),
            Some(CursorPlaneRect {
                src: (0, 0, 24, 24),
                dst: (1908, 88, 24, 24),
            })
        );
        assert_eq!(
            cursor_plane_rect((1920, 100), (12, 12), (24, 24), (1920, 0), (1920, 1080),),
            Some(CursorPlaneRect {
                src: (0, 0, 24, 24),
                dst: (-12, 88, 24, 24),
            })
        );
    }

    #[test]
    fn poll_deadline_shapes_timeout_values() {
        // No deadline blocks indefinitely and never expires.
        assert_eq!(poll_ms_remaining(None), -1);
        assert!(!deadline_passed(None));

        // A future deadline yields a bounded positive remaining budget.
        let future = std::time::Instant::now() + Duration::from_secs(60);
        let remaining = poll_ms_remaining(Some(future));
        assert!((1..=60_000).contains(&remaining));
        assert!(!deadline_passed(Some(future)));

        // A passed deadline degrades to a non-blocking poll.
        let past = std::time::Instant::now() - Duration::from_secs(1);
        assert_eq!(poll_ms_remaining(Some(past)), 0);
        assert!(deadline_passed(Some(past)));
    }

    #[test]
    fn busy_nonblocking_commit_is_a_transient_frame_skip() {
        let error = std::io::Error::from_raw_os_error(libc::EBUSY);
        assert!(commit_error_is_transient(&error));
        assert!(matches!(commit_error(error), DrmError::Busy));
    }

    #[test]
    fn card_candidates_honor_explicit_override() {
        assert_eq!(
            candidate_cards_with_override(Some("/dev/dri/card-test".into())),
            vec![PathBuf::from("/dev/dri/card-test")]
        );
    }

    #[test]
    fn imported_buffer_exposes_single_xrgb_plane() {
        let buffer = ImportedBuffer {
            size: (1920, 1080),
            stride: 7680,
            modifier: DrmModifier::Linear,
            format: DrmFourcc::Xrgb8888,
            offset: 0,
            gem: BufferHandle::from(std::num::NonZeroU32::new(1).unwrap()),
        };
        assert_eq!(buffer.size(), (1920, 1080));
        assert_eq!(buffer.format(), DrmFourcc::Xrgb8888);
        assert_eq!(buffer.pitches(), [7680, 0, 0, 0]);
        assert_eq!(buffer.offsets(), [0; 4]);
        assert_eq!(buffer.modifier(), Some(DrmModifier::Linear));
    }

    #[test]
    fn parses_in_formats_modifier_bitsets() {
        // Header + two fourcc values + one aligned modifier record.
        let mut blob = vec![0_u8; 56];
        blob[8..12].copy_from_slice(&2_u32.to_ne_bytes());
        blob[12..16].copy_from_slice(&24_u32.to_ne_bytes());
        blob[16..20].copy_from_slice(&1_u32.to_ne_bytes());
        blob[20..24].copy_from_slice(&32_u32.to_ne_bytes());
        blob[24..28].copy_from_slice(&(DrmFourcc::Argb8888 as u32).to_ne_bytes());
        blob[28..32].copy_from_slice(&(DrmFourcc::Xrgb8888 as u32).to_ne_bytes());
        blob[32..40].copy_from_slice(&0b10_u64.to_ne_bytes());
        blob[40..44].copy_from_slice(&0_u32.to_ne_bytes());
        blob[48..56].copy_from_slice(&0x1234_u64.to_ne_bytes());

        assert_eq!(
            parse_format_modifiers(&blob, DrmFourcc::Xrgb8888 as u32).unwrap(),
            vec![0x1234]
        );
        assert!(
            parse_format_modifiers(&blob, DrmFourcc::Argb8888 as u32)
                .unwrap()
                .is_empty()
        );
    }

    #[test]
    fn native_modifier_precedes_linear_fallback() {
        // Preserve deterministic preference even when the kernel blob lists
        // LINEAR first: output composition should use a GPU-native tiled
        // layout whenever the primary plane and Vulkan both accept it.
        let mut blob = vec![0_u8; 80];
        blob[8..12].copy_from_slice(&1_u32.to_ne_bytes());
        blob[12..16].copy_from_slice(&24_u32.to_ne_bytes());
        blob[16..20].copy_from_slice(&2_u32.to_ne_bytes());
        blob[20..24].copy_from_slice(&32_u32.to_ne_bytes());
        blob[24..28].copy_from_slice(&(DrmFourcc::Xrgb8888 as u32).to_ne_bytes());

        let linear = u64::from(DrmModifier::Linear);
        let tiled = u64::from(DrmModifier::I915_x_tiled);
        for (base, modifier) in [(32, linear), (56, tiled)] {
            blob[base..base + 8].copy_from_slice(&1_u64.to_ne_bytes());
            blob[base + 8..base + 12].copy_from_slice(&0_u32.to_ne_bytes());
            blob[base + 16..base + 24].copy_from_slice(&modifier.to_ne_bytes());
        }

        assert_eq!(
            parse_format_modifiers(&blob, DrmFourcc::Xrgb8888 as u32).unwrap(),
            vec![tiled, linear]
        );
    }

    #[test]
    fn rejects_out_of_bounds_in_formats_blob() {
        let mut blob = vec![0_u8; 24];
        blob[8..12].copy_from_slice(&u32::MAX.to_ne_bytes());
        blob[12..16].copy_from_slice(&24_u32.to_ne_bytes());
        assert!(parse_format_modifiers(&blob, DrmFourcc::Xrgb8888 as u32).is_err());
    }

    #[test]
    fn output_assignment_backtracks_and_intersects_modifiers() {
        let raw = |value| std::num::NonZeroU32::new(value).unwrap();
        // Mode is a transparent kernel modeinfo value and assignment never
        // reads it; all-zero modeinfo is valid test storage.
        let mode: Mode = unsafe { std::mem::zeroed() };
        let candidates = vec![
            OutputCandidate {
                connector: connector::Handle::from(raw(1)),
                name: "A".into(),
                mode,
                physical_size_mm: None,
                ppi: None,
                kind: OutputKind::External,
                scale: Scale::IDENTITY,
                choices: vec![
                    OutputChoice {
                        crtc: crtc::Handle::from(raw(10)),
                        plane: plane::Handle::from(raw(20)),
                        modifiers: vec![0, 7],
                    },
                    OutputChoice {
                        crtc: crtc::Handle::from(raw(11)),
                        plane: plane::Handle::from(raw(21)),
                        modifiers: vec![0],
                    },
                ],
                available_modes: Vec::new(),
                color_caps: aegis_model::edid::EdidColorCapabilities::default(),
            },
            OutputCandidate {
                connector: connector::Handle::from(raw(2)),
                name: "B".into(),
                mode,
                physical_size_mm: None,
                ppi: None,
                kind: OutputKind::External,
                scale: Scale::IDENTITY,
                choices: vec![OutputChoice {
                    crtc: crtc::Handle::from(raw(10)),
                    plane: plane::Handle::from(raw(20)),
                    modifiers: vec![0, 9],
                }],
                available_modes: Vec::new(),
                color_caps: aegis_model::edid::EdidColorCapabilities::default(),
            },
        ];

        let (selected, modifiers) = assign_outputs(&candidates).unwrap();
        assert_eq!(selected[0].crtc, crtc::Handle::from(raw(11)));
        assert_eq!(selected[1].crtc, crtc::Handle::from(raw(10)));
        assert_eq!(modifiers, vec![0]);
    }

    #[test]
    fn scanout_capabilities_are_checked_per_client_format() {
        let mut formats = HashMap::new();
        formats.insert(DrmFourcc::Xrgb8888 as u32, vec![0, 7]);
        formats.insert(DrmFourcc::Argb8888 as u32, vec![0, 9]);

        assert!(device::scanout_formats_support(
            &formats,
            DrmFourcc::Argb8888 as u32,
            9
        ));
        assert!(!device::scanout_formats_support(
            &formats,
            DrmFourcc::Argb8888 as u32,
            7
        ));
        assert!(!device::scanout_formats_support(
            &formats,
            DrmFourcc::Abgr8888 as u32,
            0
        ));
    }

    #[test]
    fn scanout_modifier_intersection_requires_every_output() {
        assert_eq!(
            output::intersect_modifier_sets(&[vec![0, 7, 9], vec![0, 9], vec![9, 11]]),
            vec![9]
        );
        assert!(output::intersect_modifier_sets(&[]).is_empty());
    }

    #[test]
    fn pick_mode_without_spec_uses_highest_pixel_count_then_refresh() {
        let modes = [
            (1920, 1080, 60_000, false),
            (2560, 1440, 60_000, true),
            (2560, 1440, 144_000, false),
            (3840, 2160, 60_000, false),
        ];
        assert_eq!(pick_mode(&modes, None), Some(3));

        let same_resolution = [(2560, 1440, 60_000, true), (2560, 1440, 144_000, false)];
        assert_eq!(pick_mode(&same_resolution, None), Some(1));
        assert_eq!(pick_mode(&[], None), None);
    }

    #[test]
    fn pick_mode_with_spec_requires_exact_size() {
        let modes = [(1920, 1080, 60_000, false), (2560, 1440, 60_000, true)];
        let spec: ModeSpec = "1920x1080".parse().unwrap();
        assert_eq!(pick_mode(&modes, Some(&spec)), Some(0));
        // Nothing matches → None so the caller can fall back and warn.
        let missing: ModeSpec = "1280x720".parse().unwrap();
        assert_eq!(pick_mode(&modes, Some(&missing)), None);
    }

    #[test]
    fn pick_mode_with_spec_uses_highest_refresh_then_tie_breakers() {
        let modes = [
            (1920, 1080, 60_000, false),
            (1920, 1080, 144_000, false),
            (1920, 1080, 75_000, true),
        ];
        let spec: ModeSpec = "1920x1080".parse().unwrap();
        assert_eq!(pick_mode(&modes, Some(&spec)), Some(1));
        // An exact refresh request selects only that rate (rounded).
        let hz: ModeSpec = "1920x1080@144".parse().unwrap();
        assert_eq!(pick_mode(&modes, Some(&hz)), Some(1));
        let odd: ModeSpec = "1920x1080@75".parse().unwrap();
        assert_eq!(pick_mode(&modes, Some(&odd)), Some(2));
    }
}
