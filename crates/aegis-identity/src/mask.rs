use image::RgbaImage;

const EDGE_AA: f32 = 1.5;

fn coverage(dist: f32, radius: f32) -> f32 {
    ((radius - dist) / EDGE_AA).clamp(0.0, 1.0)
}

pub fn circle_mask_premultiplied(source: &RgbaImage) -> RgbaImage {
    let edge = source.width();
    let mut out = RgbaImage::new(edge, edge);
    let center = (edge as f32 - 1.0) * 0.5;
    let radius = center - 0.5;
    for y in 0..edge {
        for x in 0..edge {
            let dx = x as f32 - center;
            let dy = y as f32 - center;
            let c = coverage((dx * dx + dy * dy).sqrt(), radius);
            let pixel = source.get_pixel(x, y);
            out.put_pixel(
                x,
                y,
                image::Rgba([
                    (pixel[0] as f32 * c).round() as u8,
                    (pixel[1] as f32 * c).round() as u8,
                    (pixel[2] as f32 * c).round() as u8,
                    (pixel[3] as f32 * c).round() as u8,
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
    fn outside_the_disc_is_transparent() {
        let solid = RgbaImage::from_raw(8, 8, vec![255; 8 * 8 * 4]).unwrap();
        let masked = circle_mask_premultiplied(&solid);
        assert_eq!(masked.get_pixel(0, 0).0, [0, 0, 0, 0]);
        assert_eq!(masked.get_pixel(4, 4).0, [255, 255, 255, 255]);
    }
}
