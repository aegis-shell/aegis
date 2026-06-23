//! A top-center workspace indicator: one numbered tile per workspace on the
//! focused output, the current one highlighted. Clicking a tile switches to
//! that workspace. Hidden while there is only one workspace (nothing to
//! switch to). See ADR-0025.

use lens::{Align, Color, Frame, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{Chrome, ChromeEvents};
use ass_core::workspace::WorkspaceSnapshot;

/// Panel height. Holds one square tile per workspace.
const BAR_HEIGHT: f32 = 34.0;
/// Gap between the panel and the top edge of the output.
const BAR_TOP_MARGIN: f32 = 10.0;
/// Side length of a square workspace tile.
const BAR_TILE: f32 = 30.0;
/// Gap between adjacent tiles inside the panel.
const BAR_TILE_GAP: f32 = 6.0;
/// Padding inside the panel; must match the `pad` passed to the overlay opts.
const BAR_PAD: f32 = 5.0;

/// The workspace indicator. Stateless beyond its appearance; reads the shared
/// workspace snapshot each frame.
pub struct WorkspaceBar;

impl WorkspaceBar {
    pub fn new() -> WorkspaceBar {
        WorkspaceBar
    }
}

impl Default for WorkspaceBar {
    fn default() -> Self {
        WorkspaceBar::new()
    }
}

impl Chrome for WorkspaceBar {
    fn render(
        &mut self,
        f: &mut Frame,
        input: &Input,
        _windows: &[ass_core::window::Window],
        workspaces: &WorkspaceSnapshot,
        out: &mut ChromeEvents,
    ) {
        // Single-output MVP (ADR-0028 adds more): render the first output's
        // workspaces. Hide the bar while there is nothing to switch to.
        let Some(output) = workspaces.outputs.first() else {
            return;
        };
        if output.workspaces.len() < 2 {
            return;
        }

        let disp = input.as_raw().display_size;
        let n = output.workspaces.len() as f32;
        let bar_w = n * BAR_TILE + (n - 1.0) * BAR_TILE_GAP + 2.0 * BAR_PAD;
        let bar_rect = Rect {
            x: (disp.x - bar_w) * 0.5,
            y: BAR_TOP_MARGIN,
            w: bar_w,
            h: BAR_HEIGHT,
        };
        let opts = OverlayOpts {
            bg: Color::rgba(28, 30, 44, 220),
            border: Color::rgba(60, 64, 84, 255),
            border_width: 1.0,
            radius: 12.0,
            pad: BAR_PAD,
            cross: Align::Center,
            ..Default::default()
        };
        f.overlay("ass-wsbar", bar_rect, &opts, |f| {
            let row = LayoutOpts {
                gap: BAR_TILE_GAP,
                cross: Align::Center,
                ..Default::default()
            };
            f.row_ex(&row, |f| {
                for (i, ws) in output.workspaces.iter().enumerate() {
                    f.size_next(BAR_TILE, BAR_TILE);
                    // The visible number is the position (1-based); the stable
                    // workspace id is what we switch to. `[n]` marks the
                    // current workspace, ` n ` the others — same `button` API
                    // the window list uses, no special "active button" needed.
                    let is_current = output.current == Some(ws.id);
                    let label = if is_current {
                        format!("[{}]", i + 1)
                    } else {
                        format!(" {} ", i + 1)
                    };
                    if f.button(&label) {
                        out.switch_workspace = Some(ws.id);
                    }
                }
            });
        });
    }
}
