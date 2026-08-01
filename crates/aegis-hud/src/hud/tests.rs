use super::*;
use aegis_shell::Reserved;

fn workspaces_empty() -> WorkspaceSnapshot {
    WorkspaceSnapshot {
        outputs: Vec::new(),
    }
}

#[test]
fn the_hud_never_reserves_space_or_captures_the_pointer() {
    let bar = Hud::new();
    let workspaces = workspaces_empty();
    assert_eq!(bar.reserved(), Reserved::default());
    assert!(!bar.captures_pointer(10.0, 10.0, (1920.0, 1080.0), &[], &workspaces));
    assert!(!bar.captures_pointer(960.0, 4.0, (1920.0, 1080.0), &[], &workspaces));
}

#[test]
fn fullscreen_window_hides_the_hud() {
    let mut bar = Hud::new();
    let mut fullscreen = Window::new(aegis_core::window::WindowId(7));
    fullscreen.state.fullscreen = true;
    bar.update_windows(&[fullscreen]);

    let workspaces = workspaces_empty();
    assert!(bar.fullscreen_active);
    assert_eq!(bar.backdrop_blur_sigma(), 0.0);
    assert!(
        bar.backdrop_regions((1920.0, 1080.0), &[], &workspaces)
            .is_empty()
    );
    assert!(!bar.anim_pending());

    bar.update_windows(&[]);
    assert!(!bar.fullscreen_active);
}

#[test]
fn maximized_and_minimized_fullscreen_windows_keep_the_hud_visible() {
    let mut bar = Hud::new();
    let mut maximized = Window::new(aegis_core::window::WindowId(7));
    maximized.state.maximized = true;
    bar.update_windows(&[maximized]);
    assert!(!bar.fullscreen_active);

    let mut minimized_fullscreen = Window::new(aegis_core::window::WindowId(8));
    minimized_fullscreen.state.fullscreen = true;
    minimized_fullscreen.minimized = true;
    bar.update_windows(&[minimized_fullscreen]);
    assert!(!bar.fullscreen_active);
}

#[test]
fn cursor_proximity_targets_zero_and_distance_targets_full_visibility() {
    let chip = Rect {
        x: 8.0,
        y: 8.0,
        w: 200.0,
        h: 32.0,
    };
    // On the chip and inside the proximity margin: hidden.
    assert_eq!(Hud::fade_target(chip, (100.0, 20.0)), 0.0);
    assert_eq!(Hud::fade_target(chip, (100.0, 8.0 + 32.0 + 40.0)), 0.0);
    // Beyond the inflated rect: shown.
    assert_eq!(Hud::fade_target(chip, (100.0, 200.0)), 1.0);
    assert_eq!(Hud::fade_target(chip, (500.0, 20.0)), 1.0);
}

#[test]
fn chip_fade_eases_toward_the_target_and_snaps_under_reduced_motion() {
    let mut bar = Hud::new();
    bar.layout.visible = [true, true];
    bar.layout.chips[LEFT] = Rect {
        x: 8.0,
        y: 8.0,
        w: 200.0,
        h: 32.0,
    };

    // Cursor parked on the left chip: its fade eases toward 0, the center
    // chip stays at 1, and the animation reports pending work.
    bar.advance_fade(0.016, (100.0, 20.0));
    assert!(bar.chip_fade[LEFT] < 1.0 && bar.chip_fade[LEFT] > 0.0);
    assert_eq!(bar.chip_fade[CENTER], 1.0);
    assert!(bar.anim_pending());
    for _ in 0..600 {
        bar.advance_fade(0.016, (100.0, 20.0));
    }
    assert_eq!(bar.chip_fade[LEFT], 0.0);
    assert!(!bar.anim_pending());

    // Reduced motion resolves the fade in one step.
    bar.set_reduced_motion(true);
    bar.advance_fade(0.016, (1000.0, 500.0));
    assert_eq!(bar.chip_fade[LEFT], 1.0);
}

#[test]
fn backdrop_prepass_advances_glass_and_content_on_the_same_frame() {
    let mut bar = Hud::new();
    let workspaces = workspaces_empty();
    let mut input = Input::new((1920.0, 1080.0), 0.016);
    input.set_cursor(10.0, 10.0);

    bar.prepare_backdrop(&input, &[], &workspaces);

    assert!(bar.frame_prepared);
    assert!(bar.chip_fade[LEFT] < 1.0 && bar.chip_fade[LEFT] > 0.0);
    let glass = bar.liquid_glass_regions((1920.0, 1080.0), &[], &workspaces);
    assert_eq!(glass.len(), 1);
    assert_eq!(glass[0].opacity, bar.chip_fade[LEFT]);
}

#[test]
fn backdrop_regions_cover_only_the_visible_chips() {
    let mut bar = Hud::new();
    bar.layout.visible = [true, true];
    bar.chip_fade = [1.0, 0.5];
    bar.chip_target = bar.chip_fade;
    bar.layout.chips[LEFT] = Rect {
        x: 8.0,
        y: 8.0,
        w: 200.0,
        h: 32.0,
    };
    bar.layout.chips[CENTER] = Rect {
        x: 1600.0,
        y: 8.0,
        w: 220.0,
        h: 32.0,
    };
    let workspaces = workspaces_empty();
    let regions = bar.backdrop_regions((1920.0, 1080.0), &[], &workspaces);
    assert_eq!(regions.len(), 2);
    assert_eq!(regions[0].x, 8.0);
    assert_eq!(regions[1].x, 1600.0);
    assert_eq!(bar.backdrop_blur_sigma(), BACKDROP_BLUR_SIGMA);
    let glass = bar.liquid_glass_regions((1920.0, 1080.0), &[], &workspaces);
    assert_eq!(glass.len(), 2);
    assert_eq!(glass[0].bounds, regions[0]);
    assert_eq!(glass[0].corner_radius, CHIP_RADIUS);
    assert_eq!(glass[0].opacity, 1.0);
    assert_eq!(glass[1].opacity, 0.5);

    // Every chip faded: no blur work at all (an empty region list with a
    // nonzero sigma would be treated as full-screen).
    bar.chip_fade = [0.0, 0.0];
    let regions = bar.backdrop_regions((1920.0, 1080.0), &[], &workspaces);
    assert!(regions.is_empty());
    assert!(
        bar.liquid_glass_regions((1920.0, 1080.0), &[], &workspaces)
            .is_empty()
    );
    assert_eq!(bar.backdrop_blur_sigma(), 0.0);
}

#[test]
fn workspace_dot_states_use_size_and_brightness() {
    assert!(workspace_dot_diameter(true) > workspace_dot_diameter(false));
    let active_alpha = workspace_dot_color(true).components().3;
    let inactive_alpha = workspace_dot_color(false).components().3;
    assert!(active_alpha > inactive_alpha);
}

#[test]
fn status_icons_follow_the_reported_state() {
    assert_eq!(
        network_icon_name(NetworkState::Wired),
        "network-wired-symbolic"
    );
    assert_eq!(
        network_icon_name(NetworkState::Wifi),
        "network-wireless-signal-excellent-symbolic"
    );
    assert_eq!(
        network_icon_name(NetworkState::Offline),
        "network-offline-symbolic"
    );
}

#[test]
fn battery_icon_uses_nearest_available_theme_step() {
    assert_eq!(
        battery_icon_name(BatteryStatus {
            percent: 64,
            charging: false,
        }),
        "battery-level-60-symbolic"
    );
    assert_eq!(
        battery_icon_name(BatteryStatus {
            percent: 100,
            charging: true,
        }),
        "battery-level-90-charging-symbolic"
    );
}

#[test]
fn tray_fold_keeps_everything_within_budget() {
    let fold = fold_tray(5, 5);
    assert_eq!((fold.visible, fold.hidden), (5, 0));
    let fold = fold_tray(0, 5);
    assert_eq!((fold.visible, fold.hidden), (0, 0));
    // Exactly at budget no indicator slot is reserved.
    let fold = fold_tray(5, 5);
    assert_eq!(fold.hidden, 0);
}

#[test]
fn tray_fold_reserves_one_slot_for_registered_sni_overflow() {
    let fold = fold_tray(7, 5);
    assert_eq!((fold.visible, fold.hidden), (4, 3));
}
