//! Notification toasts: a frameless, display-only strip stacked at the
//! top-right, newest on top — plain floating text in the VR/AR manner, no
//! background panel, no border, no pointer capture. Each toast is visible
//! for [`TOAST_TTL_MS`] and then simply gone; every interaction (dismissal)
//! lives in the command panel's Messages section, which reads the same
//! queue's much longer retention history.
//!
//! The component reads a shared [`NotificationQueue`] each frame, so it
//! needs no per-frame snapshot from the shell and adds no `Chrome` trait
//! parameter. The queue's retention TTL no longer drives presentation: the
//! toast applies its own window against the compositor clock the queue is
//! ticked with ([`NotificationQueue::now_ms`]).

use std::sync::{Arc, Mutex};

use lens::{Align, Color, Frame, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{Chrome, ChromeEvents, Localizer, ellipsize};
use aegis_core::notify::{Notification, NotificationQueue};
use aegis_core::window::Window;
use aegis_core::workspace::WorkspaceSnapshot;

const TOAST_W: f32 = 300.0;
const TOAST_H: f32 = 64.0;
const TOAST_GAP: f32 = 8.0;
const TOAST_TOP_MARGIN: f32 = crate::HUD_HEIGHT + 10.0;
const TOAST_RIGHT_MARGIN: f32 = 10.0;
/// Cap the visible stack so a flood does not fill the screen.
const MAX_VISIBLE: usize = 5;
/// How long a toast stays on screen. Aging out is not a queue mutation, so
/// it bumps no revision — [`Self::anim_pending`] keeps the frames coming
/// until every visible toast has crossed the window.
const TOAST_TTL_MS: u64 = 3_000;

/// Whether `n` is still inside its presentation window at `now_ms`.
fn within_window(n: &Notification, now_ms: u64) -> bool {
    now_ms.saturating_sub(n.at_ms) <= TOAST_TTL_MS
}

/// The notification toast strip. Beyond its borrowed queue handle it caches
/// the rendered entries keyed by the queue's revision, so an unchanged queue
/// is not re-cloned every frame; the per-frame age filter runs on the cached
/// entries. The main loop pushes, expires, and dismisses entries — this
/// component only renders, and captures no input.
pub struct Toast {
    queue: Arc<Mutex<NotificationQueue>>,
    /// `(revision, entries)` of the last clone from the queue; `None` until
    /// the first render (or after do-not-disturb hid the stack).
    cache: Option<(u64, Vec<Notification>)>,
}

impl Toast {
    /// Construct with a shared queue the main loop also pushes to.
    pub fn new(queue: Arc<Mutex<NotificationQueue>>) -> Toast {
        Toast { queue, cache: None }
    }

    /// The entries eligible for presentation right now: do-not-disturb off,
    /// inside the toast window, newest first, capped. Reads the queue under
    /// a brief lock; `render` reuses the revision cache instead.
    fn presentable(&self) -> Vec<Notification> {
        let queue = self.queue.lock().unwrap();
        if queue.do_not_disturb() {
            return Vec::new();
        }
        let now_ms = queue.now_ms();
        queue
            .recent()
            .iter()
            .rev()
            .filter(|n| within_window(n, now_ms))
            .take(MAX_VISIBLE)
            .cloned()
            .collect()
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
        _out: &mut ChromeEvents,
    ) {
        let disp = input.as_raw().display_size;
        // Re-clone only when the queue's revision moved; otherwise render
        // the cached entries. Newest on top: iterate the tail in reverse,
        // capped.
        let notifications: &[Notification] = {
            let queue = self.queue.lock().unwrap();
            if queue.do_not_disturb() {
                self.cache = None;
                &[]
            } else {
                let stale = self
                    .cache
                    .as_ref()
                    .map(|(revision, _)| *revision != queue.revision())
                    .unwrap_or(true);
                if stale {
                    let entries = queue
                        .recent()
                        .iter()
                        .rev()
                        .take(MAX_VISIBLE)
                        .cloned()
                        .collect();
                    self.cache = Some((queue.revision(), entries));
                }
                &self.cache.as_ref().unwrap().1
            }
        };
        let now_ms = self.queue.lock().unwrap().now_ms();
        let mut slot = 0;
        for n in notifications {
            // The age filter runs every frame: a toast crossing its window
            // is not a queue mutation and bumps no revision.
            if !within_window(n, now_ms) {
                continue;
            }
            let rect = Rect {
                x: disp.x - TOAST_W - TOAST_RIGHT_MARGIN,
                y: TOAST_TOP_MARGIN + slot as f32 * (TOAST_H + TOAST_GAP),
                w: TOAST_W,
                h: TOAST_H,
            };
            slot += 1;
            // Frameless: transparent background, no border, no radius —
            // floating text over the desktop.
            let opts = OverlayOpts {
                bg: Color::TRANSPARENT,
                border: Color::TRANSPARENT,
                border_width: 0.0,
                radius: 0.0,
                pad: 0.0,
                ..Default::default()
            };
            let id: usize = n.id as usize;
            let overlay_id = format!("aegis-toast-{id}");
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
                        let text_width = (rect.w - 18.0).max(0.0);
                        let title = ellipsize(f, &title, 12.0, text_width);
                        f.label_compact_sized(&title, 12.0);
                        if !n.body.is_empty() {
                            let body = ellipsize(f, &n.body, 10.5, text_width);
                            f.label_compact_sized(&body, 10.5);
                        }
                    },
                );
            });
        }
    }

    fn anim_pending(&self) -> bool {
        // A toast aging out produces no queue event; keep the frames coming
        // until the last visible toast crosses its window so the strip
        // actually clears on screen.
        !self.presentable().is_empty()
    }

    fn requires_composition(&self) -> bool {
        !self.presentable().is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_copy_is_unicode_safe_and_bounded() {
        assert_eq!(crate::truncate("Fuji connected", 20), "Fuji connected");
        assert_eq!(crate::truncate("真实通知已经联通", 6), "真实通知已…");
    }

    #[test]
    fn presentation_window_is_three_seconds_inclusive() {
        let n = Notification {
            id: 0,
            summary: "title".into(),
            body: String::new(),
            app_id: None,
            external_id: None,
            at_ms: 10_000,
        };
        assert!(within_window(&n, 10_000), "fresh toast is visible");
        assert!(within_window(&n, 13_000), "exactly 3s is still visible");
        assert!(!within_window(&n, 13_001), "past 3s the toast is gone");
        // A clock reading older than the post time never underflows.
        assert!(within_window(&n, 5_000));
    }
}
