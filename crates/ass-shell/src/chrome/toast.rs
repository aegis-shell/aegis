//! Notification toasts: transient panels stacked at the top-right, newest on
//! top. The component reads a shared [`NotificationQueue`] each frame, so it
//! needs no per-frame snapshot from the shell and adds no `Chrome` trait
//! parameter. Entries expire on their own (the main loop ticks the queue).

use std::sync::{Arc, Mutex};

use lens::{Align, Color, Frame, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{Chrome, ChromeEvents, Localizer};
use ass_core::notify::NotificationQueue;
use ass_core::window::Window;
use ass_core::workspace::WorkspaceSnapshot;

const TOAST_W: f32 = 300.0;
const TOAST_H: f32 = 64.0;
const TOAST_GAP: f32 = 8.0;
const TOAST_TOP_MARGIN: f32 = crate::HUD_HEIGHT + 10.0;
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
        _i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let disp = input.as_raw().display_size;
        let cursor = input.as_raw().cursor;
        let pressed = input
            .as_raw()
            .mouse_pressed
            .first()
            .copied()
            .unwrap_or(false);
        let notifications: Vec<_> = {
            let queue = self.queue.lock().unwrap();
            if queue.do_not_disturb() {
                Vec::new()
            } else {
                queue
                    .recent()
                    .iter()
                    .rev()
                    .take(MAX_VISIBLE)
                    .cloned()
                    .collect()
            }
        };
        // Newest on top: iterate the tail in reverse, capped.
        for (i, n) in notifications.iter().enumerate() {
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
                ..Default::default()
            };
            let id: usize = n.id as usize;
            let overlay_id = format!("ass-toast-{id}");
            f.layer(&overlay_id, rect, &opts, |f| {
                let title = match &n.app_id {
                    Some(app) => format!("{} · {app}", n.summary),
                    None => n.summary.clone(),
                };
                f.column_ex(
                    &LayoutOpts {
                        width: rect.w,
                        height: rect.h,
                        gap: 3.0,
                        pad: 9.0,
                        cross: Align::Start,
                        ..Default::default()
                    },
                    |f| {
                        f.label_compact_sized(&truncate(&title, 42), 12.0);
                        if !n.body.is_empty() {
                            f.label_compact_sized(&truncate(&n.body, 52), 10.5);
                        }
                    },
                );
            });
            if pressed
                && cursor.x >= rect.x
                && cursor.x < rect.x + rect.w
                && cursor.y >= rect.y
                && cursor.y < rect.y + rect.h
            {
                out.dismissed_notification = Some(n.id);
            }
        }
    }

    fn captures_pointer(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        let queue = self.queue.lock().unwrap();
        if queue.do_not_disturb() {
            return false;
        }
        let visible = queue.recent().len().min(MAX_VISIBLE);
        (0..visible).any(|i| {
            let left = display.0 - TOAST_W - TOAST_RIGHT_MARGIN;
            let top = TOAST_TOP_MARGIN + i as f32 * (TOAST_H + TOAST_GAP);
            x >= left && x < left + TOAST_W && y >= top && y < top + TOAST_H
        })
    }
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_copy_is_unicode_safe_and_bounded() {
        assert_eq!(truncate("Neenee connected", 20), "Neenee connected");
        assert_eq!(truncate("真实通知已经联通", 6), "真实通知已…");
    }
}
