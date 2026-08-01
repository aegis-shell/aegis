//! Pure lens option factories for aegis surface materials.

use lens::{Align, LayoutOpts, OverlayOpts};

use crate::Design;

/// The frosted-glass material used by compact menus and popovers.
#[must_use]
pub fn popover(design: &Design) -> OverlayOpts {
    OverlayOpts {
        bg: design.colors.popover_surface,
        border: design.colors.popover_border,
        border_width: design.strokes.hairline,
        radius: design.radii.popover,
        pad: 0.0,
        ..Default::default()
    }
}

/// The persistent glass surface behind the bottom dock.
///
/// The compositor's analytic liquid-glass pass supplies the physical body —
/// refraction, adaptive tint and rim light — so this painted layer stays
/// deliberately minimal: a whisper of white for cohesion and no painted
/// border (the glass rim provides the edge definition, not an outline).
#[must_use]
pub fn dock(design: &Design) -> OverlayOpts {
    OverlayOpts {
        bg: design.colors.dock_surface,
        border: design.colors.dock_border,
        border_width: 0.0,
        radius: design.radii.dock,
        pad: 0.0,
        cross: Align::Center,
        ..Default::default()
    }
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
pub fn sao_panel(sao: &crate::tokens::Sao) -> OverlayOpts {
    OverlayOpts {
        bg: sao.surface,
        border: sao.border,
        border_width: 1.0,
        radius: 16.0,
        pad: 0.0,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use lens::Color;

    use super::*;

    #[test]
    fn popover_material_preserves_the_existing_glass_values() {
        let material = popover(&Design::dark());
        assert_eq!(material.bg, Color::rgba(255, 255, 255, 38));
        assert_eq!(material.border, Color::rgba(255, 255, 255, 72));
        assert_eq!(material.border_width, 1.0);
        assert_eq!(material.radius, 12.0);
        assert_eq!(material.pad, 0.0);
    }

    #[test]
    fn dock_material_keeps_the_painted_layer_minimal() {
        let material = dock(&Design::dark());
        assert_eq!(material.bg, Color::rgba(255, 255, 255, 12));
        assert_eq!(material.border, Color::rgba(255, 255, 255, 0));
        assert_eq!(material.border_width, 0.0);
        assert_eq!(material.radius, 18.0);
        assert_eq!(material.cross, Align::Center);
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
}
