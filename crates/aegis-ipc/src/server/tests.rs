use super::*;

fn frame(stream_id: u64) -> StreamFramePayload {
    StreamFramePayload::Pixels(StreamPixelFrame {
        stream_id,
        sequence: 1,
        width: 2,
        height: 2,
        stride: 8,
        format: StreamPixelFormat::Bgra8,
        damage: vec![aegis_model::Rect::new(0, 0, 2, 2)],
        dropped: 0,
        pixels: Arc::from(&[7u8; 16][..]),
    })
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
                session: aegis_security::authority::ActorSessionId(1),
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
                session: aegis_security::authority::ActorSessionId(1),
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
    let mut model = aegis_model::interaction_domain::InteractionDomainModel::new();
    let own = model.create_agent_interaction_domain_for_subject(
        "own",
        aegis_model::interaction_domain::SeatCapabilities::POINTER_KEYBOARD,
        Some("prin_a".into()),
    );
    let other = model.create_agent_interaction_domain_for_subject(
        "other",
        aegis_model::interaction_domain::SeatCapabilities::POINTER_KEYBOARD,
        Some("prin_b".into()),
    );
    let snapshot = model.snapshot();

    let own_revoke = InteractionDomainAction::Revoke {
        interaction_domain: own.interaction_domain,
        fallback: aegis_model::interaction_domain::HUMAN_INTERACTION_DOMAIN,
        expected_revision: None,
    };
    assert!(authorize_subject_interaction_domain_action("prin_a", &own_revoke, &snapshot).is_ok());

    let other_revoke = InteractionDomainAction::Revoke {
        interaction_domain: other.interaction_domain,
        fallback: aegis_model::interaction_domain::HUMAN_INTERACTION_DOMAIN,
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

/// Minimal handler for the stream dispatch paths: always permits the
/// "stream" scope, records the dmabuf opt-in gate and slot releases, and
/// answers a dmabuf start with a real three-slot descriptor table.
struct StreamHandler {
    starts: Mutex<Vec<bool>>,
    releases: Mutex<Vec<(u64, u32)>>,
}

impl StreamHandler {
    fn new() -> Self {
        Self {
            starts: Mutex::new(Vec::new()),
            releases: Mutex::new(Vec::new()),
        }
    }
}

fn test_memfd(byte: u8) -> std::os::fd::OwnedFd {
    use std::io::Write as _;
    use std::os::fd::FromRawFd as _;
    // SAFETY: the name is a static NUL-terminated C string and the flags are
    // the documented memfd_create bitset.
    let fd = unsafe { libc::memfd_create(c"aegis-ipc-test".as_ptr(), libc::MFD_CLOEXEC) };
    assert!(fd >= 0, "memfd_create failed");
    // SAFETY: `fd` is a fresh owned descriptor.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    file.write_all(&[byte]).unwrap();
    file.into()
}

impl Handler for StreamHandler {
    fn policy_caps(&self) -> ConnectionCapabilities {
        ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        }
    }
    fn windows(&self) -> Vec<aegis_model::window::Window> {
        Vec::new()
    }
    fn workspaces(&self) -> aegis_model::workspace::WorkspaceSnapshot {
        aegis_model::workspace::WorkspaceSnapshot {
            outputs: Vec::new(),
        }
    }
    fn notifications(&self) -> Vec<aegis_model::notify::Notification> {
        Vec::new()
    }
    fn outputs(&self) -> Vec<aegis_model::output::OutputInfo> {
        Vec::new()
    }
    fn journal_since(&self, _since: u64) -> crate::journal::JournalSnapshot {
        crate::journal::JournalSnapshot {
            entries: Vec::new(),
            oldest_seq: 0,
            latest_seq: 0,
        }
    }
    fn resolve_scope(&self, name: &str) -> Option<Scope> {
        (name == "stream").then(|| Scope {
            ops: Some(vec![ActorCapability::StreamOutput]),
            ..Scope::default()
        })
    }
    fn stream_output_start(
        &self,
        _conn_id: u64,
        _max_fps: Option<u32>,
        _target: crate::schema::StreamTarget,
        allow_dmabuf: bool,
    ) -> Result<StreamInfo, String> {
        self.starts.lock().unwrap().push(allow_dmabuf);
        let (format, slots) = if allow_dmabuf {
            (
                StreamPixelFormat::Dmabuf {
                    drm_format: 0x3432_5258,
                    modifier: 0,
                },
                Some(StreamSlotTable {
                    stride: 8,
                    byte_len: 16,
                    fds: (0..3u8).map(test_memfd).collect(),
                }),
            )
        } else {
            (StreamPixelFormat::Bgra8, None)
        };
        Ok(StreamInfo {
            stream_id: 1,
            width: 2,
            height: 2,
            format,
            slots,
        })
    }
    fn stream_buffer_release(&self, stream_id: u64, slot: u32) {
        self.releases.lock().unwrap().push((stream_id, slot));
    }
    fn command(&self, _conn_id: u64, _subject: Option<&str>, _cmd: Command) {}
}

fn stream_hello(version: u32) -> Request {
    Request::Hello {
        version,
        caps: ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            interaction_domain: false,
        },
        scope: Some("stream".to_owned()),
        lease: Some(crate::schema::LeaseRequest { ttl_ms: 60_000 }),
        agent: None,
    }
}

/// Drive one connection on a scoped thread; returns its client end and the
/// writer-channel receiver. The loop exits when the client end drops.
fn spawn_connection<'scope, 'env>(
    scope: &'scope std::thread::Scope<'scope, 'env>,
    handler: &'env StreamHandler,
    streams: &'env Mutex<HashMap<u64, StreamLane>>,
    conn_id: u64,
    version: u32,
) -> (UnixStream, mpsc::Receiver<Outbound>)
where
    'env: 'scope,
{
    let (server_end, mut client) = UnixStream::pair().unwrap();
    let (tx, rx) = mpsc::sync_channel(OUTBOUND_QUEUE_DEPTH);
    let subs = Mutex::new(HashMap::new());
    let journal_subs = Mutex::new(HashMap::new());
    let next_sub = AtomicU64::new(0);
    let next_lease = AtomicU64::new(1);
    scope.spawn(move || {
        let mut read = server_end.try_clone().unwrap();
        let shutdown = Arc::new(server_end);
        drive_read_loop(
            &mut read,
            &tx,
            handler,
            &subs,
            &journal_subs,
            streams,
            &next_sub,
            &next_lease,
            &shutdown,
            conn_id,
        )
    });
    write_msg(&mut client, &stream_hello(version)).unwrap();
    match rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap() {
        Outbound::Response(Response::Hello { version: got, .. }) => {
            assert_eq!(got, version.min(PROTOCOL_VERSION));
        }
        other => panic!("expected Hello reply, got {other:?}"),
    }
    (client, rx)
}

fn recv_response(rx: &mpsc::Receiver<Outbound>) -> Response {
    match rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap() {
        Outbound::Response(response) => response,
        other => panic!("expected a plain response, got {other:?}"),
    }
}

#[test]
fn dmabuf_stream_start_requires_opt_in_and_protocol_25() {
    let handler = StreamHandler::new();
    let streams = Mutex::new(HashMap::new());
    std::thread::scope(|scope| {
        // An opted-in client speaking an older protocol gets SHM pixels.
        let (mut client24, rx24) = spawn_connection(scope, &handler, &streams, 1, 24);
        write_msg(
            &mut client24,
            &Request::StreamOutputStart {
                max_fps: None,
                target: crate::schema::StreamTarget::Output,
                dmabuf: Some(true),
            },
        )
        .unwrap();
        match recv_response(&rx24) {
            Response::StreamOutputStarted {
                format,
                slots,
                slot_stride,
                slot_bytes,
                ..
            } => {
                assert_eq!(format, StreamPixelFormat::Bgra8);
                assert_eq!((slots, slot_stride, slot_bytes), (None, None, None));
            }
            other => panic!("expected StreamOutputStarted, got {other:?}"),
        }
        drop(client24);

        // A v25 client that did not opt in gets SHM pixels as well.
        let (mut client25, rx25) = spawn_connection(scope, &handler, &streams, 2, 25);
        write_msg(
            &mut client25,
            &Request::StreamOutputStart {
                max_fps: None,
                target: crate::schema::StreamTarget::Output,
                dmabuf: None,
            },
        )
        .unwrap();
        match recv_response(&rx25) {
            Response::StreamOutputStarted { format, slots, .. } => {
                assert_eq!(format, StreamPixelFormat::Bgra8);
                assert_eq!(slots, None);
            }
            other => panic!("expected StreamOutputStarted, got {other:?}"),
        }
        drop(client25);

        // Opt-in at 25: the reply carries the slot metadata and the slot
        // descriptors follow on the writer channel.
        let (mut client25, rx25) = spawn_connection(scope, &handler, &streams, 3, 25);
        write_msg(
            &mut client25,
            &Request::StreamOutputStart {
                max_fps: None,
                target: crate::schema::StreamTarget::Output,
                dmabuf: Some(true),
            },
        )
        .unwrap();
        match rx25
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
        {
            Outbound::StreamStarted { response, table } => {
                match response {
                    Response::StreamOutputStarted {
                        format,
                        slots,
                        slot_stride,
                        slot_bytes,
                        ..
                    } => {
                        assert_eq!(
                            format,
                            StreamPixelFormat::Dmabuf {
                                drm_format: 0x3432_5258,
                                modifier: 0,
                            }
                        );
                        assert_eq!(slots, Some(3));
                        assert_eq!(slot_stride, Some(8));
                        assert_eq!(slot_bytes, Some(16));
                    }
                    other => panic!("expected StreamOutputStarted, got {other:?}"),
                }
                assert_eq!(table.fds.len(), 3);
            }
            other => panic!("expected the started-with-fds reply, got {other:?}"),
        }
        drop(client25);
    });
    assert_eq!(
        handler.starts.lock().unwrap().as_slice(),
        &[false, false, true]
    );
}

#[test]
fn stream_started_reply_sends_slot_descriptors_on_the_blob_channel() {
    let (mut server_end, mut client_end) = UnixStream::pair().unwrap();
    client_end
        .set_read_timeout(Some(std::time::Duration::from_secs(5)))
        .unwrap();
    let response = Response::StreamOutputStarted {
        stream_id: 1,
        width: 2,
        height: 2,
        format: StreamPixelFormat::Dmabuf {
            drm_format: 0x3432_5258,
            modifier: 0,
        },
        slots: Some(3),
        slot_stride: Some(8),
        slot_bytes: Some(16),
    };
    let table = StreamSlotTable {
        stride: 8,
        byte_len: 16,
        fds: (0..3u8).map(test_memfd).collect(),
    };
    write_stream_started(&mut server_end, &response, &table).unwrap();
    match read_msg::<_, Response>(&mut client_end).unwrap() {
        Response::StreamOutputStarted { slots: Some(3), .. } => {}
        other => panic!("expected StreamOutputStarted, got {other:?}"),
    }
    // One descriptor per slot, in slot order, each readable and holding the
    // byte the table was built with.
    for expected in 0..3u8 {
        let fd = crate::blob::receive_fd(&client_end).unwrap();
        use std::io::{Read as _, Seek as _};
        use std::os::fd::FromRawFd as _;
        // SAFETY: `receive_fd` returned a fresh owned descriptor.
        let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
        // SCM_RIGHTS shares the sender's file description, offset included.
        file.seek(std::io::SeekFrom::Start(0)).unwrap();
        let mut byte = [0u8; 1];
        file.read_exact(&mut byte).unwrap();
        assert_eq!(byte[0], expected);
    }
}

#[test]
fn slot_frames_cross_the_wire_without_a_blob() {
    let (mut server_end, mut client_end) = UnixStream::pair().unwrap();
    client_end
        .set_read_timeout(Some(std::time::Duration::from_millis(200)))
        .unwrap();
    let handler = StreamHandler::new();
    let scope = LiveScopeBinding {
        connection_id: 1,
        session: aegis_security::authority::ActorSessionId(1),
        name: None,
        principal: None,
        fallback: Scope {
            ops: Some(vec![ActorCapability::StreamOutput]),
            ..Scope::default()
        },
    };
    let lease_deadline = Mutex::new(std::time::Instant::now() + std::time::Duration::from_secs(60));
    let streams = Mutex::new(HashMap::new());
    let payload = StreamFramePayload::Slot(StreamSlotFrame {
        stream_id: 1,
        sequence: 7,
        width: 2,
        height: 2,
        stride: 8,
        format: StreamPixelFormat::Dmabuf {
            drm_format: 0x3432_5258,
            modifier: 0,
        },
        damage: vec![aegis_model::Rect::new(0, 0, 2, 2)],
        dropped: 3,
        slot: 2,
        byte_len: 16,
    });
    write_stream_frame(
        &mut server_end,
        payload,
        &handler,
        &scope,
        crate::schema::StreamTarget::Output,
        &lease_deadline,
        &streams,
    )
    .unwrap();
    match read_msg::<_, Event>(&mut client_end).unwrap() {
        Event::StreamFrame {
            sequence,
            dropped,
            byte_len,
            slot,
            ..
        } => {
            assert_eq!((sequence, dropped, byte_len), (7, 3, 16));
            assert_eq!(slot, Some(2));
        }
        other => panic!("expected StreamFrame, got {other:?}"),
    }
    // No blob follows a slot frame: the read times out instead of receiving.
    use std::io::Read as _;
    let mut byte = [0u8; 1];
    let error = client_end.read(&mut byte).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::WouldBlock);
}

#[test]
fn stream_buffer_release_is_forwarded_only_for_the_owning_connection() {
    let handler = StreamHandler::new();
    let streams = Mutex::new(HashMap::new());
    std::thread::scope(|scope| {
        let (mut client, rx) = spawn_connection(scope, &handler, &streams, 1, 25);
        write_msg(
            &mut client,
            &Request::StreamOutputStart {
                max_fps: None,
                target: crate::schema::StreamTarget::Output,
                dmabuf: Some(true),
            },
        )
        .unwrap();
        match rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap() {
            Outbound::StreamStarted { .. } => {}
            other => panic!("expected the started-with-fds reply, got {other:?}"),
        }

        // The owner releases a slot: forwarded and acknowledged.
        write_msg(
            &mut client,
            &Request::StreamBufferRelease {
                stream_id: 1,
                slot: 2,
            },
        )
        .unwrap();
        match recv_response(&rx) {
            Response::StreamBufferReleased { stream_id, slot } => {
                assert_eq!((stream_id, slot), (1, 2));
            }
            other => panic!("expected StreamBufferReleased, got {other:?}"),
        }

        // Another connection may not release the stream's slots.
        let (mut other, other_rx) = spawn_connection(scope, &handler, &streams, 2, 25);
        write_msg(
            &mut other,
            &Request::StreamBufferRelease {
                stream_id: 1,
                slot: 2,
            },
        )
        .unwrap();
        match recv_response(&other_rx) {
            Response::Error { message } => assert!(message.contains("unknown stream"), "{message}"),
            other => panic!("expected an error, got {other:?}"),
        }
        // An unknown stream id is refused for the owner too.
        write_msg(
            &mut client,
            &Request::StreamBufferRelease {
                stream_id: 99,
                slot: 0,
            },
        )
        .unwrap();
        match recv_response(&rx) {
            Response::Error { message } => assert!(message.contains("unknown stream"), "{message}"),
            other => panic!("expected an error, got {other:?}"),
        }
        drop(other);
        drop(client);
    });
    assert_eq!(handler.releases.lock().unwrap().as_slice(), &[(1, 2)]);
}
