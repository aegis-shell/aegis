//! Scene colors keyed by the resolved desktop color scheme.
//!
//! Shell chrome repaints from `aegis_design::Design::for_scheme`; the passes
//! covered here never touch chrome, so they read the same resolved scheme
//! directly. `System` resolves through [`ColorScheme::or_dark`], matching the
//! design tokens.

use aegis_model::settings::ColorScheme;

/// Opaque base behind every composited pixel: the dark desktop tone, or the
/// light appearance's surface gray.
pub(super) fn clear_color(scheme: ColorScheme) -> u32 {
    match scheme.or_dark() {
        ColorScheme::Dark | ColorScheme::System => flux::rgba(30, 30, 46, 255),
        ColorScheme::Light => flux::rgba(243, 245, 249, 255),
    }
}

/// Translucent dim beneath the overview's window thumbnails.
pub(super) fn overview_scrim(scheme: ColorScheme) -> u32 {
    match scheme.or_dark() {
        ColorScheme::Dark | ColorScheme::System => flux::rgba(8, 10, 20, 200),
        ColorScheme::Light => flux::rgba(243, 245, 249, 200),
    }
}

/// Base RGB of the window-switcher scrim; the caller scales its alpha by the
/// switcher's visibility.
pub(super) fn window_switcher_scrim(scheme: ColorScheme) -> (u8, u8, u8) {
    match scheme.or_dark() {
        ColorScheme::Dark | ColorScheme::System => (5, 7, 12),
        ColorScheme::Light => (243, 245, 249),
    }
}

/// Opaque base of an Interaction Domain capture, visible wherever the domain
/// has no client pixels.
pub(super) fn interaction_domain_clear(scheme: ColorScheme) -> u32 {
    match scheme.or_dark() {
        ColorScheme::Dark | ColorScheme::System => flux::rgba(17, 20, 27, 255),
        ColorScheme::Light => flux::rgba(243, 245, 249, 255),
    }
}

/// Liquid-glass body tint multiplier (`[255, 255, 255]` is neutral). The
/// light appearance keeps the multiplier near-neutral, toned to its cool
/// white surface instead of a pure white.
pub(super) fn glass_tint(scheme: ColorScheme) -> [u8; 3] {
    match scheme.or_dark() {
        ColorScheme::Dark | ColorScheme::System => [255, 255, 255],
        ColorScheme::Light => [243, 245, 249],
    }
}

#[cfg(test)]
mod tests;
