//! Per-window server-side decorations (title bars) drawn as `lens` overlays.

use lens::{Color, Frame, Input, OverlayOpts, Rect};

use crate::{Chrome, ChromeEvents};
use ass_core::window::Window;

/// Height of the server-side decoration title bar drawn above each mapped
/// toplevel. The client surface is rendered immediately below it; clicks on
/// the bar are intercepted by the shell and drive interactive move.
const TITLE_BAR_HEIGHT: f32 = 24.0;
/// Width of the close-button region at the right of each title bar.
const CLOSE_BUTTON_WIDTH: f32 = 24.0;

/// Per-window server-side decorations. Each mapped toplevel gets a title bar
/// anchored just above its surface; clicking the title starts an interactive
/// move and the close gadget posts `xdg_toplevel.close`. The component is
/// stateless — geometry comes from each window's snapshot.
pub struct Decorations;

impl Decorations {
    pub fn new() -> Decorations {
        Decorations
    }
}

impl Default for Decorations {
    fn default() -> Self {
        Decorations::new()
    }
}

impl Chrome for Decorations {
    fn render(
        &mut self,
        f: &mut Frame,
        _input: &Input,
        windows: &[Window],
        _workspaces: &crate::WorkspaceSnapshot,
        out: &mut ChromeEvents,
    ) {
        for w in windows.iter() {
            let x = w.position.x as f32;
            let y = w.position.y as f32;
            let win_w = w.size.w as f32;
            // Skip if the window has no usable extent yet (just mapped).
            if win_w <= 0.0 || w.size.h <= 0 {
                continue;
            }
            let bar_rect = Rect {
                x,
                y: y - TITLE_BAR_HEIGHT,
                w: win_w,
                h: TITLE_BAR_HEIGHT,
            };
            // Activated windows get a brighter bar.
            let bg = if w.state.activated {
                Color::rgba(70, 80, 110, 255)
            } else {
                Color::rgba(40, 44, 60, 255)
            };
            let opts = OverlayOpts {
                bg,
                border: Color::rgba(20, 20, 30, 255),
                border_width: 1.0,
                pad: 4.0,
                ..Default::default()
            };
            let overlay_id = format!("ass-tbar-{}", w.id);
            f.overlay(&overlay_id, bar_rect, &opts, |f| {
                f.row(|f| {
                    // Title text grows to fill; click starts move.
                    f.flex(1.0);
                    let label = w.title.as_deref().unwrap_or("<untitled>");
                    if f.selectable(label, false) {
                        out.move_requested = Some(w.id);
                    }
                    // Close gadget.
                    f.flex(0.0);
                    f.size_next(CLOSE_BUTTON_WIDTH, TITLE_BAR_HEIGHT - 8.0);
                    if f.button("x") {
                        out.closed = Some(w.id);
                    }
                });
            });
        }
    }
}
