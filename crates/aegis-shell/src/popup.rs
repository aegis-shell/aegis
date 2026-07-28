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
}
