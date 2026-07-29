//! Direct-display backend built on atomic DRM/KMS, libseat, and libinput.
//!
//! Flux renders into exportable offscreen Vulkan images. Each completed image
//! is imported as a DRM framebuffer and attached to the primary plane with an
//! atomic commit. The device runs three in-flight frame slots (see
//! `host::FRAMES_IN_FLIGHT`), so the slot a new frame renders into was retired
//! from scanout one flip earlier — rendering never aliases the image the CRTC
//! is still scanning. The page-flip wait in `present` is the ownership
//! boundary that keeps that invariant one commit deep.

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

use output::*;

use aegis_core::Size;
use aegis_core::input::{
    ButtonState, InputEvent, PointerAxis, PointerAxisFrame, PointerAxisRelativeDirection,
    PointerAxisSource, PointerGestureEvent, TabletEvent, TabletToolInfo, TouchpadCapabilities,
    TouchpadConfig, TouchpadScrollMethod, TouchpadStatus,
};
use aegis_core::output::{ModeSpec, OutputMode};
use drm::Device as BasicDevice;
use drm::buffer::{DrmFourcc, DrmModifier, Handle as BufferHandle, PlanarBuffer};
use drm::control::{
    self, AtomicCommitFlags, Device as ControlDevice, FbCmd2Flags, Mode, ModeTypeFlags,
    ResourceHandle, atomic, connector, crtc, plane, property,
};
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
    #[error("timed out waiting for a KMS page flip")]
    FlipTimeout,
    #[error("display set changed during presentation; frame skipped")]
    Reconfigured,
    #[error("client buffer cannot be scanned out directly")]
    ScanoutUnsupported,
}

/// Map an atomic-commit failure to a backend error. EACCES/EPERM means the
/// session lost DRM master to a VT switch whose seat Disable event has not
/// been dispatched yet — a transient, frame-skip condition that must never
/// kill the compositor.
fn commit_error(error: std::io::Error) -> DrmError {
    match error.raw_os_error() {
        Some(libc::EACCES) | Some(libc::EPERM) => {
            log::warn!("drm: commit while masterless (VT switch in flight); skipping frame");
            DrmError::Inactive
        }
        _ => DrmError::Io(error),
    }
}

/// Clip a desktop-wide damage bounding box (physical framebuffer pixels) to
/// one output's scanout rectangle, yielding the `drm_mode_rect` fields
/// `(x1, y1, x2, y2)` for `FB_DAMAGE_CLIPS`. `None` damage means "unknown"
/// and covers the whole output; `None` result means the output is untouched
/// by this frame's damage.
fn damage_clip_for_output(
    damage: Option<aegis_core::Rect>,
    output_x: u32,
    output_y: u32,
    width: u32,
    height: u32,
) -> Option<[i32; 4]> {
    let (ox, oy) = (output_x as i32, output_y as i32);
    let full = [ox, oy, ox + width as i32, oy + height as i32];
    let Some(damage) = damage else {
        return Some(full);
    };
    let x1 = damage.origin.x.max(ox);
    let y1 = damage.origin.y.max(oy);
    let x2 = (damage.origin.x.saturating_add(damage.size.w)).min(full[2]);
    let y2 = (damage.origin.y.saturating_add(damage.size.h)).min(full[3]);
    (x2 > x1 && y2 > y1).then_some([x1, y1, x2, y2])
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
struct AtomicProperties {
    connector_crtc_id: property::Handle,
    crtc_mode_id: property::Handle,
    crtc_active: property::Handle,
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
    plane_in_fence_fd: Option<property::Handle>,
    /// `FB_DAMAGE_CLIPS`: per-commit damage hint consumed by PSR-style
    /// drivers. Absent on planes/kernels without damage tracking; commits
    /// then carry no hint at all.
    plane_fb_damage_clips: Option<property::Handle>,
}

#[derive(Debug)]
struct Output {
    connector: connector::Handle,
    name: String,
    crtc: crtc::Handle,
    plane: plane::Handle,
    mode: Mode,
    mode_blob: property::Value<'static>,
    mode_blob_id: u64,
    x: u32,
    y: u32,
    props: AtomicProperties,
    /// Pre-created `FB_DAMAGE_CLIPS` blob covering the output's whole
    /// framebuffer rectangle, reused by every commit whose damage is unknown
    /// or spans the output. Present only when the plane exposes the property.
    full_damage_blob: Option<(property::Value<'static>, u64)>,
    /// The connector's advertised modes at selection time (deduplicated,
    /// highest resolution first), surfaced through `output_infos`.
    available_modes: Vec<OutputMode>,
}

#[derive(Debug)]
struct DisplaySet {
    outputs: Vec<Output>,
    size: (u32, u32),
    format: DrmFourcc,
    modifiers: Vec<u64>,
}

type OutputSignature = (String, u32, u32, u32, u32, u32);
type DisplaySignature = (DrmFourcc, Vec<u64>, Vec<OutputSignature>);

#[derive(Debug)]
struct Scanout {
    framebuffer: control::framebuffer::Handle,
    gem: BufferHandle,
    slot: u32,
    acquire_fence: Option<OwnedFd>,
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
    input_events: Vec<InputEvent>,
    gesture_events: Vec<PointerGestureEvent>,
    pointer: (f32, f32),
    explicit_sync: bool,
    sync_capable: bool,
    render_ready: bool,
    hotplug_pending: bool,
    pending_resize: Option<Size>,
    /// Modifier intersection the live Flux surface was created with.
    surface_modifiers: Vec<u64>,
    /// Set when a hotplug changed that intersection; the surface must be
    /// recreated (resize alone cannot change a surface's modifier).
    surface_stale: bool,
    /// Per-connector display-mode requests from the config's `[[output]]`
    /// entries (ADR-0028). Consulted on every output (re)selection: startup,
    /// hotplug, and session resume.
    configured_modes: HashMap<String, ModeSpec>,
    /// Retained libinput handles for touchpads currently on the seat.
    touchpads: HashMap<String, Device>,
    touchpad_config: TouchpadConfig,
    /// The compositor's Wayland server event-loop fd, registered via
    /// `Backend::set_wakeup_fd`. Polled for readability only — the main loop
    /// dispatches the server itself once the wait wakes.
    wakeup_fd: Option<RawFd>,
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

    fn output_infos(&self) -> Vec<aegis_core::output::OutputInfo> {
        self.displays
            .outputs
            .iter()
            .map(|output| {
                let (width, height) = output.mode.size();
                aegis_core::output::OutputInfo {
                    connector: output.name.clone(),
                    geometry: aegis_core::output::OutputGeometry {
                        mode: aegis_core::output::OutputMode {
                            width: width as i32,
                            height: height as i32,
                            refresh_mhz: output.mode.vrefresh().saturating_mul(1_000),
                        },
                        scale: aegis_core::output::Scale::IDENTITY,
                        transform: aegis_core::Transform::Normal,
                        logical_origin: aegis_core::Point {
                            x: output.x as i32,
                            y: output.y as i32,
                        },
                    },
                    available_modes: output.available_modes.clone(),
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

    fn dispatch(&mut self) -> bool {
        self.pump(None, !self.pending_flips.is_empty())
    }

    fn dispatch_nonblocking(&mut self) -> bool {
        // A pending page flip is a hard dma-buf ownership boundary, so even an
        // animating client must wait for vblank before acquiring another Flux
        // slot. With no flip pending this is genuinely non-blocking.
        let pending = !self.pending_flips.is_empty();
        let timeout = if pending {
            Duration::from_secs(1)
        } else {
            Duration::ZERO
        };
        self.pump(Some(timeout), pending)
    }

    fn dispatch_timeout(&mut self, timeout: Duration) -> bool {
        self.pump(Some(timeout), !self.pending_flips.is_empty())
    }

    fn set_wakeup_fd(&mut self, fd: RawFd) {
        self.wakeup_fd = Some(fd);
    }

    fn take_input(&mut self) -> Vec<InputEvent> {
        std::mem::take(&mut self.input_events)
    }

    fn take_resize(&mut self) -> Option<Size> {
        self.pending_resize.take()
    }

    fn take_pointer_gestures(&mut self) -> Vec<PointerGestureEvent> {
        std::mem::take(&mut self.gesture_events)
    }

    fn is_active(&self) -> bool {
        DrmBackend::is_active(self)
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

        if let Some(scanout) = self.retiring.take() {
            self.release_scanout(scanout);
        }
        if let Some(scanout) = self.current.take() {
            self.release_scanout(scanout);
        }
        if let Some(card) = self.card.take() {
            for output in &self.displays.outputs {
                let _ = card.destroy_property_blob(output.mode_blob_id);
                if let Some((_, blob_id)) = output.full_damage_blob.as_ref() {
                    let _ = card.destroy_property_blob(*blob_id);
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
    fn damage_clip_intersects_output_rect() {
        use aegis_core::Rect;
        // Unknown damage covers the whole output.
        assert_eq!(
            damage_clip_for_output(None, 1920, 0, 1920, 1080),
            Some([1920, 0, 3840, 1080])
        );
        // A box spanning both outputs of a side-by-side desktop clips to each.
        let damage = Some(Rect::new(1900, 100, 100, 200));
        assert_eq!(
            damage_clip_for_output(damage, 0, 0, 1920, 1080),
            Some([1900, 100, 1920, 300])
        );
        assert_eq!(
            damage_clip_for_output(damage, 1920, 0, 1920, 1080),
            Some([1920, 100, 2000, 300])
        );
        // Disjoint damage leaves the output untouched.
        let other = Some(Rect::new(0, 0, 100, 100));
        assert_eq!(damage_clip_for_output(other, 1920, 0, 1920, 1080), None);
        // Damage fully outside the framebuffer on the negative side.
        let negative = Some(Rect::new(-50, -50, 40, 40));
        assert_eq!(damage_clip_for_output(negative, 0, 0, 1920, 1080), None);
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
            },
            OutputCandidate {
                connector: connector::Handle::from(raw(2)),
                name: "B".into(),
                mode,
                choices: vec![OutputChoice {
                    crtc: crtc::Handle::from(raw(10)),
                    plane: plane::Handle::from(raw(20)),
                    modifiers: vec![0, 9],
                }],
                available_modes: Vec::new(),
            },
        ];

        let (selected, modifiers) = assign_outputs(&candidates).unwrap();
        assert_eq!(selected[0].crtc, crtc::Handle::from(raw(11)));
        assert_eq!(selected[1].crtc, crtc::Handle::from(raw(10)));
        assert_eq!(modifiers, vec![0]);
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
