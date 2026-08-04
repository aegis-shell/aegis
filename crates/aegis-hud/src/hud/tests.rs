use super::*;
use aegis_shell::Reserved;

fn workspaces_empty() -> WorkspaceSnapshot {
    WorkspaceSnapshot {
        outputs: Vec::new(),
    }
}

fn workspaces_with(count: usize, current: usize) -> WorkspaceSnapshot {
    use aegis_model::workspace::{OutputId, OutputSnapshot, WorkspaceEntry, WorkspaceId};

    let entries = (0..count)
        .map(|index| WorkspaceEntry {
            id: WorkspaceId(index as u64),
            label: None,
            tiled: false,
            toplevels: Vec::new(),
        })
        .collect::<Vec<_>>();
    WorkspaceSnapshot {
        outputs: vec![OutputSnapshot {
            id: OutputId(0),
            connector: "nested".to_owned(),
            current: entries.get(current).map(|workspace| workspace.id),
            workspaces: entries,
        }],
    }
}

#[test]
fn the_hud_never_reserves_space_or_captures_the_pointer() {
    let bar = Hud::new();
    let workspaces = workspaces_empty();
    assert!(bar.persistent_decoration());
    assert!(bar.visible_during_modal());
    assert_eq!(bar.reserved(), Reserved::default());
    assert!(!bar.captures_pointer(10.0, 10.0, (1920.0, 1080.0), &[], &workspaces));
    assert!(!bar.captures_pointer(960.0, 4.0, (1920.0, 1080.0), &[], &workspaces));
}

#[test]
fn fullscreen_window_makes_the_hud_composition_free() {
    let mut bar = Hud::new();
    bar.layout.visible = [true, true];
    bar.chip_fade = [1.0, 0.5];
    bar.chip_target = [0.0, 1.0];
    let mut fullscreen = Window::new(aegis_model::window::WindowId(7));
    fullscreen.state.fullscreen = true;
    bar.update_windows(&[fullscreen]);

    let workspaces = workspaces_empty();
    assert!(bar.fullscreen_active);
    assert!(!bar.requires_composition());
    assert_eq!(bar.backdrop_blur_sigma(), 0.0);
    assert!(
        bar.backdrop_regions((1920.0, 1080.0), &[], &workspaces)
            .is_empty()
    );
    assert!(
        bar.liquid_glass_regions((1920.0, 1080.0), &[], &workspaces)
            .is_empty()
    );
    assert!(!bar.anim_pending());

    bar.update_windows(&[]);
    assert!(!bar.fullscreen_active);
}

#[test]
fn maximized_window_keeps_the_hud_visible() {
    let mut bar = Hud::new();
    bar.layout.visible = [true, true];
    bar.chip_fade = [1.0, 0.5];
    bar.chip_target = [0.0, 1.0];
    let mut maximized = Window::new(aegis_model::window::WindowId(7));
    maximized.state.maximized = true;
    bar.update_windows(&[maximized]);

    let workspaces = workspaces_empty();
    assert!(!bar.fullscreen_active);
    assert!(bar.requires_composition());
    assert_eq!(bar.backdrop_blur_sigma(), BACKDROP_BLUR_SIGMA);
    assert_eq!(
        bar.backdrop_regions((1920.0, 1080.0), &[], &workspaces)
            .len(),
        2
    );
    assert_eq!(
        bar.liquid_glass_regions((1920.0, 1080.0), &[], &workspaces)
            .len(),
        2
    );
    assert!(bar.anim_pending());
}

#[test]
fn minimized_immersive_windows_do_not_hide_the_hud() {
    let mut bar = Hud::new();
    bar.layout.visible = [true, false];
    bar.chip_fade = [1.0, 1.0];
    bar.chip_target = bar.chip_fade;

    let mut minimized_fullscreen = Window::new(aegis_model::window::WindowId(8));
    minimized_fullscreen.state.fullscreen = true;
    minimized_fullscreen.minimized = true;
    let mut minimized_maximized = Window::new(aegis_model::window::WindowId(9));
    minimized_maximized.state.maximized = true;
    minimized_maximized.minimized = true;
    bar.update_windows(&[minimized_fullscreen, minimized_maximized]);

    let workspaces = workspaces_empty();
    assert!(!bar.fullscreen_active);
    assert!(bar.requires_composition());
    assert_eq!(bar.backdrop_blur_sigma(), BACKDROP_BLUR_SIGMA);
    assert_eq!(
        bar.backdrop_regions((1920.0, 1080.0), &[], &workspaces)
            .len(),
        1
    );
}

#[test]
fn ordinary_window_preserves_normal_hud_output() {
    let mut bar = Hud::new();
    bar.layout.visible = [true, false];
    bar.chip_fade = [1.0, 1.0];
    bar.chip_target = bar.chip_fade;
    bar.update_windows(&[Window::new(aegis_model::window::WindowId(10))]);

    assert!(!bar.fullscreen_active);
    assert!(bar.requires_composition());
    assert_eq!(bar.backdrop_blur_sigma(), BACKDROP_BLUR_SIGMA);
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
fn workspace_indicator_follows_the_shared_proximity_fade_to_hidden() {
    let mut bar = Hud::new();
    bar.layout.visible = [false, true];
    bar.layout.chips[CENTER] = Rect {
        x: 900.0,
        y: 8.0,
        w: 120.0,
        h: 32.0,
    };
    for _ in 0..600 {
        bar.advance_fade(0.016, (960.0, 20.0));
    }
    assert_eq!(bar.chip_target[CENTER], 0.0);
    assert_eq!(bar.chip_fade[CENTER], 0.0);
}

#[test]
fn workspace_indicator_has_a_preview_slot_and_animates_current_position() {
    assert_eq!(
        workspace_indicator_state(&workspaces_with(1, 0)),
        Some((2, 0))
    );
    assert_eq!(
        workspace_indicator_state(&workspaces_with(3, 2)),
        Some((3, 2))
    );

    let mut bar = Hud::new();
    bar.advance_workspace_position(0.016, &workspaces_with(3, 0));
    assert_eq!(bar.workspace_position, 0.0);
    bar.advance_workspace_position(0.016, &workspaces_with(3, 2));
    assert!(bar.workspace_position > 0.0 && bar.workspace_position < 2.0);
    assert_eq!(bar.workspace_target, 2.0);
    assert!(bar.anim_pending());

    bar.set_reduced_motion(true);
    assert_eq!(bar.workspace_position, 2.0);
}

#[test]
fn workspace_indicator_width_tracks_its_sphere_count() {
    let two = workspace_indicator_width(2);
    let three = workspace_indicator_width(3);
    assert_eq!(
        two,
        WORKSPACE_DOT_DIAMETER * 2.0 + WORKSPACE_DOT_GAP + CHIP_PAD_X * 2.0
    );
    assert_eq!(three - two, WORKSPACE_DOT_DIAMETER + WORKSPACE_DOT_GAP);
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
    assert!(bar.requires_composition());
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
    assert!(
        !bar.requires_composition(),
        "fully faded HUD output must not block primary-plane scanout"
    );
}

#[test]
fn workspace_spheres_use_brightness_alone_to_show_current_position() {
    assert!(workspace_dot_diameter() > 0.0);
    assert_eq!(workspace_dot_intensity(0, 0.0), 1.0);
    assert_eq!(workspace_dot_intensity(1, 0.0), 0.0);
    assert_eq!(workspace_dot_intensity(0, 0.5), 0.5);
    assert_eq!(workspace_dot_intensity(1, 0.5), 0.5);
    let active_alpha = workspace_dot_color(1.0).components().3;
    let inactive_alpha = workspace_dot_color(0.0).components().3;
    assert!(active_alpha > inactive_alpha);
}

#[test]
fn workspace_indicator_emits_visible_pixels_above_its_glass_body() {
    const WIDTH: usize = 800;
    const HEIGHT: usize = 80;
    let Ok(device) = flux::Device::new(true, &[], &[], 1) else {
        return;
    };
    let Ok(surface) = flux::Surface::offscreen_readback(&device, WIDTH as u32, HEIGHT as u32)
    else {
        return;
    };
    let canvas = flux::Canvas::new(&surface).unwrap();
    let mut shell = unsafe { aegis_shell::Shell::new(device.as_raw().cast()) }.unwrap();
    shell.add(Box::new(Hud::new()));
    shell.set_workspaces(workspaces_with(1, 0));
    let mut input = lens::Input::new((WIDTH as f32, HEIGHT as f32), 1.0 / 60.0);
    input.set_cursor(10.0, 70.0);
    shell.prepare_backdrop(&input);

    let frame = surface.begin_frame().unwrap();
    canvas
        .begin(&frame, Some(flux::rgba(0, 0, 0, 255)))
        .unwrap();
    unsafe {
        shell
            .render(canvas.as_raw().cast(), &input)
            .expect("HUD must render into the offscreen canvas");
    }
    canvas.end_checked().unwrap();
    frame.submit().unwrap().present().unwrap();

    let mut pixels = vec![0; WIDTH * HEIGHT * 4];
    surface.read_pixels(&mut pixels).unwrap();
    let luminance = |x: usize, y: usize| {
        let offset = (y * WIDTH + x) * 4;
        u16::from(pixels[offset]) + u16::from(pixels[offset + 1]) + u16::from(pixels[offset + 2])
    };
    // Two slots make a 45 px chip centered at x=377.5. The active 7 px
    // sphere is centered at roughly (391, 24); x=380 is glass-only.
    assert!(
        luminance(391, 24) > luminance(380, 24) + 120,
        "the active workspace sphere must remain visible over its glass body",
    );
}

#[test]
fn floating_foregrounds_share_one_contour_with_geometry_specific_widths() {
    let text = hud_text_outline(1.0);
    let glyph = hud_glyph_outline(1.0);
    assert_eq!(text.color, glyph.color);
    assert!(text.width < glyph.width);
    assert_eq!(text.color, hud_contour_color());

    let faded = hud_text_outline(0.5);
    assert!(faded.color.components().3 < text.color.components().3);
    assert_eq!(faded.width, text.width);
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
