//! VRM avatar animation and offscreen rendering backend.
//!
//! The backend consumes an explicit VRM model, optional legacy VRMA companion,
//! and caller-owned camera parameters. It owns VRM/VRMA parsing, humanoid
//! retargeting, motion state, skinning, and the reusable GPU texture. Account
//! defaults, still portraits, source precedence, XDG discovery, filesystem
//! observation, and surrounding chrome belong to the parent persona layer
//! and presentation hosts.

mod model;
mod motion;

pub use model::{AnimationSupport, CameraError, VrmCamera, VrmError as Error};
pub use motion::{MotionInfo, MotionKind};

use std::path::Path;

use flux::{Device, Image};

/// Square offscreen target edge, in physical pixels.
pub(super) const ATLAS_SIZE: u32 = 256;

/// A prepared VRM avatar with persistent animation and offscreen GPU state.
pub(super) struct Avatar {
    model: Box<model::Model>,
}

impl Avatar {
    /// Load one explicit VRM model and its optional legacy VRMA companion.
    ///
    /// Motion-library directories are discovered beside `model_path`; source
    /// selection and precedence are deliberately not part of this crate.
    pub fn load(
        device: &Device,
        model_path: &Path,
        legacy_motion_path: Option<&Path>,
        camera: VrmCamera,
    ) -> Result<Self, Error> {
        Ok(Self {
            model: Box::new(model::Model::build(
                device,
                model_path,
                legacy_motion_path,
                camera,
            )?),
        })
    }

    /// The reusable portrait texture containing the current VRM frame.
    #[must_use]
    pub fn texture(&self) -> &Image {
        self.model.texture()
    }

    /// Report whether at least one usable VRMA clip was loaded.
    #[must_use]
    pub fn animation_support(&self) -> AnimationSupport {
        self.model.animation_support()
    }

    /// Replace the caller-owned camera parameters and re-render immediately.
    /// Returns `Ok(false)` when the parameters are unchanged.
    pub fn set_camera(&mut self, camera: VrmCamera) -> Result<bool, Error> {
        self.model.set_camera(camera)
    }

    /// Advance the selected motion and refresh the reusable GPU texture.
    pub fn advance(&mut self, delta_seconds: f32) -> Result<bool, Error> {
        self.model.advance(delta_seconds)
    }

    /// Whether a motion is currently selected and needs frame advances.
    #[must_use]
    pub fn is_animated(&self) -> bool {
        self.model.is_playing()
    }

    /// Metadata for every loaded VRMA clip.
    #[must_use]
    pub fn motions(&self) -> Vec<MotionInfo> {
        self.model.motions()
    }

    /// Name of the clip currently advancing, or `None` for a rest pose.
    #[must_use]
    pub fn current_motion(&self) -> Option<&str> {
        self.model.current_motion()
    }

    /// Start a named idle or action clip from its first frame.
    pub fn play_motion(&mut self, name: &str) -> bool {
        self.model.play_motion(name)
    }

    /// Start one action from the non-repeating shuffled action pool.
    pub fn play_random_action(&mut self) -> Option<&str> {
        self.model.play_random_action()
    }
}

/// True when `path` names a VRM model by extension.
#[cfg(test)]
fn is_vrm_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("vrm"))
}

/// True when `path` names a VRM Animation clip.
fn is_vrma_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("vrma"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vrm_extensions_are_recognised() {
        assert!(is_vrm_path(Path::new("/home/me/avatar.vrm")));
        assert!(!is_vrm_path(Path::new("/home/me/idle.VRMA")));
        assert!(!is_vrm_path(Path::new("/home/me/photo.png")));
        assert!(is_vrma_path(Path::new("/home/me/idle.VRMA")));
        assert!(!is_vrma_path(Path::new("/home/me/avatar.vrm")));
    }
}
