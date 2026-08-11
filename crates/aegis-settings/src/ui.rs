use aegis_design::{Design, materials};
use aegis_shell::{Localizer, Message};
use lens::{Align, Frame, LayoutOpts};

pub(crate) fn unavailable_row(frame: &mut Frame, label: &str, i18n: &Localizer, design: &Design) {
    frame.row_ex(
        &LayoutOpts {
            height: 26.0,
            gap: 8.0,
            cross: Align::Center,
            ..Default::default()
        },
        |frame| {
            frame.label_sized(label, design.typography.label);
            frame.flex(1.0);
            frame.spacer(0.0);
            frame.label_sized(i18n.text(Message::Unavailable), design.typography.footnote);
        },
    );
}

pub(crate) fn settings_card_layout(design: &Design) -> LayoutOpts {
    LayoutOpts {
        min_height: 96.0,
        gap: 8.0,
        pad: 15.0,
        cross: Align::Stretch,
        ..materials::card(design)
    }
}

pub(crate) fn section_heading_layout() -> LayoutOpts {
    LayoutOpts {
        height: 24.0,
        gap: 8.0,
        cross: Align::Center,
        ..Default::default()
    }
}
