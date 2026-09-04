//! lens theme factories for product-level surfaces.

use lens::{Color, Theme};

use crate::Design;

/// Apply the shared compact menu appearance to an existing lens theme.
#[must_use]
pub fn menu(base: Theme, design: &Design) -> Theme {
    base.with_fg(design.colors.menu_text)
        .with_border(design.colors.menu_border)
        .with_hover(design.colors.menu_surface_hover)
        .with_active(design.colors.menu_surface_active)
        .with_corner_radius(design.radii.menu_item)
        .with_border_width(0.0)
}

/// Derive the subdued menu-heading theme without changing other menu tokens.
#[must_use]
pub fn menu_heading(menu: Theme, design: &Design) -> Theme {
    menu.with_fg(design.colors.menu_text_heading)
}

/// Derive the inert-row menu theme without changing other menu tokens.
#[must_use]
pub fn menu_disabled(menu: Theme, design: &Design) -> Theme {
    menu.with_fg(design.colors.menu_text_disabled)
}

/// The application theme used by System Settings and trusted chrome surfaces.
///
/// The lens base must match the design's scheme: it supplies the widget
/// defaults the tokens below do not override (caret, selection, focus ring),
/// which would sit on the wrong tonal side otherwise.
#[must_use]
pub fn application(design: &Design) -> Theme {
    let base = application_base(design);
    base.with_bg(design.colors.application_surface)
        .with_fg(design.colors.application_text)
        .with_accent(design.colors.application_accent)
        .with_border(design.colors.application_border)
        .with_hover(design.colors.application_surface_hover)
        .with_active(design.colors.application_surface_active)
        .with_corner_radius(design.radii.control)
        .with_border_width(design.strokes.hairline)
        .with_slider_track_color(design.colors.slider_track)
        .with_slider_fill_color(design.colors.slider_fill)
        .with_slider_knob_color(design.colors.slider_knob)
        .with_scrollbar_width(design.strokes.scrollbar)
        .with_scrollbar_radius(design.radii.scrollbar)
        // Thumb-only bar: a permanent track strip reads as a stray divider
        // on cards and chrome surfaces. The scheme-correct rest/hover/active
        // thumb ramp comes from the lens base theme.
        .with_scrollbar_track_color(Color::TRANSPARENT)
}

/// The scheme-correct lens base (light or dark) with only the foreground
/// re-toned to the design's text color.
///
/// Hosts that own the lens context push this onto `Ui::set_theme` when the
/// desktop scheme changes so bare lens widgets — vector icons and anything
/// not explicitly styled — draw on the same tonal side as the design
/// instead of the creation-time dark token set. It deliberately carries no
/// background, border, or accent overrides: components supply their own
/// surface materials and only inherit the tonal defaults.
#[must_use]
pub fn application_base(design: &Design) -> Theme {
    let base = if design.is_light() {
        Theme::light()
    } else {
        Theme::dark()
    };
    base.with_fg(design.colors.application_text)
}

/// The scheme-correct widget theme inside the command panel's solid surfaces.
#[must_use]
pub fn hud(colors: &crate::CommandPanelColors) -> Theme {
    let base = if colors.is_light() {
        Theme::light()
    } else {
        Theme::dark()
    };
    base.with_bg(colors.surface)
        .with_fg(colors.text)
        .with_accent(colors.accent)
        .with_border(colors.border)
        .with_hover(colors.accent_surface_hover)
        .with_active(colors.accent_surface_hover)
        .with_corner_radius(8.0)
        .with_border_width(0.0)
        .with_slider_track_color(colors.control_track)
        .with_slider_fill_color(colors.accent)
        .with_slider_knob_color(colors.control_knob)
        .with_scrollbar_width(5.0)
        .with_scrollbar_radius(2.5)
        // Thumb-only bar: no permanent track strip. The rest state borrows
        // the gauge-track hue; dragging uses the shared system-blue accent.
        .with_scrollbar_track_color(Color::TRANSPARENT)
        .with_scrollbar_thumb_color(colors.control_track.with_alpha(80))
        .with_scrollbar_thumb_hover_color(colors.control_track.with_alpha(130))
        .with_scrollbar_thumb_active_color(colors.accent.with_alpha(170))
}

/// Derive the subdued HUD caption theme without changing other tokens.
#[must_use]
pub fn hud_muted(theme: Theme, colors: &crate::CommandPanelColors) -> Theme {
    theme.with_fg(colors.text_muted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_theme_uses_semantic_tokens() {
        let design = Design::dark();
        let theme = menu(Theme::light(), &design);
        assert_eq!(theme.fg(), design.colors.menu_text);
        assert_eq!(theme.border(), design.colors.menu_border);
        assert_eq!(theme.hover(), design.colors.menu_surface_hover);
        assert_eq!(theme.active(), design.colors.menu_surface_active);
    }

    #[test]
    fn application_theme_preserves_the_existing_control_palette() {
        let design = Design::dark();
        let theme = application(&design);
        assert_eq!(theme.bg(), design.colors.application_surface);
        assert_eq!(theme.fg(), design.colors.application_text);
        assert_eq!(theme.accent(), design.colors.application_accent);
    }

    #[test]
    fn application_theme_follows_the_design_scheme() {
        let design = Design::light();
        let theme = application(&design);
        assert_eq!(theme.bg(), design.colors.application_surface);
        assert_eq!(theme.fg(), design.colors.application_text);
        assert_eq!(theme.accent(), design.colors.application_accent);
    }

    #[test]
    fn hud_theme_follows_the_command_panel_scheme() {
        for colors in [
            crate::CommandPanelColors::dark(),
            crate::CommandPanelColors::light(),
        ] {
            let theme = hud(&colors);
            assert_eq!(theme.fg(), colors.text);
            assert_eq!(theme.bg(), colors.surface);
            assert_eq!(theme.accent(), colors.accent);
            assert_eq!(theme.hover(), colors.accent_surface_hover);
            assert_eq!(hud_muted(theme, &colors).fg(), colors.text_muted);
        }
    }
}
