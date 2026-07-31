//! Secret storage for the aegis portal: an at-rest encrypted vault, the
//! native `org.freedesktop.impl.portal.Secret` backend interface
//! (`portal`), and a transitional `org.freedesktop.secrets` compatibility
//! shim (`compat`).
//!
//! All secret state lives behind ONE `std::sync::Mutex<SecretState>` shared
//! by every served interface. Secret-service traffic is rare, so a single
//! lock beats wssp's per-field lock soup. Interface methods lock, mutate,
//! and drop the guard before any `.await` — the guard must never cross an
//! await point (zbus dispatches methods on its async executor). Persisting
//! (`sync_to_vault`) happens inside the same critical section as the
//! mutation it belongs to.

pub(crate) mod compat;
pub(crate) mod portal;
pub(crate) mod vault;

use std::collections::HashMap;
use std::io::Write;
use std::os::fd::OwnedFd;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use aes::Aes128;
use cbc::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use cbc::{Decryptor, Encryptor};
use rand::RngCore;
use rand::rngs::OsRng;
use zbus::zvariant::OwnedObjectPath;
use zeroize::Zeroize;

use vault::{CollectionData, ItemData, Vault, VaultData};

/// Errors that keep secret support from coming up at all.
#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    /// No `$XDG_DATA_HOME` (or fallback), so the vault directory cannot be
    /// located.
    #[error("no XDG data directory available")]
    NoDataDir,
    /// Filesystem failure around the vault directory or files.
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    /// Key derivation, encryption, or decryption failed.
    #[error("crypto error: {0}")]
    Crypto(String),
    /// The vault file is truncated or otherwise malformed.
    #[error("vault error: {0}")]
    Vault(String),
    /// (De)serialization of the vault contents failed.
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
}

/// Every piece of mutable secret state, guarded by one mutex.
pub struct SecretState {
    /// The open vault; `None` means locked.
    pub(crate) vault: Option<Vault>,
    pub(crate) collections: HashMap<String, CollectionRt>,
    /// Compat session transport keys (AES/DH), keyed by session object path.
    /// Only the compat layer reads or writes this.
    pub(crate) sessions: HashMap<OwnedObjectPath, SessionCrypto>,
    /// Callers queued behind a vault unlock (see `enqueue_unlock_request`).
    pub(crate) pending_unlocks: Vec<PendingUnlock>,
    /// Whether the unlock worker thread is alive.
    pub(crate) unlock_worker_active: bool,
    pub(crate) vault_path: PathBuf,
    pub(crate) salt_path: PathBuf,
    pub(crate) key_path: PathBuf,
}

/// Runtime state of one collection.
pub(crate) struct CollectionRt {
    pub(crate) label: String,
    pub(crate) deleted: bool,
    pub(crate) items: HashMap<String, ItemRt>,
}

/// Runtime state of one item.
pub(crate) struct ItemRt {
    pub(crate) label: String,
    pub(crate) attributes: HashMap<String, String>,
    pub(crate) secret: Vec<u8>,
    pub(crate) deleted: bool,
}

/// Compat session transport crypto: plaintext, or AES-128-CBC with the
/// DH-derived session key (RFC 2409 group 2 + HKDF-SHA256; the exchange
/// itself lives in `compat::session`). CPU-only, so it runs under the state
/// lock.
#[derive(Clone)]
pub(crate) enum SessionCrypto {
    Plain,
    Dh(Vec<u8>),
}

impl Drop for SessionCrypto {
    fn drop(&mut self) {
        if let SessionCrypto::Dh(key) = self {
            key.zeroize();
        }
    }
}

impl SessionCrypto {
    /// Encrypt a secret for transport; returns `(parameters, value)` where
    /// parameters carry the CBC IV (empty for plaintext sessions).
    pub(crate) fn encrypt(&self, secret: &[u8]) -> Result<(Vec<u8>, Vec<u8>), SecretError> {
        match self {
            SessionCrypto::Plain => Ok((vec![], secret.to_vec())),
            SessionCrypto::Dh(key) => {
                let mut iv = [0u8; 16];
                OsRng.fill_bytes(&mut iv);
                let encryptor = Encryptor::<Aes128>::new(key.as_slice().into(), &iv.into());
                let mut buf = vec![0u8; secret.len() + 16];
                buf[..secret.len()].copy_from_slice(secret);
                let ct_len = encryptor
                    .encrypt_padded_mut::<Pkcs7>(&mut buf, secret.len())
                    .map_err(|e| SecretError::Crypto(format!("AES encryption failed: {e}")))?
                    .len();
                buf.truncate(ct_len);
                Ok((iv.to_vec(), buf))
            }
        }
    }

    /// Decrypt a transported secret; `iv` is the parameters field produced
    /// by `encrypt`.
    pub(crate) fn decrypt(&self, iv: &[u8], ciphertext: &[u8]) -> Result<Vec<u8>, SecretError> {
        match self {
            SessionCrypto::Plain => Ok(ciphertext.to_vec()),
            SessionCrypto::Dh(key) => {
                if iv.len() != 16 {
                    return Err(SecretError::Crypto(format!(
                        "invalid IV length: {}",
                        iv.len()
                    )));
                }
                let mut iv_arr = [0u8; 16];
                iv_arr.copy_from_slice(iv);

                let decryptor = Decryptor::<Aes128>::new(key.as_slice().into(), &iv_arr.into());
                let mut buf = ciphertext.to_vec();

                match decryptor.decrypt_padded_mut::<Pkcs7>(&mut buf) {
                    Ok(pt) => {
                        let result = pt.to_vec();
                        buf.zeroize();
                        Ok(result)
                    }
                    Err(e) => {
                        buf.zeroize();
                        log::error!("portal: secret AES decryption/unpad failed: {e}");
                        Err(SecretError::Crypto(format!("decryption failed: {e}")))
                    }
                }
            }
        }
    }
}

impl SecretState {
    /// Whether the vault is unlocked (a vault handle exists).
    pub(crate) fn is_unlocked(&self) -> bool {
        self.vault.is_some()
    }

    /// Serialize every non-deleted collection/item and persist the vault.
    /// Callers invoke this inside the same critical section as the mutation
    /// they want persisted.
    pub(crate) fn sync_to_vault(&self) {
        let Some(vault) = &self.vault else {
            log::warn!("portal: secret sync_to_vault called while locked; ignoring");
            return;
        };
        let collections = self
            .collections
            .iter()
            .filter(|(_, col)| !col.deleted)
            .map(|(col_id, col)| CollectionData {
                id: col_id.clone(),
                label: col.label.clone(),
                items: col
                    .items
                    .iter()
                    .filter(|(_, item)| !item.deleted)
                    .map(|(item_id, item)| ItemData {
                        id: item_id.clone(),
                        label: item.label.clone(),
                        attributes: item.attributes.clone(),
                        secret: item.secret.clone(),
                    })
                    .collect(),
            })
            .collect();
        match vault.save(&VaultData { collections }) {
            Ok(()) => log::info!("portal: secret vault synced to disk"),
            Err(error) => log::error!("portal: could not sync secret vault to disk: {error}"),
        }
    }

    /// Load decrypted vault data into the runtime collections map. Clones
    /// out of `VaultData` because its zeroize-on-drop forbids moves.
    fn populate(&mut self, data: &VaultData) {
        self.collections = data
            .collections
            .iter()
            .map(|col| {
                let items = col
                    .items
                    .iter()
                    .map(|item| {
                        (
                            item.id.clone(),
                            ItemRt {
                                label: item.label.clone(),
                                attributes: item.attributes.clone(),
                                secret: item.secret.clone(),
                                deleted: false,
                            },
                        )
                    })
                    .collect();
                (
                    col.id.clone(),
                    CollectionRt {
                        label: col.label.clone(),
                        deleted: false,
                        items,
                    },
                )
            })
            .collect();
    }

    /// Unlock the vault with a password: derive the key from the on-disk
    /// salt, load, populate, and ensure the `login` collection. Shared by
    /// the PAM-token path (startup and watcher) and the compositor-chrome
    /// password prompt. The caller zeroizes its copy of the password.
    pub(crate) fn unlock_with_password(&mut self, password: &str) -> Result<(), SecretError> {
        let salt = std::fs::read_to_string(&self.salt_path)?;
        let key = Vault::derive_key(password, salt.trim())?;
        let vault = Vault::new(self.vault_path.clone(), key);
        let data = vault.load()?;
        self.populate(&data);
        self.vault = Some(vault);
        self.ensure_login_collection();
        Ok(())
    }

    /// Create the `login` collection (and persist it) when absent. Only
    /// meaningful while unlocked.
    fn ensure_login_collection(&mut self) {
        if self.collections.contains_key("login") {
            return;
        }
        self.collections.insert(
            "login".to_string(),
            CollectionRt {
                label: "Login".to_string(),
                deleted: false,
                items: HashMap::new(),
            },
        );
        self.sync_to_vault();
    }
}

// ---------------------------------------------------------------------
// Unlock coordinator
//
// A locked (password-mode) vault unlocks through the compositor's masked
// secret prompt (`PromptSecret` IPC). Every caller that needs the vault —
// compat `Unlock`/`CreateCollection` or the native portal `RetrieveSecret`
// — queues here; ONE worker thread runs the prompt and completes the whole
// batch, so any mix of callers costs the user exactly one interaction.
// ---------------------------------------------------------------------

/// The result reported to a portal `RetrieveSecret` call that had to wait
/// for a vault unlock.
pub(crate) enum PortalUnlockOutcome {
    /// The vault unlocked and the derived secret was written to the fd.
    Delivered,
    /// The user dismissed the prompt (or the vault never unlocked).
    Dismissed,
    /// The secret could not be derived or delivered.
    Failed,
}

/// What a queued compat caller wants done once the vault unlocks.
pub(crate) enum CompatUnlockKind {
    /// `Service.Unlock`: complete with the requested object list.
    Unlock(Vec<OwnedObjectPath>),
    /// `Service.CreateCollection`: create the collection, complete with its
    /// object path.
    CreateCollection { alias: String, label: String },
}

/// One caller waiting on the vault unlock.
pub(crate) enum PendingUnlock {
    /// TRANSITIONAL compat request with a bus prompt object to complete.
    Compat {
        prompt_path: OwnedObjectPath,
        kind: CompatUnlockKind,
    },
    /// Native portal request: deliver the secret over the caller's fd.
    PortalRetrieve {
        fd: OwnedFd,
        outcome: async_channel::Sender<PortalUnlockOutcome>,
    },
}

/// Write a secret to a caller-supplied fd; EOF comes from closing the write
/// end. The portal frontend forwards whatever fd the app passed — a pipe
/// (Chrome, libportal) or a socket — and plain write+close works for both,
/// unlike `shutdown(2)`, which fails ENOTSOCK on anything but a socket.
pub(crate) fn write_secret_fd(fd: OwnedFd, secret: &[u8]) -> std::io::Result<()> {
    std::fs::File::from(fd).write_all(secret)
}

/// Queue a caller behind the vault unlock, spawning the single unlock
/// worker when none is running.
pub(crate) fn enqueue_unlock_request(
    state: &Arc<Mutex<SecretState>>,
    conn: &zbus::blocking::Connection,
    socket: &Path,
    request: PendingUnlock,
) {
    let spawn = {
        let mut state = state.lock().unwrap();
        state.pending_unlocks.push(request);
        if state.unlock_worker_active {
            false
        } else {
            state.unlock_worker_active = true;
            true
        }
    };
    if !spawn {
        return;
    }
    let worker_conn = conn.clone();
    let worker_state = Arc::clone(state);
    let worker_socket = socket.to_path_buf();
    let spawned = std::thread::Builder::new()
        .name("aegis-portal-unlock".to_string())
        .spawn(move || unlock_worker(worker_conn, worker_state, worker_socket));
    if let Err(error) = spawned {
        log::error!("portal: could not spawn the unlock worker: {error}");
        // Fail the queued requests immediately so no caller hangs.
        let requests = {
            let mut state = state.lock().unwrap();
            state.unlock_worker_active = false;
            std::mem::take(&mut state.pending_unlocks)
        };
        complete_unlock_requests(conn, state, requests, false);
    }
}

/// The single unlock worker: prompt only while the vault is still locked,
/// then complete every queued caller. Callers that queued during a
/// successful prompt complete against the now-unlocked vault without a
/// second interaction.
fn unlock_worker(conn: zbus::blocking::Connection, state: Arc<Mutex<SecretState>>, socket: PathBuf) {
    loop {
        let requests = {
            let mut state = state.lock().unwrap();
            let requests = std::mem::take(&mut state.pending_unlocks);
            if requests.is_empty() {
                state.unlock_worker_active = false;
            }
            requests
        };
        if requests.is_empty() {
            return;
        }

        let unlocked = if state.lock().unwrap().is_unlocked() {
            true
        } else {
            match prompt_and_unlock(&state, &socket) {
                Ok(()) => {
                    log::info!("portal: secret vault unlocked via password prompt");
                    true
                }
                Err(reason) => {
                    log::warn!("portal: vault unlock did not complete: {reason}");
                    false
                }
            }
        };
        if unlocked {
            // TRANSITIONAL (compat): the freshly unlocked collections and
            // items go on the bus. Removal: delete this call with `compat/`.
            if let Err(error) = compat::register_collections(&conn, &state) {
                log::error!("portal: could not register collections after unlock: {error}");
            }
        }
        complete_unlock_requests(&conn, &state, requests, unlocked);
    }
}

/// Ask the user for the vault password through compositor chrome and unlock
/// with it. Blocks on the IPC round-trip; the state lock is only taken for
/// the key derivation, never across the IPC.
fn prompt_and_unlock(state: &Arc<Mutex<SecretState>>, socket: &Path) -> Result<(), String> {
    let mut capture = crate::ipc::PortalCapture::new(socket.to_path_buf());
    let mut password = match capture.prompt_secret(
        "Unlock Keyring".to_string(),
        Some("The secret vault is locked. Enter its password to unlock it.".to_string()),
    ) {
        Ok(aegis_ipc::SecretPromptResult::Secret { value }) => value,
        Ok(aegis_ipc::SecretPromptResult::Cancelled) => return Err("prompt dismissed".into()),
        Err(error) => return Err(format!("secret prompt failed: {error}")),
    };
    let unlocked = {
        let mut state = state.lock().unwrap();
        state.unlock_with_password(&password)
    };
    password.zeroize();
    unlocked.map_err(|e| format!("wrong password or unreadable vault: {e}"))
}

/// Complete one drained batch against the unlock outcome.
fn complete_unlock_requests(
    conn: &zbus::blocking::Connection,
    state: &Arc<Mutex<SecretState>>,
    requests: Vec<PendingUnlock>,
    unlocked: bool,
) {
    for request in requests {
        match request {
            PendingUnlock::Compat { prompt_path, kind } => {
                // TRANSITIONAL: delete this call with `compat/`.
                compat::complete_request(conn, state, &prompt_path, kind, unlocked);
            }
            PendingUnlock::PortalRetrieve { fd, outcome } => {
                let result = if unlocked {
                    deliver_portal_secret(state, fd)
                } else {
                    PortalUnlockOutcome::Dismissed
                };
                if outcome.send_blocking(result).is_err() {
                    log::warn!("portal: RetrieveSecret caller went away before unlock completed");
                }
            }
        }
    }
}

/// Derive the portal secret and stream it into the caller's fd. Runs on the
/// unlock worker; the batch was already unlocked (or this is not called).
fn deliver_portal_secret(state: &Arc<Mutex<SecretState>>, fd: OwnedFd) -> PortalUnlockOutcome {
    let mut secret = {
        let state = state.lock().unwrap();
        match state.vault.as_ref() {
            Some(vault) => portal::derive_portal_secret(vault.get_master_key()),
            None => return PortalUnlockOutcome::Failed,
        }
    };
    let written = write_secret_fd(fd, &secret);
    secret.zeroize();
    match written {
        Ok(()) => PortalUnlockOutcome::Delivered,
        Err(error) => {
            log::warn!("portal: could not write the RetrieveSecret fd after unlock: {error}");
            PortalUnlockOutcome::Failed
        }
    }
}

/// The PAM token path: `$XDG_RUNTIME_DIR/aegis-pam-token`, falling back
/// to `/run/user/<uid>/` (mirrors `pam_aegis`'s writer).
pub(crate) fn pam_token_path() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR")
        && !dir.is_empty()
    {
        return Some(PathBuf::from(dir).join("aegis-pam-token"));
    }
    // SAFETY: getuid simply reads the process uid.
    let uid = unsafe { libc::getuid() };
    Some(PathBuf::from(format!("/run/user/{uid}/aegis-pam-token")))
}

/// Read and delete the PAM-cached login password, if a token exists and is
/// safely shaped (owned by this user, mode 0600). Deletion always follows a
/// read attempt so a stale or malformed token never loops.
pub(crate) fn consume_pam_token() -> Option<String> {
    let path = pam_token_path()?;
    let metadata = std::fs::metadata(&path).ok()?;
    // SAFETY: getuid simply reads the process uid.
    let uid = unsafe { libc::getuid() };
    if metadata.uid() != uid || metadata.permissions().mode() & 0o777 != 0o600 {
        log::warn!(
            "portal: refusing PAM token with unsafe ownership/mode at {}",
            path.display()
        );
        let _ = std::fs::remove_file(&path);
        return None;
    }
    let bytes = std::fs::read(&path).ok()?;
    let _ = std::fs::remove_file(&path);
    let mut password = String::from_utf8(bytes).ok()?;
    while password.ends_with('\n') || password.ends_with('\r') {
        password.pop();
    }
    if password.is_empty() {
        return None;
    }
    Some(password)
}

/// Initialize secret support: resolve the vault directory under
/// `$XDG_DATA_HOME/aegis/secrets` and open (or create) the vault.
pub fn init() -> Result<Arc<Mutex<SecretState>>, SecretError> {
    let dir = dirs::data_dir()
        .ok_or(SecretError::NoDataDir)?
        .join("aegis")
        .join("secrets");
    init_in(&dir)
}

/// Filesystem-level startup, split from `init` so tests can point it at a
/// temporary directory.
///
/// Three on-disk shapes are recognized:
///
/// - `vault.key` exists: keyfile mode. The vault opens immediately; a
///   missing `vault.enc` reads as an empty vault and is created on disk,
///   while a corrupt one is a hard error (never silently discard secrets).
/// - Only `vault.salt` exists: a password-mode vault (e.g. migrated from
///   wssp). A PAM-cached login password (`pam_aegis` token) unlocks
///   immediately; otherwise the process starts LOCKED and `Unlock` asks
///   the user through the compositor's masked secret prompt
///   (`PromptSecret` IPC).
/// - Neither exists: first run. A fresh keyfile (mode 0600) and an empty
///   vault are created.
pub(crate) fn init_in(dir: &Path) -> Result<Arc<Mutex<SecretState>>, SecretError> {
    std::fs::create_dir_all(dir)?;
    std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;

    let mut state = SecretState {
        vault: None,
        collections: HashMap::new(),
        sessions: HashMap::new(),
        pending_unlocks: Vec::new(),
        unlock_worker_active: false,
        vault_path: dir.join("vault.enc"),
        salt_path: dir.join("vault.salt"),
        key_path: dir.join("vault.key"),
    };

    if state.key_path.exists() {
        let hex = std::fs::read_to_string(&state.key_path)?;
        let key = Vault::key_from_hex(&hex)?;
        let vault = Vault::new(state.vault_path.clone(), key);
        let missing = !state.vault_path.exists();
        let data = vault.load()?;
        if missing {
            // A missing vault file reads as empty; persist it so the next
            // start sees a real file.
            vault.save(&VaultData {
                collections: vec![],
            })?;
        }
        state.populate(&data);
        state.vault = Some(vault);
        log::info!("portal: secret vault unlocked via keyfile");
    } else if state.salt_path.exists() {
        // A PAM-cached login password (pam_aegis) unlocks without
        // prompting; otherwise the vault waits for the compositor-chrome
        // prompt.
        match consume_pam_token() {
            Some(mut password) => {
                let mut locked_reason = None;
                match state.unlock_with_password(&password) {
                    Ok(()) => log::info!("portal: secret vault unlocked via PAM token"),
                    Err(error) => locked_reason = Some(format!("PAM-token unlock failed: {error}")),
                }
                password.zeroize();
                if let Some(reason) = locked_reason {
                    log::warn!("portal: {reason}; starting LOCKED");
                }
            }
            None => {
                log::info!(
                    "portal: password-protected secret vault found (vault.salt); starting \
                     LOCKED, Unlock will prompt through compositor chrome"
                );
            }
        }
    } else {
        // First run: keyfile mode, empty vault.
        let key = Vault::generate_key();
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(&state.key_path)?;
        file.write_all(Vault::key_to_hex(&key).as_bytes())?;
        drop(file);
        std::fs::set_permissions(&state.key_path, std::fs::Permissions::from_mode(0o600))?;
        let vault = Vault::new(state.vault_path.clone(), key);
        vault.save(&VaultData {
            collections: vec![],
        })?;
        state.vault = Some(vault);
        log::info!(
            "portal: secret vault initialized in keyfile mode at {}",
            dir.display()
        );
    }

    if state.is_unlocked() {
        state.ensure_login_collection();
    }
    Ok(Arc::new(Mutex::new(state)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let mut suffix = [0u8; 8];
        OsRng.fill_bytes(&mut suffix);
        let suffix: String = suffix.iter().map(|b| format!("{b:02x}")).collect();
        std::env::temp_dir().join(format!("aegis-secret-test-{tag}-{suffix}"))
    }

    #[test]
    fn first_run_creates_keyfile_and_empty_vault_then_reunlocks() {
        let dir = temp_dir("first-run");

        let state = init_in(&dir).expect("first-run init");
        assert!(state.lock().unwrap().is_unlocked());

        // Directory and keyfile permissions are hardened.
        let dir_mode = std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777;
        assert_eq!(dir_mode, 0o700);
        let key_mode = std::fs::metadata(dir.join("vault.key"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(key_mode, 0o600);
        assert!(dir.join("vault.enc").exists());

        // The login collection is created and persisted.
        assert!(state.lock().unwrap().collections.contains_key("login"));

        // A second init on the same directory unlocks and sees login.
        let state = init_in(&dir).expect("second init");
        let state = state.lock().unwrap();
        assert!(state.is_unlocked());
        assert!(state.collections.contains_key("login"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn salt_only_vault_stays_locked() {
        let dir = temp_dir("salt-only");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("vault.salt"), "c29tZXNhbHQ").unwrap();

        let state = init_in(&dir).expect("salt-only init must not fail");
        assert!(!state.lock().unwrap().is_unlocked());

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_crypto_roundtrip() {
        let key = vec![0x42u8; 16];
        let crypto = SessionCrypto::Dh(key);
        let (iv, ciphertext) = crypto.encrypt(b"top secret").expect("encrypt");
        assert_eq!(iv.len(), 16);
        assert_ne!(ciphertext, b"top secret");
        let plaintext = crypto.decrypt(&iv, &ciphertext).expect("decrypt");
        assert_eq!(plaintext, b"top secret");

        let (params, value) = SessionCrypto::Plain.encrypt(b"plain").expect("encrypt");
        assert!(params.is_empty());
        assert_eq!(
            SessionCrypto::Plain.decrypt(&params, &value).unwrap(),
            b"plain"
        );
    }
}
