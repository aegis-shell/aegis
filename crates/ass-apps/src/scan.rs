//! The application scanner: walks `applications/` trees and parses each entry.

use std::collections::HashSet;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use ass_core::app::{ApplicationTarget, Entry};
use ini::Ini;

use crate::icon::resolve_icon_scaled;
use crate::locale::{Locale, current_locale};
use crate::xdg::icon_search_bases;
use crate::{AppsError, DEFAULT_ICON_SIZE, DEFAULT_ICON_THEME};

struct ParseContext<'a> {
    locale: &'a Locale,
    icon_bases: &'a [PathBuf],
    icon_theme: &'a str,
    icon_scale: u32,
    current_desktops: &'a [String],
}

/// Enumerate entries under each `applications/` subtree of `roots`, in order.
///
/// `roots` are the XDG data directories (e.g. `~/.local/share`, `/usr/share`).
/// Each is scanned for `<root>/applications/**/*.desktop`. The first file
/// with a given desktop id wins; later duplicates are dropped (user overrides
/// system). Parse failures are logged and skipped rather than aborting the
/// whole scan — a single malformed `.desktop` must not hide the rest.
pub fn enumerate_in(roots: &[PathBuf]) -> Vec<Entry> {
    enumerate_in_with_theme_and_scale(roots, DEFAULT_ICON_THEME, 1)
}

/// Enumerate entries using an explicit icon theme.
///
/// This is the same scan as [`enumerate_in`], but lets a compositor supply
/// its user-selected theme while preserving `hicolor` as the final fallback.
pub fn enumerate_in_with_theme(roots: &[PathBuf], icon_theme: &str) -> Vec<Entry> {
    enumerate_in_with_theme_and_scale(roots, icon_theme, 1)
}

/// Enumerate entries using an explicit icon theme and output scale.
pub fn enumerate_in_with_theme_and_scale(
    roots: &[PathBuf],
    icon_theme: &str,
    icon_scale: u32,
) -> Vec<Entry> {
    let locale = current_locale();
    let icon_bases = icon_search_bases();
    let current_desktops = current_desktops();
    let context = ParseContext {
        locale: &locale,
        icon_bases: &icon_bases,
        icon_theme,
        icon_scale,
        current_desktops: &current_desktops,
    };
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
            match parse_path(&path, &id, &context) {
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
        } else if (ft.is_file() || (ft.is_symlink() && path.is_file()))
            && path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e == "desktop")
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
    context: &ParseContext<'_>,
) -> Result<Option<Entry>, AppsError> {
    let text = std::fs::read_to_string(path).map_err(AppsError::Io)?;
    parse_text(&text, path, id, context)
}

/// Parse an in-memory `.desktop` document. Exposed for tests and for callers
/// that already hold the text.
pub fn parse_str(text: &str, id: &str) -> Result<Option<Entry>, AppsError> {
    let locale = current_locale();
    let icon_bases = icon_search_bases();
    let current_desktops = current_desktops();
    let context = ParseContext {
        locale: &locale,
        icon_bases: &icon_bases,
        icon_theme: DEFAULT_ICON_THEME,
        icon_scale: 1,
        current_desktops: &current_desktops,
    };
    parse_text(text, Path::new(id), id, &context)
}

fn parse_text(
    text: &str,
    path: &Path,
    id: &str,
    context: &ParseContext<'_>,
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
    // Hidden is a deletion marker, not merely a presentation hint. Because
    // the desktop id was marked seen before parsing, a higher-precedence
    // Hidden entry also masks a lower-precedence system entry.
    if section.get("Hidden").map(truthy).unwrap_or(false) {
        return Ok(None);
    }
    if section.get("NoDisplay").map(truthy).unwrap_or(false) {
        return Ok(None);
    }
    if !visible_on_desktop(section, context.current_desktops) {
        return Ok(None);
    }
    let try_exec = section.get("TryExec").map(str::to_string);
    if let Some(prog) = &try_exec
        && !try_exec_resolves(prog)
    {
        return Ok(None);
    }

    let Some(name) =
        pick_localized(section, "Name", context.locale).filter(|name| !name.is_empty())
    else {
        return Ok(None);
    };
    // This launcher currently starts the compatibility Exec path even for a
    // DBusActivatable entry. Entries without that required fallback are not
    // launchable by this backend and must not appear as broken rows.
    let exec = section
        .get("Exec")
        .map(str::to_string)
        .filter(|exec| !exec.trim().is_empty());
    if exec.is_none() {
        return Ok(None);
    }

    let icon = section
        .get("Icon")
        .map(str::to_string)
        .filter(|icon| !icon.is_empty());
    let icon_path = icon.as_deref().and_then(|i| {
        resolve_icon_scaled(
            i,
            Some(context.icon_theme),
            context.icon_bases,
            DEFAULT_ICON_SIZE,
            context.icon_scale.max(1),
        )
    });

    Ok(Some(Entry {
        target: ApplicationTarget::External,
        id: id.to_string(),
        name,
        generic_name: pick_localized(section, "GenericName", context.locale),
        comment: pick_localized(section, "Comment", context.locale),
        exec,
        icon,
        icon_path,
        categories: section
            .get("Categories")
            .map(split_semicol)
            .unwrap_or_default(),
        keywords: pick_localized(section, "Keywords", context.locale)
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

fn current_desktops() -> Vec<String> {
    std::env::var("XDG_CURRENT_DESKTOP")
        .ok()
        .map(|value| {
            value
                .split(':')
                .filter(|desktop| !desktop.is_empty())
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// Apply the Desktop Entry Specification's ordered OnlyShowIn/NotShowIn
/// decision against the colon-separated XDG_CURRENT_DESKTOP names.
fn visible_on_desktop(section: &ini::Properties, desktops: &[String]) -> bool {
    let only = section
        .get("OnlyShowIn")
        .map(split_semicol)
        .unwrap_or_default();
    let not = section
        .get("NotShowIn")
        .map(split_semicol)
        .unwrap_or_default();

    for desktop in desktops {
        if only.contains(desktop) {
            return true;
        }
        if not.contains(desktop) {
            return false;
        }
    }
    only.is_empty()
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
        return is_executable_file(p);
    }
    let mut components = p.components();
    if !matches!(components.next(), Some(std::path::Component::Normal(_)))
        || components.next().is_some()
    {
        return false;
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|dir| is_executable_file(&dir.join(prog)))
}

fn is_executable_file(path: &Path) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};

    fn parse_with(
        text: &str,
        id: &str,
        locale: &Locale,
        desktops: &[String],
    ) -> Result<Option<Entry>, AppsError> {
        let context = ParseContext {
            locale,
            icon_bases: &[],
            icon_theme: DEFAULT_ICON_THEME,
            icon_scale: 1,
            current_desktops: desktops,
        };
        parse_text(text, Path::new(id), id, &context)
    }

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
        let e = parse_with(text, "x.desktop", &locale, &[])
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
        let e = parse_with(text, "x.desktop", &locale, &[])
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
        let e = parse_with(text, "x.desktop", &locale, &[])
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

    #[test]
    fn hidden_entry_masks_itself() {
        let hidden = "[Desktop Entry]\nType=Application\nHidden=true\nName=Hidden\nExec=hidden\n";
        assert!(parse_str(hidden, "hidden.desktop").unwrap().is_none());
    }

    #[test]
    fn desktop_visibility_obeys_only_and_not_show_in() {
        let locale = Locale::parse("en_US.UTF-8");
        let only =
            "[Desktop Entry]\nType=Application\nName=Only\nExec=only\nOnlyShowIn=GNOME;KDE;\n";
        let not = "[Desktop Entry]\nType=Application\nName=Not\nExec=not\nNotShowIn=niri;\n";
        let desktops = ["niri".to_string(), "GNOME".to_string()];

        assert!(
            parse_with(only, "only.desktop", &locale, &desktops)
                .unwrap()
                .is_some()
        );
        assert!(
            parse_with(not, "not.desktop", &locale, &desktops)
                .unwrap()
                .is_none()
        );
        assert!(
            parse_with(only, "only.desktop", &locale, &["niri".to_string()])
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn flatpak_style_desktop_symlink_is_enumerated_by_link_id() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("exports/share");
        let applications = root.join("applications");
        let target = temp.path().join("app/org.example.App/export/app.desktop");
        fs::create_dir_all(&applications).unwrap();
        fs::create_dir_all(target.parent().unwrap()).unwrap();
        fs::write(
            &target,
            "[Desktop Entry]\nType=Application\nName=Example\nExec=/usr/bin/true\n",
        )
        .unwrap();
        symlink(&target, applications.join("org.example.App.desktop")).unwrap();

        let entries = enumerate_in(&[root]);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].id, "org.example.App.desktop");
        assert_eq!(entries[0].name, "Example");
    }

    #[test]
    fn directory_symlinks_are_not_recursed() {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().join("root");
        let external = temp.path().join("external");
        fs::create_dir_all(root.join("applications")).unwrap();
        fs::create_dir_all(&external).unwrap();
        fs::write(
            external.join("outside.desktop"),
            "[Desktop Entry]\nType=Application\nName=Outside\nExec=/usr/bin/true\n",
        )
        .unwrap();
        symlink(&external, root.join("applications/linked-dir")).unwrap();

        assert!(enumerate_in(&[root]).is_empty());
    }

    #[test]
    fn try_exec_requires_an_executable_file() {
        let temp = tempfile::tempdir().unwrap();
        let program = temp.path().join("program");
        fs::write(&program, "#!/bin/sh\n").unwrap();
        fs::set_permissions(&program, fs::Permissions::from_mode(0o644)).unwrap();
        let text = format!(
            "[Desktop Entry]\nType=Application\nName=Program\nExec={}\nTryExec={}\n",
            program.display(),
            program.display()
        );
        assert!(parse_str(&text, "program.desktop").unwrap().is_none());

        fs::set_permissions(&program, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(parse_str(&text, "program.desktop").unwrap().is_some());
    }
}
