//! Persistent window state model and storage.
//!
//! Stores remembered window positions, sizes, layout roles, and workspaces
//! keyed by `app_id` across compositor restarts.

use std::collections::BTreeMap;
#[cfg(feature = "serde")]
use std::path::Path;
use std::path::PathBuf;

/// Saved window state for an application (keyed by `app_id`).
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SavedWindowState {
    /// Saved position in compositor logical coordinates.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub position: Option<crate::Point>,

    /// Saved size in compositor logical coordinates.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub size: Option<crate::Size>,

    /// Saved 1-based workspace index.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub workspace: Option<u32>,

    /// Saved layout role (Floating vs Tiled).
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub layout_role: Option<crate::layout::LayoutRole>,

    /// Saved maximized state.
    #[cfg_attr(
        feature = "serde",
        serde(default, skip_serializing_if = "Option::is_none")
    )]
    pub maximized: Option<bool>,
}

/// On-disk persistent store for window states.
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowStateStore {
    #[cfg_attr(feature = "serde", serde(default))]
    version: u32,

    /// Map of `app_id` -> `SavedWindowState`.
    #[cfg_attr(feature = "serde", serde(default))]
    pub entries: BTreeMap<String, SavedWindowState>,
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
    const CURRENT_VERSION: u32 = 1;

    /// Maximum entries retained in the store to prevent unbounded growth.
    pub const DEFAULT_MAX_ENTRIES: usize = 500;

    /// Get the default path for persistent window state file:
    /// `$XDG_STATE_HOME/aegis/window_state.json` or `~/.local/state/aegis/window_state.json`.
    pub fn default_path() -> PathBuf {
        if let Ok(path) = std::env::var("XDG_STATE_HOME")
            && !path.trim().is_empty()
        {
            return PathBuf::from(path).join("aegis").join("window_state.json");
        }
        if let Ok(home) = std::env::var("HOME")
            && !home.trim().is_empty()
        {
            return PathBuf::from(home)
                .join(".local")
                .join("state")
                .join("aegis")
                .join("window_state.json");
        }
        PathBuf::from("window_state.json")
    }

    /// Load the store from a JSON file. Returns empty store if missing or invalid.
    #[cfg(feature = "serde")]
    pub fn load_from_path(path: &Path) -> Self {
        if !path.exists() {
            return Self::default();
        }
        let Ok(content) = std::fs::read_to_string(path) else {
            return Self::default();
        };
        let mut store: Self = serde_json::from_str(&content).unwrap_or_default();
        store.migrate();
        store
    }

    /// Save the store to a JSON file. Automatically creates parent directories.
    #[cfg(feature = "serde")]
    pub fn save_to_path(&self, path: &Path) -> Result<(), std::io::Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).map_err(std::io::Error::other)?;
        std::fs::write(path, json)
    }

    /// Retrieve remembered state for `app_id`.
    pub fn get(&self, app_id: &str) -> Option<&SavedWindowState> {
        self.entries.get(app_id)
    }

    /// Insert or update remembered state for `app_id`.
    pub fn update(&mut self, app_id: String, state: SavedWindowState) {
        if app_id.is_empty() {
            return;
        }
        self.entries.insert(app_id, state);
        self.prune(Self::DEFAULT_MAX_ENTRIES);
    }

    /// Prune oldest entries if the store exceeds `max_entries`.
    pub fn prune(&mut self, max_entries: usize) {
        while self.entries.len() > max_entries {
            if let Some(first_key) = self.entries.keys().next().cloned() {
                self.entries.remove(&first_key);
            } else {
                break;
            }
        }
    }

    #[cfg(feature = "serde")]
    fn migrate(&mut self) {
        if self.version == 0 {
            for state in self.entries.values_mut() {
                state.workspace = state
                    .workspace
                    .map(|workspace| workspace.saturating_sub(1).max(1));
            }
        }
        self.version = Self::CURRENT_VERSION;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_store_update_and_prune() {
        let mut store = WindowStateStore::default();
        store.update(
            "org.mozilla.firefox".into(),
            SavedWindowState {
                position: Some(crate::Point { x: 100, y: 200 }),
                size: Some(crate::Size { w: 1024, h: 768 }),
                workspace: Some(2),
                layout_role: Some(crate::layout::LayoutRole::Floating),
                maximized: Some(false),
            },
        );

        let retrieved = store.get("org.mozilla.firefox").unwrap();
        assert_eq!(retrieved.position, Some(crate::Point { x: 100, y: 200 }));
        assert_eq!(retrieved.size, Some(crate::Size { w: 1024, h: 768 }));
        assert_eq!(retrieved.workspace, Some(2));
    }

    #[test]
    #[cfg(feature = "serde")]
    fn legacy_workspace_ids_are_migrated_to_one_based_positions() {
        let dir = std::env::temp_dir().join(format!(
            "aegis-window-state-migration-{}",
            std::process::id()
        ));
        let path = dir.join("window_state.json");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            &path,
            r#"{
                "entries": {
                    "first": { "workspace": 2 },
                    "second": { "workspace": 3 }
                }
            }"#,
        )
        .unwrap();

        let store = WindowStateStore::load_from_path(&path);
        assert_eq!(store.get("first").unwrap().workspace, Some(1));
        assert_eq!(store.get("second").unwrap().workspace, Some(2));

        let _ = std::fs::remove_dir_all(dir);
    }
}
