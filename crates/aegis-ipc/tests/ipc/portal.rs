use super::*;

#[test]
fn pick_app_requires_control_and_an_explicit_scope_op() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let requested = ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    };
    let choices = vec!["org.example.A.desktop".to_string()];
    // Scoped with the explicit PickApp op + control: the chosen id
    // round-trips.
    let mut scoped = Client::connect_scoped(&path, requested, "app-pick").expect("scoped connect");
    let result = scoped
        .pick_app(choices.clone(), None, None)
        .expect("app pick succeeds");
    assert_eq!(
        result,
        aegis_ipc::AppPickResult::App {
            id: "org.example.Chosen.desktop".to_string()
        }
    );
    assert_eq!(
        handler.app_picks.lock().unwrap().as_slice(),
        &[(1, choices.clone())]
    );

    // A scope without the op is refused even though it has control.
    let mut focus_scope =
        Client::connect_scoped(&path, requested, "focus-first").expect("scoped connect");
    let err = focus_scope
        .pick_app(choices.clone(), None, None)
        .unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Unscoped connections never inherit the op (fail-closed, like input).
    let mut unscoped = Client::connect_with(&path, requested).expect("unscoped connect");
    let err = unscoped.pick_app(choices.clone(), None, None).unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Without the control capability the request is refused earlier.
    let mut query_only = Client::connect(&path).expect("query connect");
    let err = query_only.pick_app(choices, None, None).unwrap_err();
    assert!(err.to_string().contains("control capability"), "{err}");
}

#[test]
fn interactive_request_bounds_fail_before_ui_handlers_run() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let requested = ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    };

    let mut app_pick =
        Client::connect_scoped(&path, requested, "app-pick").expect("app-pick connect");
    let err = app_pick
        .pick_app(
            vec!["duplicate.desktop".into(), "duplicate.desktop".into()],
            None,
            None,
        )
        .unwrap_err();
    assert!(err.to_string().contains("duplicate"), "{err}");
    let err = app_pick
        .pick_app(
            vec!["org.example.App.desktop".into()],
            None,
            Some("not-in-the-list.desktop".into()),
        )
        .unwrap_err();
    assert!(err.to_string().contains("inconsistent"), "{err}");
    assert!(handler.app_picks.lock().unwrap().is_empty());

    let mut confirm = Client::connect_scoped(&path, requested, "confirm").expect("confirm connect");
    let err = confirm
        .pick_confirm(" ".into(), "body".into(), None)
        .unwrap_err();
    assert!(err.to_string().contains("confirmation labels"), "{err}");
    let err = confirm
        .pick_confirm("Confirm".into(), "x".repeat(4_097), None)
        .unwrap_err();
    assert!(err.to_string().contains("confirmation labels"), "{err}");
    assert!(handler.confirms.lock().unwrap().is_empty());

    let mut wallpaper =
        Client::connect_scoped(&path, requested, "wallpaper").expect("wallpaper connect");
    let err = wallpaper
        .set_wallpaper(PathBuf::from("relative/wall.png"))
        .unwrap_err();
    assert!(err.to_string().contains("wallpaper path"), "{err}");
    assert!(handler.wallpapers.lock().unwrap().is_empty());

    let mut secret =
        Client::connect_scoped(&path, requested, "secret-prompt").expect("secret-prompt connect");
    let err = secret
        .prompt_secret(" ".into(), None)
        .expect_err("blank secret title must fail closed");
    assert!(
        err.to_string().contains("invalid resource")
            || err.to_string().contains("resource label")
            || err.to_string().contains("secret prompt labels"),
        "{err}"
    );
    assert!(handler.secret_prompts.lock().unwrap().is_empty());
}

#[test]
fn prompt_secret_requires_control_and_an_explicit_scope_op() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let requested = ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    };
    // Scoped with the explicit PromptSecret op + control: the secret
    // round-trips.
    let mut scoped =
        Client::connect_scoped(&path, requested, "secret-prompt").expect("scoped connect");
    let result = scoped
        .prompt_secret("Unlock".to_string(), None)
        .expect("prompt succeeds");
    assert_eq!(
        result,
        aegis_ipc::SecretPromptResult::Secret {
            value: "hunter2".to_string()
        }
    );
    assert_eq!(
        handler.secret_prompts.lock().unwrap().as_slice(),
        &[(1, "Unlock".to_string())]
    );

    // A scope without the op is refused even though it has control.
    let mut focus_scope =
        Client::connect_scoped(&path, requested, "focus-first").expect("scoped connect");
    let err = focus_scope
        .prompt_secret("Unlock".to_string(), None)
        .unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Unscoped connections never inherit the op (fail-closed, like input).
    let mut unscoped = Client::connect_with(&path, requested).expect("unscoped connect");
    let err = unscoped
        .prompt_secret("Unlock".to_string(), None)
        .unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Without the control capability the request is refused earlier.
    let mut query_only = Client::connect(&path).expect("query connect");
    let err = query_only
        .prompt_secret("Unlock".to_string(), None)
        .unwrap_err();
    assert!(err.to_string().contains("control capability"), "{err}");
}

#[test]
fn pick_confirm_requires_control_and_an_explicit_scope_op() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let requested = ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    };
    // Scoped with the explicit PickConfirm op + control: the answer
    // round-trips.
    let mut scoped = Client::connect_scoped(&path, requested, "confirm").expect("scoped connect");
    let result = scoped
        .pick_confirm("Share?".to_string(), "body".to_string(), None)
        .expect("confirm succeeds");
    assert_eq!(result, aegis_ipc::ConfirmPickResult::Confirmed);
    assert_eq!(
        handler.confirms.lock().unwrap().as_slice(),
        &[(1, "Share?".to_string())]
    );

    // A scope without the op is refused even though it has control.
    let mut focus_scope =
        Client::connect_scoped(&path, requested, "focus-first").expect("scoped connect");
    let err = focus_scope
        .pick_confirm("Share?".to_string(), "body".to_string(), None)
        .unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Unscoped connections never inherit the op (fail-closed, like input).
    let mut unscoped = Client::connect_with(&path, requested).expect("unscoped connect");
    let err = unscoped
        .pick_confirm("Share?".to_string(), "body".to_string(), None)
        .unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Without the control capability the request is refused earlier.
    let mut query_only = Client::connect(&path).expect("query connect");
    let err = query_only
        .pick_confirm("Share?".to_string(), "body".to_string(), None)
        .unwrap_err();
    assert!(err.to_string().contains("control capability"), "{err}");
}

#[test]
fn set_wallpaper_requires_control_and_an_explicit_scope_op() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let requested = ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    };
    let image = PathBuf::from("/tmp/wall.png");
    // Scoped with the explicit SetWallpaper op + control: the mutation
    // receipt round-trips.
    let mut scoped = Client::connect_scoped(&path, requested, "wallpaper").expect("scoped connect");
    scoped.set_wallpaper(image.clone()).expect("set succeeds");
    assert_eq!(
        handler.wallpapers.lock().unwrap().as_slice(),
        &[(1, image.clone())]
    );

    // A scope without the op is refused even though it has control.
    let mut focus_scope =
        Client::connect_scoped(&path, requested, "focus-first").expect("scoped connect");
    let err = focus_scope.set_wallpaper(image.clone()).unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Unscoped connections never inherit the op (fail-closed, like input).
    let mut unscoped = Client::connect_with(&path, requested).expect("unscoped connect");
    let err = unscoped.set_wallpaper(image.clone()).unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Without the control capability the request is refused earlier.
    let mut query_only = Client::connect(&path).expect("query connect");
    let err = query_only.set_wallpaper(image).unwrap_err();
    assert!(err.to_string().contains("control capability"), "{err}");
}
