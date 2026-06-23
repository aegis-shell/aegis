//! macOS-style dock: a rounded translucent panel at the bottom-center of the
//! output with one icon tile per mapped toplevel.

use std::collections::HashMap;
use std::ffi::c_void;

use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{Chrome, ChromeEvents};
use ass_core::window::Window;

/// Height of the dock panel. Holds one tile per mapped toplevel; clicking a
/// tile focuses it.
const DOCK_HEIGHT: f32 = 64.0;
/// Gap between the dock panel and the bottom edge of the output.
const DOCK_BOTTOM_MARGIN: f32 = 12.0;
/// Side length of a square dock tile (the icon area).
const DOCK_TILE: f32 = 44.0;
/// Gap between adjacent dock tiles inside the panel.
const DOCK_TILE_GAP: f32 = 8.0;
/// Padding inside the dock panel; must match the `pad` passed to the overlay
/// opts so the panel's width math accounts for it.
const DOCK_PAD: f32 = 6.0;
/// Minimum dock panel width so it reads as a bar even with a single tile.
const DOCK_MIN_WIDTH: f32 = 96.0;

/// The macOS-style dock. A rounded translucent panel anchored to a
/// bottom-center rect, holding one tile per mapped toplevel. A tile click
/// focuses that window; the activated window's tile is highlighted. When an
/// application icon texture is available for a window's `app_id` it is drawn
/// via `image_button_active`; otherwise the dock falls back to a glyph.
pub struct Dock {
    /// `app_id` (lowercased) → borrowed icon texture pointer. Borrowed from
    /// the binary's `IconCache`, which owns the `flux::Image`s and outlives
    /// this component.
    icons: HashMap<String, *mut c_void>,
}

impl Dock {
    pub fn new() -> Dock {
        Dock {
            icons: HashMap::new(),
        }
    }

    /// Construct with a pre-decoded icon map (`app_id` → `flux_image` pointer
    /// erased to `c_void`). The caller retains ownership of the textures.
    pub fn with_icons(icons: HashMap<String, *mut c_void>) -> Dock {
        Dock { icons }
    }
}

impl Default for Dock {
    fn default() -> Self {
        Dock::new()
    }
}

impl Chrome for Dock {
    fn render(&mut self, f: &mut Frame, input: &Input, windows: &[Window], _workspaces: &crate::WorkspaceSnapshot, out: &mut ChromeEvents) {
        let disp = input.as_raw().display_size;
        let n = windows.len().max(1);
        let dock_w =
            (n as f32 * DOCK_TILE + (n as f32 - 1.0).max(0.0) * DOCK_TILE_GAP + 2.0 * DOCK_PAD)
                .max(DOCK_MIN_WIDTH);
        let dock_rect = Rect {
            x: (disp.x - dock_w) * 0.5,
            y: disp.y - DOCK_HEIGHT - DOCK_BOTTOM_MARGIN,
            w: dock_w,
            h: DOCK_HEIGHT,
        };
        let dock_opts = OverlayOpts {
            bg: Color::rgba(28, 30, 44, 220),
            border: Color::rgba(60, 64, 84, 255),
            border_width: 1.0,
            radius: 16.0,
            pad: DOCK_PAD,
            cross: Align::Center,
            ..Default::default()
        };
        f.overlay("ass-dock", dock_rect, &dock_opts, |f| {
            if windows.is_empty() {
                f.label_sized("no apps", 12.0);
                return;
            }
            let row = LayoutOpts {
                gap: DOCK_TILE_GAP,
                cross: Align::Center,
                ..Default::default()
            };
            f.row_ex(&row, |f| {
                for w in windows.iter() {
                    f.size_next(DOCK_TILE, DOCK_TILE);
                    // Look up a decoded icon texture by the window's app_id
                    // (lowercased). Present → draw the raster tile; absent →
                    // glyph fallback. The pointer crosses from the binary's
                    // flux binding type to lens's ABI-identical flux_image.
                    let icon_ptr = w
                        .app_id
                        .as_deref()
                        .and_then(|a| self.icons.get(&a.to_ascii_lowercase()).copied());
                    let clicked = if let Some(ptr) = icon_ptr {
                        unsafe {
                            f.image_button_active(
                                ptr as *mut lens::sys::flux_image,
                                w.state.activated,
                            )
                        }
                    } else {
                        f.icon_button_active(Icon::FileText, w.state.activated)
                    };
                    if clicked {
                        out.clicked = Some(w.id);
                    }
                }
            });
        });
    }
}
