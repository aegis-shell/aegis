//! Video wallpaper source backed by an external `ffmpeg` child process.
//!
//! `ffmpeg` decodes the source, scales to the requested resolution via
//! `-vf fps=...,scale=...`, and writes raw tightly-packed BGRA8 frames
//! to stdout (one `width * height * 4`-byte frame per source-tick). We
//! spawn it once at load and pull the first frame synchronously so
//! callers always have valid pixels on return from `open`.
//!
//! A background reader thread then keeps the most recently decoded frame
//! in a shared slot; the compositor's main thread reads it non-blocking
//! on each `poll`. When `ffmpeg` reaches EOF (short video), the reader
//! respawns it so playback loops. `Drop` flips a shutdown flag that the
//! reader checks between frames; the next ffmpeg exit ends the thread.

use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::{Error, Source};

/// Target frames-per-second fed to `ffmpeg`'s `-vf fps=`. Lowering this
/// saves CPU and memory bandwidth at the cost of smoothness; 24 fps
/// matches cinema and is well below typical display refresh rates.
const VIDEO_FPS: u32 = 24;

/// How long `open` waits for the first frame before declaring ffmpeg
/// unusable for this source.
const FIRST_FRAME_TIMEOUT: Duration = Duration::from_secs(5);

/// Idle poll interval while waiting for the first frame.
const FIRST_FRAME_POLL: Duration = Duration::from_millis(20);

/// Shared state between the reader thread and the consumer.
struct Slot {
    /// Most recently decoded frame, or `None` until the first arrives.
    pixels: Option<Vec<u8>>,
    /// Bumped on every successful publish so consumers can detect
    /// "new since last I looked" without comparing bytes.
    seq: u64,
}

pub(super) struct VideoSource {
    width: u32,
    height: u32,
    slot: Arc<Mutex<Slot>>,
    shutdown: Arc<AtomicBool>,
    /// Keeps the reader thread alive for the lifetime of this source.
    _reader: thread::JoinHandle<()>,
    /// Local copy of the most recently observed frame; updated in `poll`.
    current: Vec<u8>,
    /// `Slot::seq` value already copied into `current`.
    last_seen_seq: u64,
    /// Bumped each time `current` is refreshed.
    r#gen: u64,
}

impl VideoSource {
    /// Spawn ffmpeg pointing at `path`, scaled to `target_w`×`target_h`,
    /// and block until the first frame lands in the slot.
    pub(super) fn open(path: &Path, target_w: u32, target_h: u32) -> Result<Self, Error> {
        if target_w == 0 || target_h == 0 {
            return Err(Error::UnsupportedFormat(path.to_path_buf(), None));
        }
        let slot = Arc::new(Mutex::new(Slot {
            pixels: None,
            seq: 0,
        }));
        let shutdown = Arc::new(AtomicBool::new(false));

        let reader = thread::Builder::new()
            .name("ass-wallpaper-video".into())
            .spawn({
                let path = path.to_path_buf();
                let slot = slot.clone();
                let shutdown = shutdown.clone();
                move || reader_loop(path, target_w, target_h, slot, shutdown)
            })
            .map_err(Error::FfmpegSpawn)?;

        // Wait for the first frame so callers have valid pixels on return.
        let deadline = Instant::now() + FIRST_FRAME_TIMEOUT;
        loop {
            {
                let s = slot.lock().unwrap();
                if s.pixels.is_some() {
                    break;
                }
            }
            if Instant::now() >= deadline {
                shutdown.store(true, Ordering::Release);
                return Err(Error::FfmpegEmpty(path.to_path_buf()));
            }
            thread::sleep(FIRST_FRAME_POLL);
        }

        Ok(VideoSource {
            width: target_w,
            height: target_h,
            slot,
            shutdown,
            _reader: reader,
            current: Vec::new(),
            last_seen_seq: 0,
            r#gen: 0,
        })
    }
}

impl Drop for VideoSource {
    fn drop(&mut self) {
        // The reader checks this between full frames; the next ffmpeg
        // exit (or the spawn path) observes it and returns.
        self.shutdown.store(true, Ordering::Release);
    }
}

fn reader_loop(path: PathBuf, w: u32, h: u32, slot: Arc<Mutex<Slot>>, shutdown: Arc<AtomicBool>) {
    let frame_size = (w as usize) * (h as usize) * 4;
    let mut buf = vec![0u8; frame_size];

    while !shutdown.load(Ordering::Acquire) {
        let mut child = match spawn_ffmpeg(&path, w, h) {
            Ok(c) => c,
            Err(e) => {
                log::error!("wallpaper: ffmpeg spawn failed: {e}");
                // Avoid a tight respawn loop if ffmpeg is missing or
                // permanently rejecting this input.
                thread::sleep(Duration::from_secs(2));
                continue;
            }
        };
        let mut stdout = child.stdout.take().expect("piped stdout configured");

        while !shutdown.load(Ordering::Acquire) {
            match read_exact_or_eof(&mut stdout, &mut buf) {
                Ok(true) => {
                    let mut s = slot.lock().unwrap();
                    s.pixels = Some(buf.clone());
                    s.seq = s.seq.wrapping_add(1);
                }
                Ok(false) => break, // EOF — outer loop respawns.
                Err(e) => {
                    log::warn!("wallpaper: ffmpeg read error: {e}");
                    break;
                }
            }
        }

        if shutdown.load(Ordering::Acquire) {
            let _ = child.kill();
            let _ = child.wait();
            return;
        }
        let _ = child.wait();
    }
}

fn spawn_ffmpeg(path: &Path, w: u32, h: u32) -> Result<std::process::Child, std::io::Error> {
    let vf = format!("fps={VIDEO_FPS},scale={w}:{h}:flags=lanczos");
    Command::new("ffmpeg")
        .arg("-loglevel")
        .arg("error")
        .arg("-i")
        .arg(path)
        .arg("-vf")
        .arg(&vf)
        .arg("-f")
        .arg("rawvideo")
        .arg("-pix_fmt")
        .arg("bgra")
        .arg("-an")
        .arg("-")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
}

/// Read exactly `buf.len()` bytes, or report clean EOF before any byte
/// of the next frame was seen. Partial reads accumulate across calls.
fn read_exact_or_eof(r: &mut impl Read, buf: &mut [u8]) -> std::io::Result<bool> {
    let mut filled = 0;
    while filled < buf.len() {
        let n = r.read(&mut buf[filled..])?;
        if n == 0 {
            return Ok(false);
        }
        filled += n;
    }
    Ok(true)
}

impl Source for VideoSource {
    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn poll(&mut self, _now: Instant) -> (&[u8], u64) {
        let newer: Option<Vec<u8>> = {
            let s = self.slot.lock().unwrap();
            if s.seq != self.last_seen_seq {
                self.last_seen_seq = s.seq;
                s.pixels.clone()
            } else {
                None
            }
        };
        if let Some(p) = newer {
            self.current.clear();
            self.current.extend_from_slice(&p);
            self.r#gen = self.r#gen.wrapping_add(1);
        }
        (&self.current, self.r#gen)
    }

    fn frame_count(&self) -> usize {
        0
    }
}
