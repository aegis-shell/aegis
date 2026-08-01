//! VRM avatar path: load the `.glb`-backed model and render it offscreen to a
//! portrait texture already circle-masked in its alpha channel.
//!
//! VRM 0.x and VRM 1.0 are both binary glTF containers, so the model loads
//! through `flux_scene_graph::Scene::from_glb`. Companion VRMA clips are bound
//! by humanoid bone identity, retargeted from their source T-pose, sampled on
//! the CPU, and skinned on the GPU.
//!
//! Rendering follows the established offscreen pattern: a depth-tested scene
//! pass updates one reusable sampleable render target, which a Canvas pass
//! then blits through an analytic rounded-rect clip into the published
//! texture. Hosts composite it directly without their own clip; animation
//! never performs a GPU→CPU readback or texture re-upload.

use std::path::{Path, PathBuf};

use flux::{
    Camera, Canvas, Format, Image, Material, MaterialDesc, MaterialKind, SceneColorLoad,
    SceneLight, Surface, Target,
};
use flux_scene_graph::{Animation, Bounds, Scene};

use crate::ATLAS_SIZE;
#[cfg(debug_assertions)]
use crate::mask::circle_mask_premultiplied;

/// What the loaded VRM model can do regarding VRMA animation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationSupport {
    /// VRMA clips can advance frame-to-frame (skinning and animation are
    /// available in the scene graph and the model exposes a humanoid rig).
    Animated,
    /// The model loaded but only as a posed mesh; animation is not possible
    /// in this build. Honest degradation rather than silent freezing.
    Static,
}

/// Errors specific to loading or rendering a VRM model.
#[derive(Debug, thiserror::Error)]
pub enum VrmError {
    #[error("read {0:?}")]
    Io(PathBuf, #[source] std::io::Error),
    #[error("vrm model {0:?}")]
    Gltf(PathBuf, #[source] flux_scene_graph::Error),
    #[error("vrm material for {0:?}")]
    Material(PathBuf, #[source] flux::Error),
    /// The model has no measurable bounding box, so a framing camera cannot
    /// be computed. Usually an empty or corrupt glTF.
    #[error("vrm model {0:?} has no measurable bounds")]
    NoBounds(PathBuf),
    #[error("vrma clip {0:?}")]
    Animation(PathBuf, #[source] flux_scene_graph::Error),
    #[error("render vrm model {0:?}")]
    Render(PathBuf, #[source] flux::Error),
}

/// A loaded VRM model with persistent animation and offscreen render state.
pub struct Model {
    scene: Scene,
    animation: Option<Animation>,
    material: Material,
    bounds: Bounds,
    rest_head: Option<[f32; 3]>,
    surface: Surface,
    /// One canvas serves both the circular mask blit and the debug dump.
    canvas: Canvas,
    depth: Target,
    /// Raw scene-pass output, before the circular mask.
    rendered: Image,
    /// `rendered` re-rendered through an analytic circular clip; this is the
    /// texture hosts composite.
    texture: Image,
    elapsed: f32,
    source_path: PathBuf,
    animation_path: Option<PathBuf>,
}

impl Model {
    /// Load `path` as a VRM/glTF model and prepare its render material.
    pub fn build(
        device: &Device,
        path: &Path,
        animation_path: Option<&Path>,
    ) -> Result<Self, VrmError> {
        let bytes = std::fs::read(path).map_err(|error| VrmError::Io(path.to_path_buf(), error))?;
        let scene = Scene::from_glb(device, &bytes)
            .map_err(|error| VrmError::Gltf(path.to_path_buf(), error))?;
        let bounds = scene
            .bounds()
            .ok_or_else(|| VrmError::NoBounds(path.to_path_buf()))?;
        let rest_head = scene.humanoid_bone_position("head");
        // The color format must match the offscreen render target below; depth
        // matches the scene-pass attachment. Phong gives the avatar readable
        // shading without textures (which the v0.1 loader does not import).
        let material = Material::new(
            device,
            MaterialDesc {
                kind: MaterialKind::Phong,
                base_color: [0.68, 0.72, 0.80, 1.0],
                color_format: TARGET_FORMAT,
                depth_format: DEPTH_FORMAT,
                shininess: 36.0,
                specular: 0.2,
            },
        )
        .map_err(|error| VrmError::Material(path.to_path_buf(), error))?;
        let animation = if let Some(animation_path) = animation_path {
            let bytes = std::fs::read(animation_path)
                .map_err(|error| VrmError::Io(animation_path.to_path_buf(), error))?;
            let animation = scene
                .animation_from_glb(&bytes)
                .map_err(|error| VrmError::Animation(animation_path.to_path_buf(), error))?;
            log::info!(
                "avatar: loaded VRMA {:?}: {:.3}s, {} retargeted channels",
                animation_path,
                animation.duration(),
                animation.channel_count()
            );
            Some(animation)
        } else {
            None
        };
        let surface = Surface::offscreen(device, ATLAS_SIZE, ATLAS_SIZE)
            .map_err(|error| VrmError::Render(path.to_path_buf(), error))?;
        let canvas = Canvas::new(&surface)
            .map_err(|error| VrmError::Render(path.to_path_buf(), error))?;
        let depth = Target::depth(device, ATLAS_SIZE, ATLAS_SIZE, DEPTH_FORMAT)
            .map_err(|error| VrmError::Render(path.to_path_buf(), error))?;
        let rendered = Image::render_target(device, ATLAS_SIZE, ATLAS_SIZE, TARGET_FORMAT)
            .map_err(|error| VrmError::Render(path.to_path_buf(), error))?;
        let texture = Image::render_target(device, ATLAS_SIZE, ATLAS_SIZE, TARGET_FORMAT)
            .map_err(|error| VrmError::Render(path.to_path_buf(), error))?;
        let mut model = Self {
            scene,
            animation,
            material,
            bounds,
            rest_head,
            surface,
            canvas,
            depth,
            rendered,
            texture,
            elapsed: 0.0,
            source_path: path.to_path_buf(),
            animation_path: animation_path.map(Path::to_path_buf),
        };
        model.render()?;
        Ok(model)
    }

    /// Report whether VRMA animation can advance for this model.
    #[must_use]
    pub fn animation_support(&self) -> AnimationSupport {
        if self.animation.is_some() {
            AnimationSupport::Animated
        } else {
            AnimationSupport::Static
        }
    }

    /// The circle-masked portrait texture. Every avatar texture — stills via
    /// the CPU mask, VRM via the analytic rrect blit — is masked in its alpha
    /// channel, so hosts composite directly without their own clip. The lock
    /// screen still applies its own circular clip on top; masking twice is
    /// harmless.
    #[must_use]
    pub fn texture(&self) -> &Image {
        &self.texture
    }

    /// Advance the looping clip and refresh the reusable GPU texture. Returns
    /// `Ok(false)` for a static model so hosts can stay event-driven.
    pub fn advance(&mut self, delta_seconds: f32) -> Result<bool, VrmError> {
        let Some(animation) = &self.animation else {
            return Ok(false);
        };
        let delta = if delta_seconds.is_finite() {
            delta_seconds.max(0.0)
        } else {
            0.0
        };
        let duration = animation.duration();
        self.elapsed = if duration > f32::EPSILON {
            (self.elapsed + delta).rem_euclid(duration)
        } else {
            0.0
        };
        self.render()?;
        Ok(true)
    }

    fn render(&mut self) -> Result<(), VrmError> {
        if let Some(animation) = &self.animation {
            self.scene
                .apply_animation(animation, self.elapsed, true)
                .map_err(|error| {
                    VrmError::Animation(
                        self.animation_path
                            .clone()
                            .unwrap_or_else(|| self.source_path.clone()),
                        error,
                    )
                })?;
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
        let camera = framing_camera(self.bounds, 1.0, head_offset);
        // A soft key close to the portrait camera keeps facial planes readable.
        // `direction` is the direction light travels, hence +Z for a camera on
        // the model's -Z/front side.
        let light = SceneLight {
            direction: [0.25, -0.55, 1.0],
            color: [1.0, 0.97, 0.94],
            ambient: 0.22,
        };
        self.scene
            .draw(&pass, &camera, &self.material, Some(&light));
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
        self.canvas.end_target();
        // The lock screen samples `texture()` directly. Only an explicitly
        // requested debug dump adds the GPU copy to the readable surface;
        // release builds have neither this pass nor a CPU readback path.
        #[cfg(debug_assertions)]
        if std::env::var_os("AEGIS_AVATAR_DEBUG_DUMP").is_some() {
            self.canvas
                .begin(&frame, Some(flux::rgba(0, 0, 0, 0)))
                .map_err(|error| VrmError::Render(self.source_path.clone(), error))?;
            self.canvas.draw_image(
                &self.rendered,
                0.0,
                0.0,
                ATLAS_SIZE as f32,
                ATLAS_SIZE as f32,
            );
            self.canvas.end();
        }
        frame
            .submit()
            .and_then(flux::SubmittedFrame::present)
            .map_err(|error| VrmError::Render(self.source_path.clone(), error))?;

        #[cfg(debug_assertions)]
        if let Some(path) = std::env::var_os("AEGIS_AVATAR_DEBUG_DUMP") {
            let mut pixels = vec![0u8; (ATLAS_SIZE as usize) * (ATLAS_SIZE as usize) * 4];
            if let Err(error) = self.surface.read_pixels(&mut pixels) {
                log::warn!("avatar: could not read debug preview {:?}: {error}", path);
            } else if let Some(square) = image::RgbaImage::from_raw(ATLAS_SIZE, ATLAS_SIZE, pixels)
            {
                let masked = circle_mask_premultiplied(&square);
                if let Err(error) = masked.save(&path) {
                    log::warn!("avatar: could not write debug preview {:?}: {error}", path);
                }
            }
        }
        Ok(())
    }
}

/// Compute a fixed portrait-lens camera from the model bounds. A full-body
/// VRM is deliberately cropped to the upper 25%: a small gap above the hair,
/// the face near center, and the shoulders crossing the lower edge, matching
/// an ID-photo subject/camera relationship rather than a full-body viewer.
fn framing_camera(bounds: Bounds, aspect: f32, tracking_offset: [f32; 3]) -> Camera {
    let frame = portrait_frame(bounds, aspect);
    let center_x = (bounds.min[0] + bounds.max[0]) * 0.5;
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
    let fov_y = PORTRAIT_FOV_DEGREES.to_radians();
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

fn portrait_frame(bounds: Bounds, aspect: f32) -> PortraitFrame {
    let height = (bounds.max[1] - bounds.min[1]).max(0.001);
    let depth = (bounds.max[2] - bounds.min[2]).max(0.001);
    let visible_height = height * PORTRAIT_HEIGHT_RATIO;
    // Deliberately ignore the full model width: humanoid bind poses commonly
    // extend both arms into a T-pose, which would zoom a portrait out until it
    // showed the torso. The circle is allowed to crop arms beyond the shoulder.
    let fitted_height = visible_height / aspect.clamp(0.75, 1.0);
    let center_y = bounds.max[1] - fitted_height * PORTRAIT_TOP_TO_CENTER;
    let fov_y = PORTRAIT_FOV_DEGREES.to_radians();
    let distance = (fitted_height * 0.5) / (fov_y * 0.5).tan();
    PortraitFrame {
        center_y,
        distance,
        depth,
    }
}

const PORTRAIT_FOV_DEGREES: f32 = 28.0;
const PORTRAIT_HEIGHT_RATIO: f32 = 0.25;
const PORTRAIT_TOP_TO_CENTER: f32 = 0.48;

const TARGET_FORMAT: Format = Format::FLUX_FORMAT_RGBA8_UNORM;
const DEPTH_FORMAT: Format = Format::FLUX_FORMAT_D32_SFLOAT;

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
        let frame = portrait_frame(bounds, 1.0);
        // The optical center sits around the upper chest/face, never at the
        // full-body midpoint (0.885 for this fixture).
        assert!(frame.center_y > 1.50);
        assert!(frame.center_y < 1.62);
        assert!(frame.distance > 0.7);
        assert!(frame.distance < 1.2);
    }
}
