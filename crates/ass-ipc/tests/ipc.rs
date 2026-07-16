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
    Capabilities, Client, Command, Event, Handler, OpClass, Scope, Server, PROTOCOL_VERSION,
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
    scopes: Mutex<HashMap<String, Scope>>,
}

impl TestHandler {
    /// Query-only policy (the default): no control, no session.
    fn query(windows: Vec<Window>) -> Self {
        TestHandler {
            windows,
            policy: Capabilities::QUERY,
            commands: Mutex::new(Vec::new()),
            scopes: Mutex::new(test_scopes()),
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
            },
            commands: Mutex::new(Vec::new()),
            scopes: Mutex::new(test_scopes()),
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
    fn command(&self, cmd: Command) {
        self.commands.lock().unwrap().push(cmd);
    }
    fn resolve_scope(&self, name: &str) -> Option<Scope> {
        self.scopes.lock().unwrap().get(name).cloned()
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
    assert!(handler
        .commands
        .lock()
        .unwrap()
        .contains(&Command::InjectInput {
            id: WindowId(1),
            actions: vec![action],
        }));

    let before = handler.commands.lock().unwrap().len();
    let err = scoped.inject_input(WindowId(1), vec![]).unwrap_err();
    assert!(err.to_string().contains("action count"), "{err}");
    assert_eq!(handler.commands.lock().unwrap().len(), before);
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
