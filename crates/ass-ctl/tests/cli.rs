//! End-to-end exercise of ass-ctl against a loopback ass-ipc server. Each
//! test starts a server with a fixed handler, invokes `ass_ctl::run` with a
//! command, and asserts on the output or the recorded command.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use ass_core::window::{Window, WindowState};
use ass_core::workspace::{OutputSnapshot, WorkspaceEntry, WorkspaceId, WorkspaceSnapshot};
use ass_ipc::{Command, Handler, Server};

/// A unique temp socket path namespaced by pid + counter.
fn scratch() -> PathBuf {
    static N: AtomicU64 = AtomicU64::new(0);
    let n = N.fetch_add(1, Ordering::Relaxed);
    let mut p = std::env::temp_dir();
    p.push(format!("ass-ctl-{}-{n}.sock", std::process::id()));
    p
}

/// A handler returning a fixed window/workspace set and recording commands.
struct CtlHandler {
    commands: Mutex<Vec<Command>>,
}

impl CtlHandler {
    fn new() -> Self {
        CtlHandler {
            commands: Mutex::new(Vec::new()),
        }
    }
}

impl Handler for CtlHandler {
    fn policy_caps(&self) -> ass_ipc::Capabilities {
        // Grant everything so the control/session commands run.
        ass_ipc::Capabilities {
            query: true,
            control: true,
            session: true,
        }
    }
    fn windows(&self) -> Vec<Window> {
        let mut w = Window::new(1);
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
                id: ass_core::workspace::OutputId(0),
                connector: "nested".into(),
                current: Some(WorkspaceId(0)),
                workspaces: vec![WorkspaceEntry {
                    id: WorkspaceId(0),
                    label: None,
                    toplevels: vec![1],
                }],
            }],
        }
    }
    fn notifications(&self) -> Vec<ass_core::notify::Notification> {
        Vec::new()
    }
    fn command(&self, cmd: Command) {
        self.commands.lock().unwrap().push(cmd);
    }
}

#[test]
fn windows_command_lists_the_fixed_window() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = ass_ctl::run(&path, &["windows".into()]).unwrap();
    assert!(out.contains("foot"), "{out}");
    assert!(out.contains("(foot)"), "{out}");
}

#[test]
fn workspaces_command_shows_output_and_workspace() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = ass_ctl::run(&path, &["workspaces".into()]).unwrap();
    assert!(out.contains("nested"), "{out}");
    assert!(out.contains("1 window"), "{out}");
}

#[test]
fn focus_command_sends_focus() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    let out = ass_ctl::run(&path, &["focus".into(), "1".into()]).unwrap();
    assert!(out.contains("focused 1"), "{out}");
    assert!(
        h.commands
            .lock()
            .unwrap()
            .iter()
            .any(|c| matches!(c, Command::Focus { id: 1 })),
        "{:?}",
        h.commands
    );
}

#[test]
fn switch_command_sends_workspace_switch() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    ass_ctl::run(&path, &["switch".into(), "next".into()]).unwrap();
    assert!(
        h.commands
            .lock()
            .unwrap()
            .iter()
            .any(|c| matches!(c, Command::SwitchWorkspace { .. })),
    );
}

#[test]
fn notify_command_sends_notify_with_body() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    ass_ctl::run(
        &path,
        &["notify".into(), "Hello".into(), "world".into()],
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
fn tiling_command_sends_toggle() {
    let path = scratch();
    let h = Arc::new(CtlHandler::new());
    let _s = Server::start(&path, Arc::clone(&h)).unwrap();
    ass_ctl::run(&path, &["tiling".into()]).unwrap();
    assert!(
        h.commands
            .lock()
            .unwrap()
            .iter()
            .any(|c| matches!(c, Command::ToggleTiling)),
    );
}

#[test]
fn unknown_command_errors_with_usage() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let err = ass_ctl::run(&path, &["bogus".into()]).unwrap_err();
    assert!(err.contains("unknown command 'bogus'"), "{err}");
    assert!(err.contains("usage"), "{err}");
}

#[test]
fn help_command_returns_usage() {
    let path = scratch();
    let _s = Server::start(&path, Arc::new(CtlHandler::new())).unwrap();
    let out = ass_ctl::run(&path, &["help".into()]).unwrap();
    assert!(out.contains("commands:"), "{out}");
}
