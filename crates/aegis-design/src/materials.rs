//! Pure lens option factories for aegis surface materials.

use lens::{Align, Band, Color, LayoutOpts, PlaceMode, PlaceOpts, Rect};

use crate::Design;

/// Base layout for absolutely-placed aegis surfaces.
///
/// Preserves the defaults of the retired lens `OverlayOpts` (4px child gap,
/// 6px padding) that the materials below were tuned against; the
/// [`LayoutOpts`] default of zero gap and padding would silently compact
/// every migrated surface.
#[must_use]
pub fn surface_layout() -> LayoutOpts {
    LayoutOpts {
        gap: 4.0,
        pad: 6.0,
        ..Default::default()
    }
}

/// Persistent chrome-band placement for a surface that stays on screen every
/// frame — the ADR-0060 replacement for the retired `Frame::layer`. `rect` is
/// the top-left position plus a minimum extent; the surface grows to fit its
/// body. The body always builds: there is no open/close state and no
/// dismissal.
#[must_use]
pub fn chrome_place(rect: Rect, layout: LayoutOpts) -> PlaceOpts {
    PlaceOpts {
        band: Band::Chrome,
        mode: PlaceMode::Exact,
        rect,
        transient: false,
        layout,
        ..Default::default()
    }
}

/// The frosted-glass material used by compact menus and popovers.
#[must_use]
pub fn popover(design: &Design) -> LayoutOpts {
    LayoutOpts {
        bg: design.colors.popover_surface,
        border: design.colors.popover_border,
        border_width: design.strokes.hairline,
        radius: design.radii.popover,
        pad: 0.0,
        ..surface_layout()
    }
}

/// The minimal painted foreground shared by analytic glass panels.
///
/// The compositor's analytic liquid-glass pass supplies the physical body —
/// refraction, adaptive tint and rim light — so this painted layer stays
/// deliberately minimal: a whisper of white for cohesion and no painted
/// border (the glass rim provides the edge definition, not an outline).
#[must_use]
pub fn glass_panel(design: &Design) -> LayoutOpts {
    LayoutOpts {
        bg: design.colors.glass_surface,
        border: design.colors.glass_border,
        border_width: 0.0,
        radius: design.radii.glass_panel,
        pad: 0.0,
        cross: Align::Center,
        ..surface_layout()
    }
}

/// Interaction emphasis for content nested inside an existing glass body.
///
/// This is a foreground wash, not another material body. The compositor's
/// single-body focus field supplies optical lift for selected content; this
/// layer provides a restrained fallback and immediate pointer feedback.
#[must_use]
pub fn glass_focus(design: &Design, selected: bool, visibility: f32) -> LayoutOpts {
    let color = if selected {
        design.glass_focus.selected_tint
    } else {
        design.glass_focus.hover_tint
    };
    LayoutOpts {
        bg: scale_alpha(color, visibility),
        border: Color::TRANSPARENT,
        border_width: 0.0,
        radius: design.radii.control,
        pad: 0.0,
        ..surface_layout()
    }
}

fn scale_alpha(color: Color, opacity: f32) -> Color {
    let (_, _, _, alpha) = color.components();
    color.with_alpha((alpha as f32 * opacity.clamp(0.0, 1.0)).round() as u8)
}

/// Base material for settings and system-management cards.
///
/// Callers add component-specific geometry through struct update syntax.
#[must_use]
pub fn card(design: &Design) -> LayoutOpts {
    LayoutOpts {
        bg: design.colors.card_surface,
        radius: design.radii.card,
        ..Default::default()
    }
}

/// The frosted white floating panel of the SAO command panel (ADR-0080).
///
/// Compositor backdrop blur supplies the frost behind it; this layer is the
/// white tint, edge, and corner. Callers add geometry through struct update
/// syntax.
#[must_use]
pub fn sao_panel(sao: &crate::tokens::Sao) -> LayoutOpts {
    LayoutOpts {
        bg: sao.surface,
        border: sao.border,
        border_width: 1.0,
        radius: 16.0,
        pad: 0.0,
        ..surface_layout()
    }
}

/// The dark glass floating panel of the VR/AR HUD command panel (ADR-0080).
///
/// Compositor backdrop blur supplies the frost behind it; this layer is the
/// deep blue-black tint, the low-alpha cyan edge, and the corner. Callers
/// add geometry through struct update syntax.
#[must_use]
pub fn hud_panel(hud: &crate::tokens::Hud) -> LayoutOpts {
    LayoutOpts {
        bg: hud.surface,
        border: hud.border,
        border_width: 1.0,
        radius: 16.0,
        pad: 0.0,
        ..surface_layout()
    }
}

#[cfg(test)]
mod tests {
    use lens::Color;

    use super::*;

    #[test]
    fn popover_material_preserves_the_existing_glass_values() {
        let material = popover(&Design::dark());
        // The surface stays translucent for the compositor's backdrop blur,
        // but opaque enough to read where blur is unavailable.
        assert_eq!(material.bg, Color::rgba(255, 255, 255, 110));
        assert_eq!(material.border, Color::rgba(255, 255, 255, 72));
        assert_eq!(material.border_width, 1.0);
        assert_eq!(material.radius, 12.0);
        assert_eq!(material.pad, 0.0);
    }

    #[test]
    fn glass_panel_material_keeps_the_painted_layer_minimal() {
        let material = glass_panel(&Design::dark());
        assert_eq!(material.bg, Color::rgba(255, 255, 255, 12));
        assert_eq!(material.border, Color::rgba(255, 255, 255, 0));
        assert_eq!(material.border_width, 0.0);
        assert_eq!(material.radius, 18.0);
        assert_eq!(material.cross, Align::Center);
    }

    #[test]
    fn glass_focus_is_neutral_and_never_draws_an_outline() {
        let design = Design::dark();
        let hover = glass_focus(&design, false, 1.0);
        let selected = glass_focus(&design, true, 0.5);
        assert_eq!(hover.bg, design.glass_focus.hover_tint);
        assert_eq!(selected.bg, Color::rgba(255, 255, 255, 2));
        assert_eq!(hover.border, Color::TRANSPARENT);
        assert_eq!(hover.border_width, 0.0);
        assert_eq!(hover.radius, design.radii.control);
    }

    #[test]
    fn card_material_leaves_component_geometry_unset() {
        let material = card(&Design::dark());
        assert_eq!(material.bg, Color::rgba(255, 255, 255, 14));
        assert_eq!(material.radius, 16.0);
        assert_eq!(material.width, 0.0);
        assert_eq!(material.min_height, 0.0);
    }

    #[test]
    fn sao_panel_material_uses_the_white_frosted_tokens() {
        let sao = crate::tokens::Sao::classic();
        let material = sao_panel(&sao);
        assert_eq!(material.bg, sao.surface);
        assert_eq!(material.border, sao.border);
        assert_eq!(material.border_width, 1.0);
        assert_eq!(material.radius, 16.0);
        assert_eq!(material.pad, 0.0);
    }

    #[test]
    fn hud_panel_material_uses_the_dark_glass_tokens() {
        let hud = crate::tokens::Hud::classic();
        let material = hud_panel(&hud);
        assert_eq!(material.bg, hud.surface);
        assert_eq!(material.border, hud.border);
        assert_eq!(material.border_width, 1.0);
        assert_eq!(material.radius, 16.0);
        assert_eq!(material.pad, 0.0);
    }
}
