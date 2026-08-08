//! lens theme factories for product-level surfaces.

use lens::{Color, Theme};

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
///
/// The lens base must match the design's scheme: it supplies the widget
/// defaults the tokens below do not override (caret, selection, focus ring),
/// which would sit on the wrong tonal side otherwise.
#[must_use]
pub fn application(design: &Design) -> Theme {
    let base = if design.is_light() {
        Theme::light()
    } else {
        Theme::dark()
    };
    base.with_bg(design.colors.application_surface)
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
        // Thumb-only bar: a permanent track strip reads as a stray divider
        // on cards and chrome surfaces. The scheme-correct rest/hover/active
        // thumb ramp comes from the lens base theme.
        .with_scrollbar_track_color(Color::TRANSPARENT)
}

/// The light widget theme inside the SAO command panel's white surfaces
/// (ADR-0080): dark text, amber accent, amber slider fill.
#[must_use]
pub fn sao(sao: &crate::tokens::Sao) -> Theme {
    Theme::light()
        .with_bg(sao.surface)
        .with_fg(sao.text)
        .with_accent(sao.accent)
        .with_border(sao.border)
        .with_hover(sao.accent_soft)
        .with_active(sao.accent_soft)
        .with_corner_radius(8.0)
        .with_border_width(0.0)
        .with_slider_track_color(sao.track)
        .with_slider_fill_color(sao.accent)
        .with_slider_knob_color(sao.knob)
        .with_scrollbar_width(5.0)
        .with_scrollbar_radius(2.5)
}

/// Derive the subdued SAO caption theme without changing other tokens.
#[must_use]
pub fn sao_muted(sao_theme: Theme, sao: &crate::tokens::Sao) -> Theme {
    sao_theme.with_fg(sao.text_muted)
}

/// The dark widget theme inside the command panel's VR/AR HUD surfaces
/// (ADR-0080): near-white text, cyan accent, cyan slider fill. Built on the
/// dark base so the widget defaults the tokens do not override (caret,
/// selection, focus ring) sit on the right tonal side.
#[must_use]
pub fn hud(hud: &crate::tokens::Hud) -> Theme {
    Theme::dark()
        .with_bg(hud.surface)
        .with_fg(hud.text)
        .with_accent(hud.accent)
        .with_border(hud.border)
        .with_hover(hud.accent_soft)
        .with_active(hud.accent_soft)
        .with_corner_radius(8.0)
        .with_border_width(0.0)
        .with_slider_track_color(hud.track)
        .with_slider_fill_color(hud.accent)
        .with_slider_knob_color(hud.knob)
        .with_scrollbar_width(5.0)
        .with_scrollbar_radius(2.5)
        // Thumb-only bar on the glass: no permanent track strip. The rest
        // state borrows the gauge-track hue at a readable alpha; dragging
        // lights up the signature cyan like every other active element.
        .with_scrollbar_track_color(Color::TRANSPARENT)
        .with_scrollbar_thumb_color(hud.track.with_alpha(80))
        .with_scrollbar_thumb_hover_color(hud.track.with_alpha(130))
        .with_scrollbar_thumb_active_color(hud.accent.with_alpha(170))
}

/// Derive the subdued HUD caption theme without changing other tokens.
#[must_use]
pub fn hud_muted(hud_theme: Theme, hud: &crate::tokens::Hud) -> Theme {
    hud_theme.with_fg(hud.text_muted)
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

    #[test]
    fn application_theme_follows_the_design_scheme() {
        let design = Design::light();
        let theme = application(&design);
        assert_eq!(theme.bg(), design.colors.application_surface);
        assert_eq!(theme.fg(), design.colors.application_text);
        assert_eq!(theme.accent(), design.colors.application_accent);
    }

    #[test]
    fn sao_theme_is_light_with_amber_accent() {
        let tokens = crate::tokens::Sao::classic();
        let theme = sao(&tokens);
        assert_eq!(theme.fg(), tokens.text);
        assert_eq!(theme.accent(), tokens.accent);
        assert_eq!(sao_muted(theme, &tokens).fg(), tokens.text_muted);
    }

    #[test]
    fn hud_theme_is_dark_with_cyan_accent() {
        let tokens = crate::tokens::Hud::classic();
        let theme = hud(&tokens);
        assert_eq!(theme.fg(), tokens.text);
        assert_eq!(theme.accent(), tokens.accent);
        assert_eq!(theme.hover(), tokens.accent_soft);
        assert_eq!(hud_muted(theme, &tokens).fg(), tokens.text_muted);
    }
}
