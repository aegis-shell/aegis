//! Pure lens option factories for ass surface materials.

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

/// The persistent frosted-glass surface behind the bottom dock.
#[must_use]
pub fn dock(design: &Design) -> OverlayOpts {
    OverlayOpts {
        bg: design.colors.dock_surface,
        border: design.colors.dock_border,
        border_width: design.strokes.hairline,
        radius: design.radii.dock,
        pad: 0.0,
        cross: Align::Center,
        ..Default::default()
    }
}

/// Base material for Control Center cards.
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
    fn dock_material_preserves_the_existing_panel_values() {
        let material = dock(&Design::dark());
        assert_eq!(material.bg, Color::rgba(255, 255, 255, 34));
        assert_eq!(material.border, Color::rgba(255, 255, 255, 64));
        assert_eq!(material.border_width, 1.0);
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
}
