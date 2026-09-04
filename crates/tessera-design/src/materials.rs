//! Pure lens option factories for tessera surface materials.

use lens::{Align, Band, Color, LayoutOpts, PlaceMode, PlaceOpts, Rect};

use crate::Design;

/// Base layout for absolutely-placed tessera surfaces.
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
/// Enter/exit fades belong to the lens opacity switch (`Frame::set_opacity`),
/// so the tint is always returned at full strength.
#[must_use]
pub fn glass_focus(design: &Design, selected: bool) -> LayoutOpts {
    LayoutOpts {
        bg: if selected {
            design.glass_focus.selected_tint
        } else {
            design.glass_focus.hover_tint
        },
        border: Color::TRANSPARENT,
        border_width: 0.0,
        radius: design.radii.control,
        pad: 0.0,
        ..surface_layout()
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

/// The opaque elevated surface of the command panel (ADR-0080).
#[must_use]
pub fn hud_panel(colors: &crate::CommandPanelColors) -> LayoutOpts {
    LayoutOpts {
        bg: colors.surface,
        border: colors.border,
        border_width: 1.0,
        radius: 16.0,
        pad: 0.0,
        ..surface_layout()
    }
}

/// An invisible container used purely as a layout anchor for its children —
/// the fully-transparent layer the modal prompts (secret, confirm,
/// capability, battery, app picker) stack their painted surfaces on.
#[must_use]
pub fn transparent() -> LayoutOpts {
    LayoutOpts {
        bg: Color::TRANSPARENT,
        pad: 0.0,
        ..surface_layout()
    }
}

/// A fixed-size invisible container. Callers that center their children
/// (launcher icons) layer `cross: Align::Center` on top through struct
/// update syntax.
#[must_use]
pub fn sized(width: f32, height: f32) -> LayoutOpts {
    LayoutOpts {
        width,
        height,
        ..Default::default()
    }
}

/// A fixed-size container that paints a rounded `bg` — the reliable
/// filled-rect primitive (lens paints a container's background at its solved
/// size).
#[must_use]
pub fn sized_fill(width: f32, height: f32, color: Color, radius: f32) -> LayoutOpts {
    LayoutOpts {
        width,
        height,
        bg: color,
        radius,
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
        let colors = Design::dark().colors;
        // The surface stays translucent for the compositor's backdrop blur,
        // but opaque enough to read where blur is unavailable.
        assert_eq!(material.bg, colors.popover_surface);
        assert_eq!(material.border, colors.popover_border);
        assert_eq!(material.border_width, 1.0);
        assert_eq!(material.radius, 12.0);
        assert_eq!(material.pad, 0.0);
    }

    #[test]
    fn glass_panel_material_keeps_the_painted_layer_minimal() {
        let dark = Design::dark();
        let material = glass_panel(&dark);
        assert_eq!(material.bg, dark.colors.glass_surface);
        assert_eq!(material.border, dark.colors.glass_border);
        assert_eq!(material.border_width, 0.0);
        assert_eq!(material.radius, 18.0);
        assert_eq!(material.cross, Align::Center);
    }

    #[test]
    fn glass_focus_is_neutral_and_never_draws_an_outline() {
        let design = Design::dark();
        let hover = glass_focus(&design, false);
        let selected = glass_focus(&design, true);
        assert_eq!(hover.bg, design.glass_focus.hover_tint);
        assert_eq!(selected.bg, design.glass_focus.selected_tint);
        assert_eq!(hover.border, Color::TRANSPARENT);
        assert_eq!(hover.border_width, 0.0);
        assert_eq!(hover.radius, design.radii.control);
    }

    #[test]
    fn card_material_leaves_component_geometry_unset() {
        let design = Design::dark();
        let material = card(&design);
        assert_eq!(material.bg, design.colors.card_surface);
        assert_eq!(material.radius, 16.0);
        assert_eq!(material.width, 0.0);
        assert_eq!(material.min_height, 0.0);
    }

    #[test]
    fn hud_panel_material_is_opaque_in_both_schemes() {
        for colors in [
            crate::CommandPanelColors::dark(),
            crate::CommandPanelColors::light(),
        ] {
            let material = hud_panel(&colors);
            assert_eq!(material.bg, colors.surface);
            assert_eq!(material.border, colors.border);
            assert_eq!(material.border_width, 1.0);
            assert_eq!(material.radius, 16.0);
            assert_eq!(material.pad, 0.0);
            assert_eq!(material.bg.components().3, 255);
        }
    }

    #[test]
    fn transparent_material_paints_nothing_and_keeps_surface_spacing() {
        let material = transparent();
        assert_eq!(material.bg, Color::TRANSPARENT);
        assert_eq!(material.border, Color::TRANSPARENT);
        assert_eq!(material.border_width, 0.0);
        assert_eq!(material.radius, 0.0);
        assert_eq!(material.pad, 0.0);
        assert_eq!(material.gap, surface_layout().gap);
    }

    #[test]
    fn sized_helpers_fix_extent_without_painting() {
        let material = sized(24.0, 12.0);
        assert_eq!(material.width, 24.0);
        assert_eq!(material.height, 12.0);
        assert_eq!(material.bg, Color::TRANSPARENT);

        let fill = Design::dark().colors.menu_surface_hover;
        let filled = sized_fill(8.0, 8.0, fill, 4.0);
        assert_eq!(filled.width, 8.0);
        assert_eq!(filled.height, 8.0);
        assert_eq!(filled.bg, fill);
        assert_eq!(filled.radius, 4.0);
    }
}
