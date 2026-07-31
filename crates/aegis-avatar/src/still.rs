//! Still-image avatar path: decode → cover-fit → circle-mask → upload.
//!
//! Any format the workspace `image` crate understands is accepted (PNG, JPEG,
//! WebP, GIF first frame, BMP, ICO, TIFF, TGA, QOI, PNM). The result is a
//! single premultiplied `RGBA8_UNORM` flux texture, masked to a circle so the
//! orb can never overflow its round frame regardless of the source aspect.

use std::path::Path;

use flux::{Device, Format, Image};

use crate::{ATLAS_SIZE, Error, mask::circle_mask_premultiplied};

/// Build the circular still-avatar texture from `path`.
pub fn build(device: &Device, path: &Path) -> Result<Image, Error> {
    // A VRM file reaching this path is a configuration mistake (it belongs in
    // the VRM search order); skip it rather than trying to decode glTF as a
    // raster image.
    if crate::is_vrm_path(path) {
        return Err(Error::Io(
            path.to_path_buf(),
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "vrm/vrma is a 3D model, not a still image",
            ),
        ));
    }
    let decoded = image::open(path).map_err(|error| Error::Decode(path.to_path_buf(), error))?;
    let edge = ATLAS_SIZE;
    // resize_to_fill crops to the target aspect and resamples in one pass,
    // giving a cover-fit: portrait and landscape photos both fill the disc.
    let fitted = decoded.resize_to_fill(edge, edge, image::imageops::FilterType::Lanczos3);
    let square = fitted.to_rgba8();
    let masked = circle_mask_premultiplied(&square);
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
    use super::*;
    use image::GenericImageView;

    #[test]
    fn cover_fit_squares_any_aspect() {
        let landscape = image::DynamicImage::ImageRgba8(
            image::RgbaImage::from_raw(400, 200, vec![255; 400 * 200 * 4]).unwrap(),
        )
        .resize_to_fill(32, 32, image::imageops::FilterType::Nearest);
        assert_eq!(landscape.dimensions(), (32, 32));

        let portrait = image::DynamicImage::ImageRgba8(
            image::RgbaImage::from_raw(100, 300, vec![255; 100 * 300 * 4]).unwrap(),
        )
        .resize_to_fill(32, 32, image::imageops::FilterType::Nearest);
        assert_eq!(portrait.dimensions(), (32, 32));
    }

    #[test]
    fn vrm_path_is_rejected_as_a_still_image() {
        // A `.vrm` on the still search path must be skipped, not force-decoded
        // as a raster image. The extension check is a pure function, so no GPU
        // device is needed to exercise the rejection.
        assert!(crate::is_vrm_path(Path::new("/x/avatar.vrm")));
        assert!(crate::is_vrm_path(Path::new("/x/idle.VRMA")));
        assert!(!crate::is_vrm_path(Path::new("/x/face.png")));
    }
}
