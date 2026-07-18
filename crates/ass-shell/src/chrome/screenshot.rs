//! Interactive screenshot region selector.
//!
//! A modal, full-screen overlay activated by the Print key. The user drags a
//! rectangle; releasing the pointer confirms the selection, while Escape
//! cancels. The selected region is emitted through [`ChromeEvents::screenshot_region`]
//! so the main loop can capture and save it.

use lens::{Color, Frame, Input, LayoutOpts, OverlayOpts, Rect as LensRect};

use crate::{Chrome, ChromeEvents, Localizer};
use ass_core::app::BuiltInApplication;
use ass_core::input::{KeyAction, KeyChar, key_action};
use ass_core::window::Window;
use ass_core::workspace::WorkspaceSnapshot;

/// Minimum drag distance in logical pixels before a release is treated as a
/// real selection rather than an accidental tap.
const MIN_DRAG: f32 = 8.0;

#[derive(Debug, Clone, Copy, Default)]
struct Point {
    x: f32,
    y: f32,
}

/// The screenshot region selector chrome component.
pub struct ScreenshotSelector {
    active: bool,
    /// Press origin in logical pixels.
    anchor: Option<Point>,
    /// Current cursor position in logical pixels.
    current: Point,
}

impl Default for ScreenshotSelector {
    fn default() -> Self {
        ScreenshotSelector::new()
    }
}

impl ScreenshotSelector {
    pub fn new() -> Self {
        Self {
            active: false,
            anchor: None,
            current: Point::default(),
        }
    }

    /// Open the selector, or close it if it is already active. The next frame
    /// will render the overlay and capture input until the user confirms or
    /// cancels.
    pub fn start(&mut self) {
        if self.active {
            self.reset();
        } else {
            self.active = true;
            self.anchor = None;
        }
    }

    /// Whether the selector is currently open.
    pub fn active(&self) -> bool {
        self.active
    }

    /// Compute the selected rectangle from anchor and current cursor, clamped
    /// to non-negative size.
    fn selected_rect(&self) -> Option<ass_core::Rect> {
        let anchor = self.anchor?;
        let x = anchor.x.min(self.current.x).round() as i32;
        let y = anchor.y.min(self.current.y).round() as i32;
        let w = (anchor.x.max(self.current.x) - x as f32).round() as i32;
        let h = (anchor.y.max(self.current.y) - y as f32).round() as i32;
        Some(ass_core::Rect::new(x, y, w.max(0), h.max(0)))
    }

    /// Advance the drag state from explicit input edges. Keeping this
    /// transition separate from painting makes the selection deterministic
    /// and lets the host rebuild edge flags every frame without maintaining a
    /// second, potentially stale button-level mirror in this component.
    fn update_pointer(
        &mut self,
        cursor: Point,
        pressed: bool,
        released: bool,
    ) -> Option<ass_core::Rect> {
        self.current = cursor;

        if self.anchor.is_none() {
            if pressed {
                self.anchor = Some(cursor);
            }
            return None;
        }
        if !released {
            return None;
        }

        let anchor = self.anchor.expect("checked above");
        let dx = (cursor.x - anchor.x).abs();
        let dy = (cursor.y - anchor.y).abs();
        let confirmed = (dx >= MIN_DRAG || dy >= MIN_DRAG)
            .then(|| self.selected_rect())
            .flatten()
            .filter(|rect| rect.size.w > 0 && rect.size.h > 0);
        self.reset();
        confirmed
    }

    fn reset(&mut self) {
        self.active = false;
        self.anchor = None;
    }
}

impl Chrome for ScreenshotSelector {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        if !self.active {
            return;
        }

        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        let cursor = Point {
            x: raw.cursor.x,
            y: raw.cursor.y,
        };
        let pressed = raw.mouse_pressed.first().copied().unwrap_or(false);
        let released = raw.mouse_released.first().copied().unwrap_or(false);
        if let Some(rect) = self.update_pointer(cursor, pressed, released) {
            out.screenshot_region = Some(rect);
            return;
        }
        if !self.active {
            return;
        }

        // Full-screen dimmed scrim.
        frame.layer(
            "ass-screenshot-scrim",
            LensRect {
                x: 0.0,
                y: 0.0,
                w: display.0,
                h: display.1,
            },
            &OverlayOpts {
                bg: Color::rgba(0, 0, 0, 140),
                ..Default::default()
            },
            |_| {},
        );

        if self.anchor.is_some() {
            let rect = self
                .selected_rect()
                .unwrap_or(ass_core::Rect::new(0, 0, 0, 0));
            let lens_rect = LensRect {
                x: rect.origin.x as f32,
                y: rect.origin.y as f32,
                w: rect.size.w.max(1) as f32,
                h: rect.size.h.max(1) as f32,
            };

            // Selection fill and border.
            frame.layer(
                "ass-screenshot-selection",
                lens_rect,
                &OverlayOpts {
                    bg: Color::rgba(100, 160, 255, 40),
                    border: Color::rgba(100, 160, 255, 220),
                    border_width: 1.5,
                    ..Default::default()
                },
                |_| {},
            );

            // Dimensions label.
            if rect.size.w > 0 && rect.size.h > 0 {
                let label = format!("{} x {}", rect.size.w, rect.size.h);
                let label_w = (label.len() as f32 * 6.5 + 12.0).min(120.0);
                let label_h = 22.0;
                let label_x = lens_rect.x.clamp(0.0, display.0 - label_w);
                let label_y = (lens_rect.y - label_h - 6.0).clamp(0.0, display.1 - label_h);
                frame.layer(
                    "ass-screenshot-label",
                    LensRect {
                        x: label_x,
                        y: label_y,
                        w: label_w,
                        h: label_h,
                    },
                    &OverlayOpts {
                        bg: Color::rgba(12, 14, 22, 210),
                        radius: 4.0,
                        ..Default::default()
                    },
                    |frame| {
                        frame.column_ex(
                            &LayoutOpts {
                                width: label_w,
                                height: label_h,
                                cross: lens::Align::Center,
                                ..Default::default()
                            },
                            |frame| {
                                frame.label_sized(&label, 11.0);
                            },
                        );
                    },
                );
            }
        }
    }

    fn captures_keyboard(&self) -> bool {
        self.active
    }

    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        self.active
    }

    fn modal_active(&self) -> bool {
        self.active
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn screenshot_active(&self) -> bool {
        self.active
    }

    fn open_builtin(&mut self, app: BuiltInApplication) {
        if app == BuiltInApplication::ScreenshotSelector {
            self.start();
        }
    }

    fn key_char(&mut self, key: &KeyChar, _out: &mut ChromeEvents) {
        if !self.active {
            return;
        }
        match key_action(key.keysym, key.ch) {
            KeyAction::Escape => self.reset(),
            KeyAction::Enter => {
                if let Some(rect) = self.selected_rect()
                    && rect.size.w > 0
                    && rect.size.h > 0
                {
                    _out.screenshot_region = Some(rect);
                }
                self.reset();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selected_rect_normalizes_negative_sizes() {
        let mut s = ScreenshotSelector::new();
        s.start();
        s.anchor = Some(Point { x: 100.0, y: 100.0 });
        s.current = Point { x: 50.0, y: 60.0 };
        let rect = s.selected_rect().unwrap();
        assert_eq!(rect.origin.x, 50);
        assert_eq!(rect.origin.y, 60);
        assert_eq!(rect.size.w, 50);
        assert_eq!(rect.size.h, 40);
    }

    #[test]
    fn selected_rect_is_none_without_anchor() {
        let s = ScreenshotSelector::new();
        assert!(s.selected_rect().is_none());
    }

    #[test]
    fn start_toggles_active_state_and_clears_prior_selection() {
        let mut s = ScreenshotSelector::new();
        s.start();
        assert!(s.active());
        s.anchor = Some(Point { x: 10.0, y: 10.0 });
        s.start();
        assert!(!s.active());
        assert!(s.anchor.is_none());
    }

    #[test]
    fn drag_geometry_tracks_each_pointer_update_and_confirms_on_release() {
        let mut s = ScreenshotSelector::new();
        s.start();
        assert!(
            s.update_pointer(Point { x: 20.0, y: 30.0 }, true, false)
                .is_none()
        );
        assert!(
            s.update_pointer(Point { x: 80.0, y: 90.0 }, false, false)
                .is_none()
        );
        assert_eq!(s.selected_rect(), Some(ass_core::Rect::new(20, 30, 60, 60)));

        let confirmed = s.update_pointer(Point { x: 110.0, y: 70.0 }, false, true);
        assert_eq!(confirmed, Some(ass_core::Rect::new(20, 30, 90, 40)));
        assert!(!s.active());
    }

    #[test]
    fn selector_captures_input_without_overriding_cursor_shape() {
        let mut s = ScreenshotSelector::new();
        s.start();
        let workspaces = WorkspaceSnapshot {
            outputs: Vec::new(),
        };
        assert!(s.captures_pointer(0.0, 0.0, (100.0, 100.0), &[], &workspaces));
        assert_eq!(
            s.cursor_shape_at(0.0, 0.0, (100.0, 100.0), &[], &workspaces),
            None
        );
    }
}
