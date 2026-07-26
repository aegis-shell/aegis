//! lens theme factories for product-level surfaces.

use lens::Theme;

use crate::Design;

/// Apply the shared compact menu appearance to an existing lens theme.
#[must_use]
pub fn menu(base: Theme, design: &Design) -> Theme {
    base.with_fg(design.colors.menu_text)
        .with_border(design.colors.menu_border)
        .with_hover(design.colors.menu_hover)
        .with_active(design.colors.menu_active)
        .with_corner_radius(design.radii.menu_item)
        .with_border_width(0.0)
        .with_active_indicator_width(0.0)
}

/// Derive the subdued menu-heading theme without changing other menu tokens.
#[must_use]
pub fn menu_heading(menu: Theme, design: &Design) -> Theme {
    menu.with_fg(design.colors.menu_heading)
}

/// Derive the inert-row menu theme without changing other menu tokens.
#[must_use]
pub fn menu_disabled(menu: Theme, design: &Design) -> Theme {
    menu.with_fg(design.colors.menu_disabled)
}

/// The application theme used by System Settings and trusted chrome surfaces.
#[must_use]
pub fn application(design: &Design) -> Theme {
    Theme::dark()
        .with_bg(design.colors.application_surface)
        .with_fg(design.colors.application_text)
        .with_accent(design.colors.application_accent)
        .with_border(design.colors.application_border)
        .with_hover(design.colors.application_hover)
        .with_active(design.colors.application_active)
        .with_corner_radius(design.radii.control)
        .with_border_width(design.strokes.hairline)
        .with_slider_track_color(design.colors.slider_track)
        .with_slider_fill_color(design.colors.slider_fill)
        .with_slider_knob_color(design.colors.slider_knob)
        .with_scrollbar_width(design.strokes.scrollbar)
        .with_scrollbar_radius(design.radii.scrollbar)
}

#[cfg(test)]
mod tests {
    use lens::Color;

    use super::*;

    #[test]
    fn menu_theme_uses_semantic_tokens() {
        let design = Design::dark();
        let theme = menu(Theme::light(), &design);
        assert_eq!(theme.fg(), design.colors.menu_text);
        assert_eq!(theme.border(), design.colors.menu_border);
        assert_eq!(theme.hover(), design.colors.menu_hover);
        assert_eq!(theme.active(), design.colors.menu_active);
    }

    #[test]
    fn application_theme_preserves_the_existing_control_palette() {
        let theme = application(&Design::dark());
        assert_eq!(theme.bg(), Color::rgba(25, 28, 40, 255));
        assert_eq!(theme.fg(), Color::rgba(244, 246, 252, 255));
        assert_eq!(theme.accent(), Color::rgba(102, 156, 255, 255));
    }
}
