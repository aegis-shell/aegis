//! The session HUD: display-only system status chips in the style of a
//! minimal FPS HUD (ADR-0080, ADR-0083).
//!
//! What used to be the interactive status bar is now two floating frosted
//! chips composited over the desktop: system status (network, Bluetooth,
//! battery), the StatusNotifierItem tray row, the clock, and the
//! notification count on the left; workspace dots in the center. The
//! top-right belongs to the frameless notification toast strip
//! (ADR-0083), and the Agent Workspaces status moved to the command panel
//! (`aegis-command-panel`). The chips reserve no space (tiled and
//! maximized windows run underneath), accept no pointer input (clicks fall
//! through to windows), and fade out when the cursor approaches. Every
//! interaction the bar once hosted moved to the command panel.

use std::collections::HashMap;
use std::ffi::c_void;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aegis_core::notify::{Notification, NotificationQueue};
use aegis_core::window::{SpaceUse, Window};
use aegis_core::workspace::WorkspaceSnapshot;
use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect, Theme};

use aegis_shell::{
    AppCatalog, BackdropRegion, BatteryStatus, Chrome, ChromeEvents, HUD_HEIGHT, IconSet,
    Localizer, NetworkState, SystemStatus,
};

use crate::tray::{TrayIcon, TraySnapshot};

mod rendering;

use rendering::*;

const WORKSPACE_SLOT_W: f32 = 18.0;
const WORKSPACE_ACTIVE_DOT: f32 = 8.0;
const WORKSPACE_INACTIVE_DOT: f32 = 6.0;
const CHIP_TOP: f32 = 8.0;
const CHIP_SIDE: f32 = 8.0;
const CHIP_PAD_X: f32 = 10.0;
const CHIP_RADIUS: f32 = 16.0;
const CHIP_HEIGHT: f32 = HUD_HEIGHT;
const CELL_ICON: f32 = 22.0;
const CELL_BATTERY: f32 = 52.0;
const CELL_CLOCK: f32 = 50.0;
const CELL_GAP: f32 = 2.0;
const TRAY_CELL_W: f32 = 24.0;
const MAX_TRAY_ITEMS: usize = 5;
const CLOCK_POLL_INTERVAL: Duration = Duration::from_secs(15);
const BACKDROP_BLUR_SIGMA: f32 = 12.0;
/// Cursor distance from a chip at which the chip fades out (ADR-0080).
const FADE_PROXIMITY: f32 = 56.0;
const FADE_RATE: f32 = 14.0;
/// Raster images cannot be alpha-faded by lens; they draw only while the
/// chip fade is above this floor (vector icons and text fade smoothly).
const IMAGE_FADE_FLOOR: f32 = 0.35;

/// Chip slots in the layout/fade arrays.
const LEFT: usize = 0;
const CENTER: usize = 1;

/// Per-frame chip geometry: the two chip rects and whether each exists at
/// all (the center chip vanishes when no output reports workspaces).
#[derive(Clone, Copy)]
struct ChipLayout {
    chips: [Rect; 2],
    visible: [bool; 2],
}

impl Default for ChipLayout {
    fn default() -> Self {
        const EMPTY: Rect = Rect {
            x: 0.0,
            y: 0.0,
            w: 0.0,
            h: 0.0,
        };
        ChipLayout {
            chips: [EMPTY; 2],
            visible: [false; 2],
        }
    }
}

/// The display-only HUD.
pub struct Hud {
    fullscreen_active: bool,
    reduced_motion: bool,
    icons: IconSet,
    notifications: Option<Arc<Mutex<NotificationQueue>>>,
    status: SystemStatus,
    clock: String,
    last_clock_poll: Instant,
    tray: Option<SniTray>,
    /// Per-chip (left/center) eased visibility: 1 = shown, 0 = hidden
    /// by the cursor-proximity fade.
    chip_fade: [f32; 2],
    /// The targets `chip_fade` is easing toward, refreshed every frame.
    chip_target: [f32; 2],
    /// Last frame's chip geometry, shared with `backdrop_regions` (the blur
    /// pass runs before the chrome render it feeds).
    layout: ChipLayout,
    /// Notification list cache keyed by the queue's revision; re-cloned only
    /// when the queue actually changes.
    notification_cache: Option<(u64, Arc<Vec<Notification>>)>,
}

/// Render-thread half of the StatusNotifierItem tray: the shared snapshot
/// the worker writes and the texture cache the HUD uploads from item
/// pixmaps. Read-only — tray interaction lives in the command panel.
struct SniTray {
    device: flux::Device,
    snapshot: Arc<Mutex<TraySnapshot>>,
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
    textured: bool,
}

impl Hud {
    /// Construct a standalone HUD without notification data, raster icons,
    /// or the SNI tray (used by tests and previews).
    pub fn new() -> Hud {
        Hud::with_optional_sources(None, None, None)
    }

    /// Construct the session HUD with the compositor's flux device, the
    /// shared tray snapshot (spawned once by the composition root and shared
    /// with the command panel), and the shared notification queue. The device is
    /// borrowed (non-owning, like [`aegis_shell::Shell::new`]) to upload SNI
    /// tray pixmaps to the GPU; the caller must keep it alive past the HUD.
    pub fn with_sources(
        device: &flux::Device,
        tray_snapshot: Option<Arc<Mutex<TraySnapshot>>>,
        notifications: Arc<Mutex<NotificationQueue>>,
    ) -> Hud {
        Hud::with_optional_sources(Some(device), tray_snapshot, Some(notifications))
    }

    fn with_optional_sources(
        device: Option<&flux::Device>,
        tray_snapshot: Option<Arc<Mutex<TraySnapshot>>>,
        notifications: Option<Arc<Mutex<NotificationQueue>>>,
    ) -> Hud {
        let tray = match (device, tray_snapshot) {
            (Some(device), Some(snapshot)) => {
                // SAFETY: the composition root declares its flux device
                // before the shell (and thus this HUD) and drops it after,
                // and the HUD only touches the device on the render thread.
                let device = unsafe { flux::Device::borrow_raw(device.as_raw()) };
                Some(SniTray {
                    device,
                    snapshot,
                    textures: HashMap::new(),
                    cached_cells: Vec::new(),
                })
            }
            _ => None,
        };
        let now = Instant::now();
        Hud {
            fullscreen_active: false,
            reduced_motion: false,
            icons: IconSet::default(),
            notifications,
            status: SystemStatus::default(),
            clock: "--:--".to_string(),
            last_clock_poll: now.checked_sub(CLOCK_POLL_INTERVAL).unwrap_or(now),
            tray,
            chip_fade: [1.0, 1.0],
            chip_target: [1.0, 1.0],
            layout: ChipLayout::default(),
            notification_cache: None,
        }
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

    fn themed_icon(&self, name: &str) -> Option<*mut c_void> {
        self.icons.get(&format!("aegis-hud:{name}"))
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
                // the worker's theme resolution failed (memoized there, so
                // the theme is never rescanned on the render thread). Either
                // way the item must not keep rendering the previous texture.
                tray.textures.remove(&item.key);
            }
            cells.push(SniCell {
                key: item.key.clone(),
                textured: tray.textures.contains_key(&item.key),
            });
        }
        tray.cached_cells = cells.clone();
        cells
    }

    /// Compute the two chip rects from the content each carries.
    fn chip_layout(
        &self,
        display: (f32, f32),
        workspaces: &WorkspaceSnapshot,
        notifications: usize,
        tray_visible: usize,
        tray_hidden: usize,
    ) -> ChipLayout {
        let mut layout = ChipLayout::default();
        let y = CHIP_TOP;

        // Left chip: system status (network, Bluetooth, battery), the tray
        // row, then the clock and the notification bell (ADR-0083).
        let mut width = CELL_ICON; // network is always present
        let mut cells = 1usize;
        if self.status.bluetooth_enabled.is_some() {
            width += CELL_ICON;
            cells += 1;
        }
        if self.status.battery.is_some() {
            width += CELL_BATTERY;
            cells += 1;
        }
        if tray_visible > 0 {
            width += TRAY_CELL_W * tray_visible as f32;
            cells += tray_visible;
        }
        if tray_hidden > 0 {
            width += TRAY_CELL_W;
            cells += 1;
        }
        let bell_w = if notifications == 0 { 34.0 } else { 50.0 };
        width += CELL_CLOCK + bell_w;
        cells += 2;
        let left_w = width + CHIP_PAD_X * 2.0 + CELL_GAP * cells.saturating_sub(1) as f32;
        layout.chips[LEFT] = Rect {
            x: CHIP_SIDE,
            y,
            w: left_w,
            h: CHIP_HEIGHT,
        };
        layout.visible[LEFT] = true;

        // Center chip: workspace dots.
        let slots = workspaces
            .outputs
            .first()
            .map(|output| output.workspaces.len())
            .unwrap_or(0);
        if slots > 0 {
            let w = slots as f32 * WORKSPACE_SLOT_W + CHIP_PAD_X * 2.0;
            layout.chips[CENTER] = Rect {
                x: (display.0 - w) * 0.5,
                y,
                w,
                h: CHIP_HEIGHT,
            };
            layout.visible[CENTER] = true;
        }

        layout
    }

    /// The fade target for one chip: hidden (0) while the cursor is inside
    /// the chip's proximity-inflated rect, shown (1) otherwise.
    fn fade_target(chip: Rect, cursor: (f32, f32)) -> f32 {
        let inflated = Rect {
            x: chip.x - FADE_PROXIMITY,
            y: chip.y - FADE_PROXIMITY,
            w: chip.w + FADE_PROXIMITY * 2.0,
            h: chip.h + FADE_PROXIMITY * 2.0,
        };
        if contains(inflated, cursor.0, cursor.1) {
            0.0
        } else {
            1.0
        }
    }

    fn advance_fade(&mut self, dt: f32, cursor: (f32, f32)) {
        let dt = dt.clamp(0.0, 1.0 / 15.0);
        let follow = 1.0 - (-FADE_RATE * dt).exp();
        for index in [LEFT, CENTER] {
            let target = if self.layout.visible[index] {
                Self::fade_target(self.layout.chips[index], cursor)
            } else {
                0.0
            };
            self.chip_target[index] = target;
            if self.reduced_motion || !self.layout.visible[index] {
                self.chip_fade[index] = target;
                continue;
            }
            self.chip_fade[index] += (target - self.chip_fade[index]) * follow;
            if (target - self.chip_fade[index]).abs() < 0.002 {
                self.chip_fade[index] = target;
            }
        }
    }
}

impl Default for Hud {
    fn default() -> Self {
        Hud::new()
    }
}

impl Chrome for Hud {
    fn render(
        &mut self,
        f: &mut Frame,
        input: &Input,
        _windows: &[Window],
        workspaces: &WorkspaceSnapshot,
        _i18n: &Localizer,
        _out: &mut ChromeEvents,
    ) {
        let raw = input.as_raw();
        if self.fullscreen_active {
            return;
        }

        self.refresh_status();
        let display = (raw.display_size.x, raw.display_size.y);
        let cursor = (raw.cursor.x, raw.cursor.y);
        let notifications = self.notification_snapshot();
        let sni = self.sni_cells();
        // Only applications that explicitly registered a StatusNotifierItem
        // belong in the tray. Ordinary toplevels remain windows, never
        // synthetic tray entries.
        let fold = fold_tray(sni.len(), MAX_TRAY_ITEMS);
        let sni = &sni[..fold.visible];
        self.layout = self.chip_layout(
            display,
            workspaces,
            notifications.len(),
            sni.len(),
            fold.hidden,
        );
        let layout = self.layout;
        self.advance_fade(raw.dt_seconds.max(0.0), cursor);

        let original_theme = f.theme();

        // ---- Left chip: system status + tray row -------------------------
        if layout.visible[LEFT] && self.chip_fade[LEFT] > 0.01 {
            let fade = self.chip_fade[LEFT];
            let chip = layout.chips[LEFT];
            f.layer("aegis-hud-chip-left", chip, &chip_opts(fade), |f| {
                f.column_ex(&sized(chip.w, chip.h), |_| {});
            });
            f.set_theme(faded_theme(original_theme, fade));
            let mut x = chip.x + CHIP_PAD_X;
            let mut cell = |width: f32| {
                let rect = Rect {
                    x,
                    y: chip.y,
                    w: width,
                    h: chip.h,
                };
                x += width + CELL_GAP;
                rect
            };

            let rect = cell(CELL_ICON);
            render_status_cell(
                f,
                "aegis-hud-network",
                rect,
                fade,
                self.themed_icon(network_icon_name(self.status.network)),
                Icon::Globe,
                "",
            );
            if let Some(enabled) = self.status.bluetooth_enabled {
                let rect = cell(CELL_ICON);
                // Themed-icon-only cell (lens has no Bluetooth vector glyph);
                // dimmed to a whisper while the radio is off.
                let bt_fade = fade * if enabled { 1.0 } else { 0.35 };
                if bt_fade > IMAGE_FADE_FLOOR
                    && let Some(icon) = self.themed_icon("bluetooth-symbolic")
                {
                    f.layer("aegis-hud-bluetooth", rect, &centered_layer(), |f| {
                        f.row_ex(
                            &LayoutOpts {
                                width: rect.w,
                                height: rect.h,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            |f| unsafe { f.image(icon as *mut lens::sys::flux_image, 16.0, 16.0) },
                        );
                    });
                }
            }
            if let Some(battery) = self.status.battery {
                let rect = cell(CELL_BATTERY);
                render_status_cell(
                    f,
                    "aegis-hud-battery",
                    rect,
                    fade,
                    self.themed_icon(&battery_icon_name(battery)),
                    Icon::Zap,
                    &format!("{}%", battery.percent),
                );
            }

            // StatusNotifierItem cells fill out the tray row, display-only.
            for sni_cell in sni.iter() {
                let rect = cell(TRAY_CELL_W);
                let texture = if sni_cell.textured {
                    self.tray
                        .as_ref()
                        .and_then(|tray| tray.textures.get(&sni_cell.key))
                        .map(|(_, image)| image.as_raw())
                } else {
                    None
                };
                let fallback = self.themed_icon("application-x-executable-symbolic");
                let id = format!("aegis-hud-sni-{}", sni_cell.key);
                f.layer(&id, rect, &centered_layer(), |f| {
                    f.row_ex(
                        &LayoutOpts {
                            width: rect.w,
                            height: rect.h,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            if fade > IMAGE_FADE_FLOOR {
                                match texture {
                                    Some(texture) => unsafe {
                                        f.image(texture as *mut lens::sys::flux_image, 18.0, 18.0)
                                    },
                                    None => match fallback {
                                        Some(icon) => unsafe {
                                            f.image(icon as *mut lens::sys::flux_image, 18.0, 18.0)
                                        },
                                        None => f.icon(Icon::FileText, 16.0),
                                    },
                                }
                            }
                        },
                    );
                });
            }
            // The overflow indicator counts folded items; a label, not a
            // button — the HUD accepts no clicks.
            if fold.hidden > 0 {
                let rect = cell(TRAY_CELL_W);
                render_text(
                    f,
                    "aegis-hud-tray-overflow",
                    rect,
                    &format!("+{}", fold.hidden.min(99)),
                    11.0,
                );
            }

            // Clock and notification bell close out the left chip (ADR-0083).
            let rect = cell(CELL_CLOCK);
            render_text(f, "aegis-hud-clock", rect, &self.clock, 13.5);

            let bell_w = if notifications.is_empty() { 34.0 } else { 50.0 };
            let rect = cell(bell_w);
            let count = if notifications.is_empty() {
                String::new()
            } else {
                notifications.len().min(99).to_string()
            };
            render_status_cell(
                f,
                "aegis-hud-bell",
                rect,
                fade,
                self.themed_icon("preferences-system-notifications-symbolic"),
                Icon::Bell,
                &count,
            );
        }

        // ---- Center chip: workspace dots ---------------------------------
        if layout.visible[CENTER] && self.chip_fade[CENTER] > 0.01 {
            let fade = self.chip_fade[CENTER];
            let chip = layout.chips[CENTER];
            f.layer("aegis-hud-chip-center", chip, &chip_opts(fade), |f| {
                f.column_ex(&sized(chip.w, chip.h), |_| {});
            });
            if let Some(output) = workspaces.outputs.first() {
                let mut x = chip.x + CHIP_PAD_X;
                for workspace in &output.workspaces {
                    let slot = Rect {
                        x,
                        y: chip.y,
                        w: WORKSPACE_SLOT_W,
                        h: chip.h,
                    };
                    x += WORKSPACE_SLOT_W;
                    let active = output.current == Some(workspace.id);
                    let diameter = workspace_dot_diameter(active);
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
                                    fade_color(workspace_dot_color(active), fade),
                                    diameter * 0.5,
                                ),
                                |_| {},
                            );
                        },
                    );
                }
            }
        }

        f.set_theme(original_theme);
    }

    fn update_app_catalog(&mut self, catalog: &AppCatalog) {
        self.icons = catalog.icons.clone();
    }

    fn update_system_status(&mut self, status: &SystemStatus) {
        self.status = status.clone();
    }

    fn update_windows(&mut self, windows: &[Window]) {
        // A fullscreen window owns the whole output; the HUD gets out of the
        // way entirely, as the bar always has.
        self.fullscreen_active = SpaceUse::from_windows(windows) == SpaceUse::Fullscreen;
    }

    fn anim_pending(&self) -> bool {
        if self.fullscreen_active {
            return false;
        }
        self.chip_fade
            .iter()
            .zip(self.chip_target.iter())
            .any(|(fade, target)| (fade - target).abs() > 0.002)
    }

    fn requires_composition(&self) -> bool {
        !self.fullscreen_active
    }

    fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
        if reduced {
            self.chip_fade = self.chip_target;
        }
    }

    fn backdrop_blur_sigma(&self) -> f32 {
        let any_visible = self
            .chip_fade
            .iter()
            .zip(self.layout.visible.iter())
            .any(|(fade, visible)| *visible && *fade > 0.01);
        if self.fullscreen_active || !any_visible {
            0.0
        } else {
            BACKDROP_BLUR_SIGMA
        }
    }

    fn backdrop_regions(
        &self,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        if self.fullscreen_active {
            return Vec::new();
        }
        self.layout
            .chips
            .iter()
            .zip(self.layout.visible.iter())
            .zip(self.chip_fade.iter())
            .filter(|((_, visible), fade)| **visible && **fade > 0.01)
            .map(|((chip, _), _)| BackdropRegion {
                x: chip.x,
                y: chip.y,
                w: chip.w,
                h: chip.h,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
