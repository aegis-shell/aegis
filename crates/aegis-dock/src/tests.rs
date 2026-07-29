use super::rendering::entry_matches_app_id;
use super::*;

fn app(id: &str) -> Entry {
    Entry {
        id: id.to_string(),
        ..Default::default()
    }
}

/// Register `pinned` entries the way the composition root does: one
/// catalog push that rebuilds the pinned list from [`Entry::match_keys`].
fn dock_with(pinned: Vec<Entry>) -> Dock {
    let mut dock = Dock::new();
    dock.update_app_catalog(&AppCatalog {
        apps: pinned.clone(),
        pinned,
        icons: IconSet::default(),
    });
    dock
}

fn window(id: u64, app_id: &str, activated: bool) -> Window {
    let mut w = Window {
        id: aegis_core::window::WindowId(id),
        app_id: Some(app_id.to_string()),
        ..Default::default()
    };
    w.state.activated = activated;
    w
}

fn workspace_snapshot() -> WorkspaceSnapshot {
    WorkspaceSnapshot {
        outputs: Vec::new(),
    }
}

#[test]
fn magnify_factor_is_one_at_cursor() {
    assert!(Dock::magnify_factor(0.0) > 0.999);
}

#[test]
fn magnify_factor_is_zero_outside_radius() {
    let radius = MAGNIFY_RADIUS_TILES * DOCK_TILE;
    assert_eq!(Dock::magnify_factor(radius), 0.0);
    assert_eq!(Dock::magnify_factor(radius + 1.0), 0.0);
    assert_eq!(Dock::magnify_factor(-radius), 0.0);
}

#[test]
fn magnify_factor_is_symmetric() {
    for d in [5.0, 12.0, 33.0, 50.0] {
        let r = MAGNIFY_RADIUS_TILES * DOCK_TILE;
        if d < r {
            assert!((Dock::magnify_factor(d) - Dock::magnify_factor(-d)).abs() < 1e-5);
        }
    }
}

#[test]
fn spring_approaches_target_at_rest() {
    // No time elapses → nothing moves.
    let mut s = SpringState {
        value: 10.0,
        vel: 0.0,
    };
    assert_eq!(Dock::spring(&mut s, 20.0, 0.0), 10.0);
}

#[test]
fn spring_settles_on_target() {
    // Many small steps from rest must converge to the target.
    let mut s = SpringState {
        value: 10.0,
        vel: 0.0,
    };
    for _ in 0..2000 {
        Dock::spring(&mut s, 20.0, 1.0 / 120.0);
    }
    assert!((s.value - 20.0).abs() < 0.01, "settled at {}", s.value);
}

#[test]
fn spring_overshoots_then_settles() {
    // Under-damped: from rest it should cross past the target at least once
    // before settling (the macOS lift-and-bounce).
    let mut s = SpringState {
        value: 0.0,
        vel: 0.0,
    };
    let mut overshot = false;
    for _ in 0..2000 {
        Dock::spring(&mut s, 100.0, 1.0 / 120.0);
        if s.value > 100.0 {
            overshot = true;
        }
    }
    assert!(overshot, "spring never overshot the target");
    assert!((s.value - 100.0).abs() < 0.01, "settled at {}", s.value);
}

#[test]
fn spring_is_dt_stable() {
    // A single large step (a long frame stall) must not blow up.
    let mut s = SpringState {
        value: 0.0,
        vel: 0.0,
    };
    let v = Dock::spring(&mut s, 100.0, 1.0 / 5.0);
    assert!(v.is_finite(), "value diverged: {v}");
    assert!(s.vel.is_finite(), "velocity diverged: {}", s.vel);
}

#[test]
fn pinned_apps_show_without_any_running_window() {
    let dock = dock_with(vec![app("firefox.desktop"), app("term.desktop")]);
    let tiles = dock.tiles(&[]);
    assert_eq!(
        tiles.len(),
        2,
        "both pinned apps are tiles even with no windows"
    );
    assert!(tiles.iter().all(|t| !t.running));
    // No running window → clicking launches (spawn), not focus.
    assert!(tiles.iter().all(|t| t.spawn.is_some() && t.focus.is_none()));
}

#[test]
fn running_window_folds_into_its_pinned_tile() {
    let dock = dock_with(vec![app("firefox.desktop")]);
    let tiles = dock.tiles(&[window(7, "firefox", true)]);
    assert_eq!(
        tiles.len(),
        1,
        "the window folds into the pinned tile, not a new one"
    );
    assert!(tiles[0].running);
    assert!(tiles[0].activated);
    assert_eq!(
        tiles[0].focus,
        Some(aegis_core::window::WindowId(7)),
        "clicking focuses the running window"
    );
    assert!(tiles[0].spawn.is_none());
}

#[test]
fn multiple_running_windows_fold_into_one_tile_with_multiple_instances() {
    let dock = dock_with(vec![app("firefox.desktop")]);
    let tiles = dock.tiles(&[window(7, "firefox", true), window(8, "firefox", false)]);
    assert_eq!(tiles.len(), 1);
    assert_eq!(tiles[0].windows.len(), 2);
}

#[test]
fn unpinned_running_window_is_appended() {
    let dock = dock_with(vec![app("firefox.desktop")]);
    let tiles = dock.tiles(&[window(3, "gimp", false)]);
    assert_eq!(
        tiles.len(),
        2,
        "pinned firefox plus the unpinned gimp window"
    );
    let gimp = tiles.iter().find(|t| t.key == "win:3").expect("gimp tile");
    assert!(gimp.running);
    assert!(!gimp.pinned, "the window tile is transient, not kept");
    assert_eq!(gimp.focus, Some(aegis_core::window::WindowId(3)));
}

#[test]
fn pointer_bounds_stay_at_rest_while_tiles_are_magnified() {
    let mut dock = dock_with(vec![app("firefox.desktop")]);
    dock.sizes.insert(
        "launchpad".into(),
        SpringState {
            value: DOCK_TILE_MAX,
            vel: 0.0,
        },
    );
    dock.sizes.insert(
        "app:firefox.desktop".into(),
        SpringState {
            value: DOCK_TILE_MAX,
            vel: 0.0,
        },
    );

    let display = (1920.0, 1080.0);
    let bounds = dock.pointer_bounds(&[], display);
    let visual_bounds = dock.visual_panel_bounds(&[], display);
    let expected_width = 2.0 * DOCK_TILE + DOCK_TILE_GAP + 2.0 * DOCK_PAD;
    assert_eq!(bounds.w, expected_width);
    assert_eq!(
        visual_bounds.w,
        2.0 * DOCK_TILE_MAX + DOCK_TILE_GAP + 2.0 * DOCK_PAD
    );
    assert_eq!(bounds.y, display.1 - DOCK_PANEL_HEIGHT - DOCK_BOTTOM_MARGIN);
    assert_eq!(bounds.h, DOCK_PANEL_HEIGHT);
    assert!(!dock.captures_pointer(
        display.0 * 0.5,
        bounds.y - 1.0,
        display,
        &[],
        &workspace_snapshot(),
    ));
}

#[test]
fn read_only_mirror_has_no_physical_focus_action() {
    let dock = Dock::new();
    let mut mirror = window(7, "org.example.App", false);
    mirror.read_only = true;
    let tiles = dock.tiles(&[mirror]);
    assert_eq!(tiles.len(), 1);
    assert_eq!(tiles[0].focus, None);
    assert_eq!(tiles[0].windows, vec![aegis_core::window::WindowId(7)]);
}

#[test]
fn window_invading_rest_bounds_starts_an_animated_collapse() {
    let mut dock = dock_with(vec![app("org.example.Game.desktop")]);
    let display = (1920.0, 1080.0);
    dock.last_display = Some(display);
    let mut invading = window(7, "org.example.Game", true);
    let bounds = dock.pointer_bounds(std::slice::from_ref(&invading), display);
    invading.position = aegis_core::Point {
        x: bounds.x.round() as i32 + 10,
        y: bounds.y.round() as i32 - 10,
    };
    invading.size = aegis_core::Size { w: 120, h: 40 };
    dock.update_windows(std::slice::from_ref(&invading));

    assert!(dock.dock_obscured);
    assert!(dock.effective_autohide());
    assert_eq!(
        dock.autohide_reveal, 1.0,
        "collision starts an animation instead of teleporting the Dock"
    );
    assert!(dock.collapse_pending);
    assert!(!dock.hidden_trigger_armed);
    assert_eq!(dock.reserved(), Reserved::default());

    invading.position.y = bounds.y.round() as i32 - 80;
    invading.size.h = 40;
    dock.update_windows(&[invading]);
    assert!(!dock.dock_obscured);
    assert!(!dock.effective_autohide());
    assert_eq!(
        dock.reserved().bottom,
        (DOCK_PANEL_HEIGHT + DOCK_BOTTOM_MARGIN) as i32
    );
}

#[test]
fn maximized_state_without_geometric_invasion_keeps_the_dock_reserved() {
    let mut dock = dock_with(vec![app("org.example.Game.desktop")]);
    let display = (1920.0, 1080.0);
    dock.last_display = Some(display);
    let mut maximized = window(7, "org.example.Game", true);
    maximized.state.maximized = true;
    maximized.position = aegis_core::Point { x: 100, y: 100 };
    maximized.size = aegis_core::Size { w: 1000, h: 700 };
    dock.update_windows(&[maximized]);

    assert_eq!(dock.space_use, SpaceUse::Maximized);
    assert!(!dock.dock_obscured);
    assert!(!dock.effective_autohide());
    assert_eq!(
        dock.reserved().bottom,
        (DOCK_PANEL_HEIGHT + DOCK_BOTTOM_MARGIN) as i32
    );
    assert_eq!(dock.backdrop_blur_sigma(), 12.0);
}

#[test]
fn minimized_window_does_not_count_as_a_dock_invasion() {
    let bounds = Dock::rest_bounds(1, 1, (1920.0, 1080.0));
    let mut minimized = window(7, "org.example.Game", false);
    minimized.position = aegis_core::Point {
        x: bounds.x.round() as i32,
        y: bounds.y.round() as i32,
    };
    minimized.size = aegis_core::Size { w: 100, h: 50 };
    minimized.minimized = true;
    assert!(!Dock::window_overlaps_bounds(&minimized, bounds));
}

#[test]
fn dock_surface_morphs_between_panel_and_exact_handle_geometry() {
    let display = (1920.0, 1080.0);
    let expanded_width = 320.0;
    let expanded = Dock::collapsed_panel_rect(display, expanded_width, 1.0);
    assert_eq!(expanded.w, expanded_width);
    assert_eq!(expanded.h, DOCK_PANEL_HEIGHT);
    assert_eq!(
        expanded.y,
        display.1 - DOCK_BOTTOM_MARGIN - DOCK_PANEL_HEIGHT
    );

    let collapsed = Dock::collapsed_panel_rect(display, expanded_width, 0.0);
    assert_eq!(collapsed.w, AUTOHIDE_HANDLE_WIDTH);
    assert_eq!(collapsed.h, AUTOHIDE_HANDLE_HEIGHT);
    assert_eq!(
        collapsed.y,
        display.1 - DOCK_BOTTOM_MARGIN - AUTOHIDE_HANDLE_HEIGHT
    );
    assert_eq!(expanded.y + expanded.h, collapsed.y + collapsed.h);
}

#[test]
fn dock_content_finishes_draining_before_surface_becomes_handle() {
    assert_eq!(Dock::collapse_content_progress(1.0), 1.0);
    assert_eq!(
        Dock::collapse_content_progress(AUTOHIDE_CONTENT_DRAIN_END),
        0.0
    );
    assert_eq!(Dock::collapse_content_progress(0.0), 0.0);
    assert!(
        Dock::collapse_surface_progress(AUTOHIDE_CONTENT_DRAIN_END) > 0.0,
        "the glass surface must still be visibly morphing after its icons drain"
    );
}

#[test]
fn fullscreen_window_locks_dock_hidden_without_hot_edge() {
    let mut dock = Dock::new();
    let mut fullscreen = window(7, "org.example.Game", true);
    fullscreen.state.fullscreen = true;
    dock.update_windows(&[fullscreen]);

    assert_eq!(dock.space_use, SpaceUse::Fullscreen);
    assert_eq!(dock.autohide_reveal, 0.0);
    assert_eq!(dock.reserved(), Reserved::default());
    assert_eq!(dock.backdrop_blur_sigma(), 0.0);
    assert!(
        dock.backdrop_regions((1920.0, 1080.0), &[], &workspace_snapshot())
            .is_empty()
    );
    assert!(!dock.captures_pointer(960.0, 1079.0, (1920.0, 1080.0), &[], &workspace_snapshot(),));
}

#[test]
fn fullscreen_policy_wins_and_minimized_windows_do_not_hide_dock() {
    let mut dock = Dock::new();
    let mut maximized = window(7, "org.example.Editor", true);
    maximized.state.maximized = true;
    let mut fullscreen = window(8, "org.example.Game", false);
    fullscreen.state.fullscreen = true;
    dock.update_windows(&[maximized, fullscreen.clone()]);
    assert_eq!(dock.space_use, SpaceUse::Fullscreen);

    fullscreen.minimized = true;
    dock.update_windows(&[fullscreen]);
    assert_eq!(dock.space_use, SpaceUse::Available);
    assert_eq!(
        dock.reserved().bottom,
        (DOCK_PANEL_HEIGHT + DOCK_BOTTOM_MARGIN) as i32
    );
}

#[test]
fn entry_matches_app_id_like_the_launcher_heuristic() {
    let mut e = Entry {
        id: "org.mozilla.firefox.desktop".to_string(),
        icon: Some("firefox-icon".to_string()),
        ..Default::default()
    };
    e.startup_wm_class = Some("Firefox".to_string());
    assert!(entry_matches_app_id(&e, "firefox")); // WM class, case-insensitive
    assert!(entry_matches_app_id(&e, "org.mozilla.firefox")); // desktop-id stem
    assert!(entry_matches_app_id(&e, "Firefox-Icon")); // icon name
    assert!(!entry_matches_app_id(&e, "chromium"));
    assert!(!entry_matches_app_id(&e, ""));
}

#[test]
fn rest_centres_include_the_section_gap_for_transient_tiles() {
    // 2 pinned tiles (incl. launchpad) + 1 transient window tile.
    let pinned = Dock::rest_centre_estimate(1, 3, 2, 1920.0);
    let transient = Dock::rest_centre_estimate(2, 3, 2, 1920.0);
    let pitch = DOCK_TILE + DOCK_TILE_GAP;
    assert!(
        (transient - pinned - pitch - DOCK_SECTION_GAP).abs() < 1e-5,
        "the first transient tile sits one pitch plus the section gap right of the last pinned tile"
    );
    // No transient tiles → no extra gap.
    let a = Dock::rest_centre_estimate(1, 2, 2, 1920.0);
    let b = Dock::rest_centre_estimate(0, 2, 2, 1920.0);
    assert!((a - b - pitch).abs() < 1e-5);
}
