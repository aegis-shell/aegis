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
#[must_use]
pub fn lock_layout_for(style: LockScreenStyle, width: f32, height: f32) -> LockLayout {
    match style {
        LockScreenStyle::Centered => centered_layout(width, height),
        LockScreenStyle::Cinematic => cinematic_layout(width, height),
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
}
