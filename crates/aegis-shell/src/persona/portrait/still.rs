use std::path::Path;

use flux::{Device, Format, Image};

use super::{Error, mask::circle_mask_premultiplied};

const PORTRAIT_SIZE: u32 = super::vrm::ATLAS_SIZE;

pub fn build(device: &Device, path: &Path) -> Result<Image, Error> {
    let decoded = image::open(path).map_err(|error| Error::Decode(path.to_path_buf(), error))?;
    let fitted = decoded.resize_to_fill(
        PORTRAIT_SIZE,
        PORTRAIT_SIZE,
        image::imageops::FilterType::Lanczos3,
    );
    let masked = circle_mask_premultiplied(&fitted.to_rgba8());
    Image::from_bytes(
        device,
        masked.width(),
        masked.height(),
        Format::FLUX_FORMAT_RGBA8_UNORM,
        masked.as_raw(),
    )
    .map_err(Error::Flux)
}

#[cfg(test)]
mod tests {
    use image::GenericImageView;

    #[test]
    fn cover_fit_squares_any_aspect() {
        let landscape = image::DynamicImage::ImageRgba8(
            image::RgbaImage::from_raw(400, 200, vec![255; 400 * 200 * 4]).unwrap(),
        )
        .resize_to_fill(32, 32, image::imageops::FilterType::Nearest);
        assert_eq!(landscape.dimensions(), (32, 32));
    }
}
