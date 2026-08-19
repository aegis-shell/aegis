//! List pickers, candidate selection, and scroll-window calculations.

use std::time::Duration;

use aegis_design::{Design, materials};
use lens::{Align, Color, LayoutOpts};

/// Standard duration threshold for classifying consecutive presses as a double-click.
pub const DEFAULT_DOUBLE_CLICK_TIMEOUT: Duration = Duration::from_millis(400);

/// Standard number of rows scrolled per wheel detent.
pub const DEFAULT_WHEEL_SCROLL_ROWS: f32 = 3.0;

/// Standard row height for list picker candidate items.
pub const DEFAULT_PICKER_ROW_HEIGHT: f32 = 36.0;

/// Calculate the top visible index in a list with `total` items, `visible` view capacity,
/// and preferred cursor/selection index `target`.
pub fn clamp_scroll_window(total: usize, visible: usize, scroll: usize) -> usize {
    let max_scroll = total.saturating_sub(visible);
    scroll.min(max_scroll)
}

/// Return layout options for a selectable candidate list item row.
pub fn picker_row_layout(is_selected: bool, is_hovered: bool, design: &Design) -> LayoutOpts {
    let bg = if is_selected {
        design.colors.application_accent
    } else if is_hovered {
        design.colors.application_hover
    } else {
        Color::TRANSPARENT
    };

    LayoutOpts {
        height: DEFAULT_PICKER_ROW_HEIGHT,
        pad: 6.0,
        gap: 8.0,
        radius: design.radii.control,
        bg,
        cross: Align::Center,
        ..materials::surface_layout()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_clamp_scroll_window() {
        assert_eq!(clamp_scroll_window(10, 5, 0), 0);
        assert_eq!(clamp_scroll_window(10, 5, 3), 3);
        assert_eq!(clamp_scroll_window(10, 5, 6), 5); // 10 - 5 = 5
        assert_eq!(clamp_scroll_window(3, 5, 2), 0);
    }
}
