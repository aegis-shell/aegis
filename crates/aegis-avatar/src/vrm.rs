//! VRM avatar path: load the `.glb`-backed model and render it offscreen to a
//! circular texture.
//!
//! VRM 0.x and VRM 1.0 are both binary glTF containers, so the model loads
//! through `flux_scene_graph::Scene::from_glb`. The scene graph's current
//! supported subset (POSITION/NORMAL/TEXCOORD_0, static node tree, Phong
//! material) does **not** include skins, morph targets, or animation clips —
//! the VRM humanoid skeleton, spring bones, and VRMA clips are therefore not
//! applied yet. A model still renders (as a posed mesh), and
//! [`AnimationSupport`] reports exactly what is and is not possible so callers
//! never silently get a frozen avatar when they expect motion.
//!
//! Rendering follows the established offscreen pattern: a depth-tested scene
//! pass into a sampleable `Image::render_target`, sized to the avatar atlas,
//! then read back and circle-masked so the orb stays a perfect disc.

use std::path::{Path, PathBuf};

use flux::{
    Camera, Format, Image, Material, MaterialDesc, MaterialKind, SceneColorLoad, SceneLight,
    Surface, Target,
};
use flux_scene_graph::{Bounds, Scene};

use crate::{ATLAS_SIZE, Error, mask::circle_mask_premultiplied};

/// What the loaded VRM model can do regarding VRMA animation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AnimationSupport {
    /// VRMA clips can advance frame-to-frame (skin/morph/animation landed in
    /// the scene graph and the model exposes a humanoid rig).
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
}

/// A loaded VRM model and its single-use render resources.
///
/// Built once per source change; [`Model::render_to_circle`] produces the
/// compositable texture. Kept separate from `Avatar` so a future "advance the
/// animation clock and re-render" path can hold the `Scene` across frames.
pub struct Model {
    scene: Scene,
    material: Material,
    bounds: Bounds,
    /// Animation capability detected at load time. Until the scene graph
    /// supports skins/morph/animation this is always `Static`.
    animation: AnimationSupport,
}

impl Model {
    /// Load `path` as a VRM/glTF model and prepare its render material.
    pub fn build(device: &Device, path: &Path) -> Result<Self, VrmError> {
        let bytes = std::fs::read(path).map_err(|error| VrmError::Io(path.to_path_buf(), error))?;
        let scene = Scene::from_glb(device, &bytes)
            .map_err(|error| VrmError::Gltf(path.to_path_buf(), error))?;
        let bounds = scene
            .bounds()
            .ok_or_else(|| VrmError::NoBounds(path.to_path_buf()))?;
        // The color format must match the offscreen render target below; depth
        // matches the scene-pass attachment. Phong gives the avatar readable
        // shading without textures (which the v0.1 loader does not import).
        let material = Material::new(
            device,
            MaterialDesc {
                kind: MaterialKind::Phong,
                base_color: [0.85, 0.85, 0.88, 1.0],
                color_format: TARGET_FORMAT,
                depth_format: DEPTH_FORMAT,
                shininess: 48.0,
                specular: 0.5,
            },
        )
        .map_err(|error| VrmError::Material(path.to_path_buf(), error))?;
        Ok(Self {
            scene,
            material,
            bounds,
            // Skin/morph/animation are not in the flux-scene-graph supported
            // subset yet (scene-graph.h v0.1). Reported honestly here so the
            // lock screen shows the model but never pretends it can animate.
            animation: AnimationSupport::Static,
        })
    }

    /// Report whether VRMA animation can advance for this model.
    #[must_use]
    pub fn animation_support(&self) -> AnimationSupport {
        self.animation
    }

    /// Render the model offscreen into a fresh circular, premultiplied texture
    /// sized to [`ATLAS_SIZE`]. Each call allocates its own offscreen surface,
    /// which is fine for the one-shot lock-screen orb; a per-frame animation
    /// path would cache the surface across frames.
    pub fn render_to_circle(&self, device: &Device) -> Result<Image, Error> {
        let edge = ATLAS_SIZE;
        let surface = Surface::offscreen(device, edge, edge)?;
        let depth = Target::depth(device, edge, edge, DEPTH_FORMAT)?;
        let mut frame = surface.begin_frame()?;
        // Render straight to the surface's color attachment (begin_scene_pass
        // with no image target), clearing to transparent so the circle mask
        // composites cleanly. A square render + square readback + circle mask
        // keeps the VRM and still-image outputs byte-identical in shape.
        let pass = frame.begin_scene_pass(&depth, SceneColorLoad::Clear([0.0; 4]))?;
        let camera = framing_camera(self.bounds, 1.0);
        let light = SceneLight::default();
        self.scene
            .draw(&pass, &camera, &self.material, Some(&light));
        pass.end();
        frame.submit()?.present()?;

        // Read the rendered RGBA back through the offscreen surface (its
        // readback is auto-enabled), mask to the circle, and re-upload.
        let mut pixels = vec![0u8; (edge as usize) * (edge as usize) * 4];
        surface.read_pixels(&mut pixels)?;
        let square = image::RgbaImage::from_raw(edge, edge, pixels).ok_or(flux::Error(
            flux::sys::flux_result::FLUX_ERROR_INVALID_ARGUMENT,
        ))?;
        let masked = circle_mask_premultiplied(&square);
        Image::from_bytes(
            device,
            edge,
            edge,
            Format::FLUX_FORMAT_RGBA8_UNORM,
            masked.as_raw(),
        )
        .map_err(Error::Flux)
    }
}

/// Compute a camera that frames the model's bounding sphere with a margin.
/// Head-and-shoulders VRM models compose best framed on the face, so the
/// camera looks at the bounds centre with the up axis along +Y.
fn framing_camera(bounds: Bounds, aspect: f32) -> Camera {
    let half_diag = bounds.half_diagonal().max(0.001);
    let center = bounds.center();
    // Distance to fit the bounding sphere in a ~45° vertical FOV with margin.
    let fov_y = 45.0_f32.to_radians();
    let distance = half_diag / (fov_y * 0.5).sin() * 1.15;
    // Place the camera looking down the -Z axis at the model centre, slightly
    // above centre to favour the face.
    let eye = [
        center[0],
        center[1] + half_diag * 0.05,
        center[2] + distance,
    ];
    let near = (half_diag * 0.02).max(0.01);
    let far = (distance + half_diag * 2.0) * 5.0 + 1.0;
    let mut camera = Camera::perspective(fov_y, aspect, near, far);
    camera.look_at(eye, center, [0.0, 1.0, 0.0]);
    camera
}

const TARGET_FORMAT: Format = Format::FLUX_FORMAT_RGBA8_UNORM;
const DEPTH_FORMAT: Format = Format::FLUX_FORMAT_D32_SFLOAT;

// The flux Device type alias; kept out of the public surface for clarity.
use flux::Device;
