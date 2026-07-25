//! Session environment publication.
//!
//! The compositor owns the session, so it publishes the environment that
//! launched clients and D-Bus-activated services (xdg-desktop-portal,
//! flatpak-spawn, …) need to connect back: `WAYLAND_DISPLAY`,
//! `XDG_SESSION_TYPE`, and `XDG_CURRENT_DESKTOP`. This mirrors the
//! sway/labwc/Hyprland startup sequence: set the variables in-process once
//! the Wayland socket name is known, then best-effort export them to the
//! D-Bus activation environment and the systemd --user manager.

/// Desktop name advertised through `XDG_CURRENT_DESKTOP`. The lowercase
/// project name, matching what `ass-apps` compares `OnlyShowIn=` against.
const DESKTOP_NAME: &str = "ass";

/// Session variables every Wayland client of this compositor needs.
const SESSION_VARS: [&str; 3] = ["WAYLAND_DISPLAY", "XDG_SESSION_TYPE", "XDG_CURRENT_DESKTOP"];

/// Publish the session environment for this process and everything it
/// launches, then export it to D-Bus/systemd. Called once the Wayland socket
/// exists so its name is known. `nested` sessions share the host's D-Bus and
/// systemd --user manager, so the activation export is skipped there: pointing
/// the outer session's activated services at the inner socket would break
/// them.
pub(crate) fn publish(socket_name: &str, nested: bool) {
    // SAFETY: this runs at startup before any thread that reads the process
    // environment is spawned. The only earlier thread is the capture worker,
    // which encodes frames and never touches the environment. All three
    // variables describe this compositor's session, so they are set
    // unconditionally — a nested host's inherited `XDG_SESSION_TYPE` or
    // `WAYLAND_DISPLAY` (already consumed by the backend above) does not
    // apply to clients of this compositor.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", socket_name);
        std::env::set_var("XDG_SESSION_TYPE", "wayland");
        std::env::set_var("XDG_CURRENT_DESKTOP", DESKTOP_NAME);
    }
    if nested {
        log::info!(
            "session: nested backend; D-Bus/systemd activation environment left to the host"
        );
        return;
    }
    export_activation_environment();
}

/// Best-effort export of the session environment to the D-Bus activation
/// environment and, via `--systemd`, the systemd --user manager, so services
/// activated later (portals, flatpak-spawn helpers) inherit it. A missing
/// binary or a failing command is not fatal: directly launched clients still
/// receive the environment from the compositor process itself.
fn export_activation_environment() {
    match std::process::Command::new("dbus-update-activation-environment")
        .arg("--systemd")
        .args(SESSION_VARS)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
    {
        Ok(status) if status.success() => {
            log::info!(
                "session: exported {} to the D-Bus/systemd activation environment",
                SESSION_VARS.join(" ")
            );
        }
        Ok(status) => {
            log::warn!("session: dbus-update-activation-environment exited with {status}");
        }
        Err(error) => {
            log::warn!("session: could not run dbus-update-activation-environment: {error}");
        }
    }
}
