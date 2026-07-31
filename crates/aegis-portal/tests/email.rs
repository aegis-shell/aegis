//! End-to-end exercise of the Email backend: the real `aegis-portal` daemon
//! on a private session bus (see `tests/common/`) with `AEGIS_PORTAL_MAILER`
//! pointed at a recorder script, so the xdg-email hand-off is asserted
//! without opening a mail client.
//!
//! ```sh
//! cargo test -p aegis-portal --test email
//! ```
//!
//! Without `dbus-daemon` available the test skips cleanly.

use std::collections::HashMap;
use std::os::fd::{FromRawFd, IntoRawFd, OwnedFd};
use std::process::Command;
use std::time::{Duration, Instant};

use zbus::blocking::Proxy;
use zbus::zvariant::{Fd, ObjectPath, OwnedValue, Value};

mod common;
use common::{KillOnDrop, private_bus, temp_dir, wait_for_name};

const PORTAL: &str = "org.freedesktop.impl.portal.desktop.aegis";
const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE: &str = "org.freedesktop.impl.portal.Email";

/// Poll the recorder file until the mailer ran (or fail after 5 s).
fn recorded_args(record_file: &std::path::Path) -> String {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Ok(args) = std::fs::read_to_string(record_file) {
            return args;
        }
        assert!(
            Instant::now() < deadline,
            "the mailer was not invoked within 5 s"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn compose_email_hands_off_to_the_mailer() {
    let Some(bus) = private_bus() else {
        eprintln!("compose_email_hands_off_to_the_mailer: no dbus-daemon, skipping");
        return;
    };
    let conn = bus.connect();

    let data_dir = temp_dir("data");
    let runtime_dir = temp_dir("runtime");
    let cache_dir = temp_dir("cache");
    let record_file = runtime_dir.join("mailer-args.txt");

    // A recorder standing in for xdg-email: appends its argv, one per line.
    let recorder = runtime_dir.join("record-mailer.sh");
    std::fs::write(
        &recorder,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" >> {}\n",
            record_file.display()
        ),
    )
    .expect("write recorder");
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&recorder, std::fs::Permissions::from_mode(0o755))
            .expect("chmod recorder");
    }

    // Like common::spawn_daemon, plus the mailer override and a hermetic
    // cache dir for the attachment staging assertion.
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_aegis-portal"));
    cmd.env("DBUS_SESSION_BUS_ADDRESS", bus.address())
        .env("XDG_DATA_HOME", &data_dir)
        .env("XDG_RUNTIME_DIR", &runtime_dir)
        .env("XDG_CACHE_HOME", &cache_dir)
        .env("AEGIS_PORTAL_MAILER", &recorder)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    let _daemon = KillOnDrop(cmd.spawn().expect("spawn aegis-portal"));

    wait_for_name(&conn, PORTAL);
    let email = Proxy::new(&conn, PORTAL, DESKTOP_PATH, IFACE).expect("email proxy");
    let version: u32 = email.get_property("version").expect("version property");
    assert_eq!(version, 2);

    // One attachment through a real fd: the daemon must stage it and pass a
    // --attach path.
    let payload_file = runtime_dir.join("payload.bin");
    std::fs::write(&payload_file, b"attached-bytes").expect("write payload");
    let payload = std::fs::File::open(&payload_file).expect("open payload");
    // SAFETY: the file descriptor is turned into an OwnedFd exactly once.
    let attachment: Fd<'_> = Fd::from(unsafe { OwnedFd::from_raw_fd(payload.into_raw_fd()) });

    let mut options: HashMap<String, Value<'_>> = HashMap::new();
    options.insert("address".to_string(), Value::from("to@example.com"));
    options.insert(
        "cc".to_string(),
        Value::from(vec!["carbon@example.com".to_string()]),
    );
    options.insert("subject".to_string(), Value::from("portal subject"));
    options.insert("body".to_string(), Value::from("portal body"));
    options.insert("attachment_fds".to_string(), Value::from(vec![attachment]));

    let handle = ObjectPath::try_from("/org/freedesktop/portal/desktop/request/1/mail1")
        .expect("request handle path");
    let (response, _results): (u32, HashMap<String, OwnedValue>) = email
        .call("ComposeEmail", &(handle, "dev.aegis.smoke", "", options))
        .expect("ComposeEmail");
    assert_eq!(response, 0, "ComposeEmail must report success");

    let args = recorded_args(&record_file);
    let lines: Vec<&str> = args.lines().collect();
    let flag_value = |flag: &str| {
        let at = lines
            .iter()
            .position(|arg| *arg == flag)
            .unwrap_or_else(|| panic!("missing {flag} in {args}"));
        lines[at + 1]
    };
    assert_eq!(flag_value("--cc"), "carbon@example.com");
    assert_eq!(flag_value("--subject"), "portal subject");
    assert_eq!(flag_value("--body"), "portal body");
    let attach = flag_value("--attach");
    assert!(
        attach.starts_with(cache_dir.to_str().expect("utf8 cache dir")),
        "the attachment must be staged under the cache dir: {attach}"
    );
    assert_eq!(
        std::fs::read(attach).expect("staged attachment"),
        b"attached-bytes"
    );
    assert_eq!(lines.last().copied(), Some("mailto:to@example.com"));

    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&runtime_dir);
    let _ = std::fs::remove_dir_all(&cache_dir);
}
