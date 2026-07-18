use super::*;

/// Immutable GPU readback staging detached from the presentation surface and
/// handed to the capture worker. `crop` is already converted to physical
/// pixels; the full CPU copy and every later operation stay off the
/// compositor's presentation-critical thread.
pub(super) struct CapturedPixels {
    width: u32,
    height: u32,
    readback: flux::Readback,
    crop: Option<ass_core::Rect>,
    security_generation: u64,
}

pub(super) struct PendingReadback {
    pub(super) width: u32,
    pub(super) height: u32,
    pub(super) crop: Option<ass_core::Rect>,
    pub(super) security_generation: u64,
}

pub(super) enum CaptureTarget {
    Screenshot {
        path: String,
        command: ass_ipc::Command,
        ts_mono_ms: u64,
        origin: ass_ipc::Origin,
    },
    Reply {
        reply: std::sync::mpsc::Sender<Result<ass_ipc::CaptureOutputPayload, String>>,
    },
    RealmReply {
        context: RealmCaptureContext,
        reply: std::sync::mpsc::Sender<Result<ass_ipc::CaptureRealmPayload, String>>,
    },
}

pub(super) struct PendingCapture {
    pub(super) readback: PendingReadback,
    pub(super) target: CaptureTarget,
}

pub(super) enum CaptureJob {
    Screenshot {
        capture: CapturedPixels,
        path: String,
        command: ass_ipc::Command,
        ts_mono_ms: u64,
        origin: ass_ipc::Origin,
    },
    Reply {
        capture: CapturedPixels,
        reply: std::sync::mpsc::Sender<Result<ass_ipc::CaptureOutputPayload, String>>,
    },
    RealmReply {
        capture: CapturedPixels,
        context: RealmCaptureContext,
        reply: std::sync::mpsc::Sender<Result<ass_ipc::CaptureRealmPayload, String>>,
    },
}

pub(super) enum CaptureCompletion {
    Screenshot {
        path: String,
        command: ass_ipc::Command,
        ts_mono_ms: u64,
        origin: ass_ipc::Origin,
        security_generation: u64,
        encoded: Result<Vec<u8>, String>,
    },
    Reply {
        reply: std::sync::mpsc::Sender<Result<ass_ipc::CaptureOutputPayload, String>>,
        security_generation: u64,
        encoded: Result<ass_ipc::CaptureOutputPayload, String>,
    },
    RealmReply {
        reply: std::sync::mpsc::Sender<Result<ass_ipc::CaptureRealmPayload, String>>,
        security_generation: u64,
        encoded: Result<ass_ipc::CaptureRealmPayload, String>,
    },
}

pub(super) fn read_captured_pixels(
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
        security_generation: pending.security_generation,
    })
}

/// Intersect a logical capture request with a virtual output without relying
/// on overflowing `i32` endpoint arithmetic.
pub(super) fn clamp_logical_region(
    rect: ass_core::Rect,
    width: u32,
    height: u32,
) -> Option<ass_core::Rect> {
    if rect.size.w <= 0 || rect.size.h <= 0 {
        return None;
    }
    let right = i64::from(rect.origin.x) + i64::from(rect.size.w);
    let bottom = i64::from(rect.origin.y) + i64::from(rect.size.h);
    let x0 = i64::from(rect.origin.x).clamp(0, i64::from(width));
    let y0 = i64::from(rect.origin.y).clamp(0, i64::from(height));
    let x1 = right.clamp(x0, i64::from(width));
    let y1 = bottom.clamp(y0, i64::from(height));
    (x1 > x0 && y1 > y0)
        .then(|| ass_core::Rect::new(x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32))
}

/// Convert a compositor-logical crop rectangle to physical output pixels.
///
/// Scaling both endpoints avoids accumulating a rounding error in the width
/// or height at fractional scales. The result is clamped to the readback
/// surface so regions partially outside the focused output remain safe.
pub(super) fn logical_rect_to_physical(
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
pub(super) fn crop_rgba(src: &[u8], src_w: u32, _src_h: u32, rect: ass_core::Rect) -> Vec<u8> {
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
pub(super) fn encode_capture(capture: CapturedPixels) -> Result<(u32, u32, Vec<u8>), String> {
    let mut full_rgba = vec![0u8; capture.width as usize * capture.height as usize * 4];
    capture
        .readback
        .read_pixels(&mut full_rgba)
        .map_err(|error| format!("shot pixel copy: {error}"))?;
    encode_rgba_capture(capture.width, capture.height, full_rgba, capture.crop)
}

pub(super) fn encode_rgba_capture(
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

pub(super) fn atomic_write_capture(path: &str, bytes: &[u8]) -> Result<(), String> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;

    let destination = std::path::Path::new(path);
    let parent = destination
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    let name = destination
        .file_name()
        .ok_or_else(|| format!("capture path {path:?} has no file name"))?
        .to_string_lossy();
    let nonce = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let temporary = parent.join(format!(".{name}.{}.{}.tmp", std::process::id(), nonce));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(&temporary)
            .map_err(|error| format!("create {}: {error}", temporary.display()))?;
        file.write_all(bytes)
            .map_err(|error| format!("write {}: {error}", temporary.display()))?;
        file.sync_all()
            .map_err(|error| format!("sync {}: {error}", temporary.display()))?;
        std::fs::rename(&temporary, destination).map_err(|error| {
            format!(
                "commit capture {} → {}: {error}",
                temporary.display(),
                destination.display()
            )
        })
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// Build a standards-compliant `text/uri-list` payload for a screenshot that
/// has already been committed to disk. Canonicalization makes relative
/// screenshot directories unambiguous to paste targets; percent encoding is
/// applied to the raw Unix path bytes so non-UTF-8 paths remain representable.
pub(super) fn screenshot_uri_list(path: &str) -> Result<Vec<u8>, String> {
    use std::os::unix::ffi::OsStrExt;

    let path = std::fs::canonicalize(path)
        .map_err(|error| format!("resolve screenshot URI for {path}: {error}"))?;
    let mut uri = Vec::with_capacity(path.as_os_str().as_bytes().len() * 3 + 10);
    uri.extend_from_slice(b"file://");
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &byte in path.as_os_str().as_bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~' | b'/') {
            uri.push(byte);
        } else {
            uri.extend_from_slice(&[b'%', HEX[(byte >> 4) as usize], HEX[(byte & 0x0f) as usize]]);
        }
    }
    uri.extend_from_slice(b"\r\n");
    Ok(uri)
}

/// Single bounded post-processing lane for screenshots and IPC pixel
/// captures. Only one full-frame payload may be in flight, which keeps
/// repeated requests from consuming unbounded memory or compounding stalls.
pub(super) struct CaptureWorker {
    jobs: std::sync::mpsc::Sender<CaptureJob>,
    pub(super) completions: std::sync::mpsc::Receiver<CaptureCompletion>,
    busy: std::sync::Arc<std::sync::atomic::AtomicBool>,
    allowed: std::sync::Arc<std::sync::atomic::AtomicBool>,
    security_generation: std::sync::Arc<std::sync::atomic::AtomicU64>,
}

impl CaptureWorker {
    pub(super) fn spawn() -> std::io::Result<Self> {
        let (job_tx, job_rx) = std::sync::mpsc::channel::<CaptureJob>();
        let (completion_tx, completion_rx) = std::sync::mpsc::channel::<CaptureCompletion>();
        let busy = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_busy = std::sync::Arc::clone(&busy);
        let allowed = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true));
        let worker_allowed = std::sync::Arc::clone(&allowed);
        let security_generation = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(1));
        let worker_security_generation = std::sync::Arc::clone(&security_generation);
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
                            origin,
                        } => {
                            let generation = capture.security_generation;
                            let encoded = if worker_allowed
                                .load(std::sync::atomic::Ordering::Acquire)
                                && generation
                                    == worker_security_generation
                                        .load(std::sync::atomic::Ordering::Acquire)
                            {
                                encode_capture(capture).map(|(_, _, png)| png)
                            } else {
                                Err("session locked before capture completed".into())
                            };
                            if completion_tx
                                .send(CaptureCompletion::Screenshot {
                                    path,
                                    command,
                                    ts_mono_ms,
                                    origin,
                                    security_generation: generation,
                                    encoded,
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
                            let generation = capture.security_generation;
                            let encoded = if worker_allowed
                                .load(std::sync::atomic::Ordering::Acquire)
                                && generation
                                    == worker_security_generation
                                        .load(std::sync::atomic::Ordering::Acquire)
                            {
                                encode_capture(capture).map(|(width, height, png)| {
                                    ass_ipc::CaptureOutputPayload { width, height, png }
                                })
                            } else {
                                Err("session locked before capture completed".into())
                            };
                            if completion_tx
                                .send(CaptureCompletion::Reply {
                                    reply,
                                    security_generation: generation,
                                    encoded,
                                })
                                .is_err()
                            {
                                worker_busy.store(false, std::sync::atomic::Ordering::Release);
                                break;
                            }
                        }
                        CaptureJob::RealmReply {
                            capture,
                            context,
                            reply,
                        } => {
                            let generation = capture.security_generation;
                            let encoded = if worker_allowed
                                .load(std::sync::atomic::Ordering::Acquire)
                                && generation
                                    == worker_security_generation
                                        .load(std::sync::atomic::Ordering::Acquire)
                            {
                                encode_capture(capture).map(|(width, height, png)| {
                                    ass_ipc::CaptureRealmPayload {
                                        capture: ass_ipc::RealmCapture {
                                            realm: context.realm,
                                            width,
                                            height,
                                            scale_milli: context.scale_milli,
                                            region: context.region,
                                            placements: context.placements,
                                            png_bytes: png.len() as u64,
                                            revision: context.revision,
                                        },
                                        png,
                                    }
                                })
                            } else {
                                Err("session locked before Realm capture completed".into())
                            };
                            if completion_tx
                                .send(CaptureCompletion::RealmReply {
                                    reply,
                                    security_generation: generation,
                                    encoded,
                                })
                                .is_err()
                            {
                                worker_busy.store(false, std::sync::atomic::Ordering::Release);
                                break;
                            }
                        }
                    }
                }
                worker_busy.store(false, std::sync::atomic::Ordering::Release);
            })?;
        Ok(Self {
            jobs: job_tx,
            completions: completion_rx,
            busy,
            allowed,
            security_generation,
        })
    }

    pub(super) fn reserve(&self) -> bool {
        self.busy
            .compare_exchange(
                false,
                true,
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
            )
            .is_ok()
    }

    pub(super) fn release(&self) {
        self.busy.store(false, std::sync::atomic::Ordering::Release);
    }

    pub(super) fn is_busy(&self) -> bool {
        self.busy.load(std::sync::atomic::Ordering::Acquire)
    }

    pub(super) fn delivery_gate(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        std::sync::Arc::clone(&self.allowed)
    }

    pub(super) fn set_allowed(&self, allowed: bool) {
        let was_allowed = self
            .allowed
            .swap(allowed, std::sync::atomic::Ordering::AcqRel);
        if was_allowed && !allowed {
            self.security_generation
                .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
        }
    }

    pub(super) fn security_generation(&self) -> u64 {
        self.security_generation
            .load(std::sync::atomic::Ordering::Acquire)
    }

    pub(super) fn permits(&self, security_generation: u64) -> bool {
        self.allowed.load(std::sync::atomic::Ordering::Acquire)
            && self.security_generation() == security_generation
    }

    pub(super) fn invalidate_security_context(&self) {
        self.security_generation
            .fetch_add(1, std::sync::atomic::Ordering::AcqRel);
    }

    pub(super) fn submit(&self, job: CaptureJob) -> Result<(), Box<CaptureJob>> {
        self.jobs.send(job).map_err(|error| Box::new(error.0))
    }
}

pub(super) fn refuse_capture_target(
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
            origin,
            ..
        } => journal_effect_and_broadcast(
            journal,
            ipc,
            ts_mono_ms,
            origin,
            command,
            ass_ipc::Effect::Refused { reason },
        ),
        CaptureTarget::Reply { reply } => {
            let _ = reply.send(Err(reason));
        }
        CaptureTarget::RealmReply { reply, .. } => {
            let _ = reply.send(Err(reason));
        }
    }
}

pub(super) fn queue_captured_pixels(
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
            origin,
        } => CaptureJob::Screenshot {
            capture,
            path,
            command,
            ts_mono_ms,
            origin,
        },
        CaptureTarget::Reply { reply } => CaptureJob::Reply { capture, reply },
        CaptureTarget::RealmReply { context, reply } => CaptureJob::RealmReply {
            capture,
            context,
            reply,
        },
    };
    if let Err(job) = worker.submit(job) {
        let target = match *job {
            CaptureJob::Screenshot {
                path,
                command,
                ts_mono_ms,
                origin,
                ..
            } => CaptureTarget::Screenshot {
                path,
                command,
                ts_mono_ms,
                origin,
            },
            CaptureJob::Reply { reply, .. } => CaptureTarget::Reply { reply },
            CaptureJob::RealmReply { context, reply, .. } => {
                CaptureTarget::RealmReply { context, reply }
            }
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
pub(super) fn unpremultiply(pixels: &mut [u8]) {
    for px in pixels.chunks_exact_mut(4) {
        let a = u32::from(px[3]);
        if a > 0 && a < 255 {
            for channel in &mut px[0..3] {
                *channel = ((u32::from(*channel) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
}

pub(super) fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|e| format!("png encode: {e}"))?;
    Ok(out)
}

/// flux's thread-local diagnostic for the most recent error, formatted for
/// logs; empty when the call carried no detail.
pub(super) fn flux_last_error_detail() -> String {
    let mut info: flux_sys::flux_error_info = unsafe { std::mem::zeroed() };
    unsafe { flux_sys::flux_get_last_error(&mut info) };
    if info.message.is_null() {
        return String::new();
    }
    let message = unsafe { std::ffi::CStr::from_ptr(info.message) };
    format!(" ({})", message.to_string_lossy())
}
