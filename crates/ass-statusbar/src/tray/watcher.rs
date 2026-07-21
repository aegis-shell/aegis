//! The StatusNotifierWatcher/Host D-Bus machinery.
//!
//! Three roles, all on the session bus:
//!
//! - the served `org.kde.StatusNotifierWatcher` interface (method calls are
//!   dispatched by zbus's own internal executor; the handlers are async only
//!   because that is what the served-interface API drives),
//! - a signal thread that consumes a single blocking message iterator
//!   (`NameOwnerChanged` for liveness, the item's `New*`/`PropertiesChanged`
//!   signals for icon/status refreshes, and `LayoutUpdated` on any open
//!   dbusmenu) and rewrites the shared snapshot,
//! - a command thread that takes [`TrayCommand`]s off an mpsc channel and
//!   invokes `Activate`/`SecondaryActivate`/`ContextMenu` on the item, plus
//!   the dbusmenu `GetLayout`/`Event` methods when the bar requests a
//!   host-rendered context menu.
//!
//! Everything the compositor touches crosses the thread boundary as plain
//! values through `Arc<Mutex<Shared>>`; the compositor's render thread never
//! blocks on D-Bus.

use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::{SystemTime, UNIX_EPOCH};

use zbus::zvariant::{OwnedObjectPath, Value};

use super::{
    LayoutReply, MenuState, TrayCommand, TrayIcon, TrayItem, TrayPixmap, TraySnapshot, TrayStatus,
    argb32_to_bgra, parse_layout_reply, select_pixmap,
};

const WATCHER_NAME: &str = "org.kde.StatusNotifierWatcher";
const WATCHER_PATH: &str = "/StatusNotifierWatcher";
const WATCHER_IFACE: &str = "org.kde.StatusNotifierWatcher";
const ITEM_IFACE: &str = "org.kde.StatusNotifierItem";
const DEFAULT_ITEM_PATH: &str = "/StatusNotifierItem";
const DBUS_IFACE: &str = "org.freedesktop.DBus";
const PROPERTIES_IFACE: &str = "org.freedesktop.DBus.Properties";
const DBUSMENU_IFACE: &str = "com.canonical.dbusmenu";
/// Preferred icon size: the bar renders 18px cells, so 48 covers 2x output
/// scales with headroom; the closest available size wins.
const PIXMAP_TARGET: i32 = 48;
/// Upper bound for the signal queue before the bus starts dropping for us.
const SIGNAL_QUEUE_DEPTH: usize = 2048;

/// Properties mirrored from one StatusNotifierItem.
#[derive(Default)]
struct ItemProps {
    title: String,
    icon: TrayIcon,
    status: TrayStatus,
    has_menu: bool,
    /// dbusmenu object path (the SNI `Menu` property). `None` when the item
    /// exposes no menu or registers it as `/`.
    menu_path: Option<String>,
}

/// A tracked item: identity for the bus plus the mirrored render state.
struct ItemEntry {
    /// Bus name as registered (well-known or unique).
    bus_name: String,
    /// Unique name of the registrant's connection; used as the signal-sender
    /// match and as the destination for method calls (always routable).
    unique_name: String,
    path: String,
    /// dbusmenu object path; the proxy target for `GetLayout`/`Event`/signals.
    menu_path: Option<String>,
    item: TrayItem,
}

#[derive(Default)]
struct Shared {
    items: HashMap<String, ItemEntry>,
    snapshot: Option<Arc<Mutex<TraySnapshot>>>,
    /// The currently open dbusmenu popover (the worker refreshes it on
    /// `LayoutUpdated`, the render thread reads it under the snapshot lock).
    menu: Option<MenuState>,
}

impl Shared {
    /// Rebuild the sorted render snapshot after any mutation.
    fn republish(&mut self) {
        let Some(snapshot) = &self.snapshot else {
            return;
        };
        let mut items: Vec<TrayItem> = self
            .items
            .values()
            .map(|entry| entry.item.clone())
            .collect();
        items.sort_by(|a, b| a.key.cmp(&b.key));
        *snapshot.lock().unwrap() = TraySnapshot {
            items,
            menu: self.menu.clone(),
        };
    }
}

/// Merge freshly read properties into an entry, bumping the icon generation
/// only when the icon actually changed (so the bar's texture cache survives
/// status-only updates).
fn apply_props(item: &mut TrayItem, props: ItemProps) {
    if item.icon != props.icon {
        item.icon_generation += 1;
        item.icon = props.icon;
    }
    item.title = props.title;
    item.status = props.status;
    item.has_menu = props.has_menu;
}

/// The served `org.kde.StatusNotifierWatcher` interface.
struct WatcherIface {
    /// Async handle onto the same connection; only used inside served
    /// methods, which already run on zbus's executor.
    conn: zbus::Connection,
    shared: Arc<Mutex<Shared>>,
}

#[zbus::interface(name = "org.kde.StatusNotifierWatcher")]
impl WatcherIface {
    /// Items register with either their bus name (object then lives at
    /// `/StatusNotifierItem`) or an object path on the sender's connection.
    async fn register_status_notifier_item(
        &self,
        service: &str,
        #[zbus(header)] header: zbus::message::Header<'_>,
        #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
    ) {
        let sender = header
            .sender()
            .map(|name| name.to_string())
            .unwrap_or_default();
        let (bus_name, path) = if service.starts_with('/') {
            (sender.clone(), service.to_string())
        } else {
            (service.to_string(), DEFAULT_ITEM_PATH.to_string())
        };
        let key = format!("{bus_name}{path}");
        let destination = if sender.is_empty() {
            &bus_name
        } else {
            &sender
        };

        let props = fetch_props_async(&self.conn, destination, &path).await;
        let menu_path = props.menu_path.clone();
        let mut item = TrayItem {
            key: key.clone(),
            title: String::new(),
            icon: TrayIcon::None,
            status: TrayStatus::Active,
            has_menu: false,
            icon_generation: 0,
        };
        {
            let mut shared = self.shared.lock().unwrap();
            // Re-registration keeps the generation counter so unchanged
            // icons do not force a texture re-upload.
            if let Some(previous) = shared.items.get(&key) {
                item.icon_generation = previous.item.icon_generation;
            }
            apply_props(&mut item, props);
            shared.items.insert(
                key.clone(),
                ItemEntry {
                    bus_name,
                    unique_name: sender,
                    path,
                    menu_path,
                    item,
                },
            );
            shared.republish();
        }

        log::info!("tray: registered {key}");
        if let Err(error) = emitter.status_notifier_item_registered(service).await {
            log::warn!("tray: could not emit StatusNotifierItemRegistered: {error}");
        }
    }

    /// The compositor itself is the host; external host registrations are
    /// accepted and ignored (the property already reports a host present).
    async fn register_status_notifier_host(&self, _service: &str) {}

    #[zbus(property)]
    fn registered_status_notifier_items(&self) -> Vec<String> {
        self.shared.lock().unwrap().items.keys().cloned().collect()
    }

    #[zbus(property)]
    fn is_status_notifier_host_registered(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn protocol_version(&self) -> i32 {
        0
    }

    #[zbus(signal)]
    async fn status_notifier_item_registered(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_item_unregistered(
        emitter: &zbus::object_server::SignalEmitter<'_>,
        service: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn status_notifier_host_registered(
        emitter: &zbus::object_server::SignalEmitter<'_>,
    ) -> zbus::Result<()>;
}

/// Read an item's `Title`/`Id`, icon, `Status`, and `Menu` asynchronously
/// (inside a served watcher method).
async fn fetch_props_async(conn: &zbus::Connection, destination: &str, path: &str) -> ItemProps {
    let Ok(proxy) = zbus::Proxy::new(conn, destination, path, ITEM_IFACE).await else {
        return ItemProps::default();
    };
    let id: Option<String> = proxy.get_property("Id").await.ok();
    let title: Option<String> = proxy.get_property("Title").await.ok();
    let status: Option<String> = proxy.get_property("Status").await.ok();
    let icon_name: Option<String> = proxy.get_property("IconName").await.ok();
    let icon_pixmap: Option<Vec<(i32, i32, Vec<u8>)>> = proxy.get_property("IconPixmap").await.ok();
    let menu: Option<OwnedObjectPath> = proxy.get_property("Menu").await.ok();
    build_props(id, title, status, icon_name, icon_pixmap, menu)
}

/// Same read through the blocking proxy (signal thread).
fn fetch_props_blocking(
    conn: &zbus::blocking::Connection,
    destination: &str,
    path: &str,
) -> ItemProps {
    let Ok(proxy) = zbus::blocking::Proxy::new(conn, destination, path, ITEM_IFACE) else {
        return ItemProps::default();
    };
    let id: Option<String> = proxy.get_property("Id").ok();
    let title: Option<String> = proxy.get_property("Title").ok();
    let status: Option<String> = proxy.get_property("Status").ok();
    let icon_name: Option<String> = proxy.get_property("IconName").ok();
    let icon_pixmap: Option<Vec<(i32, i32, Vec<u8>)>> = proxy.get_property("IconPixmap").ok();
    let menu: Option<OwnedObjectPath> = proxy.get_property("Menu").ok();
    build_props(id, title, status, icon_name, icon_pixmap, menu)
}

fn build_props(
    id: Option<String>,
    title: Option<String>,
    status: Option<String>,
    icon_name: Option<String>,
    icon_pixmap: Option<Vec<(i32, i32, Vec<u8>)>>,
    menu: Option<OwnedObjectPath>,
) -> ItemProps {
    let title = title
        .filter(|title| !title.is_empty())
        .or(id)
        .unwrap_or_default();
    let status = TrayStatus::parse(status.as_deref().unwrap_or("Active"));
    // Prefer the raw pixmap: we can always render it, while theme-name icons
    // currently fall back to a generic glyph (theme resolution is a TODO).
    let icon = match (icon_pixmap, icon_name) {
        (Some(pixmaps), name) => match select_pixmap(&pixmaps, PIXMAP_TARGET) {
            Some(index) => {
                let (w, h, data) = &pixmaps[index];
                TrayIcon::Pixmap(TrayPixmap {
                    width: *w as u32,
                    height: *h as u32,
                    bgra: argb32_to_bgra(data),
                })
            }
            None => name
                .filter(|name| !name.is_empty())
                .map_or(TrayIcon::None, TrayIcon::Name),
        },
        (None, Some(name)) if !name.is_empty() => TrayIcon::Name(name),
        (None, _) => TrayIcon::None,
    };
    let menu_path = menu
        .filter(|path| path.as_str() != "/")
        .map(|path| path.as_str().to_string());
    let has_menu = menu_path.is_some();
    ItemProps {
        title,
        icon,
        status,
        has_menu,
        menu_path,
    }
}

/// Start the watcher and both worker threads. See [`super::spawn`].
pub(super) fn spawn() -> Option<(Arc<Mutex<TraySnapshot>>, mpsc::Sender<TrayCommand>)> {
    let builder = match zbus::blocking::connection::Builder::session() {
        Ok(builder) => builder,
        Err(error) => {
            log::info!("tray: no session bus ({error}); SNI tray disabled");
            return None;
        }
    };
    let conn = match builder.build() {
        Ok(conn) => conn,
        Err(error) => {
            log::warn!("tray: could not connect to session bus ({error}); SNI tray disabled");
            return None;
        }
    };
    let shared = Arc::new(Mutex::new(Shared::default()));
    let iface = WatcherIface {
        conn: conn.inner().clone(),
        shared: Arc::clone(&shared),
    };
    // Serve the interface before requesting the name so no registration can
    // arrive at a name we own but do not serve yet.
    if let Err(error) = conn.object_server().at(WATCHER_PATH, iface) {
        log::warn!("tray: could not serve watcher interface ({error}); SNI tray disabled");
        return None;
    }
    if let Err(error) = conn.request_name(WATCHER_NAME) {
        log::info!("tray: {WATCHER_NAME} unavailable ({error}); SNI tray disabled");
        return None;
    }
    // Advertise that a host (this compositor) is present; items that
    // registered before we started would re-register now.
    if let Err(error) = conn.emit_signal(
        None::<&str>,
        WATCHER_PATH,
        WATCHER_IFACE,
        "StatusNotifierHostRegistered",
        &(),
    ) {
        log::warn!("tray: could not emit StatusNotifierHostRegistered: {error}");
    }

    let snapshot = Arc::new(Mutex::new(TraySnapshot::default()));
    shared.lock().unwrap().snapshot = Some(Arc::clone(&snapshot));
    let (tx, rx) = mpsc::channel::<TrayCommand>();

    let signal_conn = conn.clone();
    let signal_shared = Arc::clone(&shared);
    let spawned_signals = thread::Builder::new()
        .name("sni-tray-signals".to_string())
        .spawn(move || signal_loop(signal_conn, signal_shared));
    let spawned_commands = thread::Builder::new()
        .name("sni-tray-commands".to_string())
        .spawn(move || command_loop(conn, shared, rx));
    if spawned_signals.is_err() || spawned_commands.is_err() {
        log::warn!("tray: could not spawn worker threads; SNI tray disabled");
        return None;
    }
    Some((snapshot, tx))
}

/// Consume bus signals: drop items whose name vanished, refresh items whose
/// icon/title/status changed.
fn signal_loop(conn: zbus::blocking::Connection, shared: Arc<Mutex<Shared>>) {
    // One broad rule keeps a single iterator; traffic is filtered locally.
    // SNI signals are rare, so the volume is negligible on a session bus.
    let iter = match zbus::blocking::MessageIterator::for_match_rule(
        "type='signal'",
        &conn,
        Some(SIGNAL_QUEUE_DEPTH),
    ) {
        Ok(iter) => iter,
        Err(error) => {
            log::warn!("tray: could not subscribe to bus signals: {error}");
            return;
        }
    };
    for message in iter.flatten() {
        handle_signal(&conn, &shared, &message);
    }
    log::info!("tray: signal stream ended; SNI tray stopped");
}

fn handle_signal(
    conn: &zbus::blocking::Connection,
    shared: &Arc<Mutex<Shared>>,
    message: &zbus::Message,
) {
    let header = message.header();
    let interface = header.interface().map(|name| name.as_str());
    let member = header.member().map(|name| name.as_str());
    let sender = header.sender().map(|name| name.to_string());

    match (interface, member) {
        (Some(DBUS_IFACE), Some("NameOwnerChanged")) => {
            let Ok((name, _old, new_owner)) =
                message.body().deserialize::<(String, String, String)>()
            else {
                return;
            };
            if !new_owner.is_empty() {
                return;
            }
            let removed: Vec<String> = {
                let mut shared = shared.lock().unwrap();
                let keys: Vec<String> = shared
                    .items
                    .iter()
                    .filter(|(_, entry)| entry.unique_name == name || entry.bus_name == name)
                    .map(|(key, _)| key.clone())
                    .collect();
                for key in &keys {
                    shared.items.remove(key);
                }
                if !keys.is_empty() {
                    shared.republish();
                }
                keys
            };
            for key in removed {
                log::info!("tray: unregistered {key}");
                if let Err(error) = conn.emit_signal(
                    None::<&str>,
                    WATCHER_PATH,
                    WATCHER_IFACE,
                    "StatusNotifierItemUnregistered",
                    &key,
                ) {
                    log::warn!("tray: could not emit StatusNotifierItemUnregistered: {error}");
                }
            }
        }
        (Some(ITEM_IFACE), Some("NewIcon" | "NewTitle" | "NewStatus" | "NewAttentionIcon"))
        | (Some(PROPERTIES_IFACE), Some("PropertiesChanged")) => {
            let Some(sender) = sender else { return };
            let path = header.path().map(|path| path.to_string());
            let key = {
                let shared = shared.lock().unwrap();
                shared
                    .items
                    .iter()
                    .find(|(_, entry)| {
                        (entry.unique_name == sender || entry.bus_name == sender)
                            && Some(&entry.path) == path.as_ref()
                    })
                    .map(|(key, _)| key.clone())
            };
            let Some(key) = key else { return };
            let props =
                fetch_props_blocking(conn, &sender, path.as_deref().unwrap_or(DEFAULT_ITEM_PATH));
            let mut shared = shared.lock().unwrap();
            if let Some(entry) = shared.items.get_mut(&key) {
                apply_props(&mut entry.item, props);
                shared.republish();
            }
        }
        (Some(DBUSMENU_IFACE), Some("LayoutUpdated")) => {
            // The signal carries `(revision, parent)`; we only need to know
            // whether it belongs to the currently open menu so the render
            // thread sees fresh rows on the next frame.
            let path = header.path().map(|path| path.to_string());
            let needs_refresh = match path {
                None => false,
                Some(path) => {
                    let shared = shared.lock().unwrap();
                    match &shared.menu {
                        Some(menu) => shared
                            .items
                            .get(&menu.key)
                            .map(|entry| entry.menu_path.as_deref() == Some(path.as_str()))
                            .unwrap_or(false),
                        None => false,
                    }
                }
            };
            if needs_refresh {
                refresh_open_menu(conn, shared);
            }
        }
        _ => {}
    }
}

/// Re-fetch the layout for the currently open menu (called from the signal
/// thread when `LayoutUpdated` arrives). Holds the lock only briefly to look
/// up the target; the `GetLayout` round-trip happens unlocked.
fn refresh_open_menu(conn: &zbus::blocking::Connection, shared: &Arc<Mutex<Shared>>) {
    let target = {
        let shared = shared.lock().unwrap();
        let Some(menu) = &shared.menu else {
            return;
        };
        let Some(entry) = shared.items.get(&menu.key) else {
            return;
        };
        let destination = if entry.unique_name.is_empty() {
            entry.bus_name.clone()
        } else {
            entry.unique_name.clone()
        };
        let Some(menu_path) = entry.menu_path.clone() else {
            return;
        };
        (menu.key.clone(), destination, menu_path)
    };
    let Some((root, revision)) = fetch_menu_layout(conn, &target.1, &target.2) else {
        return;
    };
    let mut shared = shared.lock().unwrap();
    shared.menu = Some(MenuState {
        key: target.0,
        root,
        revision,
    });
    shared.republish();
}

/// Invoke activation methods on items as the bar's clicks come in.
fn command_loop(
    conn: zbus::blocking::Connection,
    shared: Arc<Mutex<Shared>>,
    rx: mpsc::Receiver<TrayCommand>,
) {
    while let Ok(command) = rx.recv() {
        match command {
            TrayCommand::Activate { key, x, y } => {
                invoke_item_method(&conn, &shared, &key, "Activate", x, y);
            }
            TrayCommand::SecondaryActivate { key, x, y } => {
                invoke_item_method(&conn, &shared, &key, "SecondaryActivate", x, y);
            }
            TrayCommand::ContextMenu { key, x, y } => {
                invoke_item_method(&conn, &shared, &key, "ContextMenu", x, y);
            }
            TrayCommand::FetchMenu { key } => fetch_menu_command(&conn, &shared, &key),
            TrayCommand::MenuEvent { key, id } => menu_event_command(&conn, &shared, &key, id),
            TrayCommand::CloseMenu { key } => close_menu_command(&shared, &key),
        }
    }
}

fn invoke_item_method(
    conn: &zbus::blocking::Connection,
    shared: &Arc<Mutex<Shared>>,
    key: &str,
    method: &str,
    x: i32,
    y: i32,
) {
    let target = shared.lock().unwrap().items.get(key).map(|entry| {
        let destination = if entry.unique_name.is_empty() {
            entry.bus_name.clone()
        } else {
            entry.unique_name.clone()
        };
        (destination, entry.path.clone())
    });
    let Some((destination, path)) = target else {
        return;
    };
    let result = zbus::blocking::Proxy::new(conn, destination.as_str(), path.as_str(), ITEM_IFACE)
        .and_then(|proxy| proxy.call::<_, _, ()>(method, &(x, y)));
    if let Err(error) = result {
        log::warn!("tray: {method} on {key} failed: {error}");
    }
}

/// `FetchMenu` handler: look up the item's dbusmenu path, call `GetLayout`,
/// parse the reply, and stash the [`MenuState`] in `Shared`. Replaces any
/// menu already open.
fn fetch_menu_command(conn: &zbus::blocking::Connection, shared: &Arc<Mutex<Shared>>, key: &str) {
    let target = shared.lock().unwrap().items.get(key).map(|entry| {
        let destination = if entry.unique_name.is_empty() {
            entry.bus_name.clone()
        } else {
            entry.unique_name.clone()
        };
        (destination, entry.menu_path.clone())
    });
    let Some((destination, Some(menu_path))) = target else {
        return;
    };
    let Some((root, revision)) = fetch_menu_layout(conn, &destination, &menu_path) else {
        return;
    };
    let mut shared = shared.lock().unwrap();
    shared.menu = Some(MenuState {
        key: key.to_string(),
        root,
        revision,
    });
    shared.republish();
}

/// `MenuEvent` handler: send `Event(id, "clicked", u32 variant, timestamp)`
/// on the item's dbusmenu proxy. The spec leaves `data` unused for `clicked`;
/// a zero `u32` variant matches what most toolkits send.
fn menu_event_command(
    conn: &zbus::blocking::Connection,
    shared: &Arc<Mutex<Shared>>,
    key: &str,
    id: i32,
) {
    let target = shared.lock().unwrap().items.get(key).map(|entry| {
        let destination = if entry.unique_name.is_empty() {
            entry.bus_name.clone()
        } else {
            entry.unique_name.clone()
        };
        (destination, entry.menu_path.clone())
    });
    let Some((destination, Some(menu_path))) = target else {
        return;
    };
    let data = Value::U32(0);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as u32)
        .unwrap_or(0);
    let result = zbus::blocking::Proxy::new(
        conn,
        destination.as_str(),
        menu_path.as_str(),
        DBUSMENU_IFACE,
    )
    .and_then(|proxy| proxy.call::<_, _, ()>("Event", &(id, "clicked", &data, timestamp)));
    if let Err(error) = result {
        log::warn!("tray: Event(clicked) on {key}:{id} failed: {error}");
    }
}

/// `CloseMenu` handler: clear `Shared.menu` if it matches `key`. The bar
/// sends this on every close so the worker can stop refreshing on
/// `LayoutUpdated`.
fn close_menu_command(shared: &Arc<Mutex<Shared>>, key: &str) {
    let mut shared = shared.lock().unwrap();
    if shared
        .menu
        .as_ref()
        .map(|menu| menu.key == key)
        .unwrap_or(false)
    {
        shared.menu = None;
        shared.republish();
    }
}

/// `GetLayout(0, -1, [property-names])` round-trip on the worker thread.
/// Returns the parsed root and revision, or `None` on any D-Bus or parse
/// failure (the caller falls back to the legacy `ContextMenu` behavior).
fn fetch_menu_layout(
    conn: &zbus::blocking::Connection,
    destination: &str,
    menu_path: &str,
) -> Option<(super::MenuNode, u32)> {
    let proxy = zbus::blocking::Proxy::new(conn, destination, menu_path, DBUSMENU_IFACE).ok()?;
    // `AboutToShow(0)` is the spec-recommended refresh hint before reading
    // the layout; the return signals whether the layout changed since the
    // last call. We ignore it (a redundant `GetLayout` is cheap).
    let _ = proxy.call::<_, _, bool>("AboutToShow", &0_i32);
    let property_names: Vec<&str> = super::MENU_PROPERTY_NAMES.to_vec();
    let reply: LayoutReply = proxy
        .call("GetLayout", &(0_i32, -1_i32, &property_names))
        .ok()?;
    let (root, revision) = parse_layout_reply(reply)?;
    Some((root, revision))
}
