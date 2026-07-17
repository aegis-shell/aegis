//! The overview chrome (M9): a modal window/workspace picker in the GNOME
//! Activities mold. The compositor's main loop draws the live window
//! thumbnails onto the grid computed by the shared `ass_core::overview`
//! geometry; this component draws the cell frames, labels, and the workspace
//! rail, and owns interaction: hover, click-to-focus, workspace switching,
//! and dismissal. It is a view mode over the same snapshots every other
//! component reads — it never mutates the window model itself.

use lens::{Align, Color, Frame, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{Chrome, ChromeEvents, CursorShape, Localizer};
use ass_core::input::{key_action, KeyAction, KeyChar};
use ass_core::overview as geom;
use ass_core::window::Window;
use ass_core::workspace::WorkspaceSnapshot;

/// Reveal/fade speed (per second, exponential approach).
const FADE_RATE: f32 = 14.0;
/// Label strip height under a thumbnail, in logical pixels.
const LABEL_H: i32 = 22;

/// The overview chrome component.
pub struct Overview {
    open: bool,
    /// Reveal fade: 0 = hidden, 1 = fully open.
    visibility: f32,
    anim_active: bool,
    /// Grid cell index under the cursor this frame.
    hovered: Option<usize>,
    /// Workspace rail tile index under the cursor this frame.
    rail_hovered: Option<usize>,
    /// Left button level last frame, so a click fires once on the press edge.
    prev_down: bool,
    reduced_motion: bool,
}

impl Default for Overview {
    fn default() -> Overview {
        Overview::new()
    }
}

impl Overview {
    pub fn new() -> Overview {
        Overview {
            open: false,
            visibility: 0.0,
            anim_active: false,
            hovered: None,
            rail_hovered: None,
            prev_down: false,
            reduced_motion: false,
        }
    }

    fn advance(&mut self, dt: f32) {
        let target = if self.open { 1.0 } else { 0.0 };
        if self.reduced_motion {
            self.visibility = target;
            self.anim_active = false;
            return;
        }
        let k = (dt * FADE_RATE).min(1.0);
        self.visibility += (target - self.visibility) * k;
        self.anim_active = (self.visibility - target).abs() > 0.002;
        if !self.anim_active {
            self.visibility = target;
        }
    }

    fn alpha(&self, base: u8) -> u8 {
        (base as f32 * self.visibility).round() as u8
    }
}

impl Chrome for Overview {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        windows: &[Window],
        workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let raw = input.as_raw();
        let display =
            ass_core::Rect::new(0, 0, raw.display_size.x as i32, raw.display_size.y as i32);
        let cursor = raw.cursor;
        let down = raw.mouse_down.first().copied().unwrap_or(false);
        let pressed = down && !self.prev_down;

        self.advance(raw.dt_seconds.max(0.0));
        self.prev_down = down;
        if self.visibility <= 0.001 && !self.open {
            self.hovered = None;
            self.rail_hovered = None;
            return;
        }

        // The rail appears whenever the focused output owns more than one
        // workspace — the same condition the thumbnail pass uses.
        let rail_tiles: Vec<(ass_core::workspace::WorkspaceId, bool)> = workspaces
            .outputs
            .first()
            .map(|o| {
                o.workspaces
                    .iter()
                    .map(|w| (w.id, Some(w.id) == o.current))
                    .collect()
            })
            .unwrap_or_default();
        let has_rail = rail_tiles.len() > 1;
        let area = geom::grid_area(display, has_rail);
        let slots = geom::grid(area, windows.len());
        let tiles = geom::rail(display, rail_tiles.len());

        // Hover + pick resolution over the exact cells the thumbnails use.
        self.hovered = None;
        self.rail_hovered = None;
        for (i, (slot, window)) in slots.iter().zip(windows.iter()).enumerate() {
            let cell = geom::fit(*slot, window.size);
            if contains_rect(cell, cursor.x, cursor.y) {
                self.hovered = Some(i);
                if pressed {
                    out.overview_pick = Some(window.id);
                    self.open = false;
                    return;
                }
            }
        }
        for (i, tile) in tiles.iter().enumerate() {
            if contains_rect(*tile, cursor.x, cursor.y) {
                self.rail_hovered = Some(i);
                if pressed {
                    out.overview_switch = Some(rail_tiles[i].0);
                    // Stay open: the refreshed window set animates in.
                }
            }
        }
        // A press that lands on neither a cell nor the rail dismisses the
        // overview (GNOME-style click-away).
        if pressed && self.hovered.is_none() && self.rail_hovered.is_none() {
            self.open = false;
            return;
        }

        // Workspace rail: one tile per workspace, current highlighted.
        if has_rail {
            for (i, tile) in tiles.iter().enumerate() {
                let (id, current) = rail_tiles[i];
                let hovered = self.rail_hovered == Some(i);
                let bg = if current {
                    Color::rgba(64, 110, 220, self.alpha(200))
                } else if hovered {
                    Color::rgba(52, 56, 72, self.alpha(210))
                } else {
                    Color::rgba(24, 26, 36, self.alpha(190))
                };
                let tid = id;
                frame.layer(
                    &format!("ass-overview-ws-{i}"),
                    to_lens(*tile),
                    &OverlayOpts {
                        bg,
                        radius: 10.0,
                        ..Default::default()
                    },
                    move |frame| {
                        frame.column_ex(
                            &LayoutOpts {
                                width: tile.size.w as f32,
                                height: tile.size.h as f32,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            move |frame| {
                                let _ = tid;
                                frame.label_sized(&format!("{}", i + 1), 15.0);
                            },
                        );
                    },
                );
            }
        }

        // Cell frames + labels over the thumbnails the main loop drew.
        for (i, (slot, window)) in slots.iter().zip(windows.iter()).enumerate() {
            let cell = geom::fit(*slot, window.size);
            let hovered = self.hovered == Some(i);
            let border = if hovered {
                Color::rgba(120, 170, 255, self.alpha(255))
            } else {
                Color::rgba(90, 96, 120, self.alpha(160))
            };
            frame.layer(
                &format!("ass-overview-cell-{i}"),
                to_lens(cell),
                &OverlayOpts {
                    border,
                    border_width: if hovered { 2.0 } else { 1.0 },
                    radius: 8.0,
                    ..Default::default()
                },
                |_| {},
            );
            let label = window
                .title
                .clone()
                .or_else(|| window.app_id.clone())
                .unwrap_or_default();
            if !label.is_empty() {
                let label_rect = Rect {
                    x: cell.origin.x as f32,
                    y: (cell.origin.y + cell.size.h + 2) as f32,
                    w: cell.size.w as f32,
                    h: LABEL_H as f32,
                };
                frame.layer(
                    &format!("ass-overview-label-{i}"),
                    label_rect,
                    &OverlayOpts::default(),
                    move |frame| {
                        frame.column_ex(
                            &LayoutOpts {
                                width: label_rect.w,
                                height: label_rect.h,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            move |frame| {
                                frame.label_compact_sized(&label, 12.0);
                            },
                        );
                    },
                );
            }
        }
    }

    fn captures_keyboard(&self) -> bool {
        self.open
    }

    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        self.open || self.visibility > 0.01
    }

    fn modal_active(&self) -> bool {
        self.open || self.visibility > 0.01
    }

    /// The overview renders while modal — it *is* the modal component.
    fn visible_during_modal(&self) -> bool {
        true
    }

    fn cursor_shape_at(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> CursorShape {
        if self.hovered.is_some() || self.rail_hovered.is_some() {
            CursorShape::Pointer
        } else {
            CursorShape::Default
        }
    }

    fn key_char(&mut self, key: &KeyChar, _out: &mut ChromeEvents) {
        if matches!(key_action(key.keysym, key.ch), KeyAction::Escape) {
            self.open = false;
        }
    }

    fn toggle_overview(&mut self, _out: &mut ChromeEvents) {
        self.open = !self.open;
        if self.open {
            self.anim_active = true;
        }
    }

    fn overview_active(&self) -> bool {
        self.open
    }

    fn anim_pending(&self) -> bool {
        self.anim_active
    }

    fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
    }
}

fn contains_rect(rect: ass_core::Rect, x: f32, y: f32) -> bool {
    x >= rect.origin.x as f32
        && y >= rect.origin.y as f32
        && x < (rect.origin.x + rect.size.w) as f32
        && y < (rect.origin.y + rect.size.h) as f32
}

fn to_lens(rect: ass_core::Rect) -> Rect {
    Rect {
        x: rect.origin.x as f32,
        y: rect.origin.y as f32,
        w: rect.size.w as f32,
        h: rect.size.h as f32,
    }
}
