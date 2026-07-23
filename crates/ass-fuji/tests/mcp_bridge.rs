use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ass_core::realm::{HUMAN_REALM, RealmModel};
use ass_core::window::{Window, WindowId};
use ass_ipc::{
    Capabilities, Effect, Handler, Journal, JournalMutation, OpClass, Origin, RealmAction,
    RealmActionResult, Scope, Server,
};
use serde_json::{Value, json};

fn scratch() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("ass-fuji-bridge-{}-{n}", std::process::id()))
}

struct TestHandler {
    realms: Mutex<RealmModel>,
    journal: Mutex<Journal>,
    notifications: Mutex<Vec<ass_core::notify::Notification>>,
    scope: Scope,
}

impl Handler for TestHandler {
    fn policy_caps(&self) -> Capabilities {
        Capabilities {
            query: true,
            control: true,
            input: true,
            session: false,
            realm: true,
        }
    }

    fn windows(&self) -> Vec<Window> {
        let realms = self.realms.lock().expect("realm lock");
        let mut window = Window::new(WindowId(7));
        window.size = ass_core::Size { w: 320, h: 180 };
        window.read_only = realms
            .interaction_group_for_window(window.id)
            .is_some_and(|group| group.control_realm != HUMAN_REALM);
        vec![window]
    }

    fn workspaces(&self) -> ass_core::workspace::WorkspaceSnapshot {
        ass_core::workspace::WorkspaceSnapshot { outputs: vec![] }
    }

    fn notifications(&self) -> Vec<ass_core::notify::Notification> {
        self.notifications
            .lock()
            .expect("notification lock")
            .clone()
    }

    fn outputs(&self) -> Vec<ass_core::output::OutputInfo> {
        vec![]
    }

    fn journal_since(&self, since: u64) -> ass_ipc::JournalSnapshot {
        self.journal.lock().expect("journal lock").since(since)
    }

    fn command(&self, conn_id: u64, cmd: ass_ipc::Command) {
        if let ass_ipc::Command::Notify {
            summary,
            body,
            app_id,
        } = &cmd
        {
            let mut notifications = self.notifications.lock().expect("notification lock");
            let id = notifications.len() as u64;
            notifications.push(ass_core::notify::Notification {
                id,
                summary: summary.clone(),
                body: body.clone(),
                app_id: app_id.clone(),
                at_ms: id,
            });
        }
        self.journal.lock().expect("journal lock").append(
            0,
            Origin::Ipc { conn_id },
            JournalMutation::Command { cmd },
            Effect::Applied,
        );
    }

    fn realms(&self) -> ass_core::realm::RealmSnapshot {
        self.realms.lock().expect("realm lock").snapshot()
    }

    fn realm_action(
        &self,
        _conn_id: u64,
        action: RealmAction,
    ) -> Result<RealmActionResult, String> {
        let mut realms = self.realms.lock().expect("realm lock");
        match action {
            RealmAction::Create {
                label,
                capabilities,
                output,
            } => {
                let bundle = realms.create_agent_realm(label, capabilities);
                if let Some(output) = output {
                    realms
                        .configure_virtual_output(bundle.realm, output)
                        .map_err(|error| error.to_string())?;
                }
                Ok(RealmActionResult::Created {
                    bundle: ass_core::realm::RealmBundle {
                        revision: realms.revision(),
                        ..bundle
                    },
                })
            }
            RealmAction::Transact {
                expected_revision,
                mutations,
            } => realms
                .transact(expected_revision, &mutations)
                .map(|receipt| RealmActionResult::TransactionCommitted { receipt })
                .map_err(|error| error.to_string()),
            RealmAction::Revoke {
                realm,
                fallback,
                expected_revision,
            } => {
                if expected_revision.is_some_and(|expected| expected != realms.revision()) {
                    return Err("revision conflict".into());
                }
                realms
                    .revoke_realm(realm, fallback)
                    .map(|receipt| RealmActionResult::Revoked { receipt })
                    .map_err(|error| error.to_string())
            }
        }
    }

    fn resolve_scope(&self, name: &str) -> Option<Scope> {
        (name == "fuji-test").then(|| self.scope.clone())
    }

    fn capture_security_active(&self) -> bool {
        true
    }

    fn capture_realm(
        &self,
        realm: ass_core::realm::RealmId,
        _region: Option<ass_core::Rect>,
    ) -> Result<ass_ipc::CaptureRealmPayload, String> {
        Ok(ass_ipc::CaptureRealmPayload {
            capture: ass_ipc::RealmCapture {
                realm,
                width: 1,
                height: 1,
                scale_milli: 1000,
                region: ass_core::Rect::new(0, 0, 1, 1),
                placements: vec![],
                png_bytes: 8,
                revision: self.realms.lock().expect("realm lock").revision(),
            },
            // Signature prefix is sufficient for transport testing; image
            // decoding belongs to capture/render integration tests.
            png: b"\x89PNG\r\n\x1a\n".to_vec(),
        })
    }
}

#[test]
fn fuji_stdio_discovers_manages_captures_and_revokes_realm() {
    let runtime_dir = scratch();
    std::fs::create_dir_all(&runtime_dir).expect("runtime dir");
    let socket = runtime_dir.join("ass.sock");
    let scope = Scope {
        ops: Some(vec![
            OpClass::CreateRealm,
            OpClass::TransactRealm,
            OpClass::RevokeRealm,
            OpClass::CaptureRealm,
            OpClass::LaunchInRealm,
            OpClass::InjectRealmInput,
            OpClass::Notify,
        ]),
        ..Scope::default()
    };
    let mut realms = RealmModel::new();
    let client = realms.register_client(Some("visual-smoke.test".into()));
    realms
        .create_interaction_group(client, &[WindowId(7)], HUMAN_REALM)
        .expect("human-controlled smoke window");
    let handler = Arc::new(TestHandler {
        realms: Mutex::new(realms),
        journal: Mutex::new(Journal::default_capacity()),
        notifications: Mutex::new(vec![]),
        scope,
    });
    let server = Server::start(&socket, Arc::clone(&handler)).expect("server");

    let mut child = Command::new(env!("CARGO_BIN_EXE_ass-fuji-mcp"))
        .arg("serve")
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("ASS_FUJI_SCOPE", "fuji-test")
        .env("ASS_FUJI_REALM_LABEL", "Fuji integration test")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn bridge");
    let requests = [
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
        json!({"jsonrpc":"2.0","method":"notifications/initialized","params":{}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"realm_ensure","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"realm_capture","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"realm_reset","arguments":{}}}),
        json!({"jsonrpc":"2.0","id":6,"method":"shutdown","params":{}}),
    ];
    {
        let stdin = child.stdin.as_mut().expect("child stdin");
        for request in requests {
            writeln!(stdin, "{request}").expect("request");
        }
    }
    drop(child.stdin.take());
    let output = child.wait_with_output().expect("bridge output");
    assert!(
        output.status.success(),
        "bridge stderr: {}",
        String::from_utf8_lossy(&output.stderr)
    );

    let responses = String::from_utf8(output.stdout)
        .expect("utf8")
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).expect("response json"))
        .collect::<Vec<_>>();
    assert_eq!(responses.len(), 6);
    let names = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"realm_capture"));
    assert!(names.contains(&"realm_input"));
    assert_eq!(responses[2]["result"]["isError"], false);
    assert_eq!(responses[3]["result"]["content"][1]["type"], "image");
    assert_eq!(responses[4]["result"]["isError"], false);
    assert!(
        handler
            .realms
            .lock()
            .expect("realm lock")
            .snapshot()
            .realms
            .iter()
            .any(|realm| realm.state == ass_core::realm::RealmState::Revoked)
    );

    let config =
        ass_fuji::bridge::BridgeConfig::new(&socket, &runtime_dir, "fuji-test", "Fuji smoke test")
            .expect("smoke config");
    let mut platform = ass_fuji::bridge::AssPlatform::connect(config).expect("smoke platform");
    let report = platform
        .smoke_with_input(Duration::ZERO, Some(WindowId(7)))
        .expect("live input smoke");
    assert_eq!(report.status, "passed");
    assert!(report.notification.observed_in_compositor_state);
    assert_ne!(report.notification.started_id, report.notification.id);
    assert_eq!(
        handler
            .notifications
            .lock()
            .expect("notification lock")
            .len(),
        2
    );
    assert_eq!(
        report.realm.lifecycle,
        ["active", "paused", "active", "revoked"]
    );
    assert_eq!(report.realm.cleanup, "revoked_test_realm");
    let input = report.visual.input_probe.expect("input probe evidence");
    assert_eq!(input.window_id, 7);
    assert_eq!(input.action, "pointer_move");
    assert_eq!(input.local_position, ass_core::Point { x: 160, y: 90 });
    assert!(input.applied);
    assert!(input.window_restored_to_human);
    assert_eq!(
        handler
            .realms
            .lock()
            .expect("realm lock")
            .interaction_group_for_window(WindowId(7))
            .expect("window group")
            .control_realm,
        HUMAN_REALM
    );

    drop(server);
    std::fs::remove_dir_all(&runtime_dir).expect("remove runtime dir");
}
