use super::*;

fn escape() -> KeyChar {
    KeyChar {
        keysym: aegis_core::input::XKB_KEY_Escape,
        ch: None,
        mods: aegis_core::input::Mods::NONE,
    }
}

fn fullscreen_window() -> Window {
    let mut window = Window::new(aegis_core::window::WindowId(7));
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
    assert!(panel.requires_composition());
    assert!(!panel.anim_pending());
    assert_eq!(panel.backdrop_blur_sigma(), BACKDROP_BLUR_SIGMA);

    panel.toggle_command_panel(&mut out);
    assert!(!panel.open);
    panel.advance(0.016);
    assert!(!panel.command_panel_active());
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
fn selecting_another_section_closes_the_tray_menu() {
    let mut panel = CommandPanel::without_sources();
    panel.select_section(Section::Tray);
    panel.menu_open_for = Some("org.example.Tray".to_string());
    panel.menu_path = vec![0];

    panel.select_section(Section::Messages);
    assert!(panel.menu_open_for.is_none());
    assert!(panel.menu_path.is_empty());
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
        let (menu, content) = CommandPanel::cluster_bounds(display);
        assert!(menu.x >= 0.0 && menu.y >= 0.0);
        assert!(content.x + content.w <= display.0 + 0.01);
        assert!(content.y + content.h <= display.1 + 0.01);
        assert!(content.x >= menu.x + menu.w);
    }
}

#[test]
fn stagger_delays_the_content_panel_behind_the_menu() {
    assert_eq!(stagger(0.0, CONTENT_STAGGER), 0.0);
    assert_eq!(stagger(CONTENT_STAGGER, CONTENT_STAGGER), 0.0);
    assert!(stagger(0.5, 0.0) > stagger(0.5, CONTENT_STAGGER));
    assert_eq!(stagger(1.0, CONTENT_STAGGER), 1.0);
}

#[test]
fn agent_workspace_row_tracks_aggregate_realm_state() {
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
