//! Direct-display backend built on atomic DRM/KMS, libseat, and libinput.
//!
//! Flux renders into exportable offscreen Vulkan images. Each completed image
//! is imported as a DRM framebuffer and attached to the primary plane with an
//! atomic commit. The event loop waits for the page-flip event before allowing
//! Flux to acquire another frame, which is the ownership boundary required by
//! the two-image offscreen ring.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};
use std::ops::RangeBounds;
use std::os::fd::{AsFd, AsRawFd, BorrowedFd, FromRawFd, IntoRawFd, OwnedFd, RawFd};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use ass_core::input::{
    ButtonState, InputEvent, PointerAxis, PointerAxisFrame, PointerAxisRelativeDirection,
    PointerAxisSource, PointerGestureEvent, TabletEvent, TabletToolInfo, TouchpadCapabilities,
    TouchpadConfig, TouchpadScrollMethod, TouchpadStatus,
};
use ass_core::output::{ModeSpec, OutputMode};
use ass_core::Size;
use drm::buffer::{DrmFourcc, DrmModifier, Handle as BufferHandle, PlanarBuffer};
use drm::control::{
    self, atomic, connector, crtc, plane, property, AtomicCommitFlags, Device as ControlDevice,
    FbCmd2Flags, Mode, ModeTypeFlags, ResourceHandle,
};
use drm::Device as BasicDevice;
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
use input::event::EventTrait;
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
    gem: BufferHandle,
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
        [0; 4]
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

impl DrmBackend {
    /// Acquire the seat, choose a connected DRM card/output, enable atomic
    /// modesetting, and attach libinput to the same seat. `configured_modes`
    /// carries the config's per-connector `mode` requests (ADR-0028) so the
    /// very first modeset already honors them.
    pub fn open(configured_modes: HashMap<String, ModeSpec>) -> Result<Self, DrmError> {
        let pending = Rc::new(Cell::new(None));
        let callback_pending = Rc::clone(&pending);
        let seat = libseat::Seat::open(move |_seat, event| {
            callback_pending.set(Some(match event {
                libseat::SeatEvent::Enable => PendingSeatEvent::Enable,
                libseat::SeatEvent::Disable => PendingSeatEvent::Disable,
            }));
        })
        .map_err(|error| DrmError::Seat(format!("open failed: {error:?}")))?;
        let seat = Rc::new(RefCell::new(seat));

        // libseat delivers the initial enable asynchronously on some backends.
        // Do not touch device nodes until it has granted the session.
        log::info!("libseat: waiting for seat to become active; switch VT if this hangs");
        while !matches!(pending.take(), Some(PendingSeatEvent::Enable)) {
            seat.borrow_mut()
                .dispatch(-1)
                .map_err(|error| DrmError::Seat(format!("initial dispatch: {error:?}")))?;
        }

        let (card, displays) = open_card_and_outputs(&seat, &configured_modes)?;
        let size = displays.size;

        let input_devices = Rc::new(RefCell::new(HashMap::new()));
        let interface = SeatInputInterface {
            seat: Rc::clone(&seat),
            devices: Rc::clone(&input_devices),
        };
        let mut input = Libinput::new_with_udev(interface);
        let seat_name = seat.borrow_mut().name().to_owned();
        input
            .udev_assign_seat(&seat_name)
            .map_err(|()| DrmError::InputSeat(seat_name.clone()))?;
        input.dispatch()?;
        let hotplug = match udev::MonitorBuilder::new()
            .and_then(|monitor| monitor.match_subsystem("drm"))
            .and_then(udev::MonitorBuilder::listen)
        {
            Ok(monitor) => Some(monitor),
            Err(error) => {
                log::warn!("drm: hotplug monitoring unavailable: {error}");
                None
            }
        };

        log::info!(
            "drm: {} output(s) on {} using {} (desktop {}x{}, {:?}, {} shared modifier(s))",
            displays.outputs.len(),
            card.path.display(),
            seat_name,
            size.0,
            size.1,
            displays.format,
            displays.modifiers.len()
        );
        for output in &displays.outputs {
            let mode = output.mode.size();
            log::info!(
                "drm: {} at {},{}: {}x{} @ {} Hz",
                output.name,
                output.x,
                output.y,
                mode.0,
                mode.1,
                output.mode.vrefresh()
            );
        }

        let mut backend = Self {
            seat,
            seat_event: pending,
            input_devices,
            input: Some(input),
            hotplug,
            card: Some(card),
            displays,
            active: true,
            failed: false,
            modeset_done: false,
            pending_flips: HashSet::new(),
            current: None,
            retiring: None,
            input_events: Vec::new(),
            gesture_events: Vec::new(),
            pointer: (size.0 as f32 * 0.5, size.1 as f32 * 0.5),
            explicit_sync: false,
            sync_capable: false,
            render_ready: true,
            hotplug_pending: false,
            pending_resize: None,
            surface_modifiers: Vec::new(),
            surface_stale: false,
            configured_modes,
            touchpads: HashMap::new(),
            touchpad_config: TouchpadConfig::default(),
        };
        // udev_assign_seat + dispatch queues the initial DeviceAdded events
        // even before the fd becomes pollable. Drain them now so Settings can
        // report devices without waiting for the user's first touch.
        backend.drain_input_queue();
        Ok(backend)
    }

    /// Create Flux's exportable offscreen target at the selected display mode.
    pub fn create_surface(&mut self, device: &flux::Device) -> Result<flux::Surface, DrmError> {
        let (width, height) = self.physical_size();
        let surface =
            flux::Surface::offscreen_dmabuf(device, width, height, &self.displays.modifiers)?;
        if !surface.is_exportable() {
            return Err(DrmError::DmabufUnsupported);
        }
        // Remember which intersection the surface was built with; a hotplug
        // that changes it flags the surface stale until it is recreated here.
        self.surface_modifiers = self.displays.modifiers.clone();
        self.surface_stale = false;
        self.sync_capable = flux::dmabuf_sync_supported(device);
        self.explicit_sync = self
            .displays
            .outputs
            .iter()
            .all(|output| output.props.plane_in_fence_fd.is_some())
            && self.sync_capable;
        log::info!(
            "drm: explicit synchronization {}",
            if self.explicit_sync {
                "enabled"
            } else {
                "unavailable; using GPU fence wait"
            }
        );
        Ok(surface)
    }

    /// Complete a Flux frame, export it, and queue it on the primary plane.
    /// The next backend dispatch waits for its page-flip event before another
    /// frame may be rendered.
    pub fn present(
        &mut self,
        surface: &flux::Surface,
        frame: flux::SubmittedFrame<'_>,
    ) -> Result<Option<OwnedFd>, DrmError> {
        if !self.active || !self.render_ready {
            return Err(DrmError::Inactive);
        }
        if !self.pending_flips.is_empty() {
            self.wait_for_flip(Duration::from_secs(1))?;
        }
        // The wait above pumps session and hotplug events, either of which may
        // have changed the world under us. A queued resize means the display
        // set was swapped and this frame was rendered for the old topology:
        // skip it instead of committing a stale buffer against the new set.
        if !self.active || !self.render_ready {
            return Err(DrmError::Inactive);
        }
        if self.pending_resize.is_some() {
            return Err(DrmError::Reconfigured);
        }
        frame.present()?;
        let dmabuf = if self.explicit_sync {
            surface.export_dmabuf_explicit()?
        } else {
            surface.export_dmabuf()?
        };
        let completion_fence = dmabuf
            .acquire_fence
            .as_ref()
            .map(OwnedFd::try_clone)
            .transpose()?;
        let scanout = self.import_scanout(dmabuf)?;
        self.commit_scanout(scanout)?;
        Ok(completion_fence)
    }

    pub fn is_active(&self) -> bool {
        self.active && self.render_ready
    }

    /// Wait until the most recently queued atomic commit has generated a
    /// page-flip event for every active CRTC. Session-lock uses this barrier
    /// before telling the locker that the secure frame is visible.
    pub fn wait_presented(&mut self) -> Result<(), DrmError> {
        if self.pending_flips.is_empty() {
            return Ok(());
        }
        self.wait_for_flip(Duration::from_secs(1))
    }

    fn card(&self) -> &Card {
        self.card.as_ref().expect("DRM card exists until drop")
    }

    fn import_scanout(&self, dmabuf: flux::SurfaceDmabuf) -> Result<Scanout, DrmError> {
        let card = self.card();
        let gem = card.prime_fd_to_buffer(dmabuf.fd.as_fd())?;
        let modifier = DrmModifier::from(dmabuf.modifier);
        let buffer = ImportedBuffer {
            size: (dmabuf.width, dmabuf.height),
            stride: dmabuf.stride,
            modifier,
            format: self.displays.format,
            gem,
        };
        let flags = if modifier == DrmModifier::Invalid {
            FbCmd2Flags::empty()
        } else {
            FbCmd2Flags::MODIFIERS
        };
        match card.add_planar_framebuffer(&buffer, flags) {
            Ok(framebuffer) => Ok(Scanout {
                framebuffer,
                gem,
                slot: dmabuf.slot,
                acquire_fence: dmabuf.acquire_fence,
            }),
            Err(error) => {
                let _ = card.close_buffer(gem);
                Err(error.into())
            }
        }
    }

    fn commit_scanout(&mut self, mut scanout: Scanout) -> Result<(), DrmError> {
        let mut request = atomic::AtomicModeReq::new();
        let acquire_fd = scanout
            .acquire_fence
            .as_ref()
            .map(AsRawFd::as_raw_fd)
            .unwrap_or(-1);
        for output in &self.displays.outputs {
            let props = output.props;
            let (width, height) = output.mode.size();
            request.add_property(
                output.connector,
                props.connector_crtc_id,
                property::Value::CRTC(Some(output.crtc)),
            );
            request.add_property(output.crtc, props.crtc_mode_id, output.mode_blob);
            request.add_property(
                output.crtc,
                props.crtc_active,
                property::Value::Boolean(true),
            );
            request.add_property(
                output.plane,
                props.plane_fb_id,
                property::Value::Framebuffer(Some(scanout.framebuffer)),
            );
            request.add_property(
                output.plane,
                props.plane_crtc_id,
                property::Value::CRTC(Some(output.crtc)),
            );
            request.add_property(
                output.plane,
                props.plane_src_x,
                property::Value::UnsignedRange((output.x as u64) << 16),
            );
            request.add_property(
                output.plane,
                props.plane_src_y,
                property::Value::UnsignedRange((output.y as u64) << 16),
            );
            request.add_property(
                output.plane,
                props.plane_src_w,
                property::Value::UnsignedRange((width as u64) << 16),
            );
            request.add_property(
                output.plane,
                props.plane_src_h,
                property::Value::UnsignedRange((height as u64) << 16),
            );
            request.add_property(
                output.plane,
                props.plane_crtc_x,
                property::Value::SignedRange(0),
            );
            request.add_property(
                output.plane,
                props.plane_crtc_y,
                property::Value::SignedRange(0),
            );
            request.add_property(
                output.plane,
                props.plane_crtc_w,
                property::Value::UnsignedRange(width as u64),
            );
            request.add_property(
                output.plane,
                props.plane_crtc_h,
                property::Value::UnsignedRange(height as u64),
            );
            if let Some(in_fence) = props.plane_in_fence_fd {
                request.add_property(
                    output.plane,
                    in_fence,
                    property::Value::SignedRange(acquire_fd as i64),
                );
            }
        }
        let mut flags = AtomicCommitFlags::NONBLOCK | AtomicCommitFlags::PAGE_FLIP_EVENT;
        if !self.modeset_done {
            flags |= AtomicCommitFlags::ALLOW_MODESET;
            // The kernel rejects TEST_ONLY | PAGE_FLIP_EVENT with EINVAL, so
            // this preflight cannot be merged with the real commit below.
            if let Err(error) = self.card().atomic_commit(
                AtomicCommitFlags::ALLOW_MODESET | AtomicCommitFlags::TEST_ONLY,
                request.clone(),
            ) {
                self.release_scanout(scanout);
                return Err(commit_error(error));
            }
        }
        if let Err(error) = self.card().atomic_commit(flags, request) {
            self.release_scanout(scanout);
            return Err(commit_error(error));
        }

        // The ioctl imported the sync_file; userspace retains and closes its
        // descriptor immediately after atomic_commit returns.
        scanout.acquire_fence = None;

        debug_assert!(self.retiring.is_none());
        self.retiring = self.current.take();
        self.current = Some(scanout);
        self.pending_flips = self
            .displays
            .outputs
            .iter()
            .map(|output| output.crtc)
            .collect();
        self.modeset_done = true;
        Ok(())
    }

    fn wait_for_flip(&mut self, timeout: Duration) -> Result<(), DrmError> {
        let alive = self.pump(Some(timeout), true);
        if !alive {
            return Err(DrmError::FlipTimeout);
        }
        if !self.pending_flips.is_empty() {
            return Err(DrmError::FlipTimeout);
        }
        Ok(())
    }

    /// Pump backend events until the flip requirement is satisfied, the
    /// `timeout` deadline expires, or the backend fails. The timeout is an
    /// overall deadline rather than a per-poll budget, so a flood of input
    /// events waking poll early cannot starve the flip wait by restarting
    /// the timeout each round. Returns `false` only when the backend is
    /// dead; a flip that outlives the deadline leaves `pending_flips`
    /// non-empty, which `wait_for_flip` reports as [`DrmError::FlipTimeout`].
    fn pump(&mut self, timeout: Option<Duration>, require_flip: bool) -> bool {
        if self.failed {
            return false;
        }
        let deadline = timeout.and_then(|value| std::time::Instant::now().checked_add(value));
        loop {
            if !self.poll_round(poll_ms_remaining(deadline)) {
                return false;
            }
            if !require_flip || self.pending_flips.is_empty() || deadline_passed(deadline) {
                return !self.failed;
            }
        }
    }

    /// One poll + dispatch round over the seat, card, input, and hotplug fds.
    /// Returns `false` when the backend has failed and the loop must exit.
    fn poll_round(&mut self, timeout_ms: i32) -> bool {
        let seat_fd = match self.seat.borrow_mut().get_fd() {
            Ok(fd) => fd.as_raw_fd(),
            Err(error) => {
                log::error!("libseat: cannot get event fd: {error:?}");
                self.failed = true;
                return false;
            }
        };
        let card_fd = self.card().as_fd().as_raw_fd();
        let input_fd = self.input.as_ref().map(AsRawFd::as_raw_fd).unwrap_or(-1);
        let hotplug_fd = self.hotplug.as_ref().map(AsRawFd::as_raw_fd).unwrap_or(-1);
        let mut fds = [
            libc::pollfd {
                fd: seat_fd,
                events: libc::POLLIN,
                revents: 0,
            },
            libc::pollfd {
                fd: card_fd,
                events: if self.active { libc::POLLIN } else { 0 },
                revents: 0,
            },
            libc::pollfd {
                fd: input_fd,
                events: if self.active { libc::POLLIN } else { 0 },
                revents: 0,
            },
            libc::pollfd {
                fd: hotplug_fd,
                events: libc::POLLIN,
                revents: 0,
            },
        ];
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as _, timeout_ms) };
        if ready < 0 {
            let error = std::io::Error::last_os_error();
            if error.kind() == std::io::ErrorKind::Interrupted {
                return true;
            }
            log::error!("backend poll failed: {error}");
            self.failed = true;
            return false;
        }

        if fds[0].revents & (libc::POLLIN | libc::POLLHUP | libc::POLLERR) != 0 {
            if let Err(error) = self.seat.borrow_mut().dispatch(0) {
                log::error!("libseat dispatch failed: {error:?}");
                self.failed = true;
                return false;
            }
            self.apply_seat_event();
        }

        // GPU unplug/revoke: exiting cleanly beats spinning on a dead card fd.
        if fds[1].revents & (libc::POLLHUP | libc::POLLNVAL) != 0 {
            log::error!("drm: card disconnected or revoked; shutting down backend");
            self.failed = true;
            return false;
        }

        if self.active && fds[1].revents & (libc::POLLIN | libc::POLLERR) != 0 {
            match self.card().receive_events() {
                Ok(events) => {
                    for event in events {
                        if let control::Event::PageFlip(event) = event {
                            self.pending_flips.remove(&event.crtc);
                        }
                    }
                    if self.pending_flips.is_empty() {
                        if let Some(retired) = self.retiring.take() {
                            log::trace!("drm: releasing Flux scanout slot {}", retired.slot);
                            self.release_scanout(retired);
                        }
                    }
                }
                Err(error) => {
                    if matches!(error.raw_os_error(), Some(libc::EACCES) | Some(libc::EPERM)) {
                        // The seat Disable event is still in flight; reading
                        // events from a masterless fd is expected, not fatal.
                        log::warn!("drm: event read while masterless (VT switch); ignoring");
                    } else {
                        log::error!("DRM event read failed: {error}");
                        self.failed = true;
                        return false;
                    }
                }
            }
        }

        if self.active && fds[2].revents & (libc::POLLIN | libc::POLLERR) != 0 {
            if let Some(mut input) = self.input.take() {
                if let Err(error) = input.dispatch() {
                    log::error!("libinput dispatch failed: {error}");
                    self.failed = true;
                } else {
                    for event in &mut input {
                        self.push_input_event(event);
                    }
                }
                self.input = Some(input);
            }
        }

        if fds[3].revents & (libc::POLLIN | libc::POLLERR) != 0 {
            if let Some(monitor) = self.hotplug.as_ref() {
                let mut saw_event = false;
                for _event in monitor.iter() {
                    saw_event = true;
                }
                self.hotplug_pending |= saw_event;
            }
        }
        if self.active && self.hotplug_pending && self.pending_flips.is_empty() {
            self.reconfigure_outputs();
        }

        !self.failed
    }

    fn apply_seat_event(&mut self) {
        match self.seat_event.take() {
            Some(PendingSeatEvent::Disable) if self.active => {
                log::info!("libseat: session disabled; suspending input and scanout");
                if let Some(input) = self.input.as_mut() {
                    input.suspend();
                }
                self.active = false;
                if let Err(error) = self.seat.borrow_mut().disable() {
                    log::error!("libseat disable acknowledgement failed: {error:?}");
                    self.failed = true;
                }
            }
            Some(PendingSeatEvent::Enable) if !self.active => {
                log::info!("libseat: session enabled; resuming input and scanout");
                if self
                    .input
                    .as_mut()
                    .is_some_and(|input| input.resume().is_err())
                {
                    log::error!("libinput resume failed");
                    self.failed = true;
                    return;
                }
                self.drain_input_queue();
                // libseat revoked the card fd while another VT owned the
                // seat: every KMS ioctl on it now fails with EACCES, and its
                // GEM handles, framebuffers, and property blobs died with the
                // fd. Close it through the seat and forget the dead records
                // without ioctls (the kernel already freed them), then
                // re-open and re-probe so the next commit runs on a fresh
                // master fd with a TEST_ONLY | ALLOW_MODESET preflight.
                if let Some(card) = self.card.take() {
                    if let Err(error) = self.seat.borrow_mut().close_device(card.device) {
                        log::warn!(
                            "libseat: failed to close revoked card {}: {error:?}",
                            card.path.display()
                        );
                    }
                }
                self.current = None;
                self.retiring = None;
                self.pending_flips.clear();
                self.hotplug_pending = false;
                match open_card_and_outputs(&self.seat, &self.configured_modes) {
                    Ok((card, displays)) => {
                        self.card = Some(card);
                        self.surface_stale |= displays.modifiers != self.surface_modifiers;
                        self.pending_resize = Some(Size {
                            w: displays.size.0 as i32,
                            h: displays.size.1 as i32,
                        });
                        self.displays = displays;
                        self.modeset_done = false;
                        self.render_ready = true;
                        self.active = true;
                    }
                    Err(error) => {
                        log::error!("drm: failed to re-open the GPU after VT resume: {error}");
                        self.failed = true;
                    }
                }
            }
            _ => {}
        }
    }

    fn push_input_event(&mut self, event: Event) {
        match event {
            Event::Device(DeviceEvent::Added(event)) => {
                self.add_input_device(event.device());
            }
            Event::Device(DeviceEvent::Removed(event)) => {
                self.remove_input_device(&event.device());
            }
            Event::Keyboard(KeyboardEvent::Key(event)) => {
                self.input_events.push(InputEvent::Key {
                    code: event.key(),
                    state: match event.key_state() {
                        input::event::keyboard::KeyState::Pressed => ButtonState::Pressed,
                        input::event::keyboard::KeyState::Released => ButtonState::Released,
                    },
                });
            }
            Event::Pointer(event) => self.push_pointer_event(event),
            Event::Touch(event) => self.push_touch_event(event),
            Event::Gesture(event) => self.push_gesture_event(event),
            Event::Tablet(event) => self.push_tablet_event(event),
            _ => {}
        }
    }

    fn drain_input_queue(&mut self) {
        if let Some(mut input) = self.input.take() {
            for event in &mut input {
                self.push_input_event(event);
            }
            self.input = Some(input);
        }
    }

    fn is_touchpad(device: &Device) -> bool {
        if !device.has_capability(DeviceCapability::Pointer) {
            return false;
        }
        let methods = device.config_scroll_methods();
        device.config_tap_finger_count() > 0
            || device.config_dwt_is_available()
            || methods.contains(&ScrollMethod::TwoFinger)
            || methods.contains(&ScrollMethod::Edge)
    }

    fn add_input_device(&mut self, mut device: Device) {
        if !Self::is_touchpad(&device) {
            return;
        }
        let sysname = device.sysname().to_owned();
        let name = device.name().into_owned();
        Self::apply_touchpad_profile(&mut device, self.touchpad_config);
        log::info!("libinput: touchpad added: {name} ({sysname})");
        self.touchpads.insert(sysname, device);
    }

    fn remove_input_device(&mut self, device: &Device) {
        let sysname = device.sysname();
        if self.touchpads.remove(sysname).is_some() {
            log::info!("libinput: touchpad removed: {} ({sysname})", device.name());
        }
    }

    fn apply_touchpad_profile(device: &mut Device, config: TouchpadConfig) {
        let name = device.name().into_owned();
        let apply = |setting: &str, result: DeviceConfigResult| {
            if let Err(error) = result {
                log::warn!("libinput: {name}: could not apply {setting}: {error:?}");
            }
        };

        if device.config_scroll_has_natural_scroll() {
            apply(
                "natural scroll",
                device.config_scroll_set_natural_scroll_enabled(config.natural_scroll),
            );
        }
        if device.config_tap_finger_count() > 0 {
            apply(
                "tap-to-click",
                device.config_tap_set_enabled(config.tap_to_click),
            );
            apply(
                "tap-and-drag",
                device.config_tap_set_drag_enabled(config.tap_and_drag),
            );
            apply(
                "drag lock",
                device.config_tap_set_drag_lock_enabled(if config.drag_lock {
                    DragLockState::EnabledTimeout
                } else {
                    DragLockState::Disabled
                }),
            );
        }
        if device.config_dwt_is_available() {
            apply(
                "disable-while-typing",
                device.config_dwt_set_enabled(config.disable_while_typing),
            );
        }
        if device.config_accel_is_available() {
            apply(
                "pointer speed",
                device.config_accel_set_speed(f64::from(config.pointer_speed.clamp(-1.0, 1.0))),
            );
        }

        let methods = device.config_scroll_methods();
        let requested = match config.scroll_method {
            TouchpadScrollMethod::TwoFinger => ScrollMethod::TwoFinger,
            TouchpadScrollMethod::Edge => ScrollMethod::Edge,
        };
        let selected = methods
            .contains(&requested)
            .then_some(requested)
            .or_else(|| {
                [ScrollMethod::TwoFinger, ScrollMethod::Edge]
                    .into_iter()
                    .find(|method| methods.contains(method))
            });
        if let Some(method) = selected {
            if method != requested {
                log::info!(
                    "libinput: {name}: requested {requested:?} scrolling unsupported; using {method:?}"
                );
            }
            apply("scroll method", device.config_scroll_set_method(method));
        }
    }

    fn current_touchpad_status(&self) -> TouchpadStatus {
        let mut capabilities = TouchpadCapabilities::default();
        let mut device_names = Vec::with_capacity(self.touchpads.len());
        for device in self.touchpads.values() {
            let tap = device.config_tap_finger_count() > 0;
            let methods = device.config_scroll_methods();
            capabilities.natural_scroll |= device.config_scroll_has_natural_scroll();
            capabilities.tap_to_click |= tap;
            capabilities.tap_and_drag |= tap;
            capabilities.drag_lock |= tap;
            capabilities.disable_while_typing |= device.config_dwt_is_available();
            capabilities.pointer_speed |= device.config_accel_is_available();
            capabilities.two_finger_scroll |= methods.contains(&ScrollMethod::TwoFinger);
            capabilities.edge_scroll |= methods.contains(&ScrollMethod::Edge);
            device_names.push(device.name().into_owned());
        }
        device_names.sort();
        device_names.dedup();
        TouchpadStatus {
            configurable: true,
            device_names,
            capabilities,
            config: self.touchpad_config,
        }
    }

    fn push_tablet_event(&mut self, event: TabletToolEvent) {
        let (width, height) = self.physical_size();
        let tool = event.tool();
        let kind = match tool.tool_type() {
            Some(TabletToolType::Pen) => 0x140,
            Some(TabletToolType::Eraser) => 0x141,
            Some(TabletToolType::Brush) => 0x142,
            Some(TabletToolType::Pencil) => 0x143,
            Some(TabletToolType::Airbrush) => 0x144,
            Some(TabletToolType::Mouse) => 0x146,
            Some(TabletToolType::Lens) => 0x147,
            _ => 0x140,
        };
        let serial = tool.serial();
        let hardware_id = tool.tool_id();
        let tool_key = if serial != 0 {
            serial
        } else {
            (hardware_id << 16) ^ u64::from(kind)
        };
        match event {
            TabletToolEvent::Proximity(event) => {
                let mut capabilities = 0u32;
                for (present, capability) in [
                    (tool.has_tilt(), 1),
                    (tool.has_pressure(), 2),
                    (tool.has_distance(), 3),
                    (tool.has_rotation(), 4),
                    (tool.has_slider(), 5),
                    (tool.has_wheel(), 6),
                ] {
                    if present {
                        capabilities |= 1 << capability;
                    }
                }
                let x = event.x_transformed(width) as f32;
                let y = event.y_transformed(height) as f32;
                self.input_events.push(InputEvent::Tablet {
                    event: TabletEvent::Proximity {
                        tool: tool_key,
                        info: TabletToolInfo {
                            serial,
                            hardware_id,
                            kind,
                            capabilities,
                        },
                        in_proximity: event.proximity_state() == ProximityState::In,
                        x,
                        y,
                        time: event.time(),
                    },
                });
                if event.proximity_state() == ProximityState::In {
                    self.push_tablet_axes(tool_key, &event, width, height);
                }
            }
            TabletToolEvent::Axis(event) => {
                self.push_tablet_axes(tool_key, &event, width, height);
            }
            TabletToolEvent::Tip(event) => {
                self.push_tablet_axes(tool_key, &event, width, height);
                self.input_events.push(InputEvent::Tablet {
                    event: TabletEvent::Tip {
                        tool: tool_key,
                        state: if event.tip_state() == TipState::Down {
                            ButtonState::Pressed
                        } else {
                            ButtonState::Released
                        },
                        time: event.time(),
                    },
                });
            }
            TabletToolEvent::Button(event) => {
                self.push_tablet_button(tool_key, &event);
            }
            // `TabletToolEvent` is non-exhaustive; future axes we do not map
            // yet are ignored rather than breaking the build.
            _ => {}
        }
    }

    fn push_tablet_axes<E: TabletToolEventTrait>(
        &mut self,
        tool: u64,
        event: &E,
        width: u32,
        height: u32,
    ) {
        self.input_events.push(InputEvent::Tablet {
            event: TabletEvent::Axes {
                tool,
                x: event.x_transformed(width) as f32,
                y: event.y_transformed(height) as f32,
                pressure: event
                    .pressure_has_changed()
                    .then(|| event.pressure() as f32),
                distance: event
                    .distance_has_changed()
                    .then(|| event.distance() as f32),
                tilt: (event.tilt_x_has_changed() || event.tilt_y_has_changed())
                    .then(|| (event.tilt_x() as f32, event.tilt_y() as f32)),
                rotation: event
                    .rotation_has_changed()
                    .then(|| event.rotation() as f32),
                slider: event
                    .slider_has_changed()
                    .then(|| event.slider_position() as f32),
                wheel: event.wheel_has_changed().then(|| {
                    (
                        event.wheel_delta() as f32,
                        event.wheel_delta_discrete() as i32,
                    )
                }),
                time: event.time(),
            },
        });
    }

    fn push_tablet_button(&mut self, tool: u64, event: &TabletToolButtonEvent) {
        self.input_events.push(InputEvent::Tablet {
            event: TabletEvent::Button {
                tool,
                button: event.button(),
                state: match event.button_state() {
                    input::event::pointer::ButtonState::Pressed => ButtonState::Pressed,
                    input::event::pointer::ButtonState::Released => ButtonState::Released,
                },
                time: event.time(),
            },
        });
    }

    fn push_pointer_event(&mut self, event: PointerEvent) {
        let (width, height) = self.physical_size();
        match event {
            PointerEvent::Motion(event) => {
                self.pointer.0 =
                    (self.pointer.0 + event.dx() as f32).clamp(0.0, width.saturating_sub(1) as f32);
                self.pointer.1 = (self.pointer.1 + event.dy() as f32)
                    .clamp(0.0, height.saturating_sub(1) as f32);
                self.input_events.push(InputEvent::PointerMotion {
                    x: self.pointer.0,
                    y: self.pointer.1,
                });
            }
            PointerEvent::MotionAbsolute(event) => {
                self.pointer = (
                    event.absolute_x_transformed(width) as f32,
                    event.absolute_y_transformed(height) as f32,
                );
                self.input_events.push(InputEvent::PointerMotion {
                    x: self.pointer.0,
                    y: self.pointer.1,
                });
            }
            PointerEvent::Button(event) => {
                self.input_events.push(InputEvent::PointerButton {
                    button: event.button(),
                    state: match event.button_state() {
                        input::event::pointer::ButtonState::Pressed => ButtonState::Pressed,
                        input::event::pointer::ButtonState::Released => ButtonState::Released,
                    },
                });
            }
            PointerEvent::ScrollWheel(event) => {
                self.push_scroll_wheel(&event);
            }
            PointerEvent::ScrollFinger(event) => {
                self.push_scroll_sequence(&event, PointerAxisSource::Finger);
            }
            PointerEvent::ScrollContinuous(event) => {
                self.push_scroll_sequence(&event, PointerAxisSource::Continuous);
            }
            #[allow(deprecated)]
            PointerEvent::Axis(event) => {
                self.push_legacy_scroll(&event);
            }
            _ => {}
        }
    }

    fn push_scroll_wheel(&mut self, event: &PointerScrollWheelEvent) {
        // wl_pointer's conventional wheel step is 10 surface units. v120
        // preserves high-resolution wheel fractions without device-angle bias.
        let mut frame = PointerAxisFrame {
            time: event.time(),
            source: Some(PointerAxisSource::Wheel),
            ..PointerAxisFrame::default()
        };
        frame.horizontal = wheel_axis(event, Axis::Horizontal);
        frame.vertical = wheel_axis(event, Axis::Vertical);
        if frame.has_data() {
            self.input_events.push(InputEvent::PointerAxis(frame));
        }
    }

    fn push_scroll_sequence<T>(&mut self, event: &T, source: PointerAxisSource)
    where
        T: PointerScrollEvent + PointerEventTrait + EventTrait,
    {
        let mut frame = PointerAxisFrame {
            time: event.time(),
            source: Some(source),
            ..PointerAxisFrame::default()
        };
        let inverted = event.device().config_scroll_natural_scroll_enabled();
        frame.horizontal = sequence_axis(event, Axis::Horizontal, inverted);
        frame.vertical = sequence_axis(event, Axis::Vertical, inverted);
        if frame.has_data() {
            self.input_events.push(InputEvent::PointerAxis(frame));
        }
    }

    #[allow(deprecated)]
    fn push_legacy_scroll(&mut self, event: &PointerAxisEvent) {
        let source = match event.axis_source() {
            AxisSource::Wheel => PointerAxisSource::Wheel,
            AxisSource::Finger => PointerAxisSource::Finger,
            AxisSource::Continuous => PointerAxisSource::Continuous,
            AxisSource::WheelTilt => PointerAxisSource::WheelTilt,
        };
        let mut frame = PointerAxisFrame {
            time: event.time(),
            source: Some(source),
            ..PointerAxisFrame::default()
        };
        let inverted = event.device().config_scroll_natural_scroll_enabled();
        frame.horizontal = legacy_axis(event, Axis::Horizontal, source, inverted);
        frame.vertical = legacy_axis(event, Axis::Vertical, source, inverted);
        if frame.has_data() {
            self.input_events.push(InputEvent::PointerAxis(frame));
        }
    }

    fn push_touch_event(&mut self, event: TouchEvent) {
        let (width, height) = self.physical_size();
        match event {
            TouchEvent::Down(event) => self.input_events.push(InputEvent::TouchDown {
                id: event.seat_slot() as i32,
                x: event.x_transformed(width) as f32,
                y: event.y_transformed(height) as f32,
            }),
            TouchEvent::Motion(event) => self.input_events.push(InputEvent::TouchMotion {
                id: event.seat_slot() as i32,
                x: event.x_transformed(width) as f32,
                y: event.y_transformed(height) as f32,
            }),
            TouchEvent::Up(event) => self.input_events.push(InputEvent::TouchUp {
                id: event.seat_slot() as i32,
            }),
            TouchEvent::Frame(_) => self.input_events.push(InputEvent::TouchFrame),
            TouchEvent::Cancel(_) => self.input_events.push(InputEvent::TouchCancel),
            _ => {}
        }
    }

    fn push_gesture_event(&mut self, event: GestureEvent) {
        let mapped = match event {
            GestureEvent::Swipe(GestureSwipeEvent::Begin(event)) => {
                Some(PointerGestureEvent::SwipeBegin {
                    time: event.time(),
                    fingers: event.finger_count().max(0) as u32,
                })
            }
            GestureEvent::Swipe(GestureSwipeEvent::Update(event)) => {
                Some(PointerGestureEvent::SwipeUpdate {
                    time: event.time(),
                    dx: event.dx() as f32,
                    dy: event.dy() as f32,
                })
            }
            GestureEvent::Swipe(GestureSwipeEvent::End(event)) => {
                Some(PointerGestureEvent::SwipeEnd {
                    time: event.time(),
                    cancelled: event.cancelled(),
                })
            }
            GestureEvent::Pinch(GesturePinchEvent::Begin(event)) => {
                Some(PointerGestureEvent::PinchBegin {
                    time: event.time(),
                    fingers: event.finger_count().max(0) as u32,
                })
            }
            GestureEvent::Pinch(GesturePinchEvent::Update(event)) => {
                Some(PointerGestureEvent::PinchUpdate {
                    time: event.time(),
                    dx: event.dx() as f32,
                    dy: event.dy() as f32,
                    scale: event.scale() as f32,
                    rotation: event.angle_delta() as f32,
                })
            }
            GestureEvent::Pinch(GesturePinchEvent::End(event)) => {
                Some(PointerGestureEvent::PinchEnd {
                    time: event.time(),
                    cancelled: event.cancelled(),
                })
            }
            GestureEvent::Hold(GestureHoldEvent::Begin(event)) => {
                Some(PointerGestureEvent::HoldBegin {
                    time: event.time(),
                    fingers: event.finger_count().max(0) as u32,
                })
            }
            GestureEvent::Hold(GestureHoldEvent::End(event)) => {
                Some(PointerGestureEvent::HoldEnd {
                    time: event.time(),
                    cancelled: event.cancelled(),
                })
            }
            _ => None,
        };
        if let Some(event) = mapped {
            self.gesture_events.push(event);
        }
    }

    fn reconfigure_outputs(&mut self) {
        self.hotplug_pending = false;
        let selected = match select_outputs(self.card(), &self.configured_modes) {
            Ok(displays) => displays,
            Err(DrmError::NoConnector) => {
                log::info!("drm: all outputs disconnected; suspending rendering");
                if self.modeset_done {
                    let _ = self.disable_outputs();
                }
                self.modeset_done = false;
                self.pending_flips.clear();
                if let Some(scanout) = self.retiring.take() {
                    self.release_scanout(scanout);
                }
                if let Some(scanout) = self.current.take() {
                    self.release_scanout(scanout);
                }
                self.render_ready = false;
                return;
            }
            Err(error) => {
                log::warn!("drm: hotplug reprobe failed; keeping current layout: {error}");
                return;
            }
        };

        if display_signature(&selected) == display_signature(&self.displays) {
            // Probe created fresh mode blobs. The existing display set remains
            // authoritative, so release only the redundant probe resources.
            for output in selected.outputs {
                let _ = self.card().destroy_property_blob(output.mode_blob_id);
            }
            self.render_ready = true;
            return;
        }

        if self.modeset_done {
            if let Err(error) = self.disable_outputs() {
                log::warn!("drm: failed to disable old hotplug layout: {error}");
            }
        }
        self.modeset_done = false;
        self.pending_flips.clear();
        if let Some(scanout) = self.retiring.take() {
            self.release_scanout(scanout);
        }
        if let Some(scanout) = self.current.take() {
            self.release_scanout(scanout);
        }

        let old = std::mem::replace(&mut self.displays, selected);
        for output in old.outputs {
            let _ = self.card().destroy_property_blob(output.mode_blob_id);
        }
        if self.displays.modifiers != self.surface_modifiers {
            // The live Flux surface was created with the old intersection and
            // resize cannot retcon its modifier; the main loop must recreate
            // it (see Backend::surface_needs_recreate).
            log::info!(
                "drm: modifier intersection changed; presentation surface must be recreated"
            );
            self.surface_stale = true;
        }
        let (width, height) = self.displays.size;
        self.pointer.0 = self.pointer.0.clamp(0.0, width.saturating_sub(1) as f32);
        self.pointer.1 = self.pointer.1.clamp(0.0, height.saturating_sub(1) as f32);
        self.explicit_sync = self.sync_capable
            && self
                .displays
                .outputs
                .iter()
                .all(|output| output.props.plane_in_fence_fd.is_some());
        self.pending_resize = Some(Size {
            w: width as i32,
            h: height as i32,
        });
        self.render_ready = true;
        log::info!(
            "drm: hotplug layout now has {} output(s), desktop {}x{}",
            self.displays.outputs.len(),
            width,
            height
        );
    }

    fn release_scanout(&self, scanout: Scanout) {
        let card = self.card();
        if let Err(error) = card.destroy_framebuffer(scanout.framebuffer) {
            log::warn!("DRM: failed to destroy framebuffer: {error}");
        }
        if let Err(error) = card.close_buffer(scanout.gem) {
            log::warn!("DRM: failed to close imported GEM handle: {error}");
        }
    }

    fn disable_outputs(&self) -> Result<(), DrmError> {
        let mut request = atomic::AtomicModeReq::new();
        for output in &self.displays.outputs {
            let props = output.props;
            request.add_property(
                output.plane,
                props.plane_fb_id,
                property::Value::Framebuffer(None),
            );
            request.add_property(
                output.plane,
                props.plane_crtc_id,
                property::Value::CRTC(None),
            );
            request.add_property(
                output.connector,
                props.connector_crtc_id,
                property::Value::CRTC(None),
            );
            request.add_property(
                output.crtc,
                props.crtc_active,
                property::Value::Boolean(false),
            );
        }
        self.card()
            .atomic_commit(AtomicCommitFlags::ALLOW_MODESET, request)?;
        Ok(())
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

    fn output_infos(&self) -> Vec<ass_core::output::OutputInfo> {
        self.displays
            .outputs
            .iter()
            .map(|output| {
                let (width, height) = output.mode.size();
                ass_core::output::OutputInfo {
                    connector: output.name.clone(),
                    geometry: ass_core::output::OutputGeometry {
                        mode: ass_core::output::OutputMode {
                            width: width as i32,
                            height: height as i32,
                            refresh_mhz: output.mode.vrefresh().saturating_mul(1_000),
                        },
                        scale: ass_core::output::Scale::IDENTITY,
                        transform: ass_core::Transform::Normal,
                        logical_origin: ass_core::Point {
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
        // A live re-modeset is intentionally not attempted: the new map takes
        // effect on the next output (re)selection (hotplug or session
        // resume), whose select_outputs consults it. Note the deferral for
        // outputs whose configured mode differs from the live one, so a
        // config reload is never silently half-applied (ADR-0026's apply
        // contract).
        for output in &self.displays.outputs {
            let Some(spec) = modes.get(&output.name) else {
                continue;
            };
            let (width, height) = output.mode.size();
            let current = OutputMode {
                width: width as i32,
                height: height as i32,
                refresh_mhz: output.mode.vrefresh().saturating_mul(1_000),
            };
            if !spec.matches(&current) {
                log::info!(
                    "drm: {}: configured mode {spec:?} differs from the live mode; \
                     it applies on the next hotplug or restart",
                    output.name
                );
            }
        }
        self.configured_modes = modes;
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
        if self.active && !self.pending_flips.is_empty() {
            if let Err(error) = self.wait_for_flip(Duration::from_secs(1)) {
                log::warn!("drm: page flip did not settle during shutdown: {error}");
            }
        }
        if self.active && self.modeset_done {
            if let Err(error) = self.disable_outputs() {
                log::warn!("drm: failed to disable output during shutdown: {error}");
            }
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

fn candidate_cards() -> Vec<PathBuf> {
    if let Some(path) = std::env::var_os("ASS_DRM_DEVICE") {
        return vec![PathBuf::from(path)];
    }
    (0..16)
        .map(|index| PathBuf::from(format!("/dev/dri/card{index}")))
        .filter(|path| path.exists())
        .collect()
}

/// Milliseconds until `deadline`, shaped as a `poll(2)` timeout: `None`
/// blocks indefinitely and an already-passed deadline polls without blocking.
fn poll_ms_remaining(deadline: Option<std::time::Instant>) -> i32 {
    match deadline {
        None => -1,
        Some(deadline) => deadline
            .saturating_duration_since(std::time::Instant::now())
            .as_millis()
            .min(i32::MAX as u128) as i32,
    }
}

/// Whether an optional pump deadline has been reached. `None` never expires.
fn deadline_passed(deadline: Option<std::time::Instant>) -> bool {
    deadline.is_some_and(|deadline| std::time::Instant::now() >= deadline)
}

fn open_card_and_outputs(
    seat: &Rc<RefCell<libseat::Seat>>,
    configured_modes: &HashMap<String, ModeSpec>,
) -> Result<(Card, DisplaySet), DrmError> {
    let candidates = candidate_cards();
    let tried = candidates
        .iter()
        .map(|path| path.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    for path in candidates {
        match seat.borrow_mut().open_device(&path) {
            Ok(device) => {
                let card = Card { device, path };
                let result = card
                    .set_client_capability(drm::ClientCapability::UniversalPlanes, true)
                    .and_then(|()| card.set_client_capability(drm::ClientCapability::Atomic, true))
                    .map_err(DrmError::from)
                    .and_then(|()| select_outputs(&card, configured_modes));
                match result {
                    Ok(output) => return Ok((card, output)),
                    Err(error) => {
                        log::warn!(
                            "drm: skipping unusable card {}: {error}",
                            card.path.display()
                        );
                        if let Err(close_error) = seat.borrow_mut().close_device(card.device) {
                            log::warn!(
                                "libseat: failed to close skipped card {}: {close_error:?}",
                                card.path.display()
                            );
                        }
                    }
                }
            }
            Err(error) => log::warn!("libseat: cannot open {}: {error:?}", path.display()),
        }
    }
    Err(DrmError::NoCard(if tried.is_empty() {
        "/dev/dri/card[0-15] (none exist)".to_owned()
    } else {
        tried
    }))
}

#[derive(Debug, Clone)]
struct OutputCandidate {
    connector: connector::Handle,
    name: String,
    mode: Mode,
    choices: Vec<OutputChoice>,
    available_modes: Vec<OutputMode>,
}

#[derive(Debug, Clone)]
struct OutputChoice {
    crtc: crtc::Handle,
    plane: plane::Handle,
    modifiers: Vec<u64>,
}

fn select_outputs(
    card: &Card,
    configured_modes: &HashMap<String, ModeSpec>,
) -> Result<DisplaySet, DrmError> {
    let resources = card.resource_handles()?;
    let mut connectors = resources
        .connectors()
        .iter()
        .filter_map(|handle| card.get_connector(*handle, true).ok())
        .filter(|info| info.state() == connector::State::Connected && !info.modes().is_empty())
        .collect::<Vec<_>>();
    connectors.sort_by_key(|info| (info.interface() as u32, info.interface_id()));
    if connectors.is_empty() {
        return Err(DrmError::NoConnector);
    }

    let planes = card
        .plane_handles()?
        .into_iter()
        .filter_map(|handle| card.get_plane(handle).ok().map(|info| (handle, info)))
        .filter(|(handle, _)| plane_type(card, *handle) == Some(control::PlaneType::Primary))
        .collect::<Vec<_>>();

    let mut assignment = None;
    for format in [DrmFourcc::Xrgb8888, DrmFourcc::Argb8888] {
        let mut candidates = Vec::with_capacity(connectors.len());
        for connector in &connectors {
            let name = connector.to_string();
            // (width, height, refresh_mhz, preferred) in connector order, so
            // an index returned by pick_mode addresses connector.modes()
            // directly.
            let tuples = connector
                .modes()
                .iter()
                .map(|mode| {
                    let (width, height) = mode.size();
                    (
                        width as i32,
                        height as i32,
                        mode.vrefresh().saturating_mul(1_000),
                        mode.mode_type().contains(ModeTypeFlags::PREFERRED),
                    )
                })
                .collect::<Vec<_>>();
            let spec = configured_modes.get(&name);
            let picked = match pick_mode(&tuples, spec) {
                Some(index) => index,
                None => {
                    // Only reachable with a spec that matched nothing.
                    if let Some(spec) = spec {
                        log::warn!(
                            "drm: {name}: configured mode {spec:?} matches no advertised mode; using the preferred mode"
                        );
                    }
                    pick_mode(&tuples, None).unwrap_or(0)
                }
            };
            let mode = connector.modes()[picked];
            let mut crtcs = Vec::new();
            if let Some(current) = connector
                .current_encoder()
                .and_then(|encoder| card.get_encoder(encoder).ok())
                .and_then(|encoder| encoder.crtc())
            {
                crtcs.push(current);
            }
            for encoder in connector.encoders() {
                if let Ok(encoder) = card.get_encoder(*encoder) {
                    for crtc in resources.filter_crtcs(encoder.possible_crtcs()) {
                        if !crtcs.contains(&crtc) {
                            crtcs.push(crtc);
                        }
                    }
                }
            }

            let mut choices = Vec::new();
            for crtc in crtcs {
                for (plane, info) in &planes {
                    if !resources
                        .filter_crtcs(info.possible_crtcs())
                        .contains(&crtc)
                        || !info.formats().contains(&(format as u32))
                    {
                        continue;
                    }
                    let modifiers = plane_modifiers(card, *plane, format)?;
                    if !modifiers.is_empty() {
                        choices.push(OutputChoice {
                            crtc,
                            plane: *plane,
                            modifiers,
                        });
                    }
                }
            }
            candidates.push(OutputCandidate {
                connector: connector.handle(),
                name,
                mode,
                choices,
                available_modes: advertised_modes(connector),
            });
        }
        if candidates
            .iter()
            .any(|candidate| candidate.choices.is_empty())
        {
            continue;
        }
        if let Some((choices, modifiers)) = assign_outputs(&candidates) {
            assignment = Some((format, candidates, choices, modifiers));
            break;
        }
    }

    let Some((format, candidates, choices, modifiers)) = assignment else {
        return Err(DrmError::NoPlane);
    };
    let mut desktop_width = 0_u32;
    let mut desktop_height = 0_u32;
    for candidate in &candidates {
        let size = candidate.mode.size();
        desktop_width = desktop_width
            .checked_add(size.0 as u32)
            .ok_or(DrmError::DesktopTooLarge(u32::MAX, desktop_height))?;
        desktop_height = desktop_height.max(size.1 as u32);
    }
    if !resources.supported_fb_width().contains(&desktop_width)
        || !resources.supported_fb_height().contains(&desktop_height)
    {
        return Err(DrmError::DesktopTooLarge(desktop_width, desktop_height));
    }

    let mut outputs: Vec<Output> = Vec::with_capacity(candidates.len());
    let mut x = 0_u32;
    for (candidate, choice) in candidates.into_iter().zip(choices) {
        let result = build_output(card, candidate, choice, x);
        let output = match result {
            Ok(output) => output,
            Err(error) => {
                for output in &outputs {
                    let _ = card.destroy_property_blob(output.mode_blob_id);
                }
                return Err(error);
            }
        };
        let size = output.mode.size();
        x += size.0 as u32;
        outputs.push(output);
    }
    Ok(DisplaySet {
        outputs,
        size: (desktop_width, desktop_height),
        format,
        modifiers,
    })
}

fn assign_outputs(candidates: &[OutputCandidate]) -> Option<(Vec<OutputChoice>, Vec<u64>)> {
    fn recurse(
        candidates: &[OutputCandidate],
        index: usize,
        used_crtcs: &mut HashSet<crtc::Handle>,
        used_planes: &mut HashSet<plane::Handle>,
        selected: &mut Vec<OutputChoice>,
        shared: Option<Vec<u64>>,
    ) -> Option<(Vec<OutputChoice>, Vec<u64>)> {
        if index == candidates.len() {
            return Some((selected.clone(), shared.unwrap_or_default()));
        }
        for choice in &candidates[index].choices {
            if used_crtcs.contains(&choice.crtc) || used_planes.contains(&choice.plane) {
                continue;
            }
            let next_shared = match &shared {
                Some(current) => current
                    .iter()
                    .copied()
                    .filter(|modifier| choice.modifiers.contains(modifier))
                    .collect::<Vec<_>>(),
                None => choice.modifiers.clone(),
            };
            if next_shared.is_empty() {
                continue;
            }
            used_crtcs.insert(choice.crtc);
            used_planes.insert(choice.plane);
            selected.push(choice.clone());
            if let Some(result) = recurse(
                candidates,
                index + 1,
                used_crtcs,
                used_planes,
                selected,
                Some(next_shared),
            ) {
                return Some(result);
            }
            selected.pop();
            used_crtcs.remove(&choice.crtc);
            used_planes.remove(&choice.plane);
        }
        None
    }

    recurse(
        candidates,
        0,
        &mut HashSet::new(),
        &mut HashSet::new(),
        &mut Vec::new(),
        None,
    )
}

fn build_output(
    card: &Card,
    candidate: OutputCandidate,
    choice: OutputChoice,
    x: u32,
) -> Result<Output, DrmError> {
    let connector_props = property_map(card, candidate.connector)?;
    let crtc_props = property_map(card, choice.crtc)?;
    let plane_props = property_map(card, choice.plane)?;
    let props = AtomicProperties {
        connector_crtc_id: required_prop(&connector_props, "CRTC_ID")?,
        crtc_mode_id: required_prop(&crtc_props, "MODE_ID")?,
        crtc_active: required_prop(&crtc_props, "ACTIVE")?,
        plane_fb_id: required_prop(&plane_props, "FB_ID")?,
        plane_crtc_id: required_prop(&plane_props, "CRTC_ID")?,
        plane_src_x: required_prop(&plane_props, "SRC_X")?,
        plane_src_y: required_prop(&plane_props, "SRC_Y")?,
        plane_src_w: required_prop(&plane_props, "SRC_W")?,
        plane_src_h: required_prop(&plane_props, "SRC_H")?,
        plane_crtc_x: required_prop(&plane_props, "CRTC_X")?,
        plane_crtc_y: required_prop(&plane_props, "CRTC_Y")?,
        plane_crtc_w: required_prop(&plane_props, "CRTC_W")?,
        plane_crtc_h: required_prop(&plane_props, "CRTC_H")?,
        plane_in_fence_fd: optional_prop(&plane_props, "IN_FENCE_FD"),
    };
    let mode_blob = card.create_property_blob(&candidate.mode)?;
    let property::Value::Blob(mode_blob_id) = mode_blob else {
        unreachable!("create_property_blob always returns Blob")
    };
    Ok(Output {
        connector: candidate.connector,
        name: candidate.name,
        crtc: choice.crtc,
        plane: choice.plane,
        mode: candidate.mode,
        mode_blob,
        mode_blob_id,
        x,
        y: 0,
        props,
        available_modes: candidate.available_modes,
    })
}

/// The connector's advertised modes, deduplicated by (width, height,
/// refresh) and sorted by pixel count then refresh rate, highest first — the
/// order `ass-ctl outputs` presents them in.
fn advertised_modes(info: &connector::Info) -> Vec<OutputMode> {
    let mut modes: Vec<OutputMode> = info
        .modes()
        .iter()
        .map(|mode| {
            let (width, height) = mode.size();
            OutputMode {
                width: width as i32,
                height: height as i32,
                refresh_mhz: mode.vrefresh().saturating_mul(1_000),
            }
        })
        .collect();
    modes.sort_by(|a, b| {
        (i64::from(b.width) * i64::from(b.height), b.refresh_mhz)
            .cmp(&(i64::from(a.width) * i64::from(a.height), a.refresh_mhz))
    });
    modes.dedup();
    modes
}

/// Choose a mode index out of `modes` — `(width, height, refresh_mhz,
/// preferred)` tuples in connector order — honoring an optional configured
/// spec (ADR-0028). Without a spec the DRM PREFERRED mode wins, falling back
/// to the first advertised mode. With a spec, matches require exact
/// width/height (plus a whole-Hz refresh match when the spec names one);
/// among matches the PREFERRED flag wins, then the highest refresh, then the
/// lowest index. `None` means nothing matched (or `modes` is empty); the
/// caller falls back to the no-spec rule.
fn pick_mode(modes: &[(i32, i32, u32, bool)], spec: Option<&ModeSpec>) -> Option<usize> {
    match spec {
        None => modes
            .iter()
            .position(|&(.., preferred)| preferred)
            .or((!modes.is_empty()).then_some(0)),
        Some(spec) => modes
            .iter()
            .enumerate()
            .filter(|(_, &(width, height, refresh_mhz, _))| {
                spec.matches(&OutputMode {
                    width,
                    height,
                    refresh_mhz,
                })
            })
            .max_by_key(|&(index, &(.., refresh_mhz, preferred))| {
                (preferred, refresh_mhz, std::cmp::Reverse(index))
            })
            .map(|(index, _)| index),
    }
}

fn display_signature(displays: &DisplaySet) -> DisplaySignature {
    (
        displays.format,
        displays.modifiers.clone(),
        displays
            .outputs
            .iter()
            .map(|output| {
                let (width, height) = output.mode.size();
                (
                    output.name.clone(),
                    width as u32,
                    height as u32,
                    output.mode.vrefresh(),
                    output.x,
                    output.y,
                )
            })
            .collect(),
    )
}

fn property_map<H: ResourceHandle>(
    card: &Card,
    handle: H,
) -> Result<HashMap<String, property::Info>, DrmError> {
    Ok(card.get_properties(handle)?.as_hashmap(card)?)
}

fn required_prop(
    props: &HashMap<String, property::Info>,
    name: &'static str,
) -> Result<property::Handle, DrmError> {
    props
        .get(name)
        .map(property::Info::handle)
        .ok_or(DrmError::MissingProperty(name))
}

fn optional_prop(props: &HashMap<String, property::Info>, name: &str) -> Option<property::Handle> {
    props.get(name).map(property::Info::handle)
}

fn plane_type(card: &Card, handle: plane::Handle) -> Option<control::PlaneType> {
    let properties = card.get_properties(handle).ok()?;
    for (&id, &value) in properties.iter() {
        let info = card.get_property(id).ok()?;
        if info.name() == c"type" {
            return match value as u32 {
                value if value == control::PlaneType::Primary as u32 => {
                    Some(control::PlaneType::Primary)
                }
                value if value == control::PlaneType::Cursor as u32 => {
                    Some(control::PlaneType::Cursor)
                }
                value if value == control::PlaneType::Overlay as u32 => {
                    Some(control::PlaneType::Overlay)
                }
                _ => None,
            };
        }
    }
    None
}

/// Return modifiers accepted by `plane` for `format`. Drivers predating the
/// IN_FORMATS property expose only the legacy implicit-layout contract, whose
/// portable dma-buf representation is linear.
fn plane_modifiers(
    card: &Card,
    plane: plane::Handle,
    format: DrmFourcc,
) -> Result<Vec<u64>, DrmError> {
    let properties = card.get_properties(plane)?;
    for (&id, &value) in properties.iter() {
        let info = card.get_property(id)?;
        if info.name().to_bytes() == b"IN_FORMATS" {
            if value == 0 {
                return Ok(vec![u64::from(DrmModifier::Linear)]);
            }
            let blob = card.get_property_blob(value)?;
            return parse_format_modifiers(&blob, format as u32);
        }
    }
    Ok(vec![u64::from(DrmModifier::Linear)])
}

/// Parse Linux's `drm_format_modifier_blob` without casting untrusted kernel
/// offsets to native structs. All bounds and arithmetic are checked first.
fn parse_format_modifiers(blob: &[u8], format: u32) -> Result<Vec<u64>, DrmError> {
    const HEADER: usize = 24;
    const MODIFIER_RECORD: usize = 24;
    if blob.len() < HEADER {
        return Err(DrmError::MalformedFormats("short header"));
    }
    let read_u32 = |offset: usize| -> Result<u32, DrmError> {
        let bytes = blob
            .get(offset..offset + 4)
            .ok_or(DrmError::MalformedFormats("u32 outside blob"))?;
        Ok(u32::from_ne_bytes(bytes.try_into().unwrap()))
    };
    let read_u64 = |offset: usize| -> Result<u64, DrmError> {
        let bytes = blob
            .get(offset..offset + 8)
            .ok_or(DrmError::MalformedFormats("u64 outside blob"))?;
        Ok(u64::from_ne_bytes(bytes.try_into().unwrap()))
    };

    let count_formats = read_u32(8)? as usize;
    let formats_offset = read_u32(12)? as usize;
    let count_modifiers = read_u32(16)? as usize;
    let modifiers_offset = read_u32(20)? as usize;
    let formats_bytes = count_formats
        .checked_mul(4)
        .and_then(|size| formats_offset.checked_add(size))
        .ok_or(DrmError::MalformedFormats("format array overflow"))?;
    let modifiers_bytes = count_modifiers
        .checked_mul(MODIFIER_RECORD)
        .and_then(|size| modifiers_offset.checked_add(size))
        .ok_or(DrmError::MalformedFormats("modifier array overflow"))?;
    if formats_offset < HEADER || formats_bytes > blob.len() {
        return Err(DrmError::MalformedFormats("format array outside blob"));
    }
    if modifiers_offset < HEADER || modifiers_bytes > blob.len() {
        return Err(DrmError::MalformedFormats("modifier array outside blob"));
    }

    let Some(format_index) =
        (0..count_formats).find(|index| read_u32(formats_offset + index * 4).ok() == Some(format))
    else {
        return Ok(Vec::new());
    };
    let mut modifiers = Vec::new();
    for index in 0..count_modifiers {
        let base = modifiers_offset + index * MODIFIER_RECORD;
        let formats = read_u64(base)?;
        let offset = read_u32(base + 8)? as usize;
        if format_index >= offset
            && format_index - offset < 64
            && formats & (1_u64 << (format_index - offset)) != 0
        {
            let modifier = read_u64(base + 16)?;
            if modifier != u64::from(DrmModifier::Invalid) && !modifiers.contains(&modifier) {
                modifiers.push(modifier);
            }
        }
    }
    // Prefer linear when both sides support it. Flux applies the same policy,
    // but ordering here also makes logs/tests deterministic.
    modifiers.sort_by_key(|modifier| (*modifier != u64::from(DrmModifier::Linear), *modifier));
    Ok(modifiers)
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let previous = std::env::var_os("ASS_DRM_DEVICE");
        std::env::set_var("ASS_DRM_DEVICE", "/dev/dri/card-test");
        assert_eq!(candidate_cards(), vec![PathBuf::from("/dev/dri/card-test")]);
        if let Some(previous) = previous {
            std::env::set_var("ASS_DRM_DEVICE", previous);
        } else {
            std::env::remove_var("ASS_DRM_DEVICE");
        }
    }

    #[test]
    fn imported_buffer_exposes_single_xrgb_plane() {
        let buffer = ImportedBuffer {
            size: (1920, 1080),
            stride: 7680,
            modifier: DrmModifier::Linear,
            format: DrmFourcc::Xrgb8888,
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
        assert!(parse_format_modifiers(&blob, DrmFourcc::Argb8888 as u32)
            .unwrap()
            .is_empty());
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
    fn pick_mode_without_spec_prefers_flagged_then_first() {
        let modes = [
            (1920, 1080, 60_000, false),
            (2560, 1440, 60_000, true),
            (1920, 1080, 144_000, false),
        ];
        assert_eq!(pick_mode(&modes, None), Some(1));
        // No PREFERRED flag anywhere → the first advertised mode.
        let plain = [(1920, 1080, 60_000, false), (1280, 720, 60_000, false)];
        assert_eq!(pick_mode(&plain, None), Some(0));
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
    fn pick_mode_with_spec_prefers_flagged_then_refresh_then_index() {
        let modes = [
            (1920, 1080, 60_000, false),
            (1920, 1080, 144_000, false),
            (1920, 1080, 75_000, true),
        ];
        let spec: ModeSpec = "1920x1080".parse().unwrap();
        // PREFERRED wins over the higher refresh.
        assert_eq!(pick_mode(&modes, Some(&spec)), Some(2));
        // Without a flagged match, the highest refresh wins.
        assert_eq!(pick_mode(&modes[..2], Some(&spec)), Some(1));
        // An exact refresh request selects only that rate (rounded).
        let hz: ModeSpec = "1920x1080@144".parse().unwrap();
        assert_eq!(pick_mode(&modes, Some(&hz)), Some(1));
        let odd: ModeSpec = "1920x1080@75".parse().unwrap();
        assert_eq!(pick_mode(&modes, Some(&odd)), Some(2));
    }
}
