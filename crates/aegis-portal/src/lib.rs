//! aegis-portal: the `xdg-desktop-portal` backend for the aegis compositor.
//!
//! A standalone D-Bus-activated process that bridges the freedesktop portal
//! backend interfaces to the compositor's own IPC (ADR-0051). Outward it
//! serves `org.freedesktop.impl.portal.Settings` v1,
//! `org.freedesktop.impl.portal.Screenshot` v1,
//! `org.freedesktop.impl.portal.ScreenCast` v2,
//! `org.freedesktop.impl.portal.Background` v1, and
//! `org.freedesktop.impl.portal.Inhibit` v1 at
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
//! ([ADR-0053](../../docs/adr/0053-portal-session-services-and-grants.md)).
//! No Wayland capture protocol is added anywhere.
//!
//! The process model follows the SNI tray precedent
//! (`crates/aegis-statusbar`): zbus's blocking API on the session bus, plain
//! `std::thread` workers, no tokio. Method dispatch runs on zbus's internal
//! executor; the compositor IPC round-trip (which blocks for up to one frame)
//! happens on a dedicated capture worker so a slow capture never stalls the
//! bus, and every screencast runs its own PipeWire main loop on a dedicated
//! cast thread.
//!
//! Portal-owned authorization state (Background decisions, ScreenCast
//! restore tokens) persists as JSON under `$XDG_DATA_HOME/aegis-portal`
//! (ADR-0053) — aegis has no PermissionStore.

mod background;
mod cast;
mod files;
mod inhibit;
mod ipc;
mod request;
mod screencast;
mod screenshot;
mod session;
mod settings;
mod state;

use std::sync::{Arc, Mutex, mpsc};

use background::BackgroundIface;
use inhibit::InhibitIface;
use ipc::PortalCapture;
use request::RequestTracker;
use screencast::{CastJob, ScreenCastIface, TokenStore};
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

/// Run the backend: serve all interfaces on the session bus, spawn the
/// capture and screencast workers, and park forever. The process is
/// D-Bus-activated and stays resident; the bus re-activates it if it dies.
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
    let (bg_jobs, bg_rx) = mpsc::channel::<background::BackgroundJob>();
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
        BackgroundIface {
            conn: conn.inner().clone(),
            tracker: Arc::clone(&tracker),
            jobs: bg_jobs,
        },
    )?;
    conn.object_server().at(
        DESKTOP_PATH,
        InhibitIface {
            jobs: inhibit_jobs.clone(),
        },
    )?;

    let worker_conn = conn.clone();
    let worker_tracker = Arc::clone(&tracker);
    let worker_socket = socket.clone();
    std::thread::Builder::new()
        .name("aegis-portal-capture".to_string())
        .spawn(move || {
            screenshot::capture_worker(
                rx,
                worker_conn,
                worker_tracker,
                PortalCapture::new(worker_socket),
            )
        })
        .map_err(|e| {
            PortalError::Bus(zbus::Error::Failure(format!("spawn capture worker: {e}")))
        })?;

    let cast_worker_conn = conn.clone();
    let cast_worker_tracker = Arc::clone(&tracker);
    let cast_worker_socket = socket.clone();
    let tokens = TokenStore::load(state::state_dir());
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
                tokens,
            )
        })
        .map_err(|e| {
            PortalError::Bus(zbus::Error::Failure(format!(
                "spawn screencast worker: {e}"
            )))
        })?;

    let bg_worker_conn = conn.clone();
    let bg_worker_tracker = Arc::clone(&tracker);
    let bg_store = background::BackgroundStore::load(state::state_dir());
    std::thread::Builder::new()
        .name("aegis-portal-background".to_string())
        .spawn(move || {
            background::background_worker(bg_rx, bg_worker_conn, bg_worker_tracker, bg_store)
        })
        .map_err(|e| {
            PortalError::Bus(zbus::Error::Failure(format!(
                "spawn background worker: {e}"
            )))
        })?;

    let inhibit_socket = socket.clone();
    let inhibit_worker_counts = Arc::clone(&inhibit_counts);
    std::thread::Builder::new()
        .name("aegis-portal-inhibit".to_string())
        .spawn(move || inhibit::inhibit_worker(inhibit_rx, inhibit_worker_counts, inhibit_socket))
        .map_err(|e| {
            PortalError::Bus(zbus::Error::Failure(format!("spawn inhibit worker: {e}")))
        })?;

    let monitor_conn = conn.clone();
    std::thread::Builder::new()
        .name("aegis-portal-inhibit-monitor".to_string())
        .spawn(move || inhibit::sender_monitor(monitor_conn, inhibit_counts, inhibit_jobs))
        .map_err(|e| {
            PortalError::Bus(zbus::Error::Failure(format!(
                "spawn inhibit sender monitor: {e}"
            )))
        })?;

    settings::spawn_watcher(conn.clone(), socket, settings_store);

    conn.request_name(BUS_NAME)?;
    log::info!(
        "portal: serving Settings+Screenshot+ScreenCast+Background+Inhibit backends as {BUS_NAME}"
    );

    // Method dispatch runs on zbus's internal executor; the main thread has
    // nothing else to do. Losing the bus name is fatal by construction: the
    // connection drops, zbus's executor stops, and the next activation
    // re-runs the whole process.
    loop {
        std::thread::park();
    }
}
