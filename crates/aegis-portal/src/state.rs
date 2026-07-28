//! Portal-owned persistent state (ADR-0053).
//!
//! aegis has no PermissionStore, so the backend persists its own authorization
//! decisions — Background grants and ScreenCast restore tokens — as JSON
//! documents under `$XDG_DATA_HOME/aegis-portal` (falling back to
//! `$HOME/.local/share/aegis-portal`). Writes are mode `0600` via
//! create-temp-then-rename, the same discipline as the capture cache
//! (`files.rs`): a reader never observes a partial document.

use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

/// The portal state directory: `$XDG_DATA_HOME/aegis-portal`, else
/// `$HOME/.local/share/aegis-portal`, else `None` (the feature that needs the
/// state directory degrades: decisions are not persisted across restarts).
pub(crate) fn state_dir() -> Option<PathBuf> {
    state_dir_from(std::env::var_os("XDG_DATA_HOME"), std::env::var_os("HOME"))
}

/// Split out for tests: environment variables are process-global.
fn state_dir_from(
    data: Option<std::ffi::OsString>,
    home: Option<std::ffi::OsString>,
) -> Option<PathBuf> {
    let base = data
        .filter(|dir| !dir.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            home.filter(|dir| !dir.is_empty())
                .map(|home| PathBuf::from(home).join(".local").join("share"))
        })?;
    Some(base.join("aegis-portal"))
}

/// Read and parse a JSON state document. Missing and corrupt files both
/// yield `None`: a missing file is the first-run case, and a corrupt one is
/// logged and treated as empty rather than failing the request (the worst
/// outcome is an application being asked again).
pub(crate) fn read_json<T: serde::de::DeserializeOwned>(dir: &Path, name: &str) -> Option<T> {
    let path = dir.join(name);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
        Err(error) => {
            log::warn!("portal: cannot read state {}: {error}", path.display());
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(value) => Some(value),
        Err(error) => {
            log::warn!("portal: ignoring corrupt state {}: {error}", path.display());
            None
        }
    }
}

/// Serialize `value` and write it as `name` under `dir`, atomically, mode
/// `0600` — state documents carry authorization decisions and unguessable
/// tokens.
pub(crate) fn write_json(dir: &Path, name: &str, value: &impl serde::Serialize) -> io::Result<()> {
    let bytes = serde_json::to_vec_pretty(value)
        .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    write_atomic(dir, name, &bytes, 0o600)
}

/// Write `bytes` as `dir/name`, atomically, with the given mode. `name`
/// must be a fixed file name, never caller input.
pub(crate) fn write_atomic(dir: &Path, name: &str, bytes: &[u8], mode: u32) -> io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let final_path = dir.join(name);
    let temporary = dir.join(format!(".{name}.{}.tmp", std::process::id()));
    let result = (|| {
        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(mode)
            .open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, &final_path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    result
}

/// An unguessable token: 16 bytes from `/dev/urandom` as lowercase hex.
/// Used for ScreenCast restore tokens (same source the compositor's Realm
/// portals use).
pub(crate) fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    let mut token = String::with_capacity(32);
    for byte in bytes {
        token.push_str(&format!("{byte:02x}"));
    }
    Ok(token)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch() -> PathBuf {
        std::env::temp_dir().join(format!(
            "aegis-portal-state-test-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn state_dir_prefers_data_home_and_falls_back_to_home() {
        let data = std::ffi::OsString::from("/data");
        let home = std::ffi::OsString::from("/home/u");
        assert_eq!(
            state_dir_from(Some(data.clone()), Some(home.clone())),
            Some(PathBuf::from("/data/aegis-portal"))
        );
        assert_eq!(
            state_dir_from(None, Some(home)),
            Some(PathBuf::from("/home/u/.local/share/aegis-portal"))
        );
        assert_eq!(
            state_dir_from(Some("".into()), None),
            None,
            "an empty XDG_DATA_HOME without HOME yields no state directory"
        );
        assert_eq!(state_dir_from(None, None), None);
    }

    #[test]
    fn json_state_round_trips_and_corruption_reads_as_empty() {
        let dir = scratch();
        let value = std::collections::HashMap::from([("app".to_string(), true)]);
        write_json(&dir, "doc.json", &value).unwrap();
        let back: Option<std::collections::HashMap<String, bool>> = read_json(&dir, "doc.json");
        assert_eq!(back, Some(value));
        // Mode 0600: tokens are authorization credentials.
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(dir.join("doc.json"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        // A corrupt document reads as empty, and no temp file lingers.
        std::fs::write(dir.join("doc.json"), b"{not json").unwrap();
        let corrupt: Option<std::collections::HashMap<String, bool>> = read_json(&dir, "doc.json");
        assert_eq!(corrupt, None);
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 1);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn random_tokens_are_hex_and_unique() {
        let a = random_token().unwrap();
        let b = random_token().unwrap();
        assert_eq!(a.len(), 32);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b);
    }
}
