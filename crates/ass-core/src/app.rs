//! The shared application model.
//!
//! A parsed, launchable freedesktop.org desktop entry of `Type=Application`,
//! backend- and renderer-agnostic. Lives here (rather than in `ass-apps`)
//! because the chrome in `ass-shell` reads it to render a launcher without
//! pulling in the `.desktop` / icon-theme parsing dependency graph; `ass-apps`
//! builds these and `ass-launch` consumes them.
//!
//! Locale-sensitive fields are resolved to a single value at parse time (in
//! `ass-apps`) so the rest of the compositor never sees `Name[xx_YY]` suffixes.

use std::path::PathBuf;

/// One launchable application, parsed from a `.desktop` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Entry {
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
    /// `ass_apps::expand_exec` / `ass_apps::expand_exec_tokens` to strip them.
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
}
