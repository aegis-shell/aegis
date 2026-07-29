use aegis_core::settings::{
    AccentColor, ColorScheme, Contrast, DesktopPreferences, SettingsAction, SettingsSnapshot,
};
use aegis_shell::{Localizer, Message};
use lens::{Align, Frame, Icon, LayoutOpts, TextBuf};

use crate::module::{
    ApplyPolicy, ModuleAvailability, ModuleCategory, ModuleEvents, ModuleId, ModuleMetadata,
    SettingsModule,
};
use crate::ui::{section_heading_layout, settings_card_layout};

pub(crate) const APPEARANCE_MODULE_ID: ModuleId = ModuleId::new("appearance");

/// Explicit-apply editor for the complete compositor-owned desktop preference
/// profile. Drafts remain local until the user applies them as one atomic IPC
/// settings transaction.
pub(crate) struct AppearanceModule {
    authoritative: DesktopPreferences,
    draft: DesktopPreferences,
    accent_color: TextBuf,
    font_name: TextBuf,
    monospace_font_name: TextBuf,
    icon_theme: TextBuf,
    cursor_theme: TextBuf,
    dirty: bool,
    invalid: bool,
}

impl AppearanceModule {
    pub(crate) fn new() -> Self {
        let preferences = DesktopPreferences::default();
        let mut module = Self {
            authoritative: preferences.clone(),
            draft: preferences,
            accent_color: TextBuf::new(16, ""),
            font_name: TextBuf::new(256, ""),
            monospace_font_name: TextBuf::new(256, ""),
            icon_theme: TextBuf::new(256, ""),
            cursor_theme: TextBuf::new(256, ""),
            dirty: false,
            invalid: false,
        };
        module.reset_editor();
        module
    }

    fn reset_editor(&mut self) {
        self.draft = self.authoritative.clone();
        self.accent_color.set(
            &self
                .authoritative
                .accent_color
                .map(AccentColor::to_hex)
                .unwrap_or_default(),
        );
        self.font_name.set(&self.authoritative.font_name);
        self.monospace_font_name
            .set(&self.authoritative.monospace_font_name);
        self.icon_theme.set(&self.authoritative.icon_theme);
        self.cursor_theme.set(&self.authoritative.cursor_theme);
        self.dirty = false;
        self.invalid = false;
    }

    fn candidate(&self) -> Result<DesktopPreferences, &'static str> {
        let mut preferences = self.draft.clone();
        preferences.accent_color = match self.accent_color.as_str().trim() {
            "" => None,
            value => Some(AccentColor::parse_hex(value)?),
        };
        preferences.font_name = self.font_name.as_str().trim().to_owned();
        preferences.monospace_font_name = self.monospace_font_name.as_str().trim().to_owned();
        preferences.icon_theme = self.icon_theme.as_str().trim().to_owned();
        preferences.cursor_theme = self.cursor_theme.as_str().trim().to_owned();
        preferences.validate()?;
        Ok(preferences)
    }
}

impl Default for AppearanceModule {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsModule for AppearanceModule {
    fn metadata(&self) -> ModuleMetadata {
        ModuleMetadata {
            id: APPEARANCE_MODULE_ID,
            title: Message::Appearance,
            icon: Icon::PenTool,
            category: ModuleCategory::Personalization,
            keywords: &[
                "theme",
                "appearance",
                "contrast",
                "icons",
                "fonts",
                "cursor",
                "motion",
            ],
            apply_policy: ApplyPolicy::Explicit,
            availability: ModuleAvailability::Available,
        }
    }

    fn render(&mut self, frame: &mut Frame, i18n: &Localizer, out: &mut ModuleEvents) {
        frame.heading(i18n.text(Message::Appearance), 2);
        frame.label_wrapped_sized(i18n.text(Message::AppearanceDescription), 12.0, 560.0);

        frame.column_ex(&settings_card_layout(), |frame| {
            frame.heading(i18n.text(Message::AppearanceColorScheme), 3);
            frame.row_ex(&section_heading_layout(), |frame| {
                frame.label_sized(i18n.text(Message::AppearanceColorScheme), 12.0);
                frame.flex(1.0);
                frame.spacer(0.0);
                let mut selected = match self.draft.color_scheme {
                    ColorScheme::System => 0,
                    ColorScheme::Dark => 1,
                    ColorScheme::Light => 2,
                };
                frame.size_next(180.0, 30.0);
                if frame.dropdown(
                    "##settings-color-scheme",
                    &mut selected,
                    &[
                        i18n.text(Message::SystemDefault),
                        i18n.text(Message::PreferDark),
                        i18n.text(Message::PreferLight),
                    ],
                ) {
                    self.draft.color_scheme = match selected {
                        1 => ColorScheme::Dark,
                        2 => ColorScheme::Light,
                        _ => ColorScheme::System,
                    };
                    self.dirty = true;
                }
            });

            if text_field(
                frame,
                i18n.text(Message::AccentColor),
                "##settings-accent-color",
                &mut self.accent_color,
            ) {
                self.dirty = true;
                self.invalid = false;
            }

            frame.row_ex(&section_heading_layout(), |frame| {
                frame.label_sized(i18n.text(Message::Contrast), 12.0);
                frame.flex(1.0);
                frame.spacer(0.0);
                let mut selected = i32::from(self.draft.contrast == Contrast::High);
                frame.size_next(180.0, 30.0);
                if frame.dropdown(
                    "##settings-contrast",
                    &mut selected,
                    &[
                        i18n.text(Message::NormalContrast),
                        i18n.text(Message::HighContrast),
                    ],
                ) {
                    self.draft.contrast = if selected == 1 {
                        Contrast::High
                    } else {
                        Contrast::Normal
                    };
                    self.dirty = true;
                }
            });

            if frame
                .setting_switch(
                    "settings-reduced-motion",
                    i18n.text(Message::ReducedMotion),
                    i18n.text(Message::ReducedMotionDescription),
                    &mut self.draft.reduced_motion,
                    false,
                )
                .changed
            {
                self.dirty = true;
            }
        });

        frame.column_ex(&settings_card_layout(), |frame| {
            frame.heading(i18n.text(Message::InterfaceFont), 3);
            if text_field(
                frame,
                i18n.text(Message::InterfaceFont),
                "##settings-interface-font",
                &mut self.font_name,
            ) {
                self.dirty = true;
                self.invalid = false;
            }
            if text_field(
                frame,
                i18n.text(Message::MonospaceFont),
                "##settings-monospace-font",
                &mut self.monospace_font_name,
            ) {
                self.dirty = true;
                self.invalid = false;
            }
            frame.row_ex(&section_heading_layout(), |frame| {
                frame.label_sized(i18n.text(Message::TextScale), 12.0);
                frame.flex(1.0);
                frame.spacer(0.0);
                frame.label_sized(&format!("{:.0}%", self.draft.text_scale * 100.0), 11.0);
            });
            let mut scale = self.draft.text_scale as f32;
            if frame.slider("##settings-text-scale", &mut scale, 0.5, 3.0) {
                self.draft.text_scale = f64::from((scale * 20.0).round() / 20.0);
                self.dirty = true;
            }
        });

        frame.column_ex(&settings_card_layout(), |frame| {
            frame.heading(i18n.text(Message::IconTheme), 3);
            if text_field(
                frame,
                i18n.text(Message::IconTheme),
                "##settings-icon-theme",
                &mut self.icon_theme,
            ) {
                self.dirty = true;
                self.invalid = false;
            }
            if text_field(
                frame,
                i18n.text(Message::CursorTheme),
                "##settings-cursor-theme",
                &mut self.cursor_theme,
            ) {
                self.dirty = true;
                self.invalid = false;
            }
            frame.row_ex(&section_heading_layout(), |frame| {
                frame.label_sized(i18n.text(Message::CursorSize), 12.0);
                frame.flex(1.0);
                frame.spacer(0.0);
                frame.label_sized(&format!("{} px", self.draft.cursor_size), 11.0);
            });
            let mut size = self.draft.cursor_size as f32;
            if frame.slider("##settings-cursor-size", &mut size, 8.0, 128.0) {
                self.draft.cursor_size = size.round() as u32;
                self.dirty = true;
            }
        });

        if self.invalid {
            frame.label_wrapped_sized(i18n.text(Message::InvalidAppearanceSettings), 11.0, 560.0);
        }
        frame.label_wrapped_sized(i18n.text(Message::AppearanceApplyHint), 11.0, 560.0);
        frame.row_ex(
            &LayoutOpts {
                height: 32.0,
                gap: 8.0,
                cross: Align::Center,
                ..Default::default()
            },
            |frame| {
                frame.size_next(210.0, 30.0);
                if frame.button(i18n.text(Message::ApplyAppearanceSettings)) && self.dirty {
                    match self.candidate() {
                        Ok(preferences) => {
                            out.actions
                                .push(SettingsAction::SetDesktopPreferences { preferences });
                            self.dirty = false;
                            self.invalid = false;
                        }
                        Err(_) => self.invalid = true,
                    }
                }
                frame.size_next(92.0, 30.0);
                if frame.button(i18n.text(Message::ResetAppearanceSettings)) {
                    self.reset_editor();
                }
            },
        );
    }

    fn update_settings(&mut self, snapshot: &SettingsSnapshot) {
        self.authoritative = snapshot.preferences.clone();
        if !self.dirty {
            self.reset_editor();
        }
    }
}

fn text_field(frame: &mut Frame, label: &str, id: &str, buffer: &mut TextBuf) -> bool {
    let mut changed = false;
    frame.column_ex(
        &LayoutOpts {
            gap: 4.0,
            cross: Align::Stretch,
            ..Default::default()
        },
        |frame| {
            frame.label_sized(label, 11.0);
            changed = frame.textfield(id, buffer);
        },
    );
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidate_parses_accent_and_trims_names() {
        let mut module = AppearanceModule::new();
        module.accent_color.set("#3366FF");
        module.icon_theme.set(" Papirus ");
        let preferences = module.candidate().unwrap();
        assert_eq!(preferences.accent_color.unwrap().to_hex(), "#3366FF");
        assert_eq!(preferences.icon_theme, "Papirus");
    }

    #[test]
    fn invalid_accent_does_not_form_a_transaction() {
        let mut module = AppearanceModule::new();
        module.accent_color.set("blue");
        assert!(module.candidate().is_err());
    }
}
