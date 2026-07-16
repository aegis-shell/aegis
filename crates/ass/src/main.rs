//! ass — autonomous surface shell.
//!
//! The process composition root: creates the nested host, Wayland server,
//! renderer, shell, wallpaper, configuration, and IPC surfaces, then runs the
//! compositor event and presentation loop.

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

/// Backdrop effects are evaluated at quarter resolution, then upsampled behind
/// the launcher. Dual-Kawase removes the lost high-frequency detail, while the
/// 16x pixel reduction bounds the cost of live 2D + 3D wallpaper capture.
const BACKDROP_DOWNSAMPLE: u32 = 4;

struct BackdropCapture {
    image: flux::Image,
    size: (u32, u32),
    format: flux::Format,
}

/// Live desktop capture used behind the full-screen application launcher.
///
/// Capture images and blur intermediates are both indexed by frame slot. A
/// slot is rewritten only after `begin_frame` has waited its fence, avoiding
/// device-wide stalls while a 3D wallpaper continues animating.
struct LauncherBackdrop {
    blur: flux::BlurFilter,
    captures: Vec<Option<BackdropCapture>>,
    was_active: bool,
    failed_session: bool,
    unsupported: bool,
}

#[derive(Clone, Copy)]
enum BackdropPlan {
    Direct,
    Capture,
}

impl LauncherBackdrop {
    fn new(device: &flux::Device) -> Result<Self, flux::Error> {
        Ok(Self {
            blur: flux::BlurFilter::new(device)?,
            captures: Vec::new(),
            was_active: false,
            failed_session: false,
            unsupported: false,
        })
    }

    fn prepare(
        &mut self,
        active: bool,
        device: &flux::Device,
        surface: &flux::Surface,
        frame: &flux::Frame<'_>,
        surface_size: (u32, u32),
    ) -> BackdropPlan {
        if !active {
            self.was_active = false;
            self.failed_session = false;
            return BackdropPlan::Direct;
        }

        let opening = !self.was_active;
        self.was_active = true;
        if opening {
            self.failed_session = false;
        }
        if self.unsupported || self.failed_session || surface_size.0 == 0 || surface_size.1 == 0 {
            return BackdropPlan::Direct;
        }
        let format = match surface.format() {
            flux::Format::FLUX_FORMAT_RGBA8_UNORM | flux::Format::FLUX_FORMAT_BGRA8_UNORM => {
                flux::Format::FLUX_FORMAT_RGBA8_UNORM
            }
            other => {
                log::warn!(
                    "launcher: realtime backdrop unavailable for surface format {other:?}; using translucent fallback"
                );
                self.unsupported = true;
                return BackdropPlan::Direct;
            }
        };

        let size = (
            surface_size.0.div_ceil(BACKDROP_DOWNSAMPLE).max(1),
            surface_size.1.div_ceil(BACKDROP_DOWNSAMPLE).max(1),
        );
        let slot = frame.index() as usize;
        if self.captures.len() <= slot {
            self.captures.resize_with(slot + 1, || None);
        }
        let target_stale = self.captures[slot]
            .as_ref()
            .is_none_or(|capture| capture.size != size || capture.format != format);
        if target_stale {
            match flux::Image::render_target(device, size.0, size.1, format) {
                Ok(image) => {
                    self.captures[slot] = Some(BackdropCapture {
                        image,
                        size,
                        format,
                    });
                }
                Err(error) => {
                    log::warn!(
                        "launcher: failed to allocate realtime backdrop target ({error}); using translucent fallback"
                    );
                    self.failed_session = true;
                    return BackdropPlan::Direct;
                }
            }
        }
        BackdropPlan::Capture
    }

    fn begin_capture(
        &mut self,
        canvas: &flux::Canvas,
        frame: &flux::Frame<'_>,
        clear: u32,
    ) -> bool {
        let Some(target) = self.target(frame) else {
            return false;
        };
        if let Err(error) = canvas.begin_target(frame, target, Some(clear)) {
            log::warn!(
                "launcher: failed to begin backdrop capture ({error}); using translucent fallback"
            );
            self.failed_session = true;
            return false;
        }
        true
    }

    fn target(&self, frame: &flux::Frame<'_>) -> Option<&flux::Image> {
        self.captures
            .get(frame.index() as usize)
            .and_then(Option::as_ref)
            .map(|capture| &capture.image)
    }

    fn capture_size(&self, frame: &flux::Frame<'_>) -> Option<(u32, u32)> {
        self.captures
            .get(frame.index() as usize)
            .and_then(Option::as_ref)
            .map(|capture| capture.size)
    }

    fn end_capture_and_blur<'backdrop>(
        &'backdrop mut self,
        canvas: &flux::Canvas,
        frame: &flux::Frame<'_>,
        sigma: f32,
    ) -> Option<flux::BlurredImage<'backdrop>> {
        canvas.end_target();
        let slot = frame.index() as usize;
        let capture = self.captures.get(slot)?.as_ref()?;
        match self.blur.apply(frame, &capture.image, sigma) {
            Ok(image) => Some(image),
            Err(error) => {
                log::warn!(
                    "launcher: realtime backdrop dispatch failed ({error}); using translucent fallback"
                );
                self.failed_session = true;
                None
            }
        }
    }
}

fn draw_wallpaper_background(
    canvas: &flux::Canvas,
    device: &flux::Device,
    wallpaper: &mut Option<ass_wallpaper::Wallpaper>,
    logical_size: (u32, u32),
    scale: f32,
) {
    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    if let Some(wallpaper) = wallpaper.as_mut() {
        wallpaper.draw(device, canvas, logical_size.0 as f32, logical_size.1 as f32);
    }
    canvas.restore();
}

fn draw_client_scene(
    canvas: &flux::Canvas,
    device: &flux::Device,
    renderer: &mut ass_render::Renderer,
    server: &ass_server::Server,
    scale: f32,
) {
    canvas.save();
    if scale != 1.0 {
        canvas.scale(scale, scale);
    }
    let shm = server.toplevel_frames();
    let dmabuf = server.toplevel_dmabuf_frames();
    let sub_shm_below = server.subsurface_frames_below();
    let sub_shm_above = server.subsurface_frames_above();
    let sub_dmabuf_below = server.subsurface_dmabuf_frames_below();
    let sub_dmabuf_above = server.subsurface_dmabuf_frames_above();
    let overlay_shm = server.overlay_frames();
    let overlay_dmabuf = server.overlay_dmabuf_frames();
    renderer.gc(shm
        .iter()
        .map(|frame| frame.id)
        .chain(dmabuf.iter().map(|frame| frame.id))
        .chain(sub_shm_below.iter().map(|frame| frame.id))
        .chain(sub_shm_above.iter().map(|frame| frame.id))
        .chain(sub_dmabuf_below.iter().map(|frame| frame.id))
        .chain(sub_dmabuf_above.iter().map(|frame| frame.id))
        .chain(overlay_shm.iter().map(|frame| frame.id))
        .chain(overlay_dmabuf.iter().map(|frame| frame.id)));
    renderer.draw_subsurfaces(device, canvas, &sub_shm_below);
    renderer.draw_dmabuf_subsurfaces(device, canvas, &sub_dmabuf_below);
    renderer.draw_toplevels(device, canvas, &shm, (0.0, 0.0));
    renderer.draw_dmabuf_toplevels(device, canvas, &dmabuf, (0.0, 0.0));
    renderer.draw_subsurfaces(device, canvas, &sub_shm_above);
    renderer.draw_dmabuf_subsurfaces(device, canvas, &sub_dmabuf_above);
    renderer.draw_toplevels(device, canvas, &overlay_shm, (0.0, 0.0));
    renderer.draw_dmabuf_toplevels(device, canvas, &overlay_dmabuf, (0.0, 0.0));
    canvas.restore();
}

/// Direct swapchain composition. A model wallpaper inserts one depth-tested
/// pass between the 2D background and client canvas draws.
#[derive(Clone, Copy)]
struct RenderGeometry {
    logical_size: (u32, u32),
    scale: f32,
}

fn draw_direct_desktop_scene(
    canvas: &flux::Canvas,
    device: &flux::Device,
    frame: &mut flux::Frame<'_>,
    wallpaper: &mut Option<ass_wallpaper::Wallpaper>,
    renderer: &mut ass_render::Renderer,
    server: &ass_server::Server,
    geometry: RenderGeometry,
) -> Result<(), flux::Error> {
    let RenderGeometry {
        logical_size,
        scale,
    } = geometry;
    draw_wallpaper_background(canvas, device, wallpaper, logical_size, scale);
    if wallpaper
        .as_ref()
        .is_some_and(|wallpaper| wallpaper.has_model())
    {
        canvas.end();
        if let Some(wallpaper) = wallpaper.as_mut() {
            wallpaper.draw_model(device, frame);
        }
        canvas.begin(frame, None)?;
    }
    draw_client_scene(canvas, device, renderer, server, scale);
    Ok(())
}

/// Dispatch an [`ass_ipc::Command`] to the server and side-effect targets. Extracted
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
        Command::Minimize { id } => server.minimize_toplevel(*id),
        Command::Close { id } => server.close_toplevel(*id),
        Command::Move { id } => server.start_interactive_move(*id),
        Command::SetWindowGeometry { id, rect } => {
            server.set_window_geometry(*id, *rect);
        }
        Command::InjectInput { .. } => {
            // Synthetic input needs shell-occlusion validation and is handled
            // beside the physical-input router in the main loop.
            debug_assert!(false, "InjectInput reached the generic command path");
        }
        Command::Cycle { forward } => server.cycle_focus(*forward),
        Command::SwitchWorkspace { dir } => server.switch_workspace(*dir),
        Command::SwitchWorkspaceTo { id } => server.switch_workspace_to(*id),
        Command::MoveToWorkspace { window, workspace } => {
            server.move_to_workspace(*window, *workspace)
        }
        Command::ToggleTiling => server.set_tiling(!server.tiling()),
        Command::Notify {
            summary,
            body,
            app_id,
        } => {
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
    journal_effect_and_broadcast(
        journal,
        ipc,
        ts_mono_ms,
        origin,
        cmd,
        ass_ipc::Effect::Applied,
    );
}

fn journal_effect_and_broadcast(
    journal: &std::sync::Arc<std::sync::Mutex<ass_ipc::Journal>>,
    ipc: &Option<ass_ipc::Server>,
    ts_mono_ms: u64,
    origin: ass_ipc::Origin,
    cmd: ass_ipc::Command,
    effect: ass_ipc::Effect,
) {
    let mut j = journal.lock().unwrap();
    let entry = j.append(ts_mono_ms, origin, cmd, effect);
    if let Some(s) = ipc.as_ref() {
        s.broadcast_journal(entry.clone());
    }
}

/// Apply one trusted Control Center mutation. Compositor-native layout changes
/// return an IPC command so they pass through the journal chokepoint; host
/// hardware controls are dispatched through their standard Linux tools.
fn apply_system_action(
    server: &mut ass_server::Server,
    notifications: &std::sync::Arc<std::sync::Mutex<ass_core::notify::NotificationQueue>>,
    status: &mut ass_shell::SystemStatus,
    action: ass_shell::SystemAction,
) -> Option<ass_ipc::Command> {
    use ass_shell::SystemAction;

    match action {
        SystemAction::ToggleMute => {
            spawn_host_command("wpctl", &["set-mute", "@DEFAULT_AUDIO_SINK@", "toggle"]);
            status.muted = !status.muted;
        }
        SystemAction::StepVolume(delta) => {
            let amount = format!(
                "{}%{}",
                delta.unsigned_abs(),
                if delta >= 0 { "+" } else { "-" }
            );
            spawn_host_command(
                "wpctl",
                &["set-volume", "@DEFAULT_AUDIO_SINK@", &amount, "-l", "1.0"],
            );
            let current = status.volume.unwrap_or(0) as i16;
            status.volume = Some((current + i16::from(delta)).clamp(0, 100) as u8);
        }
        SystemAction::SetVolume(level) => {
            let level = level.min(100);
            let amount = format!("{level}%");
            spawn_host_command(
                "wpctl",
                &["set-volume", "@DEFAULT_AUDIO_SINK@", &amount, "-l", "1.0"],
            );
            status.volume = Some(level);
        }
        SystemAction::SetBrightness(level) => {
            let level = level.clamp(1, 100);
            let amount = format!("{level}%");
            spawn_host_command("brightnessctl", &["--class=backlight", "set", &amount]);
            status.brightness = Some(level);
        }
        SystemAction::SetWifi(enabled) => {
            spawn_host_command(
                "nmcli",
                &["radio", "wifi", if enabled { "on" } else { "off" }],
            );
            status.wifi_enabled = Some(enabled);
        }
        SystemAction::SetBluetooth(enabled) => {
            spawn_host_command(
                "rfkill",
                &[if enabled { "unblock" } else { "block" }, "bluetooth"],
            );
            status.bluetooth_enabled = Some(enabled);
        }
        SystemAction::SetDoNotDisturb(enabled) => {
            notifications.lock().unwrap().set_do_not_disturb(enabled);
            status.do_not_disturb = enabled;
        }
        SystemAction::SetTiling(enabled) => {
            status.tiled = enabled;
            if server.tiling() != enabled {
                return Some(ass_ipc::Command::ToggleTiling);
            }
        }
    }
    None
}

fn spawn_host_command(program: &str, args: &[&str]) {
    let result = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(error) = result {
        log::warn!("control center: failed to start {program}: {error}");
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
        std::sync::Arc::new(std::sync::Mutex::new(
            ass_core::notify::NotificationQueue::new(5_000),
        ));

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
    let mut surface = unsafe { flux::Surface::from_vk(&device, vk_surface, w, h, true) }?;
    let canvas = flux::Canvas::new(&surface)?;
    let mut launcher_backdrop = LauncherBackdrop::new(&device)?;
    // Advertise the pre-scaled buffer to the host; takes effect on the next
    // commit (the first present below).
    host.set_buffer_scale();

    // Enumerate launchable `.desktop` entries at startup; the catalog is
    // rescanned periodically below so package installs/removals appear without
    // restarting the compositor.
    let mut icon_theme = selected_icon_theme();
    let mut icon_scale = host.scale().ceil().max(1.0) as u32;
    let mut launcher_apps = application_catalog(&icon_theme, icon_scale);
    log::info!(
        "launcher: {} launchable applications discovered (icon theme: {})",
        launcher_apps.len(),
        icon_theme
    );
    // Decode each app entry's raster icon into a flux texture once, keyed by
    // every app_id the entry might run as (StartupWMClass, desktop-id stem,
    // icon name) so the dock can look a running toplevel up by its `app_id`.
    // SVG icons are rasterized through the host's standard rsvg-convert when
    // available. The cache owns the GPU textures and must outlive the shell,
    // so it is declared before it.
    let mut icon_cache = build_icon_cache(&device, &launcher_apps, &icon_theme, icon_scale);
    let mut icon_snapshot = snapshot_icons(&launcher_apps);

    // Compositor chrome, bound to the same device. The core host ships with
    // no chrome of its own; compose it from the components the binary wants.
    let mut shell = unsafe { ass_shell::Shell::new(device.as_raw() as *mut _) }?;
    // Window decorations are intentionally not registered: windows are
    // borderless (macOS-style), managed through the dock, tiling, and key
    // bindings rather than per-window title bars.
    shell.add(Box::new(ass_shell::HudBar::with_notifications(
        std::sync::Arc::clone(&notif_queue),
        icon_cache.map.clone(),
    )));
    shell.add(Box::new(ass_shell::Toast::new(std::sync::Arc::clone(
        &notif_queue,
    ))));
    // Only the binary wires discovery to chrome (ADR-0022); the shell stays
    // free of `ass-apps`. Register the launcher after ordinary overlays so its
    // full-screen surface covers workspace/toast chrome, while the dock (added
    // last below) remains available like macOS Launchpad.
    shell.add(Box::new(ass_shell::Launcher::with_icons(
        launcher_apps.clone(),
        icon_cache.map.clone(),
    )));
    // Built-in applications share the launcher catalog with XDG entries but
    // render in-process through optics/lens. Register the backing component
    // above the launcher and ordinary chrome, while leaving the dock last.
    shell.add(Box::new(ass_shell::ControlCenter::with_icons(
        icon_cache.map.clone(),
    )));
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
    const DEFAULT_WALLPAPER: &str = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../assets/wallpapers/procedural-generation.png"
    );
    let (init_w, init_h) = host.physical_size();
    let wallpaper_override = std::env::var("ASS_WALLPAPER")
        .ok()
        .filter(|value| !value.is_empty());
    let wallpaper_path = wallpaper_override
        .clone()
        .unwrap_or_else(|| DEFAULT_WALLPAPER.to_string());
    let is_gltf = std::path::Path::new(&wallpaper_path)
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("glb"));
    let loaded = if is_gltf {
        ass_wallpaper::Wallpaper::from_gltf(&device, &surface, &wallpaper_path)
    } else {
        ass_wallpaper::Wallpaper::from_path(&wallpaper_path, init_w, init_h)
    };
    let mut wallpaper = match loaded {
        Ok(mut wallpaper) => {
            if !is_gltf {
                let model_override = std::env::var("ASS_WALLPAPER_MODEL")
                    .ok()
                    .filter(|value| !value.is_empty());
                let model_result = if let Some(path) = model_override.as_deref() {
                    wallpaper.set_model_from_gltf(&device, &surface, path)
                } else if wallpaper_override.is_none() {
                    wallpaper.set_builtin_model(&device, &surface)
                } else {
                    Ok(())
                };
                if let Err(error) = model_result {
                    log::warn!("wallpaper: 3D model disabled: {error}");
                }
            }
            log::info!("wallpaper: enabled ({wallpaper_path})");
            Some(wallpaper)
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
    server.set_output_geometry(output_geometry_from_host(
        init_w as i32,
        init_h as i32,
        host.scale(),
    ));

    // The dock: a persistent strip of pinned `.desktop` app icons (ADR-0022),
    // built from the config's `[dock] pinned` list, or auto-populated from the
    // enumerated apps that have a usable icon when no pins are configured. It
    // borrows the icon cache (which outlives the shell). Added last so it
    // stacks above the other chrome.
    let pinned = build_dock_apps(
        &launcher_apps,
        &icon_cache.map,
        config
            .as_ref()
            .map(|c| c.dock.pinned.as_slice())
            .unwrap_or(&[]),
    );
    log::info!("dock: {} app(s) pinned", pinned.len());
    shell.add(Box::new(ass_shell::Dock::with_apps(
        pinned,
        icon_cache.map.clone(),
    )));

    // One normalized status snapshot feeds both the compact HUD and the
    // built-in Control Center. Host probes run on a low-frequency cadence;
    // compositor-owned fields are refreshed immediately when they change.
    const SYSTEM_STATUS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);
    let mut system_status = ass_shell::SystemStatus::detect();
    system_status.do_not_disturb = notif_queue.lock().unwrap().do_not_disturb();
    system_status.tiled = server.tiling();
    shell.set_system_status(system_status.clone());
    let mut next_system_status_poll = std::time::Instant::now() + SYSTEM_STATUS_INTERVAL;

    // mtime-based reload watcher, polled each frame. `None` when there is no
    // default config path on this host.
    let mut reload = config_path.as_deref().map(ass_config::ReloadWatcher::at);
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
        build_ipc_scopes(config.as_ref()),
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
    // Whether chrome reported a multi-frame animation in flight last frame.
    // While true the loop pumps non-blocking dispatches and renders at a
    // ~60fps cadence so the animation advances even with the pointer still;
    // once it rests the loop goes back to blocking on the host event queue.
    let mut animating = false;
    // Pointer ownership at the end of the previous input batch. Keeping the
    // edge lets us send exactly one wl_pointer.leave when entering chrome and
    // synthesize motion before a click that returns to client content.
    let mut chrome_pointer_captured = false;
    // Synthetic pointer movement is independent of the nested host's physical
    // cursor. The next physical pointer event realigns the server before a
    // human button/axis event is delivered, preventing a click at stale
    // synthetic coordinates.
    let mut synthetic_pointer_active = false;
    let mut last_cursor_shape = 0u32;
    let mut last_cursor_hidden = false;
    // Runtime application rescan: package managers and user-created desktop
    // entries become visible in launcher/dock during a long-running session.
    const APP_RESCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
    let mut next_app_scan = std::time::Instant::now() + APP_RESCAN_INTERVAL;
    let mut previous_frame_at = std::time::Instant::now();

    loop {
        // Choose dispatch mode: non-blocking + throttle while animating so the
        // spring/wave keeps stepping; otherwise block on the host queue for the
        // next wakeup (input, client commit, resize).
        let alive = if animating {
            // Cap animation at 60fps, accounting for work already spent on
            // the previous frame. The host's own frame callbacks still flow
            // through dispatch_nonblocking.
            let frame_interval = std::time::Duration::from_micros(16_667);
            let remaining = frame_interval.saturating_sub(previous_frame_at.elapsed());
            if !remaining.is_zero() {
                std::thread::sleep(remaining);
            }
            host.dispatch_nonblocking()
        } else {
            host.dispatch_timeout(std::time::Duration::from_secs(1))
        };
        if !alive || shell.should_quit() || quit_requested {
            break;
        }
        let frame_at = std::time::Instant::now();
        let frame_dt = (frame_at - previous_frame_at)
            .as_secs_f32()
            .clamp(0.0, 1.0 / 15.0);
        previous_frame_at = frame_at;

        if frame_at >= next_system_status_poll {
            next_system_status_poll = frame_at + SYSTEM_STATUS_INTERVAL;
            let mut detected = ass_shell::SystemStatus::detect();
            detected.do_not_disturb = notif_queue.lock().unwrap().do_not_disturb();
            detected.tiled = server.tiling();
            if detected != system_status {
                system_status = detected;
                shell.set_system_status(system_status.clone());
            }
        }
        // Hot-reload the configuration when its mtime moves (ADR-0026). One
        // `stat` per frame is cheap and keeps the reload on this loop, where
        // the keymap rebuild must happen anyway. A failed reload keeps the
        // previous configuration rather than reverting silently.
        if let Some(path) = config_path.as_deref() {
            if reload.as_mut().is_some_and(|w| w.changed(path))
                && reload_config(path, &mut config, &mut keymap, &mut server)
            {
                live.set_scopes(build_ipc_scopes(config.as_ref()));
                let pinned = build_dock_apps(
                    &launcher_apps,
                    &icon_cache.map,
                    config
                        .as_ref()
                        .map(|c| c.dock.pinned.as_slice())
                        .unwrap_or(&[]),
                );
                shell.update_app_catalog(&launcher_apps, &pinned, &icon_cache.map);
            }
        }

        if std::time::Instant::now() >= next_app_scan {
            next_app_scan = std::time::Instant::now() + APP_RESCAN_INTERVAL;
            let refreshed_theme = selected_icon_theme();
            let refreshed_scale = host.scale().ceil().max(1.0) as u32;
            let refreshed = application_catalog(&refreshed_theme, refreshed_scale);
            let refreshed_snapshot = snapshot_icons(&refreshed);
            let catalog_changed = refreshed != launcher_apps;
            let icons_changed = refreshed_snapshot != icon_snapshot;
            let theme_changed = refreshed_theme != icon_theme;
            let scale_changed = refreshed_scale != icon_scale;
            if catalog_changed || icons_changed || theme_changed || scale_changed {
                log::info!(
                    "launcher: application catalog/icons changed ({} -> {}, theme {} -> {})",
                    launcher_apps.len(),
                    refreshed.len(),
                    icon_theme,
                    refreshed_theme
                );
                let refreshed_icons =
                    build_icon_cache(&device, &refreshed, &refreshed_theme, refreshed_scale);
                let pinned = build_dock_apps(
                    &refreshed,
                    &refreshed_icons.map,
                    config
                        .as_ref()
                        .map(|c| c.dock.pinned.as_slice())
                        .unwrap_or(&[]),
                );
                shell.update_app_catalog(&refreshed, &pinned, &refreshed_icons.map);
                // Components now point only at refreshed_icons; dropping the
                // old cache after the update cannot leave dangling textures.
                icon_cache = refreshed_icons;
            }
            if theme_changed && !catalog_changed && !icons_changed && !scale_changed {
                log::info!(
                    "launcher: icon theme changed ({} -> {}), resolved icons unchanged",
                    icon_theme,
                    refreshed_theme
                );
            }
            launcher_apps = refreshed;
            icon_snapshot = refreshed_snapshot;
            icon_theme = refreshed_theme;
            icon_scale = refreshed_scale;
        }

        // Drain IPC control/session commands and apply them here on the main
        // loop — the Wayland server state is not `Send`, so connection
        // threads forward through the channel rather than touching it
        // directly. Mirrors the chrome-intent drain below (ADR-0016/0027).
        let mut pending_synthetic_input = Vec::new();
        while let Ok(cmd) = ipc_cmd_rx.try_recv() {
            let ts = start.elapsed().as_millis() as u64;
            if matches!(cmd, ass_ipc::Command::InjectInput { .. }) {
                pending_synthetic_input.push((cmd, ts));
                continue;
            }
            apply_command(
                &mut server,
                &notif_queue,
                &mut quit_requested,
                &cmd,
                &ipc,
                ts,
            );
            journal_and_broadcast(&journal, &ipc, ts, ass_ipc::Origin::Ipc { conn_id: 0 }, cmd);
        }
        // Age out expired notifications once per frame.
        notif_queue
            .lock()
            .unwrap()
            .expire(start.elapsed().as_millis() as u64);

        // Process client protocol traffic.
        server.dispatch();
        for state in server.take_text_input_states() {
            host.set_text_input_state(state);
        }
        for event in host.take_text_input() {
            server.text_input_event(&event);
        }
        for event in host.take_pointer_gestures() {
            server.pointer_gesture_event(&event);
        }
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
        input.set_dt(frame_dt);
        let mut shell_scroll = (0.0_f32, 0.0_f32);
        let pointer_before = input_acc.cursor;
        let events = host.take_input();
        // When chrome (the launcher or a context menu) captures the keyboard,
        // key events go to chrome rather than the focused client. The shell
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
                        if state.is_pressed() {
                            // A pointer gesture while Super is held is a
                            // modifier drag, not a bare launcher-key tap.
                            super_tap.cancel_current();
                        }
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
                    PointerAxis { dx, dy } => {
                        shell_scroll.0 += dx;
                        shell_scroll.1 += dy;
                    }
                    // Touch events are not handled by the shell chrome yet;
                    // they route to clients via forward_input below.
                    TouchDown { .. }
                    | TouchMotion { .. }
                    | TouchUp { .. }
                    | TouchFrame
                    | TouchCancel => {}
                }
            }
            // Route the batch a second time with compositor overlays removed
            // from the client stream. Pointer motion into chrome becomes one
            // leave; buttons and scroll are consumed until the pointer exits.
            // This prevents a dock/workspace/launcher click from also clicking
            // the client window visually underneath it.
            let display = input_acc.display_size;
            let mut route_cursor = pointer_before;
            let mut forwarded = Vec::with_capacity(events.len() + 1);
            for ev in events.iter().copied() {
                use ass_core::input::InputEvent::*;
                match ev {
                    Key { .. } if keyboard_captured => {}
                    PointerMotion { x, y } => {
                        synthetic_pointer_active = false;
                        route_cursor = (x, y);
                        let captured = shell.captures_pointer_at(x, y, display);
                        if captured {
                            if !chrome_pointer_captured {
                                forwarded.push(PointerLeave);
                            }
                            // A title-bar move or edge resize begins after
                            // chrome handles the press. Once active, pointer
                            // motion still has to reach the server even while
                            // the cursor remains inside that chrome region.
                            if server.interactive().is_some() || server.drag_active() {
                                forwarded.push(ev);
                            }
                        } else {
                            forwarded.push(ev);
                        }
                        chrome_pointer_captured = captured;
                    }
                    PointerButton { state, .. } => {
                        let captured =
                            shell.captures_pointer_at(route_cursor.0, route_cursor.1, display);
                        if synthetic_pointer_active {
                            if !captured {
                                forwarded.push(PointerMotion {
                                    x: route_cursor.0,
                                    y: route_cursor.1,
                                });
                            }
                            synthetic_pointer_active = false;
                        }
                        if captured {
                            if !chrome_pointer_captured {
                                forwarded.push(PointerLeave);
                            }
                            // Chrome-initiated move/resize grabs still need a
                            // release edge to terminate even though ordinary
                            // clicks over the overlay are consumed.
                            if !state.is_pressed()
                                && (server.interactive().is_some() || server.drag_active())
                            {
                                forwarded.push(ev);
                            }
                        } else {
                            // A button/axis can be the first event after an
                            // overlay closes. Re-establish client focus before
                            // forwarding it because the enter-side motion was
                            // consumed while chrome owned the pointer.
                            if chrome_pointer_captured {
                                forwarded.push(PointerMotion {
                                    x: route_cursor.0,
                                    y: route_cursor.1,
                                });
                            }
                            forwarded.push(ev);
                        }
                        chrome_pointer_captured = captured;
                    }
                    PointerAxis { .. } => {
                        let captured =
                            shell.captures_pointer_at(route_cursor.0, route_cursor.1, display);
                        if synthetic_pointer_active {
                            if !captured {
                                forwarded.push(PointerMotion {
                                    x: route_cursor.0,
                                    y: route_cursor.1,
                                });
                            }
                            synthetic_pointer_active = false;
                        }
                        if captured {
                            if !chrome_pointer_captured {
                                forwarded.push(PointerLeave);
                            }
                        } else {
                            if chrome_pointer_captured {
                                forwarded.push(PointerMotion {
                                    x: route_cursor.0,
                                    y: route_cursor.1,
                                });
                            }
                            forwarded.push(ev);
                        }
                        chrome_pointer_captured = captured;
                    }
                    PointerLeave => {
                        synthetic_pointer_active = false;
                        route_cursor = (-1.0, -1.0);
                        chrome_pointer_captured = false;
                        forwarded.push(PointerLeave);
                    }
                    TouchDown { x, y, .. } if synthetic_pointer_active => {
                        // Touch delivery shares the server's pointer focus.
                        // Re-hit-test at the physical contact before routing
                        // the down event after a synthetic pointer move.
                        synthetic_pointer_active = false;
                        forwarded.push(PointerMotion { x, y });
                        forwarded.push(ev);
                    }
                    _ => forwarded.push(ev),
                }
            }
            let actions = server.forward_input(&forwarded, &keymap);
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
                            apply_command(
                                &mut server,
                                &notif_queue,
                                &mut quit_requested,
                                &cmd,
                                &ipc,
                                ts,
                            );
                            journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                        }
                    }
                    Action::CycleFocus => {
                        let cmd = ass_ipc::Command::Cycle { forward: true };
                        apply_command(
                            &mut server,
                            &notif_queue,
                            &mut quit_requested,
                            &cmd,
                            &ipc,
                            ts,
                        );
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                    Action::CycleFocusBack => {
                        let cmd = ass_ipc::Command::Cycle { forward: false };
                        apply_command(
                            &mut server,
                            &notif_queue,
                            &mut quit_requested,
                            &cmd,
                            &ipc,
                            ts,
                        );
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                    Action::WorkspaceNext => {
                        let cmd = ass_ipc::Command::SwitchWorkspace {
                            dir: ass_core::workspace::Switch::Next,
                        };
                        apply_command(
                            &mut server,
                            &notif_queue,
                            &mut quit_requested,
                            &cmd,
                            &ipc,
                            ts,
                        );
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                    Action::WorkspacePrev => {
                        let cmd = ass_ipc::Command::SwitchWorkspace {
                            dir: ass_core::workspace::Switch::Prev,
                        };
                        apply_command(
                            &mut server,
                            &notif_queue,
                            &mut quit_requested,
                            &cmd,
                            &ipc,
                            ts,
                        );
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                    Action::ToggleTiling => {
                        let cmd = ass_ipc::Command::ToggleTiling;
                        apply_command(
                            &mut server,
                            &notif_queue,
                            &mut quit_requested,
                            &cmd,
                            &ipc,
                            ts,
                        );
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                    Action::Quit => {
                        let cmd = ass_ipc::Command::Quit;
                        apply_command(
                            &mut server,
                            &notif_queue,
                            &mut quit_requested,
                            &cmd,
                            &ipc,
                            ts,
                        );
                        journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                    }
                }
            }
        }

        // Apply scoped synthetic actions only after physical input has updated
        // xkb modifier state. The target-local batch was authorized on the IPC
        // thread; this main-loop pass validates live geometry, z-order, and
        // shell occlusion before sending any event.
        for (cmd, ts) in pending_synthetic_input {
            let ass_ipc::Command::InjectInput { id, actions } = &cmd else {
                unreachable!();
            };
            let prepared = server.prepare_synthetic_input(*id, actions);
            let effect = if let Some(events) = prepared {
                let has_key = events
                    .iter()
                    .any(|event| matches!(event, ass_core::input::InputEvent::Key { .. }));
                let blocked_by_chrome = (has_key && shell.captures_keyboard())
                    || events.iter().any(|event| {
                        matches!(
                            *event,
                            ass_core::input::InputEvent::PointerMotion { x, y }
                                if shell.captures_pointer_at(x, y, input_acc.display_size)
                        )
                    });
                if blocked_by_chrome {
                    ass_ipc::Effect::Refused {
                        reason: "target is covered by compositor chrome".into(),
                    }
                } else {
                    server.focus_surface_by_id(*id);
                    let no_bindings = ass_core::keybind::Keymap::default();
                    let actions = server.forward_input(&events, &no_bindings);
                    debug_assert!(actions.is_empty());
                    if events.iter().any(|event| {
                        matches!(event, ass_core::input::InputEvent::PointerMotion { .. })
                    }) {
                        synthetic_pointer_active = true;
                        chrome_pointer_captured = false;
                    }
                    ass_ipc::Effect::Applied
                }
            } else {
                ass_ipc::Effect::Refused {
                    reason: "invalid, hidden, stale, or occluded target".into(),
                }
            };
            journal_effect_and_broadcast(
                &journal,
                &ipc,
                ts,
                ass_ipc::Origin::Ipc { conn_id: 0 },
                cmd,
                effect,
            );
        }
        input.set_scroll(shell_scroll.0, shell_scroll.1);

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
            server.set_output_geometry(output_geometry_from_host(sz.w, sz.h, host.scale()));
        }

        // Chrome owns the host cursor while it owns pointer routing. This is
        // what gives the launcher's search field a text caret and interactive
        // HUD/dock controls a pointing hand; leaving chrome restores the
        // focused client's requested cursor (including hidden cursors).
        let chrome_cursor = shell.cursor_shape_at(
            input_acc.cursor.0,
            input_acc.cursor.1,
            input_acc.display_size,
        );
        let compositor_cursor = server.compositor_cursor_shape();
        let owned_cursor = if server.interactive().is_some() {
            compositor_cursor
        } else {
            chrome_cursor
                .map(|shape| shape as u32)
                .or(compositor_cursor)
        };
        let cursor_hidden = owned_cursor.is_none() && server.cursor_hidden();
        let cursor_shape = owned_cursor.unwrap_or_else(|| server.cursor_shape().max(1));
        if cursor_hidden != last_cursor_hidden
            || (!cursor_hidden && cursor_shape != last_cursor_shape)
        {
            if cursor_hidden {
                host.hide_cursor();
            } else {
                host.set_cursor_shape(cursor_shape);
            }
            last_cursor_shape = cursor_shape;
            last_cursor_hidden = cursor_hidden;
        }

        // Apply the tiling policy to the current workspace when tiled mode is
        // on (ADR-0024). No-op when off; reconfigures only windows whose
        // target moved. The work-area is the focused output's logical rect
        // (ADR-0028) inset by the chrome's reserved edges, so tiles avoid
        // the dock (ADR-0024 chrome-aware work-area).
        server.apply_tiling(shell.reserved().inset(server.output_logical_rect()));

        match surface.begin_frame() {
            Ok(mut frame) => {
                let scale = host.scale();
                let logical_size = host.size_u32();
                let render_geometry = RenderGeometry {
                    logical_size,
                    scale,
                };
                let physical_size = surface.size();
                let blur_sigma = shell.backdrop_blur_sigma();
                let backdrop_regions = shell.backdrop_regions(input_acc.display_size);
                let model_active = wallpaper
                    .as_ref()
                    .is_some_and(ass_wallpaper::Wallpaper::has_model);
                let backdrop_plan = launcher_backdrop.prepare(
                    blur_sigma > 0.0 && !backdrop_regions.is_empty(),
                    &device,
                    &surface,
                    &frame,
                    physical_size,
                );

                match backdrop_plan {
                    BackdropPlan::Capture
                        if launcher_backdrop.begin_capture(&canvas, &frame, clear) =>
                    {
                        let capture_size = launcher_backdrop
                            .capture_size(&frame)
                            .unwrap_or(physical_size);
                        let capture_ratio = capture_size.0 as f32 / physical_size.0.max(1) as f32;
                        let capture_scale = scale * capture_ratio;

                        draw_wallpaper_background(
                            &canvas,
                            &device,
                            &mut wallpaper,
                            logical_size,
                            capture_scale,
                        );

                        if model_active {
                            canvas.end_target();
                            if let Some(target) = launcher_backdrop.target(&frame) {
                                if let Some(wallpaper) = wallpaper.as_mut() {
                                    wallpaper.draw_model_to(&device, &mut frame, target);
                                }
                                canvas.begin_target(&frame, target, None)?;
                            }
                        }

                        draw_client_scene(&canvas, &device, &mut renderer, &server, capture_scale);
                        let blurred = launcher_backdrop.end_capture_and_blur(
                            &canvas,
                            &frame,
                            blur_sigma * capture_scale,
                        );
                        canvas.begin(&frame, Some(clear))?;
                        // Preserve the live desktop everywhere, then replace
                        // only the component-declared glass regions with the
                        // shared blurred capture. This is a true backdrop
                        // effect rather than a full-screen blur hidden under
                        // an opaque top-bar colour.
                        draw_direct_desktop_scene(
                            &canvas,
                            &device,
                            &mut frame,
                            &mut wallpaper,
                            &mut renderer,
                            &server,
                            render_geometry,
                        )?;
                        if let Some(image) = blurred {
                            for region in &backdrop_regions {
                                let x = region.x.max(0.0) * scale;
                                let y = region.y.max(0.0) * scale;
                                let w = region
                                    .w
                                    .max(0.0)
                                    .min(logical_size.0 as f32 - region.x.max(0.0))
                                    * scale;
                                let h = region
                                    .h
                                    .max(0.0)
                                    .min(logical_size.1 as f32 - region.y.max(0.0))
                                    * scale;
                                if w <= 0.0 || h <= 0.0 {
                                    continue;
                                }
                                canvas.save();
                                canvas.clip_rect(x, y, w, h);
                                image.draw(
                                    &canvas,
                                    0.0,
                                    0.0,
                                    physical_size.0 as f32,
                                    physical_size.1 as f32,
                                );
                                canvas.restore();
                            }
                        }
                    }
                    BackdropPlan::Capture | BackdropPlan::Direct => {
                        canvas.begin(&frame, Some(clear))?;
                        draw_direct_desktop_scene(
                            &canvas,
                            &device,
                            &mut frame,
                            &mut wallpaper,
                            &mut renderer,
                            &server,
                            render_geometry,
                        )?;
                    }
                }
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
                let do_not_disturb = notif_queue.lock().unwrap().do_not_disturb();
                let tiled = server.tiling();
                if system_status.do_not_disturb != do_not_disturb || system_status.tiled != tiled {
                    system_status.do_not_disturb = do_not_disturb;
                    system_status.tiled = tiled;
                    shell.set_system_status(system_status.clone());
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
                    apply_command(
                        &mut server,
                        &notif_queue,
                        &mut quit_requested,
                        &cmd,
                        &ipc,
                        ts,
                    );
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                if let Some(id) = shell.take_closed_window() {
                    let cmd = ass_ipc::Command::Close { id };
                    apply_command(
                        &mut server,
                        &notif_queue,
                        &mut quit_requested,
                        &cmd,
                        &ipc,
                        ts,
                    );
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                if let Some(id) = shell.take_move_requested() {
                    let cmd = ass_ipc::Command::Move { id };
                    apply_command(
                        &mut server,
                        &notif_queue,
                        &mut quit_requested,
                        &cmd,
                        &ipc,
                        ts,
                    );
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                for action in shell.take_window_actions() {
                    let cmd = match action {
                        ass_shell::WindowAction::Focus(id) => ass_ipc::Command::Focus { id },
                        ass_shell::WindowAction::Minimize(id) => ass_ipc::Command::Minimize { id },
                        ass_shell::WindowAction::Close(id) => ass_ipc::Command::Close { id },
                    };
                    apply_command(
                        &mut server,
                        &notif_queue,
                        &mut quit_requested,
                        &cmd,
                        &ipc,
                        ts,
                    );
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                if let Some(id) = shell.take_switch_workspace() {
                    let cmd = ass_ipc::Command::SwitchWorkspaceTo { id };
                    apply_command(
                        &mut server,
                        &notif_queue,
                        &mut quit_requested,
                        &cmd,
                        &ipc,
                        ts,
                    );
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                if let Some(id) = shell.take_dismissed_notification() {
                    let cmd = ass_ipc::Command::DismissNotification { id };
                    apply_command(
                        &mut server,
                        &notif_queue,
                        &mut quit_requested,
                        &cmd,
                        &ipc,
                        ts,
                    );
                    journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                }
                if let Some(app) = shell.take_open_builtin() {
                    shell.open_builtin(app);
                }
                let system_actions = shell.take_system_actions();
                if !system_actions.is_empty() {
                    for action in system_actions {
                        if let Some(cmd) = apply_system_action(
                            &mut server,
                            &notif_queue,
                            &mut system_status,
                            action,
                        ) {
                            apply_command(
                                &mut server,
                                &notif_queue,
                                &mut quit_requested,
                                &cmd,
                                &ipc,
                                ts,
                            );
                            journal_and_broadcast(&journal, &ipc, ts, origin, cmd);
                        }
                    }
                    shell.set_system_status(system_status.clone());
                    // Reconcile optimistic hardware state shortly after the
                    // detached host command has had time to take effect.
                    next_system_status_poll =
                        std::time::Instant::now() + std::time::Duration::from_millis(500);
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
                frame.submit()?.present()?;

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
        animating = shell.anim_pending()
            || wallpaper
                .as_ref()
                .is_some_and(ass_wallpaper::Wallpaper::has_model);
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
) -> bool {
    let apply = |config: &Option<ass_config::Config>, server: &mut ass_server::Server| {
        server.set_window_rules(
            config
                .as_ref()
                .map(|c| c.window_rules.clone())
                .unwrap_or_default(),
        );
        if let Some(c) = config.as_ref() {
            server.set_layout_params(c.layout.clone().into());
        } else {
            server.set_layout_params(ass_core::layout::LayoutParams::default());
        }
    };
    match ass_config::load(path) {
        Ok(Some(new_cfg)) => {
            log::info!("config: reloaded {}", path.display());
            *config = Some(new_cfg);
            *keymap = build_keymap(config.as_ref());
            apply(config, server);
            true
        }
        Ok(None) => {
            log::warn!("config: {} removed; reverting to defaults", path.display());
            *config = None;
            *keymap = build_keymap(config.as_ref());
            apply(config, server);
            true
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
            false
        }
    }
}

/// Build the nested output geometry from its logical surface size and the
/// host's preferred render scale. `wl_output.mode` is expressed in physical
/// pixels while xdg-output derives the original logical size by dividing by
/// `scale`; keeping both in one constructor prevents the two coordinate spaces
/// from silently drifting apart.
fn output_geometry_from_host(
    logical_w: i32,
    logical_h: i32,
    scale: f32,
) -> ass_core::output::OutputGeometry {
    let scale = if scale.is_finite() && scale > 0.0 {
        scale
    } else {
        1.0
    };
    ass_core::output::OutputGeometry {
        mode: ass_core::output::OutputMode {
            width: (logical_w.max(1) as f32 * scale).round() as i32,
            height: (logical_h.max(1) as f32 * scale).round() as i32,
            refresh_mhz: 0,
        },
        scale: ass_core::output::Scale(scale),
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

/// Compile the trusted named IPC scopes from configuration. Invalid operation
/// names are ignored inside an explicit allowlist (therefore granting nothing
/// for that entry) and logged; they never turn into an unrestricted scope.
fn build_ipc_scopes(
    config: Option<&ass_config::Config>,
) -> std::collections::HashMap<String, ass_ipc::Scope> {
    let mut scopes = std::collections::HashMap::new();
    let Some(config) = config else {
        return scopes;
    };

    for declared in &config.agent.scopes {
        let name = declared.name.trim();
        if name.is_empty() {
            log::warn!("config: ignoring agent scope with an empty name");
            continue;
        }
        if scopes.contains_key(name) {
            log::warn!("config: duplicate agent scope '{name}' ignored");
            continue;
        }

        let ops = if declared.ops.is_empty() {
            None
        } else {
            Some(
                declared
                    .ops
                    .iter()
                    .filter_map(|op| match ipc_op_class(op) {
                        Some(op) => Some(op),
                        None => {
                            log::warn!("config: agent scope '{name}' has unknown operation '{op}'");
                            None
                        }
                    })
                    .collect(),
            )
        };
        let windows = (!declared.windows.is_empty()).then(|| {
            declared
                .windows
                .iter()
                .copied()
                .map(ass_core::window::WindowId)
                .collect()
        });
        let workspaces = (!declared.workspaces.is_empty()).then(|| {
            declared
                .workspaces
                .iter()
                .copied()
                .map(ass_core::workspace::WorkspaceId)
                .collect()
        });
        scopes.insert(
            name.to_string(),
            ass_ipc::Scope {
                windows,
                workspaces,
                outputs: None,
                ops,
            },
        );
    }
    scopes
}

fn ipc_op_class(name: &str) -> Option<ass_ipc::OpClass> {
    use ass_ipc::OpClass;
    match name.trim().to_ascii_lowercase().as_str() {
        "focus" => Some(OpClass::Focus),
        "minimize" => Some(OpClass::Minimize),
        "close" => Some(OpClass::Close),
        "move" => Some(OpClass::Move),
        "setwindowgeometry" | "set_window_geometry" => Some(OpClass::SetWindowGeometry),
        "injectinput" | "inject_input" => Some(OpClass::InjectInput),
        "cycle" => Some(OpClass::Cycle),
        "switchworkspace" | "switch_workspace" => Some(OpClass::SwitchWorkspace),
        "switchworkspaceto" | "switch_workspace_to" => Some(OpClass::SwitchWorkspaceTo),
        "movetoworkspace" | "move_to_workspace" => Some(OpClass::MoveToWorkspace),
        "toggletiling" | "toggle_tiling" => Some(OpClass::ToggleTiling),
        "notify" => Some(OpClass::Notify),
        "dismissnotification" | "dismiss_notification" => Some(OpClass::DismissNotification),
        _ => None,
    }
}

/// Shared live window snapshot for the IPC (ADR-0027). The main loop writes
/// the same `Vec<Window>` it hands the shell; connection threads read it.
/// `query`-capability commands never mutate, so the lock is an `RwLock` and
/// reads from several connections do not block each other. `control`/
/// `session` commands arrive through [`ass_ipc::Handler::command`] and are forwarded
/// to the main loop via the channel the binary owns — the Wayland server
/// state is not `Send`, so connection threads must not touch it directly.
struct LiveState {
    windows: std::sync::RwLock<Vec<ass_core::window::Window>>,
    workspaces: std::sync::RwLock<ass_core::workspace::WorkspaceSnapshot>,
    outputs: std::sync::RwLock<Vec<ass_core::output::OutputInfo>>,
    notifications: std::sync::Arc<std::sync::Mutex<ass_core::notify::NotificationQueue>>,
    journal: std::sync::Arc<std::sync::Mutex<ass_ipc::Journal>>,
    commands: std::sync::Mutex<std::sync::mpsc::Sender<ass_ipc::Command>>,
    scopes: std::sync::RwLock<std::collections::HashMap<String, ass_ipc::Scope>>,
}

impl LiveState {
    fn new(
        commands: std::sync::mpsc::Sender<ass_ipc::Command>,
        notifications: std::sync::Arc<std::sync::Mutex<ass_core::notify::NotificationQueue>>,
        journal: std::sync::Arc<std::sync::Mutex<ass_ipc::Journal>>,
        scopes: std::collections::HashMap<String, ass_ipc::Scope>,
    ) -> LiveState {
        LiveState {
            windows: std::sync::RwLock::new(Vec::new()),
            workspaces: std::sync::RwLock::new(
                ass_core::workspace::WorkspaceModel::new().snapshot(),
            ),
            outputs: std::sync::RwLock::new(Vec::new()),
            notifications,
            journal,
            commands: std::sync::Mutex::new(commands),
            scopes: std::sync::RwLock::new(scopes),
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

    fn set_scopes(&self, scopes: std::collections::HashMap<String, ass_ipc::Scope>) {
        *self.scopes.write().unwrap() = scopes;
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
            input: true,
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

    fn resolve_scope(&self, name: &str) -> Option<ass_ipc::Scope> {
        self.scopes.read().unwrap().get(name).cloned()
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

/// Raster extensions the `image` crate decodes directly. SVG/SVGZ uses the
/// standard librsvg command-line rasterizer when installed and otherwise
/// falls back to the dock glyph without failing startup.
const RASTER_ICON_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tiff", "ico"];
const SVG_ICON_EXTS: &[&str] = &["svg", "svgz"];
const HUD_SYMBOLIC_ICON_NAMES: &[&str] = &[
    "audio-volume-muted-symbolic",
    "audio-volume-low-symbolic",
    "audio-volume-medium-symbolic",
    "audio-volume-high-symbolic",
    "network-wireless-signal-excellent-symbolic",
    "network-wired-symbolic",
    "network-offline-symbolic",
    "preferences-system-notifications-symbolic",
    "preferences-system-symbolic",
    "window-close-symbolic",
    "application-x-executable-symbolic",
];

/// Resolve the host's selected application icon theme. An explicit ass
/// override wins; otherwise query the GTK/GSettings desktop preference used
/// by niri and other toolkit-neutral Wayland sessions. `hicolor` remains the
/// portable fallback when GSettings is unavailable.
fn selected_icon_theme() -> String {
    if let Some(theme) = std::env::var("ASS_ICON_THEME")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
    {
        return theme;
    }

    let output = std::process::Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "icon-theme"])
        .output();
    output
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .and_then(|value| parse_gsettings_string(&value))
        .unwrap_or_else(|| ass_apps::DEFAULT_ICON_THEME.to_string())
}

/// Merge XDG applications with compositor-owned system applications. Built-in
/// entries deliberately use the same `Entry` model so launcher search,
/// context menus, pinning, and icon lookup have one catalog contract.
fn application_catalog(icon_theme: &str, icon_scale: u32) -> Vec<ass_core::app::Entry> {
    let mut applications = ass_apps::enumerate_with_theme_and_scale(icon_theme, icon_scale.max(1));
    let i18n = ass_shell::Localizer::from_env();
    applications.push(ass_core::app::Entry::control_center(
        i18n.text(ass_shell::Message::ControlCenter),
        i18n.text(ass_shell::Message::BuiltInSystemApp),
    ));
    applications
}

fn parse_gsettings_string(value: &str) -> Option<String> {
    let value = value.trim();
    let unquoted = value
        .strip_prefix('\'')
        .and_then(|value| value.strip_suffix('\''))
        .or_else(|| {
            value
                .strip_prefix('"')
                .and_then(|value| value.strip_suffix('"'))
        })
        .unwrap_or(value)
        .trim();
    (!unquoted.is_empty()).then(|| unquoted.to_string())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct IconFileStamp {
    len: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
    device: u64,
    inode: u64,
}

/// Snapshot only icons the catalog actually uses. Metadata follows symlinks,
/// so a Flatpak `current/active` update is noticed even when the exported icon
/// path itself remains unchanged.
fn snapshot_icons(
    apps: &[ass_core::app::Entry],
) -> std::collections::BTreeMap<std::path::PathBuf, Option<IconFileStamp>> {
    use std::os::unix::fs::MetadataExt;

    let mut snapshot = std::collections::BTreeMap::new();
    for path in apps.iter().filter_map(|entry| entry.icon_path.as_ref()) {
        snapshot.entry(path.clone()).or_insert_with(|| {
            std::fs::metadata(path).ok().map(|metadata| IconFileStamp {
                len: metadata.len(),
                modified_seconds: metadata.mtime(),
                modified_nanoseconds: metadata.mtime_nsec(),
                device: metadata.dev(),
                inode: metadata.ino(),
            })
        });
    }
    snapshot
}

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
        match apps.iter().find(|e| app_keys(e).contains(&want)) {
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
fn build_icon_cache(
    device: &flux::Device,
    apps: &[ass_core::app::Entry],
    icon_theme: &str,
    icon_scale: u32,
) -> IconCache {
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
        let Some(decoded) = decode_icon(path, &ext, icon_scale) else {
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

    // HUD status assets come from the same icon theme as applications. SVGs
    // are rasterized at output scale (and subsequently sampled down by lens),
    // avoiding the coarse single-pixel strokes of compositor glyphs while
    // retaining the host theme's silhouettes and proportions.
    let mut symbolic_names: Vec<String> = HUD_SYMBOLIC_ICON_NAMES
        .iter()
        .map(|name| (*name).to_string())
        .collect();
    for level in (0..=100).step_by(10) {
        symbolic_names.push(format!("battery-level-{level}-symbolic"));
        symbolic_names.push(format!("battery-level-{level}-charging-symbolic"));
    }
    let mut hud_count = 0usize;
    for name in symbolic_names {
        let Some(path) =
            ass_apps::resolve_icon_scaled(&name, Some(icon_theme), &[], 24, icon_scale.max(1))
        else {
            log::debug!("hud icon: '{name}' was not found in theme '{icon_theme}'");
            continue;
        };
        let ext = path
            .extension()
            .and_then(|extension| extension.to_str())
            .map(str::to_ascii_lowercase)
            .unwrap_or_default();
        let Some(decoded) = decode_icon(&path, &ext, icon_scale) else {
            continue;
        };
        let mut rgba = decoded.to_rgba8();
        // Symbolic themes commonly encode a dark CSS foreground intended for
        // toolkit recolouring. The compositor has no GTK style context, so
        // apply the HUD's light foreground while preserving every coverage
        // value produced by SVG antialiasing.
        for pixel in rgba.pixels_mut() {
            if pixel[3] != 0 {
                pixel[0] = 246;
                pixel[1] = 246;
                pixel[2] = 248;
            }
        }
        let (w, h) = rgba.dimensions();
        let mut bgra = rgba.into_raw();
        for chunk in bgra.chunks_exact_mut(4) {
            chunk.swap(0, 2);
        }
        match flux::Image::from_bytes(device, w, h, flux::Format::FLUX_FORMAT_BGRA8_UNORM, &bgra) {
            Ok(image) => {
                let ptr = image.as_raw() as *mut c_void;
                map.insert(format!("ass-hud:{name}"), ptr);
                if name == "preferences-system-symbolic" {
                    // Stable application-icon key for the compositor-owned
                    // control center entry and component header.
                    map.insert("ass-control-center".into(), ptr);
                }
                images.push(image);
                hud_count += 1;
            }
            Err(error) => log::warn!("hud icon: upload failed for {}: {error:?}", path.display()),
        }
    }

    log::info!(
        "icons: {} application texture(s), {hud_count} themed HUD symbol(s)",
        images.len().saturating_sub(hud_count)
    );
    IconCache {
        _images: images,
        map,
    }
}

/// Decode a desktop icon. Raster formats stay in-process; SVG is converted to
/// a bounded PNG on stdout so malformed or enormous vector sources cannot
/// dictate an unbounded GPU texture. Every failure is a normal glyph fallback.
fn decode_icon(path: &std::path::Path, ext: &str, icon_scale: u32) -> Option<image::DynamicImage> {
    if RASTER_ICON_EXTS.contains(&ext) {
        return image::open(path).ok();
    }
    if !SVG_ICON_EXTS.contains(&ext) {
        return None;
    }
    let target = ass_apps::DEFAULT_ICON_SIZE
        .saturating_mul(icon_scale.max(1))
        .min(512)
        .to_string();
    let output = std::process::Command::new("rsvg-convert")
        .args([
            "--width",
            &target,
            "--height",
            &target,
            "--keep-aspect-ratio",
        ])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        log::debug!("icon: SVG rasterization failed for {}", path.display());
        return None;
    }
    image::load_from_memory(&output.stdout).ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nested_output_geometry_preserves_logical_size_at_integer_scale() {
        let geometry = output_geometry_from_host(945, 924, 2.0);
        assert_eq!(geometry.mode.width, 1890);
        assert_eq!(geometry.mode.height, 1848);
        assert_eq!(geometry.scale, ass_core::output::Scale(2.0));
        assert_eq!(geometry.logical_size(), ass_core::Size { w: 945, h: 924 });
    }

    #[test]
    fn nested_output_geometry_preserves_logical_size_at_fractional_scale() {
        let geometry = output_geometry_from_host(945, 924, 1.5);
        assert_eq!(geometry.mode.width, 1418);
        assert_eq!(geometry.mode.height, 1386);
        assert_eq!(geometry.scale, ass_core::output::Scale(1.5));
        assert_eq!(geometry.logical_size(), ass_core::Size { w: 945, h: 924 });
    }

    #[test]
    fn parses_gsettings_icon_theme_string() {
        assert_eq!(
            parse_gsettings_string("'Papirus-Dark'\n").as_deref(),
            Some("Papirus-Dark")
        );
        assert_eq!(
            parse_gsettings_string("\"Adwaita\"").as_deref(),
            Some("Adwaita")
        );
        assert_eq!(parse_gsettings_string("  "), None);
    }

    #[test]
    fn config_agent_scopes_compile_to_fail_closed_ipc_allowlists() {
        let config = ass_config::Config::parse(
            "schema_version = 1\n\
             [[agent.scope]]\n\
             name = \"focus-one\"\n\
             ops = [\"Focus\", \"NotARealOperation\"]\n\
             windows = [7]\n\
             workspaces = [3]\n",
        )
        .unwrap();
        let scopes = build_ipc_scopes(Some(&config));
        let scope = scopes.get("focus-one").expect("compiled scope");

        assert_eq!(scope.ops, Some(vec![ass_ipc::OpClass::Focus]));
        assert!(scope.permits(&ass_ipc::Command::Focus {
            id: ass_core::window::WindowId(7),
        }));
        assert!(!scope.permits(&ass_ipc::Command::Focus {
            id: ass_core::window::WindowId(8),
        }));
        assert!(!scope.permits(&ass_ipc::Command::Close {
            id: ass_core::window::WindowId(7),
        }));
    }

    #[test]
    fn automation_operation_names_accept_canonical_and_snake_case() {
        assert_eq!(
            ipc_op_class("SetWindowGeometry"),
            Some(ass_ipc::OpClass::SetWindowGeometry)
        );
        assert_eq!(
            ipc_op_class("set_window_geometry"),
            Some(ass_ipc::OpClass::SetWindowGeometry)
        );
        assert_eq!(
            ipc_op_class("inject_input"),
            Some(ass_ipc::OpClass::InjectInput)
        );
    }
}
