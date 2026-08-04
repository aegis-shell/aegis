use super::*;

#[test]
fn handshake_reports_query_always_granted() {
    let path = scratch();
    let handler = Arc::new(TestHandler::query(sample_windows()));
    let _server = Server::start(&path, handler).expect("bind");

    // Request more than the policy grants; the server intersects and forces
    // query on, so the client learns the truth.
    let client = Client::connect_with(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: true,
            session: true,
            interaction_domain: true,
        },
    )
    .expect("connect");
    let caps = client.caps();
    assert!(caps.query, "query is always granted");
    assert!(!caps.control, "control is refused by the query-only policy");
    assert!(!caps.session, "session is refused by the query-only policy");
}

#[test]
fn server_socket_is_owner_only() {
    let path = scratch();
    let handler = Arc::new(TestHandler::query(vec![]));
    let _server = Server::start(&path, handler).expect("bind");

    let mode = std::fs::metadata(&path)
        .expect("socket metadata")
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
}

#[test]
fn start_refuses_to_replace_a_non_socket_path() {
    let path = scratch();
    std::fs::write(&path, b"keep me").expect("seed regular file");
    let handler = Arc::new(TestHandler::query(vec![]));

    let err = Server::start(&path, handler)
        .err()
        .expect("regular path must be refused");
    assert_eq!(err.kind(), std::io::ErrorKind::AlreadyExists);
    assert_eq!(
        std::fs::read(&path).expect("regular file preserved"),
        b"keep me"
    );
    std::fs::remove_file(path).expect("cleanup regular file");
}

#[test]
fn second_server_cannot_steal_an_active_socket() {
    let path = scratch();
    let first = Arc::new(TestHandler::query(sample_windows()));
    let _server = Server::start(&path, first).expect("first bind");

    let second = Arc::new(TestHandler::query(vec![]));
    let err = Server::start(&path, second)
        .err()
        .expect("active socket must not be replaced");
    assert_eq!(err.kind(), std::io::ErrorKind::AddrInUse);

    let mut client = Client::connect(&path).expect("first server remains reachable");
    assert_eq!(client.windows().expect("query first server").len(), 2);
}

#[test]
fn settings_query_and_confirmed_transaction_round_trip() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_with(
        &path,
        ConnectionCapabilities {
            query: true,
            session: true,
            ..ConnectionCapabilities::default()
        },
    )
    .expect("connect");

    let before = client.settings().expect("settings snapshot");
    assert_eq!(before.revision, 7);
    let mut config = before.touchpad.config;
    config.natural_scroll = !config.natural_scroll;
    let action = SettingsAction::SetTouchpad { config };
    let receipt = client
        .apply_settings(Some(before.revision), action.clone())
        .expect("confirmed apply");
    assert_eq!(receipt.revision, 8);
    assert_eq!(
        handler.settings_actions.lock().unwrap().as_slice(),
        std::slice::from_ref(&action)
    );
    assert_eq!(client.settings().unwrap().touchpad.config, config);

    let preferences = aegis_model::settings::DesktopPreferences {
        color_scheme: aegis_model::settings::ColorScheme::Dark,
        icon_theme: "Papirus".into(),
        ..Default::default()
    };
    let appearance_action = SettingsAction::SetDesktopPreferences {
        preferences: preferences.clone(),
    };
    let receipt = client
        .apply_settings(Some(8), appearance_action.clone())
        .expect("confirmed desktop-preferences apply");
    assert_eq!(receipt.revision, 9);
    assert_eq!(
        handler.settings_actions.lock().unwrap().as_slice(),
        &[action.clone(), appearance_action.clone()]
    );
    assert_eq!(client.settings().unwrap().preferences, preferences);

    let idle = aegis_model::settings::IdleSettings {
        dim_after_seconds: 120,
        lock_after_seconds: 300,
        display_off_after_seconds: 360,
        suspend_after_seconds: 900,
        ..Default::default()
    };
    let idle_action = SettingsAction::SetIdle { settings: idle };
    let receipt = client
        .apply_settings(Some(9), idle_action.clone())
        .expect("confirmed idle-policy apply");
    assert_eq!(receipt.revision, 10);
    assert_eq!(
        handler.settings_actions.lock().unwrap().as_slice(),
        &[action, appearance_action, idle_action]
    );
    assert_eq!(client.settings().unwrap().idle, idle);
}

#[test]
fn system_status_query_and_control_round_trip() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let requested = ConnectionCapabilities {
        query: true,
        control: true,
        ..ConnectionCapabilities::default()
    };
    let mut client = Client::connect_scoped(&path, requested, "system").expect("connect");
    assert_eq!(
        client.scope().ops,
        Some(vec![ActorCapability::SystemControl])
    );

    let status = client.system_status().expect("system status");
    assert_eq!(status.volume, Some(42));
    assert_eq!(status.brightness, Some(73));

    let action = SystemAction::SetBrightness { level: 55 };
    client
        .apply_system_action(action.clone())
        .expect("apply system control");
    assert_eq!(*handler.system_actions.lock().unwrap(), [action]);
    assert!(handler.commands.lock().unwrap().is_empty());

    let mut denied =
        Client::connect_scoped(&path, requested, "focus-first").expect("restricted connect");
    let error = denied
        .apply_system_action(SystemAction::ToggleMute)
        .unwrap_err();
    assert!(error.to_string().contains("out of scope"), "{error}");
}

#[test]
fn system_control_returns_the_authoritative_apply_error() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    *handler.system_result.lock().unwrap() = Err("backend refused output power".into());
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_with(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            ..ConnectionCapabilities::default()
        },
    )
    .expect("connect");

    let error = client
        .apply_system_action(SystemAction::ToggleMute)
        .unwrap_err();
    assert!(
        error.to_string().contains("backend refused output power"),
        "{error}"
    );
    assert_eq!(
        *handler.system_actions.lock().unwrap(),
        [SystemAction::ToggleMute]
    );
}

#[test]
fn invalid_system_control_is_refused_before_dispatch() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_with(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            ..ConnectionCapabilities::default()
        },
    )
    .expect("connect");

    let error = client
        .apply_system_action(SystemAction::SetVolume { level: 101 })
        .unwrap_err();
    assert!(error.to_string().contains("outside 0..=100"), "{error}");
    assert!(handler.system_actions.lock().unwrap().is_empty());
    assert!(handler.commands.lock().unwrap().is_empty());
}

#[test]
fn settings_transaction_rejects_stale_revision() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_with(
        &path,
        ConnectionCapabilities {
            query: true,
            session: true,
            ..ConnectionCapabilities::default()
        },
    )
    .expect("connect");
    let error = client
        .apply_settings(
            Some(6),
            SettingsAction::SetTouchpad {
                config: aegis_model::input::TouchpadConfig::default(),
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("revision conflict"), "{error}");
    assert!(handler.settings_actions.lock().unwrap().is_empty());
}

#[test]
fn settings_mutation_requires_session_capability_and_is_audited() {
    let path = scratch();
    let handler = Arc::new(TestHandler::query(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect(&path).expect("connect");
    let error = client
        .apply_settings(
            None,
            SettingsAction::SetTouchpad {
                config: aegis_model::input::TouchpadConfig::default(),
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("session capability"), "{error}");
    let refusals = handler.refusals.lock().unwrap();
    assert!(matches!(
        refusals.as_slice(),
        [(
            _,
            aegis_ipc::JournalMutation::Settings {
                before_revision: 7,
                after_revision: 7,
                ..
            },
            _
        )]
    ));
}
