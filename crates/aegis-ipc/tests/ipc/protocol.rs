use super::*;

#[test]
fn get_windows_returns_the_live_snapshot() {
    let path = scratch();
    let handler = Arc::new(TestHandler::query(sample_windows()));
    let _server = Server::start(&path, handler).expect("bind");

    let mut client = Client::connect(&path).expect("connect");
    let windows = client.windows().expect("get_windows");
    assert_eq!(windows.len(), 2);
    assert_eq!(windows[0].id, WindowId(1));
    assert_eq!(windows[0].title.as_deref(), Some("first"));
    assert!(windows[0].state.activated);
    assert_eq!(windows[1].id, WindowId(2));
    assert!(windows[1].state.maximized);
}

#[test]
fn repeated_requests_on_one_connection() {
    let path = scratch();
    let handler = Arc::new(TestHandler::query(sample_windows()));
    let _server = Server::start(&path, handler).expect("bind");

    let mut client = Client::connect(&path).expect("connect");
    let first = client.windows().expect("first get");
    let second = client.windows().expect("second get");
    assert_eq!(first.len(), second.len());
    assert_eq!(first[0].id, second[0].id);
}

#[test]
fn capture_writer_rechecks_live_security_before_sending_memfd() {
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
    let mut client = Client::connect_scoped(&path, requested, "capture").expect("connect");
    handler
        .capture_security_active
        .store(false, Ordering::Release);
    let error = client
        .capture_output()
        .expect_err("writer must refuse pixels after the live gate closes");
    assert!(
        error.to_string().contains("authorization changed"),
        "{error}"
    );
}

#[test]
fn authenticated_capture_rechecks_the_live_principal_ceiling() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let identity = aegis_ipc::AgentIdentity {
        principal: aegis_security::authority::ActorPrincipal::new("prin_capture").unwrap(),
        pregranted: vec![ActorCapability::CaptureOutput],
        gated: vec![],
    };
    *handler.lookup_result.lock().unwrap() = Some(identity.clone());
    *handler.refresh_result.lock().unwrap() = Ok(Some(identity));
    handler.capture_delay_ms.store(200, Ordering::Relaxed);
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let requested = ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    };
    let mut client = Client::connect_agent_with_timeout(
        &path,
        requested,
        None,
        aegis_ipc::AgentHello {
            label: Some("capture test".into()),
            requested: vec![ActorCapability::CaptureOutput],
            credential: Some("cred_capture".into()),
        },
        Duration::from_secs(5),
    )
    .expect("authenticated connect");

    let revoke = Arc::clone(&handler);
    let revoker = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(50));
        *revoke.refresh_result.lock().unwrap() = Err("principal was revoked".into());
    });
    let error = client
        .capture_output()
        .expect_err("revoked principal must not receive in-flight pixels");
    revoker.join().unwrap();
    assert!(error.to_string().contains("out of scope"), "{error}");
}

#[test]
fn capture_window_delivers_pixels_for_a_pregranted_window() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, handler).expect("bind");
    let requested = ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    };
    let mut client = Client::connect_scoped(&path, requested, "capture-window").expect("connect");
    let capture = client
        .capture_window(WindowId(1))
        .expect("pregranted window capture");
    assert_eq!(capture.window, WindowId(1));
    assert_eq!(capture.png, vec![6u8, 5, 4]);
    assert_eq!(capture.scale_milli, 1000);
}

#[test]
fn capture_window_is_refused_without_the_explicit_operation() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, handler).expect("bind");
    let requested = ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    };
    let mut client = Client::connect_scoped(&path, requested, "capture").expect("connect");
    let error = client
        .capture_window(WindowId(1))
        .expect_err("CaptureOutput must not imply CaptureWindow");
    assert!(error.to_string().contains("out of scope"), "{error}");
}

#[test]
fn capture_window_is_refused_outside_the_window_allowlist() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, handler).expect("bind");
    let requested = ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    };
    let mut client = Client::connect_scoped(&path, requested, "capture-window").expect("connect");
    let error = client
        .capture_window(WindowId(2))
        .expect_err("window outside the scope allowlist must be refused");
    assert!(error.to_string().contains("out of scope"), "{error}");
}

#[test]
fn capture_window_writer_rechecks_live_security_before_sending_memfd() {
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
    let mut client = Client::connect_scoped(&path, requested, "capture-window").expect("connect");
    handler
        .capture_security_active
        .store(false, Ordering::Release);
    let error = client
        .capture_window(WindowId(1))
        .expect_err("writer must refuse pixels after the live gate closes");
    assert!(
        error.to_string().contains("authorization changed"),
        "{error}"
    );
}

#[test]
fn wrong_protocol_version_is_refused_at_handshake() {
    use aegis_ipc::Request;
    let path = scratch();
    let handler = Arc::new(TestHandler::query(vec![]));
    let _server = Server::start(&path, handler).expect("bind");

    let mut s = std::os::unix::net::UnixStream::connect(&path).unwrap();
    let bad = Request::Hello {
        version: PROTOCOL_VERSION + 1,
        caps: ConnectionCapabilities::QUERY,
        scope: None,
        lease: None,
        agent: None,
    };
    aegis_ipc::codec::write_msg(&mut s, &bad).unwrap();
    let resp: aegis_ipc::Response = aegis_ipc::codec::read_msg(&mut s).unwrap();
    match resp {
        aegis_ipc::Response::Error { message } => {
            assert!(
                message.contains("unsupported protocol version"),
                "{message}"
            );
        }
        other => panic!("expected Error, got {other:?}"),
    }
}

#[test]
fn control_command_is_queued_and_acked() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let mut client = Client::connect_with(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
    )
    .expect("connect");
    client
        .command(Command::Close { id: WindowId(2) })
        .expect("command");
    client
        .command(Command::Focus {
            id: WindowId(1),
            reveal: true,
        })
        .expect("command");
    client
        .command(Command::Cycle { forward: true })
        .expect("command");

    // The command() calls block until the server acked (Ok), by which point
    // the handler has recorded each one.
    let recorded = handler.commands.lock().unwrap();
    assert_eq!(recorded.len(), 3, "{recorded:?}");
    assert!(recorded.contains(&Command::Close { id: WindowId(2) }));
    assert!(recorded.contains(&Command::Focus {
        id: WindowId(1),
        reveal: true
    }));
}

#[test]
fn ipc_origins_are_unique_and_pre_dispatch_refusals_are_audited() {
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
    let mut first = Client::connect_with(&path, requested).expect("first connection");
    let mut second = Client::connect_with(&path, requested).expect("second connection");
    first
        .command(Command::Focus {
            id: WindowId(1),
            reveal: true,
        })
        .expect("first command");
    second
        .command(Command::Focus {
            id: WindowId(2),
            reveal: true,
        })
        .expect("second command");
    let ids = handler.command_connections.lock().unwrap().clone();
    assert_eq!(ids.len(), 2);
    assert_ne!(ids[0], 0);
    assert_ne!(ids[0], ids[1]);

    let mut query_only = Client::connect(&path).expect("query-only connection");
    query_only
        .command(Command::Close { id: WindowId(1) })
        .expect_err("missing control capability must be refused");
    let refusals = handler.refusals.lock().unwrap();
    let (conn_id, mutation, reason) = refusals.last().expect("audited refusal");
    assert_ne!(*conn_id, 0);
    assert!(reason.contains("capability"));
    assert!(matches!(
        mutation,
        aegis_ipc::JournalMutation::Command {
            cmd: aegis_ipc::AuditedCommand::Close { id: WindowId(1) }
        }
    ));
}

#[test]
fn resource_grants_revalidate_scope_and_audit_refusals_without_secrets() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let requested = ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    };
    let resource = aegis_ipc::ActorResource::FilesystemPath {
        path: PathBuf::from("/private/customer.db"),
        access: aegis_ipc::FilesystemAccess::Read,
    };
    let mut client =
        Client::connect_scoped(&path, requested, "file-resource").expect("scoped connect");
    let grant = client
        .request_resource_grant(resource.clone(), Duration::from_secs(10), 2)
        .expect("exact grant");
    let consumed = client
        .consume_resource_grant(grant.id.clone(), resource.clone())
        .expect("first use");
    assert_eq!(consumed.uses_remaining, 1);

    handler
        .scopes
        .lock()
        .unwrap()
        .get_mut("file-resource")
        .unwrap()
        .ops = Some(Vec::new());
    let error = client
        .consume_resource_grant(grant.id.clone(), resource)
        .expect_err("live capability removal must stop consumption");
    assert!(error.to_string().contains("out of scope"), "{error}");

    let refusals = handler.refusals.lock().unwrap();
    let (_, mutation, reason) = refusals.last().expect("resource refusal audited");
    assert_eq!(reason, "resource grant consume refused");
    assert!(matches!(
        mutation,
        aegis_ipc::JournalMutation::ResourceGrantAttempt {
            action: aegis_ipc::ResourceGrantAttemptAction::Consume,
            capability: Some(ActorCapability::ReadFile),
            resource_kind: Some(aegis_ipc::ResourceKind::FilesystemPath),
            ..
        }
    ));
    let encoded = serde_json::to_string(&(mutation, reason)).unwrap();
    assert!(!encoded.contains("/private/customer.db"));
    assert!(!encoded.contains(grant.id.0.as_str()));
}

#[test]
fn session_command_quit_is_accepted() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let mut client = Client::connect_scoped(
        &path,
        ConnectionCapabilities {
            query: true,
            control: false,
            input: false,
            session: true,
            interaction_domain: false,
        },
        aegis_ipc::LOCAL_AGENT_ADMIN_SCOPE,
    )
    .expect("connect");
    client.command(Command::Quit).expect("quit command");
    assert!(handler.commands.lock().unwrap().contains(&Command::Quit));
}

#[test]
fn command_refused_without_the_required_capability() {
    let path = scratch();
    // Permissive policy, but the client connects requesting query only.
    let handler = Arc::new(TestHandler::permissive(vec![]));
    let _server = Server::start(&path, handler).expect("bind");

    let mut client = Client::connect(&path).expect("connect"); // query only
    let err = client
        .command(Command::Close { id: WindowId(1) })
        .unwrap_err();
    assert!(err.to_string().contains("capability"), "{}", err);
}

#[test]
fn subscriber_receives_broadcast_events() {
    let path = scratch();
    let handler = Arc::new(TestHandler::query(vec![]));
    let server = Server::start(&path, handler).expect("bind");

    let mut client = Client::connect(&path).expect("connect");
    client.subscribe().expect("subscribe");
    // The handler/registry insert happened before Subscribed was sent, so the
    // subscriber is registered by the time we broadcast.
    server.broadcast(Event::WindowsChanged);
    let ev = client.next_event().expect("event");
    assert_eq!(ev, Event::WindowsChanged);
}

#[test]
fn get_workspaces_returns_snapshot() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, handler).expect("bind");

    let mut client = Client::connect(&path).expect("connect");
    let snap = client.workspaces().expect("get_workspaces");
    assert_eq!(snap.outputs.len(), 1);
    let o = &snap.outputs[0];
    // The test handler places every window id on its single workspace.
    assert_eq!(o.workspaces.len(), 1);
    assert_eq!(o.workspaces[0].toplevels.len(), 2);
}

#[test]
fn switch_workspace_command_is_queued() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let mut client = Client::connect_with(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
    )
    .expect("connect");
    client
        .switch_workspace(aegis_model::workspace::Switch::Next)
        .expect("switch");
    let recorded = handler.commands.lock().unwrap();
    assert!(
        recorded
            .iter()
            .any(|c| matches!(c, Command::SwitchWorkspace { .. })),
        "{recorded:?}"
    );
}

#[test]
fn workspace_changed_event_is_broadcast() {
    let path = scratch();
    let handler = Arc::new(TestHandler::query(vec![]));
    let server = Server::start(&path, handler).expect("bind");

    let mut client = Client::connect(&path).expect("connect");
    client.subscribe().expect("subscribe");
    server.broadcast(Event::WorkspaceChanged);
    let ev = client.next_event().expect("event");
    assert_eq!(ev, Event::WorkspaceChanged);
}

#[test]
fn notify_command_is_queued_and_acked() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let mut client = Client::connect_with(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
    )
    .expect("connect");
    client
        .notify("Hello", "world", Some("app".into()))
        .expect("notify");
    let recorded = handler.commands.lock().unwrap();
    assert!(
        recorded
            .iter()
            .any(|c| matches!(c, Command::Notify { summary, .. } if summary == "Hello")),
        "{recorded:?}"
    );
}

#[test]
fn notified_event_carries_the_notification() {
    let path = scratch();
    let handler = Arc::new(TestHandler::query(vec![]));
    let server = Server::start(&path, handler).expect("bind");

    let mut client = Client::connect(&path).expect("connect");
    client.subscribe().expect("subscribe");
    let n = aegis_model::notify::Notification {
        id: 7,
        summary: "ping".into(),
        body: "pong".into(),
        app_id: None,
        external_id: None,
        at_ms: 0,
    };
    server.broadcast(Event::Notified {
        notification: n.clone(),
    });
    let ev = client.next_event().expect("event");
    match ev {
        Event::Notified { notification } => assert_eq!(notification, n),
        other => panic!("expected Notified, got {other:?}"),
    }
}
