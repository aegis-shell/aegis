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
        tessera_ipc::AppPickResult::App {
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
        tessera_ipc::SecretPromptResult::Secret {
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
    assert_eq!(result, tessera_ipc::ConfirmPickResult::Confirmed);
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

#[test]
fn enumerate_outputs_returns_the_lean_capture_addressing_form() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    // A plain query-only connection may enumerate: it is metadata, gated
    // exactly like GetSettings.
    let mut client = Client::connect(&path).expect("query connect");
    let outputs = client.enumerate_outputs().expect("enumerate outputs");
    assert_eq!(outputs.len(), 2);
    for output in &outputs {
        assert!(output.geometry.is_none(), "lean reply: {output:?}");
        assert!(output.available_modes.is_none(), "lean reply: {output:?}");
    }
    assert_eq!(outputs[0].connector, "HDMI-A-1");
    assert!(outputs[0].primary);
    assert_eq!(outputs[0].rect, tessera_model::Rect::new(0, 0, 1920, 1080));
    assert_eq!(outputs[1].connector, "DP-1");
    assert!(!outputs[1].primary);
    assert_eq!(outputs[1].rect, tessera_model::Rect::new(1920, 0, 2560, 1440));

    // GetOutputs keeps answering with the rich geometry/mode form.
    let rich = client.outputs().expect("get outputs");
    assert_eq!(rich.len(), 2);
    assert_eq!(rich[0].connector, "HDMI-A-1");
    assert_eq!(rich[0].geometry.mode.width, 1920);
    assert_eq!(rich[0].available_modes.len(), 1);
}

#[test]
fn stream_start_accepts_a_connector_selector_and_cursor_mode() {
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
    let mut scoped = Client::connect_scoped(&path, requested, "stream").expect("scoped connect");
    scoped
        .start_output_stream_with(
            Some(60),
            tessera_ipc::StreamTarget::Output {
                output: Some("HDMI-A-1".into()),
            },
            Some(tessera_ipc::StreamCursorMode::Embedded),
        )
        .expect("connector-addressed stream starts");
    assert_eq!(
        handler.stream_targets.lock().unwrap().as_slice(),
        &[tessera_ipc::StreamTarget::Output {
            output: Some("HDMI-A-1".into()),
        }]
    );
    assert_eq!(
        handler.stream_cursors.lock().unwrap().as_slice(),
        &[tessera_ipc::StreamCursorMode::Embedded]
    );
}

#[test]
fn stream_geometry_changed_freezes_but_keeps_the_stream() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let requested = ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    };
    let mut client = Client::connect_scoped(&path, requested, "stream").expect("connect");
    let started = client.start_output_stream(None).expect("stream starts");

    // The compositor reports a geometry change: the event arrives, and the
    // lane stays registered — the stream is frozen, not ended.
    assert!(server.stream_geometry_changed(started.stream_id, 2560, 1440));
    let message = client.next_stream_message().expect("geometry event");
    let tessera_ipc::StreamMessage::GeometryChanged {
        stream_id,
        width,
        height,
    } = message
    else {
        panic!("expected GeometryChanged, got {message:?}");
    };
    assert_eq!((stream_id, width, height), (started.stream_id, 2560, 1440));

    // The documented restart path works: stop the frozen stream, start a
    // fresh one at the new geometry.
    client
        .stop_output_stream(started.stream_id)
        .expect("stop succeeds after freeze");
    assert_eq!(
        handler.stream_stops.lock().unwrap().as_slice(),
        &[started.stream_id]
    );
    let restarted = client.start_output_stream(None).expect("restart succeeds");
    assert_ne!(restarted.stream_id, started.stream_id);
}

#[test]
fn pick_target_output_kind_reaches_the_handler() {
    // The compositor-side output pick (version 29, ADR-0128) rides the same
    // fail-closed gate as the other kinds; here the wire path forwards the
    // kind unchanged so the handler can drive its output-mode chrome.
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
    let mut scoped = Client::connect_scoped(&path, requested, "pick").expect("scoped connect");
    scoped
        .pick_target(tessera_ipc::PickKind::Output)
        .expect("output pick succeeds");
    assert_eq!(
        handler.picks.lock().unwrap().as_slice(),
        &[(1, tessera_ipc::PickKind::Output)]
    );

    // The op gate is kind-agnostic: an unscoped connection is refused.
    let mut unscoped = Client::connect_with(&path, requested).expect("unscoped connect");
    let err = unscoped
        .pick_target(tessera_ipc::PickKind::Output)
        .unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");
}
