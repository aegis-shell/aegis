//! Modal dialog scaffolding, standard action buttons, and keyboard traps.

use aegis_design::{Design, materials};
use aegis_model::input::KeyAction;
use lens::{Frame, LayoutOpts, Rect};

use crate::geom::contains;

/// Standard modal dialog width for Aegis prompts.
pub const DEFAULT_MODAL_WIDTH: f32 = 460.0;
/// Standard modal inner padding.
pub const DEFAULT_MODAL_PAD: f32 = 16.0;
/// Standard title row height.
pub const DEFAULT_TITLE_HEIGHT: f32 = 24.0;
/// Standard action button height.
pub const DEFAULT_BUTTON_HEIGHT: f32 = 30.0;
/// Standard action button width for binary yes/no dialogs.
pub const DEFAULT_BUTTON_WIDTH: f32 = 96.0;
/// Standard Gaussian blur sigma for modal backdrop regions.
pub const DEFAULT_BACKDROP_BLUR_SIGMA: f32 = 18.0;

/// Standard labels for the four ADR-0088 runtime grant persistence levels:
/// Deny, Allow once, This session, Always.
pub const GRANT_LABELS: [&str; 4] = ["Deny", "Allow once", "This session", "Always"];
/// The index of the default affirmative grant option ("Allow once").
pub const GRANT_ACCENT_INDEX: usize = 1;

/// Style variant for action buttons.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ActionButtonStyle {
    /// Default surface background (unaccented).
    #[default]
    Default,
    /// Accented affirmative button (e.g. primary action or "Allow once").
    Accented,
    /// Subtle/card surface background for secondary actions (e.g. "Cancel").
    Subtle,
    /// Destructive/critical action highlight.
    Destructive,
}

/// Place the full-display darkened scrim behind a modal surface.
pub fn place_modal_scrim(frame: &mut Frame, id: &str, display: (f32, f32), design: &Design) {
    frame.place(
        id,
        &materials::chrome_place(
            Rect {
                x: 0.0,
                y: 0.0,
                w: display.0,
                h: display.1,
            },
            LayoutOpts {
                bg: design.colors.modal_scrim,
                ..materials::surface_layout()
            },
        ),
        |_| {},
    );
}

/// Place the modal's Liquid Glass background panel.
pub fn place_modal_panel(frame: &mut Frame, id: &str, panel_rect: Rect, design: &Design) {
    frame.place(
        id,
        &materials::chrome_place(panel_rect, materials::glass_panel(design)),
        |_| {},
    );
}

/// Render a modal title in a single line.
pub fn render_dialog_title(frame: &mut Frame, id: &str, rect: Rect, title: &str, design: &Design) {
    frame.place(
        id,
        &materials::chrome_place(rect, materials::transparent()),
        |frame| {
            frame.centered(rect.w, rect.h, |frame| {
                frame.label_compact_sized(title, design.typography.headline);
            });
        },
    );
}

/// Render a single styled action button.
pub fn render_action_button(
    frame: &mut Frame,
    id: &str,
    rect: Rect,
    label: &str,
    style: ActionButtonStyle,
    is_hovered: bool,
    design: &Design,
) {
    let bg = match style {
        ActionButtonStyle::Accented => {
            if is_hovered {
                design.colors.application_surface_active
            } else {
                design.colors.application_accent
            }
        }
        ActionButtonStyle::Subtle | ActionButtonStyle::Default => {
            if is_hovered {
                design.colors.application_surface_hover
            } else {
                design.colors.card_surface
            }
        }
        ActionButtonStyle::Destructive => {
            if is_hovered {
                design.colors.critical
            } else {
                design.colors.card_surface
            }
        }
    };

    frame.place(
        id,
        &materials::chrome_place(
            rect,
            LayoutOpts {
                bg,
                radius: design.radii.control,
                pad: 0.0,
                ..materials::surface_layout()
            },
        ),
        |frame| {
            frame.centered(rect.w, rect.h, |frame| {
                frame.label_compact_sized(label, design.typography.body);
            });
        },
    );
}

/// Render standard binary dialog actions (e.g. Cancel on the left, OK/Allow on the right).
pub fn render_dialog_actions_two_button(
    frame: &mut Frame,
    base_id: &str,
    cancel_rect: Rect,
    confirm_rect: Rect,
    cancel_label: &str,
    confirm_label: &str,
    cursor: (f32, f32),
    design: &Design,
) {
    let cancel_hovered = contains(cancel_rect, cursor.0, cursor.1);
    let confirm_hovered = contains(confirm_rect, cursor.0, cursor.1);

    render_action_button(
        frame,
        &format!("{base_id}-cancel"),
        cancel_rect,
        cancel_label,
        ActionButtonStyle::Subtle,
        cancel_hovered,
        design,
    );

    render_action_button(
        frame,
        &format!("{base_id}-confirm"),
        confirm_rect,
        confirm_label,
        ActionButtonStyle::Accented,
        confirm_hovered,
        design,
    );
}

/// Render the four standard ADR-0088 runtime grant buttons:
/// [Deny] [Allow once] [This session] [Always].
pub fn render_grant_action_buttons(
    frame: &mut Frame,
    base_id: &str,
    rects: &[Rect; 4],
    cursor: (f32, f32),
    design: &Design,
) {
    for (index, (&rect, &label)) in rects.iter().zip(GRANT_LABELS.iter()).enumerate() {
        let is_hovered = contains(rect, cursor.0, cursor.1);
        let style = if index == GRANT_ACCENT_INDEX {
            ActionButtonStyle::Accented
        } else {
            ActionButtonStyle::Subtle
        };
        render_action_button(
            frame,
            &format!("{base_id}-grant-{index}"),
            rect,
            label,
            style,
            is_hovered,
            design,
        );
    }
}

/// Returns true if the given key action represents an affirmative confirmation (Enter/Return).
pub fn is_confirm_key(action: &KeyAction) -> bool {
    matches!(action, KeyAction::Enter)
}

/// Returns true if the given key action represents a cancellation/dismissal (Escape).
pub fn is_cancel_key(action: &KeyAction) -> bool {
    matches!(action, KeyAction::Escape)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_key_classification() {
        assert!(is_confirm_key(&KeyAction::Enter));
        assert!(!is_confirm_key(&KeyAction::Escape));
        assert!(!is_confirm_key(&KeyAction::Tab));

        assert!(is_cancel_key(&KeyAction::Escape));
        assert!(!is_cancel_key(&KeyAction::Enter));
    }
}
