//! Shared presentation contract for compositor-rendered live previews.
//!
//! Components own layout, animation, and interaction lifecycles. This module
//! owns the common card model and the pure derivations that must agree across
//! compositor scene rendering, shell chrome, hit-testing, and Liquid Glass
//! focus.

use aegis_design::{Design, PreviewSelectionStyle, materials};
use aegis_model::window::WindowId;
use lens::{Color, LayoutOpts};

use crate::{BackdropRegion, LiquidGlassFocus};

/// One live client-preview card shared by compositor and chrome rendering.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PreviewCard {
    pub window: WindowId,
    pub geometry: aegis_model::window_switcher::Card,
    pub corner_radius: f32,
}

impl PreviewCard {
    #[must_use]
    pub fn contains(self, x: f32, y: f32) -> bool {
        let rect = self.geometry.outer;
        x >= rect.origin.x as f32
            && y >= rect.origin.y as f32
            && x < (rect.origin.x + rect.size.w) as f32
            && y < (rect.origin.y + rect.size.h) as f32
    }
}

/// Shared switcher presentation prepared once per frame.
///
/// The executable uses these exact targets for live client previews and the
/// shell uses them for focus, labels, hit-testing, and animation. Keeping a
/// single snapshot prevents the chrome and client scene from drifting apart.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowSwitcherPresentation {
    pub mode: aegis_model::window_switcher::Mode,
    pub panel: aegis_model::Rect,
    pub cards: Vec<PreviewCard>,
    /// Independently animated focus geometry. In fixed mode this moves
    /// between cards; in carousel mode it remains at the output centre.
    pub selection_indicator: Option<aegis_model::window_switcher::Card>,
    pub selected: Option<WindowId>,
    pub inactive_content_brightness: f32,
    pub visibility: f32,
}

/// One compositor-rendered live-preview popover contributed by ordinary
/// chrome, such as the group of running windows above a hovered Dock tile.
#[derive(Debug, Clone, PartialEq)]
pub struct LivePreviewPresentation {
    pub panel: aegis_model::Rect,
    pub cards: Vec<PreviewCard>,
    pub focused: Option<WindowId>,
    pub inactive_content_brightness: f32,
    pub visibility: f32,
}

/// Visual state of content hosted inside one preview panel body.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PreviewCardState {
    Rest,
    Hovered,
    Selected,
}

/// Minimal painted foreground for the parent preview panel.
#[must_use]
pub fn panel_material(design: &Design, visibility: f32) -> LayoutOpts {
    let mut material = materials::glass_panel(design);
    material.bg = scale_alpha(material.bg, visibility);
    material.radius = design.radii.glass_panel;
    material
}

/// Shared foreground treatment for one preview card.
#[must_use]
pub fn card_material(
    design: &Design,
    state: PreviewCardState,
    visibility: f32,
    corner_radius: f32,
) -> LayoutOpts {
    let mut material = match state {
        PreviewCardState::Rest => materials::surface_layout(),
        PreviewCardState::Hovered => materials::glass_focus(design, false, visibility),
        PreviewCardState::Selected => materials::glass_focus(design, true, visibility),
    };
    material.radius = corner_radius;
    material.pad = 0.0;
    material
}

fn scale_alpha(color: Color, opacity: f32) -> Color {
    let (_, _, _, alpha) = color.components();
    color.with_alpha((alpha as f32 * opacity.clamp(0.0, 1.0)).round() as u8)
}

/// Resolve the topmost card under a point, optionally preferring a staged
/// card that may overlap its stationary siblings.
#[must_use]
pub fn hit_test(
    cards: &[PreviewCard],
    preferred: Option<WindowId>,
    x: f32,
    y: f32,
) -> Option<WindowId> {
    preferred
        .and_then(|preferred| {
            cards
                .iter()
                .find(|card| card.window == preferred && card.contains(x, y))
        })
        .or_else(|| cards.iter().rev().find(|card| card.contains(x, y)))
        .map(|card| card.window)
}

/// Derive the parent's single optical focus field from a preview card.
#[must_use]
pub fn focus_field(
    cards: &[PreviewCard],
    focused: Option<WindowId>,
    design: &Design,
) -> Option<LiquidGlassFocus> {
    let card = cards.iter().find(|card| Some(card.window) == focused)?;
    Some(focus_for_rect(
        card.geometry.outer,
        card.corner_radius,
        design,
    ))
}

/// Build an optical focus field for independently animated selection
/// geometry, such as the window switcher's moving indicator.
#[must_use]
pub fn focus_for_rect(
    bounds: aegis_model::Rect,
    corner_radius: f32,
    design: &Design,
) -> LiquidGlassFocus {
    LiquidGlassFocus {
        bounds: BackdropRegion::from(bounds),
        corner_radius,
        strength: design.glass_focus.field_strength,
    }
}

/// Keep preview pixels opaque and express hierarchy through brightness.
#[must_use]
pub fn content_brightness(focused: Option<WindowId>, candidate: WindowId, inactive: f32) -> f32 {
    if focused.is_none() || focused == Some(candidate) {
        1.0
    } else {
        inactive.clamp(0.0, 1.0)
    }
}

/// Apply a named selection treatment while preserving the shared card
/// geometry contract.
#[must_use]
pub fn selected_geometry(
    card: aegis_model::window_switcher::Card,
    style: PreviewSelectionStyle,
    design: &Design,
) -> aegis_model::window_switcher::Card {
    let selection = design.preview.selection(style);
    if selection.scale == 1.0 && selection.lift == 0.0 {
        return card;
    }
    let scale = |value: i32| ((value as f32 * selection.scale).round() as i32).max(1);
    let width = scale(card.outer.size.w);
    let preview_height = scale(card.preview.size.h);
    let label_height = card.label.size.h;
    let height = preview_height + label_height;
    let centre_x = card.outer.origin.x + card.outer.size.w / 2;
    let x = centre_x - width / 2;
    let y = card.outer.origin.y - (height - card.outer.size.h) / 2 - selection.lift.round() as i32;
    aegis_model::window_switcher::Card {
        outer: aegis_model::Rect::new(x, y, width, height),
        preview: aegis_model::Rect::new(x, y, width, preview_height),
        label: aegis_model::Rect::new(x, y + preview_height, width, label_height),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn card(window: u64, x: i32) -> PreviewCard {
        PreviewCard {
            window: WindowId(window),
            geometry: aegis_model::window_switcher::Card {
                outer: aegis_model::Rect::new(x, 10, 100, 70),
                preview: aegis_model::Rect::new(x, 10, 100, 50),
                label: aegis_model::Rect::new(x, 60, 100, 20),
            },
            corner_radius: Design::dark().radii.control,
        }
    }

    #[test]
    fn preferred_card_wins_in_an_overlap() {
        let cards = [card(1, 0), card(2, 50)];
        assert_eq!(
            hit_test(&cards, Some(WindowId(1)), 75.0, 30.0),
            Some(WindowId(1))
        );
        assert_eq!(hit_test(&cards, None, 75.0, 30.0), Some(WindowId(2)));
    }

    #[test]
    fn focus_uses_the_complete_card_target() {
        let design = Design::dark();
        let cards = [card(7, 20)];
        let focus = focus_field(&cards, Some(WindowId(7)), &design).unwrap();
        assert_eq!(focus.bounds, BackdropRegion::from(cards[0].geometry.outer));
        assert_eq!(focus.corner_radius, design.radii.control);
        assert_eq!(focus.strength, design.glass_focus.field_strength);
    }

    #[test]
    fn staged_selection_scales_and_lifts_without_resizing_the_label() {
        let design = Design::dark();
        let base = card(1, 100).geometry;
        let staged = selected_geometry(base, PreviewSelectionStyle::Staged, &design);
        assert!(staged.outer.size.w > base.outer.size.w);
        assert!(staged.outer.origin.y < base.outer.origin.y);
        assert_eq!(staged.label.size.h, base.label.size.h);
        assert_eq!(
            selected_geometry(base, PreviewSelectionStyle::Focused, &design),
            base
        );
    }

    #[test]
    fn card_states_stay_inside_the_parent_material() {
        let design = Design::dark();
        let rest = card_material(&design, PreviewCardState::Rest, 1.0, 9.0);
        let hovered = card_material(&design, PreviewCardState::Hovered, 1.0, 9.0);
        let selected = card_material(&design, PreviewCardState::Selected, 0.5, 9.0);
        assert_eq!(rest.bg, Color::TRANSPARENT);
        assert_eq!(hovered.bg, design.glass_focus.hover_tint);
        assert_eq!(selected.bg, Color::rgba(255, 255, 255, 2));
        assert_eq!(hovered.border_width, 0.0);
        assert_eq!(selected.border_width, 0.0);
        assert_eq!(selected.radius, 9.0);
    }
}
