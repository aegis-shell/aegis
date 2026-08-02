//! End-to-end exercise of the IPC: loopback `Server` with test `Handler`s
//! and a `Client` over a real unix socket on a process-unique temp path.
//! No Vulkan or Wayland dependency.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aegis_core::window::{Window, WindowId};
use aegis_ipc::{
    Capabilities, Client, Command, Event, Handler, OpClass, PROTOCOL_VERSION, RealmAction,
    RealmActionResult, Scope, Server, SettingsAction, SettingsReceipt, SettingsSnapshot,
    SystemAction, SystemStatus,
};

/// A unique throwaway socket path under the temp dir, namespaced by pid +
/// counter so parallel test processes do not collide.
fn scratch() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut p = std::env::temp_dir();
    p.push(format!("aegis-ipc-{pid}-{n}.sock"));
    p
}

/// One recorded pairing invocation: connection, display label, requested set.
type PairCall = (u64, Option<String>, Vec<OpClass>);

/// A handler returning a fixed window snapshot and recording the commands it
/// receives. `policy` selects what it grants.
struct TestHandler {
    windows: Vec<Window>,
    policy: Capabilities,
    commands: Mutex<Vec<Command>>,
    command_connections: Mutex<Vec<u64>>,
    realm_actions: Mutex<Vec<RealmAction>>,
    realm_connections: Mutex<Vec<u64>>,
    settings: Mutex<SettingsSnapshot>,
    system_status: Mutex<SystemStatus>,
    system_actions: Mutex<Vec<SystemAction>>,
    system_connections: Mutex<Vec<u64>>,
    system_result: Mutex<Result<(), String>>,
    settings_actions: Mutex<Vec<SettingsAction>>,
    settings_connections: Mutex<Vec<u64>>,
    refusals: Mutex<Vec<(u64, aegis_ipc::JournalMutation, String)>>,
    scopes: Mutex<HashMap<String, Scope>>,
    capture_delay_ms: AtomicU64,
    capture_security_active: std::sync::atomic::AtomicBool,
    stream_starts: AtomicU64,
    stream_stops: Mutex<Vec<u64>>,
    stream_disconnects: Mutex<Vec<u64>>,
    stream_targets: Mutex<Vec<aegis_ipc::StreamTarget>>,
    picks: Mutex<Vec<(u64, aegis_ipc::PickKind)>>,
    app_picks: Mutex<Vec<(u64, Vec<String>)>>,
    secret_prompts: Mutex<Vec<(u64, String)>>,
    confirms: Mutex<Vec<(u64, String)>>,
    wallpapers: Mutex<Vec<(u64, std::path::PathBuf)>>,
    idle_inhibits: Mutex<Vec<(u64, bool)>>,
    idle_disconnects: Mutex<Vec<u64>>,
    pair_result: Mutex<Result<aegis_ipc::PairedAgent, String>>,
    pair_calls: Mutex<Vec<PairCall>>,
    lookup_result: Mutex<Option<aegis_ipc::AgentIdentity>>,
    lockdown_flag: std::sync::atomic::AtomicBool,
    grants: Mutex<Vec<(String, OpClass, bool)>>,
    grant_calls: Mutex<Vec<(u64, String, OpClass)>>,
    request_grant_result: Mutex<Result<bool, String>>,
    principal_infos: Mutex<Vec<aegis_ipc::AgentPrincipalInfo>>,
    grant_infos: Mutex<Vec<aegis_ipc::AgentGrantInfo>>,
    management_log: Mutex<Vec<String>>,
    register_result: Mutex<Result<(String, String), String>>,
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
            settings: Mutex::new(SettingsSnapshot {
                revision: 7,
                ..SettingsSnapshot::default()
            }),
            system_status: Mutex::new(SystemStatus {
                volume: Some(42),
                brightness: Some(73),
                ..SystemStatus::default()
            }),
            system_actions: Mutex::new(Vec::new()),
            system_connections: Mutex::new(Vec::new()),
            system_result: Mutex::new(Ok(())),
            settings_actions: Mutex::new(Vec::new()),
            settings_connections: Mutex::new(Vec::new()),
            refusals: Mutex::new(Vec::new()),
            scopes: Mutex::new(test_scopes()),
            capture_delay_ms: AtomicU64::new(0),
            capture_security_active: std::sync::atomic::AtomicBool::new(true),
            stream_starts: AtomicU64::new(0),
            stream_stops: Mutex::new(Vec::new()),
            stream_disconnects: Mutex::new(Vec::new()),
            stream_targets: Mutex::new(Vec::new()),
            picks: Mutex::new(Vec::new()),
            app_picks: Mutex::new(Vec::new()),
            secret_prompts: Mutex::new(Vec::new()),
            confirms: Mutex::new(Vec::new()),
            wallpapers: Mutex::new(Vec::new()),
            idle_inhibits: Mutex::new(Vec::new()),
            idle_disconnects: Mutex::new(Vec::new()),
            pair_result: Mutex::new(Err("pairing not offered".into())),
            pair_calls: Mutex::new(Vec::new()),
            lookup_result: Mutex::new(None),
            lockdown_flag: std::sync::atomic::AtomicBool::new(false),
            grants: Mutex::new(Vec::new()),
            grant_calls: Mutex::new(Vec::new()),
            request_grant_result: Mutex::new(Err("no interactive grant".into())),
            principal_infos: Mutex::new(Vec::new()),
            grant_infos: Mutex::new(Vec::new()),
            management_log: Mutex::new(Vec::new()),
            register_result: Mutex::new(Err("no register".into())),
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
            settings: Mutex::new(SettingsSnapshot {
                revision: 7,
                ..SettingsSnapshot::default()
            }),
            system_status: Mutex::new(SystemStatus {
                volume: Some(42),
                brightness: Some(73),
                ..SystemStatus::default()
            }),
            system_actions: Mutex::new(Vec::new()),
            system_connections: Mutex::new(Vec::new()),
            system_result: Mutex::new(Ok(())),
            settings_actions: Mutex::new(Vec::new()),
            settings_connections: Mutex::new(Vec::new()),
            refusals: Mutex::new(Vec::new()),
            scopes: Mutex::new(test_scopes()),
            capture_delay_ms: AtomicU64::new(0),
            capture_security_active: std::sync::atomic::AtomicBool::new(true),
            stream_starts: AtomicU64::new(0),
            stream_stops: Mutex::new(Vec::new()),
            stream_disconnects: Mutex::new(Vec::new()),
            stream_targets: Mutex::new(Vec::new()),
            picks: Mutex::new(Vec::new()),
            app_picks: Mutex::new(Vec::new()),
            secret_prompts: Mutex::new(Vec::new()),
            confirms: Mutex::new(Vec::new()),
            wallpapers: Mutex::new(Vec::new()),
            idle_inhibits: Mutex::new(Vec::new()),
            idle_disconnects: Mutex::new(Vec::new()),
            pair_result: Mutex::new(Err("pairing not offered".into())),
            pair_calls: Mutex::new(Vec::new()),
            lookup_result: Mutex::new(None),
            lockdown_flag: std::sync::atomic::AtomicBool::new(false),
            grants: Mutex::new(Vec::new()),
            grant_calls: Mutex::new(Vec::new()),
            request_grant_result: Mutex::new(Err("no interactive grant".into())),
            principal_infos: Mutex::new(Vec::new()),
            grant_infos: Mutex::new(Vec::new()),
            management_log: Mutex::new(Vec::new()),
            register_result: Mutex::new(Err("no register".into())),
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
    fn workspaces(&self) -> aegis_core::workspace::WorkspaceSnapshot {
        // A minimal snapshot: one output, one empty workspace. Sufficient for
        // the IPC plumbing tests; the model itself is exercised in aegis-core.
        use aegis_core::workspace::{OutputSnapshot, WorkspaceEntry, WorkspaceId};
        aegis_core::workspace::WorkspaceSnapshot {
            outputs: vec![OutputSnapshot {
                id: aegis_core::workspace::OutputId(0),
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
    fn notifications(&self) -> Vec<aegis_core::notify::Notification> {
        Vec::new()
    }
    fn outputs(&self) -> Vec<aegis_core::output::OutputInfo> {
        Vec::new()
    }
    fn journal_since(&self, _since: u64) -> aegis_ipc::JournalSnapshot {
        aegis_ipc::JournalSnapshot {
            entries: vec![],
            oldest_seq: 1,
            latest_seq: 0,
        }
    }
    fn command(&self, conn_id: u64, cmd: Command) {
        self.command_connections.lock().unwrap().push(conn_id);
        self.commands.lock().unwrap().push(cmd);
    }
    fn realms(&self) -> aegis_core::realm::RealmSnapshot {
        let mut model = aegis_core::realm::RealmModel::new();
        model.create_agent_realm(
            "test",
            aegis_core::realm::SeatCapabilities::POINTER_KEYBOARD,
        );
        let mut snapshot = model.snapshot();
        snapshot.revision = 4;
        snapshot
    }
    fn settings(&self) -> SettingsSnapshot {
        self.settings.lock().unwrap().clone()
    }
    fn system_status(&self) -> SystemStatus {
        self.system_status.lock().unwrap().clone()
    }
    fn system_action(&self, conn_id: u64, action: SystemAction) -> Result<(), String> {
        self.system_connections.lock().unwrap().push(conn_id);
        self.system_actions.lock().unwrap().push(action);
        self.system_result.lock().unwrap().clone()
    }
    fn settings_action(
        &self,
        conn_id: u64,
        expected_revision: Option<u64>,
        action: SettingsAction,
    ) -> Result<SettingsReceipt, String> {
        let mut snapshot = self.settings.lock().unwrap();
        if expected_revision.is_some_and(|expected| expected != snapshot.revision) {
            return Err(format!(
                "settings revision conflict: expected {}, actual {}",
                expected_revision.unwrap(),
                snapshot.revision
            ));
        }
        self.settings_connections.lock().unwrap().push(conn_id);
        self.settings_actions.lock().unwrap().push(action.clone());
        match action {
            SettingsAction::SetTouchpad { config } => snapshot.touchpad.config = config,
            SettingsAction::SetDisplay { .. } => {}
            SettingsAction::SetDesktopPreferences { preferences } => {
                snapshot.preferences = preferences;
            }
            SettingsAction::SetIdle { settings } => snapshot.idle = settings,
        }
        snapshot.revision += 1;
        Ok(SettingsReceipt {
            revision: snapshot.revision,
        })
    }
    fn realm_action(
        &self,
        conn_id: u64,
        _subject: Option<&str>,
        action: RealmAction,
    ) -> Result<RealmActionResult, String> {
        self.realm_connections.lock().unwrap().push(conn_id);
        self.realm_actions.lock().unwrap().push(action.clone());
        match action {
            RealmAction::Create { .. } => Ok(RealmActionResult::Created {
                bundle: aegis_core::realm::RealmBundle {
                    principal: aegis_core::realm::PrincipalId(2),
                    realm: aegis_core::realm::RealmId(2),
                    seat: aegis_core::realm::SeatId(2),
                    revision: 2,
                },
            }),
            RealmAction::Transact { .. } => Ok(RealmActionResult::TransactionCommitted {
                receipt: aegis_core::realm::RealmTransactionReceipt {
                    before_revision: 1,
                    after_revision: 2,
                    results: Vec::new(),
                },
            }),
            RealmAction::Revoke {
                realm, fallback, ..
            } => Ok(RealmActionResult::Revoked {
                receipt: aegis_core::realm::RealmRevocation {
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
        _region: Option<aegis_core::Rect>,
    ) -> Result<aegis_ipc::CaptureOutputPayload, String> {
        std::thread::sleep(std::time::Duration::from_millis(
            self.capture_delay_ms.load(Ordering::Relaxed),
        ));
        Ok(aegis_ipc::CaptureOutputPayload {
            width: 2,
            height: 1,
            png: vec![1u8, 2, 3, 4, 5],
        })
    }
    fn resolve_scope(&self, name: &str) -> Option<Scope> {
        self.scopes.lock().unwrap().get(name).cloned()
    }
    fn agent_lookup(&self, _credential: &str) -> Option<aegis_ipc::AgentIdentity> {
        self.lookup_result.lock().unwrap().clone()
    }
    fn pair_agent(
        &self,
        conn_id: u64,
        label: Option<&str>,
        requested: &[OpClass],
    ) -> Result<aegis_ipc::PairedAgent, String> {
        self.pair_calls.lock().unwrap().push((
            conn_id,
            label.map(str::to_owned),
            requested.to_vec(),
        ));
        self.pair_result.lock().unwrap().clone()
    }
    fn lockdown(&self) -> bool {
        self.lockdown_flag.load(Ordering::Relaxed)
    }
    fn grant_for(&self, principal: &str, op: OpClass) -> Option<bool> {
        self.grants
            .lock()
            .unwrap()
            .iter()
            .find(|(p, o, _)| p == principal && *o == op)
            .map(|(_, _, decision)| *decision)
    }
    fn request_grant(&self, conn_id: u64, principal: &str, op: OpClass) -> Result<bool, String> {
        self.grant_calls
            .lock()
            .unwrap()
            .push((conn_id, principal.to_owned(), op));
        self.request_grant_result.lock().unwrap().clone()
    }
    fn agent_principals(&self) -> Vec<aegis_ipc::AgentPrincipalInfo> {
        self.principal_infos.lock().unwrap().clone()
    }
    fn agent_grants(&self, principal: Option<&str>) -> Vec<aegis_ipc::AgentGrantInfo> {
        self.grant_infos
            .lock()
            .unwrap()
            .iter()
            .filter(|grant| principal.is_none_or(|p| grant.principal == p))
            .cloned()
            .collect()
    }
    fn rename_agent_principal(&self, principal: &str, label: Option<&str>) -> Result<(), String> {
        self.management_log
            .lock()
            .unwrap()
            .push(format!("rename:{principal}:{label:?}"));
        Ok(())
    }
    fn forget_agent_principal(&self, principal: &str) -> Result<(), String> {
        self.management_log
            .lock()
            .unwrap()
            .push(format!("forget:{principal}"));
        Ok(())
    }
    fn set_agent_ceiling(
        &self,
        principal: &str,
        pregranted: &[OpClass],
        gated: &[OpClass],
    ) -> Result<(), String> {
        self.management_log.lock().unwrap().push(format!(
            "ceiling:{principal}:{}+{}",
            pregranted.len(),
            gated.len()
        ));
        Ok(())
    }
    fn register_agent(
        &self,
        label: Option<&str>,
        pregranted: &[OpClass],
        gated: &[OpClass],
    ) -> Result<(String, String), String> {
        self.management_log.lock().unwrap().push(format!(
            "register:{label:?}:{}+{}",
            pregranted.len(),
            gated.len()
        ));
        self.register_result.lock().unwrap().clone()
    }
    fn revoke_agent_grant(&self, principal: &str, op: OpClass) -> Result<(), String> {
        self.management_log
            .lock()
            .unwrap()
            .push(format!("revoke:{principal}:{op:?}"));
        Ok(())
    }
    fn audit_refusal(&self, conn_id: u64, mutation: aegis_ipc::JournalMutation, reason: String) {
        self.refusals
            .lock()
            .unwrap()
            .push((conn_id, mutation, reason));
    }
    fn capture_security_active(&self) -> bool {
        self.capture_security_active.load(Ordering::Acquire)
    }
    fn stream_output_start(
        &self,
        _conn_id: u64,
        _max_fps: Option<u32>,
        target: aegis_ipc::StreamTarget,
    ) -> Result<aegis_ipc::StreamInfo, String> {
        self.stream_targets.lock().unwrap().push(target);
        let stream_id = self.stream_starts.fetch_add(1, Ordering::AcqRel) + 1;
        Ok(aegis_ipc::StreamInfo {
            stream_id,
            width: 2,
            height: 2,
            format: aegis_ipc::StreamPixelFormat::Bgra8,
        })
    }
    fn stream_output_stop(&self, stream_id: u64) {
        self.stream_stops.lock().unwrap().push(stream_id);
    }
    fn streams_disconnected(&self, conn_id: u64) {
        self.stream_disconnects.lock().unwrap().push(conn_id);
    }
    fn set_idle_inhibit(&self, conn_id: u64, inhibit: bool) -> Result<bool, String> {
        self.idle_inhibits.lock().unwrap().push((conn_id, inhibit));
        Ok(inhibit)
    }
    fn idle_inhibit_disconnected(&self, conn_id: u64) {
        self.idle_disconnects.lock().unwrap().push(conn_id);
    }
    fn pick_target(
        &self,
        conn_id: u64,
        kind: aegis_ipc::PickKind,
    ) -> Result<aegis_ipc::PickResult, String> {
        self.picks.lock().unwrap().push((conn_id, kind));
        Ok(aegis_ipc::PickResult::Region {
            rect: aegis_core::Rect::new(1, 2, 30, 40),
        })
    }
    fn pick_app(
        &self,
        conn_id: u64,
        choices: Vec<String>,
        _subject: Option<String>,
        _last_choice: Option<String>,
    ) -> Result<aegis_ipc::AppPickResult, String> {
        self.app_picks.lock().unwrap().push((conn_id, choices));
        Ok(aegis_ipc::AppPickResult::App {
            id: "org.example.Chosen.desktop".to_string(),
        })
    }
    fn prompt_secret(
        &self,
        conn_id: u64,
        title: String,
        _reason: Option<String>,
    ) -> Result<aegis_ipc::SecretPromptResult, String> {
        self.secret_prompts.lock().unwrap().push((conn_id, title));
        Ok(aegis_ipc::SecretPromptResult::Secret {
            value: "hunter2".to_string(),
        })
    }
    fn pick_confirm(
        &self,
        conn_id: u64,
        title: String,
        _body: String,
        _accept_label: Option<String>,
    ) -> Result<aegis_ipc::ConfirmPickResult, String> {
        self.confirms.lock().unwrap().push((conn_id, title));
        Ok(aegis_ipc::ConfirmPickResult::Confirmed)
    }
    fn set_wallpaper(&self, conn_id: u64, path: std::path::PathBuf) -> Result<(), String> {
        self.wallpapers.lock().unwrap().push((conn_id, path));
        Ok(())
    }
    fn capture_realm(
        &self,
        realm: aegis_core::realm::RealmId,
        region: Option<aegis_core::Rect>,
    ) -> Result<aegis_ipc::CaptureRealmPayload, String> {
        std::thread::sleep(std::time::Duration::from_millis(
            self.capture_delay_ms.load(Ordering::Relaxed),
        ));
        Ok(aegis_ipc::CaptureRealmPayload {
            capture: aegis_ipc::RealmCapture {
                realm,
                width: 2,
                height: 1,
                scale_milli: 1250,
                region: region.unwrap_or_else(|| aegis_core::Rect::new(0, 0, 2, 1)),
                placements: vec![aegis_core::realm::RealmWindowPlacement {
                    window: WindowId(1),
                    output_rect: aegis_core::Rect::new(0, 0, 2, 1),
                    surface_size: aegis_core::Size { w: 20, h: 10 },
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
            aegis_ipc::LOCAL_AGENT_ADMIN_SCOPE.into(),
            Scope {
                ops: Some(Vec::new()),
                ..Scope::default()
            },
        ),
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
            "stream".into(),
            Scope {
                ops: Some(vec![OpClass::StreamOutput]),
                ..Scope::default()
            },
        ),
        (
            "idle".into(),
            Scope {
                ops: Some(vec![OpClass::IdleInhibit]),
                ..Scope::default()
            },
        ),
        (
            "system".into(),
            Scope {
                ops: Some(vec![OpClass::SystemControl]),
                ..Scope::default()
            },
        ),
        (
            "pick".into(),
            Scope {
                ops: Some(vec![OpClass::PickTarget]),
                ..Scope::default()
            },
        ),
        (
            "app-pick".into(),
            Scope {
                ops: Some(vec![OpClass::PickApp]),
                ..Scope::default()
            },
        ),
        (
            "secret-prompt".into(),
            Scope {
                ops: Some(vec![OpClass::PromptSecret]),
                ..Scope::default()
            },
        ),
        (
            "confirm".into(),
            Scope {
                ops: Some(vec![OpClass::PickConfirm]),
                ..Scope::default()
            },
        ),
        (
            "wallpaper".into(),
            Scope {
                ops: Some(vec![OpClass::SetWallpaper]),
                ..Scope::default()
            },
        ),
        (
            "realm".into(),
            Scope {
                realms: Some(vec![
                    aegis_core::realm::HUMAN_REALM,
                    aegis_core::realm::RealmId(2),
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
fn settings_query_and_confirmed_transaction_round_trip() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_with(
        &path,
        Capabilities {
            query: true,
            session: true,
            ..Capabilities::default()
        },
    )
    .expect("connect");

    let before = client.settings().expect("settings snapshot");
    assert_eq!(before.revision, 7);
    let mut config = before.touchpad.config;
    config.natural_scroll = !config.natural_scroll;
    let action = SettingsAction::SetTouchpad { config };
    let receipt = client
        .apply_settings(Some(before.revision), action.clone())
        .expect("confirmed apply");
    assert_eq!(receipt.revision, 8);
    assert_eq!(
        handler.settings_actions.lock().unwrap().as_slice(),
        std::slice::from_ref(&action)
    );
    assert_eq!(client.settings().unwrap().touchpad.config, config);

    let preferences = aegis_core::settings::DesktopPreferences {
        color_scheme: aegis_core::settings::ColorScheme::Dark,
        icon_theme: "Papirus".into(),
        ..Default::default()
    };
    let appearance_action = SettingsAction::SetDesktopPreferences {
        preferences: preferences.clone(),
    };
    let receipt = client
        .apply_settings(Some(8), appearance_action.clone())
        .expect("confirmed desktop-preferences apply");
    assert_eq!(receipt.revision, 9);
    assert_eq!(
        handler.settings_actions.lock().unwrap().as_slice(),
        &[action.clone(), appearance_action.clone()]
    );
    assert_eq!(client.settings().unwrap().preferences, preferences);

    let idle = aegis_core::settings::IdleSettings {
        dim_after_seconds: 120,
        lock_after_seconds: 300,
        display_off_after_seconds: 360,
        suspend_after_seconds: 900,
        ..Default::default()
    };
    let idle_action = SettingsAction::SetIdle { settings: idle };
    let receipt = client
        .apply_settings(Some(9), idle_action.clone())
        .expect("confirmed idle-policy apply");
    assert_eq!(receipt.revision, 10);
    assert_eq!(
        handler.settings_actions.lock().unwrap().as_slice(),
        &[action, appearance_action, idle_action]
    );
    assert_eq!(client.settings().unwrap().idle, idle);
}

#[test]
fn system_status_query_and_control_round_trip() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let requested = Capabilities {
        query: true,
        control: true,
        ..Capabilities::default()
    };
    let mut client = Client::connect_scoped(&path, requested, "system").expect("connect");
    assert_eq!(client.scope().ops, Some(vec![OpClass::SystemControl]));

    let status = client.system_status().expect("system status");
    assert_eq!(status.volume, Some(42));
    assert_eq!(status.brightness, Some(73));

    let action = SystemAction::SetBrightness { level: 55 };
    client
        .apply_system_action(action.clone())
        .expect("apply system control");
    assert_eq!(*handler.system_actions.lock().unwrap(), [action]);
    assert!(handler.commands.lock().unwrap().is_empty());

    let mut denied =
        Client::connect_scoped(&path, requested, "focus-first").expect("restricted connect");
    let error = denied
        .apply_system_action(SystemAction::ToggleMute)
        .unwrap_err();
    assert!(error.to_string().contains("out of scope"), "{error}");
}

#[test]
fn system_control_returns_the_authoritative_apply_error() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    *handler.system_result.lock().unwrap() = Err("backend refused output power".into());
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_with(
        &path,
        Capabilities {
            query: true,
            control: true,
            ..Capabilities::default()
        },
    )
    .expect("connect");

    let error = client
        .apply_system_action(SystemAction::ToggleMute)
        .unwrap_err();
    assert!(
        error.to_string().contains("backend refused output power"),
        "{error}"
    );
    assert_eq!(
        *handler.system_actions.lock().unwrap(),
        [SystemAction::ToggleMute]
    );
}

#[test]
fn invalid_system_control_is_refused_before_dispatch() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_with(
        &path,
        Capabilities {
            query: true,
            control: true,
            ..Capabilities::default()
        },
    )
    .expect("connect");

    let error = client
        .apply_system_action(SystemAction::SetVolume { level: 101 })
        .unwrap_err();
    assert!(error.to_string().contains("outside 0..=100"), "{error}");
    assert!(handler.system_actions.lock().unwrap().is_empty());
    assert!(handler.commands.lock().unwrap().is_empty());
}

#[test]
fn settings_transaction_rejects_stale_revision() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_with(
        &path,
        Capabilities {
            query: true,
            session: true,
            ..Capabilities::default()
        },
    )
    .expect("connect");
    let error = client
        .apply_settings(
            Some(6),
            SettingsAction::SetTouchpad {
                config: aegis_core::input::TouchpadConfig::default(),
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("revision conflict"), "{error}");
    assert!(handler.settings_actions.lock().unwrap().is_empty());
}

#[test]
fn settings_mutation_requires_session_capability_and_is_audited() {
    let path = scratch();
    let handler = Arc::new(TestHandler::query(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect(&path).expect("connect");
    let error = client
        .apply_settings(
            None,
            SettingsAction::SetTouchpad {
                config: aegis_core::input::TouchpadConfig::default(),
            },
        )
        .unwrap_err();
    assert!(error.to_string().contains("session capability"), "{error}");
    let refusals = handler.refusals.lock().unwrap();
    assert!(matches!(
        refusals.as_slice(),
        [(
            _,
            aegis_ipc::JournalMutation::Settings {
                before_revision: 7,
                after_revision: 7,
                ..
            },
            _
        )]
    ));
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
    use aegis_core::input::SyntheticInputAction;

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
        position: aegis_core::Point { x: 10, y: 20 },
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

fn stream_frame(stream_id: u64, sequence: u64) -> aegis_ipc::StreamFramePayload {
    aegis_ipc::StreamFramePayload {
        stream_id,
        sequence,
        width: 2,
        height: 2,
        stride: 8,
        format: aegis_ipc::StreamPixelFormat::Bgra8,
        damage: vec![aegis_core::Rect::new(0, 0, 2, 2)],
        dropped: 0,
        pixels: Arc::from(&[7u8; 16][..]),
    }
}

#[test]
fn stream_output_start_requires_control_and_an_explicit_scope_op() {
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
    let mut scoped = Client::connect_scoped(&path, requested, "stream").expect("scoped connect");
    let started = scoped.start_output_stream(Some(30)).expect("stream starts");
    assert_eq!(started.stream_id, 1);
    assert_eq!((started.width, started.height), (2, 2));
    assert_eq!(started.format, aegis_ipc::StreamPixelFormat::Bgra8);

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

    let requested = Capabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        realm: false,
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

    let requested = Capabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        realm: false,
    };
    // Scoped with the explicit PickTarget op + control: the picked region
    // round-trips.
    let mut scoped = Client::connect_scoped(&path, requested, "pick").expect("scoped connect");
    let result = scoped
        .pick_target(aegis_ipc::PickKind::Region)
        .expect("pick succeeds");
    assert_eq!(
        result,
        aegis_ipc::PickResult::Region {
            rect: aegis_core::Rect::new(1, 2, 30, 40)
        }
    );
    assert_eq!(
        handler.picks.lock().unwrap().as_slice(),
        &[(1, aegis_ipc::PickKind::Region)]
    );

    // A scope without the op is refused even though it has control.
    let mut focus_scope =
        Client::connect_scoped(&path, requested, "focus-first").expect("scoped connect");
    let err = focus_scope
        .pick_target(aegis_ipc::PickKind::Pixel)
        .unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Unscoped connections never inherit the op (fail-closed, like input).
    let mut unscoped = Client::connect_with(&path, requested).expect("unscoped connect");
    let err = unscoped
        .pick_target(aegis_ipc::PickKind::Window)
        .unwrap_err();
    assert!(err.to_string().contains("out of scope"), "{err}");

    // Without the control capability the request is refused earlier.
    let mut query_only = Client::connect(&path).expect("query connect");
    let err = query_only
        .pick_target(aegis_ipc::PickKind::Region)
        .unwrap_err();
    assert!(err.to_string().contains("control capability"), "{err}");

    // A locked/inactive session refuses before any chrome opens.
    handler
        .capture_security_active
        .store(false, Ordering::Release);
    let mut scoped = Client::connect_scoped(&path, requested, "pick").expect("scoped connect");
    let err = scoped.pick_target(aegis_ipc::PickKind::Region).unwrap_err();
    assert!(err.to_string().contains("locked or inactive"), "{err}");
}

#[test]
fn stream_output_start_forwards_a_window_target() {
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
    let mut scoped = Client::connect_scoped(&path, requested, "stream").expect("scoped connect");
    scoped
        .start_output_stream_target(
            Some(30),
            aegis_ipc::StreamTarget::Window {
                window: WindowId(2),
            },
        )
        .expect("window stream starts");
    assert_eq!(
        handler.stream_targets.lock().unwrap().as_slice(),
        &[aegis_ipc::StreamTarget::Window {
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
            aegis_ipc::StreamTarget::Window {
                window: WindowId(2)
            },
            aegis_ipc::StreamTarget::Output,
        ]
    );
}

#[test]
fn disconnecting_releases_a_held_idle_inhibitor() {
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
    let requested = Capabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        realm: false,
    };
    let mut client = Client::connect_scoped(&path, requested, "stream").expect("connect");
    let started = client.start_output_stream(None).expect("stream starts");

    // A pushed frame arrives as metadata + sealed pixel memfd.
    assert!(server.push_stream_frame(stream_frame(started.stream_id, 1)));
    let message = client.next_stream_message().expect("frame arrives");
    let aegis_ipc::StreamMessage::Frame(frame) = message else {
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
    let requested = Capabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        realm: false,
    };
    let mut client = Client::connect_scoped(&path, requested, "stream").expect("connect");
    let started = client.start_output_stream(None).expect("stream starts");

    // Revoke the scope: the writer's per-frame re-check ends the stream
    // instead of attaching pixels (ADR-0052).
    handler.scopes.lock().unwrap().remove("stream");
    assert!(server.push_stream_frame(stream_frame(started.stream_id, 1)));
    let message = client.next_stream_message().expect("stream end arrives");
    let aegis_ipc::StreamMessage::Ended { stream_id, reason } = message else {
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
    let requested = Capabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        realm: false,
    };
    let mut client = Client::connect_scoped(&path, requested, "stream").expect("connect");
    client.start_output_stream(None).expect("stream starts");
    client.request_lease_renewal(900_000).expect("renewal sent");
    let message = client.next_stream_message().expect("renewal reply");
    assert_eq!(message, aegis_ipc::StreamMessage::LeaseRenewed);
}

#[test]
fn disconnecting_stops_owned_streams() {
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
            capabilities: aegis_core::realm::SeatCapabilities::POINTER_KEYBOARD,
            output: Some(aegis_core::realm::VirtualOutput::DEFAULT_AGENT),
        })
        .expect("create realm");
    assert!(matches!(
        result,
        RealmActionResult::Created {
            bundle: aegis_core::realm::RealmBundle {
                realm: aegis_core::realm::RealmId(2),
                ..
            },
            ..
        }
    ));
    let capture = client
        .capture_realm(aegis_core::realm::RealmId(2), None)
        .expect("realm capture");
    assert_eq!((capture.width, capture.height, capture.revision), (2, 1, 4));
    assert_eq!(capture.scale_milli, 1250);
    assert_eq!(capture.region, aegis_core::Rect::new(0, 0, 2, 1));
    assert_eq!(
        capture.placements,
        vec![aegis_core::realm::RealmWindowPlacement {
            window: WindowId(1),
            output_rect: aegis_core::Rect::new(0, 0, 2, 1),
            surface_size: aegis_core::Size { w: 20, h: 10 },
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
            capabilities: aegis_core::realm::SeatCapabilities::POINTER_KEYBOARD,
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
fn scoped_timeout_connection_completes_handshake() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(sample_windows()));
    let _server = Server::start(&path, handler).expect("bind");

    let client = Client::connect_scoped_with_timeout(
        &path,
        Capabilities::QUERY,
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

    let client = Client::connect_with_timeout(&path, Capabilities::QUERY, Duration::from_secs(1))
        .expect("connect with timeout");

    assert!(client.caps().query);
    assert_eq!(client.scope(), &Scope::unscoped());
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
            capabilities: aegis_core::realm::SeatCapabilities::POINTER_KEYBOARD,
            output: None,
        })
        .expect_err("removed scope must revoke Realm mutation");
    assert!(
        action_error.to_string().contains("out of scope"),
        "{action_error}"
    );
    let capture_error = client
        .capture_realm(aegis_core::realm::RealmId(2), None)
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
    use aegis_ipc::Request;
    let path = scratch();
    let handler = Arc::new(TestHandler::query(vec![]));
    let _server = Server::start(&path, handler).expect("bind");

    let mut s = std::os::unix::net::UnixStream::connect(&path).unwrap();
    let bad = Request::Hello {
        version: PROTOCOL_VERSION + 1,
        caps: Capabilities::QUERY,
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
        aegis_ipc::JournalMutation::Command {
            cmd: Command::Close { id: WindowId(1) }
        }
    ));
}

#[test]
fn session_command_quit_is_accepted() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let mut client = Client::connect_scoped(
        &path,
        Capabilities {
            query: true,
            control: false,
            input: false,
            session: true,
            realm: false,
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
        .switch_workspace(aegis_core::workspace::Switch::Next)
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
    let n = aegis_core::notify::Notification {
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

#[test]
fn pick_app_requires_control_and_an_explicit_scope_op() {
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
    let choices = vec!["org.example.A.desktop".to_string()];
    // Scoped with the explicit PickApp op + control: the chosen id
    // round-trips.
    let mut scoped = Client::connect_scoped(&path, requested, "app-pick").expect("scoped connect");
    let result = scoped
        .pick_app(choices.clone(), None, None)
        .expect("app pick succeeds");
    assert_eq!(
        result,
        aegis_ipc::AppPickResult::App {
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
fn prompt_secret_requires_control_and_an_explicit_scope_op() {
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
    // Scoped with the explicit PromptSecret op + control: the secret
    // round-trips.
    let mut scoped =
        Client::connect_scoped(&path, requested, "secret-prompt").expect("scoped connect");
    let result = scoped
        .prompt_secret("Unlock".to_string(), None)
        .expect("prompt succeeds");
    assert_eq!(
        result,
        aegis_ipc::SecretPromptResult::Secret {
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

    let requested = Capabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        realm: false,
    };
    // Scoped with the explicit PickConfirm op + control: the answer
    // round-trips.
    let mut scoped = Client::connect_scoped(&path, requested, "confirm").expect("scoped connect");
    let result = scoped
        .pick_confirm("Share?".to_string(), "body".to_string(), None)
        .expect("confirm succeeds");
    assert_eq!(result, aegis_ipc::ConfirmPickResult::Confirmed);
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

    let requested = Capabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        realm: false,
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
fn pairing_issues_credential_and_synthetic_scope() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    *handler.pair_result.lock().unwrap() = Ok(aegis_ipc::PairedAgent {
        principal: "prin_1".into(),
        credential: "cred_1".into(),
        pregranted: vec![OpClass::Focus],
        gated: vec![OpClass::CaptureRealm],
    });
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let client = Client::connect_agent_with_timeout(
        &path,
        Capabilities {
            query: true,
            control: true,
            input: true,
            session: false,
            realm: true,
        },
        None,
        aegis_ipc::AgentHello {
            label: Some("Codex".into()),
            requested: vec![OpClass::Focus, OpClass::CaptureRealm],
            credential: None,
        },
        Duration::from_secs(5),
    )
    .expect("pairing connects");
    let issued = client.agent_issued().expect("pairing outcome");
    assert_eq!(issued.principal, "prin_1");
    assert_eq!(issued.credential.as_deref(), Some("cred_1"));
    assert_eq!(client.scope().ops, Some(vec![OpClass::Focus]));
    assert_eq!(client.scope().ask_ops, Some(vec![OpClass::CaptureRealm]));
    assert!(
        client.caps().input,
        "a paired agent keeps the input capability class"
    );
    let calls = handler.pair_calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].1.as_deref(), Some("Codex"));
    assert_eq!(calls[0].2, vec![OpClass::Focus, OpClass::CaptureRealm]);
}

#[test]
fn pairing_denial_refuses_the_handshake() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    *handler.pair_result.lock().unwrap() = Err("the user declined".into());
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let error = match Client::connect_agent_with_timeout(
        &path,
        Capabilities::QUERY,
        None,
        aegis_ipc::AgentHello {
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
fn recognized_credential_binds_without_pairing() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    *handler.lookup_result.lock().unwrap() = Some(aegis_ipc::AgentIdentity {
        principal: "prin_9".into(),
        pregranted: vec![OpClass::Notify],
        gated: vec![],
    });
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let client = Client::connect_agent_with_timeout(
        &path,
        Capabilities::QUERY,
        None,
        aegis_ipc::AgentHello {
            label: Some("Codex".into()),
            requested: vec![OpClass::Notify],
            credential: Some("cred_9".into()),
        },
        Duration::from_secs(5),
    )
    .expect("recognized credential connects");
    assert!(handler.pair_calls.lock().unwrap().is_empty());
    let issued = client.agent_issued().expect("pairing outcome");
    assert_eq!(issued.principal, "prin_9");
    assert!(issued.credential.is_none());
    assert_eq!(client.scope().ops, Some(vec![OpClass::Notify]));
    assert_eq!(client.scope().ask_ops, Some(vec![]));
}

#[test]
fn builtin_scope_connections_do_not_pair() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    handler.scopes.lock().unwrap().insert(
        aegis_ipc::LOCAL_REALM_ADMIN_SCOPE.to_string(),
        Scope {
            ops: Some(vec![OpClass::Notify]),
            ..Scope::default()
        },
    );
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let client = Client::connect_agent_with_timeout(
        &path,
        Capabilities::QUERY,
        Some(aegis_ipc::LOCAL_REALM_ADMIN_SCOPE.to_string()),
        aegis_ipc::AgentHello {
            label: Some("aegis".into()),
            requested: vec![],
            credential: None,
        },
        Duration::from_secs(5),
    )
    .expect("built-in scope connects without pairing");
    assert!(handler.pair_calls.lock().unwrap().is_empty());
    assert!(client.agent_issued().is_none());
    assert_eq!(client.scope().ops, Some(vec![OpClass::Notify]));
}

#[test]
fn declared_scope_pairs_but_keeps_the_configured_ceiling() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    *handler.pair_result.lock().unwrap() = Ok(aegis_ipc::PairedAgent {
        principal: "prin_2".into(),
        credential: "cred_2".into(),
        pregranted: vec![OpClass::Close],
        gated: vec![],
    });
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let client = Client::connect_agent_with_timeout(
        &path,
        Capabilities::QUERY,
        Some("focus-first".to_string()),
        aegis_ipc::AgentHello {
            label: Some("Codex".into()),
            requested: vec![OpClass::Close],
            credential: None,
        },
        Duration::from_secs(5),
    )
    .expect("declared scope connects");
    // The ceiling stays the configured scope, not the registry split.
    assert_eq!(client.scope().ops, Some(vec![OpClass::Focus]));
    assert_eq!(client.scope().windows, Some(vec![WindowId(1)]));
    assert!(client.agent_issued().is_some());
    assert_eq!(handler.pair_calls.lock().unwrap().len(), 1);
}

#[test]
fn lockdown_strips_privileges_from_unpaired_connections() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let privileged = Capabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        realm: false,
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
    pregranted: Vec<OpClass>,
    gated: Vec<OpClass>,
) -> (Arc<TestHandler>, PathBuf) {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    *handler.pair_result.lock().unwrap() = Ok(aegis_ipc::PairedAgent {
        principal: "prin_g".into(),
        credential: "cred_g".into(),
        pregranted,
        gated,
    });
    (handler, path)
}

fn grant_client(path: &std::path::Path, caps: Capabilities) -> Client {
    Client::connect_agent_with_timeout(
        path,
        caps,
        None,
        aegis_ipc::AgentHello {
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
    let (handler, path) = grant_paired_handler(vec![OpClass::Focus], vec![OpClass::Close]);
    *handler.request_grant_result.lock().unwrap() = Ok(true);
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = grant_client(
        &path,
        Capabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            realm: false,
        },
    );

    client
        .command(Command::Close { id: WindowId(1) })
        .expect("granted command proceeds");
    let grant_calls = handler.grant_calls.lock().unwrap();
    assert_eq!(grant_calls.len(), 1);
    assert_eq!(grant_calls[0].1, "prin_g");
    assert_eq!(grant_calls[0].2, OpClass::Close);
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
    let (handler, path) = grant_paired_handler(vec![OpClass::Focus], vec![OpClass::Close]);
    *handler.request_grant_result.lock().unwrap() = Ok(false);
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = grant_client(
        &path,
        Capabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            realm: false,
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
    let (handler, path) = grant_paired_handler(vec![OpClass::Focus], vec![OpClass::Close]);
    handler
        .grants
        .lock()
        .unwrap()
        .push(("prin_g".into(), OpClass::Close, true));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = grant_client(
        &path,
        Capabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            realm: false,
        },
    );

    client
        .command(Command::Close { id: WindowId(1) })
        .expect("recorded grant proceeds");
    assert!(handler.grant_calls.lock().unwrap().is_empty());
}

#[test]
fn recorded_denial_refuses_without_prompting() {
    let (handler, path) = grant_paired_handler(vec![OpClass::Focus], vec![OpClass::Close]);
    handler
        .grants
        .lock()
        .unwrap()
        .push(("prin_g".into(), OpClass::Close, false));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = grant_client(
        &path,
        Capabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            realm: false,
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
            ops: Some(vec![OpClass::Focus]),
            ask_ops: Some(vec![OpClass::Close]),
            ..Scope::default()
        },
    );
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let mut client = Client::connect_scoped(
        &path,
        Capabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            realm: false,
        },
        "ask-close",
    )
    .expect("declared scope connects");
    client
        .command(Command::Focus { id: WindowId(1) })
        .expect("pregranted operations work without pairing");
    let error = client
        .command(Command::Close { id: WindowId(1) })
        .expect_err("askable operations require a paired agent");
    assert!(error.to_string().contains("paired agent"));
}

#[test]
fn realm_action_proceeds_through_the_grant_path() {
    let (handler, path) = grant_paired_handler(vec![], vec![OpClass::CreateRealm]);
    *handler.request_grant_result.lock().unwrap() = Ok(true);
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = grant_client(
        &path,
        Capabilities {
            query: true,
            control: false,
            input: false,
            session: false,
            realm: true,
        },
    );

    let result = client
        .realm_action(RealmAction::Create {
            label: "agent".into(),
            capabilities: aegis_core::realm::SeatCapabilities::POINTER_KEYBOARD,
            output: None,
        })
        .expect("granted realm action");
    assert!(matches!(result, RealmActionResult::Created { .. }));
    assert_eq!(handler.realm_actions.lock().unwrap().len(), 1);
}

#[test]
fn agent_management_round_trips() {
    let path = scratch();
    let handler = Arc::new(TestHandler::permissive(vec![]));
    handler
        .principal_infos
        .lock()
        .unwrap()
        .push(aegis_ipc::AgentPrincipalInfo {
            principal: "prin_1".into(),
            label: Some("Codex".into()),
            pregranted: vec![OpClass::Focus],
            gated: vec![OpClass::Close],
            created_at: 1,
        });
    handler.grant_infos.lock().unwrap().extend([
        aegis_ipc::AgentGrantInfo {
            principal: "prin_1".into(),
            op: OpClass::Close,
            decision: aegis_ipc::AgentGrantDecision::Allow,
            granted_at: 2,
        },
        aegis_ipc::AgentGrantInfo {
            principal: "prin_2".into(),
            op: OpClass::Notify,
            decision: aegis_ipc::AgentGrantDecision::Deny,
            granted_at: 3,
        },
    ]);
    *handler.register_result.lock().unwrap() = Ok(("prin_9".into(), "cred_9".into()));
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");
    let mut client = Client::connect_scoped(
        &path,
        Capabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            realm: false,
        },
        aegis_ipc::LOCAL_AGENT_ADMIN_SCOPE,
    )
    .expect("connect");

    let principals = client.agent_principals().expect("principals");
    assert_eq!(principals.len(), 1);
    assert_eq!(principals[0].label.as_deref(), Some("Codex"));
    assert_eq!(principals[0].gated, vec![OpClass::Close]);

    assert_eq!(client.agent_grants(None).expect("grants").len(), 2);
    let filtered = client.agent_grants(Some("prin_1")).expect("filtered");
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].op, OpClass::Close);

    client
        .rename_agent_principal("prin_1", Some("New name"))
        .expect("rename");
    client.forget_agent_principal("prin_2").expect("forget");
    client
        .set_agent_ceiling(
            "prin_1",
            vec![OpClass::Focus, OpClass::Notify],
            vec![OpClass::Close],
        )
        .expect("ceiling");
    let (principal, credential) = client
        .register_agent(Some("Fleet"), vec![OpClass::Focus], vec![])
        .expect("register");
    assert_eq!(
        (principal.as_str(), credential.as_str()),
        ("prin_9", "cred_9")
    );
    client
        .revoke_agent_grant("prin_1", OpClass::Close)
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
        Capabilities::QUERY,
        aegis_ipc::LOCAL_AGENT_ADMIN_SCOPE,
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
        aegis_ipc::LOCAL_PORTAL_SCOPE.to_string(),
        Scope {
            ops: Some(vec![OpClass::Notify]),
            ..Scope::default()
        },
    );
    let _server = Server::start(&path, Arc::clone(&handler)).expect("bind");

    let privileged = Capabilities {
        query: true,
        control: true,
        input: false,
        session: false,
        realm: false,
    };
    let portal = Client::connect_scoped(&path, privileged, aegis_ipc::LOCAL_PORTAL_SCOPE)
        .expect("built-in scope connects under lockdown");
    assert!(
        portal.caps().control,
        "built-in platform components keep privileges under lockdown"
    );
}
