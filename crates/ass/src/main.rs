//! ass — autonomous surface shell.
//!
//! M0c entry point: brings up a nested host window, creates a `VkSurfaceKHR`
//! over flux's Vulkan instance, and presents a cleared frame each vsync. The
//! Wayland *server* (accepting client surfaces) lands in M1.

use ass_backend::nested::{NestedHost, DEVICE_EXTENSIONS, INSTANCE_EXTENSIONS};
use ass_backend::Backend;

fn main() {
    // Initialize before anything logs. `RUST_LOG` controls verbosity; default
    // to `info` so the bring-up sequence is visible without configuration.
    let _ = env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info"))
        .format_timestamp_secs()
        .try_init();

    if let Err(e) = run() {
        log::error!("ass: {e}");
        std::process::exit(1);
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    log::info!(
        "ass {} — autonomous surface shell",
        env!("CARGO_PKG_VERSION")
    );

    // flux device with the instance extensions a nested Wayland surface needs,
    // plus the dma-buf import extensions when the driver supports them (fall
    // back to swapchain-only if device creation rejects them).
    let mut dev_exts: Vec<&std::ffi::CStr> = DEVICE_EXTENSIONS.to_vec();
    dev_exts.extend_from_slice(&flux::DMABUF_DEVICE_EXTENSIONS);
    let device = match flux::Device::new(false, &INSTANCE_EXTENSIONS, &dev_exts) {
        Ok(d) => d,
        Err(_) => flux::Device::new(false, &INSTANCE_EXTENSIONS, &DEVICE_EXTENSIONS)?,
    };
    log::info!(
        "flux: device created (windowed); dma-buf import {}",
        if flux::dmabuf_supported(&device) {
            "supported"
        } else {
            "unavailable"
        }
    );

    // Host window + Vulkan surface.
    let mut host = NestedHost::open("ass", 1280, 720)?;
    let vk_surface = host.create_vk_surface(&device)?;
    let (w, h) = host.size_u32();
    log::info!("nested: host window {w}x{h}, VkSurfaceKHR created");

    // flux presentable surface + canvas.
    let surface = unsafe { flux::Surface::from_vk(&device, vk_surface, w, h, true) }?;
    let canvas = flux::Canvas::new(&surface)?;

    // Enumerate launchable `.desktop` entries once at startup; both the
    // launcher chrome and the dock icon cache consume the snapshot.
    let launcher_apps = ass_apps::enumerate();
    log::info!(
        "launcher: {} launchable applications discovered",
        launcher_apps.len()
    );
    // Decode each app entry's raster icon into a flux texture once, keyed by
    // every app_id the entry might run as (StartupWMClass, desktop-id stem,
    // icon name) so the dock can look a running toplevel up by its `app_id`.
    // SVG icons are skipped (no rasterizer yet) and fall back to the dock
    // glyph. The cache owns the GPU textures and must outlive the shell, so it
    // is declared before it.
    let icon_cache = build_icon_cache(&device, &launcher_apps);

    // Compositor chrome, bound to the same device. The core host ships with
    // no chrome of its own; compose it from the components the binary wants.
    let mut shell = unsafe { ass_shell::Shell::new(device.as_raw() as *mut _) }?;
    shell.add(Box::new(ass_shell::WindowList::new()));
    shell.add(Box::new(ass_shell::Decorations::new()));
    // Only the binary wires discovery to chrome (ADR-0022); the shell stays
    // free of `ass-apps`. Click a launcher row to spawn detached via
    // `ass-launch`; click a dock tile to focus / restore its window.
    shell.add(Box::new(ass_shell::Launcher::new(launcher_apps.clone())));
    shell.add(Box::new(ass_shell::Dock::with_icons(
        icon_cache.map.clone(),
    )));
    let mut input = ass_shell::Input::default();
    // Seed the chrome's logical extent so widgets can lay out before the first
    // resize arrives. Updated each frame from the host size.
    {
        let sz = host.size();
        input.set_display_size(sz.w as f32, sz.h as f32);
    }

    // Wayland server: accept client connections on its own socket.
    let mut server = ass_server::Server::new()?;
    log::info!("server: listening on WAYLAND_DISPLAY={}", server.socket());

    // Compositing of client surfaces.
    let mut renderer = ass_render::Renderer::new();
    let start = std::time::Instant::now();

    // Optional wallpaper: a still image (png/jpg/webp/gif/…) or a short
    // video decoded by an external ffmpeg. Set $ASS_WALLPAPER to a path;
    // if absent or load fails, the frame's clear colour shows through.
    // The video decode resolution is seeded from the initial host size;
    // later resizes GPU-scale the wallpaper on draw without re-decoding.
    let (init_w, init_h) = host.size_u32();
    let mut wallpaper = match std::env::var("ASS_WALLPAPER")
        .ok()
        .filter(|s| !s.is_empty())
    {
        Some(path) => match ass_wallpaper::Wallpaper::from_path(&path, init_w, init_h) {
            Ok(w) => {
                log::info!("wallpaper: enabled ({path})");
                Some(w)
            }
            Err(e) => {
                log::warn!("wallpaper: load failed for {path}: {e}");
                None
            }
        },
        None => None,
    };

    let clear = flux::rgba(30, 30, 46, 255);
    let mut frame_count: u64 = 0;

    // Global launcher hotkey: a bare Super tap (press and release with no
    // other key in between) toggles the launcher. Super still works as a
    // modifier for every other combo — only a clean tap fires. See ADR-0022.
    let mut super_tap = ass_core::input::TapDetector::super_tap();
    // Tracks the previous frame's keyboard-capture state so the main loop can
    // grab/release the keyboard on edges (launcher open/close).
    let mut prev_captured = false;

    // Global key bindings: built-in defaults plus optional `$ASS_KEYBINDS`
    // overrides (format: `super+space=launcher;super+q=close;...`). `forward_input`
    // consumes a matched key before delivering it to the focused client.
    let keymap = match std::env::var("ASS_KEYBINDS") {
        Ok(s) if !s.trim().is_empty() => {
            let (overrides, errs) = ass_core::keybind::Keymap::parse_overrides(&s);
            for e in &errs {
                log::warn!("keybind: {e}");
            }
            if overrides.is_empty() {
                ass_core::keybind::Keymap::defaults()
            } else {
                log::info!(
                    "keybind: {} override(s) from $ASS_KEYBINDS",
                    overrides.len()
                );
                ass_core::keybind::Keymap::defaults().with_overrides(overrides)
            }
        }
        _ => ass_core::keybind::Keymap::defaults(),
    };
    log::info!(
        "keybinds: {} active (set $ASS_KEYBINDS to override)",
        keymap.len()
    );
    let mut quit_requested = false;

    while host.dispatch() && !shell.should_quit() && !quit_requested {
        // Process client protocol traffic.
        server.dispatch();

        // Drain backend input: forward to clients (via the server's seat) and
        // mirror into the shell's input snapshot so chrome gets first dibs on
        // clicks (e.g. the Quit button). The chrome reads the same pointer
        // position; routing priority is decided by the shell's hit-test.
        let events = host.take_input();
        // When chrome (the launcher) captures the keyboard, key events go to
        // the chrome's search box rather than the focused client. The shell
        // reports capture state from the previous frame's render / key
        // handling, so this is stable for the whole batch.
        let keyboard_captured = shell.captures_keyboard();
        if !events.is_empty() {
            for ev in &events {
                use ass_core::input::InputEvent::*;
                match *ev {
                    PointerMotion { x, y } => {
                        input.set_cursor(x, y);
                    }
                    PointerButton { button, state } => {
                        // Map Linux BTN_* codes (0x110=left, 0x111=right,
                        // 0x112=middle) to lens's MouseButton. Other buttons
                        // are dropped; the chrome only consumes these three.
                        let mapped = match button {
                            0x110 => Some(lens::MouseButton::Left),
                            0x111 => Some(lens::MouseButton::Right),
                            0x112 => Some(lens::MouseButton::Middle),
                            _ => None,
                        };
                        if let Some(b) = mapped {
                            if state.is_pressed() {
                                input.set_mouse_pressed(b, true);
                                input.set_mouse_down(b, true);
                            } else {
                                input.set_mouse_released(b, true);
                                input.set_mouse_down(b, false);
                            }
                        }
                    }
                    PointerLeave => {
                        input.set_cursor(-1.0, -1.0);
                    }
                    Key { code, state } if keyboard_captured => {
                        // Capture: advance the server's xkb state on every key
                        // event (press and release both keep modifier tracking
                        // consistent), and feed the launcher brain only on
                        // press so typed characters are not double-counted.
                        // Captured keys are withheld from clients below.
                        if super_tap.on_key(code, state.is_pressed()) {
                            shell.toggle();
                        }
                        if let Some(kc) = server.key_char(code, state.is_pressed()) {
                            if state.is_pressed() {
                                shell.key_char(kc);
                            }
                        }
                    }
                    Key { code, state } => {
                        // Not capturing: keys forward to the client normally
                        // (below). The Super-tap detector still observes every
                        // key so a clean tap can open the launcher.
                        if super_tap.on_key(code, state.is_pressed()) {
                            shell.toggle();
                        }
                    }
                    PointerAxis { .. } => {}
                }
            }
            // Hand the events to the server for client routing after the shell
            // has seen them. The Quit button intercepts clicks when over chrome.
            // When the launcher captures the keyboard, withhold key events
            // from clients — they belong to the search box, not the focused
            // surface. Pointer events route normally in both cases.
            let actions = if keyboard_captured {
                let forwarded: Vec<ass_core::input::InputEvent> = events
                    .iter()
                    .copied()
                    .filter(|e| !matches!(e, ass_core::input::InputEvent::Key { .. }))
                    .collect();
                server.forward_input(&forwarded, &keymap)
            } else {
                server.forward_input(&events, &keymap)
            };
            // Dispatch matched global bindings. (Empty while the launcher
            // captures the keyboard — those keys went to the search box.)
            for action in actions {
                use ass_core::keybind::Action;
                match action {
                    Action::ToggleLauncher => shell.toggle(),
                    Action::CloseFocused => {
                        if let Some(id) = server.focused_toplevel_id() {
                            server.close_toplevel(id);
                        }
                    }
                    Action::CycleFocus => server.cycle_focus(true),
                    Action::CycleFocusBack => server.cycle_focus(false),
                    Action::Quit => quit_requested = true,
                }
            }
        }

        if let Some(sz) = host.take_resize() {
            surface.resize(sz.w as u32, sz.h as u32)?;
            input.set_display_size(sz.w as f32, sz.h as f32);
        }

        match surface.begin_frame() {
            Ok(frame) => {
                canvas.begin(&frame, Some(clear))?;
                // Wallpaper first (bottom-most), then client windows, then
                // the compositor chrome on top. The wallpaper is drawn at
                // the current output size so it always fills the frame;
                // resizes after load are absorbed by GPU scaling.
                if let Some(wp) = wallpaper.as_mut() {
                    let (cw, ch) = host.size_u32();
                    wp.draw(&device, &canvas, cw as f32, ch as f32);
                }
                // Client windows next. Subsurfaces split into below-parent
                // (drawn under the toplevel) and above-parent (drawn over
                // the toplevel) per `wl_subsurface.place_above` /
                // `place_below`. The renderer's per-id texture cache is
                // shared across all four lists.
                {
                    let shm = server.toplevel_frames();
                    let dmabuf = server.toplevel_dmabuf_frames();
                    let sub_shm_below = server.subsurface_frames_below();
                    let sub_shm_above = server.subsurface_frames_above();
                    let sub_dmabuf_below = server.subsurface_dmabuf_frames_below();
                    let sub_dmabuf_above = server.subsurface_dmabuf_frames_above();
                    renderer.gc(shm
                        .iter()
                        .map(|f| f.id)
                        .chain(dmabuf.iter().map(|f| f.id))
                        .chain(sub_shm_below.iter().map(|f| f.id))
                        .chain(sub_shm_above.iter().map(|f| f.id))
                        .chain(sub_dmabuf_below.iter().map(|f| f.id))
                        .chain(sub_dmabuf_above.iter().map(|f| f.id)));
                    renderer.draw_subsurfaces(&device, &canvas, &sub_shm_below);
                    renderer.draw_dmabuf_subsurfaces(&device, &canvas, &sub_dmabuf_below);
                    renderer.draw_toplevels(&device, &canvas, &shm, (0.0, 0.0));
                    renderer.draw_dmabuf_toplevels(&device, &canvas, &dmabuf, (0.0, 0.0));
                    renderer.draw_subsurfaces(&device, &canvas, &sub_shm_above);
                    renderer.draw_dmabuf_subsurfaces(&device, &canvas, &sub_dmabuf_above);
                }
                // Hand the shell a snapshot of live toplevels so the chrome's
                // window list reflects the current set. The shell reads
                // title/app_id/activated off each Window to draw its buttons.
                shell.set_windows(server.windows());
                unsafe { shell.render(canvas.as_raw() as *mut _, &input)? };
                // Drain chrome interactions and forward to the server's
                // window-management API. Each is set at most once per frame.
                if let Some(id) = shell.take_clicked_window() {
                    server.focus_surface_by_id(id);
                }
                if let Some(id) = shell.take_closed_window() {
                    server.close_toplevel(id);
                }
                if let Some(id) = shell.take_move_requested() {
                    server.start_interactive_move(id);
                }
                // Launch the application the launcher's clicked row asked for.
                // The child is detached (setsid) and inherits the Wayland/XDG
                // environment, so it connects back to this compositor and
                // survives it exiting. See ass-launch / ADR-0022.
                if let Some(entry) = shell.take_spawn() {
                    match ass_launch::launch(&entry, &ass_launch::LaunchOpts::default()) {
                        Ok(report) => {
                            log::info!("launcher: spawned {} (pid {})", entry.id, report.pid)
                        }
                        Err(e) => log::warn!("launcher: failed to spawn {}: {e}", entry.id),
                    }
                }
                // Apply keyboard-grab transitions the chrome requested this
                // frame (launcher opened or closed). Done after the intent
                // drains so a launcher "focus running app" action (which sets
                // a new keyboard focus) takes precedence over restoring the
                // pre-grab focus. The grab sends `wl_keyboard.leave` to the
                // focused client and the release sends `wl_keyboard.enter`
                // back, keeping the focused client's state consistent with the
                // capture decision. See ADR-0022.
                let captured = shell.captures_keyboard();
                if captured && !prev_captured {
                    server.grab_keyboard_focus();
                } else if !captured && prev_captured {
                    server.release_keyboard_focus();
                }
                prev_captured = captured;
                canvas.end();
                frame.submit()?;
                frame.present()?;

                // Pace clients: fire frame callbacks for this presentation.
                server.send_frame_callbacks(start.elapsed().as_millis() as u32);

                frame_count += 1;
                if frame_count == 1 {
                    log::info!("nested: first frame presented (with shell chrome)");
                }
            }
            Err(_) => {
                // Out-of-date / lost: rebuild the swapchain at the current size.
                let (nw, nh) = host.size_u32();
                surface.resize(nw, nh)?;
            }
        }
    }

    log::info!("ass: window closed after {frame_count} frames");
    device.wait_idle();
    Ok(())
}

/// Decoded application-icon textures for the dock. `_images` owns the GPU
/// textures; `map` keys raw pointers (borrowed from `_images`) by every
/// `app_id` the entry might run as. The cache must outlive the shell, which
/// holds clones of the pointers in its dock component.
struct IconCache {
    _images: Vec<flux::Image>,
    map: std::collections::HashMap<String, *mut std::ffi::c_void>,
}

/// Raster extensions the `image` crate decodes for us. SVG needs a separate
/// rasterizer (resvg / librsvg) and is a follow-up; entries whose only icon is
/// SVG fall back to the dock glyph.
const RASTER_ICON_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tiff", "ico"];

/// Decode each app entry's icon into a flux texture, keyed by every id the
/// window might report as `app_id` (StartupWMClass, the desktop-id stem, and
/// the icon name, all lowercased). The first key to claim a texture wins per
/// entry, so a texture is never double-counted.
fn build_icon_cache(device: &flux::Device, apps: &[ass_core::app::Entry]) -> IconCache {
    use std::ffi::c_void;
    let mut images: Vec<flux::Image> = Vec::new();
    let mut map: std::collections::HashMap<String, *mut c_void> = std::collections::HashMap::new();

    for entry in apps {
        let Some(path) = &entry.icon_path else {
            continue;
        };
        let ext = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| e.to_ascii_lowercase())
            .unwrap_or_default();
        if !RASTER_ICON_EXTS.contains(&ext.as_str()) {
            continue; // SVG / unknown — glyph fallback.
        }
        let Ok(decoded) = image::open(path) else {
            continue;
        };
        let rgba = decoded.to_rgba8();
        let (w, h) = rgba.dimensions();
        let mut bgra = rgba.into_raw();
        for chunk in bgra.chunks_exact_mut(4) {
            chunk.swap(0, 2); // RGBA8 -> BGRA8 (flux samples BGRA8_UNORM).
        }
        match flux::Image::from_bytes(device, w, h, flux::Format::FLUX_FORMAT_BGRA8_UNORM, &bgra) {
            Ok(img) => {
                let ptr = img.as_raw() as *mut c_void;
                if let Some(wm) = &entry.startup_wm_class {
                    if !wm.is_empty() {
                        map.entry(wm.to_ascii_lowercase()).or_insert(ptr);
                    }
                }
                let stem = entry.id.strip_suffix(".desktop").unwrap_or(&entry.id);
                if !stem.is_empty() {
                    map.entry(stem.to_ascii_lowercase()).or_insert(ptr);
                }
                if let Some(ic) = &entry.icon {
                    if !ic.is_empty() {
                        map.entry(ic.to_ascii_lowercase()).or_insert(ptr);
                    }
                }
                images.push(img);
            }
            Err(e) => log::warn!("icon: upload failed for {}: {e:?}", path.display()),
        }
    }

    log::info!("dock: {} app icon(s) decoded", images.len());
    IconCache {
        _images: images,
        map,
    }
}
