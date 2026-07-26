//! Interactive screenshot region selector and portal target picker.
//!
//! A modal, full-screen overlay activated by the Print key. The user drags a
//! rectangle; releasing the pointer only *stages* the selection — the frozen
//! screen keeps showing it until the user explicitly confirms (Enter/Space)
//! or cancels (Escape). A new press starts a fresh drag, replacing the staged
//! selection. The confirmed region is emitted through
//! [`ChromeEvents::screenshot_region`] so the main loop can capture and save
//! it.
//!
//! The same overlay also serves the portal's interactive picks (ADR-0054),
//! opened through [`Chrome::start_pick`] with a [`PickerMode`]:
//!
//! - `Region` is the Print-key interaction, but the confirmed rect goes to
//!   the waiting IPC request instead of the screenshot file path.
//! - `Pixel` draws a full-screen crosshair; a click emits the picked point
//!   through [`ChromeEvents::picked_point`].
//! - `Window` highlights the window under the cursor; a click emits its id
//!   through [`ChromeEvents::picked_window`], a click on empty desktop (or
//!   Enter/Space) chooses the whole output through
//!   [`ChromeEvents::pick_output`].
//!
//! Escape always cancels a picker session through
//! [`ChromeEvents::pick_cancelled`]; the compositor closes the loop by
//! answering the IPC request.

use lens::{Color, Frame, Input, LayoutOpts, OverlayOpts, Rect as LensRect};

use crate::{Chrome, ChromeEvents, Localizer, Message};
use aegis_core::app::BuiltInApplication;
use aegis_core::input::{KeyAction, KeyChar, key_action};
use aegis_core::window::{Window, WindowId};
use aegis_core::workspace::WorkspaceSnapshot;

/// Minimum drag distance in logical pixels before a release is treated as a
/// real selection rather than an accidental tap.
const MIN_DRAG: f32 = 8.0;

/// The interaction a portal picker session asks for (ADR-0054).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerMode {
    /// Drag out a screen region (the Print-key interaction).
    Region,
    /// Click one screen point (colour picking).
    Pixel,
    /// Click a window, or choose the whole output.
    Window,
}

#[derive(Debug, Clone, Copy, Default)]
struct Point {
    x: f32,
    y: f32,
}

/// The screenshot region selector / portal picker chrome component.
pub struct ScreenshotSelector {
    active: bool,
    /// Current interaction; `Region` outside picker sessions.
    mode: PickerMode,
    /// Whether the session was opened for an IPC pick (ADR-0054) rather than
    /// the Print key. Picker sessions emit the pick events and are the only
    /// ones [`Chrome::cancel_pick`] may interrupt.
    picker: bool,
    /// Press origin in logical pixels while a drag is in progress.
    anchor: Option<Point>,
    /// Current cursor position in logical pixels.
    current: Point,
    /// Selection staged by a completed drag, waiting for explicit
    /// confirmation. Drawn like the live drag rect but persistent.
    confirmed: Option<aegis_core::Rect>,
    /// Window under the cursor in window-pick mode (topmost first).
    hovered: Option<WindowId>,
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
            mode: PickerMode::Region,
            picker: false,
            anchor: None,
            current: Point::default(),
            confirmed: None,
            hovered: None,
        }
    }

    /// Open the selector, or close it if it is already active. The next frame
    /// will render the overlay and capture input until the user confirms or
    /// cancels.
    pub fn start(&mut self) {
        if self.active {
            self.reset();
        } else {
            self.open(PickerMode::Region, false);
        }
    }

    fn open(&mut self, mode: PickerMode, picker: bool) {
        self.active = true;
        self.mode = mode;
        self.picker = picker;
        self.anchor = None;
        self.confirmed = None;
        self.hovered = None;
    }

    /// Whether the selector is currently open.
    pub fn active(&self) -> bool {
        self.active
    }

    /// Compute the dragged rectangle from anchor and current cursor, clamped
    /// to non-negative size.
    fn drag_rect(&self) -> Option<aegis_core::Rect> {
        let anchor = self.anchor?;
        self.drag_rect_at(anchor, self.current)
    }

    /// Rectangle of a drag from `anchor` to `cursor`, independent of the
    /// component's live drag state.
    fn drag_rect_at(&self, anchor: Point, cursor: Point) -> Option<aegis_core::Rect> {
        let x = anchor.x.min(cursor.x).round() as i32;
        let y = anchor.y.min(cursor.y).round() as i32;
        let w = (anchor.x.max(cursor.x) - x as f32).round() as i32;
        let h = (anchor.y.max(cursor.y) - y as f32).round() as i32;
        Some(aegis_core::Rect::new(x, y, w.max(0), h.max(0)))
    }

    /// The rectangle currently shown: the staged selection once a drag has
    /// completed, otherwise the in-progress drag.
    fn shown_rect(&self) -> Option<aegis_core::Rect> {
        self.confirmed.or_else(|| self.drag_rect())
    }

    /// The topmost window containing `cursor`, or `None` over empty desktop.
    /// `windows` is in z-order with the topmost last (the same order the
    /// renderer draws), so the last containing window wins.
    fn window_at(windows: &[Window], cursor: Point) -> Option<WindowId> {
        windows
            .iter()
            .rev()
            .filter(|window| !window.minimized)
            .find(|window| {
                let left = window.position.x as f32;
                let top = window.position.y as f32;
                cursor.x >= left
                    && cursor.x < left + window.size.w as f32
                    && cursor.y >= top
                    && cursor.y < top + window.size.h as f32
            })
            .map(|window| window.id)
    }

    /// Advance the region-drag state from explicit input edges. Releasing the
    /// pointer never confirms: it stages the rect into `confirmed` and the
    /// overlay stays up. A new press starts a fresh drag, discarding the
    /// staged selection.
    fn update_pointer(&mut self, cursor: Point, pressed: bool, released: bool) {
        self.current = cursor;

        if pressed {
            self.anchor = Some(cursor);
            self.confirmed = None;
            return;
        }
        let Some(anchor) = self.anchor else {
            return;
        };
        if !released {
            return;
        }

        self.anchor = None;
        let dx = (cursor.x - anchor.x).abs();
        let dy = (cursor.y - anchor.y).abs();
        if dx >= MIN_DRAG || dy >= MIN_DRAG {
            self.confirmed = self
                .drag_rect_at(anchor, cursor)
                .filter(|rect| rect.size.w > 0 && rect.size.h > 0);
        }
    }

    /// Emit the staged selection through the chrome events and close. Used by
    /// the confirm keys; pointer confirmation goes through the same path. A
    /// picker session confirmed with no staged selection counts as a cancel,
    /// so the waiting IPC request never hangs on a closed overlay.
    fn confirm(&mut self, out: &mut ChromeEvents) {
        if let Some(rect) = self.confirmed.or_else(|| self.drag_rect())
            && rect.size.w > 0
            && rect.size.h > 0
        {
            out.screenshot_region = Some(rect);
        } else if self.picker {
            out.pick_cancelled = true;
        }
        self.reset();
    }

    fn reset(&mut self) {
        self.active = false;
        self.picker = false;
        self.anchor = None;
        self.confirmed = None;
        self.hovered = None;
    }

    /// Draw the region-mode overlay: selection rect, dimensions label, and
    /// the confirm hint once a selection is staged.
    fn render_region(&self, frame: &mut Frame, display: (f32, f32), i18n: &Localizer) {
        let Some(rect) = self.shown_rect() else {
            return;
        };
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

        // Confirm/cancel hint once a selection is staged.
        if self.confirmed.is_some() {
            let hint = i18n.text(Message::ScreenshotConfirmHint);
            let hint_w = (hint.chars().count() as f32 * 6.5 + 16.0).min(display.0);
            let hint_h = 26.0;
            let hint_x = lens_rect.x.clamp(0.0, display.0 - hint_w);
            let below = lens_rect.y + lens_rect.h + 6.0;
            let hint_y = if below + hint_h <= display.1 {
                below
            } else {
                (lens_rect.y - hint_h - 6.0).max(0.0)
            };
            frame.layer(
                "ass-screenshot-hint",
                LensRect {
                    x: hint_x,
                    y: hint_y,
                    w: hint_w,
                    h: hint_h,
                },
                &OverlayOpts {
                    bg: Color::rgba(12, 14, 22, 210),
                    radius: 4.0,
                    ..Default::default()
                },
                |frame| {
                    frame.column_ex(
                        &LayoutOpts {
                            width: hint_w,
                            height: hint_h,
                            cross: lens::Align::Center,
                            ..Default::default()
                        },
                        |frame| {
                            frame.label_sized(hint, 11.0);
                        },
                    );
                },
            );
        }
    }

    /// Draw the pixel-mode crosshair through the cursor.
    fn render_crosshair(&self, frame: &mut Frame, display: (f32, f32)) {
        let crosshair = Color::rgba(100, 160, 255, 220);
        frame.layer(
            "ass-picker-crosshair-v",
            LensRect {
                x: self.current.x.round() - 0.5,
                y: 0.0,
                w: 1.0,
                h: display.1,
            },
            &OverlayOpts {
                bg: crosshair,
                ..Default::default()
            },
            |_| {},
        );
        frame.layer(
            "ass-picker-crosshair-h",
            LensRect {
                x: 0.0,
                y: self.current.y.round() - 0.5,
                w: display.0,
                h: 1.0,
            },
            &OverlayOpts {
                bg: crosshair,
                ..Default::default()
            },
            |_| {},
        );
    }

    /// Draw the window-mode highlight over the hovered window, with its
    /// title (or a whole-output marker when hovering empty desktop).
    fn render_window_pick(&self, frame: &mut Frame, display: (f32, f32), windows: &[Window]) {
        let window = self
            .hovered
            .and_then(|id| windows.iter().find(|window| window.id == id));
        let (rect, label) = match window {
            Some(window) => (
                LensRect {
                    x: window.position.x as f32,
                    y: window.position.y as f32,
                    w: window.size.w.max(1) as f32,
                    h: window.size.h.max(1) as f32,
                },
                window.title.clone().unwrap_or_default(),
            ),
            // Empty desktop: the click picks the whole output.
            None => (
                LensRect {
                    x: 0.0,
                    y: 0.0,
                    w: display.0,
                    h: display.1,
                },
                String::new(),
            ),
        };
        frame.layer(
            "ass-picker-window",
            rect,
            &OverlayOpts {
                bg: Color::rgba(100, 160, 255, 40),
                border: Color::rgba(100, 160, 255, 220),
                border_width: 1.5,
                ..Default::default()
            },
            |_| {},
        );
        if !label.is_empty() {
            let label_w = (label.chars().count() as f32 * 6.5 + 12.0).min(display.0);
            let label_h = 22.0;
            let label_x = rect.x.clamp(0.0, display.0 - label_w);
            let label_y = (rect.y - label_h - 6.0).clamp(0.0, display.1 - label_h);
            frame.layer(
                "ass-picker-window-label",
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

impl Chrome for ScreenshotSelector {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
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
        match self.mode {
            PickerMode::Region => self.update_pointer(cursor, pressed, released),
            PickerMode::Pixel => {
                self.current = cursor;
                if pressed {
                    out.picked_point = Some(aegis_core::Point {
                        x: cursor.x.round() as i32,
                        y: cursor.y.round() as i32,
                    });
                    self.reset();
                }
            }
            PickerMode::Window => {
                self.current = cursor;
                self.hovered = Self::window_at(windows, cursor);
                if pressed {
                    match self.hovered {
                        Some(id) => out.picked_window = Some(id),
                        // Empty desktop: the user chose the whole output.
                        None => out.pick_output = true,
                    }
                    self.reset();
                }
            }
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

        match self.mode {
            PickerMode::Region => self.render_region(frame, display, i18n),
            PickerMode::Pixel => self.render_crosshair(frame, display),
            PickerMode::Window => self.render_window_pick(frame, display, windows),
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

    fn start_pick(&mut self, mode: PickerMode) {
        self.open(mode, true);
    }

    fn cancel_pick(&mut self) {
        if self.picker {
            self.reset();
        }
    }

    fn key_char(&mut self, key: &KeyChar, out: &mut ChromeEvents) {
        if !self.active {
            return;
        }
        match key_action(key.keysym, key.ch) {
            KeyAction::Escape => {
                if self.picker {
                    out.pick_cancelled = true;
                }
                self.reset();
            }
            KeyAction::Enter | KeyAction::Char(' ') => match self.mode {
                PickerMode::Region => self.confirm(out),
                // Confirm keys choose the whole output in window mode.
                PickerMode::Window => {
                    out.pick_output = true;
                    self.reset();
                }
                PickerMode::Pixel => {}
            },
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_rect_normalizes_negative_sizes() {
        let mut s = ScreenshotSelector::new();
        s.start();
        s.anchor = Some(Point { x: 100.0, y: 100.0 });
        s.current = Point { x: 50.0, y: 60.0 };
        let rect = s.drag_rect().unwrap();
        assert_eq!(rect.origin.x, 50);
        assert_eq!(rect.origin.y, 60);
        assert_eq!(rect.size.w, 50);
        assert_eq!(rect.size.h, 40);
    }

    #[test]
    fn shown_rect_is_none_without_anchor_or_selection() {
        let s = ScreenshotSelector::new();
        assert!(s.shown_rect().is_none());
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
    fn release_stages_selection_without_closing() {
        let mut s = ScreenshotSelector::new();
        s.start();
        s.update_pointer(Point { x: 20.0, y: 30.0 }, true, false);
        s.update_pointer(Point { x: 80.0, y: 90.0 }, false, false);
        s.update_pointer(Point { x: 110.0, y: 70.0 }, false, true);
        assert_eq!(s.confirmed, Some(aegis_core::Rect::new(20, 30, 90, 40)));
        assert!(s.active(), "release must keep the selector open");
        assert!(s.anchor.is_none());
    }

    #[test]
    fn confirm_key_emits_staged_region_and_closes() {
        let mut s = ScreenshotSelector::new();
        s.start();
        s.update_pointer(Point { x: 20.0, y: 30.0 }, true, false);
        s.update_pointer(Point { x: 110.0, y: 70.0 }, false, true);

        let mut out = ChromeEvents::default();
        s.key_char(
            &KeyChar {
                keysym: aegis_core::input::XKB_KEY_Return,
                ch: None,
                mods: aegis_core::input::Mods::NONE,
            },
            &mut out,
        );
        assert_eq!(
            out.screenshot_region,
            Some(aegis_core::Rect::new(20, 30, 90, 40))
        );
        assert!(!s.active());
    }

    #[test]
    fn new_press_replaces_staged_selection() {
        let mut s = ScreenshotSelector::new();
        s.start();
        s.update_pointer(Point { x: 20.0, y: 30.0 }, true, false);
        s.update_pointer(Point { x: 110.0, y: 70.0 }, false, true);
        assert!(s.confirmed.is_some());

        s.update_pointer(Point { x: 200.0, y: 200.0 }, true, false);
        assert!(s.confirmed.is_none());
        assert_eq!(s.drag_rect(), Some(aegis_core::Rect::new(200, 200, 0, 0)));
    }

    #[test]
    fn escape_cancels_staged_selection_without_emitting() {
        let mut s = ScreenshotSelector::new();
        s.start();
        s.update_pointer(Point { x: 20.0, y: 30.0 }, true, false);
        s.update_pointer(Point { x: 110.0, y: 70.0 }, false, true);

        let mut out = ChromeEvents::default();
        s.key_char(
            &KeyChar {
                keysym: aegis_core::input::XKB_KEY_Escape,
                ch: None,
                mods: aegis_core::input::Mods::NONE,
            },
            &mut out,
        );
        assert!(out.screenshot_region.is_none());
        assert!(!out.pick_cancelled, "the Print-key flow stays silent");
        assert!(!s.active());
    }

    #[test]
    fn tiny_release_keeps_selector_waiting() {
        let mut s = ScreenshotSelector::new();
        s.start();
        s.update_pointer(Point { x: 50.0, y: 50.0 }, true, false);
        s.update_pointer(Point { x: 52.0, y: 53.0 }, false, true);
        assert!(s.confirmed.is_none());
        assert!(s.active());
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

    fn window(id: u64, x: i32, y: i32, w: i32, h: i32) -> Window {
        let mut window = Window::new(WindowId(id));
        window.position = aegis_core::Point { x, y };
        window.size = aegis_core::Size { w, h };
        window
    }

    #[test]
    fn window_at_prefers_the_topmost_containing_window() {
        // z-order: the last window is topmost.
        let windows = vec![window(1, 0, 0, 400, 300), window(2, 100, 100, 200, 150)];
        assert_eq!(
            ScreenshotSelector::window_at(&windows, Point { x: 150.0, y: 150.0 }),
            Some(WindowId(2))
        );
        assert_eq!(
            ScreenshotSelector::window_at(&windows, Point { x: 20.0, y: 20.0 }),
            Some(WindowId(1))
        );
        assert_eq!(
            ScreenshotSelector::window_at(&windows, Point { x: 500.0, y: 20.0 }),
            None
        );
    }

    #[test]
    fn picker_escape_emits_pick_cancelled() {
        let mut s = ScreenshotSelector::new();
        s.start_pick(PickerMode::Pixel);
        assert!(s.active());
        let mut out = ChromeEvents::default();
        s.key_char(
            &KeyChar {
                keysym: aegis_core::input::XKB_KEY_Escape,
                ch: None,
                mods: aegis_core::input::Mods::NONE,
            },
            &mut out,
        );
        assert!(out.pick_cancelled);
        assert!(!s.active());
    }

    #[test]
    fn picker_region_confirm_without_selection_cancels() {
        let mut s = ScreenshotSelector::new();
        s.start_pick(PickerMode::Region);
        let mut out = ChromeEvents::default();
        s.key_char(
            &KeyChar {
                keysym: aegis_core::input::XKB_KEY_Return,
                ch: None,
                mods: aegis_core::input::Mods::NONE,
            },
            &mut out,
        );
        assert!(out.screenshot_region.is_none());
        assert!(out.pick_cancelled);
        assert!(!s.active());
    }

    #[test]
    fn cancel_pick_only_closes_picker_sessions() {
        let mut s = ScreenshotSelector::new();
        s.start();
        s.cancel_pick();
        assert!(s.active(), "the Print-key session is not interrupted");
        s.reset();

        s.start_pick(PickerMode::Window);
        s.cancel_pick();
        assert!(!s.active());
    }

    #[test]
    fn window_mode_enter_chooses_the_whole_output() {
        let mut s = ScreenshotSelector::new();
        s.start_pick(PickerMode::Window);
        let mut out = ChromeEvents::default();
        s.key_char(
            &KeyChar {
                keysym: aegis_core::input::XKB_KEY_Return,
                ch: None,
                mods: aegis_core::input::Mods::NONE,
            },
            &mut out,
        );
        assert!(out.pick_output);
        assert!(!s.active());
    }
}
