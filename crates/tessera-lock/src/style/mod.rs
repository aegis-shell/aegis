//! Lock-screen compositions, one module per style.
//!
//! [`common`] defines the shared [`StylePainter`][common::StylePainter]
//! framework every composition plugs into; `centered`, `cinematic`, and
//! `bsod` each own one composition end to end (background, materials,
//! clock, identity). [`qr`] is the dependency-free QR encoder backing the
//! stop-screen easter egg.

pub mod bsod;
pub mod centered;
pub mod cinematic;
pub mod common;
pub(crate) mod qr;

use tessera_config::LockScreenStyle;
use flux::{Canvas, GradientStop};

pub use common::FramePresentation;
pub(crate) use common::painter_for;

use crate::render::LockBackground;
use centered::centered_scrim_stops;
use cinematic::cinematic_scrim_stops;

/// Shared artwork background for the wallpaper-backed compositions: cover
/// art or a configured solid, the dim scrim, and the style's gradient wash.
/// Compositions with their own identity color (the stop screen) never call
/// this.
pub(crate) fn paint_artwork_background(
    canvas: &Canvas,
    device: &flux::Device,
    background: &mut LockBackground,
    output: (u32, u32),
    style: LockScreenStyle,
    dim: f32,
) {
    let artwork = matches!(&*background, LockBackground::Wallpaper(_));
    match background {
        LockBackground::Wallpaper(wallpaper) => {
            wallpaper.draw_cover(device, canvas, output.0 as f32, output.1 as f32);
        }
        LockBackground::Solid([red, green, blue]) => {
            canvas.fill_rect(
                0.0,
                0.0,
                output.0 as f32,
                output.1 as f32,
                flux::rgba(*red, *green, *blue, 255),
            );
        }
    }
    if !artwork {
        return;
    }
    let dim = (dim.clamp(0.0, 0.85) * 255.0).round() as u8;
    canvas.fill_rect(
        0.0,
        0.0,
        output.0 as f32,
        output.1 as f32,
        flux::rgba(3, 6, 12, dim),
    );
    let (start, end, stops): ((f32, f32), (f32, f32), &[GradientStop]) = match style {
        LockScreenStyle::Centered => ((0.0, 0.0), (0.0, output.1 as f32), &centered_scrim_stops()),
        LockScreenStyle::Cinematic | LockScreenStyle::Bsod => (
            (0.0, output.1 as f32 * 0.18),
            (output.0 as f32, output.1 as f32),
            &cinematic_scrim_stops(),
        ),
    };
    canvas.fill_rect_linear_gradient(
        (0.0, 0.0, output.0 as f32, output.1 as f32),
        start,
        end,
        stops,
    );
}
