//! User avatar loading and rendering for Aegis.
//!
//! Resolves an avatar source from XDG-conformant locations, then produces a
//! GPU-ready portrait texture that any chrome surface (the lock screen, the
//! command panel, a settings page) composites directly.
//!
//! **Contract: every avatar texture is circle-masked in its alpha channel.**
//! Hosts draw it as-is, without their own clip:
//!
//! - **Still images** — any format the workspace `image` crate decodes.
//!   Cover-fit to a square, masked to a circle on the CPU, premultiplied,
//!   uploaded once.
//! - **VRM models** — VRM 0.x / VRM 1.0 (`.glb` containers) loaded through
//!   `flux-scene-graph` and rendered offscreen to the same portrait atlas,
//!   then blitted through an analytic rounded-rect clip into the published
//!   texture. A companion `.vrma` is retargeted and rendered into a
//!   persistent texture.
//!
//! The crate owns decode and GPU state only. It has no Wayland connection and
//! no presentation loop. See ADR-0080.

mod mask;
mod resolve;
mod still;
mod vrm;

pub use resolve::{candidate_paths, vrm_candidate_paths, vrma_candidate_paths};
pub use vrm::AnimationSupport;

use std::path::{Path, PathBuf};

use flux::{Device, Format, Image};

/// Square atlas edge, in physical pixels, that every avatar is rasterised to.
/// 256 is crisp at the largest orb size (96 logical px × ~2 scale) and stays
/// under a single 256 KiB RGBA upload.
pub const ATLAS_SIZE: u32 = 256;

/// Errors raised while loading or preparing an avatar.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("avatar: decode {0:?}")]
    Decode(PathBuf, #[source] image::ImageError),
    #[error("avatar: read {0:?}")]
    Io(PathBuf, #[source] std::io::Error),
    #[error("avatar: {0}")]
    Vrm(#[from] vrm::VrmError),
    #[error("avatar: texture upload failed: {0}")]
    Flux(#[from] flux::Error),
}

/// What an [`Avatar`] was built from, and what its renderer can do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AvatarKind {
    /// A still image. Always single-texture, no per-frame work.
    Still,
    /// A 3D VRM model. `animation` describes whether VRMA clips can advance;
    /// see [`AnimationSupport`].
    Animated3d { animation: AnimationSupport },
}

enum Content {
    Still(Image),
    Model(vrm::Model),
}

/// A prepared avatar. Still images own one immutable texture; VRM avatars own
/// their scene, clip, clock, and reusable offscreen texture for zero-readback
/// animation.
pub struct Avatar {
    content: Content,
    kind: AvatarKind,
}

impl Avatar {
    /// Resolve and build the user's avatar from the XDG-conformant search
    /// path, in precedence order. A still image always wins over a VRM model
    /// when both are configured.
    ///
    /// Returns `Ok(None)` only when no candidate exists at all, so the caller
    /// can fall back to its own procedural orb. Any candidate that is merely
    /// missing or in a format this build cannot decode is skipped in favour
    /// of the next candidate.
    pub fn load(device: &Device) -> Result<Option<Self>, Error> {
        let load_still = || -> Result<Option<Self>, Error> {
            for candidate in candidate_paths() {
                match still::build(device, &candidate) {
                    Ok(texture) => {
                        return Ok(Some(Self {
                            content: Content::Still(texture),
                            kind: AvatarKind::Still,
                        }));
                    }
                    Err(Error::Io(path, _)) if path == candidate => continue,
                    Err(Error::Decode(path, _)) if path == candidate => continue,
                    Err(error) => return Err(error),
                }
            }
            Ok(None)
        };

        // Still images take precedence: a user with both a photo and a model
        // almost certainly wants the photo as their identity orb. The explicit
        // debug override is the exception: it must win so it reliably previews
        // the ignored source-tree fixture on machines that already have a face.
        let debug_override = resolve::debug_assets_enabled();
        if !debug_override && let Some(avatar) = load_still()? {
            return Ok(Some(avatar));
        }
        // VRM is the deliberate second choice: heavier to render than a photo.
        for (candidate, motion) in vrm_candidate_paths()
            .into_iter()
            .zip(vrma_candidate_paths())
        {
            let motion = motion.is_file().then_some(motion.as_path());
            match vrm::Model::build(device, &candidate, motion) {
                Ok(model) => {
                    let animation = model.animation_support();
                    return Ok(Some(Self {
                        content: Content::Model(model),
                        kind: AvatarKind::Animated3d { animation },
                    }));
                }
                Err(vrm::VrmError::Io(path, _)) if path == candidate => continue,
                Err(vrm::VrmError::Gltf(path, _)) if path == candidate => continue,
                Err(error) => return Err(error.into()),
            }
        }
        if debug_override {
            load_still()
        } else {
            Ok(None)
        }
    }

    #[must_use]
    pub fn kind(&self) -> AvatarKind {
        self.kind
    }

    #[must_use]
    pub fn texture(&self) -> &Image {
        match &self.content {
            Content::Still(texture) => texture,
            Content::Model(model) => model.texture(),
        }
    }

    /// Advance an animated avatar and refresh its texture. Static images and
    /// VRMs without a companion clip return `Ok(false)` without GPU work.
    pub fn advance(&mut self, delta_seconds: f32) -> Result<bool, Error> {
        match &mut self.content {
            Content::Still(_) => Ok(false),
            Content::Model(model) => model.advance(delta_seconds).map_err(Error::Vrm),
        }
    }

    #[must_use]
    pub fn is_animated(&self) -> bool {
        matches!(
            self.kind,
            AvatarKind::Animated3d {
                animation: AnimationSupport::Animated
            }
        )
    }
}

/// True when `path` names a VRM model by extension. VRM 0.x and 1.0 both use
/// a binary glTF container with the conventional `.vrm` extension.
pub fn is_vrm_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("vrm"))
}

/// True when `path` names a VRM Animation clip. A VRMA is paired with a VRM
/// model and never loaded as a renderable scene by itself.
pub fn is_vrma_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("vrma"))
}

/// Build the procedural gradient orb as a circle-masked texture, used when no
/// user avatar is configured. Rasterising the gradient here (instead of via
/// Flux's `fill_rect_radial_gradient`, which paints a full square) means the
/// disc boundary lives in the texture's alpha channel and can never leak
/// square corners — exactly the property a loaded photo avatar already has.
pub fn procedural_orb(device: &Device) -> Result<Image, Error> {
    let edge = ATLAS_SIZE;
    let mut buffer = image::RgbaImage::new(edge, edge);
    let center = (edge as f32 - 1.0) * 0.5;
    let radius = center - 0.5;
    let aa = 1.5_f32;
    // Gradient control point: a cool-blue radial highlight offset toward the
    // upper-left, matching the historical orb look.
    let (gx, gy) = (center, center - edge as f32 * 0.14);
    let gr = edge as f32 * 0.9;
    for y in 0..edge {
        for x in 0..edge {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            let coverage = ((radius - dist) / aa).clamp(0.0, 1.0);
            let gd = ((x as f32 - gx).powi(2) + (y as f32 - gy).powi(2)).sqrt();
            let t = (gd / gr).clamp(0.0, 1.0);
            let (r, g, b) = gradient_stop(t);
            buffer.put_pixel(
                x,
                y,
                image::Rgba([
                    (r * coverage).round() as u8,
                    (g * coverage).round() as u8,
                    (b * coverage).round() as u8,
                    (255.0 * coverage).round() as u8,
                ]),
            );
        }
    }
    Image::from_bytes(
        device,
        edge,
        edge,
        Format::FLUX_FORMAT_RGBA8_UNORM,
        buffer.as_raw(),
    )
    .map_err(Error::Flux)
}

/// Three-stop cool-blue gradient sampled at normalised position `t ∈ [0,1]`.
fn gradient_stop(t: f32) -> (f32, f32, f32) {
    let stops: [(f32, [f32; 3]); 3] = [
        (0.0, [158.0, 195.0, 255.0]),
        (0.55, [83.0, 125.0, 207.0]),
        (1.0, [35.0, 57.0, 105.0]),
    ];
    if t <= stops[0].0 {
        return (stops[0].1[0], stops[0].1[1], stops[0].1[2]);
    }
    for window in stops.windows(2) {
        let (t0, c0) = window[0];
        let (t1, c1) = window[1];
        if t <= t1 {
            let f = (t - t0) / (t1 - t0).max(1e-6);
            return (
                c0[0] + (c1[0] - c0[0]) * f,
                c0[1] + (c1[1] - c0[1]) * f,
                c0[2] + (c1[2] - c0[2]) * f,
            );
        }
    }
    (stops[2].1[0], stops[2].1[1], stops[2].1[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vrm_extensions_are_recognised() {
        assert!(is_vrm_path(Path::new("/home/me/avatar.vrm")));
        assert!(is_vrm_path(Path::new("/tmp/glb-named-like-this.vrm")));
        assert!(!is_vrm_path(Path::new("/home/me/idle.VRMA")));
        assert!(!is_vrm_path(Path::new("/home/me/.face")));
        assert!(!is_vrm_path(Path::new("/home/me/photo.png")));
        assert!(is_vrma_path(Path::new("/home/me/idle.VRMA")));
        assert!(!is_vrma_path(Path::new("/home/me/avatar.vrm")));
    }
}
