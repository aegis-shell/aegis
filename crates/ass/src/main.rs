//! ass — autonomous surface shell.
//!
//! The process composition root: selects a presentation host, creates the Wayland server,
//! renderer, shell, wallpaper, configuration, and IPC surfaces, then runs the
//! compositor event and presentation loop.

use ass_backend::drm::DrmError;
use ass_backend::host::{BackendKind, Host, HostError};
use ass_backend::Backend;
use std::os::fd::AsRawFd;

mod cursor;

fn main() {
    // Initialize before anything logs. `RUST_LOG` controls verbosity; default
    // to `info` so the bring-up sequence is visible without configuration.
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .try_init();

    if let Err(e) = run() {
        log::error!("ass: {e}");
        std::process::exit(1);
    }
}

/// Persistent (level) input state carried across frames. Per-frame edges
/// (mouse pressed/released, scroll, text, key events) are *not* held here;
/// they are built fresh each frame from backend events and live only for the
/// iteration. This matches lens's contract that the host owns edge derivation
/// (see the `lens::Input` docstring) and mirrors iris's wayland host
/// `drain_input` pattern. Keeping level state separate from per-frame edges
/// guarantees a press/release edge can never leak into the next frame and
/// trigger phantom clicks in immediate-mode widgets.
#[derive(Default)]
struct InputAccumulator {
    cursor: (f32, f32),
    mouse_down: [bool; 3],
    display_size: (f32, f32),
}

impl InputAccumulator {
    /// Mirror of `lens::Input::set_mouse_down` so callers can update the
    /// level state alongside the per-frame snapshot through the same
    /// `lens::MouseButton` key.
    fn set_mouse_down(&mut self, b: lens::MouseButton, down: bool) {
        let idx = match b {
            lens::MouseButton::Left => 0,
            lens::MouseButton::Right => 1,
            lens::MouseButton::Middle => 2,
        };
        self.mouse_down[idx] = down;
    }
}

/// Backdrop effects are evaluated at quarter resolution, then upsampled behind
/// the launcher. Dual-Kawase removes the lost high-frequency detail, while the
/// 16x pixel reduction bounds the cost of live 2D + 3D wallpaper capture.
const BACKDROP_DOWNSAMPLE: u32 = 4;

struct BackdropCapture {
    image: flux::Image,
    size: (u32, u32),
    format: flux::Format,
}

/// Live desktop capture used behind the full-screen application launcher.
///
/// Capture images and blur intermediates are both indexed by frame slot. A
/// slot is rewritten only after `begin_frame` has waited its fence, avoiding
/// device-wide stalls while a 3D wallpaper continues animating.
struct LauncherBackdrop {
    blur: flux::BlurFilter,
    captures: Vec<Option<BackdropCapture>>,
    was_active: bool,
    failed_session: bool,
    unsupported: bool,
}

#[derive(Clone, Copy)]
enum BackdropPlan {
    Direct,
    Capture,
}

impl LauncherBackdrop {
    fn new(device: &flux::Device) -> Result<Self, flux::Error> {
        Ok(Self {
            blur: flux::BlurFilter::new(device)?,
            captures: Vec::new(),
            was_active: false,
            failed_session: false,
            unsupported: false,
        })
    }

    fn prepare(
        &mut self,
        active: bool,
        device: &flux::Device,
        surface: &flux::Surface,
        frame: &flux::Frame<'_>,
        surface_size: (u32, u32),
    ) -> BackdropPlan {
        if !active {
            self.was_active = false;
            self.failed_session = false;
            return BackdropPlan::Direct;
        }

        let opening = !self.was_active;
        self.was_active = true;
        if opening {
            self.failed_session = false;
        }
        if self.unsupported || self.failed_session || surface_size.0 == 0 || surface_size.1 == 0 {
            return BackdropPlan::Direct;
        }
        let format = match surface.format() {
            flux::Format::FLUX_FORMAT_RGBA8_UNORM | flux::Format::FLUX_FORMAT_BGRA8_UNORM => {
                flux::Format::FLUX_FORMAT_RGBA8_UNORM
            }
            other => {
                log::warn!(
                    "launcher: realtime backdrop unavailable for surface format {other:?}; using translucent fallback"
                );
                self.unsupported = true;
                return BackdropPlan::Direct;
            }
        };

        let size = (
            surface_size.0.div_ceil(BACKDROP_DOWNSAMPLE).max(1),
            surface_size.1.div_ceil(BACKDROP_DOWNSAMPLE).max(1),
        );
        let slot = frame.index() as usize;
        if self.captures.len() <= slot {
            self.captures.resize_with(slot + 1, || None);
        }
        let target_stale = self.captures[slot]
            .as_ref()
            .is_none_or(|capture| capture.size != size || capture.format != format);
        if target_stale {
            match flux::Image::render_target(device, size.0, size.1, format) {
                Ok(image) => {
                    self.captures[slot] = Some(BackdropCapture {
                        image,
                        size,
                        format,
                    });
                }
                Err(error) => {
                    log::warn!(
                        "launcher: failed to allocate realtime backdrop target ({error}); using translucent fallback"
                    );
                    self.failed_session = true;
                    return BackdropPlan::Direct;
                }
            }
        }
        BackdropPlan::Capture
    }

    fn begin_capture(
        &mut self,
        canvas: &flux::Canvas,
        frame: &flux::Frame<'_>,
        clear: u32,
    ) -> bool {
        let Some(target) = self.target(frame) else {
            return false;
        };
        if let Err(error) = canvas.begin_target(frame, target, Some(clear)) {
            log::warn!(
                "launcher: failed to begin backdrop capture ({error}); using translucent fallback"
            );
            self.failed_session = true;
            return false;
        }
        true
    }

    fn target(&self, frame: &flux::Frame<'_>) -> Option<&flux::Image> {
        self.captures
            .get(frame.index() as usize)
            .and_then(Option::as_ref)
            .map(|capture| &capture.image)
    }

    fn capture_size(&self, frame: &flux::Frame<'_>) -> Option<(u32, u32)> {
        self.captures
            .get(frame.index() as usize)
            .and_then(Option::as_ref)
            .map(|capture| capture.size)
    }

    fn end_capture_and_blur<'backdrop>(
        &'backdrop mut self,
        canvas: &flux::Canvas,
        frame: &flux::Frame<'_>,
        sigma: f32,
    ) -> Option<flux::BlurredImage<'backdrop>> {
        canvas.end_target();
        let slot = frame.index() as usize;
        let capture = self.captures.get(slot)?.as_ref()?;
        match self.blur.apply(frame, &capture.image, sigma) {
            Ok(image) => Some(image),
            Err(error) => {
                log::warn!(
                    "launcher: realtime backdrop dispatch failed ({error}); using translucent fallback"
                );
                self.failed_session = true;
                None
            }
        }
    }
}

fn draw_wallpaper_background(
    canvas: &flux::Canvas,
    device: &flux::Device,
    wallpaper: &mut Option<ass_wallpaper::Wallpaper>,
    logical_size: (u32, u32),
    scale: f32,
) {
    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    if let Some(wallpaper) = wallpaper.as_mut() {
        wallpaper.draw(device, canvas, logical_size.0 as f32, logical_size.1 as f32);
    }
    canvas.restore();
}

fn draw_client_scene(
    canvas: &flux::Canvas,
    device: &flux::Device,
    renderer: &mut ass_render::Renderer,
    server: &ass_server::Server,
    scale: f32,
) {
    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    let shm = server.toplevel_frames();
    let dmabuf = server.toplevel_dmabuf_frames();
    let sub_shm_below = server.subsurface_frames_below();
    let sub_shm_above = server.subsurface_frames_above();
    let sub_dmabuf_below = server.subsurface_dmabuf_frames_below();
    let sub_dmabuf_above = server.subsurface_dmabuf_frames_above();
    let overlay_shm = server.overlay_frames();
    let overlay_dmabuf = server.overlay_dmabuf_frames();
    renderer.gc(shm
        .iter()
        .map(|frame| frame.id)
        .chain(dmabuf.iter().map(|frame| frame.id))
        .chain(sub_shm_below.iter().map(|frame| frame.id))
        .chain(sub_shm_above.iter().map(|frame| frame.id))
        .chain(sub_dmabuf_below.iter().map(|frame| frame.id))
        .chain(sub_dmabuf_above.iter().map(|frame| frame.id))
        .chain(overlay_shm.iter().map(|frame| frame.id))
        .chain(overlay_dmabuf.iter().map(|frame| frame.id)));
    renderer.draw_subsurfaces(device, canvas, &sub_shm_below);
    renderer.draw_dmabuf_subsurfaces(device, canvas, &sub_dmabuf_below);
    renderer.draw_toplevels(device, canvas, &shm, (0.0, 0.0));
    renderer.draw_dmabuf_toplevels(device, canvas, &dmabuf, (0.0, 0.0));
    renderer.draw_subsurfaces(device, canvas, &sub_shm_above);
    renderer.draw_dmabuf_subsurfaces(device, canvas, &sub_dmabuf_above);
    renderer.draw_toplevels(device, canvas, &overlay_shm, (0.0, 0.0));
    renderer.draw_dmabuf_toplevels(device, canvas, &overlay_dmabuf, (0.0, 0.0));
    canvas.restore();
}

/// Fail-closed session-lock composition. The opaque physical-pixel fill is
/// emitted after all normal desktop/chrome work, then only surfaces owned by
/// the active lock client (plus its cursor) are allowed above it.
fn draw_lock_scene(
    canvas: &flux::Canvas,
    device: &flux::Device,
    renderer: &mut ass_render::Renderer,
    server: &ass_server::Server,
    physical_size: (u32, u32),
    scale: f32,
) {
    canvas.save();
    canvas.fill_rect(
        0.0,
        0.0,
        physical_size.0 as f32,
        physical_size.1 as f32,
        flux::rgba(0, 0, 0, 255),
    );
    canvas.restore();

    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    let shm = server.lock_frames();
    let dmabuf = server.lock_dmabuf_frames();
    renderer.gc(shm
        .iter()
        .map(|frame| frame.id)
        .chain(dmabuf.iter().map(|frame| frame.id)));
    renderer.draw_toplevels(device, canvas, &shm, (0.0, 0.0));
    renderer.draw_dmabuf_toplevels(device, canvas, &dmabuf, (0.0, 0.0));
    canvas.restore();
}

/// Overview scene (M9): the desktop dimmed, then every visible window drawn
/// as a live thumbnail on the shared `ass_core::overview` grid — the exact
/// geometry the overview chrome uses for its frames, labels, and hit-testing.
/// Z-order is preserved bottom-to-top so overlapping thumbnails read like
/// the desktop stack.
fn draw_overview_scene(
    canvas: &flux::Canvas,
    device: &flux::Device,
    renderer: &mut ass_render::Renderer,
    server: &ass_server::Server,
    logical_size: (u32, u32),
    scale: f32,
) {
    canvas.save();
    canvas.fill_rect(
        0.0,
        0.0,
        logical_size.0 as f32 * scale,
        logical_size.1 as f32 * scale,
        flux::rgba(8, 10, 20, 200),
    );
    canvas.restore();

    let windows = server.windows();
    if windows.is_empty() {
        return;
    }
    let rail = server
        .workspace_snapshot()
        .outputs
        .first()
        .map(|o| o.workspaces.len() > 1)
        .unwrap_or(false);
    let display = ass_core::Rect::new(0, 0, logical_size.0 as i32, logical_size.1 as i32);
    let area = ass_core::overview::grid_area(display, rail);
    let slots = ass_core::overview::grid(area, windows.len());
    let cells: std::collections::HashMap<
        ass_core::window::WindowId,
        (ass_core::Rect, ass_core::Point, ass_core::Size),
    > = windows
        .iter()
        .zip(slots.iter())
        .map(|(w, slot)| {
            (
                w.id,
                (ass_core::overview::fit(*slot, w.size), w.position, w.size),
            )
        })
        .collect();
    let map = move |window: Option<ass_core::window::WindowId>, natural: ass_core::Rect| {
        let Some((cell, base, win_size)) = window.and_then(|id| cells.get(&id)) else {
            return natural;
        };
        let k = cell.size.w as f32 / win_size.w.max(1) as f32;
        let remap = |v: i32, b: i32| (v - b) as f32 * k;
        ass_core::Rect::new(
            cell.origin.x + remap(natural.origin.x, base.x).round() as i32,
            cell.origin.y + remap(natural.origin.y, base.y).round() as i32,
            (natural.size.w as f32 * k).round().max(1.0) as i32,
            (natural.size.h as f32 * k).round().max(1.0) as i32,
        )
    };

    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    let shm = server.toplevel_frames();
    let dmabuf = server.toplevel_dmabuf_frames();
    let sub_shm_below = server.subsurface_frames_below();
    let sub_shm_above = server.subsurface_frames_above();
    let sub_dmabuf_below = server.subsurface_dmabuf_frames_below();
    let sub_dmabuf_above = server.subsurface_dmabuf_frames_above();
    renderer.gc(shm
        .iter()
        .map(|frame| frame.id)
        .chain(dmabuf.iter().map(|frame| frame.id))
        .chain(sub_shm_below.iter().map(|frame| frame.id))
        .chain(sub_shm_above.iter().map(|frame| frame.id))
        .chain(sub_dmabuf_below.iter().map(|frame| frame.id))
        .chain(sub_dmabuf_above.iter().map(|frame| frame.id)));
    renderer.draw_subsurfaces_mapped(device, canvas, &sub_shm_below, &map);
    renderer.draw_dmabuf_subsurfaces_mapped(device, canvas, &sub_dmabuf_below, &map);
    renderer.draw_toplevels_mapped(device, canvas, &shm, &map);
    renderer.draw_dmabuf_toplevels_mapped(device, canvas, &dmabuf, &map);
    renderer.draw_subsurfaces_mapped(device, canvas, &sub_shm_above, &map);
    renderer.draw_dmabuf_subsurfaces_mapped(device, canvas, &sub_dmabuf_above, &map);
    canvas.restore();
}

/// Software cursor for direct KMS. First the XDG cursor theme
/// (`$XCURSOR_THEME`/`$XCURSOR_SIZE`, inheritance included) via
/// [`cursor::CursorCache`]; the hand-drawn glyph set below remains as the
/// fallback when the theme ships no image for the requested shape.
/// Client-provided cursor surfaces are already composited by `ass-server`;
/// this covers the cursor-shape protocol and compositor-owned cursors.
fn draw_software_cursor(
    canvas: &flux::Canvas,
    device: &flux::Device,
    cache: &mut cursor::CursorCache,
    position: (f32, f32),
    shape: u32,
    scale: f32,
) {
    if let Some(cursor) = cache.get(device, shape, scale) {
        canvas.save();
        canvas.scale(scale, scale);
        let inv = 1.0 / scale.max(0.25);
        canvas.draw_image(
            &cursor.image,
            position.0 - cursor.xhot * inv,
            position.1 - cursor.yhot * inv,
            cursor.width * inv,
            cursor.height * inv,
        );
        canvas.restore();
        return;
    }
    draw_glyph_cursor(canvas, position, shape, scale);
}

/// Hand-drawn glyph fallback: used only when the XDG theme has no image for
/// the shape (or no theme is installed at all).
fn draw_glyph_cursor(canvas: &flux::Canvas, position: (f32, f32), shape: u32, scale: f32) {
    let black = flux::rgba(12, 12, 16, 255);
    let white = flux::rgba(245, 245, 248, 255);
    canvas.save();
    canvas.scale(scale, scale);
    canvas.translate(position.0, position.1);

    let bar = |canvas: &flux::Canvas, x: f32, y: f32, w: f32, h: f32| {
        canvas.fill_rrect(x - 1.0, y - 1.0, w + 2.0, h + 2.0, 1.5, black);
        canvas.fill_rrect(x, y, w, h, 1.0, white);
    };
    match shape {
        7 | 8 | 32 | 36 => {
            bar(canvas, 8.0, 0.0, 3.0, 19.0);
            bar(canvas, 0.0, 8.0, 19.0, 3.0);
        }
        9 => {
            bar(canvas, 8.0, 0.0, 3.0, 20.0);
            bar(canvas, 4.0, 0.0, 11.0, 3.0);
            bar(canvas, 4.0, 17.0, 11.0, 3.0);
        }
        10 => {
            bar(canvas, 0.0, 8.0, 20.0, 3.0);
            bar(canvas, 0.0, 4.0, 3.0, 11.0);
            bar(canvas, 17.0, 4.0, 3.0, 11.0);
        }
        18 | 25 | 26 | 30 => {
            bar(canvas, 0.0, 8.0, 20.0, 3.0);
            bar(canvas, 0.0, 4.0, 3.0, 11.0);
            bar(canvas, 17.0, 4.0, 3.0, 11.0);
        }
        19 | 22 | 27 | 31 => {
            bar(canvas, 8.0, 0.0, 3.0, 20.0);
            bar(canvas, 4.0, 0.0, 11.0, 3.0);
            bar(canvas, 4.0, 17.0, 11.0, 3.0);
        }
        20 | 24 | 28 => {
            canvas.translate(2.0, 15.0);
            canvas.rotate(-std::f32::consts::FRAC_PI_4);
            bar(canvas, 0.0, 0.0, 22.0, 3.0);
            bar(canvas, 0.0, -3.0, 3.0, 9.0);
            bar(canvas, 19.0, -3.0, 3.0, 9.0);
        }
        21 | 23 | 29 => {
            canvas.translate(4.0, 0.0);
            canvas.rotate(std::f32::consts::FRAC_PI_4);
            bar(canvas, 0.0, 0.0, 22.0, 3.0);
            bar(canvas, 0.0, -3.0, 3.0, 9.0);
            bar(canvas, 19.0, -3.0, 3.0, 9.0);
        }
        _ => {
            // Arrow-like diagonal with a short tail; hotspot is (0, 0).
            canvas.rotate(-std::f32::consts::FRAC_PI_4);
            bar(canvas, 0.0, 0.0, 18.0, 4.0);
            bar(canvas, 10.0, 2.0, 4.0, 10.0);
        }
    }
    canvas.restore();
}

/// Immutable GPU readback staging detached from the presentation surface and
/// handed to the capture worker. `crop` is already converted to physical
/// pixels; the full CPU copy and every later operation stay off the
/// compositor's presentation-critical thread.
struct CapturedPixels {
    width: u32,
    height: u32,
    readback: flux::Readback,
    crop: Option<ass_core::Rect>,
}

struct PendingReadback {
    width: u32,
    height: u32,
    crop: Option<ass_core::Rect>,
}

enum CaptureTarget {
    Screenshot {
        path: String,
        command: ass_ipc::Command,
        ts_mono_ms: u64,
    },
    Reply {
        reply: std::sync::mpsc::Sender<Result<(u32, u32, String), String>>,
    },
}

struct PendingCapture {
    readback: PendingReadback,
    target: CaptureTarget,
}

enum CaptureJob {
    Screenshot {
        capture: CapturedPixels,
        path: String,
        command: ass_ipc::Command,
        ts_mono_ms: u64,
    },
    Reply {
        capture: CapturedPixels,
        reply: std::sync::mpsc::Sender<Result<(u32, u32, String), String>>,
    },
}

struct ScreenshotCompletion {
    command: ass_ipc::Command,
    ts_mono_ms: u64,
    effect: ass_ipc::Effect,
}

fn read_captured_pixels(
    surface: &flux::Surface,
    pending: PendingReadback,
) -> Result<CapturedPixels, String> {
    let readback = surface
        .take_readback()
        .map_err(|error| format!("detach shot readback: {error}{}", flux_last_error_detail()))?;
    Ok(CapturedPixels {
        width: pending.width,
        height: pending.height,
        readback,
        crop: pending.crop,
    })
}

/// Convert a compositor-logical crop rectangle to physical output pixels.
///
/// Scaling both endpoints avoids accumulating a rounding error in the width
/// or height at fractional scales. The result is clamped to the readback
/// surface so regions partially outside the focused output remain safe.
fn logical_rect_to_physical(
    rect: ass_core::Rect,
    scale: f32,
    width: u32,
    height: u32,
) -> ass_core::Rect {
    let scale = if scale.is_finite() && scale > 0.0 {
        f64::from(scale)
    } else {
        1.0
    };
    let right = i64::from(rect.origin.x) + i64::from(rect.size.w.max(0));
    let bottom = i64::from(rect.origin.y) + i64::from(rect.size.h.max(0));
    let scaled = |value: i64| (value as f64 * scale).round() as i64;
    let x0 = scaled(i64::from(rect.origin.x)).clamp(0, i64::from(width));
    let y0 = scaled(i64::from(rect.origin.y)).clamp(0, i64::from(height));
    let x1 = scaled(right).clamp(x0, i64::from(width));
    let y1 = scaled(bottom).clamp(y0, i64::from(height));
    ass_core::Rect::new(x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32)
}

/// Extract a sub-rectangle from a full RGBA8 buffer.
fn crop_rgba(src: &[u8], src_w: u32, _src_h: u32, rect: ass_core::Rect) -> Vec<u8> {
    let src_w = src_w as usize;
    let x = rect.origin.x as usize;
    let y = rect.origin.y as usize;
    let w = rect.size.w.max(0) as usize;
    let h = rect.size.h.max(0) as usize;
    let mut out = Vec::with_capacity(w * h * 4);
    for row in y..y + h {
        let start = (row * src_w + x) * 4;
        out.extend_from_slice(&src[start..start + w * 4]);
    }
    out
}

/// Finish a capture away from the frame thread. Cropping first bounds the
/// unpremultiply and PNG work for region captures.
fn encode_capture(capture: CapturedPixels) -> Result<(u32, u32, Vec<u8>), String> {
    let mut full_rgba = vec![0u8; capture.width as usize * capture.height as usize * 4];
    capture
        .readback
        .read_pixels(&mut full_rgba)
        .map_err(|error| format!("shot pixel copy: {error}"))?;
    encode_rgba_capture(capture.width, capture.height, full_rgba, capture.crop)
}

fn encode_rgba_capture(
    full_width: u32,
    full_height: u32,
    full_rgba: Vec<u8>,
    crop: Option<ass_core::Rect>,
) -> Result<(u32, u32, Vec<u8>), String> {
    let (width, height, mut rgba) = match crop {
        Some(crop) => (
            crop.size.w as u32,
            crop.size.h as u32,
            crop_rgba(&full_rgba, full_width, full_height, crop),
        ),
        None => (full_width, full_height, full_rgba),
    };
    unpremultiply(&mut rgba);
    let png = encode_png(width, height, &rgba)?;
    Ok((width, height, png))
}

/// Single bounded post-processing lane for screenshots and IPC pixel
/// captures. Only one full-frame payload may be in flight, which keeps
/// repeated requests from consuming unbounded memory or compounding stalls.
struct CaptureWorker {
    jobs: std::sync::mpsc::Sender<CaptureJob>,
    completions: std::sync::mpsc::Receiver<ScreenshotCompletion>,
    busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl CaptureWorker {
    fn spawn() -> std::io::Result<Self> {
        let (job_tx, job_rx) = std::sync::mpsc::channel::<CaptureJob>();
        let (completion_tx, completion_rx) = std::sync::mpsc::channel::<ScreenshotCompletion>();
        let busy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_busy = std::sync::Arc::clone(&busy);
        std::thread::Builder::new()
            .name("ass-capture".into())
            .spawn(move || {
                while let Ok(job) = job_rx.recv() {
                    match job {
                        CaptureJob::Screenshot {
                            capture,
                            path,
                            command,
                            ts_mono_ms,
                        } => {
                            let effect = match encode_capture(capture).and_then(|(_, _, png)| {
                                std::fs::write(&path, png)
                                    .map_err(|error| format!("write {path}: {error}"))
                            }) {
                                Ok(()) => {
                                    log::info!("screenshot: wrote {path}");
                                    ass_ipc::Effect::Applied
                                }
                                Err(reason) => ass_ipc::Effect::Refused { reason },
                            };
                            if completion_tx
                                .send(ScreenshotCompletion {
                                    command,
                                    ts_mono_ms,
                                    effect,
                                })
                                .is_err()
                            {
                                worker_busy.store(false, std::sync::atomic::Ordering::Release);
                                break;
                            }
                            // The main loop clears `busy` after it records the
                            // completion, keeping the loop awake until then.
                        }
                        CaptureJob::Reply { capture, reply } => {
                            let result = encode_capture(capture).map(|(width, height, png)| {
                                (width, height, ass_ipc::base64::encode(&png))
                            });
                            let _ = reply.send(result);
                            worker_busy.store(false, std::sync::atomic::Ordering::Release);
                        }
                    }
                }
                worker_busy.store(false, std::sync::atomic::Ordering::Release);
            })?;
        Ok(Self {
            jobs: job_tx,
            completions: completion_rx,
            busy,
        })
    }

    fn reserve(&self) -> bool {
        self.busy
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
    }

    fn release(&self) {
        self.busy.store(false, std::sync::atomic::Ordering::Release);
    }

    fn is_busy(&self) -> bool {
        self.busy.load(std::sync::atomic::Ordering::Acquire)
    }

    fn submit(&self, job: CaptureJob) -> Result<(), Box<CaptureJob>> {
        self.jobs.send(job).map_err(|error| Box::new(error.0))
    }
}

fn refuse_capture_target(
    worker: &CaptureWorker,
    target: CaptureTarget,
    reason: String,
    journal: &std::sync::Arc<std::sync::Mutex<ass_ipc::Journal>>,
    ipc: &Option<ass_ipc::Server>,
) {
    worker.release();
    match target {
        CaptureTarget::Screenshot {
            command,
            ts_mono_ms,
            ..
        } => journal_effect_and_broadcast(
            journal,
            ipc,
            ts_mono_ms,
            ass_ipc::Origin::Ipc { conn_id: 0 },
            command,
            ass_ipc::Effect::Refused { reason },
        ),
        CaptureTarget::Reply { reply } => {
            let _ = reply.send(Err(reason));
        }
    }
}

fn queue_captured_pixels(
    worker: &CaptureWorker,
    capture: CapturedPixels,
    target: CaptureTarget,
    journal: &std::sync::Arc<std::sync::Mutex<ass_ipc::Journal>>,
    ipc: &Option<ass_ipc::Server>,
) {
    let job = match target {
        CaptureTarget::Screenshot {
            path,
            command,
            ts_mono_ms,
        } => CaptureJob::Screenshot {
            capture,
            path,
            command,
            ts_mono_ms,
        },
        CaptureTarget::Reply { reply } => CaptureJob::Reply { capture, reply },
    };
    if let Err(job) = worker.submit(job) {
        let target = match *job {
            CaptureJob::Screenshot {
                path,
                command,
                ts_mono_ms,
                ..
            } => CaptureTarget::Screenshot {
                path,
                command,
                ts_mono_ms,
            },
            CaptureJob::Reply { reply, .. } => CaptureTarget::Reply { reply },
        };
        refuse_capture_target(
            worker,
            target,
            "capture worker stopped".to_owned(),
            journal,
            ipc,
        );
    }
}

/// Convert premultiplied RGBA8 (the flux/Wayland contract) to the straight
/// alpha PNG encoders expect.
fn unpremultiply(pixels: &mut [u8]) {
    for px in pixels.chunks_exact_mut(4) {
        let a = u32::from(px[3]);
        if a > 0 && a < 255 {
            for channel in &mut px[0..3] {
                *channel = ((u32::from(*channel) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("png encode: {e}"))?;
    Ok(out)
}

/// flux's thread-local diagnostic for the most recent error, formatted for
/// logs; empty when the call carried no detail.
fn flux_last_error_detail() -> String {
    let mut info: flux_sys::flux_error_info = unsafe { std::mem::zeroed() };
    unsafe { flux_sys::flux_get_last_error(&mut info) };
    if info.message.is_null() {
        return String::new();
    }
    let message = unsafe { std::ffi::CStr::from_ptr(info.message) };
    format!(" ({})", message.to_string_lossy())
}

/// One pixel-capture request from an IPC connection thread, answered by the
/// main loop after it copies the exact output frame being submitted.
struct CaptureRequest {
    reply: std::sync::mpsc::Sender<Result<(u32, u32, String), String>>,
    /// Logical-pixel region to capture, or `None` for the full output.
    region: Option<ass_core::Rect>,
}

/// Direct swapchain composition. A model wallpaper inserts one depth-tested
/// pass between the 2D background and client canvas draws.
#[derive(Clone, Copy)]
struct RenderGeometry {
    logical_size: (u32, u32),
    scale: f32,
}

#[allow(clippy::too_many_arguments)]
fn draw_direct_desktop_scene(
    canvas: &flux::Canvas,
    device: &flux::Device,
    frame: &mut flux::Frame<'_>,
    wallpaper: &mut Option<ass_wallpaper::Wallpaper>,
    renderer: &mut ass_render::Renderer,
    server: &ass_server::Server,
    geometry: RenderGeometry,
    overview: bool,
) -> Result<(), flux::Error> {
    let RenderGeometry {
        logical_size,
        scale,
    } = geometry;
    draw_wallpaper_background(canvas, device, wallpaper, logical_size, scale);
    if wallpaper
        .as_ref()
        .is_some_and(|wallpaper| wallpaper.has_model())
    {
        canvas.end();
        if let Some(wallpaper) = wallpaper.as_mut() {
            wallpaper.draw_model(device, frame);
        }
        canvas.begin(frame, None)?;
    }
    if overview {
        draw_overview_scene(canvas, device, renderer, server, logical_size, scale);
    } else {
        draw_client_scene(canvas, device, renderer, server, scale);
    }
    Ok(())
}

/// Dispatch an [`ass_ipc::Command`] to the server and side-effect targets. Extracted
/// from the three mutation sources (IPC, keybindings, chrome) so the journal
/// chokepoint (ADR-0033) sees every mutation through one path.
fn apply_command(
    server: &mut ass_server::Server,
    notif_queue: &std::sync::Arc<std::sync::Mutex<ass_core::notify::NotificationQueue>>,
    quit: &mut bool,
    cmd: &ass_ipc::Command,
    ipc: &Option<ass_ipc::Server>,
    ts_mono_ms: u64,
) {
    use ass_ipc::Command;
    match cmd {
        Command::Focus { id } => server.focus_surface_by_id(*id),
        Command::Minimize { id } => server.minimize_toplevel(*id),
        Command::Close { id } => server.close_toplevel(*id),
        Command::Move { id } => server.start_interactive_move(*id),
        Command::SetWindowGeometry { id, rect } => {
            server.set_window_geometry(*id, *rect);
        }
        Command::InjectInput { .. } => {
            // Synthetic input needs shell-occlusion validation and is handled
            // beside the physical-input router in the main loop.
            debug_assert!(false, "InjectInput reached the generic command path");
        }
        Command::Screenshot { .. } => {
            // Screenshots need the GPU objects and are handled beside the
            // frame renderer in the main loop.
            debug_assert!(false, "Screenshot reached the generic command path");
        }
        Command::ToggleOverview => {
            // The overview is shell-owned; toggled beside the IPC drain.
            debug_assert!(false, "ToggleOverview reached the generic command path");
        }
        Command::Cycle { forward } => server.cycle_focus(*forward),
        Command::SwitchWorkspace { dir } => server.switch_workspace(*dir),
        Command::SwitchWorkspaceTo { id } => server.switch_workspace_to(*id),
        Command::MoveToWorkspace { window, workspace } => {
            server.move_to_workspace(*window, *workspace)
        }
        Command::ToggleTiling => server.set_tiling(!server.tiling()),
        Command::Notify {
            summary,
            body,
            app_id,
        } => {
            let n = notif_queue.lock().unwrap().push(
                summary.clone(),
                body.clone(),
                app_id.clone(),
                ts_mono_ms,
            );
            if let Some(s) = ipc.as_ref() {
                s.broadcast(ass_ipc::Event::Notified { notification: n });
            }
        }
        Command::DismissNotification { id } => {
            notif_queue.lock().unwrap().dismiss(*id);
        }
        Command::Quit => *quit = true,
    }
}

/// Record a mutation in the journal and push it to journal subscribers
/// (ADR-0033).
fn journal_and_broadcast(
    journal: &std::sync::Arc<std::sync::Mutex<ass_ipc::Journal>>,
    ipc: &Option<ass_ipc::Server>,
    ts_mono_ms: u64,
    origin: ass_ipc::Origin,
    cmd: ass_ipc::Command,
) {
    journal_effect_and_broadcast(
        journal,
        ipc,
        ts_mono_ms,
        origin,
        cmd,
        ass_ipc::Effect::Applied,
    );
}

fn journal_effect_and_broadcast(
    journal: &std::sync::Arc<std::sync::Mutex<ass_ipc::Journal>>,
    ipc: &Option<ass_ipc::Server>,
    ts_mono_ms: u64,
    origin: ass_ipc::Origin,
    cmd: ass_ipc::Command,
    effect: ass_ipc::Effect,
) {
    let mut j = journal.lock().unwrap();
    let entry = j.append(ts_mono_ms, origin, cmd, effect);
    if let Some(s) = ipc.as_ref() {
        s.broadcast_journal(entry.clone());
    }
}

/// Apply one trusted Control Center mutation. Compositor-native layout changes
/// return an IPC command so they pass through the journal chokepoint; host
/// hardware controls are dispatched through their standard Linux tools.
fn apply_system_action(
    server: &mut ass_server::Server,
    notifications: &std::sync::Arc<std::sync::Mutex<ass_core::notify::NotificationQueue>>,
    status: &mut ass_shell::SystemStatus,
    action: ass_shell::SystemAction,
) -> Option<ass_ipc::Command> {
    use ass_shell::SystemAction;

    match action {
        SystemAction::ToggleMute => {
            spawn_host_command("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]);
            status.muted = !status.muted;
        }
        SystemAction::StepVolume(delta) => {
            let amount = format!(
                "{}%{}",
                delta.unsigned_abs(),
                if delta >= 0 { "+" } else { "-" }
            );
            spawn_host_command(
                "wpctl",
                &["set-volume", "@DEFAULT_AUDIO_SINK@", &amount, "-l", "1.0"],
            );
            let current = status.volume.unwrap_or(0) as i16;
            status.volume = Some((current + i16::from(delta)).clamp(0, 100) as u8);
        }
        SystemAction::SetVolume(level) => {
            let level = level.min(100);
            let amount = format!("{level}%");
            spawn_host_command(
                "wpctl",
                &["set-volume", "@DEFAULT_AUDIO_SINK@", &amount, "-l", "1.0"],
            );
            status.volume = Some(level);
        }
        SystemAction::SetBrightness(level) => {
            let level = level.clamp(1, 100);
            let amount = format!("{level}%");
            spawn_host_command("brightnessctl", &["--class=backlight", "set", &amount]);
            status.brightness = Some(level);
        }
        SystemAction::SetWifi(enabled) => {
            spawn_host_command(
                "nmcli",
                &["radio", "wifi", if enabled { "on" } else { "off" }],
            );
            status.wifi_enabled = Some(enabled);
        }
        SystemAction::SetBluetooth(enabled) => {
            spawn_host_command(
                "rfkill",
                &[if enabled { "unblock" } else { "block" }, "bluetooth"],
            );
            status.bluetooth_enabled = Some(enabled);
        }
        SystemAction::SetDoNotDisturb(enabled) => {
            notifications.lock().unwrap().set_do_not_disturb(enabled);
            status.do_not_disturb = enabled;
        }
        SystemAction::SetTiling(enabled) => {
            status.tiled = enabled;
            if server.tiling() != enabled {
                return Some(ass_ipc::Command::ToggleTiling);
            }
        }
        // Touchpad profiles are persisted and applied by the main loop, which
        // owns both the config file and the selected input backend.
        SystemAction::SetTouchpad(_) => {}
    }
    None
}

fn spawn_host_command(program: &str, args: &[&str]) {
    let result = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(error) = result {
        log::warn!("control center: failed to start {program}: {error}");
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    log::info!(
        "ass {} — autonomous surface shell",
        env!("CARGO_PKG_VERSION")
    );

    // Notification queue (M9, over the IPC): shared between the IPC handler
    // (reads), the toast chrome component (renders), and this loop (pushes
    // on `Notify`, expires each frame). Declared early so the toast
    // component registration below can clone it.
    let notif_queue: std::sync::Arc<std::sync::Mutex<ass_core::notify::NotificationQueue>> =
        std::sync::Arc::new(std::sync::Mutex::new(
            ass_core::notify::NotificationQueue::new(5_000),
        ));

    // Declarative configuration (ADR-0026). One TOML file at
    // `$XDG_CONFIG_HOME/ass/config.toml` is the source of truth; absence is
    // not an error (built-in defaults apply). A malformed or
    // schema-incompatible file is logged and skipped, not fatal. Loaded
    // before the backend so configured display modes are known at the very
    // first modeset (ADR-0028).
    let config_path = ass_config::default_path();
    let mut config = load_config(config_path.as_deref());

    // Select the presentation target before Vulkan creation: nested Wayland
    // requires WSI extensions, while DRM requires exportable offscreen images.
    // `auto` uses an outer Wayland display when present and atomic DRM on a TTY.
    let backend_kind = requested_backend()?;
    let host_bootstrap = Host::open(
        backend_kind,
        "ass",
        1280,
        720,
        configured_output_modes(config.as_ref()),
    )?;
    let device = host_bootstrap.create_device()?;
    // Move the host into a binding declared after the device so Rust drops the
    // host-owned VkSurfaceKHR before Flux destroys its VkInstance.
    let mut host = host_bootstrap;
    host.set_touchpad_config(
        config
            .as_ref()
            .map(|c| c.input.touchpad)
            .unwrap_or_default(),
    );
    log::info!(
        "flux: device created for {} backend; dma-buf {}",
        host.name(),
        if flux::dmabuf_supported(&device) {
            "supported"
        } else {
            "unavailable"
        }
    );

    // Nested mode creates a Vulkan WSI swapchain. DRM mode creates an
    // exportable offscreen ring that the backend imports into KMS.
    let (w, h) = host.physical_size();
    log::info!(
        "{}: presentation target {w}x{h} (scale {})",
        host.name(),
        host.scale()
    );

    // Flux presentation surface + canvas.
    let mut surface = host.create_surface(&device)?;
    if let Err(error) = surface.prepare_readback() {
        log::warn!(
            "capture: could not preallocate readback staging: {error}{}",
            flux_last_error_detail()
        );
    }
    let mut canvas = flux::Canvas::new(&surface)?;
    let mut launcher_backdrop = LauncherBackdrop::new(&device)?;
    // A requested presentation frame remains in mapped readback staging until
    // the main loop copies it into an owned CPU buffer.
    let mut pending_capture: Option<PendingCapture> = None;
    // PNG compression, base64 conversion, and file writes run here instead
    // of pausing the compositor frame thread after GPU readback.
    let capture_worker = CaptureWorker::spawn()?;
    // XDG cursor theme cache for the software cursor on direct KMS.
    let mut cursor_cache = cursor::CursorCache::default();
    // Advertise the pre-scaled buffer to the host; takes effect on the next
    // commit (the first present below).
    host.set_buffer_scale();

    // `config` was loaded above, before the backend, so output policy also
    // applies before icons decode.
    let mut screenshot_dir = config
        .as_ref()
        .map(|c| std::path::PathBuf::from(&c.screenshot.save_dir))
        .unwrap_or_else(ass_config::default_screenshot_dir);

    // Wayland server: accept client connections on its own socket. Created
    // before the icon pass so the effective output scale (backend-reported
    // geometry plus any `[[output]]` override) is known when icons decode.
    let mut server = ass_server::Server::new_with_render_caps(
        flux::dmabuf_supported(&device),
        flux::dmabuf_sync_supported(&device),
    )?;
    server.set_outputs(host.output_infos());
    log::info!("server: listening on WAYLAND_DISPLAY={}", server.socket());
    if let Some(c) = config.as_ref() {
        server.set_output_policies(c.output_policies());
    }
    // The effective scale the whole frame renders at: the primary output's
    // geometry after overrides, falling back to the host's own scale
    // (nested, where the host compositor owns scaling).
    let effective_scale = server
        .output_infos()
        .first()
        .map(|o| o.geometry.scale.as_f32())
        .filter(|s| *s > 0.0)
        .unwrap_or_else(|| host.scale());

    // Enumerate launchable `.desktop` entries at startup; the catalog is
    // rescanned periodically below so package installs/removals appear without
    // restarting the compositor.
    let mut icon_theme = selected_icon_theme();
    let mut icon_scale = effective_scale.ceil().max(1.0) as u32;
    let mut launcher_apps = application_catalog(&icon_theme, icon_scale);
    log::info!(
        "launcher: {} launchable applications discovered (icon theme: {})",
        launcher_apps.len(),
        icon_theme
    );
    // Decode each app entry's raster icon into a flux texture once, keyed by
    // every app_id the entry might run as (StartupWMClass, desktop-id stem,
    // icon name) so the dock can look a running toplevel up by its `app_id`.
    // SVG icons are rasterized through the host's standard rsvg-convert when
    // available. The cache owns the GPU textures and must outlive the shell,
    // so it is declared before it.
    let mut icon_cache = build_icon_cache(&device, &launcher_apps, &icon_theme, icon_scale);
    let mut icon_snapshot = snapshot_icons(&launcher_apps);

    // Compositor chrome, bound to the same device. The core host ships with
    // no chrome of its own; compose it from the components the binary wants.
    let mut shell = unsafe { ass_shell::Shell::new(device.as_raw() as *mut _) }?;
    // Window decorations are intentionally not registered: windows are
    // borderless (macOS-style), managed through the dock, tiling, and key
    // bindings rather than per-window title bars.
    shell.add(Box::new(ass_shell::HudBar::with_notifications(
        std::sync::Arc::clone(&notif_queue),
        icon_cache.map.clone(),
    )));
    shell.add(Box::new(ass_shell::Toast::new(std::sync::Arc::clone(
        &notif_queue,
    ))));
    // Only the binary wires discovery to chrome (ADR-0022); the shell stays
    // free of `ass-apps`. Register the launcher after ordinary overlays so its
    // full-screen surface covers workspace/toast chrome, while the dock (added
    // last below) remains available like macOS Launchpad.
    shell.add(Box::new(ass_shell::Launcher::with_icons(
        launcher_apps.clone(),
        icon_cache.map.clone(),
    )));
    // The overview (M9): a modal window/workspace picker over the same live
    // scene; registered with the modal chrome so it covers ordinary overlays.
    shell.add(Box::new(ass_shell::Overview::new()));
    // Built-in applications share the launcher catalog with XDG entries but
    // render in-process through optics/lens. Register the backing component
    // above the launcher and ordinary chrome, while leaving the dock last.
    shell.add(Box::new(ass_shell::ControlCenter::with_icons(
        icon_cache.map.clone(),
    )));
    // Interactive screenshot region selector, triggered by the Print key.
    shell.add(Box::new(ass_shell::ScreenshotSelector::new()));
    // The dock is added after the config is loaded below, so it can read the
    // `[dock]` pinned list.
    let mut input_acc = InputAccumulator::default();
    // Seed the chrome's logical extent so widgets can lay out before the first
    // resize arrives. The server's output geometry (backend + overrides) is
    // authoritative; the host size is the nested fallback.
    {
        let logical = server
            .output_infos()
            .first()
            .map(|o| o.geometry.logical_size());
        let (w, h) = logical
            .map(|s| (s.w as f32, s.h as f32))
            .unwrap_or_else(|| {
                let sz = host.size();
                (sz.w as f32, sz.h as f32)
            });
        input_acc.display_size = (w, h);
    }

    // Repoint $WAYLAND_DISPLAY at this compositor's socket so children laun-
    // ched from here (the dock / launcher via `ass-launch`) connect back to
    // *us*, not the host session ass is nested in. The host connection was
    // already captured above by `Host::open`, which does not
    // re-read the env var after connect, so overwriting it here is safe.
    // `ass-launch::inherit_display_env` reads this var to seed each child.
    std::env::set_var("WAYLAND_DISPLAY", server.socket());

    // Compositing of client surfaces.
    let mut renderer = ass_render::Renderer::new();
    let start = std::time::Instant::now();

    // Wallpaper: a still image (png/jpg/webp/gif/…) or a short video decoded by
    // an external ffmpeg. `$ASS_WALLPAPER` selects the image; with it unset we
    // fall back to a bundled demo wallpaper so a bare `cargo run` shows a
    // desktop rather than the bare clear colour. The default is resolved at
    // compile time relative to the crate, so it works straight from
    // `cargo run`. A missing/failed load is not fatal — the clear colour shows
    // through.
    //
    // The decode resolution is seeded from the initial *physical* host size so
    // the wallpaper is decoded at the framebuffer's true resolution; later
    // resizes GPU-scale the wallpaper on draw without re-decoding.
    const DEFAULT_WALLPAPER: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/wallpapers/procedural-generation.png"
    );
    let (init_w, init_h) = host.physical_size();
    let wallpaper_override = std::env::var("ASS_WALLPAPER")
        .ok()
        .filter(|value| !value.is_empty());
    let wallpaper_path = wallpaper_override
        .clone()
        .unwrap_or_else(|| DEFAULT_WALLPAPER.to_string());
    let is_gltf = std::path::Path::new(&wallpaper_path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("glb"));
    let loaded = if is_gltf {
        ass_wallpaper::Wallpaper::from_gltf(&device, &surface, &wallpaper_path)
    } else {
        ass_wallpaper::Wallpaper::from_path(&wallpaper_path, init_w, init_h)
    };
    let mut wallpaper = match loaded {
        Ok(mut wallpaper) => {
            if !is_gltf {
                let model_override = std::env::var("ASS_WALLPAPER_MODEL")
                    .ok()
                    .filter(|value| !value.is_empty());
                let model_result = if let Some(path) = model_override.as_deref() {
                    wallpaper.set_model_from_gltf(&device, &surface, path)
                } else if wallpaper_override.is_none() {
                    wallpaper.set_builtin_model(&device, &surface)
                } else {
                    Ok(())
                };
                if let Err(error) = model_result {
                    log::warn!("wallpaper: 3D model disabled: {error}");
                }
            }
            log::info!("wallpaper: enabled ({wallpaper_path})");
            Some(wallpaper)
        }
        Err(e) => {
            log::warn!("wallpaper: load failed for {wallpaper_path}: {e}");
            None
        }
    };

    let clear = flux::rgba(30, 30, 46, 255);
    let mut frame_count: u64 = 0;
    // Nested-only deferral for retired client buffers: with no exportable
    // completion fence, the loop releases them a few presented frames late
    // instead of stalling the whole device on a wait_idle. Holds the frame
    // count at which the first pending retirement was seen.
    let mut retired_defer: Option<u64> = None;

    // Global launcher hotkey: a bare Super tap (press and release with no
    // other key in between) toggles the launcher. Super still works as a
    // modifier for every other combo — only a clean tap fires. See ADR-0022.
    let mut super_tap = ass_core::input::TapDetector::super_tap();
    // Tracks the previous frame's keyboard-capture state so the main loop can
    // grab/release the keyboard on edges (launcher open/close).
    let mut prev_captured = false;

    // Global key bindings: built-in defaults overridden by the config file's
    // `[[keybind]]` entries. The deprecated `$ASS_KEYBINDS` env var is still
    // honored as a transitional override (logged) and takes precedence over
    // the file; it is removed before the desktop phase closes. `forward_input`
    // consumes a matched key before delivering it to the focused client.
    let mut keymap = build_keymap(config.as_ref());
    log::info!("keybinds: {} active", keymap.len());
    // Seed the window rules from the loaded config (ADR-0026). Re-applied on
    // each reload above.
    server.set_window_rules(
        config
            .as_ref()
            .map(|c| c.window_rules.clone())
            .unwrap_or_default(),
    );
    // Seed the tiling layout params (ADR-0024) and the focused output's
    // geometry (ADR-0028) from the config and the initial host size.
    if let Some(c) = config.as_ref() {
        server.set_layout_params(c.layout.clone().into());
        server.set_tiling_default(c.layout.default_tiled);
        shell.set_reduced_motion(c.ui.reduced_motion);
        server.set_reduced_motion(c.ui.reduced_motion);
        cursor_cache.set_config(c.ui.cursor_theme.clone(), c.ui.cursor_size);
        server.set_output_policies(c.output_policies());
    }
    // The dock: a persistent strip of pinned `.desktop` app icons (ADR-0022),
    // built from the config's `[dock] pinned` list, or auto-populated from the
    // enumerated apps that have a usable icon when no pins are configured. It
    // borrows the icon cache (which outlives the shell). Added last so it
    // stacks above the other chrome.
    let pinned = build_dock_apps(
        &launcher_apps,
        &icon_cache.map,
        config
            .as_ref()
            .map(|c| c.dock.pinned.as_slice())
            .unwrap_or(&[]),
        config.as_ref().map(|c| c.dock.autopopulate).unwrap_or(true),
    );
    log::info!("dock: {} app(s) pinned", pinned.len());
    shell.add(Box::new(ass_shell::Dock::with_apps(
        pinned,
        icon_cache.map.clone(),
    )));

    // One normalized status snapshot feeds both the compact HUD and the
    // built-in Control Center. Host probes (wpctl fork+exec) run on a helper
    // thread so the compositor never blocks a frame on a subprocess; the
    // main loop applies the latest snapshot it finds on the channel.
    const SYSTEM_STATUS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);
    let mut system_status = ass_shell::SystemStatus::detect();
    system_status.do_not_disturb = notif_queue.lock().unwrap().do_not_disturb();
    system_status.tiled = server.tiling();
    system_status.touchpad = host.touchpad_status();
    shell.set_system_status(system_status.clone());
    let (status_tx, status_rx) = std::sync::mpsc::channel::<ass_shell::SystemStatus>();
    std::thread::Builder::new()
        .name("ass-status".into())
        .spawn(move || {
            while status_tx.send(ass_shell::SystemStatus::detect()).is_ok() {
                std::thread::sleep(SYSTEM_STATUS_INTERVAL);
            }
        })
        .expect("spawn status poller");

    // mtime-based reload watcher, polled each frame. `None` when there is no
    // default config path on this host.
    let mut reload = config_path.as_deref().map(ass_config::ReloadWatcher::at);
    let mut quit_requested = false;

    // IPC and introspection surface (ADR-0027). A unix socket at
    // `$XDG_RUNTIME_DIR/ass.sock` serves the `query` capability over a
    // snapshot shared with the main loop via an `Arc`. Connection threads
    // read the snapshot; the main loop writes it each frame. `control`/
    // `session` commands come back through `ipc_cmd_rx` and are applied on
    // this thread. Bind failure is non-fatal so the compositor runs without
    // IPC rather than crashing. `ipc` is held to the end of `run()` so its
    // `Drop` removes the socket.
    let (ipc_cmd_tx, ipc_cmd_rx) = std::sync::mpsc::channel::<ass_ipc::Command>();
    let (capture_tx, capture_rx) = std::sync::mpsc::channel::<CaptureRequest>();
    let journal = std::sync::Arc::new(std::sync::Mutex::new(ass_ipc::Journal::default_capacity()));
    let live = std::sync::Arc::new(LiveState::new(
        ipc_cmd_tx,
        capture_tx,
        std::sync::Arc::clone(&notif_queue),
        std::sync::Arc::clone(&journal),
        build_ipc_scopes(config.as_ref()),
    ));
    let ipc: Option<ass_ipc::Server> = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) => {
            let path = std::path::PathBuf::from(d).join("ass.sock");
            match ass_ipc::Server::start(&path, std::sync::Arc::clone(&live)) {
                Ok(s) => {
                    log::info!("ipc: listening on {}", path.display());
                    Some(s)
                }
                Err(e) => {
                    log::warn!("ipc: failed to bind {}: {e}", path.display());
                    None
                }
            }
        }
        None => {
            log::warn!("ipc: $XDG_RUNTIME_DIR unset; no IPC socket");
            None
        }
    };
    // Signature of the last broadcast window set, used to detect changes.
    let mut last_win_sig: Option<Vec<(ass_core::window::WindowId, bool, Option<String>)>> = None;
    // Last broadcast workspace snapshot, used to detect model changes.
    let mut last_ws_snap: Option<ass_core::workspace::WorkspaceSnapshot> = None;
    // Whether chrome reported a multi-frame animation in flight last frame.
    // While true the loop pumps non-blocking dispatches and renders at a
    // ~60fps cadence so the animation advances even with the pointer still;
    // once it rests the loop goes back to blocking on the host event queue.
    let mut animating = false;
    // Pointer ownership at the end of the previous input batch. Keeping the
    // edge lets us send exactly one wl_pointer.leave when entering chrome and
    // synthesize motion before a click that returns to client content.
    let mut chrome_pointer_captured = false;
    // Synthetic pointer movement is independent of the nested host's physical
    // cursor. The next physical pointer event realigns the server before a
    // human button/axis event is delivered, preventing a click at stale
    // synthetic coordinates.
    let mut synthetic_pointer_active = false;
    let mut last_cursor_shape = 0u32;
    let mut last_cursor_hidden = false;
    // Runtime application rescan: package managers and user-created desktop
    // entries become visible in launcher/dock during a long-running session.
    // The scan decodes icon files — far too slow for the frame loop — so a
    // worker thread does the reading and the main loop only applies results
    // (GPU texture upload + catalog swap) when they arrive.
    const APP_RESCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
    let mut next_app_scan = std::time::Instant::now() + APP_RESCAN_INTERVAL;
    let (scan_req_tx, scan_req_rx) = std::sync::mpsc::channel::<u32>();
    #[allow(clippy::type_complexity)]
    let (scan_result_tx, scan_result_rx) = std::sync::mpsc::channel::<(
        String,
        Vec<ass_core::app::Entry>,
        std::collections::BTreeMap<std::path::PathBuf, Option<IconFileStamp>>,
    )>();
    std::thread::Builder::new()
        .name("ass-app-scan".into())
        .spawn(move || {
            while let Ok(scale) = scan_req_rx.recv() {
                let theme = selected_icon_theme();
                let catalog = application_catalog(&theme, scale);
                let snapshot = snapshot_icons(&catalog);
                if scan_result_tx.send((theme, catalog, snapshot)).is_err() {
                    break;
                }
            }
        })
        .expect("spawn app scanner");
    let mut pending_scan_scale = icon_scale;
    let mut previous_frame_at = std::time::Instant::now();

    loop {
        // Choose dispatch mode: while animating, wait on the event queue
        // with a deadline that caps the animation at ~60fps; input that
        // arrives mid-budget wakes the loop and is processed immediately
        // (a blind sleep would leave it queued). Once idle, block on the
        // host queue for the next wakeup (input, client commit, resize).
        // Presentation itself is throttled by the backend (FIFO acquire /
        // the KMS flip wait), so this deadline is only an upper bound.
        let alive = if animating {
            let frame_interval = std::time::Duration::from_micros(16_667);
            let remaining = frame_interval.saturating_sub(previous_frame_at.elapsed());
            if remaining.is_zero() {
                host.dispatch_nonblocking()
            } else {
                host.dispatch_timeout(remaining)
            }
        } else {
            host.dispatch_timeout(std::time::Duration::from_secs(1))
        };
        if !alive || shell.should_quit() || quit_requested {
            break;
        }
        while let Ok(completion) = capture_worker.completions.try_recv() {
            journal_effect_and_broadcast(
                &journal,
                &ipc,
                completion.ts_mono_ms,
                ass_ipc::Origin::Ipc { conn_id: 0 },
                completion.command,
                completion.effect,
            );
            capture_worker.release();
        }
        if pending_capture.is_some() {
            let readiness = surface.read_pixels_ready().map_err(|error| {
                format!(
                    "shot readback readiness: {error}{}",
                    flux_last_error_detail()
                )
            });
            match readiness {
                Ok(false) => {}
                Ok(true) => {
                    let pending = pending_capture.take().expect("checked above");
                    match read_captured_pixels(&surface, pending.readback) {
                        Ok(capture) => queue_captured_pixels(
                            &capture_worker,
                            capture,
                            pending.target,
                            &journal,
                            &ipc,
                        ),
                        Err(reason) => refuse_capture_target(
                            &capture_worker,
                            pending.target,
                            reason,
                            &journal,
                            &ipc,
                        ),
                    }
                }
                Err(reason) => {
                    let pending = pending_capture.take().expect("checked above");
                    refuse_capture_target(&capture_worker, pending.target, reason, &journal, &ipc);
                }
            }
        }
        // libseat revokes DRM/input devices while another VT owns the seat.
        // The backend continues dispatching seat events but no rendering or
        // client input may occur until the enable event restores ownership.
        if !host.is_active() {
            continue;
        }
        let frame_at = std::time::Instant::now();
        let frame_dt = (frame_at - previous_frame_at)
            .as_secs_f32()
            .clamp(0.0, 1.0 / 15.0);
        previous_frame_at = frame_at;

        while let Ok(mut detected) = status_rx.try_recv() {
            detected.do_not_disturb = notif_queue.lock().unwrap().do_not_disturb();
            detected.tiled = server.tiling();
            detected.touchpad = host.touchpad_status();
            if detected != system_status {
                system_status = detected;
                shell.set_system_status(system_status.clone());
            }
        }
        let touchpad_status = host.touchpad_status();
        if touchpad_status != system_status.touchpad {
            system_status.touchpad = touchpad_status;
            shell.set_system_status(system_status.clone());
        }
        // Hot-reload the configuration when its mtime moves (ADR-0026). One
        // `stat` per frame is cheap and keeps the reload on this loop, where
        // the keymap rebuild must happen anyway. A failed reload keeps the
        // previous configuration rather than reverting silently.
        if let Some(path) = config_path.as_deref() {
            if reload.as_mut().is_some_and(|w| w.changed(path))
                && reload_config(
                    path,
                    &mut config,
                    &mut keymap,
                    &mut server,
                    &mut shell,
                    &mut cursor_cache,
                )
            {
                screenshot_dir = config
                    .as_ref()
                    .map(|c| std::path::PathBuf::from(&c.screenshot.save_dir))
                    .unwrap_or_else(ass_config::default_screenshot_dir);
                // Output follow-through: hand the backend the fresh mode
                // requests (consumed at the next modeset), then re-feed its
                // current geometries so a policy *removed* from the file
                // reverts to the backend-reported value instead of lingering
                // in the live output set.
                host.set_configured_modes(configured_output_modes(config.as_ref()));
                system_status.touchpad = host.set_touchpad_config(
                    config
                        .as_ref()
                        .map(|c| c.input.touchpad)
                        .unwrap_or_default(),
                );
                shell.set_system_status(system_status.clone());
                server.set_outputs(host.output_infos());
                live.set_scopes(build_ipc_scopes(config.as_ref()));
                let pinned = build_dock_apps(
                    &launcher_apps,
                    &icon_cache.map,
                    config
                        .as_ref()
                        .map(|c| c.dock.pinned.as_slice())
                        .unwrap_or(&[]),
                    config.as_ref().map(|c| c.dock.autopopulate).unwrap_or(true),
                );
                shell.update_app_catalog(&launcher_apps, &pinned, &icon_cache.map);
            }
        }

        if std::time::Instant::now() >= next_app_scan {
            next_app_scan = std::time::Instant::now() + APP_RESCAN_INTERVAL;
            pending_scan_scale = host.scale().ceil().max(1.0) as u32;
            let _ = scan_req_tx.send(pending_scan_scale);
        }
        while let Ok((refreshed_theme, refreshed, refreshed_snapshot)) = scan_result_rx.try_recv() {
            let refreshed_scale = pending_scan_scale;
            let catalog_changed = refreshed != launcher_apps;
            let icons_changed = refreshed_snapshot != icon_snapshot;
            let theme_changed = refreshed_theme != icon_theme;
            let scale_changed = refreshed_scale != icon_scale;
            if catalog_changed || icons_changed || theme_changed || scale_changed {
                log::info!(
                    "launcher: application catalog/icons changed ({} -> {}, theme {} -> {})",
                    launcher_apps.len(),
                    refreshed.len(),
                    icon_theme,
                    refreshed_theme
                );
                let refreshed_icons =
                    build_icon_cache(&device, &refreshed, &refreshed_theme, refreshed_scale);
                let pinned = build_dock_apps(
                    &refreshed,
                    &refreshed_icons.map,
                    config
                        .as_ref()
                        .map(|c| c.dock.pinned.as_slice())
                        .unwrap_or(&[]),
                    config.as_ref().map(|c| c.dock.autopopulate).unwrap_or(true),
                );
                shell.update_app_catalog(&refreshed, &pinned, &refreshed_icons.map);
                // Components now point only at refreshed_icons; dropping the
                // old cache after the update cannot leave dangling textures.
                icon_cache = refreshed_icons;
            }
            if theme_changed && !catalog_changed && !icons_changed && !scale_changed {
                log::info!(
                    "launcher: icon theme changed ({} -> {}), resolved icons unchanged",
                    icon_theme,
                    refreshed_theme
                );
            }
            launcher_apps = refreshed;
            icon_snapshot = refreshed_snapshot;
            icon_theme = refreshed_theme;
            icon_scale = refreshed_scale;
        }

        // Drain IPC control/session commands and apply them here on the main
        // loop — the Wayland server state is not `Send`, so connection
        // threads forward through the channel rather than touching it
        // directly. Mirrors the chrome-intent drain below (ADR-0016/0027).
        let mut pending_synthetic_input = Vec::new();
        let mut pending_screenshots = Vec::new();
        while let Ok(cmd) = ipc_cmd_rx.try_recv() {
            let ts = start.elapsed().as_millis() as u64;
            if server.session_locked() {
                journal_effect_and_broadcast(
                    &journal,
                    &ipc,
                    ts,
                    ass_ipc::Origin::Ipc { conn_id: 0 },
                    cmd,
                    ass_ipc::Effect::Refused {
                        reason: "session is locked".into(),
                    },
                );
                continue;
            }
            if matches!(cmd, ass_ipc::Command::InjectInput { .. }) {
                pending_synthetic_input.push((cmd, ts));
                continue;
            }
            // The overview lives in the shell; toggling it is a presentation
            // change, not a server mutation, but it still passes the journal.
            if matches!(cmd, ass_ipc::Command::ToggleOverview) {
                shell.toggle_overview();
                journal_and_broadcast(&journal, &ipc, ts, ass_ipc::Origin::Ipc { conn_id: 0 }, cmd);
                continue;
            }
            // Screenshots render with the GPU objects below, not in the
            // generic command path.
            if matches!(cmd, ass_ipc::Command::Screenshot { .. }) {
                pending_screenshots.push((cmd, ts));
                continue;
            }
            apply_command(
                &mut server,
                &notif_queue,
                &mut quit_requested,
                &cmd,
                &ipc,
                ts,
            );
            journal_and_broadcast(&journal, &ipc, ts, ass_ipc::Origin::Ipc { conn_id: 0 }, cmd);
        }
        // Age out expired notifications once per frame.
        notif_queue
            .lock()
            .unwrap()
            .expire(start.elapsed().as_millis() as u64);

        // Process client protocol traffic.
        server.dispatch();
        let session_locked = server.session_locked();
        if session_locked {
            super_tap.cancel_current();
        }
        for state in server.take_text_input_states() {
            host.set_text_input_state(state);
        }
        for event in host.take_text_input() {
            server.text_input_event(&event);
        }
        for event in host.take_pointer_gestures() {
            server.pointer_gesture_event(&event);
        }
        // Drain backend input: forward to clients (via the server's seat) and
        // mirror into the shell's input snapshot so chrome gets first dibs on
        // clicks (e.g. the Quit button). The chrome reads the same pointer
        // position; routing priority is decided by the shell's hit-test.
        //
        // The per-frame `Input` snapshot is rebuilt from the accumulator each
        // iteration: only level state (cursor position, button-held, display
        // size) is carried in; edge flags (pressed/released/scroll/keys/text)
        // start at zero so a press/release in one frame can never bleed into
        // the next and trigger phantom clicks in immediate-mode widgets.
        let mut input = ass_shell::Input::default();
        input.set_display_size(input_acc.display_size.0, input_acc.display_size.1);
        input.set_cursor(input_acc.cursor.0, input_acc.cursor.1);
        input.set_mouse_down(lens::MouseButton::Left, input_acc.mouse_down[0]);
        input.set_mouse_down(lens::MouseButton::Right, input_acc.mouse_down[1]);
        input.set_mouse_down(lens::MouseButton::Middle, input_acc.mouse_down[2]);
        input.set_dt(frame_dt);
        let mut shell_scroll = (0.0_f32, 0.0_f32);
        let mut shell_scroll_pixels = (0.0_f32, 0.0_f32);
        let pointer_before = input_acc.cursor;
        let mut events = host.take_input();
        if !events.is_empty() {
            server.note_user_activity();
        }
        // Coordinate contract: backends emit absolute coordinates in their
        // native space — the nested host in its already-scaled logical
        // pixels, direct KMS in physical panel pixels. The compositor's
        // logical space follows the server's output geometry, which may
        // carry a configured scale override, so convert to logical once
        // here instead of every consumer doing it. On nested the factor is
        // 1.0; on DRM with an unmodified backend scale it is 1.0 too —
        // only a configured override changes it.
        let effective_scale = server
            .output_infos()
            .first()
            .map(|o| o.geometry.scale.as_f32())
            .filter(|s| *s > 0.0)
            .unwrap_or_else(|| host.scale());
        let coord_factor = host.scale() / effective_scale;
        if (coord_factor - 1.0).abs() > f32::EPSILON {
            for ev in &mut events {
                use ass_core::input::{InputEvent::*, TabletEvent};
                match ev {
                    PointerMotion { x, y }
                    | TouchDown { x, y, .. }
                    | TouchMotion { x, y, .. }
                    | Tablet {
                        event: TabletEvent::Proximity { x, y, .. } | TabletEvent::Axes { x, y, .. },
                    } => {
                        *x *= coord_factor;
                        *y *= coord_factor;
                    }
                    _ => {}
                }
            }
        }
        // When chrome (the launcher or a context menu) captures the keyboard,
        // key events go to chrome rather than the focused client. The shell
        // reports capture state from the previous frame's render / key
        // handling, so this is stable for the whole batch.
        let keyboard_captured = !session_locked && shell.captures_keyboard();
        if !events.is_empty() {
            for ev in &events {
                use ass_core::input::InputEvent::*;
                match *ev {
                    PointerMotion { x, y } => {
                        input.set_cursor(x, y);
                        input_acc.cursor = (x, y);
                    }
                    PointerButton { button, state } => {
                        if state.is_pressed() {
                            // A pointer gesture while Super is held is a
                            // modifier drag, not a bare launcher-key tap.
                            super_tap.cancel_current();
                        }
                        // Map Linux BTN_* codes (0x110=left, 0x111=right,
                        // 0x112=middle) to lens's MouseButton. Other buttons
                        // are dropped; the chrome only consumes these three.
                        let mapped = match button {
                            0x110 => Some(lens::MouseButton::Left),
                            0x111 => Some(lens::MouseButton::Right),
                            0x112 => Some(lens::MouseButton::Middle),
                            _ => None,
                        };
                        if let Some(b) = mapped {
                            if state.is_pressed() {
                                input.set_mouse_pressed(b, true);
                                input.set_mouse_down(b, true);
                                input_acc.set_mouse_down(b, true);
                            } else {
                                input.set_mouse_released(b, true);
                                input.set_mouse_down(b, false);
                                input_acc.set_mouse_down(b, false);
                            }
                        }
                    }
                    PointerLeave => {
                        input.set_cursor(-1.0, -1.0);
                        input_acc.cursor = (-1.0, -1.0);
                    }
                    Key { code, state } if keyboard_captured => {
                        // Capture: advance the server's xkb state on every key
                        // event (press and release both keep modifier tracking
                        // consistent), and feed the launcher brain only on
                        // press so typed characters are not double-counted.
                        // Captured keys are withheld from clients below.
                        if !session_locked && super_tap.on_key(code, state.is_pressed()) {
                            shell.toggle();
                        }
                        if let Some(kc) = server.key_char(code, state.is_pressed()) {
                            if state.is_pressed() {
                                shell.key_char(kc);
                            }
                        }
                        // VT switch keys stay compositor-owned while chrome
                        // holds the keyboard too.
                        if let Some(vt) = server.take_vt_switch() {
                            log::info!("{}: VT switch requested to tty{vt}", host.name());
                            host.switch_vt(vt);
                        }
                    }
                    Key { code, state } => {
                        // Not capturing: keys forward to the client normally
                        // (below). The Super-tap detector still observes every
                        // key so a clean tap can open the launcher.
                        if !session_locked && super_tap.on_key(code, state.is_pressed()) {
                            shell.toggle();
                        }
                    }
                    PointerAxis(frame) => {
                        use ass_core::input::PointerAxisSource;
                        if matches!(
                            frame.source,
                            Some(PointerAxisSource::Wheel | PointerAxisSource::WheelTilt)
                        ) {
                            shell_scroll.0 += frame.horizontal.wheel_steps();
                            shell_scroll.1 += frame.vertical.wheel_steps();
                        } else {
                            shell_scroll_pixels.0 += frame.dx();
                            shell_scroll_pixels.1 += frame.dy();
                        }
                    }
                    // Touch events are not handled by the shell chrome yet;
                    // they route to clients via forward_input below.
                    TouchDown { .. }
                    | TouchMotion { .. }
                    | TouchUp { .. }
                    | TouchFrame
                    | TouchCancel
                    | Tablet { .. } => {}
                }
            }
            // Route the batch a second time with compositor overlays removed
            // from the client stream. Pointer motion into chrome becomes one
            // leave; buttons and scroll are consumed until the pointer exits.
            // This prevents a dock/workspace/launcher click from also clicking
            // the client window visually underneath it.
            let display = input_acc.display_size;
            let mut route_cursor = pointer_before;
            let mut forwarded = Vec::with_capacity(events.len() + 1);
            for ev in events.iter().copied() {
                use ass_core::input::InputEvent::*;
                match ev {
                    Key { .. } if keyboard_captured => {}
                    PointerMotion { x, y } => {
                        synthetic_pointer_active = false;
                        route_cursor = (x, y);
                        let captured = !session_locked && shell.captures_pointer_at(x, y, display);
                        if captured {
                            if !chrome_pointer_captured {
                                forwarded.push(PointerLeave);
                            }
                            // A title-bar move or edge resize begins after
                            // chrome handles the press. Once active, pointer
                            // motion still has to reach the server even while
                            // the cursor remains inside that chrome region.
                            if server.interactive().is_some() || server.drag_active() {
                                forwarded.push(ev);
                            }
                        } else {
                            forwarded.push(ev);
                        }
                        chrome_pointer_captured = captured;
                    }
                    PointerButton { state, .. } => {
                        let captured = !session_locked
                            && shell.captures_pointer_at(route_cursor.0, route_cursor.1, display);
                        if synthetic_pointer_active {
                            if !captured {
                                forwarded.push(PointerMotion {
                                    x: route_cursor.0,
                                    y: route_cursor.1,
                                });
                            }
                            synthetic_pointer_active = false;
                        }
                        if captured {
                            if !chrome_pointer_captured {
                                forwarded.push(PointerLeave);
                            }
                            // Chrome-initiated move/resize grabs still need a
                            // release edge to terminate even though ordinary
                            // clicks over the overlay are consumed.
                            if !state.is_pressed()
                                && (server.interactive().is_some() || server.drag_active())
                            {
                                forwarded.push(ev);
                            }
                        } else {
                            // A button/axis can be the first event after an
                            // overlay closes. Re-establish client focus before
                            // forwarding it because the enter-side motion was
                            // consumed while chrome owned the pointer.
                            if chrome_pointer_captured {
                                forwarded.push(PointerMotion {
                                    x: route_cursor.0,
                                    y: route_cursor.1,
                                });
                            }
                            forwarded.push(ev);
                        }
                        chrome_pointer_captured = captured;
                    }
                    PointerAxis(_) => {
                        let captured = !session_locked
                            && shell.captures_pointer_at(route_cursor.0, route_cursor.1, display);
                        if synthetic_pointer_active {
                            if !captured {
                                forwarded.push(PointerMotion {
                                    x: route_cursor.0,
                                    y: route_cursor.1,
                                });
                            }
                            synthetic_pointer_active = false;
                        }
                        if captured {
                            if !chrome_pointer_captured {
                                forwarded.push(PointerLeave);
                            }
                        } else {
                            if chrome_pointer_captured {
                                forwarded.push(PointerMotion {
                                    x: route_cursor.0,
                                    y: route_cursor.1,
                                });
                            }
                            forwarded.push(ev);
                        }
                        chrome_pointer_captured = captured;
                    }
                    PointerLeave => {
                        synthetic_pointer_active = false;
                        route_cursor = (-1.0, -1.0);
                        chrome_pointer_captured = false;
                        forwarded.push(PointerLeave);
                    }
                    TouchDown { x, y, .. } if synthetic_pointer_active => {
                        // Touch delivery shares the server's pointer focus.
                        // Re-hit-test at the physical contact before routing
                        // the down event after a synthetic pointer move.
                        synthetic_pointer_active = false;
                        forwarded.push(PointerMotion { x, y });
                        forwarded.push(ev);
                    }
                    _ => forwarded.push(ev),
                }
            }
            let actions = server.forward_input(&forwarded, &keymap);
            // Ctrl+Alt+Fn: the compositor performs console VT switches itself
            // through libseat (the kernel never sees the key once libinput
            // owns evdev). No-op on the nested backend.
            if let Some(vt) = server.take_vt_switch() {
                log::info!("{}: VT switch requested to tty{vt}", host.name());
                host.switch_vt(vt);
            }
            // Dispatch matched global bindings. (Empty while the launcher
            // captures the keyboard — those keys went to the search box.)
            for action in actions {
                use ass_core::keybind::Action;
                let ts = start.elapsed().as_millis() as u64;
                let origin = ass_ipc::Origin::Keybinding;
                match action {
                    Action::ToggleLauncher => shell.toggle(),
                    Action::ToggleOverview => shell.toggle_overview(),
                    Action::CloseFocused => {
                        if let Some(id) = server.focused_toplevel_id() {
                            let cmd = ass_ipc::Command::Close { id };
                            apply_command(
                                &mut server,
                                &notif_queue,
                                &mut quit_requested,
                                &cmd,
                                &ipc,
                                ts,
                            );
                            journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                        }
                    }
                    Action::CycleFocus => {
                        let cmd = ass_ipc::Command::Cycle { forward: true };
                        apply_command(
                            &mut server,
                            &notif_queue,
                            &mut quit_requested,
                            &cmd,
                            &ipc,
                            ts,
                        );
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                    Action::CycleFocusBack => {
                        let cmd = ass_ipc::Command::Cycle { forward: false };
                        apply_command(
                            &mut server,
                            &notif_queue,
                            &mut quit_requested,
                            &cmd,
                            &ipc,
                            ts,
                        );
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                    Action::WorkspaceNext => {
                        let cmd = ass_ipc::Command::SwitchWorkspace {
                            dir: ass_core::workspace::Switch::Next,
                        };
                        apply_command(
                            &mut server,
                            &notif_queue,
                            &mut quit_requested,
                            &cmd,
                            &ipc,
                            ts,
                        );
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                    Action::WorkspacePrev => {
                        let cmd = ass_ipc::Command::SwitchWorkspace {
                            dir: ass_core::workspace::Switch::Prev,
                        };
                        apply_command(
                            &mut server,
                            &notif_queue,
                            &mut quit_requested,
                            &cmd,
                            &ipc,
                            ts,
                        );
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                    Action::ToggleTiling => {
                        let cmd = ass_ipc::Command::ToggleTiling;
                        apply_command(
                            &mut server,
                            &notif_queue,
                            &mut quit_requested,
                            &cmd,
                            &ipc,
                            ts,
                        );
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                    Action::Quit => {
                        let cmd = ass_ipc::Command::Quit;
                        apply_command(
                            &mut server,
                            &notif_queue,
                            &mut quit_requested,
                            &cmd,
                            &ipc,
                            ts,
                        );
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                    Action::Screenshot => {
                        // Refuse to open the selector while locked or inactive;
                        // the selector itself also suppresses confirmation in
                        // those states, but this avoids the modal entirely.
                        if session_locked || !host.is_active() {
                            log::debug!("screenshot: suppressed while locked or inactive");
                            continue;
                        }
                        shell.start_screenshot();
                    }
                }
            }
        }

        // Apply scoped synthetic actions only after physical input has updated
        // xkb modifier state. The target-local batch was authorized on the IPC
        // thread; this main-loop pass validates live geometry, z-order, and
        // shell occlusion before sending any event.
        for (cmd, ts) in pending_synthetic_input {
            let ass_ipc::Command::InjectInput { id, actions } = &cmd else {
                unreachable!();
            };
            let prepared = server.prepare_synthetic_input(*id, actions);
            let effect = if let Some(events) = prepared {
                let has_key = events
                    .iter()
                    .any(|event| matches!(event, ass_core::input::InputEvent::Key { .. }));
                let blocked_by_chrome = (has_key && shell.captures_keyboard())
                    || events.iter().any(|event| {
                        matches!(
                            *event,
                            ass_core::input::InputEvent::PointerMotion { x, y }
                                if shell.captures_pointer_at(x, y, input_acc.display_size)
                        )
                    });
                if blocked_by_chrome {
                    ass_ipc::Effect::Refused {
                        reason: "target is covered by compositor chrome".into(),
                    }
                } else {
                    server.focus_surface_by_id(*id);
                    let no_bindings = ass_core::keybind::Keymap::default();
                    let actions = server.forward_input(&events, &no_bindings);
                    debug_assert!(actions.is_empty());
                    if events.iter().any(|event| {
                        matches!(event, ass_core::input::InputEvent::PointerMotion { .. })
                    }) {
                        synthetic_pointer_active = true;
                        chrome_pointer_captured = false;
                    }
                    ass_ipc::Effect::Applied
                }
            } else {
                ass_ipc::Effect::Refused {
                    reason: "invalid, hidden, stale, or occluded target".into(),
                }
            };
            journal_effect_and_broadcast(
                &journal,
                &ipc,
                ts,
                ass_ipc::Origin::Ipc { conn_id: 0 },
                cmd,
                effect,
            );
        }
        if session_locked {
            // Keep compositor chrome inert while it is hidden beneath the
            // secure frame. Physical events have already reached the lock
            // client through the server path above.
            input = ass_shell::Input::default();
            input.set_display_size(input_acc.display_size.0, input_acc.display_size.1);
            input.set_cursor(-1.0, -1.0);
            input.set_dt(frame_dt);
        } else {
            input.set_scroll(shell_scroll.0, shell_scroll.1);
            input.set_scroll_pixels(shell_scroll_pixels.0, shell_scroll_pixels.1);
        }

        // A host resize or an output-scale change (window moved to a monitor
        // with a different scale) reports the new *logical* size. The swapchain
        // follows the physical size; layout, input, and the advertised output
        // geometry stay logical. Re-advertise the buffer scale so the host
        // keeps mapping our pre-scaled buffer 1:1.
        if let Some(sz) = host.take_resize() {
            if let Some(capture) = pending_capture.take() {
                refuse_capture_target(
                    &capture_worker,
                    capture.target,
                    "output changed before the captured frame became readable".to_owned(),
                    &journal,
                    &ipc,
                );
            }
            if host.surface_needs_recreate() {
                // Direct KMS: a hotplug changed the plane-modifier
                // intersection the surface was created with. Resize cannot
                // retcon a modifier, so recreate the surface and its canvas
                // at the new display set instead.
                surface = host.create_surface(&device)?;
                canvas = flux::Canvas::new(&surface)?;
            } else {
                let (pw, ph) = host.physical_size();
                surface.resize(pw, ph)?;
            }
            if let Err(error) = surface.prepare_readback() {
                log::warn!(
                    "capture: could not preallocate resized readback staging: {error}{}",
                    flux_last_error_detail()
                );
            }
            host.set_buffer_scale();
            server.set_outputs(host.output_infos());
            // The logical extent follows the fresh server geometry (backend
            // + overrides), so a scale override or a hotplug to a
            // different-scale output re-lays the chrome out correctly.
            let logical = server
                .output_infos()
                .first()
                .map(|o| o.geometry.logical_size())
                .map(|s| (s.w as f32, s.h as f32))
                .unwrap_or((sz.w as f32, sz.h as f32));
            input_acc.display_size = logical;
            input.set_display_size(logical.0, logical.1);
        }

        // Chrome owns the host cursor while it owns pointer routing. This is
        // what gives the launcher's search field a text caret and interactive
        // HUD/dock controls a pointing hand; leaving chrome restores the
        // focused client's requested cursor (including hidden cursors).
        let chrome_cursor = (!session_locked)
            .then(|| {
                shell.cursor_shape_at(
                    input_acc.cursor.0,
                    input_acc.cursor.1,
                    input_acc.display_size,
                )
            })
            .flatten();
        let compositor_cursor = server.compositor_cursor_shape();
        let owned_cursor = if server.interactive().is_some() {
            compositor_cursor
        } else {
            chrome_cursor
                .map(|shape| shape as u32)
                .or(compositor_cursor)
        };
        let cursor_hidden = owned_cursor.is_none() && server.cursor_hidden();
        let cursor_shape = owned_cursor.unwrap_or_else(|| server.cursor_shape().max(1));
        if cursor_hidden != last_cursor_hidden
            || (!cursor_hidden && cursor_shape != last_cursor_shape)
        {
            if cursor_hidden {
                host.hide_cursor();
            } else {
                host.set_cursor_shape(cursor_shape);
            }
            last_cursor_shape = cursor_shape;
            last_cursor_hidden = cursor_hidden;
        }

        // Apply the tiling policy to the current workspace when tiled mode is
        // on (ADR-0024). No-op when off; reconfigures only windows whose
        // target moved. The work-area is the focused output's logical rect
        // (ADR-0028) inset by the chrome's reserved edges, so tiles avoid
        // the dock (ADR-0024 chrome-aware work-area).
        server.apply_tiling(shell.reserved().inset(server.output_logical_rect()));

        match surface.begin_frame() {
            Ok(mut frame) => {
                renderer.begin_frame();
                // Render scale and logical extent come from the server's
                // output geometry (backend + `[[output]]` overrides), not
                // the host, so a configured scale actually changes the
                // desktop. Nested outputs report the host scale, so the
                // nested path is unchanged.
                let (scale, logical_size) = {
                    let geometry = server.output_infos().first().map(|o| o.geometry);
                    let scale = geometry
                        .map(|g| g.scale.as_f32())
                        .filter(|s| *s > 0.0)
                        .unwrap_or_else(|| host.scale());
                    let logical = geometry
                        .map(|g| g.logical_size())
                        .map(|s| (s.w.max(1) as u32, s.h.max(1) as u32))
                        .unwrap_or_else(|| host.size_u32());
                    (scale, logical)
                };
                let render_geometry = RenderGeometry {
                    logical_size,
                    scale,
                };
                let physical_size = surface.size();
                // Bind at most one pending request to this presentation
                // frame. The readback copy is recorded after every scene and
                // cursor draw, so it captures exactly the pixels submitted
                // below rather than a later re-render of mutable state.
                let mut frame_capture: Option<(Option<ass_core::Rect>, CaptureTarget)> = None;
                for req in capture_rx.try_iter() {
                    if session_locked || !host.is_active() {
                        let _ = req
                            .reply
                            .send(Err("session is locked or inactive".to_owned()));
                    } else if !capture_worker.reserve() {
                        let _ = req
                            .reply
                            .send(Err("another capture is still being processed".to_owned()));
                    } else {
                        frame_capture =
                            Some((req.region, CaptureTarget::Reply { reply: req.reply }));
                    }
                }
                for (cmd, ts) in pending_screenshots.drain(..) {
                    let ass_ipc::Command::Screenshot { path, region } = &cmd else {
                        continue;
                    };
                    if session_locked || !host.is_active() {
                        journal_effect_and_broadcast(
                            &journal,
                            &ipc,
                            ts,
                            ass_ipc::Origin::Ipc { conn_id: 0 },
                            cmd,
                            ass_ipc::Effect::Refused {
                                reason: "session is locked or inactive".into(),
                            },
                        );
                    } else if !capture_worker.reserve() {
                        journal_effect_and_broadcast(
                            &journal,
                            &ipc,
                            ts,
                            ass_ipc::Origin::Ipc { conn_id: 0 },
                            cmd,
                            ass_ipc::Effect::Refused {
                                reason: "another capture is still being processed".into(),
                            },
                        );
                    } else {
                        frame_capture = Some((
                            *region,
                            CaptureTarget::Screenshot {
                                path: path.clone(),
                                command: cmd,
                                ts_mono_ms: ts,
                            },
                        ));
                    }
                }
                let blur_sigma = shell.backdrop_blur_sigma();
                let backdrop_regions = shell.backdrop_regions(input_acc.display_size);
                let model_active = wallpaper
                    .as_ref()
                    .is_some_and(ass_wallpaper::Wallpaper::has_model);
                // Overview mode (M9) swaps the whole client scene for the
                // thumbnail grid and skips the launcher-blur capture path.
                let overview_active = shell.overview_active();
                let backdrop_plan = if overview_active {
                    BackdropPlan::Direct
                } else {
                    launcher_backdrop.prepare(
                        blur_sigma > 0.0 && !backdrop_regions.is_empty(),
                        &device,
                        &surface,
                        &frame,
                        physical_size,
                    )
                };

                match backdrop_plan {
                    BackdropPlan::Capture
                        if launcher_backdrop.begin_capture(&canvas, &frame, clear) =>
                    {
                        let capture_size = launcher_backdrop
                            .capture_size(&frame)
                            .unwrap_or(physical_size);
                        let capture_ratio = capture_size.0 as f32 / physical_size.0.max(1) as f32;
                        let capture_scale = scale * capture_ratio;

                        draw_wallpaper_background(
                            &canvas,
                            &device,
                            &mut wallpaper,
                            logical_size,
                            capture_scale,
                        );

                        if model_active {
                            canvas.end_target();
                            if let Some(target) = launcher_backdrop.target(&frame) {
                                if let Some(wallpaper) = wallpaper.as_mut() {
                                    wallpaper.draw_model_to(&device, &mut frame, target);
                                }
                                canvas.begin_target(&frame, target, None)?;
                            }
                        }

                        draw_client_scene(&canvas, &device, &mut renderer, &server, capture_scale);
                        let blurred = launcher_backdrop.end_capture_and_blur(
                            &canvas,
                            &frame,
                            blur_sigma * capture_scale,
                        );
                        canvas.begin(&frame, Some(clear))?;
                        // Preserve the live desktop everywhere, then replace
                        // only the component-declared glass regions with the
                        // shared blurred capture. This is a true backdrop
                        // effect rather than a full-screen blur hidden under
                        // an opaque top-bar colour.
                        draw_direct_desktop_scene(
                            &canvas,
                            &device,
                            &mut frame,
                            &mut wallpaper,
                            &mut renderer,
                            &server,
                            render_geometry,
                            overview_active,
                        )?;
                        if let Some(image) = blurred {
                            for region in &backdrop_regions {
                                let x = region.x.max(0.0) * scale;
                                let y = region.y.max(0.0) * scale;
                                let w = region
                                    .w
                                    .max(0.0)
                                    .min(logical_size.0 as f32 - region.x.max(0.0))
                                    * scale;
                                let h = region
                                    .h
                                    .max(0.0)
                                    .min(logical_size.1 as f32 - region.y.max(0.0))
                                    * scale;
                                if w <= 0.0 || h <= 0.0 {
                                    continue;
                                }
                                canvas.save();
                                canvas.clip_rect(x, y, w, h);
                                image.draw(
                                    &canvas,
                                    0.0,
                                    0.0,
                                    physical_size.0 as f32,
                                    physical_size.1 as f32,
                                );
                                canvas.restore();
                            }
                        }
                    }
                    BackdropPlan::Capture | BackdropPlan::Direct => {
                        canvas.begin(&frame, Some(clear))?;
                        draw_direct_desktop_scene(
                            &canvas,
                            &device,
                            &mut frame,
                            &mut wallpaper,
                            &mut renderer,
                            &server,
                            render_geometry,
                            overview_active,
                        )?;
                    }
                }
                // Hand the shell a snapshot of live toplevels so the chrome's
                // window list reflects the current set. The shell reads
                // title/app_id/activated off each Window to draw its buttons.
                // The same snapshot is mirrored to the IPC (ADR-0027) so the
                // chrome and external tools read identical state, and a
                // change broadcasts `WindowsChanged` to subscribers.
                let win_snapshot = server.windows();
                let sig: Vec<(ass_core::window::WindowId, bool, Option<String>)> = win_snapshot
                    .iter()
                    .map(|w| (w.id, w.state.activated, w.title.clone()))
                    .collect();
                if last_win_sig.as_ref() != Some(&sig) {
                    last_win_sig = Some(sig);
                    if let Some(s) = ipc.as_ref() {
                        s.broadcast(ass_ipc::Event::WindowsChanged);
                    }
                }
                live.set_windows(win_snapshot.clone());
                shell.set_windows(win_snapshot);
                // Mirror the workspace snapshot and broadcast `WorkspaceChanged`
                // on any model mutation (switch, place, remove, reap).
                let ws_snap = server.workspace_snapshot();
                let ws_changed = last_ws_snap.as_ref() != Some(&ws_snap);
                live.set_workspaces(ws_snap.clone());
                shell.set_workspaces(ws_snap.clone());
                live.set_outputs(server.output_infos());
                if ws_changed {
                    last_ws_snap = Some(ws_snap);
                    if let Some(s) = ipc.as_ref() {
                        s.broadcast(ass_ipc::Event::WorkspaceChanged);
                    }
                }
                let do_not_disturb = notif_queue.lock().unwrap().do_not_disturb();
                let tiled = server.tiling();
                if system_status.do_not_disturb != do_not_disturb || system_status.tiled != tiled {
                    system_status.do_not_disturb = do_not_disturb;
                    system_status.tiled = tiled;
                    shell.set_system_status(system_status.clone());
                }
                // Report the output scale so lens rasterises chrome crisply on
                // a HiDPI host; layout and input stay in logical pixels.
                shell.set_scale(scale);
                unsafe { shell.render(canvas.as_raw() as *mut _, &input)? };
                // Confirmation resets the selector before it draws, so this
                // same presentation frame contains the desktop without the
                // selection overlay. Bind that exact frame to the request.
                if let Some(region) = shell.take_screenshot_region() {
                    let path = screenshot_path(&screenshot_dir);
                    let ts = start.elapsed().as_millis() as u64;
                    let command = ass_ipc::Command::Screenshot {
                        path: path.clone(),
                        region: Some(region),
                    };
                    if capture_worker.reserve() {
                        frame_capture = Some((
                            Some(region),
                            CaptureTarget::Screenshot {
                                path,
                                command,
                                ts_mono_ms: ts,
                            },
                        ));
                    } else {
                        journal_effect_and_broadcast(
                            &journal,
                            &ipc,
                            ts,
                            ass_ipc::Origin::Ipc { conn_id: 0 },
                            command,
                            ass_ipc::Effect::Refused {
                                reason: "another capture is still being processed".into(),
                            },
                        );
                    }
                }
                if session_locked {
                    draw_lock_scene(
                        &canvas,
                        &device,
                        &mut renderer,
                        &server,
                        physical_size,
                        scale,
                    );
                }
                // Drain chrome interactions and forward through the apply
                // chokepoint (ADR-0033) so the journal records them.
                let ts = start.elapsed().as_millis() as u64;
                let origin = ass_ipc::Origin::Chrome;
                if let Some(id) = shell.take_clicked_window() {
                    let cmd = ass_ipc::Command::Focus { id };
                    apply_command(
                        &mut server,
                        &notif_queue,
                        &mut quit_requested,
                        &cmd,
                        &ipc,
                        ts,
                    );
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                // Overview intents (M9): a thumbnail pick focuses its window;
                // a rail tile switches workspace while the overview stays open.
                if let Some(id) = shell.take_overview_pick() {
                    let cmd = ass_ipc::Command::Focus { id };
                    apply_command(
                        &mut server,
                        &notif_queue,
                        &mut quit_requested,
                        &cmd,
                        &ipc,
                        ts,
                    );
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                if let Some(id) = shell.take_overview_switch() {
                    let cmd = ass_ipc::Command::SwitchWorkspaceTo { id };
                    apply_command(
                        &mut server,
                        &notif_queue,
                        &mut quit_requested,
                        &cmd,
                        &ipc,
                        ts,
                    );
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                if let Some(id) = shell.take_closed_window() {
                    let cmd = ass_ipc::Command::Close { id };
                    apply_command(
                        &mut server,
                        &notif_queue,
                        &mut quit_requested,
                        &cmd,
                        &ipc,
                        ts,
                    );
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                if let Some(id) = shell.take_move_requested() {
                    let cmd = ass_ipc::Command::Move { id };
                    apply_command(
                        &mut server,
                        &notif_queue,
                        &mut quit_requested,
                        &cmd,
                        &ipc,
                        ts,
                    );
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                for action in shell.take_window_actions() {
                    let cmd = match action {
                        ass_shell::WindowAction::Focus(id) => ass_ipc::Command::Focus { id },
                        ass_shell::WindowAction::Minimize(id) => ass_ipc::Command::Minimize { id },
                        ass_shell::WindowAction::Close(id) => ass_ipc::Command::Close { id },
                    };
                    apply_command(
                        &mut server,
                        &notif_queue,
                        &mut quit_requested,
                        &cmd,
                        &ipc,
                        ts,
                    );
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                if let Some(id) = shell.take_switch_workspace() {
                    let cmd = ass_ipc::Command::SwitchWorkspaceTo { id };
                    apply_command(
                        &mut server,
                        &notif_queue,
                        &mut quit_requested,
                        &cmd,
                        &ipc,
                        ts,
                    );
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                if let Some(id) = shell.take_dismissed_notification() {
                    let cmd = ass_ipc::Command::DismissNotification { id };
                    apply_command(
                        &mut server,
                        &notif_queue,
                        &mut quit_requested,
                        &cmd,
                        &ipc,
                        ts,
                    );
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                if let Some(app) = shell.take_open_builtin() {
                    shell.open_builtin(app);
                }
                let system_actions = shell.take_system_actions();
                if !system_actions.is_empty() {
                    for action in system_actions {
                        if let ass_shell::SystemAction::SetTouchpad(profile) = action {
                            if let Some(path) = config_path.as_deref() {
                                if let Err(error) = ass_config::set_touchpad_config(path, &profile)
                                {
                                    log::warn!(
                                        "touchpad: failed to persist settings to {}: {error}",
                                        path.display()
                                    );
                                }
                            } else {
                                log::warn!("touchpad: cannot persist settings; no config path");
                            }
                            if let Some(current) = config.as_mut() {
                                current.input.touchpad = profile;
                            }
                            system_status.touchpad = host.set_touchpad_config(profile);
                            continue;
                        }
                        if let Some(cmd) = apply_system_action(
                            &mut server,
                            &notif_queue,
                            &mut system_status,
                            action,
                        ) {
                            apply_command(
                                &mut server,
                                &notif_queue,
                                &mut quit_requested,
                                &cmd,
                                &ipc,
                                ts,
                            );
                            journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                        }
                    }
                    shell.set_system_status(system_status.clone());
                    // Reconcile optimistic hardware state right away: this
                    // path only fires on an explicit user action (volume,
                    // brightness, radios), so a one-off detect here is cheap
                    // and gives the HUD immediate feedback.
                    let mut detected = ass_shell::SystemStatus::detect();
                    detected.do_not_disturb = system_status.do_not_disturb;
                    detected.tiled = system_status.tiled;
                    detected.touchpad = host.touchpad_status();
                    system_status = detected;
                    shell.set_system_status(system_status.clone());
                }
                // The dock's Launchpad tile was clicked: toggle the launcher,
                // the same path as the Super-tap hotkey.
                if shell.take_toggle_launcher() {
                    shell.toggle();
                }
                // Dock context-menu pin/unpin requests: toggle the app in the
                // persisted `[dock] pinned` list, write the config back, and
                // refresh the dock immediately rather than waiting for the
                // live-reload watcher to notice the mtime change.
                let pin_toggles = shell.take_dock_pin_toggles();
                if !pin_toggles.is_empty() {
                    let mut pinned_list = config
                        .as_ref()
                        .map(|c| c.dock.pinned.clone())
                        .unwrap_or_default();
                    for id in pin_toggles {
                        let Some(entry) = launcher_apps.iter().find(|e| e.id == id) else {
                            continue;
                        };
                        // Match by application identity, not string equality:
                        // the config may name the same app by its stem, WM
                        // class, or icon name.
                        let keys = app_keys(entry);
                        let matches = |name: &String| keys.contains(&name.to_ascii_lowercase());
                        if pinned_list.iter().any(matches) {
                            pinned_list.retain(|name| !matches(name));
                        } else {
                            pinned_list.push(entry.id.clone());
                        }
                    }
                    if let Some(path) = config_path.as_deref() {
                        if let Err(e) = ass_config::set_dock_pinned(path, &pinned_list) {
                            log::warn!("dock: failed to persist pins: {e}");
                        }
                    } else {
                        log::warn!("dock: cannot persist pins; no config path");
                    }
                    if let Some(c) = config.as_mut() {
                        c.dock.pinned = pinned_list.clone();
                        c.dock.autopopulate = false;
                    }
                    let pinned =
                        build_dock_apps(&launcher_apps, &icon_cache.map, &pinned_list, false);
                    shell.update_app_catalog(&launcher_apps, &pinned, &icon_cache.map);
                }
                // Launch the application the launcher's clicked row asked for.
                // The child is detached (setsid) and inherits the Wayland/XDG
                // environment, so it connects back to this compositor and
                // survives it exiting. See ass-launch / ADR-0022.
                if let Some(entry) = shell.take_spawn() {
                    match ass_launch::launch(&entry, &ass_launch::LaunchOpts::default()) {
                        Ok(report) => {
                            log::info!("launcher: spawned {} (pid {})", entry.id, report.pid)
                        }
                        Err(e) => log::warn!("launcher: failed to spawn {}: {e}", entry.id),
                    }
                }
                // Apply keyboard-grab transitions the chrome requested this
                // frame (launcher opened or closed). Done after the intent
                // drains so a launcher "focus running app" action (which sets
                // a new keyboard focus) takes precedence over restoring the
                // pre-grab focus. The grab sends `wl_keyboard.leave` to the
                // focused client and the release sends `wl_keyboard.enter`
                // back, keeping the focused client's state consistent with the
                // capture decision. See ADR-0022.
                let captured = !session_locked && shell.captures_keyboard();
                if captured && !prev_captured {
                    server.grab_keyboard_focus();
                } else if !captured && prev_captured {
                    server.release_keyboard_focus();
                }
                prev_captured = captured;
                if host.uses_software_cursor() && !cursor_hidden {
                    draw_software_cursor(
                        &canvas,
                        &device,
                        &mut cursor_cache,
                        input_acc.cursor,
                        cursor_shape,
                        scale,
                    );
                }
                canvas.end();
                let mut capture_for_present = frame_capture.take().and_then(|(crop, target)| {
                    let readback = PendingReadback {
                        width: physical_size.0,
                        height: physical_size.1,
                        crop: crop.map(|rect| {
                            logical_rect_to_physical(
                                rect,
                                render_geometry.scale,
                                physical_size.0,
                                physical_size.1,
                            )
                        }),
                    };
                    match frame.request_readback() {
                        Ok(()) => Some(PendingCapture { readback, target }),
                        Err(error) => {
                            refuse_capture_target(
                                &capture_worker,
                                target,
                                format!(
                                    "frame readback request: {error}{}",
                                    flux_last_error_detail()
                                ),
                                &journal,
                                &ipc,
                            );
                            None
                        }
                    }
                });
                let submitted = match frame.submit() {
                    Ok(submitted) => submitted,
                    Err(error) => {
                        if let Some(capture) = capture_for_present.take() {
                            refuse_capture_target(
                                &capture_worker,
                                capture.target,
                                format!("captured frame submission failed: {error}"),
                                &journal,
                                &ipc,
                            );
                        }
                        return Err(error.into());
                    }
                };
                let completion_fence = match host.present(&surface, submitted) {
                    Ok(fence) => fence,
                    Err(error) => {
                        if let Some(capture) = capture_for_present.take() {
                            refuse_capture_target(
                                &capture_worker,
                                capture.target,
                                format!("captured frame was not presented: {error}"),
                                &journal,
                                &ipc,
                            );
                        }
                        // Transient direct-display conditions (VT switch,
                        // hotplug reconfigure, flip timeout): drop this frame
                        // and keep the session alive instead of exiting.
                        if matches!(
                            error,
                            HostError::Drm(
                                DrmError::FlipTimeout | DrmError::Inactive | DrmError::Reconfigured
                            )
                        ) {
                            log::warn!(
                                "{}: transient present failure; skipping frame: {error}",
                                host.name()
                            );
                            continue;
                        }
                        return Err(error.into());
                    }
                };
                if let Some(capture) = capture_for_present {
                    debug_assert!(pending_capture.is_none());
                    pending_capture = Some(capture);
                }
                if server.lock_confirmation_pending() {
                    match host.wait_presented(&device) {
                        Ok(()) => server.presentation_complete(),
                        Err(error) => log::error!(
                            "session lock: secure frame was not confirmed; keeping lock request pending: {error}"
                        ),
                    }
                }
                if server.retired_buffers_pending() {
                    if completion_fence.is_none() && !host.uses_software_cursor() {
                        // Nested swapchain presentation has no exportable
                        // completion fence. Rather than stalling the whole
                        // device on a wait_idle, release the buffers a few
                        // frames late: after more presented frames than
                        // flux's in-flight slots (3), the GPU can no longer
                        // reference their contents.
                        let since = retired_defer.get_or_insert(frame_count);
                        if frame_count.saturating_sub(*since) >= 4 {
                            server.release_retired_buffers(None);
                            retired_defer = None;
                        }
                    } else {
                        server.release_retired_buffers(
                            completion_fence.as_ref().map(AsRawFd::as_raw_fd),
                        );
                    }
                }

                // Pace clients: fire frame callbacks for this presentation.
                server.send_frame_callbacks(start.elapsed().as_millis() as u32);

                frame_count += 1;
                if frame_count == 1 {
                    log::info!("{}: first frame presented (with shell chrome)", host.name());
                }
            }
            Err(error) if error.0 == flux_sys::flux_result::FLUX_ERROR_TIMEOUT => {
                for (command, ts_mono_ms) in pending_screenshots.drain(..) {
                    journal_effect_and_broadcast(
                        &journal,
                        &ipc,
                        ts_mono_ms,
                        ass_ipc::Origin::Ipc { conn_id: 0 },
                        command,
                        ass_ipc::Effect::Refused {
                            reason: "output frame timed out before capture".to_owned(),
                        },
                    );
                }
                // The previous frame's GPU work did not retire inside the
                // frame timeout (or the presentation engine released no
                // swapchain image): transient. Skip this iteration and let
                // the next wakeup retry instead of rebuilding the
                // swapchain against a busy device.
                continue;
            }
            Err(_) => {
                for (command, ts_mono_ms) in pending_screenshots.drain(..) {
                    journal_effect_and_broadcast(
                        &journal,
                        &ipc,
                        ts_mono_ms,
                        ass_ipc::Origin::Ipc { conn_id: 0 },
                        command,
                        ass_ipc::Effect::Refused {
                            reason: "output changed before capture".to_owned(),
                        },
                    );
                }
                // Out-of-date / lost: rebuild the swapchain at the current
                // physical size.
                if let Some(capture) = pending_capture.take() {
                    refuse_capture_target(
                        &capture_worker,
                        capture.target,
                        "output changed before the captured frame became readable".to_owned(),
                        &journal,
                        &ipc,
                    );
                }
                let (nw, nh) = host.physical_size();
                surface.resize(nw, nh)?;
                if let Err(error) = surface.prepare_readback() {
                    log::warn!(
                        "capture: could not preallocate resized readback staging: {error}{}",
                        flux_last_error_detail()
                    );
                }
            }
        }

        // Decide whether the next iteration keeps ticking (animation in
        // flight) or blocks for the next host wakeup. Read after render so a
        // freshly-started wave (cursor just entered the dock band) is caught
        // the same frame it begins.
        animating = shell.anim_pending()
            || server.transitions_pending()
            || capture_worker.is_busy()
            || wallpaper
                .as_ref()
                .is_some_and(ass_wallpaper::Wallpaper::has_model);
    }

    log::info!(
        "ass: {} session ended after {frame_count} frames",
        host.name()
    );
    device.wait_idle();
    Ok(())
}

/// Resolve `--backend auto|drm|nested`, falling back to `$ASS_BACKEND` and
/// then `auto`. X11/XWayland are intentionally not accepted backends.
fn requested_backend() -> Result<BackendKind, Box<dyn std::error::Error>> {
    let mut selected = std::env::var("ASS_BACKEND").unwrap_or_else(|_| "auto".to_owned());
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        if let Some(value) = argument.strip_prefix("--backend=") {
            selected = value.to_owned();
        } else if argument == "--backend" {
            selected = args.next().ok_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "--backend requires auto, drm, or nested",
                )
            })?;
        } else if argument == "--help" || argument == "-h" {
            println!("Usage: ass [--backend auto|drm|nested]");
            std::process::exit(0);
        } else {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                format!("unknown option {argument:?}; try --help"),
            )
            .into());
        }
    }
    Ok(selected.parse()?)
}

/// `[[output]]` mode requests as the backend's connector → `ModeSpec` map
/// (ADR-0028). Entries without a `mode` keep the connector's preferred mode.
fn configured_output_modes(
    config: Option<&ass_config::Config>,
) -> std::collections::HashMap<String, ass_core::output::ModeSpec> {
    config
        .map(|c| c.output_policies())
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(connector, policy)| policy.mode.map(|mode| (connector, mode)))
        .collect()
}

/// Generate a timestamped screenshot filename inside `dir`, creating the
/// directory if it does not exist.
fn screenshot_path(dir: &std::path::Path) -> String {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let _ = std::fs::create_dir_all(dir);
    dir.join(format!("ass-{ms}.png"))
        .to_string_lossy()
        .into_owned()
}

/// Load the configuration from `path`, logging diagnostics on failure.
/// `None` (no path, or a file that does not exist) means "use built-in
/// defaults" and is not an error.
fn load_config(path: Option<&std::path::Path>) -> Option<ass_config::Config> {
    let path = path?;
    match ass_config::load(path) {
        Ok(Some(c)) => {
            log::info!("config: loaded {}", path.display());
            Some(c)
        }
        Ok(None) => None,
        Err(e) => {
            match &e {
                ass_config::LoadError::Invalid { diagnostics, .. } => {
                    for d in diagnostics {
                        log::warn!("config: {d}");
                    }
                }
                _ => log::warn!("config: {e}"),
            }
            log::warn!("config: using built-in defaults");
            None
        }
    }
}

/// Re-load `path` and, on success, swap in the new config and rebuild the
/// keymap. On failure, keep the previous config and keymap.
fn reload_config(
    path: &std::path::Path,
    config: &mut Option<ass_config::Config>,
    keymap: &mut ass_core::keybind::Keymap,
    server: &mut ass_server::Server,
    shell: &mut ass_shell::Shell,
    cursor_cache: &mut cursor::CursorCache,
) -> bool {
    let apply = |config: &Option<ass_config::Config>,
                 server: &mut ass_server::Server,
                 shell: &mut ass_shell::Shell,
                 cursor_cache: &mut cursor::CursorCache| {
        server.set_window_rules(
            config
                .as_ref()
                .map(|c| c.window_rules.clone())
                .unwrap_or_default(),
        );
        if let Some(c) = config.as_ref() {
            server.set_layout_params(c.layout.clone().into());
            server.set_tiling_default(c.layout.default_tiled);
            shell.set_reduced_motion(c.ui.reduced_motion);
            server.set_reduced_motion(c.ui.reduced_motion);
            cursor_cache.set_config(c.ui.cursor_theme.clone(), c.ui.cursor_size);
            server.set_output_policies(c.output_policies());
        } else {
            server.set_layout_params(ass_core::layout::LayoutParams::default());
            server.set_tiling_default(false);
            shell.set_reduced_motion(false);
            server.set_reduced_motion(false);
            cursor_cache.set_config(None, None);
            server.set_output_policies(std::collections::HashMap::new());
        }
    };
    match ass_config::load(path) {
        Ok(Some(new_cfg)) => {
            log::info!("config: reloaded {}", path.display());
            *config = Some(new_cfg);
            *keymap = build_keymap(config.as_ref());
            apply(config, server, shell, cursor_cache);
            true
        }
        Ok(None) => {
            log::warn!("config: {} removed; reverting to defaults", path.display());
            *config = None;
            *keymap = build_keymap(config.as_ref());
            apply(config, server, shell, cursor_cache);
            true
        }
        Err(e) => {
            match &e {
                ass_config::LoadError::Invalid { diagnostics, .. } => {
                    for d in diagnostics {
                        log::warn!("config: {d}");
                    }
                }
                _ => log::warn!("config: {e}"),
            }
            log::warn!("config: reload failed; keeping previous configuration");
            false
        }
    }
}

/// Build the nested output geometry from its logical surface size and the
/// host's preferred render scale. `wl_output.mode` is expressed in physical
/// pixels while xdg-output derives the original logical size by dividing by
/// `scale`; keeping both in one constructor prevents the two coordinate spaces
/// from silently drifting apart.
#[cfg(test)]
fn output_geometry_from_host(
    logical_w: i32,
    logical_h: i32,
    scale: f32,
) -> ass_core::output::OutputGeometry {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    ass_core::output::OutputGeometry {
        mode: ass_core::output::OutputMode {
            width: (logical_w.max(1) as f32 * scale).round() as i32,
            height: (logical_h.max(1) as f32 * scale).round() as i32,
            refresh_mhz: 0,
        },
        scale: ass_core::output::Scale(scale),
        transform: ass_core::Transform::Normal,
        logical_origin: ass_core::Point::default(),
    }
}

/// Build the active keymap from the config file's `[[keybind]]` entries,
/// layered over the built-in defaults. The deprecated `$ASS_KEYBINDS` env
/// var is honored as a transitional override that takes precedence over the
/// file (ADR-0026); it is logged and removed before the desktop phase
/// closes.
fn build_keymap(config: Option<&ass_config::Config>) -> ass_core::keybind::Keymap {
    let mut overrides: Vec<ass_core::keybind::Keybind> = Vec::new();

    // Deprecated env override — highest precedence so existing setups keep
    // working during the transition.
    if let Ok(s) = std::env::var("ASS_KEYBINDS") {
        if !s.trim().is_empty() {
            log::warn!(
                "keybind: $ASS_KEYBINDS is deprecated; move it to the \
                 `[[keybind]]` section of the config file"
            );
            let (env_binds, errs) = ass_core::keybind::Keymap::parse_overrides(&s);
            for e in &errs {
                log::warn!("keybind: {e}");
            }
            overrides.extend(env_binds);
        }
    }

    // Config-file overrides — below the env override.
    if let Some(cfg) = config {
        let (cfg_binds, errs) = cfg.resolve_keybinds();
        for e in &errs {
            log::warn!("config: {e}");
        }
        overrides.extend(cfg_binds);
    }

    if overrides.is_empty() {
        ass_core::keybind::Keymap::defaults()
    } else {
        log::info!("keybinds: {} override(s) applied", overrides.len());
        ass_core::keybind::Keymap::defaults().with_overrides(overrides)
    }
}

/// Compile the trusted named IPC scopes from configuration. Invalid operation
/// names are ignored inside an explicit allowlist (therefore granting nothing
/// for that entry) and logged; they never turn into an unrestricted scope.
fn build_ipc_scopes(
    config: Option<&ass_config::Config>,
) -> std::collections::HashMap<String, ass_ipc::Scope> {
    let mut scopes = std::collections::HashMap::new();
    let Some(config) = config else {
        return scopes;
    };

    for declared in &config.agent.scopes {
        let name = declared.name.trim();
        if name.is_empty() {
            log::warn!("config: ignoring agent scope with an empty name");
            continue;
        }
        if scopes.contains_key(name) {
            log::warn!("config: duplicate agent scope '{name}' ignored");
            continue;
        }

        let ops = if declared.ops.is_empty() {
            None
        } else {
            Some(
                declared
                    .ops
                    .iter()
                    .filter_map(|op| match ipc_op_class(op) {
                        Some(op) => Some(op),
                        None => {
                            log::warn!("config: agent scope '{name}' has unknown operation '{op}'");
                            None
                        }
                    })
                    .collect(),
            )
        };
        let windows = (!declared.windows.is_empty()).then(|| {
            declared
                .windows
                .iter()
                .copied()
                .map(ass_core::window::WindowId)
                .collect()
        });
        let workspaces = (!declared.workspaces.is_empty()).then(|| {
            declared
                .workspaces
                .iter()
                .copied()
                .map(ass_core::workspace::WorkspaceId)
                .collect()
        });
        scopes.insert(
            name.to_string(),
            ass_ipc::Scope {
                windows,
                workspaces,
                outputs: None,
                ops,
            },
        );
    }
    scopes
}

fn ipc_op_class(name: &str) -> Option<ass_ipc::OpClass> {
    use ass_ipc::OpClass;
    match name.trim().to_ascii_lowercase().as_str() {
        "focus" => Some(OpClass::Focus),
        "minimize" => Some(OpClass::Minimize),
        "close" => Some(OpClass::Close),
        "move" => Some(OpClass::Move),
        "setwindowgeometry" | "set_window_geometry" => Some(OpClass::SetWindowGeometry),
        "injectinput" | "inject_input" => Some(OpClass::InjectInput),
        "cycle" => Some(OpClass::Cycle),
        "switchworkspace" | "switch_workspace" => Some(OpClass::SwitchWorkspace),
        "switchworkspaceto" | "switch_workspace_to" => Some(OpClass::SwitchWorkspaceTo),
        "movetoworkspace" | "move_to_workspace" => Some(OpClass::MoveToWorkspace),
        "toggletiling" | "toggle_tiling" => Some(OpClass::ToggleTiling),
        "notify" => Some(OpClass::Notify),
        "dismissnotification" | "dismiss_notification" => Some(OpClass::DismissNotification),
        _ => None,
    }
}

/// Shared live window snapshot for the IPC (ADR-0027). The main loop writes
/// the same `Vec<Window>` it hands the shell; connection threads read it.
/// `query`-capability commands never mutate, so the lock is an `RwLock` and
/// reads from several connections do not block each other. `control`/
/// `session` commands arrive through [`ass_ipc::Handler::command`] and are forwarded
/// to the main loop via the channel the binary owns — the Wayland server
/// state is not `Send`, so connection threads must not touch it directly.
struct LiveState {
    windows: std::sync::RwLock<Vec<ass_core::window::Window>>,
    workspaces: std::sync::RwLock<ass_core::workspace::WorkspaceSnapshot>,
    outputs: std::sync::RwLock<Vec<ass_core::output::OutputInfo>>,
    notifications: std::sync::Arc<std::sync::Mutex<ass_core::notify::NotificationQueue>>,
    journal: std::sync::Arc<std::sync::Mutex<ass_ipc::Journal>>,
    commands: std::sync::Mutex<std::sync::mpsc::Sender<ass_ipc::Command>>,
    capture: std::sync::Mutex<std::sync::mpsc::Sender<CaptureRequest>>,
    scopes: std::sync::RwLock<std::collections::HashMap<String, ass_ipc::Scope>>,
}

impl LiveState {
    fn new(
        commands: std::sync::mpsc::Sender<ass_ipc::Command>,
        capture: std::sync::mpsc::Sender<CaptureRequest>,
        notifications: std::sync::Arc<std::sync::Mutex<ass_core::notify::NotificationQueue>>,
        journal: std::sync::Arc<std::sync::Mutex<ass_ipc::Journal>>,
        scopes: std::collections::HashMap<String, ass_ipc::Scope>,
    ) -> LiveState {
        LiveState {
            windows: std::sync::RwLock::new(Vec::new()),
            workspaces: std::sync::RwLock::new(
                ass_core::workspace::WorkspaceModel::new().snapshot(),
            ),
            outputs: std::sync::RwLock::new(Vec::new()),
            notifications,
            journal,
            commands: std::sync::Mutex::new(commands),
            capture: std::sync::Mutex::new(capture),
            scopes: std::sync::RwLock::new(scopes),
        }
    }

    fn set_windows(&self, windows: Vec<ass_core::window::Window>) {
        *self.windows.write().unwrap() = windows;
    }

    fn set_workspaces(&self, snapshot: ass_core::workspace::WorkspaceSnapshot) {
        *self.workspaces.write().unwrap() = snapshot;
    }

    fn set_outputs(&self, outputs: Vec<ass_core::output::OutputInfo>) {
        *self.outputs.write().unwrap() = outputs;
    }

    fn set_scopes(&self, scopes: std::collections::HashMap<String, ass_ipc::Scope>) {
        *self.scopes.write().unwrap() = scopes;
    }
}

impl ass_ipc::Handler for LiveState {
    /// The socket lives in `$XDG_RUNTIME_DIR` (user-only), so every local
    /// client is the user; grant all capabilities. The capability boundary
    /// becomes load-bearing for the M10 agent phase, where a scope narrows it.
    fn policy_caps(&self) -> ass_ipc::Capabilities {
        ass_ipc::Capabilities {
            query: true,
            control: true,
            input: true,
            session: true,
        }
    }

    fn windows(&self) -> Vec<ass_core::window::Window> {
        self.windows.read().unwrap().clone()
    }

    fn workspaces(&self) -> ass_core::workspace::WorkspaceSnapshot {
        self.workspaces.read().unwrap().clone()
    }

    fn notifications(&self) -> Vec<ass_core::notify::Notification> {
        self.notifications.lock().unwrap().snapshot()
    }

    fn outputs(&self) -> Vec<ass_core::output::OutputInfo> {
        self.outputs.read().unwrap().clone()
    }

    fn journal_since(&self, since: u64) -> ass_ipc::JournalSnapshot {
        self.journal.lock().unwrap().since(since)
    }

    fn command(&self, cmd: ass_ipc::Command) {
        // Best-effort: a send fails only if the main loop has dropped the
        // receiver (compositor shutting down); the command is then lost,
        // which is the right outcome.
        let _ = self.commands.lock().unwrap().send(cmd);
    }

    fn resolve_scope(&self, name: &str) -> Option<ass_ipc::Scope> {
        self.scopes.read().unwrap().get(name).cloned()
    }

    fn capture_output(&self, region: Option<ass_core::Rect>) -> Result<(u32, u32, String), String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.capture
            .lock()
            .unwrap()
            .send(CaptureRequest {
                reply: reply_tx,
                region,
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        // The main loop answers after the next frame; two seconds is far
        // beyond any frame budget and bounds a wedged-GPU stall.
        reply_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "capture timed out".to_owned())?
    }
}

/// Decoded application-icon textures for the dock. `_images` owns the GPU
/// textures; `map` keys raw pointers (borrowed from `_images`) by every
/// `app_id` the entry might run as. The cache must outlive the shell, which
/// holds clones of the pointers in its dock component.
struct IconCache {
    _images: Vec<flux::Image>,
    map: std::collections::HashMap<String, *mut std::ffi::c_void>,
}

/// Raster extensions the `image` crate decodes directly. SVG/SVGZ uses the
/// standard librsvg command-line rasterizer when installed and otherwise
/// falls back to the dock glyph without failing startup.
const RASTER_ICON_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tiff", "ico"];
const SVG_ICON_EXTS: &[&str] = &["svg", "svgz"];
const HUD_SYMBOLIC_ICON_NAMES: &[&str] = &[
    "audio-volume-muted-symbolic",
    "audio-volume-low-symbolic",
    "audio-volume-medium-symbolic",
    "audio-volume-high-symbolic",
    "network-wireless-signal-excellent-symbolic",
    "network-wired-symbolic",
    "network-offline-symbolic",
    "preferences-system-notifications-symbolic",
    "preferences-system-symbolic",
    "window-close-symbolic",
    "application-x-executable-symbolic",
];

/// Resolve the host's selected application icon theme. An explicit ass
/// override wins; otherwise query the GTK/GSettings desktop preference used
/// by niri and other toolkit-neutral Wayland sessions. `hicolor` remains the
/// portable fallback when GSettings is unavailable.
fn selected_icon_theme() -> String {
    if let Some(theme) = std::env::var("ASS_ICON_THEME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return theme;
    }

    let output = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "icon-theme"])
        .output();
    output
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|value| parse_gsettings_string(&value))
        .unwrap_or_else(|| ass_apps::DEFAULT_ICON_THEME.to_string())
}

/// Merge XDG applications with compositor-owned system applications. Built-in
/// entries deliberately use the same `Entry` model so launcher search,
/// context menus, pinning, and icon lookup have one catalog contract.
fn application_catalog(icon_theme: &str, icon_scale: u32) -> Vec<ass_core::app::Entry> {
    let mut applications = ass_apps::enumerate_with_theme_and_scale(icon_theme, icon_scale.max(1));
    let i18n = ass_shell::Localizer::from_env();
    applications.push(ass_core::app::Entry::control_center(
        i18n.text(ass_shell::Message::ControlCenter),
        i18n.text(ass_shell::Message::BuiltInSystemApp),
    ));
    applications
}

fn parse_gsettings_string(value: &str) -> Option<String> {
    let value = value.trim();
    let unquoted = value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
        .or_else(|| {
            value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
        })
        .unwrap_or(value)
        .trim();
    (!unquoted.is_empty()).then(|| unquoted.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IconFileStamp {
    len: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    device: u64,
    inode: u64,
}

/// Snapshot only icons the catalog actually uses. Metadata follows symlinks,
/// so a Flatpak `current/active` update is noticed even when the exported icon
/// path itself remains unchanged.
fn snapshot_icons(
    apps: &[ass_core::app::Entry],
) -> std::collections::BTreeMap<std::path::PathBuf, Option<IconFileStamp>> {
    use std::os::unix::fs::MetadataExt;

    let mut snapshot = std::collections::BTreeMap::new();
    for path in apps.iter().filter_map(|entry| entry.icon_path.as_ref()) {
        snapshot.entry(path.clone()).or_insert_with(|| {
            std::fs::metadata(path).ok().map(|metadata| IconFileStamp {
                len: metadata.len(),
                modified_seconds: metadata.mtime(),
                modified_nanoseconds: metadata.mtime_nsec(),
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        });
    }
    snapshot
}

/// The lowercased ids an entry might be matched by: its `StartupWMClass`, the
/// desktop-file stem, and the declared icon name. These are the same keys
/// [`build_icon_cache`] files icons under, so a dock tile can both find its
/// icon and fold a running toplevel (matched by `app_id`) into itself.
fn app_keys(entry: &ass_core::app::Entry) -> Vec<String> {
    let mut keys = Vec::new();
    let mut push = |s: &str| {
        let s = s.to_ascii_lowercase();
        if !s.is_empty() && !keys.contains(&s) {
            keys.push(s);
        }
    };
    if let Some(wm) = &entry.startup_wm_class {
        push(wm);
    }
    push(entry.id.strip_suffix(".desktop").unwrap_or(&entry.id));
    if let Some(ic) = &entry.icon {
        push(ic);
    }
    keys
}

/// How many apps to auto-pin to the dock when the config pins none, so the bar
/// is populated with real XDG icons out of the box rather than empty.
const DEFAULT_PINNED_MAX: usize = 12;

/// Build the dock's pinned app list. When `pinned` names apps, each name is
/// resolved against the enumerated entries by id / desktop-stem / WM class /
/// icon name (case-insensitive), in the order given; unresolved names are
/// logged and skipped. When `pinned` is empty and `autopopulate` is set, the
/// first [`DEFAULT_PINNED_MAX`] apps that have a decoded icon are pinned
/// automatically; with `autopopulate` off, an empty list stays empty (the
/// user's manual "no pins" choice).
fn build_dock_apps(
    apps: &[ass_core::app::Entry],
    icons: &std::collections::HashMap<String, *mut std::ffi::c_void>,
    pinned: &[String],
    autopopulate: bool,
) -> Vec<ass_shell::DockApp> {
    let make = |entry: &ass_core::app::Entry| ass_shell::DockApp {
        entry: entry.clone(),
        keys: app_keys(entry),
    };
    if pinned.is_empty() {
        if !autopopulate {
            return Vec::new();
        }
        return apps
            .iter()
            .filter(|e| app_keys(e).iter().any(|k| icons.contains_key(k)))
            .take(DEFAULT_PINNED_MAX)
            .map(make)
            .collect();
    }
    let mut out = Vec::with_capacity(pinned.len());
    for name in pinned {
        let want = name.to_ascii_lowercase();
        match apps.iter().find(|e| app_keys(e).contains(&want)) {
            Some(e) => out.push(make(e)),
            None => log::warn!("dock: pinned app '{name}' not found among enumerated entries"),
        }
    }
    out
}

/// Decode each app entry's icon into a flux texture, keyed by every id the
/// window might report as `app_id` (StartupWMClass, the desktop-id stem, and
/// the icon name, all lowercased). The first key to claim a texture wins per
/// entry, so a texture is never double-counted.
fn build_icon_cache(
    device: &flux::Device,
    apps: &[ass_core::app::Entry],
    icon_theme: &str,
    icon_scale: u32,
) -> IconCache {
    use std::ffi::c_void;
    let mut images: Vec<flux::Image> = Vec::new();
    let mut map: std::collections::HashMap<String, *mut c_void> = std::collections::HashMap::new();

    for entry in apps {
        let Some(path) = &entry.icon_path else {
            continue;
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        let Some(decoded) = decode_icon(path, &ext, icon_scale) else {
            continue;
        };
        let rgba = decoded.to_rgba8();
        let (w, h) = rgba.dimensions();
        let mut bgra = rgba.into_raw();
        for chunk in bgra.chunks_exact_mut(4) {
            chunk.swap(0, 2); // RGBA8 -> BGRA8 (flux samples BGRA8_UNORM).
        }
        match flux::Image::from_bytes(device, w, h, flux::Format::FLUX_FORMAT_BGRA8_UNORM, &bgra) {
            Ok(img) => {
                let ptr = img.as_raw() as *mut c_void;
                // Key the texture under every id a window might report as its
                // `app_id`; the dock resolves both icons and running-window
                // matches through these same keys.
                for key in app_keys(entry) {
                    map.entry(key).or_insert(ptr);
                }
                images.push(img);
            }
            Err(e) => log::warn!("icon: upload failed for {}: {e:?}", path.display()),
        }
    }

    // HUD status assets come from the same icon theme as applications. SVGs
    // are rasterized at output scale (and subsequently sampled down by lens),
    // avoiding the coarse single-pixel strokes of compositor glyphs while
    // retaining the host theme's silhouettes and proportions.
    let mut symbolic_names: Vec<String> = HUD_SYMBOLIC_ICON_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    for level in (0..=100).step_by(10) {
        symbolic_names.push(format!("battery-level-{level}-symbolic"));
        symbolic_names.push(format!("battery-level-{level}-charging-symbolic"));
    }
    let mut hud_count = 0usize;
    for name in symbolic_names {
        let Some(path) =
            ass_apps::resolve_icon_scaled(&name, Some(icon_theme), &[], 24, icon_scale.max(1))
        else {
            log::debug!("hud icon: '{name}' was not found in theme '{icon_theme}'");
            continue;
        };
        let ext = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let Some(decoded) = decode_icon(&path, &ext, icon_scale) else {
            continue;
        };
        let mut rgba = decoded.to_rgba8();
        // Symbolic themes commonly encode a dark CSS foreground intended for
        // toolkit recolouring. The compositor has no GTK style context, so
        // apply the HUD's light foreground while preserving every coverage
        // value produced by SVG antialiasing.
        for pixel in rgba.pixels_mut() {
            if pixel[3] != 0 {
                pixel[0] = 246;
                pixel[1] = 246;
                pixel[2] = 248;
            }
        }
        let (w, h) = rgba.dimensions();
        let mut bgra = rgba.into_raw();
        for chunk in bgra.chunks_exact_mut(4) {
            chunk.swap(0, 2);
        }
        match flux::Image::from_bytes(device, w, h, flux::Format::FLUX_FORMAT_BGRA8_UNORM, &bgra) {
            Ok(image) => {
                let ptr = image.as_raw() as *mut c_void;
                map.insert(format!("ass-hud:{name}"), ptr);
                if name == "preferences-system-symbolic" {
                    // Stable application-icon key for the compositor-owned
                    // control center entry and component header.
                    map.insert("ass-control-center".into(), ptr);
                }
                images.push(image);
                hud_count += 1;
            }
            Err(error) => log::warn!("hud icon: upload failed for {}: {error:?}", path.display()),
        }
    }

    log::info!(
        "icons: {} application texture(s), {hud_count} themed HUD symbol(s)",
        images.len().saturating_sub(hud_count)
    );
    IconCache {
        _images: images,
        map,
    }
}

/// Decode a desktop icon. Raster formats stay in-process; SVG is converted to
/// a bounded PNG on stdout so malformed or enormous vector sources cannot
/// dictate an unbounded GPU texture. Every failure is a normal glyph fallback.
fn decode_icon(path: &std::path::Path, ext: &str, icon_scale: u32) -> Option<image::DynamicImage> {
    if RASTER_ICON_EXTS.contains(&ext) {
        return image::open(path).ok();
    }
    if !SVG_ICON_EXTS.contains(&ext) {
        return None;
    }
    let target = ass_apps::DEFAULT_ICON_SIZE
        .saturating_mul(icon_scale.max(1))
        .min(512)
        .to_string();
    let output = std::process::Command::new("rsvg-convert")
        .args([
            "--width",
            &target,
            "--height",
            &target,
            "--keep-aspect-ratio",
        ])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        log::debug!("icon: SVG rasterization failed for {}", path.display());
        return None;
    }
    image::load_from_memory(&output.stdout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_output_geometry_preserves_logical_size_at_integer_scale() {
        let geometry = output_geometry_from_host(945, 924, 2.0);
        assert_eq!(geometry.mode.width, 1890);
        assert_eq!(geometry.mode.height, 1848);
        assert_eq!(geometry.scale, ass_core::output::Scale(2.0));
        assert_eq!(geometry.logical_size(), ass_core::Size { w: 945, h: 924 });
    }

    #[test]
    fn nested_output_geometry_preserves_logical_size_at_fractional_scale() {
        let geometry = output_geometry_from_host(945, 924, 1.5);
        assert_eq!(geometry.mode.width, 1418);
        assert_eq!(geometry.mode.height, 1386);
        assert_eq!(geometry.scale, ass_core::output::Scale(1.5));
        assert_eq!(geometry.logical_size(), ass_core::Size { w: 945, h: 924 });
    }

    #[test]
    fn logical_capture_region_scales_to_physical_pixels() {
        assert_eq!(
            logical_rect_to_physical(ass_core::Rect::new(10, 20, 100, 80), 2.0, 3840, 2160),
            ass_core::Rect::new(20, 40, 200, 160)
        );
    }

    #[test]
    fn logical_capture_region_scales_endpoints_and_clamps() {
        assert_eq!(
            logical_rect_to_physical(ass_core::Rect::new(-10, 10, 30, 20), 1.5, 30, 40),
            ass_core::Rect::new(0, 15, 30, 25)
        );
        assert_eq!(
            logical_rect_to_physical(ass_core::Rect::new(10, 20, 100, 80), 0.0, 200, 200),
            ass_core::Rect::new(10, 20, 100, 80)
        );
    }

    #[test]
    fn capture_encoding_crops_and_unpremultiplies_worker_payload() {
        let (width, height, png) = encode_rgba_capture(
            2,
            1,
            vec![10, 20, 30, 255, 50, 25, 0, 128],
            Some(ass_core::Rect::new(1, 0, 1, 1)),
        )
        .unwrap();
        assert_eq!((width, height), (1, 1));
        let decoded = image::load_from_memory(&png).unwrap().into_rgba8();
        assert_eq!(decoded.into_raw(), vec![100, 50, 0, 128]);
    }

    #[test]
    fn parses_gsettings_icon_theme_string() {
        assert_eq!(
            parse_gsettings_string("'Papirus-Dark'\n").as_deref(),
            Some("Papirus-Dark")
        );
        assert_eq!(
            parse_gsettings_string("\"Adwaita\"").as_deref(),
            Some("Adwaita")
        );
        assert_eq!(parse_gsettings_string("  "), None);
    }

    #[test]
    fn config_agent_scopes_compile_to_fail_closed_ipc_allowlists() {
        let config = ass_config::Config::parse(
            "schema_version = 1\n\
             [[agent.scope]]\n\
             name = \"focus-one\"\n\
             ops = [\"Focus\", \"NotARealOperation\"]\n\
             windows = [7]\n\
             workspaces = [3]\n",
        )
        .unwrap();
        let scopes = build_ipc_scopes(Some(&config));
        let scope = scopes.get("focus-one").expect("compiled scope");

        assert_eq!(scope.ops, Some(vec![ass_ipc::OpClass::Focus]));
        assert!(scope.permits(&ass_ipc::Command::Focus {
            id: ass_core::window::WindowId(7),
        }));
        assert!(!scope.permits(&ass_ipc::Command::Focus {
            id: ass_core::window::WindowId(8),
        }));
        assert!(!scope.permits(&ass_ipc::Command::Close {
            id: ass_core::window::WindowId(7),
        }));
    }

    #[test]
    fn automation_operation_names_accept_canonical_and_snake_case() {
        assert_eq!(
            ipc_op_class("SetWindowGeometry"),
            Some(ass_ipc::OpClass::SetWindowGeometry)
        );
        assert_eq!(
            ipc_op_class("set_window_geometry"),
            Some(ass_ipc::OpClass::SetWindowGeometry)
        );
        assert_eq!(
            ipc_op_class("inject_input"),
            Some(ass_ipc::OpClass::InjectInput)
        );
    }
}
