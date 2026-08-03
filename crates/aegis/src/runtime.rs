use crate::*;

mod agent_auth;
mod app_pick;
mod apps;
mod capability_pick;
mod capture;
mod commands;
mod config;
mod confirm_pick;
mod damage;
mod event_loop;
mod idle;
mod input;
mod ipc;
mod iteration;
mod pick;
mod presentation;
mod presentation_state;
mod realm;
mod rendering;
mod scanout;
mod secret_prompt;
mod session;
mod settings;
mod state;
mod stream;
mod system;

use agent_auth::*;
use app_pick::*;
use apps::*;
use capability_pick::*;
use capture::*;
use commands::*;
use config::*;
use confirm_pick::*;
use damage::*;
use idle::*;
use input::*;
use ipc::*;
use iteration::*;
use pick::*;
use presentation::*;
use presentation_state::*;
use realm::*;
use rendering::*;
use scanout::*;
use secret_prompt::*;
use state::*;
use stream::*;
use system::*;

const DEFAULT_WALLPAPER: &[u8] =
    include_bytes!("../../../assets/wallpapers/procedural-generation.png");

#[cfg(test)]
mod tests;

/// One serialized queue for every write to the user TOML config file
/// (ADR-0026). `aegis-config` persists edits as read-modify-write cycles, so
/// concurrent writers (dock pin clicks, System Settings commits)
/// could lose each other's updates; funnelling all writes through a single
/// worker thread makes them execute strictly in send order. Fire-and-forget
/// jobs (dock pins) log failures on the worker; synchronous jobs (settings
/// commits) carry a oneshot receipt so the IPC reply stays accurate while
/// the write itself still happens off the main loop.
#[derive(Clone)]
pub(super) struct ConfigWriter {
    store: Option<aegis_config::ConfigStore>,
    tx: std::sync::mpsc::Sender<ConfigWriteJob>,
}

struct ConfigWriteJob {
    store: aegis_config::ConfigStore,
    edit: aegis_config::ConfigEdit,
    receipt: Option<std::sync::mpsc::Sender<Result<(), String>>>,
}

impl ConfigWriter {
    /// Queue a typed edit without blocking the caller.
    pub(super) fn enqueue(&self, edit: aegis_config::ConfigEdit) -> Result<(), String> {
        let store = self
            .store
            .clone()
            .ok_or_else(|| "no writable configuration path is available".to_owned())?;
        self.tx
            .send(ConfigWriteJob {
                store,
                edit,
                receipt: None,
            })
            .map_err(|_| "config write worker stopped".to_owned())
    }

    /// Queue a write and block until the worker reports the result. Used by
    /// settings commits, which must surface persistence failures in their
    /// IPC reply; the block is bounded by one TOML rewrite per queued job.
    pub(super) fn apply_and_wait(&self, edit: aegis_config::ConfigEdit) -> Result<(), String> {
        let store = self
            .store
            .clone()
            .ok_or_else(|| "no writable configuration path is available".to_owned())?;
        let (receipt_tx, receipt_rx) = std::sync::mpsc::channel();
        self.tx
            .send(ConfigWriteJob {
                store,
                edit,
                receipt: Some(receipt_tx),
            })
            .map_err(|_| "config write worker stopped".to_owned())?;
        receipt_rx
            .recv()
            .map_err(|_| "config write worker stopped".to_owned())?
    }
}

pub(crate) fn run() -> Result<(), Box<dyn std::error::Error>> {
    log::info!(
        "aegis {} — autonomous surface shell",
        env!("CARGO_PKG_VERSION")
    );
    match aegis_launcher::prepare_realm_host() {
        Ok(root) => log::info!(
            "Realm cgroup host prepared under delegated root {}",
            root.display()
        ),
        Err(error) => log::warn!(
            "Realm application launch disabled until Aegis runs in its own \
             cpu/memory/pids-delegated systemd service: {error}"
        ),
    }

    // Notification queue (M9, over the IPC): shared between the IPC handler
    // (reads), the toast chrome component (renders), and this loop (pushes
    // on `Notify`, expires each frame). The TTL is the retention horizon for
    // the command panel's Messages list, the HUD count, and IPC history —
    // the toast strip applies its own 3-second presentation window on top.
    // Declared early so the toast component registration below can clone it.
    let notif_queue: std::sync::Arc<std::sync::Mutex<aegis_core::notify::NotificationQueue>> =
        std::sync::Arc::new(std::sync::Mutex::new(
            aegis_core::notify::NotificationQueue::new(3_600_000),
        ));

    // Declarative configuration (ADR-0026). One TOML file at
    // `$XDG_CONFIG_HOME/aegis/config.toml` is the source of truth; absence is
    // not an error (built-in defaults apply). A malformed or
    // schema-incompatible file is logged and skipped, not fatal. Loaded
    // before the backend so configured display modes are known at the very
    // first modeset (ADR-0028).
    let config_path = aegis_config::default_path();
    let config = load_config(config_path.as_deref());
    let desktop_preferences = effective_desktop_preferences(config.as_ref());

    // Select the presentation target before Vulkan creation: nested Wayland
    // requires WSI extensions, while DRM requires exportable offscreen images.
    // `auto` uses an outer Wayland display when present and atomic DRM on a TTY.
    let backend_kind = requested_backend()?;
    let host_bootstrap = Host::open(
        backend_kind,
        "aegis",
        1280,
        720,
        configured_output_modes(config.as_ref()),
    )?;
    let device = host_bootstrap.create_device()?;
    // Move the host into a binding declared after the device so Rust drops the
    // host-owned VkSurfaceKHR before Flux destroys its VkInstance.
    let mut host = host_bootstrap;
    host.set_touchpad_config(
        config
            .as_ref()
            .map(|c| c.input.touchpad)
            .unwrap_or_default(),
    );
    log::info!(
        "flux: device created for {} backend; dma-buf {}",
        host.name(),
        if flux::dmabuf_supported(&device) {
            "supported"
        } else {
            "unavailable"
        }
    );

    // Nested mode creates a Vulkan WSI swapchain. DRM mode creates an
    // exportable offscreen ring that the backend imports into KMS.
    let (w, h) = host.physical_size();
    log::info!(
        "{}: presentation target {w}x{h} (scale {})",
        host.name(),
        host.scale()
    );

    // Flux presentation surface + canvas.
    let surface = host.create_surface(&device)?;
    if let Err(error) = surface.prepare_readback() {
        log::warn!(
            "capture: could not preallocate readback staging: {error}{}",
            flux_last_error_detail()
        );
    }
    let canvas = flux::Canvas::new(&surface)?;
    let launcher_backdrop = LauncherBackdrop::new(&device)?;
    // Frozen-frame snapshot behind the screenshot selector: allocated lazily
    // on the first trigger, reused across later sessions.
    let screenshot_freeze = ScreenshotFreeze::new();
    // A requested presentation frame remains in mapped readback staging until
    // the main loop copies it into an owned CPU buffer.
    let pending_capture: Option<PendingCapture> = None;
    // PNG compression and file writes run here instead of pausing the
    // compositor frame thread after GPU readback.
    let capture_worker = CaptureWorker::spawn()?;
    // XDG cursor theme cache for the software cursor on direct KMS.
    let mut cursor_cache = cursor::CursorCache::default();
    cursor_cache.set_preferences(
        desktop_preferences.cursor_theme.clone(),
        desktop_preferences.cursor_size,
    );
    // Advertise the pre-scaled buffer to the host; takes effect on the next
    // commit (the first present below).
    host.set_buffer_scale();

    // `config` was loaded above, before the backend, so output policy also
    // applies before icons decode.
    let screenshot_dir = config
        .as_ref()
        .map(|c| std::path::PathBuf::from(&c.screenshot.save_dir))
        .unwrap_or_else(aegis_config::default_screenshot_dir);

    // Wayland server: accept client connections on its own socket. Created
    // before the icon pass so the effective output scale (backend-reported
    // geometry plus any `[[output]]` override) is known when icons decode.
    let dmabuf_main_device = host.dmabuf_feedback_device(&device);
    let dmabuf_scanout_formats = host.dmabuf_scanout_formats();
    let dmabuf_scanout_device = host.dmabuf_scanout_device();
    if host.name() == "drm" && dmabuf_main_device.is_none() {
        log::warn!(
            "drm: linux-dmabuf v4 feedback disabled because the main DRM device is unknown; \
             OpenGL clients may fall back to software rendering"
        );
    }
    let mut server = aegis_compositor::Server::new_with_dmabuf_feedback(
        flux::dmabuf_supported(&device),
        flux::dmabuf_sync_supported(&device),
        aegis_render::formats_with_modifiers(&device),
        dmabuf_main_device,
        dmabuf_scanout_formats,
        dmabuf_scanout_device,
    )?;
    server.set_outputs(host.output_infos());
    log::info!("server: listening on WAYLAND_DISPLAY={}", server.socket());
    // Client commits and capture-worker completions share one pollable wakeup
    // fd. Capture post-processing can therefore leave the compositor fully
    // idle and still deliver a freshly encoded clipboard immediately, instead
    // of posing as animation until the one-second maintenance tick.
    capture_worker.register_server_wakeup_fd(server.event_loop_fd())?;
    host.set_wakeup_fd(capture_worker.wakeup_fd());
    // Publish the session environment now that the socket name is known, so
    // launched clients and D-Bus-activated services can connect back.
    session::publish(server.socket(), host.name() == "nested");
    if let Some(c) = config.as_ref() {
        server.set_output_policies(c.output_policies());
    }
    // The effective scale the whole frame renders at: the primary output's
    // geometry after overrides, falling back to the host's own scale
    // (nested, where the host compositor owns scaling).
    let effective_scale = server
        .output_infos()
        .first()
        .map(|o| o.geometry.scale.as_f32())
        .filter(|s| *s > 0.0)
        .unwrap_or_else(|| host.scale());

    // Enumerate launchable `.desktop` entries at startup; the catalog is
    // rescanned periodically below so package installs/removals appear without
    // restarting the compositor.
    let icon_theme = desktop_preferences.icon_theme.clone();
    let icon_scale = effective_icon_scale(Some(effective_scale), host.scale());
    let launcher_apps = application_catalog(&icon_theme, icon_scale);
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
    let decoded_icons = decode_icons(&launcher_apps, &icon_theme, icon_scale);
    let icon_cache = build_icon_cache(&device, &decoded_icons);
    let icon_snapshot = snapshot_icons(&launcher_apps);

    // Compositor chrome, bound to the same device. The core host ships with
    // no chrome of its own; compose it from the components the binary wants.
    let mut shell = unsafe { aegis_shell::Shell::new(device.as_raw() as *mut _) }?;
    // Per-window chrome is intentionally absent: decoration ownership lives
    // in the Wayland server, and borderless windows are managed through the
    // Dock, gestures, tiling, and key bindings.
    // Read-only physical mirrors get one compositor-owned guard responsible
    // for their disabled presentation and pointer ownership. The independent
    // Realm seat remains the actual input-authority boundary in the server.
    shell.add(Box::new(aegis_shell::ControlledWindowGuard::new()));
    // Agent input feedback is compositor-owned and non-interactive. Register
    // it above the mirror guard so activity remains legible, but below the HUD,
    // notifications, and modal trusted chrome. Directed Realm capture renders
    // client surfaces directly and therefore never includes this layer.
    shell.add(Box::new(aegis_shell::AgentFeedback::new()));
    // The SNI tray service is spawned once and shared: the HUD reads the
    // snapshot for its display-only tray row, and the command panel additionally
    // holds the command channel for tray interaction (ADR-0080).
    let tray = aegis_tray::spawn();
    if config.as_ref().map(|c| c.hud.enabled).unwrap_or(true) {
        shell.add(Box::new(aegis_hud::Hud::with_sources(
            &device,
            tray.as_ref()
                .map(|(snapshot, _)| std::sync::Arc::clone(snapshot)),
            std::sync::Arc::clone(&notif_queue),
        )));
    }
    shell.add(Box::new(aegis_shell::Toast::new(std::sync::Arc::clone(
        &notif_queue,
    ))));
    // Only the binary wires discovery to chrome (ADR-0022); the shell stays
    // free of `aegis-apps`. Register the launcher after ordinary overlays so its
    // full-screen surface covers workspace/toast chrome, while the dock (added
    // last below) remains available like macOS Launchpad. Components start
    // empty; the application catalog is pushed below and fanned out to every
    // registered component.
    shell.add(Box::new(aegis_shell::Launcher::new()));
    // Prism is the compact application-search surface. It shares the
    // launcher's catalog and launch/focus event path while keeping its own
    // Spotlight-style presentation and input state in a standalone crate.
    shell.add(Box::new(aegis_prism::Prism::new()));
    // The overview (M9): a modal window/workspace picker over the same live
    // scene; registered with the modal chrome so it covers ordinary overlays.
    shell.add(Box::new(aegis_shell::Overview::new()));
    // The command panel (ADR-0080): the interactive counterpart of the
    // display-only HUD — quick settings, tray activation with dbusmenu
    // popovers, and notification dismissal in one modal surface, toggled by
    // the Super+S binding or a four-finger touchpad swipe.
    shell.add(Box::new(aegis_command_panel::CommandPanel::new(
        &device,
        tray,
        std::sync::Arc::clone(&notif_queue),
    )));
    // Built-in applications share the launcher catalog with XDG entries but
    // render in-process through optics/lens. Immediate system controls live in
    // the command panel (ADR-0080); Realm authority management remains its
    // own surface.
    shell.add(Box::new(aegis_ai_workspaces::AiWorkspaces::new()));
    // Interactive screenshot region selector, triggered by the Print key.
    shell.add(Box::new(aegis_shell::ScreenshotSelector::new()));
    // User-consent application picker (the AppChooser portal's compositor
    // side), opened by PickApp IPC requests.
    shell.add(Box::new(aegis_shell::AppPicker::new()));
    // Masked secret prompt (the secret vault's password unlock), opened by
    // PromptSecret IPC requests.
    shell.add(Box::new(aegis_shell::SecretPrompt::new()));
    // Yes/no confirmation dialog (portal consent flows), opened by
    // PickConfirm IPC requests.
    shell.add(Box::new(aegis_shell::ConfirmPrompt::new()));
    // Capability-borrowing checklist (ADR-0088 agent pairing), opened by
    // PairAgent IPC requests.
    shell.add(Box::new(aegis_shell::CapabilityPrompt::new()));
    // The dock is registered after the config is loaded below, so the pushed
    // catalog already carries the resolved `[dock]` pinned list.
    let mut input_acc = InputAccumulator::default();
    // Seed the chrome's logical extent so widgets can lay out before the first
    // resize arrives. The server's output geometry (backend + overrides) is
    // authoritative; the host size is the nested fallback.
    {
        let logical = server
            .output_infos()
            .first()
            .map(|o| o.geometry.logical_size());
        let (w, h) = logical
            .map(|s| (s.w as f32, s.h as f32))
            .unwrap_or_else(|| {
                let sz = host.size();
                (sz.w as f32, sz.h as f32)
            });
        input_acc.display_size = (w, h);
    }

    // Compositing of client surfaces.
    let renderer = aegis_render::Renderer::new();
    let realm_processes = RealmProcesses::default();
    let realm_render_targets: std::collections::BTreeMap<
        aegis_core::realm::RealmId,
        RealmRenderTarget,
    > = std::collections::BTreeMap::new();
    let pending_realm_capture: Option<PendingRealmCapture> = None;
    let realm_damage_sequence = 0u64;
    let start = std::time::Instant::now();

    // Wallpaper modes are persistent configuration: image, video, 3D, or a
    // back-to-front parallax image stack. The historical environment source
    // and model remain explicit startup overrides. With no source configured,
    // embedded bytes keep installed builds independent of build-tree paths.
    //
    // The decode resolution is seeded from the initial *physical* host size so
    // the wallpaper is decoded at the framebuffer's true resolution; later
    // resizes GPU-scale the wallpaper on draw without re-decoding.
    let (init_w, init_h) = host.physical_size();
    let wallpaper = match load_wallpaper(
        config.as_ref(),
        config_path.as_deref(),
        &device,
        &surface,
        (init_w, init_h),
        DEFAULT_WALLPAPER,
    ) {
        Ok((mut wallpaper, label)) => {
            wallpaper.set_reduced_motion(desktop_preferences.reduced_motion);
            log::info!("wallpaper: enabled ({label})");
            Some(wallpaper)
        }
        Err(error) => {
            log::warn!("wallpaper: load failed: {error}");
            None
        }
    };

    let clear = flux::rgba(30, 30, 46, 255);
    let frame_count: u64 = 0;
    // Nested-only deferral for retired client buffers: with no exportable
    // completion fence, the loop releases them a few presented frames late
    // instead of stalling the whole device on a wait_idle. Holds the frame
    // count at which the first pending retirement was seen.
    let retired_defer: Option<u64> = None;

    // A compositor overlay changes the owner of new key presses, not the
    // Wayland keyboard focus. Preserve that owner until the matching release
    // so opening or closing chrome cannot split one physical key sequence.
    let keyboard_capture = aegis_core::input::KeyboardCaptureState::default();

    // Global key bindings: built-in defaults overridden by the config file's
    // `[[keybind]]` entries. `forward_input` consumes a matched key before
    // delivering it to the focused client.
    let keymap = build_keymap(config.as_ref());
    log::info!("keybinds: {} active", keymap.len());
    // Touchpad swipe bindings, same layering: `[[gesture]]` entries over the
    // built-in defaults (ADR-0082). Rebuilt alongside the keymap on reload.
    let gesture_map = build_gesture_map(config.as_ref());
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
        server.set_tiling_default(c.layout.default_tiled);
        server.set_remember_window_positions(c.layout.remember_window_positions);
        server.set_decoration_policy(c.ui.window_decorations);
        server.set_output_policies(c.output_policies());
        server.set_allow_quit_while_locked(c.dev.allow_quit_while_locked);
    }
    shell.set_reduced_motion(desktop_preferences.reduced_motion);
    server.set_reduced_motion(desktop_preferences.reduced_motion);
    // The dock: a persistent strip of pinned `.desktop` app icons (ADR-0022).
    // Resolve the pinned entries from the config's `[dock] pinned` list.
    // Automatic selection remains an explicit opt-in; an unconfigured session
    // starts with only the Applications tile. Push the catalog (entries + pins
    // + the borrowed icon cache, which outlives the shell) before registering
    // the dock: `Shell::add` seeds new components with the current catalog.
    // The dock stays last so it stacks above the other chrome.
    let pinned = resolve_pinned(
        &launcher_apps,
        &icon_cache.map,
        config
            .as_ref()
            .map(|c| c.dock.pinned.as_slice())
            .unwrap_or(&[]),
        config
            .as_ref()
            .map(|c| c.dock.autopopulate)
            .unwrap_or(false),
    );
    log::info!("dock: {} app(s) pinned", pinned.len());
    shell.set_app_catalog(aegis_shell::AppCatalog {
        apps: launcher_apps.clone(),
        pinned,
        icons: aegis_shell::IconSet::from_raw(icon_cache.map.clone()),
    });
    let autohide = config.as_ref().map(|c| c.dock.autohide).unwrap_or(false);
    let autohide_timeout = config
        .as_ref()
        .map(|c| c.dock.autohide_timeout)
        .unwrap_or(2.5);
    let mut dock = aegis_dock::Dock::new();
    dock.set_autohide(autohide);
    dock.set_autohide_timeout(autohide_timeout);
    shell.add(Box::new(dock));
    // Register the held-Super switcher last so its selection chrome stacks
    // above the Dock while the renderer supplies live window previews below.
    shell.add(Box::new(aegis_shell::WindowSwitcher::new()));

    // One normalized status snapshot feeds compositor chrome and IPC. Host
    // probes (wpctl/nmcli fork+exec) run on a helper thread so the compositor
    // never blocks a frame on a subprocess; the main loop applies the latest
    // snapshot it finds on the channel.
    //
    // The poll is split into two cadences. The cheap poll reads only `/sys`
    // (battery, brightness, charging, network link) every few seconds to keep
    // the HUD fresh. The two forked commands — `wpctl get-volume` and
    // `nmcli radio wifi` — change far more slowly (volume only on user action,
    // which already triggers an out-of-cycle refresh; the Wi-Fi radio is
    // toggled rarely), so they run on a longer interval instead of forking
    // twice every cycle just to re-discover an unchanged answer.
    const SYSTEM_STATUS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(3);
    const FORKED_STATUS_INTERVAL: std::time::Duration = std::time::Duration::from_secs(30);
    let mut system_status = aegis_shell::detect_system_status();
    system_status.do_not_disturb = notif_queue.lock().unwrap().do_not_disturb();
    system_status.tiled = server.tiling();
    system_status.touchpad = host.touchpad_status();
    system_status.display = aegis_shell::DisplayStatus {
        configurable: host.name() == "drm",
        outputs: server.output_infos(),
        error: None,
    };
    shell.set_system_status(system_status.clone());
    // Seed the panel with one resource sample so it never opens on all zeros.
    shell.set_resource_stats(aegis_shell::ResourceProbe::new().sample());
    let (resource_tx, resource_rx) = std::sync::mpsc::channel::<aegis_shell::ResourceStats>();
    let (status_tx, status_rx) = std::sync::mpsc::channel::<aegis_shell::SystemStatus>();
    // System actions wake the poller for an out-of-cycle refresh so the HUD
    // reconciles its optimistic values right away; the main loop itself never
    // waits on a probe subprocess.
    let (status_refresh_tx, status_refresh_rx) = std::sync::mpsc::channel::<()>();
    std::thread::Builder::new()
        .name("aegis-status".into())
        .spawn(move || {
            // Last-known values for the forked fields; carried across cheap
            // polls so the snapshot stays coherent between full probes.
            struct StatusProbe {
                last_volume: Option<u8>,
                last_muted: bool,
                last_wifi: Option<bool>,
            }
            impl StatusProbe {
                fn full(&mut self) -> aegis_shell::SystemStatus {
                    let (volume, muted, wifi) = aegis_shell::detect_forked_status();
                    self.last_volume = volume;
                    self.last_muted = muted;
                    self.last_wifi = wifi;
                    aegis_shell::detect_system_status_lightweight(volume, muted, wifi)
                }
                fn cheap(&self) -> aegis_shell::SystemStatus {
                    aegis_shell::detect_system_status_lightweight(
                        self.last_volume,
                        self.last_muted,
                        self.last_wifi,
                    )
                }
            }
            let mut probe = StatusProbe {
                last_volume: None,
                last_muted: false,
                last_wifi: None,
            };
            // Drain any refresh requests queued during the previous probe so a
            // burst of volume key presses collapses into one full probe.
            let drain_refresh = || {
                while status_refresh_rx.try_recv().is_ok() {}
            };
            // The inner loop only exits by returning, so the initial probe
            // send is a one-shot guard, not a loop (clippy: never loops).
            if status_tx.send(probe.full()).is_ok() {
                let mut next_forked_deadline = std::time::Instant::now() + FORKED_STATUS_INTERVAL;
                loop {
                    // A queued refresh request re-probes out of cycle instead
                    // of waiting out the interval; disconnection means the main
                    // loop is gone.
                    match status_refresh_rx.recv_timeout(SYSTEM_STATUS_INTERVAL) {
                        Ok(()) => {
                            // Refresh requested: run a full probe immediately
                            // so optimistic HUD values reconcile at once, then
                            // reset the forked cadence.
                            if status_tx.send(probe.full()).is_err() {
                                return;
                            }
                            next_forked_deadline =
                                std::time::Instant::now() + FORKED_STATUS_INTERVAL;
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                            let now = std::time::Instant::now();
                            if now >= next_forked_deadline {
                                if status_tx.send(probe.full()).is_err() {
                                    return;
                                }
                                next_forked_deadline = now + FORKED_STATUS_INTERVAL;
                            } else {
                                // Cheap poll: stay off the fork path and reuse
                                // the last volume/wifi values.
                                if status_tx.send(probe.cheap()).is_err() {
                                    return;
                                }
                            }
                        }
                        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                            drain_refresh();
                            return;
                        }
                    }
                }
            }
        })
        .expect("spawn status poller");
    // Resource utilisation (CPU/GPU/memory/net/disk) polls on its own channel:
    // the probe reads only /proc and /sys plus one statvfs, so it never
    // blocks a frame, and a failed send means the main loop is gone.
    std::thread::Builder::new()
        .name("aegis-resources".into())
        .spawn(move || {
            let mut probe = aegis_shell::ResourceProbe::new();
            loop {
                if resource_tx.send(probe.sample()).is_err() {
                    return;
                }
                std::thread::sleep(std::time::Duration::from_secs(2));
            }
        })
        .expect("spawn resource poller");

    // mtime-based reload watcher, polled each frame. `None` when there is no
    // default config path on this host.
    let reload = config_path.as_deref().map(aegis_config::ReloadWatcher::at);
    let quit_requested = false;

    // Config-file persistence: every TOML rewrite (dock pins, touchpad
    // profile, output settings) runs on this single worker in send order, so
    // the read-modify-write cycles in `aegis-config` can never interleave
    // and lose each other's updates, and the frame loop never blocks on the
    // file write itself (settings commits block only on the receipt).
    let (config_write_tx, config_write_rx) = std::sync::mpsc::channel::<ConfigWriteJob>();
    std::thread::Builder::new()
        .name("aegis-config-write".into())
        .spawn(move || {
            while let Ok(job) = config_write_rx.recv() {
                let result = job.store.apply(job.edit).map_err(|error| error.to_string());
                match job.receipt {
                    Some(receipt) => {
                        let _ = receipt.send(result);
                    }
                    None => {
                        if let Err(e) = result {
                            log::warn!("{e}");
                        }
                    }
                }
            }
        })
        .expect("spawn config write worker");
    let config_writer = ConfigWriter {
        store: config_path.clone().map(aegis_config::ConfigStore::new),
        tx: config_write_tx,
    };

    // IPC and introspection surface (ADR-0027). A unix socket at
    // `$XDG_RUNTIME_DIR/aegis.sock` serves the `query` capability over a
    // snapshot shared with the main loop via an `Arc`. Connection threads
    // read the snapshot; the main loop writes it each frame. `control`/
    // `session` commands come back through `ipc_cmd_rx` and are applied on
    // this thread. Bind failure is non-fatal so the compositor runs without
    // IPC rather than crashing. `ipc` is held to the end of `run()` so its
    // `Drop` removes the socket.
    let (ipc_cmd_tx, ipc_cmd_rx) = std::sync::mpsc::channel::<IpcCommandRequest>();
    let (system_control_tx, system_control_rx) = std::sync::mpsc::channel::<SystemControlRequest>();
    let (capture_tx, capture_rx) = std::sync::mpsc::channel::<CaptureRequest>();
    let (realm_control_tx, realm_control_rx) = std::sync::mpsc::channel::<RealmControlRequest>();
    let (settings_control_tx, settings_control_rx) =
        std::sync::mpsc::channel::<SettingsControlRequest>();
    let (wallpaper_control_tx, wallpaper_control_rx) =
        std::sync::mpsc::channel::<WallpaperControlRequest>();
    let (realm_capture_tx, realm_capture_rx) = std::sync::mpsc::channel::<RealmCaptureRequest>();
    let (stream_control_tx, stream_control_rx) = std::sync::mpsc::channel::<StreamControlRequest>();
    let (idle_control_tx, idle_control_rx) = std::sync::mpsc::channel::<IdleControlRequest>();
    let (pick_control_tx, pick_control_rx) = std::sync::mpsc::channel::<PickControlRequest>();
    let (app_pick_control_tx, app_pick_control_rx) =
        std::sync::mpsc::channel::<AppPickControlRequest>();
    let (secret_prompt_control_tx, secret_prompt_control_rx) =
        std::sync::mpsc::channel::<SecretPromptControlRequest>();
    let (confirm_pick_control_tx, confirm_pick_control_rx) =
        std::sync::mpsc::channel::<ConfirmPickControlRequest>();
    let (capability_pick_control_tx, capability_pick_control_rx) =
        std::sync::mpsc::channel::<CapabilityPickControlRequest>();
    let (journal_refusal_tx, journal_refusal_rx) =
        std::sync::mpsc::channel::<JournalRefusalRequest>();
    let (auth_event_tx, auth_event_rx) = std::sync::mpsc::channel::<AuthEventRequest>();
    let journal =
        std::sync::Arc::new(std::sync::Mutex::new(aegis_ipc::Journal::default_capacity()));
    let (agent_registry, grant_store) = match std::env::var_os("XDG_DATA_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".local/share"))
        }) {
        Some(base) => (
            PrincipalRegistry::load(base.join("aegis/principals.json")),
            GrantStore::load(base.join("aegis/grants.json")),
        ),
        None => {
            log::warn!(
                "no XDG_DATA_HOME/HOME: agent principal registry and grants are session-only"
            );
            (PrincipalRegistry::in_memory(), GrantStore::in_memory())
        }
    };
    let agent_lockdown = config.as_ref().map_or(true, |config| config.agent.lockdown);
    let live = std::sync::Arc::new(LiveState::new(
        LiveChannels {
            commands: ipc_cmd_tx,
            system_controls: system_control_tx,
            capture: capture_tx,
            realm_controls: realm_control_tx,
            settings_controls: settings_control_tx,
            wallpaper_controls: wallpaper_control_tx,
            realm_capture: realm_capture_tx,
            stream_controls: stream_control_tx,
            idle_controls: idle_control_tx,
            pick_controls: pick_control_tx,
            app_pick_controls: app_pick_control_tx,
            secret_prompt_controls: secret_prompt_control_tx,
            confirm_pick_controls: confirm_pick_control_tx,
            capability_pick_controls: capability_pick_control_tx,
            journal_refusals: journal_refusal_tx,
            auth_events: auth_event_tx,
        },
        capture_worker.delivery_gate(),
        std::sync::Arc::clone(&notif_queue),
        std::sync::Arc::clone(&journal),
        builtin_ipc_scopes(),
        agent_registry,
        grant_store,
        agent_lockdown,
    ));
    let settings_revision = 0;
    live.set_settings(aegis_ipc::SettingsSnapshot {
        revision: settings_revision,
        touchpad: system_status.touchpad.clone(),
        display: system_status.display.clone(),
        preferences: desktop_preferences.clone(),
        idle: config
            .as_ref()
            .map(|config| config.idle)
            .unwrap_or_default(),
    });
    live.set_system_status(system_status.clone());
    let ipc: Option<aegis_ipc::Server> = match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(d) => {
            let path = std::path::PathBuf::from(d).join("aegis.sock");
            match aegis_ipc::Server::start(&path, std::sync::Arc::clone(&live)) {
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
    // Start the policy client only after both the Wayland and IPC sockets are
    // published. It is supervised for the lifetime of this runtime and
    // inherits the exact session environment advertised above.
    let idle_process = session::IdleProcess::start(
        config
            .as_ref()
            .map(|config| config.idle)
            .unwrap_or_default(),
        host.name() == "nested",
        ipc.is_some(),
    );
    // Signature of the last broadcast window set, used to detect changes.
    let last_win_sig: Option<WindowEventSignature> = None;
    let last_space_use = None;
    // Content hashes/revisions of the last fanned-out snapshots. The frame
    // loop rebuilds the owned snapshots only when these move; chrome and IPC
    // keep the previously pushed copy otherwise.
    let last_windows_hash: Option<u64> = None;
    let last_ws_sig: Option<u64> = None;
    let last_realm_revision: Option<u64> = None;
    let last_outputs_revision: Option<u64> = None;
    let previous_agent_suspended = false;
    let automatically_paused_realms = std::collections::BTreeSet::new();
    // Whether chrome reported a multi-frame animation in flight last frame.
    // While true the loop pumps non-blocking dispatches and renders at the
    // output's refresh cadence so the animation advances even with the
    // pointer still; once it rests the loop goes back to blocking on the
    // host event queue.
    let animating = false;
    // Pointer ownership at the end of the previous input batch. Keeping the
    // edge lets us send exactly one wl_pointer.leave when entering chrome and
    // synthesize motion before a click that returns to client content.
    let chrome_pointer_captured = false;
    // Synthetic pointer movement is independent of the nested host's physical
    // cursor. The next physical pointer event realigns the server before a
    // human button/axis event is delivered, preventing a click at stale
    // synthetic coordinates.
    let synthetic_pointer_active = false;
    let last_cursor_shape = 0u32;
    let last_cursor_hidden = false;
    // Runtime application rescan: package managers and user-created desktop
    // entries become visible in launcher/dock during a long-running session.
    // The scan decodes icon files — far too slow for the frame loop — so a
    // worker thread does the reading and decoding, and the main loop only
    // applies results (GPU texture upload + catalog swap) when they arrive.
    const APP_RESCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);
    let next_app_scan = std::time::Instant::now() + APP_RESCAN_INTERVAL;
    let (scan_req_tx, scan_req_rx) = std::sync::mpsc::channel::<AppScanRequest>();
    let (scan_result_tx, scan_result_rx) = std::sync::mpsc::channel::<AppScanResult>();
    std::thread::Builder::new()
        .name("aegis-app-scan".into())
        .spawn(move || {
            while let Ok(request) = scan_req_rx.recv() {
                let theme = request.icon_theme;
                let catalog = application_catalog(&theme, request.scale);
                let snapshot = snapshot_icons(&catalog);
                let decoded = decode_icons(&catalog, &theme, request.scale);
                if scan_result_tx
                    .send((theme, request.scale, catalog, snapshot, decoded))
                    .is_err()
                {
                    break;
                }
            }
        })
        .expect("spawn app scanner");
    let previous_render_at = std::time::Instant::now();
    let agent_activity_sequence = 0;

    CompositorRuntime {
        notif_queue,
        config_path,
        config,
        device,
        host,
        surface,
        canvas,
        launcher_backdrop,
        screenshot_freeze,
        pending_capture,
        capture_worker,
        streams: OutputStreams::new(),
        stream_job_in_flight: false,
        cursor_cache,
        screenshot_dir,
        server,
        icon_theme,
        icon_scale,
        launcher_apps,
        icon_cache,
        icon_snapshot,
        shell,
        input_acc,
        gesture_map,
        swipe: None,
        renderer,
        realm_processes,
        realm_render_targets,
        pending_realm_capture,
        realm_damage_sequence,
        agent_activity_sequence,
        start,
        wallpaper,
        clear,
        frame_count,
        retired_defer,
        primary_plane_state: PrimaryPlaneState::default(),
        scanout_telemetry: ScanoutTelemetry::new(),
        keyboard_capture,
        keymap,
        system_status,
        status_rx,
        status_refresh_tx,
        resource_rx,
        config_writer,
        reload,
        idle_process,
        quit_requested,
        ipc_cmd_rx,
        system_control_rx,
        capture_rx,
        realm_control_rx,
        settings_control_rx,
        wallpaper_control_rx,
        realm_capture_rx,
        stream_control_rx,
        idle_control_rx,
        pick_rx: pick_control_rx,
        pending_pick: None,
        pending_pick_open: None,
        app_pick_rx: app_pick_control_rx,
        pending_app_pick: None,
        secret_prompt_rx: secret_prompt_control_rx,
        pending_secret_prompt: None,
        confirm_pick_rx: confirm_pick_control_rx,
        pending_confirm_pick: None,
        capability_pick_rx: capability_pick_control_rx,
        pending_capability_pick: None,
        ipc_idle_inhibits: IdleInhibits::default(),
        journal_refusal_rx,
        auth_event_rx,
        journal,
        live,
        ipc,
        last_win_sig,
        last_space_use,
        last_windows_hash,
        last_ws_sig,
        last_realm_revision,
        last_outputs_revision,
        last_surface_gens: std::collections::HashMap::new(),
        surface_gens_scratch: std::collections::HashMap::new(),
        last_notif_revision: None,
        last_chrome_mode: None,
        last_session_locked: false,
        last_presented_cursor: None,
        last_presented_cursor_position: None,
        composite_slot_damage: Vec::new(),
        last_present_minute: None,
        chrome_dirty: false,
        force_full_redraw: false,
        presentation: PresentationScheduler::new(),
        pending_frame: None,
        settings_revision,
        previous_agent_suspended,
        automatically_paused_realms,
        animating,
        chrome_pointer_captured,
        synthetic_pointer_active,
        last_cursor_shape,
        last_cursor_hidden,
        next_app_scan,
        scan_req_tx,
        scan_result_rx,
        previous_render_at,
    }
    .run_loop()
}
