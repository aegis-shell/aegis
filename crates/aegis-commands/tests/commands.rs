//! End-to-end exercise of native `aegis` commands against a loopback IPC server. Each
//! test starts a server with a fixed handler, invokes `aegis_commands::run` with a
//! command, and asserts on the output or the recorded command.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use aegis_ipc::{Command, Handler, InteractionDomainAction, InteractionDomainActionResult, Server};
use aegis_model::window::{Window, WindowId, WindowState};
use aegis_model::workspace::{OutputSnapshot, WorkspaceEntry, WorkspaceId, WorkspaceSnapshot};
use clap::Parser;

static N: AtomicU64 = AtomicU64::new(0);

/// A unique temp socket path namespaced by pid + counter.
fn scratch() -> PathBuf {
    let n = N.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!("aegis-command-{}-{n}.sock", std::process::id()));
    p
}

/// A handler returning a fixed window/workspace set and recording commands.
struct CtlHandler {
    commands: Mutex<Vec<Command>>,
    interaction_domain_actions: Mutex<Vec<InteractionDomainAction>>,
    management_log: Mutex<Vec<String>>,
}

impl CtlHandler {
    fn new() -> Self {
        CtlHandler {
            commands: Mutex::new(Vec::new()),
            interaction_domain_actions: Mutex::new(Vec::new()),
            management_log: Mutex::new(Vec::new()),
        }
    }
}

impl Handler for CtlHandler {
    fn policy_caps(&self) -> aegis_ipc::ConnectionCapabilities {
        // Grant everything so the control/session commands run.
        aegis_ipc::ConnectionCapabilities {
            query: true,
            control: true,
            input: false,
            session: true,
            interaction_domain: true,
        }
    }
    fn windows(&self) -> Vec<Window> {
        let mut w = Window::new(WindowId(1));
        w.title = Some("foot".into());
        w.app_id = Some("foot".into());
        w.state = WindowState {
            activated: true,
            ..Default::default()
        };
        vec![w]
    }
    fn workspaces(&self) -> WorkspaceSnapshot {
        WorkspaceSnapshot {
            outputs: vec![OutputSnapshot {
                id: aegis_model::workspace::OutputId(0),
                connector: "nested".into(),
                current: Some(WorkspaceId(0)),
                workspaces: vec![WorkspaceEntry {
                    id: WorkspaceId(0),
                    label: None,
                    tiled: false,
                    toplevels: vec![WindowId(1)],
                }],
            }],
        }
    }
    fn notifications(&self) -> Vec<aegis_model::notify::Notification> {
        vec![aegis_model::notify::Notification {
            id: 7,
            summary: "Build complete".into(),
            body: "All checks passed".into(),
            app_id: Some("ci".into()),
            external_id: None,
            at_ms: 10,
        }]
    }
    fn system_status(&self) -> aegis_ipc::SystemStatus {
        aegis_ipc::SystemStatus {
            volume: Some(42),
            muted: true,
            network: aegis_model::system::NetworkState::Wifi,
            battery: Some(aegis_model::system::BatteryStatus {
                percent: 81,
                charging: true,
            }),
            wifi_enabled: Some(true),
            bluetooth_enabled: Some(false),
            brightness: Some(73),
            do_not_disturb: true,
            tiled: false,
            ..aegis_ipc::SystemStatus::default()
        }
    }
    fn outputs(&self) -> Vec<aegis_model::output::OutputInfo> {
        vec![aegis_model::output::OutputInfo {
            connector: "nested".into(),
            geometry: aegis_model::output::OutputGeometry {
                mode: aegis_model::output::OutputMode {
                    width: 1280,
                    height: 720,
                    refresh_mhz: 60000,
                },
                scale: aegis_model::output::Scale::IDENTITY,
                transform: aegis_model::Transform::Normal,
                logical_origin: aegis_model::Point::default(),
            },
            available_modes: vec![
                aegis_model::output::OutputMode {
                    width: 1280,
                    height: 720,
                    refresh_mhz: 60000,
                },
                aegis_model::output::OutputMode {
                    width: 1920,
                    height: 1080,
                    refresh_mhz: 60000,
                },
            ],
        }]
    }
    fn command(&self, _conn_id: u64, _subject: Option<&str>, cmd: Command) {
        self.commands.lock().unwrap().push(cmd);
    }
    fn system_action(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        action: aegis_ipc::SystemAction,
    ) -> Result<(), String> {
        self.commands
            .lock()
            .unwrap()
            .push(Command::System { action });
        Ok(())
    }
    fn journal_since(&self, _since: u64) -> aegis_ipc::JournalSnapshot {
        aegis_ipc::JournalSnapshot {
            entries: vec![aegis_ipc::JournalEntry {
                seq: 3,
                ts_mono_ms: 42,
                origin: aegis_ipc::Origin::Chrome,
                mutation: aegis_ipc::JournalMutation::Command {
                    cmd: aegis_ipc::AuditedCommand::Focus { id: WindowId(1) },
                },
                effect: aegis_ipc::Effect::Applied,
            }],
            oldest_seq: 1,
            latest_seq: 3,
        }
    }

    fn resolve_scope(&self, name: &str) -> Option<aegis_ipc::Scope> {
        if name == aegis_ipc::LOCAL_OWNER_ADMIN_SCOPE {
            return Some(aegis_ipc::Scope::unscoped());
        }
        if name == aegis_ipc::LOCAL_AGENT_ADMIN_SCOPE {
            return Some(aegis_ipc::Scope {
                ops: Some(Vec::new()),
                ..aegis_ipc::Scope::default()
            });
        }
        (name == aegis_ipc::LOCAL_INTERACTION_DOMAIN_ADMIN_SCOPE).then(|| aegis_ipc::Scope {
            windows: None,
            workspaces: None,
            outputs: None,
            interaction_domains: None,
            ops: Some(vec![
                aegis_ipc::ActorCapability::CreateInteractionDomain,
                aegis_ipc::ActorCapability::TransactInteractionDomain,
                aegis_ipc::ActorCapability::RevokeInteractionDomain,
                aegis_ipc::ActorCapability::CaptureInteractionDomain,
                aegis_ipc::ActorCapability::LaunchInInteractionDomain,
            ]),
            ask_ops: None,
        })
    }

    fn interaction_domains(&self) -> aegis_model::interaction_domain::InteractionDomainSnapshot {
        let mut model = aegis_model::interaction_domain::InteractionDomainModel::new();
        model.create_agent_interaction_domain(
            "Test Agent",
            aegis_model::interaction_domain::SeatCapabilities::POINTER_KEYBOARD,
        );
        model.snapshot()
    }

    fn interaction_domain_action(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        action: InteractionDomainAction,
    ) -> Result<InteractionDomainActionResult, String> {
        self.interaction_domain_actions
            .lock()
            .unwrap()
            .push(action.clone());
        match action {
            InteractionDomainAction::Create { .. } => Ok(InteractionDomainActionResult::Created {
                bundle: aegis_model::interaction_domain::InteractionDomainBundle {
                    principal: aegis_model::interaction_domain::InteractionPrincipalId(2),
                    interaction_domain: aegis_model::interaction_domain::InteractionDomainId(2),
                    seat: aegis_model::interaction_domain::SeatId(2),
                    revision: 2,
                },
            }),
            InteractionDomainAction::Transact {
                expected_revision,
                mutations,
            } => Ok(InteractionDomainActionResult::TransactionCommitted {
                receipt: aegis_model::interaction_domain::InteractionDomainTransactionReceipt {
                    before_revision: expected_revision.unwrap_or(2),
                    after_revision: expected_revision.unwrap_or(2) + mutations.len() as u64,
                    results: Vec::new(),
                },
            }),
            InteractionDomainAction::Revoke {
                interaction_domain,
                fallback,
                ..
            } => Ok(InteractionDomainActionResult::Revoked {
                receipt: aegis_model::interaction_domain::InteractionDomainRevocation {
                    interaction_domain,
                    fallback,
                    transferred_groups: Vec::new(),
                    revision: 3,
                },
            }),
        }
    }

    fn capture_interaction_domain(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
        region: Option<aegis_model::Rect>,
    ) -> Result<aegis_ipc::CaptureInteractionDomainPayload, String> {
        Ok(aegis_ipc::CaptureInteractionDomainPayload {
            capture: aegis_ipc::InteractionDomainCapture {
                interaction_domain,
                width: 2,
                height: 1,
                scale_milli: 1000,
                region: region.unwrap_or_else(|| aegis_model::Rect::new(0, 0, 2, 1)),
                placements: vec![
                    aegis_model::interaction_domain::InteractionDomainWindowPlacement {
                        window: WindowId(1),
                        output_rect: aegis_model::Rect::new(0, 0, 2, 1),
                        surface_size: aegis_model::Size { w: 2, h: 1 },
                    },
                ],
                observation: aegis_ipc::SemanticObservation {
                    token: aegis_ipc::ObservationToken("0".repeat(64)),
                    ttl_ms: 15_000,
                    snapshot: aegis_model::semantic::SemanticSnapshot {
                        interaction_domain,
                        authority_revision: 2,
                        objects: Vec::new(),
                    },
                },
                png_bytes: 3,
                revision: 2,
            },
            png: b"png".to_vec(),
        })
    }

    fn agent_principals(&self) -> Vec<aegis_ipc::AgentPrincipalInfo> {
        vec![aegis_ipc::AgentPrincipalInfo {
            principal: "prin_1".into(),
            label: Some("Codex".into()),
            pregranted: vec![aegis_ipc::ActorCapability::Focus],
            gated: vec![aegis_ipc::ActorCapability::Close],
            created_at: 1,
        }]
    }

    fn agent_grants(&self, principal: Option<&str>) -> Vec<aegis_ipc::AgentGrantInfo> {
        vec![aegis_ipc::AgentGrantInfo {
            principal: "prin_1".into(),
            op: aegis_ipc::ActorCapability::Close,
            decision: aegis_ipc::AgentGrantDecision::Allow,
            granted_at: 2,
        }]
        .into_iter()
        .filter(|grant| principal.is_none_or(|p| grant.principal == p))
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
        pregranted: &[aegis_ipc::ActorCapability],
        gated: &[aegis_ipc::ActorCapability],
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
        _pregranted: &[aegis_ipc::ActorCapability],
        _gated: &[aegis_ipc::ActorCapability],
    ) -> Result<(String, String), String> {
        self.management_log
            .lock()
            .unwrap()
            .push(format!("register:{label:?}"));
        Ok(("prin_9".into(), "cred_9".into()))
    }

    fn revoke_agent_grant(
        &self,
        principal: &str,
        op: aegis_ipc::ActorCapability,
    ) -> Result<(), String> {
        self.management_log
            .lock()
            .unwrap()
            .push(format!("revoke:{principal}:{op:?}"));
        Ok(())
    }
}

#[test]
fn window_domain_lists_the_fixed_window() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_commands::run(&path, &["window".into()]).unwrap();
    assert!(out.contains("foot"), "{out}");
    assert!(out.contains("(foot)"), "{out}");
}

#[test]
fn json_flag_emits_parseable_json_for_window_domain() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_commands::run(&path, &["window".into(), "--json".into()]).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let arr = parsed.as_array().expect("a JSON array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["app_id"], "foot");
    assert_eq!(arr[0]["state"]["activated"], true);
}

#[test]
fn workspace_domain_shows_display_and_workspace() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_commands::run(&path, &["workspace".into()]).unwrap();
    assert!(out.contains("nested"), "{out}");
    assert!(out.contains("1 window"), "{out}");
}

#[test]
fn display_domain_lists_advertised_modes_and_marks_current() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_commands::run(&path, &["display".into()]).unwrap();
    assert!(out.contains("nested 1280x720@60.000Hz"), "{out}");
    assert!(out.contains("1280x720@60.000Hz (current)"), "{out}");
    assert!(out.contains("1920x1080@60.000Hz"), "{out}");
}

#[test]
fn notification_domain_lists_active_notifications() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_commands::run(&path, &["notification".into()]).unwrap();
    assert!(out.contains("Build complete"), "{out}");
    assert!(out.contains("All checks passed"), "{out}");
}

#[test]
fn system_status_command_formats_human_and_json_snapshots() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_commands::run(&path, &["system".into()]).unwrap();
    assert!(out.contains("audio: 42% (muted)"), "{out}");
    assert!(out.contains("wifi: on; bluetooth: off"), "{out}");
    assert!(out.contains("battery: 81% charging"), "{out}");
    assert!(out.contains("do not disturb: on"), "{out}");

    let json = aegis_commands::run(&path, &["--json".into(), "system".into()]).unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).expect("system status JSON");
    assert_eq!(value["volume"], 42);
    assert_eq!(value["brightness"], 73);
    assert_eq!(value["network"], "Wifi");
}

#[test]
fn system_control_commands_send_typed_actions() {
    let path = scratch();
    let handler = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&handler)).unwrap();
    let receipt = aegis_commands::run(
        &path,
        &[
            "--json".into(),
            "system".into(),
            "volume".into(),
            "55".into(),
        ],
    )
    .unwrap();
    let receipt: serde_json::Value = serde_json::from_str(&receipt).unwrap();
    assert_eq!(receipt["ok"], true);
    aegis_commands::run(&path, &["system".into(), "step-volume".into(), "-2".into()]).unwrap();
    aegis_commands::run(&path, &["system".into(), "wifi".into(), "off".into()]).unwrap();
    assert_eq!(
        handler.commands.lock().unwrap().as_slice(),
        &[
            Command::System {
                action: aegis_ipc::SystemAction::SetVolume { level: 55 },
            },
            Command::System {
                action: aegis_ipc::SystemAction::StepVolume { delta: -2 },
            },
            Command::System {
                action: aegis_ipc::SystemAction::SetWifi { enabled: false },
            },
        ]
    );
}

#[test]
fn system_control_cli_rejects_out_of_range_levels() {
    let error = aegis_commands::run(
        std::path::Path::new(""),
        &["system".into(), "brightness".into(), "0".into()],
    )
    .unwrap_err();
    assert_eq!(error.exit_code(), 2);
    assert!(error.to_string().contains("1..=100"), "{error}");
}

#[test]
fn journal_command_lists_entries() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_commands::run(
        &path,
        &[
            "journal".into(),
            "list".into(),
            "--since".into(),
            "2".into(),
        ],
    )
    .unwrap();
    assert!(out.contains("#3"), "{out}");
    assert!(out.contains("Focus"), "{out}");
}

#[test]
fn interaction_domain_domain_uses_admin_scope_and_lists_agent_interaction_domain() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_commands::run(&path, &["interaction-domain".into(), "list".into()]).unwrap();
    assert!(out.contains("Test Agent"), "{out}");
    assert!(out.contains("agent-2"), "{out}");
}

#[test]
fn interaction_domain_create_and_transfer_use_optimistic_actions() {
    let path = scratch();
    let handler = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&handler)).unwrap();
    let created = aegis_commands::run(
        &path,
        &[
            "interaction-domain".into(),
            "create".into(),
            "Builder".into(),
        ],
    )
    .unwrap();
    assert!(
        created.contains("created Interaction Domain 2"),
        "{created}"
    );
    let transferred = aegis_commands::run(
        &path,
        &[
            "interaction-domain".into(),
            "transfer".into(),
            "1".into(),
            "2".into(),
        ],
    )
    .unwrap();
    assert!(transferred.contains("committed"), "{transferred}");
    let actions = handler.interaction_domain_actions.lock().unwrap();
    assert!(matches!(
        &actions[0],
        InteractionDomainAction::Create { label, .. } if label == "Builder"
    ));
    assert!(matches!(
        &actions[1],
        InteractionDomainAction::Transact {
            expected_revision: Some(2),
            mutations,
        } if matches!(
            mutations.as_slice(),
            [aegis_model::interaction_domain::InteractionDomainMutation::TransferWindow {
                window: WindowId(1),
                target: aegis_model::interaction_domain::InteractionDomainId(2),
                retain_source_as_observer: true,
            }]
        )
    ));
}

#[test]
fn interaction_domain_capture_is_committed_atomically_to_requested_path() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let capture = dir.path().join("interaction_domain.png");
    let out = aegis_commands::run(
        &path,
        &[
            "interaction-domain".into(),
            "capture".into(),
            "2".into(),
            capture.to_string_lossy().into_owned(),
        ],
    )
    .unwrap();
    assert!(out.contains("2x1"), "{out}");
    assert_eq!(std::fs::read(capture).unwrap(), b"png");
}

#[test]
fn interaction_domain_capture_json_exposes_pixel_to_input_mapping() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let capture = dir.path().join("interaction_domain.png");
    let out = aegis_commands::run(
        &path,
        &[
            "--json".into(),
            "interaction-domain".into(),
            "capture".into(),
            "2".into(),
            "--region=0,0,2,1".into(),
            capture.to_string_lossy().into_owned(),
        ],
    )
    .unwrap();
    let json: serde_json::Value = serde_json::from_str(&out).expect("capture metadata JSON");
    assert_eq!(json["scale_milli"], 1000);
    assert_eq!(json["region"]["size"]["w"], 2);
    assert_eq!(json["placements"][0]["window"], 1);
    assert_eq!(json["placements"][0]["surface_size"]["h"], 1);
    assert_eq!(std::fs::read(capture).unwrap(), b"png");
}

#[test]
fn window_focus_sends_focus() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    let out = aegis_commands::run(&path, &["window".into(), "focus".into(), "1".into()]).unwrap();
    assert!(out.contains("focused 1"), "{out}");
    assert!(
        h.commands
            .lock()
            .unwrap()
            .iter()
            .any(|c| matches!(c, Command::Focus { id: WindowId(1) })),
        "{:?}",
        h.commands
    );
}

#[test]
fn window_minimize_sends_minimize() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    let out =
        aegis_commands::run(&path, &["window".into(), "minimize".into(), "1".into()]).unwrap();
    assert!(out.contains("minimized 1"), "{out}");
    assert!(
        h.commands
            .lock()
            .unwrap()
            .contains(&Command::Minimize { id: WindowId(1) })
    );
}

#[test]
fn window_geometry_sends_logical_rectangle() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    let out = aegis_commands::run(
        &path,
        &[
            "window".into(),
            "geometry".into(),
            "1".into(),
            "-20".into(),
            "30".into(),
            "800".into(),
            "600".into(),
        ],
    )
    .unwrap();
    assert!(out.contains("-20,30 800x600"), "{out}");
    assert!(
        h.commands
            .lock()
            .unwrap()
            .contains(&Command::SetWindowGeometry {
                id: WindowId(1),
                rect: aegis_model::Rect::new(-20, 30, 800, 600),
            })
    );
}

#[test]
fn workspace_switch_relative_sends_workspace_switch() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    aegis_commands::run(&path, &["workspace".into(), "switch".into(), "next".into()]).unwrap();
    assert!(
        h.commands
            .lock()
            .unwrap()
            .iter()
            .any(|c| matches!(c, Command::SwitchWorkspace { .. })),
    );
}

#[test]
fn workspace_switch_id_sends_direct_workspace_switch() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    let out =
        aegis_commands::run(&path, &["workspace".into(), "switch".into(), "9".into()]).unwrap();
    assert!(out.contains("workspace 9"), "{out}");
    assert!(h.commands.lock().unwrap().iter().any(|c| matches!(
        c,
        Command::SwitchWorkspaceTo { id } if id.0 == 9
    )));
}

#[test]
fn bare_and_explicit_run_invocations_select_the_compositor() {
    let bare = aegis_commands::Cli::try_parse_from(["aegis"]).unwrap();
    assert!(bare.runs_compositor());

    let explicit = aegis_commands::Cli::try_parse_from(["aegis", "run"]).unwrap();
    assert!(explicit.runs_compositor());

    let display = aegis_commands::Cli::try_parse_from(["aegis", "display"]).unwrap();
    assert!(!display.runs_compositor());
}

#[test]
fn notification_send_sends_notify_with_body() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    aegis_commands::run(
        &path,
        &[
            "notification".into(),
            "send".into(),
            "Hello".into(),
            "world".into(),
        ],
    )
    .unwrap();
    assert!(
        h.commands.lock().unwrap().iter().any(|c| matches!(
            c,
            Command::Notify { summary, body, .. } if summary == "Hello" && body == "world"
        )),
        "{:?}",
        h.commands
    );
}

#[test]
fn notification_dismiss_sends_dismiss_notification() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    let out = aegis_commands::run(
        &path,
        &["notification".into(), "dismiss".into(), "7".into()],
    )
    .unwrap();
    assert!(out.contains("dismissed 7"), "{out}");
    assert!(
        h.commands
            .lock()
            .unwrap()
            .iter()
            .any(|c| matches!(c, Command::DismissNotification { id: 7 })),
        "{:?}",
        h.commands
    );
}

#[test]
fn workspace_layout_toggle_sends_toggle() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    aegis_commands::run(
        &path,
        &["workspace".into(), "layout".into(), "toggle".into()],
    )
    .unwrap();
    assert!(
        h.commands
            .lock()
            .unwrap()
            .iter()
            .any(|c| matches!(c, Command::ToggleTiling)),
    );
}

#[test]
fn workspace_move_window_sends_move_to_workspace() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    let out = aegis_commands::run(
        &path,
        &[
            "workspace".into(),
            "move-window".into(),
            "42".into(),
            "3".into(),
        ],
    )
    .unwrap();
    assert!(out.contains("moved window 42 to workspace 3"), "{out}");
    assert!(
        h.commands.lock().unwrap().iter().any(|c| matches!(
            c,
            Command::MoveToWorkspace { window: WindowId(42), workspace } if workspace.0 == 3
        )),
        "{:?}",
        h.commands
    );
}

#[test]
fn unknown_command_errors_with_usage() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let err = aegis_commands::run(&path, &["bogus".into()]).unwrap_err();
    let rendered = err.to_string();
    assert!(
        rendered.contains("unrecognized subcommand 'bogus'"),
        "{rendered}"
    );
    assert!(rendered.contains("Usage:"), "{rendered}");
    assert_eq!(err.exit_code(), 2);
}

#[test]
fn help_command_returns_usage() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_commands::run(&path, &["help".into()]).unwrap();
    assert!(out.contains("Commands:"), "{out}");
}

#[test]
fn permissions_list_renders_principals_and_grants() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_commands::run(&path, &["permissions".into(), "list".into()]).unwrap();
    assert!(out.contains("Codex (prin_1)"), "{out}");
    assert!(out.contains("ceiling: Focus (gated: Close)"), "{out}");
    assert!(out.contains("Close: Allow"), "{out}");
}

#[test]
fn permissions_list_json_emits_parseable_json() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_commands::run(
        &path,
        &["permissions".into(), "list".into(), "--json".into()],
    )
    .unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    assert_eq!(parsed["principals"][0]["principal"], "prin_1");
    assert_eq!(parsed["principals"][0]["label"], "Codex");
    assert_eq!(parsed["grants"][0]["op"]["type"], "Close");
    assert_eq!(parsed["grants"][0]["decision"]["type"], "Allow");
}

#[test]
fn permissions_revoke_parses_op_names_and_confirms() {
    let path = scratch();
    let handler = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&handler)).unwrap();
    let out = aegis_commands::run(
        &path,
        &[
            "permissions".into(),
            "revoke".into(),
            "prin_1".into(),
            "capture_interaction_domain".into(),
        ],
    )
    .unwrap();
    assert!(
        out.contains("revoked the CaptureInteractionDomain grant for prin_1"),
        "{out}"
    );
    assert_eq!(
        handler.management_log.lock().unwrap().as_slice(),
        &["revoke:prin_1:CaptureInteractionDomain".to_string()][..]
    );
}

#[test]
fn permissions_revoke_rejects_unknown_op_names() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let error = aegis_commands::run(
        &path,
        &[
            "permissions".into(),
            "revoke".into(),
            "prin_1".into(),
            "NotARealOperation".into(),
        ],
    )
    .expect_err("unknown op is a usage error");
    assert!(matches!(error, aegis_commands::CliError::Usage(_)));
}

#[test]
fn permissions_forget_rename_and_ceiling_confirm() {
    let path = scratch();
    let handler = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&handler)).unwrap();

    let out = aegis_commands::run(
        &path,
        &[
            "permissions".into(),
            "rename".into(),
            "prin_1".into(),
            "OpenCode".into(),
        ],
    )
    .unwrap();
    assert!(out.contains("renamed prin_1 to 'OpenCode'"), "{out}");

    let out = aegis_commands::run(
        &path,
        &["permissions".into(), "forget".into(), "prin_2".into()],
    )
    .unwrap();
    assert!(out.contains("forgot principal prin_2"), "{out}");

    let out = aegis_commands::run(
        &path,
        &[
            "permissions".into(),
            "set-ceiling".into(),
            "prin_1".into(),
            "--pregrant".into(),
            "Focus,Notify".into(),
            "--gated".into(),
            "Close".into(),
        ],
    )
    .unwrap();
    assert!(out.contains("replaced the ceiling of prin_1"), "{out}");

    let log = handler.management_log.lock().unwrap();
    assert_eq!(
        log.as_slice(),
        &[
            "rename:prin_1:Some(\"OpenCode\")".to_string(),
            "forget:prin_2".to_string(),
            "ceiling:prin_1:2+1".to_string(),
        ][..]
    );
}

#[test]
fn permissions_register_prints_the_issued_credential() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_commands::run(
        &path,
        &[
            "permissions".into(),
            "register".into(),
            "Fleet".into(),
            "--pregrant".into(),
            "Focus".into(),
        ],
    )
    .unwrap();
    assert!(out.contains("registered prin_9"), "{out}");
    assert!(out.contains("credential: cred_9"), "{out}");
}

#[test]
fn config_validate_is_local_and_reports_the_schema() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(&path, "schema_version = 2\n").unwrap();
    let out = aegis_commands::run(
        std::path::Path::new(""),
        &[
            "config".into(),
            "validate".into(),
            path.display().to_string(),
        ],
    )
    .unwrap();
    assert!(out.contains("configuration valid"), "{out}");
    assert!(out.contains("schema 2"), "{out}");
}

#[test]
fn config_migrate_is_explicit_and_reports_the_backup() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    std::fs::write(
        &path,
        "schema_version = 1 # retained\n[ui]\nreduced_motion = true\n",
    )
    .unwrap();
    let out = aegis_commands::run(
        std::path::Path::new(""),
        &[
            "config".into(),
            "migrate".into(),
            path.display().to_string(),
        ],
    )
    .unwrap();
    assert!(out.contains("from schema 1 to 2"), "{out}");
    assert!(out.contains("schema-v1.bak"), "{out}");
    let migrated = std::fs::read_to_string(&path).unwrap();
    assert!(migrated.contains("schema_version = 2 # retained"));
    aegis_config::Config::parse(&migrated).unwrap();
}
