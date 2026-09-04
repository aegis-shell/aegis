//! Continuous output frame streaming (ADR-0052).
//!
//! The registry lives on the compositor main loop. A stream is registered by
//! an IPC `StreamOutputStart` (authorization happens in `tessera-ipc` before the
//! request reaches this loop), is throttled to its `max_fps`, and fans one
//! shared GPU readback out to every due SHM stream. Delivery goes through
//! `tessera_ipc::Server::push_stream_frame`, whose bounded lane reports drops
//! back so the stream's cumulative `dropped` counter stays accurate.
//!
//! A client that explicitly opts in (IPC protocol 25) gets the zero-copy
//! dmabuf transport instead: a per-stream exportable capture surface receives
//! a GPU copy of each presented frame, the client learns the fixed slot ring
//! once at start, and frame events reference a slot without a pixel blob. A
//! delivered slot stays consumer-owned — pinned — until the client's
//! `StreamBufferRelease`; only a free slot may be rendered into again.
//!
//! Window targets (ADR-0127) do not crop the shared frame: each window
//! stream renders the window's complete surface tree into its own cached
//! offscreen target, independently of presentation and therefore safe
//! against occlusion, minimization, and foreign workspaces. Both transports
//! share the pacing (dirty tree at `max_fps`, one liveness re-render per
//! second), the cursor compositing, and the damage accumulation machinery.

use std::collections::BTreeMap;
use std::os::fd::{AsRawFd, OwnedFd};
use std::time::{Duration, Instant};

use super::*;

/// Default frame-rate cap when the client leaves `max_fps` unset.
const DEFAULT_MAX_FPS: u32 = 30;
/// Hard bounds on the negotiated frame-rate cap.
const MIN_MAX_FPS: u32 = 1;
const MAX_MAX_FPS: u32 = 240;
/// How long a live WINDOW stream may go without a rendered frame before the
/// liveness tick forces one re-render of its (clean) tree, so a consumer
/// observes ~1 fps and minimized windows keep honest thumbnails
/// (ADR-0127). Output streams pace differently: a due output stream
/// *forces a presentation* at its negotiated `max_fps` cadence, so the
/// liveness concept does not apply to them (ADR-0130).
pub(super) const LIVENESS_INTERVAL: Duration = Duration::from_secs(1);

/// DRM fourcc announced for dmabuf stream frames: the capture surfaces hold
/// opaque BGRA8 pixels, which is XRGB8888 on the wire.
const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258;
/// Capture-surface slot ring depth: the device-wide flux frames-in-flight
/// count the compositor's DRM device is created with.
const STREAM_SLOT_COUNT: usize = 3;
/// A slot acquire fence that never signals means the GPU wedged; drop the
/// frame and recycle the slot rather than stalling the ring forever.
const SLOT_FENCE_TIMEOUT: Duration = Duration::from_secs(1);

/// One control message from an IPC connection thread, applied on the main
/// loop. Mirrors the capture/interaction domain-control request pattern.
pub(super) struct StreamControlRequest {
    pub(super) conn_id: u64,
    pub(super) action: StreamControl,
}

pub(super) enum StreamControl {
    Start {
        max_fps: Option<u32>,
        target: tessera_ipc::StreamTarget,
        /// The client's explicit zero-copy opt-in (IPC protocol 25).
        allow_dmabuf: bool,
        /// The negotiated cursor mode (IPC protocol 29), already defaulted
        /// to `Hidden` by the IPC dispatcher.
        cursor: tessera_ipc::StreamCursorMode,
        reply: std::sync::mpsc::Sender<Result<tessera_ipc::StreamInfo, String>>,
    },
    /// The server already unregistered the delivery lane (`StreamOutputStop`
    /// request, per-frame authorization failure, or server-side end); the
    /// main loop only drops its own state.
    Stop { stream_id: u64 },
    /// The consumer finished reading a delivered dmabuf slot (IPC protocol
    /// 25); the slot may be rendered into again.
    ReleaseSlot { stream_id: u64, slot: u32 },
    /// The connection disconnected; every stream it owned was unregistered
    /// server-side.
    Disconnect,
}

/// Consumer-ownership state of one capture-surface slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotState {
    /// Available for the next rendered frame.
    Free,
    /// Rendered and exported; its acquire fence has not signaled yet, so no
    /// frame event references it.
    Rendering,
    /// Frame event delivered; the consumer owns the slot until
    /// `StreamBufferRelease`.
    Pinned,
}

/// Ring position and per-slot ownership of one dmabuf stream's capture
/// surface. Flux advances its frame ring in lockstep with submissions
/// starting at slot 0, so the slot a submission lands in is known before
/// `begin_frame`; submitting only into `Free` slots keeps the two rings
/// synchronized and never overwrites a consumer-owned image.
struct SlotRing {
    states: Vec<SlotState>,
    next: usize,
}

impl SlotRing {
    fn new(count: usize) -> Self {
        Self {
            states: vec![SlotState::Free; count],
            next: 0,
        }
    }

    /// The slot the next submitted frame lands in, or `None` while the
    /// consumer still owns it (the due frame then counts as dropped).
    fn next_submission_slot(&self) -> Option<usize> {
        (self.states[self.next] == SlotState::Free).then_some(self.next)
    }

    /// Record a submission into `slot` (the ring position at render time)
    /// and advance the ring.
    fn submitted(&mut self, slot: usize) {
        debug_assert_eq!(slot, self.next, "flux ring and slot tracking diverged");
        if slot < self.states.len() {
            self.states[slot] = SlotState::Rendering;
            self.next = (slot + 1) % self.states.len();
        }
    }

    /// A slot's acquire fence signaled: `delivered` distinguishes a frame
    /// handed to the connection lane (consumer-owned from here) from a
    /// backpressure drop (immediately reusable).
    fn fence_signaled(&mut self, slot: usize, delivered: bool) {
        if slot < self.states.len() {
            self.states[slot] = if delivered {
                SlotState::Pinned
            } else {
                SlotState::Free
            };
        }
    }

    /// Recycle a slot whose fence timed out: the consumer never saw the
    /// frame, so the slot is reusable without a release.
    fn recycle(&mut self, slot: usize) {
        if slot < self.states.len() && self.states[slot] == SlotState::Rendering {
            self.states[slot] = SlotState::Free;
        }
    }

    /// True while the ring's next submission slot is consumer-owned: every
    /// due frame drops until a `StreamBufferRelease` arrives. The two older
    /// slots are necessarily Rendering or Pinned as well, so the ring is
    /// genuinely full.
    fn next_is_pinned(&self) -> bool {
        self.states[self.next] == SlotState::Pinned
    }

    /// The consumer finished reading a pinned slot (`StreamBufferRelease`).
    fn release(&mut self, slot: u32) {
        if let Some(state) = self.states.get_mut(slot as usize)
            && *state == SlotState::Pinned
        {
            *state = SlotState::Free;
        }
    }
}

/// Damage sampled for one in-flight stream frame (ADR-0127): the raw
/// desktop-space accumulation plus the crop origin it was sampled against.
/// The stream's accumulator clears only when the frame is *delivered*; a
/// backpressure or security drop folds the sample back so the regions a
/// missed frame carried stay accumulated for the next one.
#[derive(Clone)]
pub(super) struct SampledDamage {
    origin: tessera_model::Point,
    damage: FrameDamage,
}

/// The full-target damage rect reported whenever precise damage is
/// unavailable (a forced/liveness frame, a moved crop origin, or damage
/// that never intersected the target): over-reporting is always safe.
fn full_target_damage(size: (u32, u32)) -> Vec<tessera_model::Rect> {
    vec![tessera_model::Rect::new(0, 0, size.0 as i32, size.1 as i32)]
}

/// Translate a sampled desktop-space damage into one stream's target
/// coordinate space: shift by the crop origin and clip to the target
/// extent. Falls back to the full target rect when nothing precise
/// survives — including when every accumulated rect lay outside the crop,
/// which keeps the wire contract free of empty damage lists.
fn damage_in_target(sampled: &SampledDamage, size: (u32, u32)) -> Vec<tessera_model::Rect> {
    let Some(rects) = sampled.damage.area_rects() else {
        return full_target_damage(size);
    };
    let extent = tessera_model::Rect::new(0, 0, size.0 as i32, size.1 as i32);
    let translated: Vec<tessera_model::Rect> = rects
        .iter()
        .filter_map(|rect| {
            tessera_model::Rect::new(
                rect.origin.x - sampled.origin.x,
                rect.origin.y - sampled.origin.y,
                rect.size.w,
                rect.size.h,
            )
            .intersect(extent)
        })
        .collect();
    if translated.is_empty() {
        full_target_damage(size)
    } else {
        translated
    }
}

/// A frame rendered into a capture-surface slot whose acquire fence has not
/// signaled yet. The imported source image, when the frame was blitted from
/// the presented dma-buf, is held until the fence fires — the slot's GPU
/// work may sample it — and retired with the entry's drop. Per-window
/// renders carry no source: the renderer's texture cache owns the sampled
/// images.
struct PendingSlotFrame {
    slot: usize,
    fence: OwnedFd,
    _source: Option<flux::Image>,
    sequence: u64,
    dropped: u64,
    submitted_at: Instant,
    /// Capture security generation snapshot at blit time (the SHM/readback
    /// path carries the same value through `CaptureCompletion::Stream`).
    /// A frame whose generation no longer matches at delivery covered a
    /// lock→unlock (or VT) boundary and is dropped, never handed to the
    /// consumer, mirroring the SHM worker's completion check.
    security_generation: u64,
    /// Damage sampled at capture time, delivered with the frame.
    damage: SampledDamage,
}

/// GPU state of a zero-copy dmabuf stream (IPC protocol 25): the per-stream
/// capture surface and its canvas, the slot ring, and the frames awaiting
/// their acquire fence.
struct DmabufStream {
    surface: flux::Surface,
    canvas: flux::Canvas,
    modifier: u64,
    slot_stride: u32,
    slot_bytes: u64,
    ring: SlotRing,
    pending: Vec<PendingSlotFrame>,
    /// Set when a ring-full drop has been logged; cleared (with a recovery
    /// log) on the next successful submission, so a stall logs once per
    /// episode instead of once per dropped frame.
    ring_stalled: bool,
}

/// Everything a new dmabuf stream needs, built at start time: the capture
/// surface with its canvas, the announced modifier, and the slot table the
/// IPC layer transfers to the client.
pub(super) struct DmabufCapture {
    surface: flux::Surface,
    canvas: flux::Canvas,
    modifier: u64,
    table: tessera_ipc::StreamSlotTable,
}

/// The cached offscreen readback target of an SHM window stream (ADR-0127):
/// created at stream start at the negotiated physical extent and reused for
/// every frame until the stream stops (a geometry restart is a fresh stream
/// with a fresh target).
pub(super) struct WindowShmTarget {
    surface: flux::Surface,
    canvas: flux::Canvas,
}

impl WindowShmTarget {
    pub(super) fn new(device: &flux::Device, width: u32, height: u32) -> Result<Self, String> {
        let surface =
            flux::Surface::offscreen_readback(device, width, height).map_err(|error| {
                format!(
                    "allocate window stream target: {error}{}",
                    flux_last_error_detail()
                )
            })?;
        surface.prepare_readback().map_err(|error| {
            format!(
                "prepare window stream readback: {error}{}",
                flux_last_error_detail()
            )
        })?;
        let canvas = flux::Canvas::new(&surface).map_err(|error| {
            format!(
                "create window stream canvas: {error}{}",
                flux_last_error_detail()
            )
        })?;
        Ok(Self { surface, canvas })
    }
}

/// Production state of one window stream's SHM frame (ADR-0127). dmabuf
/// window streams track their in-flight frames in the slot ring instead.
pub(super) enum WindowStreamStage {
    /// No frame in flight: the stream may render when its pacing says so.
    Idle,
    /// A frame was submitted to the cached surface; its readback has not
    /// completed. Only one frame traverses a stream's surface at a time.
    AwaitingReadback {
        security_generation: u64,
        damage: SampledDamage,
    },
    /// The completed readback is being converted on the capture worker; the
    /// completion arrives keyed by stream id.
    Converting { damage: SampledDamage },
}

/// Per-window-stream independent rendering state (ADR-0127). The window's
/// complete surface tree renders into the stream's own target — never
/// cropped from the desktop frame — so occlusion, minimization, and foreign
/// workspaces cannot leak foreign pixels into the stream.
pub(super) struct WindowStream {
    /// The SHM readback target; `None` when the stream runs the dmabuf
    /// transport (its surface and canvas live in [`DmabufStream`]).
    pub(super) shm: Option<WindowShmTarget>,
    /// Window/output model signatures at the last geometry re-resolution;
    /// the live geometry is re-resolved only when one of them moves.
    pub(super) geometry_sig: (u64, u64),
    /// Toplevel logical origin and extent at the last geometry
    /// re-resolution, for renders between re-resolutions and the
    /// cursor-over-window test.
    pub(super) origin: tessera_model::Point,
    pub(super) logical_size: tessera_model::Size,
    /// Capture scale in milli-units at the last geometry re-resolution.
    pub(super) scale_milli: u32,
    /// Content generations of the window's surface tree at the last
    /// rendered frame; a mismatch against the live tree marks it dirty.
    pub(super) generations: std::collections::HashMap<usize, u64>,
    /// The tree changed since the last rendered frame (recomputed each
    /// drive); a dirty tree paces renders at the stream's `max_fps`.
    pub(super) dirty: bool,
    /// In-flight SHM frame state.
    pub(super) stage: WindowStreamStage,
    /// When a completed readback first found the capture worker reserved by
    /// a one-shot; a frame held too long logs once instead of starving
    /// silently. Cleared when the frame leaves for the worker.
    pub(super) held_since: Option<Instant>,
}

struct OutputStream {
    conn_id: u64,
    frame_interval: Duration,
    last_frame: Option<Instant>,
    sequence: u64,
    dropped: u64,
    /// What the stream captures (ADR-0054, ADR-0126).
    target: tessera_ipc::StreamTarget,
    /// The negotiated cursor mode (IPC protocol 29, ADR-0127).
    cursor: tessera_ipc::StreamCursorMode,
    /// Physical size at start. A stream whose live target size differs is
    /// frozen with `StreamGeometryChanged`: consumers negotiate one fixed
    /// video size and restart explicitly.
    size: (u32, u32),
    /// Frozen after a geometry change (IPC protocol 29): the stream stays
    /// registered but is never due and never forces presentation until the
    /// client restarts it.
    frozen: bool,
    /// Zero-copy transport state (IPC protocol 25); `None` for SHM streams.
    dmabuf: Option<DmabufStream>,
    /// Independent per-window rendering state (ADR-0127); `None` for output
    /// targets.
    window: Option<WindowStream>,
    /// Damage accumulated since this stream's last *delivered* frame, in
    /// physical desktop pixels. Initialized full so the first frame is
    /// complete; sampled (cloned) when a frame is captured for the stream
    /// and cleared only on successful delivery, so dropped frames keep
    /// their regions accumulated (ADR-0127).
    damage_since_delivery: FrameDamage,
    /// The crop origin the damage accumulator was last sampled against, for
    /// the origin-move guard (a moved crop invalidates the coordinate space
    /// of everything accumulated).
    damage_origin: Option<tessera_model::Point>,
    /// SHM output streams: the damage sample for the frame currently
    /// traversing the shared readback lane, cloned post-present for every
    /// due stream (window and dmabuf streams carry theirs in their own
    /// in-flight state).
    pending_frame_damage: Option<SampledDamage>,
}

/// The live output streams, keyed by stream id.
pub(super) struct OutputStreams {
    next_id: u64,
    streams: BTreeMap<u64, OutputStream>,
}

impl OutputStream {
    /// The frame was delivered: the damage accumulator restarts from here.
    fn note_delivered(&mut self) {
        self.damage_since_delivery = FrameDamage::None;
    }

    /// The frame never reached the consumer: fold its damage sample back
    /// into the accumulator so the next frame still covers its regions.
    fn fold_damage_back(&mut self, sampled: SampledDamage) {
        self.damage_since_delivery = union_frame_damage(
            std::mem::replace(&mut self.damage_since_delivery, FrameDamage::None),
            sampled.damage,
        );
    }
}

impl Default for OutputStreams {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputStreams {
    pub(super) fn new() -> Self {
        Self {
            next_id: 1,
            streams: BTreeMap::new(),
        }
    }

    /// Register a stream and answer the IPC requester. `size` is the
    /// stream's physical pixel extent at start time — the output's extent
    /// for an output target, the window's for a window target (ADR-0054).
    pub(super) fn start(
        &mut self,
        conn_id: u64,
        max_fps: Option<u32>,
        size: (u32, u32),
        target: tessera_ipc::StreamTarget,
        cursor: tessera_ipc::StreamCursorMode,
    ) -> tessera_ipc::StreamInfo {
        let max_fps = max_fps
            .unwrap_or(DEFAULT_MAX_FPS)
            .clamp(MIN_MAX_FPS, MAX_MAX_FPS);
        let stream_id = self.next_id;
        self.next_id += 1;
        self.streams.insert(
            stream_id,
            OutputStream {
                conn_id,
                frame_interval: Duration::from_secs(1) / max_fps,
                last_frame: None,
                sequence: 0,
                dropped: 0,
                target,
                cursor,
                size,
                frozen: false,
                dmabuf: None,
                window: None,
                damage_since_delivery: FrameDamage::Full,
                damage_origin: None,
                pending_frame_damage: None,
            },
        );
        tessera_ipc::StreamInfo {
            stream_id,
            width: size.0,
            height: size.1,
            format: tessera_ipc::StreamPixelFormat::Bgra8,
            slots: None,
        }
    }

    /// Attach the independent-rendering state to a freshly started window
    /// stream (ADR-0127). The IPC reply is already computed; the state only
    /// takes part in the frame drives that follow.
    pub(super) fn attach_window(&mut self, stream_id: u64, window: WindowStream) {
        if let Some(stream) = self.streams.get_mut(&stream_id) {
            stream.window = Some(window);
        }
    }

    /// Register a zero-copy dmabuf stream (IPC protocol 25): SHM start plus
    /// the capture-surface state, answered with the dmabuf format and the
    /// slot table the IPC layer transfers to the client.
    pub(super) fn start_dmabuf(
        &mut self,
        conn_id: u64,
        max_fps: Option<u32>,
        size: (u32, u32),
        target: tessera_ipc::StreamTarget,
        cursor: tessera_ipc::StreamCursorMode,
        capture: DmabufCapture,
    ) -> tessera_ipc::StreamInfo {
        let slot_count = capture.table.fds.len();
        let slot_stride = capture.table.stride;
        let slot_bytes = capture.table.byte_len;
        let modifier = capture.modifier;
        let mut info = self.start(conn_id, max_fps, size, target, cursor);
        if let Some(stream) = self.streams.get_mut(&info.stream_id) {
            stream.dmabuf = Some(DmabufStream {
                surface: capture.surface,
                canvas: capture.canvas,
                modifier,
                slot_stride,
                slot_bytes,
                ring: SlotRing::new(slot_count),
                pending: Vec::new(),
                ring_stalled: false,
            });
        }
        info.format = tessera_ipc::StreamPixelFormat::Dmabuf {
            drm_format: DRM_FORMAT_XRGB8888,
            modifier,
        };
        info.slots = Some(capture.table);
        info
    }

    pub(super) fn stop(&mut self, stream_id: u64) {
        self.streams.remove(&stream_id);
    }

    /// How many capture streams are live right now, across every transport
    /// and target. Drives the shell's recording indicator (ADR-0128).
    pub(super) fn len(&self) -> usize {
        self.streams.len()
    }

    /// Drop every stream `conn_id` owned (its IPC connection went away).
    pub(super) fn disconnect(&mut self, conn_id: u64) {
        self.streams.retain(|_, stream| stream.conn_id != conn_id);
    }

    /// The consumer finished reading a dmabuf stream's slot (IPC protocol
    /// 25). Unknown streams, SHM streams, and slots that are not pinned are
    /// ignored: releases carry no authority worth an error path.
    pub(super) fn release_slot(&mut self, stream_id: u64, slot: u32) {
        let Some(dmabuf) = self
            .streams
            .get_mut(&stream_id)
            .and_then(|stream| stream.dmabuf.as_mut())
        else {
            return;
        };
        dmabuf.ring.release(slot);
    }

    /// Ids of OUTPUT-target streams due a frame at `now`, filtered by
    /// transport. A stream that never received a frame is due immediately.
    /// Frozen streams (geometry change pending a client restart) are never
    /// due. Window streams pace themselves independently of presentation
    /// (ADR-0127) and never appear here.
    fn due_ids_by_transport(&self, now: Instant, dmabuf: bool) -> Vec<u64> {
        self.streams
            .iter()
            .filter(|(_, stream)| !stream.frozen)
            .filter(|(_, stream)| stream.window.is_none())
            .filter(|(_, stream)| stream.dmabuf.is_some() == dmabuf)
            .filter(|(_, stream)| {
                stream
                    .last_frame
                    .is_none_or(|last| now.duration_since(last) >= stream.frame_interval)
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// Whether an OUTPUT-target stream is due a frame at its negotiated
    /// `max_fps` cadence — its first frame, or one full `frame_interval`
    /// without one — and may therefore *force* a presentation even on a
    /// static screen (ADR-0130: an active stream paces the loop, replacing
    /// ADR-0126's opportunistic-only capture whose one-second liveness
    /// floor starved consumers on quiet desktops). Window streams render
    /// offscreen and never force presentation.
    fn forcing_due(&self, now: Instant, dmabuf: bool) -> bool {
        self.streams
            .values()
            .filter(|stream| !stream.frozen)
            .filter(|stream| stream.window.is_none())
            .filter(|stream| stream.dmabuf.is_some() == dmabuf)
            .any(|stream| {
                stream
                    .last_frame
                    .is_none_or(|last| now.duration_since(last) >= stream.frame_interval)
            })
    }

    /// Whether a due SHM stream may force a presentation right now.
    pub(super) fn forcing_due_shm(&self, now: Instant) -> bool {
        self.forcing_due(now, false)
    }

    /// Whether a due dmabuf stream may force a composite right now.
    pub(super) fn forcing_due_dmabuf(&self, now: Instant) -> bool {
        self.forcing_due(now, true)
    }

    /// Whether any live OUTPUT-target stream exists. Direct scanout is
    /// disqualified while one does (ADR-0130): a page-flipped frame never
    /// passes through the compositor, so under scanout streams would only
    /// ever observe the forced-cadence composites.
    pub(super) fn any_output_live(&self) -> bool {
        self.streams
            .values()
            .any(|stream| !stream.frozen && stream.window.is_none())
    }

    /// Time until the soonest stream-driven wakeup at `now` — zero when one
    /// is already reached — across every live stream, or `None` when no
    /// streams exist. The main loop caps its idle wait with this. Output
    /// streams wake at their `frame_interval` deadline: a due output stream
    /// forces a presentation, so the loop must wake in time to drive it
    /// (ADR-0130). Window streams (ADR-0127) additionally wake at their
    /// `max_fps` cadence while their tree is dirty, fall back to the
    /// liveness tick otherwise, and poll briefly while a readback is in
    /// flight.
    pub(super) fn next_stream_wake_in(&self, now: Instant) -> Option<Duration> {
        self.streams
            .values()
            .filter(|stream| !stream.frozen)
            .map(|stream| {
                let pacing = stream.last_frame.map_or(Duration::ZERO, |last| {
                    (last + stream.frame_interval).saturating_duration_since(now)
                });
                let Some(window) = &stream.window else {
                    return pacing;
                };
                let liveness = stream.last_frame.map_or(Duration::ZERO, |last| {
                    (last + LIVENESS_INTERVAL).saturating_duration_since(now)
                });
                if matches!(window.stage, WindowStreamStage::AwaitingReadback { .. }) {
                    // The GPU readback is in flight; poll for it shortly.
                    return liveness.min(Duration::from_millis(1));
                }
                if window.dirty && matches!(window.stage, WindowStreamStage::Idle) {
                    return liveness.min(pacing);
                }
                liveness
            })
            .min()
    }

    /// Ids of due SHM streams (the shared-readback fan-out).
    pub(super) fn due_shm_ids(&self, now: Instant) -> Vec<u64> {
        self.due_ids_by_transport(now, false)
    }

    /// Whether any SHM stream is due — the per-frame capture-gating hot path
    /// only needs the predicate; this is `due_shm_ids` without the Vec.
    pub(super) fn any_shm_due(&self, now: Instant) -> bool {
        self.streams
            .iter()
            .filter(|(_, stream)| !stream.frozen)
            .filter(|(_, stream)| stream.window.is_none())
            .filter(|(_, stream)| stream.dmabuf.is_none())
            .any(|(_, stream)| {
                stream
                    .last_frame
                    .is_none_or(|last| now.duration_since(last) >= stream.frame_interval)
            })
    }

    /// Ids of due dmabuf streams (the post-present slot fan-out).
    pub(super) fn due_dmabuf_ids(&self, now: Instant) -> Vec<u64> {
        self.due_ids_by_transport(now, true)
    }

    /// Whether any dmabuf stream is due (predicate-only, allocation-free).
    pub(super) fn any_dmabuf_due(&self, now: Instant) -> bool {
        self.streams
            .iter()
            .filter(|(_, stream)| !stream.frozen)
            .filter(|(_, stream)| stream.window.is_none())
            .filter(|(_, stream)| stream.dmabuf.is_some())
            .any(|(_, stream)| {
                stream
                    .last_frame
                    .is_none_or(|last| now.duration_since(last) >= stream.frame_interval)
            })
    }

    /// Record that `stream_id` was offered a frame at `now`; `delivered`
    /// distinguishes a queued frame from a backpressure drop.
    pub(super) fn record_frame(&mut self, stream_id: u64, now: Instant, delivered: bool) {
        let Some(stream) = self.streams.get_mut(&stream_id) else {
            return;
        };
        stream.last_frame = Some(now);
        if delivered {
            stream.sequence += 1;
        } else {
            stream.dropped += 1;
        }
    }

    /// The next sequence number and cumulative drop count for a frame about
    /// to be pushed. The frame metadata carries the drop count *before* this
    /// frame so a consumer can compute per-frame deltas.
    pub(super) fn sequence_and_dropped(&self, stream_id: u64) -> Option<(u64, u64)> {
        self.streams
            .get(&stream_id)
            .map(|stream| (stream.sequence + 1, stream.dropped))
    }

    /// The crop target and start-time physical size of one live stream.
    pub(super) fn target_of(
        &self,
        stream_id: u64,
    ) -> Option<(tessera_ipc::StreamTarget, (u32, u32))> {
        self.streams
            .get(&stream_id)
            .map(|stream| (stream.target.clone(), stream.size))
    }

    /// The negotiated cursor mode of one live stream (IPC protocol 29).
    pub(super) fn cursor_of(&self, stream_id: u64) -> Option<tessera_ipc::StreamCursorMode> {
        self.streams.get(&stream_id).map(|stream| stream.cursor)
    }

    /// Whether any live SHM output stream negotiated the embedded cursor
    /// mode. Only then does a shared-readback binding attach a cursor
    /// snapshot for the worker to blend (ADR-0127); window streams draw
    /// their cursor on the GPU and never need it.
    pub(super) fn any_shm_embedded(&self) -> bool {
        self.streams.values().any(|stream| {
            !stream.frozen
                && stream.window.is_none()
                && stream.dmabuf.is_none()
                && stream.cursor == tessera_ipc::StreamCursorMode::Embedded
        })
    }

    /// Fold one presented frame's damage into every live stream's
    /// accumulator (ADR-0127). Frozen streams skip: their accumulator is
    /// discarded with them when the client restarts into a fresh stream.
    pub(super) fn accumulate_damage(&mut self, damage: &FrameDamage) {
        if matches!(damage, FrameDamage::None) {
            return;
        }
        for stream in self.streams.values_mut() {
            if stream.frozen {
                continue;
            }
            stream.damage_since_delivery = union_frame_damage(
                std::mem::replace(&mut stream.damage_since_delivery, FrameDamage::None),
                damage.clone(),
            );
        }
    }

    /// Clone one stream's damage accumulator for a frame about to be
    /// captured, applying the origin-move guard: if the stream's crop
    /// origin moved since the last sample, everything accumulated belongs
    /// to the old coordinate space and only full damage is honest. The
    /// accumulator itself is NOT reset here; it clears on delivery
    /// ([`OutputStream::note_delivered`]) so a dropped frame's regions stay
    /// accumulated for the next one.
    fn sample_damage(&mut self, stream_id: u64, origin: tessera_model::Point) -> SampledDamage {
        let Some(stream) = self.streams.get_mut(&stream_id) else {
            return SampledDamage {
                origin,
                damage: FrameDamage::Full,
            };
        };
        let moved = stream.damage_origin != Some(origin);
        stream.damage_origin = Some(origin);
        let damage = if moved {
            FrameDamage::Full
        } else {
            stream.damage_since_delivery.clone()
        };
        SampledDamage { origin, damage }
    }

    /// Freeze a stream after a geometry change (IPC protocol 29): it stays
    /// registered but is never due and never forces presentation, until the
    /// client restarts it with `StreamOutputStop` + `StreamOutputStart`.
    pub(super) fn freeze(&mut self, stream_id: u64) {
        if let Some(stream) = self.streams.get_mut(&stream_id) {
            stream.frozen = true;
        }
    }
}

/// The scale the output frame renders at: the primary output's geometry
/// (backend + `[[output]]` overrides), falling back to the host's own scale
/// (nested). Mirrors the presentation path's computation so window crops
/// land on the same physical pixels the readback carries.
pub(super) fn output_render_scale(server: &tessera_compositor::Server, host: &Host) -> f32 {
    server
        .output_infos()
        .first()
        .map(|output| output.geometry.scale.as_f32())
        .filter(|scale| *scale > 0.0)
        .unwrap_or_else(|| host.scale())
}

/// One output's current rectangle in physical desktop-frame pixels,
/// resolved from the live model by connector name (IPC protocol 29). The
/// logical rect maps through the desktop render scale exactly like a
/// window's rect, so an output's rectangle always contains the windows the
/// compositor placed on it. `None` when no live output carries the name.
pub(super) fn resolve_output_rect(
    outputs: &[tessera_model::output::OutputInfo],
    connector: &str,
    scale: f32,
    frame_width: u32,
    frame_height: u32,
) -> Option<tessera_model::Rect> {
    let output = outputs
        .iter()
        .find(|candidate| candidate.connector == connector)?;
    Some(logical_rect_to_physical(
        output.geometry.logical_rect(),
        scale,
        frame_width,
        frame_height,
    ))
}

/// Extract one target region's rows out of a shared full-frame readback.
/// `rect` is in physical pixels and already clamped to the frame.
fn crop_stream_frame(
    width: u32,
    bgra: &[u8],
    rect: tessera_model::Rect,
) -> (u32, u32, std::sync::Arc<[u8]>) {
    let crop_width = rect.size.w.max(0) as u32;
    let crop_height = rect.size.h.max(0) as u32;
    let x = rect.origin.x as usize;
    let y = rect.origin.y as usize;
    let row = crop_width as usize * 4;
    let mut out = Vec::with_capacity(row * crop_height as usize);
    for line in y..y + crop_height as usize {
        let start = (line * width as usize + x) * 4;
        out.extend_from_slice(&bgra[start..start + row]);
    }
    (crop_width, crop_height, out.into())
}

impl CompositorRuntime {
    /// Publish the live capture-stream count to the shell's recording
    /// indicator and IPC status subscribers when it changed (ADR-0128).
    /// Called after every mutation of the stream set — control drains on
    /// the iteration path, geometry/blit endings on the presentation
    /// paths — so the indicator tracks the frame the change happened on
    /// instead of waiting for unrelated damage.
    pub(super) fn publish_capture_stream_count(&mut self) {
        let count = self.streams.len() as u32;
        if self.system_status.capture_streams == count {
            return;
        }
        self.system_status.capture_streams = count;
        publish_system_status_parts(&self.system_status, &mut self.shell, &self.live, &self.ipc);
        self.damage.chrome_dirty = true;
    }

    /// Fan one converted readback out to every due SHM stream. Bounded per
    /// stream: a full delivery lane reports `false` from
    /// `tessera_ipc::Server::push_stream_frame` and the frame counts as dropped
    /// (ADR-0052), so a stalled consumer only ever loses frames. Connector
    /// streams crop the shared frame to their output's current rectangle
    /// (ADR-0126); window streams are no longer here — they render their
    /// own frames offscreen (ADR-0127). A target whose size changed freezes
    /// its stream with `StreamGeometryChanged` instead of delivering a size
    /// the consumer never negotiated; a connector that disappeared ends its
    /// stream. Streams that negotiated the embedded cursor mode are served
    /// the worker's cursor-composited twin of the frame when one was
    /// produced (ADR-0127); everyone else shares the pristine frame.
    pub(super) fn deliver_stream_frame(&mut self, frame: StreamPixels) {
        let Some(ipc) = self.ipc.as_ref() else {
            return;
        };
        let now = Instant::now();
        let mut outputs = None;
        let mut ended: Vec<(u64, String)> = Vec::new();
        let mut frozen: Vec<(u64, u32, u32)> = Vec::new();
        for stream_id in self.streams.due_shm_ids(now) {
            let Some((sequence, dropped)) = self.streams.sequence_and_dropped(stream_id) else {
                continue;
            };
            let Some((target, size)) = self.streams.target_of(stream_id) else {
                continue;
            };
            let cursor_embedded =
                self.streams.cursor_of(stream_id) == Some(tessera_ipc::StreamCursorMode::Embedded);
            let pixels = if cursor_embedded {
                frame.cursor_bgra.as_ref().unwrap_or(&frame.bgra)
            } else {
                &frame.bgra
            };
            let sampled = self
                .streams
                .streams
                .get_mut(&stream_id)
                .and_then(|stream| stream.pending_frame_damage.take());
            let payload_damage = |size: (u32, u32)| match &sampled {
                Some(sampled) => damage_in_target(sampled, size),
                None => full_target_damage(size),
            };
            let cropped = |rect: tessera_model::Rect| {
                let (width, height, pixels) = crop_stream_frame(frame.width, pixels, rect);
                let damage = payload_damage((width, height));
                tessera_ipc::StreamFramePayload::Pixels(tessera_ipc::StreamPixelFrame {
                    stream_id,
                    sequence,
                    width,
                    height,
                    stride: width * 4,
                    format: tessera_ipc::StreamPixelFormat::Bgra8,
                    damage,
                    dropped,
                    pixels,
                })
            };
            let payload = match &target {
                tessera_ipc::StreamTarget::Output { output: None } => {
                    let damage = payload_damage((frame.width, frame.height));
                    tessera_ipc::StreamFramePayload::Pixels(tessera_ipc::StreamPixelFrame {
                        stream_id,
                        sequence,
                        width: frame.width,
                        height: frame.height,
                        stride: frame.width * 4,
                        format: tessera_ipc::StreamPixelFormat::Bgra8,
                        damage,
                        dropped,
                        pixels: std::sync::Arc::clone(pixels),
                    })
                }
                tessera_ipc::StreamTarget::Output {
                    output: Some(connector),
                } => {
                    let outputs = outputs.get_or_insert_with(|| self.server.output_infos());
                    let scale = output_render_scale(&self.server, &self.host);
                    match resolve_output_rect(outputs, connector, scale, frame.width, frame.height)
                    {
                        Some(rect) if (rect.size.w as u32, rect.size.h as u32) == size => {
                            cropped(rect)
                        }
                        Some(rect) => {
                            frozen.push((stream_id, rect.size.w as u32, rect.size.h as u32));
                            continue;
                        }
                        None => {
                            ended.push((stream_id, format!("output '{connector}' disconnected")));
                            continue;
                        }
                    }
                }
                // Window streams render independently (ADR-0127) and are
                // never due for the shared readback.
                tessera_ipc::StreamTarget::Window { .. } => continue,
            };
            let delivered = ipc.push_stream_frame(payload);
            self.streams.record_frame(stream_id, now, delivered);
            if delivered && let Some(stream) = self.streams.streams.get_mut(&stream_id) {
                stream.note_delivered();
            }
        }
        for (stream_id, width, height) in frozen {
            log::info!("stream {stream_id}: target geometry changed to {width}x{height}; freezing");
            ipc.stream_geometry_changed(stream_id, width, height);
            self.streams.freeze(stream_id);
        }
        for (stream_id, reason) in ended {
            log::info!("stream {stream_id}: {reason}; ending");
            ipc.end_stream(stream_id, &reason);
            self.streams.stop(stream_id);
        }
        self.publish_capture_stream_count();
    }

    /// Clone the damage accumulator of every due SHM stream for the frame
    /// just presented (ADR-0127). Runs once per composite that bound a
    /// stream readback, after the frame's own damage was accumulated, so
    /// the samples describe exactly the pixels this frame carries. The
    /// accumulators keep their contents: they clear on delivery, so a
    /// backpressure drop loses nothing.
    pub(super) fn stash_shm_stream_damage(&mut self) {
        let now = Instant::now();
        let scale = output_render_scale(&self.server, &self.host);
        let (frame_width, frame_height) = self.surface.size();
        let mut outputs = None;
        for stream_id in self.streams.due_shm_ids(now) {
            let origin = match self.streams.target_of(stream_id) {
                Some((tessera_ipc::StreamTarget::Output { output: None }, _)) => {
                    tessera_model::Point { x: 0, y: 0 }
                }
                Some((
                    tessera_ipc::StreamTarget::Output {
                        output: Some(connector),
                    },
                    _,
                )) => {
                    let outputs = outputs.get_or_insert_with(|| self.server.output_infos());
                    match resolve_output_rect(outputs, &connector, scale, frame_width, frame_height)
                    {
                        Some(rect) => rect.origin,
                        // Delivery ends or freezes the stream; nothing to sample.
                        None => continue,
                    }
                }
                _ => continue,
            };
            let sampled = self.streams.sample_damage(stream_id, origin);
            if let Some(stream) = self.streams.streams.get_mut(&stream_id) {
                stream.pending_frame_damage = Some(sampled);
            }
        }
    }

    /// The cursor state to attach to a shared stream readback binding
    /// (ADR-0127): `Some` only when at least one live SHM output stream
    /// negotiated the embedded cursor mode and a theme cursor is currently
    /// drawable. The state is rasterized when the frame's readback is
    /// requested; the capture worker then produces a cursor-composited twin
    /// of the frame next to the pristine one. On the software-cursor
    /// fallback the presented frame already contains the cursor, so nothing
    /// is attached (`hidden` cannot subtract it there — nested/degraded
    /// only); a client-provided cursor surface is already composited into
    /// the frame as an overlay and is likewise left alone.
    pub(super) fn stream_shm_cursor_state(&self) -> Option<CaptureCursorState> {
        if !self.streams.any_shm_embedded() || self.host.uses_software_cursor() {
            return None;
        }
        let state = self.capture_cursor_state();
        (!state.hidden && !state.client_surface).then_some(state)
    }

    /// Create a dmabuf stream's capture surface and enumerate its slot ring
    /// (IPC protocol 25): `STREAM_SLOT_COUNT` blank frames visit the slots in
    /// order, exporting one descriptor per slot for the client's slot table.
    /// The modifier is constrained to the presentation surface's, so the
    /// post-present copy never crosses formats. Any failure is reported to
    /// the caller, which falls back to SHM.
    pub(super) fn create_dmabuf_capture(
        &self,
        width: u32,
        height: u32,
    ) -> Result<DmabufCapture, String> {
        let modifier = self
            .surface
            .dmabuf_modifier()
            .ok_or_else(|| "presentation surface is not dma-buf exportable".to_owned())?;
        let surface = flux::Surface::offscreen_dmabuf(&self.device, width, height, &[modifier])
            .map_err(|error| format!("capture surface: {error}{}", flux_last_error_detail()))?;
        let canvas = flux::Canvas::new(&surface)
            .map_err(|error| format!("capture canvas: {error}{}", flux_last_error_detail()))?;
        let mut fds: Vec<Option<OwnedFd>> = (0..STREAM_SLOT_COUNT).map(|_| None).collect();
        let mut stride = None;
        for (expected_slot, slot_fd) in fds.iter_mut().enumerate() {
            let frame = surface
                .begin_frame()
                .map_err(|error| format!("capture slot clear: {error}"))?;
            begin_opaque_frame(&canvas, &frame, flux::rgba(0, 0, 0, 255))
                .map_err(|error| format!("capture slot clear: {error}"))?;
            canvas
                .end_frame_checked()
                .map_err(|error| format!("capture slot clear: {error}"))?;
            let submitted = frame
                .submit()
                .map_err(|error| format!("capture slot clear: {error}"))?;
            submitted
                .present()
                .map_err(|error| format!("capture slot clear: {error}"))?;
            // Blocking export: start-up latency is acceptable, and the ring
            // order is the slot order.
            let export = surface.export_dmabuf().map_err(|error| {
                format!("capture slot export: {error}{}", flux_last_error_detail())
            })?;
            if export.slot as usize != expected_slot {
                return Err(format!(
                    "capture ring visited slot {}, expected {expected_slot}",
                    export.slot
                ));
            }
            if export.width != width || export.height != height {
                return Err("capture slot extent mismatch".to_owned());
            }
            match stride {
                Some(known) if known != export.stride => {
                    return Err("capture slots disagree on row stride".to_owned());
                }
                None => stride = Some(export.stride),
                _ => {}
            }
            *slot_fd = Some(export.fd);
        }
        let stride = stride.expect("the slot ring exported at least one slot");
        Ok(DmabufCapture {
            surface,
            canvas,
            modifier,
            table: tessera_ipc::StreamSlotTable {
                stride,
                byte_len: u64::from(stride) * u64::from(height),
                fds: fds
                    .into_iter()
                    .map(|fd| fd.expect("every ring slot exported"))
                    .collect(),
            },
        })
    }

    /// Copy the just-presented frame into every due dmabuf stream's next ring
    /// slot (IPC protocol 25). Runs after a successful composite present; the
    /// frame event is delivered later, once the slot's acquire fence signals
    /// (`poll_dmabuf_stream_fences`). A slot still owned by the consumer
    /// drops the due frame instead of stalling the ring. A frame that was
    /// submitted but could not be exported ends its stream: flux's ring
    /// advanced while the tracking here did not, so continuing could
    /// overwrite a consumer-owned slot. Window streams render their own
    /// tree into their capture surface (ADR-0127) and are not driven here.
    pub(super) fn blit_dmabuf_stream_frames(&mut self, acquire_fence: Option<&OwnedFd>) {
        let Some(presented) = self.host.presented_dmabuf() else {
            return;
        };
        let now = Instant::now();
        let scale = output_render_scale(&self.server, &self.host);
        let mut outputs = None;
        let mut ended: Vec<(u64, String)> = Vec::new();
        let mut frozen: Vec<(u64, u32, u32)> = Vec::new();
        // The cursor sprite is composited on the GPU for embedded streams
        // (ADR-0127). On the software-cursor fallback the presented frame
        // already contains the cursor; drawing a second one would double it
        // (and `hidden` cannot subtract it there — nested/degraded only).
        let cursor_state = (!self.host.uses_software_cursor())
            .then(|| self.capture_cursor_state())
            .filter(|state| !state.hidden && !state.client_surface);
        for stream_id in self.streams.due_dmabuf_ids(now) {
            // Connector-addressed streams blit only their output's
            // sub-region of the presented frame (IPC protocol 29); the same
            // geometry-change/gone rules as the SHM path apply.
            let mut cursor_blit = None;
            let src_rect = match self.streams.target_of(stream_id) {
                Some((
                    tessera_ipc::StreamTarget::Output {
                        output: Some(connector),
                    },
                    size,
                )) => {
                    let outputs = outputs.get_or_insert_with(|| self.server.output_infos());
                    match resolve_output_rect(
                        outputs,
                        &connector,
                        scale,
                        presented.width,
                        presented.height,
                    ) {
                        Some(rect) if (rect.size.w as u32, rect.size.h as u32) == size => {
                            cursor_blit = cursor_state.as_ref().and_then(|state| {
                                output_cursor_blit(
                                    state,
                                    outputs
                                        .iter()
                                        .find(|output| output.connector == connector)
                                        .map(|output| output.geometry.logical_rect()),
                                    scale,
                                )
                            });
                            Some(rect)
                        }
                        Some(rect) => {
                            frozen.push((stream_id, rect.size.w as u32, rect.size.h as u32));
                            continue;
                        }
                        None => {
                            ended.push((stream_id, format!("output '{connector}' disconnected")));
                            continue;
                        }
                    }
                }
                Some((tessera_ipc::StreamTarget::Output { output: None }, size)) => {
                    cursor_blit = cursor_state.as_ref().and_then(|state| {
                        let logical = tessera_model::Rect::new(
                            0,
                            0,
                            (size.0 as f32 / scale).round() as i32,
                            (size.1 as f32 / scale).round() as i32,
                        );
                        output_cursor_blit(state, Some(logical), scale)
                    });
                    None
                }
                _ => None,
            };
            let cursor_blit = cursor_blit.filter(|_| {
                self.streams.cursor_of(stream_id) == Some(tessera_ipc::StreamCursorMode::Embedded)
            });
            let damage_origin = src_rect
                .as_ref()
                .map(|rect| rect.origin)
                .unwrap_or(tessera_model::Point { x: 0, y: 0 });
            let sampled = self.streams.sample_damage(stream_id, damage_origin);
            let Some(stream) = self.streams.streams.get_mut(&stream_id) else {
                continue;
            };
            let Some(dmabuf) = stream.dmabuf.as_mut() else {
                continue;
            };
            let Some(_slot) = dmabuf.ring.next_submission_slot() else {
                // The consumer still owns the ring's next slot.
                stream.dropped += 1;
                if dmabuf.ring.next_is_pinned() && !dmabuf.ring_stalled {
                    dmabuf.ring_stalled = true;
                    log::warn!(
                        "stream {stream_id}: every capture slot is consumer-owned; \
                         dropping frames until the consumer releases one"
                    );
                }
                continue;
            };
            let sequence = stream.sequence + 1;
            let dropped = stream.dropped;
            // Snapshot the security generation with the blit: delivery below
            // re-checks it, mirroring the SHM worker's completion check.
            let security_generation = self.capture_worker.security_generation();
            match blit_presented_frame(
                &self.device,
                dmabuf,
                &presented,
                acquire_fence,
                src_rect.as_ref(),
                cursor_blit,
                &mut self.cursor_cache,
            ) {
                Ok((slot, fence, source)) => {
                    if dmabuf.ring_stalled {
                        dmabuf.ring_stalled = false;
                        log::info!("stream {stream_id}: capture slots are flowing again");
                    }
                    dmabuf.ring.submitted(slot);
                    dmabuf.pending.push(PendingSlotFrame {
                        slot,
                        fence,
                        _source: Some(source),
                        sequence,
                        dropped,
                        submitted_at: now,
                        security_generation,
                        damage: sampled,
                    });
                    stream.sequence += 1;
                    stream.last_frame = Some(now);
                }
                Err(BlitFailure::Retryable(reason)) => {
                    log::warn!("stream {stream_id}: dmabuf capture frame failed: {reason}");
                    stream.dropped += 1;
                    stream.last_frame = Some(now);
                }
                Err(BlitFailure::Submitted(reason)) => ended.push((stream_id, reason)),
            }
        }
        if let Some(ipc) = self.ipc.as_ref() {
            for (stream_id, width, height) in frozen {
                log::info!(
                    "stream {stream_id}: target geometry changed to {width}x{height}; freezing"
                );
                ipc.stream_geometry_changed(stream_id, width, height);
                self.streams.freeze(stream_id);
            }
            for (stream_id, reason) in ended {
                log::warn!("stream {stream_id}: {reason}; ending");
                ipc.end_stream(stream_id, &reason);
                self.streams.stop(stream_id);
            }
        }
        self.publish_capture_stream_count();
    }

    /// Deliver dmabuf stream frames whose acquire fence signaled (IPC
    /// protocol 25), mirroring the SHM path's `read_pixels_ready` polling. A
    /// signaled frame is pushed to its connection lane and its slot becomes
    /// consumer-owned until `StreamBufferRelease`; a full lane or a wedged
    /// fence counts the frame as dropped and recycles the slot. A dropped
    /// frame's damage folds back into the stream's accumulator so the next
    /// delivered frame still covers its regions (ADR-0127).
    pub(super) fn poll_dmabuf_stream_fences(&mut self) {
        let Some(ipc) = self.ipc.as_ref() else {
            return;
        };
        let now = Instant::now();
        for (stream_id, stream) in &mut self.streams.streams {
            let stream_id = *stream_id;
            let Some(dmabuf) = stream.dmabuf.as_mut() else {
                continue;
            };
            let mut index = 0;
            while index < dmabuf.pending.len() {
                let signaled = fence_signaled(&dmabuf.pending[index].fence);
                let timed_out =
                    now.duration_since(dmabuf.pending[index].submitted_at) >= SLOT_FENCE_TIMEOUT;
                if !signaled && !timed_out {
                    index += 1;
                    continue;
                }
                let pending = dmabuf.pending.remove(index);
                if !signaled {
                    log::warn!(
                        "stream {stream_id}: slot {} acquire fence timed out; frame dropped",
                        pending.slot
                    );
                    dmabuf.ring.recycle(pending.slot);
                    stream.dropped += 1;
                    // Disjoint-field fold: `dmabuf` is still borrowed above.
                    stream.damage_since_delivery = union_frame_damage(
                        std::mem::replace(&mut stream.damage_since_delivery, FrameDamage::None),
                        pending.damage.damage,
                    );
                    continue;
                }
                // Security-generation check (mirrors the SHM worker's
                // completion gate): a frame blitted before a lock→unlock or
                // VT boundary must never reach the consumer afterwards.
                if !self.capture_worker.permits(pending.security_generation) {
                    log::debug!(
                        "stream {stream_id}: slot {} crossed a security boundary; frame dropped",
                        pending.slot
                    );
                    dmabuf.ring.recycle(pending.slot);
                    stream.dropped += 1;
                    stream.damage_since_delivery = union_frame_damage(
                        std::mem::replace(&mut stream.damage_since_delivery, FrameDamage::None),
                        pending.damage.damage,
                    );
                    continue;
                }
                let (width, height) = stream.size;
                let payload = tessera_ipc::StreamFramePayload::Slot(tessera_ipc::StreamSlotFrame {
                    stream_id,
                    sequence: pending.sequence,
                    width,
                    height,
                    stride: dmabuf.slot_stride,
                    format: tessera_ipc::StreamPixelFormat::Dmabuf {
                        drm_format: DRM_FORMAT_XRGB8888,
                        modifier: dmabuf.modifier,
                    },
                    damage: damage_in_target(&pending.damage, (width, height)),
                    dropped: pending.dropped,
                    slot: pending.slot as u32,
                    byte_len: dmabuf.slot_bytes,
                });
                let delivered = ipc.push_stream_frame(payload);
                dmabuf.ring.fence_signaled(pending.slot, delivered);
                if delivered {
                    // Disjoint from the `dmabuf` field borrow.
                    stream.damage_since_delivery = FrameDamage::None;
                } else {
                    stream.dropped += 1;
                    stream.damage_since_delivery = union_frame_damage(
                        std::mem::replace(&mut stream.damage_since_delivery, FrameDamage::None),
                        pending.damage.damage,
                    );
                }
            }
        }
    }
}

/// Non-blocking poll on a sync_file fence: a signaled (or errored) fence is
/// readable.
fn fence_signaled(fence: &OwnedFd) -> bool {
    let mut pollfd = libc::pollfd {
        fd: fence.as_raw_fd(),
        events: libc::POLLIN,
        revents: 0,
    };
    // SAFETY: `pollfd` references a live descriptor for the duration of the
    // call and a zero timeout never blocks.
    let signaled = unsafe { libc::poll(&mut pollfd, 1, 0) };
    signaled > 0
}

/// Why a capture-surface blit failed. The distinction that matters is
/// whether flux's ring advanced: only a submission moves it, and the slot
/// tracking in [`SlotRing`] must stay in lockstep with it.
#[derive(Debug)]
enum BlitFailure {
    /// Nothing was submitted; the stream may retry its next due frame.
    Retryable(String),
    /// The frame was submitted but the slot could not be exported; the ring
    /// position diverged and the stream must end.
    Submitted(String),
}

/// A GPU cursor draw into a capture-surface pass (ADR-0127): the theme
/// cursor's shape, its logical position relative to the capture target's
/// origin, and the target's render scale. Drawn after the frame content so
/// the sprite composits above it, clipped by the target extent.
struct StreamCursorBlit {
    shape: u32,
    position: (f32, f32),
    scale: f32,
}

/// The cursor draw request for one embedded output dmabuf stream (ADR-0127):
/// the theme cursor's shape and its logical position relative to the
/// capture target's origin, when the cursor position (its hotspot) falls
/// inside the target's logical rect. `None` keeps the capture cursor-free.
fn output_cursor_blit(
    state: &CaptureCursorState,
    target_logical: Option<tessera_model::Rect>,
    scale: f32,
) -> Option<StreamCursorBlit> {
    let rect = target_logical?;
    let (cx, cy) = state.position;
    let inside = cx >= rect.origin.x as f32
        && cx < (rect.origin.x + rect.size.w) as f32
        && cy >= rect.origin.y as f32
        && cy < (rect.origin.y + rect.size.h) as f32;
    inside.then_some(StreamCursorBlit {
        shape: state.shape,
        position: (cx - rect.origin.x as f32, cy - rect.origin.y as f32),
        scale,
    })
}

/// Submit one rendered capture-surface frame and export its slot with the
/// explicit acquire fence (ADR-0055). Shared by the post-present blit and
/// the per-window capture-surface renders (ADR-0127); everything past the
/// submit advances flux's ring, so failures there are [`BlitFailure::Submitted`].
fn submit_capture_frame(
    dmabuf: &DmabufStream,
    frame: flux::Frame<'_>,
) -> Result<(usize, OwnedFd), BlitFailure> {
    let submitted = frame
        .submit()
        .map_err(|error| BlitFailure::Retryable(format!("capture submit: {error}")))?;
    submitted
        .present()
        .map_err(|error| BlitFailure::Submitted(format!("capture present: {error}")))?;
    let export = dmabuf.surface.export_dmabuf_explicit().map_err(|error| {
        BlitFailure::Submitted(format!(
            "capture slot export: {error}{}",
            flux_last_error_detail()
        ))
    })?;
    if export.slot as usize >= dmabuf.ring.states.len() {
        return Err(BlitFailure::Submitted(format!(
            "capture export reported out-of-ring slot {}",
            export.slot
        )));
    }
    let Some(fence) = export.acquire_fence else {
        return Err(BlitFailure::Submitted(
            "capture slot export returned no acquire fence".to_owned(),
        ));
    };
    Ok((export.slot as usize, fence))
}

/// Import the presented frame and copy it into the capture surface's next
/// ring slot, exporting the slot with its explicit acquire fence. Returns
/// the slot, the fence to poll, and the imported source image (retired when
/// the fence fires). The presented descriptor stays owned by the caller;
/// flux receives duplicates. `src_rect` (IPC protocol 29) samples only that
/// physical-pixel sub-region of the presented frame, for
/// connector-addressed streams; `None` copies the whole frame. `cursor`
/// (ADR-0127) composites the theme cursor sprite above the copied frame,
/// for streams that negotiated the embedded cursor mode.
#[allow(clippy::too_many_arguments)]
fn blit_presented_frame(
    device: &flux::Device,
    dmabuf: &mut DmabufStream,
    presented: &tessera_backend::host::PresentedDmabuf,
    acquire_fence: Option<&OwnedFd>,
    src_rect: Option<&tessera_model::Rect>,
    cursor: Option<StreamCursorBlit>,
    cursor_cache: &mut cursor::CursorCache,
) -> Result<(usize, OwnedFd, flux::Image), BlitFailure> {
    let fd = presented
        .fd
        .try_clone()
        .map_err(|error| BlitFailure::Retryable(format!("duplicate presented dma-buf: {error}")))?;
    let fence = acquire_fence
        .map(|fence| {
            fence.try_clone().map_err(|error| {
                BlitFailure::Retryable(format!("duplicate acquire fence: {error}"))
            })
        })
        .transpose()?;
    // SAFETY: the backend's descriptor references a live dma-buf matching the
    // metadata it reported; the fence, when present, orders the capture pass
    // after the compositor's own rendering of that image. The import entry
    // points take `OwnedFd`s by value: on success flux consumes and closes
    // them, on error the `OwnedFd` drops close them — no leak, no
    // double-close, so the old `mem::forget` choreography is gone.
    let import = unsafe {
        match fence {
            Some(fence) => flux::Image::import_dmabuf_with_acquire_fence(
                device,
                presented.width,
                presented.height,
                flux::Format::Bgra8Unorm,
                presented.modifier,
                fd,
                0,
                presented.stride,
                fence,
            ),
            None => flux::Image::import_dmabuf(
                device,
                presented.width,
                presented.height,
                flux::Format::Bgra8Unorm,
                presented.modifier,
                fd,
                0,
                presented.stride,
            ),
        }
    };
    let source = import.map_err(|error| {
        BlitFailure::Retryable(format!(
            "import presented dma-buf: {error}{}",
            flux_last_error_detail()
        ))
    })?;
    let frame = dmabuf.surface.begin_frame().map_err(|error| {
        BlitFailure::Retryable(format!(
            "capture begin_frame: {error}{}",
            flux_last_error_detail()
        ))
    })?;
    begin_opaque_frame(&dmabuf.canvas, &frame, flux::rgba(0, 0, 0, 255))
        .map_err(|error| BlitFailure::Retryable(format!("capture pass: {error}")))?;
    let (dst_w, dst_h) = dmabuf.surface.size();
    match src_rect {
        // Connector-addressed stream: sample only the output's sub-region
        // of the desktop frame into the (output-sized) capture surface.
        Some(rect) => {
            let full_w = presented.width as f32;
            let full_h = presented.height as f32;
            dmabuf.canvas.draw_image_opaque_sub(
                &source,
                0.0,
                0.0,
                dst_w as f32,
                dst_h as f32,
                rect.origin.x as f32 / full_w,
                rect.origin.y as f32 / full_h,
                rect.size.w as f32 / full_w,
                rect.size.h as f32 / full_h,
            );
        }
        None => dmabuf.canvas.draw_image_opaque(
            &source,
            0.0,
            0.0,
            presented.width as f32,
            presented.height as f32,
        ),
    }
    // Embedded cursor mode (ADR-0127): the theme cursor composits above the
    // captured frame, translated into the target's coordinate space.
    if let Some(cursor) = cursor {
        draw_software_cursor(
            &dmabuf.canvas,
            device,
            cursor_cache,
            cursor.position,
            cursor.shape,
            cursor.scale,
        );
    }
    dmabuf
        .canvas
        .end_frame_checked()
        .map_err(|error| BlitFailure::Retryable(format!("capture pass: {error}")))?;
    let (slot, fence) = submit_capture_frame(dmabuf, frame)?;
    Ok((slot, fence, source))
}

impl CompositorRuntime {
    /// Reconcile every live stream with a new presentation-surface geometry
    /// (IPC protocol 29, ADR-0126). Called for ANY surface-size change —
    /// hotplug or config mode change through `take_resize`, and the flux
    /// `begin_frame` failure rebuild — so streams can no longer keep
    /// delivering at a stale geometry. Per stream target: a whole-desktop
    /// stream whose desktop size changed, and a connector stream whose
    /// output's size changed, are frozen with `StreamGeometryChanged`
    /// (the client restarts them at the new geometry); a connector that
    /// disappeared ends its stream. Pure position moves are followed
    /// silently at the next frame. Window streams detect their own
    /// geometry changes lazily at delivery.
    pub(super) fn handle_output_geometry_change(&mut self) {
        let Some(ipc) = self.ipc.as_ref() else {
            return;
        };
        let (frame_width, frame_height) = self.surface.size();
        let scale = output_render_scale(&self.server, &self.host);
        let outputs = self.server.output_infos();
        let mut actions: Vec<(u64, GeometryAction)> = Vec::new();
        for (stream_id, stream) in &self.streams.streams {
            if stream.frozen {
                continue;
            }
            let action = match &stream.target {
                tessera_ipc::StreamTarget::Output { output: None } => {
                    if stream.size != (frame_width, frame_height) {
                        GeometryAction::Freeze(frame_width, frame_height)
                    } else {
                        GeometryAction::Unchanged
                    }
                }
                tessera_ipc::StreamTarget::Output {
                    output: Some(connector),
                } => {
                    match resolve_output_rect(&outputs, connector, scale, frame_width, frame_height)
                    {
                        Some(rect) if (rect.size.w as u32, rect.size.h as u32) == stream.size => {
                            GeometryAction::Unchanged
                        }
                        Some(rect) => {
                            GeometryAction::Freeze(rect.size.w as u32, rect.size.h as u32)
                        }
                        None => GeometryAction::End(format!("output '{connector}' disconnected")),
                    }
                }
                // Window streams keep their lazy delivery-time detection.
                tessera_ipc::StreamTarget::Window { .. } => GeometryAction::Unchanged,
            };
            if action != GeometryAction::Unchanged {
                actions.push((*stream_id, action));
            }
        }
        if actions.is_empty() {
            return;
        }
        log::info!(
            "stream: output geometry changed; reconciling {} stream(s)",
            actions.len()
        );
        for (stream_id, action) in actions {
            match action {
                GeometryAction::Freeze(width, height) => {
                    log::info!(
                        "stream {stream_id}: target geometry changed to {width}x{height}; freezing"
                    );
                    ipc.stream_geometry_changed(stream_id, width, height);
                    self.streams.freeze(stream_id);
                }
                GeometryAction::End(reason) => {
                    log::info!("stream {stream_id}: {reason}; ending");
                    ipc.end_stream(stream_id, &reason);
                    self.streams.stop(stream_id);
                }
                GeometryAction::Unchanged => {}
            }
        }
        self.publish_capture_stream_count();
    }
}

/// Content-generation snapshot of one window's surface tree (ADR-0127):
/// every surface id with its current commit generation. The scene frame
/// lists enumerate exactly the window's tree (toplevel, subsurfaces,
/// popups), so a diff against the snapshot from the last render is the
/// per-window dirty signal — the same per-surface generation machinery the
/// output damage tracker diffs globally.
fn window_tree_generations(
    server: &tessera_compositor::Server,
    window: tessera_model::window::WindowId,
) -> std::collections::HashMap<usize, u64> {
    server
        .window_capture_frames(window)
        .iter()
        .map(|frame| (frame.id, frame.generation))
        .chain(
            server
                .window_capture_dmabuf_frames(window)
                .iter()
                .map(|frame| (frame.id, frame.generation)),
        )
        .collect()
}

/// Whether a window stream renders in this drive (ADR-0127): a dirty tree
/// paces at the stream's `max_fps`; the liveness tick re-renders a clean
/// tree so a consumer still observes ~1 fps and minimized windows keep
/// honest thumbnails. A stream with a frame in flight never double-books
/// its surface.
fn window_stream_render_due(
    dirty: bool,
    stage_idle: bool,
    last_frame: Option<Instant>,
    frame_interval: Duration,
    now: Instant,
) -> bool {
    if !stage_idle {
        return false;
    }
    let fps_due = last_frame.is_none_or(|last| now.duration_since(last) >= frame_interval);
    let liveness_due = last_frame.is_none_or(|last| now.duration_since(last) >= LIVENESS_INTERVAL);
    (dirty && fps_due) || liveness_due
}

/// The cursor draw request for an embedded window stream (ADR-0127): the
/// theme cursor's shape and its logical position relative to the window's
/// origin, when the cursor position (its hotspot) falls inside the window's
/// logical rect. `None` keeps the frame cursor-free; a client-provided
/// cursor surface is filtered by the caller (it is not part of the
/// window's surface tree and can never appear in a window stream).
fn window_stream_cursor(
    state: &CaptureCursorState,
    origin: tessera_model::Point,
    logical_size: tessera_model::Size,
) -> Option<(u32, (f32, f32))> {
    let (cx, cy) = state.position;
    let inside = cx >= origin.x as f32
        && cx < (origin.x + logical_size.w) as f32
        && cy >= origin.y as f32
        && cy < (origin.y + logical_size.h) as f32;
    inside.then_some((state.shape, (cx - origin.x as f32, cy - origin.y as f32)))
}

/// Render the window's surface tree (plus the negotiated cursor) into the
/// SHM stream's cached readback target and submit the frame. Mirrors the
/// one-shot `begin_window_capture` sequence, but reuses the per-stream
/// surface and leaves the in-flight bookkeeping to the caller.
#[allow(clippy::too_many_arguments)]
fn render_window_stream_shm(
    device: &flux::Device,
    renderer: &mut tessera_render::Renderer,
    server: &tessera_compositor::Server,
    target: &WindowShmTarget,
    geometry: &WindowTreeGeometry,
    cursor: Option<(u32, (f32, f32))>,
    cursor_cache: &mut cursor::CursorCache,
    scheme: tessera_model::settings::ColorScheme,
) -> Result<(), String> {
    let mut frame = target.surface.begin_frame().map_err(|error| {
        format!(
            "begin window stream frame: {error}{}",
            flux_last_error_detail()
        )
    })?;
    begin_opaque_frame(&target.canvas, &frame, interaction_domain_clear(scheme))
        .map_err(|error| format!("begin window stream pass: {error}"))?;
    draw_window_tree(device, renderer, server, &target.canvas, geometry);
    if let Some((shape, position)) = cursor {
        let scale = geometry.scale_milli as f32 / 1000.0;
        draw_software_cursor(&target.canvas, device, cursor_cache, position, shape, scale);
    }
    target
        .canvas
        .end_frame_checked()
        .map_err(|error| format!("end window stream pass: {error}"))?;
    frame
        .request_readback()
        .map_err(|error| format!("request window stream readback: {error}"))?;
    frame
        .submit()
        .and_then(flux::SubmittedFrame::present)
        .map_err(|error| format!("submit window stream frame: {error}"))?;
    Ok(())
}

/// Render the window's surface tree (plus the negotiated cursor) into the
/// dmabuf stream's next capture-surface slot and export it with its
/// explicit acquire fence (ADR-0127). Shares the submit/export tail with
/// the presented-frame blit so the ring bookkeeping — and its failure
/// semantics — stay identical.
#[allow(clippy::too_many_arguments)]
fn render_window_stream_dmabuf(
    device: &flux::Device,
    renderer: &mut tessera_render::Renderer,
    server: &tessera_compositor::Server,
    dmabuf: &mut DmabufStream,
    geometry: &WindowTreeGeometry,
    cursor: Option<(u32, (f32, f32))>,
    cursor_cache: &mut cursor::CursorCache,
    scheme: tessera_model::settings::ColorScheme,
) -> Result<(usize, OwnedFd), BlitFailure> {
    let frame = dmabuf.surface.begin_frame().map_err(|error| {
        BlitFailure::Retryable(format!(
            "capture begin_frame: {error}{}",
            flux_last_error_detail()
        ))
    })?;
    begin_opaque_frame(&dmabuf.canvas, &frame, interaction_domain_clear(scheme))
        .map_err(|error| BlitFailure::Retryable(format!("capture pass: {error}")))?;
    draw_window_tree(device, renderer, server, &dmabuf.canvas, geometry);
    if let Some((shape, position)) = cursor {
        let scale = geometry.scale_milli as f32 / 1000.0;
        draw_software_cursor(&dmabuf.canvas, device, cursor_cache, position, shape, scale);
    }
    dmabuf
        .canvas
        .end_frame_checked()
        .map_err(|error| BlitFailure::Retryable(format!("capture pass: {error}")))?;
    submit_capture_frame(dmabuf, frame)
}

impl CompositorRuntime {
    /// Drive every live window stream one step (ADR-0127): re-resolve
    /// geometry when the window/output model moved (a size change freezes
    /// the stream with `StreamGeometryChanged`, a closed window ends it, a
    /// pure position move is followed silently), then render the streams
    /// whose pacing is due into their own offscreen targets — independently
    /// of presentation. Runs once per main-loop iteration; the loop's idle
    /// wait consults [`OutputStreams::next_stream_wake_in`] so dirty
    /// windows keep their `max_fps` cadence.
    pub(super) fn drive_window_streams(&mut self) {
        if self.server.session_locked() || !self.host.is_active() {
            return;
        }
        let now = Instant::now();
        let sig = (
            self.server.all_windows_signature(),
            self.server.outputs_revision(),
        );
        let scheme = self.shell.design().scheme;
        let security_generation = self.capture_worker.security_generation();
        let mut ended: Vec<(u64, String)> = Vec::new();
        let mut frozen: Vec<(u64, u32, u32)> = Vec::new();
        let ids: Vec<u64> = self
            .streams
            .streams
            .iter()
            .filter(|(_, stream)| !stream.frozen && stream.window.is_some())
            .map(|(stream_id, _)| *stream_id)
            .collect();
        if ids.is_empty() {
            return;
        }
        // The theme cursor, when one is currently drawable. A client-owned
        // cursor surface is not part of any window's surface tree, so
        // window streams can never show it; nothing is drawn then.
        let cursor_state = {
            let state = self.capture_cursor_state();
            (!state.hidden && !state.client_surface).then_some(state)
        };
        for stream_id in ids {
            let Some(stream) = self.streams.streams.get_mut(&stream_id) else {
                continue;
            };
            let window_id = match stream.target {
                tessera_ipc::StreamTarget::Window { window } => window,
                _ => continue,
            };
            let window = stream
                .window
                .as_mut()
                .expect("window stream state attached");
            // Geometry re-resolution is signature-gated: between signature
            // moves the cached origin/size/scale are authoritative.
            let mut fresh_geometry = None;
            if window.geometry_sig != sig {
                window.geometry_sig = sig;
                match window_tree_geometry(&self.server, window_id) {
                    Ok(geometry) => {
                        if (geometry.physical_width, geometry.physical_height) != stream.size {
                            frozen.push((
                                stream_id,
                                geometry.physical_width,
                                geometry.physical_height,
                            ));
                            continue;
                        }
                        window.origin = geometry.origin;
                        window.logical_size = geometry.logical_size;
                        window.scale_milli = geometry.scale_milli;
                        fresh_geometry = Some(geometry);
                    }
                    Err(reason) => {
                        // A window that vanished from the model closed; any
                        // other resolution failure (e.g. a zero-sized
                        // window) ends the stream with its own reason.
                        let reason = if reason.starts_with("unknown window") {
                            "window closed".to_owned()
                        } else {
                            reason
                        };
                        ended.push((stream_id, reason));
                        continue;
                    }
                }
            }
            // Dirty check: the tree's content generations against the last
            // rendered snapshot. A re-render of a clean tree happens only
            // at the liveness tick.
            let live = window_tree_generations(&self.server, window_id);
            window.dirty = live != window.generations;
            let stage_idle = matches!(window.stage, WindowStreamStage::Idle);
            if !window_stream_render_due(
                window.dirty,
                stage_idle,
                stream.last_frame,
                stream.frame_interval,
                now,
            ) {
                continue;
            }
            let geometry = fresh_geometry.unwrap_or(WindowTreeGeometry {
                window: window_id,
                scale_milli: window.scale_milli,
                physical_width: stream.size.0,
                physical_height: stream.size.1,
                origin: window.origin,
                logical_size: window.logical_size,
            });
            // Damage for this frame: the desktop-space accumulation clipped
            // to the window's physical rect, translated into target
            // coordinates at delivery time.
            let scale = geometry.scale_milli as f32 / 1000.0;
            let damage_origin = tessera_model::Point {
                x: (geometry.origin.x as f32 * scale).floor() as i32,
                y: (geometry.origin.y as f32 * scale).floor() as i32,
            };
            let cursor_draw = (stream.cursor == tessera_ipc::StreamCursorMode::Embedded)
                .then_some(cursor_state.as_ref())
                .flatten()
                .and_then(|state| {
                    window_stream_cursor(state, geometry.origin, geometry.logical_size)
                });
            let sampled = self.streams.sample_damage(stream_id, damage_origin);
            let Some(stream) = self.streams.streams.get_mut(&stream_id) else {
                continue;
            };
            let window = stream
                .window
                .as_mut()
                .expect("window stream state attached");
            match &mut window.shm {
                Some(target) => {
                    match render_window_stream_shm(
                        &self.device,
                        &mut self.renderer,
                        &self.server,
                        target,
                        &geometry,
                        cursor_draw,
                        &mut self.cursor_cache,
                        scheme,
                    ) {
                        Ok(()) => {
                            window.generations = live;
                            window.dirty = false;
                            window.stage = WindowStreamStage::AwaitingReadback {
                                security_generation,
                                damage: sampled,
                            };
                            stream.last_frame = Some(now);
                        }
                        Err(reason) => {
                            log::warn!("stream {stream_id}: window frame render failed: {reason}");
                            stream.dropped += 1;
                            stream.last_frame = Some(now);
                        }
                    }
                }
                None => {
                    let Some(dmabuf) = stream.dmabuf.as_mut() else {
                        continue;
                    };
                    let Some(_slot) = dmabuf.ring.next_submission_slot() else {
                        // The consumer still owns the ring's next slot.
                        stream.dropped += 1;
                        stream.last_frame = Some(now);
                        if dmabuf.ring.next_is_pinned() && !dmabuf.ring_stalled {
                            dmabuf.ring_stalled = true;
                            log::warn!(
                                "stream {stream_id}: every capture slot is consumer-owned; \
                                 dropping frames until the consumer releases one"
                            );
                        }
                        continue;
                    };
                    let sequence = stream.sequence + 1;
                    let dropped = stream.dropped;
                    match render_window_stream_dmabuf(
                        &self.device,
                        &mut self.renderer,
                        &self.server,
                        dmabuf,
                        &geometry,
                        cursor_draw,
                        &mut self.cursor_cache,
                        scheme,
                    ) {
                        Ok((slot, fence)) => {
                            if dmabuf.ring_stalled {
                                dmabuf.ring_stalled = false;
                                log::info!("stream {stream_id}: capture slots are flowing again");
                            }
                            dmabuf.ring.submitted(slot);
                            dmabuf.pending.push(PendingSlotFrame {
                                slot,
                                fence,
                                _source: None,
                                sequence,
                                dropped,
                                submitted_at: now,
                                security_generation,
                                damage: sampled,
                            });
                            window.generations = live;
                            window.dirty = false;
                            stream.sequence += 1;
                            stream.last_frame = Some(now);
                        }
                        Err(BlitFailure::Retryable(reason)) => {
                            log::warn!("stream {stream_id}: window dmabuf frame failed: {reason}");
                            stream.dropped += 1;
                            stream.last_frame = Some(now);
                        }
                        Err(BlitFailure::Submitted(reason)) => {
                            ended.push((stream_id, reason));
                        }
                    }
                }
            }
        }
        if let Some(ipc) = self.ipc.as_ref() {
            for (stream_id, width, height) in frozen {
                log::info!(
                    "stream {stream_id}: target geometry changed to {width}x{height}; freezing"
                );
                ipc.stream_geometry_changed(stream_id, width, height);
                self.streams.freeze(stream_id);
            }
            for (stream_id, reason) in ended {
                log::info!("stream {stream_id}: {reason}; ending");
                ipc.end_stream(stream_id, &reason);
                self.streams.stop(stream_id);
            }
        }
        self.publish_capture_stream_count();
    }

    /// Hand every completed window-stream SHM readback to the capture
    /// worker for BGRA conversion (ADR-0127). The worker lane is shared
    /// with one-shot captures, which keep priority: while a one-shot
    /// reserves the lane a ready frame waits on its surface (flux keeps
    /// the completed frame mapped), and a frame held that way for too long
    /// logs once instead of starving silently.
    pub(super) fn poll_window_stream_readbacks(&mut self) {
        if self.server.session_locked() || !self.host.is_active() {
            return;
        }
        let now = Instant::now();
        let ids: Vec<u64> = self
            .streams
            .streams
            .iter()
            .filter(|(_, stream)| {
                stream.window.as_ref().is_some_and(|window| {
                    matches!(window.stage, WindowStreamStage::AwaitingReadback { .. })
                })
            })
            .map(|(stream_id, _)| *stream_id)
            .collect();
        for stream_id in ids {
            let Some(stream) = self.streams.streams.get_mut(&stream_id) else {
                continue;
            };
            let window = stream
                .window
                .as_mut()
                .expect("window stream state attached");
            let Some(target) = window.shm.as_ref() else {
                continue;
            };
            let ready = match target.surface.read_pixels_ready() {
                Ok(ready) => ready,
                Err(error) => {
                    log::warn!(
                        "stream {stream_id}: window readback readiness failed: {error}{}",
                        flux_last_error_detail()
                    );
                    window.stage = WindowStreamStage::Idle;
                    stream.dropped += 1;
                    stream.last_frame = Some(now);
                    continue;
                }
            };
            if !ready {
                continue;
            }
            if self.capture_worker.is_busy() {
                let held_since = window.held_since.get_or_insert(now);
                if now.duration_since(*held_since) >= Duration::from_secs(2) {
                    log::warn!(
                        "stream {stream_id}: window frame held behind one-shot captures \
                         for {:?}; the capture worker keeps one-shot priority",
                        now.duration_since(*held_since)
                    );
                    *held_since = now + Duration::from_secs(58);
                }
                continue;
            }
            let WindowStreamStage::AwaitingReadback {
                security_generation,
                damage,
            } = std::mem::replace(&mut window.stage, WindowStreamStage::Idle)
            else {
                continue;
            };
            if !self.capture_worker.permits(security_generation) {
                // The frame was rendered before a lock/VT boundary: drop it
                // without spending a worker conversion (ADR-0127).
                stream.dropped += 1;
                stream.fold_damage_back(damage);
                continue;
            }
            let (width, height) = stream.size;
            let readback = PendingReadback {
                width,
                height,
                crop: None,
                cursor: None,
                security_generation,
            };
            match read_captured_pixels_owned(&target.surface, readback) {
                Ok(capture) => {
                    window.held_since = None;
                    window.stage = WindowStreamStage::Converting { damage };
                    queue_captured_pixels(
                        &self.capture_worker,
                        capture,
                        CaptureTarget::StreamWindow { stream_id },
                        &self.journal,
                        &self.ipc,
                    );
                }
                Err(reason) => {
                    log::warn!("stream {stream_id}: window readback failed: {reason}");
                    stream.dropped += 1;
                    stream.last_frame = Some(now);
                    stream.fold_damage_back(damage);
                }
            }
        }
    }

    /// Deliver one converted window-stream frame arriving from the capture
    /// worker (ADR-0127). Frames that crossed a lock/VT boundary or failed
    /// conversion count as dropped and fold their damage back; a stopped
    /// stream's late completion is discarded.
    pub(super) fn deliver_window_stream_frame(
        &mut self,
        stream_id: u64,
        security_generation: u64,
        pixels: Result<StreamPixels, String>,
    ) {
        let now = Instant::now();
        let damage = match self
            .streams
            .streams
            .get_mut(&stream_id)
            .and_then(|stream| stream.window.as_mut())
        {
            Some(window) => {
                match std::mem::replace(&mut window.stage, WindowStreamStage::Idle) {
                    WindowStreamStage::Converting { damage } => Some(damage),
                    // The stream was re-rendered or stopped meanwhile; this
                    // completion is stale.
                    stage => {
                        window.stage = stage;
                        None
                    }
                }
            }
            None => None,
        };
        let Some(damage) = damage else {
            return;
        };
        let Some(ipc) = self.ipc.as_ref() else {
            return;
        };
        if !self.capture_worker.permits(security_generation) {
            if let Some(stream) = self.streams.streams.get_mut(&stream_id) {
                stream.dropped += 1;
                stream.fold_damage_back(damage);
            }
            return;
        }
        match pixels {
            Ok(frame) => {
                let Some((sequence, dropped)) = self.streams.sequence_and_dropped(stream_id) else {
                    return;
                };
                let Some((_, size)) = self.streams.target_of(stream_id) else {
                    return;
                };
                let payload = tessera_ipc::StreamFramePayload::Pixels(tessera_ipc::StreamPixelFrame {
                    stream_id,
                    sequence,
                    width: frame.width,
                    height: frame.height,
                    stride: frame.width * 4,
                    format: tessera_ipc::StreamPixelFormat::Bgra8,
                    damage: damage_in_target(&damage, size),
                    dropped,
                    pixels: frame.bgra,
                });
                let delivered = ipc.push_stream_frame(payload);
                self.streams.record_frame(stream_id, now, delivered);
                if let Some(stream) = self.streams.streams.get_mut(&stream_id) {
                    if delivered {
                        stream.note_delivered();
                    } else {
                        stream.fold_damage_back(damage);
                    }
                }
            }
            Err(reason) => {
                log::warn!("stream {stream_id}: window frame conversion failed: {reason}");
                if let Some(stream) = self.streams.streams.get_mut(&stream_id) {
                    stream.dropped += 1;
                    stream.fold_damage_back(damage);
                }
            }
        }
    }
}

/// What [`CompositorRuntime::handle_output_geometry_change`] decided for
/// one stream.
#[derive(Debug, Clone, PartialEq, Eq)]
enum GeometryAction {
    Unchanged,
    /// Freeze production and notify with `StreamGeometryChanged(w, h)`.
    Freeze(u32, u32),
    /// End the stream with `StreamEnded(reason)`.
    End(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Start a plain whole-desktop SHM stream with a hidden cursor.
    fn start(streams: &mut OutputStreams, conn: u64, fps: Option<u32>, size: (u32, u32)) -> u64 {
        streams
            .start(
                conn,
                fps,
                size,
                tessera_ipc::StreamTarget::Output { output: None },
                tessera_ipc::StreamCursorMode::Hidden,
            )
            .stream_id
    }

    #[test]
    fn max_fps_defaults_and_clamps() {
        let mut streams = OutputStreams::new();
        let default = start(&mut streams, 1, None, (1920, 1080));
        assert_eq!(
            streams.streams[&default].frame_interval,
            Duration::from_secs(1) / 30
        );
        // 240 is the new negotiated ceiling (IPC protocol 29).
        let high = start(&mut streams, 1, Some(240), (1920, 1080));
        assert_eq!(
            streams.streams[&high].frame_interval,
            Duration::from_secs(1) / 240
        );
        let beyond = start(&mut streams, 1, Some(500), (1920, 1080));
        assert_eq!(
            streams.streams[&beyond].frame_interval,
            Duration::from_secs(1) / 240
        );
        let low = start(&mut streams, 1, Some(0), (1920, 1080));
        assert_eq!(streams.streams[&low].frame_interval, Duration::from_secs(1));
    }

    #[test]
    fn due_streams_respect_the_throttle() {
        let mut streams = OutputStreams::new();
        let fast = start(&mut streams, 1, Some(60), (100, 100));
        let slow = start(&mut streams, 1, Some(1), (100, 100));
        let t0 = Instant::now();
        // Both start due.
        assert_eq!(streams.due_shm_ids(t0), vec![fast, slow]);
        streams.record_frame(fast, t0, true);
        streams.record_frame(slow, t0, true);
        // 20ms later: only the 60fps stream is due again.
        assert_eq!(
            streams.due_shm_ids(t0 + Duration::from_millis(20)),
            vec![fast]
        );
        // 1.1s later: both.
        assert_eq!(
            streams.due_shm_ids(t0 + Duration::from_millis(1100)),
            vec![fast, slow]
        );
    }

    #[test]
    fn first_frame_is_immediately_due_and_forces_a_present() {
        let mut streams = OutputStreams::new();
        let t0 = Instant::now();
        let id = start(&mut streams, 1, Some(30), (100, 100));
        // A just-started stream is forcing-due (its first frame is forced
        // even on a static screen) and wakes the loop immediately.
        assert!(streams.forcing_due_shm(t0));
        assert_eq!(streams.next_stream_wake_in(t0), Some(Duration::ZERO));
        // Once framed, it forces again as soon as its max-fps interval
        // elapses — the stream paces the loop at its negotiated cadence.
        streams.record_frame(id, t0, true);
        let interval = Duration::from_secs(1) / 30;
        assert!(!streams.forcing_due_shm(t0 + Duration::from_millis(10)));
        assert!(streams.forcing_due_shm(t0 + interval));
    }

    #[test]
    fn max_fps_due_ness_forces_a_present_and_paces_the_loop() {
        let mut streams = OutputStreams::new();
        let t0 = Instant::now();
        let fast = start(&mut streams, 1, Some(60), (100, 100));
        streams.record_frame(fast, t0, true);
        // 20ms later the 60fps stream is due and may force a frame, even on
        // a static screen (ADR-0130).
        let t1 = t0 + Duration::from_millis(20);
        assert_eq!(streams.due_shm_ids(t1), vec![fast]);
        assert!(streams.forcing_due_shm(t1));
        // The loop wakes at the fps deadline, not a liveness tick.
        let wait = streams
            .next_stream_wake_in(t0 + Duration::from_millis(10))
            .expect("stream live");
        let interval = Duration::from_secs(1) / 60;
        assert!(
            wait <= interval && wait > Duration::ZERO,
            "pacing wait: {wait:?}"
        );
        assert_eq!(
            streams.next_stream_wake_in(t0 + interval),
            Some(Duration::ZERO)
        );
    }

    #[test]
    fn frozen_streams_are_neither_due_nor_forcing_due() {
        let mut streams = OutputStreams::new();
        let t0 = Instant::now();
        let id = start(&mut streams, 1, Some(60), (100, 100));
        streams.freeze(id);
        // A frozen stream produces nothing and never wakes the loop.
        assert!(streams.due_shm_ids(t0).is_empty());
        assert!(!streams.forcing_due_shm(t0));
        assert!(!streams.any_output_live());
        assert_eq!(streams.next_stream_wake_in(t0), None);
        // A second, live stream is unaffected.
        let live = start(&mut streams, 2, Some(60), (100, 100));
        assert_eq!(streams.due_shm_ids(t0), vec![live]);
        assert!(streams.any_output_live());
        // Restart works: stop the frozen stream and start fresh at the new
        // geometry; the new stream is due immediately again.
        streams.stop(id);
        let restarted = start(&mut streams, 1, Some(60), (2560, 1440));
        assert!(streams.forcing_due_shm(t0));
        assert_eq!(
            streams.target_of(restarted).map(|(_, size)| size),
            Some((2560, 1440))
        );
    }

    #[test]
    fn next_stream_wake_paces_the_loop_at_the_soonest_due_stream() {
        let mut streams = OutputStreams::new();
        // No streams: no stream-driven wakeup.
        assert_eq!(streams.next_stream_wake_in(Instant::now()), None);

        // A stream that never received a frame is due immediately.
        let fast = start(&mut streams, 1, Some(60), (100, 100));
        assert_eq!(
            streams.next_stream_wake_in(Instant::now()),
            Some(Duration::ZERO)
        );

        // After frames, the wait is the remaining frame interval; a
        // second stream framed later pushes its own deadline out but the
        // soonest one wins.
        let t0 = Instant::now();
        streams.record_frame(fast, t0, true);
        let slow = start(&mut streams, 2, Some(1), (100, 100));
        streams.record_frame(slow, t0 + Duration::from_millis(100), true);
        let wait = streams
            .next_stream_wake_in(t0 + Duration::from_millis(10))
            .expect("streams live");
        let fast_interval = Duration::from_secs(1) / 60;
        assert!(
            wait <= fast_interval && wait > Duration::from_millis(3),
            "fast stream is due at +16.7ms: {wait:?}"
        );
        assert_eq!(
            streams.next_stream_wake_in(t0 + fast_interval),
            Some(Duration::ZERO),
            "the fast stream reached its frame deadline"
        );

        // Stopping every stream removes the stream-driven wakeup.
        streams.stop(fast);
        streams.stop(slow);
        assert_eq!(streams.next_stream_wake_in(Instant::now()), None);
    }

    #[test]
    fn sequence_and_dropped_track_delivery() {
        let mut streams = OutputStreams::new();
        let id = start(&mut streams, 1, Some(30), (100, 100));
        let now = Instant::now();
        assert_eq!(streams.sequence_and_dropped(id), Some((1, 0)));
        streams.record_frame(id, now, true);
        streams.record_frame(id, now, false);
        assert_eq!(streams.sequence_and_dropped(id), Some((2, 1)));
        streams.record_frame(id, now, true);
        assert_eq!(streams.sequence_and_dropped(id), Some((3, 1)));
    }

    #[test]
    fn window_stream_remembers_target_and_start_size() {
        let mut streams = OutputStreams::new();
        let target = tessera_ipc::StreamTarget::Window {
            window: tessera_model::window::WindowId(9),
        };
        let id = streams
            .start(
                1,
                None,
                (640, 480),
                target.clone(),
                tessera_ipc::StreamCursorMode::Hidden,
            )
            .stream_id;
        assert_eq!(streams.target_of(id), Some((target, (640, 480))));
        assert_eq!(streams.target_of(999), None);
    }

    #[test]
    fn cursor_mode_is_stored_on_the_stream_state() {
        let mut streams = OutputStreams::new();
        let id = streams
            .start(
                1,
                None,
                (100, 100),
                tessera_ipc::StreamTarget::Output { output: None },
                tessera_ipc::StreamCursorMode::Embedded,
            )
            .stream_id;
        assert_eq!(
            streams.streams[&id].cursor,
            tessera_ipc::StreamCursorMode::Embedded
        );
    }

    #[test]
    fn resolve_output_rect_maps_and_clamps_by_connector() {
        let outputs = vec![
            tessera_model::output::OutputInfo {
                connector: "HDMI-A-1".into(),
                geometry: tessera_model::output::OutputGeometry {
                    mode: tessera_model::output::OutputMode {
                        width: 1920,
                        height: 1080,
                        refresh_mhz: 60_000,
                    },
                    scale: tessera_model::output::Scale(1.0),
                    transform: tessera_model::Transform::Normal,
                    logical_origin: tessera_model::Point { x: 0, y: 0 },
                },
                available_modes: Vec::new(),
                color_caps: tessera_model::edid::EdidColorCapabilities::default(),
            },
            tessera_model::output::OutputInfo {
                connector: "DP-1".into(),
                geometry: tessera_model::output::OutputGeometry {
                    mode: tessera_model::output::OutputMode {
                        width: 2560,
                        height: 1440,
                        refresh_mhz: 60_000,
                    },
                    scale: tessera_model::output::Scale(1.0),
                    transform: tessera_model::Transform::Normal,
                    logical_origin: tessera_model::Point { x: 1920, y: 0 },
                },
                available_modes: Vec::new(),
                color_caps: tessera_model::edid::EdidColorCapabilities::default(),
            },
        ];
        assert_eq!(
            resolve_output_rect(&outputs, "DP-1", 1.0, 4480, 1440),
            Some(tessera_model::Rect::new(1920, 0, 2560, 1440))
        );
        assert_eq!(
            resolve_output_rect(&outputs, "HDMI-A-1", 1.0, 4480, 1440),
            Some(tessera_model::Rect::new(0, 0, 1920, 1080))
        );
        // Render scale 2 maps the logical layout onto the physical frame.
        assert_eq!(
            resolve_output_rect(&outputs, "DP-1", 2.0, 8960, 2880),
            Some(tessera_model::Rect::new(3840, 0, 5120, 2880))
        );
        // Unknown connectors resolve to None (an error at stream start).
        assert_eq!(
            resolve_output_rect(&outputs, "USB-C-1", 1.0, 4480, 1440),
            None
        );
    }

    #[test]
    fn crop_stream_frame_extracts_rows() {
        // 4x2 frame, pixels valued by their index.
        let bgra: Vec<u8> = (0u8..32).collect();
        let (w, h, pixels) = crop_stream_frame(4, &bgra, tessera_model::Rect::new(1, 1, 2, 1));
        assert_eq!((w, h), (2, 1));
        // Row 1 starts at byte 16; two pixels from x=1: bytes 20..28.
        assert_eq!(&pixels[..], &(20u8..28).collect::<Vec<u8>>()[..]);
    }

    #[test]
    fn stop_and_disconnect_remove_stream_state() {
        let mut streams = OutputStreams::new();
        let a = start(&mut streams, 1, None, (100, 100));
        let b = start(&mut streams, 2, None, (100, 100));
        let c = start(&mut streams, 2, None, (100, 100));
        streams.stop(a);
        assert!(!streams.streams.contains_key(&a));
        streams.disconnect(2);
        assert!(streams.streams.is_empty());
        assert!(!streams.streams.contains_key(&b));
        assert!(!streams.streams.contains_key(&c));
    }

    #[test]
    fn live_stream_count_tracks_start_stop_and_disconnect() {
        // The recording indicator's state source (ADR-0128): every registry
        // mutation moves the count the shell mirrors.
        let mut streams = OutputStreams::new();
        assert_eq!(streams.len(), 0);
        let a = start(&mut streams, 1, None, (100, 100));
        let b = start(&mut streams, 1, None, (100, 100));
        start(&mut streams, 2, None, (100, 100));
        assert_eq!(streams.len(), 3);
        streams.stop(a);
        assert_eq!(streams.len(), 2);
        streams.stop(b);
        assert_eq!(streams.len(), 1);
        streams.disconnect(2);
        assert_eq!(streams.len(), 0);
    }

    /// A window stream in registry-only form (no GPU target): the state the
    /// pacing and wakeup tests exercise.
    fn attach_stub_window(
        streams: &mut OutputStreams,
        stream_id: u64,
        stage: WindowStreamStage,
        dirty: bool,
    ) {
        streams.attach_window(
            stream_id,
            WindowStream {
                shm: None,
                geometry_sig: (0, 0),
                origin: tessera_model::Point { x: 0, y: 0 },
                logical_size: tessera_model::Size { w: 100, h: 50 },
                scale_milli: 1000,
                generations: std::collections::HashMap::new(),
                dirty,
                stage,
                held_since: None,
            },
        );
    }

    #[test]
    fn window_streams_never_join_the_presentation_driven_lanes() {
        let mut streams = OutputStreams::new();
        let t0 = Instant::now();
        let id = start(&mut streams, 1, Some(60), (100, 50));
        attach_stub_window(&mut streams, id, WindowStreamStage::Idle, true);
        // Window streams render independently of presentation: they never
        // appear in the shared-readback or presentation-forcing sets.
        assert!(streams.due_shm_ids(t0).is_empty());
        assert!(streams.due_dmabuf_ids(t0).is_empty());
        assert!(!streams.forcing_due_shm(t0));
        assert!(!streams.forcing_due_dmabuf(t0));
        // ... but they still wake the loop: a dirty window stream with no
        // frame yet renders immediately.
        assert_eq!(streams.next_stream_wake_in(t0), Some(Duration::ZERO));
    }

    #[test]
    fn window_stream_wake_follows_pacing_state() {
        let mut streams = OutputStreams::new();
        let t0 = Instant::now();
        let id = start(&mut streams, 1, Some(60), (100, 50));
        attach_stub_window(&mut streams, id, WindowStreamStage::Idle, true);
        streams.record_frame(id, t0, true);
        // Dirty and idle: the wake is the fps deadline, not the liveness tick.
        let wait = streams
            .next_stream_wake_in(t0 + Duration::from_millis(5))
            .expect("stream live");
        assert!(
            wait <= Duration::from_millis(12) && wait > Duration::from_millis(5),
            "60fps deadline: {wait:?}"
        );
        // Clean tree: only the liveness tick remains.
        streams
            .streams
            .get_mut(&id)
            .unwrap()
            .window
            .as_mut()
            .unwrap()
            .dirty = false;
        let wait = streams
            .next_stream_wake_in(t0 + Duration::from_millis(5))
            .expect("stream live");
        assert!(
            wait > Duration::from_millis(900) && wait <= LIVENESS_INTERVAL,
            "liveness deadline: {wait:?}"
        );
        // Readback in flight: poll shortly regardless of pacing.
        streams
            .streams
            .get_mut(&id)
            .unwrap()
            .window
            .as_mut()
            .unwrap()
            .stage = WindowStreamStage::AwaitingReadback {
            security_generation: 1,
            damage: SampledDamage {
                origin: tessera_model::Point { x: 0, y: 0 },
                damage: FrameDamage::Full,
            },
        };
        let wait = streams
            .next_stream_wake_in(t0 + Duration::from_millis(5))
            .expect("stream live");
        assert!(wait <= Duration::from_millis(1), "readback poll: {wait:?}");
    }

    #[test]
    fn window_stream_render_due_paces_dirty_and_liveness() {
        let t0 = Instant::now();
        let interval = Duration::from_millis(16);
        // Never framed: due immediately (first frame is forced).
        assert!(window_stream_render_due(false, true, None, interval, t0));
        // Dirty + fps interval elapsed: render.
        assert!(window_stream_render_due(
            true,
            true,
            Some(t0),
            interval,
            t0 + interval
        ));
        // Dirty but inside the interval: wait.
        assert!(!window_stream_render_due(
            true,
            true,
            Some(t0),
            interval,
            t0 + Duration::from_millis(8)
        ));
        // Clean and inside the liveness tick: no render.
        assert!(!window_stream_render_due(
            false,
            true,
            Some(t0),
            interval,
            t0 + Duration::from_millis(500)
        ));
        // Clean but a full liveness tick gone: re-render (keeps minimized
        // thumbnails honest and the consumer fed).
        assert!(window_stream_render_due(
            false,
            true,
            Some(t0),
            interval,
            t0 + LIVENESS_INTERVAL
        ));
        // A frame in flight blocks everything, liveness included.
        assert!(!window_stream_render_due(true, false, None, interval, t0));
    }

    #[test]
    fn window_stream_cursor_clips_to_the_window_rect() {
        let state = |position| CaptureCursorState {
            position,
            shape: 1,
            hidden: false,
            client_surface: false,
        };
        let origin = tessera_model::Point { x: 100, y: 50 };
        let size = tessera_model::Size { w: 200, h: 100 };
        // Inside: the draw position is window-relative.
        let drawn = window_stream_cursor(&state((150.0, 80.0)), origin, size)
            .expect("cursor inside the window");
        assert_eq!(drawn, (1, (50.0, 30.0)));
        // On the far edge (exclusive): outside.
        assert!(window_stream_cursor(&state((300.0, 80.0)), origin, size).is_none());
        // Outside on every other side.
        assert!(window_stream_cursor(&state((99.0, 80.0)), origin, size).is_none());
        assert!(window_stream_cursor(&state((150.0, 49.0)), origin, size).is_none());
        assert!(window_stream_cursor(&state((150.0, 150.0)), origin, size).is_none());
        // Exactly on the near edge: inside.
        assert!(window_stream_cursor(&state((100.0, 50.0)), origin, size).is_some());
    }

    #[test]
    fn damage_translates_and_clips_into_target_space() {
        let sampled = SampledDamage {
            origin: tessera_model::Point { x: 100, y: 50 },
            damage: FrameDamage::Area(vec![
                tessera_model::Rect::new(110, 60, 20, 10),
                // Fully outside the target: clipped away.
                tessera_model::Rect::new(500, 500, 20, 20),
            ]),
        };
        assert_eq!(
            damage_in_target(&sampled, (200, 100)),
            vec![tessera_model::Rect::new(10, 10, 20, 10)]
        );
        // Partially overlapping damage clips to the target extent.
        let sampled = SampledDamage {
            origin: tessera_model::Point { x: 100, y: 50 },
            damage: FrameDamage::Area(vec![tessera_model::Rect::new(90, 40, 20, 20)]),
        };
        assert_eq!(
            damage_in_target(&sampled, (200, 100)),
            vec![tessera_model::Rect::new(0, 0, 10, 10)]
        );
        // Damage that never intersected the target reports the full rect
        // (the wire contract never carries an empty list).
        let sampled = SampledDamage {
            origin: tessera_model::Point { x: 0, y: 0 },
            damage: FrameDamage::Area(vec![tessera_model::Rect::new(900, 900, 10, 10)]),
        };
        assert_eq!(
            damage_in_target(&sampled, (200, 100)),
            vec![tessera_model::Rect::new(0, 0, 200, 100)]
        );
        // Full and no accumulated damage both stay conservative.
        for damage in [FrameDamage::Full, FrameDamage::None] {
            let sampled = SampledDamage {
                origin: tessera_model::Point { x: 0, y: 0 },
                damage,
            };
            assert_eq!(
                damage_in_target(&sampled, (200, 100)),
                vec![tessera_model::Rect::new(0, 0, 200, 100)]
            );
        }
    }

    #[test]
    fn damage_accumulates_until_delivery_and_survives_drops() {
        let mut streams = OutputStreams::new();
        let id = start(&mut streams, 1, Some(30), (100, 100));
        let origin = tessera_model::Point { x: 0, y: 0 };
        // The first sample of a fresh stream is full (and the origin guard
        // would force that anyway).
        let sampled = streams.sample_damage(id, origin);
        assert!(matches!(sampled.damage, FrameDamage::Full));
        streams.streams.get_mut(&id).unwrap().note_delivered();

        // Two presented frames accumulate; the sample carries both rects.
        streams.accumulate_damage(&FrameDamage::Area(vec![tessera_model::Rect::new(1, 2, 3, 4)]));
        streams.accumulate_damage(&FrameDamage::Area(vec![tessera_model::Rect::new(5, 6, 7, 8)]));
        let sampled = streams.sample_damage(id, origin);
        assert_eq!(
            sampled.damage,
            FrameDamage::Area(vec![
                tessera_model::Rect::new(1, 2, 3, 4),
                tessera_model::Rect::new(5, 6, 7, 8),
            ])
        );
        // Sampling is a clone: a second sample (a retried capture) sees the
        // same accumulation; only delivery clears it.
        let again = streams.sample_damage(id, origin);
        assert_eq!(again.damage, sampled.damage);
        streams.streams.get_mut(&id).unwrap().note_delivered();
        let after = streams.sample_damage(id, origin);
        assert!(matches!(after.damage, FrameDamage::None));

        // A dropped frame folds its damage back: nothing is lost.
        streams.accumulate_damage(&FrameDamage::Area(vec![tessera_model::Rect::new(9, 9, 1, 1)]));
        let sampled = streams.sample_damage(id, origin);
        streams.streams.get_mut(&id).unwrap().note_delivered();
        streams
            .streams
            .get_mut(&id)
            .unwrap()
            .fold_damage_back(sampled);
        let reaccumulated = streams.sample_damage(id, origin);
        assert_eq!(
            reaccumulated.damage,
            FrameDamage::Area(vec![tessera_model::Rect::new(9, 9, 1, 1)])
        );
    }

    #[test]
    fn damage_sample_reports_full_after_an_origin_move() {
        let mut streams = OutputStreams::new();
        let id = start(&mut streams, 1, Some(30), (100, 100));
        streams.accumulate_damage(&FrameDamage::Area(vec![tessera_model::Rect::new(1, 1, 5, 5)]));
        let moved = streams.sample_damage(id, tessera_model::Point { x: 50, y: 50 });
        // The origin moved (stream starts with no recorded origin): the old
        // accumulation belongs to another coordinate space.
        assert!(matches!(moved.damage, FrameDamage::Full));
        // Same origin next time: back to the accumulated (still full, the
        // accumulator was never cleared).
        streams.streams.get_mut(&id).unwrap().note_delivered();
        streams.accumulate_damage(&FrameDamage::Area(vec![tessera_model::Rect::new(2, 2, 5, 5)]));
        let steady = streams.sample_damage(id, tessera_model::Point { x: 50, y: 50 });
        assert_eq!(
            steady.damage,
            FrameDamage::Area(vec![tessera_model::Rect::new(2, 2, 5, 5)])
        );
    }

    #[test]
    fn any_shm_embedded_tracks_output_shm_streams_only() {
        let mut streams = OutputStreams::new();
        assert!(!streams.any_shm_embedded());
        let hidden = start(&mut streams, 1, None, (100, 100));
        assert!(!streams.any_shm_embedded());
        let embedded = streams
            .start(
                1,
                None,
                (100, 100),
                tessera_ipc::StreamTarget::Output { output: None },
                tessera_ipc::StreamCursorMode::Embedded,
            )
            .stream_id;
        assert!(streams.any_shm_embedded());
        // A frozen embedded stream does not trigger cursor blending.
        streams.freeze(embedded);
        assert!(!streams.any_shm_embedded());
        // A window stream with the embedded mode is irrelevant here (it
        // draws its cursor on the GPU).
        streams.stop(hidden);
        streams.stop(embedded);
        let window = start(&mut streams, 1, None, (100, 100));
        streams.streams.get_mut(&window).unwrap().cursor = tessera_ipc::StreamCursorMode::Embedded;
        attach_stub_window(&mut streams, window, WindowStreamStage::Idle, false);
        assert!(!streams.any_shm_embedded());
    }

    #[test]
    fn forcing_due_shm_paces_and_drives_presentation_without_client_damage() {
        let mut streams = OutputStreams::new();
        let id = start(&mut streams, 1, Some(60), (1920, 1080));
        let t0 = Instant::now();

        // 1. A freshly started SHM stream is due immediately (first frame forced).
        assert!(streams.forcing_due_shm(t0));
        assert!(streams.any_output_live());

        // Record framed at t0.
        streams.record_frame(id, t0, true);

        // 2. Mid-interval (e.g. 5ms after t0 for a 60fps stream ~16.6ms): not due yet.
        assert!(!streams.forcing_due_shm(t0 + Duration::from_millis(5)));
        let wait = streams
            .next_stream_wake_in(t0 + Duration::from_millis(5))
            .unwrap();
        assert!(wait <= Duration::from_millis(12));

        // 3. Once interval has passed (17ms): forcing is due again.
        assert!(streams.forcing_due_shm(t0 + Duration::from_millis(17)));

        // 4. Record next frame at t0 + 17ms: resets pacing.
        streams.record_frame(id, t0 + Duration::from_millis(17), true);
        assert!(!streams.forcing_due_shm(t0 + Duration::from_millis(20)));
        assert!(streams.forcing_due_shm(t0 + Duration::from_millis(35)));
    }
}

#[cfg(test)]
mod dmabuf_tests {
    use super::*;

    #[test]
    fn slot_ring_tracks_consumer_ownership() {
        let mut ring = SlotRing::new(3);
        assert_eq!(ring.next_submission_slot(), Some(0));
        ring.submitted(0);
        assert_eq!(ring.next_submission_slot(), Some(1));
        ring.submitted(1);
        ring.submitted(2);
        // The ring wrapped; slot 0's frame is still rendering.
        assert_eq!(ring.next_submission_slot(), None);

        // A delivered frame pins its slot: a due frame drops rather than
        // overwriting consumer-owned content.
        ring.fence_signaled(0, true);
        assert_eq!(ring.next_submission_slot(), None);
        // A delivery failure frees its slot but the ring position stays.
        ring.fence_signaled(1, false);
        assert_eq!(ring.next_submission_slot(), None);

        // The consumer's release unblocks the ring.
        ring.release(0);
        assert_eq!(ring.next_submission_slot(), Some(0));
        ring.submitted(0);
        assert_eq!(ring.next_submission_slot(), Some(1));
    }

    #[test]
    fn release_only_frees_a_pinned_slot() {
        let mut ring = SlotRing::new(3);
        ring.submitted(0);
        // Still rendering: a release does not apply.
        ring.release(0);
        assert_eq!(ring.states[0], SlotState::Rendering);
        // Out-of-range slots are ignored.
        ring.release(9);
        ring.fence_signaled(0, true);
        ring.release(0);
        assert_eq!(ring.states[0], SlotState::Free);
    }

    #[test]
    fn timed_out_slot_is_recycled_without_a_release() {
        let mut ring = SlotRing::new(2);
        ring.submitted(0);
        ring.submitted(1);
        // Slot 0's fence "timed out" before its delivery: recycled.
        ring.recycle(0);
        assert_eq!(ring.states[0], SlotState::Free);
        assert_eq!(ring.next_submission_slot(), Some(0));
        // A pinned slot is never recycled: the consumer owns it.
        ring.submitted(0);
        ring.fence_signaled(0, true);
        ring.recycle(0);
        assert_eq!(ring.states[0], SlotState::Pinned);
    }

    #[test]
    fn release_slot_ignores_shm_and_unknown_streams() {
        let mut streams = OutputStreams::new();
        let id = streams
            .start(
                1,
                None,
                (100, 100),
                tessera_ipc::StreamTarget::Output { output: None },
                tessera_ipc::StreamCursorMode::Hidden,
            )
            .stream_id;
        // No dmabuf transport anywhere: releases are inert no-ops.
        streams.release_slot(id, 0);
        streams.release_slot(999, 0);
    }

    #[test]
    fn due_ids_split_by_transport() {
        let mut streams = OutputStreams::new();
        let shm = streams
            .start(
                1,
                None,
                (100, 100),
                tessera_ipc::StreamTarget::Output { output: None },
                tessera_ipc::StreamCursorMode::Hidden,
            )
            .stream_id;
        let now = Instant::now();
        assert_eq!(streams.due_shm_ids(now), vec![shm]);
        assert!(streams.due_dmabuf_ids(now).is_empty());
    }

    /// End-to-end capture-surface exercise: slot enumeration order, the
    /// explicit acquire fence of an exported frame, and slot reuse. Skipped
    /// without a dma-buf-capable Vulkan device on this machine.
    #[test]
    fn dmabuf_capture_surface_enumerates_and_exports_slots() {
        let Ok(device) = flux::Device::new_with_options(flux::DeviceOptions {
            headless: true,
            frames_in_flight: STREAM_SLOT_COUNT as u32,
            required_features: flux::DeviceFeatures::DMABUF,
            optional_features: flux::DeviceFeatures::DMABUF_SYNC_FILE,
            ..flux::DeviceOptions::default()
        }) else {
            return;
        };
        const LINEAR: u64 = 0; // DRM_FORMAT_MOD_LINEAR
        let Ok(surface) = flux::Surface::offscreen_dmabuf(&device, 64, 48, &[LINEAR]) else {
            return;
        };
        let canvas = flux::Canvas::new(&surface).unwrap();
        let mut exports = Vec::new();
        for expected_slot in 0..STREAM_SLOT_COUNT {
            let frame = surface.begin_frame().unwrap();
            begin_opaque_frame(&canvas, &frame, flux::rgba(0, 0, 0, 255)).unwrap();
            canvas.end_frame_checked().unwrap();
            frame.submit().unwrap().present().unwrap();
            let export = surface.export_dmabuf().unwrap();
            assert_eq!(export.slot as usize, expected_slot);
            assert_eq!((export.width, export.height), (64, 48));
            assert!(export.stride >= 64 * 4);
            exports.push(export);
        }
        // Every slot exported a distinct live descriptor.
        let raw: std::collections::HashSet<_> =
            exports.iter().map(|export| export.fd.as_raw_fd()).collect();
        assert_eq!(raw.len(), STREAM_SLOT_COUNT);

        // A submitted frame exports with a pollable acquire fence, and the
        // ring wrapped back to slot 0.
        let frame = surface.begin_frame().unwrap();
        begin_opaque_frame(&canvas, &frame, flux::rgba(10, 20, 30, 255)).unwrap();
        canvas.end_frame_checked().unwrap();
        frame.submit().unwrap().present().unwrap();
        let export = surface.export_dmabuf_explicit().unwrap();
        assert_eq!(export.slot, 0);
        let fence = export
            .acquire_fence
            .expect("explicit export carries a fence");
        let mut pollfd = libc::pollfd {
            fd: fence.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `pollfd` references a live descriptor for the call.
        let result = unsafe { libc::poll(&mut pollfd, 1, 2000) };
        assert!(result > 0, "capture slot fence did not signal");
    }

    /// End-to-end window-stream render exercise (ADR-0127): the SHM target
    /// renders and reads back a uniform frame, and the dmabuf path shares
    /// the blit's submit/export tail (slot 0 with a signaling fence). The
    /// scene carries no window, so the tree draw is the clear alone — this
    /// exercises the pipeline (surface, canvas, pass, submit, readback /
    /// export), not window content. Skipped without a Vulkan device or a
    /// Wayland runtime dir on this machine.
    #[test]
    fn window_stream_render_pipeline_smoke() {
        let Ok(device) = flux::Device::new_with_options(flux::DeviceOptions {
            headless: true,
            frames_in_flight: STREAM_SLOT_COUNT as u32,
            required_features: flux::DeviceFeatures::DMABUF,
            optional_features: flux::DeviceFeatures::DMABUF_SYNC_FILE,
            ..flux::DeviceOptions::default()
        }) else {
            return;
        };
        if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
            return;
        }
        let Ok(server) = tessera_compositor::Server::new() else {
            return;
        };
        let mut renderer = tessera_render::Renderer::new();
        let mut cursor_cache = cursor::CursorCache::default();
        let scheme = tessera_model::settings::ColorScheme::Dark;
        let geometry = WindowTreeGeometry {
            window: tessera_model::window::WindowId(1),
            scale_milli: 1000,
            physical_width: 64,
            physical_height: 48,
            origin: tessera_model::Point { x: 0, y: 0 },
            logical_size: tessera_model::Size { w: 64, h: 48 },
        };

        // SHM: render into the cached readback target and read it back.
        let target = WindowShmTarget::new(&device, 64, 48).expect("shm target");
        render_window_stream_shm(
            &device,
            &mut renderer,
            &server,
            &target,
            &geometry,
            None,
            &mut cursor_cache,
            scheme,
        )
        .expect("window shm render");
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if target.surface.read_pixels_ready().unwrap_or(false) {
                break;
            }
            assert!(Instant::now() < deadline, "window readback never completed");
            std::thread::sleep(Duration::from_millis(5));
        }
        let capture = read_captured_pixels_owned(
            &target.surface,
            PendingReadback {
                width: 64,
                height: 48,
                crop: None,
                cursor: None,
                security_generation: 1,
            },
        )
        .expect("read window pixels");
        let pixels = stream_pixels(capture).expect("convert window pixels");
        assert_eq!((pixels.width, pixels.height), (64, 48));
        assert_eq!(pixels.bgra.len(), 64 * 48 * 4);
        // The empty tree renders the opaque clear: one flat color modulo
        // the renderer's ordered dither (±1–2 per channel), alpha always 255.
        let first = &pixels.bgra[0..4];
        assert!(
            pixels.bgra.chunks_exact(4).all(|px| {
                px[3] == 255 && (0..3).all(|channel| px[channel].abs_diff(first[channel]) <= 2)
            }),
            "empty window tree should render a flat opaque clear, first pixel {first:?}"
        );

        // dmabuf: the window render shares the blit's submit/export tail.
        const LINEAR: u64 = 0; // DRM_FORMAT_MOD_LINEAR
        let Ok(surface) = flux::Surface::offscreen_dmabuf(&device, 64, 48, &[LINEAR]) else {
            return;
        };
        let canvas = flux::Canvas::new(&surface).unwrap();
        let mut dmabuf = DmabufStream {
            surface,
            canvas,
            modifier: LINEAR,
            slot_stride: 0,
            slot_bytes: 0,
            ring: SlotRing::new(STREAM_SLOT_COUNT),
            pending: Vec::new(),
            ring_stalled: false,
        };
        let (slot, fence) = render_window_stream_dmabuf(
            &device,
            &mut renderer,
            &server,
            &mut dmabuf,
            &geometry,
            None,
            &mut cursor_cache,
            scheme,
        )
        .expect("window dmabuf render");
        assert_eq!(slot, 0, "the first window frame lands in slot 0");
        let mut pollfd = libc::pollfd {
            fd: fence.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `pollfd` references a live descriptor for the call.
        let result = unsafe { libc::poll(&mut pollfd, 1, 2000) };
        assert!(result > 0, "window capture slot fence did not signal");
    }
}
