//! The command panel: a full-screen modal overlay in a VR/AR personal-info
//! HUD language (ADR-0080) — deep blue-black translucent "dark glass"
//! floating panels with a cyan accent, thin hairlines, and corner brackets
//! over the standard dark blurred scrim.
//!
//! One centered cluster of surfaces: a header band with the user's
//! profile (avatar, display name, groups) on the left and a live machine
//! monitor (chassis glyph plus CPU/GPU/RAM/NET/DISK/BAT gauges fed by
//! `ChromeUpdate::ResourceStats`) on the right; below it the main panel,
//! topped by a flat tab bar holding the System quick-settings tab plus one
//! tab per available `aegis-settings` module (with the close button at its
//! right end); and the side column stacking the notification list over the
//! StatusNotifierItem tray. The System tab carries quick settings (volume,
//! brightness, radios, do-not-disturb) and the Agent Workspaces status row
//! that the HUD's dropped right chip once carried (ADR-0083); settings
//! module tabs host the module registry's pages and route their
//! `SettingsAction`s through `ChromeEvents::settings_actions`; the tray
//! keeps left-click activation with host-rendered dbusmenu popovers, and
//! the notification list keeps click-to-dismiss. The panel opens through
//! the `Super+S` keybinding or a four-finger touchpad swipe down, and
//! closes on Escape, a scrim click, the tab bar's close button, the same
//! binding, or a four-finger swipe up.
//!
//! Scope (ADR-0115): the panel is the display-and-control surface for
//! desktop-computer behavior — the domains daily desktop use involves:
//! sound, displays, network and Bluetooth, power, the session,
//! notifications, the tray, the user's persona, machine resources, and
//! desktop preferences. The scope test is the user's computer, not this
//! compositor: a surface belongs when its subject exists on any desktop
//! computer regardless of the compositor implementation. Compositor-
//! mechanism surfaces — window-tree internals, protocol or IPC state,
//! introspection, developer tooling — stay out and remain CLI/MCP
//! territory. Domains follow the user's model rather than the implementing
//! component, so window tiling and Agent Workspaces are in scope while
//! Interaction Domain lifecycle management is not.
//!
//! Like the HUD and the dock, the panel is compositor-owned lens
//! chrome on the [`aegis_shell`] `Chrome` seam: snapshots arrive through the
//! trait each frame, and user intents leave through `ChromeEvents` plus the
//! shared `aegis-tray` command channel.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};

use aegis_design::tokens::{Hud, TypeScale};
use aegis_design::{AvatarRole, Design, GlassRole, materials, themes};
use aegis_model::input::KeyChar;
use aegis_model::interaction_domain::{
    InteractionDomainKind, InteractionDomainSnapshot, InteractionDomainState,
};
use aegis_model::notify::{Notification, NotificationQueue};
use aegis_model::settings::{SettingsAction, SettingsSnapshot};
use aegis_model::window::{SpaceUse, Window};
use aegis_model::workspace::WorkspaceSnapshot;
use aegis_settings::builtin_settings_modules;
use aegis_settings::module::{ModuleAvailability, ModuleEvents, ModuleId, ModuleRegistry};
use aegis_shell::persona::{Portrait, PortraitConfig, PortraitWatcher, Profile};
use lens::{Align, Color, Frame, Input, LayoutOpts, Rect};

use aegis_shell::{
    BackdropRegion, ChassisKind, Chrome, ChromeCommand, ChromeEvents, ChromeUpdate, CursorShape,
    IconSet, LiquidGlassRegion, Localizer, Message, NetworkState, ResourceStats,
    SystemAction, SystemStatus, place_popup, truncate,
};
use aegis_tray::{MenuNode, MenuState, TrayCommand, TrayHandle, TrayIcon};

mod rendering;

use rendering::*;

#[cfg(test)]
mod tests;

const BACKDROP_BLUR_SIGMA: f32 = 14.0;
/// Layout constants for the 9-part visual HUD anchor architecture:
/// - Top-Left (左上): Compact user profile chip (48px avatar + name).
/// - Top-Right (右上): Floating notification stream.
/// - Center (中): Split Main Control Panel (Left Nav Rail + Right Tab View).
const PROFILE_W: f32 = 280.0;
const PROFILE_H: f32 = 72.0;
const MAIN_W: f32 = 720.0;
const MAIN_H: f32 = 460.0;
const NOTIF_W: f32 = 260.0;
const NOTIF_H: f32 = 200.0;
/// The tray panel's fixed height at the side column's bottom: a small
/// section header plus two tray-grid rows.
const TRAY_PANEL_H: f32 = 172.0;
/// The main panel's flat tab bar.
#[allow(dead_code)]
const TAB_BAR_H: f32 = 40.0;
const PANEL_GAP: f32 = 12.0;
/// Main panel's reveal lags the header's by this fraction.
const CONTENT_STAGGER: f32 = 0.18;
/// The side column's reveal lags the header's by this fraction.
const SIDE_STAGGER: f32 = 0.26;
/// Samples kept per sparkline metric (CPU/GPU/RAM).
const HISTORY_CAP: usize = 48;

/// The command panel explicitly owns its VRM composition. Keeping the
/// parameters here lets another host choose a different crop without changing
/// the VRM renderer or the shared portrait source policy.
const AVATAR_CAMERA: aegis_shell::persona::VrmCamera =
    aegis_shell::persona::VrmCamera::new(28.0, 0.25, 0.48, 0.0);

// dbusmenu popover geometry. Placement follows the shared shell popup policy.
const MENU_WIDTH: f32 = 236.0;
const MENU_PAD: f32 = 7.0;
const MENU_ROW_HEIGHT: f32 = 28.0;
const MENU_HEADER_HEIGHT: f32 = 23.0;
const MENU_SECTION_HEIGHT: f32 = 7.0;

// Tray grid geometry: cells are 76×64 inside an 84×72 pitch; the column
// count adapts to the content width.
#[allow(dead_code)]
const TRAY_CELL_W: f32 = 84.0;
#[allow(dead_code)]
const TRAY_CELL_H: f32 = 72.0;

fn presentation_anim_pending(
    reveal: f32,
    target: f32,
    avatar_playing: bool,
    avatar_reload_pending: bool,
) -> bool {
    (reveal - target).abs() > 0.002 || avatar_playing || avatar_reload_pending
}

/// The main panel's tabs: the System quick settings plus one tab per
/// available settings module from the registry, in registry order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Tab {
    System,
    Settings(ModuleId),
}

/// A click resolved inside the tab bar, applied after the render pass so
/// the bar can finish drawing before state changes.
enum TabAction {
    Select(Tab),
}

/// One row of the header band's machine monitor, in priority order; the
/// band shows at most five rows.
#[allow(dead_code)]
enum Gauge {
    /// CPU: sparkline from the sample history + percent.
    Cpu,
    /// GPU (only when the driver exposes a busy percent): bar + percent.
    Gpu(f32),
    Ram {
        fraction: f32,
        value: String,
    },
    /// Throughput figures span the bar and value cells; there is no bar.
    Net {
        value: String,
    },
    Disk {
        fraction: f32,
        value: String,
    },
    Battery {
        fraction: f32,
        value: String,
        charging: bool,
    },
}

/// Render-thread half of the StatusNotifierItem tray: the shared snapshot the
/// worker writes, the command channel back to it, and the texture cache the
/// panel uploads from item pixmaps. Same contract as the HUD's
/// read-only half, plus the command channel for interaction.
#[allow(dead_code)]
struct SniTray {
    device: flux::Device,
    handle: TrayHandle,
    /// Uploaded SNI textures keyed by item key, tagged with the snapshot's
    /// icon generation so status-only updates do not re-upload.
    textures: HashMap<String, (u64, flux::Image)>,
    /// Last frame's cells, reused when the snapshot lock is contended (the
    /// worker publishing a large menu tree must never stall rendering).
    cached_cells: Vec<SniCell>,
}

/// One SNI cell to draw this frame, distilled from the tray snapshot.
#[allow(dead_code)]
#[derive(Clone)]
struct SniCell {
    key: String,
    title: String,
    has_menu: bool,
    textured: bool,
    /// Cell rect (filled in during the tray-grid layout pass).
    rect: Rect,
}

/// The per-cell visuals for the tray grid, distilled before the layout
/// closures so they borrow no `self` state.
#[allow(dead_code)]
struct TrayCellVisual {
    key: String,
    title: String,
    has_menu: bool,
    texture: Option<*mut lens::sys::flux_image>,
    fallback: Option<*mut lens::sys::flux_image>,
}

/// Render-side cache of the shared dbusmenu tree, tagged with the worker's
/// `menu_revision` so the (potentially large) tree is re-cloned only when the
/// menu actually changed.
struct MenuSnapshotCache {
    revision: u64,
    menu: Option<Arc<MenuState>>,
}

/// The modal command panel.
pub struct CommandPanel {
    open: bool,
    /// Eased reveal amount; kept while closing so the surfaces fade and
    /// slide out instead of vanishing in one frame.
    reveal: f32,
    /// The main panel's active tab.
    tab: Tab,
    /// Settings module registry behind the module tabs; modules own their
    /// draft/apply state and render into the active tab's body.
    modules: ModuleRegistry,
    /// Latest persistent-settings snapshot, seeded and refreshed by the
    /// shell; module tabs render a placeholder until the first one arrives.
    settings: Option<SettingsSnapshot>,
    /// The shell-wide design snapshot the hosted settings modules paint
    /// from, seeded and refreshed by `ChromeUpdate::Appearance`.
    design: Design,
    /// Accessibility reduced-motion policy shared with the other chrome.
    reduced_motion: bool,
    prev_down: bool,
    status: SystemStatus,
    /// Latest host utilization sample behind the header band's gauges.
    stats: ResourceStats,
    /// Sparkline histories for the header band's CPU/GPU/RAM gauges.
    cpu_history: History,
    gpu_history: History,
    ram_history: History,
    /// Local account behind the header band's profile zone, resolved once
    /// at construction (never per frame).
    profile: Profile,
    /// Header-band portrait selected by the shared profile contract. Without
    /// configured content, initials render over a flat host-owned disc; no
    /// fallback texture is synthesized by either resource layer.
    avatar: Option<Portrait>,
    /// Immutable source order shared with the watcher and every reload.
    portrait_config: PortraitConfig,
    /// Non-owning view of the compositor device, used only on the render
    /// thread to construct a complete hot-reload replacement.
    avatar_device: Option<flux::Device>,
    /// Filesystem observation is notification-only; all decode and GPU work
    /// stays on the render thread.
    avatar_watcher: Option<PortraitWatcher>,
    /// One-shot latch so an animated avatar's advance failure logs once.
    avatar_warned: bool,
    /// Agent Interaction Domain aggregate behind the System section's Agent Workspaces
    /// status row (ADR-0083).
    interaction_domains: InteractionDomainSnapshot,
    icons: IconSet,
    notifications: Arc<Mutex<NotificationQueue>>,
    /// Notification list cache keyed by the queue's revision; re-cloned only
    /// when the queue actually changes.
    notification_cache: Option<(u64, Arc<Vec<Notification>>)>,
    tray: Option<SniTray>,
    /// SNI item key whose dbusmenu popover is showing (set on the right-click
    /// that opens it, cleared by click-away, item disappearance, tab change,
    /// or panel close).
    menu_open_for: Option<String>,
    /// Breadcrumb of submenu ids from root to the current view; `path[0]` is
    /// always the root sentinel (0), each later id descends one level.
    menu_path: Vec<i32>,
    /// The tray cell rect that opened the popover (kept current each frame so
    /// `place_popup` re-anchors on relayout).
    menu_owner: Rect,
    /// One-frame flag suppressing the same right-press that opened the menu
    /// from also immediately closing it.
    menu_just_opened: bool,
    menu_cache: Option<MenuSnapshotCache>,
}

impl CommandPanel {
    #[cfg(test)]
    fn toggle_command_panel(&mut self, out: &mut ChromeEvents) {
        <Self as Chrome>::command(self, &ChromeCommand::ToggleCommandPanel, out);
    }

    #[cfg(test)]
    fn set_reduced_motion(&mut self, reduced: bool) {
        <Self as Chrome>::update(self, ChromeUpdate::ReducedMotion(reduced));
    }

    #[cfg(test)]
    fn update_windows(&mut self, windows: &[Window]) {
        <Self as Chrome>::update(self, ChromeUpdate::Windows(windows));
    }

    #[cfg(test)]
    fn update_resource_stats(&mut self, stats: &ResourceStats) {
        <Self as Chrome>::update(self, ChromeUpdate::ResourceStats(stats));
    }

    /// Construct the panel. The flux device is borrowed (non-owning, like
    /// [`aegis_shell::Shell::new`]) to upload SNI tray pixmaps to the GPU;
    /// the caller must keep it alive past the panel. The tray handle comes
    /// from the composition root's single `aegis_tray::spawn()` shared with
    /// the HUD; `None` leaves the tray section empty.
    pub fn new(
        device: &flux::Device,
        tray: Option<TrayHandle>,
        notifications: Arc<Mutex<NotificationQueue>>,
    ) -> CommandPanel {
        let tray = tray.map(|handle| {
            // SAFETY: the composition root declares its flux device before
            // the shell (and thus this panel) and drops it after, and the
            // panel only touches the device on the render thread.
            let device = unsafe { flux::Device::borrow_raw(device.as_raw()) };
            SniTray {
                device,
                handle,
                textures: HashMap::new(),
                cached_cells: Vec::new(),
            }
        });
        // Source precedence and still/VRM routing come from the shared
        // profile contract. The panel supplies only its VRM camera and owns
        // the outer disc, keyline, and reveal treatment.
        let portrait_config = PortraitConfig::current();
        let avatar = match Portrait::load_transactional(device, &portrait_config, AVATAR_CAMERA) {
            Ok(loaded) => loaded,
            Err(error) => {
                log::warn!("command-panel: avatar load failed, using initials: {error}");
                None
            }
        };
        // SAFETY: the composition root owns the device and drops the shell
        // (including this panel) before the device. Chrome methods run on the
        // compositor render thread.
        let avatar_device = unsafe { flux::Device::borrow_raw(device.as_raw()) };
        let avatar_watcher = match PortraitWatcher::new(&portrait_config) {
            Ok(watcher) => Some(watcher),
            Err(error) => {
                log::warn!("command-panel: avatar hot reload disabled: {error}");
                None
            }
        };
        let open_on_start = cfg!(debug_assertions)
            && std::env::var_os("AEGIS_COMMAND_PANEL_OPEN").is_some_and(|value| !value.is_empty());
        if cfg!(debug_assertions) {
            if let Ok(mut queue) = notifications.lock() {
                if queue.snapshot().is_empty() {
                    queue.push(
                        "战术网络",
                        "已建立量子加密链路，节点 0x7F 延迟 4ms",
                        Some("aegis-net".into()),
                        1000,
                    );
                    queue.push(
                        "核心遥测",
                        "GPU Shader 预热完成，显存动态分配 384MB",
                        Some("aegis-gpu".into()),
                        2000,
                    );
                    queue.push(
                        "安全总线",
                        "Agent 交互域沙箱环境完整性验证通过",
                        Some("aegis-security".into()),
                        3000,
                    );
                    queue.push(
                        "电源管理",
                        "已开启高能效模式，当前电池电量 92% (充电中)",
                        Some("aegis-power".into()),
                        4000,
                    );
                }
            }
        }
        CommandPanel {
            open: open_on_start,
            reveal: if open_on_start { 1.0 } else { 0.0 },
            tab: Tab::System,
            modules: builtin_settings_modules(),
            settings: None,
            design: Design::dark(),
            reduced_motion: false,
            prev_down: false,
            status: SystemStatus::default(),
            stats: ResourceStats::default(),
            cpu_history: History::new(HISTORY_CAP),
            gpu_history: History::new(HISTORY_CAP),
            ram_history: History::new(HISTORY_CAP),
            profile: Profile::current().unwrap_or_else(|_| Profile::fallback()),
            avatar,
            portrait_config,
            avatar_device: Some(avatar_device),
            avatar_watcher,
            avatar_warned: false,
            interaction_domains: aegis_model::interaction_domain::InteractionDomainModel::new()
                .snapshot(),
            icons: IconSet::default(),
            notifications,
            notification_cache: None,
            tray,
            menu_open_for: None,
            menu_path: Vec::new(),
            menu_owner: Rect {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            },
            menu_just_opened: false,
            menu_cache: None,
        }
    }

    /// Test/preview constructor without a GPU device, tray, or notification
    /// source.
    #[cfg(test)]
    fn without_sources() -> CommandPanel {
        let (notifications, _) = (Arc::new(Mutex::new(NotificationQueue::new(3_600_000))), ());
        CommandPanel {
            open: false,
            reveal: 0.0,
            tab: Tab::System,
            modules: builtin_settings_modules(),
            settings: None,
            design: Design::dark(),
            reduced_motion: false,
            prev_down: false,
            status: SystemStatus::default(),
            stats: ResourceStats::default(),
            cpu_history: History::new(HISTORY_CAP),
            gpu_history: History::new(HISTORY_CAP),
            ram_history: History::new(HISTORY_CAP),
            profile: Profile::current().unwrap_or_else(|_| Profile::fallback()),
            avatar: None,
            portrait_config: PortraitConfig::new(Vec::new()),
            avatar_device: None,
            avatar_watcher: None,
            avatar_warned: false,
            interaction_domains: aegis_model::interaction_domain::InteractionDomainModel::new()
                .snapshot(),
            icons: IconSet::default(),
            notifications,
            notification_cache: None,
            tray: None,
            menu_open_for: None,
            menu_path: Vec::new(),
            menu_owner: Rect {
                x: 0.0,
                y: 0.0,
                w: 0.0,
                h: 0.0,
            },
            menu_just_opened: false,
            menu_cache: None,
        }
    }

    /// Whether the panel currently owns the chrome layer: open, or still
    /// animating closed.
    fn active(&self) -> bool {
        self.open || self.reveal > 0.01
    }

    fn avatar_reload_pending(&self) -> bool {
        self.avatar_watcher
            .as_ref()
            .is_some_and(PortraitWatcher::needs_poll)
    }

    /// Rebuild on the render thread and publish only a complete replacement.
    /// A failed decode/upload leaves the currently displayed GPU resources
    /// untouched and schedules a bounded retry.
    fn reload_avatar_if_ready(&mut self) {
        let ready = self
            .avatar_watcher
            .as_mut()
            .is_some_and(PortraitWatcher::poll);
        if !ready {
            return;
        }
        if let Some(watcher) = &mut self.avatar_watcher
            && let Err(error) = watcher.refresh()
        {
            log::warn!("command-panel: could not refresh avatar watches: {error}");
        }
        let Some(device) = &self.avatar_device else {
            return;
        };
        let previous_motion = self
            .avatar
            .as_ref()
            .and_then(Portrait::current_motion)
            .map(str::to_owned);
        match Portrait::load_transactional(device, &self.portrait_config, AVATAR_CAMERA) {
            Ok(Some(mut replacement)) => {
                let restored = previous_motion
                    .as_deref()
                    .is_some_and(|name| replacement.play_motion(name));
                if !restored && self.open {
                    replacement.play_random_action();
                }
                self.avatar = Some(replacement);
                self.avatar_warned = false;
                log::info!("command-panel: avatar hot reloaded");
            }
            Ok(None) => {
                self.avatar = None;
                self.avatar_warned = false;
                log::info!("command-panel: avatar removed, using initials");
            }
            Err(error) => {
                log::warn!("command-panel: avatar hot reload failed, keeping current: {error}");
                if let Some(watcher) = &mut self.avatar_watcher {
                    watcher.retry();
                }
            }
        }
    }

    fn advance(&mut self, dt: f32) {
        let target = if self.open { 1.0 } else { 0.0 };
        if self.reduced_motion {
            self.reveal = target;
            return;
        }
        let dt = dt.clamp(0.0, 1.0 / 15.0);
        let follow = 1.0 - (-15.0 * dt).exp();
        self.reveal += (target - self.reveal) * follow;
        if (target - self.reveal).abs() < 0.002 {
            self.reveal = target;
        }
    }

    /// Close the panel, also dismissing any open dbusmenu popover.
    fn close(&mut self) {
        self.open = false;
        if let Some(key) = self.menu_open_for.take() {
            self.menu_path.clear();
            self.menu_just_opened = false;
            self.send_tray_command(TrayCommand::CloseMenu { key });
        }
    }

    fn select_tab(&mut self, tab: Tab) {
        if self.tab != tab {
            self.tab = tab;
            // A tab switch drops an open tray popover even though the tray
            // itself stays visible in the side column, matching the old
            // section-switch semantics.
            if let Some(key) = self.menu_open_for.take() {
                self.menu_path.clear();
                self.send_tray_command(TrayCommand::CloseMenu { key });
            }
        }
    }

    /// The profile panel (screen top-left), notifications panel (screen top-right),
    /// main control panel (screen center), and side column (machine monitor over tray,
    /// Calculate panel bounds based on the 9-region spatial anchor system:
    /// - Top-Left (左上): User Profile Chip
    /// - Top-Right (右上): Notifications Stream
    /// - Center (中): Split Main Control Panel (Left Nav Rail + Right Tab View)
    fn cluster_bounds(display: (f32, f32)) -> (Rect, Rect, Rect, Rect) {
        let margin_x = (display.0 * 0.025).clamp(16.0, 48.0);
        let margin_y = (display.1 * 0.035).clamp(16.0, 40.0);
        let gap = PANEL_GAP;

        // 1. Top-Left Anchor (左上): User Profile Chip
        let profile_w = PROFILE_W.min((display.0 * 0.35).max(140.0));
        let profile_h = PROFILE_H.min((display.1 * 0.18).max(48.0));
        let profile = Rect {
            x: margin_x,
            y: margin_y,
            w: profile_w.min((display.0 - margin_x * 2.0).max(1.0)),
            h: profile_h.min((display.1 - margin_y * 2.0).max(1.0)),
        };

        // 2. Top-Right Anchor (右上): Notifications Stream
        let notif_w = NOTIF_W.min((display.0 * 0.38).max(140.0));
        let notif_h = NOTIF_H.min((display.1 * 0.38).max(64.0));
        let notif_x = (display.0 - margin_x - notif_w).max(profile.x + profile.w + gap);
        let notifications = Rect {
            x: notif_x,
            y: margin_y,
            w: notif_w.min((display.0 - notif_x - margin_x).max(1.0)),
            h: notif_h.min((display.1 - margin_y * 2.0).max(1.0)),
        };

        // 3. Center Anchor (中): Split Main Control Panel (Nav Rail + Content View)
        let main_w = MAIN_W.min((display.0 - margin_x * 2.0).max(1.0));
        let main_h = MAIN_H.min((display.1 - margin_y * 2.0).max(1.0));
        let main_x = ((display.0 - main_w) * 0.5).max(margin_x);
        let main_y = ((display.1 - main_h) * 0.5).max(margin_y);
        let main = Rect {
            x: main_x,
            y: main_y,
            w: main_w.min((display.0 - main_x - margin_x).max(1.0)),
            h: (display.1 - main_y - margin_y).min(main_h).max(1.0),
        };

        // 4. Side Column (System Telemetry): Temporarily disabled / zero bounds
        let side = Rect {
            x: display.0,
            y: display.1,
            w: 0.0,
            h: 0.0,
        };

        (profile, main, notifications, side)
    }

    /// Clone the notification queue, memoized on the queue's revision: an
    /// unchanged queue reuses the cached `Arc` instead of re-cloning every
    /// entry every frame.
    fn notification_snapshot(&mut self) -> Arc<Vec<Notification>> {
        let queue = self.notifications.lock().unwrap();
        let revision = queue.revision();
        let stale = self
            .notification_cache
            .as_ref()
            .map(|(cached, _)| *cached != revision)
            .unwrap_or(true);
        if stale {
            self.notification_cache = Some((revision, Arc::new(queue.snapshot())));
        }
        Arc::clone(&self.notification_cache.as_ref().unwrap().1)
    }

    fn send_tray_command(&self, command: TrayCommand) {
        if let Some(tray) = &self.tray {
            // The worker may be gone (bus disconnected); clicks just drop.
            let _ = tray.handle.send(command);
        }
    }

    fn themed_icon(&self, name: &str) -> Option<*mut c_void> {
        self.icons.get(&format!("aegis-hud:{name}"))
    }

    /// Read the shared menu snapshot, memoized on the worker's
    /// `menu_revision` so the menu tree is re-cloned only when it changes. A
    /// contended snapshot lock (the worker publishing a large tree) serves
    /// the cached menu rather than blocking the frame.
    fn menu_snapshot(&mut self) -> Option<Arc<MenuState>> {
        let tray = self.tray.as_ref()?;
        if let Ok(snapshot) = tray.handle.snapshot().try_lock() {
            let stale = self
                .menu_cache
                .as_ref()
                .map(|cache| cache.revision != snapshot.menu_revision)
                .unwrap_or(true);
            if stale {
                self.menu_cache = Some(MenuSnapshotCache {
                    revision: snapshot.menu_revision,
                    menu: snapshot.menu.clone().map(Arc::new),
                });
            }
        }
        self.menu_cache.as_ref()?.menu.clone()
    }

    /// Read the SNI snapshot under a brief lock, upload any new or changed
    /// icons into the texture cache, and return the visible cells for this
    /// frame. Runs on the render thread; never touches D-Bus. When the worker
    /// holds the snapshot lock the previous frame's cells are reused.
    #[allow(dead_code)]
    fn sni_cells(&mut self) -> Vec<SniCell> {
        let Some(tray) = &mut self.tray else {
            return Vec::new();
        };
        let Ok(snapshot) = tray.handle.snapshot().try_lock() else {
            return tray.cached_cells.clone();
        };
        tray.textures
            .retain(|key, _| snapshot.items.iter().any(|item| &item.key == key));
        let mut cells = Vec::new();
        for item in &snapshot.items {
            if !item.is_visible() {
                continue;
            }
            let stale = tray
                .textures
                .get(&item.key)
                .map(|(generation, _)| *generation != item.icon_generation)
                .unwrap_or(true);
            if let TrayIcon::Pixmap(pixmap) = &item.icon {
                if stale {
                    match flux::Image::from_bytes(
                        &tray.device,
                        pixmap.width,
                        pixmap.height,
                        flux::Format::FLUX_FORMAT_BGRA8_UNORM,
                        &pixmap.bgra,
                    ) {
                        Ok(image) => {
                            tray.textures
                                .insert(item.key.clone(), (item.icon_generation, image));
                        }
                        Err(error) => {
                            log::warn!("tray: icon upload for {} failed: {error}", item.key);
                            tray.textures.remove(&item.key);
                        }
                    }
                }
            } else {
                // `None` ships no icon; a `Name` left in the snapshot means
                // the worker's theme resolution failed. Either way the item
                // must not keep rendering the previous texture.
                tray.textures.remove(&item.key);
            }
            cells.push(SniCell {
                key: item.key.clone(),
                title: item.title.clone(),
                has_menu: item.has_menu,
                textured: tray.textures.contains_key(&item.key),
                rect: Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.0,
                    h: 0.0,
                },
            });
        }
        tray.cached_cells = cells.clone();
        cells
    }

    fn close_menu(&mut self, key: String) {
        self.menu_open_for = None;
        self.menu_path.clear();
        self.menu_just_opened = false;
        self.send_tray_command(TrayCommand::CloseMenu { key });
    }
}

mod presentation;
impl Chrome for CommandPanel {
    fn render(
        &mut self,
        f: &mut Frame,
        input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let raw = input.as_raw();
        let dt = raw.dt_seconds.max(0.0);
        self.advance(dt);
        self.reload_avatar_if_ready();
        let display = (raw.display_size.x, raw.display_size.y);
        let cursor = (raw.cursor.x, raw.cursor.y);
        let down = raw.mouse_down.first().copied().unwrap_or(false);
        let pressed = down && !self.prev_down;
        if !self.active() {
            self.prev_down = down;
            return;
        }
        // An animated avatar advances with the panel's frames; static
        // avatars and the initials fallback do no GPU work, so the panel
        // stays event-driven off `anim_pending`.
        if self.open
            && self.avatar.as_ref().is_some_and(Portrait::is_animated)
            && let Some(avatar) = &mut self.avatar
            && let Err(error) = avatar.advance(dt)
            && !self.avatar_warned
        {
            log::warn!("command-panel: avatar advance failed: {error}");
            self.avatar_warned = true;
        }
        let reveal = self.reveal.clamp(0.0, 1.0);
        let (profile_rect, main_rect, notifications_rect, side_rect) =
            Self::cluster_bounds(display);
        let tray_h = TRAY_PANEL_H.min((side_rect.h - PANEL_GAP - 60.0).max(56.0));
        let machine_rect = Rect {
            x: side_rect.x,
            y: side_rect.y,
            w: side_rect.w,
            h: (side_rect.h - tray_h - PANEL_GAP).max(1.0),
        };
        let tray_rect = Rect {
            x: side_rect.x,
            y: side_rect.y + side_rect.h - tray_h,
            w: side_rect.w,
            h: tray_h,
        };

        // Click-away: a press landing on none of the cluster's surfaces nor
        // an open tray popover dismisses the panel.
        let on_popover = self
            .open_popover_bounds(display)
            .map(|rect| contains(rect, cursor.0, cursor.1))
            .unwrap_or(false);
        if pressed
            && !contains(profile_rect, cursor.0, cursor.1)
            && !contains(notifications_rect, cursor.0, cursor.1)
            && !contains(main_rect, cursor.0, cursor.1)
            && !contains(side_rect, cursor.0, cursor.1)
            && !on_popover
        {
            self.close();
        }

        // Tabs stop accepting presses while the panel is closing.
        let pressed = pressed && self.open;

        let profile_progress = stagger(reveal, 0.0);
        let notif_progress = stagger(reveal, 0.0);
        let content_progress = ease_out_cubic(stagger(reveal, CONTENT_STAGGER));
        let side_progress = ease_out_cubic(stagger(reveal, SIDE_STAGGER));
        // lens stamps each built node with the context opacity, so one
        // switch fades every element of a section — text, icons, images,
        // sliders, scrollbars; the switch is restored after the cluster
        // (lens also resets it at every frame begin).
        f.set_opacity(profile_progress);
        self.render_profile_panel(f, profile_rect, profile_progress, i18n);
        f.set_opacity(notif_progress);
        self.render_notifications_panel(f, notifications_rect, notif_progress, i18n, out);
        f.set_opacity(content_progress);
        self.render_main_panel(f, main_rect, content_progress, i18n, out);
        // System telemetry / side monitor temporarily disabled per user direction.
        let _ = (side_progress, machine_rect, tray_rect);

        // The dbusmenu popover floats above the panels; it belongs to the
        // tray, so it fades with the side column.
        if self.menu_open_for.is_some()
            && let Some(menu) = self.menu_snapshot()
            && Some(&menu.key) == self.menu_open_for.as_ref()
        {
            self.render_tray_menu(f, &menu, display, cursor, pressed);
        }
        f.set_opacity(1.0);

        self.prev_down = down;
    }

    fn captures_keyboard(&self) -> bool {
        self.active()
    }

    fn key_char(&mut self, kc: &KeyChar, _out: &mut ChromeEvents) {
        if kc.keysym != aegis_model::input::XKB_KEY_Escape || !self.open {
            return;
        }
        // Escape peels the innermost surface first: an open tray menu, then
        // the panel itself.
        if let Some(key) = self.menu_open_for.clone() {
            self.close_menu(key);
        } else {
            self.close();
        }
    }

    fn command(&mut self, command: &ChromeCommand<'_>, _out: &mut ChromeEvents) {
        if matches!(command, ChromeCommand::ToggleCommandPanel) {
            if self.open {
                self.close();
            } else {
                self.open = true;
                if let Some(name) = self
                    .avatar
                    .as_mut()
                    .and_then(|avatar| avatar.play_random_action().map(str::to_owned))
                {
                    log::debug!("command-panel: playing avatar action {name:?}");
                }
            }
        }
    }

    fn command_panel_active(&self) -> bool {
        self.active()
    }

    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        // Modal: the whole screen belongs to the panel (scrim click-away
        // included) while it is open or animating.
        self.active()
    }

    fn modal_active(&self) -> bool {
        self.active()
    }

    fn exclusive_presentation_active(&self) -> bool {
        self.active()
    }

    /// The panel renders while modal — it *is* the modal component.
    fn visible_during_modal(&self) -> bool {
        true
    }

    fn cursor_shape_at(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        self.active().then_some(CursorShape::Pointer)
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::AppCatalog(catalog) => self.icons = catalog.icons.clone(),
            ChromeUpdate::SystemStatus(status) => self.status = status.clone(),
            ChromeUpdate::ResourceStats(stats) => {
                self.stats = *stats;
                self.cpu_history.push(stats.cpu_percent);
                if let Some(gpu) = stats.gpu_percent {
                    self.gpu_history.push(gpu);
                }
                let ram_percent = if stats.mem_total_bytes > 0 {
                    stats.mem_used_bytes as f32 / stats.mem_total_bytes as f32 * 100.0
                } else {
                    0.0
                };
                self.ram_history.push(ram_percent);
            }
            ChromeUpdate::InteractionDomains(snapshot) => {
                self.interaction_domains = snapshot.clone();
            }
            ChromeUpdate::Appearance(design) => {
                self.design = *design;
            }
            ChromeUpdate::Settings(snapshot) => {
                self.settings = Some(snapshot.clone());
                self.modules.update_settings(snapshot);
            }
            ChromeUpdate::Windows(windows) => {
                // A fullscreen window owns the whole output; get out of its way.
                if SpaceUse::from_windows(windows) == SpaceUse::Fullscreen && self.open {
                    self.close();
                }
            }
            ChromeUpdate::ReducedMotion(reduced) => {
                self.reduced_motion = reduced;
                if reduced {
                    self.reveal = if self.open { 1.0 } else { 0.0 };
                }
            }
            _ => {}
        }
    }

    fn anim_pending(&self) -> bool {
        let target = if self.open { 1.0 } else { 0.0 };
        presentation_anim_pending(
            self.reveal,
            target,
            self.open && self.avatar.as_ref().is_some_and(Portrait::is_animated),
            self.avatar_reload_pending(),
        )
    }

    fn requires_composition(&self) -> bool {
        self.active()
    }

    fn backdrop_blur_sigma(&self) -> f32 {
        if self.active() {
            BACKDROP_BLUR_SIGMA
        } else {
            0.0
        }
    }

    fn backdrop_regions(
        &self,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        // The command panel is boundless floating chrome: individual components
        // declare their own stable `capture_bounds` footprint through
        // `liquid_glass_regions`, avoiding expensive and flicker-prone fullscreen
        // backdrop captures on cursor movement.
        Vec::new()
    }

    fn liquid_glass_regions(
        &self,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<LiquidGlassRegion> {
        if !self.active() {
            return Vec::new();
        }
        let reveal = self.reveal.clamp(0.0, 1.0);
        let profile_progress = stagger(reveal, 0.0);
        let notif_progress = stagger(reveal, 0.0);
        let content_progress = ease_out_cubic(stagger(reveal, CONTENT_STAGGER));

        let (profile_rect, main_rect, notifications_rect, _) = Self::cluster_bounds(display);
        let nav_w = 190.0_f32.min(main_rect.w * 0.28).max(150.0);
        let gap = 16.0;
        let view_w = (main_rect.w - nav_w - gap).max(1.0);
        let nav_rect = Rect {
            x: main_rect.x,
            y: main_rect.y,
            w: nav_w,
            h: main_rect.h,
        };
        let view_rect = Rect {
            x: main_rect.x + nav_w + gap,
            y: main_rect.y,
            w: view_w,
            h: main_rect.h,
        };

        let mut regions = Vec::new();

        // 1. Top-Left Profile Chip (GlassRole::Chip)
        if profile_progress > 0.01 {
            let region = LiquidGlassRegion::from_role(
                &self.design,
                GlassRole::Chip,
                BackdropRegion::from(profile_rect),
                self.design.radii.chip.max(20.0),
                profile_progress,
            )
            .with_capture_bounds(BackdropRegion::from(profile_rect));
            regions.push(region);
        }

        // 2. Top-Right Notifications (GlassRole::FloatingPanel)
        if notif_progress > 0.01 {
            let region = LiquidGlassRegion::from_role(
                &self.design,
                GlassRole::FloatingPanel,
                BackdropRegion::from(notifications_rect),
                self.design.radii.glass_panel,
                notif_progress,
            )
            .with_capture_bounds(BackdropRegion::from(notifications_rect));
            regions.push(region);
        }

        // 3. Central Control Panel: Left Individual Semicircle Capsule Tabs + Right View
        if content_progress > 0.01 {
            let mut tabs = vec![Tab::System];
            tabs.extend(
                self.modules
                    .metadata()
                    .filter(|module| module.availability == ModuleAvailability::Available)
                    .map(|module| Tab::Settings(module.id)),
            );

            const CAPSULE_H: f32 = 44.0;
            const TAB_GAP: f32 = 8.0;

            // Emit an individual 100% semicircle LiquidGlassRegion for each capsule tab
            // with exact 1:1 alignment to the Lens 2D foreground and unified capture bounds.
            for (index, tab) in tabs.iter().enumerate() {
                let tab_rect = Rect {
                    x: nav_rect.x,
                    y: nav_rect.y + index as f32 * (CAPSULE_H + TAB_GAP),
                    w: nav_rect.w,
                    h: CAPSULE_H,
                };
                let selected = self.tab == *tab;
                let mut region = LiquidGlassRegion::from_role(
                    &self.design,
                    GlassRole::Chip,
                    BackdropRegion::from(tab_rect),
                    CAPSULE_H * 0.5, // 100% semicircle on left and right ends
                    content_progress,
                )
                .with_capture_bounds(BackdropRegion::from(nav_rect));

                if selected {
                    // Active tab: Crystal clear luminous glass lit from within
                    region.frost_strength = 0.45; // Clearer luminous core
                    region.tint_strength = 1.25;
                    region.saturation = 1.20;
                }
                regions.push(region);
            }

            // Right Tab Page View (GlassRole::ProminentPanel)
            let view_region = LiquidGlassRegion::from_role(
                &self.design,
                GlassRole::ProminentPanel,
                BackdropRegion::from(view_rect),
                self.design.radii.glass_panel,
                content_progress,
            )
            .with_capture_bounds(BackdropRegion::from(view_rect));
            regions.push(view_region);
        }

        regions
    }
}
