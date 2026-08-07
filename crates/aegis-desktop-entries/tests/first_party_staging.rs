use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;

const CHILD_ROOT: &str = "AEGIS_STAGED_APP_TEST_ROOT";
const STAGED_ID: &str = "org.example.Staged";

#[test]
fn staged_entry_is_discovered_through_xdg() {
    if let Some(root) = std::env::var_os(CHILD_ROOT) {
        assert_staged_entry(&PathBuf::from(root));
        return;
    }

    let temp = tempfile::tempdir().expect("create staging test directory");
    stage_entry(temp.path());

    let status = Command::new(std::env::current_exe().expect("locate integration test binary"))
        .arg("--exact")
        .arg("staged_entry_is_discovered_through_xdg")
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

fn stage_entry(root: &Path) {
    let data = root.join("share");
    let applications = data.join("applications");
    let icons = data.join("icons/hicolor/scalable/apps");
    let binary = root.join("bin/aegis-staged");

    fs::create_dir_all(&applications).expect("create applications directory");
    fs::create_dir_all(&icons).expect("create icon directory");
    fs::create_dir_all(binary.parent().unwrap()).expect("create bin directory");
    fs::create_dir_all(root.join("home")).expect("create test home");

    fs::write(
        applications.join(format!("{STAGED_ID}.desktop")),
        format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name=Staged Test App\n\
             Exec=aegis-staged\n\
             TryExec=aegis-staged\n\
             Icon={STAGED_ID}\n\
             Categories=Settings;\n\
             StartupWMClass={STAGED_ID}\n"
        ),
    )
    .expect("write staged desktop entry");
    fs::write(
        icons.join(format!("{STAGED_ID}.svg")),
        "<svg xmlns=\"http://www.w3.org/2000/svg\" width=\"128\" height=\"128\">\n\
         <rect width=\"128\" height=\"128\" fill=\"#336699\"/>\n\
         </svg>\n",
    )
    .expect("write staged icon");

    fs::write(&binary, "#!/bin/sh\nexit 0\n").expect("write staged test executable");
    let mut permissions = fs::metadata(&binary).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&binary, permissions).expect("make staged test executable");

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

fn assert_staged_entry(root: &Path) {
    let applications = aegis_desktop_entries::enumerate_with_theme_and_scale("hicolor", 1);
    let staged = applications
        .iter()
        .find(|entry| entry.id == format!("{STAGED_ID}.desktop"))
        .expect("staged entry was not discovered");

    assert_eq!(staged.exec.as_deref(), Some("aegis-staged"));
    assert_eq!(staged.try_exec.as_deref(), Some("aegis-staged"));
    assert_eq!(staged.icon.as_deref(), Some(STAGED_ID));
    assert_eq!(staged.startup_wm_class.as_deref(), Some(STAGED_ID));
    assert_eq!(
        staged.icon_path.as_deref(),
        Some(
            root.join(format!("share/icons/hicolor/scalable/apps/{STAGED_ID}.svg"))
                .as_path()
        )
    );
}
