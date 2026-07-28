//! Filesystem-level integration for `aegis-config`: loading from a path and
//! mtime-based reload transitions. Uses a process-unique path under the
//! system temp dir so no external `tempfile` dependency is needed.

use std::fs;
use std::path::PathBuf;
use std::thread;
use std::time::Duration;

use aegis_config::ReloadWatcher;

/// A unique throwaway path under the temp dir, namespaced by pid + a counter
/// so parallel test processes do not collide.
fn scratch(name: &str) -> PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut p = std::env::temp_dir();
    p.push(format!("aegis-config-{pid}-{n}-{name}.toml"));
    p
}

/// `mtime` has coarse resolution on many filesystems (often 1 second), so a
/// back-to-back write may not register as a change. Sleep just long enough
/// to force the timestamp to advance.
fn force_mtime_advance() {
    thread::sleep(Duration::from_millis(1100));
}

#[test]
fn missing_file_loads_as_none() {
    let path = scratch("missing");
    // The scratch path is unique and never created here.
    assert!(!path.exists());
    assert!(matches!(aegis_config::load(&path), Ok(None)));
}

#[test]
fn load_reads_and_parses_a_real_file() {
    let path = scratch("good");
    fs::write(
        &path,
        "schema_version = 1\n\
         [[keybind]]\n\
         mods = [\"super\"]\n\
         key = \"q\"\n\
         action = \"close\"\n",
    )
    .unwrap();
    let cfg = aegis_config::load(&path)
        .unwrap()
        .expect("file should load");
    let (binds, errs) = cfg.resolve_keybinds();
    assert!(errs.is_empty());
    assert_eq!(binds.len(), 1);
    fs::remove_file(&path).ok();
}

#[test]
fn invalid_file_reports_diagnostics_not_a_crash() {
    let path = scratch("bad");
    fs::write(&path, "schema_version = 7\n").unwrap();
    match aegis_config::load(&path) {
        Err(aegis_config::LoadError::Invalid { diagnostics, .. }) => {
            assert!(diagnostics.iter().any(|d| d.message.contains('7')));
        }
        other => panic!("expected Invalid, got {other:?}"),
    }
    fs::remove_file(&path).ok();
}

#[test]
fn reload_watcher_reports_modify_create_and_delete() {
    let path = scratch("reload");
    fs::write(&path, "schema_version = 1\n").unwrap();
    let mut w = ReloadWatcher::at(&path);
    // Baseline captured at construction; no change yet.
    assert!(!w.changed(&path), "no change immediately after baseline");

    // Modify: a new mtime (after forcing resolution to advance).
    force_mtime_advance();
    fs::write(&path, "schema_version = 1\n# edited\n").unwrap();
    assert!(w.changed(&path), "modification detected");
    assert!(!w.changed(&path), "reported once per transition");

    // Delete.
    fs::remove_file(&path).unwrap();
    assert!(w.changed(&path), "deletion detected");
    assert!(!w.changed(&path), "deletion reported once");

    // Re-create.
    force_mtime_advance();
    fs::write(&path, "schema_version = 1\n").unwrap();
    assert!(w.changed(&path), "re-creation detected");
    fs::remove_file(&path).ok();
}

#[test]
fn default_path_points_at_aegis_config_toml() {
    let Some(p) = aegis_config::default_path() else {
        // No home dir on this host; nothing to assert.
        return;
    };
    // .../aegis/config.toml: parent dir ends in `aegis`, file is `config.toml`.
    assert!(
        p.parent().is_some_and(|par| par.ends_with("aegis")),
        "{}",
        p.display()
    );
    assert!(p.file_name().and_then(|n| n.to_str()) == Some("config.toml"));
}
