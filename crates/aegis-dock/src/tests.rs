use super::rendering::{
    entry_matches_app_id, hit_test_tiles, live_preview_hit, live_preview_layout, snapped_hairline,
    strip_centres,
};
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
        position: DockPosition::Bottom,
    });
    dock
}

fn window(id: u64, app_id: &str, activated: bool) -> Window {
    let mut w = Window {
        id: aegis_model::window::WindowId(id),
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
fn dock_is_persistent_decoration() {
    let dock = Dock::new();
    assert!(dock.persistent_decoration());
    assert!(dock.visible_during_modal());
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
fn spring_remains_bounded_and_settles_at_thirty_fps() {
    let mut s = SpringState {
        value: DOCK_TILE,
        vel: 0.0,
    };
    for _ in 0..300 {
        Dock::spring(&mut s, DOCK_TILE_MAX, 1.0 / 30.0);
        assert!(
            s.value >= 0.0 && s.value <= DOCK_TILE_MAX * 2.0,
            "spring escaped its visual range: {}",
            s.value
        );
    }
    assert!((s.value - DOCK_TILE_MAX).abs() < 0.01);
    assert!(s.vel.abs() < 0.01);
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
        Some(aegis_model::window::WindowId(7)),
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
    let gimp = tiles
        .iter()
        .find(|t| t.key == "transient:gimp")
        .expect("gimp tile");
    assert!(gimp.running);
    assert!(!gimp.pinned, "the window tile is transient, not kept");
    assert_eq!(gimp.focus, Some(aegis_model::window::WindowId(3)));
}

#[test]
fn tile_strip_uses_the_workspace_global_window_list() {
    let mut dock = dock_with(vec![app("firefox.desktop")]);
    let visible = window(7, "firefox", true);
    let hidden = window(3, "gimp", false);

    // The visible-set push feeds only the autohide/SpaceUse policy; the strip
    // must not grow from it.
    dock.update(ChromeUpdate::Windows(std::slice::from_ref(&visible)));
    assert_eq!(
        dock.tiles(&dock.all_windows).len(),
        1,
        "only the pinned app before any global snapshot"
    );

    // The workspace-global push builds the strip: the pinned tile folds its
    // running window and a window on another workspace gets a transient tile.
    dock.update(ChromeUpdate::AllWindows(&[visible.clone(), hidden]));
    let tiles = dock.tiles(&dock.all_windows);
    assert_eq!(tiles.len(), 2);
    let firefox = tiles
        .iter()
        .find(|t| t.key == "app:firefox.desktop")
        .unwrap();
    assert!(firefox.running && firefox.activated);
    let gimp = tiles.iter().find(|t| t.key == "transient:gimp").unwrap();
    assert!(gimp.running && !gimp.pinned);
    assert_eq!(gimp.focus, Some(aegis_model::window::WindowId(3)));

    // A later visible-set push without the hidden window must not drop its
    // tile; only the global list owns strip membership.
    dock.update(ChromeUpdate::Windows(std::slice::from_ref(&visible)));
    assert!(
        dock.tiles(&dock.all_windows)
            .iter()
            .any(|t| t.key == "transient:gimp")
    );

    // The transient tile leaves when the window leaves the global list.
    dock.update(ChromeUpdate::AllWindows(std::slice::from_ref(&visible)));
    assert!(
        !dock
            .tiles(&dock.all_windows)
            .iter()
            .any(|t| t.key == "transient:gimp")
    );
}

#[test]
fn scale_update_is_retained_for_hairline_snapping() {
    let mut dock = Dock::new();
    assert_eq!(dock.scale, 1.0);
    dock.update(ChromeUpdate::Scale(2.0));
    assert_eq!(dock.scale, 2.0);
}

#[test]
fn hairline_snaps_to_the_device_pixel_grid() {
    // Scale 2: a fractional center lands on a device-pixel boundary and the
    // width is exactly one device pixel.
    let (center, width) = snapped_hairline(100.3, 2.0);
    assert_eq!((center, width), (100.5, 0.5));
    // Scale 1: a 1 logical px line at (within half a pixel of) the same
    // center — the pre-snap appearance.
    let (center, width) = snapped_hairline(100.3, 1.0);
    assert_eq!((center, width), (100.0, 1.0));
    // An already-aligned center is untouched.
    let (center, width) = snapped_hairline(100.0, 2.0);
    assert_eq!((center, width), (100.0, 0.5));
}

#[test]
fn unpinned_windows_keep_open_order_when_focus_reorders_snapshot() {
    let dock = Dock::new();
    let first = dock.tiles(&[window(1, "first", false), window(2, "second", true)]);
    assert_eq!(
        first
            .iter()
            .map(|tile| tile.key.as_str())
            .collect::<Vec<_>>(),
        vec!["transient:first", "transient:second"]
    );

    let reordered = dock.tiles(&[window(2, "second", false), window(1, "first", true)]);
    assert_eq!(
        reordered
            .iter()
            .map(|tile| tile.key.as_str())
            .collect::<Vec<_>>(),
        vec!["transient:first", "transient:second"],
        "focus/stacking order must not move a transient Dock tile"
    );
    assert!(reordered[0].activated);
    assert!(!reordered[1].activated);

    let after_close_and_open = dock.tiles(&[window(3, "third", true), window(2, "second", false)]);
    assert_eq!(
        after_close_and_open
            .iter()
            .map(|tile| tile.key.as_str())
            .collect::<Vec<_>>(),
        vec!["transient:second", "transient:third"],
        "a newly opened application is appended after surviving transient tiles"
    );
}

#[test]
fn unpinned_windows_of_one_app_fold_into_one_tile() {
    let dock = Dock::new();
    let tiles = dock.tiles(&[
        window(1, "gimp", false),
        window(2, "gimp", true),
        window(3, "other", false),
    ]);
    assert_eq!(tiles.len(), 2, "one tile per application, pinned or not");
    let gimp = &tiles[0];
    assert_eq!(gimp.key, "transient:gimp");
    assert!(gimp.running && gimp.activated);
    assert_eq!(
        gimp.windows,
        vec![
            aegis_model::window::WindowId(2),
            aegis_model::window::WindowId(1)
        ],
        "the activated window leads the group"
    );
    assert_eq!(gimp.focus, Some(aegis_model::window::WindowId(2)));
    assert_eq!(gimp.pin_entry, None, "no desktop entry in the catalog");
    assert!(!tiles[1].activated);
}

#[test]
fn unpinned_app_resolves_its_desktop_entry_for_pinning() {
    let mut dock = Dock::new();
    dock.update_app_catalog(&AppCatalog {
        apps: vec![app("gimp.desktop")],
        pinned: vec![],
        icons: IconSet::default(),
        position: DockPosition::Bottom,
    });
    let tiles = dock.tiles(&[window(1, "Gimp", false)]);
    assert_eq!(tiles.len(), 1);
    assert_eq!(tiles[0].key, "transient:gimp.desktop");
    assert_eq!(tiles[0].pin_entry.as_deref(), Some("gimp.desktop"));
}

#[test]
fn a_window_without_app_id_stands_alone() {
    let dock = Dock::new();
    let mut lone = window(1, "gimp", false);
    lone.app_id = None;
    lone.title = Some("Untitled".to_string());
    let tiles = dock.tiles(&[lone]);
    assert_eq!(tiles.len(), 1);
    assert_eq!(tiles[0].key, "win:1");
    assert_eq!(tiles[0].pin_entry, None);
    assert_eq!(tiles[0].label, "Untitled");
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
    let bounds = dock.pointer_bounds(display);
    let visual_bounds = dock.visual_panel_bounds(display);
    let expected_width = 2.0 * DOCK_TILE + DOCK_TILE_GAP + 2.0 * DOCK_PAD;
    assert_eq!(bounds.w, expected_width);
    assert_eq!(
        visual_bounds.w,
        2.0 * DOCK_TILE_MAX + DOCK_TILE_GAP + 2.0 * DOCK_PAD
    );
    assert_eq!(bounds.y, display.1 - DOCK_PANEL_HEIGHT - DOCK_EDGE_MARGIN);
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
    assert_eq!(tiles[0].windows, vec![aegis_model::window::WindowId(7)]);
}

#[test]
fn window_invading_rest_bounds_starts_an_animated_collapse() {
    let mut dock = dock_with(vec![app("org.example.Game.desktop")]);
    let display = (1920.0, 1080.0);
    dock.last_display = Some(display);
    let mut invading = window(7, "org.example.Game", true);
    let bounds = dock.pointer_bounds(display);
    invading.position = aegis_model::Point {
        x: bounds.x.round() as i32 + 10,
        y: bounds.y.round() as i32 - 10,
    };
    invading.size = aegis_model::Size { w: 120, h: 40 };
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
        (DOCK_PANEL_HEIGHT + DOCK_EDGE_MARGIN) as i32
    );
}

#[test]
fn maximized_state_forces_an_animated_collapse_without_geometric_invasion() {
    let mut dock = dock_with(vec![app("org.example.Game.desktop")]);
    let display = (1920.0, 1080.0);
    dock.last_display = Some(display);
    let mut maximized = window(7, "org.example.Game", true);
    maximized.state.maximized = true;
    maximized.position = aegis_model::Point { x: 100, y: 100 };
    maximized.size = aegis_model::Size { w: 1000, h: 700 };
    dock.update_windows(std::slice::from_ref(&maximized));

    assert_eq!(dock.space_use, SpaceUse::Maximized);
    assert!(!dock.dock_obscured);
    assert!(dock.effective_autohide());
    assert_eq!(dock.autohide_reveal, 1.0);
    assert!(dock.collapse_pending);
    assert!(!dock.hidden_trigger_armed);
    assert!(dock.anim_pending());
    assert_eq!(dock.reserved(), Reserved::default());
    assert_eq!(dock.backdrop_blur_sigma(), 12.0);

    let mut restored = maximized;
    restored.state.maximized = false;
    dock.update_windows(&[restored]);
    assert_eq!(dock.space_use, SpaceUse::Available);
    assert!(!dock.effective_autohide());
    assert!(!dock.collapse_pending);
    assert_eq!(
        dock.reserved().bottom,
        (DOCK_PANEL_HEIGHT + DOCK_EDGE_MARGIN) as i32
    );
}

#[test]
fn disabling_user_autohide_does_not_override_maximized_collapse() {
    let mut dock = Dock::new();
    let mut maximized = window(7, "org.example.Game", true);
    maximized.state.maximized = true;
    dock.update_windows(&[maximized]);
    dock.autohide_reveal = 0.0;

    dock.set_autohide(true);
    dock.set_autohide(false);

    assert!(dock.effective_autohide());
    assert_eq!(dock.autohide_reveal, 0.0);
    assert_eq!(dock.reserved(), Reserved::default());
}

#[test]
fn minimized_window_does_not_count_as_a_dock_invasion() {
    let bounds = Dock::rest_bounds(1, 1, DockPosition::Bottom, (1920.0, 1080.0));
    let mut minimized = window(7, "org.example.Game", false);
    minimized.position = aegis_model::Point {
        x: bounds.x.round() as i32,
        y: bounds.y.round() as i32,
    };
    minimized.size = aegis_model::Size { w: 100, h: 50 };
    minimized.minimized = true;
    assert!(!Dock::window_overlaps_bounds(&minimized, bounds));
}

#[test]
fn dock_surface_morphs_between_panel_and_exact_handle_geometry() {
    let display = (1920.0, 1080.0);
    let expanded_width = 320.0;
    let expanded = Dock::collapsed_panel_rect(DockPosition::Bottom, display, expanded_width, 1.0);
    assert_eq!(expanded.w, expanded_width);
    assert_eq!(expanded.h, DOCK_PANEL_HEIGHT);
    assert_eq!(expanded.y, display.1 - DOCK_EDGE_MARGIN - DOCK_PANEL_HEIGHT);

    let collapsed = Dock::collapsed_panel_rect(DockPosition::Bottom, display, expanded_width, 0.0);
    assert_eq!(collapsed.w, AUTOHIDE_HANDLE_WIDTH);
    assert_eq!(collapsed.h, AUTOHIDE_HANDLE_HEIGHT);
    assert_eq!(
        collapsed.y,
        display.1 - DOCK_EDGE_MARGIN - AUTOHIDE_HANDLE_HEIGHT
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
fn collapsed_handle_has_no_tile_hover_target() {
    let display = (1920.0, 1080.0);
    let rest_bounds = Dock::rest_bounds(1, 1, DockPosition::Bottom, display);
    let panel_rect = Dock::collapsed_panel_rect(DockPosition::Bottom, display, rest_bounds.w, 0.0);
    let old_icon_rect = Rect {
        x: display.0 * 0.5 - DOCK_TILE * 0.5,
        y: rest_bounds.y + 5.0,
        w: DOCK_TILE,
        h: DOCK_TILE,
    };
    let cursor = (display.0 * 0.5, old_icon_rect.y + old_icon_rect.h * 0.5);

    assert_eq!(
        hit_test_tiles(
            cursor,
            rest_bounds,
            rest_bounds,
            1.0,
            &[old_icon_rect],
            false
        ),
        Some(0),
        "the same point owns the visible tile while the Dock is expanded"
    );
    assert!(
        hit_test_tiles(
            cursor,
            rest_bounds,
            panel_rect,
            Dock::collapse_content_progress(0.0),
            &[old_icon_rect],
            false,
        )
        .is_none()
    );

    let mut dock = Dock::new();
    dock.set_autohide(true);
    dock.autohide_reveal = 0.0;
    assert!(!dock.captures_pointer(cursor.0, cursor.1, display, &[], &workspace_snapshot(),));
    let indicator = Dock::collapsed_indicator_bounds(DockPosition::Bottom, display);
    assert!(dock.captures_pointer(
        indicator.x + indicator.w * 0.5,
        indicator.y + indicator.h * 0.5,
        display,
        &[],
        &workspace_snapshot(),
    ));
    assert!(!dock.captures_pointer(
        display.0 * 0.5,
        display.1 - 1.0,
        display,
        &[],
        &workspace_snapshot(),
    ));
}

#[test]
fn capsule_is_the_only_collapsed_reveal_target() {
    let display = (1920.0, 1080.0);
    let mut dock = Dock::new();
    dock.set_autohide(true);
    dock.autohide_reveal = 0.0;
    let indicator = Dock::collapsed_indicator_bounds(DockPosition::Bottom, display);
    let workspaces = workspace_snapshot();

    assert!(dock.captures_pointer(
        indicator.x + indicator.w * 0.5,
        indicator.y + indicator.h * 0.5,
        display,
        &[],
        &workspaces,
    ));
    for point in [
        (indicator.x - 1.0, indicator.y + indicator.h * 0.5),
        (
            indicator.x + indicator.w + 1.0,
            indicator.y + indicator.h * 0.5,
        ),
        (indicator.x + indicator.w * 0.5, indicator.y - 1.0),
        (
            indicator.x + indicator.w * 0.5,
            indicator.y + indicator.h + 1.0,
        ),
    ] {
        assert!(
            !dock.captures_pointer(point.0, point.1, display, &[], &workspaces),
            "point {point:?} outside the capsule must remain client-owned"
        );
    }
}

#[test]
fn collapsed_dock_does_not_reuse_its_old_resting_region_as_a_trigger() {
    let display = (1920.0, 1080.0);
    let dock = Dock::new();
    let rest = dock.pointer_bounds(display);
    let indicator = Dock::collapsed_indicator_bounds(DockPosition::Bottom, display);
    let old_dock_point = (rest.x + rest.w * 0.5, rest.y + 8.0);
    let capsule_point = (
        indicator.x + indicator.w * 0.5,
        indicator.y + indicator.h * 0.5,
    );

    assert!(!Dock::collapsed_indicator_contains(
        DockPosition::Bottom,
        old_dock_point,
        display
    ));
    assert!(!Dock::pointer_keeps_revealed(
        true,
        0.0,
        false,
        old_dock_point,
        rest,
        DockPosition::Bottom,
        display,
    ));
    assert!(Dock::pointer_keeps_revealed(
        true,
        0.0,
        true,
        capsule_point,
        rest,
        DockPosition::Bottom,
        display,
    ));
    assert!(Dock::pointer_keeps_revealed(
        true,
        1.0,
        false,
        old_dock_point,
        rest,
        DockPosition::Bottom,
        display,
    ));
}

#[test]
fn expanded_autohide_dock_keeps_pointer_across_bottom_gap() {
    let display = (1920.0, 1080.0);
    let mut dock = Dock::new();
    dock.set_autohide(true);
    dock.autohide_reveal = 1.0;
    let rest = dock.pointer_bounds(display);
    let gap_y = rest.y + rest.h + DOCK_EDGE_MARGIN * 0.5;
    let indicator = Dock::collapsed_indicator_bounds(DockPosition::Bottom, display);
    let former_indicator_edge_x = indicator.x + 1.0;

    assert!(Dock::expanded_trigger_contains(
        DockPosition::Bottom,
        (display.0 * 0.5, gap_y),
        rest,
        display,
    ));
    assert!(dock.captures_pointer(display.0 * 0.5, gap_y, display, &[], &workspace_snapshot(),));
    assert!(
        former_indicator_edge_x < rest.x,
        "the regression point must be outside the expanded panel"
    );
    assert!(!dock.captures_pointer(
        former_indicator_edge_x,
        indicator.y + indicator.h * 0.5,
        display,
        &[],
        &workspace_snapshot(),
    ));
    assert!(!Dock::expanded_trigger_contains(
        DockPosition::Bottom,
        (rest.x - 1.0, gap_y),
        rest,
        display,
    ));
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
    assert!(!dock.requires_composition());
    assert!(
        dock.backdrop_regions((1920.0, 1080.0), &[], &workspace_snapshot())
            .is_empty()
    );
    assert!(!dock.captures_pointer(960.0, 1079.0, (1920.0, 1080.0), &[], &workspace_snapshot(),));
}

#[test]
fn dock_backdrop_is_one_analytic_rounded_body() {
    let dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let display = (1920.0, 1080.0);
    let workspaces = workspace_snapshot();

    let backdrop = dock.backdrop_regions(display, &[], &workspaces);
    let glass = dock.liquid_glass_regions(display, &[], &workspaces);

    // The panel declares no backdrop region of its own: its glass body
    // carries an animation-stable capture footprint, so reveal and
    // magnification morphs cannot invalidate the compositor's capture.
    assert!(backdrop.is_empty());
    assert_eq!(glass.len(), 1);
    let footprint = glass[0].capture_bounds.expect("panel declares a footprint");
    let bounds = glass[0].bounds;
    assert!(footprint.x <= bounds.x && footprint.y <= bounds.y);
    assert!(footprint.x + footprint.w >= bounds.x + bounds.w);
    assert!(footprint.y + footprint.h >= bounds.y + bounds.h);
    assert_eq!(glass[0].corner_radius, Design::dark().radii.glass_panel);
    assert_eq!(glass[0].opacity, 1.0);
}

#[test]
fn collapsed_autohide_handle_stays_an_analytic_glass_body() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    dock.set_autohide(true);
    dock.autohide_reveal = 0.0;
    let display = (1920.0, 1080.0);
    let workspaces = workspace_snapshot();

    assert!(dock.requires_composition());
    assert_eq!(dock.backdrop_blur_sigma(), 12.0);
    let backdrop = dock.backdrop_regions(display, &[], &workspaces);
    let glass = dock.liquid_glass_regions(display, &[], &workspaces);
    assert!(backdrop.is_empty());
    assert_eq!(glass.len(), 1);
    // At rest the footprint shrinks to the handle itself: nothing animates,
    // so there is no envelope to cover.
    assert_eq!(glass[0].capture_bounds, Some(glass[0].bounds));
    assert_eq!(glass[0].bounds.w, AUTOHIDE_HANDLE_WIDTH);
    assert_eq!(glass[0].bounds.h, AUTOHIDE_HANDLE_HEIGHT);
    assert_eq!(glass[0].corner_radius, AUTOHIDE_HANDLE_HEIGHT * 0.5);
    assert_eq!(glass[0].opacity, 1.0);
}

#[test]
fn settled_maximized_collapse_keeps_the_capsule_composited() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let mut maximized = window(7, "org.example.Editor", true);
    maximized.state.maximized = true;
    dock.update_windows(std::slice::from_ref(&maximized));
    dock.autohide_reveal = 0.0;
    dock.collapse_pending = false;

    assert_eq!(dock.space_use, SpaceUse::Maximized);
    assert!(dock.requires_composition());
    assert_eq!(dock.backdrop_blur_sigma(), 12.0);
    let glass = dock.liquid_glass_regions(
        (1920.0, 1080.0),
        &[maximized.clone()],
        &workspace_snapshot(),
    );
    assert_eq!(glass.len(), 1);
    assert_eq!(glass[0].bounds.w, AUTOHIDE_HANDLE_WIDTH);
    assert_eq!(glass[0].bounds.h, AUTOHIDE_HANDLE_HEIGHT);

    dock.autohide_reveal = 0.25;
    assert!(dock.requires_composition());
    assert_eq!(dock.backdrop_blur_sigma(), 12.0);
    // Mid-reveal the glass body morphs, but its capture footprint is the
    // stable full-panel envelope.
    let glass = dock.liquid_glass_regions((1920.0, 1080.0), &[maximized], &workspace_snapshot());
    assert_eq!(glass.len(), 1);
    let footprint = glass[0].capture_bounds.expect("panel declares a footprint");
    assert!(footprint.w >= AUTOHIDE_HANDLE_WIDTH);
    assert_eq!(footprint.h, DOCK_PANEL_HEIGHT);
}

#[test]
fn backdrop_prepass_reveals_only_from_the_capsule_body() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let mut maximized = window(7, "org.example.Editor", true);
    maximized.state.maximized = true;
    dock.update_windows(std::slice::from_ref(&maximized));
    dock.autohide_reveal = 0.0;
    dock.collapse_pending = false;
    dock.hidden_trigger_armed = true;
    let display = (1920.0, 1080.0);
    let workspaces = workspace_snapshot();

    // The bottom edge outside the visible capsule remains client-owned.
    let mut outside = Input::new(display, 0.016);
    outside.set_cursor(display.0 * 0.5, display.1 - 1.0);
    dock.prepare_backdrop(&outside, std::slice::from_ref(&maximized), &workspaces);
    assert_eq!(dock.autohide_idle, dock.autohide_timeout);
    assert!(dock.requires_composition());

    // Hovering the capsule itself starts the normal reveal animation.
    let indicator = Dock::collapsed_indicator_bounds(DockPosition::Bottom, display);
    let mut capsule = Input::new(display, 0.016);
    capsule.set_cursor(
        indicator.x + indicator.w * 0.5,
        indicator.y + indicator.h * 0.5,
    );
    dock.prepare_backdrop(&capsule, &[maximized], &workspaces);
    assert_eq!(dock.autohide_idle, 0.0);
    assert!(dock.anim_pending());
}

#[test]
fn running_app_preview_layout_exposes_every_window_inside_the_output() {
    let owner = lens::Rect {
        x: 910.0,
        y: 980.0,
        w: 84.0,
        h: 84.0,
    };
    let windows: Vec<_> = (1..=7).map(aegis_model::window::WindowId).collect();
    let presentation = live_preview_layout(
        &Design::dark(),
        (1920.0, 1080.0),
        owner,
        &windows,
        1.0,
        DockPosition::Bottom,
    );

    assert_eq!(presentation.cards.len(), windows.len());
    assert!(presentation.panel.origin.x >= PREVIEW_SCREEN_MARGIN as i32);
    assert!(presentation.panel.origin.y >= PREVIEW_SCREEN_MARGIN as i32);
    assert!(presentation.panel.origin.x + presentation.panel.size.w <= 1920);
    for card in &presentation.cards {
        assert!(windows.contains(&card.window));
        assert_eq!(card.geometry.preview.size.w, card.geometry.outer.size.w);
        assert_eq!(
            card.geometry.preview.size.h + card.geometry.label.size.h,
            card.geometry.outer.size.h
        );
    }

    for card in &presentation.cards {
        let centre_x =
            card.geometry.outer.origin.x as f32 + card.geometry.outer.size.w as f32 * 0.5;
        let centre_y =
            card.geometry.outer.origin.y as f32 + card.geometry.outer.size.h as f32 * 0.5;
        assert_eq!(
            live_preview_hit(&presentation, centre_x, centre_y),
            Some(card.window),
            "each preview card resolves to its own focus target"
        );
    }
}

#[test]
fn live_preview_adds_one_panel_body_and_keeps_its_pointer_bridge() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let owner = lens::Rect {
        x: 900.0,
        y: 970.0,
        w: 84.0,
        h: 84.0,
    };
    let presentation = live_preview_layout(
        &Design::dark(),
        (1920.0, 1080.0),
        owner,
        &[
            aegis_model::window::WindowId(7),
            aegis_model::window::WindowId(8),
        ],
        1.0,
        DockPosition::Bottom,
    );
    let panel = presentation.panel;
    dock.tooltip_alpha = 1.0;
    dock.hover_owner_bounds = Some(owner);
    dock.hover_surface_bounds = Some(lens::Rect {
        x: panel.origin.x as f32,
        y: panel.origin.y as f32,
        w: panel.size.w as f32,
        h: panel.size.h as f32,
    });
    dock.live_preview = Some(presentation);

    let glass = dock.liquid_glass_regions((1920.0, 1080.0), &[], &workspace_snapshot());
    assert_eq!(glass.len(), 2);
    assert_eq!(glass[1].corner_radius, Design::dark().radii.glass_panel);
    let bridge_y = (panel.origin.y + panel.size.h) as f32 + PREVIEW_PANEL_GAP * 0.5;
    assert!(dock.captures_pointer(
        owner.x + owner.w * 0.5,
        bridge_y,
        (1920.0, 1080.0),
        &[],
        &workspace_snapshot(),
    ));
    assert!(dock.captures_pointer(
        owner.x + owner.w * 0.5,
        owner.y + owner.h * 0.25,
        (1920.0, 1080.0),
        &[],
        &workspace_snapshot(),
    ));
}

#[test]
fn hovered_live_preview_uses_one_parent_body_with_an_optical_focus_field() {
    let mut dock = dock_with(vec![app("org.example.Editor.desktop")]);
    let owner = lens::Rect {
        x: 900.0,
        y: 970.0,
        w: 84.0,
        h: 84.0,
    };
    let window = aegis_model::window::WindowId(7);
    let presentation = live_preview_layout(
        &Design::dark(),
        (1920.0, 1080.0),
        owner,
        &[window],
        1.0,
        DockPosition::Bottom,
    );
    let panel = presentation.panel;
    dock.tooltip_alpha = 1.0;
    dock.hover_surface_bounds = Some(lens::Rect {
        x: panel.origin.x as f32,
        y: panel.origin.y as f32,
        w: panel.size.w as f32,
        h: panel.size.h as f32,
    });
    let mut focused_presentation = presentation.clone();
    focused_presentation.focused = Some(window);
    dock.hovered_preview = Some(window);
    dock.live_preview = Some(focused_presentation);

    let glass = dock.liquid_glass_regions((1920.0, 1080.0), &[], &workspace_snapshot());
    assert_eq!(glass.len(), 2);
    let focus = glass[1].focus.expect("preview panel should carry focus");
    assert_eq!(
        focus.bounds.x,
        presentation.cards[0].geometry.outer.origin.x as f32
    );
    assert_eq!(
        focus.bounds.y,
        presentation.cards[0].geometry.outer.origin.y as f32
    );
    assert_eq!(
        focus.bounds.w,
        presentation.cards[0].geometry.outer.size.w as f32
    );
    assert_eq!(
        focus.bounds.h,
        presentation.cards[0].geometry.outer.size.h as f32
    );
    assert_eq!(focus.corner_radius, Design::dark().radii.control);
    assert_eq!(focus.strength, Design::dark().glass_focus.field_strength);
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
        (DOCK_PANEL_HEIGHT + DOCK_EDGE_MARGIN) as i32
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
    // 2 pinned tiles (incl. launchpad) + 1 transient application tile.
    let pinned = Dock::rest_centre_estimate(1, 3, 2, 1920.0);
    let transient = Dock::rest_centre_estimate(2, 3, 2, 1920.0);
    let pitch = DOCK_TILE + DOCK_TILE_GAP;
    assert!(
        (transient - pinned - DOCK_TILE - DOCK_SECTION_GAP).abs() < 1e-5,
        "the section boundary replaces the ordinary gap with the wider section gap"
    );
    // No transient tiles → no extra gap.
    let a = Dock::rest_centre_estimate(1, 2, 2, 1920.0);
    let b = Dock::rest_centre_estimate(0, 2, 2, 1920.0);
    assert!((a - b - pitch).abs() < 1e-5);
}

#[test]
fn live_strip_centres_settle_onto_the_rest_estimates() {
    // Launchpad + 2 pinned apps (one running) + 1 transient application. With
    // every spring at rest and no drag permutation, the live strip geometry
    // must reproduce the resting estimates the magnification factor and the
    // pointer bounds are computed from — including the section boundary gap.
    let dock = dock_with(vec![app("firefox.desktop"), app("terminal.desktop")]);
    let windows = [window(1, "firefox", true), window(2, "unknown-app", false)];
    let tiles = Dock::frame_tiles(
        &dock.tile_cache,
        &dock.apps,
        &dock.all_apps,
        &dock.icons,
        dock.catalog_revision,
        &windows,
        Some("Applications"),
    );
    let n = tiles.len();
    assert_eq!(n, 4);
    let pinned_count = tiles.iter().filter(|t| t.pinned).count();
    assert_eq!(pinned_count, 3);

    let order: Vec<usize> = (0..n).collect();
    let eased = vec![DOCK_TILE; n];
    let bar_len = n as f32 * DOCK_TILE
        + (n as f32 - 1.0) * DOCK_TILE_GAP
        + (DOCK_SECTION_GAP - DOCK_TILE_GAP)
        + 2.0 * DOCK_PAD;
    let origin = (1920.0 - bar_len) * 0.5;
    let centres = strip_centres(&eased, &order, origin, pinned_count, DOCK_SECTION_GAP);
    for (i, centre) in centres.iter().enumerate() {
        let rest = Dock::rest_centre_estimate(i, n, pinned_count, 1920.0);
        assert!(
            (centre - rest).abs() < 1e-4,
            "tile {i}: live centre {centre} != rest estimate {rest}"
        );
    }

    // The section boundary replaces the ordinary tile gap with the wider
    // section gap, so the divider at its edge-to-edge midpoint keeps exactly
    // one ordinary gap of clearance on each side.
    let last_pinned_edge = centres[pinned_count - 1] + eased[pinned_count - 1] * 0.5;
    let first_transient_edge = centres[pinned_count] - eased[pinned_count] * 0.5;
    assert!((first_transient_edge - last_pinned_edge - DOCK_SECTION_GAP).abs() < 1e-4);
}

#[test]
fn strip_centres_place_the_section_gap_at_the_preview_boundary() {
    // Pin preview: the transient tile (index 3) previews inside the pinned
    // strip at slot 2, so the boundary gap lands right after it.
    let eased = vec![DOCK_TILE; 4];
    let order = vec![0, 1, 3, 2];
    let centres = strip_centres(&eased, &order, 0.0, 3, DOCK_SECTION_GAP);
    let pitch = DOCK_TILE + DOCK_TILE_GAP;
    let first = DOCK_PAD + DOCK_TILE * 0.5;
    assert!((centres[0] - first).abs() < 1e-4);
    assert!((centres[1] - (first + pitch)).abs() < 1e-4);
    // Slot 2 holds the dragged transient tile: the ordinary gap.
    assert!((centres[3] - (first + 2.0 * pitch)).abs() < 1e-4);
    // Slot 3: the wider section gap replaces the ordinary gap.
    assert!((centres[2] - (first + 2.0 * pitch + DOCK_TILE + DOCK_SECTION_GAP)).abs() < 1e-4);
}

#[test]
fn drop_section_follows_the_section_divider() {
    // Launchpad + 2 pinned apps + 1 transient app on a 1920-wide bottom dock.
    let (n, pinned_count, axis_len) = (4, 3, 1920.0);
    let boundary = Dock::section_boundary_axis(n, pinned_count, axis_len);
    let last_pinned = Dock::rest_centre_estimate(2, n, pinned_count, axis_len);
    assert!((boundary - (last_pinned + DOCK_TILE * 0.5 + DOCK_SECTION_GAP * 0.5)).abs() < 1e-4);
    assert_eq!(
        Dock::drop_section_at(boundary - 1.0, n, pinned_count, axis_len),
        DropSection::Pinned
    );
    assert_eq!(
        Dock::drop_section_at(boundary + 1.0, n, pinned_count, axis_len),
        DropSection::Transient
    );
}

#[test]
fn drop_insert_index_swaps_once_the_cursor_passes_a_tile_centre() {
    // Rest centres of the pinned strip with the dragged tile removed.
    let centres = [100.0, 166.0, 232.0];
    assert_eq!(Dock::drop_insert_index(&centres, 50.0), 0);
    assert_eq!(Dock::drop_insert_index(&centres, 100.0), 0);
    // Past the next tile's centre (its midpoint) the insertion slot moves
    // past it — one swap per centre crossed.
    assert_eq!(Dock::drop_insert_index(&centres, 100.1), 1);
    assert_eq!(Dock::drop_insert_index(&centres, 166.0), 1);
    assert_eq!(Dock::drop_insert_index(&centres, 166.1), 2);
    assert_eq!(Dock::drop_insert_index(&centres, 400.0), 3);
}

#[test]
fn reorder_commit_stays_inside_the_pinned_range() {
    // The insertion index ranges over the pinned sequence (excluding the
    // dragged tile) by construction, so a committed reorder can neither
    // displace the leading Launchpad tile nor cross the pinned/transient
    // separator.
    let mut ids = vec!["a".to_string(), "b".to_string(), "c".to_string()];
    // Drag "a" past "b"'s centre: one swap.
    assert!(Dock::move_element(&mut ids, 0, 1));
    assert_eq!(ids, vec!["b", "a", "c"]);
    // Drag "a" (now at 1) to the end of the pinned strip.
    assert!(Dock::move_element(&mut ids, 1, 2));
    assert_eq!(ids, vec!["b", "c", "a"]);
    // Drag "a" back before every pinned tile.
    assert!(Dock::move_element(&mut ids, 2, 0));
    assert_eq!(ids, vec!["a", "b", "c"]);
    // Dropping on the tile's own slot is a no-op.
    assert!(!Dock::move_element(&mut ids, 1, 1));
    assert_eq!(ids, vec!["a", "b", "c"]);
    // Out-of-range indices never panic or mutate.
    assert!(!Dock::move_element(&mut ids, 3, 0));
    assert!(!Dock::move_element(&mut ids, 0, 4));
    assert_eq!(ids, vec!["a", "b", "c"]);
}

#[test]
fn drag_threshold_promotes_only_past_the_threshold() {
    let origin = (100.0, 100.0);
    assert!(!Dock::drag_threshold_exceeded(origin, (103.0, 104.0)));
    assert!(!Dock::drag_threshold_exceeded(origin, origin));
    assert!(Dock::drag_threshold_exceeded(origin, (107.0, 100.0)));
    // Diagonal travel past the same distance also promotes.
    assert!(Dock::drag_threshold_exceeded(origin, (105.0, 105.0)));
    assert!(!Dock::drag_threshold_exceeded(origin, (104.0, 104.0)));
}

#[test]
fn edge_drag_target_prefers_the_nearest_in_zone_edge() {
    let display = (1920.0, 1080.0);
    assert_eq!(
        Dock::edge_drag_target((50.0, 540.0), display),
        Some(DockPosition::Left)
    );
    assert_eq!(
        Dock::edge_drag_target((960.0, 1000.0), display),
        Some(DockPosition::Bottom)
    );
    assert_eq!(
        Dock::edge_drag_target((1870.0, 540.0), display),
        Some(DockPosition::Right)
    );
    // In a corner the nearer edge wins.
    assert_eq!(
        Dock::edge_drag_target((20.0, 1040.0), display),
        Some(DockPosition::Left)
    );
    assert_eq!(
        Dock::edge_drag_target((80.0, 1060.0), display),
        Some(DockPosition::Bottom)
    );
    // The screen centre and the top edge are outside every zone; the dock
    // keeps its current edge there (top is deliberately not a target).
    assert_eq!(Dock::edge_drag_target((960.0, 540.0), display), None);
    assert_eq!(Dock::edge_drag_target((960.0, 4.0), display), None);
}

#[test]
fn rest_bounds_and_reserved_follow_the_configured_edge() {
    let display = (1920.0, 1080.0);
    // Launchpad + one pinned tile: 2 tiles, 1 pinned (both pinned here; the
    // pinned count only matters for the section gap).
    let bar_len = 2.0 * DOCK_TILE + DOCK_TILE_GAP + 2.0 * DOCK_PAD;

    let bottom = Dock::rest_bounds(2, 2, DockPosition::Bottom, display);
    assert_eq!(bottom.y, display.1 - DOCK_PANEL_HEIGHT - DOCK_EDGE_MARGIN);
    assert_eq!(bottom.h, DOCK_PANEL_HEIGHT);
    assert_eq!(bottom.w, bar_len);

    let left = Dock::rest_bounds(2, 2, DockPosition::Left, display);
    assert_eq!(left.x, DOCK_EDGE_MARGIN);
    assert_eq!(left.w, DOCK_PANEL_HEIGHT);
    assert_eq!(left.y, (display.1 - bar_len) * 0.5);
    assert_eq!(left.h, bar_len);

    let right = Dock::rest_bounds(2, 2, DockPosition::Right, display);
    assert_eq!(right.x, display.0 - DOCK_PANEL_HEIGHT - DOCK_EDGE_MARGIN);
    assert_eq!(right.w, DOCK_PANEL_HEIGHT);
    assert_eq!(right.h, bar_len);

    let extent = (DOCK_PANEL_HEIGHT + DOCK_EDGE_MARGIN) as i32;
    let mut dock = Dock::new();
    assert_eq!(dock.reserved().bottom, extent);
    assert_eq!(dock.reserved().left, 0);
    dock.set_position(DockPosition::Left);
    assert_eq!(dock.reserved().left, extent);
    assert_eq!(dock.reserved().bottom, 0);
    dock.set_position(DockPosition::Right);
    assert_eq!(dock.reserved().right, extent);
    assert_eq!(dock.reserved().left, 0);
}

#[test]
fn side_dock_collapsed_geometry_anchors_to_its_edge() {
    let display = (1920.0, 1080.0);
    let expanded_len = 320.0;

    let expanded = Dock::collapsed_panel_rect(DockPosition::Left, display, expanded_len, 1.0);
    assert_eq!(expanded.x, DOCK_EDGE_MARGIN);
    assert_eq!(expanded.w, DOCK_PANEL_HEIGHT);
    assert_eq!(expanded.h, expanded_len);
    assert_eq!(expanded.y, (display.1 - expanded_len) * 0.5);

    let collapsed = Dock::collapsed_panel_rect(DockPosition::Left, display, expanded_len, 0.0);
    assert_eq!(collapsed.x, DOCK_EDGE_MARGIN);
    assert_eq!(collapsed.w, AUTOHIDE_HANDLE_HEIGHT);
    assert_eq!(collapsed.h, AUTOHIDE_HANDLE_WIDTH);
    // The handle stays centred on the edge, like the bottom one.
    assert_eq!(
        collapsed.y + collapsed.h * 0.5,
        display.1 * 0.5,
        "the side handle is centred vertically"
    );
    assert_eq!(expanded.x, collapsed.x);

    let right = Dock::collapsed_panel_rect(DockPosition::Right, display, expanded_len, 1.0);
    assert_eq!(right.x, display.0 - DOCK_EDGE_MARGIN - DOCK_PANEL_HEIGHT);
    let right_collapsed =
        Dock::collapsed_panel_rect(DockPosition::Right, display, expanded_len, 0.0);
    assert_eq!(
        right_collapsed.x,
        display.0 - DOCK_EDGE_MARGIN - AUTOHIDE_HANDLE_HEIGHT
    );
}

#[test]
fn catalog_push_updates_position_and_reconciles_optimistic_order() {
    let mut dock = dock_with(vec![app("a.desktop"), app("b.desktop")]);
    assert_eq!(dock.position, DockPosition::Bottom);

    // A push carrying a new edge moves the dock (a live config edit).
    dock.update_app_catalog(&AppCatalog {
        apps: vec![app("a.desktop"), app("b.desktop")],
        pinned: vec![app("a.desktop"), app("b.desktop")],
        icons: IconSet::default(),
        position: DockPosition::Right,
    });
    assert_eq!(dock.position, DockPosition::Right);

    // A queued optimistic reorder is superseded by the reconciling push.
    dock.pending_order = Some(vec!["b.desktop".to_string(), "a.desktop".to_string()]);
    dock.update_app_catalog(&AppCatalog {
        apps: vec![app("a.desktop"), app("b.desktop")],
        pinned: vec![app("a.desktop"), app("b.desktop")],
        icons: IconSet::default(),
        position: DockPosition::Left,
    });
    assert!(dock.pending_order.is_none());
    assert_eq!(dock.position, DockPosition::Left);

    // While an edge drag holds the panel, a catalog push must not yank the
    // dock back to the configured edge mid-gesture.
    dock.press = Some(PressState {
        origin: (10.0, 500.0),
        target: PressTarget::Panel,
        dragging: true,
        section: DropSection::Pinned,
        insert: None,
        start_position: DockPosition::Left,
    });
    dock.update_app_catalog(&AppCatalog {
        apps: vec![app("a.desktop"), app("b.desktop")],
        pinned: vec![app("a.desktop"), app("b.desktop")],
        icons: IconSet::default(),
        position: DockPosition::Bottom,
    });
    assert_eq!(
        dock.position,
        DockPosition::Left,
        "an in-flight edge drag owns the live position"
    );
}

#[test]
fn minimize_targets_map_windows_to_their_resting_tile_icons() {
    let mut dock = dock_with(vec![app("firefox.desktop")]);
    let pinned_window = window(1, "firefox", true);
    let transient = window(2, "terminal", false);
    dock.update(ChromeUpdate::AllWindows(&[
        pinned_window.clone(),
        transient.clone(),
    ]));
    let targets = dock.minimize_targets((1920.0, 1080.0));
    assert_eq!(targets.len(), 2, "one target per running window");

    let firefox = targets
        .iter()
        .find(|(id, _)| *id == pinned_window.id)
        .expect("pinned window has a target")
        .1;
    let terminal = targets
        .iter()
        .find(|(id, _)| *id == transient.id)
        .expect("transient window has a target")
        .1;
    // Bottom dock: resting icons are DOCK_TILE squares inside the bottom panel.
    assert_eq!(firefox.size.w, DOCK_TILE as i32);
    assert_eq!(firefox.size.h, DOCK_TILE as i32);
    assert!(firefox.origin.y + firefox.size.h > 1080 - 100);
    assert!(firefox.origin.y + firefox.size.h <= 1080);
    assert_ne!(firefox, terminal, "different tiles give different targets");

    // A second window of the same app folds into the same tile: same target.
    // The transient stays around so the strip length (and therefore every
    // icon's resting position) is unchanged.
    let second_firefox = window(3, "Firefox", false);
    dock.update(ChromeUpdate::AllWindows(&[
        pinned_window.clone(),
        second_firefox.clone(),
        transient.clone(),
    ]));
    let targets = dock.minimize_targets((1920.0, 1080.0));
    let first = targets
        .iter()
        .find(|(id, _)| *id == pinned_window.id)
        .map(|(_, rect)| *rect);
    let second = targets
        .iter()
        .find(|(id, _)| *id == second_firefox.id)
        .map(|(_, rect)| *rect);
    assert_eq!(first, Some(firefox));
    assert_eq!(second, Some(firefox));
}

#[test]
fn minimize_targets_follow_the_dock_edge() {
    let mut dock = dock_with(vec![app("firefox.desktop")]);
    dock.set_position(DockPosition::Left);
    let w = window(1, "firefox", false);
    dock.update(ChromeUpdate::AllWindows(std::slice::from_ref(&w)));
    let targets = dock.minimize_targets((1920.0, 1080.0));
    let rect = targets
        .iter()
        .find(|(id, _)| *id == w.id)
        .expect("window has a target")
        .1;
    // Left dock: the resting icon sits inside the left panel, vertically centred.
    assert!(rect.origin.x < 100, "icon hugs the left edge: {rect:?}");
    let centre_y = rect.origin.y + rect.size.h / 2;
    assert!(
        (centre_y - 540).abs() < 100,
        "icon near vertical centre: {rect:?}"
    );
}
