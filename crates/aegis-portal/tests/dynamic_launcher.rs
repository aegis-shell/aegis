//! End-to-end exercise of the DynamicLauncher backend: the real
//! `aegis-portal` daemon on a private session bus (see `tests/common/`),
//! plus a fake compositor answering `PickConfirm`.
//!
//! ```sh
//! cargo test -p aegis-portal --test dynamic_launcher
//! ```
//!
//! Without `dbus-daemon` available the test skips cleanly.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use aegis_ipc::{
    Capabilities, ConfirmPickResult, Handler, JournalSnapshot, OpClass, Scope, Server,
};
use zbus::blocking::Proxy;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

mod common;
use common::{KillOnDrop, private_bus, spawn_daemon, temp_dir, wait_for_name};

const PORTAL: &str = "org.freedesktop.impl.portal.desktop.aegis";
const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.impl.portal.DynamicLauncher";

struct FakeCompositor {
    prompts: Mutex<Vec<(String, String)>>,
    answer: Mutex<ConfirmPickResult>,
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
        Vec::new()
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

    fn command(&self, _conn_id: u64, _cmd: aegis_ipc::Command) {}

    fn resolve_scope(&self, name: &str) -> Option<Scope> {
        (name == aegis_ipc::LOCAL_PORTAL_SCOPE).then(|| Scope {
            ops: Some(vec![OpClass::PickConfirm]),
            ..Scope::default()
        })
    }

    fn pick_confirm(
        &self,
        _conn_id: u64,
        title: String,
        body: String,
        _accept_label: Option<String>,
    ) -> Result<ConfirmPickResult, String> {
        self.prompts.lock().unwrap().push((title, body));
        Ok(*self.answer.lock().unwrap())
    }
}

#[test]
fn dynamic_launcher_end_to_end() {
    let Some(bus) = private_bus() else {
        eprintln!("dynamic_launcher_end_to_end: no dbus-daemon, skipping");
        return;
    };
    let conn = bus.connect();

    let data_dir = temp_dir("data");
    let runtime_dir = temp_dir("runtime");

    let fake = Arc::new(FakeCompositor {
        prompts: Mutex::new(Vec::new()),
        answer: Mutex::new(ConfirmPickResult::Confirmed),
    });
    let _server = Server::start(&runtime_dir.join("aegis.sock"), Arc::clone(&fake))
        .expect("bind fake compositor IPC");

    let _daemon = KillOnDrop(spawn_daemon(&bus, &data_dir, &runtime_dir));
    wait_for_name(&conn, PORTAL);

    let launcher = Proxy::new(&conn, PORTAL, DESKTOP_PATH, IFACE).expect("dynamic launcher proxy");
    let version: u32 = launcher.get_property("version").expect("version property");
    assert_eq!(version, 1);
    let types: u32 = launcher
        .get_property("SupportedLauncherTypes")
        .expect("SupportedLauncherTypes property");
    assert_eq!(types, 1, "only Application launchers are supported");

    // -- PrepareInstall: consent → echoed name/icon + a fresh token.
    let icon_bytes: Vec<u8> = vec![1, 2, 3, 4];
    let handle = ObjectPath::try_from("/org/freedesktop/portal/desktop/request/1/dl1")
        .expect("request handle path");
    let (response, results): (u32, HashMap<String, OwnedValue>) = launcher
        .call(
            "PrepareInstall",
            &(
                handle,
                "dev.aegis.smoke",
                "",
                "Recipe Box",
                Value::from(icon_bytes.clone()),
                HashMap::<String, Value<'_>>::new(),
            ),
        )
        .expect("PrepareInstall");
    assert_eq!(response, 0, "a consented install must report success");
    let name = String::try_from(results["name"].clone()).expect("name is a string");
    assert_eq!(name, "Recipe Box");
    let icon: Vec<u8> = Vec::try_from(Value::from(results["icon"].clone())).expect("icon echoes");
    assert_eq!(icon, icon_bytes);
    let token = String::try_from(results["token"].clone()).expect("token is a string");
    assert_eq!(token.len(), 32, "the token is 16 random bytes as hex");
    {
        let prompts = fake.prompts.lock().unwrap();
        let (title, body) = prompts.first().expect("a consent prompt ran");
        assert_eq!(title, "Install Launcher");
        assert!(
            body.contains("dev.aegis.smoke") && body.contains("Recipe Box"),
            "{body}"
        );
    }

    // -- RequestInstallToken: non-interactive installs are never allowed.
    let response: u32 = launcher
        .call(
            "RequestInstallToken",
            &("dev.aegis.smoke", HashMap::<String, Value<'_>>::new()),
        )
        .expect("RequestInstallToken");
    assert_ne!(response, 0, "install tokens must never bypass consent");

    // -- A declined consent answers 1.
    *fake.answer.lock().unwrap() = ConfirmPickResult::Cancelled;
    let handle = ObjectPath::try_from("/org/freedesktop/portal/desktop/request/1/dl2")
        .expect("request handle path");
    let (response, results): (u32, HashMap<String, OwnedValue>) = launcher
        .call(
            "PrepareInstall",
            &(
                handle,
                "dev.aegis.smoke",
                "",
                "Nope",
                Value::from(icon_bytes),
                HashMap::<String, Value<'_>>::new(),
            ),
        )
        .expect("PrepareInstall");
    assert_eq!(response, 1, "a declined consent must answer 1");
    assert!(results.is_empty());

    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&runtime_dir);
}
