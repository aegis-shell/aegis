//! End-to-end coverage of the protocol-28 primitive families (ADR-0125):
//! Transact preflight/commit/conflict, Observe's per-class scope gating,
//! and scope-filtered agent subscription lanes.

use super::*;

fn paired_handler(
    pregranted: Vec<ActorCapability>,
) -> (Arc<TestHandler>, PathBuf) {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    *handler.pair_result.lock().unwrap() = Ok(aegis_ipc::PairedAgent {
        principal: aegis_security::authority::ActorPrincipal::new("prin_prim").unwrap(),
        credential: "cred_prim".into(),
        pregranted,
        gated: vec![],
    });
    (handler, path)
}

fn paired_client(path: &std::path::Path, caps: ConnectionCapabilities) -> Client {
    Client::connect_agent_with_timeout(
        path,
        caps,
        None,
        aegis_ipc::AgentHello {
            label: Some("primitives".into()),
            requested: vec![],
            credential: None,
        },
        Duration::from_secs(5),
    )
    .expect("pairing connects")
}

#[test]
fn transact_commits_and_returns_per_op_receipts() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_with(&path, ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    })
    .expect("connect");

    let result = client
        .transact(
            None,
            vec![
                aegis_ipc::TransactOp::Focus {
                    id: WindowId(1),
                    reveal: false,
                },
                aegis_ipc::TransactOp::Minimize { id: WindowId(2) },
            ],
        )
        .expect("transact");
    let aegis_ipc::TransactResult::Committed { receipt } = result else {
        panic!("expected commit, got {result:?}");
    };
    assert_eq!(receipt.before_seq, 0);
    assert_eq!(receipt.after_seq, 2);
    assert_eq!(
        receipt
            .results
            .iter()
            .map(|result| (result.seq, &result.effect))
            .collect::<Vec<_>>(),
        vec![
            (1, &aegis_ipc::Effect::Applied),
            (2, &aegis_ipc::Effect::Applied)
        ]
    );
    assert_eq!(
        handler.commands.lock().unwrap().as_slice(),
        [
            Command::Focus {
                id: WindowId(1),
                reveal: false
            },
            Command::Minimize { id: WindowId(2) }
        ]
    );
}

#[test]
fn transact_preflight_refuses_out_of_scope_batch_without_applying() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_scoped_with_timeout(
        &path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
        "focus-first",
        Duration::from_secs(5),
    )
    .expect("connect");

    let error = client
        .transact(
            None,
            vec![
                aegis_ipc::TransactOp::Focus {
                    id: WindowId(1),
                    reveal: true,
                },
                aegis_ipc::TransactOp::Close { id: WindowId(1) },
            ],
        )
        .expect_err("an out-of-scope op refuses the whole batch");
    assert!(error.to_string().contains("out of scope"), "{error}");
    assert!(
        handler.commands.lock().unwrap().is_empty(),
        "no op may apply when the batch refuses"
    );
    let refusals = handler.refusals.lock().unwrap();
    assert_eq!(refusals.len(), 1);
    assert!(
        matches!(
            &refusals[0].1,
            aegis_ipc::JournalMutation::Command {
                cmd: aegis_ipc::AuditedCommand::Close { .. }
            }
        ),
        "the refused op is the audited one: {:?}",
        refusals[0].1
    );
}

#[test]
fn transact_precondition_conflict_applies_nothing() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_with(&path, ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    })
    .expect("connect");

    let result = client
        .transact(None, vec![aegis_ipc::TransactOp::ToggleTiling])
        .expect("first transact");
    let aegis_ipc::TransactResult::Committed { receipt } = result else {
        panic!("expected commit, got {result:?}");
    };
    assert_eq!(receipt.after_seq, 1);

    let result = client
        .transact(
            Some(receipt.before_seq),
            vec![aegis_ipc::TransactOp::Minimize { id: WindowId(1) }],
        )
        .expect("conflicting transact");
    assert_eq!(
        result,
        aegis_ipc::TransactResult::PreconditionConflict {
            expected: 0,
            actual: 1
        }
    );
    assert_eq!(
        handler.commands.lock().unwrap().len(),
        1,
        "a conflicting batch applies nothing"
    );

    let result = client
        .transact(
            Some(receipt.after_seq),
            vec![aegis_ipc::TransactOp::Minimize { id: WindowId(1) }],
        )
        .expect("retry with the fresh cursor");
    assert!(
        matches!(result, aegis_ipc::TransactResult::Committed { .. }),
        "a retry at the fresh cursor commits: {result:?}"
    );
}

#[test]
fn transact_rejects_empty_and_invalid_batches() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_with(&path, ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    })
    .expect("connect");

    let error = client
        .transact(None, vec![])
        .expect_err("an empty batch is refused");
    assert!(error.to_string().contains("ops"), "{error}");
    let error = client
        .transact(
            None,
            vec![aegis_ipc::TransactOp::SetWindowGeometry {
                id: WindowId(1),
                rect: aegis_model::Rect::new(0, 0, 0, 10),
            }],
        )
        .expect_err("an invalid op is refused");
    assert!(!error.to_string().is_empty());
    assert!(handler.commands.lock().unwrap().is_empty());
}

#[test]
fn observe_returns_all_classes_for_anonymous_owner_clients() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect(&path).expect("connect");

    let snapshot = client.observe().expect("observe");
    assert_eq!(snapshot.windows.expect("windows").len(), 2);
    assert!(snapshot.workspaces.is_some());
    assert!(snapshot.outputs.is_some());
    assert!(snapshot.notifications.is_some());
    assert!(snapshot.interaction_domains.is_some());
    let cursor = snapshot.journal_cursor.expect("journal cursor");
    assert_eq!(cursor.latest_seq, 0);
}

#[test]
fn observe_gates_each_class_by_agent_scope() {
    let (handler, path) = paired_handler(vec![ActorCapability::ObserveWindows]);
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = paired_client(&path, ConnectionCapabilities::QUERY);

    let snapshot = client.observe().expect("observe");
    assert_eq!(snapshot.windows.expect("windows").len(), 2);
    assert!(snapshot.workspaces.is_none());
    assert!(snapshot.outputs.is_none());
    assert!(snapshot.notifications.is_none());
    assert!(snapshot.interaction_domains.is_none());
    assert!(snapshot.journal_cursor.is_none());
}

#[test]
fn agent_coarse_subscription_is_gated_by_observation_scope() {
    let (handler, path) = paired_handler(vec![ActorCapability::ObserveWindows]);
    let server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = paired_client(&path, ConnectionCapabilities::QUERY);
    client.subscribe().expect("subscribe");

    server.broadcast(Event::WorkspaceChanged);
    server.broadcast(Event::InteractionDomainsChanged { revision: 9 });
    server.broadcast(Event::WindowsChanged);

    client.set_io_timeout(Some(Duration::from_millis(500))).unwrap();
    match client.next_event().expect("one permitted event") {
        Event::WindowsChanged => {}
        other => panic!("expected WindowsChanged, got {other:?}"),
    }
    let error = client
        .next_event()
        .expect_err("scope-refused events are not delivered");
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock, "{error}");
}

#[test]
fn agent_journal_subscription_filters_by_subject_and_scope() {
    let (handler, path) = paired_handler(vec![
        ActorCapability::ObserveJournal,
        ActorCapability::Focus,
    ]);
    let server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = paired_client(&path, ConnectionCapabilities::QUERY);
    client.subscribe_journal().expect("subscribe journal");

    let focus = aegis_ipc::JournalMutation::Command {
        cmd: aegis_ipc::AuditedCommand::from(&Command::Focus {
            id: WindowId(1),
            reveal: true,
        }),
    };
    let close = aegis_ipc::JournalMutation::Command {
        cmd: aegis_ipc::AuditedCommand::from(&Command::Close { id: WindowId(1) }),
    };
    // Another principal's mutation is never visible to this agent.
    server.broadcast_journal(aegis_ipc::JournalEntry {
        seq: 1,
        ts_mono_ms: 1,
        origin: aegis_ipc::Origin::Actor {
            conn_id: 99,
            principal: "prin_other".into(),
        },
        mutation: focus.clone(),
        effect: aegis_ipc::Effect::Applied,
    });
    // A resource-permitted entry from a non-Actor origin passes through,
    // matching `GetJournal`'s filter exactly.
    server.broadcast_journal(aegis_ipc::JournalEntry {
        seq: 2,
        ts_mono_ms: 2,
        origin: aegis_ipc::Origin::Ipc { conn_id: 3 },
        mutation: close,
        effect: aegis_ipc::Effect::Applied,
    });
    // The agent's own permitted mutation is delivered.
    server.broadcast_journal(aegis_ipc::JournalEntry {
        seq: 3,
        ts_mono_ms: 3,
        origin: aegis_ipc::Origin::Actor {
            conn_id: 1,
            principal: "prin_prim".into(),
        },
        mutation: focus,
        effect: aegis_ipc::Effect::Applied,
    });

    client.set_io_timeout(Some(Duration::from_millis(500))).unwrap();
    // The other principal's entry was filtered; the resource-permitted
    // entries arrive in order, matching `GetJournal`'s filter exactly.
    for expected_seq in [2, 3] {
        match client.next_event().expect("permitted entry") {
            Event::Journal { entry } => assert_eq!(entry.seq, expected_seq),
            other => panic!("expected a journal event, got {other:?}"),
        }
    }
    let error = client
        .next_event()
        .expect_err("filtered entries are not delivered");
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock, "{error}");
}

#[test]
fn agent_subscriptions_observe_scope_narrowing_live() {
    let (handler, path) = paired_handler(vec![ActorCapability::ObserveWindows]);
    let server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = paired_client(&path, ConnectionCapabilities::QUERY);
    client.subscribe().expect("subscribe");

    server.broadcast(Event::WindowsChanged);
    client.set_io_timeout(Some(Duration::from_millis(500))).unwrap();
    assert!(matches!(
        client.next_event().expect("event while permitted"),
        Event::WindowsChanged
    ));

    // The principal registry narrowing mid-stream (a forgotten agent fails
    // closed) stops delivery without dropping the lane.
    *handler.refresh_result.lock().unwrap() = Err("forgotten principal".into());
    server.broadcast(Event::WindowsChanged);
    let error = client
        .next_event()
        .expect_err("a forgotten principal stops delivery");
    assert_eq!(error.kind(), std::io::ErrorKind::WouldBlock, "{error}");
}
