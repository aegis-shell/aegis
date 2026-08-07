use super::*;

fn escape() -> KeyChar {
    KeyChar {
        keysym: aegis_model::input::XKB_KEY_Escape,
        ch: None,
        mods: aegis_model::input::Mods::NONE,
    }
}

fn fullscreen_window() -> Window {
    let mut window = Window::new(aegis_model::window::WindowId(7));
    window.state.fullscreen = true;
    window
}

#[test]
fn toggle_opens_and_closes_the_panel() {
    let mut panel = CommandPanel::without_sources();
    panel.set_reduced_motion(true);
    assert!(!panel.command_panel_active());
    assert!(!panel.captures_keyboard());

    let mut out = ChromeEvents::default();
    panel.toggle_command_panel(&mut out);
    assert!(panel.open);
    panel.advance(0.016);
    assert!(panel.command_panel_active());
    assert!(panel.captures_keyboard());
    assert!(panel.modal_active());
    assert!(panel.exclusive_presentation_active());
    assert!(panel.requires_composition());
    assert!(!panel.anim_pending());
    assert_eq!(panel.backdrop_blur_sigma(), BACKDROP_BLUR_SIGMA);

    panel.toggle_command_panel(&mut out);
    assert!(!panel.open);
    panel.advance(0.016);
    assert!(!panel.command_panel_active());
    assert!(!panel.exclusive_presentation_active());
    assert_eq!(panel.backdrop_blur_sigma(), 0.0);
}

#[test]
fn reveal_eases_instead_of_snapping_without_reduced_motion() {
    let mut panel = CommandPanel::without_sources();
    let mut out = ChromeEvents::default();
    panel.toggle_command_panel(&mut out);
    panel.advance(0.016);
    let early = panel.reveal;
    assert!(early > 0.0 && early < 1.0);
    assert!(panel.anim_pending());
    for _ in 0..240 {
        panel.advance(0.016);
    }
    assert_eq!(panel.reveal, 1.0);
    assert!(!panel.anim_pending());
}

#[test]
fn reduced_motion_snaps_reveal_to_its_target() {
    let mut panel = CommandPanel::without_sources();
    let mut out = ChromeEvents::default();
    panel.toggle_command_panel(&mut out);
    panel.set_reduced_motion(true);
    assert_eq!(panel.reveal, 1.0);
    panel.set_reduced_motion(false);
    panel.toggle_command_panel(&mut out);
    panel.set_reduced_motion(true);
    assert_eq!(panel.reveal, 0.0);
}

#[test]
fn a_playing_avatar_keeps_the_panel_frame_loop_alive_after_reveal() {
    assert!(presentation_anim_pending(1.0, 1.0, true, false));
    assert!(!presentation_anim_pending(1.0, 1.0, false, false));
}

#[test]
fn escape_peels_the_tray_menu_before_the_panel() {
    let mut panel = CommandPanel::without_sources();
    panel.set_reduced_motion(true);
    let mut out = ChromeEvents::default();
    panel.toggle_command_panel(&mut out);
    panel.menu_open_for = Some("org.example.Tray".to_string());
    panel.menu_path = vec![0];

    panel.key_char(&escape(), &mut out);
    assert!(
        panel.menu_open_for.is_none(),
        "first Escape closes the menu"
    );
    assert!(panel.open, "panel stays open");

    panel.key_char(&escape(), &mut out);
    assert!(!panel.open, "second Escape closes the panel");
}

#[test]
fn selecting_another_tab_closes_the_tray_menu() {
    let mut panel = CommandPanel::without_sources();
    let module = panel
        .modules
        .metadata()
        .find(|module| module.availability == ModuleAvailability::Available)
        .expect("at least one available settings module");
    panel.menu_open_for = Some("org.example.Tray".to_string());
    panel.menu_path = vec![0];

    panel.select_tab(Tab::Settings(module.id));
    assert!(panel.menu_open_for.is_none());
    assert!(panel.menu_path.is_empty());
}

#[test]
fn settings_actions_coalesce_per_variant() {
    let set_idle = || SettingsAction::SetIdle {
        settings: aegis_model::settings::IdleSettings::default(),
    };
    let set_preferences = || SettingsAction::SetDesktopPreferences {
        preferences: aegis_model::settings::DesktopPreferences::default(),
    };
    assert!(same_action_kind(&set_idle(), &set_idle()));
    assert!(same_action_kind(&set_preferences(), &set_preferences()));
    assert!(!same_action_kind(&set_idle(), &set_preferences()));
}

#[test]
fn a_fullscreen_window_closes_the_panel() {
    let mut panel = CommandPanel::without_sources();
    panel.set_reduced_motion(true);
    let mut out = ChromeEvents::default();
    panel.toggle_command_panel(&mut out);
    panel.menu_open_for = Some("org.example.Tray".to_string());

    panel.update_windows(&[fullscreen_window()]);
    assert!(!panel.open);
    assert!(panel.menu_open_for.is_none());
    panel.advance(0.016);
    assert!(!panel.command_panel_active());
}

#[test]
fn cluster_bounds_stay_inside_small_displays() {
    for display in [(320.0, 480.0), (800.0, 600.0), (1920.0, 1080.0)] {
        let (header, main, notifications, tray) = CommandPanel::cluster_bounds(display);
        for rect in [header, main, notifications, tray] {
            assert!(rect.x >= 0.0 && rect.y >= 0.0);
            assert!(rect.x + rect.w <= display.0 + 0.01);
            assert!(rect.y + rect.h <= display.1 + 0.01);
        }
        assert!(main.y >= header.y + header.h);
        assert!(notifications.x >= main.x + main.w);
        // The header spans the full cluster width; main + gap + side column
        // divide the same width below it.
        assert!((header.w - (main.w + PANEL_GAP + notifications.w)).abs() < 0.01);
        // The side column: notifications on top, tray pinned to the bottom,
        // both full column width and together as tall as the main panel.
        assert_eq!(notifications.w, tray.w);
        assert_eq!(notifications.y, main.y);
        assert!((notifications.h + PANEL_GAP + tray.h - main.h).abs() < 0.01);
        assert!(main.w >= MAIN_FLOOR_W);
        assert!(notifications.w >= SIDE_FLOOR_W);
    }
}

#[test]
fn full_size_displays_get_the_design_geometry() {
    let (header, main, notifications, tray) = CommandPanel::cluster_bounds((1920.0, 1080.0));
    assert_eq!(header.h, HEADER_H);
    assert_eq!(main.w, CONTENT_W);
    assert_eq!(main.h, CONTENT_H);
    assert_eq!(notifications.w, SIDE_W);
    assert_eq!(tray.w, SIDE_W);
    assert_eq!(tray.h, TRAY_PANEL_H);
    assert_eq!(header.w, CONTENT_W + PANEL_GAP + SIDE_W);
    assert_eq!(notifications.h + PANEL_GAP + tray.h, CONTENT_H);
}

#[test]
fn history_evicts_the_oldest_sample_past_its_cap() {
    let mut history = History::new(4);
    for value in [10.0, 20.0, 30.0, 40.0] {
        history.push(value);
    }
    assert_eq!(history.len(), 4);
    history.push(50.0);
    history.push(60.0);
    assert_eq!(history.len(), 4);
    assert_eq!(history.newest(), Some(60.0));
    let samples: Vec<f32> = history.samples().collect();
    assert_eq!(samples, vec![30.0, 40.0, 50.0, 60.0]);
}

#[test]
fn resource_stats_feed_the_sparkline_histories() {
    let mut panel = CommandPanel::without_sources();
    let mut stats = ResourceStats {
        cpu_percent: 42.0,
        gpu_percent: Some(17.0),
        mem_used_bytes: 8 << 30,
        mem_total_bytes: 16 << 30,
        ..ResourceStats::default()
    };
    panel.update_resource_stats(&stats);
    assert_eq!(panel.cpu_history.newest(), Some(42.0));
    assert_eq!(panel.gpu_history.newest(), Some(17.0));
    assert_eq!(panel.ram_history.newest(), Some(50.0));

    // A vanished GPU probe must not push stale samples.
    stats.gpu_percent = None;
    stats.cpu_percent = 5.0;
    panel.update_resource_stats(&stats);
    assert_eq!(panel.cpu_history.len(), 2);
    assert_eq!(panel.gpu_history.len(), 1);
    assert_eq!(panel.stats.cpu_percent, 5.0);
}

#[test]
fn format_rate_picks_si_units_and_precision() {
    assert_eq!(format_rate(0.0), "0B");
    assert_eq!(format_rate(512.0), "512B");
    assert_eq!(format_rate(340.0 * 1024.0), "340K");
    assert_eq!(format_rate(1.25 * 1024.0 * 1024.0), "1.2M");
    assert_eq!(format_rate(9.6 * 1024.0), "9.6K");
    assert_eq!(format_rate(64.0 * 1024.0 * 1024.0), "64M");
    assert_eq!(format_rate(2.0 * 1024.0 * 1024.0 * 1024.0), "2.0G");
}

#[test]
fn format_gib_pair_renders_used_over_total_in_gib() {
    assert_eq!(format_gib_pair(0, 0), "0.0/0.0G");
    let gib = 1u64 << 30;
    assert_eq!(format_gib_pair(gib * 64 / 10, gib * 156 / 10), "6.4/15.6G");
}

#[test]
fn profile_resolves_for_the_current_process_user() {
    let profile = Profile::current().expect("passwd record for the test user");
    assert!(!profile.username.is_empty());
    assert!(!profile.display_name.is_empty());
    assert!(!profile.initials.is_empty());
    // The primary group is always part of the list when group lookup works.
    assert!(!profile.groups.is_empty());
}

#[test]
fn stagger_delays_the_content_panel_behind_the_menu() {
    assert_eq!(stagger(0.0, CONTENT_STAGGER), 0.0);
    assert_eq!(stagger(CONTENT_STAGGER, CONTENT_STAGGER), 0.0);
    assert!(stagger(0.5, 0.0) > stagger(0.5, CONTENT_STAGGER));
    assert_eq!(stagger(1.0, CONTENT_STAGGER), 1.0);
}

#[test]
fn agent_workspace_row_tracks_aggregate_interaction_domain_state() {
    let i18n = Localizer::new("en-US");
    let mut model = aegis_model::interaction_domain::InteractionDomainModel::new();
    let indicator = agent_workspace_indicator(&model.snapshot(), &i18n);
    assert_eq!(indicator.state, AgentWorkspaceState::Idle);
    assert_eq!(indicator.label, "Agent Workspaces");

    let bundle = model.create_agent_interaction_domain("Fuji", Default::default());
    let mut snapshot = model.snapshot();
    let indicator = agent_workspace_indicator(&snapshot, &i18n);
    assert_eq!(indicator.state, AgentWorkspaceState::Active);
    assert_eq!(indicator.label, "Fuji · Active");

    snapshot
        .interaction_domains
        .iter_mut()
        .find(|interaction_domain| interaction_domain.id == bundle.interaction_domain)
        .expect("agent InteractionDomain")
        .state = InteractionDomainState::Paused;
    let indicator = agent_workspace_indicator(&snapshot, &i18n);
    assert_eq!(indicator.state, AgentWorkspaceState::Paused);
    assert_eq!(indicator.label, "Fuji · Paused");

    let research = model.create_agent_interaction_domain("Research Agent", Default::default());
    let mut snapshot = model.snapshot();
    snapshot
        .interaction_domains
        .iter_mut()
        .find(|interaction_domain| interaction_domain.id == bundle.interaction_domain)
        .expect("agent InteractionDomain")
        .state = InteractionDomainState::Paused;
    let indicator = agent_workspace_indicator(&snapshot, &i18n);
    assert_eq!(indicator.state, AgentWorkspaceState::PartiallyPaused);
    assert_eq!(indicator.label, "2 workspaces · Partially paused");

    snapshot
        .interaction_domains
        .iter_mut()
        .find(|interaction_domain| interaction_domain.id == research.interaction_domain)
        .expect("second agent InteractionDomain")
        .state = InteractionDomainState::Paused;
    let indicator = agent_workspace_indicator(&snapshot, &i18n);
    assert_eq!(indicator.state, AgentWorkspaceState::Paused);
    assert_eq!(indicator.label, "2 workspaces · Paused");

    for interaction_domain in snapshot
        .interaction_domains
        .iter_mut()
        .filter(|interaction_domain| interaction_domain.kind == InteractionDomainKind::Agent)
    {
        interaction_domain.state = InteractionDomainState::Revoked;
    }
    let indicator = agent_workspace_indicator(&snapshot, &i18n);
    assert_eq!(indicator.state, AgentWorkspaceState::Idle);
    assert_eq!(indicator.label, "Agent Workspaces");
}
