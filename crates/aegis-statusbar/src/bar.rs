//! The session status bar: a compact top bar with integrated workspace state,
//! active window context, clock, application tray, system status, and a small
//! status-and-controls panel. The rendering and interaction remain
//! compositor-owned lens chrome.

use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use aegis_core::app::BuiltInApplication;
use aegis_core::notify::{Notification, NotificationQueue};
use aegis_core::realm::{RealmKind, RealmSnapshot, RealmState};
use aegis_core::window::Window;
use aegis_core::workspace::WorkspaceSnapshot;
use aegis_design::{Design, materials, themes};
use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect, Theme};

use aegis_shell::{
    AppCatalog, BackdropRegion, BatteryStatus, Chrome, ChromeEvents, CursorShape, HUD_HEIGHT,
    IconSet, Localizer, Message, NetworkState, Reserved, SystemAction, SystemStatus,
};

use crate::tray::{self, MenuNode, MenuState, TrayCommand, TrayIcon, TraySnapshot};

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
const FUJI_PANEL_GAP: f32 = 7.0;
const FUJI_PANEL_W: f32 = 372.0;
const FUJI_PANEL_H: f32 = 224.0;

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
    fullscreen_active: bool,
    panel_open: bool,
    /// Whether the Fuji assistant surface is expanded from its permanent
    /// status-bar entry.
    fuji_open: bool,
    /// Eased reveal amount retained while the panel closes, so the surface
    /// contracts back into the entry instead of disappearing in one frame.
    fuji_reveal: f32,
    /// Continuous phase for the compositor-owned algorithm visualization.
    fuji_phase: f32,
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

struct TrayCell {
    window: aegis_core::window::WindowId,
    key: String,
    icon: Option<*mut c_void>,
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
            fuji_open: false,
            fuji_reveal: 0.0,
            fuji_phase: 0.0,
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

    fn fuji_panel_bounds(display: (f32, f32)) -> Rect {
        let w = FUJI_PANEL_W.min((display.0 - 16.0).max(240.0));
        let h = FUJI_PANEL_H.min((display.1 - HUD_HEIGHT - 16.0).max(160.0));
        Rect {
            x: (display.0 - w - 8.0).max(8.0),
            y: HUD_HEIGHT + FUJI_PANEL_GAP,
            w,
            h,
        }
    }

    fn revealed_fuji_panel_bounds(&self, display: (f32, f32)) -> Rect {
        let panel = Self::fuji_panel_bounds(display);
        let progress = ease_out_cubic(self.fuji_reveal.clamp(0.0, 1.0));
        let w = 76.0_f32.min(panel.w) + (panel.w - 76.0_f32.min(panel.w)) * progress;
        let h = HUD_HEIGHT.min(panel.h) + (panel.h - HUD_HEIGHT.min(panel.h)) * progress;
        Rect {
            x: panel.x + panel.w - w,
            y: panel.y,
            w,
            h,
        }
    }

    fn fuji_action_bounds(&self, display: (f32, f32)) -> Rect {
        let panel = self.revealed_fuji_panel_bounds(display);
        let split = (panel.w * 0.43).clamp(98.0, 154.0);
        Rect {
            x: panel.x + split,
            y: panel.y + panel.h - 45.0,
            w: (panel.w - split - 15.0).max(1.0),
            h: 32.0,
        }
    }

    fn advance_fuji_animation(&mut self, dt: f32) {
        let target = if self.fuji_open { 1.0 } else { 0.0 };
        if self.reduced_motion {
            self.fuji_reveal = target;
            return;
        }
        let dt = dt.clamp(0.0, 1.0 / 15.0);
        let follow = 1.0 - (-15.0 * dt).exp();
        self.fuji_reveal += (target - self.fuji_reveal) * follow;
        if (target - self.fuji_reveal).abs() < 0.002 {
            self.fuji_reveal = target;
        }
        if self.fuji_open || self.fuji_reveal > 0.01 {
            self.fuji_phase = (self.fuji_phase + dt).rem_euclid(std::f32::consts::TAU);
        }
    }

    fn dismiss_transient_ui(&mut self) {
        self.panel_open = false;
        self.fuji_open = false;
        self.fuji_reveal = 0.0;
        if let Some(key) = self.menu_open_for.clone() {
            self.close_menu(key);
        }
    }

    fn render_fuji_panel(
        &self,
        frame: &mut Frame,
        display: (f32, f32),
        cursor: (f32, f32),
        indicator: &AgentIndicator,
        i18n: &Localizer,
    ) {
        let panel = self.revealed_fuji_panel_bounds(display);
        let reveal = self.fuji_reveal.clamp(0.0, 1.0);
        let content = ((reveal - 0.24) / 0.76).clamp(0.0, 1.0);
        frame.layer(
            "ass-hud-fuji-panel",
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
        render_fuji_algorithm(
            frame,
            visual,
            self.fuji_phase,
            content,
            indicator.state == AgentIndicatorState::Active,
            self.reduced_motion,
        );

        let copy_x = panel.x + split;
        let copy_w = (panel.w - split - 15.0).max(1.0);
        render_text_left(
            frame,
            "ass-hud-fuji-title",
            Rect {
                x: copy_x,
                y: panel.y + 20.0,
                w: copy_w,
                h: 27.0,
            },
            i18n.text(Message::Fuji),
            18.0,
        );
        render_text_left(
            frame,
            "ass-hud-fuji-subtitle",
            Rect {
                x: copy_x,
                y: panel.y + 52.0,
                w: copy_w,
                h: 22.0,
            },
            &truncate(
                i18n.text(Message::FujiPanelDescription),
                (copy_w / 6.2) as usize,
            ),
            11.0,
        );

        let state_label = match indicator.state {
            AgentIndicatorState::Ready => i18n.text(Message::FujiReady),
            AgentIndicatorState::Active => i18n.text(Message::RealmActive),
            AgentIndicatorState::Paused => i18n.text(Message::RealmPaused),
        };
        let state = Rect {
            x: copy_x,
            y: panel.y + 84.0,
            w: (frame.measure_text(state_label, 10.5).width + 22.0).min(copy_w),
            h: 25.0,
        };
        let state_accent = indicator_accent(indicator.state);
        frame.layer(
            "ass-hud-fuji-state",
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
        render_text(frame, "ass-hud-fuji-state-label", state, state_label, 10.5);

        let action = self.fuji_action_bounds(display);
        let hovered = contains(action, cursor.0, cursor.1);
        frame.layer(
            "ass-hud-fuji-open",
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
                                i18n.text(Message::FujiOpenWorkspaces),
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
        self.icons.get(&format!("ass-hud:{name}"))
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
            "ass-hud-panel-close",
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
        f.layer("ass-hud-audio-card", audio, &card_opts(false), |f| {
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
            "ass-hud-brightness-card",
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
            "ass-hud-connectivity-card",
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
        f.layer("ass-hud-desktop-card", desktop, &card_opts(false), |f| {
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
        let list_y = Self::panel_notification_list_y(display);
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

/// Permanent Fuji entry plus the live Agent Realm state it summarizes.
struct AgentIndicator {
    label: String,
    state: AgentIndicatorState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AgentIndicatorState {
    Ready,
    Active,
    Paused,
}

fn agent_indicator(snapshot: &RealmSnapshot, i18n: &Localizer) -> AgentIndicator {
    let live = snapshot
        .realms
        .iter()
        .filter(|realm| realm.kind == RealmKind::Agent && realm.state != RealmState::Revoked)
        .collect::<Vec<_>>();
    let active = live.iter().any(|realm| realm.state == RealmState::Active);
    let state = match live.as_slice() {
        [] => AgentIndicatorState::Ready,
        _ if active => AgentIndicatorState::Active,
        _ => AgentIndicatorState::Paused,
    };
    let label = match live.as_slice() {
        [] => i18n.text(Message::Fuji).to_string(),
        [realm] => format!(
            "{} · {}",
            realm.label,
            if active {
                i18n.text(Message::RealmActive)
            } else {
                i18n.text(Message::RealmPaused)
            }
        ),
        realms => format!(
            "AI {} · {}",
            realms.len(),
            if active {
                i18n.text(Message::RealmActive)
            } else {
                i18n.text(Message::RealmPaused)
            }
        ),
    };
    AgentIndicator { label, state }
}

fn render_agent_indicator(
    frame: &mut Frame,
    rect: Rect,
    indicator: &AgentIndicator,
    hovered: bool,
) {
    let accent = indicator_accent(indicator.state);
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
        y: rect.y + (rect.h - 10.0) * 0.5,
        w: 10.0,
        h: 10.0,
    };
    frame.layer(
        "ass-hud-fuji-entry-orb",
        dot,
        &OverlayOpts {
            bg: accent.with_alpha(66),
            border: accent,
            border_width: 1.0,
            radius: dot.w * 0.5,
            ..Default::default()
        },
        |_| {},
    );
    render_text_left(
        frame,
        "ass-hud-fuji-entry-label",
        Rect {
            x: rect.x + 24.0,
            y: rect.y,
            w: (rect.w - 30.0).max(1.0),
            h: rect.h,
        },
        &truncate(&indicator.label, ((rect.w - 30.0) / 6.8).max(4.0) as usize),
        10.5,
    );
}

fn indicator_accent(state: AgentIndicatorState) -> Color {
    match state {
        AgentIndicatorState::Ready => Color::rgba(173, 119, 255, 255),
        AgentIndicatorState::Active => Color::rgba(82, 193, 255, 255),
        AgentIndicatorState::Paused => Color::rgba(240, 184, 84, 255),
    }
}

/// Draw a seek-safe, compositor-owned "algorithm core": layered Siri-like
/// colour fields, orbiting inference nodes, and a responsive signal strip.
/// It is presentation only; fuji's model and credentials remain out of
/// process behind the existing Agent/Realm boundary.
fn render_fuji_algorithm(
    frame: &mut Frame,
    rect: Rect,
    phase: f32,
    progress: f32,
    active: bool,
    reduced_motion: bool,
) {
    let phase = if reduced_motion { 0.82 } else { phase };
    let diameter = (rect.w * 0.74).min(rect.h * 0.62).clamp(42.0, 94.0);
    let center = (
        rect.x + rect.w * 0.5,
        rect.y + (rect.h * 0.48).min(rect.h - 27.0),
    );
    let energy = if active { 1.0 } else { 0.72 };
    let breathe = 1.0 + phase.sin() * 0.045 * energy;

    render_disc(
        frame,
        "ass-hud-fuji-glow",
        center,
        diameter * 1.48 * breathe,
        Color::rgba(78, 83, 255, fade_alpha(34, progress)),
    );
    render_ring(
        frame,
        "ass-hud-fuji-orbit-outer",
        center,
        diameter * 1.18,
        Color::rgba(101, 220, 255, fade_alpha(78, progress)),
        1.0,
    );
    render_ring(
        frame,
        "ass-hud-fuji-orbit-inner",
        center,
        diameter * 0.86,
        Color::rgba(215, 111, 255, fade_alpha(96, progress)),
        1.0,
    );

    let core_layers = [
        (
            diameter * 0.68,
            Color::rgba(72, 105, 255, fade_alpha(224, progress)),
            0.0_f32,
        ),
        (
            diameter * 0.54,
            Color::rgba(190, 77, 255, fade_alpha(206, progress)),
            2.1,
        ),
        (
            diameter * 0.38,
            Color::rgba(255, 91, 184, fade_alpha(194, progress)),
            4.2,
        ),
        (
            diameter * 0.22,
            Color::rgba(116, 241, 255, fade_alpha(238, progress)),
            5.3,
        ),
    ];
    for (index, (size, color, offset)) in core_layers.into_iter().enumerate() {
        let drift = diameter * 0.065 * energy;
        let layer_center = (
            center.0 + (phase * 1.25 + offset).cos() * drift,
            center.1 + (phase * 1.55 + offset).sin() * drift,
        );
        render_disc(
            frame,
            &format!("ass-hud-fuji-core-{index}"),
            layer_center,
            size * breathe,
            color,
        );
    }

    for index in 0..7 {
        let offset = index as f32 * std::f32::consts::TAU / 7.0;
        let angle = phase * (0.58 + index as f32 * 0.025) + offset;
        let radius = diameter * (0.49 + (index % 2) as f32 * 0.09);
        let node_center = (
            center.0 + angle.cos() * radius,
            center.1 + angle.sin() * radius * 0.58,
        );
        let node_size = 3.2 + (index % 3) as f32 * 1.25;
        let color = match index % 3 {
            0 => Color::rgba(91, 226, 255, fade_alpha(220, progress)),
            1 => Color::rgba(187, 112, 255, fade_alpha(212, progress)),
            _ => Color::rgba(255, 117, 198, fade_alpha(204, progress)),
        };
        render_disc(
            frame,
            &format!("ass-hud-fuji-node-{index}"),
            node_center,
            node_size,
            color,
        );
    }

    let bar_count = 9;
    let strip_w = (rect.w * 0.62).min(76.0);
    let bar_w = 3.0;
    let gap = (strip_w - bar_count as f32 * bar_w) / (bar_count - 1) as f32;
    let strip_x = center.0 - strip_w * 0.5;
    let baseline = rect.y + rect.h - 8.0;
    for index in 0..bar_count {
        let wave = ((phase * 2.6 + index as f32 * 0.72).sin() * 0.5 + 0.5) * energy;
        let height = 3.0 + wave * 12.0;
        let bar = Rect {
            x: strip_x + index as f32 * (bar_w + gap),
            y: baseline - height,
            w: bar_w,
            h: height,
        };
        frame.layer(
            &format!("ass-hud-fuji-signal-{index}"),
            bar,
            &OverlayOpts {
                bg: Color::rgba(
                    119 + (index as u8 * 9).min(80),
                    141,
                    255,
                    fade_alpha(196, progress),
                ),
                border: Color::TRANSPARENT,
                radius: bar_w * 0.5,
                ..Default::default()
            },
            |_| {},
        );
    }
}

fn render_disc(frame: &mut Frame, id: &str, center: (f32, f32), diameter: f32, color: Color) {
    let rect = Rect {
        x: center.0 - diameter * 0.5,
        y: center.1 - diameter * 0.5,
        w: diameter,
        h: diameter,
    };
    frame.layer(
        id,
        rect,
        &OverlayOpts {
            bg: color,
            border: Color::TRANSPARENT,
            radius: diameter * 0.5,
            ..Default::default()
        },
        |_| {},
    );
}

fn render_ring(
    frame: &mut Frame,
    id: &str,
    center: (f32, f32),
    diameter: f32,
    color: Color,
    width: f32,
) {
    let rect = Rect {
        x: center.0 - diameter * 0.5,
        y: center.1 - diameter * 0.5,
        w: diameter,
        h: diameter,
    };
    frame.layer(
        id,
        rect,
        &OverlayOpts {
            bg: Color::TRANSPARENT,
            border: color,
            border_width: width,
            radius: diameter * 0.5,
            ..Default::default()
        },
        |_| {},
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
        let raw = input.as_raw();
        if self.fullscreen_active {
            self.prev_down = raw.mouse_down.first().copied().unwrap_or(false);
            self.prev_right_down = raw.mouse_down.get(1).copied().unwrap_or(false);
            return;
        }

        self.refresh_status();
        self.advance_fuji_animation(raw.dt_seconds.max(0.0));
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
        let label_w = agent_indicator.label.chars().count() as f32 * 6.8 + 32.0;
        let agent = take_right(&mut right_x, label_w.clamp(72.0, AGENT_INDICATOR_MAX_W));
        render_agent_indicator(
            f,
            agent,
            &agent_indicator,
            contains(agent, cursor.0, cursor.1),
        );
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
                f.row_ex(
                    &LayoutOpts {
                        width: rect.w,
                        height: rect.h,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| match tray_cell.icon {
                        Some(icon) => unsafe {
                            f.image(icon as *mut lens::sys::flux_image, 18.0, 18.0)
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
                "ass-hud-tray-overflow",
                rect,
                &format!("+{}", fold.hidden.min(99)),
                11.0,
            );
        }

        if pressed && contains(audio, cursor.0, cursor.1) {
            out.system_actions.push(SystemAction::ToggleMute);
        } else if pressed && contains(network, cursor.0, cursor.1) {
            self.panel_open = !self.panel_open;
            self.fuji_open = false;
        }
        let scroll_y = raw.scroll_y * 40.0 + raw.scroll_pixels_y;
        if contains(audio, cursor.0, cursor.1) && scroll_y.abs() > 0.01 {
            let amount = if scroll_y < 0.0 { 2 } else { -2 };
            out.system_actions
                .push(SystemAction::StepVolume { delta: amount });
        }
        if pressed && contains(bell, cursor.0, cursor.1) {
            self.panel_open = !self.panel_open;
            self.fuji_open = false;
        } else if pressed && contains(agent, cursor.0, cursor.1) {
            self.panel_open = false;
            self.fuji_open = !self.fuji_open;
        }

        if self.fuji_open && pressed {
            let action = self.fuji_action_bounds(display);
            if self.fuji_reveal > 0.45 && contains(action, cursor.0, cursor.1) {
                self.fuji_open = false;
                out.open_builtin = Some(BuiltInApplication::AiWorkspaces);
            } else if !contains(Self::fuji_panel_bounds(display), cursor.0, cursor.1)
                && !contains(agent, cursor.0, cursor.1)
            {
                self.fuji_open = false;
            }
        }
        if self.fuji_reveal > 0.001 {
            self.render_fuji_panel(f, display, cursor, &agent_indicator, i18n);
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
        if (self.fuji_open || self.fuji_reveal > 0.001)
            && contains(Self::fuji_panel_bounds(display), x, y)
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
        let fullscreen_active = windows
            .iter()
            .any(|window| !window.minimized && window.state.fullscreen);
        if fullscreen_active && !self.fullscreen_active {
            self.dismiss_transient_ui();
        }
        self.fullscreen_active = fullscreen_active;
    }

    fn anim_pending(&self) -> bool {
        if self.fullscreen_active || self.reduced_motion {
            false
        } else {
            self.fuji_open
                || (self.fuji_reveal - if self.fuji_open { 1.0 } else { 0.0 }).abs() > 0.002
        }
    }

    fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
        if reduced {
            self.fuji_reveal = if self.fuji_open { 1.0 } else { 0.0 };
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
        if self.fuji_open || self.fuji_reveal > 0.001 {
            let panel = self.revealed_fuji_panel_bounds(display);
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

/// The current local time as `HH:MM` — the same string `date +%H:%M`
/// produced, but resolved in-process so the render thread never forks.
/// `localtime_r` is the thread-safe local-time breakdown.
fn local_clock() -> Option<String> {
    // SAFETY: `time` writes a valid `time_t` into `now`, and `localtime_r`
    // either writes a valid `tm` into `broken` and returns a pointer to it or
    // returns null.
    unsafe {
        let mut now: libc::time_t = 0;
        libc::time(&mut now);
        let mut broken: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut broken).is_null() {
            return None;
        }
        Some(format!("{:02}:{:02}", broken.tm_hour, broken.tm_min))
    }
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
/// `aegis_shell::chrome::app_menu::place_popup` verbatim.
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
        y: 0.0,
        w: width,
        h: HUD_HEIGHT,
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

fn render_text(f: &mut Frame, id: &str, rect: Rect, text: &str, size: f32) {
    f.layer(id, rect, &centered_layer(), |f| {
        f.row_ex(
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
        f.row_ex(
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

fn unavailable_control(f: &mut Frame, label: &str, i18n: &Localizer) {
    f.row_ex(
        &LayoutOpts {
            height: 20.0,
            gap: 6.0,
            cross: Align::Center,
            ..Default::default()
        },
        |f| {
            f.label_compact_sized(label, 10.5);
            f.flex(1.0);
            f.spacer(0.0);
            f.label_compact_sized(i18n.text(Message::Unavailable), 10.0);
        },
    );
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

fn ease_out_cubic(value: f32) -> f32 {
    let inverse = 1.0 - value.clamp(0.0, 1.0);
    1.0 - inverse * inverse * inverse
}

fn fade_alpha(base: u8, progress: f32) -> u8 {
    (base as f32 * progress.clamp(0.0, 1.0)).round() as u8
}

fn faded_theme(theme: Theme, progress: f32) -> Theme {
    let fade = |color: Color| {
        let (_, _, _, opacity) = color.components();
        color.with_alpha(fade_alpha(opacity, progress))
    };
    theme
        .with_fg(fade(theme.fg()))
        .with_accent(fade(theme.accent()))
        .with_border(fade(theme.border()))
        .with_hover(fade(theme.hover()))
        .with_active(fade(theme.active()))
        .with_disabled(fade(theme.disabled()))
        .with_error(fade(theme.error()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_bar_reserves_exactly_its_visual_height() {
        assert_eq!(StatusBar::new().reserved().top, HUD_HEIGHT as i32);
    }

    #[test]
    fn fullscreen_window_hides_status_bar_and_releases_its_surface_policy() {
        let mut bar = StatusBar::new();
        bar.panel_open = true;
        bar.fuji_open = true;
        bar.fuji_reveal = 1.0;
        bar.menu_open_for = Some("org.example.Tray".to_string());

        let mut fullscreen = Window::new(aegis_core::window::WindowId(7));
        fullscreen.state.fullscreen = true;
        bar.update_windows(&[fullscreen]);

        let workspaces = WorkspaceSnapshot {
            outputs: Vec::new(),
        };
        assert!(bar.fullscreen_active);
        assert!(!bar.panel_open);
        assert!(!bar.fuji_open);
        assert_eq!(bar.fuji_reveal, 0.0);
        assert!(bar.menu_open_for.is_none());
        assert_eq!(bar.reserved(), Reserved::default());
        assert_eq!(bar.backdrop_blur_sigma(), 0.0);
        assert!(
            bar.backdrop_regions((1920.0, 1080.0), &[], &workspaces)
                .is_empty()
        );
        assert!(!bar.captures_pointer(10.0, 10.0, (1920.0, 1080.0), &[], &workspaces,));
        assert!(!bar.anim_pending());

        bar.update_windows(&[]);
        assert!(!bar.fullscreen_active);
        assert_eq!(bar.reserved().top, HUD_HEIGHT as i32);
        assert_eq!(bar.backdrop_blur_sigma(), BACKDROP_BLUR_SIGMA);
        assert!(bar.captures_pointer(10.0, 10.0, (1920.0, 1080.0), &[], &workspaces,));
    }

    #[test]
    fn maximized_and_minimized_fullscreen_windows_keep_status_bar_visible() {
        let mut bar = StatusBar::new();
        let mut maximized = Window::new(aegis_core::window::WindowId(7));
        maximized.state.maximized = true;
        bar.update_windows(&[maximized]);
        assert!(!bar.fullscreen_active);

        let mut minimized_fullscreen = Window::new(aegis_core::window::WindowId(8));
        minimized_fullscreen.state.fullscreen = true;
        minimized_fullscreen.minimized = true;
        bar.update_windows(&[minimized_fullscreen]);
        assert!(!bar.fullscreen_active);
        assert_eq!(bar.reserved().top, HUD_HEIGHT as i32);
        assert_eq!(bar.backdrop_blur_sigma(), BACKDROP_BLUR_SIGMA);
    }

    #[test]
    fn fuji_entry_is_permanent_and_tracks_live_realm_state() {
        let i18n = Localizer::new("en-US");
        let mut model = aegis_core::realm::RealmModel::new();
        let indicator = agent_indicator(&model.snapshot(), &i18n);
        assert_eq!(indicator.state, AgentIndicatorState::Ready);
        assert_eq!(indicator.label, "Fuji");

        let bundle = model.create_agent_realm("Fuji", Default::default());
        let mut snapshot = model.snapshot();
        let indicator = agent_indicator(&snapshot, &i18n);
        assert_eq!(indicator.state, AgentIndicatorState::Active);
        assert_eq!(indicator.label, "Fuji · Active");

        snapshot
            .realms
            .iter_mut()
            .find(|realm| realm.id == bundle.realm)
            .expect("agent Realm")
            .state = RealmState::Paused;
        let indicator = agent_indicator(&snapshot, &i18n);
        assert_eq!(indicator.state, AgentIndicatorState::Paused);
        assert_eq!(indicator.label, "Fuji · Paused");

        snapshot
            .realms
            .iter_mut()
            .find(|realm| realm.id == bundle.realm)
            .expect("agent Realm")
            .state = RealmState::Revoked;
        let indicator = agent_indicator(&snapshot, &i18n);
        assert_eq!(indicator.state, AgentIndicatorState::Ready);
        assert_eq!(indicator.label, "Fuji");
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
    fn fuji_panel_stays_inside_narrow_displays_and_expands_from_the_right() {
        let final_panel = StatusBar::fuji_panel_bounds((320.0, 480.0));
        assert!(final_panel.x >= 0.0);
        assert!(final_panel.x + final_panel.w <= 320.0);
        assert!(final_panel.y + final_panel.h <= 480.0);

        let mut bar = StatusBar::new();
        let collapsed = bar.revealed_fuji_panel_bounds((320.0, 480.0));
        bar.fuji_reveal = 1.0;
        let expanded = bar.revealed_fuji_panel_bounds((320.0, 480.0));
        assert!(expanded.w > collapsed.w);
        assert_eq!(expanded.x + expanded.w, collapsed.x + collapsed.w);
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
}
