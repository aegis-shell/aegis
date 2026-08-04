use super::*;

#[test]
fn request_getwindows_serializes_as_tagged_unit() {
    let json = serde_json::to_string(&Request::GetWindows).unwrap();
    assert_eq!(json, r#"{"type":"GetWindows"}"#);
}

#[test]
fn hello_round_trips() {
    let req = Request::Hello {
        version: PROTOCOL_VERSION,
        caps: ConnectionCapabilities {
            query: true,
            control: false,
            input: false,
            session: true,
            interaction_domain: false,
        },
        scope: None,
        lease: Some(LeaseRequest::default()),
        agent: None,
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: Request = serde_json::from_str(&json).unwrap();
    assert_eq!(back, req);
}

#[test]
fn hello_with_agent_declaration_round_trips() {
    let req = Request::Hello {
        version: PROTOCOL_VERSION,
        caps: ConnectionCapabilities::QUERY,
        scope: None,
        lease: None,
        agent: Some(AgentHello {
            label: Some("Codex".into()),
            requested: vec![
                ActorCapability::Focus,
                ActorCapability::CaptureInteractionDomain,
            ],
            credential: Some("cred".into()),
        }),
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: Request = serde_json::from_str(&json).unwrap();
    assert_eq!(back, req);
}

#[test]
fn exact_resource_grant_requests_round_trip_without_ambient_fields() {
    let resource = ActorResource::NetworkOrigin {
        scheme: "https".into(),
        host: "amazon.com".into(),
        port: None,
    };
    for request in [
        Request::RequestResourceGrant {
            resource: resource.clone(),
            ttl_ms: 30_000,
            uses: 1,
        },
        Request::ConsumeResourceGrant {
            id: ResourceGrantId("opaque-handle".into()),
            resource: resource.clone(),
        },
        Request::RevokeResourceGrant {
            id: ResourceGrantId("opaque-handle".into()),
        },
    ] {
        let encoded = serde_json::to_string(&request).unwrap();
        assert_eq!(serde_json::from_str::<Request>(&encoded).unwrap(), request);
    }
}

#[test]
fn accessibility_provider_protocol_round_trips_complete_revisions_and_actions() {
    let update = AccessibilityTreeUpdate {
        window: WindowId(42),
        revision: 7,
        nodes: vec![aegis_semantic::AccessibilityNode {
            local_id: 1,
            parent_local_id: None,
            role: SemanticRole::Button,
            name: Some("Submit".into()),
            description: None,
            value: None,
            bounds: Rect::new(0, 0, 80, 32),
            state: SemanticState {
                visible: true,
                enabled: true,
                ..SemanticState::default()
            },
            actions: vec![SemanticAction::Invoke],
        }],
    };
    let request = Request::PublishAccessibilityTree {
        update: update.clone(),
    };
    let encoded = serde_json::to_string(&request).unwrap();
    assert_eq!(serde_json::from_str::<Request>(&encoded).unwrap(), request);
    let bindings = serde_json::to_string(&Request::GetAccessibilityWindows).unwrap();
    assert_eq!(
        serde_json::from_str::<Request>(&bindings).unwrap(),
        Request::GetAccessibilityWindows
    );

    let response = Response::AccessibilityAction {
        request: Some(SemanticActionRequest {
            request_id: 11,
            target: aegis_model::semantic::SemanticObjectId {
                window: update.window,
                local: 1,
            },
            provider_node_id: 1,
            tree_revision: update.revision,
            action: aegis_model::semantic::SemanticActionIntent::Invoke,
        }),
    };
    let encoded = serde_json::to_string(&response).unwrap();
    let decoded = serde_json::from_str::<Response>(&encoded).unwrap();
    assert_eq!(
        serde_json::to_value(decoded).unwrap(),
        serde_json::to_value(response).unwrap()
    );
}

#[test]
fn response_hello_carries_pairing_outcome_only_when_present() {
    let bare = Response::Hello {
        version: PROTOCOL_VERSION,
        caps: ConnectionCapabilities::QUERY,
        scope: Scope::unscoped(),
        lease: None,
        session: None,
        agent: None,
    };
    assert!(
        serde_json::to_value(&bare).unwrap().get("agent").is_none(),
        "absent pairing stays off the wire"
    );
    let paired = Response::Hello {
        version: PROTOCOL_VERSION,
        caps: ConnectionCapabilities::QUERY,
        scope: Scope::unscoped(),
        lease: None,
        session: None,
        agent: Some(AgentIssued {
            principal: "prin_1".into(),
            credential: Some("cred".into()),
        }),
    };
    let json = serde_json::to_string(&paired).unwrap();
    let back: Response = serde_json::from_str(&json).unwrap();
    assert_eq!(
        serde_json::to_value(back).unwrap(),
        serde_json::to_value(&paired).unwrap(),
        "pairing outcome round-trips"
    );
}

#[test]
fn sensitive_response_copies_are_zeroized_explicitly() {
    let mut hello = Response::Hello {
        version: PROTOCOL_VERSION,
        caps: ConnectionCapabilities::QUERY,
        scope: Scope::unscoped(),
        lease: None,
        session: None,
        agent: Some(AgentIssued {
            principal: "prin_1".into(),
            credential: Some("credential-secret".into()),
        }),
    };
    hello.zeroize_sensitive();
    let Response::Hello {
        agent: Some(AgentIssued { credential, .. }),
        ..
    } = &hello
    else {
        panic!("expected paired hello")
    };
    assert_eq!(credential.as_deref(), Some(""));

    let mut registered = Response::AgentRegistered {
        principal: "prin_2".into(),
        credential: "registered-secret".into(),
    };
    registered.zeroize_sensitive();
    let Response::AgentRegistered { credential, .. } = registered else {
        panic!("expected registered agent")
    };
    assert!(credential.is_empty());

    let mut secret = SecretPromptResult::Secret {
        value: "typed-secret".into(),
    };
    secret.zeroize();
    assert_eq!(
        secret,
        SecretPromptResult::Secret {
            value: String::new()
        }
    );

    let response_debug = format!(
        "{:?}",
        Response::SecretPrompted {
            result: SecretPromptResult::Secret {
                value: "never-log-me".into()
            }
        }
    );
    assert_eq!(response_debug, "SecretPrompted");
    assert!(!response_debug.contains("never-log-me"));
    let hello_debug = format!(
        "{:?}",
        AgentHello {
            label: Some("Agent".into()),
            requested: Vec::new(),
            credential: Some("never-log-this-credential".into()),
        }
    );
    assert!(hello_debug.contains("[REDACTED]"));
    assert!(!hello_debug.contains("never-log-this-credential"));
}

#[test]
fn caps_intersect_and_force_query() {
    let client = ConnectionCapabilities {
        query: true,
        control: true,
        input: true,
        session: true,
        interaction_domain: true,
    };
    let policy = ConnectionCapabilities::QUERY; // query only
    let granted = policy.intersect(client).with_query_always();
    assert!(granted.query);
    assert!(!granted.control);
    assert!(!granted.input);
    assert!(!granted.session);
}

#[test]
fn capabilities_from_older_v2_peer_default_input_off() {
    let caps: ConnectionCapabilities =
        serde_json::from_str(r#"{"query":true,"control":true,"session":false}"#).unwrap();
    assert!(caps.query);
    assert!(caps.control);
    assert!(!caps.input);
}

#[test]
fn windows_response_round_trips_with_a_window() {
    let mut w = Window::new(WindowId(42));
    w.title = Some("demo".into());
    w.app_id = Some("org.example.app".into());
    w.state.activated = true;
    let resp = Response::Windows { windows: vec![w] };
    let json = serde_json::to_string(&resp).unwrap();
    let back: Response = serde_json::from_str(&json).unwrap();
    match back {
        Response::Windows { windows } => {
            assert_eq!(windows.len(), 1);
            assert_eq!(windows[0].id, WindowId(42));
            assert_eq!(windows[0].title.as_deref(), Some("demo"));
            assert!(windows[0].state.activated);
        }
        _ => panic!("expected Windows"),
    }
}

#[test]
fn command_round_trips_and_tags() {
    let cmd = Command::Close { id: WindowId(7) };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains(r#""type":"Close""#), "{json}");
    assert!(json.contains(r#""id":7"#), "{json}");
    let back: Command = serde_json::from_str(&json).unwrap();
    assert_eq!(back, cmd);
}

#[test]
fn minimize_command_round_trips_and_is_window_scoped() {
    let cmd = Command::Minimize { id: WindowId(9) };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(json, r#"{"type":"Minimize","id":9}"#);
    assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd);

    let scope = Scope {
        windows: Some(vec![WindowId(9)]),
        ops: Some(vec![ActorCapability::Minimize]),
        ..Scope::default()
    };
    assert!(scope.permits(&cmd));
    assert!(!scope.permits(&Command::Minimize { id: WindowId(10) }));
}

#[test]
fn geometry_command_is_control_scoped_and_validated() {
    let cmd = Command::SetWindowGeometry {
        id: WindowId(9),
        rect: Rect::new(10, 20, 800, 600),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd);
    assert!(cmd.required_cap().control);
    assert!(cmd.validate().is_ok());
    assert!(
        Command::SetWindowGeometry {
            id: WindowId(9),
            rect: Rect::new(0, 0, 0, 600),
        }
        .validate()
        .is_err()
    );

    let scope = Scope {
        windows: Some(vec![WindowId(9)]),
        ops: Some(vec![ActorCapability::SetWindowGeometry]),
        ..Scope::default()
    };
    assert!(scope.permits(&cmd));
}

#[test]
fn synthetic_input_is_separately_capability_and_window_scoped() {
    let cmd = Command::InjectInput {
        id: WindowId(9),
        actions: vec![SyntheticInputAction::Click {
            position: aegis_model::Point { x: 20, y: 30 },
            button: 0x110,
        }],
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd);
    let cap = cmd.required_cap();
    assert!(cap.input);
    assert!(!cap.control);
    assert!(cmd.validate().is_ok());

    let scope = Scope {
        windows: Some(vec![WindowId(9)]),
        ops: Some(vec![ActorCapability::InjectInput]),
        ..Scope::default()
    };
    assert!(scope.permits(&cmd));
    assert!(!scope.permits(&Command::InjectInput {
        id: WindowId(10),
        actions: vec![SyntheticInputAction::KeyPress { code: 30 }],
    }));
    assert!(
        Command::InjectInput {
            id: WindowId(9),
            actions: vec![],
        }
        .validate()
        .is_err()
    );
}

#[test]
fn required_cap_separates_control_and_session() {
    assert!(Command::Focus { id: WindowId(1) }.required_cap().control);
    assert!(Command::Cycle { forward: true }.required_cap().control);
    assert!(Command::Quit.required_cap().session);
    assert!(!Command::Quit.required_cap().control);
    assert!(
        Command::InjectInput {
            id: WindowId(1),
            actions: vec![SyntheticInputAction::KeyPress { code: 30 }],
        }
        .required_cap()
        .input
    );
}

#[test]
fn unguarded_interaction_domain_input_is_not_part_of_the_protocol() {
    let legacy =
        r#"{"type":"InjectInteractionDomainInput","interaction_domain":2,"id":9,"actions":[]}"#;
    assert!(serde_json::from_str::<Command>(legacy).is_err());
}

#[test]
fn event_serializes_as_tagged_unit() {
    let json = serde_json::to_string(&Event::WindowsChanged).unwrap();
    assert_eq!(json, r#"{"type":"WindowsChanged"}"#);
    let back: Event = serde_json::from_str(&json).unwrap();
    assert_eq!(back, Event::WindowsChanged);
}

#[test]
fn space_use_event_preserves_maximized_and_fullscreen() {
    let maximized = Event::SpaceUseChanged {
        state: SpaceUse::Maximized,
    };
    let fullscreen = Event::SpaceUseChanged {
        state: SpaceUse::Fullscreen,
    };
    let maximized_json = serde_json::to_string(&maximized).unwrap();
    let fullscreen_json = serde_json::to_string(&fullscreen).unwrap();
    assert!(maximized_json.contains(r#""state":"maximized""#));
    assert!(fullscreen_json.contains(r#""state":"fullscreen""#));
    assert_ne!(maximized_json, fullscreen_json);
    assert_eq!(
        serde_json::from_str::<Event>(&fullscreen_json).unwrap(),
        fullscreen
    );
}

#[test]
fn set_maximized_command_round_trips_and_uses_geometry_authority() {
    let command = Command::SetMaximized {
        id: WindowId(42),
        maximized: true,
    };
    let json = serde_json::to_string(&command).unwrap();
    assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), command);
    assert_eq!(command.op_class(), ActorCapability::SetWindowGeometry);
    assert!(command.required_cap().control);
}

#[test]
fn switch_workspace_command_round_trips() {
    // A nested internally-tagged enum (Command variant carrying `Switch`).
    let cmd = Command::SwitchWorkspace { dir: Switch::Next };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains(r#""type":"SwitchWorkspace""#), "{json}");
    assert!(json.contains(r#""dir":{"type":"Next"}"#), "{json}");
    let back: Command = serde_json::from_str(&json).unwrap();
    assert_eq!(back, cmd);
}

#[test]
fn toggle_tiling_command_round_trips() {
    let json = serde_json::to_string(&Command::ToggleTiling).unwrap();
    assert_eq!(json, r#"{"type":"ToggleTiling"}"#);
    assert_eq!(
        serde_json::from_str::<Command>(&json).unwrap(),
        Command::ToggleTiling
    );
    assert!(Command::ToggleTiling.required_cap().control);
}

#[test]
fn system_control_command_has_a_stable_tagged_shape() {
    let cmd = Command::System {
        action: SystemAction::SetVolume { level: 55 },
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(
        json,
        r#"{"type":"System","action":{"type":"SetVolume","level":55}}"#
    );
    assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd);
    assert_eq!(cmd.op_class(), ActorCapability::SystemControl);
    assert!(cmd.required_cap().control);
    assert!(cmd.validate().is_ok());
}

#[test]
fn output_power_command_has_a_stable_tagged_shape() {
    let cmd = Command::System {
        action: SystemAction::SetOutputPower { powered: false },
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(
        json,
        r#"{"type":"System","action":{"type":"SetOutputPower","powered":false}}"#
    );
    assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd);
    assert!(cmd.required_cap().control);
}

#[test]
fn move_to_workspace_command_round_trips() {
    let cmd = Command::MoveToWorkspace {
        window: WindowId(42),
        workspace: WorkspaceId(3),
    };
    let json = serde_json::to_string(&cmd).unwrap();
    assert!(json.contains(r#""type":"MoveToWorkspace""#), "{json}");
    assert!(
        json.contains(r#""window":42"#) && json.contains(r#""workspace":3"#),
        "{json}"
    );
    assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd);
    assert!(cmd.required_cap().control);
}

#[test]
fn dismiss_notification_command_round_trips() {
    let cmd = Command::DismissNotification { id: 7 };
    let json = serde_json::to_string(&cmd).unwrap();
    assert_eq!(json, r#"{"type":"DismissNotification","id":7}"#);
    assert_eq!(serde_json::from_str::<Command>(&json).unwrap(), cmd);
    assert!(cmd.required_cap().control);
}

#[test]
fn unscoped_scope_permits_everything() {
    let s = Scope::unscoped();
    assert!(s.permits(&Command::Focus { id: WindowId(1) }));
    assert!(s.permits(&Command::Close { id: WindowId(99) }));
    assert!(s.permits(&Command::Quit));
    assert!(!s.permits(&Command::InjectInput {
        id: WindowId(1),
        actions: vec![SyntheticInputAction::KeyPress { code: 30 }],
    }));
}

#[test]
fn scoped_ops_reject_unlisted_commands() {
    let s = Scope {
        ops: Some(vec![ActorCapability::Focus]),
        ..Scope::default()
    };
    assert!(s.permits(&Command::Focus { id: WindowId(1) }));
    assert!(!s.permits(&Command::Close { id: WindowId(1) }));
}

#[test]
fn scope_ask_ops_serialize_only_when_present() {
    let with_ask = Scope {
        ask_ops: Some(vec![ActorCapability::Close]),
        ..Scope::default()
    };
    let json = serde_json::to_value(&with_ask).unwrap();
    assert_eq!(json["ask_ops"], serde_json::json!([{ "type": "Close" }]));
    assert_eq!(
        serde_json::from_value::<Scope>(json).unwrap(),
        with_ask,
        "ask_ops round-trips"
    );
    assert!(
        serde_json::to_value(Scope::default())
            .unwrap()
            .get("ask_ops")
            .is_none(),
        "absent ask_ops stays off the wire"
    );
}

#[test]
fn ask_ops_make_unlisted_commands_requestable_not_permitted() {
    let s = Scope {
        ops: Some(vec![ActorCapability::Focus]),
        ask_ops: Some(vec![ActorCapability::Close]),
        ..Scope::default()
    };
    let close = Command::Close { id: WindowId(1) };
    assert!(!s.permits(&close), "an ask entry never pre-grants");
    assert_eq!(
        s.decide_command(&close),
        AuthorizationDecision::Ask(ActorCapability::Close)
    );
    assert_eq!(
        s.decide_command(&Command::Focus { id: WindowId(1) }),
        AuthorizationDecision::Permit
    );
    assert_eq!(
        s.decide_command(&Command::Minimize { id: WindowId(1) }),
        AuthorizationDecision::Deny,
        "neither ops nor ask_ops names Minimize"
    );
}

#[test]
fn ask_decision_still_enforces_resource_allowlists() {
    let s = Scope {
        windows: Some(vec![WindowId(1)]),
        ops: Some(vec![ActorCapability::Focus]),
        ask_ops: Some(vec![ActorCapability::Close]),
        ..Scope::default()
    };
    assert_eq!(
        s.decide_command(&Command::Close { id: WindowId(1) }),
        AuthorizationDecision::Ask(ActorCapability::Close)
    );
    assert_eq!(
        s.decide_command(&Command::Close { id: WindowId(9) }),
        AuthorizationDecision::Deny,
        "a window outside the allowlist is outside the ask ceiling"
    );
}

#[test]
fn unscoped_scope_never_asks() {
    let s = Scope::unscoped();
    assert!(!s.asks(ActorCapability::Close));
    assert_eq!(
        s.decide_interaction_domain_input(InteractionDomainId(2)),
        AuthorizationDecision::Deny
    );
    assert_eq!(
        s.decide_interaction_domain_capture(InteractionDomainId(2)),
        AuthorizationDecision::Deny
    );
}

#[test]
fn interaction_domain_action_and_capture_have_ask_decisions() {
    let s = Scope {
        ask_ops: Some(vec![
            ActorCapability::TransactInteractionDomain,
            ActorCapability::CaptureInteractionDomain,
        ]),
        ..Scope::default()
    };
    let transact = InteractionDomainAction::Transact {
        expected_revision: None,
        mutations: vec![InteractionDomainMutation::SetState {
            interaction_domain: InteractionDomainId(2),
            state: aegis_model::interaction_domain::InteractionDomainState::Paused,
        }],
    };
    assert!(!s.permits_interaction_domain_action(&transact));
    assert_eq!(
        s.decide_interaction_domain_action(&transact),
        AuthorizationDecision::Ask(ActorCapability::TransactInteractionDomain)
    );
    assert_eq!(
        s.decide_interaction_domain_capture(InteractionDomainId(2)),
        AuthorizationDecision::Ask(ActorCapability::CaptureInteractionDomain)
    );
    let create = InteractionDomainAction::Create {
        label: "agent".into(),
        capabilities: SeatCapabilities::POINTER_KEYBOARD,
        output: None,
    };
    assert_eq!(
        s.decide_interaction_domain_action(&create),
        AuthorizationDecision::Deny,
        "CreateInteractionDomain is in neither ops nor ask_ops"
    );
}

#[test]
fn scoped_windows_enforce_allowlist() {
    let s = Scope {
        windows: Some(vec![WindowId(1), WindowId(2)]),
        ..Scope::default()
    };
    assert!(s.permits(&Command::Focus { id: WindowId(1) }));
    assert!(s.permits(&Command::Focus { id: WindowId(2) }));
    assert!(!s.permits(&Command::Focus { id: WindowId(3) }));
}

#[test]
fn session_commands_bypass_scope() {
    let s = Scope {
        ops: Some(vec![]),
        ..Scope::default()
    };
    assert!(s.permits(&Command::Quit), "Quit is session-level");
}

#[test]
fn move_to_workspace_checks_both_window_and_workspace() {
    let s = Scope {
        windows: Some(vec![WindowId(1)]),
        workspaces: Some(vec![WorkspaceId(2)]),
        ..Scope::default()
    };
    assert!(s.permits(&Command::MoveToWorkspace {
        window: WindowId(1),
        workspace: WorkspaceId(2)
    }));
    assert!(!s.permits(&Command::MoveToWorkspace {
        window: WindowId(1),
        workspace: WorkspaceId(3)
    }));
    assert!(!s.permits(&Command::MoveToWorkspace {
        window: WindowId(9),
        workspace: WorkspaceId(2)
    }));
}

#[test]
fn screenshot_command_op_class_depends_on_region_presence() {
    let full = Command::Screenshot {
        path: "a.png".into(),
        region: None,
    };
    let region = Command::Screenshot {
        path: "b.png".into(),
        region: Some(Rect::new(10, 20, 100, 80)),
    };
    assert_eq!(full.op_class(), ActorCapability::Screenshot);
    assert_eq!(region.op_class(), ActorCapability::ScreenshotRegion);

    let full_scope = Scope {
        ops: Some(vec![ActorCapability::Screenshot]),
        ..Scope::default()
    };
    let region_scope = Scope {
        ops: Some(vec![ActorCapability::ScreenshotRegion]),
        ..Scope::default()
    };
    assert!(full_scope.permits(&full));
    assert!(!full_scope.permits(&region));
    assert!(region_scope.permits(&region));
    assert!(!region_scope.permits(&full));
}

#[test]
fn command_validation_bounds_private_text_and_exact_paths() {
    let valid_notification = Command::Notify {
        summary: "Ready".into(),
        body: "done".into(),
        app_id: Some("org.example.App".into()),
        external_id: None,
    };
    assert!(valid_notification.validate().is_ok());
    assert!(
        Command::Notify {
            summary: "x".repeat(1_025),
            body: String::new(),
            app_id: None,
            external_id: None,
        }
        .validate()
        .is_err()
    );
    assert!(
        Command::Notify {
            summary: "bad\0summary".into(),
            body: String::new(),
            app_id: None,
            external_id: None,
        }
        .validate()
        .is_err()
    );

    assert!(
        Command::Screenshot {
            path: "/tmp/aegis-shot.png".into(),
            region: None,
        }
        .validate()
        .is_ok()
    );
    for path in ["relative.png", "/tmp/../secret.png", "/tmp//shot.png"] {
        assert!(
            Command::Screenshot {
                path: path.into(),
                region: None,
            }
            .validate()
            .is_err(),
            "accepted ambiguous path {path}"
        );
    }

    for desktop_id in ["", ".", "..", "../app.desktop", "bad\\app.desktop"] {
        assert!(
            Command::LaunchInInteractionDomain {
                interaction_domain: InteractionDomainId(2),
                desktop_id: desktop_id.into(),
            }
            .validate()
            .is_err(),
            "accepted malformed desktop id {desktop_id}"
        );
    }
    assert!(
        Command::LaunchInInteractionDomain {
            interaction_domain: InteractionDomainId(2),
            desktop_id: "org.example.App.desktop".into(),
        }
        .validate()
        .is_ok()
    );
}

#[test]
fn capture_output_request_round_trips_with_optional_region() {
    let req = Request::CaptureOutput {
        region: Some(Rect::new(10, 20, 100, 80)),
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("\"region\""), "{json}");
    let back: Request = serde_json::from_str(&json).unwrap();
    assert_eq!(back, req);

    let default: Request = serde_json::from_str(r#"{"type":"CaptureOutput"}"#).unwrap();
    assert_eq!(default, Request::CaptureOutput { region: None });
}

#[test]
fn interaction_domain_capture_response_round_trips_correlated_layout_metadata() {
    let capture = InteractionDomainCapture {
        interaction_domain: InteractionDomainId(7),
        width: 500,
        height: 250,
        scale_milli: 1250,
        region: Rect::new(100, 50, 400, 200),
        placements: vec![InteractionDomainWindowPlacement {
            window: WindowId(42),
            output_rect: Rect::new(120, 70, 300, 150),
            surface_size: aegis_model::Size { w: 900, h: 450 },
        }],
        observation: SemanticObservation {
            token: ObservationToken("a".repeat(64)),
            ttl_ms: 15_000,
            snapshot: SemanticSnapshot {
                interaction_domain: InteractionDomainId(7),
                authority_revision: 19,
                objects: Vec::new(),
            },
        },
        png_bytes: 3,
        revision: 19,
    };
    let json = serde_json::to_string(&Response::CaptureInteractionDomain { capture })
        .expect("serialize capture");
    let decoded: Response = serde_json::from_str(&json).expect("deserialize capture");
    let Response::CaptureInteractionDomain { capture } = decoded else {
        panic!("expected InteractionDomain capture response");
    };
    assert_eq!(capture.interaction_domain, InteractionDomainId(7));
    assert_eq!(capture.region, Rect::new(100, 50, 400, 200));
    assert_eq!(capture.placements[0].window, WindowId(42));
    assert_eq!(
        capture.placements[0].surface_size,
        aegis_model::Size { w: 900, h: 450 }
    );
    assert_eq!(capture.revision, 19);
}

#[test]
fn hello_with_scope_name_round_trips() {
    let req = Request::Hello {
        version: PROTOCOL_VERSION,
        caps: ConnectionCapabilities::QUERY,
        scope: Some("browser-helper".into()),
        lease: None,
        agent: None,
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: Request = serde_json::from_str(&json).unwrap();
    assert_eq!(back, req);
}

#[test]
fn stream_output_start_round_trips_with_optional_max_fps() {
    let with_fps = Request::StreamOutputStart {
        max_fps: Some(60),
        target: StreamTarget::Output,
    };
    let json = serde_json::to_string(&with_fps).unwrap();
    assert!(json.contains(r#""type":"StreamOutputStart""#), "{json}");
    assert!(json.contains(r#""max_fps":60"#), "{json}");
    assert!(
        !json.contains("target"),
        "default target is skipped: {json}"
    );
    assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), with_fps);

    let default: Request = serde_json::from_str(r#"{"type":"StreamOutputStart"}"#).unwrap();
    assert_eq!(
        default,
        Request::StreamOutputStart {
            max_fps: None,
            target: StreamTarget::Output
        }
    );
}

#[test]
fn stream_output_start_round_trips_a_window_target() {
    let req = Request::StreamOutputStart {
        max_fps: None,
        target: StreamTarget::Window {
            window: WindowId(7),
        },
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(
        json.contains(r#""target":{"type":"Window","window":7}"#),
        "{json}"
    );
    assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), req);
}

#[test]
fn pick_target_round_trips_all_kinds_and_results() {
    for kind in [PickKind::Region, PickKind::Pixel, PickKind::Window] {
        let req = Request::PickTarget { kind };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), req);
    }
    for result in [
        PickResult::Region {
            rect: Rect::new(10, 20, 300, 200),
        },
        PickResult::Pixel {
            point: aegis_model::Point { x: 4, y: 8 },
            rgb: [255, 128, 0],
        },
        PickResult::Window { id: WindowId(3) },
        PickResult::Output,
        PickResult::Cancelled,
    ] {
        let resp = Response::Picked { result };
        let json = serde_json::to_string(&resp).unwrap();
        match serde_json::from_str::<Response>(&json).unwrap() {
            Response::Picked { result: back } => assert_eq!(back, result),
            other => panic!("expected Picked, got {other:?}"),
        }
    }
}

#[test]
fn stream_output_stop_round_trips() {
    let req = Request::StreamOutputStop { stream_id: 9 };
    let json = serde_json::to_string(&req).unwrap();
    assert_eq!(json, r#"{"type":"StreamOutputStop","stream_id":9}"#);
    assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), req);
}

#[test]
fn set_idle_inhibit_round_trips() {
    let req = Request::SetIdleInhibit { inhibit: true };
    let json = serde_json::to_string(&req).unwrap();
    assert_eq!(json, r#"{"type":"SetIdleInhibit","inhibit":true}"#);
    assert_eq!(serde_json::from_str::<Request>(&json).unwrap(), req);

    let resp = Response::IdleInhibitSet { inhibited: false };
    let json = serde_json::to_string(&resp).unwrap();
    assert_eq!(json, r#"{"type":"IdleInhibitSet","inhibited":false}"#);
    match serde_json::from_str::<Response>(&json).unwrap() {
        Response::IdleInhibitSet { inhibited } => assert!(!inhibited),
        other => panic!("expected IdleInhibitSet, got {other:?}"),
    }
}

#[test]
fn stream_output_started_response_round_trips() {
    let resp = Response::StreamOutputStarted {
        stream_id: 3,
        width: 1920,
        height: 1080,
        format: StreamPixelFormat::Bgra8,
    };
    let json = serde_json::to_string(&resp).unwrap();
    assert!(json.contains(r#""format":{"type":"Bgra8"}"#), "{json}");
    let back: Response = serde_json::from_str(&json).unwrap();
    match back {
        Response::StreamOutputStarted {
            stream_id,
            width,
            height,
            format,
        } => {
            assert_eq!((stream_id, width, height), (3, 1920, 1080));
            assert_eq!(format, StreamPixelFormat::Bgra8);
        }
        other => panic!("expected StreamOutputStarted, got {other:?}"),
    }
}

#[test]
fn stream_frame_event_round_trips_with_metadata() {
    let event = Event::StreamFrame {
        stream_id: 1,
        sequence: 42,
        width: 640,
        height: 480,
        stride: 2560,
        format: StreamPixelFormat::Bgra8,
        damage: vec![Rect::new(0, 0, 640, 480)],
        dropped: 7,
        byte_len: 640 * 480 * 4,
    };
    let json = serde_json::to_string(&event).unwrap();
    assert!(json.contains(r#""sequence":42"#), "{json}");
    assert!(json.contains(r#""dropped":7"#), "{json}");
    assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
}

#[test]
fn stream_ended_event_round_trips() {
    let event = Event::StreamEnded {
        stream_id: 5,
        reason: "scope revoked".into(),
    };
    let json = serde_json::to_string(&event).unwrap();
    assert_eq!(serde_json::from_str::<Event>(&json).unwrap(), event);
}
