//! The compositor-owned Control Center application.
//!
//! It is launched through the same application catalog as desktop entries,
//! but remains a trusted in-process surface rendered with optics/lens. The UI
//! emits typed intents; it never invokes host commands itself.

use std::collections::HashMap;
use std::ffi::c_void;

use ass_core::Point;
use ass_core::app::{BuiltInApplication, Entry};
use ass_core::input::{KeyAction, KeyChar, TouchpadScrollMethod, key_action};
use ass_core::output::{ModeSpec, OutputInfo, OutputMode};
use ass_core::realm::{RealmId, RealmKind, RealmSnapshot, RealmState};
use ass_core::window::Window;
use ass_core::workspace::WorkspaceSnapshot;
use lens::{Align, Color, Frame, Icon, Input, LayoutOpts, OverlayOpts, Rect, TextBuf, Theme};

use crate::{
    BackdropRegion, Chrome, ChromeEvents, CursorShape, DockApp, Localizer, Message, RealmIntent,
    Reserved, SystemAction, SystemStatus,
};

const APP_MAX_W: f32 = 860.0;
const APP_MAX_H: f32 = 590.0;
const APP_MARGIN: f32 = 24.0;
const APP_RADIUS: f32 = 24.0;
const BACKDROP_BLUR_SIGMA: f32 = 18.0;
const DISPLAY_LAYOUT_RIGHT: i32 = 0;
const DISPLAY_LAYOUT_LEFT: i32 = 1;
const DISPLAY_LAYOUT_ABOVE: i32 = 2;
const DISPLAY_LAYOUT_BELOW: i32 = 3;
const DISPLAY_LAYOUT_CUSTOM: i32 = 4;

/// A trusted built-in application with a stable launcher identity.
pub struct ControlCenter {
    open: bool,
    status: SystemStatus,
    icons: HashMap<String, *mut c_void>,
    modal_reserved: Reserved,
    volume: f32,
    brightness: f32,
    display_output: i32,
    display_mode: i32,
    display_scale: f32,
    display_primary: bool,
    display_layout: i32,
    display_x: TextBuf,
    display_y: TextBuf,
    display_editor_connector: Option<String>,
    display_dirty: bool,
    page: i32,
    realms: RealmSnapshot,
    pending_revoke: Option<RealmId>,
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
            display_output: 0,
            display_mode: 0,
            display_scale: 1.0,
            display_primary: true,
            display_layout: DISPLAY_LAYOUT_RIGHT,
            display_x: TextBuf::new(16, "0"),
            display_y: TextBuf::new(16, "0"),
            display_editor_connector: None,
            display_dirty: false,
            page: 0,
            realms: ass_core::realm::RealmModel::new().snapshot(),
            pending_revoke: None,
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

    fn sync_display_editor(&mut self, connector: Option<&str>) {
        let outputs = &self.status.display.outputs;
        if outputs.is_empty() {
            self.display_output = 0;
            self.display_mode = 0;
            self.display_editor_connector = None;
            self.display_dirty = false;
            return;
        }
        let preferred = connector
            .or(self.display_editor_connector.as_deref())
            .and_then(|connector| {
                outputs
                    .iter()
                    .position(|output| output.connector == connector)
            })
            .unwrap_or(0);
        let output = outputs[preferred].clone();
        self.display_output = preferred as i32;
        self.display_editor_connector = Some(output.connector.clone());
        self.display_mode = output
            .available_modes
            .iter()
            .position(|mode| *mode == output.geometry.mode)
            .unwrap_or(0) as i32;
        self.display_scale = output.geometry.scale.as_f32().clamp(0.25, 4.0);
        self.display_primary = preferred == 0;
        self.display_x
            .set(&output.geometry.logical_origin.x.to_string());
        self.display_y
            .set(&output.geometry.logical_origin.y.to_string());
        self.display_layout = outputs
            .first()
            .map(|primary| infer_display_layout(&output, primary))
            .unwrap_or(DISPLAY_LAYOUT_CUSTOM);
        self.display_dirty = false;
    }

    fn display_position(&self, output: &OutputInfo, mode: OutputMode) -> Option<Point> {
        let custom = || {
            Some(Point {
                x: self.display_x.as_str().trim().parse().ok()?,
                y: self.display_y.as_str().trim().parse().ok()?,
            })
        };
        if self.display_layout == DISPLAY_LAYOUT_CUSTOM {
            return custom();
        }
        let primary = self.status.display.outputs.first()?;
        if primary.connector == output.connector {
            return custom();
        }
        let primary_rect = primary.geometry.logical_rect();
        let scale = self.display_scale.clamp(0.25, 4.0);
        let selected_w = ((mode.width as f32) / scale).round().max(1.0) as i32;
        let selected_h = ((mode.height as f32) / scale).round().max(1.0) as i32;
        Some(match self.display_layout {
            DISPLAY_LAYOUT_LEFT => Point {
                x: primary_rect.origin.x - selected_w,
                y: primary_rect.origin.y,
            },
            DISPLAY_LAYOUT_ABOVE => Point {
                x: primary_rect.origin.x,
                y: primary_rect.origin.y - selected_h,
            },
            DISPLAY_LAYOUT_BELOW => Point {
                x: primary_rect.origin.x,
                y: primary_rect.origin.y + primary_rect.size.h,
            },
            _ => Point {
                x: primary_rect.origin.x + primary_rect.size.w,
                y: primary_rect.origin.y,
            },
        })
    }

    fn render_display(&mut self, frame: &mut Frame, i18n: &Localizer, out: &mut ChromeEvents) {
        let outputs = self.status.display.outputs.clone();
        frame.column_ex(&settings_card_layout(), |frame| {
            frame.row_ex(&section_heading_layout(), |frame| {
                frame.icon(Icon::Square, 17.0);
                frame.heading(i18n.text(Message::Display), 3);
            });
            frame.label_wrapped_sized(i18n.text(Message::DisplayDescription), 12.0, 560.0);

            if outputs.is_empty() {
                unavailable_row(frame, i18n.text(Message::NoDisplays), i18n);
                return;
            }
            let output_labels = outputs
                .iter()
                .map(|output| {
                    format!(
                        "{} · {} × {}",
                        output.connector, output.geometry.mode.width, output.geometry.mode.height
                    )
                })
                .collect::<Vec<_>>();
            let output_items = output_labels.iter().map(String::as_str).collect::<Vec<_>>();
            let before_output = self.display_output;
            frame.label_sized(i18n.text(Message::Displays), 12.0);
            frame.dropdown(
                "##control-center-display-output",
                &mut self.display_output,
                &output_items,
            );
            self.display_output = self
                .display_output
                .clamp(0, outputs.len().saturating_sub(1) as i32);
            if self.display_output != before_output
                || self.display_editor_connector.as_deref()
                    != Some(outputs[self.display_output as usize].connector.as_str())
            {
                let connector = outputs[self.display_output as usize].connector.clone();
                self.sync_display_editor(Some(&connector));
            }
            let output = outputs[self.display_output as usize].clone();

            if !self.status.display.configurable {
                frame.label_wrapped_sized(i18n.text(Message::DisplayHostManaged), 11.0, 560.0);
                display_summary(frame, &output);
                return;
            }

            let modes = if output.available_modes.is_empty() {
                vec![output.geometry.mode]
            } else {
                output.available_modes.clone()
            };
            let mode_labels = modes.iter().map(format_output_mode).collect::<Vec<_>>();
            let mode_items = mode_labels.iter().map(String::as_str).collect::<Vec<_>>();
            self.display_mode = self
                .display_mode
                .clamp(0, modes.len().saturating_sub(1) as i32);
            frame.label_sized(i18n.text(Message::ResolutionAndRefresh), 12.0);
            if frame.dropdown(
                "##control-center-display-mode",
                &mut self.display_mode,
                &mode_items,
            ) {
                self.display_dirty = true;
            }

            frame.row_ex(&section_heading_layout(), |frame| {
                frame.label_sized(i18n.text(Message::Scale), 12.0);
                frame.flex(1.0);
                frame.spacer(0.0);
                frame.label_sized(&format!("{:.0}%", self.display_scale * 100.0), 12.0);
            });
            if frame.slider(
                "##control-center-display-scale",
                &mut self.display_scale,
                0.25,
                4.0,
            ) {
                self.display_scale = (self.display_scale * 4.0).round() / 4.0;
                self.display_dirty = true;
            }

            if self.display_primary {
                frame.label_sized(i18n.text(Message::PrimaryDisplay), 12.0);
            } else {
                frame.size_next(160.0, 30.0);
                if frame.button(i18n.text(Message::MakePrimary)) {
                    self.display_primary = true;
                    self.display_dirty = true;
                }
            }

            let arrangement_labels = [
                i18n.text(Message::RightOfPrimary),
                i18n.text(Message::LeftOfPrimary),
                i18n.text(Message::AbovePrimary),
                i18n.text(Message::BelowPrimary),
                i18n.text(Message::CustomPosition),
            ];
            frame.label_sized(i18n.text(Message::Arrangement), 12.0);
            let current_primary = outputs
                .first()
                .is_some_and(|primary| primary.connector == output.connector);
            if current_primary {
                self.display_layout = DISPLAY_LAYOUT_CUSTOM;
                frame.label_sized(i18n.text(Message::CustomPosition), 11.0);
            } else if frame.dropdown(
                "##control-center-display-arrangement",
                &mut self.display_layout,
                &arrangement_labels,
            ) {
                self.display_dirty = true;
            }
            if self.display_layout == DISPLAY_LAYOUT_CUSTOM {
                frame.row_ex(
                    &LayoutOpts {
                        gap: 10.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |frame| {
                        frame.flex(1.0);
                        frame.column_ex(
                            &LayoutOpts {
                                gap: 4.0,
                                cross: Align::Stretch,
                                ..Default::default()
                            },
                            |frame| {
                                frame.label_sized(i18n.text(Message::HorizontalPosition), 11.0);
                                if frame.textfield(
                                    "##control-center-display-position-x",
                                    &mut self.display_x,
                                ) {
                                    self.display_dirty = true;
                                }
                            },
                        );
                        frame.flex(1.0);
                        frame.column_ex(
                            &LayoutOpts {
                                gap: 4.0,
                                cross: Align::Stretch,
                                ..Default::default()
                            },
                            |frame| {
                                frame.label_sized(i18n.text(Message::VerticalPosition), 11.0);
                                if frame.textfield(
                                    "##control-center-display-position-y",
                                    &mut self.display_y,
                                ) {
                                    self.display_dirty = true;
                                }
                            },
                        );
                    },
                );
            }

            let mode = modes[self.display_mode as usize];
            let position = self.display_position(&output, mode);
            if position.is_none() {
                frame.label_wrapped_sized(i18n.text(Message::InvalidPosition), 11.0, 560.0);
            }
            if let Some(error) = self.status.display.error.as_deref() {
                frame.label_wrapped_sized(error, 11.0, 560.0);
            }
            frame.label_wrapped_sized(i18n.text(Message::DisplayApplyHint), 11.0, 560.0);
            frame.row_ex(
                &LayoutOpts {
                    height: 32.0,
                    gap: 8.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |frame| {
                    frame.size_next(190.0, 30.0);
                    let apply = frame.button(i18n.text(Message::ApplyDisplaySettings));
                    if apply
                        && self.display_dirty
                        && let Some(position) = position
                    {
                        out.system_actions
                            .push(SystemAction::SetDisplay(crate::DisplaySettings {
                                connector: output.connector.clone(),
                                mode: ModeSpec {
                                    width: mode.width,
                                    height: mode.height,
                                    refresh_hz: Some(mode.refresh_mhz.saturating_add(500) / 1_000),
                                },
                                scale: f64::from(self.display_scale.clamp(0.25, 4.0)),
                                position,
                                primary: self.display_primary,
                            }));
                        self.display_dirty = false;
                    }
                    frame.size_next(92.0, 30.0);
                    if frame.button(i18n.text(Message::ResetDisplaySettings)) {
                        let connector = output.connector.clone();
                        self.sync_display_editor(Some(&connector));
                    }
                },
            );
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

    fn render_realms(&mut self, frame: &mut Frame, i18n: &Localizer, out: &mut ChromeEvents) {
        frame.column_ex(
            &LayoutOpts {
                gap: 5.0,
                cross: Align::Stretch,
                ..Default::default()
            },
            |frame| {
                frame.heading(i18n.text(Message::AiWorkspaces), 2);
                frame.label_wrapped_sized(i18n.text(Message::AiWorkspacesDescription), 12.0, 560.0);
            },
        );
        frame.size_next(0.0, 32.0);
        if frame.button(i18n.text(Message::NewAiWorkspace)) {
            let ordinal = self
                .realms
                .realms
                .iter()
                .filter(|realm| realm.kind == RealmKind::Agent)
                .count()
                + 1;
            out.realm_intents.push(RealmIntent::Create {
                label: format!("AI Workspace {ordinal}"),
            });
        }

        let realms = self
            .realms
            .realms
            .iter()
            .filter(|realm| realm.kind == RealmKind::Agent)
            .cloned()
            .collect::<Vec<_>>();
        for realm in realms {
            let controlled_windows = self
                .realms
                .interaction_groups
                .iter()
                .filter(|group| group.control_realm == realm.id)
                .map(|group| group.windows.len())
                .sum::<usize>();
            frame.column_ex(&settings_card_layout(), |frame| {
                frame.row_ex(&section_heading_layout(), |frame| {
                    frame.heading(&realm.label, 3);
                    frame.flex(1.0);
                    frame.spacer(0.0);
                    let state = match realm.state {
                        RealmState::Active => i18n.text(Message::RealmActive),
                        RealmState::Paused => i18n.text(Message::RealmPaused),
                        RealmState::Revoked => i18n.text(Message::RealmRevoked),
                    };
                    frame.label_sized(state, 11.0);
                });
                frame.label_sized(
                    &format!("Realm {} · {}", realm.id.0, controlled_windows),
                    11.0,
                );
                if realm.state != RealmState::Revoked {
                    frame.row_ex(
                        &LayoutOpts {
                            height: 30.0,
                            gap: 8.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |frame| {
                            frame.flex(1.0);
                            frame.size_next(116.0, 28.0);
                            let next = if realm.state == RealmState::Active {
                                RealmState::Paused
                            } else {
                                RealmState::Active
                            };
                            let label = if next == RealmState::Paused {
                                i18n.text(Message::PauseRealm)
                            } else {
                                i18n.text(Message::ResumeRealm)
                            };
                            if frame.button(label) {
                                out.realm_intents.push(RealmIntent::SetState {
                                    realm: realm.id,
                                    state: next,
                                    expected_revision: self.realms.revision,
                                });
                            }
                            frame.size_next(132.0, 28.0);
                            let confirming = self.pending_revoke == Some(realm.id);
                            let label = if confirming {
                                i18n.text(Message::ConfirmRevokeRealm)
                            } else {
                                i18n.text(Message::RevokeRealm)
                            };
                            if frame.button(label) {
                                if confirming {
                                    out.realm_intents.push(RealmIntent::Revoke {
                                        realm: realm.id,
                                        expected_revision: self.realms.revision,
                                    });
                                    self.pending_revoke = None;
                                } else {
                                    self.pending_revoke = Some(realm.id);
                                }
                            }
                        },
                    );
                }
            });
        }
    }

    fn render_navigation(&mut self, frame: &mut Frame, i18n: &Localizer) {
        for (page, icon, label) in [
            (0, Icon::Grid, i18n.text(Message::Desktop)),
            (1, Icon::MousePointer, i18n.text(Message::Touchpad)),
            (2, Icon::Radio, i18n.text(Message::Connectivity)),
            (3, Icon::Sliders, i18n.text(Message::SoundAndDisplay)),
            (4, Icon::Grid, i18n.text(Message::AiWorkspaces)),
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
                (3, i18n.text(Message::SoundAndDisplay)),
            ],
            [
                (4, i18n.text(Message::AiWorkspaces)),
                (0, i18n.text(Message::Desktop)),
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
                self.render_display(frame, i18n, out);
            }
            4 => self.render_realms(frame, i18n, out),
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
        let selected = self.display_editor_connector.clone();
        let selected_still_present = selected.as_deref().is_some_and(|connector| {
            status
                .display
                .outputs
                .iter()
                .any(|output| output.connector == connector)
        });
        self.status = status.clone();
        self.volume = status.volume.unwrap_or(0) as f32;
        self.brightness = status.brightness.unwrap_or(0) as f32;
        if !self.display_dirty || !selected_still_present {
            self.sync_display_editor(selected.as_deref());
        }
    }

    fn update_realms(&mut self, snapshot: &RealmSnapshot) {
        self.realms = snapshot.clone();
        if self.pending_revoke.is_some_and(|id| {
            snapshot
                .realms
                .iter()
                .find(|realm| realm.id == id)
                .is_none_or(|realm| realm.state == RealmState::Revoked)
        }) {
            self.pending_revoke = None;
        }
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
        if self.open { BACKDROP_BLUR_SIGMA } else { 0.0 }
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

fn infer_display_layout(output: &OutputInfo, primary: &OutputInfo) -> i32 {
    if output.connector == primary.connector {
        return DISPLAY_LAYOUT_CUSTOM;
    }
    let output_rect = output.geometry.logical_rect();
    let primary_rect = primary.geometry.logical_rect();
    if output_rect.origin.x == primary_rect.origin.x + primary_rect.size.w
        && output_rect.origin.y == primary_rect.origin.y
    {
        DISPLAY_LAYOUT_RIGHT
    } else if output_rect.origin.x + output_rect.size.w == primary_rect.origin.x
        && output_rect.origin.y == primary_rect.origin.y
    {
        DISPLAY_LAYOUT_LEFT
    } else if output_rect.origin.y + output_rect.size.h == primary_rect.origin.y
        && output_rect.origin.x == primary_rect.origin.x
    {
        DISPLAY_LAYOUT_ABOVE
    } else if output_rect.origin.y == primary_rect.origin.y + primary_rect.size.h
        && output_rect.origin.x == primary_rect.origin.x
    {
        DISPLAY_LAYOUT_BELOW
    } else {
        DISPLAY_LAYOUT_CUSTOM
    }
}

fn format_output_mode(mode: &OutputMode) -> String {
    let refresh = if mode.refresh_mhz.is_multiple_of(1_000) {
        format!("{} Hz", mode.refresh_mhz / 1_000)
    } else {
        format!("{:.2} Hz", mode.refresh_mhz as f64 / 1_000.0)
    };
    format!("{} × {} · {refresh}", mode.width, mode.height)
}

fn display_summary(frame: &mut Frame, output: &OutputInfo) {
    frame.label_sized(&format_output_mode(&output.geometry.mode), 12.0);
    frame.label_sized(
        &format!(
            "{:.0}% · ({}, {})",
            output.geometry.scale.as_f32() * 100.0,
            output.geometry.logical_origin.x,
            output.geometry.logical_origin.y
        ),
        11.0,
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

    fn output(connector: &str, x: i32, y: i32, width: i32, height: i32) -> OutputInfo {
        let mode = OutputMode {
            width,
            height,
            refresh_mhz: 60_000,
        };
        OutputInfo {
            connector: connector.into(),
            geometry: ass_core::output::OutputGeometry {
                mode,
                scale: ass_core::output::Scale::IDENTITY,
                transform: ass_core::Transform::Normal,
                logical_origin: Point { x, y },
            },
            available_modes: vec![mode],
        }
    }

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

    #[test]
    fn display_editor_tracks_connector_mode_and_extended_layout() {
        let mut center = ControlCenter::new();
        center.update_system_status(&SystemStatus {
            display: crate::DisplayStatus {
                configurable: true,
                outputs: vec![
                    output("eDP-1", 0, 0, 1920, 1080),
                    output("DP-1", 1920, 0, 2560, 1440),
                ],
                error: None,
            },
            ..SystemStatus::default()
        });
        center.sync_display_editor(Some("DP-1"));
        assert_eq!(center.display_output, 1);
        assert_eq!(center.display_mode, 0);
        assert_eq!(center.display_layout, DISPLAY_LAYOUT_RIGHT);
        assert!(!center.display_primary);
        assert_eq!(center.display_x.as_str(), "1920");
        assert_eq!(center.display_y.as_str(), "0");
    }

    #[test]
    fn relative_layout_uses_edited_mode_and_scale() {
        let mut center = ControlCenter::new();
        let primary = output("eDP-1", 0, 0, 1920, 1080);
        let secondary = output("DP-1", 1920, 0, 2560, 1440);
        center.status.display.outputs = vec![primary, secondary.clone()];
        center.display_layout = DISPLAY_LAYOUT_LEFT;
        center.display_scale = 2.0;
        assert_eq!(
            center.display_position(
                &secondary,
                OutputMode {
                    width: 2560,
                    height: 1440,
                    refresh_mhz: 144_000,
                },
            ),
            Some(Point { x: -1280, y: 0 })
        );
    }
}
