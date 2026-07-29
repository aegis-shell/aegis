//! Shared geometry for the compositor-owned Super+Tab window switcher.
//!
//! The renderer uses each card's preview rect for live client surfaces while
//! shell chrome uses the same cards for borders and labels.

use crate::Rect;

const OUTER_MARGIN: i32 = 32;
const PANEL_PAD: i32 = 22;
const CARD_GAP: i32 = 14;
const CARD_MAX_W: i32 = 238;
const CARD_MIN_W: i32 = 88;
const CARD_LABEL_H: i32 = 38;
const PREVIEW_ASPECT: f32 = 0.64;

/// One window card in the switcher strip.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Card {
    /// Complete card including preview and label.
    pub outer: Rect,
    /// Live client-surface target.
    pub preview: Rect,
    /// Title/application label below the preview.
    pub label: Rect,
}

/// Centred switcher panel and its one-to-one window cards.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Layout {
    pub panel: Rect,
    pub cards: Vec<Card>,
}

/// Lay out the switcher as a carousel whose selected card is always centred.
///
/// `cards` stays in caller order; only its positions rotate. This lets the
/// shell and compositor keep addressing previews by stable window id while
/// the visual focus remains fixed at the centre of the output.
pub fn carousel_layout(display: Rect, windows: usize, selected: usize) -> Layout {
    let mut layout = layout(display, windows);
    if windows == 0 {
        return layout;
    }

    let selected = selected.min(windows - 1);
    let initial_card_w = layout.cards[0].outer.size.w;
    let gap = layout
        .cards
        .first()
        .map(|card| {
            if windows > 1 {
                (layout.panel.size.w - PANEL_PAD * 2 - card.outer.size.w * windows as i32)
                    / windows.saturating_sub(1) as i32
            } else {
                0
            }
        })
        .unwrap_or(0);
    let centre_x = display.origin.x + display.size.w / 2;
    let half_space = (centre_x - display.origin.x)
        .min(display.origin.x + display.size.w - centre_x)
        - OUTER_MARGIN;
    let furthest = (windows / 2) as i32;
    let max_centred_w = if furthest == 0 {
        initial_card_w
    } else {
        ((half_space - PANEL_PAD - furthest * gap) * 2 / (furthest * 2 + 1)).max(1)
    };
    let card_w = initial_card_w.min(max_centred_w);
    let step = card_w + gap;
    let preview_h = ((card_w as f32 * PREVIEW_ASPECT).round() as i32).max(56);
    let card_h = preview_h + CARD_LABEL_H;
    let centre_y = display.origin.y + display.size.h / 2;
    let card_y = centre_y - card_h / 2;

    for (index, card) in layout.cards.iter_mut().enumerate() {
        let forward = (index + windows - selected) % windows;
        let offset = if forward <= windows / 2 {
            forward as i32
        } else {
            forward as i32 - windows as i32
        };
        let x = centre_x - card_w / 2 + offset * step;
        *card = Card {
            outer: Rect::new(x, card_y, card_w, card_h),
            preview: Rect::new(x, card_y, card_w, preview_h),
            label: Rect::new(x, card_y + preview_h, card_w, CARD_LABEL_H),
        };
    }

    let left = layout
        .cards
        .iter()
        .map(|card| card.outer.origin.x)
        .min()
        .unwrap_or(centre_x);
    let right = layout
        .cards
        .iter()
        .map(|card| card.outer.origin.x + card.outer.size.w)
        .max()
        .unwrap_or(centre_x);
    layout.panel = Rect::new(
        left - PANEL_PAD,
        card_y - PANEL_PAD,
        right - left + PANEL_PAD * 2,
        card_h + PANEL_PAD * 2,
    );
    layout
}

/// Lay out one horizontal card per visible window, shrinking cards as needed
/// to keep the complete strip on the output.
pub fn layout(display: Rect, windows: usize) -> Layout {
    if windows == 0 {
        let panel = Rect::new(
            display.origin.x + display.size.w / 2 - 100,
            display.origin.y + display.size.h / 2 - 40,
            200,
            80,
        );
        return Layout {
            panel,
            cards: Vec::new(),
        };
    }

    let available_w = (display.size.w - OUTER_MARGIN * 2).max(CARD_MIN_W + PANEL_PAD * 2);
    let content_w = (available_w - PANEL_PAD * 2).max(windows as i32);
    let gap = if windows > 1 {
        CARD_GAP.min(
            ((content_w - CARD_MIN_W * windows as i32).max(0) / windows.saturating_sub(1) as i32)
                .max(0),
        )
    } else {
        0
    };
    let gaps = gap * windows.saturating_sub(1) as i32;
    let card_w = ((content_w - gaps) / windows as i32).clamp(1, CARD_MAX_W);
    let preview_h = ((card_w as f32 * PREVIEW_ASPECT).round() as i32).max(56);
    let card_h = preview_h + CARD_LABEL_H;
    let panel_w = card_w * windows as i32 + gaps + PANEL_PAD * 2;
    let panel_h = card_h + PANEL_PAD * 2;
    let panel = Rect::new(
        display.origin.x + (display.size.w - panel_w) / 2,
        display.origin.y + (display.size.h - panel_h) / 2,
        panel_w,
        panel_h,
    );

    let cards = (0..windows)
        .map(|index| {
            let x = panel.origin.x + PANEL_PAD + index as i32 * (card_w + gap);
            let y = panel.origin.y + PANEL_PAD;
            Card {
                outer: Rect::new(x, y, card_w, card_h),
                preview: Rect::new(x, y, card_w, preview_h),
                label: Rect::new(x, y + preview_h, card_w, CARD_LABEL_H),
            }
        })
        .collect();
    Layout { panel, cards }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switcher_cards_are_centred_and_stay_on_screen() {
        let display = Rect::new(0, 0, 1920, 1080);
        let layout = layout(display, 5);
        assert_eq!(layout.cards.len(), 5);
        assert!(layout.panel.origin.x >= 0);
        assert!(layout.panel.origin.y >= 0);
        assert!(layout.panel.origin.x + layout.panel.size.w <= display.size.w);
        assert!(layout.panel.origin.y + layout.panel.size.h <= display.size.h);
        for card in &layout.cards {
            assert_eq!(card.preview.size.w, card.outer.size.w);
            assert_eq!(card.preview.size.h + card.label.size.h, card.outer.size.h);
        }
    }

    #[test]
    fn many_windows_shrink_instead_of_overflowing() {
        let display = Rect::new(0, 0, 1280, 720);
        let layout = layout(display, 10);
        assert_eq!(layout.cards.len(), 10);
        assert!(layout.panel.origin.x >= 0);
        assert!(layout.panel.origin.x + layout.panel.size.w <= display.size.w);
    }

    #[test]
    fn carousel_keeps_every_selection_at_the_visual_centre() {
        let display = Rect::new(0, 0, 1920, 1080);
        for selected in 0..5 {
            let layout = carousel_layout(display, 5, selected);
            let card = layout.cards[selected];
            assert_eq!(
                card.outer.origin.x + card.outer.size.w / 2,
                display.size.w / 2
            );
            assert_eq!(
                card.outer.origin.y + card.outer.size.h / 2,
                display.size.h / 2
            );
        }
    }

    #[test]
    fn carousel_panel_stays_on_screen_for_even_window_counts() {
        let display = Rect::new(0, 0, 1280, 720);
        for selected in 0..10 {
            let layout = carousel_layout(display, 10, selected);
            assert!(layout.panel.origin.x >= 0);
            assert!(layout.panel.origin.x + layout.panel.size.w <= display.size.w);
        }
    }
}
