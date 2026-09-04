//! Persistent compositor-owned window placement state.
//!
//! Remembered positions are an implementation detail of window management:
//! no backend, renderer, shell, IPC client, or companion process consumes
//! this store. Keeping the JSON adapter here leaves `tessera-model` limited to
//! shared effect-free values and deterministic rules.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// Saved window state for an application, keyed by `app_id`.
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub(crate) struct SavedWindowState {
    /// Saved position in compositor logical coordinates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) position: Option<tessera_model::Point>,

    /// Saved size in compositor logical coordinates.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) size: Option<tessera_model::Size>,

    /// Saved 1-based workspace index.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) workspace: Option<u32>,

    /// Saved layout role.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) layout_role: Option<tessera_model::layout::LayoutRole>,

    /// Saved maximized state.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) maximized: Option<bool>,
}

/// Versioned on-disk store for compositor-owned window state.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub(crate) struct WindowStateStore {
    #[serde(default)]
    version: u32,

    /// Map of `app_id` to remembered state.
    #[serde(default)]
    pub(crate) entries: BTreeMap<String, SavedWindowState>,
}

impl Default for WindowStateStore {
    fn default() -> Self {
        Self {
            version: Self::CURRENT_VERSION,
            entries: BTreeMap::new(),
        }
    }
}

impl WindowStateStore {
    const CURRENT_VERSION: u32 = 2;
    const DEFAULT_MAX_ENTRIES: usize = 500;

    /// Resolve the compositor's default persistent-window-state path.
    pub(crate) fn default_path() -> PathBuf {
        if let Ok(path) = std::env::var("XDG_STATE_HOME")
            && !path.trim().is_empty()
        {
            return PathBuf::from(path).join("tessera").join("window_state.json");
        }
        if let Ok(home) = std::env::var("HOME")
            && !home.trim().is_empty()
        {
            return PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("tessera")
                .join("window_state.json");
        }
        PathBuf::from("window_state.json")
    }

    /// Load a valid current-version store, falling back to an empty store.
    pub(crate) fn load_from_path(path: &Path) -> Self {
        let Ok(content) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let store: Self = serde_json::from_str(&content).unwrap_or_default();
        if store.version == Self::CURRENT_VERSION {
            store
        } else {
            Self::default()
        }
    }

    /// Atomically replace the store after creating its parent directory.
    pub(crate) fn save_to_path(&self, path: &Path) -> Result<(), std::io::Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(std::io::Error::other)?;
        let temporary = path.with_extension("json.tmp");
        std::fs::write(&temporary, json)?;
        std::fs::rename(temporary, path)
    }

    pub(crate) fn get(&self, app_id: &str) -> Option<&SavedWindowState> {
        self.entries.get(app_id)
    }

    pub(crate) fn update(&mut self, app_id: String, state: SavedWindowState) {
        if app_id.is_empty() {
            return;
        }
        self.entries.insert(app_id, state);
        self.prune(Self::DEFAULT_MAX_ENTRIES);
    }

    fn prune(&mut self, max_entries: usize) {
        while self.entries.len() > max_entries {
            let Some(first_key) = self.entries.keys().next().cloned() else {
                break;
            };
            self.entries.remove(&first_key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn update_and_prune_preserve_bounded_state() {
        let mut store = WindowStateStore::default();
        store.update(
            "org.mozilla.firefox".into(),
            SavedWindowState {
                position: Some(tessera_model::Point { x: 100, y: 200 }),
                size: Some(tessera_model::Size { w: 1024, h: 768 }),
                workspace: Some(2),
                layout_role: Some(tessera_model::layout::LayoutRole::Floating),
                maximized: Some(false),
            },
        );

        let retrieved = store.get("org.mozilla.firefox").unwrap();
        assert_eq!(
            retrieved.position,
            Some(tessera_model::Point { x: 100, y: 200 })
        );
        assert_eq!(retrieved.size, Some(tessera_model::Size { w: 1024, h: 768 }));
        assert_eq!(retrieved.workspace, Some(2));
    }

    #[test]
    fn unsupported_versions_are_rejected() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("window_state.json");
        std::fs::write(
            &path,
            r#"{
                "entries": {
                    "first": { "workspace": 2 }
                }
            }"#,
        )
        .unwrap();

        let store = WindowStateStore::load_from_path(&path);
        assert!(store.entries.is_empty());
    }

    #[test]
    fn save_replaces_the_previous_file_without_leaving_a_temporary() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("window_state.json");
        let mut store = WindowStateStore::default();
        store.update("first".into(), SavedWindowState::default());
        store.save_to_path(&path).unwrap();
        store.update("second".into(), SavedWindowState::default());
        store.save_to_path(&path).unwrap();

        let loaded = WindowStateStore::load_from_path(&path);
        assert_eq!(loaded.entries.len(), 2);
        assert!(!path.with_extension("json.tmp").exists());
    }
}
