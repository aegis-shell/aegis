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
        let (profile, main, notifications, network, side) = CommandPanel::cluster_bounds(display);
        for rect in [profile, main, notifications, network, side] {
            assert!(rect.x >= 0.0 && rect.y >= 0.0);
            assert!(rect.x + rect.w <= display.0 + 0.01);
            assert!(rect.y + rect.h <= display.1 + 0.01);
        }
        assert!(notifications.x >= profile.x + profile.w);
        assert_eq!(notifications.y, profile.y);
    }
}

#[test]
fn full_size_displays_get_the_design_geometry() {
    let (profile, main, notifications, network, side) =
        CommandPanel::cluster_bounds((1920.0, 1080.0));
    // Profile is compact in top-left
    assert_eq!(profile.w, 300.0);
    assert_eq!(profile.h, 84.0);
    assert_eq!(profile.x, 48.0);
    assert_eq!(profile.y, 37.8);

    // Notifications is compact in top-right
    assert_eq!(notifications.w, 260.0);
    assert_eq!(notifications.h, 200.0);
    assert_eq!(notifications.x, 1920.0 - 48.0 - 260.0);
    assert_eq!(notifications.y, 37.8);

    // The network monitor sits at the right-middle anchor, below the
    // notification stream and sharing its right edge.
    assert_eq!(network.w, 300.0);
    assert_eq!(network.h, 300.0);
    assert_eq!(network.x, 1920.0 - 48.0 - 300.0);
    assert_eq!(network.y, notifications.y + notifications.h + PANEL_GAP);

    // Main Split Control Panel is centered on screen and clear of the
    // network column.
    assert_eq!(main.w, 720.0);
    assert_eq!(main.h, 460.0);
    assert_eq!(main.x, (1920.0 - 720.0) * 0.5);
    assert_eq!(main.y, (1080.0 - 460.0) * 0.5);
    assert!(main.x + main.w <= network.x - PANEL_GAP);

    // Side Column (Machine Monitor + Tray) is disabled
    assert_eq!(side.w, 0.0);
    assert_eq!(side.h, 0.0);
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
fn network_identity_names_the_interface_and_wifi_ssid() {
    let i18n = Localizer::new("en-US");
    let mut status = SystemStatus::default();

    // Offline: no interface to name.
    assert_eq!(network_identity(&status, &i18n), "Offline");

    // Wired: the interface alone.
    status.network = NetworkState::Wired;
    status.network_interface = "enp3s0".into();
    assert_eq!(network_identity(&status, &i18n), "enp3s0");

    // Wi-Fi: the interface plus the SSID once the forked probe answers.
    status.network = NetworkState::Wifi;
    status.network_interface = "wlan0".into();
    status.wifi_ssid = Some("Homelab-5G".into());
    assert_eq!(network_identity(&status, &i18n), "wlan0 · Homelab-5G");

    // Wi-Fi before the probe answers: fall back to the interface.
    status.wifi_ssid = None;
    assert_eq!(network_identity(&status, &i18n), "wlan0");
}

#[test]
fn command_panel_emits_liquid_glass_regions_when_open() {
    let mut panel = CommandPanel::without_sources();
    let display = (1920.0, 1080.0);
    let workspaces = WorkspaceSnapshot {
        outputs: Vec::new(),
    };
    let mut out = ChromeEvents::default();

    // Inactive panel returns no liquid glass regions
    assert!(
        panel
            .liquid_glass_regions(display, &[], &workspaces)
            .is_empty()
    );

    // Opened panel returns physical liquid glass bodies (profile, notifications, network, capsule tabs, view)
    panel.toggle_command_panel(&mut out);
    panel.reveal = 1.0;
    let regions = panel.liquid_glass_regions(display, &[], &workspaces);
    assert!(regions.len() >= 4);
    assert_eq!(regions[0].opacity, 1.0); // Notifications
    assert_eq!(regions[1].opacity, 1.0); // Network monitor (right-middle)
    assert_eq!(regions[1].corner_radius, panel.design.radii.glass_panel);
    // Each capsule has 100% semicircle ends (corner_radius == height * 0.5)
    assert_eq!(regions[2].corner_radius, 22.0); // First capsule tab
    let last = regions.last().unwrap();
    assert_eq!(last.opacity, 1.0); // Right Content View
}

#[test]
fn power_mode_projection_covers_the_four_toggle_combinations() {
    use aegis_model::power::PowerMode;
    // Display axis × security axis, the four user scenarios:
    // (keep awake, auto lock) → mode.
    assert_eq!(
        crate::presentation::power_mode_for(false, true),
        PowerMode::Balanced
    );
    assert_eq!(
        crate::presentation::power_mode_for(true, true),
        PowerMode::Secure
    );
    assert_eq!(
        crate::presentation::power_mode_for(true, false),
        PowerMode::Awake
    );
    // The forbidden combination (blank while never locking) projects onto
    // Awake: the security boundary wins and the display axis reads back
    // honestly as "awake".
    assert_eq!(
        crate::presentation::power_mode_for(false, false),
        PowerMode::Awake
    );
}

#[test]
fn projected_modes_never_blank_an_unlocked_session() {
    for keep_awake in [false, true] {
        for auto_lock in [false, true] {
            let mode = crate::presentation::power_mode_for(keep_awake, auto_lock);
            if !mode.locks_automatically() {
                assert!(
                    !mode.blanks_display(),
                    "{mode:?} would blank a never-locking session"
                );
            }
        }
    }
}
