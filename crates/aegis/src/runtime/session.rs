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
/// project name, matching what `aegis-apps` compares `OnlyShowIn=` against.
const DESKTOP_NAME: &str = "aegis";

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

const CONTROL_MESSAGE_LOCK: &[u8] = b"LOCK";
const CONTROL_MESSAGE_STOP: &[u8] = b"STOP";
const RESTART_BACKOFF: std::time::Duration = std::time::Duration::from_secs(5);
const OUTPUT_WAKE_RETRY: std::time::Duration = std::time::Duration::from_secs(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) enum OutputWakeReason {
    CoordinatorRecovery,
    SessionUnlock,
}

impl OutputWakeReason {
    pub(super) fn description(self) -> &'static str {
        match self {
            Self::CoordinatorRecovery => "idle coordinator recovery",
            Self::SessionUnlock => "session unlock",
        }
    }
}

/// Sticky recovery intent owned by the compositor rather than the policy
/// process. A transient KMS busy/inactive result must not consume the only
/// chance to wake a display after the coordinator exits.
struct OutputWakeRecovery {
    reason: Option<OutputWakeReason>,
    retry_at: std::time::Instant,
}

impl OutputWakeRecovery {
    fn new(now: std::time::Instant) -> Self {
        Self {
            reason: None,
            retry_at: now,
        }
    }

    fn require(&mut self, reason: OutputWakeReason, now: std::time::Instant) {
        // Re-observing the same condition must not erase a failure backoff.
        // A user-visible unlock is more urgent and may promote a coordinator
        // recovery intent to an immediate attempt.
        if self.reason.is_none_or(|current| reason > current) {
            self.reason = Some(reason);
            self.retry_at = now;
        }
    }

    fn due(&mut self, outputs_powered: bool, now: std::time::Instant) -> Option<OutputWakeReason> {
        if outputs_powered {
            self.reason = None;
            return None;
        }
        (now >= self.retry_at).then_some(self.reason).flatten()
    }

    fn failed(&mut self, now: std::time::Instant) {
        self.retry_at = now + OUTPUT_WAKE_RETRY;
    }

    fn complete(&mut self) {
        self.reason = None;
    }
}

/// Supervised first-party idle coordinator for this compositor session.
///
/// The compositor owns lifecycle but not policy execution: `aegis-idle` is a
/// normal Wayland client using ext-idle-notify-v1. A crashing coordinator is
/// restarted, and a config update asks the old process to restore any dimmed
/// or powered-down outputs before the replacement starts.
pub(super) struct IdleProcess {
    child: Option<std::process::Child>,
    settings: aegis_model::settings::IdleSettings,
    nested: bool,
    available: bool,
    stopping: bool,
    restart_at: std::time::Instant,
    output_wake: OutputWakeRecovery,
}

impl IdleProcess {
    pub(super) fn start(
        settings: aegis_model::settings::IdleSettings,
        nested: bool,
        available: bool,
    ) -> Self {
        let now = std::time::Instant::now();
        let mut process = Self {
            child: None,
            settings,
            nested,
            available,
            stopping: false,
            restart_at: now,
            output_wake: OutputWakeRecovery::new(now),
        };
        if available {
            process.spawn();
        } else {
            log::warn!(
                "session: idle coordination disabled because this compositor does not own the IPC socket"
            );
        }
        process
    }

    pub(super) fn reconfigure(&mut self, settings: aegis_model::settings::IdleSettings) {
        if self.settings == settings {
            return;
        }
        self.settings = settings;
        if !self.available {
            return;
        }
        self.output_wake.require(
            OutputWakeReason::CoordinatorRecovery,
            std::time::Instant::now(),
        );
        self.stopping = true;
        if !send_control(CONTROL_MESSAGE_STOP)
            && let Some(child) = self.child.as_mut()
        {
            let _ = child.kill();
        }
        if self.child.is_none() {
            self.stopping = false;
            self.spawn();
        }
    }

    /// Reap/restart the coordinator. Output-wake recovery remains sticky until
    /// the backend confirms success through [`Self::output_wake_succeeded`].
    pub(super) fn maintain(&mut self) {
        if !self.available {
            return;
        }
        let exited = self
            .child
            .as_mut()
            .and_then(|child| child.try_wait().ok())
            .flatten();
        if let Some(status) = exited {
            self.child = None;
            let now = std::time::Instant::now();
            self.output_wake
                .require(OutputWakeReason::CoordinatorRecovery, now);
            if self.stopping {
                log::debug!("session: idle coordinator replaced ({status})");
                self.stopping = false;
                self.restart_at = now;
            } else {
                log::warn!("session: idle coordinator exited ({status}); scheduling restart");
                self.restart_at = now + RESTART_BACKOFF;
            }
        }
        if self.child.is_none() && std::time::Instant::now() >= self.restart_at {
            self.spawn();
        }
    }

    pub(super) fn require_output_wake(&mut self, reason: OutputWakeReason) {
        self.output_wake.require(reason, std::time::Instant::now());
    }

    pub(super) fn output_wake_due(&mut self, outputs_powered: bool) -> Option<OutputWakeReason> {
        self.output_wake
            .due(outputs_powered, std::time::Instant::now())
    }

    pub(super) fn output_wake_failed(&mut self) {
        self.output_wake.failed(std::time::Instant::now());
    }

    pub(super) fn output_wake_succeeded(&mut self) {
        self.output_wake.complete();
    }

    pub(super) fn lock_now(&mut self) {
        if !self.available {
            self.start_direct_lock();
            return;
        }
        self.maintain();
        if !self.stopping && send_control(CONTROL_MESSAGE_LOCK) {
            return;
        }
        // The daemon may still be between exec and bind. Its one-shot mode
        // retries the control path and securely falls back to aegis-lock.
        let result = trusted_sibling_program("aegis-idle").and_then(|program| {
            std::process::Command::new(program)
                .arg("--lock-now")
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .spawn()
        });
        if let Err(error) = result {
            log::error!("session: could not request lock: {error}");
            self.start_direct_lock();
        }
    }

    fn start_direct_lock(&self) {
        let fallback = trusted_sibling_program("aegis-lock").and_then(|program| {
            std::process::Command::new(program)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .spawn()
        });
        if let Err(error) = fallback {
            log::error!("session: could not start aegis-lock fallback: {error}");
        }
    }

    fn spawn(&mut self) {
        let settings = self.settings;
        let enabled_timeout = |seconds: u32| {
            if settings.enabled && seconds != 0 {
                seconds.to_string()
            } else {
                "off".to_owned()
            }
        };
        let program = match trusted_sibling_program("aegis-idle") {
            Ok(program) => program,
            Err(error) => {
                log::warn!("session: could not start aegis-idle: {error}");
                self.restart_at = std::time::Instant::now() + RESTART_BACKOFF;
                return;
            }
        };
        let mut command = std::process::Command::new(program);
        command
            .args([
                "--dim-after",
                &enabled_timeout(if self.nested {
                    0
                } else {
                    settings.dim_after_seconds
                }),
            ])
            .args([
                "--lock-after",
                &enabled_timeout(settings.lock_after_seconds),
            ])
            .args([
                "--display-off-after",
                &enabled_timeout(if self.nested {
                    0
                } else {
                    settings.display_off_after_seconds
                }),
            ])
            .args([
                "--suspend-after",
                &enabled_timeout(if self.nested {
                    0
                } else {
                    settings.suspend_after_seconds
                }),
            ])
            .args(["--dim-percent", &settings.dim_percent.to_string()])
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null());
        if self.nested {
            command.arg("--no-logind");
        }
        match command.spawn() {
            Ok(child) => {
                log::info!(
                    "session: idle coordinator started (automatic idle {})",
                    if settings.enabled {
                        "enabled"
                    } else {
                        "disabled"
                    }
                );
                self.child = Some(child);
            }
            Err(error) => {
                log::warn!("session: could not start aegis-idle: {error}");
                self.restart_at = std::time::Instant::now() + RESTART_BACKOFF;
            }
        }
    }
}

impl Drop for IdleProcess {
    fn drop(&mut self) {
        if self.child.is_none() {
            return;
        }
        let _ = send_control(CONTROL_MESSAGE_STOP);
        if let Some(child) = self.child.as_mut() {
            for _ in 0..20 {
                if child.try_wait().ok().flatten().is_some() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn send_control(message: &[u8]) -> bool {
    let Some(path) = control_socket_path() else {
        return false;
    };
    std::os::unix::net::UnixDatagram::unbound()
        .and_then(|socket| {
            socket.connect(path)?;
            socket.send(message)
        })
        .is_ok()
}

fn control_socket_path() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(std::path::PathBuf::from)
        .map(|directory| directory.join("aegis-idle.sock"))
}

fn trusted_sibling_program(name: &str) -> std::io::Result<std::ffi::OsString> {
    use std::os::unix::fs::{MetadataExt as _, PermissionsExt as _};

    let current = std::env::current_exe()?;
    let path = current.with_file_name(name);
    let current_metadata = std::fs::symlink_metadata(&current)?;
    let metadata = std::fs::symlink_metadata(&path).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!("trusted sibling {name} binary is missing: {error}"),
        )
    })?;
    let parent = path.parent().ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "trusted sibling has no parent directory",
        )
    })?;
    let parent_metadata = std::fs::symlink_metadata(parent)?;
    let mode = metadata.permissions().mode();
    if !metadata.file_type().is_file()
        || !parent_metadata.file_type().is_dir()
        || metadata.uid() != current_metadata.uid()
        || parent_metadata.uid() != current_metadata.uid()
        || mode & 0o111 == 0
        || mode & 0o022 != 0
        || parent_metadata.permissions().mode() & 0o022 != 0
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            format!("trusted sibling {name} has unsafe ownership, mode, or parent"),
        ));
    }
    Ok(path.into_os_string())
}

fn forward_adapter_environment(command: &mut std::process::Command) {
    command.env_clear();
    for name in [
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
        "AT_SPI_BUS_ADDRESS",
        "LANG",
        "LC_ALL",
        "RUST_LOG",
    ] {
        if let Some(value) = std::env::var_os(name) {
            command.env(name, value);
        }
    }
}

/// Supervised, out-of-process accessibility adapter. Its credential is a
/// compositor-lifetime secret delivered through stdin, never argv, the
/// environment, or persistent storage. The process reconnects with the same
/// ephemeral principal after crashes; dropping the compositor kills it and
/// destroys the principal registry that recognizes the credential.
pub(super) struct SemanticAdapterProcess {
    child: Option<std::process::Child>,
    credential: String,
    available: bool,
    restart_at: std::time::Instant,
}

impl SemanticAdapterProcess {
    pub(super) fn start(credential: String, available: bool) -> Self {
        let mut process = Self {
            child: None,
            credential,
            available,
            restart_at: std::time::Instant::now(),
        };
        if available {
            process.spawn();
        } else {
            log::info!(
                "session: semantic accessibility adapter disabled outside the production IPC session"
            );
        }
        process
    }

    pub(super) fn maintain(&mut self) {
        if !self.available {
            return;
        }
        if let Some(status) = self
            .child
            .as_mut()
            .and_then(|child| child.try_wait().ok())
            .flatten()
        {
            self.child = None;
            self.restart_at = std::time::Instant::now() + RESTART_BACKOFF;
            log::warn!(
                "session: semantic accessibility adapter exited ({status}); scheduling restart"
            );
        }
        if self.child.is_none() && std::time::Instant::now() >= self.restart_at {
            self.spawn();
        }
    }

    fn spawn(&mut self) {
        let program = match trusted_sibling_program("aegis-atspi") {
            Ok(program) => program,
            Err(error) => {
                log::warn!("session: could not start aegis-atspi: {error}");
                self.restart_at = std::time::Instant::now() + RESTART_BACKOFF;
                return;
            }
        };
        let mut command = std::process::Command::new(program);
        forward_adapter_environment(&mut command);
        command
            .arg("--credential-stdin")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::null());
        match command.spawn() {
            Ok(mut child) => {
                let delivered = child.stdin.take().is_some_and(|mut stdin| {
                    use std::io::Write as _;
                    writeln!(stdin, "{}", self.credential).is_ok()
                });
                if delivered {
                    log::info!("session: semantic accessibility adapter started");
                    self.child = Some(child);
                } else {
                    let _ = child.kill();
                    let _ = child.wait();
                    log::warn!("session: could not deliver aegis-atspi credential");
                    self.restart_at = std::time::Instant::now() + RESTART_BACKOFF;
                }
            }
            Err(error) => {
                log::warn!("session: could not start aegis-atspi: {error}");
                self.restart_at = std::time::Instant::now() + RESTART_BACKOFF;
            }
        }
    }
}

impl Drop for SemanticAdapterProcess {
    fn drop(&mut self) {
        use zeroize::Zeroize as _;
        self.credential.zeroize();
        if let Some(child) = self.child.as_mut() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn output_wake_recovery_survives_transient_failure_until_confirmed() {
        let now = std::time::Instant::now();
        let mut recovery = OutputWakeRecovery::new(now);
        recovery.require(OutputWakeReason::CoordinatorRecovery, now);
        assert_eq!(
            recovery.due(false, now),
            Some(OutputWakeReason::CoordinatorRecovery)
        );

        recovery.failed(now);
        recovery.require(OutputWakeReason::CoordinatorRecovery, now);
        assert_eq!(recovery.due(false, now), None);
        assert_eq!(
            recovery.due(false, now + OUTPUT_WAKE_RETRY),
            Some(OutputWakeReason::CoordinatorRecovery)
        );

        recovery.complete();
        assert_eq!(recovery.due(false, now + OUTPUT_WAKE_RETRY), None);
    }

    #[test]
    fn session_unlock_promotes_a_backed_off_recovery_intent() {
        let now = std::time::Instant::now();
        let mut recovery = OutputWakeRecovery::new(now);
        recovery.require(OutputWakeReason::CoordinatorRecovery, now);
        recovery.failed(now);
        assert_eq!(recovery.due(false, now), None);

        recovery.require(OutputWakeReason::SessionUnlock, now);
        assert_eq!(
            recovery.due(false, now),
            Some(OutputWakeReason::SessionUnlock)
        );
    }

    #[test]
    fn observing_powered_outputs_completes_stale_recovery_intent() {
        let now = std::time::Instant::now();
        let mut recovery = OutputWakeRecovery::new(now);
        recovery.require(OutputWakeReason::CoordinatorRecovery, now);
        assert_eq!(recovery.due(true, now), None);
        assert_eq!(recovery.due(false, now), None);
    }
}
