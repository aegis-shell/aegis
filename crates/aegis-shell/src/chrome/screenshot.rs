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
//! opened through [`ChromeCommand::StartPick`] with a [`PickerMode`]:
//!
//! - `Region` is the Print-key interaction, but the confirmed rect goes to
//!   the waiting IPC request instead of the screenshot file path.
//! - `Pixel` draws a compact optical loupe; a click emits the picked point
//!   through [`ChromeEvents::picked_point`].
//! - `Window` highlights the window under the cursor; a click emits its id
//!   through [`ChromeEvents::picked_window`], a click on empty desktop (or
//!   Enter/Space) chooses the whole output through
//!   [`ChromeEvents::pick_output`].
//! - `Output` highlights the output under the cursor; a click (or
//!   Enter/Space) emits its connector through
//!   [`ChromeEvents::picked_output`] (version 29, ADR-0128).
//!
//! Escape always cancels a picker session through
//! [`ChromeEvents::pick_cancelled`]; the compositor closes the loop by
//! answering the IPC request.

use aegis_design::{Design, GlassRole, materials};
use lens::{Align, Color, Frame, Input, LayoutOpts, Rect as LensRect, Style};

use crate::{
    BackdropRegion, Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape,
    LiquidGlassRegion, Localizer, Message, ellipsize,
};
use aegis_model::app::BuiltInApplication;
use aegis_model::input::{KeyAction, KeyChar, key_action};
use aegis_model::window::{Window, WindowId};
use aegis_model::workspace::WorkspaceSnapshot;

/// Minimum drag distance in logical pixels before a release is treated as a
/// real selection rather than an accidental tap.
const MIN_DRAG: f32 = 8.0;

/// Shared optical character of screenshot-picker glass bodies. The selector
/// is static, so it uses the same full-strength blur as the window switcher
/// without introducing another component-local material.
const BACKDROP_BLUR_SIGMA: f32 = 16.0;
const PIXEL_LENS_SIZE: f32 = 48.0;
const STATUS_PAD: f32 = 8.0;
const STATUS_MARGIN: f32 = 12.0;
/// Sub-logical-pixel bands keep the inverse rounded-corner mask smooth on
/// both 1× and HiDPI outputs without creating extra floating Lens layers.
const SCRIM_CORNER_BAND_HEIGHT: f32 = 0.5;

/// The interaction a portal picker session asks for (ADR-0054).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerMode {
    /// Drag out a screen region (the Print-key interaction).
    Region,
    /// Click one screen point (colour picking).
    Pixel,
    /// Click a window, or choose the whole output.
    Window,
    /// Click an output to pick it by connector (version 29, ADR-0128).
    Output,
}

#[derive(Debug, Clone, Copy, Default)]
struct Point {
    x: f32,
    y: f32,
}

#[derive(Debug, Clone, Copy)]
struct CornerBand {
    height: f32,
    inset: f32,
}

/// The screenshot region selector / portal picker chrome component.
pub struct ScreenshotSelector {
    active: bool,
    /// Current interaction; `Region` outside picker sessions.
    mode: PickerMode,
    /// Whether the session was opened for an IPC pick (ADR-0054) rather than
    /// the Print key. Picker sessions emit the pick events and are the only
    /// ones [`ChromeCommand::CancelPick`] may interrupt.
    picker: bool,
    /// Press origin in logical pixels while a drag is in progress.
    anchor: Option<Point>,
    /// Current cursor position in logical pixels.
    current: Point,
    /// Selection staged by a completed drag, waiting for explicit
    /// confirmation. Drawn like the live drag rect but persistent.
    confirmed: Option<aegis_model::Rect>,
    /// Window under the cursor in window-pick mode (topmost first).
    hovered: Option<WindowId>,
    /// Output under the cursor in output-pick mode (its connector), hit-tested
    /// against the pushed output snapshot.
    hovered_output: Option<String>,
    /// The outputs the output-pick mode hit-tests and highlights: the live
    /// connectors and their desktop logical rects, mirrored from the pushed
    /// [`crate::SystemStatus`].
    outputs: Vec<aegis_model::output::OutputInfo>,
    /// The design snapshot the selector paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`crate::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
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
            hovered_output: None,
            outputs: Vec::new(),
            design: Design::dark(),
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
        self.hovered_output = None;
    }

    /// Whether the selector is currently open.
    pub fn active(&self) -> bool {
        self.active
    }

    /// Compute the dragged rectangle from anchor and current cursor, clamped
    /// to non-negative size.
    fn drag_rect(&self) -> Option<aegis_model::Rect> {
        let anchor = self.anchor?;
        self.drag_rect_at(anchor, self.current)
    }

    /// Rectangle of a drag from `anchor` to `cursor`, independent of the
    /// component's live drag state.
    fn drag_rect_at(&self, anchor: Point, cursor: Point) -> Option<aegis_model::Rect> {
        let x = anchor.x.min(cursor.x).round() as i32;
        let y = anchor.y.min(cursor.y).round() as i32;
        let w = (anchor.x.max(cursor.x) - x as f32).round() as i32;
        let h = (anchor.y.max(cursor.y) - y as f32).round() as i32;
        Some(aegis_model::Rect::new(x, y, w.max(0), h.max(0)))
    }

    /// The rectangle currently shown: the staged selection once a drag has
    /// completed, otherwise the in-progress drag.
    fn shown_rect(&self) -> Option<aegis_model::Rect> {
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

    /// The connector of the output whose desktop logical rect contains
    /// `cursor`, or `None` outside every output. The same coordinate space
    /// [`Self::window_at`] hit-tests in: compositor logical pixels.
    fn output_at(outputs: &[aegis_model::output::OutputInfo], cursor: Point) -> Option<String> {
        outputs
            .iter()
            .find(|output| {
                let rect = output.geometry.logical_rect();
                let left = rect.origin.x as f32;
                let top = rect.origin.y as f32;
                cursor.x >= left
                    && cursor.x < left + rect.size.w as f32
                    && cursor.y >= top
                    && cursor.y < top + rect.size.h as f32
            })
            .map(|output| output.connector.clone())
    }

    /// Confirm the hovered output in output-pick mode: its connector goes to
    /// the waiting IPC request. With no output under the cursor (a layout
    /// transition can leave a gap) the pick degrades to the legacy
    /// whole-output answer instead of hanging.
    fn confirm_output(&mut self, out: &mut ChromeEvents) {
        match self.hovered_output.clone() {
            Some(connector) => out.picked_output = Some(connector),
            None => out.pick_output = true,
        }
        self.reset();
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
        self.hovered_output = None;
    }

    /// Current Liquid Glass body. Region and window selection use the target
    /// itself; pixel picking uses a compact lens centred on the pointer.
    fn glass_rect(&self, display: (f32, f32), windows: &[Window]) -> Option<LensRect> {
        match self.mode {
            PickerMode::Region => self
                .shown_rect()
                .and_then(|rect| (rect.size.w > 0 && rect.size.h > 0).then_some(to_lens(rect))),
            PickerMode::Pixel => {
                let w = PIXEL_LENS_SIZE.min(display.0.max(1.0));
                let h = PIXEL_LENS_SIZE.min(display.1.max(1.0));
                Some(LensRect {
                    x: (self.current.x - w * 0.5).clamp(0.0, (display.0 - w).max(0.0)),
                    y: (self.current.y - h * 0.5).clamp(0.0, (display.1 - h).max(0.0)),
                    w,
                    h,
                })
            }
            PickerMode::Window => self
                .hovered
                .and_then(|id| windows.iter().find(|window| window.id == id))
                .map(|window| LensRect {
                    x: window.position.x as f32,
                    y: window.position.y as f32,
                    w: window.size.w.max(1) as f32,
                    h: window.size.h.max(1) as f32,
                }),
            PickerMode::Output => self
                .hovered_output
                .as_ref()
                .and_then(|connector| {
                    self.outputs
                        .iter()
                        .find(|output| &output.connector == connector)
                })
                .map(|output| {
                    let rect = output.geometry.logical_rect();
                    LensRect {
                        x: rect.origin.x as f32,
                        y: rect.origin.y as f32,
                        w: rect.size.w.max(1) as f32,
                        h: rect.size.h.max(1) as f32,
                    }
                }),
        }
    }

    fn glass_radius(&self, rect: LensRect) -> f32 {
        self.design
            .radii
            .glass_panel
            .min(rect.w * 0.5)
            .min(rect.h * 0.5)
    }

    /// Dim only outside the active optical body. A full-screen translucent
    /// placement would dim the already-composited Liquid Glass along with the
    /// desktop and collapse it back into a classic flat selection rectangle.
    fn render_scrim(&self, frame: &mut Frame, display: (f32, f32), hole: Option<LensRect>) {
        let scrim = LayoutOpts {
            bg: self.design.colors.scrim.with_alpha(128),
            ..materials::surface_layout()
        };
        let full = LensRect {
            x: 0.0,
            y: 0.0,
            w: display.0,
            h: display.1,
        };
        let regions = scrim_regions(full, hole);
        for (index, rect) in regions.into_iter().enumerate() {
            if rect.w <= 0.0 || rect.h <= 0.0 {
                continue;
            }
            frame.place(
                &format!("aegis-screenshot-scrim-{index}"),
                &materials::chrome_place(rect, scrim),
                |_| {},
            );
        }
        if let Some(hole) = hole {
            render_rounded_scrim_corners(frame, hole, self.glass_radius(hole), scrim.bg);
        }
    }

    fn render_status_pill(
        frame: &mut Frame,
        id: &str,
        text: &str,
        anchor: LensRect,
        display: (f32, f32),
        selected: bool,
        design: &Design,
    ) {
        let max_text_width = (display.0 - 2.0 * (STATUS_MARGIN + STATUS_PAD)).max(1.0);
        let text = ellipsize(frame, text, design.typography.label, max_text_width);
        let metrics = frame.measure_text(&text, design.typography.label);
        let size = (
            metrics.width + STATUS_PAD * 2.0,
            metrics.height + STATUS_PAD * 2.0,
        );
        let rect = status_rect(anchor, size, display);
        let mut material = materials::glass_focus(design, selected);
        material.radius = rect.h * 0.5;

        let original_theme = frame.theme();
        frame.set_theme(original_theme.with_fg(design.hud_foreground.primary));
        frame.place(id, &materials::chrome_place(rect, material), |frame| {
            // A row's cross axis is vertical. With an explicitly measured body
            // and equal padding on both sides, the compact glyph box is centred
            // on both axes instead of inheriting the regular label's theme pad.
            frame.row_ex(
                &LayoutOpts {
                    width: rect.w,
                    height: rect.h,
                    pad: STATUS_PAD,
                    cross: Align::Center,
                    ..Default::default()
                },
                |frame| {
                    frame.push_style(
                        Style::new()
                            .with_outline_color(design.hud_foreground.contour)
                            .with_outline_width(design.hud_foreground.text_contour_width),
                    );
                    frame.label_compact_sized(&text, design.typography.label);
                    frame.pop_style();
                },
            );
        });
        frame.set_theme(original_theme);
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

        // Minimal foreground tint only. The compositor-owned analytic pass
        // supplies the body, refraction, rim light, and shadow.
        let design = &self.design;
        let mut material = materials::glass_panel(design);
        material.radius = self.glass_radius(lens_rect);
        frame.place(
            "aegis-screenshot-selection",
            &materials::chrome_place(lens_rect, material),
            |_| {},
        );

        // One measured status pill is content inside the glass body, not a
        // second outlined material. It expands to include the confirmation
        // hint after the drag is staged.
        if rect.size.w > 0 && rect.size.h > 0 {
            let dimensions = format!("{} × {}", rect.size.w, rect.size.h);
            let status = if self.confirmed.is_some() {
                format!(
                    "{dimensions}  —  {}",
                    i18n.text(Message::ScreenshotConfirmHint)
                )
            } else {
                dimensions
            };
            Self::render_status_pill(
                frame,
                "aegis-screenshot-status",
                &status,
                lens_rect,
                display,
                self.confirmed.is_some(),
                design,
            );
        }
    }

    /// Draw a compact optical loupe instead of classic full-screen crosshair
    /// rules. The analytic body is supplied through `liquid_glass_regions`.
    fn render_pixel_lens(&self, frame: &mut Frame, display: (f32, f32)) {
        let Some(rect) = self.glass_rect(display, &[]) else {
            return;
        };
        let design = &self.design;
        let mut material = materials::glass_panel(design);
        material.radius = self.glass_radius(rect);
        frame.place(
            "aegis-picker-pixel-lens",
            &materials::chrome_place(rect, material),
            |_| {},
        );
        frame.place(
            "aegis-picker-pixel-centre",
            &materials::chrome_place(
                LensRect {
                    x: self.current.x.round() - 2.0,
                    y: self.current.y.round() - 2.0,
                    w: 4.0,
                    h: 4.0,
                },
                LayoutOpts {
                    bg: design.hud_foreground.primary,
                    radius: 2.0,
                    ..materials::surface_layout()
                },
            ),
            |_| {},
        );
    }

    /// Draw the Liquid Glass highlight and title for the hovered window.
    /// Empty desktop remains the implicit whole-output target.
    fn render_window_pick(&self, frame: &mut Frame, display: (f32, f32), windows: &[Window]) {
        let Some(window) = self
            .hovered
            .and_then(|id| windows.iter().find(|window| window.id == id))
        else {
            // Empty desktop remains the whole-output target, represented by
            // the unobstructed frozen canvas instead of a fake full-screen
            // glass body.
            return;
        };
        let rect = LensRect {
            x: window.position.x as f32,
            y: window.position.y as f32,
            w: window.size.w.max(1) as f32,
            h: window.size.h.max(1) as f32,
        };
        let label = window.title.clone().unwrap_or_default();
        frame.place(
            "aegis-picker-window",
            &materials::chrome_place(rect, {
                let mut material = materials::glass_panel(&self.design);
                material.radius = self.glass_radius(rect);
                material
            }),
            |_| {},
        );
        if !label.is_empty() {
            Self::render_status_pill(
                frame,
                "aegis-picker-window-label",
                &label,
                rect,
                display,
                true,
                &self.design,
            );
        }
    }

    /// Draw the Liquid Glass highlight and connector label for the hovered
    /// output. With no output under the cursor the frozen canvas itself is
    /// the implicit whole-desktop target, exactly like window mode's empty
    /// desktop.
    fn render_output_pick(&self, frame: &mut Frame, display: (f32, f32)) {
        let Some(output) = self.hovered_output.as_ref().and_then(|connector| {
            self.outputs
                .iter()
                .find(|output| &output.connector == connector)
        }) else {
            return;
        };
        let logical = output.geometry.logical_rect();
        let rect = LensRect {
            x: logical.origin.x as f32,
            y: logical.origin.y as f32,
            w: logical.size.w.max(1) as f32,
            h: logical.size.h.max(1) as f32,
        };
        frame.place(
            "aegis-picker-output",
            &materials::chrome_place(rect, {
                let mut material = materials::glass_panel(&self.design);
                material.radius = self.glass_radius(rect);
                material
            }),
            |_| {},
        );
        Self::render_status_pill(
            frame,
            "aegis-picker-output-label",
            &output.connector,
            rect,
            display,
            true,
            &self.design,
        );
    }

    fn start_pick(&mut self, mode: PickerMode) {
        self.open(mode, true);
    }

    fn cancel_pick(&mut self) {
        if self.picker {
            self.reset();
        }
    }
}

fn to_lens(rect: aegis_model::Rect) -> LensRect {
    LensRect {
        x: rect.origin.x as f32,
        y: rect.origin.y as f32,
        w: rect.size.w.max(1) as f32,
        h: rect.size.h.max(1) as f32,
    }
}

fn to_backdrop(rect: LensRect) -> BackdropRegion {
    BackdropRegion {
        x: rect.x,
        y: rect.y,
        w: rect.w,
        h: rect.h,
    }
}

fn status_rect(anchor: LensRect, size: (f32, f32), display: (f32, f32)) -> LensRect {
    let w = size.0.min(display.0.max(1.0));
    let h = size.1.min(display.1.max(1.0));
    let inside = anchor.w >= w + STATUS_MARGIN * 2.0 && anchor.h >= h + STATUS_MARGIN * 2.0;
    let x = if inside {
        anchor.x + STATUS_MARGIN
    } else {
        anchor.x + (anchor.w - w) * 0.5
    }
    .clamp(0.0, (display.0 - w).max(0.0));
    let y = if inside {
        anchor.y + STATUS_MARGIN
    } else if anchor.y + anchor.h + STATUS_MARGIN + h <= display.1 {
        anchor.y + anchor.h + STATUS_MARGIN
    } else {
        anchor.y - STATUS_MARGIN - h
    }
    .clamp(0.0, (display.1 - h).max(0.0));
    LensRect { x, y, w, h }
}

fn scrim_regions(full: LensRect, hole: Option<LensRect>) -> [LensRect; 4] {
    let Some(hole) = hole else {
        let empty = LensRect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        };
        return [full, empty, empty, empty];
    };
    let left = hole.x.clamp(full.x, full.x + full.w);
    let top = hole.y.clamp(full.y, full.y + full.h);
    let right = (hole.x + hole.w).clamp(left, full.x + full.w);
    let bottom = (hole.y + hole.h).clamp(top, full.y + full.h);
    [
        LensRect {
            x: full.x,
            y: full.y,
            w: full.w,
            h: top - full.y,
        },
        LensRect {
            x: full.x,
            y: bottom,
            w: full.w,
            h: full.y + full.h - bottom,
        },
        LensRect {
            x: full.x,
            y: top,
            w: left - full.x,
            h: bottom - top,
        },
        LensRect {
            x: right,
            y: top,
            w: full.x + full.w - right,
            h: bottom - top,
        },
    ]
}

/// Horizontal samples of one top rounded corner's inverse mask. Mirroring the
/// bands across both axes fills the four areas outside the rounded glass body
/// but inside its rectangular bounds. Sampling at each band's centre matches
/// the raster coverage point and avoids a bright seam or dark overlap.
fn rounded_corner_bands(radius: f32) -> Vec<CornerBand> {
    let radius = radius.max(0.0);
    let mut bands = Vec::new();
    let mut y = 0.0;
    while y < radius {
        let height = SCRIM_CORNER_BAND_HEIGHT.min(radius - y);
        let sample_y = y + height * 0.5;
        let dy = radius - sample_y;
        let inset = radius - (radius * radius - dy * dy).max(0.0).sqrt();
        bands.push(CornerBand { height, inset });
        y += height;
    }
    bands
}

fn render_rounded_scrim_corners(frame: &mut Frame, hole: LensRect, radius: f32, color: Color) {
    let bands = rounded_corner_bands(radius);
    if bands.is_empty() {
        return;
    }
    let layer = LayoutOpts {
        gap: 0.0,
        pad: 0.0,
        cross: Align::Stretch,
        ..materials::surface_layout()
    };
    frame.place(
        "aegis-screenshot-scrim-rounded-corners",
        &materials::chrome_place(hole, layer),
        |frame| {
            frame.column_ex(
                &LayoutOpts {
                    width: hole.w,
                    height: hole.h,
                    gap: 0.0,
                    pad: 0.0,
                    cross: Align::Stretch,
                    ..Default::default()
                },
                |frame| {
                    for band in &bands {
                        render_scrim_corner_band(frame, hole.w, *band, color);
                    }
                    let middle = (hole.h - radius * 2.0).max(0.0);
                    if middle > 0.0 {
                        frame.spacer(middle);
                    }
                    for band in bands.iter().rev() {
                        render_scrim_corner_band(frame, hole.w, *band, color);
                    }
                },
            );
        },
    );
}

fn render_scrim_corner_band(frame: &mut Frame, width: f32, band: CornerBand, color: Color) {
    frame.row_ex(
        &LayoutOpts {
            width,
            height: band.height,
            gap: 0.0,
            pad: 0.0,
            cross: Align::Stretch,
            ..Default::default()
        },
        |frame| {
            let inset = band.inset.min(width * 0.5).max(0.0);
            if inset <= f32::EPSILON {
                frame.spacer(width);
                return;
            }
            let fill = LayoutOpts {
                width: inset,
                height: band.height,
                bg: color,
                ..Default::default()
            };
            frame.column_ex(&fill, |_| {});
            frame.spacer((width - inset * 2.0).max(0.0));
            frame.column_ex(&fill, |_| {});
        },
    );
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
        match self.mode {
            // Region state is prepared before the compositor queries the
            // matching glass geometry, so foreground and optics share one rect.
            PickerMode::Region => {}
            PickerMode::Pixel => {
                self.current = cursor;
                if pressed {
                    out.picked_point = Some(aegis_model::Point {
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
            PickerMode::Output => {
                self.current = cursor;
                self.hovered_output = Self::output_at(&self.outputs, cursor);
                if pressed {
                    self.confirm_output(out);
                }
            }
        }
        if !self.active {
            return;
        }

        let glass_rect = self.glass_rect(display, windows);
        self.render_scrim(frame, display, glass_rect);

        match self.mode {
            PickerMode::Region => self.render_region(frame, display, i18n),
            PickerMode::Pixel => self.render_pixel_lens(frame, display),
            PickerMode::Window => self.render_window_pick(frame, display, windows),
            PickerMode::Output => self.render_output_pick(frame, display),
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

    fn requires_composition(&self) -> bool {
        self.active
    }

    fn prepare_backdrop(
        &mut self,
        input: &Input,
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) {
        if !self.active {
            return;
        }
        let raw = input.as_raw();
        let cursor = Point {
            x: raw.cursor.x,
            y: raw.cursor.y,
        };
        match self.mode {
            PickerMode::Region => self.update_pointer(
                cursor,
                raw.mouse_pressed.first().copied().unwrap_or(false),
                raw.mouse_released.first().copied().unwrap_or(false),
            ),
            PickerMode::Pixel => self.current = cursor,
            PickerMode::Window => {
                self.current = cursor;
                self.hovered = Self::window_at(windows, cursor);
            }
            PickerMode::Output => {
                self.current = cursor;
                self.hovered_output = Self::output_at(&self.outputs, cursor);
            }
        }
    }

    fn backdrop_blur_sigma(&self) -> f32 {
        if self.active
            && match self.mode {
                PickerMode::Region => self
                    .shown_rect()
                    .is_some_and(|rect| rect.size.w > 0 && rect.size.h > 0),
                PickerMode::Pixel => true,
                PickerMode::Window => self.hovered.is_some(),
                PickerMode::Output => self.hovered_output.is_some(),
            }
        {
            BACKDROP_BLUR_SIGMA
        } else {
            0.0
        }
    }

    fn backdrop_regions(
        &self,
        display: (f32, f32),
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        self.glass_rect(display, windows)
            .map(|rect| vec![to_backdrop(rect)])
            .unwrap_or_default()
    }

    fn liquid_glass_regions(
        &self,
        display: (f32, f32),
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<LiquidGlassRegion> {
        self.glass_rect(display, windows)
            .map(|rect| {
                vec![LiquidGlassRegion::from_role(
                    &self.design,
                    GlassRole::FloatingPanel,
                    BackdropRegion::from(rect),
                    self.glass_radius(rect),
                    1.0,
                )]
            })
            .unwrap_or_default()
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn screenshot_active(&self) -> bool {
        self.active
    }

    fn cursor_shape_at(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        self.active.then_some(CursorShape::Default)
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::Appearance(design) => self.design = *design,
            ChromeUpdate::SystemStatus(status) => {
                self.outputs = status.display.outputs.clone();
            }
            _ => {}
        }
    }

    fn command(&mut self, command: &ChromeCommand<'_>, _out: &mut ChromeEvents) {
        match command {
            ChromeCommand::OpenBuiltIn(BuiltInApplication::ScreenshotSelector) => self.start(),
            ChromeCommand::StartPick(mode) => self.start_pick(*mode),
            ChromeCommand::CancelPick => self.cancel_pick(),
            _ => {}
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
                // Confirm keys pick the hovered output in output mode.
                PickerMode::Output => self.confirm_output(out),
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
        assert_eq!(s.confirmed, Some(aegis_model::Rect::new(20, 30, 90, 40)));
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
                keysym: aegis_model::input::XKB_KEY_Return,
                ch: None,
                mods: aegis_model::input::Mods::NONE,
            },
            &mut out,
        );
        assert_eq!(
            out.screenshot_region,
            Some(aegis_model::Rect::new(20, 30, 90, 40))
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
        assert_eq!(s.drag_rect(), Some(aegis_model::Rect::new(200, 200, 0, 0)));
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
                keysym: aegis_model::input::XKB_KEY_Escape,
                ch: None,
                mods: aegis_model::input::Mods::NONE,
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
    fn selector_captures_input_with_a_visible_live_cursor() {
        let mut s = ScreenshotSelector::new();
        s.start();
        let workspaces = WorkspaceSnapshot {
            outputs: Vec::new(),
        };
        assert!(s.captures_keyboard());
        assert!(s.captures_pointer(0.0, 0.0, (100.0, 100.0), &[], &workspaces));
        assert_eq!(
            s.cursor_shape_at(0.0, 0.0, (100.0, 100.0), &[], &workspaces),
            Some(CursorShape::Default)
        );
    }

    #[test]
    fn active_region_is_one_borderless_liquid_glass_body() {
        let mut s = ScreenshotSelector::new();
        s.start();
        s.update_pointer(Point { x: 20.0, y: 30.0 }, true, false);
        s.update_pointer(Point { x: 220.0, y: 150.0 }, false, true);
        let workspaces = WorkspaceSnapshot {
            outputs: Vec::new(),
        };

        assert_eq!(s.backdrop_blur_sigma(), BACKDROP_BLUR_SIGMA);
        let backdrop = s.backdrop_regions((800.0, 600.0), &[], &workspaces);
        let glass = s.liquid_glass_regions((800.0, 600.0), &[], &workspaces);
        assert_eq!(backdrop.len(), 1);
        assert_eq!(glass.len(), 1);
        assert_eq!(glass[0].bounds, backdrop[0]);
        assert_eq!(glass[0].corner_radius, Design::dark().radii.glass_panel);
        assert!(glass[0].focus.is_none());
    }

    #[test]
    fn status_geometry_stays_inside_a_roomy_selection_and_centres_when_outside() {
        let roomy = LensRect {
            x: 100.0,
            y: 80.0,
            w: 500.0,
            h: 300.0,
        };
        assert_eq!(
            status_rect(roomy, (180.0, 32.0), (800.0, 600.0)),
            LensRect {
                x: 112.0,
                y: 92.0,
                w: 180.0,
                h: 32.0,
            }
        );

        let narrow = LensRect {
            x: 100.0,
            y: 80.0,
            w: 80.0,
            h: 40.0,
        };
        assert_eq!(
            status_rect(narrow, (180.0, 32.0), (800.0, 600.0)),
            LensRect {
                x: 50.0,
                y: 132.0,
                w: 180.0,
                h: 32.0,
            }
        );
    }

    #[test]
    fn scrim_regions_leave_the_selection_body_undimmed() {
        let full = LensRect {
            x: 0.0,
            y: 0.0,
            w: 800.0,
            h: 600.0,
        };
        let hole = LensRect {
            x: 100.0,
            y: 80.0,
            w: 200.0,
            h: 120.0,
        };
        let regions = scrim_regions(full, Some(hole));
        let area: f32 = regions.iter().map(|rect| rect.w * rect.h).sum();
        assert_eq!(area, full.w * full.h - hole.w * hole.h);
        assert!(regions.iter().all(|rect| {
            rect.x + rect.w <= hole.x
                || rect.x >= hole.x + hole.w
                || rect.y + rect.h <= hole.y
                || rect.y >= hole.y + hole.h
        }));
    }

    #[test]
    fn rounded_scrim_corner_bands_fill_only_the_glass_corner_cutouts() {
        let radius = Design::dark().radii.glass_panel;
        let bands = rounded_corner_bands(radius);
        assert!(!bands.is_empty());
        assert!(bands.windows(2).all(|pair| pair[0].inset > pair[1].inset));
        assert!((bands.iter().map(|band| band.height).sum::<f32>() - radius).abs() < 0.001);

        // Four mirrored corners should approximate the exact area between a
        // square-cornered rectangle and the matching rounded rectangle.
        let sampled_area = bands
            .iter()
            .map(|band| band.inset * band.height * 4.0)
            .sum::<f32>();
        let analytic_area = (4.0 - std::f32::consts::PI) * radius * radius;
        assert!((sampled_area - analytic_area).abs() < 1.0);
    }

    fn window(id: u64, x: i32, y: i32, w: i32, h: i32) -> Window {
        let mut window = Window::new(WindowId(id));
        window.position = aegis_model::Point { x, y };
        window.size = aegis_model::Size { w, h };
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
                keysym: aegis_model::input::XKB_KEY_Escape,
                ch: None,
                mods: aegis_model::input::Mods::NONE,
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
                keysym: aegis_model::input::XKB_KEY_Return,
                ch: None,
                mods: aegis_model::input::Mods::NONE,
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
                keysym: aegis_model::input::XKB_KEY_Return,
                ch: None,
                mods: aegis_model::input::Mods::NONE,
            },
            &mut out,
        );
        assert!(out.pick_output);
        assert!(!s.active());
    }

    fn test_output(
        connector: &str,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
    ) -> aegis_model::output::OutputInfo {
        aegis_model::output::OutputInfo {
            connector: connector.into(),
            geometry: aegis_model::output::OutputGeometry {
                mode: aegis_model::output::OutputMode {
                    width: w,
                    height: h,
                    refresh_mhz: 60_000,
                },
                scale: aegis_model::output::Scale(1.0),
                transform: aegis_model::Transform::Normal,
                logical_origin: aegis_model::Point { x, y },
            },
            available_modes: Vec::new(),
            color_caps: aegis_model::edid::EdidColorCapabilities::default(),
        }
    }

    #[test]
    fn output_mode_hit_tests_against_the_output_rects() {
        let outputs = vec![
            test_output("HDMI-A-1", 0, 0, 1920, 1080),
            test_output("DP-1", 1920, 0, 2560, 1440),
        ];
        assert_eq!(
            ScreenshotSelector::output_at(&outputs, Point { x: 20.0, y: 20.0 }),
            Some("HDMI-A-1".to_owned())
        );
        assert_eq!(
            ScreenshotSelector::output_at(&outputs, Point { x: 2000.0, y: 20.0 }),
            Some("DP-1".to_owned())
        );
        assert_eq!(
            ScreenshotSelector::output_at(&outputs, Point { x: 5000.0, y: 20.0 }),
            None
        );
        // Edge: the right/bottom edge is exclusive, like window hit-testing.
        assert_eq!(
            ScreenshotSelector::output_at(&outputs, Point { x: 1920.0, y: 20.0 }),
            Some("DP-1".to_owned())
        );
    }

    #[test]
    fn output_mode_enter_emits_the_hovered_connector() {
        let mut s = ScreenshotSelector::new();
        s.start_pick(PickerMode::Output);
        s.outputs = vec![
            test_output("HDMI-A-1", 0, 0, 1920, 1080),
            test_output("DP-1", 1920, 0, 2560, 1440),
        ];
        s.hovered_output = Some("DP-1".to_owned());
        let mut out = ChromeEvents::default();
        s.key_char(
            &KeyChar {
                keysym: aegis_model::input::XKB_KEY_Return,
                ch: None,
                mods: aegis_model::input::Mods::NONE,
            },
            &mut out,
        );
        assert_eq!(out.picked_output.as_deref(), Some("DP-1"));
        assert!(!out.pick_output);
        assert!(!s.active());
    }

    #[test]
    fn output_mode_enter_without_a_hover_falls_back_to_bare_output() {
        let mut s = ScreenshotSelector::new();
        s.start_pick(PickerMode::Output);
        let mut out = ChromeEvents::default();
        s.key_char(
            &KeyChar {
                keysym: aegis_model::input::XKB_KEY_Return,
                ch: None,
                mods: aegis_model::input::Mods::NONE,
            },
            &mut out,
        );
        assert!(out.pick_output);
        assert!(out.picked_output.is_none());
        assert!(!s.active());
    }

    #[test]
    fn output_mode_escape_cancels() {
        let mut s = ScreenshotSelector::new();
        s.start_pick(PickerMode::Output);
        let mut out = ChromeEvents::default();
        s.key_char(
            &KeyChar {
                keysym: aegis_model::input::XKB_KEY_Escape,
                ch: None,
                mods: aegis_model::input::Mods::NONE,
            },
            &mut out,
        );
        assert!(out.pick_cancelled);
        assert!(out.picked_output.is_none());
        assert!(!s.active());
    }

    #[test]
    fn output_mode_status_updates_refresh_the_hit_test_snapshot() {
        let mut s = ScreenshotSelector::new();
        let status = crate::SystemStatus {
            display: aegis_model::settings::DisplayStatus {
                outputs: vec![test_output("HDMI-A-1", 0, 0, 1920, 1080)],
                ..aegis_model::settings::DisplayStatus::default()
            },
            ..crate::SystemStatus::default()
        };
        s.update(ChromeUpdate::SystemStatus(&status));
        assert_eq!(s.outputs.len(), 1);
        assert_eq!(s.outputs[0].connector, "HDMI-A-1");
    }
}
