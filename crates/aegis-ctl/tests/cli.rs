//! End-to-end exercise of aegis-ctl against a loopback aegis-ipc server. Each
//! test starts a server with a fixed handler, invokes `aegis_ctl::run` with a
//! command, and asserts on the output or the recorded command.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use aegis_core::window::{Window, WindowId, WindowState};
use aegis_core::workspace::{OutputSnapshot, WorkspaceEntry, WorkspaceId, WorkspaceSnapshot};
use aegis_ipc::{Command, Handler, RealmAction, RealmActionResult, Server};

static N: AtomicU64 = AtomicU64::new(0);

/// A unique temp socket path namespaced by pid + counter.
fn scratch() -> PathBuf {
    let n = N.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!("aegis-ctl-{}-{n}.sock", std::process::id()));
    p
}

/// A handler returning a fixed window/workspace set and recording commands.
struct CtlHandler {
    commands: Mutex<Vec<Command>>,
    realm_actions: Mutex<Vec<RealmAction>>,
}

impl CtlHandler {
    fn new() -> Self {
        CtlHandler {
            commands: Mutex::new(Vec::new()),
            realm_actions: Mutex::new(Vec::new()),
        }
    }
}

impl Handler for CtlHandler {
    fn policy_caps(&self) -> aegis_ipc::Capabilities {
        // Grant everything so the control/session commands run.
        aegis_ipc::Capabilities {
            query: true,
            control: true,
            input: false,
            session: true,
            realm: true,
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
                id: aegis_core::workspace::OutputId(0),
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
    fn notifications(&self) -> Vec<aegis_core::notify::Notification> {
        vec![aegis_core::notify::Notification {
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
            network: aegis_core::system::NetworkState::Wifi,
            battery: Some(aegis_core::system::BatteryStatus {
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
    fn outputs(&self) -> Vec<aegis_core::output::OutputInfo> {
        vec![aegis_core::output::OutputInfo {
            connector: "nested".into(),
            geometry: aegis_core::output::OutputGeometry {
                mode: aegis_core::output::OutputMode {
                    width: 1280,
                    height: 720,
                    refresh_mhz: 60000,
                },
                scale: aegis_core::output::Scale::IDENTITY,
                transform: aegis_core::Transform::Normal,
                logical_origin: aegis_core::Point::default(),
            },
            available_modes: vec![
                aegis_core::output::OutputMode {
                    width: 1280,
                    height: 720,
                    refresh_mhz: 60000,
                },
                aegis_core::output::OutputMode {
                    width: 1920,
                    height: 1080,
                    refresh_mhz: 60000,
                },
            ],
        }]
    }
    fn command(&self, _conn_id: u64, cmd: Command) {
        self.commands.lock().unwrap().push(cmd);
    }
    fn system_action(&self, _conn_id: u64, action: aegis_ipc::SystemAction) -> Result<(), String> {
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
                    cmd: Command::Focus { id: WindowId(1) },
                },
                effect: aegis_ipc::Effect::Applied,
            }],
            oldest_seq: 1,
            latest_seq: 3,
        }
    }

    fn resolve_scope(&self, name: &str) -> Option<aegis_ipc::Scope> {
        (name == aegis_ipc::LOCAL_REALM_ADMIN_SCOPE).then(|| aegis_ipc::Scope {
            windows: None,
            workspaces: None,
            outputs: None,
            realms: None,
            ops: Some(vec![
                aegis_ipc::OpClass::CreateRealm,
                aegis_ipc::OpClass::TransactRealm,
                aegis_ipc::OpClass::RevokeRealm,
                aegis_ipc::OpClass::CaptureRealm,
                aegis_ipc::OpClass::LaunchInRealm,
            ]),
            ask_ops: None,
        })
    }

    fn realms(&self) -> aegis_core::realm::RealmSnapshot {
        let mut model = aegis_core::realm::RealmModel::new();
        model.create_agent_realm(
            "Test Agent",
            aegis_core::realm::SeatCapabilities::POINTER_KEYBOARD,
        );
        model.snapshot()
    }

    fn realm_action(
        &self,
        _conn_id: u64,
        action: RealmAction,
    ) -> Result<RealmActionResult, String> {
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
            RealmAction::Transact {
                expected_revision,
                mutations,
            } => Ok(RealmActionResult::TransactionCommitted {
                receipt: aegis_core::realm::RealmTransactionReceipt {
                    before_revision: expected_revision.unwrap_or(2),
                    after_revision: expected_revision.unwrap_or(2) + mutations.len() as u64,
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

    fn capture_realm(
        &self,
        realm: aegis_core::realm::RealmId,
        region: Option<aegis_core::Rect>,
    ) -> Result<aegis_ipc::CaptureRealmPayload, String> {
        Ok(aegis_ipc::CaptureRealmPayload {
            capture: aegis_ipc::RealmCapture {
                realm,
                width: 2,
                height: 1,
                scale_milli: 1000,
                region: region.unwrap_or_else(|| aegis_core::Rect::new(0, 0, 2, 1)),
                placements: vec![aegis_core::realm::RealmWindowPlacement {
                    window: WindowId(1),
                    output_rect: aegis_core::Rect::new(0, 0, 2, 1),
                    surface_size: aegis_core::Size { w: 2, h: 1 },
                }],
                png_bytes: 3,
                revision: 2,
            },
            png: b"png".to_vec(),
        })
    }
}

#[test]
fn windows_command_lists_the_fixed_window() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_ctl::run(&path, &["windows".into()]).unwrap();
    assert!(out.contains("foot"), "{out}");
    assert!(out.contains("(foot)"), "{out}");
}

#[test]
fn json_flag_emits_parseable_json_for_windows() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_ctl::run(&path, &["windows".into(), "--json".into()]).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&out).expect("valid JSON");
    let arr = parsed.as_array().expect("a JSON array");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["app_id"], "foot");
    assert_eq!(arr[0]["state"]["activated"], true);
}

#[test]
fn workspaces_command_shows_output_and_workspace() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_ctl::run(&path, &["workspaces".into()]).unwrap();
    assert!(out.contains("nested"), "{out}");
    assert!(out.contains("1 window"), "{out}");
}

#[test]
fn outputs_command_lists_advertised_modes_and_marks_current() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_ctl::run(&path, &["outputs".into()]).unwrap();
    assert!(out.contains("nested 1280x720@60.000Hz"), "{out}");
    assert!(out.contains("1280x720@60.000Hz (current)"), "{out}");
    assert!(out.contains("1920x1080@60.000Hz"), "{out}");
}

#[test]
fn notifications_command_lists_active_notifications() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_ctl::run(&path, &["notifications".into()]).unwrap();
    assert!(out.contains("Build complete"), "{out}");
    assert!(out.contains("All checks passed"), "{out}");
}

#[test]
fn system_status_command_formats_human_and_json_snapshots() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_ctl::run(&path, &["system".into(), "status".into()]).unwrap();
    assert!(out.contains("audio: 42% (muted)"), "{out}");
    assert!(out.contains("wifi: on; bluetooth: off"), "{out}");
    assert!(out.contains("battery: 81% charging"), "{out}");
    assert!(out.contains("do not disturb: on"), "{out}");

    let json = aegis_ctl::run(&path, &["--json".into(), "system".into(), "status".into()]).unwrap();
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
    aegis_ctl::run(&path, &["system".into(), "volume".into(), "55".into()]).unwrap();
    aegis_ctl::run(&path, &["system".into(), "step-volume".into(), "-2".into()]).unwrap();
    aegis_ctl::run(&path, &["system".into(), "wifi".into(), "off".into()]).unwrap();
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
    let error = aegis_ctl::run(
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
    let out = aegis_ctl::run(&path, &["journal".into(), "2".into()]).unwrap();
    assert!(out.contains("#3"), "{out}");
    assert!(out.contains("Focus"), "{out}");
}

#[test]
fn realms_command_uses_admin_scope_and_lists_agent_realm() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = aegis_ctl::run(&path, &["realm".into(), "list".into()]).unwrap();
    assert!(out.contains("Test Agent"), "{out}");
    assert!(out.contains("agent-2"), "{out}");
}

#[test]
fn realm_create_and_transfer_use_optimistic_actions() {
    let path = scratch();
    let handler = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&handler)).unwrap();
    let created =
        aegis_ctl::run(&path, &["realm".into(), "create".into(), "Builder".into()]).unwrap();
    assert!(created.contains("created Realm 2"), "{created}");
    let transferred = aegis_ctl::run(
        &path,
        &["realm".into(), "transfer".into(), "1".into(), "2".into()],
    )
    .unwrap();
    assert!(transferred.contains("committed"), "{transferred}");
    let actions = handler.realm_actions.lock().unwrap();
    assert!(matches!(
        &actions[0],
        RealmAction::Create { label, .. } if label == "Builder"
    ));
    assert!(matches!(
        &actions[1],
        RealmAction::Transact {
            expected_revision: Some(2),
            mutations,
        } if matches!(
            mutations.as_slice(),
            [aegis_core::realm::RealmMutation::TransferWindow {
                window: WindowId(1),
                target: aegis_core::realm::RealmId(2),
                retain_source_as_observer: true,
            }]
        )
    ));
}

#[test]
fn realm_capture_is_committed_atomically_to_requested_path() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let capture = dir.path().join("realm.png");
    let out = aegis_ctl::run(
        &path,
        &[
            "realm".into(),
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
fn realm_capture_json_exposes_pixel_to_input_mapping() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let capture = dir.path().join("realm.png");
    let out = aegis_ctl::run(
        &path,
        &[
            "--json".into(),
            "realm".into(),
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
fn focus_command_sends_focus() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    let out = aegis_ctl::run(&path, &["focus".into(), "1".into()]).unwrap();
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
fn minimize_command_sends_minimize() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    let out = aegis_ctl::run(&path, &["minimize".into(), "1".into()]).unwrap();
    assert!(out.contains("minimized 1"), "{out}");
    assert!(
        h.commands
            .lock()
            .unwrap()
            .contains(&Command::Minimize { id: WindowId(1) })
    );
}

#[test]
fn set_geometry_command_sends_logical_rectangle() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    let out = aegis_ctl::run(
        &path,
        &[
            "set-geometry".into(),
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
                rect: aegis_core::Rect::new(-20, 30, 800, 600),
            })
    );
}

#[test]
fn switch_command_sends_workspace_switch() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    aegis_ctl::run(&path, &["switch".into(), "next".into()]).unwrap();
    assert!(
        h.commands
            .lock()
            .unwrap()
            .iter()
            .any(|c| matches!(c, Command::SwitchWorkspace { .. })),
    );
}

#[test]
fn switch_to_command_sends_direct_workspace_switch() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    let out = aegis_ctl::run(&path, &["switch-to".into(), "9".into()]).unwrap();
    assert!(out.contains("workspace 9"), "{out}");
    assert!(h.commands.lock().unwrap().iter().any(|c| matches!(
        c,
        Command::SwitchWorkspaceTo { id } if id.0 == 9
    )));
}

#[test]
fn binary_prints_query_output() {
    let root = std::env::temp_dir().join(format!(
        "aegis-ctl-bin-{}-{}",
        std::process::id(),
        N.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&root).unwrap();
    let socket = root.join("aegis.sock");
    let _s = Server::start(&socket, Arc::new(CtlHandler::new())).unwrap();
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_aegis-ctl"))
        .env("XDG_RUNTIME_DIR", &root)
        .arg("windows")
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.status);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("foot"), "stdout was {stdout:?}");
    std::fs::remove_dir_all(&root).ok();
}

#[test]
fn binary_help_needs_no_runtime_directory() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_aegis-ctl"))
        .env_remove("XDG_RUNTIME_DIR")
        .arg("--help")
        .output()
        .unwrap();
    assert!(output.status.success(), "{:?}", output.status);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Commands:"), "stdout was {stdout:?}");
}

#[test]
fn notify_command_sends_notify_with_body() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    aegis_ctl::run(&path, &["notify".into(), "Hello".into(), "world".into()]).unwrap();
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
fn dismiss_command_sends_dismiss_notification() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    let out = aegis_ctl::run(&path, &["dismiss".into(), "7".into()]).unwrap();
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
fn tiling_command_sends_toggle() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    aegis_ctl::run(&path, &["tiling".into()]).unwrap();
    assert!(
        h.commands
            .lock()
            .unwrap()
            .iter()
            .any(|c| matches!(c, Command::ToggleTiling)),
    );
}

#[test]
fn move_to_command_sends_move_to_workspace() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    let out = aegis_ctl::run(&path, &["move-to".into(), "42".into(), "3".into()]).unwrap();
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
    let err = aegis_ctl::run(&path, &["bogus".into()]).unwrap_err();
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
    let out = aegis_ctl::run(&path, &["help".into()]).unwrap();
    assert!(out.contains("Commands:"), "{out}");
}
