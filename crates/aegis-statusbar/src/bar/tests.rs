use super::*;

#[test]
fn status_bar_reserves_exactly_its_visual_height() {
    assert_eq!(StatusBar::new().reserved().top, HUD_HEIGHT as i32);
}

#[test]
fn fullscreen_window_hides_status_bar_and_releases_its_surface_policy() {
    let mut bar = StatusBar::new();
    bar.panel_open = true;
    bar.agent_panel_open = true;
    bar.agent_panel_reveal = 1.0;
    bar.menu_open_for = Some("org.example.Tray".to_string());

    let mut fullscreen = Window::new(aegis_core::window::WindowId(7));
    fullscreen.state.fullscreen = true;
    bar.update_windows(&[fullscreen]);

    let workspaces = WorkspaceSnapshot {
        outputs: Vec::new(),
    };
    assert!(bar.fullscreen_active);
    assert!(!bar.panel_open);
    assert!(!bar.agent_panel_open);
    assert_eq!(bar.agent_panel_reveal, 0.0);
    assert!(bar.menu_open_for.is_none());
    assert_eq!(bar.reserved(), Reserved::default());
    assert_eq!(bar.backdrop_blur_sigma(), 0.0);
    assert!(
        bar.backdrop_regions((1920.0, 1080.0), &[], &workspaces)
            .is_empty()
    );
    assert!(!bar.captures_pointer(10.0, 10.0, (1920.0, 1080.0), &[], &workspaces,));
    assert!(!bar.anim_pending());

    bar.update_windows(&[]);
    assert!(!bar.fullscreen_active);
    assert_eq!(bar.reserved().top, HUD_HEIGHT as i32);
    assert_eq!(bar.backdrop_blur_sigma(), BACKDROP_BLUR_SIGMA);
    assert!(bar.captures_pointer(10.0, 10.0, (1920.0, 1080.0), &[], &workspaces,));
}

#[test]
fn maximized_and_minimized_fullscreen_windows_keep_status_bar_visible() {
    let mut bar = StatusBar::new();
    let mut maximized = Window::new(aegis_core::window::WindowId(7));
    maximized.state.maximized = true;
    bar.update_windows(&[maximized]);
    assert!(!bar.fullscreen_active);

    let mut minimized_fullscreen = Window::new(aegis_core::window::WindowId(8));
    minimized_fullscreen.state.fullscreen = true;
    minimized_fullscreen.minimized = true;
    bar.update_windows(&[minimized_fullscreen]);
    assert!(!bar.fullscreen_active);
    assert_eq!(bar.reserved().top, HUD_HEIGHT as i32);
    assert_eq!(bar.backdrop_blur_sigma(), BACKDROP_BLUR_SIGMA);
}

#[test]
fn agent_workspace_entry_is_permanent_and_tracks_aggregate_realm_state() {
    let i18n = Localizer::new("en-US");
    let mut model = aegis_core::realm::RealmModel::new();
    let indicator = agent_workspace_indicator(&model.snapshot(), &i18n);
    assert_eq!(indicator.state, AgentWorkspaceState::Idle);
    assert_eq!(indicator.label, "Agent Workspaces");

    let bundle = model.create_agent_realm("Fuji", Default::default());
    let mut snapshot = model.snapshot();
    let indicator = agent_workspace_indicator(&snapshot, &i18n);
    assert_eq!(indicator.state, AgentWorkspaceState::Active);
    assert_eq!(indicator.label, "Fuji · Active");

    snapshot
        .realms
        .iter_mut()
        .find(|realm| realm.id == bundle.realm)
        .expect("agent Realm")
        .state = RealmState::Paused;
    let indicator = agent_workspace_indicator(&snapshot, &i18n);
    assert_eq!(indicator.state, AgentWorkspaceState::Paused);
    assert_eq!(indicator.label, "Fuji · Paused");

    let research = model.create_agent_realm("Research Agent", Default::default());
    let mut snapshot = model.snapshot();
    snapshot
        .realms
        .iter_mut()
        .find(|realm| realm.id == bundle.realm)
        .expect("agent Realm")
        .state = RealmState::Paused;
    let indicator = agent_workspace_indicator(&snapshot, &i18n);
    assert_eq!(indicator.state, AgentWorkspaceState::PartiallyPaused);
    assert_eq!(indicator.label, "2 workspaces · Partially paused");

    snapshot
        .realms
        .iter_mut()
        .find(|realm| realm.id == research.realm)
        .expect("second agent Realm")
        .state = RealmState::Paused;
    let indicator = agent_workspace_indicator(&snapshot, &i18n);
    assert_eq!(indicator.state, AgentWorkspaceState::Paused);
    assert_eq!(indicator.label, "2 workspaces · Paused");

    for realm in snapshot
        .realms
        .iter_mut()
        .filter(|realm| realm.kind == RealmKind::Agent)
    {
        realm.state = RealmState::Revoked;
    }
    let indicator = agent_workspace_indicator(&snapshot, &i18n);
    assert_eq!(indicator.state, AgentWorkspaceState::Idle);
    assert_eq!(indicator.label, "Agent Workspaces");
}

#[test]
fn panel_stays_inside_narrow_displays() {
    let panel = StatusBar::panel_bounds((320.0, 480.0));
    assert!(panel.x >= 0.0);
    assert!(panel.x + panel.w <= 320.0);
    assert!(panel.y >= HUD_HEIGHT);
    assert!(panel.y + panel.h <= 480.0);
}

#[test]
fn agent_panel_stays_inside_narrow_displays_and_expands_from_the_right() {
    let final_panel = StatusBar::agent_panel_bounds((320.0, 480.0));
    assert!(final_panel.x >= 0.0);
    assert!(final_panel.x + final_panel.w <= 320.0);
    assert!(final_panel.y + final_panel.h <= 480.0);

    let mut bar = StatusBar::new();
    let collapsed = bar.revealed_agent_panel_bounds((320.0, 480.0));
    bar.agent_panel_reveal = 1.0;
    let expanded = bar.revealed_agent_panel_bounds((320.0, 480.0));
    assert!(expanded.w > collapsed.w);
    assert_eq!(expanded.x + expanded.w, collapsed.x + collapsed.w);
}

#[test]
fn workspace_dot_states_use_size_and_brightness() {
    assert!(workspace_dot_diameter(true, false) > workspace_dot_diameter(false, false));
    assert!(workspace_dot_diameter(false, true) > workspace_dot_diameter(false, false));
    let active_alpha = workspace_dot_color(true, false).components().3;
    let inactive_alpha = workspace_dot_color(false, false).components().3;
    assert!(active_alpha > inactive_alpha);
}

#[test]
fn status_icons_follow_the_reported_state() {
    let mut status = SystemStatus {
        volume: Some(12),
        ..SystemStatus::default()
    };
    assert_eq!(volume_icon_name(&status), "audio-volume-low-symbolic");
    status.volume = Some(55);
    assert_eq!(volume_icon_name(&status), "audio-volume-medium-symbolic");
    status.muted = true;
    assert_eq!(volume_icon_name(&status), "audio-volume-muted-symbolic");
    assert_eq!(
        network_icon_name(NetworkState::Wired),
        "network-wired-symbolic"
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
