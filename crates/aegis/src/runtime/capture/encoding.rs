use super::geometry::crop_rgba;

/// Immutable GPU readback staging detached from the presentation surface and
/// handed to the capture worker. `crop` is already converted to physical
/// pixels; the full CPU copy and every later operation stay off the
/// compositor's presentation-critical thread.
pub(in crate::runtime) struct CapturedPixels {
    width: u32,
    height: u32,
    readback: flux::Readback,
    crop: Option<aegis_core::Rect>,
    pub(super) security_generation: u64,
}

pub(in crate::runtime) struct PendingReadback {
    pub(in crate::runtime) width: u32,
    pub(in crate::runtime) height: u32,
    pub(in crate::runtime) crop: Option<aegis_core::Rect>,
    pub(in crate::runtime) security_generation: u64,
}

pub(in crate::runtime) fn read_captured_pixels(
    surface: &flux::Surface,
    pending: PendingReadback,
) -> Result<CapturedPixels, String> {
    let readback = surface
        .take_readback()
        .map_err(|error| format!("detach shot readback: {error}{}", flux_last_error_detail()))?;
    Ok(CapturedPixels {
        width: pending.width,
        height: pending.height,
        readback,
        crop: pending.crop,
        security_generation: pending.security_generation,
    })
}

/// Finish a capture away from the frame thread. Cropping first bounds the
/// unpremultiply and PNG work for region captures.
pub(super) fn encode_capture(capture: CapturedPixels) -> Result<(u32, u32, Vec<u8>), String> {
    let mut full_rgba = vec![0u8; capture.width as usize * capture.height as usize * 4];
    capture
        .readback
        .read_pixels(&mut full_rgba)
        .map_err(|error| format!("shot pixel copy: {error}"))?;
    encode_rgba_capture(capture.width, capture.height, full_rgba, capture.crop)
}

pub(in crate::runtime) fn encode_rgba_capture(
    full_width: u32,
    full_height: u32,
    full_rgba: Vec<u8>,
    crop: Option<aegis_core::Rect>,
) -> Result<(u32, u32, Vec<u8>), String> {
    let (width, height, mut rgba) = match crop {
        Some(crop) => (
            crop.size.w as u32,
            crop.size.h as u32,
            crop_rgba(&full_rgba, full_width, full_height, crop),
        ),
        None => (full_width, full_height, full_rgba),
    };
    unpremultiply(&mut rgba);
    let png = encode_png(width, height, &rgba)?;
    Ok((width, height, png))
}

/// Convert premultiplied RGBA8 (the flux/Wayland contract) to the straight
/// alpha PNG encoders expect.
fn unpremultiply(pixels: &mut [u8]) {
    for px in pixels.chunks_exact_mut(4) {
        let alpha = u32::from(px[3]);
        if alpha > 0 && alpha < 255 {
            for channel in &mut px[0..3] {
                *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
            }
        }
    }
}

/// Read back one user-picked pixel (ADR-0054). The crop marks the picked
/// spot in physical pixels; the returned RGB is straight-alpha, matching
/// the portal's `(ddd)` colour contract after a `/255` normalization.
pub(in crate::runtime) fn read_picked_pixel(capture: CapturedPixels) -> Result<[u8; 3], String> {
    let mut full_rgba = vec![0u8; capture.width as usize * capture.height as usize * 4];
    capture
        .readback
        .read_pixels(&mut full_rgba)
        .map_err(|error| format!("picked-pixel copy: {error}"))?;
    let crop = capture.crop.ok_or("pixel pick lost its crop rect")?;
    let x = crop.origin.x.clamp(0, capture.width as i32 - 1) as usize;
    let y = crop.origin.y.clamp(0, capture.height as i32 - 1) as usize;
    let at = (y * capture.width as usize + x) * 4;
    unpremultiply(&mut full_rgba[at..at + 4]);
    Ok([full_rgba[at], full_rgba[at + 1], full_rgba[at + 2]])
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;

    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
        .map_err(|error| format!("png encode: {error}"))?;
    Ok(out)
}

/// Convert a readback to one raw stream frame (ADR-0052): the flux/Wayland
/// contract is premultiplied RGBA8, the wire format is opaque BGRA8 with the
/// alpha channel forced to 255. Composited desktop frames are opaque, so the
/// premultiplied→straight distinction cannot produce visible error here;
/// the swap is a pure byte shuffle.
pub(in crate::runtime) fn stream_pixels(
    capture: CapturedPixels,
) -> Result<super::worker::StreamPixels, String> {
    let mut rgba = vec![0u8; capture.width as usize * capture.height as usize * 4];
    capture
        .readback
        .read_pixels(&mut rgba)
        .map_err(|error| format!("stream pixel copy: {error}"))?;
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = 255;
    }
    Ok(super::worker::StreamPixels {
        width: capture.width,
        height: capture.height,
        bgra: rgba.into(),
        damage: Vec::new(),
    })
}

/// flux's thread-local diagnostic for the most recent error, formatted for
/// logs; empty when the call carried no detail.
pub(in crate::runtime) fn flux_last_error_detail() -> String {
    let mut info: flux_sys::flux_error_info = unsafe { std::mem::zeroed() };
    unsafe { flux_sys::flux_get_last_error(&mut info) };
    if info.message.is_null() {
        return String::new();
    }
    let message = unsafe { std::ffi::CStr::from_ptr(info.message) };
    format!(" ({})", message.to_string_lossy())
}
