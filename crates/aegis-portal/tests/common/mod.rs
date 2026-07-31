//! Shared helpers for the aegis-portal end-to-end tests.
//!
//! Every test gets its OWN private session bus (a spawned `dbus-daemon`)
//! and spawns the daemon with `DBUS_SESSION_BUS_ADDRESS` pointing at it.
//! The ambient environment of a developer machine points at the live
//! session bus; tests must never claim well-known names there, so nothing
//! here reads the ambient bus address.
//!
//! Cargo compiles this module into every test binary, so helpers not used
//! by one binary warn as dead code; the blanket allow keeps each test file
//! free to use only what it needs.
#![allow(dead_code)]

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// A private session bus; killed on drop.
pub struct PrivateBus {
    address: String,
    child: Child,
}

impl PrivateBus {
    /// The daemon and every client must connect here, never to the ambient
    /// session bus.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// A blocking zbus connection to this bus.
    pub fn connect(&self) -> zbus::blocking::Connection {
        zbus::blocking::connection::Builder::address(self.address.as_str())
            .expect("valid private bus address")
            .build()
            .expect("connect to the private bus")
    }
}

impl Drop for PrivateBus {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Spawn a private `dbus-daemon --session`; `None` when dbus-daemon is not
/// installed (tests skip).
pub fn private_bus() -> Option<PrivateBus> {
    let mut child = Command::new("dbus-daemon")
        .args(["--session", "--nofork", "--print-address=1"])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = child.stdout.take()?;
    let mut line = String::new();
    BufReader::new(stdout).read_line(&mut line).ok()?;
    let address = line.trim().to_string();
    if address.is_empty() {
        let _ = child.kill();
        return None;
    }
    Some(PrivateBus { address, child })
}

/// Spawn the portal daemon bound to the private bus and hermetic XDG dirs.
/// `AEGIS_PORTAL_E2E_DAEMON_LOG=<path>` captures the daemon's stderr into
/// that file for debugging hung flows.
pub fn spawn_daemon(bus: &PrivateBus, data: &PathBuf, runtime: &PathBuf) -> Child {
    let stderr: Stdio = match std::env::var_os("AEGIS_PORTAL_E2E_DAEMON_LOG") {
        Some(path) => Stdio::from(std::fs::File::create(path).expect("create daemon log file")),
        None => Stdio::null(),
    };
    Command::new(env!("CARGO_BIN_EXE_aegis-portal"))
        .env("DBUS_SESSION_BUS_ADDRESS", bus.address())
        .env("XDG_DATA_HOME", data)
        .env("XDG_RUNTIME_DIR", runtime)
        .env(
            "RUST_LOG",
            std::env::var("RUST_LOG").unwrap_or_else(|_| "info".into()),
        )
        .stdout(Stdio::null())
        .stderr(stderr)
        .spawn()
        .expect("spawn aegis-portal")
}

/// Kill a daemon child on drop.
pub struct KillOnDrop(pub Child);

impl Drop for KillOnDrop {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// A unique temp directory tagged by test name and pid.
pub fn temp_dir(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let dir = std::env::temp_dir().join(format!(
        "aegis-portal-e2e-{tag}-{}-{nanos}",
        std::process::id()
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

/// Wait until the daemon owns `name` (10 s bound).
pub fn wait_for_name(conn: &zbus::blocking::Connection, name: &str) {
    let fdo = zbus::blocking::Proxy::new(
        conn,
        "org.freedesktop.DBus",
        "/org/freedesktop/DBus",
        "org.freedesktop.DBus",
    )
    .expect("fdo proxy");
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let owned: bool = fdo
            .call("NameHasOwner", &(name,))
            .expect("NameHasOwner call");
        if owned {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for the daemon to own {name}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// A plain pipe pair — the fd shape real `RetrieveSecret` clients (Chrome,
/// libportal) pass, NOT a socket. Guards the regression where the backend
/// used the socket-only `shutdown(2)` and failed ENOTSOCK on pipes.
pub fn pipe_pair() -> (std::fs::File, std::os::fd::OwnedFd) {
    use std::os::fd::FromRawFd;
    let mut fds = [-1; 2];
    // SAFETY: fds is a valid out-array; on success both ends are owned fds.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe(2)");
    // SAFETY: each raw fd is wrapped exactly once.
    unsafe {
        (
            std::fs::File::from_raw_fd(fds[0]),
            std::os::fd::OwnedFd::from_raw_fd(fds[1]),
        )
    }
}

/// Read a pipe to EOF with a timeout guard (the daemon closes its write
/// end after delivering, or drops it on failure).
pub fn read_all_with_timeout(mut file: std::fs::File, timeout: Duration) -> Vec<u8> {
    use std::io::Read;
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = file.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    rx.recv_timeout(timeout)
        .expect("the pipe must reach EOF within the timeout")
}
