//! Manual end-to-end smoke test for the SNI tray, against a real session
//! bus. Run it on a private bus so it cannot clash with a running desktop's
//! own StatusNotifierWatcher:
//!
//! ```sh
//! dbus-run-session -- cargo test -p aegis-tray --test tray_smoke -- --ignored --nocapture
//! ```
//!
//! Exercises the full loop: watcher name ownership, item registration,
//! property mirroring into the snapshot, pixmap conversion, and `Activate`
//! delivery back to the item. Not run by default (needs a session bus with
//! a free `org.kde.StatusNotifierWatcher` name).

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aegis_tray::{self, TrayCommand, TrayIcon};

/// A minimal fake StatusNotifierItem: fixed properties, and a record of the
/// activation methods the watcher invoked.
struct FakeItem {
    calls: Arc<Mutex<Vec<String>>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierItem")]
impl FakeItem {
    async fn activate(&self, x: i32, y: i32) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("Activate({x},{y})"));
    }

    async fn secondary_activate(&self, x: i32, y: i32) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("SecondaryActivate({x},{y})"));
    }

    async fn context_menu(&self, x: i32, y: i32) {
        self.calls
            .lock()
            .unwrap()
            .push(format!("ContextMenu({x},{y})"));
    }

    #[zbus(property)]
    fn id(&self) -> &str {
        // A synchronous item may not dispatch property requests until its
        // RegisterStatusNotifierItem call returns. A slow getter models that
        // shape and guards the watcher from putting property I/O on the
        // registration reply path.
        std::thread::sleep(Duration::from_millis(750));
        "fake-item"
    }

    #[zbus(property)]
    fn title(&self) -> &str {
        "Fake Item"
    }

    #[zbus(property)]
    fn status(&self) -> &str {
        "Active"
    }

    #[zbus(property)]
    fn icon_name(&self) -> &str {
        "fake-icon"
    }

    /// A single 2x2 red pixmap (ARGB32, network byte order).
    #[zbus(property)]
    fn icon_pixmap(&self) -> Vec<(i32, i32, Vec<u8>)> {
        vec![(2, 2, [0xff, 0xff, 0x00, 0x00].repeat(4))]
    }

    #[zbus(property)]
    fn menu(&self) -> zbus::zvariant::ObjectPath<'static> {
        zbus::zvariant::ObjectPath::from_static_str("/").unwrap()
    }
}

fn wait_for(condition: impl Fn() -> bool) -> bool {
    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline {
        if condition() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(25));
    }
    false
}

#[test]
#[ignore = "needs a session bus with a free StatusNotifierWatcher name"]
fn watcher_registers_item_and_delivers_activate() {
    let Some(tray) = aegis_tray::spawn() else {
        eprintln!("no session bus or watcher name taken; skipping");
        return;
    };

    // Client connection serving the fake item at its default path.
    let calls = Arc::new(Mutex::new(Vec::new()));
    let client = zbus::blocking::connection::Builder::session()
        .unwrap()
        .serve_at(
            "/StatusNotifierItem",
            FakeItem {
                calls: Arc::clone(&calls),
            },
        )
        .unwrap()
        .name("org.example.FakeItem")
        .unwrap()
        .build()
        .unwrap();

    let watcher = zbus::blocking::Proxy::new(
        &client,
        "org.kde.StatusNotifierWatcher",
        "/StatusNotifierWatcher",
        "org.kde.StatusNotifierWatcher",
    )
    .unwrap();
    let register_started = Instant::now();
    watcher
        .call::<_, _, ()>("RegisterStatusNotifierItem", &"org.example.FakeItem")
        .unwrap();
    assert!(
        register_started.elapsed() < Duration::from_millis(500),
        "registration waited for item properties"
    );

    // The item shows up in the snapshot, mirrored from the fake properties.
    let key = "org.example.FakeItem/StatusNotifierItem";
    assert!(wait_for(|| {
        tray.snapshot()
            .lock()
            .unwrap()
            .items
            .iter()
            .any(|item| item.key == key && item.title == "Fake Item")
    }));
    {
        let snapshot = tray.snapshot().lock().unwrap();
        let item = snapshot.items.iter().find(|item| item.key == key).unwrap();
        assert_eq!(item.title, "Fake Item");
        assert!(item.is_visible());
        assert!(!item.has_menu);
        match &item.icon {
            TrayIcon::Pixmap(pixmap) => {
                assert_eq!((pixmap.width, pixmap.height), (2, 2));
                // ARGB red (ff ff 00 00) becomes BGRA (00 00 ff ff).
                assert_eq!(&pixmap.bgra[..4], &[0x00, 0x00, 0xff, 0xff]);
            }
            other => panic!("expected a pixmap icon, got {other:?}"),
        }
    }

    // The watcher's own property lists the registration.
    let registered: Vec<String> = watcher
        .get_property("RegisteredStatusNotifierItems")
        .unwrap();
    assert!(registered.contains(&key.to_string()));
    let host_registered: bool = watcher
        .get_property("IsStatusNotifierHostRegistered")
        .unwrap();
    assert!(host_registered);

    // A left click from the shell reaches the item as Activate(x, y).
    tray.send(TrayCommand::Activate {
        key: key.to_string(),
        x: 10,
        y: 20,
    })
    .unwrap();
    assert!(wait_for(|| calls
        .lock()
        .unwrap()
        .contains(&"Activate(10,20)".to_string())));

    // When the item's connection drops, the watcher forgets it. The proxy
    // holds a connection clone, so it must go first.
    drop(watcher);
    drop(client);
    assert!(wait_for(|| !tray
        .snapshot()
        .lock()
        .unwrap()
        .items
        .iter()
        .any(|item| item.key == key)));
}
