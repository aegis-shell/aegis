//! VRM avatar path: load the `.glb`-backed model and render it offscreen to a
//! portrait texture already circle-masked in its alpha channel.
//!
//! VRM 0.x and VRM 1.0 are both binary glTF containers, so the model loads
//! through `flux_scene_graph::Scene::from_glb_with_materials`, including its
//! embedded base-colour textures and per-primitive alpha/culling state. VRMA
//! motion-library clips are bound by humanoid bone identity, retargeted from
//! their source T-pose, sampled on the CPU, and skinned on the GPU.
//!
//! Rendering follows the established offscreen pattern: a depth-tested scene
//! pass updates one reusable sampleable render target, which a Canvas pass
//! then blits through an analytic rounded-rect clip into the published
//! texture. Hosts composite it directly without their own clip; animation
//! never performs a GPU→CPU readback or texture re-upload.

use std::path::{Path, PathBuf};

use flux::{Camera, Canvas, Format, Image, SceneColorLoad, SceneLight, Surface, Target};
use flux_scene_graph::{Bounds, MaterialTarget, Scene};

#[cfg(debug_assertions)]
use super::super::mask::circle_mask_premultiplied;
use super::ATLAS_SIZE;
use super::motion::{MotionInfo, MotionLibrary};

/// Caller-owned parameters for framing a VRM in the square offscreen target.
///
/// Ratios are relative to the model's measured height, so one profile remains
/// useful across VRMs with different authored scales. The renderer follows
/// animated head translation while preserving this composition.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VrmCamera {
    /// Vertical perspective field of view in degrees, in `(1, 179)`.
    pub vertical_fov_degrees: f32,
    /// Fraction of total model height visible in the square portrait.
    pub visible_height_ratio: f32,
    /// Optical center measured down from the top of the visible frame.
    pub center_from_top_ratio: f32,
    /// Horizontal target offset as a fraction of total model height.
    pub horizontal_offset_ratio: f32,
}

impl VrmCamera {
    /// Construct camera parameters for a normalized portrait composition.
    pub const fn new(
        vertical_fov_degrees: f32,
        visible_height_ratio: f32,
        center_from_top_ratio: f32,
        horizontal_offset_ratio: f32,
    ) -> Self {
        Self {
            vertical_fov_degrees,
            visible_height_ratio,
            center_from_top_ratio,
            horizontal_offset_ratio,
        }
    }

    fn validate(self) -> Result<Self, CameraError> {
        if !self.vertical_fov_degrees.is_finite()
            || !(1.0..179.0).contains(&self.vertical_fov_degrees)
        {
            return Err(CameraError::VerticalFov(self.vertical_fov_degrees));
        }
        if !self.visible_height_ratio.is_finite() || self.visible_height_ratio <= 0.0 {
            return Err(CameraError::VisibleHeight(self.visible_height_ratio));
        }
        if !self.center_from_top_ratio.is_finite() {
            return Err(CameraError::CenterFromTop(self.center_from_top_ratio));
        }
        if !self.horizontal_offset_ratio.is_finite() {
            return Err(CameraError::HorizontalOffset(self.horizontal_offset_ratio));
        }
        Ok(self)
    }
}

/// Invalid caller-provided VRM camera parameters.
#[derive(Clone, Copy, Debug, PartialEq, thiserror::Error)]
pub enum CameraError {
    #[error("vertical FOV must be finite and between 1 and 179 degrees, got {0}")]
    VerticalFov(f32),
    #[error("visible-height ratio must be finite and positive, got {0}")]
    VisibleHeight(f32),
    #[error("center-from-top ratio must be finite, got {0}")]
    CenterFromTop(f32),
    #[error("horizontal-offset ratio must be finite, got {0}")]
    HorizontalOffset(f32),
}

/// What the loaded VRM model can do regarding VRMA animation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationSupport {
    /// One or more VRMA clips can advance frame-to-frame (skinning and
    /// animation are available in the scene graph and the model exposes a
    /// humanoid rig).
    Animated,
    /// The model loaded without a usable VRMA clip and remains in its rest
    /// pose. Honest degradation rather than silent freezing.
    Static,
}

/// Errors specific to loading or rendering a VRM model.
#[derive(Debug, thiserror::Error)]
pub enum VrmError {
    #[error("invalid VRM camera: {0}")]
    Camera(#[from] CameraError),
    #[error("read {0:?}")]
    Io(PathBuf, #[source] std::io::Error),
    #[error("vrm model {0:?}")]
    Gltf(PathBuf, #[source] flux_scene_graph::LoadError),
    /// The model has no measurable bounding box, so a framing camera cannot
    /// be computed. Usually an empty or corrupt glTF.
    #[error("vrm model {0:?} has no measurable bounds")]
    NoBounds(PathBuf),
    #[error("vrma clip {0:?}")]
    Animation(PathBuf, #[source] flux_scene_graph::Error),
    #[error("avatar motion path {0:?} must be a directory")]
    MotionDirectory(PathBuf),
    #[error("avatar motion path {0:?} must be a regular file")]
    MotionFile(PathBuf),
    #[error("avatar motion {0:?} must use a lowercase ASCII name beginning with a letter")]
    MotionName(PathBuf),
    #[error("duplicate avatar motion {0:?}: {1:?} and {2:?}")]
    DuplicateMotion(String, PathBuf, PathBuf),
    #[error("avatar motion {0:?} has invalid duration {1}")]
    MotionDuration(PathBuf, f32),
    #[error("avatar motion {0:?} has no retargetable channels")]
    MotionChannels(PathBuf),
    #[error("render vrm model {0:?}")]
    Render(PathBuf, #[source] flux::Error),
}

/// A loaded VRM model with persistent animation and offscreen render state.
pub struct Model {
    scene: Scene,
    motions: MotionLibrary,
    bounds: Bounds,
    rest_head: Option<[f32; 3]>,
    camera: VrmCamera,
    surface: Surface,
    /// One canvas serves both the circular mask blit and the debug dump.
    canvas: Canvas,
    depth: Target,
    /// Raw scene-pass output, before the circular mask.
    rendered: Image,
    /// `rendered` re-rendered through an analytic circular clip; this is the
    /// texture hosts composite.
    texture: Image,
    source_path: PathBuf,
}

impl Model {
    /// Load `path` as a VRM/glTF model and prepare its render material.
    pub fn build(
        device: &Device,
        path: &Path,
        animation_path: Option<&Path>,
        camera: VrmCamera,
    ) -> Result<Self, VrmError> {
        let camera = camera.validate()?;
        let bytes = std::fs::read(path).map_err(|error| VrmError::Io(path.to_path_buf(), error))?;
        let scene = Scene::from_glb_with_materials(
            device,
            &bytes,
            MaterialTarget {
                color_format: TARGET_FORMAT,
                depth_format: DEPTH_FORMAT,
            },
        )
        .map_err(|error| VrmError::Gltf(path.to_path_buf(), error))?;
        let bounds = scene
            .bounds()
            .ok_or_else(|| VrmError::NoBounds(path.to_path_buf()))?;
        let rest_head = scene.humanoid_bone_position("head");
        let motions = MotionLibrary::load(
            &scene,
            path.parent().unwrap_or_else(|| Path::new(".")),
            animation_path,
        )?;
        let surface = Surface::offscreen(device, ATLAS_SIZE, ATLAS_SIZE)
            .map_err(|error| VrmError::Render(path.to_path_buf(), error))?;
        let canvas =
            Canvas::new(&surface).map_err(|error| VrmError::Render(path.to_path_buf(), error))?;
        let depth = Target::depth(device, ATLAS_SIZE, ATLAS_SIZE, DEPTH_FORMAT)
            .map_err(|error| VrmError::Render(path.to_path_buf(), error))?;
        let rendered = Image::render_target(device, ATLAS_SIZE, ATLAS_SIZE, TARGET_FORMAT)
            .map_err(|error| VrmError::Render(path.to_path_buf(), error))?;
        let texture = Image::render_target(device, ATLAS_SIZE, ATLAS_SIZE, TARGET_FORMAT)
            .map_err(|error| VrmError::Render(path.to_path_buf(), error))?;
        let mut model = Self {
            scene,
            motions,
            bounds,
            rest_head,
            camera,
            surface,
            canvas,
            depth,
            rendered,
            texture,
            source_path: path.to_path_buf(),
        };
        model.render()?;
        Ok(model)
    }

    /// Report whether VRMA animation can advance for this model.
    #[must_use]
    pub fn animation_support(&self) -> AnimationSupport {
        if !self.motions.is_empty() {
            AnimationSupport::Animated
        } else {
            AnimationSupport::Static
        }
    }

    pub fn set_camera(&mut self, camera: VrmCamera) -> Result<bool, VrmError> {
        let camera = camera.validate()?;
        if camera == self.camera {
            return Ok(false);
        }
        self.camera = camera;
        self.render()?;
        Ok(true)
    }

    /// The VRM portrait texture, masked in its alpha channel during the final
    /// offscreen blit. Hosts remain free to apply their own geometric crop as
    /// part of the surrounding presentation.
    #[must_use]
    pub fn texture(&self) -> &Image {
        &self.texture
    }

    #[must_use]
    pub fn motions(&self) -> Vec<MotionInfo> {
        self.motions.infos()
    }

    #[must_use]
    pub fn current_motion(&self) -> Option<&str> {
        self.motions.current_name()
    }

    pub fn play_motion(&mut self, name: &str) -> bool {
        self.motions.play(name)
    }

    pub fn play_random_action(&mut self) -> Option<&str> {
        self.motions.play_random_action()
    }

    #[must_use]
    pub fn is_playing(&self) -> bool {
        self.motions.is_playing()
    }

    /// Advance the selected clip and refresh the reusable GPU texture. Returns
    /// `Ok(false)` for a rest-pose model so hosts can stay event-driven.
    pub fn advance(&mut self, delta_seconds: f32) -> Result<bool, VrmError> {
        if !self.motions.advance(delta_seconds) {
            return Ok(false);
        }
        self.render()?;
        Ok(true)
    }

    fn render(&mut self) -> Result<(), VrmError> {
        if let Some((animation, elapsed, animation_path)) = self.motions.sample() {
            self.scene
                .apply_animation(animation, elapsed, false)
                .map_err(|error| VrmError::Animation(animation_path.to_path_buf(), error))?;
        } else {
            self.scene.reset_pose();
        }
        let mut frame = self
            .surface
            .begin_frame()
            .map_err(|error| VrmError::Render(self.source_path.clone(), error))?;
        let pass = frame
            .begin_image_scene_pass(&self.rendered, &self.depth, SceneColorLoad::Clear([0.0; 4]))
            .map_err(|error| VrmError::Render(self.source_path.clone(), error))?;
        let head_offset = self
            .rest_head
            .zip(self.scene.humanoid_bone_position("head"))
            .map(|(rest, current)| {
                [
                    current[0] - rest[0],
                    current[1] - rest[1],
                    current[2] - rest[2],
                ]
            })
            .unwrap_or([0.0; 3]);
        let camera = framing_camera(self.bounds, 1.0, head_offset, self.camera);
        // A soft key close to the portrait camera keeps facial planes readable.
        // `direction` is the direction light travels, hence +Z for a camera on
        // the model's -Z/front side.
        let light = SceneLight {
            direction: [0.25, -0.55, 1.0],
            color: [1.0, 0.97, 0.94],
            ambient: 0.22,
        };
        self.scene.draw_materials(&pass, &camera, Some(&light));
        pass.end();
        // Blit the scene through an analytic circular clip into the published
        // texture, so hosts (including the lens-based command panel, which
        // cannot clip images to a circle) composite it without their own
        // mask. The radius of half the edge makes the rrect a full circle.
        self.canvas
            .begin_target(&frame, &self.texture, Some(flux::rgba(0, 0, 0, 0)))
            .map_err(|error| VrmError::Render(self.source_path.clone(), error))?;
        self.canvas.draw_image_rrect(
            &self.rendered,
            0.0,
            0.0,
            ATLAS_SIZE as f32,
            ATLAS_SIZE as f32,
            ATLAS_SIZE as f32 * 0.5,
        );
        self.canvas
            .end_target_checked()
            .map_err(|error| VrmError::Render(self.source_path.clone(), error))?;
        // The lock screen samples `texture()` directly. Only an explicitly
        // requested debug dump adds the GPU copy to the readable surface;
        // release builds have neither this pass nor a CPU readback path.
        #[cfg(debug_assertions)]
        if std::env::var_os("AEGIS_AVATAR_DEBUG_DUMP").is_some() {
            self.canvas
                .begin_frame(Some(&frame), Some(flux::rgba(0, 0, 0, 0)))
                .map_err(|error| VrmError::Render(self.source_path.clone(), error))?;
            self.canvas.draw_image(
                &self.rendered,
                0.0,
                0.0,
                ATLAS_SIZE as f32,
                ATLAS_SIZE as f32,
            );
            self.canvas
                .end_frame_checked()
                .map_err(|error| VrmError::Render(self.source_path.clone(), error))?;
        }
        frame
            .submit()
            .and_then(flux::SubmittedFrame::present)
            .map_err(|error| VrmError::Render(self.source_path.clone(), error))?;

        #[cfg(debug_assertions)]
        if let Some(path) = std::env::var_os("AEGIS_AVATAR_DEBUG_DUMP") {
            let mut pixels = vec![0u8; (ATLAS_SIZE as usize) * (ATLAS_SIZE as usize) * 4];
            if let Err(error) = self.surface.read_pixels(&mut pixels) {
                log::warn!("avatar: could not read debug preview {path:?}: {error}");
            } else if let Some(square) = image::RgbaImage::from_raw(ATLAS_SIZE, ATLAS_SIZE, pixels)
            {
                let masked = circle_mask_premultiplied(&square);
                if let Err(error) = masked.save(&path) {
                    log::warn!("avatar: could not write debug preview {path:?}: {error}");
                }
            }
        }
        Ok(())
    }
}

/// Compute a portrait camera from model bounds and caller-owned normalized
/// composition parameters.
fn framing_camera(
    bounds: Bounds,
    aspect: f32,
    tracking_offset: [f32; 3],
    config: VrmCamera,
) -> Camera {
    let frame = portrait_frame(bounds, aspect, config);
    let height = (bounds.max[1] - bounds.min[1]).max(0.001);
    let center_x = (bounds.min[0] + bounds.max[0]) * 0.5 + height * config.horizontal_offset_ratio;
    let center_z = (bounds.min[2] + bounds.max[2]) * 0.5;
    let center = [
        center_x + tracking_offset[0],
        frame.center_y + tracking_offset[1],
        center_z + tracking_offset[2],
    ];
    // VRM's humanoid forward axis is -Z, so the camera belongs on the -Z
    // side looking toward +Z. A +Z eye shows the back of a conforming avatar.
    let eye = [
        center[0],
        center[1],
        bounds.min[2] + tracking_offset[2] - frame.distance,
    ];
    let near = (frame.distance - frame.depth * 1.5).max(0.01);
    let far = frame.distance + frame.depth * 3.0 + 1.0;
    let fov_y = config.vertical_fov_degrees.to_radians();
    let mut camera = Camera::perspective(fov_y, aspect, near, far);
    camera.look_at(eye, center, [0.0, 1.0, 0.0]);
    camera
}

#[derive(Clone, Copy, Debug)]
struct PortraitFrame {
    center_y: f32,
    distance: f32,
    depth: f32,
}

fn portrait_frame(bounds: Bounds, aspect: f32, config: VrmCamera) -> PortraitFrame {
    let height = (bounds.max[1] - bounds.min[1]).max(0.001);
    let depth = (bounds.max[2] - bounds.min[2]).max(0.001);
    let visible_height = height * config.visible_height_ratio;
    // Deliberately ignore the full model width: humanoid bind poses commonly
    // extend both arms into a T-pose, which would zoom a portrait out until it
    // showed the torso. The circle is allowed to crop arms beyond the shoulder.
    let fitted_height = visible_height / aspect.clamp(0.75, 1.0);
    let center_y = bounds.max[1] - fitted_height * config.center_from_top_ratio;
    let fov_y = config.vertical_fov_degrees.to_radians();
    let distance = (fitted_height * 0.5) / (fov_y * 0.5).tan();
    PortraitFrame {
        center_y,
        distance,
        depth,
    }
}

const TARGET_FORMAT: Format = Format::Rgba8Unorm;
const DEPTH_FORMAT: Format = Format::D32Sfloat;

// The flux Device type alias; kept out of the public surface for clarity.
use flux::Device;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_body_bounds_become_a_head_and_shoulders_crop() {
        let bounds = Bounds {
            min: [-0.81, 0.0, -0.16],
            max: [0.81, 1.77, 0.16],
        };
        let frame = portrait_frame(bounds, 1.0, VrmCamera::new(28.0, 0.25, 0.48, 0.0));
        // The optical center sits around the upper chest/face, never at the
        // full-body midpoint (0.885 for this fixture).
        assert!(frame.center_y > 1.50);
        assert!(frame.center_y < 1.62);
        assert!(frame.distance > 0.7);
        assert!(frame.distance < 1.2);
    }

    #[test]
    fn caller_camera_rejects_non_finite_and_impossible_values() {
        assert!(matches!(
            VrmCamera::new(0.0, 0.25, 0.48, 0.0).validate(),
            Err(CameraError::VerticalFov(0.0))
        ));
        assert!(matches!(
            VrmCamera::new(28.0, f32::NAN, 0.48, 0.0).validate(),
            Err(CameraError::VisibleHeight(value)) if value.is_nan()
        ));
    }
}
