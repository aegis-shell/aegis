//! Settings, control panels, and form scaffolding.

use tessera_design::{Design, materials};
use lens::{Align, Frame, LayoutOpts};

/// Returns standard layout options for a settings card surface.
pub fn settings_card_layout(design: &Design) -> LayoutOpts {
    LayoutOpts {
        min_height: 96.0,
        gap: 8.0,
        pad: 15.0,
        cross: Align::Stretch,
        ..materials::card(design)
    }
}

/// Returns standard layout options for a section heading row.
pub fn section_heading_layout() -> LayoutOpts {
    LayoutOpts {
        height: 24.0,
        gap: 8.0,
        cross: Align::Center,
        ..Default::default()
    }
}

/// Render a section heading row with standard typography and alignment.
pub fn render_section_heading(frame: &mut Frame, title: &str, design: &Design) {
    frame.row_ex(&section_heading_layout(), |frame| {
        frame.label_sized(title, design.typography.headline);
    });
}

/// Render a standard unavailable/stub setting row.
pub fn render_unavailable_row(
    frame: &mut Frame,
    label: &str,
    unavailable_text: &str,
    design: &Design,
) {
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
            frame.label_sized(unavailable_text, design.typography.footnote);
        },
    );
}

/// Render a generic setting row: `[Label (+ optional Subtitle)] + [Flex Spacer] + [Control]`.
pub fn render_setting_row<F>(
    frame: &mut Frame,
    label: &str,
    subtitle: Option<&str>,
    design: &Design,
    control: F,
) where
    F: FnOnce(&mut Frame),
{
    frame.row_ex(
        &LayoutOpts {
            height: if subtitle.is_some() { 36.0 } else { 26.0 },
            gap: 8.0,
            cross: Align::Center,
            ..Default::default()
        },
        |frame| {
            frame.column_ex(
                &LayoutOpts {
                    gap: 2.0,
                    cross: Align::Start,
                    ..Default::default()
                },
                |frame| {
                    frame.label_sized(label, design.typography.label);
                    if let Some(sub) = subtitle {
                        frame.label_sized(sub, design.typography.footnote);
                    }
                },
            );
            frame.flex(1.0);
            frame.spacer(0.0);
            control(frame);
        },
    );
}

/// Render a standard card container.
pub fn render_card<F>(frame: &mut Frame, design: &Design, content: F)
where
    F: FnOnce(&mut Frame),
{
    frame.column_ex(&settings_card_layout(design), content);
}
