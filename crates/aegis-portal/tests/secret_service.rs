//! End-to-end smoke test for the secret stack: spawns the real
//! `aegis-portal` daemon on a private session bus (see `tests/common/`) and
//! exercises the transitional `org.freedesktop.secrets` API plus the native
//! `org.freedesktop.impl.portal.Secret.RetrieveSecret` fd transfer. The
//! private bus is hermetic: the live session bus is never touched.
//!
//! ```sh
//! cargo test -p aegis-portal --test secret_service
//! ```
//!
//! Without `dbus-daemon` available the test skips cleanly.

use std::collections::HashMap;
use std::io::Read;
use std::os::fd::{FromRawFd, OwnedFd};
use std::time::Duration;

use zbus::blocking::Proxy;
use zbus::zvariant::{Fd, ObjectPath, OwnedObjectPath, Value};

mod common;
use common::{KillOnDrop, private_bus, spawn_daemon, temp_dir, wait_for_name};

const SERVICE: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const PORTAL: &str = "org.freedesktop.impl.portal.desktop.aegis";
const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";

/// Mirror of the daemon's wire struct (compat `SecretStruct`).
#[derive(serde::Deserialize, zbus::zvariant::Type, Debug)]
#[allow(unused)]
struct SecretStruct {
    session: OwnedObjectPath,
    parameters: Vec<u8>,
    value: Vec<u8>,
    content_type: String,
}

/// The HKDF-SHA256 portal-secret derivation, duplicated from
/// `secret::portal` (the wire contract under test).
fn expected_portal_secret(data_dir: &std::path::Path) -> [u8; 32] {
    let hex = std::fs::read_to_string(data_dir.join("aegis/secrets/vault.key"))
        .expect("daemon must create vault.key");
    let hex = hex.trim();
    assert_eq!(hex.len(), 64, "vault.key must hold 32 hex bytes");
    let mut key = [0u8; 32];
    for (i, chunk) in hex.as_bytes().chunks(2).enumerate() {
        key[i] = u8::from_str_radix(std::str::from_utf8(chunk).unwrap(), 16).unwrap();
    }
    let hk = hkdf::Hkdf::<sha2::Sha256>::new(None, &key);
    let mut out = [0u8; 32];
    hk.expand(b"aegis.portal.Secret/v1", &mut out).unwrap();
    out
}

/// Read a pipe to EOF with a timeout guard (the daemon closes its write
/// end after delivering the secret).
fn read_all_with_timeout(mut file: std::fs::File, timeout: Duration) -> Vec<u8> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = file.read_to_end(&mut buf);
        let _ = tx.send(buf);
    });
    rx.recv_timeout(timeout)
        .expect("the portal secret must arrive within the timeout")
}

/// A plain pipe pair, the fd shape real RetrieveSecret clients (Chrome,
/// libportal) pass — NOT a socket. Guards the regression where the backend
/// used the socket-only `shutdown(2)` and failed ENOTSOCK on pipes.
fn pipe_pair() -> (std::fs::File, OwnedFd) {
    let mut fds = [-1; 2];
    // SAFETY: fds is a valid out-array; on success both ends are owned fds.
    assert_eq!(unsafe { libc::pipe(fds.as_mut_ptr()) }, 0, "pipe(2)");
    // SAFETY: each raw fd is wrapped exactly once.
    unsafe {
        (
            std::fs::File::from_raw_fd(fds[0]),
            OwnedFd::from_raw_fd(fds[1]),
        )
    }
}

#[test]
fn secret_service_end_to_end() {
    let Some(bus) = private_bus() else {
        eprintln!("secret_service_end_to_end: no dbus-daemon, skipping");
        return;
    };
    let conn = bus.connect();

    let data_dir = temp_dir("data");
    let runtime_dir = temp_dir("runtime");
    let _daemon = KillOnDrop(spawn_daemon(&bus, &data_dir, &runtime_dir));

    wait_for_name(&conn, PORTAL);
    wait_for_name(&conn, SERVICE);

    // -- compat layer: org.freedesktop.Secret.Service ----------------------
    let service = Proxy::new(
        &conn,
        SERVICE,
        SERVICE_PATH,
        "org.freedesktop.Secret.Service",
    )
    .expect("service proxy");

    let login: OwnedObjectPath = service
        .call("ReadAlias", &("default",))
        .expect("ReadAlias default");
    assert_eq!(
        login.as_str(),
        "/org/freedesktop/secrets/collection/login",
        "the default alias must resolve to the login collection"
    );

    let (_output, session_path): (zbus::zvariant::OwnedValue, OwnedObjectPath) = service
        .call("OpenSession", &("plain", Value::from("")))
        .expect("OpenSession plain");

    // Store a secret through the plain session.
    let mut attributes = HashMap::new();
    attributes.insert("purpose".to_string(), "smoke".to_string());
    let mut properties: HashMap<&str, Value<'_>> = HashMap::new();
    properties.insert(
        "org.freedesktop.Secret.Item.Label",
        Value::from("smoke item"),
    );
    properties.insert(
        "org.freedesktop.Secret.Item.Attributes",
        Value::from(attributes.clone()),
    );
    let collection = Proxy::new(
        &conn,
        SERVICE,
        login.as_str(),
        "org.freedesktop.Secret.Collection",
    )
    .expect("collection proxy");
    let (item_path, _): (OwnedObjectPath, OwnedObjectPath) = collection
        .call(
            "CreateItem",
            &(
                properties,
                (
                    session_path.clone(),
                    Vec::<u8>::new(),
                    b"s3cr3t".to_vec(),
                    "text/plain".to_string(),
                ),
                false,
            ),
        )
        .expect("CreateItem");

    // Read it back through both Item.GetSecret and Service.GetSecrets.
    let item = Proxy::new(
        &conn,
        SERVICE,
        item_path.as_str(),
        "org.freedesktop.Secret.Item",
    )
    .expect("item proxy");
    let (secret,): (SecretStruct,) = item
        .call("GetSecret", &(session_path.clone(),))
        .expect("GetSecret");
    assert_eq!(secret.value, b"s3cr3t");

    let secrets: HashMap<OwnedObjectPath, SecretStruct> = service
        .call(
            "GetSecrets",
            &(vec![item_path.clone()], session_path.clone()),
        )
        .expect("GetSecrets");
    assert_eq!(secrets[&item_path].value, b"s3cr3t");

    let (matched, locked): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = service
        .call("SearchItems", &(attributes,))
        .expect("SearchItems");
    assert!(locked.is_empty());
    assert!(matched.contains(&item_path));

    // Item.Delete drops the object from the bus, not just the search index.
    let _: OwnedObjectPath = item.call("Delete", &()).expect("Delete item");
    assert!(
        item.call("GetSecret", &(session_path.clone(),))
            .map(|_: (SecretStruct,)| ())
            .is_err(),
        "a deleted item must leave the bus"
    );
    let mut remaining = HashMap::new();
    remaining.insert("purpose".to_string(), "smoke".to_string());
    let (matched, _): (Vec<OwnedObjectPath>, Vec<OwnedObjectPath>) = service
        .call("SearchItems", &(remaining,))
        .expect("SearchItems after Delete");
    assert!(!matched.contains(&item_path));

    // -- native interface: org.freedesktop.impl.portal.Secret --------------
    // Every backend interface must serve the spec's lowercase `version`
    // property; xdg-desktop-portal skips interfaces whose version it cannot
    // read (zbus would otherwise auto-PascalCase it to `Version`).
    let portal = Proxy::new(
        &conn,
        PORTAL,
        DESKTOP_PATH,
        "org.freedesktop.impl.portal.Secret",
    )
    .expect("portal secret proxy");
    let version: u32 = portal.get_property("version").expect("version property");
    assert_eq!(version, 1);
    for (interface, expected) in [
        ("org.freedesktop.impl.portal.Settings", 1),
        ("org.freedesktop.impl.portal.Screenshot", 2),
        ("org.freedesktop.impl.portal.ScreenCast", 3),
    ] {
        let proxy = Proxy::new(&conn, PORTAL, DESKTOP_PATH, interface).expect("backend proxy");
        let version: u32 = proxy
            .get_property("version")
            .unwrap_or_else(|e| panic!("{interface} must serve a lowercase version property: {e}"));
        assert_eq!(version, expected, "{interface} version");
    }

    // Lockdown: stateless, every restriction flag permissive.
    let lockdown = Proxy::new(
        &conn,
        PORTAL,
        DESKTOP_PATH,
        "org.freedesktop.impl.portal.Lockdown",
    )
    .expect("lockdown proxy");
    for property in [
        "disable-printing",
        "disable-save-to-disk",
        "disable-application-handlers",
        "disable-location",
        "disable-camera",
        "disable-microphone",
        "disable-sound-output",
    ] {
        let restricted: bool = lockdown
            .get_property(property)
            .unwrap_or_else(|e| panic!("Lockdown must serve {property}: {e}"));
        assert!(!restricted, "Lockdown {property} must be permissive");
    }

    let (read_end, write_end) = pipe_pair();
    let fd: Fd<'_> = Fd::from(write_end);
    let handle = ObjectPath::try_from("/org/freedesktop/portal/desktop/request/1/smoke")
        .expect("request handle path");
    let (response, _results): (u32, HashMap<String, zbus::zvariant::OwnedValue>) = portal
        .call(
            "RetrieveSecret",
            &(
                handle,
                "dev.aegis.smoke",
                fd,
                HashMap::<String, Value<'_>>::new(),
            ),
        )
        .expect("RetrieveSecret");
    assert_eq!(response, 0, "RetrieveSecret must report success");

    let delivered = read_all_with_timeout(read_end, Duration::from_secs(5));
    assert_eq!(
        delivered.as_slice(),
        &expected_portal_secret(&data_dir),
        "the pipe must deliver HKDF-SHA256(vault key, aegis.portal.Secret/v1)"
    );

    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&runtime_dir);
}
