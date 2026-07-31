//! End-to-end exercise of the Account backend: the real `aegis-portal`
//! daemon on a private session bus (see `tests/common/`), plus a fake
//! compositor answering `PickConfirm`. An affirmative answer releases the
//! identity; a declined one answers 1.
//!
//! ```sh
//! cargo test -p aegis-portal --test account
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
const IFACE: &str = "org.freedesktop.impl.portal.Account";

/// A fake compositor: records every `PickConfirm` request and answers with
/// a scripted outcome.
struct FakeCompositor {
    prompts: Mutex<Vec<(String, String)>>,
    answer: ConfirmPickResult,
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
        Ok(self.answer)
    }
}

/// Run one GetUserInformation against the scripted answer, returning the
/// `(response, results)` pair.
fn get_user_information(
    answer: ConfirmPickResult,
    with_avatar: bool,
) -> Option<(u32, HashMap<String, OwnedValue>, Arc<FakeCompositor>)> {
    let Some(bus) = private_bus() else {
        eprintln!("account: no dbus-daemon, skipping");
        return None;
    };
    let conn = bus.connect();

    let data_dir = temp_dir("data");
    let runtime_dir = temp_dir("runtime");
    if with_avatar {
        let avatars = data_dir.join("aegis/avatars");
        std::fs::create_dir_all(&avatars).expect("avatar dir");
        std::fs::write(avatars.join("face.png"), b"png").expect("avatar fixture");
    }

    let fake = Arc::new(FakeCompositor {
        prompts: Mutex::new(Vec::new()),
        answer,
    });
    let _server = Server::start(&runtime_dir.join("aegis.sock"), Arc::clone(&fake))
        .expect("bind fake compositor IPC");

    let _daemon = KillOnDrop(spawn_daemon(&bus, &data_dir, &runtime_dir));
    wait_for_name(&conn, PORTAL);

    let account = Proxy::new(&conn, PORTAL, DESKTOP_PATH, IFACE).expect("account proxy");
    let version: u32 = account.get_property("version").expect("version property");
    assert_eq!(version, 1);

    let mut options: HashMap<String, Value<'_>> = HashMap::new();
    options.insert(
        "reason".to_string(),
        Value::from("Allows your personal information to be included in recipes you share."),
    );
    let handle = ObjectPath::try_from("/org/freedesktop/portal/desktop/request/1/acc1")
        .expect("request handle path");
    let (response, results): (u32, HashMap<String, OwnedValue>) = account
        .call(
            "GetUserInformation",
            &(handle, "dev.aegis.smoke", "", options),
        )
        .expect("GetUserInformation");
    let result = (response, results, fake);
    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&runtime_dir);
    Some(result)
}

#[test]
fn consent_shares_the_identity_with_avatar() {
    let Some((response, results, fake)) = get_user_information(ConfirmPickResult::Confirmed, true)
    else {
        return;
    };
    assert_eq!(response, 0, "an affirmative answer must report success");
    let id = String::try_from(results["id"].clone()).expect("id is a string");
    let name = String::try_from(results["name"].clone()).expect("name is a string");
    assert!(!id.is_empty() && !name.is_empty());
    let image = String::try_from(results["image"].clone()).expect("image is a string");
    assert!(
        image.starts_with("file://") && image.contains("aegis/avatars/face.png"),
        "the avatar URI must point at the canonical location: {image}"
    );
    let prompts = fake.prompts.lock().unwrap();
    let (title, body) = prompts.first().expect("a consent prompt ran");
    assert_eq!(title, "Share Personal Information");
    assert!(body.contains("dev.aegis.smoke"), "{body}");
    assert!(
        body.contains("recipes"),
        "the reason reaches the dialog: {body}"
    );
}

#[test]
fn a_declined_consent_releases_nothing() {
    let Some((response, results, _)) = get_user_information(ConfirmPickResult::Cancelled, false)
    else {
        return;
    };
    assert_eq!(response, 1, "a declined consent must answer 1");
    assert!(results.is_empty());
}
