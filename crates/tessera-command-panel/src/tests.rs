use super::*;

fn escape() -> KeyChar {
    KeyChar {
        keysym: tessera_model::input::XKB_KEY_Escape,
        ch: None,
        mods: tessera_model::input::Mods::NONE,
    }
}

fn fullscreen_window() -> Window {
    let mut window = Window::new(tessera_model::window::WindowId(7));
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
    assert_eq!(panel.backdrop_blur_sigma(), 0.0);

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
        settings: tessera_model::settings::IdleSettings::default(),
    };
    let set_preferences = || SettingsAction::SetDesktopPreferences {
        preferences: tessera_model::settings::DesktopPreferences::default(),
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
        let (profile, main, notifications, clock, tray, media, work_mode, power) =
            CommandPanel::cluster_bounds(display);
        for rect in [
            profile,
            main,
            notifications,
            clock,
            tray,
            media,
            work_mode,
            power,
        ] {
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
    let (profile, main, notifications, clock, tray, media, work_mode, power) =
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

    // The clock sits at top-center, clear of both top corners.
    assert!(clock.x > profile.x + profile.w);
    assert!(clock.x + clock.w < notifications.x);
    assert_eq!(clock.y, profile.y);

    // The tray column hugs the left-middle anchor.
    assert_eq!(tray.x, 48.0);
    assert!(tray.y > notifications.y);
    assert!(tray.h < 1080.0 - 37.8 * 2.0);

    // The MPRIS card owns the left-bottom anchor and stays clear of the
    // centered main surface.
    assert_eq!(media.x, profile.x);
    assert_eq!(media.y + media.h, 1080.0 - 37.8);
    assert!(media.x + media.w + PANEL_GAP <= main.x);

    // The right-bottom band is horizontal and pinned into the corner:
    // power/session flush to the right margin, work mode to its left, both
    // flush to the bottom margin and sharing one height.
    let margin_y = 37.8;
    assert_eq!(power.x + power.w, 1920.0 - 48.0);
    assert_eq!(power.y + power.h, 1080.0 - margin_y);
    assert_eq!(work_mode.x + work_mode.w + PANEL_GAP, power.x);
    assert_eq!(work_mode.y + work_mode.h, power.y + power.h);
    assert_eq!(work_mode.h, power.h);
    assert!(work_mode.y > notifications.y + notifications.h);

    // The main surface and clock share the screen's true centre axis. The
    // bottom band does not squeeze a panel that sits well above it.
    assert_eq!(main.w, MAIN_W);
    assert_eq!(main.h, MAIN_H);
    assert_eq!(main.x + main.w * 0.5, 1920.0 * 0.5);
    assert_eq!(clock.x + clock.w * 0.5, main.x + main.w * 0.5);
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
fn clock_strings_render_a_time_and_a_date() {
    let (time, date) = crate::presentation::clock_strings();
    assert!(time.contains(':'), "wall-clock time renders as HH:MM");
    assert!(!date.is_empty(), "the date line renders");
}

#[test]
fn stagger_delays_the_content_panel_behind_the_menu() {
    assert_eq!(stagger(0.0, CONTENT_STAGGER), 0.0);
    assert_eq!(stagger(CONTENT_STAGGER, CONTENT_STAGGER), 0.0);
    assert!(stagger(0.5, 0.0) > stagger(0.5, CONTENT_STAGGER));
    assert_eq!(stagger(1.0, CONTENT_STAGGER), 1.0);
}

#[test]
fn command_panel_never_requests_backdrop_effects() {
    let mut panel = CommandPanel::without_sources();
    let display = (1920.0, 1080.0);
    let workspaces = WorkspaceSnapshot {
        outputs: Vec::new(),
    };
    let mut out = ChromeEvents::default();

    // No hidden-state effect declarations.
    assert!(
        panel
            .liquid_glass_regions(display, &[], &workspaces)
            .is_empty()
    );
    assert!(panel.backdrop_regions(display, &[], &workspaces).is_empty());

    // Opening paints solid surfaces through Lens and does not activate the
    // compositor's capture, blur, or liquid-glass paths.
    panel.toggle_command_panel(&mut out);
    panel.reveal = 1.0;
    assert_eq!(panel.backdrop_blur_sigma(), 0.0);
    assert!(panel.backdrop_regions(display, &[], &workspaces).is_empty());
    assert!(
        panel
            .liquid_glass_regions(display, &[], &workspaces)
            .is_empty()
    );
}

#[test]
fn command_panel_palette_tracks_the_live_appearance() {
    let mut panel = CommandPanel::without_sources();
    let dark = panel.panel_colors();
    assert!(!dark.is_light());

    panel.design = Design::light();
    let light = panel.panel_colors();
    assert!(light.is_light());
    assert_ne!(light.background, dark.background);
    assert_ne!(light.surface, dark.surface);
    assert_ne!(light.text, dark.text);
    assert_eq!(light.background.components().3, 255);
    assert_eq!(light.surface.components().3, 255);
}

#[test]
fn power_mode_projection_covers_the_four_toggle_combinations() {
    use tessera_model::power::PowerMode;
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

#[test]
fn work_mode_and_power_session_panels_render_within_cluster() {
    let display = (1920.0, 1080.0);
    let (_profile, _main, notifications, _clock, _tray, _media, work_mode, power) =
        CommandPanel::cluster_bounds(display);

    assert!(work_mode.w > 0.0);
    assert!(work_mode.h > 0.0);
    assert!(power.w > 0.0);
    assert!(power.h > 0.0);

    // The right-bottom band is one horizontal row sharing a height, clear
    // below the notification stream.
    assert!(work_mode.y > notifications.y + notifications.h);
    assert_eq!(work_mode.y, power.y);
    assert_eq!(work_mode.h, power.h);
    assert!(power.x > work_mode.x + work_mode.w);
}

#[test]
fn request_system_confirm_dedupes_while_one_is_pending() {
    let mut panel = CommandPanel::without_sources();
    let mut out = ChromeEvents::default();

    // The first destructive request latches and leaves through the events.
    panel.request_system_confirm(SystemAction::PowerOff, &mut out);
    assert_eq!(panel.power_pending_confirm, Some(SystemAction::PowerOff));
    assert_eq!(out.system_actions.len(), 1);

    // A second request while the consent dialog is still resolving is
    // dropped, not stacked.
    panel.request_system_confirm(SystemAction::Reboot, &mut out);
    assert_eq!(out.system_actions.len(), 1);
    assert_eq!(panel.power_pending_confirm, Some(SystemAction::PowerOff));

    // Closing the panel clears the latch so a reopen accepts a new action.
    panel.close();
    assert_eq!(panel.power_pending_confirm, None);
}

#[test]
fn scrollbar_reveals_fade_out_after_wheel_activity_stops() {
    let mut panel = CommandPanel::without_sources();
    panel.notif_scrollbar_reveal = 1.0;
    panel.tray_scrollbar_reveal = 1.0;

    // A handful of 60fps frames of decay settles both reveals to zero.
    for _ in 0..120 {
        panel.advance(1.0 / 60.0);
    }
    assert_eq!(panel.notif_scrollbar_reveal, 0.0);
    assert_eq!(panel.tray_scrollbar_reveal, 0.0);
    // And with nothing moving, no interaction animation stays pending.
    assert!(!panel.interaction_anim_pending());
}
