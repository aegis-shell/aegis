//! End-to-end exercise of the FileChooser backend: the real `aegis-portal`
//! daemon on a private session bus (see `tests/common/`), plus a fake
//! compositor IPC server answering `PickFile` with scripted results.
//! Verifies the D-Bus → IPC → results mapping without a display.
//!
//! ```sh
//! cargo test -p aegis-portal --test file_chooser
//! ```
//!
//! Without `dbus-daemon` available the test skips cleanly.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use aegis_ipc::{
    Capabilities, FilePickOptions, FilePickResult, Handler, JournalSnapshot, OpClass, Scope, Server,
};
use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{ObjectPath, OwnedValue, Value};

mod common;
use common::{KillOnDrop, private_bus, spawn_daemon, temp_dir, wait_for_name};

const PORTAL: &str = "org.freedesktop.impl.portal.desktop.aegis";
const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.impl.portal.FileChooser";

/// A fake compositor: records every `PickFile` request and answers from a
/// scripted result queue (last entry repeats).
struct FakeCompositor {
    picks: Mutex<Vec<FilePickOptions>>,
    results: Mutex<VecDeque<FilePickResult>>,
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
            ]),
            ..Scope::default()
        })
    }

    fn pick_file(&self, _conn_id: u64, options: FilePickOptions) -> Result<FilePickResult, String> {
        self.picks.lock().unwrap().push(options);
        let mut results = self.results.lock().unwrap();
        let result = results
            .front()
            .cloned()
            .unwrap_or(FilePickResult::Cancelled);
        if results.len() > 1 {
            results.pop_front();
        }
        Ok(result)
    }
}

fn chooser(conn: &Connection) -> Proxy<'_> {
    Proxy::new(conn, PORTAL, DESKTOP_PATH, IFACE).expect("file chooser proxy")
}

/// Call one chooser method with a unique request handle.
fn call_chooser(
    proxy: &Proxy<'_>,
    method: &str,
    title: &str,
    options: HashMap<String, Value<'_>>,
    serial: u32,
) -> (u32, HashMap<String, OwnedValue>) {
    let handle = ObjectPath::try_from(format!(
        "/org/freedesktop/portal/desktop/request/1/fc{serial}"
    ))
    .expect("request handle path");
    proxy
        .call(method, &(handle, "dev.aegis.smoke", "", title, options))
        .unwrap_or_else(|e| panic!("{method} must succeed at the bus level: {e}"))
}

fn result_uris(results: &HashMap<String, OwnedValue>) -> Vec<String> {
    let uris = results
        .get("uris")
        .unwrap_or_else(|| panic!("results must contain uris: {results:?}"));
    let value: Value<'_> = uris.clone().into();
    Vec::<String>::try_from(value).expect("uris is a string array")
}

#[test]
fn file_chooser_end_to_end() {
    let Some(bus) = private_bus() else {
        eprintln!("file_chooser_end_to_end: no dbus-daemon, skipping");
        return;
    };
    let conn = bus.connect();

    let data_dir = temp_dir("data");
    let runtime_dir = temp_dir("runtime");

    // The fake compositor's IPC socket must exist before the daemon's worker
    // first connects (workers connect lazily, but keep it deterministic).
    let fake = Arc::new(FakeCompositor {
        picks: Mutex::new(Vec::new()),
        results: Mutex::new(VecDeque::from(vec![FilePickResult::Paths {
            paths: vec![std::path::PathBuf::from("/tmp/fake-chosen.txt")],
            filter: Some(0),
        }])),
    });
    let _server = Server::start(&runtime_dir.join("aegis.sock"), Arc::clone(&fake))
        .expect("bind fake compositor IPC");

    let _daemon = KillOnDrop(spawn_daemon(&bus, &data_dir, &runtime_dir));

    wait_for_name(&conn, PORTAL);
    let chooser = chooser(&conn);

    let version: u32 = chooser.get_property("version").expect("version property");
    assert_eq!(version, 3);

    // -- OpenFile with a filter: response 0, the fake's file URI, and the
    // selected-filter echo.
    let rules: Vec<(u32, String)> = vec![(0, "*.txt".to_string())];
    let mut options: HashMap<String, Value<'_>> = HashMap::new();
    options.insert(
        "filters".to_string(),
        Value::from(vec![("Text".to_string(), rules)]),
    );
    let (response, results) = call_chooser(&chooser, "OpenFile", "Open it", options, 1);
    assert_eq!(response, 0, "OpenFile must report success: {results:?}");
    assert_eq!(result_uris(&results), ["file:///tmp/fake-chosen.txt"]);
    assert!(results.contains_key("current_filter"));
    assert!(results.contains_key("choices"));
    {
        let picks = fake.picks.lock().unwrap();
        let last = picks.last().expect("a pick was recorded");
        assert_eq!(last.mode, aegis_ipc::FilePickMode::Open);
        assert_eq!(last.filters.len(), 1);
        assert_eq!(last.filters[0].label, "Text");
        assert_eq!(last.filters[0].patterns, ["*.txt"]);
    }

    // -- SaveFile: current_folder/current_name reach the picker unchanged.
    *fake.results.lock().unwrap() = VecDeque::from(vec![FilePickResult::Paths {
        paths: vec![std::path::PathBuf::from("/tmp/fake-save.png")],
        filter: None,
    }]);
    let mut options: HashMap<String, Value<'_>> = HashMap::new();
    let mut folder = b"/tmp".to_vec();
    folder.push(0);
    options.insert("current_folder".to_string(), Value::from(folder));
    options.insert("current_name".to_string(), Value::from("out.png"));
    let (response, results) = call_chooser(&chooser, "SaveFile", "Save it", options, 2);
    assert_eq!(response, 0, "SaveFile must report success: {results:?}");
    assert_eq!(result_uris(&results), ["file:///tmp/fake-save.png"]);
    {
        let picks = fake.picks.lock().unwrap();
        let last = picks.last().expect("a pick was recorded");
        assert_eq!(last.mode, aegis_ipc::FilePickMode::Save);
        assert_eq!(last.current_name.as_deref(), Some("out.png"));
        assert_eq!(
            last.current_folder.as_deref(),
            Some(std::path::Path::new("/tmp"))
        );
    }

    // -- SaveFiles: the suggested names are appended to the chosen folder.
    *fake.results.lock().unwrap() = VecDeque::from(vec![FilePickResult::Paths {
        paths: vec![std::path::PathBuf::from("/chosen/dir")],
        filter: None,
    }]);
    let mut options: HashMap<String, Value<'_>> = HashMap::new();
    let names: Vec<Vec<u8>> = vec![b"one.txt\0".to_vec(), b"two.txt\0".to_vec()];
    options.insert("files".to_string(), Value::from(names));
    let (response, results) = call_chooser(&chooser, "SaveFiles", "Save them", options, 3);
    assert_eq!(response, 0, "SaveFiles must report success: {results:?}");
    assert_eq!(
        result_uris(&results),
        ["file:///chosen/dir/one.txt", "file:///chosen/dir/two.txt"]
    );
    {
        let picks = fake.picks.lock().unwrap();
        let last = picks.last().expect("a pick was recorded");
        assert_eq!(last.mode, aegis_ipc::FilePickMode::ChooseDir);
    }

    // -- A user cancellation maps to response 1.
    *fake.results.lock().unwrap() = VecDeque::from(vec![FilePickResult::Cancelled]);
    let (response, _results) = call_chooser(&chooser, "OpenFile", "Cancel it", HashMap::new(), 4);
    assert_eq!(response, 1, "a dismissed picker must answer 1 (cancelled)");

    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&runtime_dir);
}
