/// Intersect a logical capture request with a virtual output without relying
/// on overflowing `i32` endpoint arithmetic.
pub(in crate::runtime) fn clamp_logical_region(
    rect: ass_core::Rect,
    width: u32,
    height: u32,
) -> Option<ass_core::Rect> {
    if rect.size.w <= 0 || rect.size.h <= 0 {
        return None;
    }
    let right = i64::from(rect.origin.x) + i64::from(rect.size.w);
    let bottom = i64::from(rect.origin.y) + i64::from(rect.size.h);
    let x0 = i64::from(rect.origin.x).clamp(0, i64::from(width));
    let y0 = i64::from(rect.origin.y).clamp(0, i64::from(height));
    let x1 = right.clamp(x0, i64::from(width));
    let y1 = bottom.clamp(y0, i64::from(height));
    (x1 > x0 && y1 > y0)
        .then(|| ass_core::Rect::new(x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32))
}

/// Convert a compositor-logical crop rectangle to physical output pixels.
///
/// Scaling both endpoints avoids accumulating a rounding error in the width
/// or height at fractional scales. The result is clamped to the readback
/// surface so regions partially outside the focused output remain safe.
pub(in crate::runtime) fn logical_rect_to_physical(
    rect: ass_core::Rect,
    scale: f32,
    width: u32,
    height: u32,
) -> ass_core::Rect {
    let scale = if scale.is_finite() && scale > 0.0 {
        f64::from(scale)
    } else {
        1.0
    };
    let right = i64::from(rect.origin.x) + i64::from(rect.size.w.max(0));
    let bottom = i64::from(rect.origin.y) + i64::from(rect.size.h.max(0));
    let scaled = |value: i64| (value as f64 * scale).round() as i64;
    let x0 = scaled(i64::from(rect.origin.x)).clamp(0, i64::from(width));
    let y0 = scaled(i64::from(rect.origin.y)).clamp(0, i64::from(height));
    let x1 = scaled(right).clamp(x0, i64::from(width));
    let y1 = scaled(bottom).clamp(y0, i64::from(height));
    ass_core::Rect::new(x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32)
}

/// Extract a sub-rectangle from a full RGBA8 buffer.
pub(super) fn crop_rgba(
    src: &[u8],
    src_width: u32,
    _src_height: u32,
    rect: ass_core::Rect,
) -> Vec<u8> {
    let src_width = src_width as usize;
    let x = rect.origin.x as usize;
    let y = rect.origin.y as usize;
    let width = rect.size.w.max(0) as usize;
    let height = rect.size.h.max(0) as usize;
    let mut out = Vec::with_capacity(width * height * 4);
    for row in y..y + height {
        let start = (row * src_width + x) * 4;
        out.extend_from_slice(&src[start..start + width * 4]);
    }
    out
}
