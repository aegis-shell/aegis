use tessera_design::Design;
use tessera_model::settings::SettingsSnapshot;
use tessera_shell::{Localizer, Message};
use lens::{Align, Frame, Icon, LayoutOpts};

use crate::module::{ModuleEvents, ModuleMetadata, SettingsModule};
use crate::ui::settings_card_layout;

/// A discoverable module whose ownership boundary is settled while its
/// authoritative backend is still absent. Keeping these pages honest makes
/// routes and navigation stable without presenting controls that cannot be
/// persisted or applied.
pub(crate) struct UnavailableModule {
    metadata: ModuleMetadata,
    description: Message,
}

impl UnavailableModule {
    pub(crate) const fn new(metadata: ModuleMetadata, description: Message) -> Self {
        Self {
            metadata,
            description,
        }
    }
}

impl SettingsModule for UnavailableModule {
    fn metadata(&self) -> ModuleMetadata {
        self.metadata
    }

    fn render(
        &mut self,
        frame: &mut Frame,
        i18n: &Localizer,
        design: &Design,
        _out: &mut ModuleEvents,
    ) {
        frame.heading(i18n.text(self.metadata.title), 2);
        frame.label_wrapped_sized(i18n.text(self.description), design.typography.label, 560.0);
        frame.column_ex(&settings_card_layout(design), |frame| {
            frame.row_ex(
                &LayoutOpts {
                    gap: 10.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |frame| {
                    frame.icon(Icon::Shield, 22.0);
                    frame.heading(i18n.text(Message::SettingsBackendUnavailable), 3);
                },
            );
            frame.label_wrapped_sized(
                i18n.text(Message::SettingsBackendUnavailableDescription),
                design.typography.footnote,
                560.0,
            );
        });
    }

    fn update_settings(&mut self, _snapshot: &SettingsSnapshot) {}
}
