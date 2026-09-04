use tessera_design::Design;
use tessera_model::input::{KeyboardConfig, MouseStatus, TouchpadScrollMethod, TouchpadStatus};
use tessera_model::settings::{SettingsAction, SettingsSnapshot};
use tessera_shell::{Localizer, Message};
use lens::{Align, Frame, Icon, LayoutOpts};

use crate::module::{
    ApplyPolicy, ModuleAvailability, ModuleCategory, ModuleEvents, ModuleId, ModuleMetadata,
    SettingsModule,
};
use crate::ui::{section_heading_layout, settings_card_layout, unavailable_row};

pub(crate) const INPUT_MODULE_ID: ModuleId = ModuleId::new("input");

/// Capability-driven input settings: keyboard repeat, mouse speed and
/// scrolling, and the touchpad profile. Unsupported controls remain visible
/// but disabled so hotplugging a more capable device does not change routes.
pub(crate) struct InputModule {
    touchpad: TouchpadStatus,
    mouse: MouseStatus,
    keyboard: KeyboardConfig,
}

impl InputModule {
    pub(crate) fn new() -> Self {
        Self {
            touchpad: TouchpadStatus::default(),
            mouse: MouseStatus::default(),
            keyboard: KeyboardConfig::default(),
        }
    }
}

impl Default for InputModule {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsModule for InputModule {
    fn metadata(&self) -> ModuleMetadata {
        ModuleMetadata {
            id: INPUT_MODULE_ID,
            title: Message::Input,
            icon: Icon::MousePointer,
            category: ModuleCategory::Hardware,
            keywords: &[
                "input", "keyboard", "repeat", "mouse", "pointer", "speed", "scroll", "touchpad",
                "tap", "gesture",
            ],
            apply_policy: ApplyPolicy::Instant,
            availability: ModuleAvailability::Available,
        }
    }

    fn render(
        &mut self,
        frame: &mut Frame,
        i18n: &Localizer,
        design: &Design,
        out: &mut ModuleEvents,
    ) {
        // The whole domain commits as one coherent profile; draft edits in
        // local copies and emit one SetInput action when anything moved.
        let mut touchpad = self.touchpad.config;
        let mut mouse = self.mouse.config;
        // The keyboard profile is integral (repeats/second, milliseconds);
        // sliders edit float drafts that are rounded back on commit.
        let mut keyboard_rate = self.keyboard.repeat_rate as f32;
        let mut keyboard_delay = self.keyboard.repeat_delay_ms as f32;
        let mut keyboard = self.keyboard;
        let mut changed = false;

        let touchpad_devices = self.touchpad.device_count() > 0;
        let mouse_devices = self.mouse.device_count() > 0;
        let can_edit_touchpad =
            |capability: bool| self.touchpad.configurable && touchpad_devices && capability;
        let can_edit_mouse =
            |capability: bool| self.mouse.configurable && mouse_devices && capability;

        frame.column_ex(
            &LayoutOpts {
                gap: 5.0,
                cross: Align::Stretch,
                ..Default::default()
            },
            |frame| {
                frame.heading(i18n.text(Message::Input), 2);
                frame.label_sized(
                    i18n.text(Message::InputDescription),
                    design.typography.label,
                );
                if !self.touchpad.configurable && !self.mouse.configurable {
                    frame.label_wrapped_sized(
                        i18n.text(Message::InputHostManaged),
                        design.typography.footnote,
                        560.0,
                    );
                } else {
                    let mut devices: Vec<String> = Vec::new();
                    if touchpad_devices {
                        devices.extend(self.touchpad.device_names.iter().cloned());
                    }
                    if mouse_devices {
                        devices.extend(self.mouse.device_names.iter().cloned());
                    }
                    if devices.is_empty() {
                        frame.label_wrapped_sized(
                            i18n.text(Message::NoTouchpadDetected),
                            design.typography.footnote,
                            560.0,
                        );
                    } else {
                        devices.sort();
                        devices.dedup();
                        frame.label_sized(&devices.join(" · "), design.typography.footnote);
                    }
                }
            },
        );

        // ---- Keyboard -------------------------------------------------
        frame.column_ex(&settings_card_layout(design), |frame| {
            frame.heading(i18n.text(Message::Keyboard), 3);
            frame.label_wrapped_sized(
                i18n.text(Message::KeyboardRepeatDescription),
                design.typography.footnote,
                560.0,
            );
            frame.separator();
            frame.row_ex(&section_heading_layout(), |frame| {
                frame.label_sized(i18n.text(Message::RepeatRate), design.typography.label);
                frame.flex(1.0);
                frame.spacer(0.0);
                frame.label_sized(
                    &format!(
                        "{} / {}",
                        keyboard_rate.round() as u32,
                        i18n.text(Message::RepeatRateUnit)
                    ),
                    design.typography.footnote,
                );
            });
            changed |= frame.slider(
                "##keyboard-repeat-rate",
                &mut keyboard_rate,
                0.0,
                tessera_model::input::MAX_REPEAT_RATE as f32,
            );
            frame.row_ex(
                &LayoutOpts {
                    height: 18.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |frame| {
                    frame.label_sized(
                        i18n.text(Message::RepeatDisabled),
                        design.typography.caption,
                    );
                    frame.flex(1.0);
                    frame.spacer(0.0);
                    frame.label_sized(i18n.text(Message::Fast), design.typography.caption);
                },
            );

            frame.row_ex(&section_heading_layout(), |frame| {
                frame.label_sized(i18n.text(Message::RepeatDelay), design.typography.label);
                frame.flex(1.0);
                frame.spacer(0.0);
                frame.label_sized(
                    &format!(
                        "{} {}",
                        keyboard_delay.round() as u32,
                        i18n.text(Message::Milliseconds)
                    ),
                    design.typography.footnote,
                );
            });
            changed |= frame.slider(
                "##keyboard-repeat-delay",
                &mut keyboard_delay,
                50.0,
                tessera_model::input::MAX_REPEAT_DELAY_MS as f32,
            );
            frame.row_ex(
                &LayoutOpts {
                    height: 18.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |frame| {
                    frame.label_sized(i18n.text(Message::Slow), design.typography.caption);
                    frame.flex(1.0);
                    frame.spacer(0.0);
                    frame.label_sized(i18n.text(Message::Fast), design.typography.caption);
                },
            );
        });

        // ---- Mouse ----------------------------------------------------
        frame.column_ex(&settings_card_layout(design), |frame| {
            frame.heading(i18n.text(Message::Mouse), 3);
            render_pointer_speed(
                frame,
                i18n,
                design,
                "mouse",
                &mut mouse.pointer_speed,
                can_edit_mouse(self.mouse.capabilities.pointer_speed),
                &mut changed,
            );
            render_scroll_speed(
                frame,
                i18n,
                design,
                "mouse",
                &mut mouse.scroll_speed,
                // The scroll factor applies to wheel motion from any
                // configurable pointer, independent of the other rows.
                self.mouse.configurable && mouse_devices,
                &mut changed,
            );
            frame.separator();
            changed |= frame
                .setting_switch(
                    "mouse-natural-scroll",
                    i18n.text(Message::NaturalScroll),
                    i18n.text(Message::NaturalScrollDescription),
                    &mut mouse.natural_scroll,
                    !can_edit_mouse(self.mouse.capabilities.natural_scroll),
                )
                .changed;
        });

        // ---- Touchpad -------------------------------------------------
        frame.column_ex(&settings_card_layout(design), |frame| {
            frame.heading(i18n.text(Message::Touchpad), 3);
            changed |= frame
                .setting_switch(
                    "touchpad-tap-to-click",
                    i18n.text(Message::TapToClick),
                    i18n.text(Message::TapToClickDescription),
                    &mut touchpad.tap_to_click,
                    !can_edit_touchpad(self.touchpad.capabilities.tap_to_click),
                )
                .changed;
            changed |= frame
                .setting_switch(
                    "touchpad-tap-and-drag",
                    i18n.text(Message::TapAndDrag),
                    i18n.text(Message::TapAndDragDescription),
                    &mut touchpad.tap_and_drag,
                    !touchpad.tap_to_click
                        || !can_edit_touchpad(self.touchpad.capabilities.tap_and_drag),
                )
                .changed;
            changed |= frame
                .setting_switch(
                    "touchpad-drag-lock",
                    i18n.text(Message::DragLock),
                    i18n.text(Message::DragLockDescription),
                    &mut touchpad.drag_lock,
                    !touchpad.tap_to_click
                        || !touchpad.tap_and_drag
                        || !can_edit_touchpad(self.touchpad.capabilities.drag_lock),
                )
                .changed;
            changed |= frame
                .setting_switch(
                    "touchpad-disable-while-typing",
                    i18n.text(Message::DisableWhileTyping),
                    i18n.text(Message::DisableWhileTypingDescription),
                    &mut touchpad.disable_while_typing,
                    !can_edit_touchpad(self.touchpad.capabilities.disable_while_typing),
                )
                .changed;

            frame.separator();
            render_pointer_speed(
                frame,
                i18n,
                design,
                "touchpad",
                &mut touchpad.pointer_speed,
                can_edit_touchpad(self.touchpad.capabilities.pointer_speed),
                &mut changed,
            );

            frame.separator();
            changed |= frame
                .setting_switch(
                    "touchpad-natural-scroll",
                    i18n.text(Message::NaturalScroll),
                    i18n.text(Message::NaturalScrollDescription),
                    &mut touchpad.natural_scroll,
                    !can_edit_touchpad(self.touchpad.capabilities.natural_scroll),
                )
                .changed;
            render_scroll_speed(
                frame,
                i18n,
                design,
                "touchpad",
                &mut touchpad.scroll_speed,
                // The scroll factor applies to touchpad scroll sequences from
                // any configurable device with a scroll method.
                can_edit_touchpad(
                    self.touchpad.capabilities.two_finger_scroll
                        || self.touchpad.capabilities.edge_scroll,
                ),
                &mut changed,
            );

            let can_two_finger = can_edit_touchpad(self.touchpad.capabilities.two_finger_scroll);
            let can_edge = can_edit_touchpad(self.touchpad.capabilities.edge_scroll);
            frame.row_ex(
                &LayoutOpts {
                    min_height: 34.0,
                    gap: 12.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |frame| {
                    frame.label_sized(i18n.text(Message::ScrollMethod), design.typography.label);
                    frame.flex(1.0);
                    frame.spacer(0.0);
                    if can_two_finger && can_edge {
                        frame.size_next(190.0, 30.0);
                        let mut selected = match touchpad.scroll_method {
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
                            touchpad.scroll_method = if selected == 0 {
                                TouchpadScrollMethod::TwoFinger
                            } else {
                                TouchpadScrollMethod::Edge
                            };
                            changed = true;
                        }
                    } else if can_two_finger {
                        frame.label_sized(
                            i18n.text(Message::TwoFingerScroll),
                            design.typography.footnote,
                        );
                    } else if can_edge {
                        frame.label_sized(
                            i18n.text(Message::EdgeScroll),
                            design.typography.footnote,
                        );
                    } else {
                        frame.label_sized(
                            i18n.text(Message::Unavailable),
                            design.typography.footnote,
                        );
                    }
                },
            );
        });

        if changed {
            touchpad.pointer_speed = touchpad.pointer_speed.clamp(-1.0, 1.0);
            touchpad.scroll_speed = touchpad.scroll_speed.clamp(0.1, 10.0);
            mouse.pointer_speed = mouse.pointer_speed.clamp(-1.0, 1.0);
            mouse.scroll_speed = mouse.scroll_speed.clamp(0.1, 10.0);
            keyboard.repeat_rate = keyboard_rate
                .round()
                .clamp(0.0, tessera_model::input::MAX_REPEAT_RATE as f32)
                as u32;
            keyboard.repeat_delay_ms = keyboard_delay
                .round()
                .clamp(1.0, tessera_model::input::MAX_REPEAT_DELAY_MS as f32)
                as u32;
            self.touchpad.config = touchpad;
            self.mouse.config = mouse;
            self.keyboard = keyboard;
            out.actions.push(SettingsAction::SetInput {
                config: tessera_model::input::InputConfig {
                    touchpad,
                    mouse,
                    keyboard,
                },
            });
        }
    }

    fn update_settings(&mut self, snapshot: &SettingsSnapshot) {
        self.touchpad = snapshot.input.touchpad.clone();
        self.mouse = snapshot.input.mouse.clone();
        self.keyboard = snapshot.input.keyboard;
    }
}

/// Shared pointer-speed row + slider, used by the mouse and touchpad cards.
fn render_pointer_speed(
    frame: &mut Frame,
    i18n: &Localizer,
    design: &Design,
    device: &str,
    speed: &mut f32,
    editable: bool,
    changed: &mut bool,
) {
    frame.row_ex(&section_heading_layout(), |frame| {
        frame.label_sized(i18n.text(Message::PointerSpeed), design.typography.label);
        frame.flex(1.0);
        frame.spacer(0.0);
        frame.label_sized(
            &format!("{:+.0}%", *speed * 100.0),
            design.typography.footnote,
        );
    });
    if editable {
        *changed |= frame.slider(&format!("##{device}-pointer-speed"), speed, -1.0, 1.0);
        frame.row_ex(
            &LayoutOpts {
                height: 18.0,
                cross: Align::Center,
                ..Default::default()
            },
            |frame| {
                frame.label_sized(i18n.text(Message::Slow), design.typography.caption);
                frame.flex(1.0);
                frame.spacer(0.0);
                frame.label_sized(i18n.text(Message::Fast), design.typography.caption);
            },
        );
    } else {
        unavailable_row(frame, i18n.text(Message::PointerSpeed), i18n, design);
    }
}

/// Shared scroll-speed row + slider, used by the mouse and touchpad cards.
fn render_scroll_speed(
    frame: &mut Frame,
    i18n: &Localizer,
    design: &Design,
    device: &str,
    speed: &mut f32,
    editable: bool,
    changed: &mut bool,
) {
    frame.row_ex(&section_heading_layout(), |frame| {
        frame.label_sized(i18n.text(Message::ScrollSpeed), design.typography.label);
        frame.flex(1.0);
        frame.spacer(0.0);
        frame.label_sized(&format!("×{:.1}", *speed), design.typography.footnote);
    });
    if editable {
        *changed |= frame.slider(&format!("##{device}-scroll-speed"), speed, 0.1, 10.0);
        frame.row_ex(
            &LayoutOpts {
                height: 18.0,
                cross: Align::Center,
                ..Default::default()
            },
            |frame| {
                frame.label_sized(i18n.text(Message::Slow), design.typography.caption);
                frame.flex(1.0);
                frame.spacer(0.0);
                frame.label_sized(i18n.text(Message::Fast), design.typography.caption);
            },
        );
    } else {
        unavailable_row(frame, i18n.text(Message::ScrollSpeed), i18n, design);
    }
}
