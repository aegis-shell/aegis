//! The command panel: a full-screen modal overlay in the Sword Art
//! Online menu language (ADR-0080) — frosted white floating panels with an
//! amber accent over the standard dark blurred scrim.
//!
//! The HUD is display-only, so the interactions it used to host
//! live here: quick settings (volume, brightness, radios, do-not-disturb),
//! StatusNotifierItem tray activation with host-rendered dbusmenu popovers,
//! and the notification list with dismissal. The System section also shows
//! the Agent Workspaces status row that the HUD's dropped right chip once
//! carried (ADR-0083). The panel opens through the
//! `Super+S` keybinding or a four-finger touchpad swipe down, and closes on
//! Escape, a scrim click, the same binding, or a four-finger swipe up.
//!
//! Like the HUD and the dock, the panel is compositor-owned lens
//! chrome on the [`aegis_shell`] `Chrome` seam: snapshots arrive through the
//! trait each frame, and user intents leave through `ChromeEvents` plus the
//! shared `aegis-tray` command channel.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, mpsc};

use aegis_core::input::KeyChar;
use aegis_core::notify::{Notification, NotificationQueue};
use aegis_core::realm::{RealmKind, RealmSnapshot, RealmState};
use aegis_core::window::{SpaceUse, Window};
use aegis_core::workspace::WorkspaceSnapshot;
use aegis_design::materials;
use aegis_design::themes;
use aegis_design::tokens::Sao;
use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect, Theme};

use aegis_shell::{
    AppCatalog, BackdropRegion, Chrome, ChromeEvents, CursorShape, IconSet, Localizer, Message,
    NetworkState, SystemAction, SystemStatus, place_popup, truncate,
};
use aegis_tray::{MenuNode, MenuState, TrayCommand, TrayIcon, TraySnapshot};

mod rendering;

use rendering::*;

#[cfg(test)]
mod tests;

const SCRIM_ALPHA: u8 = 132;
const BACKDROP_BLUR_SIGMA: f32 = 14.0;
const MENU_PANEL_W: f32 = 240.0;
const CONTENT_PANEL_W: f32 = 560.0;
const CONTENT_PANEL_H: f32 = 520.0;
const PANEL_GAP: f32 = 12.0;
/// Content panel's reveal lags the menu panel's by this fraction.
const CONTENT_STAGGER: f32 = 0.18;

// dbusmenu popover geometry. Placement follows the shared shell popup policy.
const MENU_WIDTH: f32 = 236.0;
const MENU_PAD: f32 = 7.0;
const MENU_ROW_HEIGHT: f32 = 28.0;
const MENU_HEADER_HEIGHT: f32 = 23.0;
const MENU_SECTION_HEIGHT: f32 = 7.0;

const TRAY_COLS: usize = 5;
const TRAY_CELL_W: f32 = 96.0;
const TRAY_CELL_H: f32 = 76.0;
const MAX_MESSAGE_ROWS: usize = 8;

/// The panel's left-column sections.
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
    prev_right_down: bool,
    status: SystemStatus,
    /// Agent Realm aggregate behind the System section's Agent Workspaces
    /// status row (ADR-0083).
    realms: RealmSnapshot,
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
        CommandPanel {
            open: false,
            reveal: 0.0,
            section: Section::System,
            reduced_motion: false,
            prev_down: false,
            prev_right_down: false,
            status: SystemStatus::default(),
            realms: aegis_core::realm::RealmModel::new().snapshot(),
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
            prev_right_down: false,
            status: SystemStatus::default(),
            realms: aegis_core::realm::RealmModel::new().snapshot(),
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

    /// The side menu and content panel bounds, centered as one cluster. On
    /// narrow outputs the menu panel shrinks proportionally so the cluster
    /// always fits inside the display.
    fn cluster_bounds(display: (f32, f32)) -> (Rect, Rect) {
        let total_w =
            (MENU_PANEL_W + PANEL_GAP + CONTENT_PANEL_W).min((display.0 - 32.0).max(120.0));
        let menu_w = MENU_PANEL_W.min(total_w * 0.32);
        let content_w = (total_w - menu_w - PANEL_GAP).max(60.0);
        let h = (display.1 - 48.0).clamp(120.0, CONTENT_PANEL_H);
        let x = ((display.0 - total_w) * 0.5).max(8.0);
        let y = ((display.1 - h) * 0.5).max(8.0);
        let menu = Rect { x, y, w: menu_w, h };
        let content = Rect {
            x: x + menu_w + PANEL_GAP,
            y,
            w: content_w,
            h,
        };
        (menu, content)
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

// ---- rendering -----------------------------------------------------------

impl CommandPanel {
    /// Bounds of the currently open dbusmenu popover, if any.
    fn open_popover_bounds(&mut self, display: (f32, f32)) -> Option<Rect> {
        let key = self.menu_open_for.clone()?;
        let menu = self.menu_snapshot().filter(|menu| menu.key == key)?;
        aegis_tray::visible_children(&menu.root, &self.menu_path)
            .map(|visible| menu_bounds(self.menu_owner, visible, display))
    }

    /// The SAO section menu: white floating panel, amber ring header, and
    /// one ringed row per section (selected = solid amber highlight bar).
    fn render_menu_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        cursor: (f32, f32),
        pressed: bool,
        i18n: &Localizer,
    ) {
        let sao = Sao::classic();
        let slide = (1.0 - ease_out_cubic(progress)) * -24.0;
        let rect = Rect {
            x: rect.x + slide,
            ..rect
        };
        f.layer(
            "aegis-sao-menu-panel",
            rect,
            &OverlayOpts {
                bg: fade_color(sao.surface, progress),
                border: fade_color(sao.border, progress),
                border_width: 1.0,
                radius: 16.0,
                pad: 0.0,
                ..Default::default()
            },
            |f| {
                f.column_ex(&sized(rect.w, rect.h), |_| {});
            },
        );

        // Header: amber ring + core over the panel title.
        let header_center = (rect.x + 34.0, rect.y + 36.0);
        render_ring(
            f,
            "aegis-sao-menu-header-ring",
            header_center,
            34.0,
            fade_color(sao.accent, progress),
            1.6,
        );
        render_disc(
            f,
            "aegis-sao-menu-header-core",
            header_center,
            8.0,
            fade_color(sao.accent, progress),
        );
        let original = f.theme();
        f.set_theme(faded_theme(themes::sao(&sao), progress));
        f.layer(
            "aegis-sao-menu-title",
            Rect {
                x: rect.x + 58.0,
                y: rect.y + 22.0,
                w: (rect.w - 70.0).max(1.0),
                h: 28.0,
            },
            &transparent(),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: (rect.w - 70.0).max(1.0),
                        height: 28.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| f.label_compact_sized(i18n.text(Message::CommandPanel), 14.5),
                );
            },
        );
        f.set_theme(original);

        // Section rows.
        let row_y0 = rect.y + 70.0;
        for (index, section) in Section::ALL.iter().enumerate() {
            let row = Rect {
                x: rect.x + 10.0,
                y: row_y0 + index as f32 * 50.0,
                w: rect.w - 20.0,
                h: 44.0,
            };
            let selected = self.section == *section;
            let hovered = contains(row, cursor.0, cursor.1);
            if selected {
                f.layer(
                    &format!("aegis-sao-menu-row-bg-{index}"),
                    row,
                    &OverlayOpts {
                        bg: fade_color(sao.accent, progress),
                        border: Color::TRANSPARENT,
                        radius: 10.0,
                        ..Default::default()
                    },
                    |_| {},
                );
            } else if hovered {
                f.layer(
                    &format!("aegis-sao-menu-row-bg-{index}"),
                    row,
                    &OverlayOpts {
                        bg: fade_color(sao.accent_soft, progress),
                        border: Color::TRANSPARENT,
                        radius: 10.0,
                        ..Default::default()
                    },
                    |_| {},
                );
            }
            let center = (row.x + 26.0, row.y + row.h * 0.5);
            let (glyph_color, label_color) = if selected {
                render_disc(
                    f,
                    &format!("aegis-sao-menu-disc-{index}"),
                    center,
                    30.0,
                    fade_color(sao.accent, progress),
                );
                (sao.on_accent, sao.on_accent)
            } else {
                render_ring(
                    f,
                    &format!("aegis-sao-menu-ring-{index}"),
                    center,
                    30.0,
                    fade_color(sao.accent, progress),
                    1.5,
                );
                (sao.accent, sao.text)
            };
            let original = f.theme();
            let glyph_theme = themes::sao(&sao).with_fg(glyph_color);
            f.set_theme(faded_theme(glyph_theme, progress));
            f.layer(
                &format!("aegis-sao-menu-glyph-{index}"),
                Rect {
                    x: center.0 - 15.0,
                    y: center.1 - 15.0,
                    w: 30.0,
                    h: 30.0,
                },
                &transparent(),
                |f| {
                    f.row_ex(
                        &LayoutOpts {
                            width: 30.0,
                            height: 30.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            f.flex(1.0);
                            f.spacer(0.0);
                            f.icon(section.icon(), 15.0);
                            f.flex(1.0);
                            f.spacer(0.0);
                        },
                    );
                },
            );
            let label_theme = themes::sao(&sao).with_fg(label_color);
            f.set_theme(faded_theme(label_theme, progress));
            f.layer(
                &format!("aegis-sao-menu-label-{index}"),
                Rect {
                    x: row.x + 48.0,
                    y: row.y,
                    w: (row.w - 56.0).max(1.0),
                    h: row.h,
                },
                &transparent(),
                |f| {
                    f.row_ex(
                        &LayoutOpts {
                            width: (row.w - 56.0).max(1.0),
                            height: row.h,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| f.label_compact_sized(section.label(i18n), 13.0),
                    );
                },
            );
            f.set_theme(original);
            if pressed && hovered {
                self.select_section(*section);
            }
        }
    }

    /// The white content panel: section title header plus the active
    /// section's body, sliding up slightly as it reveals.
    #[allow(clippy::too_many_arguments)]
    fn render_content_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        display: (f32, f32),
        cursor: (f32, f32),
        pressed: bool,
        right_pressed: bool,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let sao = Sao::classic();
        let rise = (1.0 - progress) * 16.0;
        let rect = Rect {
            y: rect.y + rise,
            ..rect
        };
        f.layer(
            "aegis-sao-content-panel",
            rect,
            &OverlayOpts {
                bg: fade_color(sao.surface, progress),
                border: fade_color(sao.border, progress),
                border_width: 1.0,
                radius: 16.0,
                pad: 0.0,
                ..Default::default()
            },
            |f| {
                f.column_ex(&sized(rect.w, rect.h), |_| {});
            },
        );

        // Header: section title + close button.
        let original = f.theme();
        f.set_theme(faded_theme(themes::sao(&sao), progress));
        let mut close_clicked = false;
        let header = Rect {
            x: rect.x + 18.0,
            y: rect.y + 10.0,
            w: rect.w - 36.0,
            h: 34.0,
        };
        let section_label = self.section.label(i18n);
        f.layer("aegis-sao-content-header", header, &transparent(), |f| {
            f.row_ex(
                &LayoutOpts {
                    width: header.w,
                    height: header.h,
                    gap: 10.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |f| {
                    f.label_compact_sized(section_label, 15.0);
                    f.flex(1.0);
                    f.spacer(0.0);
                    f.size_next(30.0, 28.0);
                    close_clicked = f.icon_button(Icon::X);
                },
            );
        });
        f.set_theme(original);
        if close_clicked {
            self.close();
        }

        let area = Rect {
            x: rect.x + 18.0,
            y: rect.y + 52.0,
            w: rect.w - 36.0,
            h: rect.h - 70.0,
        };
        match self.section {
            Section::System => self.render_system_section(f, area, progress, i18n, out),
            Section::Tray => {
                self.render_tray_section(f, area, progress, cursor, pressed, right_pressed, i18n)
            }
            Section::Messages => {
                self.render_messages_section(f, area, progress, cursor, pressed, i18n, out)
            }
        }
        let _ = display;
    }

    /// Quick settings, ported from the status bar's old status-and-controls
    /// panel and laid out as full-width SAO groups.
    fn render_system_section(
        &mut self,
        f: &mut Frame,
        area: Rect,
        progress: f32,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let sao = Sao::classic();
        let original = f.theme();
        f.set_theme(faded_theme(themes::sao(&sao), progress));
        let status = self.status.clone();
        let volume_themed = self.themed_icon(volume_icon_name(&status));
        let network_themed = self.themed_icon(network_icon_name(status.network));
        let network_label = network_text(&status, i18n);
        let agent_indicator = agent_workspace_indicator(&self.realms, i18n);
        let agent_status_text = if agent_indicator.state == AgentWorkspaceState::Idle {
            agent_workspace_state_label(agent_indicator.state, i18n).to_string()
        } else {
            agent_indicator.label.clone()
        };
        f.layer("aegis-sao-system", area, &transparent(), |f| {
            f.column_ex(
                &LayoutOpts {
                    width: area.w,
                    height: area.h,
                    gap: 12.0,
                    cross: Align::Stretch,
                    ..Default::default()
                },
                |f| {
                    // Sound group.
                    f.row_ex(
                        &LayoutOpts {
                            height: 22.0,
                            gap: 8.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            match volume_themed {
                                Some(icon) => unsafe {
                                    f.image(icon as *mut lens::sys::flux_image, 16.0, 16.0)
                                },
                                None => f.icon(volume_icon(&status), 15.0),
                            }
                            f.label_compact_sized(i18n.text(Message::Sound), 12.5);
                            f.flex(1.0);
                            f.spacer(0.0);
                            f.label_compact_sized(
                                &status
                                    .volume
                                    .map(|level| format!("{level}%"))
                                    .unwrap_or_else(|| "--".into()),
                                11.0,
                            );
                        },
                    );
                    if status.volume.is_some() {
                        let mut volume = status.volume.unwrap_or(0) as f32;
                        if f.slider("##sao-volume", &mut volume, 0.0, 100.0) {
                            out.system_actions.push(SystemAction::SetVolume {
                                level: volume.round().clamp(0.0, 100.0) as u8,
                            });
                        }
                        let mut muted = status.muted;
                        if f.checkbox(i18n.text(Message::Muted), &mut muted) {
                            out.system_actions.push(SystemAction::ToggleMute);
                        }
                    } else {
                        unavailable_control(f, i18n.text(Message::Volume), i18n);
                    }
                    f.separator();

                    // Brightness group.
                    f.row_ex(
                        &LayoutOpts {
                            height: 22.0,
                            gap: 8.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            f.icon(Icon::Zap, 15.0);
                            f.label_compact_sized(i18n.text(Message::Brightness), 12.5);
                            f.flex(1.0);
                            f.spacer(0.0);
                            f.label_compact_sized(
                                &status
                                    .brightness
                                    .map(|level| format!("{level}%"))
                                    .unwrap_or_else(|| "--".into()),
                                11.0,
                            );
                        },
                    );
                    if status.brightness.is_some() {
                        let mut brightness = status.brightness.unwrap_or(1) as f32;
                        if f.slider("##sao-brightness", &mut brightness, 1.0, 100.0) {
                            out.system_actions.push(SystemAction::SetBrightness {
                                level: brightness.round().clamp(1.0, 100.0) as u8,
                            });
                        }
                    } else {
                        unavailable_control(f, i18n.text(Message::Brightness), i18n);
                    }
                    f.separator();

                    // Connectivity group.
                    f.row_ex(
                        &LayoutOpts {
                            height: 22.0,
                            gap: 8.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            match network_themed {
                                Some(icon) => unsafe {
                                    f.image(icon as *mut lens::sys::flux_image, 16.0, 16.0)
                                },
                                None => f.icon(Icon::Globe, 15.0),
                            }
                            f.label_compact_sized(i18n.text(Message::Connectivity), 12.5);
                            f.flex(1.0);
                            f.spacer(0.0);
                            f.label_compact_sized(network_label, 11.0);
                        },
                    );
                    if status.wifi_enabled.is_some() {
                        let mut wifi = status.wifi_enabled.unwrap_or(false);
                        if f.checkbox(i18n.text(Message::Wifi), &mut wifi) {
                            out.system_actions
                                .push(SystemAction::SetWifi { enabled: wifi });
                        }
                    } else {
                        unavailable_control(f, i18n.text(Message::Wifi), i18n);
                    }
                    if status.bluetooth_enabled.is_some() {
                        let mut bluetooth = status.bluetooth_enabled.unwrap_or(false);
                        if f.checkbox(i18n.text(Message::Bluetooth), &mut bluetooth) {
                            out.system_actions
                                .push(SystemAction::SetBluetooth { enabled: bluetooth });
                        }
                    } else {
                        unavailable_control(f, i18n.text(Message::Bluetooth), i18n);
                    }
                    f.separator();

                    // Desktop group.
                    f.row_ex(
                        &LayoutOpts {
                            height: 22.0,
                            gap: 8.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            f.icon(Icon::Grid, 15.0);
                            f.label_compact_sized(i18n.text(Message::Desktop), 12.5);
                        },
                    );
                    let mut do_not_disturb = status.do_not_disturb;
                    if f.checkbox(i18n.text(Message::DoNotDisturb), &mut do_not_disturb) {
                        out.system_actions.push(SystemAction::SetDoNotDisturb {
                            enabled: do_not_disturb,
                        });
                    }
                    let mut tiled = status.tiled;
                    if f.checkbox(i18n.text(Message::TiledLayout), &mut tiled) {
                        out.system_actions
                            .push(SystemAction::SetTiling { enabled: tiled });
                    }
                    f.separator();

                    // Agent Workspaces status row: display-only aggregate of
                    // the live Agent Realms (moved here from the HUD's right
                    // chip, ADR-0083).
                    f.row_ex(
                        &LayoutOpts {
                            height: 22.0,
                            gap: 8.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            f.icon(Icon::Users, 15.0);
                            f.label_compact_sized(i18n.text(Message::AiWorkspaces), 12.5);
                            f.flex(1.0);
                            f.spacer(0.0);
                            f.label_compact_sized(&agent_status_text, 11.0);
                        },
                    );
                    f.separator();

                    // Session group: an immediate lock trigger and the
                    // "always on" idle inhibitor, which suspends automatic
                    // dimming, locking, and display power-off while held.
                    if f.button(i18n.text(Message::LockNow)) {
                        out.lock = true;
                    }
                    let mut always_on = status.idle_inhibited;
                    if f.checkbox(i18n.text(Message::AlwaysOn), &mut always_on) {
                        out.system_actions
                            .push(SystemAction::SetIdleInhibit { inhibit: always_on });
                    }
                },
            );
        });
        f.set_theme(original);
    }

    /// The interactive tray grid: left-click activates, right-click opens
    /// the host-rendered dbusmenu popover (or `SecondaryActivate`).
    #[allow(clippy::too_many_arguments)]
    fn render_tray_section(
        &mut self,
        f: &mut Frame,
        area: Rect,
        progress: f32,
        cursor: (f32, f32),
        pressed: bool,
        right_pressed: bool,
        i18n: &Localizer,
    ) {
        let sao = Sao::classic();
        let mut cells = self.sni_cells();
        if cells.is_empty() {
            let original = f.theme();
            let muted = themes::sao_muted(themes::sao(&sao), &sao);
            f.set_theme(faded_theme(muted, progress));
            f.layer("aegis-sao-tray-empty", area, &transparent(), |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: area.w,
                        height: area.h,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        f.flex(1.0);
                        f.spacer(0.0);
                        f.label_compact_sized(i18n.text(Message::NoTrayItems), 12.0);
                        f.flex(1.0);
                        f.spacer(0.0);
                    },
                );
            });
            f.set_theme(original);
            return;
        }
        for (index, cell) in cells.iter_mut().enumerate() {
            let col = index % TRAY_COLS;
            let grid_row = index / TRAY_COLS;
            let rect = Rect {
                x: area.x + col as f32 * TRAY_CELL_W,
                y: area.y + grid_row as f32 * TRAY_CELL_H,
                w: TRAY_CELL_W - 8.0,
                h: TRAY_CELL_H - 8.0,
            };
            cell.rect = rect;
            let hovered = contains(rect, cursor.0, cursor.1);
            let texture = if cell.textured {
                self.tray
                    .as_ref()
                    .and_then(|tray| tray.textures.get(&cell.key))
                    .map(|(_, image)| image.as_raw())
            } else {
                None
            };
            let fallback_themed = self.themed_icon("application-x-executable-symbolic");
            let title = truncate(&cell.title, 12);
            let original = f.theme();
            f.set_theme(faded_theme(themes::sao(&sao), progress));
            f.layer(
                &format!("aegis-sao-tray-cell-{index}"),
                rect,
                &OverlayOpts {
                    bg: if hovered {
                        fade_color(sao.accent_soft, progress)
                    } else {
                        Color::TRANSPARENT
                    },
                    border: Color::TRANSPARENT,
                    radius: 10.0,
                    ..Default::default()
                },
                |f| {
                    f.column_ex(
                        &LayoutOpts {
                            width: rect.w,
                            height: rect.h,
                            gap: 3.0,
                            pad: 6.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            match texture {
                                Some(texture) => unsafe {
                                    f.image(texture as *mut lens::sys::flux_image, 28.0, 28.0)
                                },
                                None => match fallback_themed {
                                    Some(icon) => unsafe {
                                        f.image(icon as *mut lens::sys::flux_image, 26.0, 26.0)
                                    },
                                    None => f.icon(Icon::FileText, 22.0),
                                },
                            }
                            f.label_compact_sized(&title, 9.0);
                        },
                    );
                },
            );
            f.set_theme(original);
            let (x, y) = (cursor.0 as i32, cursor.1 as i32);
            if pressed && hovered {
                self.send_tray_command(TrayCommand::Activate {
                    key: cell.key.clone(),
                    x,
                    y,
                });
            } else if right_pressed && hovered {
                // Items that expose a Menu object path get the host-rendered
                // popover; everything else keeps the SNI `SecondaryActivate`
                // fallback.
                if cell.has_menu {
                    self.menu_open_for = Some(cell.key.clone());
                    self.menu_path = vec![0];
                    self.menu_owner = rect;
                    self.menu_just_opened = true;
                    self.send_tray_command(TrayCommand::FetchMenu {
                        key: cell.key.clone(),
                    });
                } else {
                    self.send_tray_command(TrayCommand::SecondaryActivate {
                        key: cell.key.clone(),
                        x,
                        y,
                    });
                }
            }
        }
        // Re-anchor the open popover to its owner cell; close it when the
        // backing item vanished from the snapshot.
        if let Some(key) = self.menu_open_for.clone() {
            if let Some(cell) = cells.iter().find(|cell| cell.key == key) {
                self.menu_owner = cell.rect;
            } else {
                self.menu_open_for = None;
                self.menu_path.clear();
                self.send_tray_command(TrayCommand::CloseMenu { key });
            }
        }
    }

    /// The notification list, newest first; a row click dismisses.
    #[allow(clippy::too_many_arguments)]
    fn render_messages_section(
        &mut self,
        f: &mut Frame,
        area: Rect,
        progress: f32,
        cursor: (f32, f32),
        pressed: bool,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let sao = Sao::classic();
        let notifications = self.notification_snapshot();
        let original = f.theme();
        if notifications.is_empty() {
            let muted = themes::sao_muted(themes::sao(&sao), &sao);
            f.set_theme(faded_theme(muted, progress));
            f.layer("aegis-sao-messages-empty", area, &transparent(), |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: area.w,
                        height: area.h,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        f.flex(1.0);
                        f.spacer(0.0);
                        f.label_compact_sized(i18n.text(Message::NoNotifications), 12.0);
                        f.flex(1.0);
                        f.spacer(0.0);
                    },
                );
            });
            f.set_theme(original);
            return;
        }
        let base = themes::sao(&sao);
        let row_theme = faded_theme(base, progress);
        let muted_theme = faded_theme(themes::sao_muted(base, &sao), progress);
        f.set_theme(row_theme);
        for (index, notification) in notifications
            .iter()
            .rev()
            .take(MAX_MESSAGE_ROWS)
            .enumerate()
        {
            let row = Rect {
                x: area.x,
                y: area.y + index as f32 * 64.0,
                w: area.w,
                h: 58.0,
            };
            if row.y + row.h > area.y + area.h {
                break;
            }
            let hovered = contains(row, cursor.0, cursor.1);
            let summary = truncate(&notification.summary, 48);
            let body = truncate(&notification.body, 72);
            f.layer(
                &format!("aegis-sao-message-{}", notification.id),
                row,
                &OverlayOpts {
                    bg: if hovered {
                        fade_color(sao.accent_soft, progress)
                    } else {
                        fade_color(sao.surface_dim, progress)
                    },
                    border: Color::TRANSPARENT,
                    radius: 12.0,
                    pad: 0.0,
                    ..Default::default()
                },
                |f| {
                    f.column_ex(
                        &LayoutOpts {
                            width: row.w,
                            height: row.h,
                            gap: 2.0,
                            pad: 10.0,
                            cross: Align::Start,
                            ..Default::default()
                        },
                        |f| {
                            f.label_compact_sized(&summary, 12.5);
                            if !body.is_empty() {
                                f.set_theme(muted_theme);
                                f.label_compact_sized(&body, 10.5);
                                f.set_theme(row_theme);
                            }
                        },
                    );
                },
            );
            if pressed && hovered {
                out.dismissed_notification = Some(notification.id);
            }
        }
        f.set_theme(original);
    }

    /// Render the dbusmenu popover. The visible rows come from walking
    /// `menu.root.children` along `self.menu_path`. Submenu rows push onto
    /// `menu_path`, leaf rows send `MenuEvent` and dismiss the popover, and
    /// click-away closes it unless the press falls on the owner tray cell.
    fn render_tray_menu(
        &mut self,
        f: &mut Frame,
        menu: &MenuState,
        display: (f32, f32),
        cursor: (f32, f32),
        pressed: bool,
    ) {
        // If a targeted submenu id no longer exists (the worker truncated the
        // tree on `LayoutUpdated`), pop back to the nearest valid level.
        while aegis_tray::visible_children(&menu.root, &self.menu_path).is_none()
            && self.menu_path.len() > 1
        {
            self.menu_path.pop();
        }
        let visible = match aegis_tray::visible_children(&menu.root, &self.menu_path) {
            Some(rows) => rows,
            None => return,
        };

        let popover_bounds = menu_bounds(self.menu_owner, visible, display);
        let in_owner = contains(self.menu_owner, cursor.0, cursor.1);
        let in_popover = contains(popover_bounds, cursor.0, cursor.1);
        if !self.menu_just_opened && pressed && !in_owner && !in_popover {
            self.close_menu(menu.key.clone());
            return;
        }
        self.menu_just_opened = false;

        let sao = Sao::classic();
        let original_theme = f.theme();
        let menu_theme = themes::sao(&sao);
        let dim_theme = themes::sao_muted(menu_theme, &sao);

        let header_visible = self.menu_path.len() > 1;
        let mut action: Option<MenuRowAction> = None;
        f.set_theme(menu_theme);
        f.layer(
            "aegis-sao-sni-menu",
            popover_bounds,
            &materials::sao_panel(&sao),
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
                            f.push_id("sao-menu-back");
                            if f.selectable("‹ Back", false) {
                                action = Some(MenuRowAction::Back);
                            }
                            f.pop_id();
                        }
                        for row in visible.iter() {
                            if !row.visible {
                                continue;
                            }
                            if row.kind == aegis_tray::MenuEntryKind::Separator {
                                f.size_next(inner_w, MENU_SECTION_HEIGHT);
                                f.separator();
                                continue;
                            }
                            f.size_next(inner_w, MENU_ROW_HEIGHT);
                            f.push_id(&format!("sao-menu-row-{}", row.id));
                            if !row.enabled {
                                // Disabled rows render as inert labels with a
                                // dim foreground — selectable would still
                                // capture the click, which the dbusmenu spec
                                // forbids.
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
                        }
                    },
                );
            },
        );
        f.set_theme(original_theme);

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
}

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
        self.advance(raw.dt_seconds.max(0.0));
        let display = (raw.display_size.x, raw.display_size.y);
        let cursor = (raw.cursor.x, raw.cursor.y);
        let down = raw.mouse_down.first().copied().unwrap_or(false);
        let pressed = down && !self.prev_down;
        let right_down = raw.mouse_down.get(1).copied().unwrap_or(false);
        let right_pressed = right_down && !self.prev_right_down;
        if !self.active() {
            self.prev_down = down;
            self.prev_right_down = right_down;
            return;
        }
        let reveal = self.reveal.clamp(0.0, 1.0);
        let (menu_rect, content_rect) = Self::cluster_bounds(display);

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

        // Click-away: a press landing on neither panel nor an open tray
        // popover dismisses the panel.
        let on_popover = self
            .open_popover_bounds(display)
            .map(|rect| contains(rect, cursor.0, cursor.1))
            .unwrap_or(false);
        if pressed
            && !contains(menu_rect, cursor.0, cursor.1)
            && !contains(content_rect, cursor.0, cursor.1)
            && !on_popover
        {
            self.close();
        }

        // Sections stop accepting presses while the panel is closing.
        let pressed = pressed && self.open;
        let right_pressed = right_pressed && self.open;

        let menu_progress = stagger(reveal, 0.0);
        let content_progress = ease_out_cubic(stagger(reveal, CONTENT_STAGGER));
        self.render_menu_panel(f, menu_rect, menu_progress, cursor, pressed, i18n);
        self.render_content_panel(
            f,
            content_rect,
            content_progress,
            display,
            cursor,
            pressed,
            right_pressed,
            i18n,
            out,
        );

        // The dbusmenu popover floats above the panels.
        if self.menu_open_for.is_some()
            && let Some(menu) = self.menu_snapshot()
            && Some(&menu.key) == self.menu_open_for.as_ref()
        {
            self.render_tray_menu(f, &menu, display, cursor, pressed);
        }

        self.prev_down = down;
        self.prev_right_down = right_down;
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

    fn update_realms(&mut self, snapshot: &RealmSnapshot) {
        self.realms = snapshot.clone();
    }

    fn update_windows(&mut self, windows: &[Window]) {
        // A fullscreen window owns the whole output; get out of its way.
        if SpaceUse::from_windows(windows) == SpaceUse::Fullscreen && self.open {
            self.close();
        }
    }

    fn anim_pending(&self) -> bool {
        let target = if self.open { 1.0 } else { 0.0 };
        (self.reveal - target).abs() > 0.002
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
