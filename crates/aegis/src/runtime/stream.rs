//! Continuous output frame streaming (ADR-0052).
//!
//! The registry lives on the compositor main loop. A stream is registered by
//! an IPC `StreamOutputStart` (authorization happens in `aegis-ipc` before the
//! request reaches this loop), is throttled to its `max_fps`, and fans one
//! shared GPU readback out to every due SHM stream. Delivery goes through
//! `aegis_ipc::Server::push_stream_frame`, whose bounded lane reports drops
//! back so the stream's cumulative `dropped` counter stays accurate.
//!
//! A client that explicitly opts in (IPC protocol 25) gets the zero-copy
//! dmabuf transport instead: a per-stream exportable capture surface receives
//! a GPU copy of each presented frame, the client learns the fixed slot ring
//! once at start, and frame events reference a slot without a pixel blob. A
//! delivered slot stays consumer-owned — pinned — until the client's
//! `StreamBufferRelease`; only a free slot may be rendered into again.

use std::collections::BTreeMap;
use std::os::fd::{AsRawFd, OwnedFd};
use std::time::{Duration, Instant};

use super::*;

/// Default frame-rate cap when the client leaves `max_fps` unset.
const DEFAULT_MAX_FPS: u32 = 30;
/// Hard bounds on the negotiated frame-rate cap.
const MIN_MAX_FPS: u32 = 1;
const MAX_MAX_FPS: u32 = 60;

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
        target: aegis_ipc::StreamTarget,
        /// The client's explicit zero-copy opt-in (IPC protocol 25).
        allow_dmabuf: bool,
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::StreamInfo, String>>,
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

    /// The consumer finished reading a pinned slot (`StreamBufferRelease`).
    fn release(&mut self, slot: u32) {
        if let Some(state) = self.states.get_mut(slot as usize)
            && *state == SlotState::Pinned
        {
            *state = SlotState::Free;
        }
    }
}

/// A frame rendered into a capture-surface slot whose acquire fence has not
/// signaled yet. The imported source image is held until the fence fires —
/// the slot's GPU work may sample it — and retired with the entry's drop.
struct PendingSlotFrame {
    slot: usize,
    fence: OwnedFd,
    _source: flux::Image,
    sequence: u64,
    dropped: u64,
    submitted_at: Instant,
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
}

/// Everything a new dmabuf stream needs, built at start time: the capture
/// surface with its canvas, the announced modifier, and the slot table the
/// IPC layer transfers to the client.
pub(super) struct DmabufCapture {
    surface: flux::Surface,
    canvas: flux::Canvas,
    modifier: u64,
    table: aegis_ipc::StreamSlotTable,
}

struct OutputStream {
    conn_id: u64,
    frame_interval: Duration,
    last_frame: Option<Instant>,
    sequence: u64,
    dropped: u64,
    /// What the stream crops from each output frame (ADR-0054).
    target: aegis_ipc::StreamTarget,
    /// Physical size at start. A window stream whose live size differs ends:
    /// consumers negotiate one fixed video size.
    size: (u32, u32),
    /// Zero-copy transport state (IPC protocol 25); `None` for SHM streams.
    dmabuf: Option<DmabufStream>,
}

/// The live output streams, keyed by stream id.
pub(super) struct OutputStreams {
    next_id: u64,
    streams: BTreeMap<u64, OutputStream>,
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
        target: aegis_ipc::StreamTarget,
    ) -> aegis_ipc::StreamInfo {
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
                size,
                dmabuf: None,
            },
        );
        aegis_ipc::StreamInfo {
            stream_id,
            width: size.0,
            height: size.1,
            format: aegis_ipc::StreamPixelFormat::Bgra8,
            slots: None,
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
        capture: DmabufCapture,
    ) -> aegis_ipc::StreamInfo {
        let slot_count = capture.table.fds.len();
        let slot_stride = capture.table.stride;
        let slot_bytes = capture.table.byte_len;
        let modifier = capture.modifier;
        let mut info = self.start(
            conn_id,
            max_fps,
            size,
            aegis_ipc::StreamTarget::Output,
        );
        if let Some(stream) = self.streams.get_mut(&info.stream_id) {
            stream.dmabuf = Some(DmabufStream {
                surface: capture.surface,
                canvas: capture.canvas,
                modifier,
                slot_stride,
                slot_bytes,
                ring: SlotRing::new(slot_count),
                pending: Vec::new(),
            });
        }
        info.format = aegis_ipc::StreamPixelFormat::Dmabuf {
            drm_format: DRM_FORMAT_XRGB8888,
            modifier,
        };
        info.slots = Some(capture.table);
        info
    }

    pub(super) fn stop(&mut self, stream_id: u64) {
        self.streams.remove(&stream_id);
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

    /// Ids of streams due a frame at `now`, filtered by transport. A stream
    /// that never received a frame is due immediately.
    fn due_ids_by_transport(&self, now: Instant, dmabuf: bool) -> Vec<u64> {
        self.streams
            .iter()
            .filter(|(_, stream)| stream.dmabuf.is_some() == dmabuf)
            .filter(|(_, stream)| {
                stream
                    .last_frame
                    .is_none_or(|last| now.duration_since(last) >= stream.frame_interval)
            })
            .map(|(id, _)| *id)
            .collect()
    }

    /// Ids of due SHM streams (the shared-readback fan-out).
    pub(super) fn due_shm_ids(&self, now: Instant) -> Vec<u64> {
        self.due_ids_by_transport(now, false)
    }

    /// Ids of due dmabuf streams (the post-present slot fan-out).
    pub(super) fn due_dmabuf_ids(&self, now: Instant) -> Vec<u64> {
        self.due_ids_by_transport(now, true)
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
    ) -> Option<(aegis_ipc::StreamTarget, (u32, u32))> {
        self.streams
            .get(&stream_id)
            .map(|stream| (stream.target, stream.size))
    }

    /// Remove every stream, returning the removed ids.
    pub(super) fn stop_all(&mut self) -> Vec<u64> {
        std::mem::take(&mut self.streams).into_keys().collect()
    }
}

/// The scale the output frame renders at: the primary output's geometry
/// (backend + `[[output]]` overrides), falling back to the host's own scale
/// (nested). Mirrors the presentation path's computation so window crops
/// land on the same physical pixels the readback carries.
pub(super) fn output_render_scale(server: &aegis_compositor::Server, host: &Host) -> f32 {
    server
        .output_infos()
        .first()
        .map(|output| output.geometry.scale.as_f32())
        .filter(|scale| *scale > 0.0)
        .unwrap_or_else(|| host.scale())
}

/// One window's current visible region in physical output pixels, clamped
/// to the frame. `None` when the window is gone. A window partly off the
/// output reports its clamped (smaller) size, which the caller treats as a
/// geometry change and ends the stream (ADR-0054).
pub(super) fn window_physical_rect(
    windows: &[aegis_model::window::Window],
    window: aegis_model::window::WindowId,
    scale: f32,
    frame_width: u32,
    frame_height: u32,
) -> Option<aegis_model::Rect> {
    let window = windows.iter().find(|candidate| candidate.id == window)?;
    let logical = aegis_model::Rect {
        origin: window.position,
        size: window.size,
    };
    Some(logical_rect_to_physical(
        logical,
        scale,
        frame_width,
        frame_height,
    ))
}

/// Extract one window's rows out of a shared full-frame readback.
/// `rect` is in physical pixels and already clamped to the frame.
fn crop_stream_frame(
    frame: &StreamPixels,
    rect: aegis_model::Rect,
) -> (u32, u32, std::sync::Arc<[u8]>) {
    let width = rect.size.w.max(0) as u32;
    let height = rect.size.h.max(0) as u32;
    let x = rect.origin.x as usize;
    let y = rect.origin.y as usize;
    let row = width as usize * 4;
    let mut out = Vec::with_capacity(row * height as usize);
    for line in y..y + height as usize {
        let start = (line * frame.width as usize + x) * 4;
        out.extend_from_slice(&frame.bgra[start..start + row]);
    }
    (width, height, out.into())
}

impl CompositorRuntime {
    /// Fan one converted readback out to every due SHM stream. Bounded per
    /// stream: a full delivery lane reports `false` from
    /// `aegis_ipc::Server::push_stream_frame` and the frame counts as dropped
    /// (ADR-0052), so a stalled consumer only ever loses frames. Window
    /// streams crop the shared frame to their window's current visible
    /// region (ADR-0054); a window that closed or changed size ends its
    /// stream instead of delivering a size the consumer never negotiated.
    pub(super) fn deliver_stream_frame(&mut self, frame: StreamPixels) {
        let Some(ipc) = self.ipc.as_ref() else {
            return;
        };
        let now = Instant::now();
        let mut windows = None;
        let mut ended: Vec<(u64, String)> = Vec::new();
        for stream_id in self.streams.due_shm_ids(now) {
            let Some((sequence, dropped)) = self.streams.sequence_and_dropped(stream_id) else {
                continue;
            };
            let Some((target, size)) = self.streams.target_of(stream_id) else {
                continue;
            };
            let damage_rects = if frame.damage.is_empty() {
                vec![aegis_model::Rect::new(
                    0,
                    0,
                    frame.width as i32,
                    frame.height as i32,
                )]
            } else {
                frame.damage.clone()
            };
            let payload = match target {
                aegis_ipc::StreamTarget::Output => {
                    aegis_ipc::StreamFramePayload::Pixels(aegis_ipc::StreamPixelFrame {
                        stream_id,
                        sequence,
                        width: frame.width,
                        height: frame.height,
                        stride: frame.width * 4,
                        format: aegis_ipc::StreamPixelFormat::Bgra8,
                        damage: damage_rects,
                        dropped,
                        pixels: std::sync::Arc::clone(&frame.bgra),
                    })
                }
                aegis_ipc::StreamTarget::Window { window } => {
                    let windows = windows.get_or_insert_with(|| self.server.windows());
                    let scale = output_render_scale(&self.server, &self.host);
                    match window_physical_rect(windows, window, scale, frame.width, frame.height) {
                        Some(rect) if (rect.size.w as u32, rect.size.h as u32) == size => {
                            let (width, height, pixels) = crop_stream_frame(&frame, rect);
                            aegis_ipc::StreamFramePayload::Pixels(aegis_ipc::StreamPixelFrame {
                                stream_id,
                                sequence,
                                width,
                                height,
                                stride: width * 4,
                                format: aegis_ipc::StreamPixelFormat::Bgra8,
                                damage: vec![aegis_model::Rect::new(
                                    0,
                                    0,
                                    width as i32,
                                    height as i32,
                                )],
                                dropped,
                                pixels,
                            })
                        }
                        Some(_) => {
                            ended.push((stream_id, "window geometry changed".to_owned()));
                            continue;
                        }
                        None => {
                            ended.push((stream_id, "window closed".to_owned()));
                            continue;
                        }
                    }
                }
            };
            let delivered = ipc.push_stream_frame(payload);
            self.streams.record_frame(stream_id, now, delivered);
        }
        for (stream_id, reason) in ended {
            log::info!("stream {stream_id}: {reason}; ending");
            ipc.end_stream(stream_id, &reason);
            self.streams.stop(stream_id);
        }
    }

    /// Create a dmabuf stream's capture surface and enumerate its slot ring
    /// (IPC protocol 25): `STREAM_SLOT_COUNT` blank frames visit the slots in
    /// order, exporting one descriptor per slot for the client's slot table.
    /// The modifier is constrained to the presentation surface's, so the
    /// post-present copy never crosses formats. Any failure is reported to
    /// the caller, which falls back to SHM.
    pub(super) fn create_dmabuf_capture(&self, width: u32, height: u32) -> Result<DmabufCapture, String> {
        let modifier = self
            .surface
            .dmabuf_modifier()
            .ok_or_else(|| "presentation surface is not dma-buf exportable".to_owned())?;
        let surface = flux::Surface::offscreen_dmabuf(&self.device, width, height, &[modifier])
            .map_err(|error| {
                format!(
                    "capture surface: {error}{}",
                    flux_last_error_detail()
                )
            })?;
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
                .end_checked()
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
            table: aegis_ipc::StreamSlotTable {
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
    /// overwrite a consumer-owned slot.
    pub(super) fn blit_dmabuf_stream_frames(&mut self, acquire_fence: Option<&OwnedFd>) {
        let Some(presented) = self.host.presented_dmabuf() else {
            return;
        };
        let now = Instant::now();
        let mut ended: Vec<(u64, String)> = Vec::new();
        for stream_id in self.streams.due_dmabuf_ids(now) {
            let Some(stream) = self.streams.streams.get_mut(&stream_id) else {
                continue;
            };
            let Some(dmabuf) = stream.dmabuf.as_mut() else {
                continue;
            };
            let Some(_slot) = dmabuf.ring.next_submission_slot() else {
                // The consumer still owns the ring's next slot.
                stream.dropped += 1;
                stream.last_frame = Some(now);
                continue;
            };
            let sequence = stream.sequence + 1;
            let dropped = stream.dropped;
            match blit_presented_frame(&self.device, dmabuf, &presented, acquire_fence) {
                Ok((slot, fence, source)) => {
                    dmabuf.ring.submitted(slot);
                    dmabuf.pending.push(PendingSlotFrame {
                        slot,
                        fence,
                        _source: source,
                        sequence,
                        dropped,
                        submitted_at: now,
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
            for (stream_id, reason) in ended {
                log::warn!("stream {stream_id}: dmabuf capture failed after submit: {reason}; ending");
                ipc.end_stream(stream_id, &reason);
                self.streams.stop(stream_id);
            }
        }
    }

    /// Deliver dmabuf stream frames whose acquire fence signaled (IPC
    /// protocol 25), mirroring the SHM path's `read_pixels_ready` polling. A
    /// signaled frame is pushed to its connection lane and its slot becomes
    /// consumer-owned until `StreamBufferRelease`; a full lane or a wedged
    /// fence counts the frame as dropped and recycles the slot.
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
                    continue;
                }
                let (width, height) = stream.size;
                let payload = aegis_ipc::StreamFramePayload::Slot(aegis_ipc::StreamSlotFrame {
                    stream_id,
                    sequence: pending.sequence,
                    width,
                    height,
                    stride: dmabuf.slot_stride,
                    format: aegis_ipc::StreamPixelFormat::Dmabuf {
                        drm_format: DRM_FORMAT_XRGB8888,
                        modifier: dmabuf.modifier,
                    },
                    damage: vec![aegis_model::Rect::new(0, 0, width as i32, height as i32)],
                    dropped: pending.dropped,
                    slot: pending.slot as u32,
                    byte_len: dmabuf.slot_bytes,
                });
                let delivered = ipc.push_stream_frame(payload);
                dmabuf.ring.fence_signaled(pending.slot, delivered);
                if !delivered {
                    stream.dropped += 1;
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
enum BlitFailure {
    /// Nothing was submitted; the stream may retry its next due frame.
    Retryable(String),
    /// The frame was submitted but the slot could not be exported; the ring
    /// position diverged and the stream must end.
    Submitted(String),
}

/// Import the presented frame and copy it into the capture surface's next
/// ring slot, exporting the slot with its explicit acquire fence. Returns
/// the slot, the fence to poll, and the imported source image (retired when
/// the fence fires). The presented descriptor stays owned by the caller;
/// flux receives duplicates.
fn blit_presented_frame(
    device: &flux::Device,
    dmabuf: &mut DmabufStream,
    presented: &aegis_backend::host::PresentedDmabuf,
    acquire_fence: Option<&OwnedFd>,
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
    // after the compositor's own rendering of that image.
    let import = unsafe {
        match &fence {
            Some(fence) => flux::Image::import_dmabuf_with_acquire_fence(
                device,
                presented.width,
                presented.height,
                flux::Format::FLUX_FORMAT_BGRA8_UNORM,
                presented.modifier,
                fd.as_raw_fd(),
                0,
                presented.stride,
                fence.as_raw_fd(),
            ),
            None => flux::Image::import_dmabuf(
                device,
                presented.width,
                presented.height,
                flux::Format::FLUX_FORMAT_BGRA8_UNORM,
                presented.modifier,
                fd.as_raw_fd(),
                0,
                presented.stride,
            ),
        }
    };
    let source = match import {
        Ok(image) => {
            // Flux took ownership of both descriptor duplicates.
            std::mem::forget(fd);
            if let Some(fence) = fence {
                std::mem::forget(fence);
            }
            image
        }
        Err(error) => {
            return Err(BlitFailure::Retryable(format!(
                "import presented dma-buf: {error}{}",
                flux_last_error_detail()
            )))
        }
    };
    let frame = dmabuf
        .surface
        .begin_frame()
        .map_err(|error| {
            BlitFailure::Retryable(format!(
                "capture begin_frame: {error}{}",
                flux_last_error_detail()
            ))
        })?;
    begin_opaque_frame(&dmabuf.canvas, &frame, flux::rgba(0, 0, 0, 255))
        .map_err(|error| BlitFailure::Retryable(format!("capture pass: {error}")))?;
    dmabuf.canvas.draw_image_opaque(
        &source,
        0.0,
        0.0,
        presented.width as f32,
        presented.height as f32,
    );
    dmabuf
        .canvas
        .end_checked()
        .map_err(|error| BlitFailure::Retryable(format!("capture pass: {error}")))?;
    // Everything past the submit advances flux's ring.
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
    Ok((export.slot as usize, fence, source))
}

impl CompositorRuntime {
    /// End every live stream from the server side (output geometry change),
    /// notifying each client and dropping local state.
    pub(super) fn end_all_streams(&mut self, reason: &str) {
        let ids = self.streams.stop_all();
        if ids.is_empty() {
            return;
        }
        log::info!("stream: ending {} stream(s): {reason}", ids.len());
        if let Some(ipc) = self.ipc.as_ref() {
            for stream_id in ids {
                ipc.end_stream(stream_id, reason);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn max_fps_defaults_and_clamps() {
        let mut streams = OutputStreams::new();
        let default = streams.start(1, None, (1920, 1080), aegis_ipc::StreamTarget::Output);
        assert_eq!(
            streams.streams[&default.stream_id].frame_interval,
            Duration::from_secs(1) / 30
        );
        let high = streams.start(1, Some(240), (1920, 1080), aegis_ipc::StreamTarget::Output);
        assert_eq!(
            streams.streams[&high.stream_id].frame_interval,
            Duration::from_secs(1) / 60
        );
        let low = streams.start(1, Some(0), (1920, 1080), aegis_ipc::StreamTarget::Output);
        assert_eq!(
            streams.streams[&low.stream_id].frame_interval,
            Duration::from_secs(1)
        );
    }

    #[test]
    fn due_streams_respect_the_throttle() {
        let mut streams = OutputStreams::new();
        let fast = streams
            .start(1, Some(60), (100, 100), aegis_ipc::StreamTarget::Output)
            .stream_id;
        let slow = streams
            .start(1, Some(1), (100, 100), aegis_ipc::StreamTarget::Output)
            .stream_id;
        let t0 = Instant::now();
        // Both start due.
        assert_eq!(streams.due_shm_ids(t0), vec![fast, slow]);
        streams.record_frame(fast, t0, true);
        streams.record_frame(slow, t0, true);
        // 20ms later: only the 60fps stream is due again.
        assert_eq!(streams.due_shm_ids(t0 + Duration::from_millis(20)), vec![fast]);
        // 1.1s later: both.
        assert_eq!(
            streams.due_shm_ids(t0 + Duration::from_millis(1100)),
            vec![fast, slow]
        );
    }

    #[test]
    fn sequence_and_dropped_track_delivery() {
        let mut streams = OutputStreams::new();
        let id = streams
            .start(1, Some(30), (100, 100), aegis_ipc::StreamTarget::Output)
            .stream_id;
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
        let target = aegis_ipc::StreamTarget::Window {
            window: aegis_model::window::WindowId(9),
        };
        let id = streams.start(1, None, (640, 480), target).stream_id;
        assert_eq!(streams.target_of(id), Some((target, (640, 480))));
        assert_eq!(streams.target_of(999), None);
    }

    #[test]
    fn window_physical_rect_scales_and_clamps() {
        let mut window = aegis_model::window::Window::new(aegis_model::window::WindowId(3));
        window.position = aegis_model::Point { x: 10, y: 20 };
        window.size = aegis_model::Size { w: 100, h: 50 };
        let windows = vec![window];

        // Scale 2: logical 100x50 at (10,20) becomes physical 200x100 at
        // (20,40).
        let rect =
            window_physical_rect(&windows, aegis_model::window::WindowId(3), 2.0, 1920, 1080)
                .expect("window is live");
        assert_eq!(rect, aegis_model::Rect::new(20, 40, 200, 100));

        // Partially offscreen: clamped to the frame (the caller ends the
        // stream on the size change).
        let mut window = aegis_model::window::Window::new(aegis_model::window::WindowId(4));
        window.position = aegis_model::Point { x: -20, y: 0 };
        window.size = aegis_model::Size { w: 100, h: 50 };
        let rect = window_physical_rect(&[window], aegis_model::window::WindowId(4), 1.0, 200, 200)
            .expect("window is live");
        assert_eq!(rect, aegis_model::Rect::new(0, 0, 80, 50));

        assert!(
            window_physical_rect(&windows, aegis_model::window::WindowId(99), 1.0, 100, 100)
                .is_none()
        );
    }

    #[test]
    fn crop_stream_frame_extracts_rows() {
        // 4x2 frame, pixels valued by their index.
        let bgra: Vec<u8> = (0u8..32).collect();
        let frame = StreamPixels {
            width: 4,
            height: 2,
            bgra: bgra.into(),
            damage: Vec::new(),
        };
        let (w, h, pixels) = crop_stream_frame(&frame, aegis_model::Rect::new(1, 1, 2, 1));
        assert_eq!((w, h), (2, 1));
        // Row 1 starts at byte 16; two pixels from x=1: bytes 20..28.
        assert_eq!(&pixels[..], &(20u8..28).collect::<Vec<u8>>()[..]);
    }

    #[test]
    fn stop_and_disconnect_remove_stream_state() {
        let mut streams = OutputStreams::new();
        let a = streams
            .start(1, None, (100, 100), aegis_ipc::StreamTarget::Output)
            .stream_id;
        let b = streams
            .start(2, None, (100, 100), aegis_ipc::StreamTarget::Output)
            .stream_id;
        let c = streams
            .start(2, None, (100, 100), aegis_ipc::StreamTarget::Output)
            .stream_id;
        streams.stop(a);
        assert!(!streams.streams.contains_key(&a));
        streams.disconnect(2);
        assert!(streams.streams.is_empty());
        assert!(!streams.streams.contains_key(&b));
        assert!(!streams.streams.contains_key(&c));
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
            .start(1, None, (100, 100), aegis_ipc::StreamTarget::Output)
            .stream_id;
        // No dmabuf transport anywhere: releases are inert no-ops.
        streams.release_slot(id, 0);
        streams.release_slot(999, 0);
    }

    #[test]
    fn due_ids_split_by_transport() {
        let mut streams = OutputStreams::new();
        let shm = streams
            .start(1, None, (100, 100), aegis_ipc::StreamTarget::Output)
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
            canvas.end_checked().unwrap();
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
        canvas.end_checked().unwrap();
        frame.submit().unwrap().present().unwrap();
        let export = surface.export_dmabuf_explicit().unwrap();
        assert_eq!(export.slot, 0);
        let fence = export.acquire_fence.expect("explicit export carries a fence");
        let mut pollfd = libc::pollfd {
            fd: fence.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: `pollfd` references a live descriptor for the call.
        let result = unsafe { libc::poll(&mut pollfd, 1, 2000) };
        assert!(result > 0, "capture slot fence did not signal");
    }
}
