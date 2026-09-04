//! Binary-entry tests for the unified `tessera` command surface.

use std::sync::Arc;

use tessera_ipc::{Handler, Server};
use tessera_model::window::{Window, WindowId};

struct EntryHandler;

impl Handler for EntryHandler {
    fn policy_caps(&self) -> tessera_ipc::ConnectionCapabilities {
        tessera_ipc::ConnectionCapabilities::QUERY
    }

    fn windows(&self) -> Vec<Window> {
        let mut window = Window::new(WindowId(7));
        window.title = Some("entry-test".into());
        window.app_id = Some("entry-test".into());
        vec![window]
    }

    fn workspaces(&self) -> tessera_model::workspace::WorkspaceSnapshot {
        tessera_model::workspace::WorkspaceSnapshot {
            outputs: Vec::new(),
        }
    }

    fn notifications(&self) -> Vec<tessera_model::notify::Notification> {
        Vec::new()
    }

    fn outputs(&self) -> Vec<tessera_model::output::OutputInfo> {
        Vec::new()
    }

    fn journal_since(&self, _since: u64) -> tessera_ipc::JournalSnapshot {
        tessera_ipc::JournalSnapshot {
            entries: Vec::new(),
            oldest_seq: 0,
            latest_seq: 0,
        }
    }

    fn command(&self, _conn_id: u64, _subject: Option<&str>, _command: tessera_ipc::Command) {}
}

#[test]
fn domain_command_reaches_the_running_session() {
    let runtime = tempfile::tempdir().unwrap();
    let socket = runtime.path().join("tessera.sock");
    let _server = Server::start(&socket, Arc::new(EntryHandler)).unwrap();

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_tessera"))
        .env("XDG_RUNTIME_DIR", runtime.path())
        .arg("window")
        .output()
        .unwrap();

    assert!(output.status.success(), "{:?}", output.status);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("entry-test"), "stdout was {stdout:?}");
}

#[test]
fn help_is_local_and_advertises_resource_domains() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_tessera"))
        .env_remove("XDG_RUNTIME_DIR")
        .arg("--help")
        .output()
        .unwrap();

    assert!(output.status.success(), "{:?}", output.status);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("Commands:"), "stdout was {stdout:?}");
    assert!(stdout.contains("display"), "stdout was {stdout:?}");
    assert!(stdout.contains("window"), "stdout was {stdout:?}");
    assert!(!stdout.contains("tessera-cli"), "stdout was {stdout:?}");
}

#[test]
fn compositor_mode_rejects_client_only_output_flags() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_tessera"))
        .env_remove("XDG_RUNTIME_DIR")
        .args(["--json", "run"])
        .output()
        .unwrap();

    assert_eq!(output.status.code(), Some(2));
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(
        stderr.contains("--json requires a session-management command"),
        "stderr was {stderr:?}"
    );
}
