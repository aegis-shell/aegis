use std::collections::HashMap;
use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tessera_ipc::{
    ActorCapability, ConnectionCapabilities, Effect, Handler, InteractionDomainAction,
    InteractionDomainActionResult, Journal, JournalMutation, Origin, Server,
};
use tessera_model::interaction_domain::{HUMAN_INTERACTION_DOMAIN, InteractionDomainModel};
use tessera_model::window::{Window, WindowId};
use serde_json::{Value, json};

fn scratch() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("tessera-mcp-bridge-{}-{n}", std::process::id()))
}

struct TestHandler {
    interaction_domains: Mutex<InteractionDomainModel>,
    journal: Mutex<Journal>,
    notifications: Mutex<Vec<tessera_model::notify::Notification>>,
    observations: Mutex<HashMap<String, u64>>,
}

impl Handler for TestHandler {
    fn pair_agent(
        &self,
        _conn_id: u64,
        _label: Option<&str>,
        _requested: &[ActorCapability],
    ) -> Result<tessera_ipc::PairedAgent, String> {
        Ok(tessera_ipc::PairedAgent {
            principal: tessera_ipc::ActorPrincipal::new("prin_test").unwrap(),
            credential: "cred_test".into(),
            pregranted: vec![
                ActorCapability::ObserveWindows,
                ActorCapability::ObserveWorkspaces,
                ActorCapability::ObserveOutputs,
                ActorCapability::ObserveNotifications,
                ActorCapability::ObserveJournal,
                ActorCapability::ObserveInteractionDomains,
                ActorCapability::Focus,
                ActorCapability::CreateInteractionDomain,
                ActorCapability::TransactInteractionDomain,
                ActorCapability::RevokeInteractionDomain,
                ActorCapability::CaptureInteractionDomain,
                ActorCapability::CaptureWindow,
                ActorCapability::ObserveInteractionDomain,
                ActorCapability::LaunchInInteractionDomain,
                ActorCapability::LaunchApp,
                ActorCapability::InjectInteractionDomainInput,
                ActorCapability::Focus,
                ActorCapability::Notify,
            ],
            gated: vec![],
        })
    }

    fn policy_caps(&self) -> ConnectionCapabilities {
        ConnectionCapabilities {
            query: true,
            control: true,
            input: true,
            session: false,
            interaction_domain: true,
        }
    }

    fn windows(&self) -> Vec<Window> {
        let interaction_domains = self
            .interaction_domains
            .lock()
            .expect("interaction_domain lock");
        let mut window = Window::new(WindowId(7));
        window.size = tessera_model::Size { w: 320, h: 180 };
        window.read_only = interaction_domains
            .interaction_group_for_window(window.id)
            .is_some_and(|group| group.control_interaction_domain != HUMAN_INTERACTION_DOMAIN);
        vec![window]
    }

    fn workspaces(&self) -> tessera_model::workspace::WorkspaceSnapshot {
        tessera_model::workspace::WorkspaceSnapshot { outputs: vec![] }
    }

    fn notifications(&self) -> Vec<tessera_model::notify::Notification> {
        self.notifications
            .lock()
            .expect("notification lock")
            .clone()
    }

    fn outputs(&self) -> Vec<tessera_model::output::OutputInfo> {
        vec![]
    }

    fn journal_since(&self, since: u64) -> tessera_ipc::JournalSnapshot {
        self.journal.lock().expect("journal lock").since(since)
    }

    fn command(&self, conn_id: u64, subject: Option<&str>, cmd: tessera_ipc::Command) {
        if let tessera_ipc::Command::Notify {
            summary,
            body,
            app_id,
            external_id: _,
        } = &cmd
        {
            let mut notifications = self.notifications.lock().expect("notification lock");
            let id = notifications.len() as u64;
            notifications.push(tessera_model::notify::Notification {
                id,
                summary: summary.clone(),
                body: body.clone(),
                app_id: app_id.clone(),
                external_id: None,
                at_ms: id,
            });
        }
        self.journal.lock().expect("journal lock").append(
            0,
            Origin::ipc(conn_id, subject),
            JournalMutation::Command {
                cmd: tessera_ipc::AuditedCommand::from(&cmd),
            },
            Effect::Applied,
        );
    }

    fn transact(
        &self,
        conn_id: u64,
        subject: Option<&str>,
        expected_journal_seq: Option<u64>,
        _expected_interaction_domain_revision: Option<u64>,
        ops: Vec<tessera_ipc::Command>,
    ) -> Result<tessera_ipc::TransactResult, String> {
        let mut journal = self.journal.lock().expect("journal lock");
        let before_seq = journal.latest_seq();
        if let Some(expected) = expected_journal_seq
            && expected != before_seq
        {
            return Ok(tessera_ipc::TransactResult::PreconditionConflict {
                precondition: tessera_ipc::TransactPrecondition::JournalSeq,
                expected,
                actual: before_seq,
            });
        }
        let mut after_seq = before_seq;
        let mut results = Vec::with_capacity(ops.len());
        for cmd in ops {
            if let tessera_ipc::Command::Notify {
                summary,
                body,
                app_id,
                external_id: _,
            } = &cmd
            {
                let mut notifications = self.notifications.lock().expect("notification lock");
                let id = notifications.len() as u64;
                notifications.push(tessera_model::notify::Notification {
                    id,
                    summary: summary.clone(),
                    body: body.clone(),
                    app_id: app_id.clone(),
                    external_id: None,
                    at_ms: id,
                });
            }
            let entry = journal.append(
                0,
                Origin::ipc(conn_id, subject),
                JournalMutation::Command {
                    cmd: tessera_ipc::AuditedCommand::from(&cmd),
                },
                Effect::Applied,
            );
            after_seq = entry.seq;
            results.push(tessera_ipc::TransactOpResult {
                seq: entry.seq,
                effect: entry.effect,
            });
        }
        Ok(tessera_ipc::TransactResult::Committed {
            receipt: tessera_ipc::TransactReceipt {
                before_seq,
                after_seq,
                results,
            },
        })
    }

    fn interaction_domains(&self) -> tessera_model::interaction_domain::InteractionDomainSnapshot {
        self.interaction_domains
            .lock()
            .expect("interaction_domain lock")
            .snapshot()
    }

    fn interaction_domain_action(
        &self,
        _conn_id: u64,
        subject: Option<&str>,
        action: InteractionDomainAction,
    ) -> Result<InteractionDomainActionResult, String> {
        let mut interaction_domains = self
            .interaction_domains
            .lock()
            .expect("interaction_domain lock");
        match action {
            InteractionDomainAction::Create {
                label,
                capabilities,
                output,
            } => {
                let bundle = interaction_domains.create_agent_interaction_domain_for_subject(
                    label,
                    capabilities,
                    subject.map(str::to_owned),
                );
                if let Some(output) = output {
                    interaction_domains
                        .configure_virtual_output(bundle.interaction_domain, output)
                        .map_err(|error| error.to_string())?;
                }
                Ok(InteractionDomainActionResult::Created {
                    bundle: tessera_model::interaction_domain::InteractionDomainBundle {
                        revision: interaction_domains.revision(),
                        ..bundle
                    },
                })
            }
            InteractionDomainAction::Transact {
                expected_revision,
                mutations,
            } => interaction_domains
                .transact(expected_revision, &mutations)
                .map(|receipt| InteractionDomainActionResult::TransactionCommitted { receipt })
                .map_err(|error| error.to_string()),
            InteractionDomainAction::Revoke {
                interaction_domain,
                fallback,
                expected_revision,
            } => {
                if expected_revision
                    .is_some_and(|expected| expected != interaction_domains.revision())
                {
                    return Err("revision conflict".into());
                }
                interaction_domains
                    .revoke_interaction_domain(interaction_domain, fallback)
                    .map(|receipt| InteractionDomainActionResult::Revoked { receipt })
                    .map_err(|error| error.to_string())
            }
        }
    }

    fn capture_security_active(&self) -> bool {
        true
    }

    fn capture_interaction_domain(
        &self,
        conn_id: u64,
        _subject: Option<&str>,
        interaction_domain: tessera_model::interaction_domain::InteractionDomainId,
        _region: Option<tessera_model::Rect>,
    ) -> Result<tessera_ipc::CaptureInteractionDomainPayload, String> {
        let revision = self
            .interaction_domains
            .lock()
            .expect("interaction_domain lock")
            .revision();
        self.observations
            .lock()
            .expect("observation lock")
            .insert("a".repeat(64), conn_id);
        Ok(tessera_ipc::CaptureInteractionDomainPayload {
            capture: tessera_ipc::InteractionDomainCapture {
                interaction_domain,
                width: 1,
                height: 1,
                scale_milli: 1000,
                region: tessera_model::Rect::new(0, 0, 1, 1),
                placements: vec![],
                observation: tessera_ipc::SemanticObservation {
                    token: tessera_ipc::ObservationToken("a".repeat(64)),
                    ttl_ms: 15_000,
                    snapshot: tessera_model::semantic::SemanticSnapshot {
                        interaction_domain,
                        authority_revision: revision,
                        objects: Vec::new(),
                    },
                },
                png_bytes: 8,
                revision,
            },
            // Signature prefix is sufficient for transport testing; image
            // decoding belongs to capture/render integration tests.
            png: b"\x89PNG\r\n\x1a\n".to_vec(),
        })
    }

    fn capture_window(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        window: WindowId,
    ) -> Result<tessera_ipc::CaptureWindowPayload, String> {
        Ok(tessera_ipc::CaptureWindowPayload {
            capture: tessera_ipc::WindowCapture {
                window,
                width: 1,
                height: 1,
                scale_milli: 1000,
                rect: tessera_model::Rect::new(0, 0, 1, 1),
                png_bytes: 8,
            },
            png: b"\x89PNG\r\n\x1a\n".to_vec(),
        })
    }

    fn observe_interaction_domain(
        &self,
        conn_id: u64,
        _subject: Option<&str>,
        interaction_domain: tessera_model::interaction_domain::InteractionDomainId,
    ) -> Result<tessera_ipc::SemanticObservation, String> {
        let revision = self
            .interaction_domains
            .lock()
            .expect("interaction_domain lock")
            .revision();
        self.observations
            .lock()
            .expect("observation lock")
            .insert("b".repeat(64), conn_id);
        Ok(tessera_ipc::SemanticObservation {
            token: tessera_ipc::ObservationToken("b".repeat(64)),
            ttl_ms: 15_000,
            snapshot: tessera_model::semantic::SemanticSnapshot {
                interaction_domain,
                authority_revision: revision,
                objects: vec![tessera_model::semantic::SemanticObject {
                    id: tessera_model::semantic::SemanticObjectId::for_window(WindowId(7)),
                    parent: None,
                    window: WindowId(7),
                    source: tessera_model::semantic::SemanticSource::Compositor,
                    role: tessera_model::semantic::SemanticRole::Window,
                    name: Some("smoke".into()),
                    description: None,
                    value: None,
                    app_id: Some("visual-smoke.test".into()),
                    bounds: tessera_model::Rect::new(0, 0, 320, 180),
                    local_size: tessera_model::Size { w: 320, h: 180 },
                    state: tessera_model::semantic::SemanticState {
                        visible: true,
                        enabled: true,
                        ..Default::default()
                    },
                    actions: vec![tessera_model::semantic::SemanticAction::Pointer],
                    revision: 1,
                }],
            },
        })
    }

    fn act_in_interaction_domain(
        &self,
        conn_id: u64,
        subject: Option<&str>,
        _scope_name: Option<&str>,
        _scope: tessera_ipc::Scope,
        intent: tessera_ipc::ActorActionIntent,
    ) -> Result<tessera_ipc::ActorActionReceipt, String> {
        let owner = self
            .observations
            .lock()
            .expect("observation lock")
            .remove(&intent.observation.0);
        if owner != Some(conn_id) {
            return Err("observation belongs to another connection".into());
        }
        let revision = self
            .interaction_domains
            .lock()
            .expect("interaction_domain lock")
            .revision();
        let receipt = tessera_ipc::ActorActionReceipt {
            action_id: 1,
            interaction_domain: intent.interaction_domain,
            target: intent.target,
            window: WindowId(7),
            authority_revision: revision,
            actions_applied: intent.actions.len() as u32,
            committed_mono_ms: 0,
        };
        self.journal.lock().expect("journal lock").append(
            0,
            Origin::ipc(conn_id, subject),
            JournalMutation::ActorAction {
                action_id: Some(receipt.action_id),
                interaction_domain: intent.interaction_domain,
                target: intent.target,
                window: Some(WindowId(7)),
                actions: tessera_ipc::audit_semantic_actions(&intent.actions),
                actions_truncated: false,
                authority_revision: Some(revision),
            },
            Effect::Applied,
        );
        Ok(receipt)
    }

    fn connection_disconnected(&self, conn_id: u64) {
        self.observations
            .lock()
            .expect("observation lock")
            .retain(|_, owner| *owner != conn_id);
    }
}

#[test]
fn stdio_discovers_manages_captures_and_revokes_interaction_domain() {
    let runtime_dir = scratch();
    std::fs::create_dir_all(&runtime_dir).expect("runtime dir");
    let socket = runtime_dir.join("tessera.sock");
    let mut interaction_domains = InteractionDomainModel::new();
    let client = interaction_domains.register_client(Some("visual-smoke.test".into()));
    interaction_domains
        .create_interaction_group(client, &[WindowId(7)], HUMAN_INTERACTION_DOMAIN)
        .expect("human-controlled smoke window");
    let handler = Arc::new(TestHandler {
        interaction_domains: Mutex::new(interaction_domains),
        journal: Mutex::new(Journal::default_capacity()),
        notifications: Mutex::new(vec![]),
        observations: Mutex::new(HashMap::new()),
    });
    let server = Server::start(&socket, Arc::clone(&handler)).expect("server");

    let mut child = Command::new(env!("CARGO_BIN_EXE_tessera-mcp"))
        .arg("serve")
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("TESSERA_MCP_DATA_DIR", runtime_dir.join("data"))
        .env("TESSERA_MCP_INSTANCE_ID", "bridge-integration-test")
        .env(
            "TESSERA_MCP_INTERACTION_DOMAIN_LABEL",
            "Bridge integration test",
        )
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn bridge");
    let meta = json!({
        "io.modelcontextprotocol/protocolVersion": "2026-07-28",
        "io.modelcontextprotocol/clientInfo": {"name": "integration-test", "version": "1"},
        "io.modelcontextprotocol/clientCapabilities": {}
    });
    let requests = [
        json!({"jsonrpc":"2.0","id":1,"method":"server/discover","params":{"_meta": &meta}}),
        json!({"jsonrpc":"2.0","id":2,"method":"tools/list","params":{"_meta": &meta}}),
        json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"interaction_domain_ensure","arguments":{},"_meta": &meta}}),
        json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"interaction_domain_observe","arguments":{},"_meta": &meta}}),
        json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"interaction_domain_input","arguments":{"target_window_id":7,"target_local_id":0,"observation_token":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb","actions":[{"type":"pointer_move","x":160,"y":90}]},"_meta": &meta}}),
        json!({"jsonrpc":"2.0","id":6,"method":"tools/call","params":{"name":"interaction_domain_capture","arguments":{},"_meta": &meta}}),
        json!({"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"window_capture","arguments":{"window_id":7},"_meta": &meta}}),
        json!({"jsonrpc":"2.0","id":8,"method":"tools/call","params":{"name":"launch_app","arguments":{"desktop_id":"visual-smoke.test.desktop","new_workspace":true,"workspace_label":"smoke run"},"_meta": &meta}}),
        json!({"jsonrpc":"2.0","id":9,"method":"tools/call","params":{"name":"launch_app","arguments":{"desktop_id":"visual-smoke.test.desktop","workspace_id":1,"new_workspace":true},"_meta": &meta}}),
        json!({"jsonrpc":"2.0","id":10,"method":"tools/call","params":{"name":"focus_window","arguments":{"window_id":7,"switch_workspace":false},"_meta": &meta}}),
        json!({"jsonrpc":"2.0","id":11,"method":"tools/call","params":{"name":"interaction_domain_reset","arguments":{},"_meta": &meta}}),
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
    assert_eq!(responses.len(), 11);
    assert_eq!(responses[0]["result"]["supportedVersions"][0], "2026-07-28");
    let names = responses[1]["result"]["tools"]
        .as_array()
        .expect("tools")
        .iter()
        .filter_map(|tool| tool["name"].as_str())
        .collect::<Vec<_>>();
    assert!(names.contains(&"interaction_domain_capture"));
    assert!(names.contains(&"interaction_domain_observe"));
    assert!(names.contains(&"interaction_domain_input"));
    assert!(names.contains(&"window_capture"));
    assert!(names.contains(&"launch_app"));
    assert_eq!(responses[2]["result"]["isError"], false);
    assert_eq!(responses[3]["result"]["isError"], false);
    assert_eq!(responses[4]["result"]["isError"], false);
    let input_result = responses[4]["result"]["content"][0]["text"]
        .as_str()
        .expect("interaction_domain_input JSON text");
    assert!(input_result.contains(r#""status":"committed""#));
    assert!(input_result.contains(r#""window":7"#));
    assert!(input_result.contains(r#""local":0"#));
    assert_eq!(responses[5]["result"]["content"][1]["type"], "image");
    assert_eq!(responses[6]["result"]["isError"], false);
    let window_capture_text = responses[6]["result"]["content"][0]["text"]
        .as_str()
        .expect("window_capture JSON text");
    assert!(window_capture_text.contains(r#""window_id":7"#));
    assert_eq!(responses[6]["result"]["content"][1]["type"], "image");
    assert_eq!(responses[7]["result"]["isError"], false);
    let launch_text = responses[7]["result"]["content"][0]["text"]
        .as_str()
        .expect("launch_app JSON text");
    assert!(launch_text.contains(r#""status":"queued""#));
    assert_eq!(responses[8]["result"]["isError"], true);
    assert_eq!(responses[9]["result"]["isError"], false);
    assert_eq!(responses[10]["result"]["isError"], false);
    assert!(
        handler
            .journal
            .lock()
            .expect("journal lock")
            .since(0)
            .entries
            .iter()
            .any(|entry| matches!(
                &entry.mutation,
                JournalMutation::Command {
                    cmd: tessera_ipc::AuditedCommand::LaunchApp { desktop_id, workspace }
                } if desktop_id == "visual-smoke.test.desktop" && workspace.is_none()
            )),
        "launch_app with fresh-workspace placement was not journaled"
    );
    assert!(
        handler
            .interaction_domains
            .lock()
            .expect("interaction_domain lock")
            .snapshot()
            .interaction_domains
            .iter()
            .any(|interaction_domain| interaction_domain.state
                == tessera_model::interaction_domain::InteractionDomainState::Revoked)
    );

    let config =
        tessera_mcp::BridgeConfig::new(&socket, &runtime_dir, "bridge-test", "Bridge smoke test")
            .expect("smoke config");
    let mut platform = tessera_mcp::TesseraPlatform::connect(config).expect("smoke platform");
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
        report.interaction_domain.lifecycle,
        ["active", "paused", "active", "revoked"]
    );
    assert_eq!(
        report.interaction_domain.cleanup,
        "revoked_test_interaction_domain"
    );
    let input = report.visual.input_probe.expect("input probe evidence");
    assert_eq!(input.window_id, 7);
    assert_eq!(input.action, "pointer_move");
    assert_eq!(input.local_position, tessera_model::Point { x: 160, y: 90 });
    assert!(input.applied);
    assert!(input.window_restored_to_human);
    assert_eq!(
        handler
            .interaction_domains
            .lock()
            .expect("interaction_domain lock")
            .interaction_group_for_window(WindowId(7))
            .expect("window group")
            .control_interaction_domain,
        HUMAN_INTERACTION_DOMAIN
    );

    drop(server);
    std::fs::remove_dir_all(&runtime_dir).expect("remove runtime dir");
}
