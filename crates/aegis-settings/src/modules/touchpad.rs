use aegis_model::input::{TouchpadScrollMethod, TouchpadStatus};
use aegis_model::settings::{SettingsAction, SettingsSnapshot};
use aegis_shell::{Localizer, Message};
use lens::{Align, Frame, Icon, LayoutOpts};

use crate::module::{
    ApplyPolicy, ModuleAvailability, ModuleCategory, ModuleEvents, ModuleId, ModuleMetadata,
    SettingsModule,
};
use crate::ui::{section_heading_layout, settings_card_layout, unavailable_row};

pub(crate) const TOUCHPAD_MODULE_ID: ModuleId = ModuleId::new("touchpad");

/// Capability-driven touchpad settings. Unsupported controls remain visible
/// but disabled so hotplugging a more capable device does not change routes.
pub(crate) struct TouchpadModule {
    status: TouchpadStatus,
}

impl TouchpadModule {
    pub(crate) fn new() -> Self {
        Self {
            status: TouchpadStatus::default(),
        }
    }
}

impl Default for TouchpadModule {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsModule for TouchpadModule {
    fn metadata(&self) -> ModuleMetadata {
        ModuleMetadata {
            id: TOUCHPAD_MODULE_ID,
            title: Message::Touchpad,
            icon: Icon::MousePointer,
            category: ModuleCategory::Hardware,
            keywords: &["touchpad", "pointer", "tap", "scroll", "gesture"],
            apply_policy: ApplyPolicy::Instant,
            availability: ModuleAvailability::Available,
        }
    }

    fn render(&mut self, frame: &mut Frame, i18n: &Localizer, out: &mut ModuleEvents) {
        let status = self.status.clone();
        let mut config = status.config;
        let mut changed = false;
        let has_devices = status.device_count() > 0;
        let can_edit = |capability: bool| status.configurable && has_devices && capability;

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
            self.status.config = config;
            out.actions.push(SettingsAction::SetTouchpad { config });
        }
    }

    fn update_settings(&mut self, snapshot: &SettingsSnapshot) {
        self.status = snapshot.touchpad.clone();
    }
}
