//! Scene colors keyed by the resolved desktop color scheme.
//!
//! The values live in `tessera_design` (`Design::scene`) so every scene tone
//! has exactly one source of truth; these accessors convert the tokens into
//! the packed forms the render passes consume. `System` resolves inside
//! `Design::for_scheme` through [`ColorScheme::or_dark`].

use tessera_model::settings::ColorScheme;

fn scene(scheme: ColorScheme) -> tessera_design::SceneColors {
    tessera_design::Design::for_scheme(scheme).scene
}

/// Opaque base behind every composited pixel: the dark desktop tone, or the
/// light appearance's surface gray.
pub(super) fn clear_color(scheme: ColorScheme) -> u32 {
    let (r, g, b, a) = scene(scheme).clear_color.components();
    flux::rgba(r, g, b, a)
}

/// Base RGB of the overview scrim; the caller scales its alpha by the
/// overview's reveal progress.
pub(super) fn overview_scrim(scheme: ColorScheme) -> (u8, u8, u8) {
    let (r, g, b, _) = scene(scheme).overview_scrim.components();
    (r, g, b)
}

/// Base RGB of the window-switcher scrim; the caller scales its alpha by the
/// switcher's visibility.
pub(super) fn window_switcher_scrim(scheme: ColorScheme) -> (u8, u8, u8) {
    let (r, g, b, _) = scene(scheme).window_switcher_scrim.components();
    (r, g, b)
}

/// Opaque base of an Interaction Domain capture, visible wherever the domain
/// has no client pixels.
pub(super) fn interaction_domain_clear(scheme: ColorScheme) -> u32 {
    let (r, g, b, a) = scene(scheme).interaction_domain_clear.components();
    flux::rgba(r, g, b, a)
}

/// Liquid-glass body tint multiplier (`[255, 255, 255]` is neutral). The
/// light appearance keeps the multiplier near-neutral, toned to its cool
/// white surface instead of a pure white.
pub(super) fn glass_tint(scheme: ColorScheme) -> [u8; 3] {
    scene(scheme).glass_tint
}

#[cfg(test)]
mod tests;
