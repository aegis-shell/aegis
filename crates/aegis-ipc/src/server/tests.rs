use super::*;

fn frame(stream_id: u64) -> StreamFramePayload {
    StreamFramePayload {
        stream_id,
        sequence: 1,
        width: 2,
        height: 2,
        stride: 8,
        format: StreamPixelFormat::Bgra8,
        damage: vec![aegis_core::Rect::new(0, 0, 2, 2)],
        dropped: 0,
        pixels: Arc::from(&[7u8; 16][..]),
    }
}

/// A `Server` without a listener: the lane bookkeeping under test lives
/// behind `push_stream_frame`, which never touches the accept thread.
fn bare_server() -> Server {
    Server {
        _accept: thread::spawn(|| {}),
        socket: PathBuf::new(),
        subs: Arc::new(Mutex::new(HashMap::new())),
        journal_broadcaster: JournalBroadcaster::default(),
        streams: Arc::new(Mutex::new(HashMap::new())),
    }
}

fn add_lane(server: &Server, stream_id: u64, tx: SyncSender<Outbound>) {
    server.streams.lock().unwrap().insert(
        stream_id,
        StreamLane {
            conn_id: 1,
            tx,
            scope: LiveScopeBinding {
                connection_id: 1,
                session: aegis_authority::ActorSessionId(1),
                name: None,
                principal: None,
                fallback: Scope::unscoped(),
            },
            target: crate::schema::StreamTarget::Output,
            lease_deadline: Arc::new(Mutex::new(std::time::Instant::now())),
            queued: Arc::new(AtomicU32::new(0)),
        },
    );
}

#[test]
fn stream_lane_bounds_queued_frames_at_lane_depth() {
    let server = bare_server();
    // Nobody drains the receiver, so the writer never decrements
    // `queued`: the lane fills exactly at STREAM_LANE_DEPTH and further
    // pushes drop instead of queueing (ADR-0052 backpressure).
    let (tx, _rx) = mpsc::sync_channel(OUTBOUND_QUEUE_DEPTH);
    add_lane(&server, 1, tx);
    for _ in 0..STREAM_LANE_DEPTH {
        assert!(server.push_stream_frame(frame(1)));
    }
    assert!(!server.push_stream_frame(frame(1)));
    // Unknown streams refuse without queueing.
    assert!(!server.push_stream_frame(frame(99)));
}

#[test]
fn stream_lane_refills_as_the_writer_drains() {
    let server = bare_server();
    let (tx, rx) = mpsc::sync_channel(OUTBOUND_QUEUE_DEPTH);
    add_lane(&server, 1, tx);
    for _ in 0..STREAM_LANE_DEPTH {
        assert!(server.push_stream_frame(frame(1)));
    }
    // Simulate the writer consuming and decrementing.
    let mut drained = 0;
    while let Ok(Outbound::StreamFrame { queued, .. }) = rx.try_recv() {
        queued.fetch_sub(1, Ordering::AcqRel);
        drained += 1;
    }
    assert_eq!(drained, STREAM_LANE_DEPTH);
    assert!(server.push_stream_frame(frame(1)));
}

#[test]
fn slow_event_and_journal_subscribers_are_evicted_without_blocking() {
    let server = bare_server();
    let (event_tx, _event_rx) = mpsc::sync_channel(OUTBOUND_QUEUE_DEPTH);
    for _ in 0..OUTBOUND_QUEUE_DEPTH {
        event_tx
            .try_send(Outbound::Event(Event::WindowsChanged))
            .unwrap();
    }
    server.subs.lock().unwrap().insert(
        1,
        SubscriptionLane {
            tx: event_tx,
            shutdown: None,
        },
    );
    server.broadcast(Event::WorkspaceChanged);
    assert!(server.subs.lock().unwrap().is_empty());

    let (journal_tx, _journal_rx) = mpsc::sync_channel(OUTBOUND_QUEUE_DEPTH);
    for _ in 0..OUTBOUND_QUEUE_DEPTH {
        journal_tx
            .try_send(Outbound::Event(Event::WindowsChanged))
            .unwrap();
    }
    server
        .journal_broadcaster
        .subscribers
        .lock()
        .unwrap()
        .insert(
            2,
            SubscriptionLane {
                tx: journal_tx,
                shutdown: None,
            },
        );
    server
        .journal_broadcaster
        .broadcast(crate::journal::JournalEntry {
            seq: 1,
            ts_mono_ms: 1,
            origin: crate::journal::Origin::Internal,
            mutation: crate::journal::JournalMutation::CapabilityUse {
                session: aegis_authority::ActorSessionId(1),
                principal: None,
                capability: ActorCapability::IdleInhibit,
                action: crate::journal::CapabilityUseAction::Enable,
            },
            effect: crate::journal::Effect::Applied,
        });
    assert!(
        server
            .journal_broadcaster
            .subscribers
            .lock()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn connection_permits_are_bounded_and_released_on_drop() {
    let active = Arc::new(AtomicU32::new(MAX_CONNECTIONS - 1));
    let permit = ConnectionPermit::acquire(&active).expect("last permit");
    assert_eq!(active.load(Ordering::Acquire), MAX_CONNECTIONS);
    assert!(ConnectionPermit::acquire(&active).is_none());
    drop(permit);
    assert_eq!(active.load(Ordering::Acquire), MAX_CONNECTIONS - 1);
    assert!(ConnectionPermit::acquire(&active).is_some());
}

#[test]
fn full_subscription_lane_shuts_down_its_connection() {
    use std::io::Read as _;

    let (server, mut peer) = UnixStream::pair().unwrap();
    peer.set_read_timeout(Some(std::time::Duration::from_secs(1)))
        .unwrap();
    let (tx, _rx) = mpsc::sync_channel(1);
    tx.try_send(Outbound::Event(Event::WindowsChanged)).unwrap();
    let lane = SubscriptionLane {
        tx,
        shutdown: Some(Arc::new(server)),
    };
    assert!(!lane.try_send(Outbound::Event(Event::WorkspaceChanged)));
    let mut byte = [0u8; 1];
    assert_eq!(peer.read(&mut byte).unwrap(), 0);
}

#[test]
fn end_stream_unregisters_and_notifies_the_client() {
    let server = bare_server();
    let (tx, rx) = mpsc::sync_channel(OUTBOUND_QUEUE_DEPTH);
    add_lane(&server, 7, tx);
    server.end_stream(7, "output geometry changed");
    assert!(!server.push_stream_frame(frame(7)));
    match rx.recv().unwrap() {
        Outbound::Event(Event::StreamEnded { stream_id, reason }) => {
            assert_eq!(stream_id, 7);
            assert_eq!(reason, "output geometry changed");
        }
        other => panic!("expected StreamEnded, got {other:?}"),
    }
    // Ending an unknown stream is a no-op.
    server.end_stream(7, "again");
    assert!(rx.try_recv().is_err());
}

#[test]
fn authenticated_subject_cannot_cross_agent_interaction_domain_ownership() {
    let mut model = aegis_core::interaction_domain::InteractionDomainModel::new();
    let own = model.create_agent_interaction_domain_for_subject(
        "own",
        aegis_core::interaction_domain::SeatCapabilities::POINTER_KEYBOARD,
        Some("prin_a".into()),
    );
    let other = model.create_agent_interaction_domain_for_subject(
        "other",
        aegis_core::interaction_domain::SeatCapabilities::POINTER_KEYBOARD,
        Some("prin_b".into()),
    );
    let snapshot = model.snapshot();

    let own_revoke = InteractionDomainAction::Revoke {
        interaction_domain: own.interaction_domain,
        fallback: aegis_core::interaction_domain::HUMAN_INTERACTION_DOMAIN,
        expected_revision: None,
    };
    assert!(authorize_subject_interaction_domain_action("prin_a", &own_revoke, &snapshot).is_ok());

    let other_revoke = InteractionDomainAction::Revoke {
        interaction_domain: other.interaction_domain,
        fallback: aegis_core::interaction_domain::HUMAN_INTERACTION_DOMAIN,
        expected_revision: None,
    };
    assert!(
        authorize_subject_interaction_domain_action("prin_a", &other_revoke, &snapshot).is_err()
    );
    assert!(subject_owns_interaction_domain(
        &snapshot,
        "prin_b",
        other.interaction_domain
    ));
    assert!(!subject_owns_interaction_domain(
        &snapshot,
        "prin_a",
        other.interaction_domain
    ));
}
