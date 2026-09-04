//! logind sleep integration and delay inhibitor.

use std::sync::mpsc::Sender;
use std::time::Duration;

const DESTINATION: &str = "org.freedesktop.login1";
const PATH: &str = "/org/freedesktop/login1";
const INTERFACE: &str = "org.freedesktop.login1.Manager";
const SESSION_INTERFACE: &str = "org.freedesktop.login1.Session";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SleepEvent {
    Preparing,
    Resumed,
}

pub fn acquire_delay_inhibitor() -> Result<zbus::zvariant::OwnedFd, zbus::Error> {
    let connection = zbus::blocking::Connection::system()?;
    let proxy = zbus::blocking::Proxy::new(&connection, DESTINATION, PATH, INTERFACE)?;
    proxy.call(
        "Inhibit",
        &(
            "sleep",
            "tessera-idle",
            "Lock the Tessera session before sleep",
            "delay",
        ),
    )
}

/// Broadcast the freedesktop-standard session lock: call `LockSession()`
/// on this process's own logind session so every subscriber of the
/// `Session.Lock` signal — secret vaults, keyrings, agents — sees the
/// same authoritative "the session is locked" event the idle policy just
/// made true. `SetLockedHint(true)` follows on the same thread:
/// logind's `LockSession()` only emits the signal, the hint property is
/// the DE's to set (`loginctl` and tools read it).
///
/// Fire-and-forget on its own thread exactly like `suspend_async`: the
/// secure frame must not wait on a system-bus round trip, and a missing
/// bus or session (nested/remote) is logged, never fatal.
pub fn lock_session_async() {
    let _ = std::thread::Builder::new()
        .name("tessera-idle-lock-session".into())
        .spawn(|| {
            if let Err(error) = session_call("LockSession") {
                log::warn!("idle: logind LockSession broadcast failed: {error}");
            }
            if let Err(error) = session_hint(true) {
                log::warn!("idle: logind SetLockedHint(true) failed: {error}");
            }
        });
}

/// Announce the session's return after an authenticated unlock:
/// `UnlockSession()` on the own session, mirroring [`lock_session_async`],
/// and the matching locked hint.
pub fn unlock_session_async() {
    let _ = std::thread::Builder::new()
        .name("tessera-idle-unlock-session".into())
        .spawn(|| {
            if let Err(error) = session_call("UnlockSession") {
                log::warn!("idle: logind UnlockSession broadcast failed: {error}");
            }
            if let Err(error) = session_hint(false) {
                log::warn!("idle: logind SetLockedHint(false) failed: {error}");
            }
        });
}

/// Resolve the graphical session object path and call `member` on the
/// `org.freedesktop.login1.Session` interface.
///
/// Prefers `$XDG_SESSION_ID` (imported into the user-manager environment
/// by `tessera-session`): compositor children run as user services outside
/// the session scope's cgroup, where `GetSessionByPID` resolves to the
/// class=manager session — which logind exempts from locking. The PID
/// fallback covers environments without the variable.
fn session_call(member: &str) -> Result<(), zbus::Error> {
    let (connection, session_path) = session_proxy_parts()?;
    let session = zbus::blocking::Proxy::new(
        &connection,
        DESTINATION,
        session_path.as_str(),
        SESSION_INTERFACE,
    )?;
    session.call(member, &())
}

/// Set the session's `LockedHint` property (see [`lock_session_async`]).
fn session_hint(locked: bool) -> Result<(), zbus::Error> {
    let (connection, session_path) = session_proxy_parts()?;
    let session = zbus::blocking::Proxy::new(
        &connection,
        DESTINATION,
        session_path.as_str(),
        SESSION_INTERFACE,
    )?;
    session.call("SetLockedHint", &(locked,))
}

/// The shared system-bus connection and this session's object path.
type SessionParts =
    Result<(zbus::blocking::Connection, zbus::zvariant::OwnedObjectPath), zbus::Error>;

fn session_proxy_parts() -> SessionParts {
    let connection = zbus::blocking::Connection::system()?;
    let manager = zbus::blocking::Proxy::new(&connection, DESTINATION, PATH, INTERFACE)?;
    let session_path = match std::env::var("XDG_SESSION_ID") {
        Ok(id) if !id.is_empty() => manager.call("GetSession", &(&id,))?,
        _ => manager.call("GetSessionByPID", &(std::process::id()))?,
    };
    Ok((connection, session_path))
}

pub fn spawn_signal_monitor(events: Sender<SleepEvent>) {
    let _ = std::thread::Builder::new()
        .name("tessera-idle-logind".into())
        .spawn(move || {
            loop {
                match monitor(&events) {
                    Ok(false) => break,
                    Ok(true) => {
                        log::warn!("idle: logind sleep monitor ended; reconnecting");
                    }
                    Err(error) => {
                        log::warn!("idle: logind sleep monitor unavailable: {error}");
                    }
                }
                std::thread::sleep(Duration::from_secs(5));
            }
        });
}

pub fn suspend_async() {
    let _ = std::thread::Builder::new()
        .name("tessera-idle-suspend".into())
        .spawn(|| {
            let result = (|| -> Result<(), zbus::Error> {
                let connection = zbus::blocking::Connection::system()?;
                let proxy = zbus::blocking::Proxy::new(&connection, DESTINATION, PATH, INTERFACE)?;
                proxy.call("Suspend", &(false,))
            })();
            if let Err(error) = result {
                log::warn!("idle: logind suspend request failed: {error}");
            }
        });
}

/// Return `false` when the policy process has gone away and the monitor
/// should terminate; `true` means the D-Bus stream ended and can reconnect.
fn monitor(events: &Sender<SleepEvent>) -> Result<bool, zbus::Error> {
    let connection = zbus::blocking::Connection::system()?;
    let rule = format!(
        "type='signal',sender='{DESTINATION}',interface='{INTERFACE}',member='PrepareForSleep'"
    );
    let iterator =
        zbus::blocking::MessageIterator::for_match_rule(rule.as_str(), &connection, Some(8))?;
    for message in iterator.flatten() {
        let Ok(preparing) = message.body().deserialize::<bool>() else {
            continue;
        };
        if events
            .send(if preparing {
                SleepEvent::Preparing
            } else {
                SleepEvent::Resumed
            })
            .is_err()
        {
            return Ok(false);
        }
    }
    Ok(true)
}
