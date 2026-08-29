//! Overview grid geometry (M9): the pure layout math behind the unified
//! window/workspace picker, shared by the compositor's thumbnail pass and
//! the chrome's label and hit-test pass so both agree on every cell.
//!
//! The grid is a near-square arrangement of equal *slots*; each window's
//! thumbnail is then aspect-fitted inside its slot. Windows claim the slot
//! nearest their on-screen center (`assign_slots`), so the picker keeps a
//! spatial echo of the real desktop. No flux, lens, or Wayland dependency,
//! so the layout is unit-tested in isolation.

use crate::{Point, Rect, Size};

/// Gap between slots, in logical pixels.
pub const SLOT_GAP: i32 = 24;
/// Margin around the whole grid area, in logical pixels.
pub const GRID_MARGIN: i32 = 48;
/// Height of the workspace rail along the top edge, in logical pixels.
pub const RAIL_HEIGHT: i32 = 132;
/// Width of one workspace rail tile, in logical pixels.
pub const RAIL_TILE_W: i32 = 160;
/// Gap between rail tiles, in logical pixels.
pub const RAIL_GAP: i32 = 12;
/// Caption strip reserved at the bottom of a rail tile, in logical pixels.
pub const RAIL_LABEL_H: i32 = 22;
/// Padding between a rail tile's border and its live content, in logical
/// pixels.
pub const RAIL_TILE_PAD: i32 = 6;
/// Margin inside a rail tile's content area for the miniature grid.
pub const TILE_GRID_MARGIN: i32 = 4;
/// Gap between miniature slots inside a rail tile.
pub const TILE_GRID_GAP: i32 = 4;
/// Width reserved for the Interaction Domain authority shelf along the right edge.
pub const INTERACTION_DOMAIN_SHELF_WIDTH: i32 = 188;
/// Height of one Interaction Domain transfer target.
pub const INTERACTION_DOMAIN_TILE_H: i32 = 74;
/// Gap between Interaction Domain targets.
pub const INTERACTION_DOMAIN_GAP: i32 = 12;

/// The area the thumbnail grid occupies within `display` (the full logical
/// output rect): the display minus the top rail when `rail` is set. Shared by
/// the compositor's thumbnail pass and the chrome's hit-testing so both
/// agree on every cell.
pub fn grid_area(display: Rect, rail: bool) -> Rect {
    grid_area_with_interaction_domain_shelf(display, rail, false)
}

/// Thumbnail area with independent workspace and Interaction Domain shelves. Keeping this
/// geometry in core lets the compositor's client-texture pass and the shell's
/// hit testing use exactly the same cells.
pub fn grid_area_with_interaction_domain_shelf(
    display: Rect,
    rail: bool,
    interaction_domain_shelf: bool,
) -> Rect {
    let top = if rail { RAIL_HEIGHT } else { 0 };
    let right = if interaction_domain_shelf {
        INTERACTION_DOMAIN_SHELF_WIDTH
    } else {
        0
    };
    Rect {
        origin: Point {
            x: display.origin.x,
            y: display.origin.y + top,
        },
        size: Size {
            w: (display.size.w - right).max(1),
            h: (display.size.h - top).max(1),
        },
    }
}

/// Workspace rail tile rects along the top edge, horizontally centered as a
/// block, in workspace order. Shared by the thumbnail pass and hit-testing.
pub fn rail(display: Rect, count: usize) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    let y = display.origin.y + RAIL_GAP;
    let h = RAIL_HEIGHT - 2 * RAIL_GAP;
    let total = count as i32 * RAIL_TILE_W + (count as i32 - 1) * RAIL_GAP;
    let mut x = display.origin.x + (display.size.w - total).max(RAIL_GAP) / 2;
    (0..count)
        .map(|_| {
            let tile = Rect::new(x, y, RAIL_TILE_W, h);
            x += RAIL_TILE_W + RAIL_GAP;
            tile
        })
        .collect()
}

/// The live-content area of a rail tile: the tile minus its padding and the
/// caption strip at the bottom. The compositor draws the workspace's
/// miniature thumbnails here while the chrome draws the caption underneath.
pub fn tile_content(tile: Rect) -> Rect {
    Rect {
        origin: Point {
            x: tile.origin.x + RAIL_TILE_PAD,
            y: tile.origin.y + RAIL_TILE_PAD,
        },
        size: Size {
            w: (tile.size.w - 2 * RAIL_TILE_PAD).max(1),
            h: (tile.size.h - RAIL_LABEL_H - 2 * RAIL_TILE_PAD).max(1),
        },
    }
}

/// Interaction Domain authority transfer targets along the right edge. The human desktop
/// and every live agent Interaction Domain use the same geometry so a controlled window can
/// be dragged in either direction.
pub fn interaction_domain_shelf(display: Rect, count: usize) -> Vec<Rect> {
    if count == 0 {
        return Vec::new();
    }
    let x =
        display.origin.x + display.size.w - INTERACTION_DOMAIN_SHELF_WIDTH + INTERACTION_DOMAIN_GAP;
    let w = INTERACTION_DOMAIN_SHELF_WIDTH - 2 * INTERACTION_DOMAIN_GAP;
    let total =
        count as i32 * INTERACTION_DOMAIN_TILE_H + (count as i32 - 1) * INTERACTION_DOMAIN_GAP;
    let mut y = display.origin.y + (display.size.h - total).max(INTERACTION_DOMAIN_GAP) / 2;
    (0..count)
        .map(|_| {
            let tile = Rect::new(x, y, w, INTERACTION_DOMAIN_TILE_H);
            y += INTERACTION_DOMAIN_TILE_H + INTERACTION_DOMAIN_GAP;
            tile
        })
        .collect()
}

/// Compute the slot rectangles for `count` thumbnails laid out in `area`.
/// Slots are ordered row-major and simply sequential — the caller pairs them
/// with its own z-ordered window list. Empty input yields no slots.
pub fn grid(area: Rect, count: usize) -> Vec<Rect> {
    grid_with_spacing(area, count, GRID_MARGIN, SLOT_GAP)
}

/// `grid` with an explicit margin and gap, for miniature grids inside rail
/// tiles where the default spacing would swallow the tile.
pub fn grid_with_spacing(area: Rect, count: usize, margin: i32, gap: i32) -> Vec<Rect> {
    if count == 0 || area.size.w <= 0 || area.size.h <= 0 {
        return Vec::new();
    }
    let columns = (count as f32).sqrt().ceil() as i32;
    let rows = (count as i32 + columns - 1) / columns;
    let inner = Rect::new(
        area.origin.x + margin,
        area.origin.y + margin,
        (area.size.w - 2 * margin).max(1),
        (area.size.h - 2 * margin).max(1),
    );
    let slot_w = ((inner.size.w - (columns - 1) * gap) / columns).max(1);
    let slot_h = ((inner.size.h - (rows - 1) * gap) / rows).max(1);
    // Center the used portion of the inner rect when the last row is short
    // and when the slots do not fill the area exactly.
    let used_w = columns * slot_w + (columns - 1) * gap;
    let used_h = rows * slot_h + (rows - 1) * gap;
    let start_x = inner.origin.x + (inner.size.w - used_w).max(0) / 2;
    let start_y = inner.origin.y + (inner.size.h - used_h).max(0) / 2;
    (0..count)
        .map(|i| {
            let col = i as i32 % columns;
            let row = i as i32 / columns;
            Rect::new(
                start_x + col * (slot_w + gap),
                start_y + row * (slot_h + gap),
                slot_w,
                slot_h,
            )
        })
        .collect()
}

/// Assign each window the grid slot nearest its current center, so a
/// thumbnail lands close to the window's on-screen position and the picker
/// keeps a spatial echo of the real desktop (Compiz/Gala-style closest-slot
/// assignment, ADR-0116). Each pass claims the globally closest free
/// (window, slot) pair, so the result is deterministic for a given input —
/// both the compositor's thumbnail pass and the chrome's hit-testing must
/// call this with the same window list in the same order. The returned
/// pairs are in input order.
pub fn assign_slots(
    area: Rect,
    windows: &[(crate::window::WindowId, Rect)],
) -> Vec<(crate::window::WindowId, Rect)> {
    let slots = grid(area, windows.len());
    if slots.is_empty() {
        return Vec::new();
    }
    let mut taken = vec![false; slots.len()];
    let mut assigned: Vec<Option<Rect>> = vec![None; windows.len()];
    let mut pending: Vec<usize> = (0..windows.len()).collect();
    while !pending.is_empty() {
        // The globally closest free pair claims its slot first; ties break
        // toward the earlier window and the earlier slot, keeping the walk
        // deterministic.
        let mut best: Option<(usize, usize, i64)> = None;
        for (pos, &wi) in pending.iter().enumerate() {
            let (wx, wy) = center(windows[wi].1);
            for (si, slot) in slots.iter().enumerate() {
                if taken[si] {
                    continue;
                }
                let (sx, sy) = center(*slot);
                let d2 = i64::from(wx - sx) * i64::from(wx - sx)
                    + i64::from(wy - sy) * i64::from(wy - sy);
                if best.is_none_or(|(_, _, best_d2)| d2 < best_d2) {
                    best = Some((pos, si, d2));
                }
            }
        }
        let Some((pos, si, _)) = best else {
            break;
        };
        let wi = pending.remove(pos);
        taken[si] = true;
        assigned[wi] = Some(slots[si]);
    }
    windows
        .iter()
        .enumerate()
        .filter_map(|(i, (id, _))| assigned[i].map(|slot| (*id, slot)))
        .collect()
}

fn center(rect: Rect) -> (i32, i32) {
    (
        rect.origin.x + rect.size.w / 2,
        rect.origin.y + rect.size.h / 2,
    )
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

/// The thumbnail rect during the reveal fly-in: the window's real geometry
/// (`window`) at `t = 0`, its aspect-fitted grid cell at `t = 1`, linearly
/// interpolated between. The compositor's thumbnail pass and the chrome's
/// cell frames, labels, and hit-testing must all resolve geometry through
/// this one function so a cell's border tracks its flying thumbnail exactly
/// instead of sitting at the final grid position from the first frame.
pub fn animated_cell(slot: Rect, window: Rect, t: f32) -> Rect {
    let cell = fit(slot, window.size);
    if t >= 1.0 {
        return cell;
    }
    lerp_rect(window, cell, t)
}

/// Linear interpolation between two rects, used by the overview fly-in to
/// move each thumbnail from the window's real geometry to its grid cell.
fn lerp_rect(from: Rect, to: Rect, t: f32) -> Rect {
    let l = |a: i32, b: i32| (a as f32 + (b - a) as f32 * t).round() as i32;
    Rect::new(
        l(from.origin.x, to.origin.x),
        l(from.origin.y, to.origin.y),
        l(from.size.w, to.size.w).max(1),
        l(from.size.h, to.size.h).max(1),
    )
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
        assert_eq!(with_rail.origin.y, RAIL_HEIGHT);
        assert_eq!(with_rail.size.w, 1000);
        assert_eq!(with_rail.size.h, 700 - RAIL_HEIGHT);
        let with_both = grid_area_with_interaction_domain_shelf(display, true, true);
        assert_eq!(with_both.origin.y, RAIL_HEIGHT);
        assert_eq!(with_both.size.w, 1000 - INTERACTION_DOMAIN_SHELF_WIDTH);
        assert_eq!(with_both.size.h, 700 - RAIL_HEIGHT);
    }

    #[test]
    fn rail_tiles_line_up_along_the_top() {
        let display = Rect::new(0, 0, 1000, 700);
        assert!(rail(display, 0).is_empty());
        let tiles = rail(display, 3);
        assert_eq!(tiles.len(), 3);
        assert!(tiles[0].origin.y >= display.origin.y);
        assert!(tiles[2].origin.x + tiles[2].size.w <= display.size.w);
        // Horizontally centered as a block.
        let block = 3 * RAIL_TILE_W + 2 * RAIL_GAP;
        assert_eq!(tiles[0].origin.x, (1000 - block) / 2);
        // Even spacing.
        assert_eq!(
            tiles[1].origin.x - tiles[0].origin.x,
            RAIL_TILE_W + RAIL_GAP
        );
        // Every tile leaves room for its caption strip.
        for tile in &tiles {
            let content = tile_content(*tile);
            assert!(content.origin.x >= tile.origin.x);
            assert!(content.origin.y >= tile.origin.y);
            assert_eq!(
                content.size.h,
                tile.size.h - RAIL_LABEL_H - 2 * RAIL_TILE_PAD
            );
        }
    }

    #[test]
    fn mini_grid_uses_custom_spacing() {
        let area = Rect::new(0, 0, 148, 80);
        let slots = grid_with_spacing(area, 4, 4, 2);
        assert_eq!(slots.len(), 4);
        for slot in &slots {
            assert!(slot.origin.x >= area.origin.x);
            assert!(slot.origin.y >= area.origin.y);
            assert!(slot.origin.x + slot.size.w <= area.origin.x + area.size.w);
            assert!(slot.origin.y + slot.size.h <= area.origin.y + area.size.h);
            assert!(slot.size.w > 0 && slot.size.h > 0);
        }
        // grid() is grid_with_spacing with the default constants.
        let big = Rect::new(0, 0, 1280, 800);
        assert_eq!(
            grid(big, 5),
            grid_with_spacing(big, 5, GRID_MARGIN, SLOT_GAP)
        );
    }

    #[test]
    fn assign_slots_is_a_deterministic_permutation() {
        let area = Rect::new(0, 0, 1280, 800);
        let id = |n: u64| crate::window::WindowId(n);
        let windows: Vec<(crate::window::WindowId, Rect)> = (0..6)
            .map(|n| {
                (
                    id(n),
                    Rect::new(10 + n as i32 * 37, 20 + n as i32 * 53, 800, 600),
                )
            })
            .collect();
        let first = assign_slots(area, &windows);
        let second = assign_slots(area, &windows);
        assert_eq!(first, second);
        assert_eq!(first.len(), windows.len());
        // Every grid slot is used exactly once and input order is kept.
        let plain = grid(area, windows.len());
        let mut assigned = first.clone();
        assigned.sort_by_key(|(_, slot)| (slot.origin.y, slot.origin.x));
        let mut expected = plain;
        expected.sort_by_key(|slot| (slot.origin.y, slot.origin.x));
        assert_eq!(
            assigned.iter().map(|(_, s)| *s).collect::<Vec<_>>(),
            expected
        );
        assert_eq!(
            first.iter().map(|(wid, _)| *wid).collect::<Vec<_>>(),
            windows.iter().map(|(wid, _)| *wid).collect::<Vec<_>>()
        );
    }

    #[test]
    fn assign_slots_prefers_the_nearest_slot() {
        let area = Rect::new(0, 0, 1000, 1000);
        let id = |n: u64| crate::window::WindowId(n);
        // Four windows hugging the four corners.
        let corners = [(50, 50), (700, 50), (50, 700), (700, 700)];
        let windows: Vec<_> = corners
            .iter()
            .enumerate()
            .map(|(n, &(x, y))| (id(n as u64), Rect::new(x, y, 250, 250)))
            .collect();
        let assigned = assign_slots(area, &windows);
        assert_eq!(assigned.len(), 4);
        for (wi, (_, rect)) in windows.iter().enumerate() {
            let slot = assigned[wi].1;
            let (wx, wy) = center(*rect);
            let (sx, sy) = center(slot);
            // The window's own corner slot is ~130 px away; any other slot
            // is >550 px away on at least one axis.
            assert!((wx - sx).abs() <= 250, "window {wi} drifted in x");
            assert!((wy - sy).abs() <= 250, "window {wi} drifted in y");
        }
    }

    #[test]
    fn assign_slots_degenerates_to_empty() {
        assert!(assign_slots(Rect::new(0, 0, 800, 600), &[]).is_empty());
        let one = vec![(crate::window::WindowId(1), Rect::new(0, 0, 100, 100))];
        assert!(assign_slots(Rect::new(0, 0, 0, 600), &one).is_empty());
    }

    #[test]
    fn interaction_domain_shelf_stays_on_the_right_edge() {
        let display = Rect::new(20, 10, 1000, 700);
        let tiles = interaction_domain_shelf(display, 3);
        assert_eq!(tiles.len(), 3);
        assert!(tiles.iter().all(|tile| {
            tile.origin.x >= display.origin.x + display.size.w - INTERACTION_DOMAIN_SHELF_WIDTH
                && tile.origin.x + tile.size.w <= display.origin.x + display.size.w
        }));
        assert_eq!(
            tiles[1].origin.y - tiles[0].origin.y,
            INTERACTION_DOMAIN_TILE_H + INTERACTION_DOMAIN_GAP
        );
    }
}
