//! Held-Super window switcher chrome.
//!
//! The compositor renderer paints each live window into the preview rects
//! from `aegis_core::window_switcher`; this component adds the glass panel,
//! selection borders, icons, and labels. Focus moves on each Super+Tab press,
//! and releasing Super closes the strip.

use lens::{Align, Color, Frame, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{AppCatalog, Chrome, ChromeEvents, IconSet, Localizer};
use aegis_core::window::Window;
use aegis_core::workspace::WorkspaceSnapshot;

const FADE_RATE: f32 = 18.0;

pub struct WindowSwitcher {
    open: bool,
    visibility: f32,
    anim_active: bool,
    reduced_motion: bool,
    icons: IconSet,
}

impl Default for WindowSwitcher {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowSwitcher {
    pub fn new() -> Self {
        Self {
            open: false,
            visibility: 0.0,
            anim_active: false,
            reduced_motion: false,
            icons: IconSet::default(),
        }
    }

    fn advance(&mut self, dt: f32) {
        let target = if self.open { 1.0 } else { 0.0 };
        if self.reduced_motion {
            self.visibility = target;
            self.anim_active = false;
            return;
        }
        let blend = (dt * FADE_RATE).min(1.0);
        self.visibility += (target - self.visibility) * blend;
        self.anim_active = (self.visibility - target).abs() > 0.002;
        if !self.anim_active {
            self.visibility = target;
        }
    }

    fn alpha(&self, value: u8) -> u8 {
        (value as f32 * self.visibility).round() as u8
    }
}

impl Chrome for WindowSwitcher {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
        self.advance(input.as_raw().dt_seconds.max(0.0));
        if self.visibility <= 0.001 && !self.open {
            return;
        }

        let display = aegis_core::Rect::new(
            0,
            0,
            input.as_raw().display_size.x.max(1.0) as i32,
            input.as_raw().display_size.y.max(1.0) as i32,
        );
        let layout = aegis_core::window_switcher::layout(display, windows.len());
        let panel = to_lens(layout.panel);
        frame.layer(
            "aegis-window-switcher-panel",
            panel,
            &OverlayOpts {
                bg: Color::rgba(18, 21, 30, self.alpha(110)),
                border: Color::rgba(176, 190, 220, self.alpha(90)),
                border_width: 1.0,
                radius: 20.0,
                ..Default::default()
            },
            |frame| {
                frame.column_ex(
                    &LayoutOpts {
                        width: panel.w,
                        height: panel.h,
                        ..Default::default()
                    },
                    |_| {},
                );
            },
        );

        for (index, (window, card)) in windows.iter().zip(layout.cards.iter()).enumerate() {
            let selected = window.state.activated;
            let outer = to_lens(card.outer);
            frame.layer(
                &format!("aegis-window-switcher-card-{index}"),
                outer,
                &OverlayOpts {
                    bg: Color::rgba(8, 10, 16, self.alpha(if selected { 36 } else { 18 })),
                    border: if selected {
                        Color::rgba(116, 170, 255, self.alpha(255))
                    } else {
                        Color::rgba(164, 174, 196, self.alpha(105))
                    },
                    border_width: if selected { 3.0 } else { 1.0 },
                    radius: 13.0,
                    ..Default::default()
                },
                |_| {},
            );

            let label_rect = to_lens(card.label);
            let title = window
                .title
                .as_deref()
                .or(window.app_id.as_deref())
                .unwrap_or("Untitled");
            let label = truncate(title, (label_rect.w / 7.0).max(5.0) as usize);
            let icon = window
                .app_id
                .as_deref()
                .and_then(|app_id| self.icons.get(&app_id.to_ascii_lowercase()));
            frame.layer(
                &format!("aegis-window-switcher-label-{index}"),
                label_rect,
                &OverlayOpts {
                    bg: Color::rgba(12, 14, 21, self.alpha(218)),
                    radius: 9.0,
                    ..Default::default()
                },
                move |frame| {
                    frame.row_ex(
                        &LayoutOpts {
                            width: label_rect.w,
                            height: label_rect.h,
                            gap: 7.0,
                            pad: 8.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        move |frame| {
                            if let Some(icon) = icon {
                                unsafe {
                                    frame.image(icon as *mut lens::sys::flux_image, 20.0, 20.0);
                                }
                            }
                            frame.label_compact_sized(&label, 11.5);
                        },
                    );
                },
            );
        }
    }

    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        self.open
    }

    fn modal_active(&self) -> bool {
        self.open || self.visibility > 0.01
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn anim_pending(&self) -> bool {
        self.anim_active
    }

    fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
    }

    fn update_app_catalog(&mut self, catalog: &AppCatalog) {
        self.icons = catalog.icons.clone();
    }

    fn start_window_switcher(&mut self) {
        self.open = true;
        self.anim_active = true;
    }

    fn finish_window_switcher(&mut self) {
        if self.open {
            self.open = false;
            self.anim_active = true;
        }
    }

    fn window_switcher_active(&self) -> bool {
        self.open || self.visibility > 0.01
    }
}

fn to_lens(rect: aegis_core::Rect) -> Rect {
    Rect {
        x: rect.origin.x as f32,
        y: rect.origin.y as f32,
        w: rect.size.w as f32,
        h: rect.size.h as f32,
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    if text.chars().count() <= max_chars {
        return text.to_string();
    }
    let mut truncated: String = text.chars().take(max_chars.saturating_sub(1)).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn held_switcher_opens_and_modifier_release_closes_it() {
        let mut switcher = WindowSwitcher::new();
        switcher.start_window_switcher();
        assert!(switcher.window_switcher_active());
        switcher.finish_window_switcher();
        assert!(!switcher.open);
        assert!(switcher.anim_pending());
    }
}
