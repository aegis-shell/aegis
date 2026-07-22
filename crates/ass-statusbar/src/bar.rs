//! The session status bar: a compact top bar with integrated workspace state,
//! active window context, clock, application tray, system status, and a small
//! control centre. Its information architecture follows the user's Quickshell
//! HUD, while the rendering and interaction remain compositor-owned lens
//! chrome.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use ass_core::app::BuiltInApplication;
use ass_core::notify::{Notification, NotificationQueue};
use ass_core::realm::{RealmKind, RealmSnapshot, RealmState};
use ass_core::window::Window;
use ass_core::workspace::WorkspaceSnapshot;
use ass_design::{Design, materials, themes};
use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect};

use ass_shell::{
    AppCatalog, BackdropRegion, BatteryStatus, Chrome, ChromeEvents, CursorShape, HUD_HEIGHT,
    IconSet, Localizer, Message, NetworkState, Reserved, SystemAction, SystemStatus,
};

use crate::tray::{self, MenuNode, TrayCommand, TrayIcon, TraySnapshot};

const WORKSPACE_SLOT_W: f32 = 18.0;
const WORKSPACE_ACTIVE_DOT: f32 = 8.0;
const WORKSPACE_INACTIVE_DOT: f32 = 6.0;
const LEFT_MARGIN: f32 = 10.0;
const RIGHT_MARGIN: f32 = 6.0;
const TRAY_CELL_W: f32 = 26.0;
const MAX_TRAY_ITEMS: usize = 5;
/// Scale theme-resolved SNI icons are looked up and rasterized at; the
/// texture is sampled down to the 18px cell glyph, so 2x keeps HiDPI crisp.
const TRAY_ICON_SCALE: u32 = 2;
const PANEL_GAP: f32 = 6.0;
const PANEL_W: f32 = 420.0;
const PANEL_H: f32 = 326.0;
const CLOCK_POLL_INTERVAL: Duration = Duration::from_secs(15);
const BACKDROP_BLUR_SIGMA: f32 = 12.0;
const AGENT_INDICATOR_MAX_W: f32 = 154.0;

// dbusmenu popover geometry (mirrors ass-shell's app_menu.rs — kept private
// here so ass-statusbar does not reach into ass-shell's chrome internals).
// TODO: share with ass-shell::chrome::app_menu
const MENU_WIDTH: f32 = 236.0;
const MENU_MARGIN: f32 = 8.0;
const MENU_GAP: f32 = 8.0;
const MENU_PAD: f32 = 7.0;
const MENU_ROW_HEIGHT: f32 = 28.0;
const MENU_HEADER_HEIGHT: f32 = 23.0;
const MENU_SECTION_HEIGHT: f32 = 7.0;

/// Full top-bar status bar.
pub struct StatusBar {
    prev_down: bool,
    prev_right_down: bool,
    panel_open: bool,
    icons: IconSet,
    notifications: Option<Arc<Mutex<NotificationQueue>>>,
    status: SystemStatus,
    realms: RealmSnapshot,
    clock: String,
    last_clock_poll: Instant,
    tray: Option<SniTray>,
    /// SNI item key whose dbusmenu popover is showing (set on the right-click
    /// that opens it, cleared by click-away or item disappearance).
    menu_open_for: Option<String>,
    /// Breadcrumb of submenu ids from root to the current view; `path[0]` is
    /// always the root sentinel (0), each later id descends one level.
    menu_path: Vec<i32>,
    /// The SNI cell rect that opened the popover (kept current each frame so
    /// `place_popup` re-anchors on resize).
    menu_owner: Rect,
    /// One-frame flag suppressing the same right-press that opened the menu
    /// from also immediately closing it (mirrors app_menu.rs's `just_opened`).
    menu_just_opened: bool,
}

/// Render-thread half of the StatusNotifierItem tray: the shared snapshot the
/// worker writes, the command channel back to it, and the texture cache the
/// bar uploads from item pixmaps or theme-resolved icon names.
struct SniTray {
    device: flux::Device,
    snapshot: Arc<Mutex<TraySnapshot>>,
    commands: mpsc::Sender<TrayCommand>,
    /// Uploaded SNI textures keyed by item key, tagged with the snapshot's
    /// icon generation so status-only updates do not re-upload.
    textures: HashMap<String, (u64, flux::Image)>,
    /// Item keys whose theme-icon resolution failed at the tagged generation,
    /// so a missing or undecodable name does not rescan the theme every frame.
    failed: HashMap<String, u64>,
}

struct TrayCell {
    window: ass_core::window::WindowId,
    key: String,
    icon: Option<*mut c_void>,
}

/// One SNI cell to draw this frame, distilled from the tray snapshot.
struct SniCell {
    key: String,
    has_menu: bool,
    textured: bool,
    /// Cell rect (filled in during the tray-row layout pass).
    rect: Rect,
}

/// How the combined tray row folds into the slot budget: app-tray cells
/// (open windows) keep priority, SNI cells fill the remaining slots, and
/// past budget the last slot becomes a "+N" overflow indicator counting
/// everything hidden (see [`fold_tray`]).
struct TrayFold {
    visible_apps: usize,
    visible_sni: usize,
    hidden: usize,
}

impl StatusBar {
    /// Construct a standalone status bar without notification data, raster
    /// icons, or the SNI tray (used by tests and previews).
    pub fn new() -> StatusBar {
        StatusBar::with_optional_sources(None, None)
    }

    /// Construct the session status bar with the compositor's flux device and
    /// shared notification queue. The device is borrowed (non-owning, like
    /// [`ass_shell::Shell::new`]) to upload SNI tray pixmaps to the GPU; the
    /// caller must keep it alive past the bar. Application icons arrive
    /// through [`Chrome::update_app_catalog`], seeded on registration by
    /// [`ass_shell::Shell::add`]. When the session bus is unavailable the SNI
    /// tray silently stays empty.
    pub fn with_notifications(
        device: &flux::Device,
        notifications: Arc<Mutex<NotificationQueue>>,
    ) -> StatusBar {
        StatusBar::with_optional_sources(Some(device), Some(notifications))
    }

    fn with_optional_sources(
        device: Option<&flux::Device>,
        notifications: Option<Arc<Mutex<NotificationQueue>>>,
    ) -> StatusBar {
        let tray = device.and_then(|device| {
            let (snapshot, commands) = tray::spawn()?;
            // SAFETY: the composition root declares its flux device before
            // the shell (and thus this bar) and drops it after, and the bar
            // only touches the device on the render thread.
            let device = unsafe { flux::Device::borrow_raw(device.as_raw()) };
            Some(SniTray {
                device,
                snapshot,
                commands,
                textures: HashMap::new(),
                failed: HashMap::new(),
            })
        });
        let now = Instant::now();
        StatusBar {
            prev_down: false,
            prev_right_down: false,
            panel_open: false,
            icons: IconSet::default(),
            notifications,
            status: SystemStatus::default(),
            realms: ass_core::realm::RealmModel::new().snapshot(),
            clock: "--:--".to_string(),
            last_clock_poll: now.checked_sub(CLOCK_POLL_INTERVAL).unwrap_or(now),
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
        }
    }

    fn bar_bounds(display_w: f32) -> Rect {
        Rect {
            x: 0.0,
            y: 0.0,
            w: display_w.max(1.0),
            h: HUD_HEIGHT,
        }
    }

    fn panel_bounds(display: (f32, f32)) -> Rect {
        let w = PANEL_W.min((display.0 - 16.0).max(240.0));
        let h = PANEL_H.min((display.1 - HUD_HEIGHT - 16.0).max(180.0));
        Rect {
            x: (display.0 - w - 8.0).max(8.0),
            y: HUD_HEIGHT + PANEL_GAP,
            w,
            h,
        }
    }
    fn refresh_status(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_clock_poll) >= CLOCK_POLL_INTERVAL {
            if let Some(clock) = command_output("date", &["+%H:%M"]) {
                self.clock = clock;
            }
            self.last_clock_poll = now;
        }
    }

    fn notification_snapshot(&self) -> Vec<Notification> {
        self.notifications
            .as_ref()
            .map(|queue| queue.lock().unwrap().snapshot())
            .unwrap_or_default()
    }

    fn send_tray_command(&self, command: TrayCommand) {
        if let Some(tray) = &self.tray {
            // The worker may be gone (bus disconnected); clicks just drop.
            let _ = tray.commands.send(command);
        }
    }

    fn themed_icon(&self, name: &str) -> Option<*mut c_void> {
        self.icons.get(&format!("ass-hud:{name}"))
    }

    /// Read the shared menu snapshot under the worker's lock. Returns `None`
    /// when no menu is currently open or the bar has no SNI tray.
    fn menu_snapshot(&self) -> Option<crate::tray::MenuState> {
        let tray = self.tray.as_ref()?;
        let snapshot = tray.snapshot.lock().unwrap();
        snapshot.menu.clone()
    }

    fn tray_cells(&self, windows: &[Window]) -> Vec<TrayCell> {
        let mut seen = HashSet::new();
        windows
            .iter()
            .filter(|window| !window.read_only)
            .filter_map(|window| {
                let app_id = window.app_id.as_deref()?.to_ascii_lowercase();
                if !seen.insert(app_id.clone()) {
                    return None;
                }
                Some(TrayCell {
                    window: window.id,
                    icon: self.icons.get(&app_id),
                    key: app_id,
                })
            })
            // No slot limit here: the combined row folds into MAX_TRAY_ITEMS
            // in `render` (see `fold_tray`).
            .collect()
    }

    /// Read the SNI snapshot under a brief lock, upload any new or changed
    /// icons into the texture cache, and return the visible cells for this
    /// frame. Runs on the render thread; never touches D-Bus.
    fn sni_cells(&mut self) -> Vec<SniCell> {
        let Some(tray) = &mut self.tray else {
            return Vec::new();
        };
        let snapshot = tray.snapshot.lock().unwrap();
        tray.textures
            .retain(|key, _| snapshot.items.iter().any(|item| &item.key == key));
        tray.failed
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
            match &item.icon {
                TrayIcon::Pixmap(pixmap) => {
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
                }
                TrayIcon::Name(name) => {
                    // The icon generation also bumps on `IconName` changes, so
                    // the same generation tag keys theme-resolved textures.
                    let attempted = tray.failed.get(&item.key) == Some(&item.icon_generation);
                    if stale && !attempted {
                        match themed_tray_icon(&tray.device, name) {
                            Some(image) => {
                                tray.textures
                                    .insert(item.key.clone(), (item.icon_generation, image));
                            }
                            None => {
                                // Missing or undecodable icons keep the
                                // fallback glyph; memo the generation so the
                                // theme is not rescanned every frame.
                                tray.failed.insert(item.key.clone(), item.icon_generation);
                                tray.textures.remove(&item.key);
                            }
                        }
                    }
                }
                TrayIcon::None => {
                    // An item that drops its icon must not keep rendering the
                    // previous texture.
                    tray.textures.remove(&item.key);
                }
            }
            cells.push(SniCell {
                key: item.key.clone(),
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
        cells
    }

    fn render_panel(
        &self,
        f: &mut Frame,
        display: (f32, f32),
        cursor: (f32, f32),
        notifications: &[Notification],
        i18n: &Localizer,
    ) {
        let panel = Self::panel_bounds(display);
        f.layer("ass-hud-panel", panel, &panel_opts(), |f| {
            f.column_ex(&sized(panel.w, panel.h), |_| {});
        });

        render_text_left(
            f,
            "ass-hud-panel-title",
            Rect {
                x: panel.x + 18.0,
                y: panel.y + 12.0,
                w: panel.w - 70.0,
                h: 24.0,
            },
            i18n.text(Message::ControlCenter),
            15.0,
        );
        let close = Rect {
            x: panel.x + panel.w - 42.0,
            y: panel.y + 9.0,
            w: 30.0,
            h: 30.0,
        };
        render_icon_button(
            f,
            "ass-hud-panel-close",
            close,
            self.themed_icon("window-close-symbolic"),
            Icon::X,
            "",
            contains(close, cursor.0, cursor.1),
        );

        let gap = 10.0;
        let card_w = (panel.w - 36.0 - gap) * 0.5;
        let card_h = 72.0;
        let card_y = panel.y + 50.0;
        let audio = Rect {
            x: panel.x + 13.0,
            y: card_y,
            w: card_w,
            h: card_h,
        };
        let network = Rect {
            x: audio.x + card_w + gap,
            ..audio
        };
        let battery = Rect {
            y: card_y + card_h + gap,
            ..audio
        };
        let notices = Rect {
            x: network.x,
            ..battery
        };

        let volume = match self.status.volume {
            Some(level) if self.status.muted => i18n.muted_volume(level),
            Some(level) => format!("{level}%"),
            None => i18n.text(Message::Unavailable).to_string(),
        };
        render_status_card(
            f,
            "ass-hud-audio-card",
            audio,
            self.themed_icon(volume_icon_name(&self.status)),
            volume_icon(&self.status),
            (i18n.text(Message::Volume), &volume),
            contains(audio, cursor.0, cursor.1),
        );
        let network_text = match self.status.network {
            NetworkState::Wifi => i18n.text(Message::WifiConnected),
            NetworkState::Wired => i18n.text(Message::WiredConnected),
            NetworkState::Offline => i18n.text(Message::Disconnected),
        };
        render_status_card(
            f,
            "ass-hud-network-card",
            network,
            self.themed_icon(network_icon_name(self.status.network)),
            Icon::Globe,
            (i18n.text(Message::Network), network_text),
            contains(network, cursor.0, cursor.1),
        );
        let battery_text = self
            .status
            .battery
            .map(|battery| {
                if battery.charging {
                    i18n.charging_battery(battery.percent)
                } else {
                    format!("{}%", battery.percent)
                }
            })
            .unwrap_or_else(|| i18n.text(Message::NoBatteryDetected).to_string());
        let battery_icon = self
            .status
            .battery
            .and_then(|battery| self.themed_icon(&battery_icon_name(battery)));
        render_status_card(
            f,
            "ass-hud-battery-card",
            battery,
            battery_icon,
            Icon::Zap,
            (i18n.text(Message::Battery), &battery_text),
            contains(battery, cursor.0, cursor.1),
        );
        render_status_card(
            f,
            "ass-hud-notification-card",
            notices,
            self.themed_icon("preferences-system-notifications-symbolic"),
            Icon::Bell,
            (
                i18n.text(Message::Notifications),
                &i18n.recent_notification_count(notifications.len()),
            ),
            contains(notices, cursor.0, cursor.1),
        );

        let heading_y = battery.y + card_h + 14.0;
        render_text_left(
            f,
            "ass-hud-notification-heading",
            Rect {
                x: panel.x + 18.0,
                y: heading_y,
                w: panel.w - 36.0,
                h: 20.0,
            },
            i18n.text(Message::RecentNotifications),
            12.0,
        );
        let list_y = heading_y + 24.0;
        if notifications.is_empty() {
            render_text_left(
                f,
                "ass-hud-notification-empty",
                Rect {
                    x: panel.x + 18.0,
                    y: list_y,
                    w: panel.w - 36.0,
                    h: 28.0,
                },
                i18n.text(Message::NoNotifications),
                11.0,
            );
        } else {
            for (i, notification) in notifications.iter().rev().take(2).enumerate() {
                let row = Rect {
                    x: panel.x + 13.0,
                    y: list_y + i as f32 * 42.0,
                    w: panel.w - 26.0,
                    h: 36.0,
                };
                let id = format!("ass-hud-notification-{}", notification.id);
                f.layer(
                    &id,
                    row,
                    &small_card_opts(contains(row, cursor.0, cursor.1)),
                    |f| {
                        f.column_ex(
                            &LayoutOpts {
                                width: row.w,
                                height: row.h,
                                pad: 8.0,
                                cross: Align::Start,
                                ..Default::default()
                            },
                            |f| f.label_compact_sized(&truncate(&notification.summary, 44), 11.0),
                        );
                    },
                );
            }
        }
    }

    /// Render the dbusmenu popover. The visible rows come from walking
    /// `menu.root.children` along `self.menu_path`. Click handling is
    /// driven by `frame.selectable`, which returns true on the frame its
    /// row is clicked — submenu rows push onto `menu_path`, leaf rows send
    /// `MenuEvent` and dismiss the popover. Click-away closes the popover
    /// unless the press falls on the owner SNI cell (so the same press does
    /// not toggle it closed).
    fn render_menu(
        &mut self,
        f: &mut Frame,
        menu: &crate::tray::MenuState,
        display: (f32, f32),
        cursor: (f32, f32),
        pressed: bool,
    ) {
        // If a targeted submenu id no longer exists (the worker truncated the
        // tree on `LayoutUpdated`), pop back to the nearest valid level.
        while tray::visible_children(&menu.root, &self.menu_path).is_none()
            && self.menu_path.len() > 1
        {
            self.menu_path.pop();
        }
        let visible = match tray::visible_children(&menu.root, &self.menu_path) {
            Some(rows) => rows,
            None => return,
        };

        // Click-away: anything outside the popover AND outside the owner cell
        // closes the menu. The owner cell stays live so a second right-click
        // (or a stray left click on the tray icon) keeps the menu open.
        let popover_bounds = menu_bounds(self.menu_owner, visible, display);
        let in_owner = contains(self.menu_owner, cursor.0, cursor.1);
        let in_popover = contains(popover_bounds, cursor.0, cursor.1);
        if !self.menu_just_opened && pressed && !in_owner && !in_popover {
            self.close_menu(menu.key.clone());
            return;
        }
        self.menu_just_opened = false;

        let original_theme = f.theme();
        let design = Design::dark();
        let menu_theme = themes::menu(original_theme, &design);
        let dim_theme = themes::menu_disabled(menu_theme, &design);

        let header_visible = self.menu_path.len() > 1;
        let mut row_index = 0usize;
        let mut action: Option<MenuRowAction> = None;
        f.set_theme(menu_theme);
        f.layer(
            "ass-hud-sni-menu",
            popover_bounds,
            &materials::popover(&design),
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: popover_bounds.w,
                        height: popover_bounds.h,
                        gap: 0.0,
                        pad: MENU_PAD,
                        ..Default::default()
                    },
                    |f| {
                        let inner_w = popover_bounds.w - MENU_PAD * 2.0;
                        if header_visible {
                            f.size_next(inner_w, MENU_HEADER_HEIGHT);
                            f.push_id("menu-back");
                            if f.selectable("‹ Back", false) {
                                action = Some(MenuRowAction::Back);
                            }
                            f.pop_id();
                            row_index += 1;
                        }
                        for row in visible.iter() {
                            if !row.visible {
                                continue;
                            }
                            if row.kind == crate::tray::MenuEntryKind::Separator {
                                f.size_next(inner_w, MENU_SECTION_HEIGHT);
                                f.separator();
                                continue;
                            }
                            f.size_next(inner_w, MENU_ROW_HEIGHT);
                            f.push_id(&format!("menu-row-{}", row.id));
                            if !row.enabled {
                                // Disabled rows render as inert labels with a dim
                                // foreground — selectable would still capture the
                                // click, which the dbusmenu spec forbids.
                                f.set_theme(dim_theme);
                                f.label_compact_sized(&truncate(&menu_row_label(row), 32), 11.5);
                                f.set_theme(menu_theme);
                            } else if f.selectable(&truncate(&menu_row_label(row), 32), false) {
                                if row.has_submenu {
                                    action = Some(MenuRowAction::Descend(row.id));
                                } else {
                                    action = Some(MenuRowAction::Click(row.id));
                                }
                            }
                            f.pop_id();
                            row_index += 1;
                        }
                    },
                );
            },
        );
        f.set_theme(original_theme);
        let _ = row_index;

        match action {
            Some(MenuRowAction::Back) => {
                self.menu_path.pop();
            }
            Some(MenuRowAction::Descend(id)) => {
                self.menu_path.push(id);
            }
            Some(MenuRowAction::Click(id)) => {
                self.send_tray_command(TrayCommand::MenuEvent {
                    key: menu.key.clone(),
                    id,
                });
                self.close_menu(menu.key.clone());
            }
            None => {}
        }
    }

    fn close_menu(&mut self, key: String) {
        self.menu_open_for = None;
        self.menu_path.clear();
        self.menu_just_opened = false;
        self.send_tray_command(TrayCommand::CloseMenu { key });
    }
}

/// A deferred row click captured during the menu frame's column closure.
enum MenuRowAction {
    Back,
    Descend(i32),
    Click(i32),
}

/// Compact, always-visible authority summary. The detailed controls live in
/// Control Center; the bar deliberately answers only "is an agent active?"
/// and provides a direct route to that state.
struct AgentIndicator {
    label: String,
    active: bool,
}

fn agent_indicator(snapshot: &RealmSnapshot, i18n: &Localizer) -> Option<AgentIndicator> {
    let live = snapshot
        .realms
        .iter()
        .filter(|realm| realm.kind == RealmKind::Agent && realm.state != RealmState::Revoked)
        .collect::<Vec<_>>();
    let active = live.iter().any(|realm| realm.state == RealmState::Active);
    let state = if active {
        i18n.text(Message::RealmActive)
    } else {
        i18n.text(Message::RealmPaused)
    };
    let label = match live.as_slice() {
        [] => return None,
        [realm] => format!("{} · {state}", realm.label),
        realms => format!("AI {} · {state}", realms.len()),
    };
    Some(AgentIndicator { label, active })
}

fn render_agent_indicator(
    frame: &mut Frame,
    rect: Rect,
    indicator: &AgentIndicator,
    hovered: bool,
) {
    let accent = if indicator.active {
        Color::rgba(92, 168, 255, 255)
    } else {
        Color::rgba(240, 184, 84, 255)
    };
    frame.layer(
        "ass-hud-agent-indicator",
        rect,
        &OverlayOpts {
            bg: if hovered {
                Color::rgba(72, 100, 146, 112)
            } else {
                Color::rgba(42, 55, 80, 92)
            },
            border: Color::rgba(116, 151, 206, if hovered { 150 } else { 92 }),
            border_width: 1.0,
            radius: 9.0,
            ..Default::default()
        },
        |_| {},
    );
    let dot = Rect {
        x: rect.x + 9.0,
        y: rect.y + (rect.h - 7.0) * 0.5,
        w: 7.0,
        h: 7.0,
    };
    frame.layer(
        "ass-hud-agent-state-dot",
        dot,
        &OverlayOpts::default(),
        |frame| frame.column_ex(&sized_fill(dot.w, dot.h, accent, dot.w * 0.5), |_| {}),
    );
    render_text_left(
        frame,
        "ass-hud-agent-state-label",
        Rect {
            x: rect.x + 22.0,
            y: rect.y,
            w: (rect.w - 28.0).max(1.0),
            h: rect.h,
        },
        &truncate(&indicator.label, ((rect.w - 28.0) / 6.8).max(4.0) as usize),
        10.5,
    );
}

impl Default for StatusBar {
    fn default() -> Self {
        StatusBar::new()
    }
}

impl Chrome for StatusBar {
    fn render(
        &mut self,
        f: &mut Frame,
        input: &Input,
        windows: &[Window],
        workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        self.refresh_status();
        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        let cursor = (raw.cursor.x, raw.cursor.y);
        let down = raw.mouse_down.first().copied().unwrap_or(false);
        let pressed = down && !self.prev_down;
        // lens exposes the full button trio; the right button drives SNI
        // context actions (index 1 == LENS_MOUSE_RIGHT).
        let right_down = raw.mouse_down.get(1).copied().unwrap_or(false);
        let right_pressed = right_down && !self.prev_right_down;
        let notifications = self.notification_snapshot();
        let mut tray = self.tray_cells(windows);
        let mut sni = self.sni_cells();
        // Fold the combined tray row into the slot budget (see `fold_tray`):
        // app-tray cells keep priority, SNI cells fill the rest, and any
        // remainder collapses into a "+N" indicator.
        let fold = fold_tray(sni.len(), tray.len(), MAX_TRAY_ITEMS);
        tray.truncate(fold.visible_apps);
        sni.truncate(fold.visible_sni);

        let bar = Self::bar_bounds(display.0);
        f.layer("ass-hud-bar", bar, &bar_opts(), |f| {
            f.column_ex(&sized(bar.w, bar.h), |_| {});
        });
        f.layer(
            "ass-hud-bottom-border",
            Rect {
                x: 0.0,
                y: HUD_HEIGHT - 1.0,
                w: display.0,
                h: 1.0,
            },
            &OverlayOpts::default(),
            |f| {
                f.column_ex(
                    &sized_fill(display.0, 1.0, Color::rgba(255, 255, 255, 28), 0.0),
                    |_| {},
                );
            },
        );

        let mut left_x = LEFT_MARGIN;
        if let Some(output) = workspaces.outputs.first() {
            for workspace in &output.workspaces {
                let slot = Rect {
                    x: left_x,
                    y: 0.0,
                    w: WORKSPACE_SLOT_W,
                    h: HUD_HEIGHT,
                };
                let active = output.current == Some(workspace.id);
                let hovered = contains(slot, cursor.0, cursor.1);
                let diameter = workspace_dot_diameter(active, hovered);
                let dot = Rect {
                    x: slot.x + (slot.w - diameter) * 0.5,
                    y: slot.y + (slot.h - diameter) * 0.5,
                    w: diameter,
                    h: diameter,
                };
                f.layer(
                    &format!("ass-hud-workspace-dot-{}", workspace.id.0),
                    dot,
                    &OverlayOpts::default(),
                    |f| {
                        f.column_ex(
                            &sized_fill(
                                diameter,
                                diameter,
                                workspace_dot_color(active, hovered),
                                diameter * 0.5,
                            ),
                            |_| {},
                        );
                    },
                );
                if pressed && hovered {
                    out.switch_workspace = Some(workspace.id);
                }
                left_x += WORKSPACE_SLOT_W;
            }
        }

        if let Some(active) = windows.iter().find(|window| window.state.activated) {
            left_x += 12.0;
            let max_title_w = (display.0 * 0.5 - left_x - 54.0).max(0.0);
            if max_title_w > 42.0 {
                if let Some(app_id) = active.app_id.as_deref() {
                    let key = app_id.to_ascii_lowercase();
                    if let Some(icon) = self.icons.get(&key) {
                        let icon_rect = Rect {
                            x: left_x,
                            y: 7.0,
                            w: 18.0,
                            h: 18.0,
                        };
                        f.layer(
                            "ass-hud-window-icon",
                            icon_rect,
                            &centered_layer(),
                            |f| unsafe { f.image(icon as *mut lens::sys::flux_image, 18.0, 18.0) },
                        );
                        left_x += 25.0;
                    }
                }
                let title = active
                    .title
                    .as_deref()
                    .unwrap_or_else(|| i18n.text(Message::Untitled));
                render_text_left(
                    f,
                    "ass-hud-window-title",
                    Rect {
                        x: left_x,
                        y: 0.0,
                        w: max_title_w,
                        h: HUD_HEIGHT,
                    },
                    &truncate(title, (max_title_w / 7.4).max(4.0) as usize),
                    12.0,
                );
            }
        }

        render_text(
            f,
            "ass-hud-clock",
            Rect {
                x: display.0 * 0.5 - 42.0,
                y: 0.0,
                w: 84.0,
                h: HUD_HEIGHT,
            },
            &self.clock,
            13.5,
        );

        let mut right_x = display.0 - RIGHT_MARGIN;
        let control = take_right(&mut right_x, 34.0);
        let control_hover = contains(control, cursor.0, cursor.1);
        render_icon_button(
            f,
            "ass-hud-control-toggle",
            control,
            self.themed_icon("preferences-system-symbolic"),
            Icon::Settings,
            "",
            control_hover,
        );
        let bell = take_right(
            &mut right_x,
            if notifications.is_empty() { 34.0 } else { 50.0 },
        );
        let notification_count = if notifications.is_empty() {
            String::new()
        } else {
            notifications.len().min(99).to_string()
        };
        render_icon_button(
            f,
            "ass-hud-bell",
            bell,
            self.themed_icon("preferences-system-notifications-symbolic"),
            Icon::Bell,
            &notification_count,
            contains(bell, cursor.0, cursor.1),
        );
        let agent_indicator = agent_indicator(&self.realms, i18n);
        let agent = agent_indicator.as_ref().map(|indicator| {
            let label_w = indicator.label.chars().count() as f32 * 6.8 + 30.0;
            take_right(&mut right_x, label_w.clamp(72.0, AGENT_INDICATOR_MAX_W))
        });
        if let (Some(indicator), Some(rect)) = (&agent_indicator, agent) {
            render_agent_indicator(f, rect, indicator, contains(rect, cursor.0, cursor.1));
        }
        if let Some(battery) = self.status.battery {
            let rect = take_right(&mut right_x, 62.0);
            render_icon_button(
                f,
                "ass-hud-battery",
                rect,
                self.themed_icon(&battery_icon_name(battery)),
                Icon::Zap,
                &format!("{}%", battery.percent),
                contains(rect, cursor.0, cursor.1),
            );
        }
        let network = take_right(&mut right_x, 34.0);
        render_icon_button(
            f,
            "ass-hud-network",
            network,
            self.themed_icon(network_icon_name(self.status.network)),
            Icon::Globe,
            "",
            contains(network, cursor.0, cursor.1),
        );
        let audio = take_right(&mut right_x, 66.0);
        let volume_label = match self.status.volume {
            Some(_) if self.status.muted => i18n.text(Message::Muted).to_string(),
            Some(level) => format!("{level}%"),
            None => "--".to_string(),
        };
        render_icon_button(
            f,
            "ass-hud-audio",
            audio,
            self.themed_icon(volume_icon_name(&self.status)),
            volume_icon(&self.status),
            &volume_label,
            contains(audio, cursor.0, cursor.1),
        );

        for tray_cell in tray.iter().rev() {
            let rect = take_right(&mut right_x, TRAY_CELL_W);
            let hovered = contains(rect, cursor.0, cursor.1);
            let id = format!("ass-hud-tray-{}", tray_cell.key);
            f.layer(&id, rect, &icon_button_opts(hovered), |f| {
                match tray_cell.icon {
                    Some(icon) => unsafe {
                        f.image(icon as *mut lens::sys::flux_image, 18.0, 18.0)
                    },
                    None => match self.themed_icon("application-x-executable-symbolic") {
                        Some(icon) => unsafe {
                            f.image(icon as *mut lens::sys::flux_image, 18.0, 18.0)
                        },
                        None => f.icon(Icon::FileText, 16.0),
                    },
                }
            });
            if pressed && hovered {
                out.clicked = Some(tray_cell.window);
            }
        }

        // StatusNotifierItem cells continue the row leftwards from the
        // app-tray cells (same right-to-left `take_right` layout, same cell
        // size and hover style).
        for sni_cell in sni.iter_mut().rev() {
            let rect = take_right(&mut right_x, TRAY_CELL_W);
            sni_cell.rect = rect;
            let hovered = contains(rect, cursor.0, cursor.1);
            let id = format!("ass-hud-sni-{}", sni_cell.key);
            let texture = if sni_cell.textured {
                self.tray
                    .as_ref()
                    .and_then(|tray| tray.textures.get(&sni_cell.key))
                    .map(|(_, image)| image.as_raw())
            } else {
                None
            };
            f.layer(&id, rect, &icon_button_opts(hovered), |f| match texture {
                Some(texture) => unsafe {
                    f.image(texture as *mut lens::sys::flux_image, 18.0, 18.0)
                },
                None => match self.themed_icon("application-x-executable-symbolic") {
                    Some(icon) => unsafe {
                        f.image(icon as *mut lens::sys::flux_image, 18.0, 18.0)
                    },
                    None => f.icon(Icon::FileText, 16.0),
                },
            });
            let (x, y) = (cursor.0 as i32, cursor.1 as i32);
            if pressed && hovered {
                self.send_tray_command(TrayCommand::Activate {
                    key: sni_cell.key.clone(),
                    x,
                    y,
                });
            } else if right_pressed && hovered {
                // Items that expose a Menu object path get the host-rendered
                // popover; everything else keeps the SNI `SecondaryActivate`
                // fallback.
                if sni_cell.has_menu {
                    self.menu_open_for = Some(sni_cell.key.clone());
                    self.menu_path = vec![0];
                    self.menu_owner = rect;
                    self.menu_just_opened = true;
                    self.send_tray_command(TrayCommand::FetchMenu {
                        key: sni_cell.key.clone(),
                    });
                } else {
                    self.send_tray_command(TrayCommand::SecondaryActivate {
                        key: sni_cell.key.clone(),
                        x,
                        y,
                    });
                }
            }
        }

        // The overflow indicator occupies the last (leftmost) slot when the
        // fold hid items. It is a label, not a button: no click action.
        if fold.hidden > 0 {
            let rect = take_right(&mut right_x, TRAY_CELL_W);
            render_text(
                f,
                "ass-hud-tray-overflow",
                rect,
                &format!("+{}", fold.hidden.min(99)),
                11.0,
            );
        }

        if pressed && contains(audio, cursor.0, cursor.1) {
            out.system_actions.push(SystemAction::ToggleMute);
        }
        let scroll_y = raw.scroll_y * 40.0 + raw.scroll_pixels_y;
        if contains(audio, cursor.0, cursor.1) && scroll_y.abs() > 0.01 {
            let amount = if scroll_y < 0.0 { 2 } else { -2 };
            out.system_actions.push(SystemAction::StepVolume(amount));
        }
        if pressed && control_hover {
            self.panel_open = false;
            out.spawn = Some(ass_core::app::Entry::control_center(
                i18n.text(Message::ControlCenter),
                i18n.text(Message::StandaloneSettingsApp),
            ));
        } else if pressed && contains(bell, cursor.0, cursor.1) {
            self.panel_open = !self.panel_open;
        } else if pressed && agent.is_some_and(|rect| contains(rect, cursor.0, cursor.1)) {
            self.panel_open = false;
            out.open_builtin = Some(BuiltInApplication::AiWorkspaces);
        }

        if self.panel_open {
            let panel = Self::panel_bounds(display);
            let close = Rect {
                x: panel.x + panel.w - 42.0,
                y: panel.y + 9.0,
                w: 30.0,
                h: 30.0,
            };
            let card_w = (panel.w - 46.0) * 0.5;
            let audio_card = Rect {
                x: panel.x + 13.0,
                y: panel.y + 50.0,
                w: card_w,
                h: 72.0,
            };
            if pressed && contains(close, cursor.0, cursor.1) {
                self.panel_open = false;
            } else if pressed && contains(audio_card, cursor.0, cursor.1) {
                out.system_actions.push(SystemAction::ToggleMute);
            } else if pressed {
                let list_y = panel.y + 242.0;
                if let Some(notification) =
                    notifications
                        .iter()
                        .rev()
                        .take(2)
                        .enumerate()
                        .find_map(|(i, notification)| {
                            let row = Rect {
                                x: panel.x + 13.0,
                                y: list_y + i as f32 * 42.0,
                                w: panel.w - 26.0,
                                h: 36.0,
                            };
                            contains(row, cursor.0, cursor.1).then_some(notification)
                        })
                {
                    out.dismissed_notification = Some(notification.id);
                }
            }
            self.render_panel(f, display, cursor, &notifications, i18n);
        }

        // dbusmenu popover for the SNI item whose key is in `menu_open_for`.
        // The owner cell rect may move each frame, so re-anchor from the cell
        // list when possible.
        if let Some(key) = self.menu_open_for.clone() {
            if let Some(cell) = sni.iter().find(|cell| cell.key == key) {
                self.menu_owner = cell.rect;
            } else {
                // The item vanished from the snapshot — close the popover so
                // we never render a menu whose backing item is gone.
                self.menu_open_for = None;
                self.menu_path.clear();
                self.send_tray_command(TrayCommand::CloseMenu { key });
            }
        }
        if let Some(menu) = self.menu_snapshot()
            && Some(&menu.key) == self.menu_open_for.as_ref()
        {
            self.render_menu(f, &menu, display, cursor, pressed);
        }

        self.prev_down = down;
        self.prev_right_down = right_down;
    }

    fn captures_pointer(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        if contains(Self::bar_bounds(display.0), x, y) {
            return true;
        }
        if self.panel_open && contains(Self::panel_bounds(display), x, y) {
            return true;
        }
        // The popover needs pointer capture for its own click handling AND so
        // clicks outside the popover (still inside the bar's owner cell area)
        // keep the menu open instead of falling through to the desktop.
        if let Some(key) = &self.menu_open_for
            && let Some(menu) = self.menu_snapshot()
            && &menu.key == key
            && let Some(visible) = tray::visible_children(&menu.root, &self.menu_path)
            && contains(menu_bounds(self.menu_owner, visible, display), x, y)
        {
            return true;
        }
        false
    }

    fn cursor_shape_at(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        Some(CursorShape::Pointer)
    }

    fn update_app_catalog(&mut self, catalog: &AppCatalog) {
        self.icons = catalog.icons.clone();
    }

    fn update_system_status(&mut self, status: &SystemStatus) {
        self.status = status.clone();
    }

    fn update_realms(&mut self, snapshot: &RealmSnapshot) {
        self.realms = snapshot.clone();
    }

    fn reserved(&self) -> Reserved {
        Reserved {
            top: HUD_HEIGHT as i32,
            ..Reserved::default()
        }
    }

    fn backdrop_blur_sigma(&self) -> f32 {
        BACKDROP_BLUR_SIGMA
    }

    fn backdrop_regions(
        &self,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        let mut regions = vec![BackdropRegion {
            x: 0.0,
            y: 0.0,
            w: display.0,
            h: HUD_HEIGHT,
        }];
        if self.panel_open {
            let panel = Self::panel_bounds(display);
            let radius = 20.0;
            regions.push(BackdropRegion {
                x: panel.x + radius,
                y: panel.y,
                w: (panel.w - radius * 2.0).max(0.0),
                h: panel.h,
            });
            regions.push(BackdropRegion {
                x: panel.x,
                y: panel.y + radius,
                w: panel.w,
                h: (panel.h - radius * 2.0).max(0.0),
            });
        }
        // The popover paints a translucent glass material; tell the blur pass
        // to keep sampling the desktop behind it (same two-rect decomposition
        // as the panel, dodging the rounded corners).
        if let Some(key) = &self.menu_open_for
            && let Some(menu) = self.menu_snapshot()
            && &menu.key == key
            && let Some(visible) = tray::visible_children(&menu.root, &self.menu_path)
        {
            let popover = menu_bounds(self.menu_owner, visible, display);
            let radius = 12.0;
            regions.push(BackdropRegion {
                x: popover.x + radius,
                y: popover.y,
                w: (popover.w - radius * 2.0).max(0.0),
                h: popover.h,
            });
            regions.push(BackdropRegion {
                x: popover.x,
                y: popover.y + radius,
                w: popover.w,
                h: (popover.h - radius * 2.0).max(0.0),
            });
        }
        regions
    }
}

fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let output = Command::new(program)
        .args(args)
        .stderr(Stdio::null())
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

// ---- dbusmenu popover helpers -------------------------------------------

/// Decorate a row label with a toggle glyph (when present) and a submenu
/// chevron suffix (when the row opens a submenu).
fn menu_row_label(row: &MenuNode) -> String {
    let mut label = row.label.clone();
    if row.toggle.is_on() {
        let glyph = match row.toggle {
            crate::tray::MenuToggle::Checkmark(_) => "✓ ",
            crate::tray::MenuToggle::Radio(_) => "● ",
            _ => "",
        };
        label.insert_str(0, glyph);
    }
    if row.has_submenu {
        label.push(' ');
        label.push('▸');
    }
    label
}

/// Compute the visible popover bounds from the owner cell rect, the visible
/// rows (already truncated to the current `menu_path`), and the display. The
/// height counts every visible row + separator and the optional Back header.
fn menu_bounds(owner: Rect, visible: &[MenuNode], display: (f32, f32)) -> Rect {
    let separator_count = visible
        .iter()
        .filter(|row| row.visible && row.kind == crate::tray::MenuEntryKind::Separator)
        .count();
    let row_count = visible
        .iter()
        .filter(|row| row.visible && row.kind != crate::tray::MenuEntryKind::Separator)
        .count();
    let header_count = usize::from(row_count > 0 && separator_count + row_count > 0);
    // The Back header only appears once we descend into a submenu; at root
    // it is not rendered. Detect "non-root view" via owner — we don't know
    // the path here, so the caller may pass a slightly larger conservative
    // height. The actual height is recomputed during render; this function is
    // used for click-away hit-testing only, so overestimating is safe.
    let _ = header_count;
    let height = MENU_PAD * 2.0
        + MENU_HEADER_HEIGHT
        + row_count as f32 * MENU_ROW_HEIGHT
        + separator_count as f32 * MENU_SECTION_HEIGHT;
    place_popup(owner, (MENU_WIDTH, height), display)
}

/// Anchor a popover of `size` against `owner`, preferring above for bottom
/// tiles and below for top tiles, clamped to the display. Mirrors
/// `ass_shell::chrome::app_menu::place_popup` verbatim.
// TODO: share with ass-shell::chrome::app_menu
fn place_popup(owner: Rect, size: (f32, f32), display: (f32, f32)) -> Rect {
    let w = size.0.min((display.0 - MENU_MARGIN * 2.0).max(1.0));
    let h = size.1.min((display.1 - MENU_MARGIN * 2.0).max(1.0));
    let max_x = (display.0 - w - MENU_MARGIN).max(MENU_MARGIN);
    let owner_centre = owner.x + owner.w * 0.5;
    let x = (owner_centre - w * 0.5).clamp(MENU_MARGIN, max_x);
    let above = owner.y - MENU_GAP - h;
    let below = owner.y + owner.h + MENU_GAP;
    let y = if above >= MENU_MARGIN {
        above
    } else if below + h <= display.1 - MENU_MARGIN {
        below
    } else {
        above.clamp(MENU_MARGIN, (display.1 - h - MENU_MARGIN).max(MENU_MARGIN))
    };
    Rect { x, y, w, h }
}
fn volume_icon(status: &SystemStatus) -> Icon {
    if status.muted || status.volume.unwrap_or(0) == 0 {
        Icon::VolumeMuted
    } else if status.volume.unwrap_or(0) < 55 {
        Icon::VolumeLow
    } else {
        Icon::VolumeHigh
    }
}

fn volume_icon_name(status: &SystemStatus) -> &'static str {
    if status.muted || status.volume.unwrap_or(0) == 0 {
        "audio-volume-muted-symbolic"
    } else if status.volume.unwrap_or(0) < 34 {
        "audio-volume-low-symbolic"
    } else if status.volume.unwrap_or(0) < 67 {
        "audio-volume-medium-symbolic"
    } else {
        "audio-volume-high-symbolic"
    }
}

fn network_icon_name(network: NetworkState) -> &'static str {
    match network {
        NetworkState::Wifi => "network-wireless-signal-excellent-symbolic",
        NetworkState::Wired => "network-wired-symbolic",
        NetworkState::Offline => "network-offline-symbolic",
    }
}

fn battery_icon_name(battery: BatteryStatus) -> String {
    let mut level = ((battery.percent as u16 + 5) / 10 * 10).min(100) as u8;
    // Adwaita (and several inherited themes) represents a full charging
    // battery with a distinct "charged" name. Keep the regular charging
    // family for the status bar and use its visually identical 90% endpoint.
    if battery.charging && level == 100 {
        level = 90;
    }
    if battery.charging {
        format!("battery-level-{level}-charging-symbolic")
    } else {
        format!("battery-level-{level}-symbolic")
    }
}

fn take_right(x: &mut f32, width: f32) -> Rect {
    *x -= width;
    let rect = Rect {
        x: *x,
        y: 1.0,
        w: width,
        h: HUD_HEIGHT - 2.0,
    };
    *x -= 2.0;
    rect
}

/// Decide the fold for `sni` visible SNI items and `apps` app-tray cells
/// given `max` slots (assumes `max >= 1`). Within budget everything renders.
/// Past it, one slot is reserved for the "+N" indicator and the remaining
/// capacity goes to app-tray cells first — open windows stay reachable, the
/// more numerous SNI icons fold away.
fn fold_tray(sni: usize, apps: usize, max: usize) -> TrayFold {
    if apps + sni <= max {
        return TrayFold {
            visible_apps: apps,
            visible_sni: sni,
            hidden: 0,
        };
    }
    let capacity = max.saturating_sub(1);
    let visible_apps = apps.min(capacity);
    let visible_sni = sni.min(capacity - visible_apps);
    TrayFold {
        visible_apps,
        visible_sni,
        hidden: apps + sni - visible_apps - visible_sni,
    }
}

/// Raster extensions the `image` crate decodes directly. SVG/SVGZ uses the
/// standard librsvg command-line rasterizer when installed and otherwise
/// falls back to the generic glyph. Mirrors the compositor's app-icon path
/// (`ass::runtime::apps`).
const RASTER_ICON_EXTS: &[&str] = &["png", "jpg", "jpeg", "webp", "bmp", "gif", "tiff", "ico"];
const SVG_ICON_EXTS: &[&str] = &["svg", "svgz"];

/// Resolve an SNI `IconName` through the freedesktop icon theme, decode it,
/// and upload the BGRA8 texture. Returns `None` (caller renders the fallback
/// glyph) when the name is unknown or the file undecodable. Runs on the
/// render thread; the generation-tagged cache in `SniTray` bounds this to at
/// most one resolution per item per name.
fn themed_tray_icon(device: &flux::Device, name: &str) -> Option<flux::Image> {
    let path = ass_apps::resolve_icon_scaled(name, None, &[], 24, TRAY_ICON_SCALE)?;
    let ext = path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default();
    let decoded = decode_icon(&path, &ext, TRAY_ICON_SCALE)?;
    let mut rgba = decoded.to_rgba8();
    if is_symbolic_icon(&path) {
        // Symbolic themes commonly encode a dark CSS foreground intended for
        // toolkit recolouring. Apply the bar's light foreground while
        // preserving every coverage value from SVG antialiasing (same
        // treatment as the compositor's HUD symbols).
        for pixel in rgba.pixels_mut() {
            if pixel[3] != 0 {
                pixel[0] = 246;
                pixel[1] = 246;
                pixel[2] = 248;
            }
        }
    }
    let (w, h) = rgba.dimensions();
    let mut bgra = rgba.into_raw();
    for chunk in bgra.chunks_exact_mut(4) {
        chunk.swap(0, 2);
    }
    match flux::Image::from_bytes(device, w, h, flux::Format::FLUX_FORMAT_BGRA8_UNORM, &bgra) {
        Ok(image) => Some(image),
        Err(error) => {
            log::warn!("tray: icon upload for {} failed: {error}", path.display());
            None
        }
    }
}

/// Decode a resolved theme icon. Raster formats stay in-process; SVG is
/// converted to a bounded PNG on stdout so malformed or enormous vector
/// sources cannot dictate an unbounded GPU texture. Copied from the
/// compositor's app icon cache (`ass::runtime::apps::decode_icon`).
fn decode_icon(path: &Path, ext: &str, scale: u32) -> Option<image::DynamicImage> {
    if RASTER_ICON_EXTS.contains(&ext) {
        return image::open(path).ok();
    }
    if !SVG_ICON_EXTS.contains(&ext) {
        return None;
    }
    let target = ass_apps::DEFAULT_ICON_SIZE
        .saturating_mul(scale.max(1))
        .min(512)
        .to_string();
    let output = Command::new("rsvg-convert")
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
        log::debug!("tray: SVG rasterization failed for {}", path.display());
        return None;
    }
    image::load_from_memory(&output.stdout).ok()
}

/// Symbolic icons (name ends in `-symbolic`) are the only theme icons the bar
/// recolours; regular SNI icons are full-color application artwork.
fn is_symbolic_icon(path: &Path) -> bool {
    path.file_stem()
        .and_then(|stem| stem.to_str())
        .is_some_and(|stem| stem.ends_with("-symbolic"))
}

fn render_text(f: &mut Frame, id: &str, rect: Rect, text: &str, size: f32) {
    f.layer(id, rect, &centered_layer(), |f| {
        f.column_ex(
            &LayoutOpts {
                width: rect.w,
                height: rect.h,
                cross: Align::Center,
                ..Default::default()
            },
            |f| f.label_compact_sized(text, size),
        );
    });
}

fn render_text_left(f: &mut Frame, id: &str, rect: Rect, text: &str, size: f32) {
    f.layer(id, rect, &centered_layer(), |f| {
        f.column_ex(
            &LayoutOpts {
                width: rect.w,
                height: rect.h,
                cross: Align::Start,
                ..Default::default()
            },
            |f| f.label_compact_sized(text, size),
        );
    });
}

fn render_icon_button(
    f: &mut Frame,
    id: &str,
    rect: Rect,
    themed_icon: Option<*mut c_void>,
    fallback: Icon,
    label: &str,
    hovered: bool,
) {
    f.layer(id, rect, &icon_button_opts(hovered), |f| {
        f.row_ex(
            &LayoutOpts {
                width: rect.w,
                height: rect.h,
                gap: if label.is_empty() { 0.0 } else { 4.0 },
                cross: Align::Center,
                ..Default::default()
            },
            |f| {
                match themed_icon {
                    Some(icon) => unsafe {
                        f.image(icon as *mut lens::sys::flux_image, 16.0, 16.0)
                    },
                    None => f.icon(fallback, 15.0),
                }
                if !label.is_empty() {
                    f.label_compact_sized(label, 11.0);
                }
            },
        );
    });
}

fn render_status_card(
    f: &mut Frame,
    id: &str,
    rect: Rect,
    themed_icon: Option<*mut c_void>,
    fallback: Icon,
    copy: (&str, &str),
    hovered: bool,
) {
    let (title, value) = copy;
    f.layer(id, rect, &card_opts(hovered), |f| {
        f.column_ex(
            &LayoutOpts {
                width: rect.w,
                height: rect.h,
                gap: 4.0,
                pad: 10.0,
                cross: Align::Start,
                ..Default::default()
            },
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        gap: 7.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        match themed_icon {
                            Some(icon) => unsafe {
                                f.image(icon as *mut lens::sys::flux_image, 16.0, 16.0)
                            },
                            None => f.icon(fallback, 15.0),
                        }
                        f.label_compact_sized(title, 12.0);
                    },
                );
                f.label_compact_sized(value, 10.5);
            },
        );
    });
}

fn bar_opts() -> OverlayOpts {
    OverlayOpts {
        // The desktop capture underneath provides the blur; this layer is
        // only the neutral macOS-style material tint, shared with the dock.
        bg: Color::rgba(24, 26, 36, 148),
        border: Color::TRANSPARENT,
        border_width: 0.0,
        radius: 0.0,
        pad: 0.0,
        ..Default::default()
    }
}

fn workspace_dot_color(active: bool, hovered: bool) -> Color {
    if active {
        Color::rgba(248, 248, 250, 248)
    } else if hovered {
        Color::rgba(235, 235, 240, 166)
    } else {
        Color::rgba(225, 225, 232, 78)
    }
}

fn workspace_dot_diameter(active: bool, hovered: bool) -> f32 {
    if active {
        WORKSPACE_ACTIVE_DOT
    } else if hovered {
        WORKSPACE_INACTIVE_DOT + 1.0
    } else {
        WORKSPACE_INACTIVE_DOT
    }
}

fn icon_button_opts(hovered: bool) -> OverlayOpts {
    OverlayOpts {
        bg: if hovered {
            Color::rgba(255, 255, 255, 24)
        } else {
            Color::TRANSPARENT
        },
        border: if hovered {
            Color::rgba(255, 255, 255, 18)
        } else {
            Color::TRANSPARENT
        },
        border_width: if hovered { 1.0 } else { 0.0 },
        radius: 8.0,
        pad: 3.0,
        cross: Align::Center,
        ..Default::default()
    }
}

fn panel_opts() -> OverlayOpts {
    OverlayOpts {
        bg: Color::rgba(24, 26, 36, 174),
        border: Color::rgba(255, 255, 255, 46),
        border_width: 1.0,
        radius: 20.0,
        pad: 0.0,
        ..Default::default()
    }
}

fn card_opts(hovered: bool) -> OverlayOpts {
    OverlayOpts {
        bg: if hovered {
            Color::rgba(255, 255, 255, 28)
        } else {
            Color::rgba(255, 255, 255, 15)
        },
        border: Color::rgba(255, 255, 255, 26),
        border_width: 1.0,
        radius: 14.0,
        pad: 0.0,
        ..Default::default()
    }
}

fn small_card_opts(hovered: bool) -> OverlayOpts {
    OverlayOpts {
        radius: 11.0,
        ..card_opts(hovered)
    }
}

fn centered_layer() -> OverlayOpts {
    OverlayOpts {
        bg: Color::TRANSPARENT,
        border: Color::TRANSPARENT,
        border_width: 0.0,
        radius: 0.0,
        pad: 0.0,
        cross: Align::Center,
        ..Default::default()
    }
}

fn sized(w: f32, h: f32) -> LayoutOpts {
    LayoutOpts {
        width: w,
        height: h,
        ..Default::default()
    }
}

fn sized_fill(w: f32, h: f32, bg: Color, radius: f32) -> LayoutOpts {
    LayoutOpts {
        width: w,
        height: h,
        bg,
        radius,
        ..Default::default()
    }
}

fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.w && y < rect.y + rect.h
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut out: String = value.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_bar_reserves_exactly_its_visual_height() {
        assert_eq!(StatusBar::new().reserved().top, HUD_HEIGHT as i32);
    }

    #[test]
    fn agent_indicator_tracks_live_realm_state_and_hides_revoked_realms() {
        let i18n = Localizer::new("en-US");
        let mut model = ass_core::realm::RealmModel::new();
        let bundle = model.create_agent_realm("Neenee", Default::default());
        let mut snapshot = model.snapshot();
        let indicator = agent_indicator(&snapshot, &i18n).expect("live indicator");
        assert!(indicator.active);
        assert_eq!(indicator.label, "Neenee · Active");

        snapshot
            .realms
            .iter_mut()
            .find(|realm| realm.id == bundle.realm)
            .expect("agent Realm")
            .state = RealmState::Paused;
        let indicator = agent_indicator(&snapshot, &i18n).expect("paused indicator");
        assert!(!indicator.active);
        assert_eq!(indicator.label, "Neenee · Paused");

        snapshot
            .realms
            .iter_mut()
            .find(|realm| realm.id == bundle.realm)
            .expect("agent Realm")
            .state = RealmState::Revoked;
        assert!(agent_indicator(&snapshot, &i18n).is_none());
    }

    #[test]
    fn panel_stays_inside_narrow_displays() {
        let panel = StatusBar::panel_bounds((320.0, 480.0));
        assert!(panel.x >= 0.0);
        assert!(panel.x + panel.w <= 320.0);
        assert!(panel.y >= HUD_HEIGHT);
        assert!(panel.y + panel.h <= 480.0);
    }

    #[test]
    fn workspace_dot_states_use_size_and_brightness() {
        assert!(workspace_dot_diameter(true, false) > workspace_dot_diameter(false, false));
        assert!(workspace_dot_diameter(false, true) > workspace_dot_diameter(false, false));
        let active_alpha = workspace_dot_color(true, false).components().3;
        let inactive_alpha = workspace_dot_color(false, false).components().3;
        assert!(active_alpha > inactive_alpha);
    }

    #[test]
    fn status_icons_follow_the_reported_state() {
        let mut status = SystemStatus {
            volume: Some(12),
            ..SystemStatus::default()
        };
        assert_eq!(volume_icon_name(&status), "audio-volume-low-symbolic");
        status.volume = Some(55);
        assert_eq!(volume_icon_name(&status), "audio-volume-medium-symbolic");
        status.muted = true;
        assert_eq!(volume_icon_name(&status), "audio-volume-muted-symbolic");
        assert_eq!(
            network_icon_name(NetworkState::Wired),
            "network-wired-symbolic"
        );
    }

    #[test]
    fn battery_icon_uses_nearest_available_theme_step() {
        assert_eq!(
            battery_icon_name(BatteryStatus {
                percent: 64,
                charging: false,
            }),
            "battery-level-60-symbolic"
        );
        assert_eq!(
            battery_icon_name(BatteryStatus {
                percent: 100,
                charging: true,
            }),
            "battery-level-90-charging-symbolic"
        );
    }

    #[test]
    fn tray_fold_keeps_everything_within_budget() {
        let fold = fold_tray(2, 3, 5);
        assert_eq!(
            (fold.visible_sni, fold.visible_apps, fold.hidden),
            (2, 3, 0)
        );
        let fold = fold_tray(0, 0, 5);
        assert_eq!(
            (fold.visible_sni, fold.visible_apps, fold.hidden),
            (0, 0, 0)
        );
        // Exactly at budget no indicator slot is reserved.
        let fold = fold_tray(2, 3, 5);
        assert_eq!(fold.hidden, 0);
    }

    #[test]
    fn tray_fold_reserves_one_slot_and_keeps_apps_first() {
        // 3 apps + 4 SNI in 5 slots: 4 icon slots, apps win, "+3" indicator.
        let fold = fold_tray(4, 3, 5);
        assert_eq!(
            (fold.visible_sni, fold.visible_apps, fold.hidden),
            (1, 3, 3)
        );
    }

    #[test]
    fn tray_fold_hides_sni_before_apps() {
        let fold = fold_tray(9, 4, 5);
        assert_eq!(
            (fold.visible_sni, fold.visible_apps, fold.hidden),
            (0, 4, 9)
        );
    }

    #[test]
    fn tray_fold_folds_apps_when_they_alone_overflow() {
        let fold = fold_tray(0, 8, 5);
        assert_eq!(
            (fold.visible_sni, fold.visible_apps, fold.hidden),
            (0, 4, 4)
        );
    }

    #[test]
    fn symbolic_icon_detection_uses_the_file_stem() {
        assert!(is_symbolic_icon(Path::new(
            "/usr/share/icons/Adwaita/symbolic/apps/foo-symbolic.svg"
        )));
        assert!(!is_symbolic_icon(Path::new(
            "/usr/share/icons/hicolor/48x48/apps/foo.png"
        )));
        // "symbolic" appearing elsewhere in the name does not count.
        assert!(!is_symbolic_icon(Path::new("symbolic.png")));
        assert!(!is_symbolic_icon(Path::new("foo-symbolicx.svg")));
    }
}
