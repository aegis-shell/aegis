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

    // Compositor chrome, bound to the same device. The core host ships with
    // no chrome of its own; compose it from the components the binary wants.
    let mut shell = unsafe { ass_shell::Shell::new(device.as_raw() as *mut _) }?;
    shell.add(Box::new(ass_shell::WindowList::new()));
    shell.add(Box::new(ass_shell::Decorations::new()));
    shell.add(Box::new(ass_shell::Dock::new()));
    // Application launcher: enumerate every launchable `.desktop` entry on the
    // host once at startup and hand the snapshot to the component. The shell
    // stays free of `ass-apps`; only the binary wires discovery to chrome
    // (ADR-0022). Click a row to spawn the app detached via `ass-launch`.
    let launcher_apps = ass_apps::enumerate();
    log::info!("launcher: {} launchable applications discovered", launcher_apps.len());
    shell.add(Box::new(ass_shell::Launcher::new(launcher_apps)));
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

    while host.dispatch() && !shell.should_quit() {
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
                        // 0x112=middle) to flux-ui's MouseButton. Other buttons
                        // are dropped; the chrome only consumes these three.
                        let mapped = match button {
                            0x110 => Some(flux_ui::MouseButton::Left),
                            0x111 => Some(flux_ui::MouseButton::Right),
                            0x112 => Some(flux_ui::MouseButton::Middle),
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
            if keyboard_captured {
                let forwarded: Vec<ass_core::input::InputEvent> = events
                    .iter()
                    .copied()
                    .filter(|e| !matches!(e, ass_core::input::InputEvent::Key { .. }))
                    .collect();
                server.forward_input(&forwarded);
            } else {
                server.forward_input(&events);
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
