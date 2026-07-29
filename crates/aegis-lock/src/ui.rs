//! Responsive lock-screen geometry, independent of Lens and the GPU host.

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LockLayout {
    pub width: f32,
    pub height: f32,
    pub clock_y: f32,
    pub clock_size: f32,
    pub avatar_size: f32,
    pub avatar_y: f32,
    pub field_width: f32,
    pub field_height: f32,
    pub field_y: f32,
}

/// macOS-inspired composition adapted to Aegis's compact glass vocabulary.
///
/// Short displays collapse vertical gaps before shrinking essential hit
/// targets. Wide displays retain a deliberately narrow authentication focus.
#[must_use]
pub fn lock_layout(width: f32, height: f32) -> LockLayout {
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
    LockLayout {
        width,
        height,
        clock_y,
        clock_size,
        avatar_size,
        avatar_y,
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
}
