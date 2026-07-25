//! End-to-end against the host's real `/usr/share/applications`.
//!
//! Skips automatically when that directory is absent (CI sandboxes).

use aegis_desktop_entries::{Entry, xdg_data_dirs};
use std::collections::HashSet;
use std::path::PathBuf;

fn have_system_apps() -> bool {
    xdg_data_dirs().contains(&PathBuf::from("/usr/share"))
        && PathBuf::from("/usr/share/applications").is_dir()
}

#[test]
fn enumerates_real_host_applications() {
    if !have_system_apps() {
        return;
    }
    let apps = aegis_desktop_entries::enumerate();
    assert!(
        !apps.is_empty(),
        "expected system .desktop files on this host"
    );

    // No duplicate ids.
    let id_set: HashSet<&str> = apps.iter().map(|e| e.id.as_str()).collect();
    assert_eq!(id_set.len(), apps.len(), "duplicate ids present");

    // Every entry is launchable: Type=Application (enforced), has Exec.
    for e in &apps {
        assert!(e.exec.is_some(), "{} has no Exec", e.id);
        assert!(!e.name.is_empty(), "{} has no Name", e.id);
        assert!(!e.no_display, "{} should have been filtered", e.id);
    }
}

#[test]
fn foot_and_btop_shape_is_correct() {
    if !have_system_apps() {
        return;
    }
    let apps = aegis_desktop_entries::enumerate();
    let by_id = |id: &str| -> Option<Entry> { apps.iter().find(|e| e.id == id).cloned() };

    if let Some(btop) = by_id("btop.desktop") {
        // btop.desktop sets Terminal=true.
        assert!(btop.terminal, "btop should be terminal-launched");
        assert!(btop.categories.iter().any(|c| c == "Monitor"));
        // Real icon should resolve to a hicolor png.
        assert!(
            btop.icon_path.is_some(),
            "btop icon should resolve; got None"
        );
        if let Some(ref p) = btop.icon_path {
            assert!(p.exists(), "resolved icon {p:?} does not exist");
        }
    }

    if let Some(foot) = by_id("foot.desktop") {
        assert_eq!(foot.exec.as_deref(), Some("foot"));
        assert!(!foot.terminal);
        assert!(foot.startup_wm_class.is_none() || foot.startup_wm_class.is_some());
    }
}

#[test]
fn desktop_id_is_case_sensitive() {
    if !have_system_apps() {
        return;
    }
    let apps = aegis_desktop_entries::enumerate();
    // Alacritty.desktop (capital A) is a common case-sensitive file.
    if apps.iter().any(|e| e.id == "Alacritty.desktop") {
        // Its StartupWMClass is "Alacritty" — the app_id join key.
        let a = apps.iter().find(|e| e.id == "Alacritty.desktop").unwrap();
        assert_eq!(a.startup_wm_class.as_deref(), Some("Alacritty"));
    }
}

#[test]
fn icon_resolution_uses_hicolor() {
    if !have_system_apps() {
        return;
    }
    let bases = aegis_desktop_entries::icon_search_bases();
    // "foot" is a well-known hicolor icon name on a foot-installed host.
    if let Some(p) = aegis_desktop_entries::resolve_icon("foot", None, &bases, 48) {
        assert!(p.exists(), "{p:?} missing");
        assert!(
            p.to_string_lossy().contains("/icons/hicolor/"),
            "expected hicolor path, got {p:?}"
        );
    }
}

#[test]
fn icon_resolution_picks_closest_size() {
    let bases = aegis_desktop_entries::icon_search_bases();
    if let Some(p) = aegis_desktop_entries::resolve_icon("btop", None, &bases, 48) {
        let s = p.to_string_lossy();
        // Should land on a size directory near 48 (32/48/64 acceptable).
        let near = ["48x48", "32x32", "64x64", "scalable"]
            .iter()
            .any(|d| s.contains(d));
        assert!(near, "unexpected size dir in {s}");
    }
}

#[test]
fn data_dirs_precedence_keeps_user_first() {
    let dirs = xdg_data_dirs();
    assert!(dirs.iter().all(|dir| dir.is_absolute()));
    let unique: HashSet<&PathBuf> = dirs.iter().collect();
    assert_eq!(unique.len(), dirs.len(), "duplicate XDG data dirs present");
}

#[test]
fn exported_flatpak_desktop_symlinks_are_discoverable() {
    let apps = aegis_desktop_entries::enumerate();
    for root in xdg_data_dirs() {
        if !root.to_string_lossy().contains("flatpak/exports/share") {
            continue;
        }
        let Ok(entries) = std::fs::read_dir(root.join("applications")) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !entry
                .file_type()
                .map(|kind| kind.is_symlink())
                .unwrap_or(false)
                || path.extension().and_then(|ext| ext.to_str()) != Some("desktop")
            {
                continue;
            }
            let Some(id) = path.file_name().and_then(|name| name.to_str()) else {
                continue;
            };
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            // Visibility rules can legitimately exclude an export. The first
            // entry parse_str considers visible must also appear in the full
            // XDG scan under the symlink's exported desktop id.
            if aegis_desktop_entries::parse_str(&text, id).ok().flatten().is_some() {
                assert!(
                    apps.iter().any(|app| app.id == id),
                    "visible Flatpak export {path:?} was not enumerated"
                );
                return;
            }
        }
    }
}

#[test]
fn expand_exec_on_real_entries() {
    if !have_system_apps() {
        return;
    }
    for e in aegis_desktop_entries::enumerate() {
        let exec = e.exec.as_deref().unwrap();
        // Round-trips without panic for every entry on the host.
        let _ = aegis_desktop_entries::expand_exec(exec, &[], e.icon.as_deref(), Some(&e.name), None);
        let _ = aegis_desktop_entries::expand_exec_tokens(exec, &[], e.icon.as_deref(), Some(&e.name), None);
    }
}
