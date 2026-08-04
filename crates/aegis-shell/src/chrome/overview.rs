//! The overview chrome (M9): a modal window/workspace picker in the GNOME
//! Activities mold. The compositor's main loop draws the live window
//! thumbnails onto the grid computed by the shared `aegis_model::overview`
//! geometry; this component draws the cell frames, labels, and the workspace
//! rail, and owns interaction: hover, click-to-focus, workspace switching,
//! and dismissal. It is a view mode over the same snapshots every other
//! component reads — it never mutates the window model itself.

use lens::{Align, Color, Frame, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{
    Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape, InteractionDomainIntent,
    Localizer, Message,
};
use aegis_model::input::{KeyAction, KeyChar, key_action};
use aegis_model::interaction_domain::{
    InteractionDomain, InteractionDomainId, InteractionDomainKind, InteractionDomainSnapshot,
    InteractionDomainState,
};
use aegis_model::overview as geom;
use aegis_model::window::Window;
use aegis_model::workspace::WorkspaceSnapshot;

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
    interaction_domains: InteractionDomainSnapshot,
    /// Pressed window that may become a drag after crossing the threshold.
    drag_candidate: Option<aegis_model::window::WindowId>,
    drag_origin: Option<(f32, f32)>,
    dragging: bool,
    interaction_domain_hovered: Option<InteractionDomainId>,
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
            interaction_domains: aegis_model::interaction_domain::InteractionDomainModel::new()
                .snapshot(),
            drag_candidate: None,
            drag_origin: None,
            dragging: false,
            interaction_domain_hovered: None,
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

    fn live_interaction_domains(&self) -> Vec<InteractionDomain> {
        self.interaction_domains
            .interaction_domains
            .iter()
            .filter(|interaction_domain| {
                interaction_domain.state != InteractionDomainState::Revoked
            })
            .filter(|interaction_domain| {
                interaction_domain.kind == InteractionDomainKind::Human
                    || (interaction_domain.kind == InteractionDomainKind::Agent
                        && self
                            .interaction_domains
                            .interaction_domains
                            .iter()
                            .any(|candidate| candidate.kind == InteractionDomainKind::Agent))
            })
            .cloned()
            .collect()
    }

    fn control_interaction_domain_for_window(
        &self,
        window: aegis_model::window::WindowId,
    ) -> Option<InteractionDomainId> {
        self.interaction_domains
            .interaction_groups
            .iter()
            .find(|group| group.windows.contains(&window))
            .map(|group| group.control_interaction_domain)
    }

    fn reset_drag(&mut self) {
        self.drag_candidate = None;
        self.drag_origin = None;
        self.dragging = false;
        self.interaction_domain_hovered = None;
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
            aegis_model::Rect::new(0, 0, raw.display_size.x as i32, raw.display_size.y as i32);
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
        let rail_tiles: Vec<(aegis_model::workspace::WorkspaceId, bool)> = workspaces
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
        let live_interaction_domains = self.live_interaction_domains();
        let has_interaction_domain_shelf = live_interaction_domains
            .iter()
            .any(|interaction_domain| interaction_domain.kind == InteractionDomainKind::Agent);
        let area = geom::grid_area_with_interaction_domain_shelf(
            display,
            has_rail,
            has_interaction_domain_shelf,
        );
        let slots = geom::grid(area, windows.len());
        let tiles = geom::rail(display, rail_tiles.len());
        let interaction_domain_tiles = if has_interaction_domain_shelf {
            geom::interaction_domain_shelf(display, live_interaction_domains.len())
        } else {
            Vec::new()
        };

        // Hover and drag-source resolution over the exact cells the texture
        // pass uses. A click focuses; crossing the threshold starts an
        // authority drag.
        self.hovered = None;
        self.rail_hovered = None;
        self.interaction_domain_hovered = None;
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
            for (interaction_domain, tile) in live_interaction_domains
                .iter()
                .zip(interaction_domain_tiles.iter())
            {
                if contains_rect(*tile, cursor.x, cursor.y)
                    && interaction_domain.state == InteractionDomainState::Active
                {
                    self.interaction_domain_hovered = Some(interaction_domain.id);
                }
            }
        }

        if released {
            if let Some(window) = self.drag_candidate {
                if self.dragging {
                    if let Some(target) = self.interaction_domain_hovered
                        && self.control_interaction_domain_for_window(window) != Some(target)
                    {
                        out.interaction_domain_intents.push(
                            InteractionDomainIntent::TransferWindow {
                                window,
                                target,
                                retain_source_as_observer: true,
                                expected_revision: self.interaction_domains.revision,
                            },
                        );
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
            && !interaction_domain_tiles
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
                    &format!("aegis-overview-ws-{i}"),
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

        // Interaction Domain shelf: active targets accept a dragged interaction group.
        // Paused targets remain visible for state awareness but fail closed as
        // transfer destinations.
        if has_interaction_domain_shelf {
            for (i, (interaction_domain, tile)) in live_interaction_domains
                .iter()
                .zip(interaction_domain_tiles.iter())
                .enumerate()
            {
                let hovered = self.interaction_domain_hovered == Some(interaction_domain.id);
                let active = interaction_domain.state == InteractionDomainState::Active;
                let bg = if hovered {
                    Color::rgba(62, 124, 224, self.alpha(235))
                } else if active {
                    Color::rgba(29, 38, 58, self.alpha(225))
                } else {
                    Color::rgba(36, 37, 44, self.alpha(190))
                };
                let label = interaction_domain.label.clone();
                let status = match interaction_domain.state {
                    InteractionDomainState::Active => i18n.text(Message::InteractionDomainActive),
                    InteractionDomainState::Paused => i18n.text(Message::InteractionDomainPaused),
                    InteractionDomainState::Revoked => i18n.text(Message::InteractionDomainRevoked),
                };
                let hint = if interaction_domain.kind == InteractionDomainKind::Human {
                    i18n.text(Message::PhysicalDesktop)
                } else if active {
                    i18n.text(Message::DropWindowHere)
                } else {
                    i18n.text(Message::InteractionDomainPaused)
                };
                frame.layer(
                    &format!("aegis-overview-interaction_domain-{i}"),
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
                &format!("aegis-overview-cell-{i}"),
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
                    &format!("aegis-overview-label-{i}"),
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
                "aegis-overview-interaction_domain-drag-ghost",
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
                        |frame| {
                            frame.label_sized(i18n.text(Message::MoveToInteractionDomain), 11.0)
                        },
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

    fn requires_composition(&self) -> bool {
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

    fn command(&mut self, command: &ChromeCommand<'_>, _out: &mut ChromeEvents) {
        if matches!(command, ChromeCommand::ToggleOverview) {
            self.open = !self.open;
            if self.open {
                self.anim_active = true;
            }
        }
    }

    fn overview_active(&self) -> bool {
        self.open
    }

    fn anim_pending(&self) -> bool {
        self.anim_active
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::ReducedMotion(reduced) => self.reduced_motion = reduced,
            ChromeUpdate::InteractionDomains(snapshot) => {
                self.interaction_domains = snapshot.clone();
            }
            _ => {}
        }
    }
}

fn contains_rect(rect: aegis_model::Rect, x: f32, y: f32) -> bool {
    x >= rect.origin.x as f32
        && y >= rect.origin.y as f32
        && x < (rect.origin.x + rect.size.w) as f32
        && y < (rect.origin.y + rect.size.h) as f32
}

fn to_lens(rect: aegis_model::Rect) -> Rect {
    Rect {
        x: rect.origin.x as f32,
        y: rect.origin.y as f32,
        w: rect.size.w as f32,
        h: rect.size.h as f32,
    }
}
