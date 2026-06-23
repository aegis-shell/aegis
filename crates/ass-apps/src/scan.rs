//! The application scanner: walks `applications/` trees and parses each entry.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use ass_core::app::Entry;
use ini::Ini;

use crate::icon::resolve_icon;
use crate::locale::{current_locale, Locale};
use crate::xdg::icon_search_bases;
use crate::{AppsError, DEFAULT_ICON_SIZE, DEFAULT_ICON_THEME};

/// Enumerate entries under each `applications/` subtree of `roots`, in order.
///
/// `roots` are the XDG data directories (e.g. `~/.local/share`, `/usr/share`).
/// Each is scanned for `<root>/applications/**/*.desktop`. The first file
/// with a given desktop id wins; later duplicates are dropped (user overrides
/// system). Parse failures are logged and skipped rather than aborting the
/// whole scan — a single malformed `.desktop` must not hide the rest.
pub fn enumerate_in(roots: &[PathBuf]) -> Vec<Entry> {
    let locale = current_locale();
    let icon_bases = icon_search_bases();
    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<Entry> = Vec::new();

    for root in roots {
        let apps_dir = root.join("applications");
        let Ok(files) = walk_desktop_files(&apps_dir) else {
            continue;
        };
        for (id, path) in files {
            if !seen.insert(id.clone()) {
                continue;
            }
            match parse_path(&path, &id, &locale, &icon_bases) {
                Ok(Some(e)) => out.push(e),
                Ok(None) => {}
                Err(e) => log::warn!("ass-apps: skip {path:?}: {e}"),
            }
        }
    }

    out.sort_by_key(|a| a.name.to_lowercase());
    out
}

/// Recursively collect `(desktop_id, path)` pairs under `dir`, sorted for a
/// deterministic walk. `desktop_id` is the path relative to `dir` with the
/// OS separator replaced by `-` (the menu-spec desktop id). Case-sensitive.
fn walk_desktop_files(dir: &Path) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut out = Vec::new();
    visit(dir, dir, &mut out)?;
    out.sort();
    Ok(out)
}

fn visit(base: &Path, cur: &Path, out: &mut Vec<(String, PathBuf)>) -> std::io::Result<()> {
    for ent in std::fs::read_dir(cur)? {
        let ent = ent?;
        let path = ent.path();
        let ft = ent.file_type()?;
        if ft.is_dir() {
            visit(base, &path, out)?;
        } else if ft.is_file()
            && path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.eq_ignore_ascii_case("desktop"))
                .unwrap_or(false)
        {
            let rel = path.strip_prefix(base).unwrap_or(&path);
            let id = rel
                .to_string_lossy()
                .replace(std::path::MAIN_SEPARATOR, "-");
            out.push((id, path));
        }
    }
    Ok(())
}

/// Read a file off disk and parse it. Logs nothing on its own.
fn parse_path(
    path: &Path,
    id: &str,
    locale: &Locale,
    icon_bases: &[PathBuf],
) -> Result<Option<Entry>, AppsError> {
    let text = std::fs::read_to_string(path).map_err(AppsError::Io)?;
    parse_text(&text, path, id, locale, icon_bases)
}

/// Parse an in-memory `.desktop` document. Exposed for tests and for callers
/// that already hold the text.
pub fn parse_str(text: &str, id: &str) -> Result<Option<Entry>, AppsError> {
    let locale = current_locale();
    parse_text(text, Path::new(id), id, &locale, &icon_search_bases())
}

fn parse_text(
    text: &str,
    path: &Path,
    id: &str,
    locale: &Locale,
    icon_bases: &[PathBuf],
) -> Result<Option<Entry>, AppsError> {
    let ini = Ini::load_from_str(text)
        .map_err(|e| AppsError::Parse(path.to_path_buf(), e.to_string()))?;
    // Only the main entry; `[Desktop Action …]` quicklists are ignored.
    let Some(section) = ini.section(Some("Desktop Entry")) else {
        return Ok(None);
    };

    if section.get("Type").unwrap_or("") != "Application" {
        return Ok(None);
    }
    if section.get("NoDisplay").map(truthy).unwrap_or(false) {
        return Ok(None);
    }
    let try_exec = section.get("TryExec").map(str::to_string);
    if let Some(prog) = &try_exec {
        if !try_exec_resolves(prog) {
            return Ok(None);
        }
    }

    let icon = section.get("Icon").map(str::to_string);
    let icon_path = icon
        .as_deref()
        .and_then(|i| resolve_icon(i, Some(DEFAULT_ICON_THEME), icon_bases, DEFAULT_ICON_SIZE));

    Ok(Some(Entry {
        id: id.to_string(),
        name: pick_localized(section, "Name", locale)
            .unwrap_or_else(|| id.trim_end_matches(".desktop").to_string()),
        generic_name: pick_localized(section, "GenericName", locale),
        comment: pick_localized(section, "Comment", locale),
        exec: section.get("Exec").map(str::to_string),
        icon,
        icon_path,
        categories: section
            .get("Categories")
            .map(split_semicol)
            .unwrap_or_default(),
        keywords: pick_localized(section, "Keywords", locale)
            .map(|s| split_semicol(&s))
            .unwrap_or_default(),
        startup_wm_class: section
            .get("StartupWMClass")
            .map(str::to_string)
            .filter(|s| !s.is_empty()),
        try_exec,
        terminal: section.get("Terminal").map(truthy).unwrap_or(false),
        no_display: section.get("NoDisplay").map(truthy).unwrap_or(false),
        path: section.get("Path").map(PathBuf::from),
        mime_types: section
            .get("MimeType")
            .map(split_semicol)
            .unwrap_or_default(),
    }))
}

/// Pick the best value for a localized key. Tries each locale variant suffix
/// `[xx_YY]` in precedence, then the unlocalized base value.
fn pick_localized(section: &ini::Properties, base: &str, locale: &Locale) -> Option<String> {
    for variant in locale.variants() {
        let key = format!("{base}[{variant}]");
        if let Some(v) = section.get(&key) {
            return Some(v.to_string());
        }
    }
    section.get(base).map(str::to_string)
}

/// Split a desktop-entry `;`-delimited list. Trailing empty elements (such
/// lists conventionally end with `;`) are dropped.
fn split_semicol(s: &str) -> Vec<String> {
    s.split(';')
        .filter(|p| !p.is_empty())
        .map(str::to_string)
        .collect()
}

/// Recognize the spec's true-ish boolean values: `1`, `true`, `yes` (any
/// case). Everything else is false.
fn truthy(s: &str) -> bool {
    matches!(s.trim().to_ascii_lowercase().as_str(), "1" | "true" | "yes")
}

/// Resolve a `TryExec` value: an absolute path is used directly; otherwise
/// the program name is searched on `$PATH`.
fn try_exec_resolves(prog: &str) -> bool {
    let p = Path::new(prog);
    if p.is_absolute() {
        return p.exists();
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|dir| dir.join(prog).is_file())
}

#[cfg(test)]
mod tests {
    use super::*;

    const FOOT: &str = "[Desktop Entry]
Type=Application
Exec=foot
Icon=foot
Terminal=false
Categories=System;TerminalEmulator;
Name=Foot
GenericName=Terminal
Comment=A wayland native terminal emulator
";

    #[test]
    fn parses_basic_entry() {
        let e = parse_str(FOOT, "foot.desktop").unwrap().unwrap();
        assert_eq!(e.id, "foot.desktop");
        assert_eq!(e.name, "Foot");
        assert_eq!(e.exec.as_deref(), Some("foot"));
        assert_eq!(e.categories, vec!["System", "TerminalEmulator"]);
        assert!(!e.terminal);
    }

    #[test]
    fn skips_non_application_type() {
        let dir = "[Desktop Entry]\nType=Directory\nName=X\n";
        assert!(parse_str(dir, "x.directory").unwrap().is_none());
    }

    #[test]
    fn skips_no_display() {
        let hidden = "[Desktop Entry]\nType=Application\nNoDisplay=true\nName=X\n";
        assert!(parse_str(hidden, "x.desktop").unwrap().is_none());
    }

    #[test]
    fn locale_prefers_exact_variant() {
        let text = "[Desktop Entry]
Type=Application
Name=Base
Name[zh_CN]=基础
Exec=x
";
        let locale = Locale::parse("zh_CN.UTF-8");
        let e = parse_text(text, Path::new("x.desktop"), "x.desktop", &locale, &[])
            .unwrap()
            .unwrap();
        assert_eq!(e.name, "基础");
    }

    #[test]
    fn locale_falls_back_to_language() {
        let text = "[Desktop Entry]
Type=Application
Name=Base
Name[zh]=中文
Exec=x
";
        let locale = Locale::parse("zh_TW.UTF-8");
        let e = parse_text(text, Path::new("x.desktop"), "x.desktop", &locale, &[])
            .unwrap()
            .unwrap();
        assert_eq!(e.name, "中文");
    }

    #[test]
    fn locale_falls_back_to_base() {
        let text = "[Desktop Entry]
Type=Application
Name=Base
Name[de]=Basis
Exec=x
";
        let locale = Locale::parse("en_US.UTF-8");
        let e = parse_text(text, Path::new("x.desktop"), "x.desktop", &locale, &[])
            .unwrap()
            .unwrap();
        assert_eq!(e.name, "Base");
    }

    #[test]
    fn ignores_desktop_action_sections() {
        let text = "[Desktop Entry]
Type=Application
Name=Alacritty
Exec=alacritty

[Desktop Action New]
Name=New Terminal
Exec=alacritty msg new-window
";
        let e = parse_str(text, "Alacritty.desktop").unwrap().unwrap();
        assert_eq!(e.exec.as_deref(), Some("alacritty"));
    }

    #[test]
    fn truthy_and_split_helpers() {
        assert!(truthy("1") && truthy("true") && truthy("YES"));
        assert!(!truthy("0") && !truthy("false"));
        assert_eq!(
            split_semicol("System;Monitor;ConsoleOnly;"),
            vec!["System", "Monitor", "ConsoleOnly"]
        );
    }
}
