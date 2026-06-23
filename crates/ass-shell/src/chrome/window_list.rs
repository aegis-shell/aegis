//! The top-left side panel: Quit button and the per-window list.

use lens::{Frame, Input};

use crate::{Chrome, ChromeEvents};
use ass_core::window::Window;

/// The window-list side panel. Stateless beyond its appearance; reads the
/// shared snapshot each frame.
pub struct WindowList;

impl WindowList {
    pub fn new() -> WindowList {
        WindowList
    }
}

impl Default for WindowList {
    fn default() -> Self {
        WindowList::new()
    }
}

impl Chrome for WindowList {
    fn render(
        &mut self,
        f: &mut Frame,
        _input: &Input,
        windows: &[Window],
        out: &mut ChromeEvents,
    ) {
        // Stays in the auto-layout flow at the top-left of the output.
        f.column(|f| {
            f.title("ass");
            f.label("autonomous surface shell");
            f.spacer(8.0);
            if f.button("Quit") {
                out.quit = true;
            }
            if !windows.is_empty() {
                f.separator();
                f.label("Windows");
                for w in windows.iter() {
                    let title = w.title.as_deref().unwrap_or("<untitled>");
                    // ▶ marks the activated window; ◌ marks a minimized one
                    // (hidden from the screen, click to restore + focus).
                    let label = if w.state.activated {
                        format!("\u{25b6} {title}")
                    } else if w.minimized {
                        format!("\u{25cc} {title}")
                    } else {
                        format!("  {title}")
                    };
                    f.row(|f| {
                        f.flex(1.0);
                        if f.button(&label) {
                            out.clicked = Some(w.id);
                        }
                        f.flex(0.0);
                        if f.button("x") {
                            out.closed = Some(w.id);
                        }
                    });
                }
            }
        });
    }
}
