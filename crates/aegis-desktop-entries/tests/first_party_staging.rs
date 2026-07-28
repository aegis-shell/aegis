use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const CHILD_ROOT: &str = "AEGIS_STAGED_APP_TEST_ROOT";
const SETTINGS_ID: &str = "io.github.ming2k.aegis.Settings";

#[test]
fn staged_settings_is_discovered_through_xdg() {
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        assert_staged_settings(&PathBuf::from(root));
        return;
    }

    let temp = tempfile::tempdir().expect("create staging test directory");
    stage_settings(temp.path());

    let status = Command::new(std::env::current_exe().expect("locate integration test binary"))
        .arg("--exact")
        .arg("staged_settings_is_discovered_through_xdg")
        .arg("--nocapture")
        .env(CHILD_ROOT, temp.path())
        .env("HOME", temp.path().join("home"))
        .env("PATH", temp.path().join("bin"))
        .env("XDG_CURRENT_DESKTOP", "aegis")
        .env("XDG_DATA_HOME", temp.path().join("share"))
        .env("XDG_DATA_DIRS", temp.path().join("share"))
        .status()
        .expect("run isolated XDG discovery child");
    assert!(status.success(), "isolated XDG discovery child failed");
}

fn stage_settings(root: &Path) {
    let repo = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let data = root.join("share");
    let applications = data.join("applications");
    let icons = data.join("icons/hicolor/scalable/apps");
    let binary = root.join("bin/aegis-settings");

    fs::create_dir_all(&applications).expect("create applications directory");
    fs::create_dir_all(&icons).expect("create icon directory");
    fs::create_dir_all(binary.parent().unwrap()).expect("create bin directory");
    fs::create_dir_all(root.join("home")).expect("create test home");

    fs::copy(
        repo.join("contrib/io.github.ming2k.aegis.Settings.desktop"),
        applications.join(format!("{SETTINGS_ID}.desktop")),
    )
    .expect("stage Settings desktop entry");
    fs::copy(
        repo.join("contrib/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg"),
        icons.join(format!("{SETTINGS_ID}.svg")),
    )
    .expect("stage Settings icon");

    fs::write(&binary, "#!/bin/sh\nexit 0\n").expect("write Settings test executable");
    let mut permissions = fs::metadata(&binary).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&binary, permissions).expect("make Settings test executable");

    fs::write(
        data.join("icons/hicolor/index.theme"),
        "[Icon Theme]\n\
         Name=Test hicolor\n\
         Directories=scalable/apps\n\
         \n\
         [scalable/apps]\n\
         Size=128\n\
         Type=Scalable\n\
         MinSize=1\n\
         MaxSize=512\n\
         Context=Applications\n",
    )
    .expect("write test hicolor index");
}

fn assert_staged_settings(root: &Path) {
    let applications = aegis_desktop_entries::enumerate_with_theme_and_scale("hicolor", 1);
    let settings = applications
        .iter()
        .find(|entry| entry.id == format!("{SETTINGS_ID}.desktop"))
        .expect("staged Settings entry was not discovered");

    assert_eq!(settings.exec.as_deref(), Some("aegis-settings"));
    assert_eq!(settings.try_exec.as_deref(), Some("aegis-settings"));
    assert_eq!(settings.icon.as_deref(), Some(SETTINGS_ID));
    assert_eq!(settings.startup_wm_class.as_deref(), Some(SETTINGS_ID));
    assert_eq!(
        settings.icon_path.as_deref(),
        Some(
            root.join(format!(
                "share/icons/hicolor/scalable/apps/{SETTINGS_ID}.svg"
            ))
            .as_path()
        )
    );
}
