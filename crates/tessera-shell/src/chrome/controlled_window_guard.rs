//! Physical-desktop guard for windows controlled by an Agent Interaction Domain.
//!
//! Input authority remains in `tessera-model::interaction_domain` and the compositor server;
//! this chrome component only makes that boundary legible to the human. It
//! dims read-only mirrors, identifies the controlling Interaction Domain, consumes physical
//! chrome input over their complete rectangles, and requests the standard
//! `not-allowed` cursor. A press-and-drag on a mirror is the one permitted
//! gesture: it asks the compositor to move the window (position is
//! presentation state; the human still cannot focus, resize, close, or
//! deliver content input to it).

use tessera_design::Design;
use tessera_design::materials::{chrome_place, surface_layout};
use tessera_model::interaction_domain::{
    InteractionDomain, InteractionDomainSnapshot, InteractionDomainState,
};
use tessera_model::window::{Window, WindowId};
use tessera_model::workspace::WorkspaceSnapshot;
use lens::{Frame, Input, LayoutOpts, Rect};

use crate::{
    Chrome, ChromeEvents, ChromeUpdate, CursorShape, Localizer, Message, MirrorMove, ellipsize,
};

const WASH_ALPHA: u8 = 92;
const BADGE_HEIGHT: f32 = 28.0;
const BADGE_MARGIN: f32 = 8.0;
const SCANLINE_GAP: f32 = 44.0;
/// Pointer travel before a press on a mirror becomes a move drag.
const DRAG_THRESHOLD: f32 = 6.0;

/// Trusted visual and pointer-routing boundary over physical read-only mirrors.
pub struct ControlledWindowGuard {
    interaction_domains: InteractionDomainSnapshot,
    has_guarded_windows: bool,
    /// The design snapshot the guard paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`crate::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
    /// Mirror window pressed this stroke, with the press position. Becomes a
    /// move request once the pointer travels past [`DRAG_THRESHOLD`].
    drag_candidate: Option<(WindowId, (f32, f32))>,
    /// The current stroke already emitted its one move request.
    drag_emitted: bool,
    prev_down: bool,
}

impl ControlledWindowGuard {
    #[must_use]
    pub fn new() -> Self {
        Self {
            interaction_domains: tessera_model::interaction_domain::InteractionDomainModel::new()
                .snapshot(),
            has_guarded_windows: false,
            design: Design::dark(),
            drag_candidate: None,
            drag_emitted: false,
            prev_down: false,
        }
    }

    fn controlling_interaction_domain(&self, window: WindowId) -> Option<&InteractionDomain> {
        let interaction_domain = self
            .interaction_domains
            .interaction_groups
            .iter()
            .find(|group| group.windows.contains(&window))
            .map(|group| group.control_interaction_domain)?;
        self.interaction_domains
            .interaction_domains
            .iter()
            .find(|candidate| candidate.id == interaction_domain)
    }

    /// Track the physical primary button over mirror rectangles. A press
    /// arms a drag candidate; traveling past the threshold emits exactly one
    /// move request for the stroke (the compositor's interactive-move grab
    /// then follows the pointer until release). Clicks that never travel
    /// stay swallowed: the mirror remains an input barrier.
    fn track_mirror_drag(
        &mut self,
        windows: &[Window],
        cursor: (f32, f32),
        down: bool,
    ) -> Option<MirrorMove> {
        let pressed = down && !self.prev_down;
        self.prev_down = down;
        if !down {
            self.drag_candidate = None;
            self.drag_emitted = false;
            return None;
        }
        if pressed {
            self.drag_candidate =
                guarded_window_at(windows, cursor.0, cursor.1).map(|window| (window, cursor));
            self.drag_emitted = false;
        }
        if self.drag_emitted {
            return None;
        }
        let (window, origin) = self.drag_candidate?;
        if (cursor.0 - origin.0).hypot(cursor.1 - origin.1) < DRAG_THRESHOLD {
            return None;
        }
        self.drag_emitted = true;
        Some(MirrorMove { window, cursor })
    }
}

impl Default for ControlledWindowGuard {
    fn default() -> Self {
        Self::new()
    }
}

impl Chrome for ControlledWindowGuard {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let raw = input.as_raw();
        let cursor = (raw.cursor.x, raw.cursor.y);
        let down = raw.mouse_down.first().copied().unwrap_or(false);
        if let Some(request) = self.track_mirror_drag(windows, cursor, down) {
            out.mirror_move = Some(request);
        }
        let display = (raw.display_size.x.max(1.0), raw.display_size.y.max(1.0));
        for window in windows.iter().filter(|window| is_guarded_window(window)) {
            let Some(rect) = clipped_window_rect(window, display) else {
                continue;
            };
            let interaction_domain = self.controlling_interaction_domain(window.id);
            let state = interaction_domain
                .map(|interaction_domain| interaction_domain.state)
                .unwrap_or(InteractionDomainState::Active);
            let border = if state == InteractionDomainState::Paused {
                self.design.colors.menu_text_disabled.with_alpha(210)
            } else {
                self.design.colors.application_border.with_alpha(210)
            };

            frame.place(
                &format!("tessera-controlled-window-wash-{}", window.id.0),
                &chrome_place(
                    rect,
                    LayoutOpts {
                        bg: self.design.colors.modal_scrim.with_alpha(WASH_ALPHA),
                        border,
                        border_width: 2.0,
                        radius: if window.state.fullscreen { 0.0 } else { 7.0 },
                        ..surface_layout()
                    },
                ),
                |_| {},
            );

            // Sparse scan lines make the window read as a protected surface
            // without obscuring the Agent's work from the observer.
            let scanlines = ((rect.h / SCANLINE_GAP).ceil() as usize).min(16);
            for index in 1..scanlines {
                let y = rect.y + index as f32 * SCANLINE_GAP;
                if y >= rect.y + rect.h - 1.0 {
                    break;
                }
                frame.place(
                    &format!("tessera-controlled-window-scan-{}-{index}", window.id.0),
                    &chrome_place(
                        Rect {
                            x: rect.x + 2.0,
                            y,
                            w: (rect.w - 4.0).max(0.0),
                            h: 1.0,
                        },
                        LayoutOpts {
                            bg: border.with_alpha(25),
                            ..surface_layout()
                        },
                    ),
                    |_| {},
                );
            }

            if rect.w < 96.0 || rect.h < BADGE_HEIGHT + BADGE_MARGIN * 2.0 {
                continue;
            }
            let status = if state == InteractionDomainState::Paused {
                i18n.text(Message::InteractionDomainPaused)
            } else {
                i18n.text(Message::AgentOperating)
            };
            let label = match interaction_domain
                .map(|interaction_domain| interaction_domain.label.as_str())
            {
                Some(label) if !label.is_empty() => {
                    format!(
                        "{label} · {status} · {}",
                        i18n.text(Message::ReadOnlyMirror)
                    )
                }
                _ => format!("{status} · {}", i18n.text(Message::ReadOnlyMirror)),
            };
            let footnote = self.design.typography.footnote;
            let label = ellipsize(
                frame,
                &label,
                footnote,
                (rect.w - BADGE_MARGIN * 2.0 - 28.0).max(0.0),
            );
            let badge_width = (frame.measure_text(&label, footnote).width + 28.0)
                .min((rect.w - BADGE_MARGIN * 2.0).max(1.0));
            let badge = Rect {
                x: rect.x + (rect.w - badge_width) * 0.5,
                y: rect.y + BADGE_MARGIN,
                w: badge_width,
                h: BADGE_HEIGHT,
            };
            frame.place(
                &format!("tessera-controlled-window-badge-{}", window.id.0),
                &chrome_place(
                    badge,
                    LayoutOpts {
                        bg: self.design.colors.application_surface.with_alpha(232),
                        border,
                        border_width: 1.0,
                        radius: BADGE_HEIGHT * 0.5,
                        ..surface_layout()
                    },
                ),
                move |frame| {
                    frame.centered(badge.w, badge.h, |frame| {
                        frame.label_compact_sized(&label, footnote)
                    });
                },
            );
        }
    }

    fn captures_pointer(
        &self,
        x: f32,
        y: f32,
        _display: (f32, f32),
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        guarded_window_at(windows, x, y).is_some()
    }

    fn cursor_shape_at(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        // A plain click is still barred, but once the stroke becomes a move
        // drag the mirror follows the pointer like any grabbed window.
        if self.drag_emitted {
            Some(CursorShape::Default)
        } else {
            Some(CursorShape::NotAllowed)
        }
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::InteractionDomains(snapshot) => {
                self.interaction_domains = snapshot.clone();
            }
            ChromeUpdate::Windows(windows) => {
                self.has_guarded_windows = windows.iter().any(is_guarded_window);
            }
            ChromeUpdate::Appearance(design) => self.design = *design,
            _ => {}
        }
    }

    fn requires_composition(&self) -> bool {
        self.has_guarded_windows
    }
}

fn is_guarded_window(window: &Window) -> bool {
    window.read_only && !window.minimized && window.size.w > 0 && window.size.h > 0
}

fn guarded_window_at(windows: &[Window], x: f32, y: f32) -> Option<WindowId> {
    windows
        .iter()
        .rev()
        .find(|window| is_guarded_window(window) && window.contains_point(x, y))
        .map(|window| window.id)
}

fn clipped_window_rect(window: &Window, display: (f32, f32)) -> Option<Rect> {
    let left = (window.position.x as f32).max(0.0);
    let top = (window.position.y as f32).max(0.0);
    let right = (window.position.x as f32 + window.size.w as f32).min(display.0);
    let bottom = (window.position.y as f32 + window.size.h as f32).min(display.1);
    (right > left && bottom > top).then_some(Rect {
        x: left,
        y: top,
        w: right - left,
        h: bottom - top,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mirror() -> Window {
        let mut window = Window::new(WindowId(7));
        window.read_only = true;
        window.position = tessera_model::Point { x: 20, y: 30 };
        window.size = tessera_model::Size { w: 200, h: 120 };
        window
    }

    #[test]
    fn only_live_read_only_window_rectangles_are_guarded() {
        let mut window = mirror();
        assert_eq!(
            guarded_window_at(&[window.clone()], 25.0, 35.0),
            Some(window.id)
        );
        assert_eq!(guarded_window_at(&[window.clone()], 10.0, 10.0), None);

        window.read_only = false;
        assert_eq!(guarded_window_at(&[window.clone()], 25.0, 35.0), None);
        window.read_only = true;
        window.minimized = true;
        assert_eq!(guarded_window_at(&[window], 25.0, 35.0), None);
    }

    #[test]
    fn guard_requests_the_protocol_not_allowed_cursor() {
        let guard = ControlledWindowGuard::new();
        let windows = vec![mirror()];
        let workspaces = WorkspaceSnapshot {
            outputs: Vec::new(),
        };
        assert!(guard.captures_pointer(25.0, 35.0, (800.0, 600.0), &windows, &workspaces));
        assert_eq!(
            guard.cursor_shape_at(25.0, 35.0, (800.0, 600.0), &windows, &workspaces),
            Some(CursorShape::NotAllowed)
        );
    }

    #[test]
    fn press_and_drag_on_a_mirror_requests_one_move() {
        let mut guard = ControlledWindowGuard::new();
        let windows = vec![mirror()];
        // A press inside the mirror arms the drag without emitting yet.
        assert_eq!(guard.track_mirror_drag(&windows, (25.0, 35.0), true), None);
        // Jitter below the threshold stays a (swallowed) click.
        assert_eq!(
            guard.track_mirror_drag(&windows, (25.0 + DRAG_THRESHOLD - 2.0, 35.0), true),
            None
        );
        // Crossing the threshold emits exactly one move request per stroke.
        let request = guard.track_mirror_drag(&windows, (60.0, 60.0), true);
        assert_eq!(
            request,
            Some(MirrorMove {
                window: WindowId(7),
                cursor: (60.0, 60.0)
            })
        );
        assert_eq!(guard.track_mirror_drag(&windows, (80.0, 80.0), true), None);
        // Release resets the stroke; the next press re-arms.
        assert_eq!(guard.track_mirror_drag(&windows, (80.0, 80.0), false), None);
        assert_eq!(guard.track_mirror_drag(&windows, (25.0, 35.0), true), None);
        assert!(
            guard
                .track_mirror_drag(&windows, (90.0, 90.0), true)
                .is_some()
        );
    }

    #[test]
    fn presses_outside_a_mirror_never_start_a_drag() {
        let mut guard = ControlledWindowGuard::new();
        let windows = vec![mirror()];
        assert_eq!(
            guard.track_mirror_drag(&windows, (500.0, 500.0), true),
            None
        );
        assert_eq!(
            guard.track_mirror_drag(&windows, (600.0, 600.0), true),
            None
        );
    }
}
