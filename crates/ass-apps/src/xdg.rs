//! XDG base-directory resolution.
//!
//! Implements the
//! [XDG Base Directory Specification](https://specifications.freedesktop.org/basedir-spec/):
//!
//! - `$XDG_DATA_HOME` (default `$HOME/.local/share`) comes first.
//! - `$XDG_DATA_DIRS` (default `/usr/local/share:/usr/share`) follow in order.
//! - Relative and empty XDG components are ignored.
//!
//! Icon search adds the legacy `$HOME/.icons` base and the unthemed
//! `/usr/share/pixmaps` fallback around the XDG `<data>/icons` bases.

use std::ffi::{OsStr, OsString};
use std::path::PathBuf;

/// The `$XDG_DATA_HOME` / `$XDG_DATA_DIRS` list, in lookup precedence.
///
/// Invalid relative and empty entries are skipped. A missing `$HOME`
/// collapses the home-relative default to nothing rather than panicking.
pub fn xdg_data_dirs() -> Vec<PathBuf> {
    xdg_data_dirs_from(
        std::env::var_os("XDG_DATA_HOME"),
        std::env::var_os("XDG_DATA_DIRS"),
        home_dir(),
    )
}

/// Pure XDG data-directory resolution used by [`xdg_data_dirs`] and tests.
///
/// The base-directory specification requires every XDG path to be absolute;
/// invalid relative components are ignored. The system defaults are used
/// only when `XDG_DATA_DIRS` is unset or empty, never appended to an explicit
/// search path.
fn xdg_data_dirs_from(
    data_home: Option<OsString>,
    data_dirs: Option<OsString>,
    home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut out = Vec::new();

    let data_home = absolute_nonempty(data_home.as_deref()).or_else(|| {
        home.filter(|p| p.is_absolute())
            .map(|p| p.join(".local/share"))
    });
    if let Some(path) = data_home {
        push_unique(&mut out, path);
    }

    let explicit = data_dirs.as_deref().filter(|value| !value.is_empty());
    if let Some(value) = explicit {
        for path in std::env::split_paths(value).filter(|path| path.is_absolute()) {
            push_unique(&mut out, path);
        }
    } else {
        push_unique(&mut out, PathBuf::from("/usr/local/share"));
        push_unique(&mut out, PathBuf::from("/usr/share"));
    }

    out
}

fn absolute_nonempty(value: Option<&OsStr>) -> Option<PathBuf> {
    let path = PathBuf::from(value?);
    (!path.as_os_str().is_empty() && path.is_absolute()).then_some(path)
}

fn home_dir() -> Option<PathBuf> {
    absolute_nonempty(std::env::var_os("HOME").as_deref()).or_else(dirs::home_dir)
}

fn push_unique(out: &mut Vec<PathBuf>, path: PathBuf) {
    if !out.contains(&path) {
        out.push(path);
    }
}

/// Every base directory the icon-theme spec searches, in precedence.
///
/// This yields `$HOME/.icons`, every XDG `<dir>/icons`, and finally
/// `/usr/share/pixmaps`. Duplicates are removed without changing precedence.
pub fn icon_search_bases() -> Vec<PathBuf> {
    let mut out = Vec::new();
    // The icon-theme specification keeps ~/.icons as its highest-precedence
    // compatibility location. XDG data-home/icons follows through the normal
    // data-directory list.
    if let Some(home) = home_dir() {
        push_unique(&mut out, home.join(".icons"));
    }
    for d in xdg_data_dirs() {
        push_unique(&mut out, d.join("icons"));
    }
    push_unique(&mut out, PathBuf::from("/usr/share/pixmaps"));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn os(value: &str) -> Option<OsString> {
        Some(OsString::from(value))
    }

    #[test]
    fn explicit_data_dirs_are_not_extended_with_defaults() {
        let dirs = xdg_data_dirs_from(
            os("/home/test/data"),
            os("/opt/share:/srv/share"),
            Some(PathBuf::from("/home/test")),
        );
        assert_eq!(
            dirs,
            ["/home/test/data", "/opt/share", "/srv/share"].map(PathBuf::from)
        );
    }

    #[test]
    fn unset_values_use_spec_defaults() {
        let dirs = xdg_data_dirs_from(None, None, Some(PathBuf::from("/home/test")));
        assert_eq!(
            dirs,
            ["/home/test/.local/share", "/usr/local/share", "/usr/share"].map(PathBuf::from)
        );
    }

    #[test]
    fn relative_xdg_paths_are_ignored() {
        let dirs = xdg_data_dirs_from(
            os("relative/home"),
            os("relative/system:/valid/share"),
            Some(PathBuf::from("/home/test")),
        );
        assert_eq!(
            dirs,
            ["/home/test/.local/share", "/valid/share"].map(PathBuf::from)
        );
    }

    #[test]
    fn duplicate_paths_keep_first_precedence() {
        let dirs = xdg_data_dirs_from(
            os("/home/test/data"),
            os("/opt/share:/opt/share:/usr/share"),
            Some(PathBuf::from("/home/test")),
        );
        assert_eq!(
            dirs,
            ["/home/test/data", "/opt/share", "/usr/share"].map(PathBuf::from)
        );
    }

    #[test]
    fn icon_bases_include_pixmaps_and_icons() {
        let bases = icon_search_bases();
        for data_dir in xdg_data_dirs() {
            assert!(bases.contains(&data_dir.join("icons")));
        }
        assert!(bases.contains(&PathBuf::from("/usr/share/pixmaps")));
    }
}
