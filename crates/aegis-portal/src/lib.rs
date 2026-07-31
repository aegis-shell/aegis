//! aegis-portal: the `xdg-desktop-portal` backend for the aegis compositor.
//!
//! A standalone D-Bus-activated process that bridges the freedesktop portal
//! backend interfaces to the compositor's own IPC (ADR-0075). Outward it
//! serves `org.freedesktop.impl.portal.Settings` v1,
//! `org.freedesktop.impl.portal.Screenshot` v2,
//! `org.freedesktop.impl.portal.ScreenCast` v3,
//! `org.freedesktop.impl.portal.Inhibit`,
//! `org.freedesktop.impl.portal.Secret` v1,
//! `org.freedesktop.impl.portal.Lockdown`,
//! `org.freedesktop.impl.portal.FileChooser` v3,
//! `org.freedesktop.impl.portal.Email` v2,
//! `org.freedesktop.impl.portal.AppChooser` v2,
//! `org.freedesktop.impl.portal.Notification` v2,
//! `org.freedesktop.impl.portal.Account` v1,
//! `org.freedesktop.impl.portal.DynamicLauncher` v1, and
//! `org.freedesktop.impl.portal.Wallpaper` v1 at
//! `/org/freedesktop/portal/desktop` under the well-known name
//! `org.freedesktop.impl.portal.desktop.aegis`. Secret is backed by an
//! encrypted at-rest vault and needs no compositor IPC; a transitional
//! `org.freedesktop.secrets` compat shim (`secret::compat`) serves the
//! classic Secret Service API until portal-native secret retrieval is
//! universal. Inward it is an ordinary scoped
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
//! (`crates/aegis-hud`): zbus's blocking API on the session bus, plain
//! `std::thread` workers, no tokio. Method dispatch runs on zbus's internal
//! executor; the compositor IPC round-trip (which blocks for up to one frame)
//! happens on a dedicated capture worker so a slow capture never stalls the
//! bus, and every screencast runs its own PipeWire main loop on a dedicated
//! cast thread.

mod account;
mod app_chooser;
mod cast;
mod dynamic_launcher;
mod email;
mod file_chooser;
mod files;
mod inhibit;
mod ipc;
mod lockdown;
mod notification;
mod request;
mod screencast;
mod screenshot;
mod secret;
mod session;
mod settings;
mod wallpaper;

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
    let (file_chooser_jobs, file_chooser_rx) = mpsc::channel::<file_chooser::FileChooserJob>();
    let (app_chooser_jobs, app_chooser_rx) = mpsc::channel::<app_chooser::AppChooserJob>();
    let (account_jobs, account_rx) = mpsc::channel::<account::AccountJob>();
    let (dynamic_launcher_jobs, dynamic_launcher_rx) =
        mpsc::channel::<dynamic_launcher::DynamicLauncherJob>();
    let (wallpaper_jobs, wallpaper_rx) = mpsc::channel::<wallpaper::WallpaperJob>();
    let inhibit_counts = Arc::new(Mutex::new(inhibit::InhibitCounts::default()));
    let settings_store = SettingsStore::default();
    settings::prime_store(&socket, &settings_store);

    // Secret storage: the at-rest vault plus the Secret portal interface. A
    // failure here must never take down the other interfaces, so init errors
    // degrade to running without secret support.
    let secret_state = match secret::init() {
        Ok(state) => Some(state),
        Err(error) => {
            log::error!("portal: secret support disabled: {error}");
            None
        }
    };

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
    // Stateless sandbox-policy query surface; no worker, no IPC.
    conn.object_server()
        .at(DESKTOP_PATH, lockdown::LockdownIface)?;
    conn.object_server().at(
        DESKTOP_PATH,
        file_chooser::FileChooserIface {
            conn: conn.inner().clone(),
            tracker: Arc::clone(&tracker),
            jobs: file_chooser_jobs.clone(),
        },
    )?;
    // Email hand-off is fire-and-forget (no worker, no IPC).
    conn.object_server().at(
        DESKTOP_PATH,
        email::EmailIface {
            conn: conn.inner().clone(),
            tracker: Arc::clone(&tracker),
        },
    )?;
    conn.object_server().at(
        DESKTOP_PATH,
        app_chooser::AppChooserIface {
            conn: conn.inner().clone(),
            tracker: Arc::clone(&tracker),
            jobs: app_chooser_jobs.clone(),
        },
    )?;
    conn.object_server().at(
        DESKTOP_PATH,
        account::AccountIface {
            conn: conn.inner().clone(),
            tracker: Arc::clone(&tracker),
            jobs: account_jobs.clone(),
        },
    )?;
    conn.object_server().at(
        DESKTOP_PATH,
        dynamic_launcher::DynamicLauncherIface {
            conn: conn.inner().clone(),
            tracker: Arc::clone(&tracker),
            jobs: dynamic_launcher_jobs.clone(),
        },
    )?;
    conn.object_server().at(
        DESKTOP_PATH,
        wallpaper::WallpaperIface {
            conn: conn.inner().clone(),
            tracker: Arc::clone(&tracker),
            jobs: wallpaper_jobs.clone(),
        },
    )?;
    // Notification calls are short one-shots taken inline (no worker).
    conn.object_server().at(
        DESKTOP_PATH,
        notification::NotificationIface {
            capture: Mutex::new(PortalCapture::new(socket.clone())),
        },
    )?;

    if let Some(state) = &secret_state {
        conn.object_server().at(
            DESKTOP_PATH,
            secret::portal::SecretIface {
                conn: conn.clone(),
                tracker: Arc::clone(&tracker),
                state: Arc::clone(state),
                socket: socket.clone(),
            },
        )?;
    }

    // ------------------------------------------------------------------
    // TRANSITIONAL: the org.freedesktop.secrets compat layer.
    //
    // The classic Secret Service API exists only so un-sandboxed libsecret
    // clients keep working until portal-native secret retrieval
    // (org.freedesktop.impl.portal.Secret above) is universal. Everything
    // compat-only lives in `secret::compat/`; removal means deleting that
    // module, this one registration call, the compat name request further
    // below, the org.freedesktop.secrets.service activation file, the
    // compat-only `sessions`/`SessionCrypto` members of `SecretState`, and
    // the two marked compat call sites in the `secret` module's unlock
    // worker.
    // ------------------------------------------------------------------
    let compat_served = match &secret_state {
        Some(state) => match secret::compat::serve(&conn, state, &socket) {
            Ok(()) => {
                // Password-mode vaults unlock without prompting once a
                // pam_aegis token appears (login or screen unlock).
                secret::compat::spawn_pam_watcher(conn.clone(), Arc::clone(state));
                true
            }
            Err(error) => {
                log::error!("portal: org.freedesktop.secrets compat layer unavailable: {error}");
                false
            }
        },
        None => false,
    };

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

    // File picks block on user interaction (up to the compositor's pick
    // timeout), so they get their own worker rather than the capture one.
    let file_chooser_tracker = Arc::clone(&tracker);
    let file_chooser_socket = socket.clone();
    std::thread::Builder::new()
        .name("aegis-portal-file-chooser".to_string())
        .spawn(move || {
            file_chooser::file_chooser_worker(
                file_chooser_rx,
                file_chooser_tracker,
                PortalCapture::new(file_chooser_socket),
            )
        })
        .map_err(|e| {
            PortalError::Bus(zbus::Error::Failure(format!(
                "spawn file chooser worker: {e}"
            )))
        })?;

    let app_chooser_tracker = Arc::clone(&tracker);
    let app_chooser_socket = socket.clone();
    std::thread::Builder::new()
        .name("aegis-portal-app-chooser".to_string())
        .spawn(move || {
            app_chooser::app_chooser_worker(
                app_chooser_rx,
                app_chooser_tracker,
                PortalCapture::new(app_chooser_socket),
            )
        })
        .map_err(|e| {
            PortalError::Bus(zbus::Error::Failure(format!(
                "spawn app chooser worker: {e}"
            )))
        })?;

    let account_tracker = Arc::clone(&tracker);
    let account_socket = socket.clone();
    std::thread::Builder::new()
        .name("aegis-portal-account".to_string())
        .spawn(move || {
            account::account_worker(
                account_rx,
                account_tracker,
                PortalCapture::new(account_socket),
            )
        })
        .map_err(|e| {
            PortalError::Bus(zbus::Error::Failure(format!("spawn account worker: {e}")))
        })?;

    let dynamic_launcher_tracker = Arc::clone(&tracker);
    let dynamic_launcher_socket = socket.clone();
    std::thread::Builder::new()
        .name("aegis-portal-dyn-launcher".to_string())
        .spawn(move || {
            dynamic_launcher::dynamic_launcher_worker(
                dynamic_launcher_rx,
                dynamic_launcher_tracker,
                PortalCapture::new(dynamic_launcher_socket),
            )
        })
        .map_err(|e| {
            PortalError::Bus(zbus::Error::Failure(format!(
                "spawn dynamic launcher worker: {e}"
            )))
        })?;

    let wallpaper_tracker = Arc::clone(&tracker);
    let wallpaper_socket = socket.clone();
    std::thread::Builder::new()
        .name("aegis-portal-wallpaper".to_string())
        .spawn(move || {
            wallpaper::wallpaper_worker(
                wallpaper_rx,
                wallpaper_tracker,
                PortalCapture::new(wallpaper_socket),
            )
        })
        .map_err(|e| {
            PortalError::Bus(zbus::Error::Failure(format!("spawn wallpaper worker: {e}")))
        })?;

    settings::spawn_watcher(conn.clone(), socket, settings_store);

    conn.request_name(BUS_NAME)?;

    // Compat well-known name (transitional, see above). Another provider may
    // already own org.freedesktop.secrets (e.g. GNOME Keyring); in that case
    // our compat objects stay served but unreachable, which is harmless.
    if compat_served {
        match conn.request_name_with_flags(
            "org.freedesktop.secrets",
            zbus::fdo::RequestNameFlags::DoNotQueue.into(),
        ) {
            Ok(
                zbus::fdo::RequestNameReply::PrimaryOwner
                | zbus::fdo::RequestNameReply::AlreadyOwner,
            ) => {
                log::info!("portal: org.freedesktop.secrets compat name owned");
            }
            Ok(reply) => {
                log::warn!(
                    "portal: org.freedesktop.secrets is owned by another provider ({reply:?}); \
                     the compat layer stays unreachable"
                );
            }
            Err(error) => {
                log::warn!(
                    "portal: could not request org.freedesktop.secrets: {error}; \
                     the compat layer stays unreachable"
                );
            }
        }
    }

    let interfaces = if secret_state.is_some() {
        "Settings+Screenshot+ScreenCast+Inhibit+Secret+Lockdown+FileChooser+Email+AppChooser+Notification+Account+DynamicLauncher+Wallpaper"
    } else {
        "Settings+Screenshot+ScreenCast+Inhibit+Lockdown+FileChooser+Email+AppChooser+Notification+Account+DynamicLauncher+Wallpaper"
    };
    log::info!("portal: serving {interfaces} as {BUS_NAME}");

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
    const SECRETS_SERVICE: &str =
        include_str!("../../../contrib/dbus-1/services/org.freedesktop.secrets.service");

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
                "org.freedesktop.impl.portal.Secret",
                "org.freedesktop.impl.portal.Lockdown",
                "org.freedesktop.impl.portal.FileChooser",
                "org.freedesktop.impl.portal.Email",
                "org.freedesktop.impl.portal.AppChooser",
                "org.freedesktop.impl.portal.Notification",
                "org.freedesktop.impl.portal.Account",
                "org.freedesktop.impl.portal.DynamicLauncher",
                "org.freedesktop.impl.portal.Wallpaper",
            ]
        );
    }

    #[test]
    fn aegis_is_the_default_with_gtk_as_the_safety_net() {
        assert!(
            PORTALS_CONF.lines().any(|line| line == "default=aegis;gtk"),
            "the routing default is Aegis with the GTK safety net (ADR-0086)"
        );
        for interface in [
            "Settings",
            "Screenshot",
            "ScreenCast",
            "Inhibit",
            "Secret",
            "Lockdown",
            "FileChooser",
            "Email",
            "AppChooser",
            "Notification",
            "Account",
            "DynamicLauncher",
            "Wallpaper",
        ] {
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

    #[test]
    fn secrets_compat_service_file_activates_the_same_process() {
        assert!(
            SECRETS_SERVICE
                .lines()
                .any(|line| line == "Name=org.freedesktop.secrets")
        );
        assert!(
            SECRETS_SERVICE
                .lines()
                .any(|line| line == "Exec=/usr/lib/aegis/aegis-portal")
        );
    }
}
