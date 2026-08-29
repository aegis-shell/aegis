//! Pointer-driven multi-plane image wallpaper.
//!
//! Layers are ordered back-to-front and carry normalized depth. Pointer
//! samples only update the destination; a frame-rate-independent low-pass
//! state advances during drawing. That separation is what turns two pointer
//! samples on exposed wallpaper, separated by an intervening window, into a
//! continuous visual transition instead of a jump.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crate::{Error, Source, StillSource};

const PARALLAX_FRAME_INTERVAL: Duration = Duration::from_micros(16_667);
const MOTION_EPSILON: f32 = 0.000_5;

/// One back-to-front image plane. `depth` is normalized: zero is fixed at the
/// back of the scene, while one receives the full configured displacement.
#[derive(Debug, Clone, PartialEq)]
pub struct ParallaxLayerSpec {
    pub path: PathBuf,
    pub depth: f32,
}

impl ParallaxLayerSpec {
    pub fn new(path: impl Into<PathBuf>, depth: f32) -> Self {
        Self {
            path: path.into(),
            depth,
        }
    }
}

/// Motion tuning for a parallax scene.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ParallaxOptions {
    /// Maximum displacement, in logical pixels, for a plane at depth `1.0`.
    pub max_shift: f32,
    /// Approximate time for a discontinuous target change to settle 95% of
    /// the way to its destination.
    pub transition: Duration,
}

impl Default for ParallaxOptions {
    fn default() -> Self {
        Self {
            max_shift: 32.0,
            transition: Duration::from_millis(240),
        }
    }
}

struct ImagePlane {
    source: StillSource,
    width: u32,
    height: u32,
    depth: f32,
    image: Option<flux::Image>,
    last_uploaded_gen: u64,
}

impl ImagePlane {
    fn load(spec: &ParallaxLayerSpec) -> Result<Self, Error> {
        let source = StillSource::load(Path::new(&spec.path))?;
        let (width, height) = source.dimensions();
        Ok(Self {
            source,
            width,
            height,
            depth: spec.depth,
            image: None,
            last_uploaded_gen: u64::MAX,
        })
    }

    fn poll_and_upload(&mut self, device: &flux::Device, now: Instant) {
        let (pixels, generation) = self.source.poll(now);
        if generation == self.last_uploaded_gen {
            return;
        }
        let needed = self.width as usize * self.height as usize * 4;
        if pixels.len() != needed {
            log::warn!(
                "wallpaper: parallax layer pixel buffer {} bytes, expected {needed}; skipping upload",
                pixels.len()
            );
            return;
        }
        match flux::Image::from_bytes(
            device,
            self.width,
            self.height,
            flux::Format::Bgra8Unorm,
            pixels,
        ) {
            Ok(image) => {
                self.image = Some(image);
                self.last_uploaded_gen = generation;
            }
            Err(error) => log::warn!("wallpaper: parallax layer upload failed: {error:?}"),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Motion {
    current: (f32, f32),
    target: (f32, f32),
    transition: Duration,
    last_update: Instant,
}

impl Motion {
    fn new(transition: Duration) -> Self {
        Self {
            current: (0.0, 0.0),
            target: (0.0, 0.0),
            transition,
            last_update: Instant::now(),
        }
    }

    fn set_target(&mut self, target: (f32, f32), now: Instant) {
        let was_still = !self.is_animating();
        let stale = now.duration_since(self.last_update) > PARALLAX_FRAME_INTERVAL * 4;
        if target != self.target && (was_still || stale) {
            // The compositor may have slept while the cursor crossed a client
            // window. Start the new transition at this sample rather than
            // charging that idle time to a target that did not exist yet.
            self.last_update = now;
        }
        self.target = target;
    }

    fn advance(&mut self, now: Instant) {
        let dt = now.duration_since(self.last_update);
        if dt < Duration::from_millis(1) || !self.is_animating() {
            return;
        }
        self.last_update = now;
        // Three time constants reach 95.0%, so expose the more intuitive
        // settle duration while retaining a monotonic, overshoot-free filter.
        let tau = (self.transition.as_secs_f32() / 3.0).max(0.001);
        let alpha = 1.0 - (-dt.as_secs_f32().min(0.050) / tau).exp();
        self.current.0 += (self.target.0 - self.current.0) * alpha;
        self.current.1 += (self.target.1 - self.current.1) * alpha;
        if !self.is_animating() {
            self.current = self.target;
        }
    }

    fn is_animating(&self) -> bool {
        (self.target.0 - self.current.0).abs() > MOTION_EPSILON
            || (self.target.1 - self.current.1).abs() > MOTION_EPSILON
    }

    fn center(&mut self, now: Instant) {
        self.current = (0.0, 0.0);
        self.target = (0.0, 0.0);
        self.last_update = now;
    }
}

pub(super) struct ParallaxScene {
    layers: Vec<ImagePlane>,
    dimensions: (u32, u32),
    max_shift: f32,
    motion: Motion,
    pointer_active: bool,
    reduced_motion: bool,
}

impl ParallaxScene {
    pub(super) fn load(
        specs: &[ParallaxLayerSpec],
        options: ParallaxOptions,
    ) -> Result<Self, Error> {
        if !(2..=8).contains(&specs.len()) {
            return Err(Error::Parallax(
                "a parallax wallpaper needs between 2 and 8 image layers".into(),
            ));
        }
        if !options.max_shift.is_finite() || !(1.0..=256.0).contains(&options.max_shift) {
            return Err(Error::Parallax(
                "max_shift must be between 1 and 256 logical pixels".into(),
            ));
        }
        if !(Duration::from_millis(80)..=Duration::from_secs(2)).contains(&options.transition) {
            return Err(Error::Parallax(
                "transition must be between 80 ms and 2 s".into(),
            ));
        }

        let mut previous_depth = f32::NEG_INFINITY;
        let mut layers = Vec::with_capacity(specs.len());
        for spec in specs {
            if !spec.depth.is_finite() || !(0.0..=1.0).contains(&spec.depth) {
                return Err(Error::Parallax(format!(
                    "layer {:?} depth must be between 0.0 and 1.0",
                    spec.path
                )));
            }
            if spec.depth < previous_depth {
                return Err(Error::Parallax(
                    "layers must be ordered from farthest to nearest".into(),
                ));
            }
            previous_depth = spec.depth;
            layers.push(ImagePlane::load(spec)?);
        }
        let dimensions = layers
            .first()
            .map(|layer| (layer.width, layer.height))
            .unwrap_or((1, 1));
        log::info!(
            "wallpaper: parallax scene loaded ({} layers, max shift {} px, transition {:?})",
            layers.len(),
            options.max_shift,
            options.transition
        );
        Ok(Self {
            layers,
            dimensions,
            max_shift: options.max_shift,
            motion: Motion::new(options.transition),
            pointer_active: false,
            reduced_motion: false,
        })
    }

    pub(super) fn dimensions(&self) -> (u32, u32) {
        self.dimensions
    }

    pub(super) fn set_pointer(&mut self, position: Option<(f32, f32)>, viewport: (f32, f32)) {
        self.pointer_active = false;
        if self.reduced_motion || viewport.0 <= 0.0 || viewport.1 <= 0.0 {
            return;
        }
        let Some((x, y)) = position else {
            return;
        };
        if x < 0.0 || y < 0.0 || x > viewport.0 || y > viewport.1 {
            return;
        }
        let target = (
            (x / viewport.0 * 2.0 - 1.0).clamp(-1.0, 1.0),
            (y / viewport.1 * 2.0 - 1.0).clamp(-1.0, 1.0),
        );
        self.pointer_active = true;
        self.motion.set_target(target, Instant::now());
    }

    pub(super) fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
        self.pointer_active = false;
        if reduced {
            // Reduced motion means a stable centered composition, not a
            // discrete jump to the latest pointer target.
            self.motion.center(Instant::now());
        }
    }

    pub(super) fn pointer_active(&self) -> bool {
        self.pointer_active
    }

    pub(super) fn next_frame_in(&self) -> Option<Duration> {
        let layer_deadline = self
            .layers
            .iter()
            .filter_map(|layer| layer.source.next_frame_in())
            .min();
        let motion_deadline = (!self.reduced_motion && self.motion.is_animating())
            .then(|| PARALLAX_FRAME_INTERVAL.saturating_sub(self.motion.last_update.elapsed()));
        match (layer_deadline, motion_deadline) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(value), None) | (None, Some(value)) => Some(value),
            (None, None) => None,
        }
    }

    pub(super) fn draw(
        &mut self,
        device: &flux::Device,
        canvas: &flux::Canvas,
        dst_w: f32,
        dst_h: f32,
    ) {
        let now = Instant::now();
        if !self.reduced_motion {
            self.motion.advance(now);
        }
        for layer in &mut self.layers {
            layer.poll_and_upload(device, now);
            let Some(image) = layer.image.as_ref() else {
                continue;
            };
            let shift = self.max_shift * layer.depth;
            let offset = (
                -self.motion.current.0 * shift,
                -self.motion.current.1 * shift,
            );
            let (x, y, width, height) = cover_geometry(
                (layer.width as f32, layer.height as f32),
                (dst_w, dst_h),
                shift,
                offset,
            );
            canvas.draw_image(image, x, y, width, height);
        }
    }
}

fn cover_geometry(
    source: (f32, f32),
    destination: (f32, f32),
    overscan: f32,
    offset: (f32, f32),
) -> (f32, f32, f32, f32) {
    let required = (
        (destination.0 + overscan * 2.0).max(1.0),
        (destination.1 + overscan * 2.0).max(1.0),
    );
    let scale = (required.0 / source.0.max(1.0)).max(required.1 / source.1.max(1.0));
    let width = source.0 * scale;
    let height = source.1 * scale;
    (
        (destination.0 - width) * 0.5 + offset.0,
        (destination.1 - height) * 0.5 + offset.1,
        width,
        height,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cover_geometry_reserves_shift_without_exposing_an_edge() {
        let (x, y, width, height) =
            cover_geometry((1600.0, 900.0), (1280.0, 720.0), 32.0, (-32.0, 32.0));
        assert!(x <= -64.0);
        assert!(y <= 0.0);
        assert!(x + width >= 1280.0);
        assert!(y + height >= 720.0 + 32.0);
    }

    #[test]
    fn discontinuous_target_is_interpolated_monotonically() {
        let start = Instant::now();
        let mut motion = Motion {
            current: (-1.0, 0.5),
            target: (-1.0, 0.5),
            transition: Duration::from_millis(240),
            last_update: start,
        };
        motion.set_target((1.0, -0.5), start);
        motion.advance(start + Duration::from_millis(16));
        assert!(motion.current.0 > -1.0 && motion.current.0 < 1.0);
        assert!(motion.current.1 < 0.5 && motion.current.1 > -0.5);
        let first = motion.current;
        motion.advance(start + Duration::from_millis(32));
        assert!(motion.current.0 > first.0 && motion.current.0 < 1.0);
        assert!(motion.current.1 < first.1 && motion.current.1 > -0.5);
    }

    #[test]
    fn stale_time_before_a_new_target_is_not_charged_to_the_transition() {
        let start = Instant::now();
        let mut motion = Motion {
            current: (-0.8, 0.0),
            target: (-0.8, 0.0),
            transition: Duration::from_millis(240),
            last_update: start,
        };
        let reentry = start + Duration::from_secs(2);
        motion.set_target((0.8, 0.0), reentry);
        motion.advance(reentry);
        assert_eq!(motion.current, (-0.8, 0.0));
        motion.advance(reentry + Duration::from_millis(16));
        assert!(motion.current.0 > -0.8 && motion.current.0 < 0.8);
    }

    #[test]
    fn bundled_alpine_planes_decode_as_a_scene() {
        let root =
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/wallpapers/parallax-alpine");
        let specs = [
            ParallaxLayerSpec::new(root.join("far.png"), 0.0),
            ParallaxLayerSpec::new(root.join("mid.png"), 0.45),
            ParallaxLayerSpec::new(root.join("near.png"), 1.0),
        ];
        let scene = ParallaxScene::load(&specs, ParallaxOptions::default())
            .expect("bundled Alpine planes decode");
        assert_eq!(scene.layers.len(), 3);
        assert_eq!(scene.dimensions(), (1662, 946));
    }
}
