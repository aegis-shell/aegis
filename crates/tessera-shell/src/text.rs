use lens::Frame;

const ELLIPSIS: &str = "…";

/// Truncate user-facing copy at a Unicode scalar boundary and reserve the
/// final slot for an ellipsis.
pub fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut value: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    value.push('…');
    value
}

/// Fit a single line of user-facing text inside `max_width` logical pixels.
///
/// Lens uses proportional fonts, so a character-count limit cannot reliably
/// keep CJK text, wide glyphs, and mixed scripts inside the same rectangle.
/// This helper shapes the text with the active frame font and replaces the
/// longest overflowing suffix with an ellipsis. If the available width cannot
/// fit even the ellipsis, it returns an empty string rather than overflowing.
pub fn ellipsize(frame: &Frame, text: &str, size: f32, max_width: f32) -> String {
    if text.is_empty() || max_width.is_nan() || max_width <= 0.0 {
        return String::new();
    }
    if frame.measure_text(text, size).width <= max_width {
        return text.to_string();
    }
    if frame.measure_text(ELLIPSIS, size).width > max_width {
        return String::new();
    }

    // Every char index is a UTF-8 boundary. Binary search keeps the number of
    // shaping calls small even for unusually long window titles or paths.
    let boundaries: Vec<usize> = text.char_indices().map(|(index, _)| index).collect();
    let mut low = 0;
    let mut high = boundaries.len();
    while low < high {
        let middle = low + (high - low) / 2;
        let candidate = format!("{}…", &text[..boundaries[middle]]);
        if frame.measure_text(&candidate, size).width <= max_width {
            low = middle + 1;
        } else {
            high = middle;
        }
    }

    let mut best = low.saturating_sub(1);
    loop {
        let candidate = format!("{}…", &text[..boundaries[best]]);
        if frame.measure_text(&candidate, size).width <= max_width {
            return candidate;
        }
        if best == 0 {
            return ELLIPSIS.to_string();
        }
        best -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lens::{Input, Ui};

    #[test]
    fn truncation_is_unicode_safe_and_bounded() {
        assert_eq!(truncate("Fuji connected", 20), "Fuji connected");
        assert_eq!(truncate("真实通知已经联通", 6), "真实通知已…");
        assert_eq!(truncate("窗口操作", 3), "窗口…");
    }

    #[test]
    fn ellipsis_uses_measured_width_and_never_overflows() {
        let mut ui = Ui::headless().expect("create headless Lens context");
        let input = Input::new((800.0, 600.0), 1.0 / 60.0);

        ui.frame(&input, |frame| {
            let text = "A very long 窗口 title with wide glyphs";
            let size = 14.0;
            let full_width = frame.measure_text(text, size).width;
            let max_width = full_width * 0.5;
            let fitted = ellipsize(frame, text, size, max_width);

            assert!(fitted.ends_with(ELLIPSIS));
            assert!(frame.measure_text(&fitted, size).width <= max_width);
            assert!(fitted.is_char_boundary(fitted.len()));
            assert_eq!(ellipsize(frame, "Short", size, full_width), "Short");

            let ellipsis_width = frame.measure_text(ELLIPSIS, size).width;
            assert_eq!(
                ellipsize(frame, text, size, ellipsis_width * 0.5),
                "",
                "a constraint narrower than the ellipsis must still be honored"
            );
        });
    }
}
