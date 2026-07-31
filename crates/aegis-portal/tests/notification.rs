//! End-to-end exercise of the Notification backend: the real `aegis-portal`
//! daemon on a private session bus (see `tests/common/`), plus a fake
//! compositor IPC server recording `Command`s and serving a crafted
//! notification snapshot for the removal path.
//!
//! ```sh
//! cargo test -p aegis-portal --test notification
//! ```
//!
//! Without `dbus-daemon` available the test skips cleanly.

use std::sync::{Arc, Mutex};

use aegis_ipc::{Capabilities, Command, Handler, JournalSnapshot, OpClass, Scope, Server};
use zbus::blocking::Proxy;
use zbus::zvariant::{OwnedValue, Value};

mod common;
use common::{KillOnDrop, private_bus, spawn_daemon, temp_dir, wait_for_name};

const PORTAL: &str = "org.freedesktop.impl.portal.desktop.aegis";
const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.impl.portal.Notification";

/// A fake compositor: records every `Command`, and serves one crafted
/// notification so `(app_id, external_id)` resolution has a target.
struct FakeCompositor {
    commands: Mutex<Vec<Command>>,
}

impl Handler for FakeCompositor {
    fn policy_caps(&self) -> Capabilities {
        Capabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            realm: false,
        }
    }

    fn windows(&self) -> Vec<aegis_core::window::Window> {
        Vec::new()
    }

    fn workspaces(&self) -> aegis_core::workspace::WorkspaceSnapshot {
        aegis_core::workspace::WorkspaceSnapshot { outputs: vec![] }
    }

    fn notifications(&self) -> Vec<aegis_core::notify::Notification> {
        vec![aegis_core::notify::Notification {
            id: 41,
            summary: "earlier title".into(),
            body: "earlier body".into(),
            app_id: Some("dev.aegis.smoke".into()),
            external_id: Some("mail-1".into()),
            at_ms: 1,
        }]
    }

    fn outputs(&self) -> Vec<aegis_core::output::OutputInfo> {
        Vec::new()
    }

    fn journal_since(&self, _since: u64) -> JournalSnapshot {
        JournalSnapshot {
            entries: vec![],
            oldest_seq: 1,
            latest_seq: 0,
        }
    }

    fn command(&self, _conn_id: u64, cmd: Command) {
        self.commands.lock().unwrap().push(cmd);
    }

    fn resolve_scope(&self, name: &str) -> Option<Scope> {
        (name == aegis_ipc::LOCAL_PORTAL_SCOPE).then(|| Scope {
            ops: Some(vec![
                OpClass::CaptureOutput,
                OpClass::StreamOutput,
                OpClass::IdleInhibit,
                OpClass::PickTarget,
                OpClass::PickFile,
                OpClass::PickApp,
                OpClass::Notify,
                OpClass::DismissNotification,
            ]),
            ..Scope::default()
        })
    }
}

#[test]
fn notification_end_to_end() {
    let Some(bus) = private_bus() else {
        eprintln!("notification_end_to_end: no dbus-daemon, skipping");
        return;
    };
    let conn = bus.connect();

    let data_dir = temp_dir("data");
    let runtime_dir = temp_dir("runtime");

    let fake = Arc::new(FakeCompositor {
        commands: Mutex::new(Vec::new()),
    });
    let _server = Server::start(&runtime_dir.join("aegis.sock"), Arc::clone(&fake))
        .expect("bind fake compositor IPC");

    let _daemon = KillOnDrop(spawn_daemon(&bus, &data_dir, &runtime_dir));

    wait_for_name(&conn, PORTAL);
    let portal = Proxy::new(&conn, PORTAL, DESKTOP_PATH, IFACE).expect("notification proxy");

    let version: u32 = portal.get_property("version").expect("version property");
    assert_eq!(version, 2);
    let supported: std::collections::HashMap<String, OwnedValue> = portal
        .get_property("SupportedOptions")
        .expect("SupportedOptions property");
    assert!(supported.is_empty());

    // -- AddNotification: title/body/app id/external id reach the queue.
    let mut notification: std::collections::HashMap<String, Value<'_>> =
        std::collections::HashMap::new();
    notification.insert("title".to_string(), Value::from("New mail"));
    notification.insert("body".to_string(), Value::from("You have two messages"));
    portal
        .call::<_, _, ()>(
            "AddNotification",
            &("dev.aegis.smoke", "mail-2", notification),
        )
        .expect("AddNotification");

    // -- RemoveNotification: resolves (app_id, id) → compositor id 41 and
    // dismisses it; an unknown id is a no-op.
    portal
        .call::<_, _, ()>("RemoveNotification", &("dev.aegis.smoke", "mail-1"))
        .expect("RemoveNotification");
    portal
        .call::<_, _, ()>("RemoveNotification", &("dev.aegis.smoke", "no-such-id"))
        .expect("RemoveNotification of an unknown id");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        {
            let commands = fake.commands.lock().unwrap();
            let notify = commands.iter().find_map(|cmd| match cmd {
                Command::Notify {
                    summary,
                    body,
                    app_id,
                    external_id,
                } => Some((
                    summary.clone(),
                    body.clone(),
                    app_id.clone(),
                    external_id.clone(),
                )),
                _ => None,
            });
            let dismisses: Vec<u64> = commands
                .iter()
                .filter_map(|cmd| match cmd {
                    Command::DismissNotification { id } => Some(*id),
                    _ => None,
                })
                .collect();
            if let (Some((summary, body, app_id, external_id)), true) =
                (notify, !dismisses.is_empty())
            {
                assert_eq!(summary, "New mail");
                assert_eq!(body, "You have two messages");
                assert_eq!(app_id.as_deref(), Some("dev.aegis.smoke"));
                assert_eq!(external_id.as_deref(), Some("mail-2"));
                assert_eq!(dismisses, [41], "exactly the matched entry is dismissed");
                break;
            }
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the notification commands did not arrive within 5 s"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&runtime_dir);
}
