use lens::Rect;

/// Standard inset that keeps compositor popups away from output edges.
pub const POPUP_MARGIN: f32 = 8.0;
/// Standard gap between a popup and its owning chrome surface.
pub const POPUP_GAP: f32 = 8.0;

/// Place a chrome popup against its owner, preferring above and then below,
/// while keeping the complete surface inside the output.
pub fn place_popup(owner: Rect, size: (f32, f32), display: (f32, f32)) -> Rect {
    let w = size.0.min((display.0 - POPUP_MARGIN * 2.0).max(1.0));
    let h = size.1.min((display.1 - POPUP_MARGIN * 2.0).max(1.0));
    let max_x = (display.0 - w - POPUP_MARGIN).max(POPUP_MARGIN);
    let owner_centre = owner.x + owner.w * 0.5;
    let x = (owner_centre - w * 0.5).clamp(POPUP_MARGIN, max_x);
    let above = owner.y - POPUP_GAP - h;
    let below = owner.y + owner.h + POPUP_GAP;
    let y = if above >= POPUP_MARGIN {
        above
    } else if below + h <= display.1 - POPUP_MARGIN {
        below
    } else {
        above.clamp(
            POPUP_MARGIN,
            (display.1 - h - POPUP_MARGIN).max(POPUP_MARGIN),
        )
    };
    Rect { x, y, w, h }
}

/// The side of its owner a chrome popup opens toward.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum PopupSide {
    /// Above the owner (the bottom-anchored default).
    #[default]
    Above,
    /// Right of the owner (an owner on the left edge).
    Right,
    /// Left of the owner (an owner on the right edge).
    Left,
}

/// Place a chrome popup beside its owner toward `side`, falling back to the
/// opposite side and then to a clamped position when the preferred side does
/// not fit. The popup is centred on the owner along the cross axis.
pub fn place_popup_side(
    owner: Rect,
    size: (f32, f32),
    display: (f32, f32),
    side: PopupSide,
) -> Rect {
    if side == PopupSide::Above {
        return place_popup(owner, size, display);
    }
    let w = size.0.min((display.0 - POPUP_MARGIN * 2.0).max(1.0));
    let h = size.1.min((display.1 - POPUP_MARGIN * 2.0).max(1.0));
    let owner_centre = owner.y + owner.h * 0.5;
    let max_y = (display.1 - h - POPUP_MARGIN).max(POPUP_MARGIN);
    let y = (owner_centre - h * 0.5).clamp(POPUP_MARGIN, max_y);
    let (preferred, fallback) = if side == PopupSide::Right {
        (owner.x + owner.w + POPUP_GAP, owner.x - POPUP_GAP - w)
    } else {
        (owner.x - POPUP_GAP - w, owner.x + owner.w + POPUP_GAP)
    };
    let x = if preferred >= POPUP_MARGIN && preferred + w <= display.0 - POPUP_MARGIN {
        preferred
    } else if fallback >= POPUP_MARGIN && fallback + w <= display.0 - POPUP_MARGIN {
        fallback
    } else {
        preferred.clamp(
            POPUP_MARGIN,
            (display.0 - w - POPUP_MARGIN).max(POPUP_MARGIN),
        )
    };
    Rect { x, y, w, h }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn popup_prefers_above_and_stays_inside_the_output() {
        let owner = Rect {
            x: 700.0,
            y: 520.0,
            w: 72.0,
            h: 72.0,
        };
        let rect = place_popup(owner, (236.0, 180.0), (800.0, 600.0));
        assert!(rect.x >= POPUP_MARGIN);
        assert!(rect.x + rect.w <= 800.0 - POPUP_MARGIN);
        assert!(rect.y + rect.h <= owner.y - POPUP_GAP);
        assert!(rect.y >= POPUP_MARGIN);
    }

    #[test]
    fn popup_falls_below_an_owner_near_the_top() {
        let owner = Rect {
            x: 200.0,
            y: 5.0,
            w: 56.0,
            h: 56.0,
        };
        let rect = place_popup(owner, (236.0, 180.0), (800.0, 600.0));
        assert!(rect.y >= owner.y + owner.h + POPUP_GAP);
    }

    #[test]
    fn side_popup_prefers_the_given_side_and_stays_inside_the_output() {
        let owner = Rect {
            x: 20.0,
            y: 260.0,
            w: 72.0,
            h: 72.0,
        };
        let right = place_popup_side(owner, (236.0, 180.0), (800.0, 600.0), PopupSide::Right);
        assert!(right.x >= owner.x + owner.w + POPUP_GAP);
        assert!(right.x + right.w <= 800.0 - POPUP_MARGIN);
        assert!(right.y >= POPUP_MARGIN && right.y + right.h <= 600.0 - POPUP_MARGIN);

        // A centred owner has room on its left for the menu.
        let owner = Rect {
            x: 400.0,
            y: 260.0,
            w: 72.0,
            h: 72.0,
        };
        let left = place_popup_side(owner, (236.0, 180.0), (800.0, 600.0), PopupSide::Left);
        assert!(left.x + left.w <= owner.x - POPUP_GAP);
        assert!(left.x >= POPUP_MARGIN);
    }

    #[test]
    fn side_popup_falls_back_to_the_opposite_side_when_the_preferred_overflows() {
        // An owner near the right edge cannot host a popup on its right.
        let owner = Rect {
            x: 700.0,
            y: 200.0,
            w: 72.0,
            h: 72.0,
        };
        let rect = place_popup_side(owner, (236.0, 180.0), (800.0, 600.0), PopupSide::Right);
        assert!(rect.x + rect.w <= owner.x - POPUP_GAP);
        assert!(rect.x >= POPUP_MARGIN);
    }
}
