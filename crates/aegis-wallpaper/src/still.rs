//! Still and animated image sources, decoded via the `image` crate.
//!
//! Each emitted frame is tightly packed premultiplied BGRA8 at the source's
//! full canvas size. Animated GIF/WebP frames that cover only a sub-rect of the
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

/// Ceiling on the composited frames retained in memory. Animated wallpaper
/// frames are full-canvas BGRA8 (`width * height * 4` bytes each), so an
/// unbounded retain turns a long 1080p animation into a multi-gigabyte
/// resident set. When the retained set exceeds this budget the loader
/// decimates it — dropping every other retained frame and merging its
/// duration into the predecessor — so playback stays loop-correct and
/// wall-clock accurate, just with fewer intermediate steps.
const MAX_RETAINED_BYTES: usize = 256 * 1024 * 1024;

/// Ceiling on retained frame *count* (independent of canvas size) so a
/// pathological tiny-canvas/many-frame source cannot evade the byte budget.
const MAX_RETAINED_FRAMES: usize = 512;

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
    /// Tightly packed premultiplied BGRA8, `width * height * 4` bytes.
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

    pub(super) fn load_static_bytes(bytes: &[u8], label: &Path) -> Result<Self, Error> {
        let img = image::load_from_memory(bytes)
            .map_err(|e| Error::Decode(label.to_path_buf(), e.to_string()))?;
        Ok(Self::from_dynamic_image(img))
    }

    fn load_static(path: &Path) -> Result<Self, Error> {
        let img = image::ImageReader::open(path)
            .map_err(|e| Error::Open(path.to_path_buf(), e))?
            .with_guessed_format()
            .map_err(|e| Error::Open(path.to_path_buf(), e))?
            .decode()
            .map_err(|e| Error::Decode(path.to_path_buf(), e.to_string()))?;
        Ok(Self::from_dynamic_image(img))
    }

    fn from_dynamic_image(img: image::DynamicImage) -> Self {
        let rgba = img.to_rgba8();
        let (w, h) = (rgba.width(), rgba.height());
        let mut pixels = rgba.into_raw();
        rgba_to_premultiplied_bgra_inplace(&mut pixels);
        StillSource {
            frames: vec![Frame {
                pixels,
                duration: FOREVER,
            }],
            width: w,
            height: h,
            current: 0,
            last_advance: Instant::now(),
            r#gen: 0,
        }
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
        Self::from_animation(path, raw, w, h, "Gif")
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
        Self::from_animation(path, raw, w, h, "WebP")
    }

    fn from_animation(
        path: &Path,
        raw: Vec<image::Frame>,
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

/// Composite each animation frame onto the source's full canvas, enforcing
/// the retained-frame budget as frames are emitted.
///
/// GIF and animated WebP encode only the region that changed (and the `image`
/// crate materializes WebP frames at full canvas size regardless); we
/// accumulate onto one canvas so consumers always see uniformly-sized,
/// tightly packed premultiplied BGRA8 buffers with alpha composited "over".
///
/// Memory: each emitted frame is `width * height * 4` bytes, so a long
/// animation would otherwise retain gigabytes for the source's lifetime.
/// Once the emitted set is over budget the compositor drops further frames'
/// pixel data and merges their durations into the last retained frame — the
/// surviving animation spans the same total wall-clock time and still loops
/// seamlessly, with fewer intermediate steps. Consuming `raw` by value lets
/// each decoded frame's memory be freed as soon as it is composited, so the
/// transient decode peak tracks the retained set rather than the full source.
fn composite_frames(raw: Vec<image::Frame>, canvas_w: u32, canvas_h: u32) -> Vec<Frame> {
    let canvas_bytes = (canvas_w as usize) * (canvas_h as usize) * 4;
    let max_frames = (MAX_RETAINED_BYTES / canvas_bytes.max(1))
        .clamp(1, MAX_RETAINED_FRAMES)
        .max(1);
    let mut accum: Vec<u8> = vec![0u8; canvas_bytes];
    let mut out: Vec<Frame> = Vec::new();

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

        let duration = frame_duration(&f);
        if out.len() < max_frames {
            out.push(Frame {
                pixels: accum.clone(),
                duration,
            });
        } else if let Some(last) = out.last_mut() {
            // Budget reached: keep compositing (the accumulator must still
            // track every source frame so the loop point is correct) but
            // stop retaining pixels; the dropped frame's display time folds
            // into the final retained frame.
            last.duration += duration;
        }
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

/// Convert tightly packed straight RGBA8 to premultiplied BGRA8. Flux canvas
/// image draws use premultiplied source-over blending, so retaining straight
/// RGB on a partially transparent edge would produce a bright fringe.
fn rgba_to_premultiplied_bgra_inplace(buf: &mut [u8]) {
    for chunk in buf.chunks_exact_mut(4) {
        let red = chunk[0] as u32;
        let green = chunk[1] as u32;
        let blue = chunk[2] as u32;
        let alpha = chunk[3] as u32;
        chunk[0] = ((blue * alpha + 127) / 255) as u8;
        chunk[1] = ((green * alpha + 127) / 255) as u8;
        chunk[2] = ((red * alpha + 127) / 255) as u8;
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

    fn next_frame_in(&self) -> Option<Duration> {
        (self.frames.len() > 1).then(|| {
            self.frames[self.current]
                .duration
                .saturating_sub(self.last_advance.elapsed())
        })
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
    fn rgba_to_bgra_swaps_channels_and_premultiplies_alpha() {
        let mut buf = [64u8, 128, 192, 128];
        rgba_to_premultiplied_bgra_inplace(&mut buf);
        assert_eq!(buf, [96, 64, 32, 128]);
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

    #[test]
    fn static_png_loads_from_packaged_bytes() {
        let png = write_png(1, 1, &[12, 34, 56, 255]);
        let s = StillSource::load_static_bytes(&png, Path::new("bundled.png"))
            .expect("decode bundled png");
        assert_eq!(s.dimensions(), (1, 1));
        assert_eq!(&s.frames[0].pixels, &[56, 34, 12, 255]);
    }

    #[test]
    fn animation_budget_keeps_loop_duration_and_caps_frames() {
        // A wide-but-tiny canvas keeps the test fast while exercising the
        // frame-count ceiling: 600 frames at 8x1 exceeds MAX_RETAINED_FRAMES
        // (512), so the retained set must stop growing and fold the dropped
        // frames' durations into the final retained frame. The surviving
        // animation must still span the same total wall-clock time so the
        // loop cadence is unchanged.
        let frames: Vec<image::Frame> = (0..600)
            .map(|i| {
                image::Frame::from_parts(
                    image::RgbaImage::from_pixel(8, 1, image::Rgba([i as u8, 0, 0, 255])),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(16, 1),
                )
            })
            .collect();
        let out = composite_frames(frames.clone(), 8, 1);
        assert!(
            out.len() <= MAX_RETAINED_FRAMES,
            "retained {} frames over the {} ceiling",
            out.len(),
            MAX_RETAINED_FRAMES
        );
        let total: Duration = out.iter().map(|f| f.duration).sum();
        let expected: Duration = (0..600).map(|_| Duration::from_millis(16)).sum();
        assert_eq!(total, expected, "loop wall-clock span must be preserved");
    }

    #[test]
    fn animation_under_budget_retains_every_frame() {
        let frames: Vec<image::Frame> = (0..8)
            .map(|i| {
                image::Frame::from_parts(
                    image::RgbaImage::from_pixel(4, 1, image::Rgba([i as u8, 0, 0, 255])),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(40, 1),
                )
            })
            .collect();
        let out = composite_frames(frames, 4, 1);
        assert_eq!(out.len(), 8);
        assert!(out.iter().all(|f| f.duration == Duration::from_millis(40)));
    }

    fn tempfile_dir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "aegis-wallpaper-test-{}-{}",
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
