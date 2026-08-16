//! Per-instance agent state: the recovery lock, the managed Interaction
//! Domain record, and owner-only capture files (ADR-0125).
//!
//! One directory holds every artifact for one agent instance and
//! authenticated subject. Holding the lock file's `flock` for the process
//! lifetime prevents two processes sharing an instance and subject from
//! driving the same Interaction Domain; a crash releases the lock and the
//! recovery record lets the successor adopt the still-live Interaction
//! Domain.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
use std::path::{Path, PathBuf};

use aegis_model::interaction_domain::InteractionDomainId;
use serde::{Deserialize, Serialize};

const STATE_SCHEMA: u32 = 2;

/// Errors from state recovery and the managed Interaction Domain lifecycle.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    #[error("another agent process already owns scope {0:?}")]
    AlreadyRunning(String),
    #[error(
        "multiple live Agent Interaction Domains use label {label:?}: {interaction_domains:?}; choose a unique Interaction Domain label or revoke the stale Interaction Domains"
    )]
    Ambiguous {
        label: String,
        interaction_domains: Vec<u64>,
    },
    #[error("Interaction Domain recovery record is invalid or from an unsupported schema")]
    InvalidState,
    #[error("Aegis returned an unexpected Interaction Domain action response")]
    UnexpectedResponse,
    #[error("Interaction Domain recovery I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("Interaction Domain recovery record could not be decoded: {0}")]
    Json(#[from] serde_json::Error),
}

/// A filesystem-safe key for one agent instance scope: readable, short, and
/// collision-resistant so two scopes never share a lock or record file.
pub fn scope_key(scope: &str) -> String {
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

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecoveryRecord {
    schema: u32,
    pub(crate) interaction_domain: u64,
    pub(crate) label: String,
    #[serde(default)]
    pub(crate) subject: String,
}

pub(crate) struct StateStore {
    /// Holding this descriptor holds the non-blocking advisory lock for the
    /// process lifetime. It also prevents two processes sharing an instance
    /// and authenticated subject from driving the same Interaction Domain.
    _lock: File,
    state_path: PathBuf,
    temp_path: PathBuf,
    capture_path: PathBuf,
    capture_temp_path: PathBuf,
}

impl StateStore {
    pub(crate) fn acquire(dir: &Path, scope: &str) -> Result<Self, SessionError> {
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)?;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
        let key = scope_key(scope);
        let lock_path = dir.join(format!("{key}.lock"));
        let state_path = dir.join(format!("{key}.interaction_domain.json"));
        let temp_path = dir.join(format!(
            "{key}.interaction_domain.json.tmp-{}",
            std::process::id()
        ));
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
                return Err(SessionError::AlreadyRunning(scope.to_string()));
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

    pub(crate) fn read(&self) -> Result<Option<RecoveryRecord>, SessionError> {
        let bytes = match fs::read(&self.state_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let record: RecoveryRecord = serde_json::from_slice(&bytes)?;
        // Schema 1 keyed recovery only by a cosmetic label. It cannot prove
        // ownership and is deliberately ignored; the live snapshot may still
        // recover the Interaction Domain by its authenticated subject binding.
        if record.schema == 1 {
            return Ok(None);
        }
        if record.schema != STATE_SCHEMA
            || record.interaction_domain == 0
            || record.subject.is_empty()
        {
            return Err(SessionError::InvalidState);
        }
        Ok(Some(record))
    }

    pub(crate) fn write(
        &self,
        interaction_domain: InteractionDomainId,
        label: &str,
        subject: &str,
    ) -> Result<(), SessionError> {
        let bytes = serde_json::to_vec(&RecoveryRecord {
            schema: STATE_SCHEMA,
            interaction_domain: interaction_domain.0,
            label: label.to_string(),
            subject: subject.to_string(),
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
        if let Some(parent) = self.state_path.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    }

    pub(crate) fn clear(&self) -> Result<(), SessionError> {
        remove_if_exists(&self.state_path)?;
        remove_if_exists(&self.capture_path)?;
        let (window_capture, _) = self.named_capture_paths("window");
        remove_if_exists(&window_capture)?;
        if let Some(parent) = self.state_path.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(())
    }

    /// The sibling paths of one named capture kind. `name` is a fixed
    /// lowercase token ("window"): the legacy directed capture keeps
    /// `{key}.capture.png`, a named kind lands at `{key}.{name}-capture.png`.
    fn named_capture_paths(&self, name: &str) -> (PathBuf, PathBuf) {
        let base = self
            .capture_path
            .file_name()
            .and_then(|file| file.to_str())
            .and_then(|file| file.strip_suffix("capture.png"))
            .expect("capture path carries the .capture.png suffix");
        let path = self
            .capture_path
            .with_file_name(format!("{base}{name}-capture.png"));
        let temp = self.capture_temp_path.with_file_name(format!(
            "{base}{name}-capture.png.tmp-{}",
            std::process::id()
        ));
        (path, temp)
    }

    pub(crate) fn write_capture(&self, png: &[u8]) -> Result<PathBuf, SessionError> {
        Self::write_capture_at(&self.capture_path, &self.capture_temp_path, png)
    }

    pub(crate) fn write_capture_named(
        &self,
        name: &str,
        png: &[u8],
    ) -> Result<PathBuf, SessionError> {
        let (path, temp) = self.named_capture_paths(name);
        Self::write_capture_at(&path, &temp, png)
    }

    fn write_capture_at(
        path: &Path,
        temp_path: &Path,
        png: &[u8],
    ) -> Result<PathBuf, SessionError> {
        remove_if_exists(temp_path)?;
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(temp_path)?;
        file.write_all(png)?;
        file.sync_all()?;
        fs::rename(temp_path, path)?;
        if let Some(parent) = path.parent() {
            File::open(parent)?.sync_all()?;
        }
        Ok(path.to_path_buf())
    }
}

fn remove_if_exists(path: &Path) -> Result<(), io::Error> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
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
        let dir = std::env::temp_dir().join(format!(
            "aegis-ipc-client-state-{}-{serial}",
            std::process::id()
        ));
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
