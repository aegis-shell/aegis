//! Persistent compositor-owned dock state: pinned applications, autopopulate flag,
//! and screen edge anchor position.
//!
//! Stored under `$XDG_STATE_HOME/tessera/dock_state.json`.

use std::path::{Path, PathBuf};

use tessera_model::dock::DockPosition;

/// Persistent on-disk store for dock state (pinned applications, order, and edge).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DockStateStore {
    #[serde(default = "DockStateStore::default_version")]
    pub version: u32,

    /// List of pinned desktop entry IDs or stems, in display order.
    #[serde(default)]
    pub pinned: Vec<String>,

    /// Whether the dock should auto-populate from available applications
    /// when `pinned` is empty.
    #[serde(default)]
    pub autopopulate: bool,

    /// Screen edge the dock anchors to (Left, Bottom, Right).
    #[serde(default)]
    pub position: DockPosition,
}

impl Default for DockStateStore {
    fn default() -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            pinned: Vec::new(),
            autopopulate: false,
            position: DockPosition::default(),
        }
    }
}

impl DockStateStore {
    pub const CURRENT_VERSION: u32 = 1;

    const fn default_version() -> u32 {
        Self::CURRENT_VERSION
    }

    /// Resolve the compositor's default persistent dock-state path.
    ///
    /// Follows the XDG Base Directory specification:
    /// 1. `$XDG_STATE_HOME/tessera/dock_state.json`
    /// 2. `$HOME/.local/state/tessera/dock_state.json`
    /// 3. Fallback to `"dock_state.json"` in current directory.
    pub fn default_path() -> PathBuf {
        if let Ok(path) = std::env::var("XDG_STATE_HOME")
            && !path.trim().is_empty()
        {
            return PathBuf::from(path).join("tessera").join("dock_state.json");
        }
        if let Ok(home) = std::env::var("HOME")
            && !home.trim().is_empty()
        {
            return PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("tessera")
                .join("dock_state.json");
        }
        PathBuf::from("dock_state.json")
    }

    /// Load a valid current-version store from disk, or return `None` if absent or malformed.
    pub fn load_from_path(path: &Path) -> Option<Self> {
        let content = std::fs::read_to_string(path).ok()?;
        let store: Self = serde_json::from_str(&content).ok()?;
        if store.version == Self::CURRENT_VERSION {
            Some(store)
        } else {
            None
        }
    }

    /// Load the store from `path`, or initialize with seeds and immediately persist.
    pub fn load_or_init(
        path: &Path,
        seed_pinned: &[String],
        seed_autopopulate: bool,
        seed_position: DockPosition,
    ) -> Self {
        if let Some(store) = Self::load_from_path(path) {
            return store;
        }

        let store = Self {
            version: Self::CURRENT_VERSION,
            pinned: seed_pinned.to_vec(),
            autopopulate: seed_autopopulate,
            position: seed_position,
        };
        let _ = store.save_to_path(path);
        store
    }

    /// Atomically replace the store on disk via a temporary file and rename.
    pub fn save_to_path(&self, path: &Path) -> Result<(), std::io::Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, json)?;
        std::fs::rename(temporary, path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dock_state_round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dock_state.json");

        let store = DockStateStore {
            version: DockStateStore::CURRENT_VERSION,
            pinned: vec!["foot.desktop".into(), "firefox.desktop".into()],
            autopopulate: false,
            position: DockPosition::Left,
        };

        store.save_to_path(&path).unwrap();
        let loaded = DockStateStore::load_from_path(&path).unwrap();
        assert_eq!(loaded, store);
    }

    #[test]
    fn dock_state_load_or_init() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("dock_state.json");

        let seeds = vec!["seed1.desktop".into(), "seed2.desktop".into()];
        let store = DockStateStore::load_or_init(&path, &seeds, true, DockPosition::Right);
        assert_eq!(store.pinned, seeds);
        assert!(store.autopopulate);
        assert_eq!(store.position, DockPosition::Right);

        // Second load returns existing without re-seeding
        let loaded = DockStateStore::load_or_init(
            &path,
            &["other.desktop".into()],
            false,
            DockPosition::Bottom,
        );
        assert_eq!(loaded.pinned, seeds);
        assert!(loaded.autopopulate);
        assert_eq!(loaded.position, DockPosition::Right);
    }
}
