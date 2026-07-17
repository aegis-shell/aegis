//! Overview grid geometry (M9): the pure layout math behind the unified
//! window/workspace picker, shared by the compositor's thumbnail pass and
//! the chrome's label and hit-test pass so both agree on every cell.
//!
//! The grid is a near-square arrangement of equal *slots*; each window's
//! thumbnail is then aspect-fitted inside its slot. No flux, lens, or
//! Wayland dependency, so the layout is unit-tested in isolation.

use crate::{Point, Rect, Size};

/// Gap between slots, in logical pixels.
pub const SLOT_GAP: i32 = 24;
/// Margin around the whole grid area, in logical pixels.
pub const GRID_MARGIN: i32 = 48;
/// Width of the workspace rail along the left edge, in logical pixels.
pub const RAIL_WIDTH: i32 = 120;
/// Height of one workspace rail tile, in logical pixels.
pub const RAIL_TILE_H: i32 = 64;
/// Gap between rail tiles, in logical pixels.
pub const RAIL_GAP: i32 = 12;

/// The area the thumbnail grid occupies within `display` (the full logical
/// output rect): the display minus the rail when `rail` is set. Shared by
/// the compositor's thumbnail pass and the chrome's hit-testing so both
/// agree on every cell.
pub fn grid_area(display: Rect, rail: bool) -> Rect {
    if rail {
        Rect {
            origin: Point {
                x: display.origin.x + RAIL_WIDTH,
                y: display.origin.y,
            },
            size: Size {
                w: (display.size.w - RAIL_WIDTH).max(1),
                h: display.size.h,
            },
        }
    } else {
        display
    }
}

/// Workspace rail tile rects along the left edge, vertically centered, in
/// workspace order. Shared by the thumbnail pass and hit-testing.
pub fn rail(display: Rect, count: usize) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    let x = display.origin.x + RAIL_GAP;
    let w = RAIL_WIDTH - 2 * RAIL_GAP;
    let total = count as i32 * RAIL_TILE_H + (count as i32 - 1) * RAIL_GAP;
    let mut y = display.origin.y + (display.size.h - total).max(RAIL_GAP) / 2;
    (0..count)
        .map(|_| {
            let tile = Rect::new(x, y, w, RAIL_TILE_H);
            y += RAIL_TILE_H + RAIL_GAP;
            tile
        })
        .collect()
}

/// Compute the slot rectangles for `count` thumbnails laid out in `area`.
/// Slots are ordered row-major and simply sequential — the caller pairs them
/// with its own z-ordered window list. Empty input yields no slots.
pub fn grid(area: Rect, count: usize) -> Vec<Rect> {
    if count == 0 || area.size.w <= 0 || area.size.h <= 0 {
        return Vec::new();
    }
    let columns = (count as f32).sqrt().ceil() as i32;
    let rows = (count as i32 + columns - 1) / columns;
    let inner = Rect::new(
        area.origin.x + GRID_MARGIN,
        area.origin.y + GRID_MARGIN,
        (area.size.w - 2 * GRID_MARGIN).max(1),
        (area.size.h - 2 * GRID_MARGIN).max(1),
    );
    let slot_w = ((inner.size.w - (columns - 1) * SLOT_GAP) / columns).max(1);
    let slot_h = ((inner.size.h - (rows - 1) * SLOT_GAP) / rows).max(1);
    // Center the used portion of the inner rect when the last row is short
    // and when the slots do not fill the area exactly.
    let used_w = columns * slot_w + (columns - 1) * SLOT_GAP;
    let used_h = rows * slot_h + (rows - 1) * SLOT_GAP;
    let start_x = inner.origin.x + (inner.size.w - used_w).max(0) / 2;
    let start_y = inner.origin.y + (inner.size.h - used_h).max(0) / 2;
    (0..count)
        .map(|i| {
            let col = i as i32 % columns;
            let row = i as i32 / columns;
            Rect::new(
                start_x + col * (slot_w + SLOT_GAP),
                start_y + row * (slot_h + SLOT_GAP),
                slot_w,
                slot_h,
            )
        })
        .collect()
}

/// Aspect-fit a thumbnail of `content` size inside `slot`, centered. A
/// zero/degenerate content dimension falls back to the slot itself so a
/// not-yet-mapped window still gets a stable cell.
pub fn fit(slot: Rect, content: Size) -> Rect {
    if content.w <= 0 || content.h <= 0 {
        return slot;
    }
    let scale = (slot.size.w as f32 / content.w as f32).min(slot.size.h as f32 / content.h as f32);
    let w = ((content.w as f32 * scale).round() as i32).max(1);
    let h = ((content.h as f32 * scale).round() as i32).max(1);
    Rect {
        origin: Point {
            x: slot.origin.x + (slot.size.w - w) / 2,
            y: slot.origin.y + (slot.size.h - h) / 2,
        },
        size: Size { w, h },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_and_degenerate_inputs() {
        assert!(grid(Rect::new(0, 0, 800, 600), 0).is_empty());
        assert!(grid(Rect::new(0, 0, 0, 600), 3).is_empty());
    }

    #[test]
    fn slots_cover_and_stay_inside() {
        let area = Rect::new(0, 0, 1280, 800);
        for count in 1..=9 {
            let slots = grid(area, count);
            assert_eq!(slots.len(), count);
            for slot in &slots {
                assert!(slot.origin.x >= area.origin.x);
                assert!(slot.origin.y >= area.origin.y);
                assert!(slot.origin.x + slot.size.w <= area.origin.x + area.size.w);
                assert!(slot.origin.y + slot.size.h <= area.origin.y + area.size.h);
                assert!(slot.size.w > 0 && slot.size.h > 0);
            }
        }
    }

    #[test]
    fn grid_shape_is_near_square() {
        // 4 windows → 2x2; 5 → 3 cols x 2 rows.
        let four = grid(Rect::new(0, 0, 1000, 1000), 4);
        let xs: std::collections::BTreeSet<i32> = four.iter().map(|r| r.origin.x).collect();
        let ys: std::collections::BTreeSet<i32> = four.iter().map(|r| r.origin.y).collect();
        assert_eq!(xs.len(), 2);
        assert_eq!(ys.len(), 2);
        let five = grid(Rect::new(0, 0, 1000, 1000), 5);
        let xs: std::collections::BTreeSet<i32> = five.iter().map(|r| r.origin.x).collect();
        assert_eq!(xs.len(), 3);
    }

    #[test]
    fn fit_preserves_aspect_and_centers() {
        let slot = Rect::new(100, 100, 400, 300);
        let wide = fit(slot, Size { w: 1600, h: 900 });
        assert_eq!(wide.size.w, 400);
        assert_eq!(wide.size.h, 225);
        assert_eq!(wide.origin.x, 100);
        assert_eq!(wide.origin.y, 100 + (300 - 225) / 2);

        let tall = fit(slot, Size { w: 900, h: 1600 });
        assert_eq!(tall.size.h, 300);
        assert_eq!(tall.size.w, 169);
        assert_eq!(tall.origin.x, 100 + (400 - 169) / 2);
        assert_eq!(tall.origin.y, 100);
    }

    #[test]
    fn fit_degenerate_content_gets_the_slot() {
        let slot = Rect::new(5, 6, 400, 300);
        assert_eq!(fit(slot, Size { w: 0, h: 0 }), slot);
    }

    #[test]
    fn grid_area_shaves_the_rail() {
        let display = Rect::new(0, 0, 1000, 700);
        assert_eq!(grid_area(display, false), display);
        let with_rail = grid_area(display, true);
        assert_eq!(with_rail.origin.x, RAIL_WIDTH);
        assert_eq!(with_rail.size.w, 1000 - RAIL_WIDTH);
        assert_eq!(with_rail.size.h, 700);
    }

    #[test]
    fn rail_tiles_stack_centered() {
        let display = Rect::new(0, 0, 1000, 700);
        assert!(rail(display, 0).is_empty());
        let tiles = rail(display, 3);
        assert_eq!(tiles.len(), 3);
        assert!(tiles[0].origin.x >= display.origin.x);
        assert!(tiles[2].origin.y + tiles[2].size.h <= display.size.h);
        // Vertically centered as a block.
        let block = 3 * RAIL_TILE_H + 2 * RAIL_GAP;
        assert_eq!(tiles[0].origin.y, (700 - block) / 2);
        // Even spacing.
        assert_eq!(
            tiles[1].origin.y - tiles[0].origin.y,
            RAIL_TILE_H + RAIL_GAP
        );
    }
}
