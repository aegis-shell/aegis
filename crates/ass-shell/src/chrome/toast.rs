//! Notification toasts: transient panels stacked at the top-right, newest on
//! top. The component reads a shared [`NotificationQueue`] each frame, so it
//! needs no per-frame snapshot from the shell and adds no `Chrome` trait
//! parameter. Entries expire on their own (the main loop ticks the queue).

use std::sync::{Arc, Mutex};

use lens::{Align, Color, Frame, Input, OverlayOpts, Rect};

use crate::{Chrome, ChromeEvents};
use ass_core::notify::NotificationQueue;
use ass_core::window::Window;
use ass_core::workspace::WorkspaceSnapshot;

const TOAST_W: f32 = 300.0;
const TOAST_H: f32 = 56.0;
const TOAST_GAP: f32 = 8.0;
const TOAST_TOP_MARGIN: f32 = 10.0;
const TOAST_RIGHT_MARGIN: f32 = 10.0;
/// Cap the visible stack so a flood does not fill the screen.
const MAX_VISIBLE: usize = 5;

/// The notification toast stack. Stateless beyond its borrowed queue handle;
/// the main loop pushes and expires entries, this component only renders.
pub struct Toast {
    queue: Arc<Mutex<NotificationQueue>>,
}

impl Toast {
    /// Construct with a shared queue the main loop also pushes to.
    pub fn new(queue: Arc<Mutex<NotificationQueue>>) -> Toast {
        Toast { queue }
    }
}

impl Chrome for Toast {
    fn render(
        &mut self,
        f: &mut Frame,
        input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        _out: &mut ChromeEvents,
    ) {
        let disp = input.as_raw().display_size;
        let queue = self.queue.lock().unwrap();
        // Newest on top: iterate the tail in reverse, capped.
        for (i, n) in queue.recent().iter().rev().take(MAX_VISIBLE).enumerate() {
            let rect = Rect {
                x: disp.x - TOAST_W - TOAST_RIGHT_MARGIN,
                y: TOAST_TOP_MARGIN + i as f32 * (TOAST_H + TOAST_GAP),
                w: TOAST_W,
                h: TOAST_H,
            };
            let opts = OverlayOpts {
                bg: Color::rgba(28, 30, 44, 235),
                border: Color::rgba(60, 64, 84, 255),
                border_width: 1.0,
                radius: 10.0,
                pad: 8.0,
                cross: Align::Start,
                ..Default::default()
            };
            let id: usize = n.id as usize;
            let overlay_id = format!("ass-toast-{id}");
            f.overlay(&overlay_id, rect, &opts, |f| {
                let title = match &n.app_id {
                    Some(app) => format!("{} · {app}", n.summary),
                    None => n.summary.clone(),
                };
                f.label(&title);
                f.label_sized(&n.body, 12.0);
            });
        }
    }
}
