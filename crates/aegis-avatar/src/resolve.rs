//! XDG-conformant avatar source resolution.
//!
//! Reuses [`aegis_desktop_entries::xdg_data_dirs`] for the base-directory
//! spec rather than hand-rolling `$XDG_DATA_HOME` again. Two precedence
//! groups are searched:
//!
//! 1. **Still images** — the canonical Aegis location first, then the
//!    freedesktop `~/.face` convention that GNOME/SDDM/LightDM already write.
//! 2. **VRM models** — the canonical Aegis location, plus an explicitly
//!    enabled source-tree debug fixture in debug builds.
//!
//! `$XDG_DATA_HOME/aegis/avatars/` follows the canonical-namespace decision
//! (ADR-0066) and keeps user-chosen art out of the cache directory, which is
//! disposable and the wrong home for a deliberate portrait.

use std::path::PathBuf;

use aegis_desktop_entries::xdg_data_dirs;

/// Candidate still-image avatar paths, in lookup precedence.
///
/// Order: the canonical Aegis data location for every name in
/// `still_names`, then the freedesktop `~/.face` and `~/.face.icon`
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

/// Candidate VRM model paths, in lookup precedence. The opt-in local debug
/// fixture precedes the canonical Aegis data location when enabled.
pub fn vrm_candidate_paths() -> Vec<PathBuf> {
    vrm_candidate_paths_from(aegis_avatar_dir(), enabled_debug_asset_dir())
}

fn vrm_candidate_paths_from(aegis: PathBuf, debug: Option<PathBuf>) -> Vec<PathBuf> {
    debug
        .into_iter()
        .map(|dir| dir.join("avatar.vrm"))
        .chain(std::iter::once(aegis.join("avatar.vrm")))
        .collect()
}

/// Legacy companion VRMA clips paired positionally with
/// [`vrm_candidate_paths`]. Motion-library directories are derived beside the
/// selected model instead. A `.vrma` contains animation only; it is never
/// passed to the model loader as if it contained renderable meshes.
pub fn vrma_candidate_paths() -> Vec<PathBuf> {
    vrma_candidate_paths_from(aegis_avatar_dir(), enabled_debug_asset_dir())
}

fn vrma_candidate_paths_from(aegis: PathBuf, debug: Option<PathBuf>) -> Vec<PathBuf> {
    debug
        .into_iter()
        .map(|dir| dir.join("avatar.vrma"))
        .chain(std::iter::once(aegis.join("avatar.vrma")))
        .collect()
}

/// Source-tree fixtures are opt-in and compiled out of release builds. This
/// avoids making a developer's ignored files an accidental release default.
fn enabled_debug_asset_dir() -> Option<PathBuf> {
    debug_assets_enabled()
        .then(|| std::env::var_os("AEGIS_AVATAR_DEBUG_ASSETS"))
        .flatten()
        .filter(|value| !value.is_empty())
        .map(|_| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("debug-assets"))
}

pub(crate) fn debug_assets_enabled() -> bool {
    cfg!(debug_assertions)
        && std::env::var_os("AEGIS_AVATAR_DEBUG_ASSETS").is_some_and(|value| !value.is_empty())
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
    fn vrm_and_vrma_candidates_stay_paired() {
        let aegis = PathBuf::from("/data/aegis/avatars");
        let debug = Some(PathBuf::from("/src/aegis-avatar/debug-assets"));
        let models = vrm_candidate_paths_from(aegis.clone(), debug.clone());
        let motions = vrma_candidate_paths_from(aegis, debug);
        assert_eq!(models.len(), 2);
        assert_eq!(motions.len(), models.len());
        assert!(models[0].ends_with("debug-assets/avatar.vrm"));
        assert!(motions[0].ends_with("debug-assets/avatar.vrma"));
        assert!(models[1].ends_with("aegis/avatars/avatar.vrm"));
        assert!(motions[1].ends_with("aegis/avatars/avatar.vrma"));
    }

    #[test]
    fn home_dir_ignores_relative_home() {
        assert!(home_dir_from(Some("relative/home".into())).is_none());
        assert!(home_dir_from(Some("/absolute/home".into())).is_some());
        assert!(home_dir_from(None).is_none());
    }
}
