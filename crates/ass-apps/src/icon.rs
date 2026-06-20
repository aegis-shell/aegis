//! Icon-theme-spec lookup.
//!
//! Resolves a desktop entry's `Icon` value to an on-disk file. The value is
//! either an absolute path (used directly) or a theme-relative name resolved
//! through the chain:
//!
//! `requested theme` → its `index.theme` `Inherits` list → `hicolor`
//!
//! `hicolor` is always the final fallback per the
//! [Icon Theme Specification](https://specifications.freedesktop.org/icon-theme-spec/).
//! Within a theme we scan its `<size>x<size>[/@2]` subdirectories for an
//! `apps/<name>.{png,svg,xpm}` (plus a bare `/<name>` fallback) and pick the
//! size closest to the requested target. Full `index.theme` `Directories`/
//! `Context`/`MinSize`/`MaxSize`/`Threshold` semantics are deferred — see
//! ADR-0022.

use std::path::{Path, PathBuf};

use crate::xdg::icon_search_bases;
use crate::DEFAULT_ICON_THEME;

/// Icon file extensions searched, in preference order.
const ICON_EXTS: &[&str] = &["png", "svg", "svgz", "xpm"];

/// Resolve `icon` (a theme name or an absolute path) to a file.
///
/// `theme` defaults to `hicolor` when `None`; an unset `$XDG_CURRENT_DESKTOP`
/// or a non-existent theme falls through to `hicolor` regardless. `bases` is
/// the list returned by [`crate::icon_search_bases`]; pass `&[]` to use the
/// host default. `target_size` is the desired nominal pixel size.
pub fn resolve_icon(
    icon: &str,
    theme: Option<&str>,
    bases: &[PathBuf],
    target_size: u32,
) -> Option<PathBuf> {
    if icon.is_empty() {
        return None;
    }
    // Absolute paths are used verbatim.
    let p = Path::new(icon);
    if p.is_absolute() {
        return (p.exists()).then(|| p.to_path_buf());
    }

    let bases = if bases.is_empty() {
        icon_search_bases()
    } else {
        bases.to_vec()
    };
    let theme = match theme.filter(|t| !t.is_empty()) {
        Some(t) => t.to_string(),
        None => DEFAULT_ICON_THEME.to_string(),
    };

    // Walk the inheritance chain, de-duplicating, always ending at hicolor.
    for t in inheritance_chain(&theme, &bases) {
        if let Some(found) = lookup_in_theme(&t, icon, &bases, target_size) {
            return Some(found);
        }
    }
    None
}

/// Build `[theme, ...inherited..., "hicolor"]`, dropping cycles and dupes.
fn inheritance_chain(start: &str, bases: &[PathBuf]) -> Vec<String> {
    let mut chain = vec![start.to_string()];
    let mut cursor = start.to_string();
    let mut guard = 0usize;
    while guard < 16 {
        let Some(inherits) = read_inherits(&cursor, bases) else {
            break;
        };
        for parent in inherits {
            let p = parent.trim().to_string();
            if p.is_empty() || chain.contains(&p) {
                continue;
            }
            chain.push(p.clone());
            // Advance cursor to the last newly-discovered parent so we follow
            // one level deeper on the next iteration.
            cursor = p;
        }
        guard += 1;
    }
    if !chain.iter().any(|t| t == DEFAULT_ICON_THEME) {
        chain.push(DEFAULT_ICON_THEME.to_string());
    }
    chain
}

/// Parse a theme's `index.theme` `Inherits=` line. Returns `None` if the
/// theme has no `index.theme` (unusual but tolerated).
fn read_inherits(theme: &str, bases: &[PathBuf]) -> Option<Vec<String>> {
    let path = find_index_theme(theme, bases)?;
    let txt = std::fs::read_to_string(&path).ok()?;
    for line in txt.lines() {
        let line = line.trim();
        if let Some(rest) = line
            .strip_prefix("Inherits")
            .and_then(|r| r.strip_prefix('='))
        {
            return Some(rest.split(',').map(|s| s.trim().to_string()).collect());
        }
    }
    None
}

fn find_index_theme(theme: &str, bases: &[PathBuf]) -> Option<PathBuf> {
    for b in bases {
        let idx = b.join(theme).join("index.theme");
        if idx.exists() {
            return Some(idx);
        }
    }
    None
}

/// Search one theme's directory tree for `icon`, picking the closest size.
fn lookup_in_theme(
    theme: &str,
    icon: &str,
    bases: &[PathBuf],
    target: u32,
) -> Option<PathBuf> {
    let mut best: Option<(u32, PathBuf)> = None;
    for b in bases {
        let theme_dir = b.join(theme);
        let Ok(entries) = std::fs::read_dir(&theme_dir) else {
            continue;
        };
        for ent in entries.flatten() {
            let name = ent.file_name();
            let Some(size) = parse_size_dir(&name.to_string_lossy()) else {
                continue;
            };
            // Prefer the `apps/` context directory; fall back to the theme
            // dir root for non-conforming themes.
            for sub in ["apps", ""] {
                let dir = if sub.is_empty() {
                    ent.path()
                } else {
                    ent.path().join(sub)
                };
                for ext in ICON_EXTS {
                    let cand = dir.join(icon).with_extension(ext);
                    if cand.exists() {
                        let dist = size.abs_diff(target);
                        if best.as_ref().map_or(true, |(d, _)| dist < *d) {
                            best = Some((dist, cand.clone()));
                        }
                    }
                }
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Parse a nominal pixel size out of a directory name like `48x48`,
/// `48x48@2`, `scalable`, or `symbolic`. `scalable`/`symbolic` map to a very
/// large nominal size so vector sources only win when no closer raster exists.
fn parse_size_dir(name: &str) -> Option<u32> {
    let head = name.split('@').next().unwrap_or(name);
    if head == "scalable" {
        return Some(256);
    }
    if head == "symbolic" {
        return Some(8);
    }
    head.split('x').next()?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_size_variants() {
        assert_eq!(parse_size_dir("48x48"), Some(48));
        assert_eq!(parse_size_dir("48x48@2"), Some(48));
        assert_eq!(parse_size_dir("scalable"), Some(256));
        assert_eq!(parse_size_dir("symbolic"), Some(8));
        assert_eq!(parse_size_dir("bogus"), None);
    }

    #[test]
    fn empty_icon_returns_none() {
        assert!(resolve_icon("", None, &[], 48).is_none());
    }
}
