//! Instant, non-persistent controls kept separate from settings modules.

use ass_design::{Design, materials};
use ass_shell::{ChromeEvents, Localizer, Message, NetworkState, SystemAction, SystemStatus};
use lens::{Align, Frame, Icon, LayoutOpts};

use crate::ui::{section_heading_layout, unavailable_row};

pub(crate) struct QuickSettings {
    status: SystemStatus,
    volume: f32,
    brightness: f32,
}

impl QuickSettings {
    pub(crate) fn new() -> Self {
        Self {
            status: SystemStatus::default(),
            volume: 0.0,
            brightness: 0.0,
        }
    }

    /// Render instant controls. Returns `true` when the host should close
    /// because another compositor surface was requested.
    pub(crate) fn render(
        &mut self,
        frame: &mut Frame,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) -> bool {
        frame.heading(i18n.text(Message::QuickSettings), 2);
        self.render_connectivity(frame, i18n, out);
        self.render_sound(frame, i18n, out);
        self.render_brightness(frame, i18n, out);
        let close = self.render_session(frame, i18n, out);
        render_footer(frame, &self.status, i18n);
        close
    }

    pub(crate) fn update_system_status(&mut self, status: &SystemStatus) {
        self.status = status.clone();
        self.volume = status.volume.unwrap_or(0) as f32;
        self.brightness = status.brightness.unwrap_or(0) as f32;
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

    fn render_session(
        &mut self,
        frame: &mut Frame,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) -> bool {
        let mut close = false;
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
                close = true;
                out.toggle_launcher = true;
            }
            frame.size_next(0.0, 30.0);
            if frame.button(i18n.text(Message::QuitSession)) {
                out.quit = true;
            }
        });
        close
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
}

impl Default for QuickSettings {
    fn default() -> Self {
        Self::new()
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

fn network_label<'a>(status: &SystemStatus, i18n: &'a Localizer) -> &'a str {
    match status.network {
        NetworkState::Wifi => i18n.text(Message::WifiConnected),
        NetworkState::Wired => i18n.text(Message::WiredConnected),
        NetworkState::Offline => i18n.text(Message::Disconnected),
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

fn card_layout() -> LayoutOpts {
    LayoutOpts {
        flex: 1.0,
        min_height: 168.0,
        gap: 10.0,
        pad: 16.0,
        cross: Align::Stretch,
        ..materials::card(&Design::dark())
    }
}

fn wide_card_layout() -> LayoutOpts {
    LayoutOpts {
        min_height: 104.0,
        gap: 10.0,
        pad: 16.0,
        cross: Align::Stretch,
        ..materials::card(&Design::dark())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_updates_seed_instant_control_values() {
        let mut quick = QuickSettings::new();
        quick.update_system_status(&SystemStatus {
            volume: Some(63),
            brightness: Some(41),
            ..SystemStatus::default()
        });
        assert_eq!(quick.volume, 63.0);
        assert_eq!(quick.brightness, 41.0);
    }
}
