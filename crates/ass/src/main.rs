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

/// Persistent (level) input state carried across frames. Per-frame edges
/// (mouse pressed/released, scroll, text, key events) are *not* held here;
/// they are built fresh each frame from backend events and live only for the
/// iteration. This matches lens's contract that the host owns edge derivation
/// (see the `lens::Input` docstring) and mirrors iris's wayland host
/// `drain_input` pattern. Keeping level state separate from per-frame edges
/// guarantees a press/release edge can never leak into the next frame and
/// trigger phantom clicks in immediate-mode widgets.
#[derive(Default)]
struct InputAccumulator {
    cursor: (f32, f32),
    mouse_down: [bool; 3],
    display_size: (f32, f32),
}

impl InputAccumulator {
    /// Mirror of `lens::Input::set_mouse_down` so callers can update the
    /// level state alongside the per-frame snapshot through the same
    /// `lens::MouseButton` key.
    fn set_mouse_down(&mut self, b: lens::MouseButton, down: bool) {
        let idx = match b {
            lens::MouseButton::Left => 0,
            lens::MouseButton::Right => 1,
            lens::MouseButton::Middle => 2,
        };
        self.mouse_down[idx] = down;
    }
}

/// Dispatch a [`Command`] to the server and side-effect targets. Extracted
/// from the three mutation sources (IPC, keybindings, chrome) so the journal
/// chokepoint (ADR-0033) sees every mutation through one path.
fn apply_command(
    server: &mut ass_server::Server,
    notif_queue: &std::sync::Arc<std::sync::Mutex<ass_core::notify::NotificationQueue>>,
    quit: &mut bool,
    cmd: &ass_ipc::Command,
    ipc: &Option<ass_ipc::Server>,
    ts_mono_ms: u64,
) {
    use ass_ipc::Command;
    match cmd {
        Command::Focus { id } => server.focus_surface_by_id(*id),
        Command::Close { id } => server.close_toplevel(*id),
        Command::Move { id } => server.start_interactive_move(*id),
        Command::Cycle { forward } => server.cycle_focus(*forward),
        Command::SwitchWorkspace { dir } => server.switch_workspace(*dir),
        Command::SwitchWorkspaceTo { id } => server.switch_workspace_to(*id),
        Command::MoveToWorkspace { window, workspace } => {
            server.move_to_workspace(*window, *workspace)
        }
        Command::ToggleTiling => server.set_tiling(!server.tiling()),
        Command::Notify { summary, body, app_id } => {
            let n = notif_queue.lock().unwrap().push(
                summary.clone(),
                body.clone(),
                app_id.clone(),
                ts_mono_ms,
            );
            if let Some(s) = ipc.as_ref() {
                s.broadcast(ass_ipc::Event::Notified { notification: n });
            }
        }
        Command::DismissNotification { id } => {
            notif_queue.lock().unwrap().dismiss(*id);
        }
        Command::Quit => *quit = true,
    }
}

/// Record a mutation in the journal and push it to journal subscribers
/// (ADR-0033).
fn journal_and_broadcast(
    journal: &std::sync::Arc<std::sync::Mutex<ass_ipc::Journal>>,
    ipc: &Option<ass_ipc::Server>,
    ts_mono_ms: u64,
    origin: ass_ipc::Origin,
    cmd: ass_ipc::Command,
) {
    let mut j = journal.lock().unwrap();
    let entry = j.append(ts_mono_ms, origin, cmd, ass_ipc::Effect::Applied);
    if let Some(s) = ipc.as_ref() {
        s.broadcast_journal(entry.clone());
    }
}

fn run() -> Result<(), Box<dyn std::error::Error>> {
    log::info!(
        "ass {} — autonomous surface shell",
        env!("CARGO_PKG_VERSION")
    );

    // Notification queue (M9, over the IPC): shared between the IPC handler
    // (reads), the toast chrome component (renders), and this loop (pushes
    // on `Notify`, expires each frame). Declared early so the toast
    // component registration below can clone it.
    let notif_queue: std::sync::Arc<std::sync::Mutex<ass_core::notify::NotificationQueue>> =
        std::sync::Arc::new(std::sync::Mutex::new(ass_core::notify::NotificationQueue::new(
            5_000,
        )));

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

    // Host window + Vulkan surface. The swapchain is sized in *physical*
    // pixels (logical × output scale) so a HiDPI host maps our buffer 1:1
    // instead of upscaling it; `set_buffer_scale` tells the host the buffer is
    // pre-scaled. Chrome and client/wallpaper draws stay in logical
    // coordinates and are scaled up at draw time, keeping text and edges crisp.
    let mut host = NestedHost::open("ass", 1280, 720)?;
    let vk_surface = host.create_vk_surface(&device)?;
    let (w, h) = host.physical_size();
    log::info!(
        "nested: host window {w}x{h} (scale {}), VkSurfaceKHR created",
        host.scale()
    );

    // flux presentable surface + canvas.
    let surface = unsafe { flux::Surface::from_vk(&device, vk_surface, w, h, true) }?;
    let canvas = flux::Canvas::new(&surface)?;
    // Advertise the pre-scaled buffer to the host; takes effect on the next
    // commit (the first present below).
    host.set_buffer_scale();

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
    // Window decorations are intentionally not registered: windows are
    // borderless (macOS-style), managed through the dock, tiling, and key
    // bindings rather than per-window title bars.
    // Only the binary wires discovery to chrome (ADR-0022); the shell stays
    // free of `ass-apps`. The launcher opens from the dock's Launchpad tile
    // into a full-screen icon grid; clicking an icon spawns it detached via
    // `ass-launch`. It shares the dock's decoded icon textures.
    shell.add(Box::new(ass_shell::Launcher::with_icons(
        launcher_apps.clone(),
        icon_cache.map.clone(),
    )));
    shell.add(Box::new(ass_shell::WorkspaceBar::new()));
    shell.add(Box::new(ass_shell::Toast::new(std::sync::Arc::clone(
        &notif_queue,
    ))));
    // The dock is added after the config is loaded below, so it can read the
    // `[dock]` pinned list.
    let mut input_acc = InputAccumulator::default();
    // Seed the chrome's logical extent so widgets can lay out before the first
    // resize arrives. Updated each frame from the host size.
    {
        let sz = host.size();
        input_acc.display_size = (sz.w as f32, sz.h as f32);
    }

    // Wayland server: accept client connections on its own socket.
    let mut server = ass_server::Server::new()?;
    log::info!("server: listening on WAYLAND_DISPLAY={}", server.socket());

    // Repoint $WAYLAND_DISPLAY at this compositor's socket so children laun-
    // ched from here (the dock / launcher via `ass-launch`) connect back to
    // *us*, not the host session ass is nested in. The host connection was
    // already captured above as an fd by `NestedHost::open`, which does not
    // re-read the env var after connect, so overwriting it here is safe.
    // `ass-launch::inherit_display_env` reads this var to seed each child.
    std::env::set_var("WAYLAND_DISPLAY", server.socket());

    // Compositing of client surfaces.
    let mut renderer = ass_render::Renderer::new();
    let start = std::time::Instant::now();

    // Wallpaper: a still image (png/jpg/webp/gif/…) or a short video decoded by
    // an external ffmpeg. `$ASS_WALLPAPER` selects the image; with it unset we
    // fall back to a bundled demo wallpaper so a bare `cargo run` shows a
    // desktop rather than the bare clear colour. The default is resolved at
    // compile time relative to the crate, so it works straight from
    // `cargo run`. A missing/failed load is not fatal — the clear colour shows
    // through.
    //
    // The decode resolution is seeded from the initial *physical* host size so
    // the wallpaper is decoded at the framebuffer's true resolution; later
    // resizes GPU-scale the wallpaper on draw without re-decoding.
    const DEFAULT_WALLPAPER: &str =
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/wallpapers/default.jpg");
    let (init_w, init_h) = host.physical_size();
    let wallpaper_path = std::env::var("ASS_WALLPAPER")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| DEFAULT_WALLPAPER.to_string());
    let mut wallpaper = match ass_wallpaper::Wallpaper::from_path(&wallpaper_path, init_w, init_h) {
        Ok(w) => {
            log::info!("wallpaper: enabled ({wallpaper_path})");
            Some(w)
        }
        Err(e) => {
            log::warn!("wallpaper: load failed for {wallpaper_path}: {e}");
            None
        }
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

    // Declarative configuration (ADR-0026). One TOML file at
    // `$XDG_CONFIG_HOME/ass/config.toml` is the source of truth; absence is
    // not an error (built-in defaults apply). A malformed or
    // schema-incompatible file is logged and skipped, not fatal. Key
    // bindings are the first section; later milestones add more.
    let config_path = ass_config::default_path();
    let mut config = load_config(config_path.as_deref());

    // Global key bindings: built-in defaults overridden by the config file's
    // `[[keybind]]` entries. The deprecated `$ASS_KEYBINDS` env var is still
    // honored as a transitional override (logged) and takes precedence over
    // the file; it is removed before the desktop phase closes. `forward_input`
    // consumes a matched key before delivering it to the focused client.
    let mut keymap = build_keymap(config.as_ref());
    log::info!("keybinds: {} active", keymap.len());
    // Seed the window rules from the loaded config (ADR-0026). Re-applied on
    // each reload above.
    server.set_window_rules(
        config
            .as_ref()
            .map(|c| c.window_rules.clone())
            .unwrap_or_default(),
    );
    // Seed the tiling layout params (ADR-0024) and the focused output's
    // geometry (ADR-0028) from the config and the initial host size.
    if let Some(c) = config.as_ref() {
        server.set_layout_params(c.layout.clone().into());
    }
    let (init_w, init_h) = host.size_u32();
    server.set_output_geometry(output_geometry_from_size(init_w as i32, init_h as i32));

    // The dock: a persistent strip of pinned `.desktop` app icons (ADR-0022),
    // built from the config's `[dock] pinned` list, or auto-populated from the
    // enumerated apps that have a usable icon when no pins are configured. It
    // borrows the icon cache (which outlives the shell). Added last so it
    // stacks above the other chrome.
    let pinned = build_dock_apps(
        &launcher_apps,
        &icon_cache.map,
        config.as_ref().map(|c| c.dock.pinned.as_slice()).unwrap_or(&[]),
    );
    log::info!("dock: {} app(s) pinned", pinned.len());
    shell.add(Box::new(ass_shell::Dock::with_apps(
        pinned,
        icon_cache.map.clone(),
    )));

    // mtime-based reload watcher, polled each frame. `None` when there is no
    // default config path on this host.
    let mut reload = config_path
        .as_deref()
        .map(ass_config::ReloadWatcher::at);
    let mut quit_requested = false;

    // IPC and introspection surface (ADR-0027). A unix socket at
    // `$XDG_RUNTIME_DIR/ass.sock` serves the `query` capability over a
    // snapshot shared with the main loop via an `Arc`. Connection threads
    // read the snapshot; the main loop writes it each frame. `control`/
    // `session` commands come back through `ipc_cmd_rx` and are applied on
    // this thread. Bind failure is non-fatal so the compositor runs without
    // IPC rather than crashing. `ipc` is held to the end of `run()` so its
    // `Drop` removes the socket.
    let (ipc_cmd_tx, ipc_cmd_rx) = std::sync::mpsc::channel::<ass_ipc::Command>();
    let journal = std::sync::Arc::new(std::sync::Mutex::new(ass_ipc::Journal::default_capacity()));
    let live = std::sync::Arc::new(LiveState::new(
        ipc_cmd_tx,
        std::sync::Arc::clone(&notif_queue),
        std::sync::Arc::clone(&journal),
    ));
    let ipc: Option<ass_ipc::Server> = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) => {
            let path = std::path::PathBuf::from(d).join("ass.sock");
            match ass_ipc::Server::start(&path, std::sync::Arc::clone(&live)) {
                Ok(s) => {
                    log::info!("ipc: listening on {}", path.display());
                    Some(s)
                }
                Err(e) => {
                    log::warn!("ipc: failed to bind {}: {e}", path.display());
                    None
                }
            }
        }
        None => {
            log::warn!("ipc: $XDG_RUNTIME_DIR unset; no IPC socket");
            None
        }
    };
    // Signature of the last broadcast window set, used to detect changes.
    let mut last_win_sig: Option<Vec<(ass_core::window::WindowId, bool, Option<String>)>> = None;
    // Last broadcast workspace snapshot, used to detect model changes.
    let mut last_ws_snap: Option<ass_core::workspace::WorkspaceSnapshot> = None;

    // Last broadcast workspace snapshot, used to detect model changes.
    let mut last_ws_snap: Option<ass_core::workspace::WorkspaceSnapshot> = None;
    // Whether chrome reported a multi-frame animation in flight last frame.
    // While true the loop pumps non-blocking dispatches and renders at a
    // ~60fps cadence so the animation advances even with the pointer still;
    // once it rests the loop goes back to blocking on the host event queue.
    let mut animating = false;

    loop {
        // Choose dispatch mode: non-blocking + throttle while animating so the
        // spring/wave keeps stepping; otherwise block on the host queue for the
        // next wakeup (input, client commit, resize).
        let alive = if animating {
            // Cap the spin to ~60fps so we don't busy-burn a core. The host's
            // own frame callbacks still flow through dispatch_nonblocking.
            std::thread::sleep(std::time::Duration::from_millis(8));
            host.dispatch_nonblocking()
        } else {
            host.dispatch()
        };
        if !alive || shell.should_quit() || quit_requested {
            break;
        }
        // Hot-reload the configuration when its mtime moves (ADR-0026). One
        // `stat` per frame is cheap and keeps the reload on this loop, where
        // the keymap rebuild must happen anyway. A failed reload keeps the
        // previous configuration rather than reverting silently.
        if let Some(path) = config_path.as_deref() {
            if reload.as_mut().is_some_and(|w| w.changed(path)) {
                reload_config(path, &mut config, &mut keymap, &mut server);
            }
        }

        // Drain IPC control/session commands and apply them here on the main
        // loop — the Wayland server state is not `Send`, so connection
        // threads forward through the channel rather than touching it
        // directly. Mirrors the chrome-intent drain below (ADR-0016/0027).
        while let Ok(cmd) = ipc_cmd_rx.try_recv() {
            let ts = start.elapsed().as_millis() as u64;
            apply_command(&mut server, &notif_queue, &mut quit_requested, &cmd, &ipc, ts);
            journal_and_broadcast(
                &journal,
                &ipc,
                ts,
                ass_ipc::Origin::Ipc { conn_id: 0 },
                cmd,
            );
        }
        // Age out expired notifications once per frame.
        notif_queue
            .lock()
            .unwrap()
            .expire(start.elapsed().as_millis() as u64);

        // Process client protocol traffic.
        server.dispatch();

        // Drain backend input: forward to clients (via the server's seat) and
        // mirror into the shell's input snapshot so chrome gets first dibs on
        // clicks (e.g. the Quit button). The chrome reads the same pointer
        // position; routing priority is decided by the shell's hit-test.
        //
        // The per-frame `Input` snapshot is rebuilt from the accumulator each
        // iteration: only level state (cursor position, button-held, display
        // size) is carried in; edge flags (pressed/released/scroll/keys/text)
        // start at zero so a press/release in one frame can never bleed into
        // the next and trigger phantom clicks in immediate-mode widgets.
        let mut input = ass_shell::Input::default();
        input.set_display_size(input_acc.display_size.0, input_acc.display_size.1);
        input.set_cursor(input_acc.cursor.0, input_acc.cursor.1);
        input.set_mouse_down(lens::MouseButton::Left, input_acc.mouse_down[0]);
        input.set_mouse_down(lens::MouseButton::Right, input_acc.mouse_down[1]);
        input.set_mouse_down(lens::MouseButton::Middle, input_acc.mouse_down[2]);
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
                        input_acc.cursor = (x, y);
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
                                input_acc.set_mouse_down(b, true);
                            } else {
                                input.set_mouse_released(b, true);
                                input.set_mouse_down(b, false);
                                input_acc.set_mouse_down(b, false);
                            }
                        }
                    }
                    PointerLeave => {
                        input.set_cursor(-1.0, -1.0);
                        input_acc.cursor = (-1.0, -1.0);
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
                    // Touch events are not handled by the shell chrome yet;
                    // they route to clients via forward_input below.
                    TouchDown { .. }
                    | TouchMotion { .. }
                    | TouchUp { .. }
                    | TouchFrame
                    | TouchCancel => {}
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
                let ts = start.elapsed().as_millis() as u64;
                let origin = ass_ipc::Origin::Keybinding;
                match action {
                    Action::ToggleLauncher => shell.toggle(),
                    Action::CloseFocused => {
                        if let Some(id) = server.focused_toplevel_id() {
                            let cmd = ass_ipc::Command::Close { id };
                            apply_command(&mut server, &notif_queue, &mut quit_requested, &cmd, &ipc, ts);
                            journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                        }
                    }
                    Action::CycleFocus => {
                        let cmd = ass_ipc::Command::Cycle { forward: true };
                        apply_command(&mut server, &notif_queue, &mut quit_requested, &cmd, &ipc, ts);
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                    Action::CycleFocusBack => {
                        let cmd = ass_ipc::Command::Cycle { forward: false };
                        apply_command(&mut server, &notif_queue, &mut quit_requested, &cmd, &ipc, ts);
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                    Action::WorkspaceNext => {
                        let cmd = ass_ipc::Command::SwitchWorkspace {
                            dir: ass_core::workspace::Switch::Next,
                        };
                        apply_command(&mut server, &notif_queue, &mut quit_requested, &cmd, &ipc, ts);
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                    Action::WorkspacePrev => {
                        let cmd = ass_ipc::Command::SwitchWorkspace {
                            dir: ass_core::workspace::Switch::Prev,
                        };
                        apply_command(&mut server, &notif_queue, &mut quit_requested, &cmd, &ipc, ts);
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                    Action::ToggleTiling => {
                        let cmd = ass_ipc::Command::ToggleTiling;
                        apply_command(&mut server, &notif_queue, &mut quit_requested, &cmd, &ipc, ts);
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                    Action::Quit => {
                        let cmd = ass_ipc::Command::Quit;
                        apply_command(&mut server, &notif_queue, &mut quit_requested, &cmd, &ipc, ts);
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                }
            }
        }

        // A host resize or an output-scale change (window moved to a monitor
        // with a different scale) reports the new *logical* size. The swapchain
        // follows the physical size; layout, input, and the advertised output
        // geometry stay logical. Re-advertise the buffer scale so the host
        // keeps mapping our pre-scaled buffer 1:1.
        if let Some(sz) = host.take_resize() {
            let (pw, ph) = host.physical_size();
            surface.resize(pw, ph)?;
            host.set_buffer_scale();
            input_acc.display_size = (sz.w as f32, sz.h as f32);
            input.set_display_size(sz.w as f32, sz.h as f32);
            server.set_output_geometry(output_geometry_from_size(sz.w, sz.h));
        }

        // Apply the tiling policy to the current workspace when tiled mode is
        // on (ADR-0024). No-op when off; reconfigures only windows whose
        // target moved. The work-area is the focused output's logical rect
        // (ADR-0028) inset by the chrome's reserved edges, so tiles avoid
        // the dock (ADR-0024 chrome-aware work-area).
        server.apply_tiling(shell.reserved().inset(server.output_logical_rect()));

        match surface.begin_frame() {
            Ok(frame) => {
                canvas.begin(&frame, Some(clear))?;
                // The swapchain is in physical pixels; the wallpaper and the
                // client windows are positioned in logical coordinates (the
                // server's output space). Scale the canvas transform by the
                // output scale for that pass so logical units map onto the
                // physical framebuffer. The chrome (lens) manages its own scale
                // on render, so the transform is restored before it draws.
                let scale = host.scale() as f32;
                canvas.save();
                if scale != 1.0 {
                    canvas.scale(scale, scale);
                }
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
                canvas.restore();
                // Hand the shell a snapshot of live toplevels so the chrome's
                // window list reflects the current set. The shell reads
                // title/app_id/activated off each Window to draw its buttons.
                // The same snapshot is mirrored to the IPC (ADR-0027) so the
                // chrome and external tools read identical state, and a
                // change broadcasts `WindowsChanged` to subscribers.
                let win_snapshot = server.windows();
                let sig: Vec<(ass_core::window::WindowId, bool, Option<String>)> = win_snapshot
                    .iter()
                    .map(|w| (w.id, w.state.activated, w.title.clone()))
                    .collect();
                if last_win_sig.as_ref() != Some(&sig) {
                    last_win_sig = Some(sig);
                    if let Some(s) = ipc.as_ref() {
                        s.broadcast(ass_ipc::Event::WindowsChanged);
                    }
                }
                live.set_windows(win_snapshot.clone());
                shell.set_windows(win_snapshot);
                // Mirror the workspace snapshot and broadcast `WorkspaceChanged`
                // on any model mutation (switch, place, remove, reap).
                let ws_snap = server.workspace_snapshot();
                let ws_changed = last_ws_snap.as_ref() != Some(&ws_snap);
                live.set_workspaces(ws_snap.clone());
                shell.set_workspaces(ws_snap.clone());
                live.set_outputs(server.output_infos());
                if ws_changed {
                    last_ws_snap = Some(ws_snap);
                    if let Some(s) = ipc.as_ref() {
                        s.broadcast(ass_ipc::Event::WorkspaceChanged);
                    }
                }
                // Report the output scale so lens rasterises chrome crisply on
                // a HiDPI host; layout and input stay in logical pixels.
                shell.set_scale(scale);
                unsafe { shell.render(canvas.as_raw() as *mut _, &input)? };
                // Drain chrome interactions and forward through the apply
                // chokepoint (ADR-0033) so the journal records them.
                let ts = start.elapsed().as_millis() as u64;
                let origin = ass_ipc::Origin::Chrome;
                if let Some(id) = shell.take_clicked_window() {
                    let cmd = ass_ipc::Command::Focus { id };
                    apply_command(&mut server, &notif_queue, &mut quit_requested, &cmd, &ipc, ts);
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                if let Some(id) = shell.take_closed_window() {
                    let cmd = ass_ipc::Command::Close { id };
                    apply_command(&mut server, &notif_queue, &mut quit_requested, &cmd, &ipc, ts);
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                if let Some(id) = shell.take_move_requested() {
                    let cmd = ass_ipc::Command::Move { id };
                    apply_command(&mut server, &notif_queue, &mut quit_requested, &cmd, &ipc, ts);
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                if let Some(id) = shell.take_switch_workspace() {
                    let cmd = ass_ipc::Command::SwitchWorkspaceTo { id };
                    apply_command(&mut server, &notif_queue, &mut quit_requested, &cmd, &ipc, ts);
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                // The dock's Launchpad tile was clicked: toggle the launcher,
                // the same path as the Super-tap hotkey.
                if shell.take_toggle_launcher() {
                    shell.toggle();
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
                // Out-of-date / lost: rebuild the swapchain at the current
                // physical size.
                let (nw, nh) = host.physical_size();
                surface.resize(nw, nh)?;
            }
        }

        // Decide whether the next iteration keeps ticking (animation in
        // flight) or blocks for the next host wakeup. Read after render so a
        // freshly-started wave (cursor just entered the dock band) is caught
        // the same frame it begins.
        animating = shell.anim_pending();
    }

    log::info!("ass: window closed after {frame_count} frames");
    device.wait_idle();
    Ok(())
}

/// Load the configuration from `path`, logging diagnostics on failure.
/// `None` (no path, or a file that does not exist) means "use built-in
/// defaults" and is not an error.
fn load_config(path: Option<&std::path::Path>) -> Option<ass_config::Config> {
    let path = path?;
    match ass_config::load(path) {
        Ok(Some(c)) => {
            log::info!("config: loaded {}", path.display());
            Some(c)
        }
        Ok(None) => None,
        Err(e) => {
            match &e {
                ass_config::LoadError::Invalid { diagnostics, .. } => {
                    for d in diagnostics {
                        log::warn!("config: {d}");
                    }
                }
                _ => log::warn!("config: {e}"),
            }
            log::warn!("config: using built-in defaults");
            None
        }
    }
}

/// Re-load `path` and, on success, swap in the new config and rebuild the
/// keymap. On failure, keep the previous config and keymap.
fn reload_config(
    path: &std::path::Path,
    config: &mut Option<ass_config::Config>,
    keymap: &mut ass_core::keybind::Keymap,
    server: &mut ass_server::Server,
) {
    let apply = |config: &Option<ass_config::Config>, server: &mut ass_server::Server| {
        server.set_window_rules(
            config
                .as_ref()
                .map(|c| c.window_rules.clone())
                .unwrap_or_default(),
        );
        if let Some(c) = config.as_ref() {
            server.set_layout_params(c.layout.clone().into());
        }
    };
    match ass_config::load(path) {
        Ok(Some(new_cfg)) => {
            log::info!("config: reloaded {}", path.display());
            *config = Some(new_cfg);
            *keymap = build_keymap(config.as_ref());
            apply(config, server);
        }
        Ok(None) => {
            log::warn!("config: {} removed; reverting to defaults", path.display());
            *config = None;
            *keymap = build_keymap(config.as_ref());
            apply(config, server);
        }
        Err(e) => {
            match &e {
                ass_config::LoadError::Invalid { diagnostics, .. } => {
                    for d in diagnostics {
                        log::warn!("config: {d}");
                    }
                }
                _ => log::warn!("config: {e}"),
            }
            log::warn!("config: reload failed; keeping previous configuration");
        }
    }
}

/// Build the focused output's geometry from a host (physical) pixel size.
/// The nested backend presents at scale 1 with no transform; the DRM/KMS
/// backend (M4/M7) supplies the real mode, scale, and transform.
fn output_geometry_from_size(w: i32, h: i32) -> ass_core::output::OutputGeometry {
    ass_core::output::OutputGeometry {
        mode: ass_core::output::OutputMode {
            width: w,
            height: h,
            refresh_mhz: 0,
        },
        scale: ass_core::output::Scale::IDENTITY,
        transform: ass_core::Transform::Normal,
        logical_origin: ass_core::Point::default(),
    }
}

/// Build the active keymap from the config file's `[[keybind]]` entries,
/// layered over the built-in defaults. The deprecated `$ASS_KEYBINDS` env
/// var is honored as a transitional override that takes precedence over the
/// file (ADR-0026); it is logged and removed before the desktop phase
/// closes.
fn build_keymap(config: Option<&ass_config::Config>) -> ass_core::keybind::Keymap {
    let mut overrides: Vec<ass_core::keybind::Keybind> = Vec::new();

    // Deprecated env override — highest precedence so existing setups keep
    // working during the transition.
    if let Ok(s) = std::env::var("ASS_KEYBINDS") {
        if !s.trim().is_empty() {
            log::warn!(
                "keybind: $ASS_KEYBINDS is deprecated; move it to the \
                 `[[keybind]]` section of the config file"
            );
            let (env_binds, errs) = ass_core::keybind::Keymap::parse_overrides(&s);
            for e in &errs {
                log::warn!("keybind: {e}");
            }
            overrides.extend(env_binds);
        }
    }

    // Config-file overrides — below the env override.
    if let Some(cfg) = config {
        let (cfg_binds, errs) = cfg.resolve_keybinds();
        for e in &errs {
            log::warn!("config: {e}");
        }
        overrides.extend(cfg_binds);
    }

    if overrides.is_empty() {
        ass_core::keybind::Keymap::defaults()
    } else {
        log::info!("keybinds: {} override(s) applied", overrides.len());
        ass_core::keybind::Keymap::defaults().with_overrides(overrides)
    }
}

/// Shared live window snapshot for the IPC (ADR-0027). The main loop writes
/// the same `Vec<Window>` it hands the shell; connection threads read it.
/// `query`-capability commands never mutate, so the lock is an `RwLock` and
/// reads from several connections do not block each other. `control`/
/// `session` commands arrive through [`Handler::command`] and are forwarded
/// to the main loop via the channel the binary owns — the Wayland server
/// state is not `Send`, so connection threads must not touch it directly.
struct LiveState {
    windows: std::sync::RwLock<Vec<ass_core::window::Window>>,
    workspaces: std::sync::RwLock<ass_core::workspace::WorkspaceSnapshot>,
    outputs: std::sync::RwLock<Vec<ass_core::output::OutputInfo>>,
    notifications: std::sync::Arc<std::sync::Mutex<ass_core::notify::NotificationQueue>>,
    journal: std::sync::Arc<std::sync::Mutex<ass_ipc::Journal>>,
    commands: std::sync::Mutex<std::sync::mpsc::Sender<ass_ipc::Command>>,
}

impl LiveState {
    fn new(
        commands: std::sync::mpsc::Sender<ass_ipc::Command>,
        notifications: std::sync::Arc<std::sync::Mutex<ass_core::notify::NotificationQueue>>,
        journal: std::sync::Arc<std::sync::Mutex<ass_ipc::Journal>>,
    ) -> LiveState {
        LiveState {
            windows: std::sync::RwLock::new(Vec::new()),
            workspaces: std::sync::RwLock::new(ass_core::workspace::WorkspaceModel::new()
                .snapshot()),
            outputs: std::sync::RwLock::new(Vec::new()),
            notifications,
            journal,
            commands: std::sync::Mutex::new(commands),
        }
    }

    fn set_windows(&self, windows: Vec<ass_core::window::Window>) {
        *self.windows.write().unwrap() = windows;
    }

    fn set_workspaces(&self, snapshot: ass_core::workspace::WorkspaceSnapshot) {
        *self.workspaces.write().unwrap() = snapshot;
    }

    fn set_outputs(&self, outputs: Vec<ass_core::output::OutputInfo>) {
        *self.outputs.write().unwrap() = outputs;
    }
}

impl ass_ipc::Handler for LiveState {
    /// The socket lives in `$XDG_RUNTIME_DIR` (user-only), so every local
    /// client is the user; grant all capabilities. The capability boundary
    /// becomes load-bearing for the M10 agent phase, where a scope narrows it.
    fn policy_caps(&self) -> ass_ipc::Capabilities {
        ass_ipc::Capabilities {
            query: true,
            control: true,
            session: true,
        }
    }

    fn windows(&self) -> Vec<ass_core::window::Window> {
        self.windows.read().unwrap().clone()
    }

    fn workspaces(&self) -> ass_core::workspace::WorkspaceSnapshot {
        self.workspaces.read().unwrap().clone()
    }

    fn notifications(&self) -> Vec<ass_core::notify::Notification> {
        self.notifications.lock().unwrap().snapshot()
    }

    fn outputs(&self) -> Vec<ass_core::output::OutputInfo> {
        self.outputs.read().unwrap().clone()
    }

    fn journal_since(&self, since: u64) -> ass_ipc::JournalSnapshot {
        self.journal.lock().unwrap().since(since)
    }

    fn command(&self, cmd: ass_ipc::Command) {
        // Best-effort: a send fails only if the main loop has dropped the
        // receiver (compositor shutting down); the command is then lost,
        // which is the right outcome.
        let _ = self.commands.lock().unwrap().send(cmd);
    }

    fn resolve_scope(&self, _name: &str) -> Option<ass_ipc::Scope> {
        // ADR-0034 scope resolution from the loaded configuration. The
        // static scope model and enforcement are fully implemented; wiring
        // the config through to here is a follow-up that lands when the
        // desktop milestones preceding M10 are complete.
        None
    }
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

/// The lowercased ids an entry might be matched by: its `StartupWMClass`, the
/// desktop-file stem, and the declared icon name. These are the same keys
/// [`build_icon_cache`] files icons under, so a dock tile can both find its
/// icon and fold a running toplevel (matched by `app_id`) into itself.
fn app_keys(entry: &ass_core::app::Entry) -> Vec<String> {
    let mut keys = Vec::new();
    let mut push = |s: &str| {
        let s = s.to_ascii_lowercase();
        if !s.is_empty() && !keys.contains(&s) {
            keys.push(s);
        }
    };
    if let Some(wm) = &entry.startup_wm_class {
        push(wm);
    }
    push(entry.id.strip_suffix(".desktop").unwrap_or(&entry.id));
    if let Some(ic) = &entry.icon {
        push(ic);
    }
    keys
}

/// How many apps to auto-pin to the dock when the config pins none, so the bar
/// is populated with real XDG icons out of the box rather than empty.
const DEFAULT_PINNED_MAX: usize = 12;

/// Build the dock's pinned app list. When `pinned` names apps, each name is
/// resolved against the enumerated entries by id / desktop-stem / WM class /
/// icon name (case-insensitive), in the order given; unresolved names are
/// logged and skipped. When `pinned` is empty, the first [`DEFAULT_PINNED_MAX`]
/// apps that have a decoded icon are pinned automatically.
fn build_dock_apps(
    apps: &[ass_core::app::Entry],
    icons: &std::collections::HashMap<String, *mut std::ffi::c_void>,
    pinned: &[String],
) -> Vec<ass_shell::DockApp> {
    let make = |entry: &ass_core::app::Entry| ass_shell::DockApp {
        entry: entry.clone(),
        keys: app_keys(entry),
    };
    if pinned.is_empty() {
        return apps
            .iter()
            .filter(|e| app_keys(e).iter().any(|k| icons.contains_key(k)))
            .take(DEFAULT_PINNED_MAX)
            .map(make)
            .collect();
    }
    let mut out = Vec::with_capacity(pinned.len());
    for name in pinned {
        let want = name.to_ascii_lowercase();
        match apps.iter().find(|e| app_keys(e).iter().any(|k| *k == want)) {
            Some(e) => out.push(make(e)),
            None => log::warn!("dock: pinned app '{name}' not found among enumerated entries"),
        }
    }
    out
}

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
                // Key the texture under every id a window might report as its
                // `app_id`; the dock resolves both icons and running-window
                // matches through these same keys.
                for key in app_keys(entry) {
                    map.entry(key).or_insert(ptr);
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
