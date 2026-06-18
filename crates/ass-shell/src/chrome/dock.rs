//! macOS-style dock: a rounded translucent panel at the bottom-center of the
//! output with one icon tile per mapped toplevel.

use flux_ui::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect};

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
/// focuses that window; the activated window's tile is highlighted via
/// `icon_button_active`. Drawn as a `flux-ui` overlay so it sits at an
/// absolute position independent of the top-left auto-layout flow.
///
/// Stateless today; the tile layout already sizes for future per-app icons,
/// pinned apps, and magnification state.
pub struct Dock;

impl Dock {
    pub fn new() -> Dock {
        Dock
    }
}

impl Default for Dock {
    fn default() -> Self {
        Dock::new()
    }
}

impl Chrome for Dock {
    fn render(&mut self, f: &mut Frame, input: &Input, windows: &[Window], out: &mut ChromeEvents) {
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
                    if f.icon_button_active(Icon::FileText, w.state.activated) {
                        out.clicked = Some(w.id);
                    }
                }
            });
        });
    }
}
