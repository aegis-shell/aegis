//! User avatar loading and rendering for Aegis.
//!
//! Resolves an avatar source from XDG-conformant locations, then produces a
//! GPU-ready circular texture that any chrome surface (the lock screen, a
//! settings page) composites with a single alpha-aware `draw_image`.
//!
//! Two source kinds are handled:
//!
//! - **Still images** — any format the workspace `image` crate decodes.
//!   Cover-fit to a square, masked to a circle, premultiplied, uploaded once.
//! - **VRM models** — VRM 0.x / VRM 1.0 (`.glb` containers) loaded through
//!   `flux-scene-graph` and rendered offscreen to the same circular texture.
//!   VRMA animation is carried by the API but motion requires skinning/morph
//!   support in the scene graph that is not yet present; see [`AvatarKind`].
//!
//! The crate owns decode and GPU state only. It has no Wayland connection and
//! no presentation loop. See ADR-0080.

mod mask;
mod resolve;
mod still;
mod vrm;

pub use resolve::{candidate_paths, vrm_candidate_paths};
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

/// A prepared avatar: a GPU texture ready to composite, plus metadata about
/// its source kind. Built once (at startup or when the source file changes)
/// and drawn every frame by the caller.
pub struct Avatar {
    pub texture: flux::Image,
    pub kind: AvatarKind,
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
        // Still images take precedence: a user with both a photo and a model
        // almost certainly wants the photo as their identity orb.
        for candidate in candidate_paths() {
            match still::build(device, &candidate) {
                Ok(texture) => {
                    return Ok(Some(Self {
                        texture,
                        kind: AvatarKind::Still,
                    }));
                }
                Err(Error::Io(path, _)) if path == candidate => continue,
                Err(Error::Decode(path, _)) if path == candidate => continue,
                Err(error) => return Err(error),
            }
        }
        // VRM is the deliberate second choice: heavier to render and, until
        // skinning lands, cannot animate.
        for candidate in vrm_candidate_paths() {
            match vrm::Model::build(device, &candidate) {
                Ok(model) => {
                    let texture = model.render_to_circle(device)?;
                    return Ok(Some(Self {
                        texture,
                        kind: AvatarKind::Animated3d {
                            animation: model.animation_support(),
                        },
                    }));
                }
                Err(vrm::VrmError::Io(path, _)) if path == candidate => continue,
                Err(vrm::VrmError::Gltf(path, _)) if path == candidate => continue,
                Err(error) => return Err(error.into()),
            }
        }
        Ok(None)
    }
}

/// True when `path` names a VRM model by extension. Used by resolvers and
/// tests; VRM 0.x and 1.0 both ship as `.glb`, `.vrm` is the conventional
/// alias, and `.vrma` is a VRM Animation clip (treated as a model source so
/// the loader can report its animation status).
pub fn is_vrm_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| ext.eq_ignore_ascii_case("vrm") || ext.eq_ignore_ascii_case("vrma"))
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
        assert!(is_vrm_path(Path::new("/home/me/idle.VRMA")));
        assert!(is_vrm_path(Path::new("/tmp/glb-named-like-this.vrm")));
        assert!(!is_vrm_path(Path::new("/home/me/.face")));
        assert!(!is_vrm_path(Path::new("/home/me/photo.png")));
    }
}
