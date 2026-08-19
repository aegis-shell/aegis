use super::*;

/// `new_with_render_caps` threads the renderer-provided format/modifier table
/// into `State`, so `dmabuf_bind` advertises the device's real sampleable
/// modifiers instead of the LINEAR-only fallback. A bogus tiling modifier on
/// XRGB8888 must survive verbatim — that is exactly what lets a GPU client
/// avoid uncompressed LINEAR buffers.
#[test]
fn new_with_render_caps_carries_dmabuf_format_table() {
    use aegis_model::dmabuf::{DRM_FORMAT_MOD_LINEAR, DRM_FORMAT_XRGB8888, DmabufFormat};

    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    const FAKE_TILE: u64 = 0x0100_0000_0000_0001;
    let formats = vec![
        DmabufFormat {
            fourcc: DRM_FORMAT_XRGB8888,
            modifiers: vec![FAKE_TILE, DRM_FORMAT_MOD_LINEAR],
        },
        DmabufFormat {
            fourcc: aegis_model::dmabuf::DRM_FORMAT_ARGB8888,
            modifiers: vec![FAKE_TILE],
        },
    ];
    let server = Server::new_with_render_caps(true, true, formats.clone())
        .expect("Server::new_with_render_caps");
    // The table is stored on State for dmabuf_bind to read at bind time.
    assert_eq!(
        server.state.dmabuf_formats.len(),
        formats.len(),
        "format table must reach State verbatim"
    );
    assert_eq!(server.state.dmabuf_formats[0].fourcc, DRM_FORMAT_XRGB8888);
    assert_eq!(
        server.state.dmabuf_formats[0].modifiers,
        vec![FAKE_TILE, DRM_FORMAT_MOD_LINEAR]
    );
}

#[test]
fn new_with_dmabuf_feedback_carries_separate_scanout_capabilities() {
    use aegis_model::dmabuf::{DRM_FORMAT_XRGB8888, DmabufFormat};

    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    const RENDER_MODIFIER: u64 = 0x0100_0000_0000_0001;
    const SCANOUT_MODIFIER: u64 = 0x0100_0000_0000_0002;
    const MAIN_DEVICE: u64 = 0x100;
    const SCANOUT_DEVICE: u64 = 0x200;
    let server = Server::new_with_dmabuf_feedback(
        true,
        true,
        vec![DmabufFormat {
            fourcc: DRM_FORMAT_XRGB8888,
            modifiers: vec![RENDER_MODIFIER, SCANOUT_MODIFIER],
        }],
        Some(MAIN_DEVICE),
        vec![DmabufFormat {
            fourcc: DRM_FORMAT_XRGB8888,
            modifiers: vec![SCANOUT_MODIFIER],
        }],
        Some(SCANOUT_DEVICE),
    )
    .expect("Server::new_with_dmabuf_feedback");

    assert_eq!(server.state.dmabuf_main_device, Some(MAIN_DEVICE));
    assert_eq!(server.state.dmabuf_scanout_device, Some(SCANOUT_DEVICE));
    assert_eq!(server.state.dmabuf_scanout_formats.len(), 1);
    assert_eq!(
        server.state.dmabuf_scanout_formats[0].modifiers,
        vec![SCANOUT_MODIFIER]
    );
}

#[test]
fn dmabuf_feedback_update_skips_semantically_identical_capabilities() {
    use aegis_model::dmabuf::{DRM_FORMAT_ARGB8888, DRM_FORMAT_XRGB8888, DmabufFormat};

    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let renderer = vec![
        DmabufFormat {
            fourcc: DRM_FORMAT_XRGB8888,
            modifiers: vec![1, 2],
        },
        DmabufFormat {
            fourcc: DRM_FORMAT_ARGB8888,
            modifiers: vec![3],
        },
    ];
    let initial_scanout = vec![
        DmabufFormat {
            fourcc: DRM_FORMAT_XRGB8888,
            modifiers: vec![2, 1],
        },
        DmabufFormat {
            fourcc: DRM_FORMAT_ARGB8888,
            modifiers: vec![3],
        },
    ];
    let mut server =
        Server::new_with_dmabuf_feedback(true, true, renderer, Some(10), initial_scanout, Some(20))
            .expect("Server::new_with_dmabuf_feedback");

    assert!(
        !server.update_dmabuf_feedback(
            vec![
                DmabufFormat {
                    fourcc: DRM_FORMAT_ARGB8888,
                    modifiers: vec![3, 3],
                },
                DmabufFormat {
                    fourcc: DRM_FORMAT_XRGB8888,
                    modifiers: vec![1, 2, 1],
                },
            ],
            Some(20),
        ),
        "enumeration order and duplicate modifiers do not change the advertised tranche"
    );
    assert!(
        server.update_dmabuf_feedback(
            vec![DmabufFormat {
                fourcc: DRM_FORMAT_XRGB8888,
                modifiers: vec![2],
            }],
            Some(20),
        ),
        "removing an advertised scanout pair must publish a new batch"
    );
}

/// linux-dmabuf v4 is only useful if a real client can consume the feedback
/// object and its memfd-backed format table without a protocol error.
#[test]
fn dmabuf_v4_feedback_roundtrips_through_wayland_info() {
    use std::os::unix::fs::MetadataExt;

    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }

    let main_device = std::fs::metadata("/dev/null")
        .expect("stat /dev/null")
        .rdev();
    let mut server =
        Server::new_with_render_caps_and_device(true, true, Vec::new(), Some(main_device))
            .expect("Server::new_with_render_caps_and_device");
    assert_eq!(server.state.dmabuf_main_device, Some(main_device));

    let mut child = match std::process::Command::new("wayland-info")
        .env("WAYLAND_DISPLAY", server.socket())
        .env("WAYLAND_DEBUG", "client")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping: wayland-info not installed");
            return;
        }
        Err(error) => panic!("could not start wayland-info: {error}"),
    };

    let mut exited = false;
    let mut feedback_updated = false;
    for _ in 0..2_000 {
        server.dispatch();
        if !feedback_updated && !server.state.dmabuf_feedback_resources.is_empty() {
            feedback_updated = true;
            assert!(server.update_dmabuf_feedback(
                vec![aegis_model::dmabuf::DmabufFormat {
                    fourcc: aegis_model::dmabuf::DRM_FORMAT_XRGB8888,
                    modifiers: vec![aegis_model::dmabuf::DRM_FORMAT_MOD_LINEAR],
                }],
                Some(main_device),
            ));
        }
        if child.try_wait().expect("poll wayland-info").is_some() {
            exited = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    if !exited {
        let _ = child.kill();
        let _ = child.wait();
        panic!("wayland-info did not finish within two seconds");
    }

    let output = child
        .wait_with_output()
        .expect("collect wayland-info output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "wayland-info failed: {stderr}");
    let dmabuf_global = stdout
        .lines()
        .find(|line| line.contains("interface: 'zwp_linux_dmabuf_v1'"));
    assert!(
        dmabuf_global.is_some_and(|line| line.contains("version:  4")),
        "linux-dmabuf v4 global missing:\n{stdout}"
    );
    assert!(
        feedback_updated,
        "wayland-info did not keep a v4 feedback subscription alive long enough to refresh"
    );
    let mut done_per_feedback = std::collections::HashMap::<&str, usize>::new();
    for line in stderr.lines() {
        let Some(start) = line.find("zwp_linux_dmabuf_feedback_v1") else {
            continue;
        };
        let object_and_event = &line[start..];
        let Some(done) = object_and_event.find(".done()") else {
            continue;
        };
        *done_per_feedback
            .entry(&object_and_event[..done])
            .or_default() += 1;
    }
    assert!(
        done_per_feedback.values().any(|count| *count >= 2),
        "the same feedback object did not consume both complete DONE rounds; counts={done_per_feedback:?}\n{stderr}"
    );
    for _ in 0..16 {
        server.dispatch();
        if server.state.dmabuf_feedback_resources.is_empty() {
            break;
        }
    }
    assert!(
        server.state.dmabuf_feedback_resources.is_empty(),
        "disconnect must destroy and untrack every live feedback resource"
    );
}

/// `Server::new` (the test/default path, with no renderer) leaves the format
/// table empty, so `dmabuf_bind` keeps advertising the four 32-bit fourccs
/// with LINEAR — every previously-working client must stay working.
#[test]
fn server_new_has_empty_dmabuf_format_table() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let server = Server::new().expect("Server::new");
    assert!(
        server.state.dmabuf_formats.is_empty(),
        "default Server::new must not fabricate modifiers"
    );
}

/// `Server::new` brings up the display, binds an auto-named socket, and
/// returns a non-empty socket name. The socket lives in `XDG_RUNTIME_DIR`
/// (libwayland's convention) and is removed by `wl_display_destroy`.
#[test]
fn server_new_creates_socket() {
    // Skip on environments without an XDG runtime dir (CI sandboxes).
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }
    let server = Server::new().expect("Server::new");
    let socket = server.socket();
    assert!(!socket.is_empty(), "socket name must not be empty");
    let path = std::env::var("XDG_RUNTIME_DIR").unwrap() + "/" + socket;
    assert!(
        std::path::Path::new(&path).exists(),
        "socket file missing: {path}"
    );
    // Drop runs destroy and should remove the socket.
    drop(server);
    assert!(
        !std::path::Path::new(&path).exists(),
        "socket file should be removed after drop: {path}"
    );
}

/// Manual browser interoperability probe. It is ignored in normal CI because
/// it requires the Flatpak Chrome installation, but it drives Chrome's real
/// Extensions bubble through this `Server` rather than mocking focus state.
/// `AEGIS_CHROME_E2E_PROFILE` must point to a disposable copy of a profile
/// containing at least one enabled extension.
#[test]
#[ignore = "requires flatpak com.google.Chrome"]
fn chrome_extensions_menu_receives_a_complete_click() {
    let Some(profile) = std::env::var_os("AEGIS_CHROME_E2E_PROFILE") else {
        eprintln!("skipping: AEGIS_CHROME_E2E_PROFILE is not set");
        return;
    };
    let profile = profile.to_string_lossy().into_owned();
    if std::env::var_os("XDG_RUNTIME_DIR").is_none()
        || !std::process::Command::new("flatpak")
            .args(["info", "com.google.Chrome"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .is_ok_and(|status| status.success())
    {
        eprintln!("skipping: Flatpak Chrome or XDG_RUNTIME_DIR unavailable");
        return;
    }

    let mut server = Server::new().expect("Server::new");
    let profile_arg = format!("--user-data-dir={profile}");
    let mut chrome = std::process::Command::new("flatpak")
        .args([
            "run",
            "com.google.Chrome",
            "--ozone-platform=wayland",
            "--disable-gpu",
            "--no-first-run",
            "--no-default-browser-check",
            "--disable-sync",
            profile_arg.as_str(),
            "--window-size=1000,700",
            "data:text/html,<title>AEGIS_ORIGINAL</title>",
        ])
        .env("WAYLAND_DISPLAY", server.socket())
        .env("XDG_SESSION_TYPE", "wayland")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("launch Flatpak Chrome");

    let pump = |server: &mut Server, iterations: usize| {
        for _ in 0..iterations {
            server.dispatch();
            server.presentation_complete();
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    };
    pump(&mut server, 2_000);

    let root = server
        .state
        .live_surfaces()
        .find(|surface| unsafe { !(**surface).xdg_toplevel.is_null() && (**surface).mapped })
        .expect("Chrome toplevel did not map");
    let original_title = unsafe { (*root).window.title.clone() };
    let (menu_x, menu_y) = unsafe {
        (
            (*root).position.x + (*root).window.size.w - 112,
            (*root).position.y + 64,
        )
    };
    let keymap = aegis_model::keybind::Keymap::default();
    let surfaces_before_menu = server
        .state
        .live_surfaces()
        .map(|surface| unsafe { (*surface).resource as usize })
        .collect::<std::collections::HashSet<_>>();
    server.forward_input(
        &[aegis_model::input::InputEvent::pointer_move_to(
            menu_x as f32,
            menu_y as f32,
        )],
        &keymap,
    );
    assert_eq!(
        server.pointer_focus_surface(),
        Some(unsafe { (*root).resource })
    );
    pump(&mut server, 100);
    server.forward_input(
        &[aegis_model::input::InputEvent::PointerButton {
            button: 0x110,
            state: aegis_model::input::ButtonState::Pressed,
        }],
        &keymap,
    );
    pump(&mut server, 100);
    server.forward_input(
        &[aegis_model::input::InputEvent::PointerButton {
            button: 0x110,
            state: aegis_model::input::ButtonState::Released,
        }],
        &keymap,
    );
    pump(&mut server, 500);

    let popup = server
        .state
        .live_surfaces()
        .filter(|surface| unsafe {
            (**surface).mapped
                && !surfaces_before_menu.contains(&((**surface).resource as usize))
                && ((**surface).parent == root || !(**surface).xdg_popup.is_null())
        })
        .last()
        .expect("Chrome Extensions surface did not map");
    unsafe {
        eprintln!(
            "Chrome Extensions surface: xdg_popup={} grabbed={} origin={:?} size={:?}",
            !(*popup).xdg_popup.is_null(),
            (*popup).popup_grabbed,
            surface_draw_origin(&*popup),
            surface_logical_size(&*popup)
        );
    }
    let (item_x, item_y) = unsafe {
        let size = surface_logical_size(&*popup);
        let origin = surface_draw_origin(&*popup);
        (origin.x + 100, origin.y + size.h - 28)
    };
    server.forward_input(
        &[
            aegis_model::input::InputEvent::pointer_move_to(item_x as f32, item_y as f32),
            aegis_model::input::InputEvent::PointerButton {
                button: 0x110,
                state: aegis_model::input::ButtonState::Pressed,
            },
            aegis_model::input::InputEvent::PointerButton {
                button: 0x110,
                state: aegis_model::input::ButtonState::Released,
            },
        ],
        &keymap,
    );
    pump(&mut server, 1_000);

    let root = unsafe { &*root };
    eprintln!(
        "Chrome title before={original_title:?} after={:?}",
        root.window.title
    );
    assert_ne!(
        root.window.title, original_title,
        "Chrome kept the original tab active, so Manage extensions was swallowed"
    );

    let _ = chrome.kill();
    let _ = chrome.wait();
}

#[test]
fn prepared_keyboard_edges_keep_physical_order_across_route_boundary() {
    let mut state = State::new(std::ptr::null_mut());
    state.keyboard = Some(keyboard::Keyboard::new().expect("compile test keymap"));
    // Avoid Server::drop: this fixture has no wl_display to destroy.
    let mut server = std::mem::ManuallyDrop::new(Server {
        state: Box::new(state),
        socket: String::new(),
        interaction_domain_portals: Vec::new(),
        epoch: std::time::Instant::now(),
    });

    // Super began on the client route in the previous backend batch.
    let super_down = server
        .prepare_keyboard_event(
            aegis_model::input::KEY_LEFTMETA,
            aegis_model::input::ButtonState::Pressed,
        )
        .expect("keyboard");
    assert!(
        super_down
            .key_char()
            .expect("ordinary modifier")
            .mods
            .has(aegis_model::input::Mods::SUPER)
    );

    // The next batch crosses ownership: Super's release still belongs to the
    // client, while a new Alt press belongs to compositor chrome. Both must
    // retain the XKB state at their physical position in that one batch.
    let super_up = server
        .prepare_keyboard_event(
            aegis_model::input::KEY_LEFTMETA,
            aegis_model::input::ButtonState::Released,
        )
        .expect("keyboard")
        .key_char()
        .expect("ordinary modifier");
    let alt_down = server
        .prepare_keyboard_event(
            aegis_model::input::KEY_LEFTALT,
            aegis_model::input::ButtonState::Pressed,
        )
        .expect("keyboard")
        .key_char()
        .expect("ordinary modifier");

    assert!(!super_up.mods.has(aegis_model::input::Mods::SUPER));
    assert!(alt_down.mods.has(aegis_model::input::Mods::ALT));
    assert!(!alt_down.mods.has(aegis_model::input::Mods::SUPER));
    assert_eq!(server.depressed_modifiers(), aegis_model::input::Mods::ALT);
}

/// Registry absence is the intentional capability signal for Primary
/// Selection. Keep the standard clipboard and host IME protocols visible
/// while guarding against an accidental reintroduction of either
/// primary-capable global.
#[test]
fn registry_exposes_clipboard_and_host_ime_protocols() {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return;
    }

    let mut server = Server::new().expect("Server::new");
    let socket = server.socket().to_owned();
    let mut child = match std::process::Command::new("wayland-info")
        .env("WAYLAND_DISPLAY", socket)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            eprintln!("skipping: wayland-info not installed");
            return;
        }
        Err(error) => panic!("could not start wayland-info: {error}"),
    };

    let mut exited = false;
    for _ in 0..2_000 {
        server.dispatch();
        if child.try_wait().expect("poll wayland-info").is_some() {
            exited = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(1));
    }
    if !exited {
        let _ = child.kill();
        let _ = child.wait();
        panic!("wayland-info did not finish within two seconds");
    }

    let output = child
        .wait_with_output()
        .expect("collect wayland-info output");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "wayland-info failed: {stderr}");
    assert!(
        stdout.contains("interface: 'wl_data_device_manager'"),
        "standard clipboard global missing:\n{stdout}"
    );
    assert!(
        stdout.contains("interface: 'ext_data_control_manager_v1'"),
        "ext-data-control clipboard-manager global missing:\n{stdout}"
    );
    assert!(
        stdout.contains("interface: 'zwp_text_input_manager_v3'"),
        "application text-input global missing:\n{stdout}"
    );
    assert!(
        stdout.contains("interface: 'zwp_input_method_manager_v2'"),
        "host input-method global missing:\n{stdout}"
    );
    assert!(
        stdout.contains("interface: 'zwp_virtual_keyboard_manager_v1'"),
        "host virtual-keyboard global missing:\n{stdout}"
    );
    assert!(
        stdout.contains("interface: 'zxdg_exporter_v2'"),
        "xdg-foreign exporter global missing:\n{stdout}"
    );
    assert!(
        stdout.contains("interface: 'zxdg_importer_v2'"),
        "xdg-foreign importer global missing:\n{stdout}"
    );
    assert!(
        !stdout.contains("interface: 'wp_presentation'"),
        "incomplete presentation feedback must not be advertised:\n{stdout}"
    );
    assert!(
        !stdout.contains("zwp_primary_selection_device_manager_v1"),
        "Primary Selection must not be advertised:\n{stdout}"
    );
}
