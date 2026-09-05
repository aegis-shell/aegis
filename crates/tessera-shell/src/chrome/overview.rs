//! The overview chrome (M9): a modal window/workspace picker in the GNOME
//! Activities mold. The compositor's main loop draws the live window
//! thumbnails onto the grid computed by the shared `tessera_model::overview`
//! geometry; this component draws the cell frames, labels, and the workspace
//! rail, and owns interaction: hover, click-to-focus, workspace switching,
//! and dismissal. It is a view mode over the same snapshots every other
//! component reads — it never mutates the window model itself.

use tessera_design::Design;
use tessera_design::materials::{chrome_place, surface_layout};
use lens::{Align, Color, Frame, Input, LayoutOpts, Rect};

use crate::{
    Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape, InteractionDomainIntent,
    Localizer, Message,
};
use tessera_model::input::{KeyAction, KeyChar, key_action};
use tessera_model::interaction_domain::{
    InteractionDomain, InteractionDomainId, InteractionDomainKind, InteractionDomainSnapshot,
    InteractionDomainState,
};
use tessera_model::overview as geom;
use tessera_model::window::Window;
use tessera_model::workspace::WorkspaceSnapshot;

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
    drag_candidate: Option<tessera_model::window::WindowId>,
    drag_origin: Option<(f32, f32)>,
    dragging: bool,
    interaction_domain_hovered: Option<InteractionDomainId>,
    reduced_motion: bool,
    /// The design snapshot the overview paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`crate::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
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
            interaction_domains: tessera_model::interaction_domain::InteractionDomainModel::new()
                .snapshot(),
            drag_candidate: None,
            drag_origin: None,
            dragging: false,
            interaction_domain_hovered: None,
            reduced_motion: false,
            design: Design::dark(),
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
        window: tessera_model::window::WindowId,
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
            tessera_model::Rect::new(0, 0, raw.display_size.x as i32, raw.display_size.y as i32);
        let cursor = raw.cursor;
        let down = raw.mouse_down.first().copied().unwrap_or(false);
        let pressed = down && !self.prev_down;
        let released =
            raw.mouse_released.first().copied().unwrap_or(false) || (!down && self.prev_down);

        // Geometry tracks the compositor's thumbnail pass, which sampled
        // `overview_progress` at the start of this frame — i.e. the
        // visibility left by last frame's advance. Capture it before
        // advancing so cell frames stay glued to their flying thumbnails;
        // alpha below uses the freshly advanced visibility.
        let frame_t = self.visibility;
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
        let rail_tiles: Vec<(tessera_model::workspace::WorkspaceId, bool)> = workspaces
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
        // Closest-slot assignment: the same pairing the compositor's
        // thumbnail pass computes, so hover and click land on the cell the
        // user actually sees under the cursor.
        let window_rects: Vec<(tessera_model::window::WindowId, tessera_model::Rect)> = windows
            .iter()
            .map(|w| {
                (
                    w.id,
                    tessera_model::Rect {
                        origin: w.position,
                        size: w.size,
                    },
                )
            })
            .collect();
        let slots: Vec<tessera_model::Rect> = geom::assign_slots(area, &window_rects)
            .into_iter()
            .map(|(_, slot)| slot)
            .collect();
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
            let cell = animated_cell(*slot, window, frame_t);
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

        // One frame-scoped switch fades the whole overview uniformly —
        // frames, labels, and any future imagery — stamped per node at
        // build time. Restored at the end of render; lens also resets it
        // every frame begin.
        frame.set_opacity(self.visibility);

        // Workspace rail: one tile per workspace along the top edge, current
        // highlighted. The compositor draws each workspace's live miniature
        // inside the tile; chrome adds only a translucent tint, the frame,
        // and the caption so the thumbnails stay visible. Token hues keep
        // the fade-driven alphas below scheme-aware.
        let colors = self.design.colors;
        let radii = self.design.radii;
        let typography = self.design.typography;
        if has_rail {
            for (i, tile) in tiles.iter().enumerate() {
                let (_, current) = rail_tiles[i];
                let hovered = self.rail_hovered == Some(i);
                let bg = if current {
                    colors.application_accent.with_alpha(70)
                } else if hovered {
                    colors.application_surface.with_alpha(110)
                } else {
                    colors.application_surface.with_alpha(60)
                };
                let border = if current {
                    colors.application_accent.with_alpha(255)
                } else if hovered {
                    colors.application_accent.with_alpha(190)
                } else {
                    colors.application_border.with_alpha(160)
                };
                frame.place(
                    &format!("tessera-overview-ws-{i}"),
                    &chrome_place(
                        to_lens(*tile),
                        LayoutOpts {
                            bg,
                            border,
                            border_width: if current { 2.0 } else { 1.0 },
                            radius: radii.control,
                            ..surface_layout()
                        },
                    ),
                    |_| {},
                );
                let caption = Rect {
                    x: tile.origin.x as f32,
                    y: (tile.origin.y + tile.size.h - geom::RAIL_LABEL_H) as f32,
                    w: tile.size.w as f32,
                    h: geom::RAIL_LABEL_H as f32,
                };
                frame.place(
                    &format!("tessera-overview-ws-label-{i}"),
                    &chrome_place(caption, surface_layout()),
                    move |frame| {
                        frame.centered(caption.w, caption.h, move |frame| {
                            frame.label_compact_sized(&format!("{}", i + 1), typography.label);
                        });
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
                    colors.application_accent.with_alpha(235)
                } else if active {
                    colors.application_surface.with_alpha(225)
                } else {
                    colors.application_surface.with_alpha(190)
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
                frame.place(
                    &format!("tessera-overview-interaction_domain-{i}"),
                    &chrome_place(
                        to_lens(*tile),
                        LayoutOpts {
                            bg,
                            border: if hovered {
                                colors.application_accent.with_alpha(255)
                            } else {
                                colors.application_border.with_alpha(150)
                            },
                            border_width: if hovered { 2.0 } else { 1.0 },
                            radius: radii.control,
                            ..surface_layout()
                        },
                    ),
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
                                frame.label_sized(status, typography.caption);
                                frame.label_sized(hint, typography.caption);
                            },
                        );
                    },
                );
            }
        }

        // Cell frames + labels over the thumbnails the main loop drew. Each
        // cell resolves through the same fly-in interpolation as the
        // thumbnail pass, so the border rides its thumbnail through the
        // reveal instead of waiting at the final grid position.
        for (i, (slot, window)) in slots.iter().zip(windows.iter()).enumerate() {
            let cell = animated_cell(*slot, window, frame_t);
            let hovered = self.hovered == Some(i);
            let border = if window.read_only {
                // Intentional content color: the amber read-only warning is
                // content styling, not a scheme-token role.
                Color::rgba(192, 157, 86, if hovered { 255 } else { 190 })
            } else if hovered {
                colors.application_accent.with_alpha(255)
            } else {
                colors.application_border.with_alpha(160)
            };
            frame.place(
                &format!("tessera-overview-cell-{i}"),
                &chrome_place(
                    to_lens(cell),
                    LayoutOpts {
                        border,
                        border_width: if hovered { 2.0 } else { 1.0 },
                        radius: radii.menu_item,
                        ..surface_layout()
                    },
                ),
                |_| {},
            );
            let mut label = resolve_overview_hierarchy_title(window, windows);
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
                frame.place(
                    &format!("tessera-overview-label-{i}"),
                    &chrome_place(label_rect, surface_layout()),
                    move |frame| {
                        frame.centered(label_rect.w, label_rect.h, move |frame| {
                            frame.label_compact_sized(&label, typography.label);
                        });
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
            frame.place(
                "tessera-overview-interaction_domain-drag-ghost",
                &chrome_place(
                    ghost,
                    LayoutOpts {
                        bg: colors.application_accent.with_alpha(230),
                        border: colors.application_accent.with_alpha(255),
                        border_width: 1.0,
                        radius: radii.control,
                        ..surface_layout()
                    },
                ),
                |frame| {
                    frame.centered(ghost.w, ghost.h, |frame| {
                        frame.label_compact_sized(
                            i18n.text(Message::MoveToInteractionDomain),
                            typography.footnote,
                        )
                    });
                },
            );
        }

        frame.set_opacity(1.0);
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
        match command {
            ChromeCommand::ToggleOverview => {
                self.open = !self.open;
                if self.open {
                    self.anim_active = true;
                }
            }
            ChromeCommand::CloseOverview | ChromeCommand::DismissModal => {
                if self.open {
                    self.open = false;
                    self.anim_active = true;
                }
            }
            _ => {}
        }
    }

    fn overview_active(&self) -> bool {
        self.open
    }

    fn overview_progress(&self) -> f32 {
        self.visibility
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
            ChromeUpdate::Appearance(design) => self.design = *design,
            _ => {}
        }
    }
}

fn contains_rect(rect: tessera_model::Rect, x: f32, y: f32) -> bool {
    x >= rect.origin.x as f32
        && y >= rect.origin.y as f32
        && x < (rect.origin.x + rect.size.w) as f32
        && y < (rect.origin.y + rect.size.h) as f32
}

/// The cell a window's thumbnail occupies this frame: the shared fly-in
/// interpolation from the window's real geometry to its aspect-fitted grid
/// slot, keyed on the same progress the compositor's thumbnail pass used.
fn animated_cell(slot: tessera_model::Rect, window: &Window, t: f32) -> tessera_model::Rect {
    geom::animated_cell(
        slot,
        tessera_model::Rect {
            origin: window.position,
            size: window.size,
        },
        t,
    )
}

fn to_lens(rect: tessera_model::Rect) -> Rect {
    Rect {
        x: rect.origin.x as f32,
        y: rect.origin.y as f32,
        w: rect.size.w as f32,
        h: rect.size.h as f32,
    }
}

fn resolve_overview_hierarchy_title(window: &Window, windows: &[Window]) -> String {
    let mut chain = Vec::new();
    let mut current_id = window.parent_id;
    let mut depth = 0;
    while let Some(pid) = current_id {
        if depth >= 10 {
            break;
        }
        if let Some(parent) = windows.iter().find(|w| w.id == pid) {
            let p_title = parent
                .title
                .as_deref()
                .or(parent.app_id.as_deref())
                .unwrap_or_default();
            if !p_title.is_empty() {
                chain.push(p_title);
            }
            current_id = parent.parent_id;
            depth += 1;
        } else {
            break;
        }
    }
    let own_title = window
        .title
        .as_deref()
        .or(window.app_id.as_deref())
        .unwrap_or_default();

    if chain.is_empty() {
        if window.suspended_by_modal {
            format!("{own_title} ⧗")
        } else {
            own_title.to_string()
        }
    } else {
        chain.reverse();
        let mut full = chain.join(" › ");
        full.push_str(" › ");
        full.push_str(own_title);
        if window.suspended_by_modal {
            full.push_str(" ⧗");
        }
        full
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tessera_model::window::{Window, WindowId};

    #[test]
    fn overview_hierarchy_title_resolves_breadcrumbs() {
        let parent = Window {
            id: WindowId(1),
            title: Some("Editor".into()),
            ..Default::default()
        };
        let dialog = Window {
            id: WindowId(2),
            title: Some("Find & Replace".into()),
            parent_id: Some(WindowId(1)),
            ..Default::default()
        };
        let windows = vec![parent.clone(), dialog.clone()];

        assert_eq!(
            resolve_overview_hierarchy_title(&parent, &windows),
            "Editor"
        );
        assert_eq!(
            resolve_overview_hierarchy_title(&dialog, &windows),
            "Editor › Find & Replace"
        );
    }
}
