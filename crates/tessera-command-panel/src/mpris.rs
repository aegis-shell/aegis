//! Minimal MPRIS bridge for the command panel's now-playing card.
//!
//! The render thread never touches D-Bus. A blocking worker periodically
//! selects the active `org.mpris.MediaPlayer2.*` name, publishes a compact
//! snapshot, and dispatches transport commands received over `mpsc`.

use std::borrow::Borrow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, mpsc};
use std::thread;
use std::time::Duration;

use zbus::blocking::{Connection, Proxy};
use zbus::zvariant::{OwnedValue, Value};

const MPRIS_PREFIX: &str = "org.mpris.MediaPlayer2.";
const MPRIS_PATH: &str = "/org/mpris/MediaPlayer2";
const ROOT_IFACE: &str = "org.mpris.MediaPlayer2";
const PLAYER_IFACE: &str = "org.mpris.MediaPlayer2.Player";

/// Immutable data consumed by the render frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(super) struct MediaSnapshot {
    pub available: bool,
    pub playing: bool,
    pub identity: String,
    pub title: String,
    pub artist: String,
    pub can_previous: bool,
    pub can_next: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum MediaCommand {
    Previous,
    PlayPause,
    Next,
}

/// Render/command handle. Dropping its sender shuts the worker down after
/// the bounded receive timeout.
pub(super) struct MediaHandle {
    snapshot: Arc<Mutex<MediaSnapshot>>,
    commands: mpsc::Sender<MediaCommand>,
}

impl MediaHandle {
    pub fn snapshot(&self) -> MediaSnapshot {
        self.snapshot.lock().unwrap().clone()
    }

    pub fn send(&self, command: MediaCommand) {
        let _ = self.commands.send(command);
    }
}

pub(super) fn spawn() -> Option<MediaHandle> {
    let connection = match Connection::session() {
        Ok(connection) => connection,
        Err(error) => {
            log::info!("mpris: no session bus ({error}); media card is read-only empty state");
            return None;
        }
    };
    let snapshot = Arc::new(Mutex::new(MediaSnapshot::default()));
    let (commands, receiver) = mpsc::channel();
    let worker_snapshot = Arc::clone(&snapshot);
    let spawned = thread::Builder::new()
        .name("mpris-command-panel".to_string())
        .spawn(move || worker_loop(connection, worker_snapshot, receiver));
    if let Err(error) = spawned {
        log::warn!("mpris: could not spawn worker ({error})");
        return None;
    }
    Some(MediaHandle { snapshot, commands })
}

fn worker_loop(
    connection: Connection,
    snapshot: Arc<Mutex<MediaSnapshot>>,
    receiver: mpsc::Receiver<MediaCommand>,
) {
    let mut destination = refresh(&connection, &snapshot);
    loop {
        match receiver.recv_timeout(Duration::from_millis(750)) {
            Ok(command) => {
                if destination.is_none() {
                    destination = refresh(&connection, &snapshot);
                }
                if let Some(name) = destination.as_deref() {
                    invoke(&connection, name, command);
                }
                destination = refresh(&connection, &snapshot);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {
                destination = refresh(&connection, &snapshot);
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
}

/// Publish the highest-priority player and return the bus name used for
/// subsequent commands. Playing beats paused/stopped; otherwise stable bus
/// order prevents the card from jumping between idle players.
fn refresh(connection: &Connection, shared: &Arc<Mutex<MediaSnapshot>>) -> Option<String> {
    let Ok(dbus) = zbus::blocking::fdo::DBusProxy::new(connection) else {
        return None;
    };
    let Ok(mut names) = dbus.list_names() else {
        return None;
    };
    names.sort_by(|left, right| left.as_str().cmp(right.as_str()));

    let mut selected: Option<(String, MediaSnapshot)> = None;
    for name in names {
        let name = name.as_str();
        if !name.starts_with(MPRIS_PREFIX) {
            continue;
        }
        let Some(candidate) = read_player(connection, name) else {
            continue;
        };
        let replace = selected
            .as_ref()
            .is_none_or(|(_, current)| candidate.playing && !current.playing);
        if replace {
            selected = Some((name.to_string(), candidate));
        }
        if selected.as_ref().is_some_and(|(_, player)| player.playing) {
            break;
        }
    }

    let (destination, next) = selected
        .map(|(name, snapshot)| (Some(name), snapshot))
        .unwrap_or((None, MediaSnapshot::default()));
    let mut current = shared.lock().unwrap();
    if *current != next {
        *current = next;
    }
    destination
}

fn read_player(connection: &Connection, destination: &str) -> Option<MediaSnapshot> {
    let player = Proxy::new(connection, destination, MPRIS_PATH, PLAYER_IFACE).ok()?;
    let playback_status: String = player.get_property("PlaybackStatus").ok()?;
    let metadata: HashMap<String, OwnedValue> = player.get_property("Metadata").unwrap_or_default();
    let root = Proxy::new(connection, destination, MPRIS_PATH, ROOT_IFACE).ok();
    let identity = root
        .and_then(|proxy| proxy.get_property::<String>("Identity").ok())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| {
            destination
                .strip_prefix(MPRIS_PREFIX)
                .unwrap_or(destination)
                .to_string()
        });

    Some(MediaSnapshot {
        available: true,
        playing: playback_status == "Playing",
        identity,
        title: metadata_string(&metadata, "xesam:title").unwrap_or_default(),
        artist: metadata_artists(&metadata).join(", "),
        can_previous: player.get_property("CanGoPrevious").unwrap_or(false),
        can_next: player.get_property("CanGoNext").unwrap_or(false),
    })
}

fn invoke(connection: &Connection, destination: &str, command: MediaCommand) {
    let Ok(player) = Proxy::new(connection, destination, MPRIS_PATH, PLAYER_IFACE) else {
        return;
    };
    let method = match command {
        MediaCommand::Previous => "Previous",
        MediaCommand::PlayPause => "PlayPause",
        MediaCommand::Next => "Next",
    };
    if let Err(error) = player.call::<_, _, ()>(method, &()) {
        log::debug!("mpris: {method} on {destination} failed: {error}");
    }
}

fn metadata_string(metadata: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    metadata.get(key).and_then(|value| {
        Borrow::<Value>::borrow(value)
            .downcast_ref::<&str>()
            .ok()
            .map(str::to_string)
    })
}

fn metadata_artists(metadata: &HashMap<String, OwnedValue>) -> Vec<String> {
    let Some(value) = metadata.get("xesam:artist") else {
        return Vec::new();
    };
    match Borrow::<Value>::borrow(value) {
        Value::Array(values) => values
            .iter()
            .filter_map(|value| value.downcast_ref::<&str>().ok().map(str::to_string))
            .collect(),
        _ => Vec::new(),
    }
}
