use super::*;

/// End-to-end popup probes: a tiny C client (`tests/popup_probe.c`) drives
/// real xdg_popup protocol traffic against an in-process `Server`, and the
/// test feeds it physical input through `forward_input`, asserting on the
/// wire events the client logs. Ignored by default: building the probe needs
/// gcc + wayland-scanner + wayland-client headers.
///
/// Set AEGIS_POPUP_PROBE to a prebuilt binary to skip the compile step.
fn probe_binary(tag: &str) -> Option<std::path::PathBuf> {
    let source_name = if tag == "hidpi" {
        "hidpi_probe.c"
    } else {
        "popup_probe.c"
    };
    if let Some(path) = std::env::var_os("AEGIS_POPUP_PROBE") {
        let path = std::path::PathBuf::from(path);
        return path.exists().then_some(path);
    }
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join(source_name);
    if !source.exists() {
        eprintln!("skipping: tests/popup_probe.c missing");
        return None;
    }
    let out = std::env::temp_dir().join(format!("aegis-popup-probe-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&out).ok()?;
    let xdg_xml = "/usr/share/wayland-protocols/stable/xdg-shell/xdg-shell.xml";
    let header = out.join("xdg-shell-client.h");
    let code = out.join("xdg-shell-protocol.c");
    let bin = out.join("popup_probe");
    let mut generated = vec![code.clone()];
    let scanned = std::process::Command::new("wayland-scanner")
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
    if tag == "hidpi" {
        for (xml, name) in [
            (
                "/usr/share/wayland-protocols/stable/viewporter/viewporter.xml",
                "viewporter",
            ),
            (
                "/usr/share/wayland-protocols/staging/fractional-scale/fractional-scale-v1.xml",
                "fractional-scale-v1",
            ),
        ] {
            let hdr = out.join(format!("{name}-client.h"));
            let src = out.join(format!("{name}-protocol.c"));
            let ok = std::process::Command::new("wayland-scanner")
                .args(["client-header", xml])
                .arg(&hdr)
                .status()
                .ok()?
                .success()
                && std::process::Command::new("wayland-scanner")
                    .args(["private-code", xml])
                    .arg(&src)
                    .status()
                    .ok()?
                    .success();
            if !ok {
                eprintln!("skipping: wayland-scanner failed for {name}");
                return None;
            }
            generated.push(src);
        }
    }
    if !scanned {
        eprintln!("skipping: wayland-scanner failed");
        return None;
    }
    let mut build = std::process::Command::new("gcc");
    build
        .arg("-o")
        .arg(&bin)
        .arg(&source)
        .args(&generated)
        .arg("-I")
        .arg(&out)
        .args(["-lwayland-client", "-lm"]);
    let compiled = build.status().ok()?.success();
    compiled.then_some(bin)
}

struct ProbeChild {
    child: std::process::Child,
    lines: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl ProbeChild {
    fn spawn(binary: &std::path::Path, socket: &str, args: &[&str]) -> Option<ProbeChild> {
        let mut child = std::process::Command::new(binary)
            .args(args)
            .env("WAYLAND_DISPLAY", socket)
            .env("WAYLAND_DEBUG", "client")
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .ok()?;
        let streams: Vec<Box<dyn std::io::Read + Send>> = vec![
            Box::new(child.stdout.take()?),
            Box::new(child.stderr.take()?),
        ];
        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
        for stream in streams {
            let writer = std::sync::Arc::clone(&lines);
            std::thread::spawn(move || {
                use std::io::BufRead;
                let reader = std::io::BufReader::new(stream);
                for line in reader.lines() {
                    match line {
                        Ok(line) => writer.lock().unwrap().push(line),
                        Err(_) => break,
                    }
                }
            });
        }
        Some(ProbeChild { child, lines })
    }

    fn log(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }

    fn saw(&self, needle: &str) -> bool {
        self.lines
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains(needle))
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn pump(server: &mut Server, iterations: usize) {
    for _ in 0..iterations {
        server.dispatch();
        // Emulate the presented frame: clients (Qt especially) stop painting
        // entirely when frame callbacks stall.
        server.send_frame_callbacks(server.now_ms() as u32);
        server.presentation_complete();
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
}

trait ProbeLog {
    fn saw(&self, needle: &str) -> bool;
}

impl ProbeLog for ProbeChild {
    fn saw(&self, needle: &str) -> bool {
        ProbeChild::saw(self, needle)
    }
}

fn pump_until(
    server: &mut Server,
    probe: &impl ProbeLog,
    needle: &str,
    max_iterations: usize,
) -> bool {
    for _ in 0..max_iterations {
        pump(server, 10);
        if probe.saw(needle) {
            return true;
        }
    }
    false
}

fn new_test_server() -> Option<Server> {
    new_test_server_scaled(1.0)
}

fn new_test_server_scaled(scale: f32) -> Option<Server> {
    if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
        eprintln!("skipping: XDG_RUNTIME_DIR not set");
        return None;
    }
    let mut server = Server::new().expect("Server::new");
    server.set_output_geometry(aegis_model::output::OutputGeometry {
        mode: aegis_model::output::OutputMode {
            width: 1920,
            height: 1080,
            refresh_mhz: 60_000,
        },
        scale: aegis_model::output::Scale(scale),
        ..Default::default()
    });
    Some(server)
}

fn toplevel_rect(server: &Server) -> (aegis_model::Point, aegis_model::Size) {
    let root = server
        .state
        .live_surfaces()
        .find(|surface| unsafe { !(**surface).xdg_toplevel.is_null() && (**surface).mapped })
        .expect("probe toplevel did not map");
    unsafe { ((*root).position, (*root).window.size) }
}

fn popup_rect(server: &Server) -> (aegis_model::Point, aegis_model::Size) {
    let popup = server
        .state
        .live_surfaces()
        .find(|surface| unsafe { !(**surface).xdg_popup.is_null() && (**surface).mapped })
        .expect("probe popup did not map");
    unsafe {
        let origin = surface_draw_origin(&*popup);
        (origin, surface_logical_size(&*popup))
    }
}

fn keymap() -> aegis_model::keybind::Keymap {
    aegis_model::keybind::Keymap::default()
}

/// A grabbed popup (Qt/GTK menu shape) must receive hover, the click on its
/// items, and dismiss with `popup_done` on an outside click.
#[test]
#[ignore = "needs gcc + wayland-client headers to build the probe client"]
fn grabbed_menu_popup_receives_hover_click_and_outside_dismiss() {
    let tag = "menu";
    let Some(binary) = probe_binary(tag) else {
        return;
    };
    let Some(mut server) = new_test_server() else {
        return;
    };
    let socket = server.socket().to_owned();
    let mut probe = ProbeChild::spawn(&binary, &socket, &["menu"]).expect("spawn probe");
    assert!(
        pump_until(&mut server, &probe, "toplevel-mapped", 500),
        "probe toplevel never mapped:\n{}",
        probe.log().join("\n")
    );
    let (origin, size) = toplevel_rect(&server);
    assert!(
        size.w >= 400 && size.h >= 300,
        "unexpected toplevel size {size:?}"
    );

    // Click the toplevel: the probe opens its grabbed menu popup on release.
    let click = (origin.x + 100, origin.y + 100);
    server.forward_input(
        &[
            aegis_model::input::InputEvent::pointer_move_to(click.0 as f32, click.1 as f32),
            aegis_model::input::InputEvent::PointerButton {
                button: 0x110,
                state: aegis_model::input::ButtonState::Pressed,
            },
            aegis_model::input::InputEvent::PointerButton {
                button: 0x110,
                state: aegis_model::input::ButtonState::Released,
            },
        ],
        &keymap(),
    );
    assert!(
        pump_until(&mut server, &probe, "popup-mapped", 500),
        "probe popup never mapped:\n{}",
        probe.log().join("\n")
    );
    let (popup_origin, popup_size) = popup_rect(&server);
    eprintln!("popup at {popup_origin:?} size {popup_size:?}");

    // Hover the popup's center: the client must see wl_pointer.enter on the
    // popup surface.
    let hover = (
        popup_origin.x + popup_size.w / 2,
        popup_origin.y + popup_size.h / 2,
    );
    server.forward_input(
        &[
            aegis_model::input::InputEvent::pointer_move_to(hover.0 as f32, hover.1 as f32),
            aegis_model::input::InputEvent::pointer_move_to(hover.0 as f32 + 1.0, hover.1 as f32),
        ],
        &keymap(),
    );
    assert!(
        pump_until(&mut server, &probe, "enter surface=popup", 300),
        "pointer never entered the popup:\n{}",
        probe.log().join("\n")
    );

    // Click inside the popup: press and release must be delivered while the
    // popup holds pointer focus, and must NOT dismiss the grab.
    server.forward_input(
        &[
            aegis_model::input::InputEvent::PointerButton {
                button: 0x110,
                state: aegis_model::input::ButtonState::Pressed,
            },
            aegis_model::input::InputEvent::PointerButton {
                button: 0x110,
                state: aegis_model::input::ButtonState::Released,
            },
        ],
        &keymap(),
    );
    pump(&mut server, 100);
    let log = probe.log();
    let popup_enter_index = log
        .iter()
        .rposition(|line| line.contains("enter surface=popup"))
        .expect("enter popup logged");
    let click_delivered = log[popup_enter_index..]
        .iter()
        .any(|line| line.contains("button serial=") && line.contains("state=1"));
    assert!(
        click_delivered,
        "no button press reached the grabbed popup:\n{}",
        log.join("\n")
    );
    assert!(
        !log.iter().any(|line| line.contains("popup-done")),
        "an inside click dismissed the popup:\n{}",
        log.join("\n")
    );

    // Click outside every client surface: the grab must dismiss exactly once.
    server.forward_input(
        &[
            aegis_model::input::InputEvent::pointer_move_to(10.0, 1000.0),
            aegis_model::input::InputEvent::PointerButton {
                button: 0x110,
                state: aegis_model::input::ButtonState::Pressed,
            },
            aegis_model::input::InputEvent::PointerButton {
                button: 0x110,
                state: aegis_model::input::ButtonState::Released,
            },
        ],
        &keymap(),
    );
    assert!(
        pump_until(&mut server, &probe, "popup-done", 300),
        "outside click did not dismiss the grabbed popup:\n{}",
        probe.log().join("\n")
    );
    probe.kill();
}

/// A tooltip popup anchored beside the output's right edge must slide left
/// and keep its requested width — never be resized narrower (which is what
/// visually truncates Chrome's hover tooltips).
#[test]
#[ignore = "needs gcc + wayland-client headers to build the probe client"]
fn edge_tooltip_slides_without_shrinking() {
    let tag = "tooltip";
    let Some(binary) = probe_binary(tag) else {
        return;
    };
    let Some(mut server) = new_test_server() else {
        return;
    };
    let socket = server.socket().to_owned();
    // Anchor near the output's right edge: the 300px-wide popup would extend
    // past 1920, so slide_x must pull it back with the size intact.
    let mut probe =
        ProbeChild::spawn(&binary, &socket, &["tooltip", "1800", "100"]).expect("spawn probe");
    assert!(
        pump_until(&mut server, &probe, "popup-configure", 800),
        "popup configure never arrived:\n{}",
        probe.log().join("\n")
    );
    pump(&mut server, 50);
    let configure = probe
        .log()
        .into_iter()
        .find(|line| line.contains("popup-configure"))
        .expect("configure line");
    eprintln!("tooltip configure: {configure}");
    let fields = configure
        .split_whitespace()
        .filter_map(|field| field.split_once('='))
        .collect::<std::collections::HashMap<_, _>>();
    let w: i32 = fields["w"].parse().unwrap();
    let h: i32 = fields["h"].parse().unwrap();
    let x: i32 = fields["x"].parse().unwrap();
    let (origin, _size) = toplevel_rect(&server);
    assert_eq!((w, h), (300, 40), "tooltip was resized: {configure}");
    assert!(
        origin.x + x + w <= 1920,
        "tooltip still extends past the output edge: {configure} with toplevel at {origin:?}"
    );
    probe.kill();
}

/// Real-Chrome tooltip probe: sweep the pointer across the toolbar so Chrome
/// opens its hover tooltips, then compare every popup's positioner-requested
/// size against the size the compositor configured. A narrower configure is
/// exactly the visible "tooltip text is truncated" defect.
#[test]
#[ignore = "requires flatpak com.google.Chrome and AEGIS_CHROME_E2E_PROFILE"]
fn chrome_tooltips_keep_their_requested_size() {
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
    let Some(mut server) = new_test_server() else {
        return;
    };
    // Reproduce the reporter's environment: 3072x1920 @ scale 2 (1536x960 logical).
    server.set_output_geometry(aegis_model::output::OutputGeometry {
        mode: aegis_model::output::OutputMode {
            width: 3072,
            height: 1920,
            refresh_mhz: 120_000,
        },
        scale: aegis_model::output::Scale(2.0),
        ..Default::default()
    });
    let profile_arg = format!("--user-data-dir={profile}");
    let debug_log =
        std::env::temp_dir().join(format!("aegis-chrome-tooltip-{}.log", std::process::id()));
    let debug_file = std::fs::File::create(&debug_log).expect("create debug log");
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
            "data:text/html,<title>AEGIS_TOOLTIP</title>",
        ])
        .env("WAYLAND_DISPLAY", server.socket())
        .env("WAYLAND_DEBUG", "1")
        .env("XDG_SESSION_TYPE", "wayland")
        .stdout(std::process::Stdio::null())
        .stderr(debug_file)
        .spawn()
        .expect("launch Flatpak Chrome");

    pump(&mut server, 2_500);
    let Some(root) = server
        .state
        .live_surfaces()
        .find(|surface| unsafe { !(**surface).xdg_toplevel.is_null() && (**surface).mapped })
        .map(|p| p as usize)
    else {
        let _ = chrome.kill();
        let _ = chrome.wait();
        panic!("Chrome toplevel did not map");
    };
    let (origin, size) = unsafe {
        let rec = root as *mut SurfaceRec;
        ((*rec).position, (*rec).window.size)
    };
    eprintln!("chrome window at {origin:?} size {size:?}");

    // Reproduce the reporter's layout: a maximized window puts the toolbar's
    // extension icons against the output's right edge.
    let window_id = unsafe { (*(root as *mut SurfaceRec)).window.id };
    assert!(server.set_toplevel_maximized(window_id, true));
    pump(&mut server, 400);
    let (origin, size) = unsafe {
        let rec = root as *mut SurfaceRec;
        ((*rec).position, (*rec).window.size)
    };
    eprintln!("chrome maximized at {origin:?} size {size:?}");

    // Sweep the toolbar strip slowly enough for tooltips (≈500 ms dwell).
    let keymap = aegis_model::keybind::Keymap::default();
    let y = origin.y + 64;
    let mut x = origin.x + size.w - 420;
    let mut captured = false;
    let try_capture = |server: &Server, captured: &mut bool| {
        if *captured {
            return;
        }
        let popup_open = server
            .state
            .live_surfaces()
            .any(|surface| unsafe { !(*surface).xdg_popup.is_null() && (*surface).mapped });
        if popup_open {
            *captured = true;
            let (popup_origin, popup_size) = popup_rect(server);
            let shot = std::env::temp_dir().join("aegis-tooltip-scene.ppm");
            dump_scene_ppm(server, &shot);
            eprintln!(
                "tooltip popup at {popup_origin:?} size {popup_size:?}; scene dumped to {shot:?}"
            );
        }
    };
    while x < origin.x + size.w - 10 {
        server.forward_input(
            &[aegis_model::input::InputEvent::pointer_move_to(
                x as f32, y as f32,
            )],
            &keymap,
        );
        pump(&mut server, 300);
        try_capture(&server, &mut captured);
        x += 10;
    }
    pump(&mut server, 400);
    try_capture(&server, &mut captured);

    let _ = chrome.kill();
    let _ = chrome.wait();
    pump(&mut server, 50);

    let log = std::fs::read_to_string(&debug_log).expect("read chrome wayland log");
    // WAYLAND_DEBUG line shapes (client -> server requests, server events):
    //   xdg_positioner#30.set_size(276, 30)
    //   xdg_surface#29.get_popup(new id xdg_popup#31, xdg_surface#12, xdg_positioner#30)
    //   xdg_popup#31.configure(1220, 78, 276, 30)
    fn object_id(line: &str, marker: &str) -> Option<String> {
        let at = line.find(marker)?;
        let start = at + marker.len();
        let end = line[start..].find(|c: char| !c.is_ascii_digit())? + start;
        Some(line[start..end].to_owned())
    }
    fn int_args(line: &str, from: usize) -> Vec<i32> {
        let Some(open) = line[from..].find('(') else {
            return Vec::new();
        };
        let Some(close) = line[from + open..].find(')') else {
            return Vec::new();
        };
        line[from + open + 1..from + open + close]
            .split(',')
            .map(|part| part.trim().parse::<i32>())
            .collect::<Result<Vec<_>, _>>()
            .unwrap_or_default()
    }
    let mut requested: std::collections::HashMap<String, (i32, i32)> =
        std::collections::HashMap::new();
    let mut positioner_of_popup: std::collections::HashMap<String, String> =
        std::collections::HashMap::new();
    let mut failures = Vec::new();
    for line in log.lines() {
        if line.contains(".set_size(") && line.contains("xdg_positioner#") {
            if let Some(id) = object_id(line, "xdg_positioner#") {
                let args = int_args(line, 0);
                if args.len() == 2 {
                    requested.insert(id, (args[0], args[1]));
                }
            }
        } else if line.contains(".get_popup(") && line.contains("xdg_positioner#") {
            if let (Some(popup), Some(positioner)) = (
                object_id(line, "xdg_popup#"),
                object_id(line, "xdg_positioner#"),
            ) {
                positioner_of_popup.insert(popup, positioner);
            }
        } else if line.contains(".configure(")
            && line.contains("xdg_popup#")
            && let Some(popup) = object_id(line, "xdg_popup#")
        {
            let at = line.find(".configure(").unwrap();
            let args = int_args(line, at);
            if args.len() == 4 {
                let (w, h) = (args[2], args[3]);
                if let Some(request) = positioner_of_popup
                    .get(&popup)
                    .and_then(|positioner| requested.get(positioner))
                    && (w, h) != *request
                {
                    failures.push(format!(
                        "popup#{popup}: requested {request:?}, configured ({w}, {h})"
                    ));
                }
            }
        }
    }
    for (popup, positioner) in &positioner_of_popup {
        eprintln!(
            "popup#{popup} positioner#{positioner} requested {:?}",
            requested.get(positioner)
        );
    }
    eprintln!(
        "observed {} popup configures, {} mismatches",
        positioner_of_popup.len(),
        failures.len()
    );
    assert!(
        !positioner_of_popup.is_empty(),
        "no popups observed; sweep missed every tooltip (log: {debug_log:?})"
    );
    assert!(
        failures.is_empty(),
        "popups resized by compositor:\n{}",
        failures.join("\n")
    );
}

/// Composite the physical-desktop client scene in software (nearest
/// sampling, BGRA-over-backdrop) and write a binary PPM. This mirrors what
/// the GPU renderer would show, so tests can eyeball popup rendering without
/// a display.
fn dump_scene_ppm(server: &Server, path: &std::path::Path) {
    let rect = server.state.output_geometry.logical_rect();
    let w = rect.size.w.max(1) as usize;
    let h = rect.size.h.max(1) as usize;
    // Opaque dark backdrop stands in for the wallpaper.
    let mut out = vec![0x20u8; w * h * 4];
    for px in out.chunks_exact_mut(4) {
        px[3] = 0xff;
    }
    let frames = server.client_surface_frames();
    let by_id: std::collections::HashMap<usize, &aegis_model::SurfacePixels<'_>> =
        frames.iter().map(|frame| (frame.id, frame)).collect();
    for id in server.client_surface_frame_order() {
        let Some(frame) = by_id.get(&id) else {
            continue;
        };
        let geo = &frame.geometry;
        let scale = geo.buffer_scale.max(1);
        let logical_w = (frame.width / scale).max(1);
        let logical_h = (frame.height / scale).max(1);
        let dst = geo.viewport_dst.unwrap_or(aegis_model::Size {
            w: logical_w,
            h: logical_h,
        });
        let src = geo
            .viewport_src
            .unwrap_or(aegis_model::Rect::new(0, 0, logical_w, logical_h));
        for dy in 0..dst.h.max(0) {
            let out_y = geo.position.y + dy;
            if out_y < 0 || out_y >= h as i32 {
                continue;
            }
            for dx in 0..dst.w.max(0) {
                let out_x = geo.position.x + dx;
                if out_x < 0 || out_x >= w as i32 {
                    continue;
                }
                let src_lx = src.origin.x as i64
                    + (i64::from(dx) * i64::from(src.size.w)) / i64::from(dst.w.max(1));
                let src_ly = src.origin.y as i64
                    + (i64::from(dy) * i64::from(src.size.h)) / i64::from(dst.h.max(1));
                let px =
                    ((src_lx * i64::from(scale)).clamp(0, i64::from(frame.width - 1))) as usize;
                let py =
                    ((src_ly * i64::from(scale)).clamp(0, i64::from(frame.height - 1))) as usize;
                let at = (py * frame.width as usize + px) * 4;
                if at + 3 >= frame.pixels.len() {
                    continue;
                }
                let (b, g, r, a) = (
                    frame.pixels[at] as u32,
                    frame.pixels[at + 1] as u32,
                    frame.pixels[at + 2] as u32,
                    frame.pixels[at + 3] as u32,
                );
                let dst_px = &mut out[(out_y as usize * w + out_x as usize) * 4..];
                let inv = 255 - a;
                dst_px[0] = ((r * a + u32::from(dst_px[0]) * inv) / 255) as u8;
                dst_px[1] = ((g * a + u32::from(dst_px[1]) * inv) / 255) as u8;
                dst_px[2] = ((b * a + u32::from(dst_px[2]) * inv) / 255) as u8;
                dst_px[3] = 0xff;
            }
        }
    }
    let mut ppm = format!("P6\n{w} {h}\n255\n").into_bytes();
    for px in out.chunks_exact(4) {
        ppm.extend_from_slice(&px[..3]);
    }
    std::fs::write(path, ppm).expect("write scene ppm");
}

fn qt_probe_binary(tag: &str) -> Option<std::path::PathBuf> {
    if let Some(path) = std::env::var_os("AEGIS_QT_PROBE") {
        let path = std::path::PathBuf::from(path);
        return path.exists().then_some(path);
    }
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/qt_menu_probe.cpp");
    if !source.exists() {
        eprintln!("skipping: tests/qt_menu_probe.cpp missing");
        return None;
    }
    let out = std::env::temp_dir().join(format!("aegis-qt-probe-{}-{tag}", std::process::id()));
    std::fs::create_dir_all(&out).ok()?;
    let bin = out.join("qt_menu_probe");
    let flags = std::process::Command::new("pkg-config")
        .args(["--cflags", "--libs", "Qt6Widgets"])
        .output()
        .ok()?;
    if !flags.status.success() {
        eprintln!("skipping: Qt6Widgets development files unavailable");
        return None;
    }
    let flags = String::from_utf8_lossy(&flags.stdout).into_owned();
    let status = std::process::Command::new("g++")
        .args(["-O0", "-g", "-fPIC", "-o"])
        .arg(&bin)
        .arg(&source)
        .args(flags.split_whitespace())
        .status()
        .ok()?;
    status.success().then_some(bin)
}

struct QtProbe {
    child: std::process::Child,
    lines: std::sync::Arc<std::sync::Mutex<Vec<String>>>,
}

impl QtProbe {
    fn spawn(binary: &std::path::Path, socket: &str) -> Option<QtProbe> {
        let mut child = std::process::Command::new(binary)
            .env("WAYLAND_DISPLAY", socket)
            .env("QT_QPA_PLATFORM", "wayland")
            .env("WAYLAND_DEBUG", "1")
            .env(
                "QT_LOGGING_RULES",
                "qt.qpa.wayland*=true;qt.qpa.input*=true",
            )
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .ok()?;
        let stderr = child.stderr.take()?;
        let lines = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
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
        Some(QtProbe { child, lines })
    }

    fn log(&self) -> Vec<String> {
        self.lines.lock().unwrap().clone()
    }

    fn saw(&self, needle: &str) -> bool {
        self.lines
            .lock()
            .unwrap()
            .iter()
            .any(|line| line.contains(needle))
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl ProbeLog for QtProbe {
    fn saw(&self, needle: &str) -> bool {
        QtProbe::saw(self, needle)
    }
}

/// A real Qt application must activate context-menu items: right press opens
/// the grabbed xdg_popup menu under the cursor, hover enters it, and a left
/// click on an item fires its action and dismisses the menu.
#[test]
#[ignore = "needs Qt6 development files to build the probe client"]
fn qt_context_menu_item_activates_on_click() {
    qt_context_menu_click_round(1.0, "qt-menu");
}

/// Same menu flow at the reporter's HiDPI scale: fractional-scale clients
/// commit 2x buffers behind a viewport, and the popup hit-test must still
/// line up with the painted rect.
#[test]
#[ignore = "needs Qt6 development files to build the probe client"]
fn qt_context_menu_item_activates_on_click_at_scale2() {
    qt_context_menu_click_round(2.0, "qt-menu-s2");
}

fn qt_context_menu_click_round(scale: f32, tag: &str) {
    let Some(binary) = qt_probe_binary(tag) else {
        return;
    };
    let Some(mut server) = new_test_server_scaled(scale) else {
        return;
    };
    let socket = server.socket().to_owned();
    let mut qt = QtProbe::spawn(&binary, &socket).expect("spawn qt probe");
    assert!(
        pump_until(&mut server, &qt, "PROBE-READY", 1_500),
        "Qt probe did not start:\n{}",
        qt.log().join("\n")
    );
    pump(&mut server, 300);
    let (origin, size) = toplevel_rect(&server);
    eprintln!("qt window at {origin:?} size {size:?}");

    let keymap = aegis_model::keybind::Keymap::default();
    // Right-click the window centre: Qt opens the context menu there.
    let center = (origin.x + size.w / 2, origin.y + size.h / 2);
    server.forward_input(
        &[
            aegis_model::input::InputEvent::pointer_move_to(center.0 as f32, center.1 as f32),
            aegis_model::input::InputEvent::PointerButton {
                button: 0x111,
                state: aegis_model::input::ButtonState::Pressed,
            },
            aegis_model::input::InputEvent::PointerButton {
                button: 0x111,
                state: aegis_model::input::ButtonState::Released,
            },
        ],
        &keymap,
    );
    pump(&mut server, 5_000);
    let popup = server
        .state
        .live_surfaces()
        .find(|surface| unsafe { !(**surface).xdg_popup.is_null() && (**surface).mapped });
    let Some(popup) = popup else {
        let dump = std::env::temp_dir().join(format!("aegis-qt-probe-{}.log", std::process::id()));
        std::fs::write(&dump, qt.log().join("\n")).expect("write qt log");
        qt.kill();
        panic!("Qt context menu popup never mapped (full log: {dump:?})");
    };
    let (popup_origin, popup_size) =
        unsafe { (surface_draw_origin(&*popup), surface_logical_size(&*popup)) };
    eprintln!("qt menu popup at {popup_origin:?} size {popup_size:?}");

    // Hover, then click the first item (a strip near the popup's top-left).
    let item = (popup_origin.x + 30, popup_origin.y + 12);
    server.forward_input(
        &[
            aegis_model::input::InputEvent::pointer_move_to(item.0 as f32, item.1 as f32),
            aegis_model::input::InputEvent::pointer_move_to(item.0 as f32 + 2.0, item.1 as f32),
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
    pump(&mut server, 500);
    let log = qt.log();
    let fired = log.iter().any(|line| line.contains("ACTION-CTX1-FIRED"));
    if !fired {
        let interesting: Vec<String> = log
            .iter()
            .filter(|line| {
                line.contains("xdg_popup")
                    || line.contains("wl_pointer")
                    || line.contains("ACTION")
                    || line.contains("error")
            })
            .cloned()
            .collect();
        eprintln!("Qt probe traffic:\n{}", interesting.join("\n"));
    }
    assert!(
        fired,
        "Qt context-menu item did not activate (full log lines: {})",
        log.len()
    );
    qt.kill();
}

/// Chrome's fractional-scale commit pattern, minimized: a popup whose buffer
/// is 2x its viewport destination, `buffer_scale` 1, surface-local
/// `wl_surface.damage`, alternating between two same-size buffers. Every
/// frame's pixels must reach the compositor's snapshot in full — a damage
/// mapping that forgets the fractional scale would only refresh the left
/// portion of each new buffer.
#[test]
#[ignore = "needs gcc + wayland-client headers to build the probe client"]
fn fractional_scale_popup_commits_are_copied_in_full() {
    let Some(binary) = probe_binary("hidpi") else {
        return;
    };
    let Some(mut server) = new_test_server_scaled(2.0) else {
        return;
    };
    let socket = server.socket().to_owned();
    let mut probe = ProbeChild::spawn(&binary, &socket, &["hidpi"]).expect("spawn hidpi probe");
    assert!(
        pump_until(&mut server, &probe, "frames-done", 1_500),
        "hidpi probe did not finish its frames:\n{}",
        probe.log().join("\n")
    );
    pump(&mut server, 50);
    let popup = server
        .state
        .live_surfaces()
        .find(|surface| unsafe { !(**surface).xdg_popup.is_null() && (**surface).mapped })
        .expect("hidpi popup did not map");
    let rec = unsafe { &*popup };
    eprintln!(
        "hidpi popup buffer={}x{} scale={} src={:?} dst={:?}",
        rec.width, rec.height, rec.buffer_scale, rec.viewport_src, rec.viewport_dst
    );
    assert_eq!((rec.width, rec.height), (600, 80));
    // Final frame: left half green (BGRA 00 cc 00 ff), right half blue
    // (BGRA cc 33 00 ff). Sample the middle of each half.
    let w = rec.width as usize;
    let sample = |x: usize, y: usize| -> [u8; 4] {
        let at = (y * w + x) * 4;
        rec.pixels[at..at + 4].try_into().unwrap()
    };
    let left = sample(150, 40);
    let right = sample(450, 40);
    eprintln!("left={left:02x?} right={right:02x?}");
    assert_eq!(left, [0x00, 0xcc, 0x00, 0xff], "left half lost its pixels");
    assert_eq!(
        right,
        [0xcc, 0x33, 0x00, 0xff],
        "right half never received the latest frame (damage mapped with the wrong scale?)"
    );
    probe.kill();
}

/// A grabbed popup that maps with the cursor already inside it must receive
/// `wl_pointer.enter` without waiting for motion — clients (Qt menus, Chrome
/// bubbles) anchor their pointer tracking on that enter; without it the first
/// click is delivered to the owning toplevel and the menu dismisses or
/// ignores the item.
#[test]
#[ignore = "needs gcc + wayland-client headers to build the probe client"]
fn popup_mapped_under_a_stationary_cursor_receives_enter() {
    let Some(binary) = probe_binary("menu-stationary") else {
        return;
    };
    let Some(mut server) = new_test_server() else {
        return;
    };
    let socket = server.socket().to_owned();
    let mut probe = ProbeChild::spawn(&binary, &socket, &["menu"]).expect("spawn probe");
    assert!(
        pump_until(&mut server, &probe, "toplevel-mapped", 500),
        "probe toplevel never mapped:\n{}",
        probe.log().join("\n")
    );
    let (origin, _size) = toplevel_rect(&server);
    // Click at toplevel-local (100, 100): the probe's popup anchor places the
    // popup's top-left corner exactly there.
    let click = (origin.x + 100, origin.y + 100);
    server.forward_input(
        &[
            aegis_model::input::InputEvent::pointer_move_to(click.0 as f32, click.1 as f32),
            aegis_model::input::InputEvent::PointerButton {
                button: 0x110,
                state: aegis_model::input::ButtonState::Pressed,
            },
            aegis_model::input::InputEvent::PointerButton {
                button: 0x110,
                state: aegis_model::input::ButtonState::Released,
            },
        ],
        &keymap(),
    );
    assert!(
        pump_until(&mut server, &probe, "popup-mapped", 500),
        "probe popup never mapped:\n{}",
        probe.log().join("\n")
    );
    pump(&mut server, 100);
    assert!(
        probe.saw("enter surface=popup"),
        "popup mapped under a stationary cursor never received pointer enter:\n{}",
        probe.log().join("\n")
    );
    // The first click (no motion since the menu opened) must reach the popup
    // itself, not the owning toplevel.
    server.forward_input(
        &[
            aegis_model::input::InputEvent::PointerButton {
                button: 0x110,
                state: aegis_model::input::ButtonState::Pressed,
            },
            aegis_model::input::InputEvent::PointerButton {
                button: 0x110,
                state: aegis_model::input::ButtonState::Released,
            },
        ],
        &keymap(),
    );
    pump(&mut server, 100);
    let log = probe.log();
    let enter_index = log
        .iter()
        .rposition(|line| line.contains("enter surface=popup"))
        .expect("enter popup logged");
    assert!(
        log[enter_index..]
            .iter()
            .any(|line| line.contains("button serial=") && line.contains("state=1")),
        "first click after a no-motion menu open did not reach the popup:\n{}",
        log.join("\n")
    );
    assert!(
        !log.iter().any(|line| line.contains("popup-done")),
        "the inside click dismissed the popup:\n{}",
        log.join("\n")
    );
    probe.kill();
}
