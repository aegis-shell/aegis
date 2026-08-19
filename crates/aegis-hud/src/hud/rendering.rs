use super::*;
use aegis_design::materials::{chrome_place, surface_layout};
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

pub(super) fn recording_cell_width(streams: u32) -> f32 {
    if streams <= 1 { 38.0 } else { 48.0 }
}

pub(super) fn render_recording_cell(
    f: &mut Frame,
    design: &Design,
    id: &str,
    rect: Rect,
    streams: u32,
) {
    let label = if streams <= 1 {
        "REC".to_string()
    } else {
        format!("REC {streams}")
    };
    const DOT_SIZE: f32 = 7.0;
    f.place(id, &chrome_place(rect, centered_layer()), |f| {
        f.row_ex(
            &LayoutOpts {
                width: rect.w,
                height: rect.h,
                gap: 4.0,
                cross: Align::Center,
                ..Default::default()
            },
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: DOT_SIZE,
                        height: DOT_SIZE,
                        bg: design.colors.critical,
                        border: hud_contour_color(design),
                        border_width: design.hud_foreground.glyph_contour_width,
                        radius: DOT_SIZE * 0.5,
                        ..Default::default()
                    },
                    |_| {},
                );
                f.push_style(hud_text_outline(design));
                f.label_compact_sized(&label, design.typography.footnote);
                f.pop_style();
            },
        );
    });
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

pub(super) fn render_text(
    f: &mut Frame,
    design: &Design,
    id: &str,
    rect: Rect,
    text: &str,
    size: f32,
) {
    f.place(id, &chrome_place(rect, centered_layer()), |f| {
        f.row_ex(
            &LayoutOpts {
                width: rect.w,
                height: rect.h,
                cross: Align::Center,
                ..Default::default()
            },
            |f| {
                f.push_style(hud_text_outline(design));
                f.label_compact_sized(text, size);
                f.pop_style();
            },
        );
    });
}

/// One display-only status cell: a themed raster icon (or vector fallback)
/// plus an optional compact label. The frame opacity fades the raster tint
/// with the chip theme and the compositor glass body.
pub(super) fn render_status_cell(
    f: &mut Frame,
    design: &Design,
    id: &str,
    rect: Rect,
    themed_icon: Option<*mut c_void>,
    fallback: Icon,
    label: &str,
) {
    f.place(id, &chrome_place(rect, centered_layer()), |f| {
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
                        f.push_style(hud_glyph_outline(design));
                        f.image_tinted(
                            icon as *mut lens::sys::flux_image,
                            16.0,
                            16.0,
                            design.hud_foreground.primary,
                        );
                        f.pop_style();
                    },
                    None => {
                        f.push_style(hud_glyph_outline(design));
                        f.icon(fallback, 15.0);
                        f.pop_style();
                    }
                }
                if !label.is_empty() {
                    f.push_style(hud_text_outline(design));
                    f.label_compact_sized(label, design.typography.footnote);
                    f.pop_style();
                }
            },
        );
    });
}

pub(super) fn hud_contour_color(design: &Design) -> Color {
    design.hud_foreground.contour
}

pub(super) use aegis_ui::{
    chip_opts, contains, hud_glyph_outline, hud_text_outline, workspace_dot_color,
    workspace_dot_intensity,
};
#[cfg(test)]
pub(super) use aegis_ui::{hud_glyph_outline_params, hud_text_outline_params};

pub(super) fn workspace_dot_diameter() -> f32 {
    WORKSPACE_DOT_DIAMETER
}

pub(super) fn centered_layer() -> LayoutOpts {
    LayoutOpts {
        bg: Color::TRANSPARENT,
        border: Color::TRANSPARENT,
        border_width: 0.0,
        radius: 0.0,
        pad: 0.0,
        cross: Align::Center,
        ..surface_layout()
    }
}
