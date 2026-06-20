//! XDG base-directory resolution.
//!
//! Implements the
//! [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/):
//!
//! - `$XDG_DATA_HOME` (default `$HOME/.local/share`) comes first.
//! - `$XDG_DATA_DIRS` (default `/usr/local/share:/usr/share`) follow, in
//!   order. Empty components are skipped.
//!
//! Icon search additionally appends `/usr/share/pixmaps` (the icon-theme
//! spec's legacy fallback) to every `<data>/icons` set.

use std::path::PathBuf;

/// The `$XDG_DATA_HOME` / `$XDG_DATA_DIRS` list, in lookup precedence.
///
/// Empty entries are skipped. A missing `$HOME` collapses the home-relative
/// default to nothing rather than panicking.
pub fn xdg_data_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();

    // XDG_DATA_HOME: single directory, default ~/.local/share.
    let home = match std::env::var("HOME") {
        Ok(h) if !h.is_empty() => Some(PathBuf::from(h)),
        _ => dirs::data_dir(),
    };
    match std::env::var("XDG_DATA_HOME") {
        Ok(v) if !v.is_empty() && v != "$HOME/.local/share" => {
            out.push(PathBuf::from(v));
        }
        _ => {
            if let Some(h) = home {
                out.push(h.join(".local/share"));
            } else if let Some(d) = dirs::data_dir() {
                out.push(d);
            }
        }
    }

    // XDG_DATA_DIRS: colon list, default /usr/local/share:/usr/share.
    let dirs_env = std::env::var("XDG_DATA_DIRS")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "/usr/local/share:/usr/share".to_string());
    for part in dirs_env.split(':') {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        let pb = PathBuf::from(p);
        if !out.contains(&pb) {
            out.push(pb);
        }
    }

    // Guarantee the spec defaults are present even if the environment is
    // hostile (e.g. tests running with a cleared env). Deduplicated above.
    for default in ["/usr/local/share", "/usr/share"] {
        let pb = PathBuf::from(default);
        if !out.contains(&pb) {
            out.push(pb);
        }
    }

    out
}

/// Every base directory the icon-theme spec searches, in precedence.
///
/// For each XDG data dir this yields `<dir>/icons`, then appends
/// `/usr/share/pixmaps` exactly once as the legacy fallback. Duplicates are
/// removed.
pub fn icon_search_bases() -> Vec<PathBuf> {
    let mut out = Vec::new();
    for d in xdg_data_dirs() {
        let icons = d.join("icons");
        if !out.contains(&icons) {
            out.push(icons);
        }
    }
    let pixmap = PathBuf::from("/usr/share/pixmaps");
    if !out.contains(&pixmap) {
        out.push(pixmap);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn data_dirs_contains_system_defaults() {
        let dirs = xdg_data_dirs();
        assert!(dirs.contains(&PathBuf::from("/usr/share")));
        assert!(dirs.contains(&PathBuf::from("/usr/local/share")));
    }

    #[test]
    fn icon_bases_include_pixmaps_and_icons() {
        let bases = icon_search_bases();
        assert!(bases.contains(&PathBuf::from("/usr/share/icons")));
        assert!(bases.contains(&PathBuf::from("/usr/share/pixmaps")));
    }
}
