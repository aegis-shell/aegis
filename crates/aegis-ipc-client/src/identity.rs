//! A paired agent's durable identity (ADR-0088, ADR-0125): the
//! compositor-issued principal and credential this installation presents on
//! every connection.
//!
//! The identity file lives under a private data directory with owner-only
//! permissions and is replaced atomically. A missing or invalid file means
//! "not paired yet" — the next connection pairs again and a fresh credential
//! replaces the file.

use std::io::Write as _;
use std::path::PathBuf;

use crate::state::scope_key;

const IDENTITY_VERSION: u32 = 1;

/// Identity persistence or continuity failure. The message is safe to
/// surface to the user; it never contains credential material.
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct IdentityError(String);

impl From<String> for IdentityError {
    fn from(message: String) -> Self {
        Self(message)
    }
}

#[derive(Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct StoredIdentity {
    version: u32,
    principal: String,
    credential: String,
}

impl std::fmt::Debug for StoredIdentity {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("StoredIdentity")
            .field("version", &self.version)
            .field("principal", &self.principal)
            .field("credential", &"[REDACTED]")
            .finish()
    }
}

impl Drop for StoredIdentity {
    fn drop(&mut self) {
        use zeroize::Zeroize as _;
        self.credential.zeroize();
    }
}

/// A loaded pairing identity plus its persistence location.
pub struct IdentityStore {
    path: Option<PathBuf>,
    identity: std::sync::Mutex<Option<StoredIdentity>>,
}

impl IdentityStore {
    /// Load the identity for an agent-instance `key` from `dir`. `dir` of
    /// `None` keeps the identity session-only.
    pub fn load(dir: Option<PathBuf>, key: &str) -> Self {
        let Some(dir) = dir else {
            return Self {
                path: None,
                identity: std::sync::Mutex::new(None),
            };
        };
        let path = dir.join(format!("identity-{}.json", scope_key(key)));
        let identity = read_identity(&path).ok().flatten();
        Self {
            path: Some(path),
            identity: std::sync::Mutex::new(identity),
        }
    }

    /// The credential to present at the handshake, if paired.
    pub fn credential(&self) -> Option<String> {
        self.identity
            .lock()
            .expect("identity lock")
            .as_ref()
            .map(|identity| identity.credential.clone())
    }

    /// The authenticated principal bound by the compositor handshake.
    pub fn principal(&self) -> Option<String> {
        self.identity
            .lock()
            .expect("identity lock")
            .as_ref()
            .map(|identity| identity.principal.clone())
    }

    /// Record a newly issued credential. When durable identity was requested,
    /// persistence is part of establishing the identity: failing it aborts
    /// startup rather than creating authority that the next process cannot
    /// safely recover.
    pub fn store(&self, principal: &str, credential: &str) -> Result<(), IdentityError> {
        let identity = StoredIdentity {
            version: IDENTITY_VERSION,
            principal: principal.to_owned(),
            credential: credential.to_owned(),
        };
        let Some(path) = &self.path else {
            *self.identity.lock().expect("identity lock") = Some(identity);
            return Ok(());
        };
        persist(path, &identity).map_err(IdentityError)?;
        *self.identity.lock().expect("identity lock") = Some(identity);
        Ok(())
    }

    /// Confirm that an existing credential was recognized as the same
    /// principal. A mismatch fails closed; it indicates corrupted local
    /// state or a credential/registry continuity violation.
    pub fn confirm_principal(&self, principal: &str) -> Result<(), IdentityError> {
        let identity = self.identity.lock().expect("identity lock");
        let Some(identity) = identity.as_ref() else {
            return Err(IdentityError(
                "compositor recognized an identity that is absent locally".into(),
            ));
        };
        if identity.principal != principal {
            return Err(IdentityError(format!(
                "credential principal changed from {} to {principal}",
                identity.principal
            )));
        }
        Ok(())
    }
}

fn persist(path: &PathBuf, identity: &StoredIdentity) -> Result<(), String> {
    use std::os::unix::fs::{OpenOptionsExt as _, PermissionsExt as _};
    use zeroize::Zeroize as _;
    let parent = path
        .parent()
        .ok_or_else(|| format!("{} has no parent", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("create {}: {error}", parent.display()))?;
    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("protect {}: {error}", parent.display()))?;
    let tmp = path.with_extension(format!("tmp-{}", std::process::id()));
    match std::fs::remove_file(&tmp) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("remove stale {}: {error}", tmp.display())),
    }
    let mut bytes = serde_json::to_vec_pretty(identity).map_err(|error| error.to_string())?;
    let write_result = (|| {
        let mut handle = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&tmp)
            .map_err(|error| format!("create {}: {error}", tmp.display()))?;
        handle
            .write_all(&bytes)
            .map_err(|error| format!("write {}: {error}", tmp.display()))?;
        handle
            .sync_all()
            .map_err(|error| format!("sync {}: {error}", tmp.display()))
    })();
    bytes.zeroize();
    write_result?;
    std::fs::rename(&tmp, path).map_err(|error| format!("replace {}: {error}", path.display()))?;
    let parent_handle = std::fs::File::open(parent)
        .map_err(|error| format!("open {}: {error}", parent.display()))?;
    parent_handle
        .sync_all()
        .map_err(|error| format!("sync {}: {error}", parent.display()))
}

fn read_identity(path: &std::path::Path) -> Result<Option<StoredIdentity>, String> {
    use std::io::Read as _;
    use std::os::unix::fs::{MetadataExt as _, OpenOptionsExt as _, PermissionsExt as _};
    use zeroize::Zeroize as _;

    let mut options = std::fs::OpenOptions::new();
    options
        .read(true)
        .custom_flags(libc::O_CLOEXEC | libc::O_NOFOLLOW);
    let mut file = match options.open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("open {}: {error}", path.display())),
    };
    let metadata = file
        .metadata()
        .map_err(|error| format!("inspect {}: {error}", path.display()))?;
    if !metadata.file_type().is_file()
        || metadata.uid() != unsafe { libc::geteuid() }
        || metadata.permissions().mode() & 0o077 != 0
        || metadata.nlink() != 1
        || metadata.len() > 64 * 1024
    {
        return Err(format!("unsafe identity file {}", path.display()));
    }
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    if let Err(error) = file.read_to_end(&mut bytes) {
        bytes.zeroize();
        return Err(format!("read {}: {error}", path.display()));
    }
    let result = serde_json::from_slice::<StoredIdentity>(&bytes)
        .map_err(|error| format!("decode {}: {error}", path.display()))
        .and_then(|identity| {
            let valid = identity.version == IDENTITY_VERSION
                && aegis_ipc::ActorPrincipal::new(identity.principal.clone()).is_ok()
                && !identity.credential.is_empty()
                && identity.credential.len() <= 512
                && identity.credential.is_ascii()
                && !identity.credential.chars().any(char::is_control);
            valid
                .then_some(identity)
                .ok_or_else(|| format!("invalid identity file {}", path.display()))
        });
    bytes.zeroize();
    result.map(Some)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let n = COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        std::env::temp_dir().join(format!(
            "aegis-ipc-client-identity-{}-{n}",
            std::process::id()
        ))
    }

    #[test]
    fn store_then_load_round_trips() {
        let dir = scratch();
        let store = IdentityStore::load(Some(dir.clone()), "Codex");
        assert!(store.credential().is_none());
        store.store("prin_1", "cred_1").unwrap();
        assert_eq!(store.credential().as_deref(), Some("cred_1"));

        let reloaded = IdentityStore::load(Some(dir.clone()), "Codex");
        assert_eq!(reloaded.credential().as_deref(), Some("cred_1"));
        let mode = std::os::unix::fs::PermissionsExt::mode(
            &std::fs::metadata(dir.join(format!("identity-{}.json", scope_key("Codex"))))
                .unwrap()
                .permissions(),
        );
        assert_eq!(mode & 0o777, 0o600);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn corrupt_identity_means_unpaired() {
        let dir = scratch();
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("identity-Codex.json"), b"not json").unwrap();
        let store = IdentityStore::load(Some(dir.clone()), "Codex");
        assert!(store.credential().is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn labels_with_unsafe_characters_get_safe_keys() {
        let dir = scratch();
        let store = IdentityStore::load(Some(dir.clone()), "a/b");
        store.store("prin_1", "cred_1").unwrap();
        let reloaded = IdentityStore::load(Some(dir.clone()), "a/b");
        assert_eq!(reloaded.credential().as_deref(), Some("cred_1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn session_only_store_keeps_memory_identity() {
        let store = IdentityStore::load(None, "Codex");
        store.store("prin_1", "cred_1").unwrap();
        assert_eq!(store.credential().as_deref(), Some("cred_1"));
    }
}
