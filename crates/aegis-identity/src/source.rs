//! One shared source and precedence contract for every portrait consumer.

use std::path::{Path, PathBuf};

use aegis_desktop_entries::xdg_data_dirs;

/// One configured portrait candidate.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PortraitCandidate {
    Still(PathBuf),
    Vrm {
        model: PathBuf,
        legacy_motion: PathBuf,
    },
}

/// Ordered portrait candidates shared by lock, shell chrome, and portals.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PortraitConfig {
    candidates: Vec<PortraitCandidate>,
}

impl PortraitConfig {
    /// Construct an explicit ordered configuration. The first usable source
    /// wins, so embedders can choose policy without changing render code.
    pub fn new(candidates: Vec<PortraitCandidate>) -> Self {
        Self { candidates }
    }

    /// Resolve the canonical Aegis and freedesktop compatibility convention.
    ///
    /// Normal precedence is canonical stills, `~/.face` compatibility, then
    /// canonical VRM. An explicitly enabled debug VRM precedes all user
    /// candidates so source-tree previewing is deterministic.
    pub fn current() -> Self {
        Self::from_roots(aegis_avatar_dir(), home_dir(), enabled_debug_asset_dir())
    }

    pub fn candidates(&self) -> &[PortraitCandidate] {
        &self.candidates
    }

    pub(crate) fn has_existing_source(&self) -> bool {
        self.candidates.iter().any(|candidate| match candidate {
            PortraitCandidate::Still(path) => path.is_file(),
            PortraitCandidate::Vrm { model, .. } => model.is_file(),
        })
    }

    fn from_roots(aegis: PathBuf, home: Option<PathBuf>, debug: Option<PathBuf>) -> Self {
        let mut candidates = Vec::new();
        if let Some(debug) = debug {
            candidates.push(vrm_candidate(debug));
        }
        for name in ["face.png", "face.jpg", "face.webp", "face"] {
            candidates.push(PortraitCandidate::Still(aegis.join(name)));
        }
        if let Some(home) = home {
            candidates.push(PortraitCandidate::Still(home.join(".face")));
            candidates.push(PortraitCandidate::Still(home.join(".face.icon")));
        }
        candidates.push(vrm_candidate(aegis));
        Self { candidates }
    }
}

fn vrm_candidate(directory: PathBuf) -> PortraitCandidate {
    PortraitCandidate::Vrm {
        model: directory.join("avatar.vrm"),
        legacy_motion: directory.join("avatar.vrma"),
    }
}

fn enabled_debug_asset_dir() -> Option<PathBuf> {
    (cfg!(debug_assertions)
        && std::env::var_os("AEGIS_AVATAR_DEBUG_ASSETS").is_some_and(|value| !value.is_empty()))
    .then(|| {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("aegis-identity lives inside the workspace crates directory")
            .join("aegis-avatar/debug-assets")
    })
}

fn aegis_avatar_dir() -> PathBuf {
    xdg_data_dirs()
        .into_iter()
        .map(|base| base.join("aegis").join("avatars"))
        .next()
        .unwrap_or_else(|| PathBuf::from(".local/share/aegis/avatars"))
}

fn home_dir() -> Option<PathBuf> {
    home_dir_from(std::env::var_os("HOME"))
}

fn home_dir_from(home: Option<std::ffi::OsString>) -> Option<PathBuf> {
    home.filter(|home| {
        let path = Path::new(home);
        !path.as_os_str().is_empty() && path.is_absolute()
    })
    .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normal_precedence_is_shared_stills_then_vrm() {
        let config = PortraitConfig::from_roots(
            PathBuf::from("/data/aegis/avatars"),
            Some(PathBuf::from("/home/test")),
            None,
        );
        assert!(matches!(
            &config.candidates()[0],
            PortraitCandidate::Still(path) if path.ends_with("aegis/avatars/face.png")
        ));
        assert!(matches!(
            config.candidates().last().unwrap(),
            PortraitCandidate::Vrm { model, legacy_motion }
                if model.ends_with("aegis/avatars/avatar.vrm")
                    && legacy_motion.ends_with("aegis/avatars/avatar.vrma")
        ));
    }

    #[test]
    fn debug_vrm_is_an_explicit_first_candidate() {
        let config = PortraitConfig::from_roots(
            PathBuf::from("/data/aegis/avatars"),
            None,
            Some(PathBuf::from("/src/debug-assets")),
        );
        assert!(matches!(
            &config.candidates()[0],
            PortraitCandidate::Vrm { model, .. } if model.ends_with("debug-assets/avatar.vrm")
        ));
    }

    #[test]
    fn relative_home_is_not_a_portrait_root() {
        assert!(home_dir_from(Some("relative/home".into())).is_none());
        assert!(home_dir_from(Some("/absolute/home".into())).is_some());
    }
}
