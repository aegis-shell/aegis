//! Icon-theme-spec lookup.
//!
//! Resolves a desktop entry's `Icon` value to an on-disk file. Absolute paths
//! are used directly. Theme-relative names follow the freedesktop.org lookup
//! algorithm: the requested theme, every inherited theme recursively, the
//! mandatory `hicolor` fallback, and finally unthemed icons such as
//! `/usr/share/pixmaps/<name>.png`.

use std::collections::HashSet;
use std::path::{Component, Path, PathBuf};

use ini::Ini;

use crate::DEFAULT_ICON_THEME;
use crate::xdg::icon_search_bases;

/// Icon file extensions searched in specification preference order. SVGZ is
/// accepted after the standard PNG/SVG/XPM formats as a compatible extension
/// used by some real-world themes.
const ICON_EXTS: &[&str] = &["png", "svg", "xpm", "svgz"];

#[derive(Debug, Clone, Copy)]
enum DirectoryType {
    Fixed,
    Scalable,
    Threshold,
}

#[derive(Debug, Clone)]
struct ThemeDirectory {
    name: PathBuf,
    size: u32,
    scale: u32,
    kind: DirectoryType,
    min_size: u32,
    max_size: u32,
    threshold: u32,
}

impl ThemeDirectory {
    fn matches(&self, size: u32, scale: u32) -> bool {
        if self.scale != scale {
            return false;
        }
        match self.kind {
            DirectoryType::Fixed => self.size == size,
            DirectoryType::Scalable => (self.min_size..=self.max_size).contains(&size),
            DirectoryType::Threshold => {
                self.size.saturating_sub(self.threshold) <= size
                    && size <= self.size.saturating_add(self.threshold)
            }
        }
    }

    fn distance(&self, size: u32, scale: u32) -> u64 {
        let requested = u64::from(size) * u64::from(scale);
        let own_scale = u64::from(self.scale);
        let (min, max) = match self.kind {
            DirectoryType::Fixed => {
                let exact = u64::from(self.size) * own_scale;
                (exact, exact)
            }
            DirectoryType::Scalable => (
                u64::from(self.min_size) * own_scale,
                u64::from(self.max_size) * own_scale,
            ),
            DirectoryType::Threshold => (
                u64::from(self.size.saturating_sub(self.threshold)) * own_scale,
                u64::from(self.size.saturating_add(self.threshold)) * own_scale,
            ),
        };
        if requested < min {
            min - requested
        } else {
            requested.saturating_sub(max)
        }
    }
}

#[derive(Debug)]
struct ThemeIndex {
    inherits: Vec<String>,
    directories: Vec<ThemeDirectory>,
}

/// Resolve `icon` for scale 1.
///
/// `theme` defaults to `hicolor` when `None`. `bases` is normally the list
/// returned by [`crate::icon_search_bases`]; pass `&[]` to use the host
/// defaults. `target_size` is the desired nominal logical size.
pub fn resolve_icon(
    icon: &str,
    theme: Option<&str>,
    bases: &[PathBuf],
    target_size: u32,
) -> Option<PathBuf> {
    resolve_icon_scaled(icon, theme, bases, target_size, 1)
}

/// Resolve `icon` for an explicit target scale.
///
/// Scale-aware lookup distinguishes e.g. `48x48` from `48x48@2` according to
/// `index.theme`'s `Scale` metadata rather than treating both as equivalent.
pub fn resolve_icon_scaled(
    icon: &str,
    theme: Option<&str>,
    bases: &[PathBuf],
    target_size: u32,
    target_scale: u32,
) -> Option<PathBuf> {
    if icon.is_empty() || target_size == 0 || target_scale == 0 {
        return None;
    }

    let path = Path::new(icon);
    if path.is_absolute() {
        return path.is_file().then(|| path.to_path_buf());
    }
    // An iconstring is a name, not an arbitrary relative path. Reject parent
    // traversal and subpaths before joining it below.
    if !valid_icon_name(path) {
        return None;
    }

    let bases = if bases.is_empty() {
        icon_search_bases()
    } else {
        bases.to_vec()
    };
    let theme = theme
        .filter(|name| !name.is_empty())
        .unwrap_or(DEFAULT_ICON_THEME);

    let mut visited = HashSet::new();
    if let Some(found) =
        find_in_theme_tree(theme, icon, &bases, target_size, target_scale, &mut visited)
    {
        return Some(found);
    }
    if theme != DEFAULT_ICON_THEME
        && let Some(found) = find_in_theme_tree(
            DEFAULT_ICON_THEME,
            icon,
            &bases,
            target_size,
            target_scale,
            &mut visited,
        )
    {
        return Some(found);
    }

    lookup_fallback_icon(icon, &bases)
}

fn valid_icon_name(path: &Path) -> bool {
    let mut components = path.components();
    matches!(components.next(), Some(Component::Normal(_))) && components.next().is_none()
}

fn find_in_theme_tree(
    theme: &str,
    icon: &str,
    bases: &[PathBuf],
    size: u32,
    scale: u32,
    visited: &mut HashSet<String>,
) -> Option<PathBuf> {
    if !visited.insert(theme.to_string()) {
        return None;
    }
    let index = load_theme_index(theme, bases)?;
    if let Some(found) = lookup_in_theme(theme, icon, bases, size, scale, &index.directories) {
        return Some(found);
    }
    for parent in index.inherits {
        if let Some(found) = find_in_theme_tree(&parent, icon, bases, size, scale, visited) {
            return Some(found);
        }
    }
    None
}

/// Load the first `index.theme` found in base-directory precedence, as
/// required when a theme is spread across several bases.
fn load_theme_index(theme: &str, bases: &[PathBuf]) -> Option<ThemeIndex> {
    let path = bases
        .iter()
        .map(|base| base.join(theme).join("index.theme"))
        .find(|path| path.is_file())?;
    let ini = Ini::load_from_file(path).ok()?;
    let main = ini.section(Some("Icon Theme"))?;

    let inherits = main.get("Inherits").map(split_commas).unwrap_or_default();
    let mut names = main
        .get("Directories")
        .map(split_commas)
        .unwrap_or_default();
    for name in main
        .get("ScaledDirectories")
        .map(split_commas)
        .unwrap_or_default()
    {
        if !names.contains(&name) {
            names.push(name);
        }
    }

    let directories = names
        .into_iter()
        .filter_map(|name| parse_theme_directory(&ini, &name))
        .collect();
    Some(ThemeIndex {
        inherits,
        directories,
    })
}

fn split_commas(value: &str) -> Vec<String> {
    value
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .map(str::to_string)
        .collect()
}

fn parse_theme_directory(ini: &Ini, name: &str) -> Option<ThemeDirectory> {
    let path = Path::new(name);
    if path.is_absolute()
        || path
            .components()
            .any(|part| !matches!(part, Component::Normal(_)))
    {
        return None;
    }
    let section = ini.section(Some(name))?;
    let size = section
        .get("Size")?
        .parse::<u32>()
        .ok()
        .filter(|n| *n > 0)?;
    let scale = section
        .get("Scale")
        .and_then(|value| value.parse::<u32>().ok())
        .filter(|n| *n > 0)
        .unwrap_or(1);
    let kind = match section.get("Type").unwrap_or("Threshold") {
        value if value.eq_ignore_ascii_case("Fixed") => DirectoryType::Fixed,
        value if value.eq_ignore_ascii_case("Scalable") => DirectoryType::Scalable,
        value if value.eq_ignore_ascii_case("Threshold") => DirectoryType::Threshold,
        _ => return None,
    };
    let min_size = section
        .get("MinSize")
        .and_then(|value| value.parse().ok())
        .unwrap_or(size);
    let max_size = section
        .get("MaxSize")
        .and_then(|value| value.parse().ok())
        .unwrap_or(size);
    if min_size > max_size {
        return None;
    }
    let threshold = section
        .get("Threshold")
        .and_then(|value| value.parse().ok())
        .unwrap_or(2);

    Some(ThemeDirectory {
        name: path.to_path_buf(),
        size,
        scale,
        kind,
        min_size,
        max_size,
        threshold,
    })
}

fn lookup_in_theme(
    theme: &str,
    icon: &str,
    bases: &[PathBuf],
    size: u32,
    scale: u32,
    directories: &[ThemeDirectory],
) -> Option<PathBuf> {
    for directory in directories.iter().filter(|dir| dir.matches(size, scale)) {
        if let Some(found) = lookup_in_directory(theme, icon, bases, directory) {
            return Some(found);
        }
    }

    let mut best: Option<(u64, PathBuf)> = None;
    for directory in directories {
        let Some(found) = lookup_in_directory(theme, icon, bases, directory) else {
            continue;
        };
        let distance = directory.distance(size, scale);
        if best
            .as_ref()
            .is_none_or(|(best_distance, _)| distance < *best_distance)
        {
            best = Some((distance, found));
        }
    }
    best.map(|(_, path)| path)
}

fn lookup_in_directory(
    theme: &str,
    icon: &str,
    bases: &[PathBuf],
    directory: &ThemeDirectory,
) -> Option<PathBuf> {
    for base in bases {
        let dir = base.join(theme).join(&directory.name);
        if let Some(found) = lookup_file(icon, &dir) {
            return Some(found);
        }
    }
    None
}

fn lookup_fallback_icon(icon: &str, bases: &[PathBuf]) -> Option<PathBuf> {
    bases.iter().find_map(|base| lookup_file(icon, base))
}

fn lookup_file(icon: &str, directory: &Path) -> Option<PathBuf> {
    // `Path::with_extension` cannot be used here: icon names commonly follow
    // reverse-DNS application ids (for example `org.mozilla.firefox`), and it
    // would replace `.firefox` instead of appending the image extension.
    // Explicit extensions are uncommon but valid in real desktop files, so
    // try an already-complete filename before adding the standard suffixes.
    let direct = directory.join(icon);
    if direct
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| {
            ICON_EXTS
                .iter()
                .any(|candidate| extension.eq_ignore_ascii_case(candidate))
        })
        && direct.is_file()
    {
        return Some(direct);
    }
    for extension in ICON_EXTS {
        let candidate = directory.join(format!("{icon}.{extension}"));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    fn write(path: &Path, contents: &str) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, contents).unwrap();
    }

    fn index(base: &Path, theme: &str, contents: &str) {
        write(&base.join(theme).join("index.theme"), contents);
    }

    #[test]
    fn empty_or_unsafe_icon_returns_none() {
        assert!(resolve_icon("", None, &[], 48).is_none());
        assert!(resolve_icon("../escape", None, &[], 48).is_none());
        assert!(resolve_icon("nested/icon", None, &[], 48).is_none());
    }

    #[test]
    fn absolute_icon_path_is_used_directly() {
        let temp = tempfile::tempdir().unwrap();
        let icon = temp.path().join("direct.png");
        write(&icon, "png");
        assert_eq!(
            resolve_icon(icon.to_str().unwrap(), None, &[], 48),
            Some(icon)
        );
    }

    #[test]
    fn exact_size_precedes_a_closer_directory_fallback() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        index(
            base,
            "test",
            "[Icon Theme]\nName=Test\nComment=Test\nDirectories=32x32/apps,48x48/apps\n\n[32x32/apps]\nSize=32\nType=Fixed\n\n[48x48/apps]\nSize=48\nType=Fixed\n",
        );
        write(&base.join("test/32x32/apps/demo.png"), "32");
        write(&base.join("test/48x48/apps/demo.png"), "48");

        let found = resolve_icon("demo", Some("test"), &[base.to_path_buf()], 48);
        assert_eq!(found, Some(base.join("test/48x48/apps/demo.png")));
    }

    #[test]
    fn scaled_directory_uses_index_metadata() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        index(
            base,
            "test",
            "[Icon Theme]\nName=Test\nComment=Test\nDirectories=48x48/apps\nScaledDirectories=48x48@2/apps\n\n[48x48/apps]\nSize=48\nType=Fixed\n\n[48x48@2/apps]\nSize=48\nScale=2\nType=Fixed\n",
        );
        write(&base.join("test/48x48/apps/demo.png"), "1x");
        write(&base.join("test/48x48@2/apps/demo.png"), "2x");

        let found = resolve_icon_scaled("demo", Some("test"), &[base.to_path_buf()], 48, 2);
        assert_eq!(found, Some(base.join("test/48x48@2/apps/demo.png")));
    }

    #[test]
    fn every_inheritance_branch_is_recursive() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        index(
            base,
            "root",
            "[Icon Theme]\nName=Root\nComment=Root\nInherits=first,second\nDirectories=\n",
        );
        index(
            base,
            "first",
            "[Icon Theme]\nName=First\nComment=First\nInherits=grandparent\nDirectories=\n",
        );
        index(
            base,
            "second",
            "[Icon Theme]\nName=Second\nComment=Second\nDirectories=\n",
        );
        index(
            base,
            "grandparent",
            "[Icon Theme]\nName=Grandparent\nComment=Grandparent\nDirectories=64x64/apps\n\n[64x64/apps]\nSize=64\nType=Fixed\n",
        );
        write(&base.join("grandparent/64x64/apps/inherited.png"), "icon");

        let found = resolve_icon("inherited", Some("root"), &[base.to_path_buf()], 64);
        assert_eq!(
            found,
            Some(base.join("grandparent/64x64/apps/inherited.png"))
        );
    }

    #[test]
    fn hicolor_is_the_mandatory_final_theme() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        index(
            base,
            "root",
            "[Icon Theme]\nName=Root\nComment=Root\nDirectories=\n",
        );
        index(
            base,
            "hicolor",
            "[Icon Theme]\nName=Hicolor\nComment=Hicolor\nDirectories=48x48/apps\n\n[48x48/apps]\nSize=48\nType=Fixed\n",
        );
        write(&base.join("hicolor/48x48/apps/fallback.png"), "icon");

        let found = resolve_icon("fallback", Some("root"), &[base.to_path_buf()], 48);
        assert_eq!(found, Some(base.join("hicolor/48x48/apps/fallback.png")));
    }

    #[test]
    fn unthemed_pixmap_fallback_is_searched_at_base_root() {
        let temp = tempfile::tempdir().unwrap();
        let pixmaps = temp.path().join("pixmaps");
        write(&pixmaps.join("legacy.svg"), "svg");

        let found = resolve_icon("legacy", None, std::slice::from_ref(&pixmaps), 48);
        assert_eq!(found, Some(pixmaps.join("legacy.svg")));
    }

    #[test]
    fn exported_icon_symlink_is_a_valid_candidate() {
        let temp = tempfile::tempdir().unwrap();
        let pixmaps = temp.path().join("pixmaps");
        let target = temp.path().join("app/export/icon.png");
        write(&target, "png");
        fs::create_dir_all(&pixmaps).unwrap();
        symlink(&target, pixmaps.join("exported.png")).unwrap();

        let found = resolve_icon("exported", None, std::slice::from_ref(&pixmaps), 48);
        assert_eq!(found, Some(pixmaps.join("exported.png")));
    }

    #[test]
    fn reverse_dns_icon_name_keeps_every_name_component() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        index(
            base,
            "hicolor",
            "[Icon Theme]\nName=Hicolor\nComment=Hicolor\nDirectories=128x128/apps\n\n[128x128/apps]\nSize=128\nType=Fixed\n",
        );
        write(
            &base.join("hicolor/128x128/apps/org.mozilla.firefox.png"),
            "icon",
        );

        let found = resolve_icon("org.mozilla.firefox", None, &[base.to_path_buf()], 128);
        assert_eq!(
            found,
            Some(base.join("hicolor/128x128/apps/org.mozilla.firefox.png"))
        );
    }

    #[test]
    fn icon_name_with_explicit_extension_is_used_verbatim() {
        let temp = tempfile::tempdir().unwrap();
        let pixmaps = temp.path().join("pixmaps");
        write(&pixmaps.join("explicit.svg"), "svg");

        let found = resolve_icon("explicit.svg", None, std::slice::from_ref(&pixmaps), 48);
        assert_eq!(found, Some(pixmaps.join("explicit.svg")));
    }

    #[test]
    fn closest_scalable_directory_uses_min_and_max_size() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path();
        index(
            base,
            "test",
            "[Icon Theme]\nName=Test\nComment=Test\nDirectories=scalable/apps,128x128/apps\n\n[scalable/apps]\nSize=48\nType=Scalable\nMinSize=16\nMaxSize=96\n\n[128x128/apps]\nSize=128\nType=Fixed\n",
        );
        write(&base.join("test/scalable/apps/demo.svg"), "svg");
        write(&base.join("test/128x128/apps/demo.png"), "png");

        let found = resolve_icon("demo", Some("test"), &[base.to_path_buf()], 80);
        assert_eq!(found, Some(base.join("test/scalable/apps/demo.svg")));
    }
}
