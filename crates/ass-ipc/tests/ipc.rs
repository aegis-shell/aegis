//! End-to-end exercise of the IPC: loopback `Server` with test `Handler`s
//! and a `Client` over a real unix socket on a process-unique temp path.
//! No Vulkan or Wayland dependency.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ass_core::window::{Window, WindowId};
use ass_ipc::{
    Capabilities, Client, Command, Event, Handler, OpClass, PROTOCOL_VERSION, RealmAction,
    RealmActionResult, Scope, Server,
};

/// A unique throwaway socket path under the temp dir, namespaced by pid +
/// counter so parallel test processes do not collide.
fn scratch() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut p = std::env::temp_dir();
    p.push(format!("ass-ipc-{pid}-{n}.sock"));
    p
}

/// A handler returning a fixed window snapshot and recording the commands it
/// receives. `policy` selects what it grants.
struct TestHandler {
    windows: Vec<Window>,
    policy: Capabilities,
    commands: Mutex<Vec<Command>>,
    command_connections: Mutex<Vec<u64>>,
    realm_actions: Mutex<Vec<RealmAction>>,
    realm_connections: Mutex<Vec<u64>>,
    refusals: Mutex<Vec<(u64, ass_ipc::JournalMutation, String)>>,
    scopes: Mutex<HashMap<String, Scope>>,
    capture_delay_ms: AtomicU64,
    capture_security_active: std::sync::atomic::AtomicBool,
}

impl TestHandler {
    /// Query-only policy (the default): no control, no session.
    fn query(windows: Vec<Window>) -> Self {
        TestHandler {
            windows,
            policy: Capabilities::QUERY,
            commands: Mutex::new(Vec::new()),
            command_connections: Mutex::new(Vec::new()),
            realm_actions: Mutex::new(Vec::new()),
            realm_connections: Mutex::new(Vec::new()),
            refusals: Mutex::new(Vec::new()),
            scopes: Mutex::new(test_scopes()),
            capture_delay_ms: AtomicU64::new(0),
            capture_security_active: std::sync::atomic::AtomicBool::new(true),
        }
    }
    /// Grants control and session, so command tests can exercise them.
    fn permissive(windows: Vec<Window>) -> Self {
        TestHandler {
            windows,
            policy: Capabilities {
                query: true,
                control: true,
                input: true,
                session: true,
                realm: true,
            },
            commands: Mutex::new(Vec::new()),
            command_connections: Mutex::new(Vec::new()),
            realm_actions: Mutex::new(Vec::new()),
            realm_connections: Mutex::new(Vec::new()),
            refusals: Mutex::new(Vec::new()),
            scopes: Mutex::new(test_scopes()),
            capture_delay_ms: AtomicU64::new(0),
            capture_security_active: std::sync::atomic::AtomicBool::new(true),
        }
    }
}

impl Handler for TestHandler {
    fn policy_caps(&self) -> Capabilities {
        self.policy
    }
    fn windows(&self) -> Vec<Window> {
        self.windows.clone()
    }
    fn workspaces(&self) -> ass_core::workspace::WorkspaceSnapshot {
        // A minimal snapshot: one output, one empty workspace. Sufficient for
        // the IPC plumbing tests; the model itself is exercised in ass-core.
        use ass_core::workspace::{OutputSnapshot, WorkspaceEntry, WorkspaceId};
        ass_core::workspace::WorkspaceSnapshot {
            outputs: vec![OutputSnapshot {
                id: ass_core::workspace::OutputId(0),
                connector: "test".into(),
                current: Some(WorkspaceId(0)),
                workspaces: vec![WorkspaceEntry {
                    id: WorkspaceId(0),
                    label: None,
                    tiled: false,
                    toplevels: self.windows.iter().map(|w| w.id).collect(),
                }],
            }],
        }
    }
    fn notifications(&self) -> Vec<ass_core::notify::Notification> {
        Vec::new()
    }
    fn outputs(&self) -> Vec<ass_core::output::OutputInfo> {
        Vec::new()
    }
    fn journal_since(&self, _since: u64) -> ass_ipc::JournalSnapshot {
        ass_ipc::JournalSnapshot {
            entries: vec![],
            oldest_seq: 1,
            latest_seq: 0,
        }
    }
    fn command(&self, conn_id: u64, cmd: Command) {
        self.command_connections.lock().unwrap().push(conn_id);
        self.commands.lock().unwrap().push(cmd);
    }
    fn realms(&self) -> ass_core::realm::RealmSnapshot {
        let mut model = ass_core::realm::RealmModel::new();
        model.create_agent_realm("test", ass_core::realm::SeatCapabilities::POINTER_KEYBOARD);
        let mut snapshot = model.snapshot();
        snapshot.revision = 4;
        snapshot
    }
    fn realm_action(&self, conn_id: u64, action: RealmAction) -> Result<RealmActionResult, String> {
        self.realm_connections.lock().unwrap().push(conn_id);
        self.realm_actions.lock().unwrap().push(action.clone());
        match action {
            RealmAction::Create { .. } => Ok(RealmActionResult::Created {
                bundle: ass_core::realm::RealmBundle {
                    principal: ass_core::realm::PrincipalId(2),
                    realm: ass_core::realm::RealmId(2),
                    seat: ass_core::realm::SeatId(2),
                    revision: 2,
                },
            }),
            RealmAction::Transact { .. } => Ok(RealmActionResult::TransactionCommitted {
                receipt: ass_core::realm::RealmTransactionReceipt {
                    before_revision: 1,
                    after_revision: 2,
                    results: Vec::new(),
                },
            }),
            RealmAction::Revoke {
                realm, fallback, ..
            } => Ok(RealmActionResult::Revoked {
                receipt: ass_core::realm::RealmRevocation {
                    realm,
                    fallback,
                    transferred_groups: Vec::new(),
                    revision: 3,
                },
            }),
        }
    }
    fn capture_output(
        &self,
        _region: Option<ass_core::Rect>,
    ) -> Result<ass_ipc::CaptureOutputPayload, String> {
        std::thread::sleep(std::time::Duration::from_millis(
            self.capture_delay_ms.load(Ordering::Relaxed),
        ));
        Ok(ass_ipc::CaptureOutputPayload {
            width: 2,
            height: 1,
            png: vec![1u8, 2, 3, 4, 5],
        })
    }
    fn resolve_scope(&self, name: &str) -> Option<Scope> {
        self.scopes.lock().unwrap().get(name).cloned()
    }
    fn audit_refusal(&self, conn_id: u64, mutation: ass_ipc::JournalMutation, reason: String) {
        self.refusals
            .lock()
            .unwrap()
            .push((conn_id, mutation, reason));
    }
    fn capture_security_active(&self) -> bool {
        self.capture_security_active.load(Ordering::Acquire)
    }
    fn capture_realm(
        &self,
        realm: ass_core::realm::RealmId,
        region: Option<ass_core::Rect>,
    ) -> Result<ass_ipc::CaptureRealmPayload, String> {
        std::thread::sleep(std::time::Duration::from_millis(
            self.capture_delay_ms.load(Ordering::Relaxed),
        ));
        Ok(ass_ipc::CaptureRealmPayload {
            capture: ass_ipc::RealmCapture {
                realm,
                width: 2,
                height: 1,
                scale_milli: 1250,
                region: region.unwrap_or_else(|| ass_core::Rect::new(0, 0, 2, 1)),
                placements: vec![ass_core::realm::RealmWindowPlacement {
                    window: WindowId(1),
                    output_rect: ass_core::Rect::new(0, 0, 2, 1),
                    surface_size: ass_core::Size { w: 20, h: 10 },
                }],
                png_bytes: 3,
                revision: 4,
            },
            png: vec![9u8, 8, 7],
        })
    }
}

fn test_scopes() -> HashMap<String, Scope> {
    HashMap::from([
        (
            "focus-first".into(),
            Scope {
                windows: Some(vec![WindowId(1)]),
                ops: Some(vec![OpClass::Focus]),
                ..Scope::default()
            },
        ),
        (
            "input-first".into(),
            Scope {
                windows: Some(vec![WindowId(1)]),
                ops: Some(vec![OpClass::InjectInput]),
                ..Scope::default()
            },
        ),
        (
            "capture".into(),
            Scope {
                ops: Some(vec![OpClass::CaptureOutput]),
                ..Scope::default()
            },
        ),
        (
            "realm".into(),
            Scope {
                realms: Some(vec![
                    ass_core::realm::HUMAN_REALM,
                    ass_core::realm::RealmId(2),
                ]),
                ops: Some(vec![
                    OpClass::CreateRealm,
                    OpClass::TransactRealm,
                    OpClass::RevokeRealm,
                    OpClass::CaptureRealm,
                ]),
                ..Scope::default()
            },
        ),
    ])
}

fn sample_windows() -> Vec<Window> {
    let mut a = Window::new(WindowId(1));
    a.title = Some("first".into());
    a.app_id = Some("org.example.first".into());
    a.state.activated = true;
    let mut b = Window::new(WindowId(2));
    b.title = Some("second".into());
    b.state.maximized = true;
    vec![a, b]
}

#[test]
fn handshake_reports_query_always_granted() {
    let path = scratch();
    let handler = Arc::new(TestHandler::query(sample_windows()));
    let _server = Server::start(&path, handler).expect("bind");

    // Request more than the policy grants; the server intersects and forces
    // query on, so the client learns the truth.
    let client = Client::connect_with(
        &path,
        Capabilities {
            query: true,
            control: true,
            input: true,
            session: true,
            realm: true,
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
fn named_scope_is_reported_and_enforced() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let requested = Capabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        realm: false,
    };

    let mut client = Client::connect_scoped(&path, requested, "focus-first").expect("connect");
    assert_eq!(client.scope().windows, Some(vec![WindowId(1)]));
    assert_eq!(client.scope().ops, Some(vec![OpClass::Focus]));
    client
        .command(Command::Focus { id: WindowId(1) })
        .expect("allowed focus");
    let wrong_window = client
        .command(Command::Focus { id: WindowId(2) })
        .unwrap_err();
    assert!(wrong_window.to_string().contains("out of scope"));
    let wrong_operation = client
        .command(Command::Close { id: WindowId(1) })
        .unwrap_err();
    assert!(wrong_operation.to_string().contains("out of scope"));
}

#[test]
fn synthetic_input_requires_a_named_scope_and_separate_capability() {
    use ass_core::input::SyntheticInputAction;

    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let requested = Capabilities {
        query: true,
        control: false,
        input: true,
        session: false,
        realm: false,
    };

    let mut unscoped = Client::connect_with(&path, requested).expect("unscoped connect");
    assert!(!unscoped.caps().input, "unscoped input must fail closed");
    let action = SyntheticInputAction::Click {
        position: ass_core::Point { x: 10, y: 20 },
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
    let requested = Capabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        realm: false,
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

#[test]
fn realm_lifecycle_capture_and_lease_are_scoped_and_synchronous() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let requested = Capabilities {
        query: true,
        control: false,
        input: false,
        session: false,
        realm: true,
    };
    let mut client = Client::connect_scoped(&path, requested, "realm").expect("connect");
    assert!(client.caps().realm);
    let original_lease = client.lease().expect("privileged connection has lease");
    let renewed = client.renew_lease(30_000).expect("renew lease");
    assert_eq!(renewed.id, original_lease.id);
    assert_eq!(renewed.ttl_ms, 30_000);

    let snapshot = client.realms().expect("realm snapshot");
    assert_eq!(snapshot.revision, 4);
    let result = client
        .realm_action(RealmAction::Create {
            label: "test agent".into(),
            capabilities: ass_core::realm::SeatCapabilities::POINTER_KEYBOARD,
            output: Some(ass_core::realm::VirtualOutput::DEFAULT_AGENT),
        })
        .expect("create realm");
    assert!(matches!(
        result,
        RealmActionResult::Created {
            bundle: ass_core::realm::RealmBundle {
                realm: ass_core::realm::RealmId(2),
                ..
            },
            ..
        }
    ));
    let capture = client
        .capture_realm(ass_core::realm::RealmId(2), None)
        .expect("realm capture");
    assert_eq!((capture.width, capture.height, capture.revision), (2, 1, 4));
    assert_eq!(capture.scale_milli, 1250);
    assert_eq!(capture.region, ass_core::Rect::new(0, 0, 2, 1));
    assert_eq!(
        capture.placements,
        vec![ass_core::realm::RealmWindowPlacement {
            window: WindowId(1),
            output_rect: ass_core::Rect::new(0, 0, 2, 1),
            surface_size: ass_core::Size { w: 20, h: 10 },
        }]
    );
    assert_eq!(capture.png, vec![9, 8, 7]);
    assert_eq!(handler.realm_actions.lock().unwrap().len(), 1);
}

#[test]
fn expired_privileged_lease_fails_closed_without_losing_query_access() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let requested = Capabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        realm: false,
    };
    let mut client = Client::connect_scoped(&path, requested, "focus-first").expect("connect");
    client.renew_lease(1_000).expect("short lease");
    std::thread::sleep(std::time::Duration::from_millis(1_100));

    assert_eq!(
        client.windows().expect("query survives lease expiry").len(),
        2
    );
    let error = client
        .command(Command::Focus { id: WindowId(1) })
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
    let requested = Capabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        realm: false,
    };
    let mut client = Client::connect_scoped(&path, requested, "capture").expect("connect");
    client.renew_lease(1_000).expect("short lease");
    let error = client
        .capture_output()
        .expect_err("expired in-flight capture must not deliver pixels");
    assert!(error.to_string().contains("lease expired"), "{error}");
}

#[test]
fn realm_operations_fail_closed_without_named_scope() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, handler).expect("bind");
    let requested = Capabilities {
        query: true,
        control: false,
        input: false,
        session: false,
        realm: true,
    };
    let mut client = Client::connect_with(&path, requested).expect("connect");
    let error = client
        .realm_action(RealmAction::Create {
            label: "denied".into(),
            capabilities: ass_core::realm::SeatCapabilities::POINTER_KEYBOARD,
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

    let err = Client::connect_scoped(&path, Capabilities::QUERY, "missing")
        .err()
        .expect("unknown scope must fail");
    assert!(err.to_string().contains("unknown scope 'missing'"), "{err}");
}

#[test]
fn scope_revocation_applies_to_existing_connections() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let requested = Capabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        realm: false,
    };
    let mut client = Client::connect_scoped(&path, requested, "focus-first").expect("connect");
    client
        .command(Command::Focus { id: WindowId(1) })
        .expect("allowed before revoke");

    handler.scopes.lock().unwrap().clear();
    let err = client
        .command(Command::Focus { id: WindowId(1) })
        .unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");
}

#[test]
fn scope_revocation_stops_existing_realm_and_capture_connections() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let requested = Capabilities {
        query: true,
        control: false,
        input: false,
        session: false,
        realm: true,
    };
    let mut client = Client::connect_scoped(&path, requested, "realm").expect("connect");
    handler.scopes.lock().unwrap().clear();

    let action_error = client
        .realm_action(RealmAction::Create {
            label: "revoked".into(),
            capabilities: ass_core::realm::SeatCapabilities::POINTER_KEYBOARD,
            output: None,
        })
        .expect_err("removed scope must revoke Realm mutation");
    assert!(
        action_error.to_string().contains("out of scope"),
        "{action_error}"
    );
    let capture_error = client
        .capture_realm(ass_core::realm::RealmId(2), None)
        .expect_err("removed scope must revoke Realm capture");
    assert!(
        capture_error.to_string().contains("out of scope"),
        "{capture_error}"
    );
    assert!(handler.realm_actions.lock().unwrap().is_empty());
}

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
    let requested = Capabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        realm: false,
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
fn wrong_protocol_version_is_refused_at_handshake() {
    use ass_ipc::Request;
    let path = scratch();
    let handler = Arc::new(TestHandler::query(vec![]));
    let _server = Server::start(&path, handler).expect("bind");

    let mut s = std::os::unix::net::UnixStream::connect(&path).unwrap();
    let bad = Request::Hello {
        version: PROTOCOL_VERSION + 1,
        caps: Capabilities::QUERY,
        scope: None,
        lease: None,
    };
    ass_ipc::codec::write_msg(&mut s, &bad).unwrap();
    let resp: ass_ipc::Response = ass_ipc::codec::read_msg(&mut s).unwrap();
    match resp {
        ass_ipc::Response::Error { message } => {
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
        Capabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            realm: false,
        },
    )
    .expect("connect");
    client
        .command(Command::Close { id: WindowId(2) })
        .expect("command");
    client
        .command(Command::Focus { id: WindowId(1) })
        .expect("command");
    client
        .command(Command::Cycle { forward: true })
        .expect("command");

    // The command() calls block until the server acked (Ok), by which point
    // the handler has recorded each one.
    let recorded = handler.commands.lock().unwrap();
    assert_eq!(recorded.len(), 3, "{recorded:?}");
    assert!(recorded.contains(&Command::Close { id: WindowId(2) }));
    assert!(recorded.contains(&Command::Focus { id: WindowId(1) }));
}

#[test]
fn ipc_origins_are_unique_and_pre_dispatch_refusals_are_audited() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let requested = Capabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        realm: false,
    };
    let mut first = Client::connect_with(&path, requested).expect("first connection");
    let mut second = Client::connect_with(&path, requested).expect("second connection");
    first
        .command(Command::Focus { id: WindowId(1) })
        .expect("first command");
    second
        .command(Command::Focus { id: WindowId(2) })
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
        ass_ipc::JournalMutation::Command {
            cmd: Command::Close { id: WindowId(1) }
        }
    ));
}

#[test]
fn session_command_quit_is_accepted() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let mut client = Client::connect_with(
        &path,
        Capabilities {
            query: true,
            control: false,
            input: false,
            session: true,
            realm: false,
        },
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
        Capabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            realm: false,
        },
    )
    .expect("connect");
    client
        .switch_workspace(ass_core::workspace::Switch::Next)
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
        Capabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            realm: false,
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
    let n = ass_core::notify::Notification {
        id: 7,
        summary: "ping".into(),
        body: "pong".into(),
        app_id: None,
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
