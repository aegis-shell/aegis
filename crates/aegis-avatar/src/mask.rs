//! Circle alpha-mask mathematics used only by the opt-in debug readback.

use image::RgbaImage;

/// Anti-alias band width, in atlas pixels, at the [`super::ATLAS_SIZE`] of 256.
/// ~1.5 px produces a soft, alias-free circle edge.
const EDGE_AA: f32 = 1.5;

/// Coverage of a pixel at distance `dist` from the disc centre with the given
/// `radius`, in the `[0, 1]` range. Uses a smoothstep across an `EDGE_AA`-wide
/// band so the silhouette is anti-aliased rather than stair-stepped.
fn coverage(dist: f32, radius: f32) -> f32 {
    ((radius - dist) / EDGE_AA).clamp(0.0, 1.0)
}

/// Premultiply every pixel of `source` by the disc coverage at its position,
/// producing the circle-masked, SRC-over-ready buffer the avatar pipeline
/// uploads. The disc is inscribed in the square with a half-pixel inset so the
/// GPU never samples the texture edge into a visible square.
pub fn circle_mask_premultiplied(source: &RgbaImage) -> RgbaImage {
    let edge = source.width();
    let mut out = RgbaImage::new(edge, edge);
    let center = (edge as f32 - 1.0) * 0.5;
    let radius = center - 0.5;
    for y in 0..edge {
        for x in 0..edge {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let dist = (dx * dx + dy * dy).sqrt();
            let c = coverage(dist, radius);
            let pixel = source.get_pixel(x, y);
            let r = pixel[0] as f32 * c;
            let g = pixel[1] as f32 * c;
            let b = pixel[2] as f32 * c;
            let a = pixel[3] as f32 * c;
            out.put_pixel(
                x,
                y,
                image::Rgba([
                    r.round() as u8,
                    g.round() as u8,
                    b.round() as u8,
                    a.round() as u8,
                ]),
            );
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outside_the_disc_is_fully_transparent() {
        let solid = RgbaImage::from_raw(8, 8, vec![255; 8 * 8 * 4]).unwrap();
        let masked = circle_mask_premultiplied(&solid);
        // Corner pixel is well outside the unit disc → fully transparent.
        assert_eq!(masked.get_pixel(0, 0).0, [0, 0, 0, 0]);
        // Centre pixel is fully inside → fully opaque, premultiplied.
        assert_eq!(masked.get_pixel(4, 4).0, [255, 255, 255, 255]);
    }

    #[test]
    fn coverage_is_clamped_and_centred() {
        // Centre of the disc: full coverage.
        assert_eq!(coverage(0.0, 10.0), 1.0);
        // Well past the edge: zero coverage.
        assert_eq!(coverage(20.0, 10.0), 0.0);
        // Just inside the edge, within the EDGE_AA anti-alias band: a
        // fractional value strictly between 0 and 1. At dist = radius - 0.5
        // the smoothstep sits mid-band.
        let band = coverage(9.5, 10.0);
        assert!(band > 0.0 && band < 1.0, "got {band}");
    }
}
