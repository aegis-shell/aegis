//! Physical-desktop guard for windows controlled by an Agent Interaction Domain.
//!
//! Input authority remains in `aegis-model::interaction_domain` and the compositor server;
//! this chrome component only makes that boundary legible to the human. It
//! dims read-only mirrors, identifies the controlling Interaction Domain, consumes physical
//! chrome input over their complete rectangles, and requests the standard
//! `not-allowed` cursor.

use aegis_model::interaction_domain::{
    InteractionDomain, InteractionDomainId, InteractionDomainSnapshot, InteractionDomainState,
};
use aegis_model::window::{Window, WindowId};
use aegis_model::workspace::WorkspaceSnapshot;
use lens::{Align, Color, Frame, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{Chrome, ChromeEvents, ChromeUpdate, CursorShape, Localizer, Message, ellipsize};

const WASH_ALPHA: u8 = 92;
const BADGE_HEIGHT: f32 = 28.0;
const BADGE_MARGIN: f32 = 8.0;
const SCANLINE_GAP: f32 = 44.0;

/// Trusted visual and pointer-routing boundary over physical read-only mirrors.
pub struct ControlledWindowGuard {
    interaction_domains: InteractionDomainSnapshot,
    has_guarded_windows: bool,
}

impl ControlledWindowGuard {
    #[must_use]
    pub fn new() -> Self {
        Self {
            interaction_domains: aegis_model::interaction_domain::InteractionDomainModel::new()
                .snapshot(),
            has_guarded_windows: false,
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
        _out: &mut ChromeEvents,
    ) {
        let raw = input.as_raw();
        let display = (raw.display_size.x.max(1.0), raw.display_size.y.max(1.0));
        for window in windows.iter().filter(|window| is_guarded_window(window)) {
            let Some(rect) = clipped_window_rect(window, display) else {
                continue;
            };
            let interaction_domain = self.controlling_interaction_domain(window.id);
            let interaction_domain_id = interaction_domain
                .map(|interaction_domain| interaction_domain.id)
                .unwrap_or(InteractionDomainId(0));
            let state = interaction_domain
                .map(|interaction_domain| interaction_domain.state)
                .unwrap_or(InteractionDomainState::Active);
            let accent = super::agent_feedback::interaction_domain_color(interaction_domain_id);
            let border = if state == InteractionDomainState::Paused {
                Color::rgba(163, 171, 188, 210)
            } else {
                accent.with_alpha(220)
            };

            frame.layer(
                &format!("aegis-controlled-window-wash-{}", window.id.0),
                rect,
                &OverlayOpts {
                    bg: Color::rgba(8, 12, 20, WASH_ALPHA),
                    border,
                    border_width: 2.0,
                    radius: if window.state.fullscreen { 0.0 } else { 7.0 },
                    ..Default::default()
                },
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
                frame.layer(
                    &format!("aegis-controlled-window-scan-{}-{index}", window.id.0),
                    Rect {
                        x: rect.x + 2.0,
                        y,
                        w: (rect.w - 4.0).max(0.0),
                        h: 1.0,
                    },
                    &OverlayOpts {
                        bg: border.with_alpha(25),
                        ..Default::default()
                    },
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
            let label = ellipsize(
                frame,
                &label,
                11.0,
                (rect.w - BADGE_MARGIN * 2.0 - 28.0).max(0.0),
            );
            let badge_width = (frame.measure_text(&label, 11.0).width + 28.0)
                .min((rect.w - BADGE_MARGIN * 2.0).max(1.0));
            let badge = Rect {
                x: rect.x + (rect.w - badge_width) * 0.5,
                y: rect.y + BADGE_MARGIN,
                w: badge_width,
                h: BADGE_HEIGHT,
            };
            frame.layer(
                &format!("aegis-controlled-window-badge-{}", window.id.0),
                badge,
                &OverlayOpts {
                    bg: Color::rgba(16, 20, 29, 232),
                    border,
                    border_width: 1.0,
                    radius: BADGE_HEIGHT * 0.5,
                    ..Default::default()
                },
                move |frame| {
                    frame.column_ex(
                        &LayoutOpts {
                            width: badge.w,
                            height: badge.h,
                            pad: 7.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |frame| frame.label_compact_sized(&label, 11.0),
                    );
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
        Some(CursorShape::NotAllowed)
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::InteractionDomains(snapshot) => {
                self.interaction_domains = snapshot.clone();
            }
            ChromeUpdate::Windows(windows) => {
                self.has_guarded_windows = windows.iter().any(is_guarded_window);
            }
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
        window.position = aegis_model::Point { x: 20, y: 30 };
        window.size = aegis_model::Size { w: 200, h: 120 };
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
}
