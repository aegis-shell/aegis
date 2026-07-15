//! A top-center workspace indicator, macOS "Spaces"-style: a slim translucent
//! pill holding one dot per workspace on the focused output, the current one
//! drawn bright and the rest dim. Clicking a dot switches to that workspace.
//! Hidden while there is only one workspace (nothing to switch to). See
//! ADR-0025.
//!
//! Like the dock, the dots are drawn as their own small layers and clicks are
//! hit-tested against the dot slots, so the indicator reads as a row of dots
//! rather than a strip of buttons.

use lens::{Align, Color, Frame, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{Chrome, ChromeEvents};
use ass_core::workspace::WorkspaceSnapshot;

/// Height of the indicator pill.
const BAR_HEIGHT: f32 = 22.0;
/// Gap between the pill and the top edge of the output.
const BAR_TOP_MARGIN: f32 = 10.0;
/// Diameter of a workspace dot.
const BAR_DOT: f32 = 9.0;
/// Gap between adjacent dots.
const BAR_DOT_GAP: f32 = 9.0;
/// Padding between the pill edge and the first/last dot.
const BAR_PAD: f32 = 10.0;

/// The workspace indicator. Reads the shared workspace snapshot each frame;
/// the only state is the button-down level from last frame, for press-edge
/// click detection.
pub struct WorkspaceBar {
    prev_down: bool,
}

impl WorkspaceBar {
    pub fn new() -> WorkspaceBar {
        WorkspaceBar { prev_down: false }
    }

    fn bounds(workspaces: &WorkspaceSnapshot, display_w: f32) -> Option<Rect> {
        let output = workspaces.outputs.first()?;
        if output.workspaces.len() < 2 {
            return None;
        }
        let n = output.workspaces.len();
        let bar_w =
            n as f32 * BAR_DOT + (n as f32 - 1.0) * BAR_DOT_GAP + 2.0 * BAR_PAD;
        Some(Rect {
            x: (display_w - bar_w) * 0.5,
            y: BAR_TOP_MARGIN,
            w: bar_w,
            h: BAR_HEIGHT,
        })
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
        let cursor = input.as_raw().cursor;
        // Click once on the press edge: the host does not clear the per-frame
        // pressed flag, so track the button-down level transition ourselves.
        let down = input.as_raw().mouse_down.first().copied().unwrap_or(false);
        let pressed = down && !self.prev_down;
        self.prev_down = down;

        let n = output.workspaces.len();
        let bar_w =
            n as f32 * BAR_DOT + (n as f32 - 1.0) * BAR_DOT_GAP + 2.0 * BAR_PAD;
        let bar_x = (disp.x - bar_w) * 0.5;
        let bar_rect = Rect {
            x: bar_x,
            y: BAR_TOP_MARGIN,
            w: bar_w,
            h: BAR_HEIGHT,
        };
        let dot_y = BAR_TOP_MARGIN + BAR_HEIGHT * 0.5;
        let centre = |i: usize| bar_x + BAR_PAD + i as f32 * (BAR_DOT + BAR_DOT_GAP) + BAR_DOT * 0.5;

        // The pill background. A layer's rect is only an anchor, not a size, so
        // a fixed-size child forces it to the pill size.
        f.layer("ass-wsbar", bar_rect, &pill_opts(), |f| {
            f.column_ex(&sized(bar_w, BAR_HEIGHT), |_| {});
        });

        // Dots, plus click hit-testing against each dot's slot.
        let half = (BAR_DOT + BAR_DOT_GAP) * 0.5;
        let mut clicked: Option<usize> = None;
        for (i, ws) in output.workspaces.iter().enumerate() {
            let is_current = output.current == Some(ws.id);
            let cx = centre(i);
            let dot_rect = Rect {
                x: cx - BAR_DOT * 0.5,
                y: dot_y - BAR_DOT * 0.5,
                w: BAR_DOT,
                h: BAR_DOT,
            };
            let color = if is_current {
                Color::rgba(236, 238, 245, 255)
            } else {
                Color::rgba(150, 156, 178, 140)
            };
            let id = format!("ass-wsdot-{}", ws.id.0);
            f.layer(&id, dot_rect, &OverlayOpts::default(), |f| {
                f.column_ex(&sized_fill(BAR_DOT, BAR_DOT, color, BAR_DOT * 0.5), |_| {});
            });

            if pressed
                && (cursor.x - cx).abs() <= half
                && cursor.y >= BAR_TOP_MARGIN
                && cursor.y <= BAR_TOP_MARGIN + BAR_HEIGHT
            {
                clicked = Some(i);
            }
        }
        if let Some(i) = clicked {
            out.switch_workspace = Some(output.workspaces[i].id);
        }
    }

    fn captures_pointer(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        _windows: &[ass_core::window::Window],
        workspaces: &WorkspaceSnapshot,
    ) -> bool {
        Self::bounds(workspaces, display.0)
            .is_some_and(|r| x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h)
    }
}

/// The indicator pill background.
fn pill_opts() -> OverlayOpts {
    OverlayOpts {
        bg: Color::rgba(28, 30, 44, 200),
        border: Color::rgba(70, 74, 96, 140),
        border_width: 1.0,
        radius: BAR_HEIGHT * 0.5,
        pad: 0.0,
        cross: Align::Center,
        ..Default::default()
    }
}

/// A fixed-size transparent container, to force a layer to a known size.
fn sized(w: f32, h: f32) -> LayoutOpts {
    LayoutOpts {
        width: w,
        height: h,
        ..Default::default()
    }
}

/// A fixed-size container that paints a rounded `bg` — a filled circle/dot when
/// `radius` is half the side.
fn sized_fill(w: f32, h: f32, bg: Color, radius: f32) -> LayoutOpts {
    LayoutOpts {
        width: w,
        height: h,
        bg,
        radius,
        ..Default::default()
    }
}
