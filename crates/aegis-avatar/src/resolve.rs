//! XDG-conformant avatar source resolution.
//!
//! Reuses [`aegis_desktop_entries::xdg_data_dirs`] for the base-directory
//! spec rather than hand-rolling `$XDG_DATA_HOME` again. Two precedence
//! groups are searched:
//!
//! 1. **Still images** — the canonical Aegis location first, then the
//!    freedesktop `~/.face` convention that GNOME/SDDM/LightDM already write.
//! 2. **VRM models** — only the canonical Aegis location; a 3D avatar is an
//!    explicit Aegis configuration, not something other desktops write for us.
//!
//! `$XDG_DATA_HOME/aegis/avatars/` follows the canonical-namespace decision
//! (ADR-0066) and keeps user-chosen art out of the cache directory, which is
//! disposable and the wrong home for a deliberate portrait.

use std::path::PathBuf;

use aegis_desktop_entries::xdg_data_dirs;

/// Candidate still-image avatar paths, in lookup precedence.
///
/// Order: the canonical Aegis data location for every name in
/// [`still_names`], then the freedesktop `~/.face` and `~/.face.icon`
/// compatibility locations. The Aegis location wins because a user who placed
/// a file there made an explicit Aegis choice.
pub fn candidate_paths() -> Vec<PathBuf> {
    candidate_paths_from(aegis_avatar_dir(), home_dir())
}

fn candidate_paths_from(aegis: PathBuf, home: Option<PathBuf>) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    for name in still_names() {
        paths.push(aegis.join(name));
    }
    if let Some(home) = home {
        paths.push(home.join(".face"));
        paths.push(home.join(".face.icon"));
    }
    paths
}

/// Candidate VRM model paths, in lookup precedence. Only the canonical Aegis
/// data location is searched; 3D avatars are an Aegis-specific configuration.
pub fn vrm_candidate_paths() -> Vec<PathBuf> {
    let aegis = aegis_avatar_dir();
    ["avatar.vrm", "avatar.vrma"]
        .into_iter()
        .map(|name| aegis.join(name))
        .collect()
}

/// File names tried under `$XDG_DATA_HOME/aegis/avatars/`, in order. The bare
/// `face` name mirrors the freedesktop convention inside the Aegis namespace;
/// the explicit extensions let a user disambiguate when several formats coexist.
fn still_names() -> [&'static str; 4] {
    ["face.png", "face.jpg", "face.webp", "face"]
}

/// `$XDG_DATA_HOME/aegis/avatars` resolved through the workspace XDG helper.
/// Falls back to the spec defaults (`$HOME/.local/share` then the system dirs)
/// and returns the first base when none exist yet, so a user can create the
/// directory and drop a file in.
fn aegis_avatar_dir() -> PathBuf {
    xdg_data_dirs()
        .into_iter()
        .map(|base| base.join("aegis").join("avatars"))
        .next()
        .unwrap_or_else(|| PathBuf::from(".local/share/aegis/avatars"))
}

/// `$HOME` as an absolute path, or `None` when unset/relative, matching the
/// base-directory spec's "ignore invalid HOME" rule.
fn home_dir() -> Option<PathBuf> {
    home_dir_from(std::env::var_os("HOME"))
}

fn home_dir_from(home: Option<std::ffi::OsString>) -> Option<PathBuf> {
    home.filter(|home| {
        let path = PathBuf::from(home);
        !path.as_os_str().is_empty() && path.is_absolute()
    })
    .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn still_candidates_include_freedesktop_face_when_home_is_set() {
        let paths = candidate_paths_from(
            PathBuf::from("/tmp/aegis-avatar-test-home/.local/share/aegis/avatars"),
            Some(PathBuf::from("/tmp/aegis-avatar-test-home")),
        );
        // The canonical Aegis location is always first.
        assert!(paths[0].to_string_lossy().contains("aegis/avatars"));
        // The freedesktop ~/.face compatibility location is present.
        assert!(
            paths
                .iter()
                .any(|p| p.to_string_lossy().ends_with("/.face")),
            "{paths:?}"
        );
    }

    #[test]
    fn vrm_candidates_are_only_aegis_namespaced() {
        let paths = vrm_candidate_paths();
        assert!(
            paths
                .iter()
                .all(|p| p.to_string_lossy().contains("aegis/avatars"))
        );
        assert!(
            paths
                .iter()
                .any(|p| p.to_string_lossy().ends_with("avatar.vrm"))
        );
    }

    #[test]
    fn home_dir_ignores_relative_home() {
        assert!(home_dir_from(Some("relative/home".into())).is_none());
        assert!(home_dir_from(Some("/absolute/home".into())).is_some());
        assert!(home_dir_from(None).is_none());
    }
}
