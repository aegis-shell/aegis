//! logind sleep integration and delay inhibitor.

use std::sync::mpsc::Sender;
use std::time::Duration;

const DESTINATION: &str = "org.freedesktop.login1";
const PATH: &str = "/org/freedesktop/login1";
const INTERFACE: &str = "org.freedesktop.login1.Manager";

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
            "aegis-idle",
            "Lock the Aegis session before sleep",
            "delay",
        ),
    )
}

pub fn spawn_signal_monitor(events: Sender<SleepEvent>) {
    let _ = std::thread::Builder::new()
        .name("aegis-idle-logind".into())
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
        .name("aegis-idle-suspend".into())
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
