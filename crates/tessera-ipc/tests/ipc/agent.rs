use super::*;

#[test]
fn pairing_issues_credential_and_synthetic_scope() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    *handler.pair_result.lock().unwrap() = Ok(tessera_ipc::PairedAgent {
        principal: tessera_security::authority::ActorPrincipal::new("prin_1").unwrap(),
        credential: "cred_1".into(),
        pregranted: vec![ActorCapability::Focus],
        gated: vec![ActorCapability::CaptureInteractionDomain],
    });
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let client = Client::connect_agent_with_timeout(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: true,
            session: false,
            interaction_domain: true,
        },
        None,
        tessera_ipc::AgentHello {
            label: Some("Codex".into()),
            requested: vec![
                ActorCapability::Focus,
                ActorCapability::CaptureInteractionDomain,
            ],
            credential: None,
        },
        Duration::from_secs(5),
    )
    .expect("pairing connects");
    let issued = client.agent_issued().expect("pairing outcome");
    assert_eq!(issued.principal, "prin_1");
    assert_eq!(issued.credential.as_deref(), Some("cred_1"));
    assert_eq!(client.scope().ops, Some(vec![ActorCapability::Focus]));
    assert_eq!(
        client.scope().ask_ops,
        Some(vec![ActorCapability::CaptureInteractionDomain])
    );
    assert!(
        client.caps().input,
        "a paired agent keeps the input capability class"
    );
    let calls = handler.pair_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1.as_deref(), Some("Codex"));
    assert_eq!(
        calls[0].2,
        vec![
            ActorCapability::Focus,
            ActorCapability::CaptureInteractionDomain
        ]
    );
}

#[test]
fn authenticated_actor_observation_is_an_explicit_capability() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    *handler.pair_result.lock().unwrap() = Ok(tessera_ipc::PairedAgent {
        principal: tessera_security::authority::ActorPrincipal::new("prin_observer").unwrap(),
        credential: "cred_observer".into(),
        pregranted: vec![ActorCapability::Focus],
        gated: vec![],
    });
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut actor = grant_client(&path, ConnectionCapabilities::QUERY);
    let error = actor
        .windows()
        .expect_err("an action capability must not imply observation");
    assert!(error.to_string().contains("GetWindows"), "{error}");

    *handler.pair_result.lock().unwrap() = Ok(tessera_ipc::PairedAgent {
        principal: tessera_security::authority::ActorPrincipal::new("prin_observer_2").unwrap(),
        credential: "cred_observer_2".into(),
        pregranted: vec![ActorCapability::ObserveWindows],
        gated: vec![],
    });
    let mut observer = grant_client(&path, ConnectionCapabilities::QUERY);
    assert_eq!(observer.windows().expect("explicit observation").len(), 2);
}

#[test]
fn pairing_denial_refuses_the_handshake() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    *handler.pair_result.lock().unwrap() = Err("the user declined".into());
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let error = match Client::connect_agent_with_timeout(
        &path,
        ConnectionCapabilities::QUERY,
        None,
        tessera_ipc::AgentHello {
            label: None,
            requested: vec![],
            credential: None,
        },
        Duration::from_secs(5),
    ) {
        Ok(_) => panic!("denied pairing must refuse the connection"),
        Err(error) => error,
    };
    assert_eq!(error.kind(), std::io::ErrorKind::ConnectionRefused);
    assert!(error.to_string().contains("the user declined"));
}

#[test]
fn malformed_agent_declaration_is_rejected_before_pairing() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let result = Client::connect_agent_with_timeout(
        &path,
        ConnectionCapabilities::QUERY,
        None,
        tessera_ipc::AgentHello {
            label: Some("Codex".into()),
            requested: vec![ActorCapability::Focus, ActorCapability::Focus],
            credential: None,
        },
        Duration::from_secs(1),
    );
    let error = match result {
        Ok(_) => panic!("duplicate capability declaration was accepted"),
        Err(error) => error,
    };
    assert!(error.to_string().contains("Agent declaration"), "{error}");
    assert!(handler.pair_calls.lock().unwrap().is_empty());
}

#[test]
fn recognized_credential_binds_without_pairing() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    *handler.lookup_result.lock().unwrap() = Some(tessera_ipc::AgentIdentity {
        principal: tessera_security::authority::ActorPrincipal::new("prin_9").unwrap(),
        pregranted: vec![ActorCapability::Notify],
        gated: vec![],
    });
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let client = Client::connect_agent_with_timeout(
        &path,
        ConnectionCapabilities::QUERY,
        None,
        tessera_ipc::AgentHello {
            label: Some("Codex".into()),
            requested: vec![ActorCapability::Notify],
            credential: Some("cred_9".into()),
        },
        Duration::from_secs(5),
    )
    .expect("recognized credential connects");
    assert!(handler.pair_calls.lock().unwrap().is_empty());
    let issued = client.agent_issued().expect("pairing outcome");
    assert_eq!(issued.principal, "prin_9");
    assert!(issued.credential.is_none());
    assert_eq!(client.scope().ops, Some(vec![ActorCapability::Notify]));
    assert_eq!(client.scope().ask_ops, Some(vec![]));
}

#[test]
fn builtin_scope_connections_do_not_pair() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    handler.scopes.lock().unwrap().insert(
        tessera_ipc::LOCAL_INTERACTION_DOMAIN_ADMIN_SCOPE.to_string(),
        Scope {
            ops: Some(vec![ActorCapability::Notify]),
            ..Scope::default()
        },
    );
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let client = Client::connect_agent_with_timeout(
        &path,
        ConnectionCapabilities::QUERY,
        Some(tessera_ipc::LOCAL_INTERACTION_DOMAIN_ADMIN_SCOPE.to_string()),
        tessera_ipc::AgentHello {
            label: Some("tessera".into()),
            requested: vec![],
            credential: None,
        },
        Duration::from_secs(5),
    )
    .expect("built-in scope connects without pairing");
    assert!(handler.pair_calls.lock().unwrap().is_empty());
    assert!(client.agent_issued().is_none());
    assert_eq!(client.scope().ops, Some(vec![ActorCapability::Notify]));
}

#[test]
fn declared_scope_pairs_but_keeps_the_configured_ceiling() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    *handler.pair_result.lock().unwrap() = Ok(tessera_ipc::PairedAgent {
        principal: tessera_security::authority::ActorPrincipal::new("prin_2").unwrap(),
        credential: "cred_2".into(),
        pregranted: vec![ActorCapability::Close],
        gated: vec![],
    });
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let client = Client::connect_agent_with_timeout(
        &path,
        ConnectionCapabilities::QUERY,
        Some("focus-first".to_string()),
        tessera_ipc::AgentHello {
            label: Some("Codex".into()),
            requested: vec![ActorCapability::Close],
            credential: None,
        },
        Duration::from_secs(5),
    )
    .expect("declared scope connects");
    // The ceiling stays the configured scope, not the registry split.
    assert_eq!(client.scope().ops, Some(vec![ActorCapability::Focus]));
    assert_eq!(client.scope().windows, Some(vec![WindowId(1)]));
    assert!(client.agent_issued().is_some());
    assert_eq!(handler.pair_calls.lock().unwrap().len(), 1);
}

#[test]
fn lockdown_strips_privileges_from_unpaired_connections() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let privileged = ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    };
    let open = Client::connect_with(&path, privileged).expect("anonymous connect");
    assert!(open.caps().control, "default policy grants control");

    handler.lockdown_flag.store(true, Ordering::Relaxed);
    let locked = Client::connect_with(&path, privileged).expect("lockdown connect");
    assert!(
        !locked.caps().control,
        "lockdown strips privileged capabilities from unpaired connections"
    );
    assert!(locked.caps().query);
}

fn grant_paired_handler(
    pregranted: Vec<ActorCapability>,
    gated: Vec<ActorCapability>,
) -> (Arc<TestHandler>, PathBuf) {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    *handler.pair_result.lock().unwrap() = Ok(tessera_ipc::PairedAgent {
        principal: tessera_security::authority::ActorPrincipal::new("prin_g").unwrap(),
        credential: "cred_g".into(),
        pregranted,
        gated,
    });
    (handler, path)
}

fn grant_client(path: &std::path::Path, caps: ConnectionCapabilities) -> Client {
    Client::connect_agent_with_timeout(
        path,
        caps,
        None,
        tessera_ipc::AgentHello {
            label: Some("Codex".into()),
            requested: vec![],
            credential: None,
        },
        Duration::from_secs(5),
    )
    .expect("pairing connects")
}

#[test]
fn askable_command_prompts_and_proceeds_on_grant() {
    let (handler, path) =
        grant_paired_handler(vec![ActorCapability::Focus], vec![ActorCapability::Close]);
    *handler.request_grant_result.lock().unwrap() = Ok(true);
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = grant_client(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
    );

    client
        .command(Command::Close { id: WindowId(1) })
        .expect("granted command proceeds");
    let grant_calls = handler.grant_calls.lock().unwrap();
    assert_eq!(grant_calls.len(), 1);
    assert_eq!(grant_calls[0].1, "prin_g");
    assert_eq!(grant_calls[0].2, ActorCapability::Close);
    assert!(
        handler
            .commands
            .lock()
            .unwrap()
            .contains(&Command::Close { id: WindowId(1) })
    );
}

#[test]
fn askable_command_denied_stays_out_of_scope() {
    let (handler, path) =
        grant_paired_handler(vec![ActorCapability::Focus], vec![ActorCapability::Close]);
    *handler.request_grant_result.lock().unwrap() = Ok(false);
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = grant_client(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
    );

    let error = client
        .command(Command::Close { id: WindowId(1) })
        .expect_err("denied command is refused");
    assert!(error.to_string().contains("denied"));
    assert!(handler.commands.lock().unwrap().is_empty());
}

#[test]
fn recorded_grant_short_circuits_the_prompt() {
    let (handler, path) =
        grant_paired_handler(vec![ActorCapability::Focus], vec![ActorCapability::Close]);
    handler
        .grants
        .lock()
        .unwrap()
        .push(("prin_g".into(), ActorCapability::Close, true));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = grant_client(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
    );

    client
        .command(Command::Close { id: WindowId(1) })
        .expect("recorded grant proceeds");
    assert!(handler.grant_calls.lock().unwrap().is_empty());
}

#[test]
fn recorded_denial_refuses_without_prompting() {
    let (handler, path) =
        grant_paired_handler(vec![ActorCapability::Focus], vec![ActorCapability::Close]);
    handler
        .grants
        .lock()
        .unwrap()
        .push(("prin_g".into(), ActorCapability::Close, false));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = grant_client(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
    );

    let error = client
        .command(Command::Close { id: WindowId(1) })
        .expect_err("recorded denial refuses");
    assert!(error.to_string().contains("denied"));
    assert!(handler.grant_calls.lock().unwrap().is_empty());
}

#[test]
fn declared_scope_without_pairing_cannot_use_askable_operations() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    handler.scopes.lock().unwrap().insert(
        "ask-close".to_string(),
        Scope {
            ops: Some(vec![ActorCapability::Focus]),
            ask_ops: Some(vec![ActorCapability::Close]),
            ..Scope::default()
        },
    );
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let mut client = Client::connect_scoped(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
        "ask-close",
    )
    .expect("declared scope connects");
    client
        .command(Command::Focus {
            id: WindowId(1),
            reveal: true,
        })
        .expect("pregranted operations work without pairing");
    let error = client
        .command(Command::Close { id: WindowId(1) })
        .expect_err("askable operations require a paired agent");
    assert!(error.to_string().contains("paired agent"));
}

#[test]
fn interaction_domain_action_proceeds_through_the_grant_path() {
    let (handler, path) =
        grant_paired_handler(vec![], vec![ActorCapability::CreateInteractionDomain]);
    *handler.request_grant_result.lock().unwrap() = Ok(true);
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = grant_client(
        &path,
        ConnectionCapabilities {
            query: true,
            control: false,
            input: false,
            session: false,
            interaction_domain: true,
        },
    );

    let result = client
        .interaction_domain_action(InteractionDomainAction::Create {
            label: "agent".into(),
            capabilities: tessera_model::interaction_domain::SeatCapabilities::POINTER_KEYBOARD,
            output: None,
        })
        .expect("granted interaction_domain action");
    assert!(matches!(
        result,
        InteractionDomainActionResult::Created { .. }
    ));
    assert_eq!(handler.interaction_domain_actions.lock().unwrap().len(), 1);
}

#[test]
fn agent_management_round_trips() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    handler
        .principal_infos
        .lock()
        .unwrap()
        .push(tessera_ipc::AgentPrincipalInfo {
            principal: "prin_1".into(),
            label: Some("Codex".into()),
            pregranted: vec![ActorCapability::Focus],
            gated: vec![ActorCapability::Close],
            created_at: 1,
        });
    handler.grant_infos.lock().unwrap().extend([
        tessera_ipc::AgentGrantInfo {
            principal: "prin_1".into(),
            op: ActorCapability::Close,
            decision: tessera_ipc::AgentGrantDecision::Allow,
            granted_at: 2,
        },
        tessera_ipc::AgentGrantInfo {
            principal: "prin_2".into(),
            op: ActorCapability::Notify,
            decision: tessera_ipc::AgentGrantDecision::Deny,
            granted_at: 3,
        },
    ]);
    *handler.register_result.lock().unwrap() = Ok(("prin_9".into(), "cred_9".into()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_scoped(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
        tessera_ipc::LOCAL_AGENT_ADMIN_SCOPE,
    )
    .expect("connect");

    let principals = client.agent_principals().expect("principals");
    assert_eq!(principals.len(), 1);
    assert_eq!(principals[0].label.as_deref(), Some("Codex"));
    assert_eq!(principals[0].gated, vec![ActorCapability::Close]);

    assert_eq!(client.agent_grants(None).expect("grants").len(), 2);
    let filtered = client.agent_grants(Some("prin_1")).expect("filtered");
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].op, ActorCapability::Close);

    client
        .rename_agent_principal("prin_1", Some("New name"))
        .expect("rename");
    client.forget_agent_principal("prin_2").expect("forget");
    client
        .set_agent_ceiling(
            "prin_1",
            vec![ActorCapability::Focus, ActorCapability::Notify],
            vec![ActorCapability::Close],
        )
        .expect("ceiling");
    let (principal, credential) = client
        .register_agent(Some("Fleet"), vec![ActorCapability::Focus], vec![])
        .expect("register");
    assert_eq!(
        (principal.as_str(), credential.as_str()),
        ("prin_9", "cred_9")
    );
    client
        .revoke_agent_grant("prin_1", ActorCapability::Close)
        .expect("revoke");

    let log = handler.management_log.lock().unwrap();
    assert_eq!(log.len(), 5);
    assert!(log.iter().any(|entry| entry.starts_with("rename:prin_1:")));
    assert!(log.iter().any(|entry| entry == "forget:prin_2"));
    assert!(log.iter().any(|entry| entry == "ceiling:prin_1:2+1"));
    assert!(
        log.iter()
            .any(|entry| entry.starts_with("register:Some(\"Fleet\")"))
    );
    assert!(log.iter().any(|entry| entry.starts_with("revoke:prin_1:")));
}

#[test]
fn agent_management_requires_the_control_capability() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_scoped(
        &path,
        ConnectionCapabilities::QUERY,
        tessera_ipc::LOCAL_AGENT_ADMIN_SCOPE,
    )
    .expect("query connect");

    let error = client
        .forget_agent_principal("prin_1")
        .expect_err("query-only management is refused");
    assert!(error.to_string().contains("control capability"));
    assert!(handler.management_log.lock().unwrap().is_empty());
}

#[test]
fn lockdown_exempts_builtin_scope_connections() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    handler.lockdown_flag.store(true, Ordering::Relaxed);
    handler.scopes.lock().unwrap().insert(
        tessera_ipc::LOCAL_PORTAL_SCOPE.to_string(),
        Scope {
            ops: Some(vec![ActorCapability::Notify]),
            ..Scope::default()
        },
    );
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let privileged = ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    };
    let portal = Client::connect_scoped(&path, privileged, tessera_ipc::LOCAL_PORTAL_SCOPE)
        .expect("built-in scope connects under lockdown");
    assert!(
        portal.caps().control,
        "built-in platform components keep privileges under lockdown"
    );
}
