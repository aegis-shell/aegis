use super::*;

#[test]
fn nested_output_geometry_preserves_logical_size_at_integer_scale() {
    let geometry = output_geometry_from_host(945, 924, 2.0);
    assert_eq!(geometry.mode.width, 1890);
    assert_eq!(geometry.mode.height, 1848);
    assert_eq!(geometry.scale, aegis_core::output::Scale(2.0));
    assert_eq!(geometry.logical_size(), aegis_core::Size { w: 945, h: 924 });
}

#[test]
fn nested_output_geometry_preserves_logical_size_at_fractional_scale() {
    let geometry = output_geometry_from_host(945, 924, 1.5);
    assert_eq!(geometry.mode.width, 1418);
    assert_eq!(geometry.mode.height, 1386);
    assert_eq!(geometry.scale, aegis_core::output::Scale(1.5));
    assert_eq!(geometry.logical_size(), aegis_core::Size { w: 945, h: 924 });
}

#[test]
fn logical_capture_region_scales_to_physical_pixels() {
    assert_eq!(
        logical_rect_to_physical(aegis_core::Rect::new(10, 20, 100, 80), 2.0, 3840, 2160),
        aegis_core::Rect::new(20, 40, 200, 160)
    );
}

#[test]
fn realm_capture_region_is_intersected_in_logical_space() {
    assert_eq!(
        clamp_logical_region(aegis_core::Rect::new(-10, 5, 30, 40), 100, 30),
        Some(aegis_core::Rect::new(0, 5, 20, 25))
    );
    assert_eq!(
        clamp_logical_region(aegis_core::Rect::new(100, 0, 20, 20), 100, 100),
        None
    );
    assert_eq!(
        clamp_logical_region(aegis_core::Rect::new(0, 0, 0, 20), 100, 100),
        None
    );
    assert_eq!(
        clamp_logical_region(
            aegis_core::Rect::new(i32::MAX - 1, i32::MAX - 1, i32::MAX, i32::MAX),
            16_384,
            16_384,
        ),
        None
    );
}

#[test]
fn logical_capture_region_scales_endpoints_and_clamps() {
    assert_eq!(
        logical_rect_to_physical(aegis_core::Rect::new(-10, 10, 30, 20), 1.5, 30, 40),
        aegis_core::Rect::new(0, 15, 30, 25)
    );
    assert_eq!(
        logical_rect_to_physical(aegis_core::Rect::new(10, 20, 100, 80), 0.0, 200, 200),
        aegis_core::Rect::new(10, 20, 100, 80)
    );
}

#[test]
fn capture_encoding_crops_and_unpremultiplies_worker_payload() {
    let (width, height, png) = encode_rgba_capture(
        2,
        1,
        vec![10, 20, 30, 255, 50, 25, 0, 128],
        Some(aegis_core::Rect::new(1, 0, 1, 1)),
    )
    .unwrap();
    assert_eq!((width, height), (1, 1));
    let decoded = image::load_from_memory(&png).unwrap().into_rgba8();
    assert_eq!(decoded.into_raw(), vec![100, 50, 0, 128]);
}

#[test]
fn capture_security_generation_invalidates_pre_lock_frames() {
    let worker = CaptureWorker::spawn().unwrap();
    let before = worker.security_generation();
    assert!(worker.permits(before));
    worker.set_allowed(false);
    let locked = worker.security_generation();
    assert!(locked > before);
    assert!(!worker.permits(before));
    assert!(!worker.permits(locked));
    worker.set_allowed(true);
    assert_eq!(worker.security_generation(), locked);
    assert!(worker.permits(locked));
    worker.set_allowed(false);
    assert!(worker.security_generation() > locked);
}

#[test]
fn screenshot_file_uri_list_percent_encodes_path_bytes() {
    let dir = std::env::temp_dir().join(format!(
        "aegis-screenshot-uri-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("test")
    ));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("shot #.png");
    std::fs::write(&path, b"png").unwrap();
    let uri = screenshot_uri_list(path.to_str().unwrap()).unwrap();
    let uri = String::from_utf8(uri).unwrap();
    assert!(uri.starts_with("file:///"));
    assert!(uri.ends_with("/shot%20%23.png\r\n"));
    std::fs::remove_file(path).unwrap();
    std::fs::remove_dir(dir).unwrap();
}

#[test]
fn only_user_initiated_screenshots_update_the_human_clipboard() {
    assert!(screenshot_updates_human_clipboard(
        aegis_ipc::Origin::Chrome
    ));
    assert!(screenshot_updates_human_clipboard(
        aegis_ipc::Origin::Keybinding
    ));
    assert!(!screenshot_updates_human_clipboard(
        aegis_ipc::Origin::Ipc { conn_id: 7 }
    ));
    assert!(!screenshot_updates_human_clipboard(
        aegis_ipc::Origin::Internal
    ));
}

#[test]
fn desktop_preferences_have_one_deterministic_override_chain() {
    let config = aegis_config::Config::parse(
        "schema_version = 1\n\
         [ui]\n\
         icon_theme = \"Papirus-Dark\"\n\
         cursor_theme = \"Bibata\"\n\
         cursor_size = 32\n",
    )
    .unwrap();
    let preferences = resolve_desktop_preferences(
        Some(&config),
        &PreferenceOverrides {
            icon_theme: Some("Breeze".into()),
            cursor_theme: None,
            cursor_size: Some(48),
        },
    );
    assert_eq!(preferences.icon_theme, "Breeze");
    assert_eq!(preferences.cursor_theme, "Bibata");
    assert_eq!(preferences.cursor_size, 48);

    let defaults = resolve_desktop_preferences(None, &PreferenceOverrides::default());
    assert_eq!(defaults.icon_theme, "hicolor");
    assert_eq!(defaults.cursor_theme, "default");
    assert_eq!(defaults.cursor_size, 24);
}

#[test]
fn desktop_preference_overrides_are_not_copied_into_persistence() {
    let config = aegis_config::Config::parse(
        "schema_version = 1\n\
         [ui]\n\
         icon_theme = \"Papirus\"\n\
         cursor_theme = \"Bibata\"\n\
         cursor_size = 32\n",
    )
    .unwrap();
    let requested = aegis_core::settings::DesktopPreferences {
        color_scheme: aegis_core::settings::ColorScheme::Dark,
        icon_theme: "OverrideIcon".into(),
        cursor_theme: "OverrideCursor".into(),
        cursor_size: 64,
        ..config.desktop_preferences()
    };
    let persistent = preferences_for_persistence(
        Some(&config),
        requested,
        &PreferenceOverrides {
            icon_theme: Some("OverrideIcon".into()),
            cursor_theme: Some("OverrideCursor".into()),
            cursor_size: Some(64),
        },
    );
    assert_eq!(
        persistent.color_scheme,
        aegis_core::settings::ColorScheme::Dark
    );
    assert_eq!(persistent.icon_theme, "Papirus");
    assert_eq!(persistent.cursor_theme, "Bibata");
    assert_eq!(persistent.cursor_size, 32);
}

#[test]
fn icon_raster_scale_uses_effective_output_policy() {
    assert_eq!(effective_icon_scale(Some(2.0), 1.0), 2);
    assert_eq!(effective_icon_scale(Some(1.5), 1.0), 2);
    assert_eq!(effective_icon_scale(None, 2.0), 2);
    assert_eq!(effective_icon_scale(Some(f32::NAN), 0.0), 1);
}

#[test]
fn config_agent_scopes_compile_to_fail_closed_ipc_allowlists() {
    let config = aegis_config::Config::parse(
        "schema_version = 1\n\
             [[agent.scope]]\n\
             name = \"focus-one\"\n\
             ops = [\"Focus\", \"NotARealOperation\"]\n\
             windows = [7]\n\
             workspaces = [3]\n",
    )
    .unwrap();
    let scopes = build_ipc_scopes(Some(&config));
    let scope = scopes.get("focus-one").expect("compiled scope");

    assert_eq!(scope.ops, Some(vec![aegis_ipc::OpClass::Focus]));
    assert!(scope.permits(&aegis_ipc::Command::Focus {
        id: aegis_core::window::WindowId(7),
    }));
    assert!(!scope.permits(&aegis_ipc::Command::Focus {
        id: aegis_core::window::WindowId(8),
    }));
    assert!(!scope.permits(&aegis_ipc::Command::Close {
        id: aegis_core::window::WindowId(7),
    }));
    let admin = scopes
        .get(aegis_ipc::LOCAL_REALM_ADMIN_SCOPE)
        .expect("built-in Realm recovery scope");
    assert!(admin.permits(&aegis_ipc::Command::LaunchInRealm {
        realm: aegis_core::realm::RealmId(9),
        desktop_id: "foot.desktop".into(),
    }));
}

#[test]
fn realm_scope_expands_atomic_groups_before_authorizing() {
    let mut model = aegis_core::realm::RealmModel::new();
    let agent = model.create_agent_realm(
        "agent",
        aegis_core::realm::SeatCapabilities::POINTER_KEYBOARD,
    );
    let client = model.register_client(None);
    let first = aegis_core::window::WindowId(7);
    let sibling = aegis_core::window::WindowId(8);
    let group = model
        .create_interaction_group(client, &[first, sibling], aegis_core::realm::HUMAN_REALM)
        .unwrap();
    let action = aegis_ipc::RealmAction::Transact {
        expected_revision: None,
        mutations: vec![aegis_core::realm::RealmMutation::TransferWindow {
            window: first,
            target: agent.realm,
            retain_source_as_observer: true,
        }],
    };
    let one_window = aegis_ipc::Scope {
        windows: Some(vec![first]),
        workspaces: None,
        outputs: None,
        realms: Some(vec![agent.realm]),
        ops: Some(vec![aegis_ipc::OpClass::TransactRealm]),
        ask_ops: None,
    };
    assert!(one_window.permits_realm_action(&action));
    assert!(
        authorize_realm_action_against_snapshot(&one_window, &action, &model.snapshot()).is_err(),
        "an allowlisted member cannot smuggle its interaction-group sibling"
    );

    let complete_group = aegis_ipc::Scope {
        windows: Some(vec![first, sibling]),
        ..one_window
    };
    assert!(
        authorize_realm_action_against_snapshot(&complete_group, &action, &model.snapshot())
            .is_ok()
    );
    let observe = aegis_ipc::RealmAction::Transact {
        expected_revision: None,
        mutations: vec![aegis_core::realm::RealmMutation::SetObserver {
            group,
            realm: agent.realm,
            observe: true,
        }],
    };
    assert!(
        authorize_realm_action_against_snapshot(&complete_group, &observe, &model.snapshot())
            .is_ok()
    );
}

#[test]
fn automation_operation_names_accept_canonical_and_snake_case() {
    assert_eq!(
        ipc_op_class("SetWindowGeometry"),
        Some(aegis_ipc::OpClass::SetWindowGeometry)
    );
    assert_eq!(
        ipc_op_class("set_window_geometry"),
        Some(aegis_ipc::OpClass::SetWindowGeometry)
    );
    assert_eq!(
        ipc_op_class("inject_input"),
        Some(aegis_ipc::OpClass::InjectInput)
    );
    assert_eq!(
        ipc_op_class("system_control"),
        Some(aegis_ipc::OpClass::SystemControl)
    );
}
