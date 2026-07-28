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
}
