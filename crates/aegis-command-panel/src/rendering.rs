use super::*;

use lens::Icon;

/// A deferred row click captured during the menu frame's column closure.
pub(super) enum MenuRowAction {
    Back,
    Descend(i32),
    Click(i32),
}

pub(super) use aegis_ui::{contains, ease_out_cubic, render_disc, render_ring, stagger};

/// Two settings actions are the same kind when they mutate the same
/// settings section; the queue keeps only the newest draft of each kind
/// (instant modules emit one action per control change while dragging).
pub(super) fn same_action_kind(left: &SettingsAction, right: &SettingsAction) -> bool {
    std::mem::discriminant(left) == std::mem::discriminant(right)
}

/// The HUD flourish: short L-shaped strokes (12px arms, 1.5px thick) just
/// inside the four corners of a panel rect, like a VR visor's frame
/// markers. The color arrives unfaded — the frame's context opacity fades
/// the brackets like everything else built under it.
#[allow(dead_code)]
pub(super) fn render_corner_brackets(frame: &mut Frame, id: &str, rect: Rect, color: Color) {
    const ARM: f32 = 12.0;
    const THICK: f32 = 1.5;
    const INSET: f32 = 6.0;
    let mut bar = |suffix: &str, x: f32, y: f32, w: f32, h: f32| {
        frame.place(
            &format!("{id}-{suffix}"),
            &materials::chrome_place(
                Rect { x, y, w, h },
                LayoutOpts {
                    bg: color,
                    border: Color::TRANSPARENT,
                    radius: 0.0,
                    pad: 0.0,
                    ..materials::surface_layout()
                },
            ),
            |_| {},
        );
    };
    let left = rect.x + INSET;
    let right = rect.x + rect.w - INSET;
    let top = rect.y + INSET;
    let bottom = rect.y + rect.h - INSET;
    bar("tl-h", left, top, ARM, THICK);
    bar("tl-v", left, top, THICK, ARM);
    bar("tr-h", right - ARM, top, ARM, THICK);
    bar("tr-v", right - THICK, top, THICK, ARM);
    bar("bl-h", left, bottom - THICK, ARM, THICK);
    bar("bl-v", left, bottom - THICK, THICK, ARM);
    bar("br-h", right - ARM, bottom - THICK, ARM, THICK);
    bar("br-v", right - THICK, bottom - THICK, THICK, ARM);
}

pub(super) fn volume_icon(status: &SystemStatus) -> Icon {
    if status.muted || status.volume.unwrap_or(0) == 0 {
        Icon::VolumeMuted
    } else if status.volume.unwrap_or(0) < 55 {
        Icon::VolumeLow
    } else {
        Icon::VolumeHigh
    }
}

/// One compact Control Center tile: icon above label, with the whole rounded
/// rectangle acting as the target. Active state is communicated by both the
/// accent glyph and a selected surface, so colour is not the sole cue.
pub(super) fn render_control_tile(
    f: &mut Frame,
    id: &str,
    label: &str,
    icon: Icon,
    active: bool,
    enabled: bool,
    size: (f32, f32),
    hud: CommandPanelColors,
    type_scale: TypeScale,
) -> bool {
    let original = f.theme();
    let fg = if !enabled {
        hud.text_muted.with_alpha(110)
    } else if active {
        hud.accent
    } else {
        hud.text
    };
    f.set_theme(themes::hud(&hud).with_fg(fg));
    let (response, _) = f.pressable_row(
        id,
        label,
        &LayoutOpts {
            width: size.0,
            height: size.1,
            pad: 10.0,
            radius: 16.0,
            cross: Align::Center,
            bg: if active {
                hud.selection_surface
            } else {
                hud.surface_recessed
            },
            border: if active {
                hud.accent.with_alpha(70)
            } else {
                hud.border
            },
            border_width: 1.0,
            ..Default::default()
        },
        |f, _| {
            f.column_ex(
                &LayoutOpts {
                    width: (size.0 - 20.0).max(1.0),
                    height: (size.1 - 20.0).max(1.0),
                    gap: 6.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |f| {
                    f.flex(1.0);
                    f.spacer(0.0);
                    f.icon(icon, 20.0);
                    display_label(
                        f,
                        &truncate(label, ((size.0 - 24.0) / 6.6).max(4.0) as usize),
                        type_scale.footnote,
                    );
                    f.flex(1.0);
                    f.spacer(0.0);
                },
            );
        },
    );
    f.set_theme(original);
    response.clicked && enabled
}

/// A tall Control Center fader. The thick vertical track exposes value as a
/// filled capsule; its compact card keeps the value, control, and label on a
/// single visual axis instead of spreading them over several form rows.
pub(super) fn render_control_fader(
    f: &mut Frame,
    id: &str,
    label: &str,
    icon: Icon,
    value: Option<u8>,
    range: (u8, u8),
    size: (f32, f32),
    fill: Color,
    hud: CommandPanelColors,
    type_scale: TypeScale,
) -> Option<u8> {
    let original = f.theme();
    let enabled = value.is_some();
    let mut level = value.unwrap_or(range.0) as f32;
    let theme = themes::hud(&hud)
        .with_fg(if enabled { hud.text } else { hud.text_muted })
        .with_slider_track_color(hud.control_track)
        .with_slider_fill_color(if enabled { fill } else { hud.text_muted })
        .with_slider_knob_color(if enabled {
            hud.control_knob
        } else {
            hud.text_muted
        })
        .with_slider_track_thickness((size.0 * 0.42).clamp(38.0, 48.0))
        .with_slider_knob_size(8.0);
    f.set_theme(theme);
    let mut changed = false;
    f.column_ex(
        &LayoutOpts {
            width: size.0,
            height: size.1,
            gap: 5.0,
            pad: 10.0,
            radius: 18.0,
            bg: hud.surface_recessed,
            border: hud.border,
            border_width: 1.0,
            cross: Align::Center,
            ..Default::default()
        },
        |f| {
            display_label(
                f,
                &value
                    .map(|value| format!("{value}%"))
                    .unwrap_or_else(|| "--".to_string()),
                type_scale.footnote,
            );
            f.size_next((size.0 - 20.0).max(40.0), (size.1 - 73.0).max(80.0));
            changed = f.slider_vertical(
                &format!("##{id}"),
                &mut level,
                range.0 as f32,
                range.1 as f32,
                2.0,
            );
            f.row_ex(
                &LayoutOpts {
                    height: 24.0,
                    gap: 5.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |f| {
                    f.icon(icon, 15.0);
                    display_label(
                        f,
                        &truncate(label, ((size.0 - 38.0) / 6.4).max(4.0) as usize),
                        type_scale.footnote,
                    );
                },
            );
        },
    );
    f.set_theme(original);
    (changed && enabled).then(|| level.round().clamp(range.0 as f32, range.1 as f32) as u8)
}

// ---- dbusmenu popover helpers -------------------------------------------

/// Decorate a row label with a toggle glyph (when present) and a submenu
/// chevron suffix (when the row opens a submenu).
pub(super) fn menu_row_label(row: &MenuNode) -> String {
    let mut label = row.label.clone();
    if row.toggle.is_on() {
        let glyph = match row.toggle {
            aegis_tray::MenuToggle::Checkmark(_) => "✓ ",
            aegis_tray::MenuToggle::Radio(_) => "● ",
            _ => "",
        };
        label.insert_str(0, glyph);
    }
    if row.has_submenu {
        label.push(' ');
        label.push('▸');
    }
    label
}

/// Compute the visible popover bounds from the owner cell rect, the visible
/// rows (already truncated to the current `menu_path`), and the display. The
/// height counts every visible row + separator and the optional Back header.
pub(super) fn menu_bounds(owner: Rect, visible: &[MenuNode], display: (f32, f32)) -> Rect {
    let separator_count = visible
        .iter()
        .filter(|row| row.visible && row.kind == aegis_tray::MenuEntryKind::Separator)
        .count();
    let row_count = visible
        .iter()
        .filter(|row| row.visible && row.kind != aegis_tray::MenuEntryKind::Separator)
        .count();
    // Overestimating is safe: this is used for click-away hit-testing, and
    // the render pass recomputes the exact height from the same rows.
    let height = MENU_PAD * 2.0
        + MENU_HEADER_HEIGHT
        + row_count as f32 * MENU_ROW_HEIGHT
        + separator_count as f32 * MENU_SECTION_HEIGHT;
    place_popup(owner, (MENU_WIDTH, height), display)
}

// ---- display typography (ADR-0080 refresh) -------------------------------

/// Weight of the panel's display typography. lens draws text at the theme's
/// regular weight (`text_weight = 0`); the bold 600–700 range reads as the
/// game-HUD voice the panel wants for its labels.
pub(super) const DISPLAY_WEIGHT: f32 = 700.0;

/// A bold compact label: measures and draws at [`DISPLAY_WEIGHT`] in the
/// engine's default sans-serif family, so centered and right-aligned runs
/// stay geometrically exact. This is the panel's standard label call —
/// plain `label_compact_sized` inside the panel is a review flag.
pub(super) fn display_label(f: &mut Frame, text: &str, size: f32) {
    f.label_compact_weighted(text, size, DISPLAY_WEIGHT);
}

// ---- network surface charts ----------------------------------------------
