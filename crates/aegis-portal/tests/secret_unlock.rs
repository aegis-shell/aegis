//! End-to-end exercise of the password-unlock flow: a password-mode vault
//! (no keyfile) on disk, the real `aegis-portal` daemon on a private bus,
//! and a fake compositor answering `PromptSecret`. `Unlock` must prompt,
//! derive the key, and unlock; a cancelled prompt must leave the vault
//! locked.
//!
//! ```sh
//! cargo test -p aegis-portal --test secret_unlock
//! ```
//!
//! Without `dbus-daemon` available the test skips cleanly.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use aegis_ipc::{
    Capabilities, Handler, JournalSnapshot, OpClass, Scope, SecretPromptResult, Server,
};
use argon2::Argon2;
use argon2::password_hash::SaltString;
use chacha20poly1305::XChaCha20Poly1305;
use chacha20poly1305::aead::{Aead, AeadCore, KeyInit};
use hkdf::Hkdf;
use rand::rngs::OsRng;
use sha2::Sha256;
use zbus::blocking::Proxy;
use zbus::zvariant::{Fd, ObjectPath, OwnedObjectPath, Value};

mod common;
use common::{
    KillOnDrop, pipe_pair, private_bus, read_all_with_timeout, spawn_daemon, temp_dir,
    wait_for_name,
};

const SERVICE: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const PORTAL: &str = "org.freedesktop.impl.portal.desktop.aegis";
const DESKTOP_PATH: &str = "/org/freedesktop/portal/desktop";
const PASSWORD: &str = "hunter2";
/// Fixed test salt ("somesalt" in base64).
const SALT_B64: &str = "c29tZXNhbHQ";

/// Write a password-mode vault fixture (`vault.salt` + `vault.enc`, no
/// `vault.key`) holding one login collection with one item.
fn write_password_vault(secrets_dir: &std::path::Path) {
    std::fs::create_dir_all(secrets_dir).expect("create secrets dir");
    std::fs::write(secrets_dir.join("vault.salt"), SALT_B64).expect("write salt");

    let salt = SaltString::from_b64(SALT_B64).expect("parse salt");
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(PASSWORD.as_bytes(), salt.as_str().as_bytes(), &mut key)
        .expect("derive fixture key");

    // VaultData with one collection "login" and one item, mirroring the
    // daemon's serde_json layout.
    let data = serde_json::json!({
        "collections": [{
            "label": "Login",
            "id": "login",
            "items": [{
                "id": "i01",
                "label": "token",
                "attributes": { "k": "v" },
                "secret": [115, 51, 99, 114, 51, 116],
            }],
        }],
    });
    let plaintext = serde_json::to_vec(&data).expect("serialize fixture");

    let cipher = XChaCha20Poly1305::new((&key).into());
    let nonce = XChaCha20Poly1305::generate_nonce(&mut OsRng);
    let ciphertext = cipher.encrypt(&nonce, plaintext.as_ref()).expect("encrypt");
    let mut file = nonce.to_vec();
    file.extend_from_slice(&ciphertext);
    std::fs::write(secrets_dir.join("vault.enc"), file).expect("write vault");
}

/// A fake compositor answering `PromptSecret` with a scripted outcome. An
/// optional gate blocks the prompt until the test releases it, so multiple
/// concurrent unlockers can be observed mid-flight before a single worker
/// interaction completes them all.
struct FakeCompositor {
    answer: SecretPromptResult,
    gate: Option<Arc<(Mutex<bool>, std::sync::Condvar)>>,
}

impl Handler for FakeCompositor {
    fn policy_caps(&self) -> Capabilities {
        Capabilities {
            query: true,
            control: true,
            input: false,
            session: false,
            realm: false,
        }
    }

    fn windows(&self) -> Vec<aegis_core::window::Window> {
        Vec::new()
    }

    fn workspaces(&self) -> aegis_core::workspace::WorkspaceSnapshot {
        aegis_core::workspace::WorkspaceSnapshot { outputs: vec![] }
    }

    fn notifications(&self) -> Vec<aegis_core::notify::Notification> {
        Vec::new()
    }

    fn outputs(&self) -> Vec<aegis_core::output::OutputInfo> {
        Vec::new()
    }

    fn journal_since(&self, _since: u64) -> JournalSnapshot {
        JournalSnapshot {
            entries: vec![],
            oldest_seq: 1,
            latest_seq: 0,
        }
    }

    fn command(&self, _conn_id: u64, _cmd: aegis_ipc::Command) {}

    fn resolve_scope(&self, name: &str) -> Option<Scope> {
        (name == aegis_ipc::LOCAL_PORTAL_SCOPE).then(|| Scope {
            ops: Some(vec![OpClass::PromptSecret]),
            ..Scope::default()
        })
    }

    fn prompt_secret(
        &self,
        _conn_id: u64,
        _title: String,
        _reason: Option<String>,
    ) -> Result<SecretPromptResult, String> {
        if let Some(gate) = &self.gate {
            let (lock, cvar) = &**gate;
            let mut released = lock.lock().unwrap();
            while !*released {
                released = cvar.wait(released).unwrap();
            }
        }
        Ok(self.answer.clone())
    }
}

/// Run the unlock flow against one scripted prompt answer and report the
/// default-alias resolution afterwards: `/…/login` when unlocked, `/` when
/// still locked.
fn unlock_flow(answer: SecretPromptResult) -> String {
    let Some(bus) = private_bus() else {
        eprintln!("secret_unlock: no dbus-daemon, skipping");
        return "skipped".to_string();
    };
    let conn = bus.connect();

    let data_dir = temp_dir("data");
    write_password_vault(&data_dir.join("aegis/secrets"));
    let runtime_dir = temp_dir("runtime");

    let fake = Arc::new(FakeCompositor { answer, gate: None });
    let _server = Server::start(&runtime_dir.join("aegis.sock"), Arc::clone(&fake))
        .expect("bind fake compositor IPC");

    let _daemon = KillOnDrop(spawn_daemon(&bus, &data_dir, &runtime_dir));
    wait_for_name(&conn, SERVICE);

    let service = Proxy::new(
        &conn,
        SERVICE,
        SERVICE_PATH,
        "org.freedesktop.Secret.Service",
    )
    .expect("service proxy");

    // Locked at startup (password vault, no keyfile): the alias is empty.
    let alias: OwnedObjectPath = service
        .call("ReadAlias", &("default",))
        .expect("ReadAlias while locked");
    assert_eq!(alias.as_str(), "/", "password vault must start locked");

    let login =
        ObjectPath::try_from("/org/freedesktop/secrets/collection/login").expect("login path");
    let (_unlocked, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) =
        service.call("Unlock", &(vec![login],)).expect("Unlock");
    assert_ne!(prompt.as_str(), "/", "a prompt must be in flight");

    // Wait for the prompt thread to finish the IPC round-trip.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let resolved = loop {
        let alias: OwnedObjectPath = service
            .call("ReadAlias", &("default",))
            .expect("ReadAlias after unlock");
        if alias.as_str() != "/" {
            break alias.as_str().to_string();
        }
        if std::time::Instant::now() >= deadline {
            break alias.as_str().to_string();
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    };

    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&runtime_dir);
    resolved
}

#[test]
fn unlock_with_the_right_password_opens_the_vault() {
    let resolved = unlock_flow(SecretPromptResult::Secret {
        value: PASSWORD.to_string(),
    });
    if resolved == "skipped" {
        return;
    }
    assert_eq!(resolved, "/org/freedesktop/secrets/collection/login");
}

#[test]
fn a_dismissed_prompt_leaves_the_vault_locked() {
    let resolved = unlock_flow(SecretPromptResult::Cancelled);
    if resolved == "skipped" {
        return;
    }
    assert_eq!(resolved, "/");
}

/// Write a PAM token file with the given password, mode 0600, where
/// `pam_aegis` (and the daemon's consumer) expects it.
fn write_pam_token(runtime_dir: &std::path::Path, password: &str) {
    use std::os::unix::fs::OpenOptionsExt;
    let path = runtime_dir.join("aegis-pam-token");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)
        .expect("create token");
    use std::io::Write;
    file.write_all(password.as_bytes()).expect("write token");
}

/// Resolve the default alias once (unlike `unlock_flow`, no Unlock call).
fn default_alias(conn: &zbus::blocking::Connection) -> String {
    let service = Proxy::new(
        conn,
        SERVICE,
        SERVICE_PATH,
        "org.freedesktop.Secret.Service",
    )
    .expect("service proxy");
    let alias: OwnedObjectPath = service.call("ReadAlias", &("default",)).expect("ReadAlias");
    alias.as_str().to_string()
}

#[test]
fn a_pam_token_at_startup_unlocks_without_a_prompt() {
    let Some(bus) = private_bus() else {
        eprintln!("pam token startup: no dbus-daemon, skipping");
        return;
    };
    let conn = bus.connect();

    let data_dir = temp_dir("data");
    write_password_vault(&data_dir.join("aegis/secrets"));
    let runtime_dir = temp_dir("runtime");
    write_pam_token(&runtime_dir, PASSWORD);

    // No fake IPC: this path must never prompt.
    let _daemon = KillOnDrop(spawn_daemon(&bus, &data_dir, &runtime_dir));
    wait_for_name(&conn, SERVICE);

    assert_eq!(
        default_alias(&conn),
        "/org/freedesktop/secrets/collection/login",
        "the token must unlock the vault at startup"
    );
    assert!(
        !runtime_dir.join("aegis-pam-token").exists(),
        "the token is consumed and deleted"
    );

    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&runtime_dir);
}

#[test]
fn a_pam_token_arriving_mid_run_unlocks_via_the_watcher() {
    let Some(bus) = private_bus() else {
        eprintln!("pam token watcher: no dbus-daemon, skipping");
        return;
    };
    let conn = bus.connect();

    let data_dir = temp_dir("data");
    write_password_vault(&data_dir.join("aegis/secrets"));
    let runtime_dir = temp_dir("runtime");

    let _daemon = KillOnDrop(spawn_daemon(&bus, &data_dir, &runtime_dir));
    wait_for_name(&conn, SERVICE);
    assert_eq!(default_alias(&conn), "/", "the vault starts locked");

    // The login (or screen-unlock) token lands after startup.
    write_pam_token(&runtime_dir, PASSWORD);

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if default_alias(&conn) == "/org/freedesktop/secrets/collection/login" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the watcher did not unlock the vault within 10 s"
        );
        std::thread::sleep(std::time::Duration::from_millis(200));
    }

    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&runtime_dir);
}

// =====================================================================
// Locked-vault concurrency: CreateCollection and RetrieveSecret on a
// locked vault, plus concurrent Unlock completing from one prompt.
// These cover the unlock coordinator that replaced the single-flight
// `is_unlocking` flag.
// =====================================================================

/// The HKDF-SHA256 portal secret for the password-mode fixture, mirroring
/// `secret::portal::derive_portal_secret` over the Argon2id-derived key.
fn expected_portal_secret_password_mode() -> [u8; 32] {
    let salt = SaltString::from_b64(SALT_B64).expect("parse salt");
    let mut key = [0u8; 32];
    Argon2::default()
        .hash_password_into(PASSWORD.as_bytes(), salt.as_str().as_bytes(), &mut key)
        .expect("derive fixture key");
    let hk = Hkdf::<Sha256>::new(None, &key);
    let mut out = [0u8; 32];
    hk.expand(b"aegis.portal.Secret/v1", &mut out)
        .expect("expand");
    out
}

/// Run RetrieveSecret against a locked vault, returning `(response, bytes)`.
/// `None` means the private bus was unavailable and the caller must skip.
fn retrieve_secret_while_locked(answer: SecretPromptResult) -> Option<(u32, Vec<u8>)> {
    let bus = private_bus()?;
    let conn = bus.connect();

    let data_dir = temp_dir("data");
    write_password_vault(&data_dir.join("aegis/secrets"));
    let runtime_dir = temp_dir("runtime");

    let fake = Arc::new(FakeCompositor { answer, gate: None });
    let _server = Server::start(&runtime_dir.join("aegis.sock"), Arc::clone(&fake))
        .expect("bind fake compositor IPC");

    let _daemon = KillOnDrop(spawn_daemon(&bus, &data_dir, &runtime_dir));
    wait_for_name(&conn, PORTAL);

    let portal = Proxy::new(
        &conn,
        PORTAL,
        DESKTOP_PATH,
        "org.freedesktop.impl.portal.Secret",
    )
    .expect("portal proxy");
    let (read_end, write_end) = pipe_pair();
    let fd: Fd<'_> = Fd::from(write_end);
    let handle = ObjectPath::try_from("/org/freedesktop/portal/desktop/request/1/locked")
        .expect("request handle path");
    let (response, _): (
        u32,
        std::collections::HashMap<String, zbus::zvariant::OwnedValue>,
    ) = portal
        .call(
            "RetrieveSecret",
            &(
                handle,
                "dev.aegis.locked",
                fd,
                std::collections::HashMap::<String, Value<'_>>::new(),
            ),
        )
        .expect("RetrieveSecret call");
    let bytes = read_all_with_timeout(read_end, Duration::from_secs(5));

    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&runtime_dir);
    Some((response, bytes))
}

#[test]
fn retrieve_secret_while_locked_prompts_then_delivers() {
    let Some((response, bytes)) = retrieve_secret_while_locked(SecretPromptResult::Secret {
        value: PASSWORD.to_string(),
    }) else {
        eprintln!("retrieve secret locked: no dbus-daemon, skipping");
        return;
    };
    assert_eq!(
        response, 0,
        "RetrieveSecret must report success after unlock"
    );
    assert_eq!(
        bytes.as_slice(),
        &expected_portal_secret_password_mode(),
        "the pipe must deliver HKDF-SHA256(argon2(password), aegis.portal.Secret/v1)"
    );
}

#[test]
fn retrieve_secret_dismissed_reports_cancelled() {
    let Some((response, bytes)) = retrieve_secret_while_locked(SecretPromptResult::Cancelled)
    else {
        eprintln!("retrieve secret dismissed: no dbus-daemon, skipping");
        return;
    };
    assert_eq!(
        response, 1,
        "a dismissed unlock prompt must report cancelled"
    );
    assert!(
        bytes.is_empty(),
        "nothing must be written to the fd on dismissal"
    );
}

#[test]
fn create_collection_while_locked_prompts_and_creates() {
    let Some(bus) = private_bus() else {
        eprintln!("create collection locked: no dbus-daemon, skipping");
        return;
    };
    let conn = bus.connect();

    let data_dir = temp_dir("data");
    write_password_vault(&data_dir.join("aegis/secrets"));
    let runtime_dir = temp_dir("runtime");

    let fake = Arc::new(FakeCompositor {
        answer: SecretPromptResult::Secret {
            value: PASSWORD.to_string(),
        },
        gate: None,
    });
    let _server = Server::start(&runtime_dir.join("aegis.sock"), Arc::clone(&fake))
        .expect("bind fake compositor IPC");

    let _daemon = KillOnDrop(spawn_daemon(&bus, &data_dir, &runtime_dir));
    wait_for_name(&conn, SERVICE);

    let service = Proxy::new(
        &conn,
        SERVICE,
        SERVICE_PATH,
        "org.freedesktop.Secret.Service",
    )
    .expect("service proxy");
    assert_eq!(default_alias(&conn), "/", "the vault starts locked");

    // CreateCollection on a locked vault hands back an empty path and a
    // prompt in flight, exactly like Unlock; the collection is created once
    // the single prompt completes.
    let mut properties: std::collections::HashMap<&str, Value<'_>> =
        std::collections::HashMap::new();
    properties.insert(
        "org.freedesktop.Secret.Collection.Label",
        Value::from("Default"),
    );
    let (col, prompt): (OwnedObjectPath, OwnedObjectPath) = service
        .call("CreateCollection", &(properties, "default"))
        .expect("CreateCollection while locked");
    assert_eq!(col.as_str(), "/", "no collection path while locked");
    assert_ne!(prompt.as_str(), "/", "a prompt must be in flight");

    // After the prompt completes, the alias resolves to the new collection.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if default_alias(&conn) == "/org/freedesktop/secrets/collection/default" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the collection was not created within 10 s"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&runtime_dir);
}

#[test]
fn concurrent_unlocks_each_get_a_live_prompt_and_complete_from_one_interaction() {
    let Some(bus) = private_bus() else {
        eprintln!("concurrent unlock: no dbus-daemon, skipping");
        return;
    };
    let conn = bus.connect();

    let data_dir = temp_dir("data");
    write_password_vault(&data_dir.join("aegis/secrets"));
    let runtime_dir = temp_dir("runtime");

    // Gate the compositor answer so both Unlock calls register their prompt
    // objects and block before the single worker interaction completes them.
    let gate = Arc::new((Mutex::new(false), std::sync::Condvar::new()));
    let fake = Arc::new(FakeCompositor {
        answer: SecretPromptResult::Secret {
            value: PASSWORD.to_string(),
        },
        gate: Some(Arc::clone(&gate)),
    });
    let _server = Server::start(&runtime_dir.join("aegis.sock"), Arc::clone(&fake))
        .expect("bind fake compositor IPC");

    let _daemon = KillOnDrop(spawn_daemon(&bus, &data_dir, &runtime_dir));
    wait_for_name(&conn, SERVICE);

    let service = Proxy::new(
        &conn,
        SERVICE,
        SERVICE_PATH,
        "org.freedesktop.Secret.Service",
    )
    .expect("service proxy");

    let login =
        ObjectPath::try_from("/org/freedesktop/secrets/collection/login").expect("login path");
    let (_u1, p1): (Vec<OwnedObjectPath>, OwnedObjectPath) = service
        .call("Unlock", &(vec![login.clone()],))
        .expect("Unlock 1");
    let (_u2, p2): (Vec<OwnedObjectPath>, OwnedObjectPath) =
        service.call("Unlock", &(vec![login],)).expect("Unlock 2");
    assert_ne!(p1.as_str(), "/", "Unlock 1 gets a prompt");
    assert_ne!(p2.as_str(), "/", "Unlock 2 gets a prompt");
    assert_ne!(p1, p2, "each concurrent Unlock gets its own prompt object");

    // Release the single compositor interaction; the worker completes both.
    {
        let (lock, cvar) = &*gate;
        *lock.lock().unwrap() = true;
        cvar.notify_all();
    }

    // The vault unlocks once for both callers.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if default_alias(&conn) == "/org/freedesktop/secrets/collection/login" {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "the vault did not unlock within 10 s"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    // Both single-use prompt objects leave the bus once completed.
    for path in [&p1, &p2] {
        let prompt = Proxy::new(
            &conn,
            SERVICE,
            path.as_str(),
            "org.freedesktop.Secret.Prompt",
        )
        .expect("prompt proxy");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        loop {
            if prompt.call::<_, _, ()>("Prompt", &("",)).is_err() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "completed prompt {path} did not leave the bus"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    let _ = std::fs::remove_dir_all(&data_dir);
    let _ = std::fs::remove_dir_all(&runtime_dir);
}
