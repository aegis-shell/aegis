//! aegis-portal: the `xdg-desktop-portal` backend for the aegis compositor.
//!
//! A standalone D-Bus-activated process that bridges the freedesktop portal
//! backend interfaces to the compositor's own IPC (ADR-0075). Outward it
//! serves `org.freedesktop.impl.portal.Settings` v1,
//! `org.freedesktop.impl.portal.Screenshot` v2,
//! `org.freedesktop.impl.portal.ScreenCast` v3, and
//! `org.freedesktop.impl.portal.Inhibit` at
//! `/org/freedesktop/portal/desktop` under the well-known name
//! `org.freedesktop.impl.portal.desktop.aegis`. Inward it is an ordinary scoped
//! IPC client: pixels come from `Request::CaptureOutput` under the built-in
//! `aegis-portal` named scope with a sealed-memfd blob transfer
//! ([ADR-0037](../../docs/adr/0037-scoped-pixel-capture-over-ipc.md),
//! [ADR-0041](../../docs/adr/0041-sealed-file-descriptor-pixel-transport.md)),
//! screencast frames arrive through the same scope's output-frame stream
//! ([ADR-0052](../../docs/adr/0052-scoped-output-frame-streaming.md)) and are
//! republished as a PipeWire producer stream, and idle inhibits hold the
//! scope's connection-scoped global idle inhibitor
//! ([ADR-0075](../../docs/adr/0075-independent-portal-package-and-backend-contract.md)).
//! No Wayland capture protocol is added anywhere.
//!
//! The process model follows the SNI tray precedent
//! (`crates/aegis-statusbar`): zbus's blocking API on the session bus, plain
//! `std::thread` workers, no tokio. Method dispatch runs on zbus's internal
//! executor; the compositor IPC round-trip (which blocks for up to one frame)
//! happens on a dedicated capture worker so a slow capture never stalls the
//! bus, and every screencast runs its own PipeWire main loop on a dedicated
//! cast thread.

mod cast;
mod files;
mod inhibit;
mod ipc;
mod request;
mod screencast;
mod screenshot;
mod session;
mod settings;

use std::sync::{Arc, Mutex, mpsc};

use inhibit::InhibitIface;
use ipc::PortalCapture;
use request::RequestTracker;
use screencast::{CastJob, ScreenCastIface};
use screenshot::{CaptureJob, ScreenshotIface};
use session::SessionRegistry;
use settings::{SettingsIface, SettingsStore};

/// The well-known bus name the portal frontend resolves through the
/// `aegis.portal` file's `DBusName`.
pub const BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.aegis";
/// The object path every portal backend serves its interfaces at.
pub const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";

/// Errors that prevent the backend from coming up at all.
#[derive(Debug, thiserror::Error)]
pub enum PortalError {
    /// No `$XDG_RUNTIME_DIR`, so the compositor IPC socket cannot be located.
    #[error("$XDG_RUNTIME_DIR is unset; cannot locate aegis.sock")]
    NoRuntimeDir,
    /// Session-bus or object-server setup failed.
    #[error("D-Bus setup failed: {0}")]
    Bus(#[from] zbus::Error),
}

/// Run the backend: serve all interfaces on the session bus and spawn the
/// capture, screencast, and inhibit workers. The process is D-Bus-activated,
/// stays resident while the bus is connected, and exits for reactivation
/// when that connection fails.
pub fn run() -> Result<(), PortalError> {
    let runtime_dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or(PortalError::NoRuntimeDir)?;
    let socket = std::path::PathBuf::from(runtime_dir).join("aegis.sock");

    // Global PipeWire setup for every future cast thread (idempotent).
    pipewire::init();

    let conn = zbus::blocking::connection::Builder::session()?
        .build()
        .map_err(PortalError::Bus)?;

    let tracker = Arc::new(Mutex::new(RequestTracker::default()));
    let sessions = Arc::new(Mutex::new(SessionRegistry::default()));
    let (jobs, rx) = mpsc::channel::<CaptureJob>();
    let (cast_jobs, cast_rx) = mpsc::channel::<CastJob>();
    let (inhibit_jobs, inhibit_rx) = mpsc::channel::<inhibit::InhibitJob>();
    let inhibit_counts = Arc::new(Mutex::new(inhibit::InhibitCounts::default()));
    let settings_store = SettingsStore::default();
    settings::prime_store(&socket, &settings_store);

    // Serve before requesting the name so no call can arrive at a name we own
    // but do not serve yet (same ordering as the SNI tray watcher).
    conn.object_server()
        .at(DESKTOP_PATH, SettingsIface::new(settings_store.clone()))?;
    conn.object_server().at(
        DESKTOP_PATH,
        ScreenshotIface {
            conn: conn.inner().clone(),
            tracker: Arc::clone(&tracker),
            jobs,
        },
    )?;
    conn.object_server().at(
        DESKTOP_PATH,
        ScreenCastIface {
            conn: conn.inner().clone(),
            tracker: Arc::clone(&tracker),
            sessions: Arc::clone(&sessions),
            jobs: cast_jobs.clone(),
        },
    )?;
    conn.object_server().at(
        DESKTOP_PATH,
        InhibitIface {
            conn: conn.inner().clone(),
            jobs: inhibit_jobs.clone(),
        },
    )?;

    let worker_tracker = Arc::clone(&tracker);
    let worker_socket = socket.clone();
    std::thread::Builder::new()
        .name("aegis-portal-capture".to_string())
        .spawn(move || {
            screenshot::capture_worker(rx, worker_tracker, PortalCapture::new(worker_socket))
        })
        .map_err(|e| {
            PortalError::Bus(zbus::Error::Failure(format!("spawn capture worker: {e}")))
        })?;

    let cast_worker_conn = conn.clone();
    let cast_worker_tracker = Arc::clone(&tracker);
    let cast_worker_socket = socket.clone();
    std::thread::Builder::new()
        .name("aegis-portal-screencast".to_string())
        .spawn(move || {
            screencast::cast_worker(
                cast_rx,
                cast_jobs,
                cast_worker_conn,
                cast_worker_tracker,
                sessions,
                cast_worker_socket,
            )
        })
        .map_err(|e| {
            PortalError::Bus(zbus::Error::Failure(format!(
                "spawn screencast worker: {e}"
            )))
        })?;

    let inhibit_socket = socket.clone();
    let inhibit_worker_counts = Arc::clone(&inhibit_counts);
    let inhibit_conn = conn.clone();
    std::thread::Builder::new()
        .name("aegis-portal-inhibit".to_string())
        .spawn(move || {
            inhibit::inhibit_worker(
                inhibit_rx,
                inhibit_worker_counts,
                inhibit_socket,
                inhibit_conn,
            )
        })
        .map_err(|e| {
            PortalError::Bus(zbus::Error::Failure(format!("spawn inhibit worker: {e}")))
        })?;

    settings::spawn_watcher(conn.clone(), socket, settings_store);

    conn.request_name(BUS_NAME)?;
    log::info!("portal: serving Settings+Screenshot+ScreenCast+Inhibit as {BUS_NAME}");

    // Keep the main thread tied to the bus connection. A disconnected
    // backend must exit so D-Bus activation can start a fresh process.
    loop {
        std::thread::sleep(std::time::Duration::from_secs(30));
        conn.call_method(
            Some("org.freedesktop.DBus"),
            "/org/freedesktop/DBus",
            Some("org.freedesktop.DBus.Peer"),
            "Ping",
            &(),
        )?;
    }
}

#[cfg(test)]
mod integration_metadata_tests {
    const PORTAL_FILE: &str =
        include_str!("../../../contrib/xdg-desktop-portal/portals/aegis.portal");
    const PORTALS_CONF: &str =
        include_str!("../../../contrib/xdg-desktop-portal/aegis-portals.conf");
    const DBUS_SERVICE: &str = include_str!(
        "../../../contrib/dbus-1/services/org.freedesktop.impl.portal.desktop.aegis.service"
    );

    #[test]
    fn capability_file_advertises_exactly_the_served_interfaces() {
        let interfaces = PORTAL_FILE
            .lines()
            .find_map(|line| line.strip_prefix("Interfaces="))
            .expect("portal metadata must declare Interfaces");
        let advertised: Vec<_> = interfaces
            .split(';')
            .filter(|entry| !entry.is_empty())
            .collect();
        assert_eq!(
            advertised,
            [
                "org.freedesktop.impl.portal.Settings",
                "org.freedesktop.impl.portal.Screenshot",
                "org.freedesktop.impl.portal.ScreenCast",
                "org.freedesktop.impl.portal.Inhibit",
            ]
        );
    }

    #[test]
    fn unsupported_interfaces_fall_back_instead_of_reaching_aegis() {
        assert!(PORTALS_CONF.lines().any(|line| line == "default=gtk"));
        for interface in ["Settings", "Screenshot", "ScreenCast", "Inhibit"] {
            let route = format!("org.freedesktop.impl.portal.{interface}=aegis");
            assert!(
                PORTALS_CONF.lines().any(|line| line.starts_with(&route)),
                "missing explicit Aegis route for {interface}"
            );
        }
        assert!(!PORTALS_CONF.contains("Background=aegis"));
    }

    #[test]
    fn activation_uses_the_private_packaged_executable() {
        assert!(
            DBUS_SERVICE
                .lines()
                .any(|line| line == "Exec=/usr/lib/aegis/aegis-portal")
        );
    }
}
