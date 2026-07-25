use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use aegis_core::realm::{
    HUMAN_REALM, RealmId, RealmKind, RealmSnapshot, RealmState, SeatCapabilities, VirtualOutput,
};
use aegis_ipc::{Client, RealmAction, RealmActionResult};
use serde::{Deserialize, Serialize};

use crate::bridge::BridgeConfig;

const STATE_SCHEMA: u32 = 1;

/// One bridge-owned Realm and its latest model revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ManagedRealm {
    pub id: RealmId,
    pub revision: u64,
}

/// Process-local lifecycle manager with crash-recovery metadata.
pub(crate) struct RealmSession {
    label: String,
    store: StateStore,
    managed: Option<RealmId>,
}

impl RealmSession {
    pub fn acquire(config: &BridgeConfig) -> Result<Self, RealmSessionError> {
        Ok(Self {
            label: config.realm_label.clone(),
            store: StateStore::acquire(&config.state_dir(), &config.scope)?,
            managed: None,
        })
    }

    /// Find a previously managed live Realm without creating authority.
    pub fn locate(
        &mut self,
        client: &mut Client,
    ) -> Result<(RealmSnapshot, Option<ManagedRealm>), RealmSessionError> {
        let snapshot = client.realms()?;

        if let Some(id) = self.managed
            && realm_is_managed(&snapshot, id, &self.label)
        {
            return Ok((
                snapshot.clone(),
                Some(ManagedRealm {
                    id,
                    revision: snapshot.revision,
                }),
            ));
        }
        self.managed = None;

        if let Some(record) = self.store.read()?
            && record.label == self.label
            && realm_is_managed(&snapshot, RealmId(record.realm), &self.label)
        {
            let id = RealmId(record.realm);
            self.managed = Some(id);
            return Ok((
                snapshot.clone(),
                Some(ManagedRealm {
                    id,
                    revision: snapshot.revision,
                }),
            ));
        }

        let candidates = snapshot
            .realms
            .iter()
            .filter(|realm| {
                realm.kind == RealmKind::Agent
                    && realm.label == self.label
                    && realm.state != RealmState::Revoked
            })
            .map(|realm| realm.id)
            .collect::<Vec<_>>();
        match candidates.as_slice() {
            [] => {
                self.store.clear()?;
                Ok((snapshot, None))
            }
            [id] => {
                self.managed = Some(*id);
                self.store.write(*id, &self.label)?;
                Ok((
                    snapshot.clone(),
                    Some(ManagedRealm {
                        id: *id,
                        revision: snapshot.revision,
                    }),
                ))
            }
            _ => Err(RealmSessionError::Ambiguous {
                label: self.label.clone(),
                realms: candidates.iter().map(|id| id.0).collect(),
            }),
        }
    }

    /// Reuse the bridge's live Realm or create it atomically on first use.
    pub fn ensure(&mut self, client: &mut Client) -> Result<ManagedRealm, RealmSessionError> {
        let (_, existing) = self.locate(client)?;
        if let Some(existing) = existing {
            return Ok(existing);
        }
        let result = client.realm_action(RealmAction::Create {
            label: self.label.clone(),
            capabilities: SeatCapabilities::POINTER_KEYBOARD,
            output: Some(VirtualOutput::DEFAULT_AGENT),
        })?;
        let RealmActionResult::Created { bundle } = result else {
            return Err(RealmSessionError::UnexpectedResponse);
        };
        self.managed = Some(bundle.realm);
        self.store.write(bundle.realm, &self.label)?;
        Ok(ManagedRealm {
            id: bundle.realm,
            revision: bundle.revision,
        })
    }

    /// Permanently revoke the managed Realm, returning all controlled groups
    /// to the human Realm in the same optimistic revision.
    pub fn revoke(&mut self, client: &mut Client) -> Result<bool, RealmSessionError> {
        let (_, managed) = self.locate(client)?;
        let Some(managed) = managed else {
            return Ok(false);
        };
        let result = client.realm_action(RealmAction::Revoke {
            realm: managed.id,
            fallback: HUMAN_REALM,
            expected_revision: Some(managed.revision),
        })?;
        let RealmActionResult::Revoked { .. } = result else {
            return Err(RealmSessionError::UnexpectedResponse);
        };
        self.managed = None;
        self.store.clear()?;
        Ok(true)
    }

    /// Atomically persist the latest directed capture for fuji clients that
    /// do not yet forward MCP image content into the model conversation.
    pub fn store_capture(&self, png: &[u8]) -> Result<PathBuf, RealmSessionError> {
        self.store.write_capture(png)
    }
}

fn realm_is_managed(snapshot: &RealmSnapshot, id: RealmId, label: &str) -> bool {
    snapshot.realms.iter().any(|realm| {
        realm.id == id
            && realm.kind == RealmKind::Agent
            && realm.label == label
            && realm.state != RealmState::Revoked
    })
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecoveryRecord {
    schema: u32,
    realm: u64,
    label: String,
}

struct StateStore {
    /// Holding this descriptor holds the non-blocking advisory lock for the
    /// bridge lifetime. It also prevents two fuji sessions sharing a scope
    /// and injecting input through the same Realm concurrently.
    _lock: File,
    state_path: PathBuf,
    temp_path: PathBuf,
    capture_path: PathBuf,
    capture_temp_path: PathBuf,
}

impl StateStore {
    fn acquire(dir: &Path, scope: &str) -> Result<Self, RealmSessionError> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        let key = scope_key(scope);
        let lock_path = dir.join(format!("{key}.lock"));
        let state_path = dir.join(format!("{key}.realm.json"));
        let temp_path = dir.join(format!("{key}.realm.json.tmp-{}", std::process::id()));
        let capture_path = dir.join(format!("{key}.capture.png"));
        let capture_temp_path = dir.join(format!("{key}.capture.png.tmp-{}", std::process::id()));
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(false)
            .mode(0o600)
            .open(&lock_path)?;
        lock.set_permissions(fs::Permissions::from_mode(0o600))?;
        // SAFETY: flock only observes the valid, owned file descriptor and
        // does not retain a pointer. The descriptor lives in StateStore.
        let rc = unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc != 0 {
            let error = io::Error::last_os_error();
            if error.kind() == io::ErrorKind::WouldBlock
                || error.raw_os_error() == Some(libc::EWOULDBLOCK)
            {
                return Err(RealmSessionError::AlreadyRunning(scope.to_string()));
            }
            return Err(error.into());
        }
        Ok(Self {
            _lock: lock,
            state_path,
            temp_path,
            capture_path,
            capture_temp_path,
        })
    }

    fn read(&self) -> Result<Option<RecoveryRecord>, RealmSessionError> {
        let bytes = match fs::read(&self.state_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let record: RecoveryRecord = serde_json::from_slice(&bytes)?;
        if record.schema != STATE_SCHEMA || record.realm == 0 {
            return Err(RealmSessionError::InvalidState);
        }
        Ok(Some(record))
    }

    fn write(&self, realm: RealmId, label: &str) -> Result<(), RealmSessionError> {
        let bytes = serde_json::to_vec(&RecoveryRecord {
            schema: STATE_SCHEMA,
            realm: realm.0,
            label: label.to_string(),
        })?;
        match fs::remove_file(&self.temp_path) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&self.temp_path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        fs::rename(&self.temp_path, &self.state_path)?;
        Ok(())
    }

    fn clear(&self) -> Result<(), RealmSessionError> {
        remove_if_exists(&self.state_path)?;
        remove_if_exists(&self.capture_path)?;
        Ok(())
    }

    fn write_capture(&self, png: &[u8]) -> Result<PathBuf, RealmSessionError> {
        remove_if_exists(&self.capture_temp_path)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&self.capture_temp_path)?;
        file.write_all(png)?;
        file.sync_all()?;
        fs::rename(&self.capture_temp_path, &self.capture_path)?;
        Ok(self.capture_path.clone())
    }
}

fn remove_if_exists(path: &Path) -> Result<(), io::Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn scope_key(scope: &str) -> String {
    let readable = scope
        .chars()
        .take(48)
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    // FNV-1a keeps lossy readable-name collisions from sharing a lock file.
    let hash = scope
        .as_bytes()
        .iter()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        });
    format!("{readable}-{hash:016x}")
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum RealmSessionError {
    #[error("another aegis-fuji bridge already owns scope {0:?}")]
    AlreadyRunning(String),
    #[error(
        "multiple live Agent Realms use label {label:?}: {realms:?}; choose a unique ASS_FUJI_REALM_LABEL or revoke the stale Realms"
    )]
    Ambiguous { label: String, realms: Vec<u64> },
    #[error("Realm recovery record is invalid or from an unsupported schema")]
    InvalidState,
    #[error("ASS returned an unexpected Realm action response")]
    UnexpectedResponse,
    #[error("Realm recovery I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("Realm recovery record could not be decoded: {0}")]
    Json(#[from] serde_json::Error),
}

#[cfg(test)]
mod tests {
    use std::os::unix::fs::PermissionsExt;
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;

    #[test]
    fn scope_key_is_readable_but_collision_resistant() {
        assert_ne!(scope_key("a/b"), scope_key("a_b"));
        assert!(scope_key("fuji").starts_with("fuji-"));
    }

    #[test]
    fn compatibility_capture_is_owner_only_atomic_and_clearable() {
        static COUNTER: AtomicU64 = AtomicU64::new(0);
        let serial = COUNTER.fetch_add(1, Ordering::Relaxed);
        let dir =
            std::env::temp_dir().join(format!("aegis-fuji-state-{}-{serial}", std::process::id()));
        let store = StateStore::acquire(&dir, "capture-test").expect("store");
        let path = store.write_capture(b"png").expect("capture");
        assert_eq!(fs::read(&path).expect("read"), b"png");
        assert_eq!(
            fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
            0o600
        );
        store.clear().expect("clear");
        assert!(!path.exists());
        drop(store);
        fs::remove_dir_all(dir).expect("remove temp state");
    }
}
