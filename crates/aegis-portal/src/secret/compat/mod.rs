//! Transitional `org.freedesktop.secrets` (Secret Service API) compatibility
//! layer.
//!
//! Everything in this directory exists only so un-sandboxed clients speaking
//! the classic Secret Service API (libsecret & co.) keep working until
//! portal-native secret retrieval is universal. It is scheduled for removal:
//! deleting it must only require dropping this module, its single
//! registration call site and name request in `lib.rs`, the
//! `org.freedesktop.secrets.service` D-Bus activation file, the
//! compat-only `sessions`/`SessionCrypto` members of `SecretState`, and the
//! two clearly marked call sites in the `secret` module's unlock worker
//! (`register_collections` and `complete_request`). The native
//! `org.freedesktop.impl.portal.Secret` interface in `super::portal` never
//! depends on anything here.

mod collection;
mod item;
mod prompt;
mod service;
mod session;

use std::sync::{Arc, Mutex};

use rand::RngCore;
use rand::rngs::OsRng;
use zbus::blocking::Connection;
use zbus::zvariant::{OwnedObjectPath, Value};
use zeroize::Zeroize;

use super::SecretState;
use collection::CollectionIface;
use item::ItemIface;
use service::ServiceIface;

/// The object path of the Secret Service API root.
pub(super) const SERVICE_PATH: &str = "/org/freedesktop/secrets";
/// The conventional alias path served for the `login` collection.
const ALIAS_DEFAULT_PATH: &str = "/org/freedesktop/secrets/aliases/default";

/// The shared IsLocked error: the Secret Service API reports it as a
/// generic failure carrying the API's error name.
pub(super) fn is_locked() -> zbus::fdo::Error {
    zbus::fdo::Error::Failed("org.freedesktop.Secret.Error.IsLocked".to_string())
}

/// The `/` object path used as the "no prompt" prompt handle.
pub(super) fn root_path() -> OwnedObjectPath {
    OwnedObjectPath::try_from("/").expect("'/' is a valid object path")
}

/// Random 16-hex-char id component for collection/item/session/prompt
/// object paths.
pub(super) fn generate_id() -> String {
    let mut bytes = [0u8; 8];
    OsRng.fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Register every compat interface on the shared portal connection: the
/// Service at the API root and, while unlocked, all known collections and
/// items plus the `aliases/default` path for the `login` collection.
/// `socket` is the compositor IPC socket the unlock prompt is served over.
pub(crate) fn serve(
    conn: &Connection,
    state: &Arc<Mutex<SecretState>>,
    socket: &std::path::Path,
) -> zbus::Result<()> {
    conn.object_server().at(
        SERVICE_PATH,
        ServiceIface {
            conn: conn.clone(),
            state: Arc::clone(state),
            socket: socket.to_path_buf(),
        },
    )?;

    register_collections(conn, state)
}

/// Register every unlocked collection and item (plus the `aliases/default`
/// path for `login`) on the bus. Shared by startup (`serve`), the password
/// prompt's unlock, and the PAM watcher — any path that unlocks the vault
/// after the Service object already exists.
pub(crate) fn register_collections(
    conn: &Connection,
    state: &Arc<Mutex<SecretState>>,
) -> zbus::Result<()> {
    // Snapshot the unlocked collections/items, then register outside the
    // critical section.
    let snapshot = {
        let state = state.lock().unwrap();
        if !state.is_unlocked() {
            return Ok(());
        }
        state
            .collections
            .iter()
            .filter(|(_, col)| !col.deleted)
            .map(|(col_id, col)| {
                (
                    col_id.clone(),
                    col.items
                        .iter()
                        .filter(|(_, item)| !item.deleted)
                        .map(|(item_id, _)| item_id.clone())
                        .collect::<Vec<_>>(),
                )
            })
            .collect::<Vec<_>>()
    };

    for (col_id, item_ids) in snapshot {
        conn.object_server().at(
            format!("{SERVICE_PATH}/collection/{col_id}"),
            CollectionIface {
                id: col_id.clone(),
                state: Arc::clone(state),
            },
        )?;
        for item_id in item_ids {
            conn.object_server().at(
                format!("{SERVICE_PATH}/collection/{col_id}/{item_id}"),
                ItemIface {
                    collection_id: col_id.clone(),
                    item_id,
                    state: Arc::clone(state),
                },
            )?;
        }
        if col_id == "login" {
            conn.object_server().at(
                ALIAS_DEFAULT_PATH,
                CollectionIface {
                    id: col_id,
                    state: Arc::clone(state),
                },
            )?;
        }
    }
    Ok(())
}

/// Register a fresh `Prompt` object and queue the request behind the shared
/// unlock worker; the method reply carries the prompt path. Every
/// concurrent caller gets its own live prompt object, and all of them
/// complete from the worker's single compositor interaction.
pub(crate) async fn queue_prompt(
    server: &zbus::ObjectServer,
    conn: &Connection,
    state: &Arc<Mutex<SecretState>>,
    socket: &std::path::Path,
    kind: super::CompatUnlockKind,
) -> zbus::fdo::Result<OwnedObjectPath> {
    let prompt_path =
        OwnedObjectPath::try_from(format!("{SERVICE_PATH}/prompt/p{}", generate_id()))
            .map_err(|e| zbus::fdo::Error::Failed(e.to_string()))?;
    server
        .at(
            prompt_path.clone(),
            prompt::PromptIface {
                id: prompt_path.as_str().to_string(),
            },
        )
        .await
        .map_err(|e| zbus::fdo::Error::Failed(format!("could not serve the prompt: {e}")))?;
    super::enqueue_unlock_request(
        state,
        conn,
        socket,
        super::PendingUnlock::Compat {
            prompt_path: prompt_path.clone(),
            kind,
        },
    );
    Ok(prompt_path)
}

/// Complete one queued compat request against the vault-unlock outcome and
/// drop its prompt object from the bus. Runs on the unlock worker thread
/// (the `secret` module's coordinator).
pub(crate) fn complete_request(
    conn: &Connection,
    state: &Arc<Mutex<SecretState>>,
    prompt_path: &OwnedObjectPath,
    kind: super::CompatUnlockKind,
    unlocked: bool,
) {
    match kind {
        super::CompatUnlockKind::Unlock(objects) => {
            // A dismissed prompt answers with an empty object list.
            let objects = if unlocked { objects } else { Vec::new() };
            let result = Value::from(zbus::zvariant::Array::from(objects));
            complete_prompt(conn, prompt_path, !unlocked, result);
        }
        super::CompatUnlockKind::CreateCollection { alias, label } => {
            let (dismissed, path) = if unlocked {
                match collection::create_collection_now(state, &alias, &label) {
                    Ok(path) => {
                        // The new collection reaches the bus through a
                        // registration pass.
                        if let Err(error) = register_collections(conn, state) {
                            log::error!(
                                "portal: could not register collection '{alias}' on the bus: {error}"
                            );
                        }
                        (false, path)
                    }
                    Err(error) => {
                        log::error!(
                            "portal: could not create collection '{alias}' after unlock: {error}"
                        );
                        (true, root_path())
                    }
                }
            } else {
                (true, root_path())
            };
            complete_prompt(conn, prompt_path, dismissed, Value::from(path));
        }
    }
}

/// Emit `Prompt.Completed` and unregister the single-use prompt object.
fn complete_prompt(
    conn: &Connection,
    prompt_path: &OwnedObjectPath,
    dismissed: bool,
    result: Value<'static>,
) {
    if let Err(error) = conn.emit_signal(
        None::<&str>,
        prompt_path.as_str(),
        "org.freedesktop.Secret.Prompt",
        "Completed",
        &(dismissed, result),
    ) {
        log::warn!("portal: could not emit secrets Prompt.Completed for {prompt_path}: {error}");
    }
    if let Err(error) = conn
        .object_server()
        .remove::<prompt::PromptIface, _>(prompt_path.clone())
    {
        log::warn!("portal: could not unregister secrets prompt {prompt_path}: {error}");
    }
}

/// How often the PAM watcher polls for a fresh login token.
const PAM_WATCH_INTERVAL: std::time::Duration = std::time::Duration::from_secs(2);

/// Spawn the PAM watcher thread: while a password-mode vault is locked, a
/// fresh `pam_aegis` token (login or screen-unlock re-authentication)
/// unlocks it without a prompt and registers the compat objects. Keyfile
/// vaults are never locked, so the watcher exits immediately.
pub(crate) fn spawn_pam_watcher(conn: Connection, state: Arc<Mutex<SecretState>>) {
    let has_password_vault = state.lock().unwrap().salt_path.exists();
    if !has_password_vault {
        return;
    }
    let spawned = std::thread::Builder::new()
        .name("aegis-portal-pam-watch".to_string())
        .spawn(move || {
            loop {
                std::thread::sleep(PAM_WATCH_INTERVAL);
                let locked = !state.lock().unwrap().is_unlocked();
                if !locked {
                    continue;
                }
                let Some(mut password) = super::consume_pam_token() else {
                    continue;
                };
                let unlocked = {
                    let mut state = state.lock().unwrap();
                    match state.unlock_with_password(&password) {
                        Ok(()) => true,
                        Err(error) => {
                            log::warn!("portal: PAM-token unlock failed: {error}");
                            false
                        }
                    }
                };
                password.zeroize();
                if unlocked {
                    log::info!("portal: secret vault unlocked via PAM token (watcher)");
                    if let Err(error) = register_collections(&conn, &state) {
                        log::error!(
                            "portal: could not register collections after PAM unlock: {error}"
                        );
                    }
                }
            }
        });
    if let Err(error) = spawned {
        log::warn!("portal: could not spawn the PAM watcher: {error}");
    }
}
