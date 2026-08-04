//! The shared application model.
//!
//! A parsed, launchable freedesktop.org desktop entry of `Type=Application`,
//! backend- and renderer-agnostic. Lives here (rather than in `aegis-desktop-entries`)
//! because the chrome in `aegis-shell` reads it to render a launcher without
//! pulling in the `.desktop` / icon-theme parsing dependency graph; `aegis-desktop-entries`
//! builds these and `aegis-launcher` consumes them.
//!
//! Locale-sensitive fields are resolved to a single value at parse time (in
//! `aegis-desktop-entries`) so the rest of the compositor never sees `Name[xx_YY]` suffixes.

use std::path::PathBuf;

/// Desktop-file id, icon name, and Wayland app id of System Settings.
pub const SETTINGS_DESKTOP_ID: &str = "io.github.ming2k.aegis.Settings.desktop";
pub const SETTINGS_ICON_NAME: &str = "io.github.ming2k.aegis.Settings";
pub const SETTINGS_APP_ID: &str = "io.github.ming2k.aegis.Settings";
pub const AGENT_WORKSPACES_ID: &str = "aegis-agent-workspaces";
/// Previous built-in id accepted only when resolving persisted Dock pins.
const AGENT_WORKSPACES_LEGACY_ID: &str = "aegis-interaction-manager";

/// A compositor-owned application that is part of the desktop itself rather
/// than an external process described by a desktop entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BuiltInApplication {
    /// Agent Workspaces lifecycle and Interaction Domain-authority presentation.
    AgentWorkspaces,
    /// Interactive screenshot region selector.
    ScreenshotSelector,
}

/// How activating an application entry is fulfilled.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ApplicationTarget {
    /// Expand `Exec` and spawn an external process through `aegis-launcher`.
    #[default]
    External,
    /// Ask the compositor to present one of its trusted, built-in apps.
    BuiltIn(BuiltInApplication),
}

/// One launchable application, parsed from a `.desktop` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
    /// Whether activation spawns an external process or opens a trusted
    /// compositor-owned application. XDG-discovered entries use `External`;
    /// the binary adds built-ins to the same catalog explicitly.
    pub target: ApplicationTarget,
    /// The desktop file id: the entry's filename relative to an
    /// `applications/` directory (e.g. `firefox.desktop`). Case-sensitive;
    /// used as the deduplication key during enumeration.
    pub id: String,
    /// Localized `Name`. Falls back to the id with its extension stripped when
    /// the entry omits `Name`.
    pub name: String,
    /// Localized `GenericName`, if present.
    pub generic_name: Option<String>,
    /// Localized `Comment`, if present.
    pub comment: Option<String>,
    /// Raw `Exec` value with field codes still in place. Callers spawn through
    /// `aegis_desktop_entries::expand_exec` / `aegis_desktop_entries::expand_exec_tokens` to strip them.
    pub exec: Option<String>,
    /// Raw `Icon` value as written (a theme name or an absolute path). The
    /// resolved filesystem path, if found, is in [`Entry::icon_path`].
    pub icon: Option<String>,
    /// Absolute path to the best-matching icon file, resolved via the icon
    /// theme chain at parse time. `None` when no icon was declared or none
    /// could be resolved.
    pub icon_path: Option<PathBuf>,
    /// `Categories`, split on `;`. Empty vec when absent.
    pub categories: Vec<String>,
    /// `Keywords`, split on `;`. Empty vec when absent.
    pub keywords: Vec<String>,
    /// `StartupWMClass` — the value clients set as their Wayland `app_id`
    /// once launched. The compositor uses it to match a running toplevel back
    /// to this entry.
    pub startup_wm_class: Option<String>,
    /// `TryExec` program name (unresolved). Used at parse time to filter out
    /// entries whose binary is absent from `PATH`; retained for diagnostics.
    pub try_exec: Option<String>,
    /// Whether the entry asked to run inside a terminal emulator.
    pub terminal: bool,
    /// `NoDisplay` flag. Enumerators filter these out of the launcher set; the
    /// field is retained for completeness.
    pub no_display: bool,
    /// `Path` working directory the app wants spawned in.
    pub path: Option<PathBuf>,
    /// `MimeType`s the entry registered, split on `;`.
    pub mime_types: Vec<String>,
}

impl Entry {
    /// A short, single-line summary suitable for a launcher row subtitle:
    /// generic name if present, else comment, else empty.
    pub fn summary(&self) -> &str {
        self.generic_name
            .as_deref()
            .or(self.comment.as_deref())
            .unwrap_or("")
    }

    /// The lowercased ids an entry might be matched by: its `StartupWMClass`,
    /// the desktop-file stem, and the declared icon name. These are the same
    /// keys the composition root's icon cache files icons under, so a dock
    /// tile can both find its icon and fold a running toplevel (matched by
    /// `app_id`) into itself.
    pub fn match_keys(&self) -> Vec<String> {
        let mut keys = Vec::new();
        let mut push = |s: &str| {
            let s = s.to_ascii_lowercase();
            if !s.is_empty() && !keys.contains(&s) {
                keys.push(s);
            }
        };
        if let Some(wm) = &self.startup_wm_class {
            push(wm);
        }
        push(self.id.strip_suffix(".desktop").unwrap_or(&self.id));
        if let Some(ic) = &self.icon {
            push(ic);
        }
        keys
    }

    /// Match the canonical id or a narrowly scoped compatibility identity
    /// accepted while reading persisted application references.
    pub fn matches_persistent_id(&self, candidate: &str) -> bool {
        self.id.eq_ignore_ascii_case(candidate)
            || (self.id == AGENT_WORKSPACES_ID
                && candidate.eq_ignore_ascii_case(AGENT_WORKSPACES_LEGACY_ID))
    }

    /// Construct the compositor-owned Agent Workspaces catalog entry.
    pub fn agent_workspaces(name: impl Into<String>, summary: impl Into<String>) -> Entry {
        Entry {
            target: ApplicationTarget::BuiltIn(BuiltInApplication::AgentWorkspaces),
            id: AGENT_WORKSPACES_ID.into(),
            name: name.into(),
            generic_name: Some(summary.into()),
            comment: Some(
                "Manage AI-controlled application interaction domains and authority".into(),
            ),
            icon: Some("preferences-system-symbolic".into()),
            categories: vec!["System".into()],
            keywords: vec![
                "ai".into(),
                "workspace".into(),
                "interaction".into(),
                "domain".into(),
                "agent".into(),
            ],
            ..Entry::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn settings_ids_share_one_canonical_stem() {
        assert_eq!(SETTINGS_DESKTOP_ID, format!("{SETTINGS_APP_ID}.desktop"));
        assert_eq!(SETTINGS_ICON_NAME, SETTINGS_APP_ID);
    }

    #[test]
    fn agent_workspaces_has_a_stable_builtin_identity() {
        let workspaces =
            Entry::agent_workspaces("Agent Workspaces", "Interaction domain management");
        assert_eq!(workspaces.id, AGENT_WORKSPACES_ID);
        assert_eq!(
            workspaces.target,
            ApplicationTarget::BuiltIn(BuiltInApplication::AgentWorkspaces)
        );
        assert!(workspaces.matches_persistent_id(AGENT_WORKSPACES_ID));
        assert!(workspaces.matches_persistent_id(AGENT_WORKSPACES_LEGACY_ID));
        assert!(!workspaces.matches_persistent_id("aegis-ai-workspaces"));
    }

    #[test]
    fn match_keys_cover_wm_class_stem_and_icon_lowercased() {
        let entry = Entry {
            id: "org.example.App.desktop".to_string(),
            icon: Some("App-Icon".to_string()),
            startup_wm_class: Some("APP".to_string()),
            ..Entry::default()
        };
        assert_eq!(
            entry.match_keys(),
            vec!["app", "org.example.app", "app-icon"]
        );
    }

    #[test]
    fn match_keys_dedup_repeated_ids_and_skip_empty() {
        let entry = Entry {
            id: "firefox.desktop".to_string(),
            icon: Some("Firefox".to_string()),
            startup_wm_class: Some("FIREFOX".to_string()),
            ..Entry::default()
        };
        assert_eq!(entry.match_keys(), vec!["firefox"]);
        let entry = Entry {
            id: "term.desktop".to_string(),
            startup_wm_class: Some(String::new()),
            ..Entry::default()
        };
        assert_eq!(entry.match_keys(), vec!["term"]);
    }
}
