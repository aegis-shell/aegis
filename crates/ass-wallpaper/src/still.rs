//! Still and animated image sources, decoded via the `image` crate.
//!
//! Each emitted frame is tightly packed BGRA8 at the source's full canvas
//! size. Animated GIF/WebP frames that cover only a sub-rect of the
//! canvas are composited onto the previous frame's contents during
//! decode, so consumers always see uniformly-sized buffers and can pass
//! them straight to `flux::Image::from_bytes`.

use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::{Duration, Instant};

use image::codecs::gif::GifDecoder;
use image::codecs::webp::WebPDecoder;
use image::{AnimationDecoder, ImageDecoder};

use crate::{Error, Source};

/// Effectively "display forever"; used for single-frame sources so the
/// advance loop in `poll` never moves past them.
const FOREVER: Duration = Duration::from_secs(60 * 60 * 24 * 365);

/// In-memory still or animated image.
pub(super) struct StillSource {
    /// Full-canvas BGRA8 frames. Static images have exactly one entry.
    frames: Vec<Frame>,
    width: u32,
    height: u32,
    /// Index of the frame currently on display.
    current: usize,
    /// Wall-clock time when `current` was advanced.
    last_advance: Instant,
    /// Bumped each time `current` changes, so the wrapper can re-upload.
    r#gen: u64,
}

struct Frame {
    /// Tightly packed BGRA8, `width * height * 4` bytes.
    pixels: Vec<u8>,
    /// How long this frame should be shown before advancing.
    duration: Duration,
}

impl StillSource {
    pub(super) fn transparent_pixel() -> Self {
        StillSource {
            frames: vec![Frame {
                pixels: vec![0, 0, 0, 0],
                duration: FOREVER,
            }],
            width: 1,
            height: 1,
            current: 0,
            last_advance: Instant::now(),
            r#gen: 0,
        }
    }

    pub(super) fn load(path: &Path) -> Result<Self, Error> {
        let format = image::ImageReader::open(path)
            .map_err(|e| Error::Open(path.to_path_buf(), e))?
            .with_guessed_format()
            .map_err(|e| Error::Open(path.to_path_buf(), e))?
            .format();

        match format {
            Some(image::ImageFormat::Gif) => Self::load_gif(path),
            Some(image::ImageFormat::WebP) => Self::load_webp(path),
            _ => Self::load_static(path),
        }
    }

    fn load_static(path: &Path) -> Result<Self, Error> {
        let img = image::ImageReader::open(path)
            .map_err(|e| Error::Open(path.to_path_buf(), e))?
            .with_guessed_format()
            .map_err(|e| Error::Open(path.to_path_buf(), e))?
            .decode()
            .map_err(|e| Error::Decode(path.to_path_buf(), e.to_string()))?;
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        let mut pixels = rgba.into_raw();
        rgba_to_bgra_inplace(&mut pixels);
        Ok(StillSource {
            frames: vec![Frame {
                pixels,
                duration: FOREVER,
            }],
            width: w,
            height: h,
            current: 0,
            last_advance: Instant::now(),
            r#gen: 0,
        })
    }

    fn load_gif(path: &Path) -> Result<Self, Error> {
        let file = File::open(path).map_err(|e| Error::Open(path.to_path_buf(), e))?;
        let dec = GifDecoder::new(BufReader::new(file))
            .map_err(|e| Error::Decode(path.to_path_buf(), e.to_string()))?;
        let (w, h) = dec.dimensions();
        let raw = dec
            .into_frames()
            .collect_frames()
            .map_err(|e| Error::Decode(path.to_path_buf(), e.to_string()))?;
        Self::from_animation(path, &raw, w, h, "Gif")
    }

    fn load_webp(path: &Path) -> Result<Self, Error> {
        let file = File::open(path).map_err(|e| Error::Open(path.to_path_buf(), e))?;
        let dec = WebPDecoder::new(BufReader::new(file))
            .map_err(|e| Error::Decode(path.to_path_buf(), e.to_string()))?;
        if !dec.has_animation() {
            return Self::load_static(path);
        }
        let (w, h) = dec.dimensions();
        let raw = dec
            .into_frames()
            .collect_frames()
            .map_err(|e| Error::Decode(path.to_path_buf(), e.to_string()))?;
        Self::from_animation(path, &raw, w, h, "WebP")
    }

    fn from_animation(
        path: &Path,
        raw: &[image::Frame],
        canvas_w: u32,
        canvas_h: u32,
        label: &str,
    ) -> Result<Self, Error> {
        if raw.is_empty() {
            return Err(Error::UnsupportedFormat(
                path.to_path_buf(),
                Some(label.to_string()),
            ));
        }
        let frames = composite_frames(raw, canvas_w, canvas_h);
        Ok(StillSource {
            frames,
            width: canvas_w,
            height: canvas_h,
            current: 0,
            last_advance: Instant::now(),
            r#gen: 0,
        })
    }
}

/// Composite each animation frame onto the source's full canvas. GIF and
/// animated WebP encode only the region that changed; we accumulate so
/// every emitted frame is a complete image. Alpha is composited with
/// "over" against the existing canvas contents.
fn composite_frames(raw: &[image::Frame], canvas_w: u32, canvas_h: u32) -> Vec<Frame> {
    let canvas_bytes = (canvas_w as usize) * (canvas_h as usize) * 4;
    let mut accum: Vec<u8> = vec![0u8; canvas_bytes];
    let mut out = Vec::with_capacity(raw.len());

    for f in raw {
        let buf = f.buffer();
        let fw = buf.width();
        let fh = buf.height();
        let left = f.left();
        let top = f.top();
        let src = buf.as_raw();

        for y in 0..fh {
            for x in 0..fw {
                let src_off = ((y * fw + x) * 4) as usize;
                if src_off + 4 > src.len() {
                    continue;
                }
                let r = src[src_off];
                let g = src[src_off + 1];
                let b = src[src_off + 2];
                let a = src[src_off + 3];
                if a == 0 {
                    continue;
                }
                let cx = left as usize + x as usize;
                let cy = top as usize + y as usize;
                if cx >= canvas_w as usize || cy >= canvas_h as usize {
                    continue;
                }
                let dst_off = (cy * canvas_w as usize + cx) * 4;
                if a == 255 {
                    accum[dst_off] = b;
                    accum[dst_off + 1] = g;
                    accum[dst_off + 2] = r;
                    accum[dst_off + 3] = 255;
                } else {
                    let alpha = a as u32;
                    let inv = 255 - alpha;
                    let cb = accum[dst_off] as u32;
                    let cg = accum[dst_off + 1] as u32;
                    let cr = accum[dst_off + 2] as u32;
                    let ca = accum[dst_off + 3] as u32;
                    accum[dst_off] = ((b as u32 * alpha + cb * inv) / 255) as u8;
                    accum[dst_off + 1] = ((g as u32 * alpha + cg * inv) / 255) as u8;
                    accum[dst_off + 2] = ((r as u32 * alpha + cr * inv) / 255) as u8;
                    accum[dst_off + 3] = ((alpha + ca * inv) / 255) as u8;
                }
            }
        }

        let duration = frame_duration(f);
        out.push(Frame {
            pixels: accum.clone(),
            duration,
        });
    }

    out
}

fn frame_duration(f: &image::Frame) -> Duration {
    // `Delay::numer_denom_ms` exposes the frame's duration as a
    // millisecond ratio. Convert to nanoseconds for sub-ms fidelity,
    // clamped to a 60fps minimum so malformed zero-delay frames don't
    // spin the advance loop.
    let (numer, denom) = f.delay().numer_denom_ms();
    let nanos = if denom == 0 {
        100_000_000
    } else {
        (numer as u64 * 1_000_000) / denom as u64
    };
    Duration::from_nanos(nanos).max(Duration::from_millis(16))
}

/// Swap R and B in tightly packed RGBA8 to produce BGRA8 in place.
fn rgba_to_bgra_inplace(buf: &mut [u8]) {
    for chunk in buf.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }
}

impl Source for StillSource {
    fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    fn poll(&mut self, now: Instant) -> (&[u8], u64) {
        if self.frames.len() > 1 {
            // Advance zero or more times if we have fallen behind (e.g.
            // after a compositor stall). Each iteration consumes exactly
            // one frame's duration from the wall-clock debt.
            while now.duration_since(self.last_advance) >= self.frames[self.current].duration {
                self.last_advance += self.frames[self.current].duration;
                self.current = (self.current + 1) % self.frames.len();
                self.r#gen = self.r#gen.wrapping_add(1);
            }
        }
        (&self.frames[self.current].pixels, self.r#gen)
    }

    fn frame_count(&self) -> usize {
        self.frames.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    fn write_png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
        let img = image::RgbaImage::from_raw(w, h, rgba.to_vec()).unwrap();
        let mut buf = Cursor::new(Vec::new());
        image::DynamicImage::from(img)
            .write_to(&mut buf, image::ImageFormat::Png)
            .unwrap();
        buf.into_inner()
    }

    #[test]
    fn rgba_to_bgra_swaps_channels() {
        let mut buf = [10u8, 20, 30, 40];
        rgba_to_bgra_inplace(&mut buf);
        assert_eq!(buf, [30, 20, 10, 40]);
    }

    #[test]
    fn static_png_loads_one_frame_at_native_size() {
        let png = write_png(
            4,
            2,
            &[
                255, 0, 0, 255, 0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255, 255, 0, 0, 255,
                0, 255, 0, 255, 0, 0, 255, 255, 255, 255, 0, 255,
            ],
        );
        let dir = tempfile_dir();
        let path = dir.join("static.png");
        std::fs::write(&path, &png).unwrap();
        let s = StillSource::load(&path).expect("decode png");
        assert_eq!(s.dimensions(), (4, 2));
        assert_eq!(s.frame_count(), 1);
        let frame = &s.frames[0].pixels;
        assert_eq!(frame.len(), 4 * 2 * 4);
        // Red pixel (255,0,0,255) in RGBA → BGRA byte order (0,0,255,255).
        assert_eq!(&frame[0..4], &[0, 0, 255, 255]);
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ass-wallpaper-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
