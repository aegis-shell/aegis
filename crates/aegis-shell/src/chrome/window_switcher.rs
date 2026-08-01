//! Held-Super window switcher chrome.
//!
//! The compositor renderer paints each live window into the preview rects
//! from `aegis_core::window_switcher`; this component adds the glass panel,
//! selection borders, icons, labels, shared carousel animation, and pointer
//! hit-testing. Releasing Super or clicking a card closes the strip.

use std::collections::{HashMap, HashSet};

use aegis_design::{Design, materials};
use lens::{Align, Color, Frame, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{
    AppCatalog, BackdropRegion, Chrome, ChromeEvents, CursorShape, IconSet, LiquidGlassRegion,
    Localizer, WindowSwitcherCard, WindowSwitcherPresentation, truncate,
};
use aegis_core::window::{Window, WindowId};
use aegis_core::workspace::WorkspaceSnapshot;

const FADE_RATE: f32 = 18.0;
const SLIDE_RATE: f32 = 15.0;
const BACKDROP_BLUR_SIGMA: f32 = 16.0;
const PANEL_RADIUS: f32 = 20.0;

pub struct WindowSwitcher {
    open: bool,
    visibility: f32,
    anim_active: bool,
    reduced_motion: bool,
    icons: IconSet,
    /// Bottom-to-top compositor order from the last ordinary window snapshot.
    known_windows: Vec<WindowId>,
    /// Frozen MRU order for the current held-Super session.
    order: Vec<WindowId>,
    animated_cards: HashMap<WindowId, aegis_core::window_switcher::Card>,
    presentation: Option<WindowSwitcherPresentation>,
    hovered: Option<WindowId>,
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
            known_windows: Vec::new(),
            order: Vec::new(),
            animated_cards: HashMap::new(),
            presentation: None,
            hovered: None,
        }
    }

    fn advance_visibility(&mut self, dt: f32) {
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

    fn prepare(
        &mut self,
        input: &Input,
        display: aegis_core::Rect,
        windows: &[Window],
    ) -> Option<WindowSwitcherPresentation> {
        let dt = input.as_raw().dt_seconds.max(0.0);
        self.advance_visibility(dt);
        if self.visibility <= 0.001 && !self.open {
            self.presentation = None;
            self.animated_cards.clear();
            self.hovered = None;
            return None;
        }

        let live: HashSet<_> = windows.iter().map(|window| window.id).collect();
        self.order.retain(|id| live.contains(id));
        // A window mapped while the switcher is held joins at the old end
        // without perturbing the frozen MRU order already visible to the user.
        for id in windows.iter().rev().map(|window| window.id) {
            if !self.order.contains(&id) {
                self.order.push(id);
            }
        }
        if self.order.is_empty() {
            self.order
                .extend(windows.iter().rev().map(|window| window.id));
        }

        let selected = windows
            .iter()
            .find(|window| window.state.activated)
            .map(|window| window.id)
            .filter(|id| self.order.contains(id))
            .or_else(|| self.order.first().copied());
        let selected_index = selected
            .and_then(|id| self.order.iter().position(|candidate| *candidate == id))
            .unwrap_or(0);
        let target =
            aegis_core::window_switcher::carousel_layout(display, self.order.len(), selected_index);
        let blend = if self.reduced_motion {
            1.0
        } else {
            1.0 - (-dt * SLIDE_RATE).exp()
        };
        let mut cards = Vec::with_capacity(self.order.len());
        let mut cards_moving = false;
        for (id, target_card) in self.order.iter().copied().zip(target.cards.iter().copied()) {
            let current = self.animated_cards.entry(id).or_insert(target_card);
            // The chosen window owns the fixed visual centre immediately.
            // The surrounding cards glide around it instead of dragging the
            // highlight across a row of fixed slots.
            if Some(id) == selected || self.reduced_motion {
                *current = target_card;
            } else {
                *current = lerp_card(*current, target_card, blend);
            }
            cards_moving |= *current != target_card;
            cards.push(WindowSwitcherCard {
                window: id,
                geometry: *current,
            });
        }
        self.animated_cards.retain(|id, _| live.contains(id));
        self.anim_active |= cards_moving;
        if !cards_moving {
            self.anim_active = (self.visibility - if self.open { 1.0 } else { 0.0 }).abs() > 0.002;
        }

        let presentation = WindowSwitcherPresentation {
            panel: target.panel,
            cards,
            selected,
            visibility: self.visibility,
        };
        self.presentation = Some(presentation.clone());
        Some(presentation)
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
        out: &mut ChromeEvents,
    ) {
        let display = aegis_core::Rect::new(
            0,
            0,
            input.as_raw().display_size.x.max(1.0) as i32,
            input.as_raw().display_size.y.max(1.0) as i32,
        );
        if self.presentation.is_none() {
            self.prepare(input, display, windows);
        }
        let Some(presentation) = self.presentation.clone() else {
            return;
        };

        let raw = input.as_raw();
        let cursor = (raw.cursor.x, raw.cursor.y);
        self.hovered = self
            .open
            .then(|| {
                presentation
                    .cards
                    .iter()
                    .find(|card| {
                        Some(card.window) == presentation.selected
                            && contains(card.geometry.outer, cursor.0, cursor.1)
                    })
                    .or_else(|| {
                        presentation
                            .cards
                            .iter()
                            .rev()
                            .find(|card| contains(card.geometry.outer, cursor.0, cursor.1))
                    })
                    .map(|card| card.window)
            })
            .flatten();
        let pressed = raw.mouse_pressed.first().copied().unwrap_or(false);
        if self.open && pressed {
            if let Some(id) = self.hovered
                && windows
                    .iter()
                    .find(|window| window.id == id)
                    .is_some_and(|window| !window.read_only)
            {
                out.window_switcher_pick = Some(id);
            } else {
                out.window_switcher_cancel = true;
            }
            self.finish_window_switcher();
        }

        let panel = to_lens(presentation.panel);
        let mut panel_material = materials::dock(&Design::dark());
        panel_material.bg = Color::rgba(255, 255, 255, self.alpha(12));
        panel_material.radius = PANEL_RADIUS;
        frame.layer(
            "aegis-window-switcher-panel",
            panel,
            &panel_material,
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

        let mut render_cards: Vec<_> = presentation.cards.iter().collect();
        render_cards.sort_by_key(|card| Some(card.window) == presentation.selected);
        for (index, presented) in render_cards.into_iter().enumerate() {
            let Some(window) = windows.iter().find(|window| window.id == presented.window) else {
                continue;
            };
            let selected = Some(window.id) == presentation.selected;
            let hovered = Some(window.id) == self.hovered && !window.read_only;
            let outer = to_lens(presented.geometry.outer);
            frame.layer(
                &format!("aegis-window-switcher-card-{index}"),
                outer,
                &OverlayOpts {
                    bg: Color::rgba(
                        255,
                        255,
                        255,
                        self.alpha(if selected {
                            18
                        } else if hovered {
                            12
                        } else {
                            4
                        }),
                    ),
                    border: if selected {
                        Color::rgba(116, 170, 255, self.alpha(255))
                    } else if hovered {
                        Color::rgba(154, 196, 255, self.alpha(220))
                    } else {
                        Color::rgba(255, 255, 255, self.alpha(38))
                    },
                    border_width: if selected {
                        3.0
                    } else if hovered {
                        2.0
                    } else {
                        1.0
                    },
                    radius: 13.0,
                    ..Default::default()
                },
                |_| {},
            );

            let label_rect = to_lens(presented.geometry.label);
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
                    bg: Color::rgba(255, 255, 255, self.alpha(10)),
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

    fn prepare_window_switcher(
        &mut self,
        input: &Input,
        display: aegis_core::Rect,
        windows: &[Window],
    ) -> Option<WindowSwitcherPresentation> {
        self.prepare(input, display, windows)
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

    fn cursor_shape_at(
        &self,
        x: f32,
        y: f32,
        _display: (f32, f32),
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        self.open
            .then(|| {
                self.presentation.as_ref()?.cards.iter().find(|card| {
                    contains(card.geometry.outer, x, y)
                        && windows
                            .iter()
                            .find(|window| window.id == card.window)
                            .is_some_and(|window| !window.read_only)
                })
            })
            .flatten()
            .map(|_| CursorShape::Pointer)
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

    fn requires_composition(&self) -> bool {
        self.open || self.visibility > 0.01
    }

    fn backdrop_blur_sigma(&self) -> f32 {
        if self.open || self.visibility > 0.01 {
            BACKDROP_BLUR_SIGMA
        } else {
            0.0
        }
    }

    fn backdrop_regions(
        &self,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        self.presentation
            .as_ref()
            .filter(|presentation| presentation.visibility > 0.01)
            .map(|presentation| vec![backdrop_region(presentation.panel)])
            .unwrap_or_default()
    }

    fn liquid_glass_regions(
        &self,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<LiquidGlassRegion> {
        self.presentation
            .as_ref()
            .filter(|presentation| presentation.visibility > 0.01)
            .map(|presentation| {
                vec![LiquidGlassRegion {
                    bounds: backdrop_region(presentation.panel),
                    corner_radius: PANEL_RADIUS,
                    opacity: presentation.visibility,
                    shadow_alpha: 0.18,
                    shadow_blur: 16.0,
                    shadow_offset_y: 8.0,
                }]
            })
            .unwrap_or_default()
    }

    fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
    }

    fn update_app_catalog(&mut self, catalog: &AppCatalog) {
        self.icons = catalog.icons.clone();
    }

    fn update_windows(&mut self, windows: &[Window]) {
        self.known_windows = windows.iter().map(|window| window.id).collect();
    }

    fn start_window_switcher(&mut self) {
        if self.open {
            return;
        }
        self.order = self.known_windows.iter().rev().copied().collect();
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

fn backdrop_region(rect: aegis_core::Rect) -> BackdropRegion {
    BackdropRegion {
        x: rect.origin.x as f32,
        y: rect.origin.y as f32,
        w: rect.size.w as f32,
        h: rect.size.h as f32,
    }
}

fn contains(rect: aegis_core::Rect, x: f32, y: f32) -> bool {
    x >= rect.origin.x as f32
        && y >= rect.origin.y as f32
        && x < (rect.origin.x + rect.size.w) as f32
        && y < (rect.origin.y + rect.size.h) as f32
}

fn lerp_i32(from: i32, to: i32, blend: f32) -> i32 {
    let value = from as f32 + (to - from) as f32 * blend;
    if (value - to as f32).abs() < 0.75 {
        to
    } else {
        value.round() as i32
    }
}

fn lerp_rect(from: aegis_core::Rect, to: aegis_core::Rect, blend: f32) -> aegis_core::Rect {
    aegis_core::Rect::new(
        lerp_i32(from.origin.x, to.origin.x, blend),
        lerp_i32(from.origin.y, to.origin.y, blend),
        lerp_i32(from.size.w, to.size.w, blend),
        lerp_i32(from.size.h, to.size.h, blend),
    )
}

fn lerp_card(
    from: aegis_core::window_switcher::Card,
    to: aegis_core::window_switcher::Card,
    blend: f32,
) -> aegis_core::window_switcher::Card {
    aegis_core::window_switcher::Card {
        outer: lerp_rect(from.outer, to.outer, blend),
        preview: lerp_rect(from.preview, to.preview, blend),
        label: lerp_rect(from.label, to.label, blend),
    }
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

    #[test]
    fn card_animation_converges_without_overshoot() {
        let from = aegis_core::window_switcher::Card {
            outer: aegis_core::Rect::new(0, 10, 100, 80),
            preview: aegis_core::Rect::new(0, 10, 100, 50),
            label: aegis_core::Rect::new(0, 60, 100, 30),
        };
        let to = aegis_core::window_switcher::Card {
            outer: aegis_core::Rect::new(200, 10, 100, 80),
            preview: aegis_core::Rect::new(200, 10, 100, 50),
            label: aegis_core::Rect::new(200, 60, 100, 30),
        };
        let card = lerp_card(from, to, 0.25);
        assert_eq!(card.outer.origin.x, 50);
        assert!(card.outer.origin.x < to.outer.origin.x);
    }

    #[test]
    fn visible_switcher_panel_is_declared_as_liquid_glass() {
        let mut switcher = WindowSwitcher::new();
        switcher.presentation = Some(WindowSwitcherPresentation {
            panel: aegis_core::Rect::new(200, 160, 640, 240),
            cards: Vec::new(),
            selected: None,
            visibility: 0.6,
        });
        let workspaces = WorkspaceSnapshot {
            outputs: Vec::new(),
        };

        let backdrop = switcher.backdrop_regions((1280.0, 720.0), &[], &workspaces);
        let glass = switcher.liquid_glass_regions((1280.0, 720.0), &[], &workspaces);
        assert_eq!(backdrop.len(), 1);
        assert_eq!(glass.len(), 1);
        assert_eq!(glass[0].bounds, backdrop[0]);
        assert_eq!(glass[0].corner_radius, PANEL_RADIUS);
        assert_eq!(glass[0].opacity, 0.6);
    }
}
