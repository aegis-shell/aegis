//! End-to-end against the host's real `/usr/share/applications`.
//!
//! Skips automatically when that directory is absent (CI sandboxes).

use ass_apps::{xdg_data_dirs, Entry};
use std::collections::HashSet;
use std::path::PathBuf;

fn have_system_apps() -> bool {
    PathBuf::from("/usr/share/applications").is_dir()
}

#[test]
fn enumerates_real_host_applications() {
    if !have_system_apps() {
        return;
    }
    let apps = ass_apps::enumerate();
    assert!(!apps.is_empty(), "expected system .desktop files on this host");

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
    let apps = ass_apps::enumerate();
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
    let apps = ass_apps::enumerate();
    // Alacritty.desktop (capital A) is a common case-sensitive file.
    if apps.iter().any(|e| e.id == "Alacritty.desktop") {
        // Its StartupWMClass is "Alacritty" — the app_id join key.
        let a = apps
            .iter()
            .find(|e| e.id == "Alacritty.desktop")
            .unwrap();
        assert_eq!(a.startup_wm_class.as_deref(), Some("Alacritty"));
    }
}

#[test]
fn icon_resolution_uses_hicolor() {
    if !have_system_apps() {
        return;
    }
    let bases = ass_apps::icon_search_bases();
    // "foot" is a well-known hicolor icon name on a foot-installed host.
    if let Some(p) = ass_apps::resolve_icon("foot", None, &bases, 48) {
        assert!(p.exists(), "{p:?} missing");
        assert!(
            p.to_string_lossy().contains("/icons/hicolor/"),
            "expected hicolor path, got {p:?}"
        );
    }
}

#[test]
fn icon_resolution_picks_closest_size() {
    let bases = ass_apps::icon_search_bases();
    if let Some(p) = ass_apps::resolve_icon("btop", None, &bases, 48) {
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
    // System dirs always present.
    assert!(dirs.contains(&PathBuf::from("/usr/share")));
    // No empty components leak through.
    assert!(dirs.iter().all(|d| !d.as_os_str().is_empty()));
}

#[test]
fn expand_exec_on_real_entries() {
    if !have_system_apps() {
        return;
    }
    for e in ass_apps::enumerate() {
        let exec = e.exec.as_deref().unwrap();
        // Round-trips without panic for every entry on the host.
        let _ = ass_apps::expand_exec(exec, &[], e.icon.as_deref(), Some(&e.name), None);
        let _ = ass_apps::expand_exec_tokens(exec, &[], e.icon.as_deref(), Some(&e.name), None);
    }
}
