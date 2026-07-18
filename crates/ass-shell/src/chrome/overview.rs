//! The overview chrome (M9): a modal window/workspace picker in the GNOME
//! Activities mold. The compositor's main loop draws the live window
//! thumbnails onto the grid computed by the shared `ass_core::overview`
//! geometry; this component draws the cell frames, labels, and the workspace
//! rail, and owns interaction: hover, click-to-focus, workspace switching,
//! and dismissal. It is a view mode over the same snapshots every other
//! component reads — it never mutates the window model itself.

use lens::{Align, Color, Frame, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{Chrome, ChromeEvents, CursorShape, Localizer, Message, RealmIntent};
use ass_core::input::{KeyAction, KeyChar, key_action};
use ass_core::overview as geom;
use ass_core::realm::{Realm, RealmId, RealmKind, RealmSnapshot, RealmState};
use ass_core::window::Window;
use ass_core::workspace::WorkspaceSnapshot;

/// Reveal/fade speed (per second, exponential approach).
const FADE_RATE: f32 = 14.0;
/// Label strip height under a thumbnail, in logical pixels.
const LABEL_H: i32 = 22;
/// Pointer travel before a press becomes a drag instead of a click.
const DRAG_THRESHOLD: f32 = 8.0;

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
    /// Complete authority snapshot supplied by the compositor.
    realms: RealmSnapshot,
    /// Pressed window that may become a drag after crossing the threshold.
    drag_candidate: Option<ass_core::window::WindowId>,
    drag_origin: Option<(f32, f32)>,
    dragging: bool,
    realm_hovered: Option<RealmId>,
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
            realms: ass_core::realm::RealmModel::new().snapshot(),
            drag_candidate: None,
            drag_origin: None,
            dragging: false,
            realm_hovered: None,
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

    fn live_realms(&self) -> Vec<Realm> {
        self.realms
            .realms
            .iter()
            .filter(|realm| realm.state != RealmState::Revoked)
            .filter(|realm| {
                realm.kind == RealmKind::Human
                    || (realm.kind == RealmKind::Agent
                        && self
                            .realms
                            .realms
                            .iter()
                            .any(|candidate| candidate.kind == RealmKind::Agent))
            })
            .cloned()
            .collect()
    }

    fn control_realm_for_window(&self, window: ass_core::window::WindowId) -> Option<RealmId> {
        self.realms
            .interaction_groups
            .iter()
            .find(|group| group.windows.contains(&window))
            .map(|group| group.control_realm)
    }

    fn reset_drag(&mut self) {
        self.drag_candidate = None;
        self.drag_origin = None;
        self.dragging = false;
        self.realm_hovered = None;
    }
}

impl Chrome for Overview {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        windows: &[Window],
        workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let raw = input.as_raw();
        let display =
            ass_core::Rect::new(0, 0, raw.display_size.x as i32, raw.display_size.y as i32);
        let cursor = raw.cursor;
        let down = raw.mouse_down.first().copied().unwrap_or(false);
        let pressed = down && !self.prev_down;
        let released =
            raw.mouse_released.first().copied().unwrap_or(false) || (!down && self.prev_down);

        self.advance(raw.dt_seconds.max(0.0));
        self.prev_down = down;
        if self.visibility <= 0.001 && !self.open {
            self.hovered = None;
            self.rail_hovered = None;
            self.reset_drag();
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
        let live_realms = self.live_realms();
        let has_realm_shelf = live_realms
            .iter()
            .any(|realm| realm.kind == RealmKind::Agent);
        let area = geom::grid_area_with_realm_shelf(display, has_rail, has_realm_shelf);
        let slots = geom::grid(area, windows.len());
        let tiles = geom::rail(display, rail_tiles.len());
        let realm_tiles = if has_realm_shelf {
            geom::realm_shelf(display, live_realms.len())
        } else {
            Vec::new()
        };

        // Hover and drag-source resolution over the exact cells the texture
        // pass uses. A click focuses; crossing the threshold starts an
        // authority drag.
        self.hovered = None;
        self.rail_hovered = None;
        self.realm_hovered = None;
        for (i, (slot, window)) in slots.iter().zip(windows.iter()).enumerate() {
            let cell = geom::fit(*slot, window.size);
            if contains_rect(cell, cursor.x, cursor.y) {
                self.hovered = Some(i);
                if pressed {
                    self.drag_candidate = Some(window.id);
                    self.drag_origin = Some((cursor.x, cursor.y));
                }
            }
        }
        if down
            && self.drag_candidate.is_some()
            && self.drag_origin.is_some_and(|origin| {
                (cursor.x - origin.0).hypot(cursor.y - origin.1) >= DRAG_THRESHOLD
            })
        {
            self.dragging = true;
        }
        for (i, tile) in tiles.iter().enumerate() {
            if contains_rect(*tile, cursor.x, cursor.y) {
                self.rail_hovered = Some(i);
                if pressed && self.drag_candidate.is_none() {
                    out.overview_switch = Some(rail_tiles[i].0);
                    // Stay open: the refreshed window set animates in.
                }
            }
        }
        if self.dragging {
            for (realm, tile) in live_realms.iter().zip(realm_tiles.iter()) {
                if contains_rect(*tile, cursor.x, cursor.y) && realm.state == RealmState::Active {
                    self.realm_hovered = Some(realm.id);
                }
            }
        }

        if released {
            if let Some(window) = self.drag_candidate {
                if self.dragging {
                    if let Some(target) = self.realm_hovered
                        && self.control_realm_for_window(window) != Some(target)
                    {
                        out.realm_intents.push(RealmIntent::TransferWindow {
                            window,
                            target,
                            retain_source_as_observer: true,
                            expected_revision: self.realms.revision,
                        });
                    }
                } else if self
                    .hovered
                    .and_then(|index| windows.get(index))
                    .is_some_and(|candidate| candidate.id == window && !candidate.read_only)
                {
                    out.overview_pick = Some(window);
                    self.open = false;
                }
            }
            self.reset_drag();
            return;
        }
        // A press that lands on neither a cell nor the rail dismisses the
        // overview (GNOME-style click-away).
        if pressed
            && self.hovered.is_none()
            && self.rail_hovered.is_none()
            && !realm_tiles
                .iter()
                .any(|tile| contains_rect(*tile, cursor.x, cursor.y))
        {
            self.open = false;
            self.reset_drag();
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

        // Realm shelf: active targets accept a dragged interaction group.
        // Paused targets remain visible for state awareness but fail closed as
        // transfer destinations.
        if has_realm_shelf {
            for (i, (realm, tile)) in live_realms.iter().zip(realm_tiles.iter()).enumerate() {
                let hovered = self.realm_hovered == Some(realm.id);
                let active = realm.state == RealmState::Active;
                let bg = if hovered {
                    Color::rgba(62, 124, 224, self.alpha(235))
                } else if active {
                    Color::rgba(29, 38, 58, self.alpha(225))
                } else {
                    Color::rgba(36, 37, 44, self.alpha(190))
                };
                let label = realm.label.clone();
                let status = match realm.state {
                    RealmState::Active => i18n.text(Message::RealmActive),
                    RealmState::Paused => i18n.text(Message::RealmPaused),
                    RealmState::Revoked => i18n.text(Message::RealmRevoked),
                };
                let hint = if realm.kind == RealmKind::Human {
                    i18n.text(Message::PhysicalDesktop)
                } else if active {
                    i18n.text(Message::DropWindowHere)
                } else {
                    i18n.text(Message::RealmPaused)
                };
                frame.layer(
                    &format!("ass-overview-realm-{i}"),
                    to_lens(*tile),
                    &OverlayOpts {
                        bg,
                        border: if hovered {
                            Color::rgba(136, 186, 255, self.alpha(255))
                        } else {
                            Color::rgba(96, 112, 146, self.alpha(150))
                        },
                        border_width: if hovered { 2.0 } else { 1.0 },
                        radius: 12.0,
                        ..Default::default()
                    },
                    move |frame| {
                        frame.column_ex(
                            &LayoutOpts {
                                width: tile.size.w as f32,
                                height: tile.size.h as f32,
                                gap: 3.0,
                                pad: 10.0,
                                cross: Align::Start,
                                ..Default::default()
                            },
                            move |frame| {
                                frame.heading(&label, 3);
                                frame.label_sized(status, 10.0);
                                frame.label_sized(hint, 10.0);
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
            let border = if window.read_only {
                Color::rgba(192, 157, 86, self.alpha(if hovered { 255 } else { 190 }))
            } else if hovered {
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
            let mut label = window
                .title
                .clone()
                .or_else(|| window.app_id.clone())
                .unwrap_or_default();
            if window.read_only {
                if !label.is_empty() {
                    label.push_str(" · ");
                }
                label.push_str(i18n.text(Message::ReadOnlyMirror));
            }
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

        if self.dragging {
            let ghost = Rect {
                x: cursor.x - 66.0,
                y: cursor.y - 24.0,
                w: 132.0,
                h: 48.0,
            };
            frame.layer(
                "ass-overview-realm-drag-ghost",
                ghost,
                &OverlayOpts {
                    bg: Color::rgba(45, 88, 166, self.alpha(230)),
                    border: Color::rgba(154, 196, 255, self.alpha(255)),
                    border_width: 1.0,
                    radius: 10.0,
                    ..Default::default()
                },
                |frame| {
                    frame.column_ex(
                        &LayoutOpts {
                            width: ghost.w,
                            height: ghost.h,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |frame| frame.label_sized(i18n.text(Message::MoveToRealm), 11.0),
                    );
                },
            );
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
    ) -> Option<CursorShape> {
        Some(if self.dragging {
            CursorShape::Crosshair
        } else if self.hovered.is_some() || self.rail_hovered.is_some() {
            CursorShape::Pointer
        } else {
            CursorShape::Default
        })
    }

    fn key_char(&mut self, key: &KeyChar, _out: &mut ChromeEvents) {
        if matches!(key_action(key.keysym, key.ch), KeyAction::Escape) {
            self.open = false;
            self.reset_drag();
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

    fn update_realms(&mut self, snapshot: &RealmSnapshot) {
        self.realms = snapshot.clone();
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
