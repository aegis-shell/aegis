//! The session HUD: a compact top bar with integrated workspace state, active
//! window context, clock, application tray, system status, and a small control
//! centre. Its information architecture follows the user's Quickshell HUD,
//! while the rendering and interaction remain compositor-owned lens chrome.

use std::collections::{HashMap, HashSet};
use std::ffi::c_void;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ass_core::app::{BuiltInApplication, Entry};
use ass_core::notify::{Notification, NotificationQueue};
use ass_core::window::Window;
use ass_core::workspace::WorkspaceSnapshot;
use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect};

use crate::{
    BackdropRegion, BatteryStatus, Chrome, ChromeEvents, CursorShape, DockApp, Localizer, Message,
    NetworkState, Reserved, SystemAction, SystemStatus,
};

pub(crate) const HUD_HEIGHT: f32 = 32.0;
const WORKSPACE_SLOT_W: f32 = 18.0;
const WORKSPACE_ACTIVE_DOT: f32 = 8.0;
const WORKSPACE_INACTIVE_DOT: f32 = 6.0;
const LEFT_MARGIN: f32 = 10.0;
const RIGHT_MARGIN: f32 = 6.0;
const TRAY_CELL_W: f32 = 26.0;
const MAX_TRAY_ITEMS: usize = 5;
const PANEL_GAP: f32 = 6.0;
const PANEL_W: f32 = 420.0;
const PANEL_H: f32 = 326.0;
const CLOCK_POLL_INTERVAL: Duration = Duration::from_secs(15);
const BACKDROP_BLUR_SIGMA: f32 = 12.0;

/// Full top-bar HUD. `WorkspaceBar` below remains as a compatibility alias for
/// callers that constructed the earlier workspace-only component.
pub struct HudBar {
    prev_down: bool,
    panel_open: bool,
    icons: HashMap<String, *mut c_void>,
    notifications: Option<Arc<Mutex<NotificationQueue>>>,
    status: SystemStatus,
    clock: String,
    last_clock_poll: Instant,
}

pub type WorkspaceBar = HudBar;

struct TrayCell {
    window: ass_core::window::WindowId,
    key: String,
    icon: Option<*mut c_void>,
}

impl HudBar {
    /// Construct a standalone HUD without notification data or raster icons.
    pub fn new() -> HudBar {
        HudBar::with_optional_sources(None, HashMap::new())
    }

    /// Construct the session HUD with the compositor's shared notification
    /// queue and application icon cache.
    pub fn with_notifications(
        notifications: Arc<Mutex<NotificationQueue>>,
        icons: HashMap<String, *mut c_void>,
    ) -> HudBar {
        HudBar::with_optional_sources(Some(notifications), icons)
    }

    fn with_optional_sources(
        notifications: Option<Arc<Mutex<NotificationQueue>>>,
        icons: HashMap<String, *mut c_void>,
    ) -> HudBar {
        let now = Instant::now();
        HudBar {
            prev_down: false,
            panel_open: false,
            icons,
            notifications,
            status: SystemStatus::default(),
            clock: "--:--".to_string(),
            last_clock_poll: now.checked_sub(CLOCK_POLL_INTERVAL).unwrap_or(now),
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

    fn themed_icon(&self, name: &str) -> Option<*mut c_void> {
        self.icons.get(&format!("ass-hud:{name}")).copied()
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
                    icon: self.icons.get(&app_id).copied(),
                    key: app_id,
                })
            })
            .take(MAX_TRAY_ITEMS)
            .collect()
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
}

impl Default for HudBar {
    fn default() -> Self {
        HudBar::new()
    }
}

impl Chrome for HudBar {
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
        let notifications = self.notification_snapshot();
        let tray = self.tray_cells(windows);

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
                    if let Some(icon) = self.icons.get(&key).copied() {
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
            out.open_builtin = Some(BuiltInApplication::ControlCenter);
        } else if pressed && contains(bell, cursor.0, cursor.1) {
            self.panel_open = !self.panel_open;
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

        self.prev_down = down;
    }

    fn captures_pointer(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        contains(Self::bar_bounds(display.0), x, y)
            || (self.panel_open && contains(Self::panel_bounds(display), x, y))
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

    fn update_app_catalog(
        &mut self,
        _apps: &[Entry],
        _dock_apps: &[DockApp],
        icons: &HashMap<String, *mut c_void>,
    ) {
        self.icons.clone_from(icons);
    }

    fn update_system_status(&mut self, status: &SystemStatus) {
        self.status = status.clone();
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
    // family for the HUD and use its visually identical 90% endpoint.
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
    fn hud_reserves_exactly_its_visual_height() {
        assert_eq!(HudBar::new().reserved().top, HUD_HEIGHT as i32);
    }

    #[test]
    fn panel_stays_inside_narrow_displays() {
        let panel = HudBar::panel_bounds((320.0, 480.0));
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
}
