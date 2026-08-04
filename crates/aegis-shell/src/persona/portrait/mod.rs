//! Personalized portrait content shared by Aegis presentation surfaces.
//!
//! Every consumer uses the same [`PortraitConfig`], source precedence,
//! still-image normalization, VRM backend, and transactional watcher.
//! Presentation hosts own fallback visuals and surrounding chrome.

mod mask;
mod source;
mod still;
mod vrm;
mod watch;

pub use source::{PortraitCandidate, PortraitConfig};
pub use vrm::{
    AnimationSupport, CameraError, Error as VrmError, MotionInfo, MotionKind, VrmCamera,
};
pub use watch::{PortraitWatcher, WatchError};

use std::path::PathBuf;

use flux::{Device, Image};

/// Errors raised while resolving or preparing shared portrait content.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("persona portrait: decode {0:?}")]
    Decode(PathBuf, #[source] image::ImageError),
    #[error("persona portrait: {0}")]
    Avatar(#[from] VrmError),
    #[error("persona portrait: texture upload failed: {0}")]
    Flux(#[from] flux::Error),
    #[error("persona portrait: configured sources exist but none could be decoded")]
    NoUsableSource,
}

/// Selected portrait representation after applying [`PortraitConfig`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PortraitKind {
    Still,
    Vrm { animation: AnimationSupport },
}

enum Content {
    Still(Image),
    Vrm(Box<vrm::Avatar>),
}

/// Prepared portrait content selected through the shared persona contract.
pub struct Portrait {
    content: Content,
}

impl Portrait {
    /// Resolve and build the first usable configured candidate.
    pub fn load(
        device: &Device,
        config: &PortraitConfig,
        camera: VrmCamera,
    ) -> Result<Option<Self>, Error> {
        for candidate in config.candidates() {
            match candidate {
                PortraitCandidate::Still(path) => match still::build(device, path) {
                    Ok(texture) => {
                        return Ok(Some(Self {
                            content: Content::Still(texture),
                        }));
                    }
                    Err(Error::Decode(error_path, _)) if error_path == *path => continue,
                    Err(error) => return Err(error),
                },
                PortraitCandidate::Vrm {
                    model,
                    legacy_motion,
                } => {
                    let legacy_motion = legacy_motion.is_file().then_some(legacy_motion.as_path());
                    match vrm::Avatar::load(device, model, legacy_motion, camera) {
                        Ok(avatar) => {
                            return Ok(Some(Self {
                                content: Content::Vrm(Box::new(avatar)),
                            }));
                        }
                        Err(vrm::Error::Io(path, _)) if path == *model => continue,
                        Err(vrm::Error::Gltf(path, _)) if path == *model => continue,
                        Err(error) => return Err(error.into()),
                    }
                }
            }
        }
        Ok(None)
    }

    /// Build a complete live-reload replacement while distinguishing a true
    /// deletion from a temporarily malformed configured source.
    pub fn load_transactional(
        device: &Device,
        config: &PortraitConfig,
        camera: VrmCamera,
    ) -> Result<Option<Self>, Error> {
        let loaded = Self::load(device, config, camera)?;
        if loaded.is_none() && config.has_existing_source() {
            Err(Error::NoUsableSource)
        } else {
            Ok(loaded)
        }
    }

    #[must_use]
    pub fn kind(&self) -> PortraitKind {
        match &self.content {
            Content::Still(_) => PortraitKind::Still,
            Content::Vrm(avatar) => PortraitKind::Vrm {
                animation: avatar.animation_support(),
            },
        }
    }

    #[must_use]
    pub fn texture(&self) -> &Image {
        match &self.content {
            Content::Still(texture) => texture,
            Content::Vrm(avatar) => avatar.texture(),
        }
    }

    pub fn set_camera(&mut self, camera: VrmCamera) -> Result<bool, Error> {
        match &mut self.content {
            Content::Still(_) => Ok(false),
            Content::Vrm(avatar) => avatar.set_camera(camera).map_err(Error::Avatar),
        }
    }

    pub fn advance(&mut self, delta_seconds: f32) -> Result<bool, Error> {
        match &mut self.content {
            Content::Still(_) => Ok(false),
            Content::Vrm(avatar) => avatar.advance(delta_seconds).map_err(Error::Avatar),
        }
    }

    #[must_use]
    pub fn is_animated(&self) -> bool {
        matches!(&self.content, Content::Vrm(avatar) if avatar.is_animated())
    }

    #[must_use]
    pub fn motions(&self) -> Vec<MotionInfo> {
        match &self.content {
            Content::Still(_) => Vec::new(),
            Content::Vrm(avatar) => avatar.motions(),
        }
    }

    #[must_use]
    pub fn current_motion(&self) -> Option<&str> {
        match &self.content {
            Content::Still(_) => None,
            Content::Vrm(avatar) => avatar.current_motion(),
        }
    }

    pub fn play_motion(&mut self, name: &str) -> bool {
        match &mut self.content {
            Content::Still(_) => false,
            Content::Vrm(avatar) => avatar.play_motion(name),
        }
    }

    pub fn play_random_action(&mut self) -> Option<&str> {
        match &mut self.content {
            Content::Still(_) => None,
            Content::Vrm(avatar) => avatar.play_random_action(),
        }
    }
}
