//! Responsive lock-screen geometry, independent of Lens and the GPU host.

pub use aegis_config::LockScreenStyle;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LockLayout {
    pub width: f32,
    pub height: f32,
    pub clock_x: f32,
    pub clock_width: f32,
    pub clock_y: f32,
    pub clock_size: f32,
    pub avatar_x: f32,
    pub avatar_size: f32,
    pub avatar_y: f32,
    pub field_x: f32,
    pub field_width: f32,
    pub field_height: f32,
    pub field_y: f32,
}

/// Centered composition for portrait, compact, and conventional sign-in use.
///
/// Short displays collapse vertical gaps before shrinking essential hit
/// targets. Wide displays retain a deliberately narrow authentication focus.
#[must_use]
pub fn lock_layout(width: f32, height: f32) -> LockLayout {
    lock_layout_for(LockScreenStyle::Centered, width, height)
}

/// Resolve geometry for one of the supported lock-screen compositions.
///
/// The `bsod` composition renders through [`BsodLayout`]; this mapping only
/// projects its shared slots (credential area and corner clock) so generic
/// callers keep receiving sane values. The stop screen has no input box:
/// the projected field follows the counter row where typed characters echo.
#[must_use]
pub fn lock_layout_for(style: LockScreenStyle, width: f32, height: f32) -> LockLayout {
    match style {
        LockScreenStyle::Centered => centered_layout(width, height),
        LockScreenStyle::Cinematic => cinematic_layout(width, height),
        LockScreenStyle::Bsod => {
            let bsod = bsod_layout(width, height);
            LockLayout {
                width: bsod.width,
                height: bsod.height,
                clock_x: bsod.clock_x,
                clock_width: bsod.clock_width,
                clock_y: bsod.clock_y,
                clock_size: bsod.clock_size,
                avatar_x: 0.0,
                avatar_size: 0.0,
                avatar_y: 0.0,
                field_x: bsod.margin_x,
                field_y: bsod.marks_y,
                field_width: bsod.copy_width,
                field_height: bsod.counter_size * 1.2,
            }
        }
    }
}

fn centered_layout(width: f32, height: f32) -> LockLayout {
    let width = width.max(320.0);
    let height = height.max(360.0);
    let compact = height < 650.0;
    let clock_size = if compact { 58.0 } else { 88.0 };
    let clock_y = if compact {
        28.0
    } else {
        (height * 0.105).clamp(54.0, 124.0)
    };
    let avatar_size = if compact { 72.0 } else { 96.0 };
    let avatar_y = if compact {
        height * 0.37
    } else {
        (height * 0.43).clamp(clock_y + 150.0, height - 310.0)
    };
    let field_width = (width - 48.0).min(320.0);
    let field_height = 48.0;
    let field_y = (avatar_y + avatar_size + if compact { 54.0 } else { 68.0 })
        .min(height - field_height - 64.0);
    let field_x = (width - field_width) * 0.5;
    LockLayout {
        width,
        height,
        clock_x: (width - width.min(720.0)) * 0.5,
        clock_width: width.min(720.0),
        clock_y,
        clock_size,
        avatar_x: (width - avatar_size) * 0.5,
        avatar_size,
        avatar_y,
        field_x,
        field_width,
        field_height,
        field_y,
    }
}

fn cinematic_layout(width: f32, height: f32) -> LockLayout {
    let width = width.max(320.0);
    let height = height.max(360.0);
    let compact = width < 720.0 || height < 560.0;
    let side = (width * 0.052).clamp(24.0, 96.0);
    let bottom = (height * 0.09).clamp(36.0, 104.0);
    let field_width = if compact {
        (width - side * 2.0).min(340.0)
    } else {
        (width * 0.25).clamp(340.0, 440.0)
    };
    let field_height = 52.0;
    let field_x = width - side - field_width;
    let field_y = height - bottom - field_height;
    let avatar_size = if compact { 42.0 } else { 48.0 };
    let avatar_x = field_x;
    let avatar_y = field_y - avatar_size - 34.0;
    let clock_width = if compact {
        (width - side * 2.0).min(300.0)
    } else {
        340.0
    };
    LockLayout {
        width,
        height,
        clock_x: width - side - clock_width,
        clock_width,
        clock_y: (height * 0.085).clamp(28.0, 96.0),
        clock_size: if compact { 52.0 } else { 72.0 },
        avatar_x,
        avatar_size,
        avatar_y,
        field_x,
        field_width,
        field_height,
        field_y,
    }
}

/// Geometry for the `bsod` stop-screen composition.
///
/// The layout mirrors the classic full-screen stop page: a large sad face
/// above a left-aligned headline column, the counter and keystroke marks
/// woven into the page, and a support block with its QR module pinned to the
/// lower left. Density tiers keep the composition legible from phone-class
/// panels to 4K desktops.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BsodLayout {
    pub width: f32,
    pub height: f32,
    /// Left margin of the text column (and the sad-face baseline).
    pub margin_x: f32,
    pub face_y: f32,
    pub face_size: f32,
    /// Top of the headline block.
    pub headline_y: f32,
    pub headline_size: f32,
    /// Width available to the wrapped headline and support copy.
    pub copy_width: f32,
    pub counter_y: f32,
    pub counter_size: f32,
    /// Keystroke marks row directly below the counter.
    pub marks_y: f32,
    /// Support block pinned above the bottom margin.
    pub support_x: f32,
    pub support_y: f32,
    pub support_width: f32,
    pub support_size: f32,
    /// Easter-egg QR module grid beside the support block.
    pub qr_x: f32,
    pub qr_y: f32,
    pub qr_size: f32,
    /// Corner clock (hours:minutes).
    pub clock_x: f32,
    pub clock_y: f32,
    pub clock_width: f32,
    pub clock_size: f32,
}

/// Rendered type scale of one BSoD composition.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BsodTypeScale {
    pub face: f32,
    pub headline: f32,
    pub counter: f32,
    pub support: f32,
    pub clock: f32,
}

const BSOD_COMPACT_BELOW_WIDTH: f32 = 560.0;
const BSOD_ULTRA_ABOVE_WIDTH: f32 = 2200.0;

/// Type density for one output width. Compact keeps a full-bleed stop page
/// readable on phone-class panels; ultra restores desktop proportions on
/// 4K-class widths where the base sizes would look undersized.
#[must_use]
pub fn bsod_type_scale(width: f32) -> BsodTypeScale {
    if width < BSOD_COMPACT_BELOW_WIDTH {
        BsodTypeScale {
            face: 64.0,
            headline: 22.0,
            counter: 15.0,
            support: 11.0,
            clock: 13.0,
        }
    } else if width > BSOD_ULTRA_ABOVE_WIDTH {
        BsodTypeScale {
            face: 168.0,
            headline: 58.0,
            counter: 40.0,
            support: 29.0,
            clock: 30.0,
        }
    } else {
        BsodTypeScale {
            face: 96.0,
            headline: 34.0,
            counter: 23.0,
            support: 17.0,
            clock: 18.0,
        }
    }
}

/// Right edge available to the QR grid on compact panels (the full width
/// minus the margin; the clock lives top-right, out of reach).
fn qr_x_max(width: f32, margin_x: f32) -> f32 {
    width - margin_x
}

#[must_use]
pub fn bsod_layout(width: f32, height: f32) -> BsodLayout {
    let width = width.max(320.0);
    let height = height.max(360.0);
    let scale = bsod_type_scale(width);
    let compact = width < BSOD_COMPACT_BELOW_WIDTH;
    let margin_x = if compact {
        (width * 0.055).clamp(20.0, 32.0)
    } else {
        (width * 0.085).clamp(48.0, 168.0)
    };
    let bottom = (height * 0.075).clamp(28.0, 96.0);
    let clock_width = if compact { 120.0 } else { 200.0 };
    let clock_x = width - margin_x - clock_width;
    // Compact panels park the clock beside the sad face; wider outputs use
    // the classic lower-right corner, where the support copy must stop
    // short of the clock column instead of flowing beneath it.
    let clock_y = if compact {
        (height * 0.055).clamp(20.0, 32.0)
    } else {
        height - bottom - scale.clock * 1.2
    };
    let copy_width = (width - margin_x * 2.0)
        .clamp(240.0, 680.0)
        .min(if compact {
            f32::MAX
        } else {
            (clock_x - margin_x - 24.0).max(240.0)
        });
    // The QR grid sits beside the support block; the whole bottom cluster
    // (QR + copy) hugs the bottom margin. On wide outputs the QR stops left
    // of the corner-clock column so the two never share the right edge;
    // compact panels park the clock top-right where the QR cannot reach.
    let qr_size = if compact { 88.0 } else { 120.0 };
    let qr_right_limit = if compact {
        qr_x_max(width, margin_x)
    } else {
        clock_x - 24.0
    };
    let qr_x = (qr_right_limit - qr_size).max(margin_x);
    let qr_y = height - bottom - qr_size;
    let support_width = (if compact {
        qr_x - margin_x - 16.0
    } else {
        qr_x - margin_x - 24.0
    })
    .max(160.0);
    let support_x = margin_x;
    let support_block_height = scale.support * 2.0 * 1.5;
    let support_y = height - bottom - support_block_height;
    let counter_gap = if compact { 22.0 } else { 34.0 };
    let counter_y = (qr_y - counter_gap - scale.counter * 1.4).min(height * 0.62);
    let marks_y = counter_y + scale.counter * 1.6;
    let headline_lines = if compact { 3.0 } else { 2.0 };
    let headline_gap = if compact { 26.0 } else { 34.0 };
    let headline_top_gap = if compact { 26.0 } else { 36.0 };
    let face_y = if compact {
        (height * 0.055).clamp(20.0, 32.0)
    } else {
        (height * 0.11).clamp(44.0, 148.0)
    };
    let headline_y = (counter_y - headline_gap - scale.headline * 1.25 * headline_lines)
        .max(scale.face + headline_top_gap);
    BsodLayout {
        width,
        height,
        margin_x,
        face_y,
        face_size: scale.face,
        headline_y,
        headline_size: scale.headline,
        copy_width,
        counter_y,
        counter_size: scale.counter,
        marks_y,
        support_x,
        support_y,
        support_width,
        support_size: scale.support,
        qr_x,
        qr_y,
        qr_size,
        clock_x,
        clock_y,
        clock_width,
        clock_size: scale.clock,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn small_outputs_preserve_password_hit_target() {
        let layout = lock_layout(360.0, 480.0);
        assert_eq!(layout.field_height, 48.0);
        assert!(layout.field_width <= 320.0);
        assert!(layout.field_y + layout.field_height <= layout.height);
    }

    #[test]
    fn ultrawide_output_keeps_focused_field() {
        let layout = lock_layout(3440.0, 1440.0);
        assert_eq!(layout.field_width, 320.0);
    }

    #[test]
    fn cinematic_layout_anchors_credentials_to_the_lower_right() {
        let layout = lock_layout_for(LockScreenStyle::Cinematic, 1920.0, 1080.0);
        assert!(layout.field_x > layout.width * 0.6);
        assert!(layout.field_y > layout.height * 0.75);
        assert!(layout.clock_x > layout.width * 0.6);
        assert!(layout.field_x + layout.field_width < layout.width);
    }

    #[test]
    fn compact_cinematic_layout_preserves_margins_and_hit_target() {
        let layout = lock_layout_for(LockScreenStyle::Cinematic, 360.0, 480.0);
        assert!(layout.field_x >= 24.0);
        assert!(layout.field_x + layout.field_width <= layout.width - 24.0);
        assert_eq!(layout.field_height, 52.0);
    }

    #[test]
    fn bsod_layout_keeps_left_column_inside_the_safe_margins() {
        for (width, height) in [
            (360.0, 480.0),
            (390.0, 844.0),
            (1280.0, 800.0),
            (1920.0, 1080.0),
            (2560.0, 1440.0),
            (3440.0, 1440.0),
        ] {
            let layout = bsod_layout(width, height);
            assert!(layout.margin_x >= 20.0, "{width}x{height} margin");
            // Vertical stack: face, headline, counter, marks, support+QR.
            assert!(
                layout.headline_y > layout.face_y + layout.face_size * 0.75,
                "{width}x{height} headline overlaps face"
            );
            assert!(
                layout.counter_y > layout.headline_y,
                "{width}x{height} counter above headline"
            );
            assert!(
                layout.marks_y > layout.counter_y,
                "{width}x{height} marks above counter"
            );
            assert!(
                layout.support_y > layout.marks_y,
                "{width}x{height} support overlaps marks"
            );
            assert!(
                layout.support_y + layout.support_size * 2.0 * 1.5 <= layout.height,
                "{width}x{height} support block bottom"
            );
            // The QR grid never crosses into the support copy or the edge.
            assert!(
                layout.qr_x + layout.qr_size <= layout.width - layout.margin_x * 0.5,
                "{width}x{height} qr right edge"
            );
            assert!(
                layout.support_x + layout.support_width <= layout.qr_x - 8.0,
                "{width}x{height} support copy overlaps qr"
            );
            // The corner clock must never collide with the left column.
            // Wide outputs park it lower-right (horizontal separation);
            // compact outputs park it upper-right beside the face, so the
            // check becomes vertical separation from the marks row.
            if width < 560.0 {
                assert!(
                    layout.clock_y + layout.clock_size * 1.4 < layout.headline_y,
                    "{width}x{height} compact clock intrudes into the column"
                );
                assert!(
                    layout.margin_x + layout.face_size * 0.8 < layout.clock_x,
                    "{width}x{height} face overlaps the clock column"
                );
            } else {
                assert!(
                    layout.clock_x > layout.support_x + layout.support_width,
                    "{width}x{height} clock overlaps support"
                );
                assert!(
                    layout.copy_width <= layout.clock_x - layout.margin_x,
                    "{width}x{height} copy column overlaps clock column"
                );
            }
        }
    }

    #[test]
    fn bsod_type_scale_grows_with_the_output() {
        let compact = bsod_type_scale(390.0);
        let base = bsod_type_scale(1920.0);
        let ultra = bsod_type_scale(3440.0);
        assert!(compact.headline < base.headline);
        assert!(base.headline < ultra.headline);
        assert!(ultra.headline <= 58.0);
    }

    #[test]
    fn lock_layout_for_projects_shared_bsod_slots() {
        let layout = lock_layout_for(LockScreenStyle::Bsod, 1280.0, 800.0);
        assert!(layout.field_width > 0.0);
        assert!(layout.field_y + layout.field_height <= layout.height);
        assert_eq!(layout.avatar_size, 0.0);
    }
}
