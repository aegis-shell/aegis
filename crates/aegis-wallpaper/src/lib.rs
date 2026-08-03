//! Compositor wallpaper for aegis.
//!
//! Renders image, short-video, glTF, or multi-plane pointer-parallax scenes as
//! the bottom-most frame layers. Multi-frame sources (animated GIF/WebP and
//! video) and parallax transitions advance inside [`Wallpaper::draw`].
//!
//! The crate owns decode, GPU scene-resource, and animation state, but it does
//! not retain the flux device or canvas. Each frame the main loop calls
//! [`Wallpaper::draw`] with the current device, canvas, and output size. See
//! ADR-0018 and ADR-0092 for the design.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

mod model;
mod parallax;
mod still;
mod video;

use model::ModelLayer;
use parallax::ParallaxScene;
pub use parallax::{ParallaxLayerSpec, ParallaxOptions};
use still::StillSource;
use video::VideoSource;

/// Errors raised while loading or advancing a wallpaper source.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("wallpaper: open {0:?}: {1}")]
    Open(PathBuf, #[source] std::io::Error),
    #[error("wallpaper: decode {0:?}: {1}")]
    Decode(PathBuf, String),
    #[error("wallpaper: no decoder matched {0:?} (format hint: {1:?})")]
    UnsupportedFormat(PathBuf, Option<String>),
    #[error("wallpaper: ffmpeg missing or failed to spawn: {0}")]
    FfmpegSpawn(#[source] std::io::Error),
    #[error("wallpaper: ffmpeg produced no frames for {0:?}")]
    FfmpegEmpty(PathBuf),
    #[error("wallpaper: glTF model {0:?}: {1}")]
    Gltf(PathBuf, #[source] flux_scene_graph::Error),
    #[error("wallpaper: glTF model {0:?} has no measurable bounds")]
    GltfBounds(PathBuf),
    #[error("wallpaper: 3D render resource: {0}")]
    Flux(#[from] flux::Error),
    #[error("wallpaper: parallax configuration: {0}")]
    Parallax(String),
}

/// Per-source decode + pacing state. Each implementation owns its pixels
/// and exposes them via [`Source::poll`], which advances internal pacing
/// using wall-clock time and returns the current frame alongside a
/// generation counter. The wrapper re-uploads only when the generation
/// changes.
trait Source {
    /// Decoded pixel dimensions (post any internal scaling).
    fn dimensions(&self) -> (u32, u32);
    /// Advance pacing and return `(current_pixels, generation)`. The
    /// generation must change whenever the returned pixels differ from
    /// the previous call.
    fn poll(&mut self, now: Instant) -> (&[u8], u64);
    /// Number of decoded frames; 0 for live sources such as video.
    fn frame_count(&self) -> usize;
    /// Delay after which the source's visible frame changes and it wants
    /// to be polled again, or `None` for single-frame sources. Lets an
    /// otherwise idle compositor wake in time for the next animation frame
    /// instead of pacing it with a fixed timer.
    fn next_frame_in(&self) -> Option<Duration>;
}

enum SourceKind {
    Still(StillSource),
    Video(VideoSource),
}

/// A compositor wallpaper. Owns decode state (and, for video, the ffmpeg
/// child) and the most recently uploaded flux texture.
///
/// Construction decodes the source to pixels; per-frame rendering is
/// [`Wallpaper::draw`], which the main loop calls before any client
/// surfaces are composited.
pub struct Wallpaper {
    source: SourceKind,
    width: u32,
    height: u32,
    /// Cached flux texture; re-created only when the source's generation
    /// advances.
    flux_image: Option<flux::Image>,
    /// Generation last uploaded to `flux_image`. Initial value forces a
    /// first-frame upload.
    last_uploaded_gen: u64,
    /// Optional depth-tested model layer drawn between the media background
    /// and compositor client surfaces.
    model: Option<ModelLayer>,
    /// A parallax scene replaces the single 2D source while retaining the
    /// same bottom-layer draw contract.
    parallax: Option<ParallaxScene>,
}

impl Wallpaper {
    /// Load a wallpaper from `path`. Image decode is tried first; on
    /// failure the file is handed to `ffmpeg` as a short-video source.
    ///
    /// `target_w` and `target_h` set the decode resolution for video
    /// (ffmpeg scales the source to fit); images decode at their native
    /// size. On either path the final frame is GPU-scaled to whatever
    /// destination size [`Wallpaper::draw`] is given.
    pub fn from_path(path: impl AsRef<Path>, target_w: u32, target_h: u32) -> Result<Self, Error> {
        let path = path.as_ref();
        log::debug!("wallpaper: loading {path:?}");

        let metadata =
            std::fs::metadata(path).map_err(|error| Error::Open(path.to_path_buf(), error))?;
        if !metadata.is_file() {
            return Err(Error::Open(
                path.to_path_buf(),
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "wallpaper source is not a regular file",
                ),
            ));
        }

        if let Ok(still) = StillSource::load(path) {
            let (w, h) = still.dimensions();
            let count = still.frame_count();
            log::info!("wallpaper: image loaded {path:?} ({w}x{h}, {count} frame(s))");
            return Ok(Wallpaper {
                source: SourceKind::Still(still),
                width: w,
                height: h,
                flux_image: None,
                last_uploaded_gen: u64::MAX,
                model: None,
                parallax: None,
            });
        }

        log::debug!("wallpaper: not an image, trying ffmpeg");
        let video = VideoSource::open(path, target_w, target_h)?;
        let (w, h) = video.dimensions();
        log::info!("wallpaper: video loaded {path:?} ({w}x{h}, via ffmpeg)");
        Ok(Wallpaper {
            source: SourceKind::Video(video),
            width: w,
            height: h,
            flux_image: None,
            last_uploaded_gen: u64::MAX,
            model: None,
            parallax: None,
        })
    }

    /// Load only an image source. Unlike [`Wallpaper::from_path`], this does
    /// not fall back to ffmpeg when image decoding fails.
    pub fn from_image_path(path: impl AsRef<Path>) -> Result<Self, Error> {
        let path = path.as_ref();
        let metadata =
            std::fs::metadata(path).map_err(|error| Error::Open(path.to_path_buf(), error))?;
        if !metadata.is_file() {
            return Err(Error::Open(
                path.to_path_buf(),
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "wallpaper source is not a regular file",
                ),
            ));
        }
        let still = StillSource::load(path)?;
        let (width, height) = still.dimensions();
        log::info!("wallpaper: image loaded {path:?} ({width}x{height})");
        Ok(Self {
            source: SourceKind::Still(still),
            width,
            height,
            flux_image: None,
            last_uploaded_gen: u64::MAX,
            model: None,
            parallax: None,
        })
    }

    /// Load only a video source through ffmpeg.
    pub fn from_video_path(
        path: impl AsRef<Path>,
        target_w: u32,
        target_h: u32,
    ) -> Result<Self, Error> {
        let path = path.as_ref();
        let metadata =
            std::fs::metadata(path).map_err(|error| Error::Open(path.to_path_buf(), error))?;
        if !metadata.is_file() {
            return Err(Error::Open(
                path.to_path_buf(),
                std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "wallpaper source is not a regular file",
                ),
            ));
        }
        let video = VideoSource::open(path, target_w, target_h)?;
        let (width, height) = video.dimensions();
        log::info!("wallpaper: video loaded {path:?} ({width}x{height})");
        Ok(Self {
            source: SourceKind::Video(video),
            width,
            height,
            flux_image: None,
            last_uploaded_gen: u64::MAX,
            model: None,
            parallax: None,
        })
    }

    /// Decode a bundled static image without depending on a build-tree path at
    /// runtime. `label` is used only in diagnostics.
    pub fn from_static_image_bytes(bytes: &[u8], label: impl Into<PathBuf>) -> Result<Self, Error> {
        let label = label.into();
        let still = StillSource::load_static_bytes(bytes, &label)?;
        let (w, h) = still.dimensions();
        log::info!("wallpaper: bundled image loaded ({w}x{h})");
        Ok(Self {
            source: SourceKind::Still(still),
            width: w,
            height: h,
            flux_image: None,
            last_uploaded_gen: u64::MAX,
            model: None,
            parallax: None,
        })
    }

    /// Load a `.glb` as a model-only wallpaper. The scene is automatically
    /// framed, then rendered with an orbiting camera and animated key light.
    pub fn from_gltf(
        device: &flux::Device,
        surface: &flux::Surface,
        path: impl AsRef<Path>,
    ) -> Result<Self, Error> {
        let path = path.as_ref();
        let model = ModelLayer::from_path(device, surface, path)?;
        Ok(Self {
            source: SourceKind::Still(StillSource::transparent_pixel()),
            width: 1,
            height: 1,
            flux_image: None,
            last_uploaded_gen: u64::MAX,
            model: Some(model),
            parallax: None,
        })
    }

    /// Construct a model-only wallpaper with the built-in procedural scene.
    pub fn from_builtin_model(
        device: &flux::Device,
        surface: &flux::Surface,
    ) -> Result<Self, Error> {
        let model = ModelLayer::builtin(device, surface)?;
        Ok(Self {
            source: SourceKind::Still(StillSource::transparent_pixel()),
            width: 1,
            height: 1,
            flux_image: None,
            last_uploaded_gen: u64::MAX,
            model: Some(model),
            parallax: None,
        })
    }

    /// Load a back-to-front stack of image planes as a pointer-driven
    /// parallax wallpaper.
    pub fn from_parallax_layers(
        layers: &[ParallaxLayerSpec],
        options: ParallaxOptions,
    ) -> Result<Self, Error> {
        let parallax = ParallaxScene::load(layers, options)?;
        let (width, height) = parallax.dimensions();
        Ok(Self {
            source: SourceKind::Still(StillSource::transparent_pixel()),
            width,
            height,
            flux_image: None,
            last_uploaded_gen: u64::MAX,
            model: None,
            parallax: Some(parallax),
        })
    }

    /// Add a caller-selected `.glb` model over the current media background.
    pub fn set_model_from_gltf(
        &mut self,
        device: &flux::Device,
        surface: &flux::Surface,
        path: impl AsRef<Path>,
    ) -> Result<(), Error> {
        self.model = Some(ModelLayer::from_path(device, surface, path.as_ref())?);
        Ok(())
    }

    /// Add the built-in procedural knot model over the current background.
    pub fn set_builtin_model(
        &mut self,
        device: &flux::Device,
        surface: &flux::Surface,
    ) -> Result<(), Error> {
        self.model = Some(ModelLayer::builtin(device, surface)?);
        Ok(())
    }

    pub fn has_model(&self) -> bool {
        self.model.is_some()
    }

    pub fn has_parallax(&self) -> bool {
        self.parallax.is_some()
    }

    /// Whether an exposed-wallpaper pointer sample currently drives the
    /// parallax target. Callers use this to avoid taking a cursor-plane-only
    /// presentation path when the wallpaper itself must redraw.
    pub fn parallax_pointer_active(&self) -> bool {
        self.parallax
            .as_ref()
            .is_some_and(ParallaxScene::pointer_active)
    }

    /// Update the parallax target from a logical-output pointer sample.
    /// `None` means the pointer is over a client/chrome surface or outside the
    /// output; the last target is retained and any existing transition can
    /// settle normally.
    pub fn set_pointer_position(&mut self, position: Option<(f32, f32)>, viewport: (f32, f32)) {
        if let Some(parallax) = self.parallax.as_mut() {
            parallax.set_pointer(position, viewport);
        }
    }

    /// Apply the desktop-wide accessibility motion policy.
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        if let Some(parallax) = self.parallax.as_mut() {
            parallax.set_reduced_motion(reduced);
        }
    }

    /// Delay after which the wallpaper's visible frame changes, if the
    /// source animates (video or multi-frame image). `None` for single-frame
    /// sources; the 3D model layer is paced by the caller's animation tick
    /// and does not count here.
    pub fn next_frame_in(&self) -> Option<Duration> {
        if let Some(parallax) = self.parallax.as_ref() {
            return parallax.next_frame_in();
        }
        match &self.source {
            SourceKind::Still(s) => s.next_frame_in(),
            SourceKind::Video(v) => v.next_frame_in(),
        }
    }

    /// Draw the optional 3D model into its own depth-tested pass. The caller
    /// must end any canvas pass first and may begin another canvas pass with
    /// load semantics afterward.
    pub fn draw_model(&mut self, device: &flux::Device, frame: &mut flux::Frame<'_>) {
        self.draw_model_into(device, frame, None);
    }

    /// Draw the optional 3D model into a sampleable offscreen color target.
    /// The target's existing 2D background is loaded and preserved.
    pub fn draw_model_to(
        &mut self,
        device: &flux::Device,
        frame: &mut flux::Frame<'_>,
        target: &flux::Image,
    ) {
        self.draw_model_into(device, frame, Some(target));
    }

    fn draw_model_into(
        &mut self,
        device: &flux::Device,
        frame: &mut flux::Frame<'_>,
        target: Option<&flux::Image>,
    ) {
        let Some(model) = self.model.as_mut() else {
            return;
        };
        if let Err(error) = model.draw(device, frame, target) {
            log::warn!("wallpaper: disabling 3D model after render failure: {error}");
            self.model = None;
        }
    }

    /// Decoded source dimensions (not the destination size passed to
    /// `draw`).
    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    /// Composite the wallpaper into `canvas` at `(0, 0)` scaled to
    /// `(dst_w, dst_h)`. Call once per frame, before any client surfaces
    /// are drawn. For animated/video sources the per-source pacing is
    /// advanced here using wall-clock time.
    pub fn draw(&mut self, device: &flux::Device, canvas: &flux::Canvas, dst_w: f32, dst_h: f32) {
        self.draw_fitted(device, canvas, dst_w, dst_h, false);
    }

    /// Composite the wallpaper with centered cover scaling. This preserves
    /// source aspect ratio and crops overflow instead of stretching artwork.
    pub fn draw_cover(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        dst_w: f32,
        dst_h: f32,
    ) {
        self.draw_fitted(device, canvas, dst_w, dst_h, true);
    }

    fn draw_fitted(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        dst_w: f32,
        dst_h: f32,
        cover: bool,
    ) {
        if let Some(parallax) = self.parallax.as_mut() {
            parallax.draw(device, canvas, dst_w, dst_h);
            return;
        }
        let now = Instant::now();
        let (pixels, r#gen) = match &mut self.source {
            SourceKind::Still(s) => s.poll(now),
            SourceKind::Video(v) => v.poll(now),
        };

        if r#gen != self.last_uploaded_gen {
            let need = (self.width as usize) * (self.height as usize) * 4;
            if pixels.len() == need {
                match flux::Image::from_bytes(
                    device,
                    self.width,
                    self.height,
                    flux::Format::FLUX_FORMAT_BGRA8_UNORM,
                    pixels,
                ) {
                    Ok(img) => {
                        self.flux_image = Some(img);
                        self.last_uploaded_gen = r#gen;
                    }
                    Err(e) => log::warn!("wallpaper: upload failed: {e:?}"),
                }
            } else {
                log::warn!(
                    "wallpaper: pixel buffer {} bytes, expected {need}; skipping upload",
                    pixels.len()
                );
            }
        }

        if let Some(img) = &self.flux_image {
            if cover {
                let (x, y, width, height) =
                    cover_geometry((self.width as f32, self.height as f32), (dst_w, dst_h));
                canvas.draw_image(img, x, y, width, height);
            } else {
                canvas.draw_image(img, 0.0, 0.0, dst_w, dst_h);
            }
        }
    }
}

fn cover_geometry(source: (f32, f32), destination: (f32, f32)) -> (f32, f32, f32, f32) {
    let scale = (destination.0 / source.0.max(1.0)).max(destination.1 / source.1.max(1.0));
    let width = source.0 * scale;
    let height = source.1 * scale;
    (
        (destination.0 - width) * 0.5,
        (destination.1 - height) * 0.5,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_path_fails_before_video_fallback() {
        let path = std::env::temp_dir().join(format!(
            "aegis-wallpaper-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let result = Wallpaper::from_path(&path, 1920, 1080);
        assert!(matches!(result, Err(Error::Open(failed, _)) if failed == path));
    }

    #[test]
    fn cover_geometry_preserves_aspect_ratio_and_centers_crop() {
        let (x, y, width, height) = cover_geometry((1600.0, 900.0), (1000.0, 1000.0));
        assert!(x < 0.0);
        assert!(y.abs() < 0.001);
        assert!((width / height - 16.0 / 9.0).abs() < 0.001);
        assert!((height - 1000.0).abs() < 0.001);
    }
}
