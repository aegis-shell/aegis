use super::*;

#[test]
fn dark_scheme_keeps_the_historical_scene_colors() {
    assert_eq!(clear_color(ColorScheme::Dark), flux::rgba(30, 30, 46, 255));
    assert_eq!(overview_scrim(ColorScheme::Dark), (8, 10, 20));
    assert_eq!(window_switcher_scrim(ColorScheme::Dark), (5, 7, 12));
    assert_eq!(
        interaction_domain_clear(ColorScheme::Dark),
        flux::rgba(17, 20, 27, 255)
    );
    assert_eq!(glass_tint(ColorScheme::Dark), [255, 255, 255]);
}

#[test]
fn system_resolves_to_the_dark_fallback() {
    assert_eq!(
        clear_color(ColorScheme::System),
        clear_color(ColorScheme::Dark)
    );
    assert_eq!(
        overview_scrim(ColorScheme::System),
        overview_scrim(ColorScheme::Dark)
    );
}

#[test]
fn light_scheme_inverts_the_scene_tone() {
    assert_eq!(
        clear_color(ColorScheme::Light),
        flux::rgba(243, 245, 249, 255)
    );
    assert_eq!(window_switcher_scrim(ColorScheme::Light), (243, 245, 249));
    assert_eq!(glass_tint(ColorScheme::Light), [243, 245, 249]);
}
