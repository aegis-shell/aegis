//! Status chips, pills, and badges for HUD and chrome surfaces.

use aegis_design::{Design, materials};
use lens::{Align, Color, Frame, LayoutOpts, Rect, Style};

/// Standard diameter for workspace indicator dots on HUD and panel surfaces.
pub const DEFAULT_WORKSPACE_DOT_DIAMETER: f32 = 6.0;

/// Return layout options for a standard floating HUD status chip container.
pub fn chip_layout(design: &Design) -> LayoutOpts {
    LayoutOpts {
        height: 28.0,
        pad: 6.0,
        gap: 6.0,
        radius: design.radii.chip,
        cross: Align::Center,
        ..materials::surface_layout()
    }
}

/// The floating HUD chip foreground tint and border options.
pub fn chip_opts(design: &Design) -> LayoutOpts {
    LayoutOpts {
        bg: Color::rgba(24, 26, 36, 42),
        border: Color::rgba(255, 255, 255, 18),
        border_width: 0.75,
        radius: design.radii.chip,
        pad: 0.0,
        ..materials::surface_layout()
    }
}

/// Text contour outline parameters: `(color, width)`.
pub fn hud_text_outline_params(design: &Design) -> (Color, f32) {
    let hud = design.hud_foreground;
    (hud.contour, hud.text_contour_width)
}

/// Style with text contour outline for HUD typography readability over dynamic backdrops.
pub fn hud_text_outline(design: &Design) -> Style {
    let (color, width) = hud_text_outline_params(design);
    Style::new()
        .with_outline_color(color)
        .with_outline_width(width)
}

/// Glyph/icon contour outline parameters: `(color, width)`.
pub fn hud_glyph_outline_params(design: &Design) -> (Color, f32) {
    let hud = design.hud_foreground;
    (hud.contour, hud.glyph_contour_width)
}

/// Style with glyph contour outline for HUD icons.
pub fn hud_glyph_outline(design: &Design) -> Style {
    let (color, width) = hud_glyph_outline_params(design);
    Style::new()
        .with_outline_color(color)
        .with_outline_width(width)
}

/// Calculate dynamic color for a workspace dot based on pagination intensity `[0.0, 1.0]`.
pub fn workspace_dot_color(design: &Design, intensity: f32) -> Color {
    let primary = design.hud_foreground.primary;
    let intensity = intensity.clamp(0.0, 1.0);
    let alpha = (78.0 + (248.0 - 78.0) * intensity).round() as u8;
    primary.with_alpha(alpha)
}

/// Calculate workspace dot highlight intensity for dot `index` given continuous pagination `position`.
pub fn workspace_dot_intensity(index: usize, position: f32) -> f32 {
    (1.0 - (index as f32 - position).abs()).clamp(0.0, 1.0)
}

/// Render a status badge / count bubble.
pub fn render_badge(frame: &mut Frame, text: &str, is_accented: bool, design: &Design) {
    let bg = if is_accented {
        design.colors.application_accent
    } else {
        design.colors.card_surface
    };

    frame.row_ex(
        &LayoutOpts {
            height: 18.0,
            pad: 4.0,
            radius: design.radii.chip,
            bg,
            cross: Align::Center,
            ..materials::surface_layout()
        },
        |frame| {
            frame.label_sized(text, design.typography.footnote);
        },
    );
}

/// Place a status chip at an explicit screen rectangle.
pub fn place_chip<F>(
    frame: &mut Frame,
    id: &str,
    rect: Rect,
    alpha: u8,
    design: &Design,
    content: F,
) where
    F: FnOnce(&mut Frame),
{
    let bg = design.colors.glass_surface.with_alpha(alpha);

    frame.place(
        id,
        &materials::chrome_place(
            rect,
            LayoutOpts {
                bg,
                radius: design.radii.chip,
                pad: 6.0,
                gap: 6.0,
                cross: Align::Center,
                ..materials::surface_layout()
            },
        ),
        content,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workspace_dot_intensity() {
        assert_eq!(workspace_dot_intensity(0, 0.0), 1.0);
        assert_eq!(workspace_dot_intensity(1, 0.0), 0.0);
        assert_eq!(workspace_dot_intensity(0, 0.5), 0.5);
        assert_eq!(workspace_dot_intensity(1, 0.5), 0.5);
    }
}
