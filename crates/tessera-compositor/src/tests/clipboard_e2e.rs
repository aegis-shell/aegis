use crate::Server;

/// The clipboard e2e tests spawn real children that outlive their test
/// (`wl-copy` daemonizes by design), and every `Server::new` picks a
/// `wayland-N` name in the one shared `XDG_RUNTIME_DIR`. Running them in
/// parallel lets one test's server drop (and name free) while another test's
/// children are still connecting, so the socket names cross. Serialise the
/// family: each test holds this lock for its whole lifetime.
static CLIPBOARD_E2E: std::sync::Mutex<()> = std::sync::Mutex::new(());

/// wl-clipboard prefers ext-data-control when the compositor offers it and
/// then never creates its invisible 1x1 focus-helper toplevel (ADR-0133).
/// This test runs the real `wl-copy`/`wl-paste` against the real server and
/// asserts both halves of that contract: the copy/paste round-trip succeeds,
/// and no "wl-clipboard" window ever appears in the window list — so neither
/// the window switcher nor the first-map focus policy can see it.
#[test]
fn wl_clipboard_roundtrips_without_creating_a_window() {
    let _serial = CLIPBOARD_E2E
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    for binary in ["wl-copy", "wl-paste"] {
        if which(binary).is_none() {
            eprintln!("skipping: {binary} not installed");
            return;
        }
    }

    let mut server = Server::new().expect("Server::new");
    let socket = server.socket().to_owned();

    // wl-copy forks into the background after setting the selection; wait for
    // the foreground parent to exit, then keep dispatching so the background
    // child's connection settles.
    let copy = std::process::Command::new("wl-copy")
        .env("WAYLAND_DISPLAY", &socket)
        .arg("tessera-ext-data-control-probe")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn wl-copy");
    let mut copy = copy;
    let mut copy_exited = false;
    for _ in 0..4_000 {
        server.dispatch();
        if copy.try_wait().expect("poll wl-copy").is_some() {
            copy_exited = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        copy_exited,
        "wl-copy did not set the selection and return: {}",
        String::from_utf8_lossy(&copy.wait_with_output().expect("collect wl-copy").stderr)
    );

    // Settle the backgrounded wl-copy child: it must not need focus, and it
    // must not leave a toplevel behind.
    let mut saw_wl_clipboard_window = false;
    for _ in 0..2_000 {
        server.dispatch();
        saw_wl_clipboard_window |= server.all_windows().iter().any(|window| {
            window.app_id.as_deref() == Some("io.github.bugaevc.wl-clipboard")
                || window.title.as_deref() == Some("wl-clipboard")
        });
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        !saw_wl_clipboard_window,
        "wl-clipboard fell back to its invisible toplevel; ext-data-control was not used"
    );

    // Paste the selection back through the same seat.
    let mut paste = std::process::Command::new("wl-paste")
        .env("WAYLAND_DISPLAY", &socket)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn wl-paste");
    let mut paste_exited = false;
    for _ in 0..4_000 {
        server.dispatch();
        if paste.try_wait().expect("poll wl-paste").is_some() {
            paste_exited = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(paste_exited, "wl-paste did not finish within four seconds");
    let output = paste.wait_with_output().expect("collect wl-paste");
    let pasted = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "wl-paste failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        pasted.trim_end(),
        "tessera-ext-data-control-probe",
        "clipboard round-trip lost the payload"
    );
}

fn which(binary: &str) -> Option<std::path::PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(binary))
        .find(|candidate| candidate.is_file())
}

/// Compile the wl_data_device clipboard probe (tests/clipboard_probe.c).
fn clipboard_probe_binary() -> Option<std::path::PathBuf> {
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("clipboard_probe.c");
    if !source.exists() {
        eprintln!("skipping: tests/clipboard_probe.c missing");
        return None;
    }
    let out = std::env::temp_dir().join(format!("tessera-clip-probe-{}", std::process::id()));
    std::fs::create_dir_all(&out).ok()?;
    let xdg_xml = "/usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml";
    let header = out.join("xdg-shell-client.h");
    let code = out.join("xdg-shell-protocol.c");
    let bin = out.join("clipboard_probe");
    let ok = std::process::Command::new("wayland-scanner")
        .args(["client-header", xdg_xml])
        .arg(&header)
        .status()
        .ok()?
        .success()
        && std::process::Command::new("wayland-scanner")
            .args(["private-code", xdg_xml])
            .arg(&code)
            .status()
            .ok()?
            .success();
    if !ok {
        return None;
    }
    let mut build = std::process::Command::new("gcc");
    build
        .arg("-o")
        .arg(&bin)
        .arg(&source)
        .arg(&code)
        .arg("-I")
        .arg(&out)
        .args(["-lwayland-client"]);
    // Warnings are errors for the probe: a signature mismatch that compiles
    // anyway would crash or silently misbehave at dispatch time, and a
    // skipped build here would turn the e2e into a silent no-op.
    build.args(["-Werror=implicit-function-declaration", "-Wall", "-Wextra"]);
    let compiled = build.status().ok()?.success();
    if !compiled {
        panic!("clipboard probe failed to compile");
    }
    Some(bin)
}

/// The cross-family round-trip: a GUI-style client sets the selection
/// through `wl_data_device` (as every GTK/Qt app does on Ctrl+C), and a
/// clipboard manager — `wl-paste` over ext-data-control — reads it back.
///
/// This is the path where a data-control offer's `receive` must marshal the
/// `send` event for the *wl_data_source* interface: the two families use
/// different opcodes for the same logical event, and posting the wrong one
/// corrupts the source client's protocol stream instead of transferring the
/// payload. The probe logs `source-send` only when its wl_data_source
/// received a well-formed send.
#[test]
fn wl_paste_reads_a_wl_data_device_selection() {
    let _serial = CLIPBOARD_E2E
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let Some(bin) = clipboard_probe_binary() else {
        eprintln!("skipping: probe build failed");
        return;
    };
    if which("wl-paste").is_none() {
        eprintln!("skipping: wl-paste not installed");
        return;
    }

    let mut server = Server::new().expect("Server::new");
    let socket = server.socket().to_owned();

    let mut probe = std::process::Command::new(&bin)
        .env("WAYLAND_DISPLAY", &socket)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn clipboard probe");
    let stderr = probe.stderr.take().unwrap();
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let writer = std::sync::Arc::clone(&lines);
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stderr).lines() {
            match line {
                Ok(line) => writer.lock().unwrap().push(line),
                Err(_) => break,
            }
        }
    });
    let saw = |needle: &str| {
        lines
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains(needle))
    };

    // Wait for map + focus + set_selection. The probe is a first map of an
    // unseen app, so the first-map focus policy focuses it (ADR-0133).
    let mut ready = false;
    for _ in 0..4_000 {
        server.dispatch();
        if saw("selection-set") {
            ready = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        ready,
        "probe never set the selection; log:\n{}",
        lines.lock().unwrap().join("\n")
    );

    // A clipboard manager reads it back through ext-data-control.
    let mut paste = std::process::Command::new("wl-paste")
        .env("WAYLAND_DISPLAY", &socket)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn wl-paste");
    let mut paste_done = false;
    for _ in 0..4_000 {
        server.dispatch();
        if paste.try_wait().expect("poll wl-paste").is_some() {
            paste_done = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let _ = probe.kill();
    let _ = probe.wait();
    assert!(paste_done, "wl-paste did not finish; probe log:\n{}", {
        lines.lock().unwrap().join("\n")
    });
    let output = paste.wait_with_output().expect("collect wl-paste");
    assert!(
        output.status.success(),
        "wl-paste failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim_end(),
        "wl_data_device-side payload",
        "cross-family clipboard transfer lost the payload"
    );
    assert!(
        saw("source-send mime=text/plain"),
        "the wl_data_source never received a well-formed send event"
    );
}

/// The reverse cross-family round-trip: `wl-copy` (ext-data-control) sets
/// the selection while a GUI-style client holds keyboard focus, and that
/// client reads it back through `wl_data_device` — the focused app's Ctrl+V
/// path. Here the *wl_data_offer*'s `receive` must marshal `send` for an
/// *ext_data_control_source*, the mirror image of the opcode pitfall above.
#[test]
fn focused_client_reads_a_wl_copy_selection() {
    let _serial = CLIPBOARD_E2E
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let Some(bin) = clipboard_probe_binary() else {
        eprintln!("skipping: probe build failed");
        return;
    };
    if which("wl-copy").is_none() {
        eprintln!("skipping: wl-copy not installed");
        return;
    }

    let mut server = Server::new().expect("Server::new");
    let socket = server.socket().to_owned();

    // 1. The GUI client maps, takes focus, and listens on its data device.
    let mut probe = std::process::Command::new(&bin)
        .env("WAYLAND_DISPLAY", &socket)
        .arg("paste")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn clipboard probe (paste)");
    let stderr = probe.stderr.take().unwrap();
    let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let writer = std::sync::Arc::clone(&lines);
    std::thread::spawn(move || {
        use std::io::BufRead;
        for line in std::io::BufReader::new(stderr).lines() {
            match line {
                Ok(line) => writer.lock().unwrap().push(line),
                Err(_) => break,
            }
        }
    });
    let saw = |needle: &str| {
        lines
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains(needle))
    };

    let mut focused = false;
    for _ in 0..4_000 {
        server.dispatch();
        if saw("keyboard-enter") {
            focused = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        focused,
        "probe never gained focus; log:\n{}",
        lines.lock().unwrap().join("\n")
    );

    // 2. wl-copy sets the selection through ext-data-control while the GUI
    //    client is the focused one.
    let mut copy = std::process::Command::new("wl-copy")
        .env("WAYLAND_DISPLAY", &socket)
        .arg("data-control-side payload")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn wl-copy");
    let mut copy_done = false;
    for _ in 0..4_000 {
        server.dispatch();
        if copy.try_wait().expect("poll wl-copy").is_some() {
            copy_done = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(
        copy_done,
        "wl-copy did not finish: {}",
        String::from_utf8_lossy(&copy.wait_with_output().expect("collect").stderr)
    );

    // 3. The focused client must have been advertised the selection and
    //    pulled the payload through its wl_data_offer.
    let mut pasted = false;
    for _ in 0..4_000 {
        server.dispatch();
        if saw("pasted=data-control-side payload") {
            pasted = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    let _ = probe.kill();
    let _ = probe.wait();
    assert!(
        pasted,
        "focused client never read the wl-copy selection; log:\n{}",
        lines.lock().unwrap().join("\n")
    );
    assert!(
        saw("selection-received"),
        "the focused client's data device was never notified of the change"
    );
}

/// tessera has no independent primary clipboard, so `set_primary_selection`
/// must not be an alias for the regular selection: clearing the primary
/// (`wl-copy -p --clear`) must leave the user's Ctrl+C clipboard intact,
/// and copying to primary must not replace it either.
#[test]
fn primary_selection_clear_does_not_touch_the_regular_clipboard() {
    let _serial = CLIPBOARD_E2E
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    for binary in ["wl-copy", "wl-paste"] {
        if which(binary).is_none() {
            eprintln!("skipping: {binary} not installed");
            return;
        }
    }

    let mut server = Server::new().expect("Server::new");
    let socket = server.socket().to_owned();

    let run = |server: &mut Server, args: &[&str]| {
        let mut child = std::process::Command::new("wl-copy")
            .env("WAYLAND_DISPLAY", server.socket())
            .args(args)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("spawn wl-copy");
        for _ in 0..4_000 {
            server.dispatch();
            if child.try_wait().expect("poll wl-copy").is_some() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        let _ = child.wait();
    };

    // 1. Set the regular clipboard.
    run(&mut server, &["regular-payload"]);
    // 2. Clear the primary selection (a manager feature this compositor
    //    does not model). Must be a no-op on the regular clipboard.
    run(&mut server, &["-p", "--clear"]);

    // 3. The regular clipboard still holds the first payload.
    let mut paste = std::process::Command::new("wl-paste")
        .env("WAYLAND_DISPLAY", &socket)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn wl-paste");
    let mut done = false;
    for _ in 0..4_000 {
        server.dispatch();
        if paste.try_wait().expect("poll wl-paste").is_some() {
            done = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    assert!(done, "wl-paste did not finish");
    let output = paste.wait_with_output().expect("collect wl-paste");
    assert!(
        output.status.success(),
        "wl-paste failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout).trim_end(),
        "regular-payload",
        "clearing the primary selection must not clear the regular clipboard"
    );
}
