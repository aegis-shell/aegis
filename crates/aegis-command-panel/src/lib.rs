//! The command panel: a full-screen modal overlay in the Sword Art
//! Online menu language (ADR-0080) — frosted white floating panels with an
//! amber accent over the standard dark blurred scrim.
//!
//! One centered cluster of three surfaces: a header band with the user's
//! identity (avatar, display name, groups) on the left and a live machine
//! monitor (chassis glyph plus CPU/GPU/RAM/NET/DISK/BAT gauges fed by
//! `Chrome::update_resource_stats`) on the right; a narrow icon rail below
//! it holding one circular button per section and the close button; and the
//! content panel filling the rest of the cluster. The sections themselves:
//! quick settings (volume, brightness, radios, do-not-disturb),
//! StatusNotifierItem tray activation with host-rendered dbusmenu popovers,
//! and the notification list with dismissal. The System section also shows
//! the Agent Workspaces status row that the HUD's dropped right chip once
//! carried (ADR-0083). The panel opens through the
//! `Super+S` keybinding or a four-finger touchpad swipe down, and closes on
//! Escape, a scrim click, the rail's close button, the same binding, or a
//! four-finger swipe up.
//!
//! Like the HUD and the dock, the panel is compositor-owned lens
//! chrome on the [`aegis_shell`] `Chrome` seam: snapshots arrive through the
//! trait each frame, and user intents leave through `ChromeEvents` plus the
//! shared `aegis-tray` command channel.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, mpsc};

use aegis_core::input::KeyChar;
use aegis_core::interaction_domain::{
    InteractionDomainKind, InteractionDomainSnapshot, InteractionDomainState,
};
use aegis_core::notify::{Notification, NotificationQueue};
use aegis_core::window::{SpaceUse, Window};
use aegis_core::workspace::WorkspaceSnapshot;
use aegis_design::materials;
use aegis_design::themes;
use aegis_design::tokens::Sao;
use aegis_identity::{Identity, Portrait, PortraitConfig, PortraitWatcher};
use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect, Theme};

use aegis_shell::{
    AppCatalog, BackdropRegion, ChassisKind, Chrome, ChromeEvents, CursorShape, IconSet, Localizer,
    Message, NetworkState, ResourceStats, SystemAction, SystemStatus, place_popup, truncate,
};
use aegis_tray::{MenuNode, MenuState, TrayCommand, TrayIcon, TraySnapshot};

mod rendering;

use rendering::*;

#[cfg(test)]
mod tests;

const SCRIM_ALPHA: u8 = 132;
const BACKDROP_BLUR_SIGMA: f32 = 14.0;
/// The cluster's three surfaces: a full-width header band, then the icon
/// rail at the left and the content panel filling the rest.
const HEADER_H: f32 = 118.0;
const RAIL_W: f32 = 64.0;
const CONTENT_W: f32 = 640.0;
const CONTENT_H: f32 = 420.0;
const PANEL_GAP: f32 = 12.0;
/// The rail's reveal lags the header's by this fraction.
const RAIL_STAGGER: f32 = 0.06;
/// Content panel's reveal lags the header's by this fraction.
const CONTENT_STAGGER: f32 = 0.18;
/// Samples kept per sparkline metric (CPU/GPU/RAM).
const HISTORY_CAP: usize = 48;

/// The command panel explicitly owns its VRM composition. Keeping the
/// parameters here lets another host choose a different crop without changing
/// the VRM renderer or the shared portrait source policy.
const AVATAR_CAMERA: aegis_avatar::VrmCamera = aegis_avatar::VrmCamera::new(28.0, 0.25, 0.48, 0.0);

// dbusmenu popover geometry. Placement follows the shared shell popup policy.
const MENU_WIDTH: f32 = 236.0;
const MENU_PAD: f32 = 7.0;
const MENU_ROW_HEIGHT: f32 = 28.0;
const MENU_HEADER_HEIGHT: f32 = 23.0;
const MENU_SECTION_HEIGHT: f32 = 7.0;

// Tray grid geometry: cells are 76×64 inside an 84×72 pitch; the column
// count adapts to the content width.
const TRAY_CELL_W: f32 = 84.0;
const TRAY_CELL_H: f32 = 72.0;

fn presentation_anim_pending(
    reveal: f32,
    target: f32,
    avatar_playing: bool,
    avatar_reload_pending: bool,
) -> bool {
    (reveal - target).abs() > 0.002 || avatar_playing || avatar_reload_pending
}

/// The panel's sections, one circular button each on the icon rail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Section {
    System,
    Tray,
    Messages,
}

impl Section {
    const ALL: [Section; 3] = [Section::System, Section::Tray, Section::Messages];

    fn label(self, i18n: &Localizer) -> &'static str {
        match self {
            Section::System => i18n.text(Message::System),
            Section::Tray => i18n.text(Message::Tray),
            Section::Messages => i18n.text(Message::Notifications),
        }
    }

    fn icon(self) -> Icon {
        match self {
            Section::System => Icon::Settings,
            Section::Tray => Icon::Grid,
            Section::Messages => Icon::Bell,
        }
    }
}

/// A click resolved inside the icon rail, applied after the render pass so
/// the rail can finish drawing before state changes.
enum RailAction {
    Select(Section),
    Close,
}

/// One row of the header band's machine monitor, in priority order; the
/// band shows at most five rows.
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
struct SniTray {
    device: flux::Device,
    snapshot: Arc<Mutex<TraySnapshot>>,
    commands: mpsc::Sender<TrayCommand>,
    /// Uploaded SNI textures keyed by item key, tagged with the snapshot's
    /// icon generation so status-only updates do not re-upload.
    textures: HashMap<String, (u64, flux::Image)>,
    /// Last frame's cells, reused when the snapshot lock is contended (the
    /// worker publishing a large menu tree must never stall rendering).
    cached_cells: Vec<SniCell>,
}

/// One SNI cell to draw this frame, distilled from the tray snapshot.
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
    section: Section,
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
    /// Local account behind the header band's identity zone, resolved once
    /// at construction (never per frame).
    identity: Identity,
    /// Header-band portrait selected by the shared identity contract. Without
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
    /// that opens it, cleared by click-away, item disappearance, section
    /// change, or panel close).
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
    /// Construct the panel. The flux device is borrowed (non-owning, like
    /// [`aegis_shell::Shell::new`]) to upload SNI tray pixmaps to the GPU;
    /// the caller must keep it alive past the panel. The tray handle comes
    /// from the composition root's single `aegis_tray::spawn()` shared with
    /// the HUD; `None` leaves the tray section empty.
    pub fn new(
        device: &flux::Device,
        tray: Option<(Arc<Mutex<TraySnapshot>>, mpsc::Sender<TrayCommand>)>,
        notifications: Arc<Mutex<NotificationQueue>>,
    ) -> CommandPanel {
        let tray = tray.map(|(snapshot, commands)| {
            // SAFETY: the composition root declares its flux device before
            // the shell (and thus this panel) and drops it after, and the
            // panel only touches the device on the render thread.
            let device = unsafe { flux::Device::borrow_raw(device.as_raw()) };
            SniTray {
                device,
                snapshot,
                commands,
                textures: HashMap::new(),
                cached_cells: Vec::new(),
            }
        });
        // Source precedence and still/VRM routing come from the shared
        // identity contract. The panel supplies only its VRM camera and owns
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
        CommandPanel {
            open: false,
            reveal: 0.0,
            section: Section::System,
            reduced_motion: false,
            prev_down: false,
            status: SystemStatus::default(),
            stats: ResourceStats::default(),
            cpu_history: History::new(HISTORY_CAP),
            gpu_history: History::new(HISTORY_CAP),
            ram_history: History::new(HISTORY_CAP),
            identity: Identity::current().unwrap_or_else(|_| Identity::fallback()),
            avatar,
            portrait_config,
            avatar_device: Some(avatar_device),
            avatar_watcher,
            avatar_warned: false,
            interaction_domains: aegis_core::interaction_domain::InteractionDomainModel::new()
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
        CommandPanel {
            open: false,
            reveal: 0.0,
            section: Section::System,
            reduced_motion: false,
            prev_down: false,
            status: SystemStatus::default(),
            stats: ResourceStats::default(),
            cpu_history: History::new(HISTORY_CAP),
            gpu_history: History::new(HISTORY_CAP),
            ram_history: History::new(HISTORY_CAP),
            identity: Identity::current().unwrap_or_else(|_| Identity::fallback()),
            avatar: None,
            portrait_config: PortraitConfig::new(Vec::new()),
            avatar_device: None,
            avatar_watcher: None,
            avatar_warned: false,
            interaction_domains: aegis_core::interaction_domain::InteractionDomainModel::new()
                .snapshot(),
            icons: IconSet::default(),
            notifications: Arc::new(Mutex::new(NotificationQueue::new(3_600_000))),
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

    fn select_section(&mut self, section: Section) {
        if self.section != section {
            self.section = section;
            // The tray popover anchors to a cell of the Tray section; it does
            // not survive a section change.
            if let Some(key) = self.menu_open_for.take() {
                self.menu_path.clear();
                self.send_tray_command(TrayCommand::CloseMenu { key });
            }
        }
    }

    /// The header band, icon rail, and content panel bounds, centered as one
    /// cluster: the header spans the full cluster width on top, the rail sits
    /// below it at the left, and the content panel fills the rest. On small
    /// outputs the cluster shrinks proportionally (rail to a 48px minimum,
    /// content taking the remainder) so it always fits inside the display.
    fn cluster_bounds(display: (f32, f32)) -> (Rect, Rect, Rect) {
        let total_w = (RAIL_W + PANEL_GAP + CONTENT_W).min((display.0 - 32.0).max(120.0));
        let rail_w = RAIL_W.min((total_w - PANEL_GAP - 120.0).max(48.0));
        let content_w = (total_w - rail_w - PANEL_GAP).max(60.0);
        let total_h = (HEADER_H + PANEL_GAP + CONTENT_H).min((display.1 - 48.0).max(176.0));
        let header_h = HEADER_H.min((total_h - PANEL_GAP - 120.0).max(56.0));
        let content_h = (total_h - header_h - PANEL_GAP).max(80.0);
        let x = ((display.0 - total_w) * 0.5).max(8.0);
        let y = ((display.1 - total_h) * 0.5).max(8.0);
        let header = Rect {
            x,
            y,
            w: total_w,
            h: header_h,
        };
        let rail = Rect {
            x,
            y: y + header_h + PANEL_GAP,
            w: rail_w,
            h: content_h,
        };
        let content = Rect {
            x: x + rail_w + PANEL_GAP,
            y: rail.y,
            w: content_w,
            h: content_h,
        };
        (header, rail, content)
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
            let _ = tray.commands.send(command);
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
        if let Ok(snapshot) = tray.snapshot.try_lock() {
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
    fn sni_cells(&mut self) -> Vec<SniCell> {
        let Some(tray) = &mut self.tray else {
            return Vec::new();
        };
        let Ok(snapshot) = tray.snapshot.try_lock() else {
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
        let (header_rect, rail_rect, content_rect) = Self::cluster_bounds(display);

        // Dark scrim over the blurred desktop — the product's standard modal
        // backdrop, scaled in with the reveal.
        f.layer(
            "aegis-sao-scrim",
            Rect {
                x: 0.0,
                y: 0.0,
                w: display.0,
                h: display.1,
            },
            &OverlayOpts {
                bg: Color::rgba(8, 10, 18, fade_alpha(SCRIM_ALPHA, reveal)),
                border: Color::TRANSPARENT,
                radius: 0.0,
                pad: 0.0,
                ..Default::default()
            },
            |_| {},
        );

        // Click-away: a press landing on none of the three surfaces nor an
        // open tray popover dismisses the panel.
        let on_popover = self
            .open_popover_bounds(display)
            .map(|rect| contains(rect, cursor.0, cursor.1))
            .unwrap_or(false);
        if pressed
            && !contains(header_rect, cursor.0, cursor.1)
            && !contains(rail_rect, cursor.0, cursor.1)
            && !contains(content_rect, cursor.0, cursor.1)
            && !on_popover
        {
            self.close();
        }

        // Sections stop accepting presses while the panel is closing.
        let pressed = pressed && self.open;

        let header_progress = stagger(reveal, 0.0);
        let rail_progress = stagger(reveal, RAIL_STAGGER);
        let content_progress = ease_out_cubic(stagger(reveal, CONTENT_STAGGER));
        self.render_header_band(f, header_rect, header_progress, i18n);
        self.render_icon_rail(f, rail_rect, rail_progress, cursor, pressed);
        self.render_content_panel(f, content_rect, content_progress, cursor, i18n, out);

        // The dbusmenu popover floats above the panels.
        if self.menu_open_for.is_some()
            && let Some(menu) = self.menu_snapshot()
            && Some(&menu.key) == self.menu_open_for.as_ref()
        {
            self.render_tray_menu(f, &menu, display, cursor, pressed);
        }

        self.prev_down = down;
    }

    fn captures_keyboard(&self) -> bool {
        self.active()
    }

    fn key_char(&mut self, kc: &KeyChar, _out: &mut ChromeEvents) {
        if kc.keysym != aegis_core::input::XKB_KEY_Escape || !self.open {
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

    fn toggle_command_panel(&mut self, _out: &mut ChromeEvents) {
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

    fn update_app_catalog(&mut self, catalog: &AppCatalog) {
        self.icons = catalog.icons.clone();
    }

    fn update_system_status(&mut self, status: &SystemStatus) {
        self.status = status.clone();
    }

    fn update_resource_stats(&mut self, stats: &ResourceStats) {
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

    fn update_interaction_domains(&mut self, snapshot: &InteractionDomainSnapshot) {
        self.interaction_domains = snapshot.clone();
    }

    fn update_windows(&mut self, windows: &[Window]) {
        // A fullscreen window owns the whole output; get out of its way.
        if SpaceUse::from_windows(windows) == SpaceUse::Fullscreen && self.open {
            self.close();
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

    fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
        if reduced {
            self.reveal = if self.open { 1.0 } else { 0.0 };
        }
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
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        if !self.active() {
            return Vec::new();
        }
        vec![BackdropRegion {
            x: 0.0,
            y: 0.0,
            w: display.0,
            h: display.1,
        }]
    }
}
