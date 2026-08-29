//! Context menus, popup action lists, and menu item scaffolding.

use aegis_design::{Design, materials};
use lens::{Align, LayoutOpts};

/// Standard width for popup context menus.
pub const DEFAULT_MENU_WIDTH: f32 = 236.0;
/// Standard inner padding for popup context menus.
pub const DEFAULT_MENU_PAD: f32 = 7.0;
/// Standard row height for single-line menu items.
pub const DEFAULT_MENU_ROW_HEIGHT: f32 = 28.0;
/// Standard height for menu section headers.
pub const DEFAULT_MENU_HEADER_HEIGHT: f32 = 23.0;
/// Standard vertical spacing between menu sections.
pub const DEFAULT_MENU_SECTION_HEIGHT: f32 = 7.0;

/// Return layout options for a standard popup menu panel.
pub fn menu_panel_layout(design: &Design) -> LayoutOpts {
    LayoutOpts {
        width: DEFAULT_MENU_WIDTH,
        pad: DEFAULT_MENU_PAD,
        gap: 2.0,
        radius: design.radii.popover,
        cross: Align::Stretch,
        ..materials::popover(design)
    }
}

/// Return layout options for a menu item row.
pub fn menu_item_layout(is_hovered: bool, design: &Design) -> LayoutOpts {
    LayoutOpts {
        height: DEFAULT_MENU_ROW_HEIGHT,
        pad: 6.0,
        gap: 8.0,
        radius: design.radii.menu_item,
        bg: if is_hovered {
            design.colors.menu_surface_hover
        } else {
            lens::Color::TRANSPARENT
        },
        cross: Align::Center,
        ..materials::surface_layout()
    }
}
