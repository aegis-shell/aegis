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
    cursor: Option<CaptureCursor>,
    pub(super) security_generation: u64,
}

/// Cursor pixels to composite into a saved screenshot after GPU readback.
/// The source is premultiplied BGRA8, matching Xcursor/Flux; `(x, y)` is the
/// physical-pixel top-left after applying the hotspot.
pub(in crate::runtime) struct CaptureCursor {
    pub(in crate::runtime) x: i32,
    pub(in crate::runtime) y: i32,
    pub(in crate::runtime) width: u32,
    pub(in crate::runtime) height: u32,
    pub(in crate::runtime) bgra: std::sync::Arc<[u8]>,
}

pub(in crate::runtime) struct PendingReadback {
    pub(in crate::runtime) width: u32,
    pub(in crate::runtime) height: u32,
    pub(in crate::runtime) crop: Option<aegis_core::Rect>,
    pub(in crate::runtime) cursor: Option<CaptureCursor>,
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
        cursor: pending.cursor,
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
    if let Some(cursor) = capture.cursor {
        composite_cursor(&mut full_rgba, capture.width, capture.height, &cursor);
    }
    encode_rgba_capture(capture.width, capture.height, full_rgba, capture.crop)
}

/// Premultiplied source-over from an Xcursor BGRA sprite into the framebuffer
/// RGBA buffer. Compositing happens before region cropping so a cursor that
/// crosses the selection edge clips naturally.
fn composite_cursor(
    destination: &mut [u8],
    destination_width: u32,
    destination_height: u32,
    cursor: &CaptureCursor,
) {
    let expected_destination = destination_width as usize * destination_height as usize * 4;
    let expected_source = cursor.width as usize * cursor.height as usize * 4;
    if destination.len() < expected_destination || cursor.bgra.len() < expected_source {
        return;
    }

    let left = cursor.x.max(0) as u32;
    let top = cursor.y.max(0) as u32;
    let right =
        (cursor.x.saturating_add_unsigned(cursor.width)).clamp(0, destination_width as i32) as u32;
    let bottom = (cursor.y.saturating_add_unsigned(cursor.height))
        .clamp(0, destination_height as i32) as u32;
    if left >= right || top >= bottom {
        return;
    }

    for destination_y in top..bottom {
        let source_y = (destination_y as i32 - cursor.y) as usize;
        for destination_x in left..right {
            let source_x = (destination_x as i32 - cursor.x) as usize;
            let source_at = (source_y * cursor.width as usize + source_x) * 4;
            let destination_at =
                (destination_y as usize * destination_width as usize + destination_x as usize) * 4;
            let source = &cursor.bgra[source_at..source_at + 4];
            let destination = &mut destination[destination_at..destination_at + 4];
            let alpha = u32::from(source[3]);
            let inverse = 255 - alpha;

            // Xcursor is BGRA; the readback buffer is RGBA.
            for (destination_channel, source_channel) in [(0, 2), (1, 1), (2, 0), (3, 3)] {
                let over = u32::from(source[source_channel])
                    + (u32::from(destination[destination_channel]) * inverse + 127) / 255;
                destination[destination_channel] = over.min(255) as u8;
            }
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cursor_composite_converts_bgra_and_clips_to_the_output() {
        let mut rgba = vec![0u8; 3 * 2 * 4];
        for pixel in rgba.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        let cursor = CaptureCursor {
            x: -1,
            y: 0,
            width: 2,
            height: 2,
            // 50%-opaque premultiplied red in BGRA order.
            bgra: std::sync::Arc::from([0, 0, 128, 128].repeat(4)),
        };

        composite_cursor(&mut rgba, 3, 2, &cursor);

        assert_eq!(&rgba[0..4], &[128, 0, 0, 255]);
        assert_eq!(&rgba[4..8], &[0, 0, 0, 255]);
        assert_eq!(&rgba[12..16], &[128, 0, 0, 255]);
    }
}
