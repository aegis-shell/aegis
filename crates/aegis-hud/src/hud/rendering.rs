use super::*;
use std::ffi::c_void;

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

/// How registered SNI items fold into the slot budget. Past budget the last
/// slot becomes a "+N" overflow indicator counting everything hidden.
pub(super) struct TrayFold {
    pub(super) visible: usize,
    pub(super) hidden: usize,
}

pub(super) fn render_text(f: &mut Frame, id: &str, rect: Rect, text: &str, size: f32, fade: f32) {
    f.layer(id, rect, &centered_layer(), |f| {
        f.row_ex(
            &LayoutOpts {
                width: rect.w,
                height: rect.h,
                cross: Align::Center,
                ..Default::default()
            },
            |f| f.label_compact_outlined_sized(text, size, hud_text_outline(fade)),
        );
    });
}

/// One display-only status cell: a themed raster icon (or vector fallback)
/// plus an optional compact label. The raster tint follows the exact same
/// fade as the chip theme and compositor glass body.
pub(super) fn render_status_cell(
    f: &mut Frame,
    id: &str,
    rect: Rect,
    fade: f32,
    themed_icon: Option<*mut c_void>,
    fallback: Icon,
    label: &str,
) {
    f.layer(id, rect, &centered_layer(), |f| {
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
                        f.image_tinted_outlined(
                            icon as *mut lens::sys::flux_image,
                            16.0,
                            16.0,
                            hud_foreground_color(fade),
                            hud_glyph_outline(fade),
                        )
                    },
                    None => f.icon_outlined(fallback, 15.0, hud_glyph_outline(fade)),
                }
                if !label.is_empty() {
                    f.label_compact_outlined_sized(label, 11.0, hud_text_outline(fade));
                }
            },
        );
    });
}

/// Stable light core for HUD foreground content. The dark contour carries
/// bright-background separation, so the core can stay constant instead of
/// flipping every symbol independently and causing visual chatter.
pub(super) fn hud_foreground_color(fade: f32) -> Color {
    fade_color(Design::dark().hud_foreground.primary, fade)
}

pub(super) fn hud_contour_color() -> Color {
    Design::dark().hud_foreground.contour
}

pub(super) fn hud_text_outline(fade: f32) -> ForegroundOutline {
    let hud = Design::dark().hud_foreground;
    ForegroundOutline::new(fade_color(hud.contour, fade), hud.text_contour_width)
}

pub(super) fn hud_glyph_outline(fade: f32) -> ForegroundOutline {
    let hud = Design::dark().hud_foreground;
    ForegroundOutline::new(fade_color(hud.contour, fade), hud.glyph_contour_width)
}

/// The floating HUD chip foreground tint. The compositor's SDF glass pass now
/// supplies the body, refraction and rim; this intentionally stays subtle so
/// it does not turn the physical glass back into an opaque dark pill.
pub(super) fn chip_opts(fade: f32) -> OverlayOpts {
    OverlayOpts {
        bg: fade_color(Color::rgba(24, 26, 36, 42), fade),
        border: fade_color(Color::rgba(255, 255, 255, 18), fade),
        border_width: 0.75,
        radius: CHIP_RADIUS,
        pad: 0.0,
        ..Default::default()
    }
}

pub(super) fn workspace_dot_color(intensity: f32) -> Color {
    let primary = Design::dark().hud_foreground.primary;
    let intensity = intensity.clamp(0.0, 1.0);
    let alpha = (78.0 + (248.0 - 78.0) * intensity).round() as u8;
    primary.with_alpha(alpha)
}

pub(super) fn workspace_dot_diameter() -> f32 {
    WORKSPACE_DOT_DIAMETER
}

pub(super) fn workspace_dot_intensity(index: usize, position: f32) -> f32 {
    (1.0 - (index as f32 - position).abs()).clamp(0.0, 1.0)
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

pub(super) fn fade_alpha(base: u8, progress: f32) -> u8 {
    (base as f32 * progress.clamp(0.0, 1.0)).round() as u8
}

/// Scale one color's alpha by the chip fade (lens has no opacity property;
/// fading is per-color).
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
