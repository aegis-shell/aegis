use super::*;

use std::collections::VecDeque;

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

/// The network surface's identity line: the live interface, plus the SSID
/// when the link is wireless. `wlan0 · Homelab-5G` for Wi-Fi, `enp3s0` for
/// wired, a muted "Offline" when no link is up.
pub(super) fn network_identity(status: &SystemStatus, i18n: &Localizer) -> String {
    if status.network_interface.is_empty() {
        return i18n.text(Message::Offline).to_owned();
    }
    match (status.network, &status.wifi_ssid) {
        (NetworkState::Wifi, Some(ssid)) if !ssid.is_empty() => {
            format!("{} · {}", status.network_interface, ssid)
        }
        _ => status.network_interface.clone(),
    }
}

/// An unavailable-on-this-host control row: the label plus a muted
/// "Unavailable" marker, matching the status bar's old panel.
pub(super) fn unavailable_control(
    f: &mut Frame,
    label: &str,
    i18n: &Localizer,
    type_scale: TypeScale,
) {
    f.row_ex(
        &LayoutOpts {
            height: 20.0,
            gap: 6.0,
            cross: Align::Center,
            ..Default::default()
        },
        |f| {
            display_label(f, label, type_scale.footnote);
            f.flex(1.0);
            f.spacer(0.0);
            display_label(f, i18n.text(Message::Unavailable), type_scale.footnote);
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

/// Fixed-capacity ring of utilization samples for the header sparklines:
/// pushing past `cap` evicts the oldest sample.
pub(super) struct History {
    samples: VecDeque<f32>,
    cap: usize,
}

impl History {
    pub(super) fn new(cap: usize) -> History {
        History {
            samples: VecDeque::with_capacity(cap),
            cap,
        }
    }

    pub(super) fn push(&mut self, value: f32) {
        if self.cap == 0 {
            return;
        }
        while self.samples.len() >= self.cap {
            self.samples.pop_front();
        }
        self.samples.push_back(value);
    }

    #[cfg(test)]
    pub(super) fn len(&self) -> usize {
        self.samples.len()
    }

    #[cfg(test)]
    pub(super) fn newest(&self) -> Option<f32> {
        self.samples.back().copied()
    }

    #[allow(dead_code)]
    pub(super) fn samples(&self) -> impl Iterator<Item = f32> + '_ {
        self.samples.iter().copied()
    }
}

/// A thin horizontal gauge: rounded track with an accent fill of
/// `fraction * width`.
#[allow(dead_code)]
pub(super) fn gauge_bar(f: &mut Frame, id: &str, rect: Rect, fraction: f32) {
    let hud = Hud::classic();
    f.place(
        id,
        &materials::chrome_place(
            rect,
            LayoutOpts {
                bg: hud.track,
                border: Color::TRANSPARENT,
                radius: rect.h * 0.5,
                pad: 0.0,
                ..materials::surface_layout()
            },
        ),
        |_| {},
    );
    let fill_w = rect.w * fraction.clamp(0.0, 1.0);
    if fill_w >= 0.5 {
        f.place(
            &format!("{id}-fill"),
            &materials::chrome_place(
                Rect { w: fill_w, ..rect },
                LayoutOpts {
                    bg: hud.accent,
                    border: Color::TRANSPARENT,
                    radius: rect.h * 0.5,
                    pad: 0.0,
                    ..materials::surface_layout()
                },
            ),
            |_| {},
        );
    }
}

/// A btop-style history strip: thin vertical bars right-aligned in `rect`,
/// newest sample at the right, height proportional to the 0..=100 sample.
#[allow(dead_code)]
pub(super) fn render_sparkline(f: &mut Frame, metric: &str, history: &History, rect: Rect) {
    const BAR_W: f32 = 2.0;
    const BAR_GAP: f32 = 1.5;
    let hud = Hud::classic();
    let max_bars = ((rect.w + BAR_GAP) / (BAR_W + BAR_GAP)).floor().max(0.0) as usize;
    let samples: Vec<f32> = history.samples().collect();
    let count = samples.len().min(max_bars);
    for (index, sample) in samples[samples.len() - count..].iter().enumerate() {
        let h = (rect.h * (sample / 100.0).clamp(0.0, 1.0))
            .max(1.5)
            .min(rect.h);
        let right = rect.x + rect.w - (count - 1 - index) as f32 * (BAR_W + BAR_GAP);
        f.place(
            &format!("aegis-hud-spark-{metric}-{index}"),
            &materials::chrome_place(
                Rect {
                    x: right - BAR_W,
                    y: rect.y + rect.h - h,
                    w: BAR_W,
                    h,
                },
                LayoutOpts {
                    bg: hud.accent,
                    border: Color::TRANSPARENT,
                    radius: 0.75,
                    pad: 0.0,
                    ..materials::surface_layout()
                },
            ),
            |_| {},
        );
    }
}

/// SI throughput: "<10 → one decimal (`1.2M`), otherwise none (`340K`)".
#[allow(dead_code)]
pub(super) fn format_rate(bytes_per_sec: f64) -> String {
    const UNITS: [&str; 4] = ["B", "K", "M", "G"];
    let mut value = bytes_per_sec.max(0.0);
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 || value >= 10.0 {
        format!("{value:.0}{}", UNITS[unit])
    } else {
        format!("{value:.1}{}", UNITS[unit])
    }
}

/// A "used/total" GiB pair for the RAM gauge, one decimal each: `6.4/15.6G`.
#[allow(dead_code)]
pub(super) fn format_gib_pair(used: u64, total: u64) -> String {
    const GIB: f64 = 1024.0 * 1024.0 * 1024.0;
    format!("{:.1}/{:.1}G", used as f64 / GIB, total as f64 / GIB)
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

/// A framed throughput chart box: a hairline frame plus a centered
/// polyline built from the history's samples, newest at the right. The
/// polyline is drawn as thin vertical bars whose heights follow the sample
/// ramp — the workspace's only "line" idiom (lens has no path primitive).
pub(super) fn render_rate_chart(
    f: &mut Frame,
    id: &str,
    history: &History,
    rect: Rect,
    color: Color,
    frame_color: Color,
) {
    f.place(
        &format!("{id}-frame"),
        &materials::chrome_place(
            rect,
            LayoutOpts {
                bg: Color::TRANSPARENT,
                border: frame_color,
                border_width: 1.0,
                radius: 8.0,
                pad: 0.0,
                ..materials::surface_layout()
            },
        ),
        |_| {},
    );
    // Chart body inset inside the frame, keeping the stroke visible.
    const INSET: f32 = 7.0;
    let body = Rect {
        x: rect.x + INSET,
        y: rect.y + INSET,
        w: (rect.w - INSET * 2.0).max(1.0),
        h: (rect.h - INSET * 2.0).max(1.0),
    };
    const BAR_W: f32 = 2.5;
    const BAR_GAP: f32 = 2.0;
    let max_bars = ((body.w + BAR_GAP) / (BAR_W + BAR_GAP)).floor().max(0.0) as usize;
    if max_bars == 0 {
        return;
    }
    // Normalize against the series' own peak (bytes/s have no natural
    // ceiling), then ramp the bar heights along the series so the shape
    // reads as a line rather than a bar meter.
    let samples: Vec<f32> = history.samples().collect();
    let count = samples.len().min(max_bars);
    if count == 0 {
        return;
    }
    let peak = samples.iter().fold(0.0_f32, |a, b| a.max(*b)).max(1.0);
    for (index, sample) in samples[samples.len() - count..].iter().enumerate() {
        let level = (sample / peak).clamp(0.0, 1.0);
        // A 1.5px floor keeps flat traffic visible as a live baseline.
        let h = ((body.h - 2.0) * level + 1.5).min(body.h);
        let right = body.x + body.w - (count - 1 - index) as f32 * (BAR_W + BAR_GAP);
        f.place(
            &format!("{id}-bar-{index}"),
            &materials::chrome_place(
                Rect {
                    x: right - BAR_W,
                    y: body.y + body.h - h,
                    w: BAR_W,
                    h,
                },
                LayoutOpts {
                    bg: color,
                    border: Color::TRANSPARENT,
                    radius: 1.0,
                    pad: 0.0,
                    ..materials::surface_layout()
                },
            ),
            |_| {},
        );
    }
}
