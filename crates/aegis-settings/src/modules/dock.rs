use aegis_design::Design;
use aegis_model::dock::MinimizeAnimationStyle;
use aegis_model::settings::{DockSettings, SettingsAction, SettingsSnapshot};
use aegis_shell::{Localizer, Message};
use lens::{Frame, Icon};

use crate::module::{
    ApplyPolicy, ModuleAvailability, ModuleCategory, ModuleEvents, ModuleId, ModuleMetadata,
    SettingsModule,
};
use crate::ui::{section_heading_layout, settings_card_layout};

pub(crate) const DOCK_MODULE_ID: ModuleId = ModuleId::new("dock");

/// Dock presentation settings. Every control applies instantly, so the
/// module keeps no draft: it renders from the authoritative snapshot and
/// emits the full replacement on change.
pub(crate) struct DockModule {
    settings: DockSettings,
}

impl DockModule {
    pub(crate) fn new() -> Self {
        Self {
            settings: DockSettings::default(),
        }
    }
}

impl Default for DockModule {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsModule for DockModule {
    fn metadata(&self) -> ModuleMetadata {
        ModuleMetadata {
            id: DOCK_MODULE_ID,
            title: Message::Dock,
            icon: Icon::Sidebar,
            category: ModuleCategory::Personalization,
            keywords: &["dock", "minimize", "animation", "genie", "scale", "suck"],
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
        let mut settings = self.settings;
        let mut changed = false;

        frame.heading(i18n.text(Message::Dock), 2);
        frame.label_sized(i18n.text(Message::DockDescription), design.typography.label);

        frame.column_ex(&settings_card_layout(design), |frame| {
            frame.row_ex(&section_heading_layout(), |frame| {
                frame.label_sized(
                    i18n.text(Message::MinimizeAnimation),
                    design.typography.label,
                );
                frame.flex(1.0);
                frame.spacer(0.0);
                let mut selected = match settings.minimize_animation {
                    MinimizeAnimationStyle::Genie => 0,
                    MinimizeAnimationStyle::Scale => 1,
                    MinimizeAnimationStyle::Suck => 2,
                };
                frame.size_next(180.0, 30.0);
                if frame.dropdown(
                    "##settings-dock-minimize-animation",
                    &mut selected,
                    &[
                        i18n.text(Message::MinimizeAnimationGenie),
                        i18n.text(Message::MinimizeAnimationScale),
                        i18n.text(Message::MinimizeAnimationSuck),
                    ],
                ) {
                    settings.minimize_animation = match selected {
                        1 => MinimizeAnimationStyle::Scale,
                        2 => MinimizeAnimationStyle::Suck,
                        _ => MinimizeAnimationStyle::Genie,
                    };
                    changed = true;
                }
            });
        });

        if changed {
            self.settings = settings;
            out.actions.push(SettingsAction::SetDock { settings });
        }
    }

    fn update_settings(&mut self, snapshot: &SettingsSnapshot) {
        self.settings = snapshot.dock;
    }
}
