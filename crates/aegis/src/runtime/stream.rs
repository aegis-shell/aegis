//! Continuous output frame streaming (ADR-0052).
//!
//! The registry lives on the compositor main loop. A stream is registered by
//! an IPC `StreamOutputStart` (authorization happens in `ass-ipc` before the
//! request reaches this loop), is throttled to its `max_fps`, and fans one
//! shared GPU readback out to every due stream. Delivery goes through
//! `aegis_ipc::Server::push_stream_frame`, whose bounded lane reports drops
//! back so the stream's cumulative `dropped` counter stays accurate.

use std::collections::BTreeMap;
use std::time::{Duration, Instant};

use super::*;

/// Default frame-rate cap when the client leaves `max_fps` unset.
const DEFAULT_MAX_FPS: u32 = 30;
/// Hard bounds on the negotiated frame-rate cap.
const MIN_MAX_FPS: u32 = 1;
const MAX_MAX_FPS: u32 = 60;

/// One control message from an IPC connection thread, applied on the main
/// loop. Mirrors the capture/realm-control request pattern.
pub(super) struct StreamControlRequest {
    pub(super) conn_id: u64,
    pub(super) action: StreamControl,
}

pub(super) enum StreamControl {
    Start {
        max_fps: Option<u32>,
        target: aegis_ipc::StreamTarget,
        reply: std::sync::mpsc::Sender<Result<aegis_ipc::StreamInfo, String>>,
    },
    /// The server already unregistered the delivery lane (`StreamOutputStop`
    /// request, per-frame authorization failure, or server-side end); the
    /// main loop only drops its own state.
    Stop { stream_id: u64 },
    /// The connection disconnected; every stream it owned was unregistered
    /// server-side.
    Disconnect,
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
            },
        );
        aegis_ipc::StreamInfo {
            stream_id,
            width: size.0,
            height: size.1,
            format: aegis_ipc::StreamPixelFormat::Bgra8,
        }
    }

    pub(super) fn stop(&mut self, stream_id: u64) {
        self.streams.remove(&stream_id);
    }

    /// Drop every stream `conn_id` owned (its IPC connection went away).
    pub(super) fn disconnect(&mut self, conn_id: u64) {
        self.streams.retain(|_, stream| stream.conn_id != conn_id);
    }

    /// Ids of streams due a frame at `now`. A stream that never received a
    /// frame is due immediately.
    pub(super) fn due_ids(&self, now: Instant) -> Vec<u64> {
        self.streams
            .iter()
            .filter(|(_, stream)| {
                stream
                    .last_frame
                    .is_none_or(|last| now.duration_since(last) >= stream.frame_interval)
            })
            .map(|(id, _)| *id)
            .collect()
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
    windows: &[aegis_core::window::Window],
    window: aegis_core::window::WindowId,
    scale: f32,
    frame_width: u32,
    frame_height: u32,
) -> Option<aegis_core::Rect> {
    let window = windows.iter().find(|candidate| candidate.id == window)?;
    let logical = aegis_core::Rect {
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
    rect: aegis_core::Rect,
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
    /// Fan one converted readback out to every due stream. Bounded per
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
        for stream_id in self.streams.due_ids(now) {
            let Some((sequence, dropped)) = self.streams.sequence_and_dropped(stream_id) else {
                continue;
            };
            let Some((target, size)) = self.streams.target_of(stream_id) else {
                continue;
            };
            let damage_rects = if frame.damage.is_empty() {
                vec![aegis_core::Rect::new(
                    0,
                    0,
                    frame.width as i32,
                    frame.height as i32,
                )]
            } else {
                frame.damage.clone()
            };
            let payload = match target {
                aegis_ipc::StreamTarget::Output => aegis_ipc::StreamFramePayload {
                    stream_id,
                    sequence,
                    width: frame.width,
                    height: frame.height,
                    stride: frame.width * 4,
                    format: aegis_ipc::StreamPixelFormat::Bgra8,
                    damage: damage_rects,
                    dropped,
                    pixels: std::sync::Arc::clone(&frame.bgra),
                },
                aegis_ipc::StreamTarget::Window { window } => {
                    let windows = windows.get_or_insert_with(|| self.server.windows());
                    let scale = output_render_scale(&self.server, &self.host);
                    match window_physical_rect(windows, window, scale, frame.width, frame.height) {
                        Some(rect) if (rect.size.w as u32, rect.size.h as u32) == size => {
                            let (width, height, pixels) = crop_stream_frame(&frame, rect);
                            aegis_ipc::StreamFramePayload {
                                stream_id,
                                sequence,
                                width,
                                height,
                                stride: width * 4,
                                format: aegis_ipc::StreamPixelFormat::Bgra8,
                                damage: vec![aegis_core::Rect::new(
                                    0,
                                    0,
                                    width as i32,
                                    height as i32,
                                )],
                                dropped,
                                pixels,
                            }
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
        assert_eq!(streams.due_ids(t0), vec![fast, slow]);
        streams.record_frame(fast, t0, true);
        streams.record_frame(slow, t0, true);
        // 20ms later: only the 60fps stream is due again.
        assert_eq!(streams.due_ids(t0 + Duration::from_millis(20)), vec![fast]);
        // 1.1s later: both.
        assert_eq!(
            streams.due_ids(t0 + Duration::from_millis(1100)),
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
            window: aegis_core::window::WindowId(9),
        };
        let id = streams.start(1, None, (640, 480), target).stream_id;
        assert_eq!(streams.target_of(id), Some((target, (640, 480))));
        assert_eq!(streams.target_of(999), None);
    }

    #[test]
    fn window_physical_rect_scales_and_clamps() {
        let mut window = aegis_core::window::Window::new(aegis_core::window::WindowId(3));
        window.position = aegis_core::Point { x: 10, y: 20 };
        window.size = aegis_core::Size { w: 100, h: 50 };
        let windows = vec![window];

        // Scale 2: logical 100x50 at (10,20) becomes physical 200x100 at
        // (20,40).
        let rect = window_physical_rect(&windows, aegis_core::window::WindowId(3), 2.0, 1920, 1080)
            .expect("window is live");
        assert_eq!(rect, aegis_core::Rect::new(20, 40, 200, 100));

        // Partially offscreen: clamped to the frame (the caller ends the
        // stream on the size change).
        let mut window = aegis_core::window::Window::new(aegis_core::window::WindowId(4));
        window.position = aegis_core::Point { x: -20, y: 0 };
        window.size = aegis_core::Size { w: 100, h: 50 };
        let rect = window_physical_rect(&[window], aegis_core::window::WindowId(4), 1.0, 200, 200)
            .expect("window is live");
        assert_eq!(rect, aegis_core::Rect::new(0, 0, 80, 50));

        assert!(
            window_physical_rect(&windows, aegis_core::window::WindowId(99), 1.0, 100, 100)
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
        let (w, h, pixels) = crop_stream_frame(&frame, aegis_core::Rect::new(1, 1, 2, 1));
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
