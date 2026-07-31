//! End-to-end exercise of the Wallpaper backend: the real `aegis-portal`
//! daemon on a private session bus (see `tests/common/`), plus a fake
//! compositor answering `PickConfirm` and recording `SetWallpaper`.
//!
//! ```sh
//! cargo test -p aegis-portal --test wallpaper
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
const IFACE: &str = "org.freedesktop.impl.portal.Wallpaper";

struct FakeCompositor {
    confirms: Mutex<Vec<(String, String)>>,
    wallpapers: Mutex<Vec<std::path::PathBuf>>,
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
            ops: Some(vec![OpClass::PickConfirm, OpClass::SetWallpaper]),
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
        self.confirms.lock().unwrap().push((title, body));
        Ok(*self.answer.lock().unwrap())
    }

    fn set_wallpaper(&self, _conn_id: u64, path: std::path::PathBuf) -> Result<(), String> {
        self.wallpapers.lock().unwrap().push(path);
        Ok(())
    }
}

#[test]
fn wallpaper_end_to_end() {
    let Some(bus) = private_bus() else {
        eprintln!("wallpaper_end_to_end: no dbus-daemon, skipping");
        return;
    };
    let conn = bus.connect();

    let data_dir = temp_dir("data");
    let runtime_dir = temp_dir("runtime");
    let image = runtime_dir.join("new wall.png");
    std::fs::write(&image, b"png").expect("write fixture image");

    let fake = Arc::new(FakeCompositor {
        confirms: Mutex::new(Vec::new()),
        wallpapers: Mutex::new(Vec::new()),
        answer: Mutex::new(ConfirmPickResult::Confirmed),
    });
    let _server = Server::start(&runtime_dir.join("aegis.sock"), Arc::clone(&fake))
        .expect("bind fake compositor IPC");

    let _daemon = KillOnDrop(spawn_daemon(&bus, &data_dir, &runtime_dir));
    wait_for_name(&conn, PORTAL);

    let wallpaper = Proxy::new(&conn, PORTAL, DESKTOP_PATH, IFACE).expect("wallpaper proxy");
    let version: u32 = wallpaper.get_property("version").expect("version property");
    assert_eq!(version, 1);

    // -- A consented swap reaches the compositor with the decoded path.
    let uri = format!(
        "file://{}",
        image.to_str().expect("utf8 path").replace(' ', "%20")
    );
    let handle = ObjectPath::try_from("/org/freedesktop/portal/desktop/request/1/wp1")
        .expect("request handle path");
    let (response, _): (u32, HashMap<String, OwnedValue>) = wallpaper
        .call(
            "SetWallpaperURI",
            &(
                handle,
                "dev.aegis.smoke",
                "",
                uri.as_str(),
                HashMap::<String, Value<'_>>::new(),
            ),
        )
        .expect("SetWallpaperURI");
    assert_eq!(response, 0, "a consented swap must report success");
    assert_eq!(
        fake.wallpapers.lock().unwrap().as_slice(),
        std::slice::from_ref(&image)
    );
    {
        let confirms = fake.confirms.lock().unwrap();
        let (title, body) = confirms.first().expect("a consent prompt ran");
        assert_eq!(title, "Change Wallpaper");
        assert!(
            body.contains("dev.aegis.smoke") && body.contains("new wall.png"),
            "{body}"
        );
    }

    // -- A declined consent applies nothing.
    *fake.answer.lock().unwrap() = ConfirmPickResult::Cancelled;
    let handle = ObjectPath::try_from("/org/freedesktop/portal/desktop/request/1/wp2")
        .expect("request handle path");
    let (response, results): (u32, HashMap<String, OwnedValue>) = wallpaper
        .call(
            "SetWallpaperURI",
            &(
                handle,
                "dev.aegis.smoke",
                "",
                "file:///tmp/other.png",
                HashMap::<String, Value<'_>>::new(),
            ),
        )
        .expect("SetWallpaperURI");
    assert_eq!(response, 1, "a declined consent must answer 1");
    assert!(results.is_empty());
    assert_eq!(fake.wallpapers.lock().unwrap().len(), 1);

    // -- A non-file URI is rejected as invalid arguments.
    let handle = ObjectPath::try_from("/org/freedesktop/portal/desktop/request/1/wp3")
        .expect("request handle path");
    let err = wallpaper
        .call::<_, _, ()>(
            "SetWallpaperURI",
            &(
                handle,
                "dev.aegis.smoke",
                "",
                "https://example.com/x.png",
                HashMap::<String, Value<'_>>::new(),
            ),
        )
        .expect_err("a non-file URI must be rejected");
    assert!(err.to_string().contains("InvalidArgs"), "{err}");

    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&runtime_dir);
}
