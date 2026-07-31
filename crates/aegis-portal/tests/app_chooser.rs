//! End-to-end exercise of the AppChooser backend: the real `aegis-portal`
//! daemon on a private session bus (see `tests/common/`), plus a fake
//! compositor IPC server answering `PickApp` with scripted results.
//!
//! ```sh
//! cargo test -p aegis-portal --test app_chooser
//! ```
//!
//! Without `dbus-daemon` available the test skips cleanly.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use aegis_ipc::{
    AppPickResult, Capabilities, FilePickOptions, FilePickResult, Handler, JournalSnapshot,
    OpClass, Scope, Server,
};
use zbus::blocking::Proxy;
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

mod common;
use common::{KillOnDrop, private_bus, spawn_daemon, temp_dir, wait_for_name};

const PORTAL: &str = "org.freedesktop.impl.portal.desktop.aegis";
const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.impl.portal.AppChooser";

/// One recorded `PickApp` request: choices, subject, last_choice.
type RecordedPick = (Vec<String>, Option<String>, Option<String>);

/// A fake compositor: records every `PickApp` request and answers from a
/// scripted result queue (last entry repeats).
struct FakeCompositor {
    picks: Mutex<Vec<RecordedPick>>,
    results: Mutex<VecDeque<AppPickResult>>,
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
            ops: Some(vec![
                OpClass::CaptureOutput,
                OpClass::StreamOutput,
                OpClass::IdleInhibit,
                OpClass::PickTarget,
                OpClass::PickFile,
                OpClass::PickApp,
            ]),
            ..Scope::default()
        })
    }

    fn pick_file(
        &self,
        _conn_id: u64,
        _options: FilePickOptions,
    ) -> Result<FilePickResult, String> {
        Err("no file pick expected in this test".into())
    }

    fn pick_app(
        &self,
        _conn_id: u64,
        choices: Vec<String>,
        subject: Option<String>,
        last_choice: Option<String>,
    ) -> Result<AppPickResult, String> {
        self.picks
            .lock()
            .unwrap()
            .push((choices, subject, last_choice));
        let mut results = self.results.lock().unwrap();
        let result = results.front().cloned().unwrap_or(AppPickResult::Cancelled);
        if results.len() > 1 {
            results.pop_front();
        }
        Ok(result)
    }
}

#[test]
fn app_chooser_end_to_end() {
    let Some(bus) = private_bus() else {
        eprintln!("app_chooser_end_to_end: no dbus-daemon, skipping");
        return;
    };
    let conn = bus.connect();

    let data_dir = temp_dir("data");
    let runtime_dir = temp_dir("runtime");

    let fake = Arc::new(FakeCompositor {
        picks: Mutex::new(Vec::new()),
        results: Mutex::new(VecDeque::from(vec![AppPickResult::App {
            id: "org.example.Chosen.desktop".to_string(),
        }])),
    });
    let _server = Server::start(&runtime_dir.join("aegis.sock"), Arc::clone(&fake))
        .expect("bind fake compositor IPC");

    let _daemon = KillOnDrop(spawn_daemon(&bus, &data_dir, &runtime_dir));

    wait_for_name(&conn, PORTAL);
    let chooser = Proxy::new(&conn, PORTAL, DESKTOP_PATH, IFACE).expect("app chooser proxy");

    let version: u32 = chooser.get_property("version").expect("version property");
    assert_eq!(version, 2);

    // -- ChooseApplication: the candidate list, subject, and last_choice
    // reach the picker; the chosen id comes back as `choice`.
    let choices = vec![
        "firefox.desktop".to_string(),
        "org.example.Chosen.desktop".to_string(),
    ];
    let mut options: HashMap<String, Value<'_>> = HashMap::new();
    options.insert("filename".to_string(), Value::from("report.pdf"));
    options.insert("content_type".to_string(), Value::from("application/pdf"));
    options.insert(
        "last_choice".to_string(),
        Value::from("org.example.Chosen.desktop"),
    );
    let handle = ObjectPath::try_from("/org/freedesktop/portal/desktop/request/1/ac1")
        .expect("request handle path");
    let (response, results): (u32, HashMap<String, OwnedValue>) = chooser
        .call(
            "ChooseApplication",
            &(handle, "dev.aegis.smoke", "", choices, options),
        )
        .expect("ChooseApplication");
    assert_eq!(response, 0, "ChooseApplication must report success");
    let choice = results
        .get("choice")
        .map(|v| String::try_from(v.clone()).expect("choice is a string"));
    assert_eq!(choice.as_deref(), Some("org.example.Chosen.desktop"));
    {
        let picks = fake.picks.lock().unwrap();
        let (got_choices, subject, last_choice) = picks.last().expect("a pick was recorded");
        assert_eq!(got_choices.len(), 2);
        // filename wins over content_type as the subject line.
        assert_eq!(subject.as_deref(), Some("report.pdf"));
        assert_eq!(last_choice.as_deref(), Some("org.example.Chosen.desktop"));
    }

    // -- An empty candidate list is rejected as invalid arguments.
    let handle = ObjectPath::try_from("/org/freedesktop/portal/desktop/request/1/ac2")
        .expect("request handle path");
    let err = chooser
        .call::<_, _, ()>(
            "ChooseApplication",
            &(
                handle,
                "dev.aegis.smoke",
                "",
                Vec::<String>::new(),
                HashMap::<String, Value<'_>>::new(),
            ),
        )
        .expect_err("empty choices must be rejected");
    assert!(err.to_string().contains("InvalidArgs"), "{err}");

    // -- A user cancellation maps to response 1.
    *fake.results.lock().unwrap() = VecDeque::from(vec![AppPickResult::Cancelled]);
    let handle = ObjectPath::try_from("/org/freedesktop/portal/desktop/request/1/ac3")
        .expect("request handle path");
    let (response, _): (u32, HashMap<String, OwnedValue>) = chooser
        .call(
            "ChooseApplication",
            &(
                handle,
                "dev.aegis.smoke",
                "",
                vec!["firefox.desktop".to_string()],
                HashMap::<String, Value<'_>>::new(),
            ),
        )
        .expect("ChooseApplication");
    assert_eq!(response, 1, "a dismissed picker must answer 1 (cancelled)");

    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&runtime_dir);
}
