use super::geometry::crop_rgba;

/// Immutable GPU readback staging detached from the presentation surface and
/// handed to the capture worker. Region captures already contain only their
/// physical selection; the legacy `crop` field remains for non-frame sources
/// that still hand us a full readback. Every CPU operation stays off the
/// compositor's presentation-critical thread.
pub(in crate::runtime) struct CapturedPixels {
    width: u32,
    height: u32,
    pixels: CapturedPixelsSource,
    crop: Option<tessera_model::Rect>,
    cursor: Option<CaptureCursor>,
    pub(super) security_generation: u64,
}

/// CPU-side source of a captured frame. The windowed presentation surface
/// (and exportable offscreen targets) detach an on-demand [`flux::Readback`]
/// snapshot whose staging can outlive the surface; `require_readback` surfaces
/// such as Interaction Domain render targets keep their staging surface-owned
/// (flux refuses `take_readback` there), so the frame is copied into an owned
/// buffer on the main loop instead.
pub(in crate::runtime) enum CapturedPixelsSource {
    Readback(flux::Readback),
    Rgba(Vec<u8>),
}

/// Cursor pixels to composite into a saved screenshot after GPU readback.
/// The source is premultiplied BGRA8, matching Xcursor/Flux; `(x, y)` is the
/// physical-pixel top-left after applying the hotspot.
#[derive(Clone)]
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
    pub(in crate::runtime) crop: Option<tessera_model::Rect>,
    pub(in crate::runtime) cursor: Option<CaptureCursor>,
    pub(in crate::runtime) security_generation: u64,
}

/// Bind a screenshot readback and describe the tightly packed CPU-side layout
/// it will produce. Region captures stay regional end-to-end: Flux copies only
/// the requested physical rectangle, the worker allocates only that extent,
/// and no later CPU crop is needed.
pub(in crate::runtime) fn request_frame_readback(
    frame: &mut flux::Frame,
    full_size: (u32, u32),
    crop: Option<tessera_model::Rect>,
    mut cursor: Option<CaptureCursor>,
    security_generation: u64,
) -> Result<PendingReadback, String> {
    let (width, height, crop) = if let Some(crop) = crop {
        if crop.origin.x < 0 || crop.origin.y < 0 || crop.size.w <= 0 || crop.size.h <= 0 {
            return Err("frame readback request has an empty or out-of-bounds region".into());
        }
        let region = flux::ReadbackRegion {
            x: crop.origin.x as u32,
            y: crop.origin.y as u32,
            width: crop.size.w as u32,
            height: crop.size.h as u32,
        };
        frame.request_readback_region(region).map_err(|error| {
            format!(
                "frame region readback request: {error}{}",
                flux_last_error_detail()
            )
        })?;
        translate_cursor_to_region(&mut cursor, crop);
        (region.width, region.height, None)
    } else {
        frame.request_readback().map_err(|error| {
            format!(
                "frame readback request: {error}{}",
                flux_last_error_detail()
            )
        })?;
        (full_size.0, full_size.1, None)
    };
    Ok(PendingReadback {
        width,
        height,
        crop,
        cursor,
        security_generation,
    })
}

fn translate_cursor_to_region(cursor: &mut Option<CaptureCursor>, region: tessera_model::Rect) {
    if let Some(cursor) = cursor.as_mut() {
        cursor.x = cursor.x.saturating_sub(region.origin.x);
        cursor.y = cursor.y.saturating_sub(region.origin.y);
    }
}

pub(in crate::runtime) fn read_captured_pixels(
    surface: &flux::Surface,
    pending: PendingReadback,
) -> Result<CapturedPixels, String> {
    let readback = surface
        .take_readback()
        .map_err(|error| format!("detach shot readback: {error}{}", flux_last_error_detail()))?;
    let region = readback.region();
    if region.width != pending.width || region.height != pending.height {
        return Err(format!(
            "shot readback extent mismatch: requested {}x{}, received {}x{}",
            pending.width, pending.height, region.width, region.height
        ));
    }
    Ok(CapturedPixels {
        width: pending.width,
        height: pending.height,
        pixels: CapturedPixelsSource::Readback(readback),
        crop: pending.crop,
        cursor: pending.cursor,
        security_generation: pending.security_generation,
    })
}

/// Copy a completed frame from a `require_readback` surface directly into an
/// owned buffer. Flux keeps require-readback staging surface-owned and refuses
/// [`flux::Surface::take_readback`] there, so the pixel copy happens on the
/// main loop once [`flux::Surface::read_pixels_ready`] reports the frame; the
/// capture worker only encodes the already CPU-side bytes.
pub(in crate::runtime) fn read_captured_pixels_owned(
    surface: &flux::Surface,
    pending: PendingReadback,
) -> Result<CapturedPixels, String> {
    let mut full_rgba = vec![0u8; pending.width as usize * pending.height as usize * 4];
    surface
        .read_pixels(&mut full_rgba)
        .map_err(|error| format!("detach shot readback: {error}{}", flux_last_error_detail()))?;
    Ok(CapturedPixels {
        width: pending.width,
        height: pending.height,
        pixels: CapturedPixelsSource::Rgba(full_rgba),
        crop: pending.crop,
        cursor: pending.cursor,
        security_generation: pending.security_generation,
    })
}

/// Finish a capture away from the frame thread. Cropping first bounds the
/// unpremultiply and PNG work for region captures.
pub(super) fn encode_capture(capture: CapturedPixels) -> Result<(u32, u32, Vec<u8>), String> {
    let CapturedPixels {
        width,
        height,
        pixels,
        crop,
        cursor,
        security_generation: _,
    } = capture;
    let mut full_rgba = match pixels {
        CapturedPixelsSource::Readback(readback) => {
            let mut full_rgba = vec![0u8; width as usize * height as usize * 4];
            readback
                .read_pixels(&mut full_rgba)
                .map_err(|error| format!("shot pixel copy: {error}"))?;
            full_rgba
        }
        CapturedPixelsSource::Rgba(full_rgba) => full_rgba,
    };
    if let Some(cursor) = cursor {
        composite_cursor(&mut full_rgba, width, height, &cursor);
    }
    encode_rgba_capture(width, height, full_rgba, crop)
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
    composite_cursor_impl(
        destination,
        destination_width,
        destination_height,
        cursor,
        true,
    );
}

/// Premultiplied source-over from an Xcursor BGRA sprite into a BGRA stream
/// frame (cursor-embedding output streams, IPC protocol 29). Identical math
/// to [`composite_cursor`]; source and destination channel orders match, so
/// no swap.
fn composite_cursor_bgra(
    destination: &mut [u8],
    destination_width: u32,
    destination_height: u32,
    cursor: &CaptureCursor,
) {
    composite_cursor_impl(
        destination,
        destination_width,
        destination_height,
        cursor,
        false,
    );
}

fn composite_cursor_impl(
    destination: &mut [u8],
    destination_width: u32,
    destination_height: u32,
    cursor: &CaptureCursor,
    rgba_destination: bool,
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

            // Xcursor is BGRA; an RGBA destination swaps the colour channels,
            // a BGRA destination (stream frames) copies them straight.
            let channels: [(usize, usize); 4] = if rgba_destination {
                [(0, 2), (1, 1), (2, 0), (3, 3)]
            } else {
                [(0, 0), (1, 1), (2, 2), (3, 3)]
            };
            for (destination_channel, source_channel) in channels {
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
    crop: Option<tessera_model::Rect>,
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

/// Read back one user-picked pixel (ADR-0054). Region-capable frame captures
/// place the picked rectangle at buffer origin; legacy full-frame sources may
/// still carry a CPU crop. The returned RGB is straight-alpha, matching the
/// portal's `(ddd)` colour contract after a `/255` normalization.
pub(in crate::runtime) fn read_picked_pixel(capture: CapturedPixels) -> Result<[u8; 3], String> {
    let CapturedPixels {
        width,
        height,
        pixels,
        crop,
        cursor: _,
        security_generation: _,
    } = capture;
    let mut full_rgba = match pixels {
        CapturedPixelsSource::Readback(readback) => {
            let mut full_rgba = vec![0u8; width as usize * height as usize * 4];
            readback
                .read_pixels(&mut full_rgba)
                .map_err(|error| format!("picked-pixel copy: {error}"))?;
            full_rgba
        }
        CapturedPixelsSource::Rgba(full_rgba) => full_rgba,
    };
    // Region readback already rebases the picked rectangle to (0, 0). Keep
    // accepting a legacy CPU crop for full-frame/Interaction Domain sources during the
    // transition to region-capable targets.
    let (x, y) = crop.map_or((0, 0), |crop| {
        (
            crop.origin.x.clamp(0, width as i32 - 1) as usize,
            crop.origin.y.clamp(0, height as i32 - 1) as usize,
        )
    });
    let at = (y * width as usize + x) * 4;
    unpremultiply(&mut full_rgba[at..at + 4]);
    Ok([full_rgba[at], full_rgba[at + 1], full_rgba[at + 2]])
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, String> {
    use image::ImageEncoder;

    let expected = usize::try_from(width)
        .ok()
        .and_then(|width| {
            usize::try_from(height)
                .ok()
                .and_then(|height| width.checked_mul(height))
        })
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| format!("png dimensions {width}x{height} overflow address space"))?;
    if rgba.len() != expected {
        return Err(format!(
            "png RGBA length mismatch: {width}x{height} needs {expected} bytes, received {}",
            rgba.len()
        ));
    }

    let mut out = Vec::new();
    // Interactive screenshots prefer predictable latency over the last few
    // percent of file-size reduction. `Up` is a cheap, lossless scanline
    // filter that performs well on vertically coherent desktop pixels; unlike
    // `NoFilter`, it also keeps the deflate input compact enough that Fast
    // compression does less work overall. File persistence still happens on
    // the background worker and consumes this exact byte stream.
    image::codecs::png::PngEncoder::new_with_quality(
        &mut out,
        image::codecs::png::CompressionType::Fast,
        image::codecs::png::FilterType::Up,
    )
    .write_image(rgba, width, height, image::ExtendedColorType::Rgba8)
    .map_err(|error| format!("png encode: {error}"))?;
    Ok(out)
}

/// Convert a readback to one raw stream frame (ADR-0052): the flux/Wayland
/// contract is premultiplied RGBA8, the wire format is opaque BGRA8 with the
/// alpha channel forced to 255. Composited desktop frames are opaque, so the
/// premultiplied→straight distinction cannot produce visible error here;
/// the swap is a pure byte shuffle.
///
/// When the binding attached a cursor snapshot (at least one due SHM output
/// stream negotiated `embedded`), a second, cursor-composited copy of the
/// frame is produced alongside the pristine one (ADR-0127): per-stream
/// cursor modes make a shared pre-blend incorrect, and one extra frame copy
/// here keeps the blend off the frame thread. Delivery serves each stream
/// the variant its mode negotiated.
pub(in crate::runtime) fn stream_pixels(
    capture: CapturedPixels,
) -> Result<super::worker::StreamPixels, String> {
    let CapturedPixels {
        width,
        height,
        pixels,
        crop: _,
        cursor,
        security_generation: _,
    } = capture;
    let mut rgba = match pixels {
        CapturedPixelsSource::Readback(readback) => {
            let mut rgba = vec![0u8; width as usize * height as usize * 4];
            readback
                .read_pixels(&mut rgba)
                .map_err(|error| format!("stream pixel copy: {error}"))?;
            rgba
        }
        CapturedPixelsSource::Rgba(rgba) => rgba,
    };
    for px in rgba.chunks_exact_mut(4) {
        px.swap(0, 2);
        px[3] = 255;
    }
    let cursor_bgra = cursor.map(|cursor| {
        let mut blended = rgba.clone();
        composite_cursor_bgra(&mut blended, width, height, &cursor);
        blended.into()
    });
    Ok(super::worker::StreamPixels {
        width,
        height,
        bgra: rgba.into(),
        cursor_bgra,
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

    #[test]
    fn cursor_composite_bgra_preserves_channel_order() {
        let mut bgra = vec![0u8; 2 * 4];
        for pixel in bgra.chunks_exact_mut(4) {
            pixel[3] = 255;
        }
        let cursor = CaptureCursor {
            x: 0,
            y: 0,
            width: 2,
            height: 1,
            // 50%-opaque premultiplied red in BGRA order.
            bgra: std::sync::Arc::from([0, 0, 128, 128].repeat(2)),
        };

        composite_cursor_bgra(&mut bgra, 2, 1, &cursor);

        // BGRA in, BGRA out: red stays in channel 2 (unlike the RGBA blend,
        // which swaps it into channel 0).
        assert_eq!(&bgra[0..4], &[0, 0, 128, 255]);
        assert_eq!(&bgra[4..8], &[0, 0, 128, 255]);
    }

    #[test]
    fn region_readback_translates_and_clips_a_crossing_cursor() {
        let mut cursor = Some(CaptureCursor {
            // Full-output position: one pixel left of the selected region.
            x: 9,
            y: 20,
            width: 2,
            height: 1,
            bgra: std::sync::Arc::from([0, 0, 255, 255].repeat(2)),
        });
        translate_cursor_to_region(&mut cursor, tessera_model::Rect::new(10, 20, 2, 1));
        let cursor = cursor.unwrap();
        assert_eq!((cursor.x, cursor.y), (-1, 0));

        let mut region_rgba = vec![0, 0, 0, 255, 0, 0, 0, 255];
        composite_cursor(&mut region_rgba, 2, 1, &cursor);
        // Only the cursor's second source pixel intersects the selection.
        assert_eq!(&region_rgba[0..4], &[255, 0, 0, 255]);
        assert_eq!(&region_rgba[4..8], &[0, 0, 0, 255]);
    }

    #[test]
    fn interactive_png_profile_round_trips_exact_rgba() {
        let rgba = [
            0, 1, 2, 255, 10, 20, 30, 128, 255, 200, 100, 0, 7, 8, 9, 255, 11, 22, 33, 44, 99, 88,
            77, 66,
        ];
        let png = encode_png(3, 2, &rgba).expect("encode low-latency PNG");
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png)
            .expect("decode PNG")
            .into_rgba8();
        assert_eq!(decoded.dimensions(), (3, 2));
        assert_eq!(decoded.as_raw(), &rgba);

        assert!(encode_png(3, 2, &rgba[..rgba.len() - 1]).is_err());
    }
}
