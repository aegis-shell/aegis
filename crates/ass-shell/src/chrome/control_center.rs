//! The compositor-owned Control Center application.
//!
//! It is launched through the same application catalog as desktop entries,
//! but remains a trusted in-process surface rendered with optics/lens. The UI
//! emits typed intents; it never invokes host commands itself.

use std::collections::HashMap;
use std::ffi::c_void;

use ass_core::app::{BuiltInApplication, Entry};
use ass_core::input::{key_action, KeyAction, KeyChar, TouchpadScrollMethod};
use ass_core::window::Window;
use ass_core::workspace::WorkspaceSnapshot;
use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect, Theme};

use crate::{
    BackdropRegion, Chrome, ChromeEvents, CursorShape, DockApp, Localizer, Message, Reserved,
    SystemAction, SystemStatus,
};

const APP_MAX_W: f32 = 860.0;
const APP_MAX_H: f32 = 590.0;
const APP_MARGIN: f32 = 24.0;
const APP_RADIUS: f32 = 24.0;
const BACKDROP_BLUR_SIGMA: f32 = 18.0;

/// A trusted built-in application with a stable launcher identity.
pub struct ControlCenter {
    open: bool,
    status: SystemStatus,
    icons: HashMap<String, *mut c_void>,
    modal_reserved: Reserved,
    volume: f32,
    brightness: f32,
    page: i32,
}

impl ControlCenter {
    pub fn new() -> ControlCenter {
        ControlCenter::with_icons(HashMap::new())
    }

    pub fn with_icons(icons: HashMap<String, *mut c_void>) -> ControlCenter {
        ControlCenter {
            open: false,
            status: SystemStatus::default(),
            icons,
            modal_reserved: Reserved::default(),
            volume: 0.0,
            brightness: 0.0,
            page: 0,
        }
    }

    fn bounds(&self, display: (f32, f32)) -> Rect {
        let left = self.modal_reserved.left.max(0) as f32;
        let top = self.modal_reserved.top.max(0) as f32;
        let right = self.modal_reserved.right.max(0) as f32;
        let bottom = self.modal_reserved.bottom.max(0) as f32;
        let usable_w = (display.0 - left - right).max(1.0);
        let usable_h = (display.1 - top - bottom).max(1.0);
        let w = APP_MAX_W.min((usable_w - APP_MARGIN * 2.0).max(240.0));
        let h = APP_MAX_H.min((usable_h - APP_MARGIN * 2.0).max(300.0));
        Rect {
            x: left + ((usable_w - w) * 0.5).max(0.0),
            y: top + ((usable_h - h) * 0.5).max(0.0),
            w: w.min(usable_w),
            h: h.min(usable_h),
        }
    }

    fn app_icon(&self) -> Option<*mut c_void> {
        self.icons
            .get("ass-control-center")
            .or_else(|| self.icons.get("ass-hud:preferences-system-symbolic"))
            .copied()
    }

    fn render_header(&mut self, frame: &mut Frame, i18n: &Localizer) -> bool {
        let mut close = false;
        frame.row_ex(
            &LayoutOpts {
                height: 48.0,
                gap: 12.0,
                cross: Align::Center,
                ..Default::default()
            },
            |frame| {
                frame.size_next(36.0, 36.0);
                match self.app_icon() {
                    Some(icon) => unsafe {
                        frame.image(icon as *mut lens::sys::flux_image, 32.0, 32.0)
                    },
                    None => frame.icon(Icon::Settings, 28.0),
                }
                frame.column_ex(
                    &LayoutOpts {
                        gap: 1.0,
                        cross: Align::Start,
                        ..Default::default()
                    },
                    |frame| {
                        frame.heading(i18n.text(Message::ControlCenter), 2);
                        frame.label_sized(i18n.text(Message::BuiltInSystemApp), 11.0);
                    },
                );
                frame.flex(1.0);
                frame.spacer(0.0);
                frame.size_next(34.0, 30.0);
                close = frame.icon_button(Icon::X);
            },
        );
        close
    }

    fn render_connectivity(&mut self, frame: &mut Frame, i18n: &Localizer, out: &mut ChromeEvents) {
        frame.column_ex(&card_layout(), |frame| {
            frame.row_ex(&section_heading_layout(), |frame| {
                frame.icon(Icon::Radio, 17.0);
                frame.heading(i18n.text(Message::Connectivity), 3);
            });

            if let Some(mut enabled) = self.status.wifi_enabled {
                if frame.checkbox(i18n.text(Message::Wifi), &mut enabled) {
                    self.status.wifi_enabled = Some(enabled);
                    out.system_actions.push(SystemAction::SetWifi(enabled));
                }
            } else {
                unavailable_row(frame, i18n.text(Message::Wifi), i18n);
            }

            if let Some(mut enabled) = self.status.bluetooth_enabled {
                if frame.checkbox(i18n.text(Message::Bluetooth), &mut enabled) {
                    self.status.bluetooth_enabled = Some(enabled);
                    out.system_actions.push(SystemAction::SetBluetooth(enabled));
                }
            } else {
                unavailable_row(frame, i18n.text(Message::Bluetooth), i18n);
            }

            let mut dnd = self.status.do_not_disturb;
            if frame.checkbox(i18n.text(Message::DoNotDisturb), &mut dnd) {
                self.status.do_not_disturb = dnd;
                out.system_actions.push(SystemAction::SetDoNotDisturb(dnd));
            }
        });
    }

    fn render_desktop(&mut self, frame: &mut Frame, i18n: &Localizer, out: &mut ChromeEvents) {
        frame.column_ex(&card_layout(), |frame| {
            frame.row_ex(&section_heading_layout(), |frame| {
                frame.icon(Icon::Grid, 17.0);
                frame.heading(i18n.text(Message::Desktop), 3);
            });
            let mut tiled = self.status.tiled;
            if frame.checkbox(i18n.text(Message::TiledLayout), &mut tiled) {
                self.status.tiled = tiled;
                out.system_actions.push(SystemAction::SetTiling(tiled));
            }
            frame.size_next(0.0, 30.0);
            if frame.button(i18n.text(Message::OpenApplications)) {
                self.open = false;
                out.toggle_launcher = true;
            }
            frame.size_next(0.0, 30.0);
            if frame.button(i18n.text(Message::QuitSession)) {
                out.quit = true;
            }
        });
    }

    fn render_sound(&mut self, frame: &mut Frame, i18n: &Localizer, out: &mut ChromeEvents) {
        frame.column_ex(&wide_card_layout(), |frame| {
            frame.row_ex(&section_heading_layout(), |frame| {
                frame.icon(volume_icon(&self.status), 17.0);
                frame.heading(i18n.text(Message::Sound), 3);
                frame.flex(1.0);
                frame.spacer(0.0);
                frame.label_sized(
                    &self
                        .status
                        .volume
                        .map(|level| format!("{level}%"))
                        .unwrap_or_else(|| "--".into()),
                    12.0,
                );
            });
            if self.status.volume.is_some() {
                if frame.slider("##control-center-volume", &mut self.volume, 0.0, 100.0) {
                    let level = self.volume.round().clamp(0.0, 100.0) as u8;
                    self.status.volume = Some(level);
                    out.system_actions.push(SystemAction::SetVolume(level));
                }
                let mut muted = self.status.muted;
                if frame.checkbox(i18n.text(Message::Muted), &mut muted) {
                    self.status.muted = muted;
                    out.system_actions.push(SystemAction::ToggleMute);
                }
            } else {
                unavailable_row(frame, i18n.text(Message::Volume), i18n);
            }
        });
    }

    fn render_brightness(&mut self, frame: &mut Frame, i18n: &Localizer, out: &mut ChromeEvents) {
        frame.column_ex(&wide_card_layout(), |frame| {
            frame.row_ex(&section_heading_layout(), |frame| {
                frame.icon(Icon::Zap, 17.0);
                frame.heading(i18n.text(Message::Brightness), 3);
                frame.flex(1.0);
                frame.spacer(0.0);
                frame.label_sized(
                    &self
                        .status
                        .brightness
                        .map(|level| format!("{level}%"))
                        .unwrap_or_else(|| "--".into()),
                    12.0,
                );
            });
            if self.status.brightness.is_some() {
                if frame.slider(
                    "##control-center-brightness",
                    &mut self.brightness,
                    1.0,
                    100.0,
                ) {
                    let level = self.brightness.round().clamp(1.0, 100.0) as u8;
                    self.status.brightness = Some(level);
                    out.system_actions.push(SystemAction::SetBrightness(level));
                }
            } else {
                unavailable_row(frame, i18n.text(Message::Brightness), i18n);
            }
        });
    }

    fn render_touchpad(&mut self, frame: &mut Frame, i18n: &Localizer, out: &mut ChromeEvents) {
        let status = self.status.touchpad.clone();
        let mut config = status.config;
        let mut changed = false;
        let has_devices = status.device_count() > 0;
        let can_edit = |capability: bool| !status.configurable || !has_devices || capability;

        frame.column_ex(
            &LayoutOpts {
                gap: 5.0,
                cross: Align::Stretch,
                ..Default::default()
            },
            |frame| {
                frame.heading(i18n.text(Message::Touchpad), 2);
                frame.label_sized(i18n.text(Message::TouchpadDescription), 12.0);
                if !status.configurable {
                    frame.label_wrapped_sized(i18n.text(Message::TouchpadHostManaged), 11.0, 560.0);
                } else if !has_devices {
                    frame.label_wrapped_sized(i18n.text(Message::NoTouchpadDetected), 11.0, 560.0);
                } else {
                    frame.label_sized(&status.device_names.join(" · "), 11.0);
                }
            },
        );

        frame.column_ex(&settings_card_layout(), |frame| {
            frame.heading(i18n.text(Message::PointingAndClicking), 3);
            changed |= frame
                .setting_switch(
                    "touchpad-tap-to-click",
                    i18n.text(Message::TapToClick),
                    i18n.text(Message::TapToClickDescription),
                    &mut config.tap_to_click,
                    !can_edit(status.capabilities.tap_to_click),
                )
                .changed;
            changed |= frame
                .setting_switch(
                    "touchpad-tap-and-drag",
                    i18n.text(Message::TapAndDrag),
                    i18n.text(Message::TapAndDragDescription),
                    &mut config.tap_and_drag,
                    !config.tap_to_click || !can_edit(status.capabilities.tap_and_drag),
                )
                .changed;
            changed |= frame
                .setting_switch(
                    "touchpad-drag-lock",
                    i18n.text(Message::DragLock),
                    i18n.text(Message::DragLockDescription),
                    &mut config.drag_lock,
                    !config.tap_to_click
                        || !config.tap_and_drag
                        || !can_edit(status.capabilities.drag_lock),
                )
                .changed;
            changed |= frame
                .setting_switch(
                    "touchpad-disable-while-typing",
                    i18n.text(Message::DisableWhileTyping),
                    i18n.text(Message::DisableWhileTypingDescription),
                    &mut config.disable_while_typing,
                    !can_edit(status.capabilities.disable_while_typing),
                )
                .changed;

            frame.separator();
            frame.row_ex(&section_heading_layout(), |frame| {
                frame.label_sized(i18n.text(Message::PointerSpeed), 12.0);
                frame.flex(1.0);
                frame.spacer(0.0);
                frame.label_sized(&format!("{:+.0}%", config.pointer_speed * 100.0), 11.0);
            });
            if can_edit(status.capabilities.pointer_speed) {
                changed |= frame.slider(
                    "##touchpad-pointer-speed",
                    &mut config.pointer_speed,
                    -1.0,
                    1.0,
                );
                frame.row_ex(
                    &LayoutOpts {
                        height: 18.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |frame| {
                        frame.label_sized(i18n.text(Message::Slow), 10.0);
                        frame.flex(1.0);
                        frame.spacer(0.0);
                        frame.label_sized(i18n.text(Message::Fast), 10.0);
                    },
                );
            } else {
                unavailable_row(frame, i18n.text(Message::PointerSpeed), i18n);
            }
        });

        frame.column_ex(&settings_card_layout(), |frame| {
            frame.heading(i18n.text(Message::Scrolling), 3);
            changed |= frame
                .setting_switch(
                    "touchpad-natural-scroll",
                    i18n.text(Message::NaturalScroll),
                    i18n.text(Message::NaturalScrollDescription),
                    &mut config.natural_scroll,
                    !can_edit(status.capabilities.natural_scroll),
                )
                .changed;

            let can_two_finger = can_edit(status.capabilities.two_finger_scroll);
            let can_edge = can_edit(status.capabilities.edge_scroll);
            frame.row_ex(
                &LayoutOpts {
                    min_height: 34.0,
                    gap: 12.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |frame| {
                    frame.label_sized(i18n.text(Message::ScrollMethod), 12.0);
                    frame.flex(1.0);
                    frame.spacer(0.0);
                    if can_two_finger && can_edge {
                        frame.size_next(190.0, 30.0);
                        let mut selected = match config.scroll_method {
                            TouchpadScrollMethod::TwoFinger => 0,
                            TouchpadScrollMethod::Edge => 1,
                        };
                        if frame.dropdown(
                            "##touchpad-scroll-method",
                            &mut selected,
                            &[
                                i18n.text(Message::TwoFingerScroll),
                                i18n.text(Message::EdgeScroll),
                            ],
                        ) {
                            config.scroll_method = if selected == 0 {
                                TouchpadScrollMethod::TwoFinger
                            } else {
                                TouchpadScrollMethod::Edge
                            };
                            changed = true;
                        }
                    } else if can_two_finger {
                        frame.label_sized(i18n.text(Message::TwoFingerScroll), 11.0);
                    } else if can_edge {
                        frame.label_sized(i18n.text(Message::EdgeScroll), 11.0);
                    } else {
                        frame.label_sized(i18n.text(Message::Unavailable), 11.0);
                    }
                },
            );
        });

        if changed {
            config.pointer_speed = config.pointer_speed.clamp(-1.0, 1.0);
            self.status.touchpad.config = config;
            out.system_actions.push(SystemAction::SetTouchpad(config));
        }
    }

    fn render_navigation(&mut self, frame: &mut Frame, i18n: &Localizer) {
        for (page, icon, label) in [
            (0, Icon::Grid, i18n.text(Message::Desktop)),
            (1, Icon::MousePointer, i18n.text(Message::Touchpad)),
            (2, Icon::Radio, i18n.text(Message::Connectivity)),
            (3, Icon::Sliders, i18n.text(Message::SoundAndDisplay)),
        ] {
            if frame.selectable_icon(icon, label, self.page == page) {
                self.page = page;
            }
        }
    }

    fn render_compact_navigation(&mut self, frame: &mut Frame, i18n: &Localizer) {
        for entries in [
            [
                (0, i18n.text(Message::Desktop)),
                (1, i18n.text(Message::Touchpad)),
            ],
            [
                (2, i18n.text(Message::Connectivity)),
                (3, i18n.text(Message::Sound)),
            ],
        ] {
            frame.row_ex(
                &LayoutOpts {
                    gap: 4.0,
                    cross: Align::Stretch,
                    ..Default::default()
                },
                |frame| {
                    for (page, label) in entries {
                        frame.flex(1.0);
                        if frame.selectable(label, self.page == page) {
                            self.page = page;
                        }
                    }
                },
            );
        }
    }

    fn render_page(&mut self, frame: &mut Frame, i18n: &Localizer, out: &mut ChromeEvents) {
        match self.page {
            1 => self.render_touchpad(frame, i18n, out),
            2 => {
                frame.heading(i18n.text(Message::Connectivity), 2);
                self.render_connectivity(frame, i18n, out);
            }
            3 => {
                frame.heading(i18n.text(Message::SoundAndDisplay), 2);
                self.render_sound(frame, i18n, out);
                self.render_brightness(frame, i18n, out);
            }
            _ => {
                frame.heading(i18n.text(Message::Desktop), 2);
                self.render_desktop(frame, i18n, out);
                render_footer(frame, &self.status, i18n);
            }
        }
    }
}

impl Default for ControlCenter {
    fn default() -> Self {
        ControlCenter::new()
    }
}

impl Chrome for ControlCenter {
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        if !self.open {
            return;
        }

        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        let bounds = self.bounds(display);
        frame.layer(
            "ass-control-center-scrim",
            Rect {
                x: 0.0,
                y: 0.0,
                w: display.0,
                h: display.1,
            },
            &OverlayOpts {
                bg: Color::rgba(8, 10, 18, 118),
                ..Default::default()
            },
            |_| {},
        );

        let original_theme = frame.theme();
        frame.set_theme(control_center_theme());
        let mut close = false;
        frame.layer(
            "ass-control-center-app",
            bounds,
            &OverlayOpts {
                bg: Color::rgba(25, 28, 40, 238),
                border: Color::rgba(255, 255, 255, 48),
                border_width: 1.0,
                radius: APP_RADIUS,
                pad: 0.0,
                ..Default::default()
            },
            |frame| {
                frame.column_ex(
                    &LayoutOpts {
                        width: bounds.w,
                        height: bounds.h,
                        gap: 12.0,
                        pad: 22.0,
                        cross: Align::Stretch,
                        ..Default::default()
                    },
                    |frame| {
                        close = self.render_header(frame, i18n);
                        frame.separator();
                        frame.flex(1.0);
                        if bounds.w >= 640.0 {
                            frame.row_ex(
                                &LayoutOpts {
                                    flex: 1.0,
                                    gap: 18.0,
                                    cross: Align::Stretch,
                                    ..Default::default()
                                },
                                |frame| {
                                    frame.column_ex(
                                        &LayoutOpts {
                                            width: 184.0,
                                            gap: 5.0,
                                            pad: 8.0,
                                            cross: Align::Stretch,
                                            bg: Color::rgba(255, 255, 255, 10),
                                            radius: 14.0,
                                            ..Default::default()
                                        },
                                        |frame| {
                                            self.render_navigation(frame, i18n);
                                        },
                                    );
                                    frame.flex(1.0);
                                    frame.scroll("ass-control-center-page", |frame| {
                                        frame.column_ex(
                                            &LayoutOpts {
                                                gap: 12.0,
                                                cross: Align::Stretch,
                                                ..Default::default()
                                            },
                                            |frame| self.render_page(frame, i18n, out),
                                        );
                                    });
                                },
                            );
                        } else {
                            self.render_compact_navigation(frame, i18n);
                            frame.flex(1.0);
                            frame.scroll("ass-control-center-narrow-page", |frame| {
                                frame.column_ex(
                                    &LayoutOpts {
                                        gap: 12.0,
                                        cross: Align::Stretch,
                                        ..Default::default()
                                    },
                                    |frame| self.render_page(frame, i18n, out),
                                );
                            });
                        }
                    },
                );
            },
        );
        frame.set_theme(original_theme);

        let left_pressed = raw.mouse_pressed.first().copied().unwrap_or(false);
        let outside = !contains(bounds, raw.cursor.x, raw.cursor.y);
        if close || (left_pressed && outside) {
            self.open = false;
        }
    }

    fn captures_keyboard(&self) -> bool {
        self.open
    }

    fn key_char(&mut self, key: &KeyChar, _out: &mut ChromeEvents) {
        if self.open && matches!(key_action(key.keysym, key.ch), KeyAction::Escape) {
            self.open = false;
        }
    }

    fn open_builtin(&mut self, app: BuiltInApplication) {
        if app == BuiltInApplication::ControlCenter {
            self.open = true;
        }
    }

    fn update_system_status(&mut self, status: &SystemStatus) {
        self.status = status.clone();
        self.volume = status.volume.unwrap_or(0) as f32;
        self.brightness = status.brightness.unwrap_or(0) as f32;
    }

    fn update_app_catalog(
        &mut self,
        _apps: &[Entry],
        _dock_apps: &[DockApp],
        icons: &HashMap<String, *mut c_void>,
    ) {
        self.icons.clone_from(icons);
    }

    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        self.open
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

    fn modal_active(&self) -> bool {
        self.open
    }

    fn visible_during_modal(&self) -> bool {
        true
    }

    fn set_modal_reserved(&mut self, reserved: Reserved) {
        self.modal_reserved = reserved;
    }

    fn backdrop_blur_sigma(&self) -> f32 {
        if self.open {
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
        if !self.open {
            return Vec::new();
        }
        let panel = self.bounds(display);
        let radius = APP_RADIUS;
        vec![
            BackdropRegion {
                x: panel.x + radius,
                y: panel.y,
                w: (panel.w - radius * 2.0).max(0.0),
                h: panel.h,
            },
            BackdropRegion {
                x: panel.x,
                y: panel.y + radius,
                w: panel.w,
                h: (panel.h - radius * 2.0).max(0.0),
            },
        ]
    }
}

fn render_footer(frame: &mut Frame, status: &SystemStatus, i18n: &Localizer) {
    frame.row_ex(
        &LayoutOpts {
            height: 28.0,
            gap: 10.0,
            cross: Align::Center,
            ..Default::default()
        },
        |frame| {
            frame.icon(Icon::Globe, 14.0);
            frame.label_sized(network_label(status, i18n), 11.0);
            if let Some(battery) = status.battery {
                frame.flex(1.0);
                frame.spacer(0.0);
                frame.icon(Icon::Zap, 14.0);
                let label = if battery.charging {
                    i18n.charging_battery(battery.percent)
                } else {
                    format!("{}%", battery.percent)
                };
                frame.label_sized(&label, 11.0);
            }
        },
    );
}

fn unavailable_row(frame: &mut Frame, label: &str, i18n: &Localizer) {
    frame.row_ex(
        &LayoutOpts {
            height: 26.0,
            gap: 8.0,
            cross: Align::Center,
            ..Default::default()
        },
        |frame| {
            frame.label_sized(label, 12.0);
            frame.flex(1.0);
            frame.spacer(0.0);
            frame.label_sized(i18n.text(Message::Unavailable), 11.0);
        },
    );
}

fn network_label<'a>(status: &SystemStatus, i18n: &'a Localizer) -> &'a str {
    match status.network {
        crate::NetworkState::Wifi => i18n.text(Message::WifiConnected),
        crate::NetworkState::Wired => i18n.text(Message::WiredConnected),
        crate::NetworkState::Offline => i18n.text(Message::Disconnected),
    }
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

fn control_center_theme() -> Theme {
    Theme::dark()
        .with_bg(Color::rgba(25, 28, 40, 255))
        .with_fg(Color::rgba(244, 246, 252, 255))
        .with_accent(Color::rgba(102, 156, 255, 255))
        .with_border(Color::rgba(255, 255, 255, 42))
        .with_hover(Color::rgba(255, 255, 255, 24))
        .with_active(Color::rgba(102, 156, 255, 56))
        .with_corner_radius(12.0)
        .with_border_width(1.0)
        .with_slider_track_color(Color::rgba(255, 255, 255, 30))
        .with_slider_fill_color(Color::rgba(102, 156, 255, 255))
        .with_slider_knob_color(Color::rgba(255, 255, 255, 255))
        .with_scrollbar_width(5.0)
        .with_scrollbar_radius(2.5)
}

fn card_layout() -> LayoutOpts {
    LayoutOpts {
        flex: 1.0,
        min_height: 168.0,
        gap: 10.0,
        pad: 16.0,
        cross: Align::Stretch,
        bg: Color::rgba(255, 255, 255, 14),
        radius: 16.0,
        ..Default::default()
    }
}

fn wide_card_layout() -> LayoutOpts {
    LayoutOpts {
        min_height: 104.0,
        gap: 10.0,
        pad: 16.0,
        cross: Align::Stretch,
        bg: Color::rgba(255, 255, 255, 14),
        radius: 16.0,
        ..Default::default()
    }
}

fn settings_card_layout() -> LayoutOpts {
    LayoutOpts {
        min_height: 96.0,
        gap: 8.0,
        pad: 15.0,
        cross: Align::Stretch,
        bg: Color::rgba(255, 255, 255, 14),
        radius: 16.0,
        ..Default::default()
    }
}

fn section_heading_layout() -> LayoutOpts {
    LayoutOpts {
        height: 24.0,
        gap: 8.0,
        cross: Align::Center,
        ..Default::default()
    }
}

fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.w && y < rect.y + rect.h
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_window_stays_inside_small_outputs() {
        let center = ControlCenter::new();
        let bounds = center.bounds((320.0, 480.0));
        assert!(bounds.x >= 0.0 && bounds.y >= 0.0);
        assert!(bounds.x + bounds.w <= 320.0);
        assert!(bounds.y + bounds.h <= 480.0);
    }

    #[test]
    fn system_updates_seed_slider_values() {
        let mut center = ControlCenter::new();
        center.update_system_status(&SystemStatus {
            volume: Some(63),
            brightness: Some(41),
            ..SystemStatus::default()
        });
        assert_eq!(center.volume, 63.0);
        assert_eq!(center.brightness, 41.0);
    }
}
