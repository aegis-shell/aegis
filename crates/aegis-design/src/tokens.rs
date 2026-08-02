//! Semantic visual tokens for the built-in dark appearance.

use lens::Color;

/// The product design snapshot consumed by theme and material factories.
///
/// Components depend on semantic roles rather than literal color values. The
/// value is cheap to copy and leaves room for additional appearance variants
/// without changing component APIs.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Design {
    pub colors: Colors,
    pub radii: Radii,
    pub strokes: Strokes,
    pub hud_foreground: HudForeground,
}

impl Design {
    /// The canonical dark appearance currently used by compositor chrome.
    #[must_use]
    pub fn dark() -> Self {
        Self {
            colors: Colors {
                menu_text: Color::rgba(238, 240, 248, 255),
                menu_heading: Color::rgba(183, 188, 207, 255),
                menu_disabled: Color::rgba(160, 168, 188, 255),
                menu_border: Color::rgba(255, 255, 255, 78),
                menu_hover: Color::rgba(255, 255, 255, 22),
                menu_active: Color::rgba(255, 255, 255, 36),
                popover_surface: Color::rgba(255, 255, 255, 38),
                popover_border: Color::rgba(255, 255, 255, 72),
                dock_surface: Color::rgba(255, 255, 255, 12),
                dock_border: Color::rgba(255, 255, 255, 0),
                application_surface: Color::rgba(25, 28, 40, 255),
                application_text: Color::rgba(244, 246, 252, 255),
                application_accent: Color::rgba(102, 156, 255, 255),
                application_border: Color::rgba(255, 255, 255, 42),
                application_hover: Color::rgba(255, 255, 255, 24),
                application_active: Color::rgba(102, 156, 255, 56),
                slider_track: Color::rgba(255, 255, 255, 30),
                slider_fill: Color::rgba(102, 156, 255, 255),
                slider_knob: Color::rgba(255, 255, 255, 255),
                card_surface: Color::rgba(255, 255, 255, 14),
            },
            radii: Radii {
                menu_item: 7.0,
                popover: 12.0,
                dock: 18.0,
                control: 12.0,
                card: 16.0,
                scrollbar: 2.5,
            },
            strokes: Strokes {
                hairline: 1.0,
                scrollbar: 5.0,
            },
            hud_foreground: HudForeground {
                primary: Color::rgba(248, 249, 252, 255),
                contour: Color::rgba(5, 7, 12, 48),
                text_contour_width: 0.75,
                glyph_contour_width: 1.0,
            },
        }
    }
}

impl Default for Design {
    fn default() -> Self {
        Self::dark()
    }
}

/// The SAO command-panel palette (ADR-0080): frosted white floating panels
/// with an amber accent over the standard dark scrim, after the Sword Art
/// Online menu language. Kept separate from [`Colors`] because the panel is
/// a light island inside the dark product appearance.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Sao {
    /// Frosted white panel surface.
    pub surface: Color,
    /// Slightly deeper white for recessed areas inside a panel.
    pub surface_dim: Color,
    /// Panel edge against the dark scrim.
    pub border: Color,
    /// Primary text on the white surface.
    pub text: Color,
    /// Secondary text on the white surface.
    pub text_muted: Color,
    /// The signature amber accent: selected rows, rings, slider fill.
    pub accent: Color,
    /// Low-alpha accent tint for hover feedback on rows.
    pub accent_soft: Color,
    /// Text/icons drawn on top of a solid accent fill.
    pub on_accent: Color,
    /// Slider/checkbox track on the white surface.
    pub track: Color,
    /// Control knob on the white surface.
    pub knob: Color,
}

impl Sao {
    /// The canonical SAO palette.
    #[must_use]
    pub fn classic() -> Self {
        Self {
            surface: Color::rgba(248, 249, 252, 226),
            surface_dim: Color::rgba(236, 238, 244, 210),
            border: Color::rgba(255, 255, 255, 110),
            text: Color::rgba(32, 36, 48, 255),
            text_muted: Color::rgba(96, 102, 120, 255),
            accent: Color::rgba(245, 158, 30, 255),
            accent_soft: Color::rgba(245, 158, 30, 48),
            on_accent: Color::rgba(255, 255, 255, 255),
            track: Color::rgba(32, 36, 48, 28),
            knob: Color::rgba(255, 255, 255, 255),
        }
    }
}

impl Default for Sao {
    fn default() -> Self {
        Self::classic()
    }
}

/// Semantic color roles shared across compositor chrome.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Colors {
    pub menu_text: Color,
    pub menu_heading: Color,
    pub menu_disabled: Color,
    pub menu_border: Color,
    pub menu_hover: Color,
    pub menu_active: Color,
    pub popover_surface: Color,
    pub popover_border: Color,
    pub dock_surface: Color,
    pub dock_border: Color,
    pub application_surface: Color,
    pub application_text: Color,
    pub application_accent: Color,
    pub application_border: Color,
    pub application_hover: Color,
    pub application_active: Color,
    pub slider_track: Color,
    pub slider_fill: Color,
    pub slider_knob: Color,
    pub card_surface: Color,
}

/// Shared radii in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Radii {
    pub menu_item: f32,
    pub popover: f32,
    pub dock: f32,
    pub control: f32,
    pub card: f32,
    pub scrollbar: f32,
}

/// Shared stroke widths in logical pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct Strokes {
    pub hairline: f32,
    pub scrollbar: f32,
}

/// Foreground-separation policy for the display-only HUD.
///
/// The HUD floats above arbitrary wallpaper and application content, so a
/// single foreground colour cannot guarantee local contrast. A restrained
/// dark contour keeps the light core legible on bright or visually busy
/// regions while disappearing naturally over dark regions. Text and glyphs
/// share one contour colour; only their geometry-specific widths differ.
#[derive(Debug, Clone, Copy, PartialEq)]
#[non_exhaustive]
pub struct HudForeground {
    /// Light core shared by HUD labels, symbols, and active indicators.
    pub primary: Color,
    /// Dark contour/underlay shared by every floating HUD foreground form.
    pub contour: Color,
    /// Fine contour for compact text, in logical pixels.
    pub text_contour_width: f32,
    /// Contour for vector, raster, and geometric glyphs, in logical pixels.
    pub glyph_contour_width: f32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dark_tokens_preserve_the_existing_menu_palette() {
        let design = Design::dark();
        assert_eq!(design.colors.menu_text, Color::rgba(238, 240, 248, 255));
        assert_eq!(design.colors.menu_hover, Color::rgba(255, 255, 255, 22));
        assert_eq!(design.colors.menu_active, Color::rgba(255, 255, 255, 36));
        assert_eq!(design.radii.menu_item, 7.0);
    }

    #[test]
    fn sao_palette_is_a_light_island_with_amber_accent() {
        let sao = Sao::classic();
        assert_eq!(sao.accent, Color::rgba(245, 158, 30, 255));
        assert_eq!(sao.on_accent, Color::rgba(255, 255, 255, 255));
        let (_, _, _, surface_alpha) = sao.surface.components();
        assert!(surface_alpha > 200);
        let (r, g, b, _) = sao.text.components();
        assert!(r < 64 && g < 64 && b < 64);
    }

    #[test]
    fn hud_foreground_uses_one_restrained_contour_family() {
        let hud = Design::dark().hud_foreground;
        assert_eq!(hud.primary, Color::rgba(248, 249, 252, 255));
        assert_eq!(hud.contour, Color::rgba(5, 7, 12, 48));
        assert!(hud.text_contour_width > 0.0);
        assert!(hud.text_contour_width < hud.glyph_contour_width);
        assert!(hud.glyph_contour_width <= 1.0);
    }
}
