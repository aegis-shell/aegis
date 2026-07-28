use super::*;
use std::ffi::c_void;

/// A deferred row click captured during the menu frame's column closure.
pub(super) enum MenuRowAction {
    Back,
    Descend(i32),
    Click(i32),
}

/// Permanent Fuji entry plus the live Agent Realm state it summarizes.
pub(super) struct AgentIndicator {
    pub(super) label: String,
    pub(super) state: AgentIndicatorState,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum AgentIndicatorState {
    Ready,
    Active,
    Paused,
}

pub(super) fn agent_indicator(snapshot: &RealmSnapshot, i18n: &Localizer) -> AgentIndicator {
    let live = snapshot
        .realms
        .iter()
        .filter(|realm| realm.kind == RealmKind::Agent && realm.state != RealmState::Revoked)
        .collect::<Vec<_>>();
    let active = live.iter().any(|realm| realm.state == RealmState::Active);
    let state = match live.as_slice() {
        [] => AgentIndicatorState::Ready,
        _ if active => AgentIndicatorState::Active,
        _ => AgentIndicatorState::Paused,
    };
    let label = match live.as_slice() {
        [] => i18n.text(Message::Fuji).to_string(),
        [realm] => format!(
            "{} · {}",
            realm.label,
            if active {
                i18n.text(Message::RealmActive)
            } else {
                i18n.text(Message::RealmPaused)
            }
        ),
        realms => format!(
            "AI {} · {}",
            realms.len(),
            if active {
                i18n.text(Message::RealmActive)
            } else {
                i18n.text(Message::RealmPaused)
            }
        ),
    };
    AgentIndicator { label, state }
}

pub(super) fn render_agent_indicator(
    frame: &mut Frame,
    rect: Rect,
    indicator: &AgentIndicator,
    hovered: bool,
) {
    let accent = indicator_accent(indicator.state);
    frame.layer(
        "aegis-hud-agent-indicator",
        rect,
        &OverlayOpts {
            bg: if hovered {
                Color::rgba(72, 100, 146, 112)
            } else {
                Color::rgba(42, 55, 80, 92)
            },
            border: Color::rgba(116, 151, 206, if hovered { 150 } else { 92 }),
            border_width: 1.0,
            radius: 9.0,
            ..Default::default()
        },
        |_| {},
    );
    let dot = Rect {
        x: rect.x + 9.0,
        y: rect.y + (rect.h - 10.0) * 0.5,
        w: 10.0,
        h: 10.0,
    };
    frame.layer(
        "aegis-hud-fuji-entry-orb",
        dot,
        &OverlayOpts {
            bg: accent.with_alpha(66),
            border: accent,
            border_width: 1.0,
            radius: dot.w * 0.5,
            ..Default::default()
        },
        |_| {},
    );
    render_text_left(
        frame,
        "aegis-hud-fuji-entry-label",
        Rect {
            x: rect.x + 24.0,
            y: rect.y,
            w: (rect.w - 30.0).max(1.0),
            h: rect.h,
        },
        &truncate(&indicator.label, ((rect.w - 30.0) / 6.8).max(4.0) as usize),
        10.5,
    );
}

pub(super) fn indicator_accent(state: AgentIndicatorState) -> Color {
    match state {
        AgentIndicatorState::Ready => Color::rgba(173, 119, 255, 255),
        AgentIndicatorState::Active => Color::rgba(82, 193, 255, 255),
        AgentIndicatorState::Paused => Color::rgba(240, 184, 84, 255),
    }
}

/// Draw a seek-safe, compositor-owned "algorithm core": layered Siri-like
/// colour fields, orbiting inference nodes, and a responsive signal strip.
/// It is presentation only; fuji's model and credentials remain out of
/// process behind the existing Agent/Realm boundary.
pub(super) fn render_fuji_algorithm(
    frame: &mut Frame,
    rect: Rect,
    phase: f32,
    progress: f32,
    active: bool,
    reduced_motion: bool,
) {
    let phase = if reduced_motion { 0.82 } else { phase };
    let diameter = (rect.w * 0.74).min(rect.h * 0.62).clamp(42.0, 94.0);
    let center = (
        rect.x + rect.w * 0.5,
        rect.y + (rect.h * 0.48).min(rect.h - 27.0),
    );
    let energy = if active { 1.0 } else { 0.72 };
    let breathe = 1.0 + phase.sin() * 0.045 * energy;

    render_disc(
        frame,
        "aegis-hud-fuji-glow",
        center,
        diameter * 1.48 * breathe,
        Color::rgba(78, 83, 255, fade_alpha(34, progress)),
    );
    render_ring(
        frame,
        "aegis-hud-fuji-orbit-outer",
        center,
        diameter * 1.18,
        Color::rgba(101, 220, 255, fade_alpha(78, progress)),
        1.0,
    );
    render_ring(
        frame,
        "aegis-hud-fuji-orbit-inner",
        center,
        diameter * 0.86,
        Color::rgba(215, 111, 255, fade_alpha(96, progress)),
        1.0,
    );

    let core_layers = [
        (
            diameter * 0.68,
            Color::rgba(72, 105, 255, fade_alpha(224, progress)),
            0.0_f32,
        ),
        (
            diameter * 0.54,
            Color::rgba(190, 77, 255, fade_alpha(206, progress)),
            2.1,
        ),
        (
            diameter * 0.38,
            Color::rgba(255, 91, 184, fade_alpha(194, progress)),
            4.2,
        ),
        (
            diameter * 0.22,
            Color::rgba(116, 241, 255, fade_alpha(238, progress)),
            5.3,
        ),
    ];
    for (index, (size, color, offset)) in core_layers.into_iter().enumerate() {
        let drift = diameter * 0.065 * energy;
        let layer_center = (
            center.0 + (phase * 1.25 + offset).cos() * drift,
            center.1 + (phase * 1.55 + offset).sin() * drift,
        );
        render_disc(
            frame,
            &format!("aegis-hud-fuji-core-{index}"),
            layer_center,
            size * breathe,
            color,
        );
    }

    for index in 0..7 {
        let offset = index as f32 * std::f32::consts::TAU / 7.0;
        let angle = phase * (0.58 + index as f32 * 0.025) + offset;
        let radius = diameter * (0.49 + (index % 2) as f32 * 0.09);
        let node_center = (
            center.0 + angle.cos() * radius,
            center.1 + angle.sin() * radius * 0.58,
        );
        let node_size = 3.2 + (index % 3) as f32 * 1.25;
        let color = match index % 3 {
            0 => Color::rgba(91, 226, 255, fade_alpha(220, progress)),
            1 => Color::rgba(187, 112, 255, fade_alpha(212, progress)),
            _ => Color::rgba(255, 117, 198, fade_alpha(204, progress)),
        };
        render_disc(
            frame,
            &format!("aegis-hud-fuji-node-{index}"),
            node_center,
            node_size,
            color,
        );
    }

    let bar_count = 9;
    let strip_w = (rect.w * 0.62).min(76.0);
    let bar_w = 3.0;
    let gap = (strip_w - bar_count as f32 * bar_w) / (bar_count - 1) as f32;
    let strip_x = center.0 - strip_w * 0.5;
    let baseline = rect.y + rect.h - 8.0;
    for index in 0..bar_count {
        let wave = ((phase * 2.6 + index as f32 * 0.72).sin() * 0.5 + 0.5) * energy;
        let height = 3.0 + wave * 12.0;
        let bar = Rect {
            x: strip_x + index as f32 * (bar_w + gap),
            y: baseline - height,
            w: bar_w,
            h: height,
        };
        frame.layer(
            &format!("aegis-hud-fuji-signal-{index}"),
            bar,
            &OverlayOpts {
                bg: Color::rgba(
                    119 + (index as u8 * 9).min(80),
                    141,
                    255,
                    fade_alpha(196, progress),
                ),
                border: Color::TRANSPARENT,
                radius: bar_w * 0.5,
                ..Default::default()
            },
            |_| {},
        );
    }
}

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

/// The current local time as `HH:MM` — the same string `date +%H:%M`
/// produced, but resolved in-process so the render thread never forks.
/// `localtime_r` is the thread-safe local-time breakdown.
pub(super) fn local_clock() -> Option<String> {
    // SAFETY: `time` writes a valid `time_t` into `now`, and `localtime_r`
    // either writes a valid `tm` into `broken` and returns a pointer to it or
    // returns null.
    unsafe {
        let mut now: libc::time_t = 0;
        libc::time(&mut now);
        let mut broken: libc::tm = std::mem::zeroed();
        if libc::localtime_r(&now, &mut broken).is_null() {
            return None;
        }
        Some(format!("{:02}:{:02}", broken.tm_hour, broken.tm_min))
    }
}

// ---- dbusmenu popover helpers -------------------------------------------

/// Decorate a row label with a toggle glyph (when present) and a submenu
/// chevron suffix (when the row opens a submenu).
pub(super) fn menu_row_label(row: &MenuNode) -> String {
    let mut label = row.label.clone();
    if row.toggle.is_on() {
        let glyph = match row.toggle {
            crate::tray::MenuToggle::Checkmark(_) => "✓ ",
            crate::tray::MenuToggle::Radio(_) => "● ",
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
        .filter(|row| row.visible && row.kind == crate::tray::MenuEntryKind::Separator)
        .count();
    let row_count = visible
        .iter()
        .filter(|row| row.visible && row.kind != crate::tray::MenuEntryKind::Separator)
        .count();
    let header_count = usize::from(row_count > 0 && separator_count + row_count > 0);
    // The Back header only appears once we descend into a submenu; at root
    // it is not rendered. Detect "non-root view" via owner — we don't know
    // the path here, so the caller may pass a slightly larger conservative
    // height. The actual height is recomputed during render; this function is
    // used for click-away hit-testing only, so overestimating is safe.
    let _ = header_count;
    let height = MENU_PAD * 2.0
        + MENU_HEADER_HEIGHT
        + row_count as f32 * MENU_ROW_HEIGHT
        + separator_count as f32 * MENU_SECTION_HEIGHT;
    place_popup(owner, (MENU_WIDTH, height), display)
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

pub(super) fn battery_icon_name(battery: BatteryStatus) -> String {
    let mut level = ((battery.percent as u16 + 5) / 10 * 10).min(100) as u8;
    // Adwaita (and several inherited themes) represents a full charging
    // battery with a distinct "charged" name. Keep the regular charging
    // family for the status bar and use its visually identical 90% endpoint.
    if battery.charging && level == 100 {
        level = 90;
    }
    if battery.charging {
        format!("battery-level-{level}-charging-symbolic")
    } else {
        format!("battery-level-{level}-symbolic")
    }
}

pub(super) fn take_right(x: &mut f32, width: f32) -> Rect {
    *x -= width;
    let rect = Rect {
        x: *x,
        y: 0.0,
        w: width,
        h: HUD_HEIGHT,
    };
    *x -= 2.0;
    rect
}

/// Decide the fold for `items` registered SNI entries given `max` slots
/// (assumes `max >= 1`). Within budget everything renders. Past it, one slot
/// is reserved for the "+N" indicator.
pub(super) fn fold_tray(items: usize, max: usize) -> TrayFold {
    if items <= max {
        return TrayFold {
            visible: items,
            hidden: 0,
        };
    }
    let visible = max.saturating_sub(1);
    TrayFold {
        visible,
        hidden: items - visible,
    }
}

pub(super) fn render_text(f: &mut Frame, id: &str, rect: Rect, text: &str, size: f32) {
    f.layer(id, rect, &centered_layer(), |f| {
        f.row_ex(
            &LayoutOpts {
                width: rect.w,
                height: rect.h,
                cross: Align::Center,
                ..Default::default()
            },
            |f| f.label_compact_sized(text, size),
        );
    });
}

pub(super) fn render_text_left(f: &mut Frame, id: &str, rect: Rect, text: &str, size: f32) {
    f.layer(id, rect, &centered_layer(), |f| {
        f.row_ex(
            &LayoutOpts {
                width: rect.w,
                height: rect.h,
                cross: Align::Center,
                ..Default::default()
            },
            |f| f.label_compact_sized(text, size),
        );
    });
}

pub(super) fn render_icon_button(
    f: &mut Frame,
    id: &str,
    rect: Rect,
    themed_icon: Option<*mut c_void>,
    fallback: Icon,
    label: &str,
    hovered: bool,
) {
    f.layer(id, rect, &icon_button_opts(hovered), |f| {
        f.row_ex(
            &LayoutOpts {
                width: rect.w,
                height: rect.h,
                gap: if label.is_empty() { 0.0 } else { 4.0 },
                cross: Align::Center,
                ..Default::default()
            },
            |f| {
                match themed_icon {
                    Some(icon) => unsafe {
                        f.image(icon as *mut lens::sys::flux_image, 16.0, 16.0)
                    },
                    None => f.icon(fallback, 15.0),
                }
                if !label.is_empty() {
                    f.label_compact_sized(label, 11.0);
                }
            },
        );
    });
}

pub(super) fn unavailable_control(f: &mut Frame, label: &str, i18n: &Localizer) {
    f.row_ex(
        &LayoutOpts {
            height: 20.0,
            gap: 6.0,
            cross: Align::Center,
            ..Default::default()
        },
        |f| {
            f.label_compact_sized(label, 10.5);
            f.flex(1.0);
            f.spacer(0.0);
            f.label_compact_sized(i18n.text(Message::Unavailable), 10.0);
        },
    );
}

pub(super) fn bar_opts() -> OverlayOpts {
    OverlayOpts {
        // The desktop capture underneath provides the blur; this layer is
        // only the neutral macOS-style material tint, shared with the dock.
        bg: Color::rgba(24, 26, 36, 148),
        border: Color::TRANSPARENT,
        border_width: 0.0,
        radius: 0.0,
        pad: 0.0,
        ..Default::default()
    }
}

pub(super) fn workspace_dot_color(active: bool, hovered: bool) -> Color {
    if active {
        Color::rgba(248, 248, 250, 248)
    } else if hovered {
        Color::rgba(235, 235, 240, 166)
    } else {
        Color::rgba(225, 225, 232, 78)
    }
}

pub(super) fn workspace_dot_diameter(active: bool, hovered: bool) -> f32 {
    if active {
        WORKSPACE_ACTIVE_DOT
    } else if hovered {
        WORKSPACE_INACTIVE_DOT + 1.0
    } else {
        WORKSPACE_INACTIVE_DOT
    }
}

pub(super) fn icon_button_opts(hovered: bool) -> OverlayOpts {
    OverlayOpts {
        bg: if hovered {
            Color::rgba(255, 255, 255, 24)
        } else {
            Color::TRANSPARENT
        },
        border: if hovered {
            Color::rgba(255, 255, 255, 18)
        } else {
            Color::TRANSPARENT
        },
        border_width: if hovered { 1.0 } else { 0.0 },
        radius: 8.0,
        pad: 3.0,
        cross: Align::Center,
        ..Default::default()
    }
}

pub(super) fn panel_opts() -> OverlayOpts {
    OverlayOpts {
        bg: Color::rgba(24, 26, 36, 174),
        border: Color::rgba(255, 255, 255, 46),
        border_width: 1.0,
        radius: 20.0,
        pad: 0.0,
        ..Default::default()
    }
}

pub(super) fn card_opts(hovered: bool) -> OverlayOpts {
    OverlayOpts {
        bg: if hovered {
            Color::rgba(255, 255, 255, 28)
        } else {
            Color::rgba(255, 255, 255, 15)
        },
        border: Color::rgba(255, 255, 255, 26),
        border_width: 1.0,
        radius: 14.0,
        pad: 0.0,
        ..Default::default()
    }
}

pub(super) fn small_card_opts(hovered: bool) -> OverlayOpts {
    OverlayOpts {
        radius: 11.0,
        ..card_opts(hovered)
    }
}

pub(super) fn centered_layer() -> OverlayOpts {
    OverlayOpts {
        bg: Color::TRANSPARENT,
        border: Color::TRANSPARENT,
        border_width: 0.0,
        radius: 0.0,
        pad: 0.0,
        cross: Align::Center,
        ..Default::default()
    }
}

pub(super) fn sized(w: f32, h: f32) -> LayoutOpts {
    LayoutOpts {
        width: w,
        height: h,
        ..Default::default()
    }
}

pub(super) fn sized_fill(w: f32, h: f32, bg: Color, radius: f32) -> LayoutOpts {
    LayoutOpts {
        width: w,
        height: h,
        bg,
        radius,
        ..Default::default()
    }
}

pub(super) fn contains(rect: Rect, x: f32, y: f32) -> bool {
    x >= rect.x && y >= rect.y && x < rect.x + rect.w && y < rect.y + rect.h
}

pub(super) fn ease_out_cubic(value: f32) -> f32 {
    let inverse = 1.0 - value.clamp(0.0, 1.0);
    1.0 - inverse * inverse * inverse
}

pub(super) fn fade_alpha(base: u8, progress: f32) -> u8 {
    (base as f32 * progress.clamp(0.0, 1.0)).round() as u8
}

pub(super) fn faded_theme(theme: Theme, progress: f32) -> Theme {
    let fade = |color: Color| {
        let (_, _, _, opacity) = color.components();
        color.with_alpha(fade_alpha(opacity, progress))
    };
    theme
        .with_fg(fade(theme.fg()))
        .with_accent(fade(theme.accent()))
        .with_border(fade(theme.border()))
        .with_hover(fade(theme.hover()))
        .with_active(fade(theme.active()))
        .with_disabled(fade(theme.disabled()))
        .with_error(fade(theme.error()))
}
