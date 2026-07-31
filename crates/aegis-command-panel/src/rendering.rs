use super::*;

/// A deferred row click captured during the menu frame's column closure.
pub(super) enum MenuRowAction {
    Back,
    Descend(i32),
    Click(i32),
}

pub(super) fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.w && y < rect.y + rect.h
}

pub(super) fn ease_out_cubic(value: f32) -> f32 {
    let inverse = 1.0 - value.clamp(0.0, 1.0);
    1.0 - inverse * inverse * inverse
}

/// Per-panel reveal after a stagger delay: 0 until `reveal` passes `delay`,
/// then ramping to 1 at full reveal.
pub(super) fn stagger(reveal: f32, delay: f32) -> f32 {
    ((reveal - delay) / (1.0 - delay)).clamp(0.0, 1.0)
}

pub(super) fn fade_alpha(base: u8, progress: f32) -> u8 {
    (base as f32 * progress.clamp(0.0, 1.0)).round() as u8
}

/// Scale one color's alpha by the reveal progress (lens has no opacity
/// property; fading is per-color).
pub(super) fn fade_color(color: Color, progress: f32) -> Color {
    let (_, _, _, opacity) = color.components();
    color.with_alpha(fade_alpha(opacity, progress))
}

pub(super) fn faded_theme(theme: Theme, progress: f32) -> Theme {
    let fade = |color: Color| fade_color(color, progress);
    theme
        .with_fg(fade(theme.fg()))
        .with_accent(fade(theme.accent()))
        .with_border(fade(theme.border()))
        .with_hover(fade(theme.hover()))
        .with_active(fade(theme.active()))
        .with_disabled(fade(theme.disabled()))
        .with_error(fade(theme.error()))
}

pub(super) fn sized(w: f32, h: f32) -> LayoutOpts {
    LayoutOpts {
        width: w,
        height: h,
        ..Default::default()
    }
}

/// An invisible layer used purely as a layout anchor for its children.
pub(super) fn transparent() -> OverlayOpts {
    OverlayOpts {
        bg: Color::TRANSPARENT,
        border: Color::TRANSPARENT,
        border_width: 0.0,
        radius: 0.0,
        pad: 0.0,
        ..Default::default()
    }
}

/// Filled circle used for the SAO menu's ringed section icons.
pub(super) fn render_disc(
    frame: &mut Frame,
    id: &str,
    center: (f32, f32),
    diameter: f32,
    color: Color,
) {
    let rect = Rect {
        x: center.0 - diameter * 0.5,
        y: center.1 - diameter * 0.5,
        w: diameter,
        h: diameter,
    };
    frame.layer(
        id,
        rect,
        &OverlayOpts {
            bg: color,
            border: Color::TRANSPARENT,
            radius: diameter * 0.5,
            ..Default::default()
        },
        |_| {},
    );
}

/// Ring (hollow circle) for unselected SAO section icons.
pub(super) fn render_ring(
    frame: &mut Frame,
    id: &str,
    center: (f32, f32),
    diameter: f32,
    color: Color,
    width: f32,
) {
    let rect = Rect {
        x: center.0 - diameter * 0.5,
        y: center.1 - diameter * 0.5,
        w: diameter,
        h: diameter,
    };
    frame.layer(
        id,
        rect,
        &OverlayOpts {
            bg: Color::TRANSPARENT,
            border: color,
            border_width: width,
            radius: diameter * 0.5,
            ..Default::default()
        },
        |_| {},
    );
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

pub(super) fn volume_icon_name(status: &SystemStatus) -> &'static str {
    if status.muted || status.volume.unwrap_or(0) == 0 {
        "audio-volume-muted-symbolic"
    } else if status.volume.unwrap_or(0) < 34 {
        "audio-volume-low-symbolic"
    } else if status.volume.unwrap_or(0) < 67 {
        "audio-volume-medium-symbolic"
    } else {
        "audio-volume-high-symbolic"
    }
}

pub(super) fn network_icon_name(network: NetworkState) -> &'static str {
    match network {
        NetworkState::Wifi => "network-wireless-signal-excellent-symbolic",
        NetworkState::Wired => "network-wired-symbolic",
        NetworkState::Offline => "network-offline-symbolic",
    }
}

pub(super) fn network_text(status: &SystemStatus, i18n: &Localizer) -> &'static str {
    match status.network {
        NetworkState::Wifi => i18n.text(Message::WifiConnected),
        NetworkState::Wired => i18n.text(Message::WiredConnected),
        NetworkState::Offline => i18n.text(Message::Disconnected),
    }
}

/// An unavailable-on-this-host control row: the label plus a muted
/// "Unavailable" marker, matching the status bar's old panel.
pub(super) fn unavailable_control(f: &mut Frame, label: &str, i18n: &Localizer) {
    f.row_ex(
        &LayoutOpts {
            height: 20.0,
            gap: 6.0,
            cross: Align::Center,
            ..Default::default()
        },
        |f| {
            f.label_compact_sized(label, 11.0);
            f.flex(1.0);
            f.spacer(0.0);
            f.label_compact_sized(i18n.text(Message::Unavailable), 10.5);
        },
    );
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

/// The Agent Workspaces status row's content: the aggregate Agent Realm
/// state it summarizes (ported from the HUD's dropped right chip, ADR-0083).
pub(super) struct AgentWorkspaceIndicator {
    pub(super) label: String,
    pub(super) state: AgentWorkspaceState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentWorkspaceState {
    Idle,
    Active,
    Paused,
    PartiallyPaused,
}

pub(super) fn agent_workspace_indicator(
    snapshot: &RealmSnapshot,
    i18n: &Localizer,
) -> AgentWorkspaceIndicator {
    let live = snapshot
        .realms
        .iter()
        .filter(|realm| realm.kind == RealmKind::Agent && realm.state != RealmState::Revoked)
        .collect::<Vec<_>>();
    let active_count = live
        .iter()
        .filter(|realm| realm.state == RealmState::Active)
        .count();
    let state = match live.as_slice() {
        [] => AgentWorkspaceState::Idle,
        _ if active_count == live.len() => AgentWorkspaceState::Active,
        _ if active_count == 0 => AgentWorkspaceState::Paused,
        _ => AgentWorkspaceState::PartiallyPaused,
    };
    let state_label = agent_workspace_state_label(state, i18n);
    let label = match live.as_slice() {
        [] => i18n.text(Message::AiWorkspaces).to_string(),
        [realm] => format!("{} · {state_label}", realm.label),
        realms => format!(
            "{} · {state_label}",
            i18n.agent_workspace_count(realms.len())
        ),
    };
    AgentWorkspaceIndicator { label, state }
}

pub(super) fn agent_workspace_state_label(
    state: AgentWorkspaceState,
    i18n: &Localizer,
) -> &'static str {
    match state {
        AgentWorkspaceState::Idle => i18n.text(Message::NoActiveAgentWorkspaces),
        AgentWorkspaceState::Active => i18n.text(Message::RealmActive),
        AgentWorkspaceState::Paused => i18n.text(Message::RealmPaused),
        AgentWorkspaceState::PartiallyPaused => i18n.text(Message::AgentWorkspacesPartiallyPaused),
    }
}
