//! Shared presentation framework for lock-screen compositions.
//!
//! Every composition is a [`StylePainter`] plugged into the same four-stage
//! frame pipeline owned by the render host:
//!
//! 1. `paint_background` — full-bleed flux layer (wallpaper, solid, or a
//!    composition-owned identity color).
//! 2. `paint_materials` — flux shapes that lens cannot express (discs,
//!    gradients, module grids), in physical pixels.
//! 3. `paint_clock` — the clock shown even in the ambient presentation.
//! 4. `paint_identity` — the authentication content that fades in ambient.
//!
//! The host owns no composition logic: it resolves configuration, loads
//! resources, drives the frame, and forwards [`FramePresentation`] here.

use std::time::Instant;

use tessera_config::LockScreenStyle;
use tessera_lock::LockState;
use lens::{Align, Color, LayoutOpts, Theme};

use crate::profile::Profile;
use crate::render::{AvatarStatus, LockBackground, LockPalette, LockVisual};

/// Inputs shared by every paint stage of one frame.
pub struct FramePresentation<'a> {
    pub logical: (u32, u32),
    pub avatar: Option<&'a flux::Image>,
    pub avatar_status: AvatarStatus,
    pub state: &'a LockState,
    pub profile: &'a Profile,
    /// 0..=1 reveal progress of the identity content.
    pub progress: f32,
    /// Horizontal rejection-shake offset in logical pixels.
    pub feedback_offset: f32,
    pub now: Instant,
    pub scale: f32,
}

/// One lock-screen composition.
///
/// Implementations are stateless: the host passes the full presentation on
/// every frame, so a painter never outlives or caches frame state.
pub trait StylePainter {
    /// Full-bleed background. Runs on the physical-pixel canvas before any
    /// other stage; `dim` is the configured artwork scrim strength.
    fn paint_background(
        &self,
        canvas: &flux::Canvas,
        device: &flux::Device,
        background: &mut LockBackground,
        output: (u32, u32),
        dim: f32,
    );

    /// Flux shapes below the lens text layer, in physical pixels.
    fn paint_materials(&self, canvas: &flux::Canvas, frame: &FramePresentation<'_>);

    /// Clock rendered in both engaged and ambient presentations (lens layer).
    fn paint_clock(
        &self,
        ui: &mut lens::Frame,
        frame: &FramePresentation<'_>,
        clock: &str,
        date: &str,
    );

    /// Authentication content, gated by the host on reveal progress.
    fn paint_identity(&self, ui: &mut lens::Frame, frame: &FramePresentation<'_>);

    /// Whether the composition animates continuously while engaged; the
    /// host uses this to keep requesting frames. `false` is safe for static
    /// compositions.
    fn animates_while_engaged(&self, _state: &LockState) -> bool {
        false
    }
}

/// Resolve the painter for a resolved visual style.
pub fn painter_for(visual: LockVisual) -> Box<dyn StylePainter> {
    match visual.style {
        LockScreenStyle::Centered => Box::new(super::centered::CenteredPainter { visual }),
        LockScreenStyle::Cinematic => Box::new(super::cinematic::CinematicPainter { visual }),
        LockScreenStyle::Bsod => Box::new(super::bsod::BsodPainter { visual }),
    }
}

// ---------------------------------------------------------------------------
// Shared helpers available to every composition.
// ---------------------------------------------------------------------------

/// Keyboard status line, `None` when nothing worth reporting is known.
pub fn keyboard_status(state: &LockState) -> Option<String> {
    match (state.caps_lock(), state.keyboard_layout()) {
        (true, Some(layout)) => Some(format!("CAPS · {layout}")),
        (true, None) => Some("CAPS".to_owned()),
        (false, Some(layout)) => Some(layout.to_owned()),
        (false, None) => None,
    }
}

/// Lens hides the `##` suffix while hashing the complete label as widget
/// profile. A monotonically changing suffix prevents deletion from reviving
/// an older retained node for the same shorter text.
pub fn credential_label(visible: &str, revision: u64) -> String {
    format!("{visible}##lock-credential-{revision}")
}

/// Normalized offset of the brief rejection shake.
pub fn rejection_shake_offset(progress: f32) -> f32 {
    let progress = progress.clamp(0.0, 1.0);
    let envelope = 1.0 - progress;
    (progress * std::f32::consts::TAU * 3.0).sin() * 10.0 * envelope
}

/// Cinematic password marks: one diamond per entered character and never
/// empty placeholder slots, so the rail does not imply a fixed length.
pub fn cinematic_password_marks(password_len: usize) -> String {
    // `Secret` already bounds the input. Do not cap the visible sequence:
    // doing so makes both typing and deletion appear stuck above the cap.
    "◆  ".repeat(password_len).trim_end().to_owned()
}

/// A transparent theme tinted toward one foreground color. The alpha is
/// modulated by the reveal progress so identity chrome fades in ambient.
pub fn lock_theme(design: &tessera_design::Design, foreground: Color, alpha: u8) -> Theme {
    let (red, green, blue, source_alpha) = foreground.components();
    let alpha = ((u16::from(source_alpha) * u16::from(alpha)) / 255) as u8;
    // The hairline edges the field against the background: pale on the dark
    // appearance, a dark wash once the scheme turns light.
    let border = if design.is_light() {
        Color::rgba(28, 32, 44, alpha / 3)
    } else {
        Color::rgba(255, 255, 255, alpha / 3)
    };
    tessera_design::themes::application(design)
        .with_bg(Color::TRANSPARENT)
        .with_fg(Color::rgba(red, green, blue, alpha))
        .with_border(border)
}

pub fn palette_foreground(palette: LockPalette) -> Color {
    palette.foreground
}

pub fn palette_muted(palette: LockPalette) -> Color {
    palette.muted
}

pub fn centered_layer() -> LayoutOpts {
    aligned_layer(Align::Center)
}

pub fn aligned_layer(alignment: Align) -> LayoutOpts {
    LayoutOpts {
        pad: 0.0,
        cross: alignment,
        ..tessera_design::materials::surface_layout()
    }
}

/// Locale-aware copy selection shared by all compositions.
pub fn localized(en: &str, zh: &str) -> String {
    localized_ref(en, zh).to_owned()
}

pub fn localized_ref<'a>(en: &'a str, zh: &'a str) -> &'a str {
    let locale = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    if locale.starts_with("zh") { zh } else { en }
}

#[cfg(test)]
mod tests {
    use super::{credential_label, rejection_shake_offset};

    #[test]
    fn rejection_shake_crosses_both_sides_and_settles_at_origin() {
        assert!(rejection_shake_offset(1.0 / 12.0) > 0.0);
        assert!(rejection_shake_offset(3.0 / 12.0) < 0.0);
        assert_eq!(rejection_shake_offset(1.0), 0.0);
    }

    #[test]
    fn credential_edits_receive_unique_hidden_widget_identity() {
        let before = credential_label("◆  ◆", 7);
        let after_delete = credential_label("◆", 8);
        assert_eq!(before.split("##").next(), Some("◆  ◆"));
        assert_eq!(after_delete.split("##").next(), Some("◆"));
        assert_ne!(before, after_delete);
    }

    #[test]
    fn cinematic_password_marks_never_render_empty_placeholders() {
        use super::cinematic_password_marks;
        assert_eq!(cinematic_password_marks(0), "");
        assert_eq!(cinematic_password_marks(2), "◆  ◆");
        assert_eq!(cinematic_password_marks(8), "◆  ◆  ◆  ◆  ◆  ◆  ◆  ◆");
        assert_ne!(cinematic_password_marks(8), cinematic_password_marks(7));
    }
}
