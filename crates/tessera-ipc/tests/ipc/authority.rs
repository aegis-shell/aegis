use super::*;

#[test]
fn named_scope_is_reported_and_enforced() {
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

    let mut client = Client::connect_scoped(&path, requested, "focus-first").expect("connect");
    assert_eq!(client.scope().windows, Some(vec![WindowId(1)]));
    assert_eq!(client.scope().ops, Some(vec![ActorCapability::Focus]));
    client
        .command(Command::Focus {
            id: WindowId(1),
            reveal: true,
        })
        .expect("allowed focus");
    let wrong_window = client
        .command(Command::Focus {
            id: WindowId(2),
            reveal: true,
        })
        .unwrap_err();
    assert!(wrong_window.to_string().contains("out of scope"));
    let wrong_operation = client
        .command(Command::Close { id: WindowId(1) })
        .unwrap_err();
    assert!(wrong_operation.to_string().contains("out of scope"));
}

#[test]
fn synthetic_input_requires_a_named_scope_and_separate_capability() {
    use tessera_model::input::SyntheticInputAction;

    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let requested = ConnectionCapabilities {
        query: true,
        control: false,
        input: true,
        session: false,
        interaction_domain: false,
    };

    let mut unscoped = Client::connect_with(&path, requested).expect("unscoped connect");
    assert!(!unscoped.caps().input, "unscoped input must fail closed");
    let action = SyntheticInputAction::Click {
        position: tessera_model::Point { x: 10, y: 20 },
        button: 0x110,
    };
    let err = unscoped
        .inject_input(WindowId(1), vec![action])
        .unwrap_err();
    assert!(err.to_string().contains("capability not granted"), "{err}");

    let mut scoped = Client::connect_scoped(&path, requested, "input-first").expect("scoped");
    assert!(scoped.caps().input);
    scoped
        .inject_input(WindowId(1), vec![action])
        .expect("scoped input accepted");
    assert!(
        handler
            .commands
            .lock()
            .unwrap()
            .contains(&Command::InjectInput {
                id: WindowId(1),
                actions: vec![action],
            })
    );

    let before = handler.commands.lock().unwrap().len();
    let err = scoped.inject_input(WindowId(1), vec![]).unwrap_err();
    assert!(err.to_string().contains("action count"), "{err}");
    assert_eq!(handler.commands.lock().unwrap().len(), before);
}

#[test]
fn capture_output_requires_control_and_an_explicit_scope_op() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    // Scoped with the explicit CaptureOutput op + control: succeeds and the
    // payload round-trips through a sealed descriptor.
    let requested = ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    };
    let mut scoped = Client::connect_scoped(&path, requested, "capture").expect("scoped connect");
    let (w, h, png) = scoped.capture_output().expect("capture succeeds");
    assert_eq!((w, h), (2, 1));
    assert_eq!(png, vec![1u8, 2, 3, 4, 5]);

    // A scope without the op is refused even though it has control.
    let mut focus_scope =
        Client::connect_scoped(&path, requested, "focus-first").expect("scoped connect");
    let err = focus_scope.capture_output().unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Unscoped connections never inherit the op (fail-closed, like input).
    let mut unscoped = Client::connect_with(&path, requested).expect("unscoped connect");
    let err = unscoped.capture_output().unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Without the control capability the request is refused earlier.
    let mut query_only = Client::connect(&path).expect("query connect");
    let err = query_only.capture_output().unwrap_err();
    assert!(err.to_string().contains("control capability"), "{err}");
}

fn stream_frame(stream_id: u64, sequence: u64) -> tessera_ipc::StreamFramePayload {
    tessera_ipc::StreamFramePayload::Pixels(tessera_ipc::StreamPixelFrame {
        stream_id,
        sequence,
        width: 2,
        height: 2,
        stride: 8,
        format: tessera_ipc::StreamPixelFormat::Bgra8,
        damage: vec![tessera_model::Rect::new(0, 0, 2, 2)],
        dropped: 0,
        pixels: Arc::from(&[7u8; 16][..]),
    })
}

#[test]
fn stream_output_start_requires_control_and_an_explicit_scope_op() {
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
    let started = scoped.start_output_stream(Some(30)).expect("stream starts");
    assert_eq!(started.stream_id, 1);
    assert_eq!((started.width, started.height), (2, 2));
    assert_eq!(started.format, tessera_ipc::StreamPixelFormat::Bgra8);

    // A scope without the op is refused even though it has control.
    let mut focus_scope =
        Client::connect_scoped(&path, requested, "focus-first").expect("scoped connect");
    let err = focus_scope.start_output_stream(None).unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Unscoped connections never inherit the op.
    let mut unscoped = Client::connect_with(&path, requested).expect("unscoped connect");
    let err = unscoped.start_output_stream(None).unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Without the control capability the request is refused earlier.
    let mut query_only = Client::connect(&path).expect("query connect");
    let err = query_only.start_output_stream(None).unwrap_err();
    assert!(err.to_string().contains("control capability"), "{err}");
}

#[test]
fn set_idle_inhibit_requires_control_and_an_explicit_scope_op() {
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
    let mut scoped = Client::connect_scoped(&path, requested, "idle").expect("scoped connect");
    assert!(scoped.set_idle_inhibit(true).expect("inhibit set"));
    assert!(!scoped.set_idle_inhibit(false).expect("inhibit cleared"));
    assert_eq!(handler.idle_inhibits.lock().unwrap().len(), 2);

    // A scope without the op is refused even though it has control.
    let mut focus_scope =
        Client::connect_scoped(&path, requested, "focus-first").expect("scoped connect");
    let err = focus_scope.set_idle_inhibit(true).unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Unscoped connections never inherit the op (fail-closed, like input).
    let mut unscoped = Client::connect_with(&path, requested).expect("unscoped connect");
    let err = unscoped.set_idle_inhibit(true).unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Without the control capability the request is refused earlier.
    let mut query_only = Client::connect(&path).expect("query connect");
    let err = query_only.set_idle_inhibit(true).unwrap_err();
    assert!(err.to_string().contains("control capability"), "{err}");
}

#[test]
fn pick_target_requires_control_and_an_explicit_scope_op() {
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
    // Scoped with the explicit PickTarget op + control: the picked region
    // round-trips.
    let mut scoped = Client::connect_scoped(&path, requested, "pick").expect("scoped connect");
    let result = scoped
        .pick_target(tessera_ipc::PickKind::Region)
        .expect("pick succeeds");
    assert_eq!(
        result,
        tessera_ipc::PickResult::Region {
            rect: tessera_model::Rect::new(1, 2, 30, 40)
        }
    );
    assert_eq!(
        handler.picks.lock().unwrap().as_slice(),
        &[(1, tessera_ipc::PickKind::Region)]
    );

    // A scope without the op is refused even though it has control.
    let mut focus_scope =
        Client::connect_scoped(&path, requested, "focus-first").expect("scoped connect");
    let err = focus_scope
        .pick_target(tessera_ipc::PickKind::Pixel)
        .unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Unscoped connections never inherit the op (fail-closed, like input).
    let mut unscoped = Client::connect_with(&path, requested).expect("unscoped connect");
    let err = unscoped
        .pick_target(tessera_ipc::PickKind::Window)
        .unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Without the control capability the request is refused earlier.
    let mut query_only = Client::connect(&path).expect("query connect");
    let err = query_only
        .pick_target(tessera_ipc::PickKind::Region)
        .unwrap_err();
    assert!(err.to_string().contains("control capability"), "{err}");

    // A locked/inactive session refuses before any chrome opens.
    handler
        .capture_security_active
        .store(false, Ordering::Release);
    let mut scoped = Client::connect_scoped(&path, requested, "pick").expect("scoped connect");
    let err = scoped.pick_target(tessera_ipc::PickKind::Region).unwrap_err();
    assert!(err.to_string().contains("locked or inactive"), "{err}");
}

#[test]
fn stream_output_start_forwards_a_window_target() {
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
        .start_output_stream_target(
            Some(30),
            tessera_ipc::StreamTarget::Window {
                window: WindowId(2),
            },
        )
        .expect("window stream starts");
    assert_eq!(
        handler.stream_targets.lock().unwrap().as_slice(),
        &[tessera_ipc::StreamTarget::Window {
            window: WindowId(2)
        }]
    );

    // The default target stays the whole output.
    let mut scoped = Client::connect_scoped(&path, requested, "stream").expect("scoped connect");
    scoped
        .start_output_stream(None)
        .expect("output stream starts");
    assert_eq!(
        handler.stream_targets.lock().unwrap().as_slice(),
        &[
            tessera_ipc::StreamTarget::Window {
                window: WindowId(2)
            },
            tessera_ipc::StreamTarget::Output { output: None },
        ]
    );

    let mut restricted =
        Client::connect_scoped(&path, requested, "stream-first").expect("restricted connect");
    restricted
        .start_output_stream_target(
            Some(30),
            tessera_ipc::StreamTarget::Window {
                window: WindowId(1),
            },
        )
        .expect("allowlisted window stream starts");
    let error = restricted
        .start_output_stream_target(
            Some(30),
            tessera_ipc::StreamTarget::Window {
                window: WindowId(2),
            },
        )
        .expect_err("window stream must obey the live resource allowlist");
    assert!(error.to_string().contains("out of scope"), "{error}");
}

#[test]
fn disconnecting_releases_a_held_idle_inhibitor() {
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
    let mut client = Client::connect_scoped(&path, requested, "idle").expect("connect");
    assert!(client.set_idle_inhibit(true).expect("inhibit set"));
    drop(client);

    // The connection thread reaps asynchronously; poll briefly.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while handler.idle_disconnects.lock().unwrap().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "idle inhibitor never released"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn stream_frames_flow_and_stop_cleans_up() {
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

    // A pushed frame arrives as metadata + sealed pixel memfd.
    assert!(server.push_stream_frame(stream_frame(started.stream_id, 1)));
    let message = client.next_stream_message().expect("frame arrives");
    let tessera_ipc::StreamMessage::Frame(frame) = message else {
        panic!("expected a frame, got {message:?}");
    };
    assert_eq!((frame.stream_id, frame.sequence), (started.stream_id, 1));
    assert_eq!((frame.width, frame.height, frame.stride), (2, 2, 8));
    assert_eq!(frame.pixels, vec![7u8; 16]);

    // Pushing to an unknown stream is refused without queueing.
    assert!(!server.push_stream_frame(stream_frame(99, 1)));

    // Stop unregisters the lane; a second stop errors and further pushes
    // are refused.
    client
        .stop_output_stream(started.stream_id)
        .expect("stop succeeds");
    assert_eq!(
        handler.stream_stops.lock().unwrap().as_slice(),
        &[started.stream_id]
    );
    let err = client.stop_output_stream(started.stream_id).unwrap_err();
    assert!(err.to_string().contains("unknown stream"), "{err}");
    assert!(!server.push_stream_frame(stream_frame(started.stream_id, 2)));
}

#[test]
fn stream_ends_when_the_scope_is_revoked_mid_stream() {
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

    // Revoke the scope: the writer's per-frame re-check ends the stream
    // instead of attaching pixels (ADR-0052).
    handler.scopes.lock().unwrap().remove("stream");
    assert!(server.push_stream_frame(stream_frame(started.stream_id, 1)));
    let message = client.next_stream_message().expect("stream end arrives");
    let tessera_ipc::StreamMessage::Ended { stream_id, reason } = message else {
        panic!("expected StreamEnded, got {message:?}");
    };
    assert_eq!(stream_id, started.stream_id);
    assert!(reason.contains("scope"), "{reason}");
    assert_eq!(
        handler.stream_stops.lock().unwrap().as_slice(),
        &[started.stream_id]
    );
}

#[test]
fn stream_lease_renewal_surfaces_through_the_stream_reader() {
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
    let mut client = Client::connect_scoped(&path, requested, "stream").expect("connect");
    client.start_output_stream(None).expect("stream starts");
    client.request_lease_renewal(900_000).expect("renewal sent");
    let message = client.next_stream_message().expect("renewal reply");
    assert_eq!(message, tessera_ipc::StreamMessage::LeaseRenewed);
}

#[test]
fn disconnecting_stops_owned_streams() {
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
    let mut client = Client::connect_scoped(&path, requested, "stream").expect("connect");
    client.start_output_stream(None).expect("stream starts");
    drop(client);

    // The connection thread reaps asynchronously; poll briefly.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    while handler.stream_disconnects.lock().unwrap().is_empty() {
        assert!(
            std::time::Instant::now() < deadline,
            "disconnect never reaped"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn interaction_domain_lifecycle_capture_and_lease_are_scoped_and_synchronous() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let requested = ConnectionCapabilities {
        query: true,
        control: false,
        input: false,
        session: false,
        interaction_domain: true,
    };
    let mut client =
        Client::connect_scoped(&path, requested, "interaction_domain").expect("connect");
    assert!(client.caps().interaction_domain);
    let original_lease = client.lease().expect("privileged connection has lease");
    let renewed = client.renew_lease(30_000).expect("renew lease");
    assert_eq!(renewed.id, original_lease.id);
    assert_eq!(renewed.ttl_ms, 30_000);

    let snapshot = client
        .interaction_domains()
        .expect("interaction_domain snapshot");
    assert_eq!(snapshot.revision, 4);
    let result = client
        .interaction_domain_action(InteractionDomainAction::Create {
            label: "test agent".into(),
            capabilities: tessera_model::interaction_domain::SeatCapabilities::POINTER_KEYBOARD,
            output: Some(tessera_model::interaction_domain::VirtualOutput::DEFAULT_AGENT),
        })
        .expect("create interaction_domain");
    assert!(matches!(
        result,
        InteractionDomainActionResult::Created {
            bundle: tessera_model::interaction_domain::InteractionDomainBundle {
                interaction_domain: tessera_model::interaction_domain::InteractionDomainId(2),
                ..
            },
            ..
        }
    ));
    let capture = client
        .capture_interaction_domain(
            tessera_model::interaction_domain::InteractionDomainId(2),
            None,
        )
        .expect("interaction_domain capture");
    assert_eq!((capture.width, capture.height, capture.revision), (2, 1, 4));
    assert_eq!(capture.scale_milli, 1250);
    assert_eq!(capture.region, tessera_model::Rect::new(0, 0, 2, 1));
    assert_eq!(
        capture.placements,
        vec![
            tessera_model::interaction_domain::InteractionDomainWindowPlacement {
                window: WindowId(1),
                output_rect: tessera_model::Rect::new(0, 0, 2, 1),
                surface_size: tessera_model::Size { w: 20, h: 10 },
            }
        ]
    );
    assert_eq!(capture.png, vec![9, 8, 7]);
    assert_eq!(handler.interaction_domain_actions.lock().unwrap().len(), 1);
}

#[test]
fn interaction_domain_actions_require_an_observation_and_return_commit_receipts() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let requested = ConnectionCapabilities {
        query: true,
        control: false,
        input: true,
        session: false,
        interaction_domain: true,
    };
    let mut client =
        Client::connect_scoped(&path, requested, "interaction_domain").expect("connect");
    let interaction_domain = tessera_model::interaction_domain::InteractionDomainId(2);
    let observation = client
        .observe_interaction_domain(interaction_domain)
        .expect("semantic observation");
    let target = tessera_model::semantic::SemanticObjectId::for_window(WindowId(1));
    assert_eq!(
        observation.snapshot.object(target).unwrap().name.as_deref(),
        Some("first")
    );

    let receipt = client
        .inject_interaction_domain_input(
            interaction_domain,
            target,
            observation.token,
            vec![tessera_model::input::SyntheticInputAction::Click {
                button: 0x110,
                position: tessera_model::Point { x: 5, y: 7 },
            }],
        )
        .expect("observation-bound action commits synchronously");
    assert_eq!(receipt.action_id, 12);
    assert_eq!(receipt.actions_applied, 1);

    assert!(handler.commands.lock().unwrap().is_empty());
}

#[test]
fn semantic_observation_needs_query_not_interaction_domain_action_authority() {
    let path = scratch();
    let handler = Arc::new(TestHandler::query(sample_windows()));
    let _server = Server::start(&path, handler).expect("bind");
    let requested = ConnectionCapabilities {
        query: true,
        control: false,
        input: false,
        session: false,
        interaction_domain: false,
    };
    let mut client =
        Client::connect_scoped(&path, requested, "interaction_domain").expect("connect");
    assert!(!client.caps().interaction_domain);
    assert!(client.lease().is_none());
    let observation = client
        .observe_interaction_domain(tessera_model::interaction_domain::InteractionDomainId(2))
        .expect("semantic observation is independently query-scoped");
    assert_eq!(
        observation.snapshot.interaction_domain,
        tessera_model::interaction_domain::InteractionDomainId(2)
    );
}

#[test]
fn expired_privileged_lease_fails_closed_without_losing_query_access() {
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
    let mut client = Client::connect_scoped(&path, requested, "focus-first").expect("connect");
    client.renew_lease(1_000).expect("short lease");
    std::thread::sleep(std::time::Duration::from_millis(1_100));

    assert_eq!(
        client.windows().expect("query survives lease expiry").len(),
        1,
        "query survives lease expiry but remains resource-scoped"
    );
    let error = client
        .command(Command::Focus {
            id: WindowId(1),
            reveal: true,
        })
        .expect_err("expired lease must reject control");
    assert!(error.to_string().contains("lease expired"), "{error}");
    assert!(handler.commands.lock().unwrap().is_empty());
}

#[test]
fn lease_must_remain_live_until_capture_delivery() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    handler.capture_delay_ms.store(1_100, Ordering::Relaxed);
    let _server = Server::start(&path, handler).expect("bind");
    let requested = ConnectionCapabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        interaction_domain: false,
    };
    let mut client = Client::connect_scoped(&path, requested, "capture").expect("connect");
    client.renew_lease(1_000).expect("short lease");
    let error = client
        .capture_output()
        .expect_err("expired in-flight capture must not deliver pixels");
    assert!(error.to_string().contains("lease expired"), "{error}");
}

#[test]
fn interaction_domain_operations_fail_closed_without_named_scope() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, handler).expect("bind");
    let requested = ConnectionCapabilities {
        query: true,
        control: false,
        input: false,
        session: false,
        interaction_domain: true,
    };
    let mut client = Client::connect_with(&path, requested).expect("connect");
    let error = client
        .interaction_domain_action(InteractionDomainAction::Create {
            label: "denied".into(),
            capabilities: tessera_model::interaction_domain::SeatCapabilities::POINTER_KEYBOARD,
            output: None,
        })
        .unwrap_err();
    assert!(error.to_string().contains("out of scope"), "{error}");
}

#[test]
fn unknown_named_scope_is_refused_at_handshake() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, handler).expect("bind");

    let err = Client::connect_scoped(&path, ConnectionCapabilities::QUERY, "missing")
        .err()
        .expect("unknown scope must fail");
    assert!(err.to_string().contains("unknown scope 'missing'"), "{err}");
}

#[test]
fn scoped_timeout_connection_completes_handshake() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, handler).expect("bind");

    let client = Client::connect_scoped_with_timeout(
        &path,
        ConnectionCapabilities::QUERY,
        "focus-first",
        Duration::from_secs(1),
    )
    .expect("connect with timeout");

    assert!(client.caps().query);
    assert_eq!(client.scope().windows, Some(vec![WindowId(1)]));
}

#[test]
fn unscoped_timeout_connection_completes_handshake() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, handler).expect("bind");

    let client =
        Client::connect_with_timeout(&path, ConnectionCapabilities::QUERY, Duration::from_secs(1))
            .expect("connect with timeout");

    assert!(client.caps().query);
    assert_eq!(client.scope(), &Scope::unscoped());
}

#[test]
fn scope_revocation_applies_to_existing_connections() {
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
    let mut client = Client::connect_scoped(&path, requested, "focus-first").expect("connect");
    client
        .command(Command::Focus {
            id: WindowId(1),
            reveal: true,
        })
        .expect("allowed before revoke");

    handler.scopes.lock().unwrap().clear();
    let err = client
        .command(Command::Focus {
            id: WindowId(1),
            reveal: true,
        })
        .unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");
}

#[test]
fn scope_revocation_stops_existing_interaction_domain_and_capture_connections() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let requested = ConnectionCapabilities {
        query: true,
        control: false,
        input: false,
        session: false,
        interaction_domain: true,
    };
    let mut client =
        Client::connect_scoped(&path, requested, "interaction_domain").expect("connect");
    handler.scopes.lock().unwrap().clear();

    let action_error = client
        .interaction_domain_action(InteractionDomainAction::Create {
            label: "revoked".into(),
            capabilities: tessera_model::interaction_domain::SeatCapabilities::POINTER_KEYBOARD,
            output: None,
        })
        .expect_err("removed scope must revoke InteractionDomain mutation");
    assert!(
        action_error.to_string().contains("out of scope"),
        "{action_error}"
    );
    let capture_error = client
        .capture_interaction_domain(
            tessera_model::interaction_domain::InteractionDomainId(2),
            None,
        )
        .expect_err("removed scope must revoke InteractionDomain capture");
    assert!(
        capture_error.to_string().contains("out of scope"),
        "{capture_error}"
    );
    assert!(
        handler
            .interaction_domain_actions
            .lock()
            .unwrap()
            .is_empty()
    );
}
