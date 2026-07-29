//! The session status bar: a compact top bar with integrated workspace state,
//! clock, registered StatusNotifierItem tray entries, system status, and a
//! small status-and-controls panel. The rendering and interaction remain
//! compositor-owned lens chrome.

use std::cell::RefCell;
use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use aegis_core::app::BuiltInApplication;
use aegis_core::notify::{Notification, NotificationQueue};
use aegis_core::realm::{RealmKind, RealmSnapshot, RealmState};
use aegis_core::window::{SpaceUse, Window};
use aegis_core::workspace::WorkspaceSnapshot;
use aegis_design::{Design, materials, themes};
use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect, Theme};

use aegis_shell::{
    AppCatalog, BackdropRegion, BatteryStatus, Chrome, ChromeEvents, CursorShape, HUD_HEIGHT,
    IconSet, Localizer, Message, NetworkState, Reserved, SystemAction, SystemStatus, place_popup,
    truncate,
};

use crate::tray::{self, MenuNode, MenuState, TrayCommand, TrayIcon, TraySnapshot};

mod rendering;

use rendering::*;

const WORKSPACE_SLOT_W: f32 = 18.0;
const WORKSPACE_ACTIVE_DOT: f32 = 8.0;
const WORKSPACE_INACTIVE_DOT: f32 = 6.0;
const LEFT_MARGIN: f32 = 10.0;
const RIGHT_MARGIN: f32 = 6.0;
const TRAY_CELL_W: f32 = 26.0;
const MAX_TRAY_ITEMS: usize = 5;
const PANEL_GAP: f32 = 6.0;
const PANEL_W: f32 = 420.0;
const PANEL_H: f32 = 400.0;
const CLOCK_POLL_INTERVAL: Duration = Duration::from_secs(15);
const BACKDROP_BLUR_SIGMA: f32 = 12.0;
const AGENT_INDICATOR_MAX_W: f32 = 154.0;
const AGENT_PANEL_GAP: f32 = 7.0;
const AGENT_PANEL_W: f32 = 372.0;
const AGENT_PANEL_H: f32 = 224.0;

// dbusmenu popover geometry. Placement follows the shared shell popup policy.
const MENU_WIDTH: f32 = 236.0;
const MENU_PAD: f32 = 7.0;
const MENU_ROW_HEIGHT: f32 = 28.0;
const MENU_HEADER_HEIGHT: f32 = 23.0;
const MENU_SECTION_HEIGHT: f32 = 7.0;

/// Full top-bar status bar.
pub struct StatusBar {
    prev_down: bool,
    prev_right_down: bool,
    fullscreen_active: bool,
    panel_open: bool,
    /// Whether the Agent Workspaces surface is expanded from its permanent
    /// status-bar entry.
    agent_panel_open: bool,
    /// Eased reveal amount retained while the panel closes, so the surface
    /// contracts back into the entry instead of disappearing in one frame.
    agent_panel_reveal: f32,
    /// Continuous phase for the compositor-owned workspace visualization.
    agent_visual_phase: f32,
    /// Accessibility reduced-motion policy shared with the other chrome.
    reduced_motion: bool,
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
    /// Notification list cache keyed by the queue's revision; re-cloned only
    /// when the queue actually changes.
    notification_cache: Option<(u64, Arc<Vec<Notification>>)>,
    /// dbusmenu tree cache keyed by the snapshot's `menu_revision`
    /// (`RefCell` because `captures_pointer`/`backdrop_regions` read the
    /// menu through `&self`).
    menu_cache: RefCell<Option<MenuSnapshotCache>>,
}

/// Render-thread half of the StatusNotifierItem tray: the shared snapshot the
/// worker writes, the command channel back to it, and the texture cache the
/// bar uploads from item pixmaps (theme-named icons arrive pre-resolved as
/// pixmaps; see `tray::icon`).
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
    has_menu: bool,
    textured: bool,
    /// Cell rect (filled in during the tray-row layout pass).
    rect: Rect,
}

/// Render-side cache of the shared dbusmenu tree, tagged with the worker's
/// `menu_revision` so the (potentially large) tree is re-cloned only when
/// the menu actually changed, and shared cheaply between the render,
/// hit-test, and backdrop paths within a frame.
struct MenuSnapshotCache {
    revision: u64,
    menu: Option<Arc<MenuState>>,
}

/// How registered SNI items fold into the slot budget. Past budget the last
/// slot becomes a "+N" overflow indicator counting everything hidden.
struct TrayFold {
    visible: usize,
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
    /// [`aegis_shell::Shell::new`]) to upload SNI tray pixmaps to the GPU; the
    /// caller must keep it alive past the bar. Application icons arrive
    /// through [`Chrome::update_app_catalog`], seeded on registration by
    /// [`aegis_shell::Shell::add`]. When the session bus is unavailable the SNI
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
                cached_cells: Vec::new(),
            })
        });
        let now = Instant::now();
        StatusBar {
            prev_down: false,
            prev_right_down: false,
            fullscreen_active: false,
            panel_open: false,
            agent_panel_open: false,
            agent_panel_reveal: 0.0,
            agent_visual_phase: 0.0,
            reduced_motion: false,
            icons: IconSet::default(),
            notifications,
            status: SystemStatus::default(),
            realms: aegis_core::realm::RealmModel::new().snapshot(),
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
            notification_cache: None,
            menu_cache: RefCell::new(None),
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

    fn panel_notification_list_y(display: (f32, f32)) -> f32 {
        Self::panel_bounds(display).y + 298.0
    }

    fn agent_panel_bounds(display: (f32, f32)) -> Rect {
        let w = AGENT_PANEL_W.min((display.0 - 16.0).max(240.0));
        let h = AGENT_PANEL_H.min((display.1 - HUD_HEIGHT - 16.0).max(160.0));
        Rect {
            x: (display.0 - w - 8.0).max(8.0),
            y: HUD_HEIGHT + AGENT_PANEL_GAP,
            w,
            h,
        }
    }

    fn revealed_agent_panel_bounds(&self, display: (f32, f32)) -> Rect {
        let panel = Self::agent_panel_bounds(display);
        let progress = ease_out_cubic(self.agent_panel_reveal.clamp(0.0, 1.0));
        let w = 76.0_f32.min(panel.w) + (panel.w - 76.0_f32.min(panel.w)) * progress;
        let h = HUD_HEIGHT.min(panel.h) + (panel.h - HUD_HEIGHT.min(panel.h)) * progress;
        Rect {
            x: panel.x + panel.w - w,
            y: panel.y,
            w,
            h,
        }
    }

    fn agent_action_bounds(&self, display: (f32, f32)) -> Rect {
        let panel = self.revealed_agent_panel_bounds(display);
        let split = (panel.w * 0.43).clamp(98.0, 154.0);
        Rect {
            x: panel.x + split,
            y: panel.y + panel.h - 45.0,
            w: (panel.w - split - 15.0).max(1.0),
            h: 32.0,
        }
    }

    fn advance_agent_animation(&mut self, dt: f32) {
        let target = if self.agent_panel_open { 1.0 } else { 0.0 };
        if self.reduced_motion {
            self.agent_panel_reveal = target;
            return;
        }
        let dt = dt.clamp(0.0, 1.0 / 15.0);
        let follow = 1.0 - (-15.0 * dt).exp();
        self.agent_panel_reveal += (target - self.agent_panel_reveal) * follow;
        if (target - self.agent_panel_reveal).abs() < 0.002 {
            self.agent_panel_reveal = target;
        }
        if self.agent_panel_open || self.agent_panel_reveal > 0.01 {
            self.agent_visual_phase =
                (self.agent_visual_phase + dt).rem_euclid(std::f32::consts::TAU);
        }
    }

    fn dismiss_transient_ui(&mut self) {
        self.panel_open = false;
        self.agent_panel_open = false;
        self.agent_panel_reveal = 0.0;
        if let Some(key) = self.menu_open_for.clone() {
            self.close_menu(key);
        }
    }

    fn render_agent_panel(
        &self,
        frame: &mut Frame,
        display: (f32, f32),
        cursor: (f32, f32),
        indicator: &AgentWorkspaceIndicator,
        i18n: &Localizer,
    ) {
        let panel = self.revealed_agent_panel_bounds(display);
        let reveal = self.agent_panel_reveal.clamp(0.0, 1.0);
        let content = ((reveal - 0.24) / 0.76).clamp(0.0, 1.0);
        frame.layer(
            "aegis-hud-agent-workspaces-panel",
            panel,
            &OverlayOpts {
                bg: Color::rgba(15, 19, 34, fade_alpha(220, reveal)),
                border: Color::rgba(149, 184, 255, fade_alpha(112, reveal)),
                border_width: 1.0,
                radius: 22.0 * ease_out_cubic(reveal) + 10.0,
                ..Default::default()
            },
            |_| {},
        );
        if content <= 0.01 || panel.w < 150.0 || panel.h < 90.0 {
            return;
        }

        let original_theme = frame.theme();
        frame.set_theme(faded_theme(original_theme, content));

        let split = (panel.w * 0.43).clamp(98.0, 154.0);
        let visual = Rect {
            x: panel.x + 8.0,
            y: panel.y + 34.0,
            w: (split - 12.0).max(70.0),
            h: (panel.h - 45.0).max(70.0),
        };
        render_agent_workspace_visual(
            frame,
            visual,
            self.agent_visual_phase,
            content,
            matches!(
                indicator.state,
                AgentWorkspaceState::Active | AgentWorkspaceState::PartiallyPaused
            ),
            self.reduced_motion,
        );

        let copy_x = panel.x + split;
        let copy_w = (panel.w - split - 15.0).max(1.0);
        render_text_left(
            frame,
            "aegis-hud-agent-workspaces-title",
            Rect {
                x: copy_x,
                y: panel.y + 20.0,
                w: copy_w,
                h: 27.0,
            },
            i18n.text(Message::AiWorkspaces),
            18.0,
        );
        render_text_left(
            frame,
            "aegis-hud-agent-workspaces-subtitle",
            Rect {
                x: copy_x,
                y: panel.y + 52.0,
                w: copy_w,
                h: 22.0,
            },
            &truncate(
                i18n.text(Message::AgentWorkspacesPanelDescription),
                (copy_w / 6.2) as usize,
            ),
            11.0,
        );

        let state_label = agent_workspace_state_label(indicator.state, i18n);
        let state = Rect {
            x: copy_x,
            y: panel.y + 84.0,
            w: (frame.measure_text(state_label, 10.5).width + 22.0).min(copy_w),
            h: 25.0,
        };
        let state_accent = indicator_accent(indicator.state);
        frame.layer(
            "aegis-hud-agent-workspaces-state",
            state,
            &OverlayOpts {
                bg: state_accent.with_alpha(fade_alpha(34, content)),
                border: state_accent.with_alpha(fade_alpha(118, content)),
                border_width: 1.0,
                radius: state.h * 0.5,
                ..Default::default()
            },
            |_| {},
        );
        render_text(
            frame,
            "aegis-hud-agent-workspaces-state-label",
            state,
            state_label,
            10.5,
        );

        let action = self.agent_action_bounds(display);
        let hovered = contains(action, cursor.0, cursor.1);
        frame.layer(
            "aegis-hud-agent-workspaces-open",
            action,
            &OverlayOpts {
                bg: if hovered {
                    Color::rgba(102, 119, 255, fade_alpha(164, content))
                } else {
                    Color::rgba(81, 95, 218, fade_alpha(116, content))
                },
                border: Color::rgba(182, 202, 255, fade_alpha(126, content)),
                border_width: 1.0,
                radius: 11.0,
                ..Default::default()
            },
            |frame| {
                frame.row_ex(
                    &LayoutOpts {
                        width: action.w,
                        height: action.h,
                        gap: 6.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |frame| {
                        frame.label_compact_sized(
                            &truncate(
                                i18n.text(Message::OpenAgentWorkspaces),
                                ((action.w - 27.0) / 6.2).max(4.0) as usize,
                            ),
                            10.5,
                        );
                        frame.icon(Icon::ChevronRight, 13.0);
                    },
                );
            },
        );
        frame.set_theme(original_theme);
    }
    fn refresh_status(&mut self) {
        let now = Instant::now();
        if now.duration_since(self.last_clock_poll) >= CLOCK_POLL_INTERVAL {
            if let Some(clock) = local_clock() {
                self.clock = clock;
            }
            self.last_clock_poll = now;
        }
    }

    /// Clone the notification queue, memoized on the queue's revision: an
    /// unchanged queue reuses the cached `Arc` instead of re-cloning every
    /// entry every frame.
    fn notification_snapshot(&mut self) -> Arc<Vec<Notification>> {
        let Some(queue) = &self.notifications else {
            return Arc::new(Vec::new());
        };
        let queue = queue.lock().unwrap();
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
    /// `menu_revision` so the menu tree is re-cloned only when it changes and
    /// shared between all readers in a frame. A contended snapshot lock (the
    /// worker publishing a large tree) serves the cached menu rather than
    /// blocking the frame. Returns `None` when no menu is currently open or
    /// the bar has no SNI tray.
    fn menu_snapshot(&self) -> Option<Arc<MenuState>> {
        let tray = self.tray.as_ref()?;
        let mut cache = self.menu_cache.borrow_mut();
        if let Ok(snapshot) = tray.snapshot.try_lock() {
            let stale = cache
                .as_ref()
                .map(|cache| cache.revision != snapshot.menu_revision)
                .unwrap_or(true);
            if stale {
                *cache = Some(MenuSnapshotCache {
                    revision: snapshot.menu_revision,
                    menu: snapshot.menu.clone().map(Arc::new),
                });
            }
        }
        cache.as_ref()?.menu.clone()
    }

    /// Read the SNI snapshot under a brief lock, upload any new or changed
    /// icons into the texture cache, and return the visible cells for this
    /// frame. Runs on the render thread; never touches D-Bus. When the worker
    /// holds the snapshot lock (publishing what may be a large menu tree) the
    /// previous frame's cells are reused rather than blocking the frame.
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
                // the worker's theme resolution failed (memoized there, so
                // the theme is never rescanned on the render thread). Either
                // way the item must not keep rendering the previous texture.
                tray.textures.remove(&item.key);
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
        tray.cached_cells = cells.clone();
        cells
    }

    fn render_panel(
        &self,
        f: &mut Frame,
        display: (f32, f32),
        cursor: (f32, f32),
        notifications: &[Notification],
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let panel = Self::panel_bounds(display);
        f.layer("aegis-hud-panel", panel, &panel_opts(), |f| {
            f.column_ex(&sized(panel.w, panel.h), |_| {});
        });

        render_text_left(
            f,
            "aegis-hud-panel-title",
            Rect {
                x: panel.x + 18.0,
                y: panel.y + 12.0,
                w: panel.w - 70.0,
                h: 24.0,
            },
            i18n.text(Message::StatusAndControls),
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
            "aegis-hud-panel-close",
            close,
            self.themed_icon("window-close-symbolic"),
            Icon::X,
            "",
            contains(close, cursor.0, cursor.1),
        );

        let gap = 10.0;
        let card_w = (panel.w - 36.0 - gap) * 0.5;
        let card_y = panel.y + 50.0;
        let audio = Rect {
            x: panel.x + 13.0,
            y: card_y,
            w: card_w,
            h: 100.0,
        };
        let brightness = Rect {
            x: audio.x + card_w + gap,
            ..audio
        };
        let connectivity = Rect {
            y: card_y + audio.h + gap,
            h: 92.0,
            ..audio
        };
        let desktop = Rect {
            x: connectivity.x + card_w + gap,
            ..connectivity
        };

        let themed_volume_icon = self.themed_icon(volume_icon_name(&self.status));
        let mut volume = self.status.volume.unwrap_or(0) as f32;
        let mut muted = self.status.muted;
        f.layer("aegis-hud-audio-card", audio, &card_opts(false), |f| {
            f.column_ex(
                &LayoutOpts {
                    width: audio.w,
                    height: audio.h,
                    gap: 5.0,
                    pad: 10.0,
                    cross: Align::Stretch,
                    ..Default::default()
                },
                |f| {
                    f.row_ex(
                        &LayoutOpts {
                            height: 20.0,
                            gap: 7.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            match themed_volume_icon {
                                Some(icon) => unsafe {
                                    f.image(icon as *mut lens::sys::flux_image, 16.0, 16.0)
                                },
                                None => f.icon(volume_icon(&self.status), 15.0),
                            }
                            f.label_compact_sized(i18n.text(Message::Sound), 12.0);
                            f.flex(1.0);
                            f.spacer(0.0);
                            f.label_compact_sized(
                                &self
                                    .status
                                    .volume
                                    .map(|level| format!("{level}%"))
                                    .unwrap_or_else(|| "--".into()),
                                10.5,
                            );
                        },
                    );
                    if self.status.volume.is_some() {
                        if f.slider("##statusbar-volume", &mut volume, 0.0, 100.0) {
                            out.system_actions.push(SystemAction::SetVolume {
                                level: volume.round().clamp(0.0, 100.0) as u8,
                            });
                        }
                        if f.checkbox(i18n.text(Message::Muted), &mut muted) {
                            out.system_actions.push(SystemAction::ToggleMute);
                        }
                    } else {
                        f.label_compact_sized(i18n.text(Message::Unavailable), 10.5);
                    }
                },
            );
        });

        let mut brightness_level = self.status.brightness.unwrap_or(1) as f32;
        f.layer(
            "aegis-hud-brightness-card",
            brightness,
            &card_opts(false),
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: brightness.w,
                        height: brightness.h,
                        gap: 8.0,
                        pad: 10.0,
                        cross: Align::Stretch,
                        ..Default::default()
                    },
                    |f| {
                        f.row_ex(
                            &LayoutOpts {
                                height: 20.0,
                                gap: 7.0,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            |f| {
                                f.icon(Icon::Zap, 15.0);
                                f.label_compact_sized(i18n.text(Message::Brightness), 12.0);
                                f.flex(1.0);
                                f.spacer(0.0);
                                f.label_compact_sized(
                                    &self
                                        .status
                                        .brightness
                                        .map(|level| format!("{level}%"))
                                        .unwrap_or_else(|| "--".into()),
                                    10.5,
                                );
                            },
                        );
                        if self.status.brightness.is_some() {
                            if f.slider("##statusbar-brightness", &mut brightness_level, 1.0, 100.0)
                            {
                                out.system_actions.push(SystemAction::SetBrightness {
                                    level: brightness_level.round().clamp(1.0, 100.0) as u8,
                                });
                            }
                        } else {
                            f.label_compact_sized(i18n.text(Message::Unavailable), 10.5);
                        }
                    },
                );
            },
        );

        let network_text = match self.status.network {
            NetworkState::Wifi => i18n.text(Message::WifiConnected),
            NetworkState::Wired => i18n.text(Message::WiredConnected),
            NetworkState::Offline => i18n.text(Message::Disconnected),
        };
        let network_icon = self.themed_icon(network_icon_name(self.status.network));
        let mut wifi = self.status.wifi_enabled.unwrap_or(false);
        let mut bluetooth = self.status.bluetooth_enabled.unwrap_or(false);
        f.layer(
            "aegis-hud-connectivity-card",
            connectivity,
            &card_opts(false),
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: connectivity.w,
                        height: connectivity.h,
                        gap: 5.0,
                        pad: 10.0,
                        cross: Align::Stretch,
                        ..Default::default()
                    },
                    |f| {
                        f.row_ex(
                            &LayoutOpts {
                                height: 20.0,
                                gap: 7.0,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            |f| {
                                match network_icon {
                                    Some(icon) => unsafe {
                                        f.image(icon as *mut lens::sys::flux_image, 16.0, 16.0)
                                    },
                                    None => f.icon(Icon::Globe, 15.0),
                                }
                                f.label_compact_sized(i18n.text(Message::Connectivity), 12.0);
                                f.flex(1.0);
                                f.spacer(0.0);
                                f.label_compact_sized(network_text, 10.0);
                            },
                        );
                        if self.status.wifi_enabled.is_some() {
                            if f.checkbox(i18n.text(Message::Wifi), &mut wifi) {
                                out.system_actions
                                    .push(SystemAction::SetWifi { enabled: wifi });
                            }
                        } else {
                            unavailable_control(f, i18n.text(Message::Wifi), i18n);
                        }
                        if self.status.bluetooth_enabled.is_some() {
                            if f.checkbox(i18n.text(Message::Bluetooth), &mut bluetooth) {
                                out.system_actions
                                    .push(SystemAction::SetBluetooth { enabled: bluetooth });
                            }
                        } else {
                            unavailable_control(f, i18n.text(Message::Bluetooth), i18n);
                        }
                    },
                );
            },
        );

        let mut do_not_disturb = self.status.do_not_disturb;
        let mut tiled = self.status.tiled;
        f.layer("aegis-hud-desktop-card", desktop, &card_opts(false), |f| {
            f.column_ex(
                &LayoutOpts {
                    width: desktop.w,
                    height: desktop.h,
                    gap: 5.0,
                    pad: 10.0,
                    cross: Align::Stretch,
                    ..Default::default()
                },
                |f| {
                    f.row_ex(
                        &LayoutOpts {
                            height: 20.0,
                            gap: 7.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            f.icon(Icon::Grid, 15.0);
                            f.label_compact_sized(i18n.text(Message::Desktop), 12.0);
                        },
                    );
                    if f.checkbox(i18n.text(Message::DoNotDisturb), &mut do_not_disturb) {
                        out.system_actions.push(SystemAction::SetDoNotDisturb {
                            enabled: do_not_disturb,
                        });
                    }
                    if f.checkbox(i18n.text(Message::TiledLayout), &mut tiled) {
                        out.system_actions
                            .push(SystemAction::SetTiling { enabled: tiled });
                    }
                },
            );
        });

        let heading_y = panel.y + 264.0;
        render_text_left(
            f,
            "aegis-hud-notification-heading",
            Rect {
                x: panel.x + 18.0,
                y: heading_y,
                w: panel.w - 36.0,
                h: 20.0,
            },
            i18n.text(Message::RecentNotifications),
            12.0,
        );
        let list_y = Self::panel_notification_list_y(display);
        if notifications.is_empty() {
            render_text_left(
                f,
                "aegis-hud-notification-empty",
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
                let id = format!("aegis-hud-notification-{}", notification.id);
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
            "aegis-hud-sni-menu",
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
        _windows: &[Window],
        workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let raw = input.as_raw();
        if self.fullscreen_active {
            self.prev_down = raw.mouse_down.first().copied().unwrap_or(false);
            self.prev_right_down = raw.mouse_down.get(1).copied().unwrap_or(false);
            return;
        }

        self.refresh_status();
        self.advance_agent_animation(raw.dt_seconds.max(0.0));
        let display = (raw.display_size.x, raw.display_size.y);
        let cursor = (raw.cursor.x, raw.cursor.y);
        let down = raw.mouse_down.first().copied().unwrap_or(false);
        let pressed = down && !self.prev_down;
        // lens exposes the full button trio; the right button drives SNI
        // context actions (index 1 == LENS_MOUSE_RIGHT).
        let right_down = raw.mouse_down.get(1).copied().unwrap_or(false);
        let right_pressed = right_down && !self.prev_right_down;
        let notifications = self.notification_snapshot();
        let mut sni = self.sni_cells();
        // Only applications that explicitly registered a StatusNotifierItem
        // belong in the tray. Ordinary toplevels remain windows, never
        // synthetic tray entries.
        let fold = fold_tray(sni.len(), MAX_TRAY_ITEMS);
        sni.truncate(fold.visible);

        let bar = Self::bar_bounds(display.0);
        f.layer("aegis-hud-bar", bar, &bar_opts(), |f| {
            f.column_ex(&sized(bar.w, bar.h), |_| {});
        });

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
                    &format!("aegis-hud-workspace-dot-{}", workspace.id.0),
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

        render_text(
            f,
            "aegis-hud-clock",
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
            "aegis-hud-bell",
            bell,
            self.themed_icon("preferences-system-notifications-symbolic"),
            Icon::Bell,
            &notification_count,
            contains(bell, cursor.0, cursor.1),
        );
        let workspace_indicator = agent_workspace_indicator(&self.realms, i18n);
        let label_w = workspace_indicator.label.chars().count() as f32 * 6.8 + 32.0;
        let agent = take_right(&mut right_x, label_w.clamp(72.0, AGENT_INDICATOR_MAX_W));
        render_agent_workspace_indicator(
            f,
            agent,
            &workspace_indicator,
            contains(agent, cursor.0, cursor.1),
        );
        if let Some(battery) = self.status.battery {
            let rect = take_right(&mut right_x, 62.0);
            render_icon_button(
                f,
                "aegis-hud-battery",
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
            "aegis-hud-network",
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
            "aegis-hud-audio",
            audio,
            self.themed_icon(volume_icon_name(&self.status)),
            volume_icon(&self.status),
            &volume_label,
            contains(audio, cursor.0, cursor.1),
        );

        // StatusNotifierItem cells fill the tray row right-to-left.
        for sni_cell in sni.iter_mut().rev() {
            let rect = take_right(&mut right_x, TRAY_CELL_W);
            sni_cell.rect = rect;
            let hovered = contains(rect, cursor.0, cursor.1);
            let id = format!("aegis-hud-sni-{}", sni_cell.key);
            let texture = if sni_cell.textured {
                self.tray
                    .as_ref()
                    .and_then(|tray| tray.textures.get(&sni_cell.key))
                    .map(|(_, image)| image.as_raw())
            } else {
                None
            };
            f.layer(&id, rect, &icon_button_opts(hovered), |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: rect.w,
                        height: rect.h,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| match texture {
                        Some(texture) => unsafe {
                            f.image(texture as *mut lens::sys::flux_image, 18.0, 18.0)
                        },
                        None => match self.themed_icon("application-x-executable-symbolic") {
                            Some(icon) => unsafe {
                                f.image(icon as *mut lens::sys::flux_image, 18.0, 18.0)
                            },
                            None => f.icon(Icon::FileText, 16.0),
                        },
                    },
                );
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
                "aegis-hud-tray-overflow",
                rect,
                &format!("+{}", fold.hidden.min(99)),
                11.0,
            );
        }

        if pressed && contains(audio, cursor.0, cursor.1) {
            out.system_actions.push(SystemAction::ToggleMute);
        } else if pressed && contains(network, cursor.0, cursor.1) {
            self.panel_open = !self.panel_open;
            self.agent_panel_open = false;
        }
        let scroll_y = raw.scroll_y * 40.0 + raw.scroll_pixels_y;
        if contains(audio, cursor.0, cursor.1) && scroll_y.abs() > 0.01 {
            let amount = if scroll_y < 0.0 { 2 } else { -2 };
            out.system_actions
                .push(SystemAction::StepVolume { delta: amount });
        }
        if pressed && contains(bell, cursor.0, cursor.1) {
            self.panel_open = !self.panel_open;
            self.agent_panel_open = false;
        } else if pressed && contains(agent, cursor.0, cursor.1) {
            self.panel_open = false;
            self.agent_panel_open = !self.agent_panel_open;
        }

        if self.agent_panel_open && pressed {
            let action = self.agent_action_bounds(display);
            if self.agent_panel_reveal > 0.45 && contains(action, cursor.0, cursor.1) {
                self.agent_panel_open = false;
                out.open_builtin = Some(BuiltInApplication::AiWorkspaces);
            } else if !contains(Self::agent_panel_bounds(display), cursor.0, cursor.1)
                && !contains(agent, cursor.0, cursor.1)
            {
                self.agent_panel_open = false;
            }
        }
        if self.agent_panel_reveal > 0.001 {
            self.render_agent_panel(f, display, cursor, &workspace_indicator, i18n);
        }

        if self.panel_open {
            let panel = Self::panel_bounds(display);
            let close = Rect {
                x: panel.x + panel.w - 42.0,
                y: panel.y + 9.0,
                w: 30.0,
                h: 30.0,
            };
            if pressed && contains(close, cursor.0, cursor.1) {
                self.panel_open = false;
            } else if pressed {
                let list_y = Self::panel_notification_list_y(display);
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
            self.render_panel(f, display, cursor, &notifications, i18n, out);
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
        // Skip the snapshot read entirely while no popover is open.
        if self.menu_open_for.is_some()
            && let Some(menu) = self.menu_snapshot()
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
        if self.fullscreen_active {
            return false;
        }
        if contains(Self::bar_bounds(display.0), x, y) {
            return true;
        }
        if self.panel_open && contains(Self::panel_bounds(display), x, y) {
            return true;
        }
        if (self.agent_panel_open || self.agent_panel_reveal > 0.001)
            && contains(Self::agent_panel_bounds(display), x, y)
        {
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

    fn update_windows(&mut self, windows: &[Window]) {
        let fullscreen_active = SpaceUse::from_windows(windows) == SpaceUse::Fullscreen;
        if fullscreen_active && !self.fullscreen_active {
            self.dismiss_transient_ui();
        }
        self.fullscreen_active = fullscreen_active;
    }

    fn anim_pending(&self) -> bool {
        if self.fullscreen_active || self.reduced_motion {
            false
        } else {
            self.agent_panel_open
                || (self.agent_panel_reveal - if self.agent_panel_open { 1.0 } else { 0.0 }).abs()
                    > 0.002
        }
    }

    fn requires_composition(&self) -> bool {
        !self.fullscreen_active
    }

    fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
        if reduced {
            self.agent_panel_reveal = if self.agent_panel_open { 1.0 } else { 0.0 };
        }
    }

    fn reserved(&self) -> Reserved {
        if self.fullscreen_active {
            return Reserved::default();
        }
        Reserved {
            top: HUD_HEIGHT as i32,
            ..Reserved::default()
        }
    }

    fn backdrop_blur_sigma(&self) -> f32 {
        if self.fullscreen_active {
            0.0
        } else {
            BACKDROP_BLUR_SIGMA
        }
    }

    fn backdrop_regions(
        &self,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        if self.fullscreen_active {
            return Vec::new();
        }
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
        if self.agent_panel_open || self.agent_panel_reveal > 0.001 {
            let panel = self.revealed_agent_panel_bounds(display);
            let radius = 22.0_f32.min(panel.w * 0.5).min(panel.h * 0.5);
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

#[cfg(test)]
mod tests;
