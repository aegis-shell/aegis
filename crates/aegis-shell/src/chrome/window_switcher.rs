//! Held-Super window switcher chrome.
//!
//! The compositor renderer paints each live window into the preview rects
//! from `aegis_model::window_switcher`; this component adds the glass panel,
//! selection focus, icons, labels, shared carousel animation, and pointer
//! hit-testing. Releasing Super or clicking a card closes the strip.

use std::collections::{HashMap, HashSet};

use aegis_design::materials::{chrome_place, surface_layout};
use aegis_design::{Design, GlassRole, PreviewSelectionStyle};
use lens::{Align, Color, Frame, Input, LayoutOpts, Rect};

use crate::{
    BackdropRegion, Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape, IconSet,
    LiquidGlassRegion, Localizer, Message, PreviewCard, WindowSwitcherPresentation, ellipsize,
    preview,
};
use aegis_model::input::{KeyAction, KeyChar, key_action};
use aegis_model::window::{Window, WindowId};
use aegis_model::workspace::WorkspaceSnapshot;

const FADE_RATE: f32 = 18.0;
const SLIDE_RATE: f32 = 15.0;
const BACKDROP_BLUR_SIGMA: f32 = 16.0;

pub struct WindowSwitcher {
    open: bool,
    visibility: f32,
    anim_active: bool,
    reduced_motion: bool,
    icons: IconSet,
    /// Frozen MRU order for the current held-Super session.
    order: Vec<WindowId>,
    layout_key: Option<(usize, i32, i32, i32, i32)>,
    mode: aegis_model::window_switcher::Mode,
    animated_cards: HashMap<WindowId, aegis_model::window_switcher::Card>,
    animated_selection: Option<aegis_model::window_switcher::Card>,
    presentation: Option<WindowSwitcherPresentation>,
    hovered: Option<WindowId>,
    /// The design snapshot the switcher paints from, from
    /// [`ChromeUpdate::Appearance`]. Seeded on registration by
    /// [`crate::Shell::add`] and refreshed when the desktop color scheme
    /// changes; defaults to the dark appearance until the first update arrives.
    design: Design,
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
            order: Vec::new(),
            layout_key: None,
            mode: aegis_model::window_switcher::Mode::Fixed,
            animated_cards: HashMap::new(),
            animated_selection: None,
            presentation: None,
            hovered: None,
            design: Design::dark(),
        }
    }

    fn start_window_switcher(&mut self) {
        if !self.open {
            self.order.clear();
            self.layout_key = None;
            self.open = true;
            self.anim_active = true;
        }
    }

    fn finish_window_switcher(&mut self) {
        if self.open {
            self.open = false;
            self.anim_active = true;
        }
    }

    fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
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
        display: aegis_model::Rect,
        windows: &[Window],
        session_order: &[WindowId],
        selected: Option<WindowId>,
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
        let design = self.design;
        if self.open {
            // The compositor owns the frozen eligible set. Replacing from its
            // snapshot removes closed windows but deliberately ignores newly
            // mapped windows until the next switcher session.
            self.order = session_order
                .iter()
                .copied()
                .filter(|id| live.contains(id))
                .collect();
        } else {
            self.order.retain(|id| live.contains(id));
        }

        let selected = selected
            .filter(|id| self.order.contains(id))
            .or_else(|| self.presentation.as_ref().and_then(|state| state.selected))
            .filter(|id| self.order.contains(id))
            .or_else(|| self.order.first().copied());
        let selected_index = selected
            .and_then(|id| self.order.iter().position(|candidate| *candidate == id))
            .unwrap_or(0);
        let layout_key = (
            self.order.len(),
            display.origin.x,
            display.origin.y,
            display.size.w,
            display.size.h,
        );
        if self.layout_key != Some(layout_key) {
            self.layout_key = Some(layout_key);
            self.mode = aegis_model::window_switcher::mode(display, self.order.len());
            self.animated_cards.clear();
            self.animated_selection = None;
        }
        let target = aegis_model::window_switcher::layout_for_mode(
            display,
            self.order.len(),
            selected_index,
            self.mode,
        );
        let blend = if self.reduced_motion {
            1.0
        } else {
            1.0 - (-dt * SLIDE_RATE).exp()
        };
        let target_cards = target
            .cards
            .iter()
            .copied()
            .enumerate()
            .map(|(index, card)| {
                if index == selected_index {
                    preview::selected_geometry(card, PreviewSelectionStyle::Staged, &design)
                } else {
                    card
                }
            })
            .collect::<Vec<_>>();
        let mut cards = Vec::with_capacity(self.order.len());
        let mut cards_moving = false;
        for (id, target_card) in self.order.iter().copied().zip(target_cards.iter().copied()) {
            let current = self.animated_cards.entry(id).or_insert(target_card);
            if self.reduced_motion {
                *current = target_card;
            } else {
                // A circular list has one off-screen item whose logical slot
                // wraps from one tail to the other. Teleport only that distant
                // item behind the panel clip; every visible item, including
                // the incoming selection, moves exactly one adjacent step.
                if self.mode == aegis_model::window_switcher::Mode::Carousel {
                    let current_centre = current.outer.origin.x + current.outer.size.w / 2;
                    let target_centre = target_card.outer.origin.x + target_card.outer.size.w / 2;
                    if (current_centre - target_centre).abs() > target.panel.size.w / 2 {
                        *current = target_card;
                    }
                }
                *current = lerp_card(*current, target_card, blend);
            }
            cards_moving |= *current != target_card;
            if intersects(target.panel, current.outer) {
                cards.push(PreviewCard {
                    window: id,
                    geometry: *current,
                    corner_radius: design.radii.control,
                });
            }
        }
        self.animated_cards.retain(|id, _| self.order.contains(id));

        let target_selection = target_cards.get(selected_index).copied();
        let selection_moving = if let Some(target_selection) = target_selection {
            let current = self.animated_selection.get_or_insert(target_selection);
            if self.mode == aegis_model::window_switcher::Mode::Carousel || self.reduced_motion {
                *current = target_selection;
            } else {
                *current = lerp_card(*current, target_selection, blend);
            }
            *current != target_selection
        } else {
            self.animated_selection = None;
            false
        };
        self.anim_active |= cards_moving || selection_moving;
        if !cards_moving && !selection_moving {
            self.anim_active = (self.visibility - if self.open { 1.0 } else { 0.0 }).abs() > 0.002;
        }

        let presentation = WindowSwitcherPresentation {
            mode: self.mode,
            panel: target.panel,
            cards,
            selection_indicator: self.animated_selection,
            selected,
            inactive_content_brightness: design.preview.inactive_content_brightness,
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
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let display = aegis_model::Rect::new(
            0,
            0,
            input.as_raw().display_size.x.max(1.0) as i32,
            input.as_raw().display_size.y.max(1.0) as i32,
        );
        if self.presentation.is_none() {
            let order = self.order.clone();
            self.prepare(input, display, windows, &order, None);
        }
        let Some(presentation) = self.presentation.clone() else {
            return;
        };

        let raw = input.as_raw();
        let cursor = (raw.cursor.x, raw.cursor.y);
        let previous_hovered = self.hovered;
        self.hovered = self
            .open
            .then(|| {
                preview::hit_test(
                    &presentation.cards,
                    presentation.selected,
                    cursor.0,
                    cursor.1,
                )
            })
            .flatten();
        self.anim_active |= self.hovered != previous_hovered;
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

        let content_alpha = self.alpha(u8::MAX);
        let original_theme = frame.theme();
        frame.set_theme(original_theme.with_fg(original_theme.fg().with_alpha(content_alpha)));
        let design = self.design;
        let panel = to_lens(presentation.panel);
        let panel_material = preview::panel_material(&design, presentation.visibility);
        frame.place(
            "aegis-window-switcher-panel",
            &chrome_place(panel, panel_material),
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

        if let Some(indicator) = presentation.selection_indicator {
            let selection = to_lens(indicator.outer);
            frame.place(
                "aegis-window-switcher-selection",
                &chrome_place(
                    selection,
                    preview::card_material(
                        &design,
                        preview::PreviewCardState::Selected,
                        presentation.visibility,
                        design.radii.control,
                    ),
                ),
                |frame| {
                    frame.column_ex(&layer_size(selection.w, selection.h), |_| {});
                },
            );
        }

        let mut render_cards: Vec<_> = presentation.cards.iter().collect();
        render_cards.sort_by_key(|card| Some(card.window) == presentation.selected);
        for (index, presented) in render_cards.into_iter().enumerate() {
            let Some(window) = windows.iter().find(|window| window.id == presented.window) else {
                continue;
            };
            let selected = Some(window.id) == presentation.selected;
            let hovered = Some(window.id) == self.hovered && !window.read_only;
            let outer = to_lens(presented.geometry.outer);
            if hovered && !selected {
                frame.place(
                    &format!("aegis-window-switcher-card-{index}"),
                    &chrome_place(
                        outer,
                        preview::card_material(
                            &design,
                            preview::PreviewCardState::Hovered,
                            presentation.visibility,
                            presented.corner_radius,
                        ),
                    ),
                    |frame| {
                        frame.column_ex(&layer_size(outer.w, outer.h), |_| {});
                    },
                );
            }

            let label_rect = to_lens(presented.geometry.label);
            let title = window
                .title
                .as_deref()
                .or(window.app_id.as_deref())
                .unwrap_or_else(|| i18n.text(Message::UntitledWindow));
            let icon = window
                .app_id
                .as_deref()
                .and_then(|app_id| self.icons.get(&app_id.to_ascii_lowercase()));
            let occupied_width = 16.0 + if icon.is_some() { 20.0 + 7.0 } else { 0.0 };
            let item_opacity = if presentation.selected.is_none() || selected || hovered {
                1.0
            } else {
                presentation.inactive_content_brightness
            };
            let item_alpha = self.alpha((255.0 * item_opacity).round() as u8);
            let icon_tint = design.colors.application_text.with_alpha(item_alpha);
            frame.set_theme(original_theme.with_fg(original_theme.fg().with_alpha(item_alpha)));
            let label = ellipsize(frame, title, 11.5, (label_rect.w - occupied_width).max(0.0));
            frame.place(
                &format!("aegis-window-switcher-label-{index}"),
                &chrome_place(
                    label_rect,
                    LayoutOpts {
                        bg: Color::TRANSPARENT,
                        radius: design.radii.control,
                        ..surface_layout()
                    },
                ),
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
                                    frame.image_tinted(
                                        icon as *mut lens::sys::flux_image,
                                        20.0,
                                        20.0,
                                        icon_tint,
                                    );
                                }
                            }
                            frame.label_compact_sized(&label, 11.5);
                        },
                    );
                },
            );
        }
        frame.set_theme(original_theme);
    }

    fn prepare_window_switcher(
        &mut self,
        input: &Input,
        display: aegis_model::Rect,
        windows: &[Window],
        order: &[WindowId],
        selected: Option<WindowId>,
    ) -> Option<WindowSwitcherPresentation> {
        self.prepare(input, display, windows, order, selected)
    }

    fn captures_keyboard(&self) -> bool {
        self.open
    }

    fn key_char(&mut self, key: &KeyChar, out: &mut ChromeEvents) {
        if self.open && matches!(key_action(key.keysym, key.ch), KeyAction::Escape) {
            out.window_switcher_cancel = true;
            self.finish_window_switcher();
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
                    card.contains(x, y)
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
            .map(|presentation| vec![BackdropRegion::from(presentation.panel)])
            .unwrap_or_default()
    }

    fn liquid_glass_regions(
        &self,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<LiquidGlassRegion> {
        let design = self.design;
        self.presentation
            .as_ref()
            .filter(|presentation| presentation.visibility > 0.01)
            .map(|presentation| {
                vec![
                    LiquidGlassRegion::from_role(
                        &design,
                        GlassRole::FloatingPanel,
                        BackdropRegion::from(presentation.panel),
                        design.radii.glass_panel,
                        presentation.visibility,
                    )
                    .with_focus(presentation.selection_indicator.map(
                        |indicator| {
                            preview::focus_for_rect(indicator.outer, design.radii.control, &design)
                        },
                    )),
                ]
            })
            .unwrap_or_default()
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::ReducedMotion(reduced) => self.set_reduced_motion(reduced),
            ChromeUpdate::AppCatalog(catalog) => self.icons = catalog.icons.clone(),
            ChromeUpdate::Appearance(design) => self.design = *design,
            _ => {}
        }
    }

    fn command(&mut self, command: &ChromeCommand<'_>, _out: &mut ChromeEvents) {
        match command {
            ChromeCommand::StartWindowSwitcher => self.start_window_switcher(),
            ChromeCommand::FinishWindowSwitcher => self.finish_window_switcher(),
            _ => {}
        }
    }

    fn window_switcher_active(&self) -> bool {
        self.open || self.visibility > 0.01
    }
}

fn to_lens(rect: aegis_model::Rect) -> Rect {
    Rect {
        x: rect.origin.x as f32,
        y: rect.origin.y as f32,
        w: rect.size.w as f32,
        h: rect.size.h as f32,
    }
}

fn intersects(a: aegis_model::Rect, b: aegis_model::Rect) -> bool {
    a.origin.x < b.origin.x + b.size.w
        && b.origin.x < a.origin.x + a.size.w
        && a.origin.y < b.origin.y + b.size.h
        && b.origin.y < a.origin.y + a.size.h
}

fn lerp_i32(from: i32, to: i32, blend: f32) -> i32 {
    let value = from as f32 + (to - from) as f32 * blend;
    if (value - to as f32).abs() < 0.75 {
        to
    } else {
        value.round() as i32
    }
}

fn lerp_rect(from: aegis_model::Rect, to: aegis_model::Rect, blend: f32) -> aegis_model::Rect {
    aegis_model::Rect::new(
        lerp_i32(from.origin.x, to.origin.x, blend),
        lerp_i32(from.origin.y, to.origin.y, blend),
        lerp_i32(from.size.w, to.size.w, blend),
        lerp_i32(from.size.h, to.size.h, blend),
    )
}

fn lerp_card(
    from: aegis_model::window_switcher::Card,
    to: aegis_model::window_switcher::Card,
    blend: f32,
) -> aegis_model::window_switcher::Card {
    aegis_model::window_switcher::Card {
        outer: lerp_rect(from.outer, to.outer, blend),
        preview: lerp_rect(from.preview, to.preview, blend),
        label: lerp_rect(from.label, to.label, blend),
    }
}

fn layer_size(width: f32, height: f32) -> LayoutOpts {
    LayoutOpts {
        width,
        height,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn windows(count: u64) -> (Vec<Window>, Vec<WindowId>) {
        let order: Vec<_> = (1..=count).map(WindowId).collect();
        let windows = order.iter().copied().map(Window::new).collect();
        (windows, order)
    }

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
        let from = aegis_model::window_switcher::Card {
            outer: aegis_model::Rect::new(0, 10, 100, 80),
            preview: aegis_model::Rect::new(0, 10, 100, 50),
            label: aegis_model::Rect::new(0, 60, 100, 30),
        };
        let to = aegis_model::window_switcher::Card {
            outer: aegis_model::Rect::new(200, 10, 100, 80),
            preview: aegis_model::Rect::new(200, 10, 100, 50),
            label: aegis_model::Rect::new(200, 60, 100, 30),
        };
        let card = lerp_card(from, to, 0.25);
        assert_eq!(card.outer.origin.x, 50);
        assert!(card.outer.origin.x < to.outer.origin.x);
    }

    #[test]
    fn fixed_mode_stages_only_the_selected_card() {
        let mut switcher = WindowSwitcher::new();
        switcher.set_reduced_motion(true);
        switcher.start_window_switcher();
        let input = Input::default();
        let display = aegis_model::Rect::new(0, 0, 1920, 1080);
        let (windows, order) = windows(4);

        let first = switcher
            .prepare(&input, display, &windows, &order, Some(order[0]))
            .unwrap();
        let last = switcher
            .prepare(&input, display, &windows, &order, Some(order[3]))
            .unwrap();
        let base = aegis_model::window_switcher::fixed_layout(display, order.len());

        assert_eq!(first.mode, aegis_model::window_switcher::Mode::Fixed);
        let first_selected = first
            .cards
            .iter()
            .find(|card| card.window == order[0])
            .unwrap()
            .geometry;
        assert!(first_selected.outer.size.w > base.cards[0].outer.size.w);
        assert!(first_selected.outer.origin.y < base.cards[0].outer.origin.y);
        assert_eq!(
            last.cards
                .iter()
                .find(|card| card.window == order[0])
                .unwrap()
                .geometry,
            base.cards[0]
        );
        let last_selected = last
            .cards
            .iter()
            .find(|card| card.window == order[3])
            .unwrap()
            .geometry;
        assert_eq!(
            last_selected,
            preview::selected_geometry(
                base.cards[3],
                PreviewSelectionStyle::Staged,
                &Design::dark(),
            )
        );
        assert_eq!(last.selection_indicator.unwrap().outer, last_selected.outer);
        assert_eq!(
            last.inactive_content_brightness,
            Design::dark().preview.inactive_content_brightness
        );
    }

    #[test]
    fn carousel_wraps_through_the_adjacent_slot_with_a_centred_indicator() {
        let mut switcher = WindowSwitcher::new();
        switcher.set_reduced_motion(true);
        switcher.start_window_switcher();
        let input = Input::default();
        let display = aegis_model::Rect::new(0, 0, 1920, 1080);
        let (windows, order) = windows(5);

        let before = switcher
            .prepare(&input, display, &windows, &order, Some(order[4]))
            .unwrap();
        let after = switcher
            .prepare(&input, display, &windows, &order, Some(order[0]))
            .unwrap();
        let centre = display.size.w / 2;
        let incoming_before = before
            .cards
            .iter()
            .find(|card| card.window == order[0])
            .unwrap()
            .geometry
            .outer;
        let incoming_after = after
            .cards
            .iter()
            .find(|card| card.window == order[0])
            .unwrap()
            .geometry
            .outer;

        assert_eq!(after.mode, aegis_model::window_switcher::Mode::Carousel);
        assert!(incoming_before.origin.x + incoming_before.size.w / 2 > centre);
        assert_eq!(incoming_after.origin.x + incoming_after.size.w / 2, centre);
        let indicator = after.selection_indicator.unwrap().outer;
        assert_eq!(indicator.origin.x + indicator.size.w / 2, centre);
    }

    #[test]
    fn escape_requests_cancellation_without_selecting_a_window() {
        let mut switcher = WindowSwitcher::new();
        switcher.start_window_switcher();
        let mut out = ChromeEvents::default();
        switcher.key_char(
            &KeyChar {
                keysym: aegis_model::input::XKB_KEY_Escape,
                ch: None,
                mods: aegis_model::input::Mods::SUPER,
            },
            &mut out,
        );

        assert!(out.window_switcher_cancel);
        assert!(out.window_switcher_pick.is_none());
        assert!(!switcher.open);
    }

    #[test]
    fn visible_switcher_panel_is_declared_as_liquid_glass() {
        let mut switcher = WindowSwitcher::new();
        switcher.presentation = Some(WindowSwitcherPresentation {
            mode: aegis_model::window_switcher::Mode::Fixed,
            panel: aegis_model::Rect::new(200, 160, 640, 240),
            cards: Vec::new(),
            selection_indicator: None,
            selected: None,
            inactive_content_brightness: Design::dark().preview.inactive_content_brightness,
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
        assert_eq!(glass[0].corner_radius, Design::dark().radii.glass_panel);
        assert_eq!(glass[0].opacity, 0.6);
        let style = Design::dark().glass.floating_panel;
        assert_eq!(glass[0].shadow_alpha, style.shadow_alpha);
        assert_eq!(glass[0].shadow_blur, style.shadow_blur);
        assert_eq!(glass[0].shadow_offset_y, style.shadow_offset_y);
        assert!(glass[0].focus.is_none());
    }

    #[test]
    fn selected_switcher_card_is_a_focus_field_inside_the_panel_body() {
        let selected = WindowId(7);
        let card = aegis_model::window_switcher::Card {
            outer: aegis_model::Rect::new(240, 182, 220, 170),
            preview: aegis_model::Rect::new(240, 182, 220, 132),
            label: aegis_model::Rect::new(240, 314, 220, 38),
        };
        let mut switcher = WindowSwitcher::new();
        switcher.presentation = Some(WindowSwitcherPresentation {
            mode: aegis_model::window_switcher::Mode::Fixed,
            panel: aegis_model::Rect::new(200, 160, 640, 240),
            cards: vec![PreviewCard {
                window: selected,
                geometry: card,
                corner_radius: Design::dark().radii.control,
            }],
            selection_indicator: Some(card),
            selected: Some(selected),
            inactive_content_brightness: Design::dark().preview.inactive_content_brightness,
            visibility: 0.6,
        });
        let workspaces = WorkspaceSnapshot {
            outputs: Vec::new(),
        };

        let glass = switcher.liquid_glass_regions((1280.0, 720.0), &[], &workspaces);
        assert_eq!(glass.len(), 1);
        let focus = glass[0]
            .focus
            .expect("selected card should focus the panel");
        assert_eq!(focus.bounds, BackdropRegion::from(card.outer));
        assert_eq!(focus.corner_radius, Design::dark().radii.control);
        assert_eq!(focus.strength, Design::dark().glass_focus.field_strength);
    }
}
