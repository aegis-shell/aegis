use super::*;

use aegis_design::materials::{chrome_place, sized, surface_layout, transparent};
use lens::Icon;

// ---- rendering -----------------------------------------------------------

impl CommandPanel {
    /// Bounds of the currently open dbusmenu popover, if any.
    pub(super) fn open_popover_bounds(&mut self, display: (f32, f32)) -> Option<Rect> {
        let key = self.menu_open_for.clone()?;
        let menu = self.menu_snapshot().filter(|menu| menu.key == key)?;
        aegis_tray::visible_children(&menu.root, &self.menu_path)
            .map(|visible| menu_bounds(self.menu_owner, visible, display))
    }

    /// The top-left profile block: user persona (gapped-ring avatar, display
    /// name, `@username · groups`, hostname) drawn frameless — no chip glass,
    /// no background, no border — straight onto the blurred background.
    /// Slides in from the top-left.
    pub(super) fn render_profile_panel(
        &self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        _i18n: &Localizer,
    ) {
        let hud = Hud::classic();
        let type_scale = self.design.typography;
        let slide = (1.0 - ease_out_cubic(progress)) * -24.0;
        let rect = Rect {
            x: rect.x + slide,
            ..rect
        };

        let pad = 14.0;
        let center_y = rect.y + rect.h * 0.5;
        let base_theme = themes::hud(&hud);
        let muted_theme = themes::hud_muted(base_theme, &hud);
        let avatar_style = self.design.avatars.for_role(AvatarRole::PersonaHeader);
        let original = f.theme();

        // -- profile zone: 56px avatar with a gapped line ring + name lines --
        let avatar_size = 56.0;
        let avatar_center = (rect.x + pad + avatar_size * 0.5, center_y);
        // The ring floats clear of the avatar edge — the visible gap reads
        // as a deliberate stroke of the identity mark rather than a border.
        let ring_gap = 3.0;
        let ring_diameter = avatar_size + ring_gap * 2.0 + 2.0;
        render_ring(
            f,
            "aegis-hud-avatar-ring",
            avatar_center,
            ring_diameter,
            hud.accent,
            1.5,
        );
        render_disc(
            f,
            "aegis-hud-avatar-backdrop",
            avatar_center,
            avatar_size,
            avatar_style.fallback_surface,
        );
        let avatar_rect = Rect {
            x: avatar_center.0 - avatar_size * 0.5,
            y: avatar_center.1 - avatar_size * 0.5,
            w: avatar_size,
            h: avatar_size,
        };
        match &self.avatar {
            Some(avatar) => {
                let texture = avatar.texture().as_raw();
                f.place(
                    "aegis-hud-avatar",
                    &chrome_place(avatar_rect, transparent()),
                    |f| {
                        f.row_ex(&sized(avatar_size, avatar_size), |f| {
                            unsafe {
                                f.image(
                                    texture as *mut lens::sys::flux_image,
                                    avatar_size,
                                    avatar_size,
                                )
                            };
                        });
                    },
                );
            }
            None => {
                f.set_theme(base_theme.with_fg(avatar_style.fallback_foreground));
                f.place(
                    "aegis-hud-avatar-initials",
                    &chrome_place(avatar_rect, transparent()),
                    |f| {
                        f.row_ex(
                            &LayoutOpts {
                                width: avatar_size,
                                height: avatar_size,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            |f| {
                                f.flex(1.0);
                                f.spacer(0.0);
                                display_label(
                                    f,
                                    &self.profile.initials,
                                    avatar_rect.w * avatar_style.initials_scale,
                                );
                                f.flex(1.0);
                                f.spacer(0.0);
                            },
                        );
                    },
                );
            }
        }

        let text_x = rect.x + pad + ring_diameter + 14.0;
        let text_w = (rect.x + rect.w - pad - text_x).max(40.0);
        let display_name = truncate(&self.profile.display_name, (text_w / 9.5).max(4.0) as usize);
        f.set_theme(base_theme);
        f.place(
            "aegis-hud-profile-name",
            &chrome_place(
                Rect {
                    x: text_x,
                    y: center_y - 26.0,
                    w: text_w,
                    h: 24.0,
                },
                transparent(),
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: text_w,
                        height: 24.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| display_label(f, &display_name, type_scale.title),
                );
            },
        );
        let mut sub_line = format!("@{}", self.profile.username);
        if !self.profile.groups.is_empty() {
            sub_line.push_str(" · ");
            sub_line.push_str(&self.profile.groups.join(", "));
        }
        let sub_line = truncate(&sub_line, (text_w / 6.2).max(6.0) as usize);
        f.set_theme(muted_theme);
        f.place(
            "aegis-hud-profile-sub",
            &chrome_place(
                Rect {
                    x: text_x,
                    y: center_y + 1.0,
                    w: text_w,
                    h: 18.0,
                },
                transparent(),
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: text_w,
                        height: 18.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| display_label(f, &sub_line, type_scale.label),
                );
            },
        );
        // The hostname line answers "which machine": muted, sitting under
        // the account line so the block reads who-on-where.
        if !self.profile.hostname.is_empty() {
            let host_line = truncate(&self.profile.hostname, (text_w / 6.2).max(6.0) as usize);
            f.place(
                "aegis-hud-profile-host",
                &chrome_place(
                    Rect {
                        x: text_x,
                        y: center_y + 20.0,
                        w: text_w,
                        h: 16.0,
                    },
                    transparent(),
                ),
                |f| {
                    f.row_ex(
                        &LayoutOpts {
                            width: text_w,
                            height: 16.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| display_label(f, &host_line, type_scale.footnote),
                    );
                },
            );
        }
        f.set_theme(original);
    }

    /// The machine telemetry monitor: chassis glyph plus utilization gauges
    /// (CPU/GPU/RAM with sparklines, NET, DISK, BAT).
    #[allow(dead_code)]
    pub(super) fn render_machine_panel(&self, f: &mut Frame, rect: Rect, i18n: &Localizer) {
        let hud = Hud::classic();
        let type_scale = self.design.typography;
        let pad = 16.0;
        let inner_y = rect.y + pad;
        let inner_h = (rect.h - pad * 2.0).max(1.0);
        let base_theme = themes::hud(&hud);
        let muted_theme = themes::hud_muted(base_theme, &hud);
        let original = f.theme();

        let machine_x = rect.x + pad;
        let machine_right = rect.x + rect.w - pad;
        if machine_right - machine_x < 120.0 {
            f.set_theme(original);
            return;
        }

        // Chassis glyph: a thin-line machine pictogram built from placed rects.
        let glyph_cx = machine_x + 28.0;
        let chassis_label = match self.stats.chassis {
            ChassisKind::Laptop => i18n.text(Message::Laptop),
            ChassisKind::Desktop => i18n.text(Message::DesktopChassis),
        };
        let glyph_h = match self.stats.chassis {
            ChassisKind::Laptop => 24.0 + 2.0 + 2.5,
            ChassisKind::Desktop => 22.0 + 7.0 + 2.0,
        };
        let glyph_top = inner_y + (inner_h - 17.0 - glyph_h).max(0.0) * 0.5;
        let muted_line = hud.text_muted;
        let outline = |radius: f32| LayoutOpts {
            bg: Color::TRANSPARENT,
            border: muted_line,
            border_width: 1.2,
            radius,
            pad: 0.0,
            ..surface_layout()
        };
        let filled = |radius: f32| LayoutOpts {
            bg: muted_line,
            border: Color::TRANSPARENT,
            radius,
            pad: 0.0,
            ..surface_layout()
        };
        match self.stats.chassis {
            ChassisKind::Laptop => {
                let screen = Rect {
                    x: glyph_cx - 18.0,
                    y: glyph_top,
                    w: 36.0,
                    h: 24.0,
                };
                f.place(
                    "aegis-hud-chassis-screen",
                    &chrome_place(screen, outline(3.0)),
                    |_| {},
                );
                let base = Rect {
                    x: glyph_cx - 22.0,
                    y: glyph_top + 24.0 + 2.0,
                    w: 44.0,
                    h: 2.5,
                };
                f.place(
                    "aegis-hud-chassis-base",
                    &chrome_place(base, filled(1.25)),
                    |_| {},
                );
            }
            ChassisKind::Desktop => {
                let monitor = Rect {
                    x: glyph_cx - 17.0,
                    y: glyph_top,
                    w: 34.0,
                    h: 22.0,
                };
                f.place(
                    "aegis-hud-chassis-screen",
                    &chrome_place(monitor, outline(2.0)),
                    |_| {},
                );
                let stand = Rect {
                    x: glyph_cx - 1.0,
                    y: glyph_top + 22.0,
                    w: 2.0,
                    h: 7.0,
                };
                f.place(
                    "aegis-hud-chassis-stand",
                    &chrome_place(stand, filled(0.0)),
                    |_| {},
                );
                let base = Rect {
                    x: glyph_cx - 8.0,
                    y: glyph_top + 29.0,
                    w: 16.0,
                    h: 2.0,
                };
                f.place(
                    "aegis-hud-chassis-base",
                    &chrome_place(base, filled(1.0)),
                    |_| {},
                );
            }
        }
        f.set_theme(muted_theme);
        f.place(
            "aegis-hud-chassis-label",
            &chrome_place(
                Rect {
                    x: machine_x,
                    y: rect.y + rect.h - pad - 13.0,
                    w: 56.0,
                    h: 13.0,
                },
                transparent(),
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: 56.0,
                        height: 13.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        f.flex(1.0);
                        f.spacer(0.0);
                        display_label(f, chassis_label, type_scale.caption);
                        f.flex(1.0);
                        f.spacer(0.0);
                    },
                );
            },
        );

        // Gauge rows to the right of the glyph, vertically centered.
        let stats = self.stats;
        let mut gauges: Vec<Gauge> = Vec::with_capacity(6);
        gauges.push(Gauge::Cpu);
        if let Some(gpu) = stats.gpu_percent {
            gauges.push(Gauge::Gpu(gpu));
        }
        let mem_fraction = if stats.mem_total_bytes > 0 {
            stats.mem_used_bytes as f32 / stats.mem_total_bytes as f32
        } else {
            0.0
        };
        gauges.push(Gauge::Ram {
            fraction: mem_fraction,
            value: format_gib_pair(stats.mem_used_bytes, stats.mem_total_bytes),
        });
        gauges.push(Gauge::Net {
            value: format!(
                "↓{}/s ↑{}/s",
                format_rate(stats.net_rx_bytes_per_sec),
                format_rate(stats.net_tx_bytes_per_sec)
            ),
        });
        let disk_fraction = if stats.disk_total_bytes > 0 {
            stats.disk_used_bytes as f32 / stats.disk_total_bytes as f32
        } else {
            0.0
        };
        gauges.push(Gauge::Disk {
            fraction: disk_fraction,
            value: format!("{:.0}%", disk_fraction * 100.0),
        });
        if let Some(battery) = self.status.battery {
            gauges.push(Gauge::Battery {
                fraction: battery.percent as f32 / 100.0,
                value: format!("{}%", battery.percent),
                charging: battery.charging,
            });
        }
        // The panel fits five 14px rows; when every source applies the
        // battery row (last in priority) is the one that yields.
        gauges.truncate(5);

        const ROW_H: f32 = 14.0;
        const ROW_GAP: f32 = 2.5;
        let rows_h =
            gauges.len() as f32 * ROW_H + (gauges.len().saturating_sub(1)) as f32 * ROW_GAP;
        let mut row_y = inner_y + (inner_h - rows_h).max(0.0) * 0.5;
        let gauge_x = machine_x + 56.0 + 8.0;
        let gauge_w = (machine_right - gauge_x).max(1.0);
        for (index, gauge) in gauges.iter().enumerate() {
            self.render_gauge_row(
                f,
                gauge,
                index,
                Rect {
                    x: gauge_x,
                    y: row_y,
                    w: gauge_w,
                    h: ROW_H,
                },
                i18n,
            );
            row_y += ROW_H + ROW_GAP;
        }
        f.set_theme(original);
    }

    /// One gauge row of the header band's machine monitor: a 40px label
    /// cell, the bar/sparkline zone, and a 58px right-aligned value cell.
    #[allow(dead_code)]
    pub(super) fn render_gauge_row(
        &self,
        f: &mut Frame,
        gauge: &Gauge,
        index: usize,
        row: Rect,
        i18n: &Localizer,
    ) {
        let hud = Hud::classic();
        let type_scale = self.design.typography;
        let base_theme = themes::hud(&hud);
        let muted_theme = themes::hud_muted(base_theme, &hud);
        let original = f.theme();
        let label_rect = Rect {
            x: row.x,
            y: row.y,
            w: 40.0,
            h: row.h,
        };
        let bar_x = row.x + 40.0 + 6.0;
        let value_x = row.x + row.w - 58.0;
        let bar_w = (value_x - 6.0 - bar_x).max(1.0);

        // Label cell: a caption-scale text label, or a 10px icon for NET/BAT.
        let icon_label: Option<(Icon, Color)> = match gauge {
            Gauge::Net { .. } => Some((Icon::Globe, hud.text_muted)),
            Gauge::Battery { charging, .. } => Some((
                Icon::Zap,
                if *charging {
                    hud.accent
                } else {
                    hud.text_muted
                },
            )),
            _ => None,
        };
        let text_label: Option<&'static str> = match gauge {
            Gauge::Cpu => Some(i18n.text(Message::Cpu)),
            Gauge::Gpu(_) => Some(i18n.text(Message::Gpu)),
            Gauge::Ram { .. } => Some(i18n.text(Message::Memory)),
            Gauge::Disk { .. } => Some(i18n.text(Message::Disk)),
            _ => None,
        };
        if text_label.is_some() {
            f.set_theme(muted_theme);
        } else if let Some((_, color)) = icon_label {
            f.set_theme(base_theme.with_fg(color));
        }
        f.place(
            &format!("aegis-hud-gauge-label-{index}"),
            &chrome_place(label_rect, transparent()),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: 40.0,
                        height: row.h,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        if let Some(text) = text_label {
                            display_label(f, text, type_scale.caption);
                        } else if let Some((icon, _)) = icon_label {
                            f.icon(icon, 10.0);
                        }
                    },
                );
            },
        );

        // Bar/sparkline zone + value cell.
        let (value, full_span): (String, bool) = match gauge {
            Gauge::Cpu => {
                render_sparkline(
                    f,
                    "cpu",
                    &self.cpu_history,
                    Rect {
                        x: bar_x,
                        y: row.y,
                        w: bar_w,
                        h: row.h,
                    },
                );
                (format!("{:.0}%", self.stats.cpu_percent), false)
            }
            Gauge::Gpu(gpu) => {
                gauge_bar(
                    f,
                    &format!("aegis-hud-gauge-bar-{index}"),
                    Rect {
                        x: bar_x,
                        y: row.y + (row.h - 4.0) * 0.5,
                        w: bar_w,
                        h: 4.0,
                    },
                    gpu / 100.0,
                );
                (format!("{gpu:.0}%"), false)
            }
            Gauge::Ram { fraction, value } => {
                gauge_bar(
                    f,
                    &format!("aegis-hud-gauge-bar-{index}"),
                    Rect {
                        x: bar_x,
                        y: row.y + (row.h - 4.0) * 0.5,
                        w: bar_w,
                        h: 4.0,
                    },
                    *fraction,
                );
                (value.clone(), false)
            }
            Gauge::Net { value } => (value.clone(), true),
            Gauge::Disk { fraction, value } => {
                gauge_bar(
                    f,
                    &format!("aegis-hud-gauge-bar-{index}"),
                    Rect {
                        x: bar_x,
                        y: row.y + (row.h - 4.0) * 0.5,
                        w: bar_w,
                        h: 4.0,
                    },
                    *fraction,
                );
                (value.clone(), false)
            }
            Gauge::Battery {
                fraction, value, ..
            } => {
                gauge_bar(
                    f,
                    &format!("aegis-hud-gauge-bar-{index}"),
                    Rect {
                        x: bar_x,
                        y: row.y + (row.h - 4.0) * 0.5,
                        w: bar_w,
                        h: 4.0,
                    },
                    *fraction,
                );
                (value.clone(), false)
            }
        };
        let value_rect = if full_span {
            Rect {
                x: bar_x,
                y: row.y,
                w: (row.x + row.w - bar_x).max(1.0),
                h: row.h,
            }
        } else {
            Rect {
                x: value_x,
                y: row.y,
                w: 58.0,
                h: row.h,
            }
        };
        f.set_theme(base_theme);
        f.place(
            &format!("aegis-hud-gauge-value-{index}"),
            &chrome_place(value_rect, transparent()),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: value_rect.w,
                        height: row.h,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        f.flex(1.0);
                        f.spacer(0.0);
                        display_label(f, &value, type_scale.caption);
                    },
                );
            },
        );
        f.set_theme(original);
    }

    /// Returns the dedicated icon for a navigation tab.
    pub(super) fn tab_icon(tab: Tab) -> Icon {
        match tab {
            Tab::QuickControls => Icon::Sliders,
            Tab::Settings(id) => match id.as_str() {
                "display" => Icon::Activity,
                "appearance" => Icon::PenTool,
                "dock" => Icon::Sidebar,
                "power" => Icon::Zap,
                "input" | "touchpad" | "mouse" | "keyboard" => Icon::MousePointer,
                "keybindings" => Icon::Edit,
                "users" | "persona" => Icon::Users,
                "window-rules" => Icon::Grid,
                "network" | "wifi" => Icon::Globe,
                "sound" | "audio" => Icon::VolumeHigh,
                "bluetooth" => Icon::Radio,
                _ => Icon::Settings,
            },
        }
    }

    /// The central command panel, split horizontally:
    /// - Left: Capsule-shaped liquid glass navigation rail (icon + label).
    /// - Right: Gaussian blur frosted glass tab page view.
    pub(super) fn render_main_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = Hud::classic();
        let type_scale = self.design.typography;
        let rise = (1.0 - progress) * 16.0;
        let rect = Rect {
            y: rect.y + rise,
            ..rect
        };

        let nav_w = 190.0_f32.min(rect.w * 0.28).max(150.0);
        let gap = 16.0;
        let view_w = (rect.w - nav_w - gap).max(1.0);

        let nav_rect = Rect {
            x: rect.x,
            y: rect.y,
            w: nav_w,
            h: rect.h,
        };
        let view_rect = Rect {
            x: rect.x + nav_w + gap,
            y: rect.y,
            w: view_w,
            h: rect.h,
        };

        // 1. Left Column: Individual Floating Capsule Liquid Glass Navigation Rail (NO outer bounding box)
        self.render_nav_rail(f, nav_rect, i18n);

        // 2. Right Column: Glass Page View (ProminentPanel physical liquid glass)
        f.place(
            "aegis-hud-view-glass",
            &chrome_place(view_rect, materials::glass_panel(&self.design)),
            |f| {
                f.column_ex(&sized(view_rect.w, view_rect.h), |_| {});
            },
        );

        let pad_h = 20.0;
        let pad_v = 18.0;
        let header_h = 36.0;
        let active_title = match self.tab {
            Tab::QuickControls => i18n.text(Message::QuickControls),
            Tab::Settings(id) => self
                .modules
                .metadata()
                .find(|m| m.id == id)
                .map(|m| i18n.text(m.title))
                .unwrap_or("Settings"),
        };

        // Header inside Right View
        let original = f.theme();
        f.set_theme(themes::hud(&hud));
        f.place(
            "aegis-hud-view-header",
            &chrome_place(
                Rect {
                    x: view_rect.x + pad_h,
                    y: view_rect.y + pad_v,
                    w: (view_rect.w - pad_h * 2.0).max(1.0),
                    h: header_h,
                },
                transparent(),
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: (view_rect.w - pad_h * 2.0).max(1.0),
                        height: header_h,
                        cross: Align::Center,
                        gap: 10.0,
                        ..Default::default()
                    },
                    |f| {
                        f.icon(Self::tab_icon(self.tab), 22.0);
                        display_label(f, active_title, type_scale.title);
                    },
                );
            },
        );
        f.set_theme(original);

        let body_area = Rect {
            x: view_rect.x + pad_h,
            y: view_rect.y + pad_v + header_h + 10.0,
            w: (view_rect.w - pad_h * 2.0).max(1.0),
            h: (view_rect.h - pad_v * 2.0 - header_h - 10.0).max(1.0),
        };
        match self.tab {
            Tab::QuickControls => self.render_quick_controls_section(f, body_area, i18n, out),
            Tab::Settings(id) => self.render_settings_tab(f, id, body_area, i18n, out),
        }
    }

    /// The capsule-shaped liquid glass navigation rail:
    /// 100% semicircular pills with high translucency physical liquid glass.
    pub(super) fn render_nav_rail(&mut self, f: &mut Frame, rect: Rect, i18n: &Localizer) {
        let hud = Hud::classic();
        let type_scale = self.design.typography;
        let mut tabs: Vec<(Tab, &'static str)> =
            vec![(Tab::QuickControls, i18n.text(Message::QuickControls))];
        tabs.extend(
            self.modules
                .metadata()
                .filter(|module| module.availability == ModuleAvailability::Available)
                .map(|module| (Tab::Settings(module.id), i18n.text(module.title))),
        );

        let mut action: Option<TabAction> = None;
        let original = f.theme();

        const CAPSULE_H: f32 = 44.0;
        const CAPSULE_RADIUS: f32 = CAPSULE_H * 0.5; // 100% semicircle ends
        const TAB_GAP: f32 = 8.0;

        let tab_theme = themes::hud(&hud)
            .with_hover(Color::TRANSPARENT)
            .with_active(Color::TRANSPARENT);

        f.place(
            "aegis-hud-nav-rail",
            &chrome_place(rect, transparent()),
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: rect.w,
                        height: rect.h,
                        gap: TAB_GAP,
                        cross: Align::Stretch,
                        ..Default::default()
                    },
                    |f| {
                        for (index, (tab, label)) in tabs.iter().enumerate() {
                            let selected = self.tab == *tab;

                            // 100% pure physical liquid glass — ZERO 2D hover/active background paint!
                            let (text_color, icon_color) = if selected {
                                (
                                    Color::rgba(255, 255, 255, 255), // Brilliant illuminated white text
                                    hud.accent,                      // Glowing cyan accent icon
                                )
                            } else {
                                (
                                    Color::rgba(200, 208, 225, 220),
                                    Color::rgba(165, 172, 195, 200),
                                )
                            };

                            let icon = Self::tab_icon(*tab);
                            let label_text =
                                truncate(label, ((rect.w - 62.0) / 7.2).max(4.0) as usize);

                            f.set_theme(tab_theme.with_fg(text_color));
                            let (response, _) = f.pressable_row(
                                &format!("aegis-hud-tab-{index}"),
                                &label_text,
                                &LayoutOpts {
                                    height: CAPSULE_H,
                                    pad: 18.0,
                                    radius: CAPSULE_RADIUS, // 100% semicircle
                                    cross: Align::Center,
                                    gap: 12.0,
                                    bg: Color::TRANSPARENT, // ZERO mask overlay!
                                    border: Color::TRANSPARENT,
                                    border_width: 0.0,
                                    ..Default::default()
                                },
                                |f, _| {
                                    f.set_theme(tab_theme.with_fg(icon_color));
                                    f.icon(icon, 20.0);
                                    f.set_theme(tab_theme.with_fg(text_color));
                                    display_label(f, &label_text, type_scale.body);
                                },
                            );
                            if response.clicked && !selected {
                                action = Some(TabAction::Select(*tab));
                            }
                        }
                    },
                );
            },
        );
        f.set_theme(original);

        match action {
            Some(TabAction::Select(tab)) => self.select_tab(tab),
            None => {}
        }
    }

    /// A settings module tab's body: the module's page inside a scroll
    /// area, painted with the theme matching the stored design snapshot.
    /// Emitted `SettingsAction`s are forwarded to the shell tagged with the
    /// current snapshot revision, coalesced to the newest draft per action
    /// kind (instant modules emit per change while a control drags). Until
    /// the first settings snapshot arrives the tab shows a muted
    /// placeholder instead.
    pub(super) fn render_settings_tab(
        &mut self,
        f: &mut Frame,
        id: ModuleId,
        area: Rect,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = Hud::classic();
        let type_scale = self.design.typography;
        if self.settings.is_none() {
            let original = f.theme();
            let muted = themes::hud_muted(themes::hud(&hud), &hud);
            f.set_theme(muted);
            f.place(
                "aegis-hud-settings-empty",
                &chrome_place(area, transparent()),
                |f| {
                    f.row_ex(
                        &LayoutOpts {
                            width: area.w,
                            height: area.h,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            f.flex(1.0);
                            f.spacer(0.0);
                            display_label(
                                f,
                                i18n.text(Message::ConnectingToDesktop),
                                type_scale.body,
                            );
                            f.flex(1.0);
                            f.spacer(0.0);
                        },
                    );
                },
            );
            f.set_theme(original);
            return;
        }
        let design = self.design;
        let mut events = ModuleEvents::default();
        let original = f.theme();
        f.set_theme(themes::application(&design));
        f.place(
            "aegis-hud-settings",
            &chrome_place(area, transparent()),
            |f| {
                f.column_ex(&sized(area.w, area.h), |f| {
                    f.flex(1.0);
                    f.scroll("aegis-hud-settings-scroll", |f| {
                        f.column_ex(
                            &LayoutOpts {
                                gap: 12.0,
                                cross: Align::Stretch,
                                ..Default::default()
                            },
                            |f| {
                                self.modules.render(id, f, i18n, &design, &mut events);
                            },
                        );
                    });
                });
            },
        );
        f.set_theme(original);
        let revision = self.settings.as_ref().map(|settings| settings.revision);
        for action in events.actions {
            out.settings_actions
                .retain(|(_, queued)| !same_action_kind(queued, &action));
            out.settings_actions.push((revision, action));
        }
    }

    /// The top-right notifications panel: notification stream in dark glass
    /// with corner brackets. Slides in from the top-right.
    pub(super) fn render_notifications_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let slide = (1.0 - ease_out_cubic(progress)) * 24.0;
        let rect = Rect {
            x: rect.x + slide,
            ..rect
        };
        // Notifications floating glass backing
        f.place(
            "aegis-hud-notifs-glass",
            &chrome_place(
                rect,
                LayoutOpts {
                    bg: Color::rgba(24, 26, 36, 38),
                    border: Color::rgba(255, 255, 255, 16),
                    border_width: 0.75,
                    radius: self.design.radii.glass_panel,
                    pad: 0.0,
                    ..surface_layout()
                },
            ),
            |f| {
                f.column_ex(&sized(rect.w, rect.h), |_| {});
            },
        );
        let body = self.render_side_panel(
            f,
            "aegis-hud-notifications-panel",
            rect,
            i18n.text(Message::Notifications),
        );
        self.render_messages_section(f, body, i18n, out);
    }

    /// The side column: the machine telemetry monitor over the
    /// StatusNotifierItem tray in a fixed-height panel pinned to the bottom.
    #[allow(dead_code)]
    pub(super) fn render_side_column(
        &mut self,
        f: &mut Frame,
        machine: Rect,
        tray: Rect,
        progress: f32,
        cursor: (f32, f32),
        i18n: &Localizer,
    ) {
        let rise = (1.0 - progress) * 16.0;
        let machine = Rect {
            y: machine.y + rise,
            ..machine
        };
        let tray = Rect {
            y: tray.y + rise,
            ..tray
        };
        self.render_machine_panel(f, machine, i18n);
        let body =
            self.render_side_panel(f, "aegis-hud-tray-panel", tray, i18n.text(Message::Tray));
        self.render_tray_section(f, body, cursor, i18n);
    }

    /// One boundless side-column section with a small muted section header
    /// at its top; returns the body area below the header.
    fn render_side_panel(&self, f: &mut Frame, id: &str, rect: Rect, title: &str) -> Rect {
        let hud = Hud::classic();
        let type_scale = self.design.typography;
        let original = f.theme();
        f.set_theme(themes::hud_muted(themes::hud(&hud), &hud));
        f.place(
            &format!("{id}-header"),
            &chrome_place(
                Rect {
                    x: rect.x + 14.0,
                    y: rect.y + 8.0,
                    w: rect.w - 28.0,
                    h: 17.0,
                },
                transparent(),
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: rect.w - 28.0,
                        height: 17.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        display_label(f, title, type_scale.label);
                    },
                );
            },
        );
        f.set_theme(original);
        Rect {
            x: rect.x + 12.0,
            y: rect.y + 29.0,
            w: rect.w - 24.0,
            h: (rect.h - 39.0).max(1.0),
        }
    }

    /// The first tab: quick controls for the daily toggles — volume,
    /// brightness, always-on, and do-not-disturb. These are the controls
    /// users reach for without thinking, so they lead the nav rail; the
    /// remaining quick settings stay in the System section.
    pub(super) fn render_quick_controls_section(
        &mut self,
        f: &mut Frame,
        area: Rect,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = Hud::classic();
        let type_scale = self.design.typography;
        let original = f.theme();
        f.set_theme(themes::hud(&hud));
        let status = self.status.clone();
        let volume_themed = self.themed_icon(volume_icon_name(&status));
        let base_theme = themes::hud(&hud);
        let muted_theme = themes::hud_muted(themes::hud(&hud), &hud);
        let group_header = move |f: &mut Frame, label: &str| {
            f.set_theme(muted_theme);
            display_label(f, label, type_scale.footnote);
            f.set_theme(base_theme);
        };
        f.place("aegis-hud-quick", &chrome_place(area, transparent()), |f| {
            f.column_ex(&sized(area.w, area.h), |f| {
                f.flex(1.0);
                f.scroll("aegis-hud-quick-scroll", |f| {
                    f.column_ex(
                        &LayoutOpts {
                            gap: 10.0,
                            cross: Align::Stretch,
                            ..Default::default()
                        },
                        |f| {
                            // Sound.
                            group_header(f, i18n.text(Message::Sound));
                            f.row_ex(
                                &LayoutOpts {
                                    height: 26.0,
                                    gap: 10.0,
                                    cross: Align::Center,
                                    ..Default::default()
                                },
                                |f| {
                                    match volume_themed {
                                        Some(icon) => unsafe {
                                            f.image(icon as *mut lens::sys::flux_image, 18.0, 18.0)
                                        },
                                        None => f.icon(volume_icon(&status), 17.0),
                                    }
                                    display_label(f, i18n.text(Message::Sound), type_scale.body);
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                    display_label(
                                        f,
                                        &status
                                            .volume
                                            .map(|level| format!("{level}%"))
                                            .unwrap_or_else(|| "--".into()),
                                        type_scale.label,
                                    );
                                },
                            );
                            if status.volume.is_some() {
                                let mut volume = status.volume.unwrap_or(0) as f32;
                                if f.slider("##hud-quick-volume", &mut volume, 0.0, 100.0) {
                                    out.system_actions.push(SystemAction::SetVolume {
                                        level: volume.round().clamp(0.0, 100.0) as u8,
                                    });
                                }
                                let mut muted = status.muted;
                                if f.checkbox(i18n.text(Message::Muted), &mut muted) {
                                    out.system_actions.push(SystemAction::ToggleMute);
                                }
                            } else {
                                unavailable_control(
                                    f,
                                    i18n.text(Message::Volume),
                                    i18n,
                                    type_scale,
                                );
                            }
                            f.spacer(4.0);

                            // Brightness.
                            group_header(f, i18n.text(Message::Brightness));
                            f.row_ex(
                                &LayoutOpts {
                                    height: 26.0,
                                    gap: 10.0,
                                    cross: Align::Center,
                                    ..Default::default()
                                },
                                |f| {
                                    f.icon(Icon::Zap, 17.0);
                                    display_label(
                                        f,
                                        i18n.text(Message::Brightness),
                                        type_scale.body,
                                    );
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                    display_label(
                                        f,
                                        &status
                                            .brightness
                                            .map(|level| format!("{level}%"))
                                            .unwrap_or_else(|| "--".into()),
                                        type_scale.label,
                                    );
                                },
                            );
                            if status.brightness.is_some() {
                                let mut brightness = status.brightness.unwrap_or(1) as f32;
                                if f.slider("##hud-quick-brightness", &mut brightness, 1.0, 100.0) {
                                    out.system_actions.push(SystemAction::SetBrightness {
                                        level: brightness.round().clamp(1.0, 100.0) as u8,
                                    });
                                }
                            } else {
                                unavailable_control(
                                    f,
                                    i18n.text(Message::Brightness),
                                    i18n,
                                    type_scale,
                                );
                            }
                            f.spacer(4.0);

                            // The daily toggles: keep-awake, automatic
                            // locking, and do-not-disturb as switch rows.
                            // The first two compose into the session power
                            // mode (ADR-0140): keep-awake is the display
                            // axis (never blank), auto-lock is the security
                            // axis. "No auto-lock" projects onto Awake: the
                            // security boundary forbids blanking or
                            // suspending an unlocked session, so dimming is
                            // the strongest idle response left.
                            group_header(f, i18n.text(Message::Session));
                            let mode = status.power_mode;
                            let keep_awake = !mode.blanks_display();
                            let mut keep_awake_value = keep_awake;
                            if f.switch(i18n.text(Message::KeepAwake), &mut keep_awake_value)
                                && keep_awake_value != keep_awake
                            {
                                let next =
                                    power_mode_for(keep_awake_value, mode.locks_automatically());
                                out.system_actions
                                    .push(SystemAction::SetPowerMode { mode: next });
                            }
                            let auto_lock = mode.locks_automatically();
                            let mut auto_lock_value = auto_lock;
                            if f.switch(i18n.text(Message::AutoLock), &mut auto_lock_value)
                                && auto_lock_value != auto_lock
                            {
                                let next = power_mode_for(keep_awake, auto_lock_value);
                                out.system_actions
                                    .push(SystemAction::SetPowerMode { mode: next });
                            }
                            let mut do_not_disturb = status.do_not_disturb;
                            if f.switch(i18n.text(Message::DoNotDisturb), &mut do_not_disturb) {
                                out.system_actions.push(SystemAction::SetDoNotDisturb {
                                    enabled: do_not_disturb,
                                });
                            }
                        },
                    );
                });
            });
        });
        f.set_theme(original);
    }

    /// The right-middle network monitor: the live interface (plus the Wi-Fi
    /// name when the link is wireless) as its identity, two framed line
    /// charts for upload and download throughput, and the current rates as
    /// text under each chart.
    pub(super) fn render_network_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        i18n: &Localizer,
    ) {
        if rect.w < 120.0 || rect.h < 120.0 {
            return;
        }
        let hud = Hud::classic();
        let type_scale = self.design.typography;
        let status = self.status.clone();
        let network_themed = self.themed_icon(network_icon_name(status.network));
        let identity = network_identity(&status, i18n);
        let state_label = network_text(&status, i18n);
        let rx = format!("{} / s", format_rate(self.stats.net_rx_bytes_per_sec));
        let tx = format!("{} / s", format_rate(self.stats.net_tx_bytes_per_sec));
        let slide = (1.0 - ease_out_cubic(progress)) * 24.0;
        let rect = Rect {
            x: rect.x + slide,
            ..rect
        };

        let original = f.theme();
        let base_theme = themes::hud(&hud);
        let muted_theme = themes::hud_muted(base_theme, &hud);

        let pad = 16.0;
        // Identity header: icon, `wlan0 · Homelab-5G`, and the link state.
        let header_h = 30.0;
        f.set_theme(base_theme);
        f.place(
            "aegis-hud-network-header",
            &chrome_place(
                Rect {
                    x: rect.x + pad,
                    y: rect.y + pad,
                    w: (rect.w - pad * 2.0).max(1.0),
                    h: header_h,
                },
                transparent(),
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: (rect.w - pad * 2.0).max(1.0),
                        height: header_h,
                        gap: 10.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        match network_themed {
                            Some(icon) => unsafe {
                                f.image(icon as *mut lens::sys::flux_image, 20.0, 20.0)
                            },
                            None => f.icon(Icon::Globe, 18.0),
                        }
                        display_label(f, &truncate(&identity, 26), type_scale.headline);
                        f.flex(1.0);
                        f.spacer(0.0);
                        f.set_theme(muted_theme);
                        display_label(f, state_label, type_scale.footnote);
                        f.set_theme(base_theme);
                    },
                );
            },
        );

        // Two framed charts: upload over download, equal width, fed by the
        // shared throughput histories.
        let chart_top = rect.y + pad + header_h + 8.0;
        let chart_gap = 10.0;
        let chart_w = ((rect.w - pad * 2.0 - chart_gap) * 0.5).max(40.0);
        let chart_h = ((rect.h - chart_top - pad - 56.0) * 0.62).max(48.0);
        let caption_y = chart_top + chart_h + 6.0;
        for (index, chart) in [("up", &self.net_tx_history), ("down", &self.net_rx_history)]
            .iter()
            .enumerate()
        {
            let chart_rect = Rect {
                x: rect.x + pad + index as f32 * (chart_w + chart_gap),
                y: chart_top,
                w: chart_w,
                h: chart_h,
            };
            render_rate_chart(
                f,
                &format!("aegis-hud-net-{}", chart.0),
                chart.1,
                chart_rect,
                hud.accent,
                hud.border,
            );
        }
        // The captions: `Up 1.2M / s` / `Down 340K / s`.
        for (index, (label, value)) in [
            (i18n.text(Message::NetUp), &tx),
            (i18n.text(Message::NetDown), &rx),
        ]
        .iter()
        .enumerate()
        {
            let caption_rect = Rect {
                x: rect.x + pad + index as f32 * (chart_w + chart_gap),
                y: caption_y,
                w: chart_w,
                h: 40.0,
            };
            f.place(
                &format!("aegis-hud-net-caption-{}", index),
                &chrome_place(caption_rect, transparent()),
                |f| {
                    f.column_ex(
                        &LayoutOpts {
                            width: caption_rect.w,
                            height: caption_rect.h,
                            gap: 2.0,
                            cross: Align::Start,
                            ..Default::default()
                        },
                        |f| {
                            f.set_theme(muted_theme);
                            display_label(f, label, type_scale.footnote);
                            f.set_theme(base_theme);
                            display_label(f, value, type_scale.headline);
                        },
                    );
                },
            );
        }
        f.set_theme(original);
    }

    /// The interactive tray grid: left-click activates, right-click opens
    /// the host-rendered dbusmenu popover (or `SecondaryActivate`).
    #[allow(dead_code)]
    pub(super) fn render_tray_section(
        &mut self,
        f: &mut Frame,
        area: Rect,
        cursor: (f32, f32),
        i18n: &Localizer,
    ) {
        let hud = Hud::classic();
        let type_scale = self.design.typography;
        let mut cells = self.sni_cells();
        if cells.is_empty() {
            let original = f.theme();
            let muted = themes::hud_muted(themes::hud(&hud), &hud);
            f.set_theme(muted);
            f.place(
                "aegis-hud-tray-empty",
                &chrome_place(area, transparent()),
                |f| {
                    f.row_ex(
                        &LayoutOpts {
                            width: area.w,
                            height: area.h,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            f.flex(1.0);
                            f.spacer(0.0);
                            display_label(f, i18n.text(Message::NoTrayItems), type_scale.body);
                            f.flex(1.0);
                            f.spacer(0.0);
                        },
                    );
                },
            );
            f.set_theme(original);
            return;
        }
        let cols = ((area.w + 8.0) / TRAY_CELL_W).max(1.0) as usize;
        // Distill the per-cell visuals before the layout closures: those
        // capture disjoint borrows, so `self` method calls happen here.
        let fallback_themed = self
            .themed_icon("application-x-executable-symbolic")
            .map(|icon| icon as *mut lens::sys::flux_image);
        let visuals: Vec<TrayCellVisual> = cells
            .iter()
            .map(|cell| TrayCellVisual {
                key: cell.key.clone(),
                title: truncate(&cell.title, 12),
                has_menu: cell.has_menu,
                texture: if cell.textured {
                    self.tray
                        .as_ref()
                        .and_then(|tray| tray.textures.get(&cell.key))
                        .map(|(_, image)| image.as_raw() as *mut lens::sys::flux_image)
                } else {
                    None
                },
                fallback: fallback_themed,
            })
            .collect();
        // Clicks are collected during layout and applied afterwards —
        // opening a popover mutates `self`, which the closures borrow.
        let mut activations: Vec<String> = Vec::new();
        let mut secondary: Vec<(String, bool)> = Vec::new();
        let mut resolved: Vec<(String, Rect)> = Vec::new();
        let original = f.theme();
        f.set_theme(themes::hud(&hud));
        f.place("aegis-hud-tray", &chrome_place(area, transparent()), |f| {
            f.column_ex(&sized(area.w, area.h), |f| {
                f.flex(1.0);
                f.scroll("aegis-hud-tray-scroll", |f| {
                    f.column_ex(
                        &LayoutOpts {
                            gap: 8.0,
                            cross: Align::Start,
                            ..Default::default()
                        },
                        |f| {
                            for row in visuals.chunks(cols) {
                                f.row_ex(
                                    &LayoutOpts {
                                        gap: 8.0,
                                        height: TRAY_CELL_H - 8.0,
                                        cross: Align::Start,
                                        ..Default::default()
                                    },
                                    |f| {
                                        for cell in row {
                                            let (response, _) = f.pressable_row(
                                                &format!("aegis-hud-tray-cell-{}", cell.key),
                                                &cell.title,
                                                &LayoutOpts {
                                                    width: TRAY_CELL_W - 8.0,
                                                    height: TRAY_CELL_H - 8.0,
                                                    gap: 3.0,
                                                    pad: 6.0,
                                                    radius: 10.0,
                                                    cross: Align::Center,
                                                    ..Default::default()
                                                },
                                                |f, _| {
                                                    f.column_ex(
                                                        &LayoutOpts {
                                                            flex: 1.0,
                                                            gap: 3.0,
                                                            cross: Align::Center,
                                                            ..Default::default()
                                                        },
                                                        |f| {
                                                            match cell.texture {
                                                                Some(texture) => unsafe {
                                                                    f.image(texture, 28.0, 28.0)
                                                                },
                                                                None => match cell.fallback {
                                                                    Some(icon) => unsafe {
                                                                        f.image(icon, 26.0, 26.0)
                                                                    },
                                                                    None => {
                                                                        f.icon(Icon::FileText, 22.0)
                                                                    }
                                                                },
                                                            }
                                                            display_label(
                                                                f,
                                                                &cell.title,
                                                                type_scale.caption,
                                                            );
                                                        },
                                                    );
                                                },
                                            );
                                            resolved.push((cell.key.clone(), response.rect));
                                            if response.clicked {
                                                activations.push(cell.key.clone());
                                            } else if response.right_clicked {
                                                secondary.push((cell.key.clone(), cell.has_menu));
                                            }
                                        }
                                    },
                                );
                            }
                        },
                    );
                });
            });
        });
        f.set_theme(original);
        for (key, rect) in &resolved {
            if let Some(cell) = cells.iter_mut().find(|cell| &cell.key == key) {
                cell.rect = *rect;
            }
        }
        let (x, y) = (cursor.0 as i32, cursor.1 as i32);
        for key in activations {
            self.send_tray_command(TrayCommand::Activate { key, x, y });
        }
        for (key, has_menu) in secondary {
            // Items that expose a Menu object path get the host-rendered
            // popover; everything else keeps the SNI `SecondaryActivate`
            // fallback.
            if has_menu {
                self.menu_open_for = Some(key.clone());
                self.menu_path = vec![0];
                self.menu_owner = resolved
                    .iter()
                    .find(|(owner, _)| owner == &key)
                    .map(|(_, rect)| *rect)
                    .unwrap_or(self.menu_owner);
                self.menu_just_opened = true;
                self.send_tray_command(TrayCommand::FetchMenu { key });
            } else {
                self.send_tray_command(TrayCommand::SecondaryActivate { key, x, y });
            }
        }
        // Re-anchor the open popover to its owner cell; close it when the
        // backing item vanished from the snapshot.
        if let Some(key) = self.menu_open_for.clone() {
            if let Some(cell) = cells.iter().find(|cell| cell.key == key) {
                self.menu_owner = cell.rect;
            } else {
                self.menu_open_for = None;
                self.menu_path.clear();
                self.send_tray_command(TrayCommand::CloseMenu { key });
            }
        }
    }

    /// The notification list, newest first, as recessed dark glass cards in
    /// a scroll area; a card click dismisses.
    pub(super) fn render_messages_section(
        &mut self,
        f: &mut Frame,
        area: Rect,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = Hud::classic();
        let type_scale = self.design.typography;
        let notifications = self.notification_snapshot();
        let original = f.theme();
        if notifications.is_empty() {
            let muted = themes::hud_muted(themes::hud(&hud), &hud);
            f.set_theme(muted);
            f.place(
                "aegis-hud-messages-empty",
                &chrome_place(area, transparent()),
                |f| {
                    f.row_ex(
                        &LayoutOpts {
                            width: area.w,
                            height: area.h,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            f.flex(1.0);
                            f.spacer(0.0);
                            display_label(f, i18n.text(Message::NoNotifications), type_scale.body);
                            f.flex(1.0);
                            f.spacer(0.0);
                        },
                    );
                },
            );
            f.set_theme(original);
            return;
        }
        let base = themes::hud(&hud);
        let row_theme = base;
        let muted_theme = themes::hud_muted(base, &hud);
        f.set_theme(row_theme);
        f.place(
            "aegis-hud-messages",
            &chrome_place(area, transparent()),
            |f| {
                f.column_ex(&sized(area.w, area.h), |f| {
                    f.flex(1.0);
                    f.scroll("aegis-hud-messages-scroll", |f| {
                        f.column_ex(
                            &LayoutOpts {
                                gap: 6.0,
                                cross: Align::Stretch,
                                ..Default::default()
                            },
                            |f| {
                                for notification in notifications.iter().rev() {
                                    let summary = truncate(&notification.summary, 44);
                                    let body = truncate(&notification.body, 64);
                                    let (response, _) = f.pressable_row(
                                        &format!("aegis-hud-message-{}", notification.id),
                                        &summary,
                                        &LayoutOpts {
                                            height: 58.0,
                                            gap: 2.0,
                                            pad: 4.0,
                                            radius: 8.0,
                                            cross: Align::Center,
                                            ..Default::default()
                                        },
                                        |f, _| {
                                            f.column_ex(
                                                &LayoutOpts {
                                                    gap: 2.0,
                                                    cross: Align::Start,
                                                    ..Default::default()
                                                },
                                                |f| {
                                                    display_label(f, &summary, type_scale.body);
                                                    if !body.is_empty() {
                                                        f.set_theme(muted_theme);
                                                        display_label(f, &body, type_scale.label);
                                                        f.set_theme(row_theme);
                                                    }
                                                },
                                            );
                                        },
                                    );
                                    if response.clicked {
                                        out.dismissed_notification = Some(notification.id);
                                    }
                                }
                            },
                        );
                    });
                });
            },
        );
        f.set_theme(original);
    }

    /// Render the dbusmenu popover. The visible rows come from walking
    /// `menu.root.children` along `self.menu_path`. Submenu rows push onto
    /// `menu_path`, leaf rows send `MenuEvent` and dismiss the popover, and
    /// click-away closes it unless the press falls on the owner tray cell.
    pub(super) fn render_tray_menu(
        &mut self,
        f: &mut Frame,
        menu: &MenuState,
        display: (f32, f32),
        cursor: (f32, f32),
        pressed: bool,
    ) {
        // If a targeted submenu id no longer exists (the worker truncated the
        // tree on `LayoutUpdated`), pop back to the nearest valid level.
        while aegis_tray::visible_children(&menu.root, &self.menu_path).is_none()
            && self.menu_path.len() > 1
        {
            self.menu_path.pop();
        }
        let visible = match aegis_tray::visible_children(&menu.root, &self.menu_path) {
            Some(rows) => rows,
            None => return,
        };

        let popover_bounds = menu_bounds(self.menu_owner, visible, display);
        let in_owner = contains(self.menu_owner, cursor.0, cursor.1);
        let in_popover = contains(popover_bounds, cursor.0, cursor.1);
        if !self.menu_just_opened && pressed && !in_owner && !in_popover {
            self.close_menu(menu.key.clone());
            return;
        }
        self.menu_just_opened = false;

        let hud = Hud::classic();
        let type_scale = self.design.typography;
        let original_theme = f.theme();
        let menu_theme = themes::hud(&hud);
        let dim_theme = themes::hud_muted(menu_theme, &hud);

        let header_visible = self.menu_path.len() > 1;
        let mut action: Option<MenuRowAction> = None;
        f.set_theme(menu_theme);
        f.place(
            "aegis-hud-sni-menu",
            &chrome_place(popover_bounds, materials::hud_panel(&hud)),
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: popover_bounds.w,
                        height: popover_bounds.h,
                        gap: 0.0,
                        pad: MENU_PAD,
                        ..Default::default()
                    },
                    |f| {
                        let inner_w = popover_bounds.w - MENU_PAD * 2.0;
                        if header_visible {
                            f.size_next(inner_w, MENU_HEADER_HEIGHT);
                            f.push_id("hud-menu-back");
                            if f.selectable("‹ Back", false) {
                                action = Some(MenuRowAction::Back);
                            }
                            f.pop_id();
                        }
                        for row in visible.iter() {
                            if !row.visible {
                                continue;
                            }
                            if row.kind == aegis_tray::MenuEntryKind::Separator {
                                f.size_next(inner_w, MENU_SECTION_HEIGHT);
                                f.separator();
                                continue;
                            }
                            f.size_next(inner_w, MENU_ROW_HEIGHT);
                            f.push_id(&format!("hud-menu-row-{}", row.id));
                            if !row.enabled {
                                // Disabled rows render as inert labels with a
                                // dim foreground — selectable would still
                                // capture the click, which the dbusmenu spec
                                // forbids.
                                f.set_theme(dim_theme);
                                display_label(
                                    f,
                                    &truncate(&menu_row_label(row), 32),
                                    type_scale.body,
                                );
                                f.set_theme(menu_theme);
                            } else if f.selectable(&truncate(&menu_row_label(row), 32), false) {
                                if row.has_submenu {
                                    action = Some(MenuRowAction::Descend(row.id));
                                } else {
                                    action = Some(MenuRowAction::Click(row.id));
                                }
                            }
                            f.pop_id();
                        }
                    },
                );
            },
        );
        f.set_theme(original_theme);

        match action {
            Some(MenuRowAction::Back) => {
                self.menu_path.pop();
            }
            Some(MenuRowAction::Descend(id)) => {
                self.menu_path.push(id);
            }
            Some(MenuRowAction::Click(id)) => {
                self.send_tray_command(TrayCommand::MenuEvent {
                    key: menu.key.clone(),
                    id,
                });
                self.close_menu(menu.key.clone());
            }
            None => {}
        }
    }

    /// The bottom-right work mode quick switcher: a floating liquid-glass panel
    /// hosting 3 segmented pills for session power mode selection (Balanced, Awake, Secure),
    /// with an informative live status subtitle.
    pub(super) fn render_work_mode_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        if rect.w < 120.0 || rect.h < 40.0 {
            return;
        }
        let hud = Hud::classic();
        let type_scale = self.design.typography;
        let slide = (1.0 - ease_out_cubic(progress)) * 24.0;
        let rect = Rect {
            x: rect.x + slide,
            ..rect
        };

        // Floating glass backing
        f.place(
            "aegis-hud-work-mode-glass",
            &chrome_place(
                rect,
                LayoutOpts {
                    bg: Color::rgba(24, 26, 36, 38),
                    border: Color::rgba(255, 255, 255, 16),
                    border_width: 0.75,
                    radius: self.design.radii.glass_panel,
                    pad: 0.0,
                    ..surface_layout()
                },
            ),
            |f| {
                f.column_ex(&sized(rect.w, rect.h), |_| {});
            },
        );

        let pad_h = 14.0;
        let pad_v = 10.0;
        let current_mode = self.status.power_mode;

        let original = f.theme();
        let base_theme = themes::hud(&hud);
        let muted_theme = themes::hud_muted(base_theme, &hud);

        // Header: "WORK MODE" / "工作模式"
        f.set_theme(base_theme);
        let header_h = 22.0;
        f.place(
            "aegis-hud-work-mode-header",
            &chrome_place(
                Rect {
                    x: rect.x + pad_h,
                    y: rect.y + pad_v,
                    w: (rect.w - pad_h * 2.0).max(1.0),
                    h: header_h,
                },
                transparent(),
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: (rect.w - pad_h * 2.0).max(1.0),
                        height: header_h,
                        gap: 8.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        f.icon(Icon::Zap, 15.0);
                        display_label(f, i18n.text(Message::WorkMode), type_scale.footnote);
                        f.flex(1.0);
                        f.spacer(0.0);
                        f.set_theme(muted_theme);
                        let mode_name = match current_mode {
                            aegis_model::power::PowerMode::Balanced => {
                                i18n.text(Message::PowerModeBalanced)
                            }
                            aegis_model::power::PowerMode::Awake => {
                                i18n.text(Message::PowerModeAwake)
                            }
                            aegis_model::power::PowerMode::Secure => {
                                i18n.text(Message::PowerModeSecure)
                            }
                        };
                        display_label(f, mode_name, type_scale.footnote);
                        f.set_theme(base_theme);
                    },
                );
            },
        );

        // 3 Segmented Pills: Balanced, Awake, Secure
        let pills_top = rect.y + pad_v + header_h + 6.0;
        let pill_h = 32.0_f32.min((rect.h - pad_v * 2.0 - header_h - 10.0).max(20.0));
        let modes = [
            (
                aegis_model::power::PowerMode::Balanced,
                i18n.text(Message::PowerModeBalanced),
                Icon::Sliders,
            ),
            (
                aegis_model::power::PowerMode::Awake,
                i18n.text(Message::PowerModeAwake),
                Icon::Play,
            ),
            (
                aegis_model::power::PowerMode::Secure,
                i18n.text(Message::PowerModeSecure),
                Icon::Shield,
            ),
        ];

        let content_w = (rect.w - pad_h * 2.0).max(1.0);
        let pill_gap = 6.0;
        let pill_w = ((content_w - pill_gap * 2.0) / 3.0).max(20.0);

        for (index, (mode, label, icon)) in modes.into_iter().enumerate() {
            let active = current_mode == mode;
            let pill_rect = Rect {
                x: rect.x + pad_h + index as f32 * (pill_w + pill_gap),
                y: pills_top,
                w: pill_w,
                h: pill_h,
            };

            let bg = if active {
                Color::rgba(0, 255, 255, 40)
            } else {
                Color::rgba(255, 255, 255, 12)
            };
            let border = if active {
                hud.accent
            } else {
                Color::rgba(255, 255, 255, 20)
            };
            let border_width = if active { 1.2 } else { 0.75 };
            let fg = if active {
                Color::rgba(255, 255, 255, 255)
            } else {
                Color::rgba(180, 190, 210, 200)
            };
            let icon_color = if active {
                hud.accent
            } else {
                Color::rgba(150, 160, 185, 180)
            };

            let pill_theme = themes::hud(&hud).with_fg(fg);
            f.set_theme(pill_theme);

            f.place(
                &format!("aegis-hud-work-pill-place-{index}"),
                &chrome_place(pill_rect, transparent()),
                |f| {
                    let (response, _) = f.pressable_row(
                        &format!("aegis-hud-work-pill-{index}"),
                        label,
                        &LayoutOpts {
                            width: pill_w,
                            height: pill_h,
                            bg,
                            border,
                            border_width,
                            radius: pill_h * 0.5,
                            cross: Align::Center,
                            gap: 4.0,
                            pad: 6.0,
                            ..Default::default()
                        },
                        |f, _| {
                            f.set_theme(pill_theme.with_fg(icon_color));
                            f.icon(icon, 13.0);
                            f.set_theme(pill_theme);
                            let short_label = truncate(label, (pill_w / 8.5).max(3.0) as usize);
                            display_label(f, &short_label, type_scale.caption);
                        },
                    );

                    if response.clicked && !active {
                        out.system_actions.push(SystemAction::SetPowerMode { mode });
                    }
                },
            );
        }

        // Subtitle / hint describing current mode behavior (when vertical space permits)
        if rect.h >= 90.0 {
            let hint_y = pills_top + pill_h + 6.0;
            let hint_h = 16.0;
            let hint_text = match current_mode {
                aegis_model::power::PowerMode::Balanced => "Dim → Lock → Blank → Suspend",
                aegis_model::power::PowerMode::Awake => "Screen stays lit & unlocked",
                aegis_model::power::PowerMode::Secure => "Lock on idle, screen stays lit",
            };

            f.set_theme(muted_theme);
            f.place(
                "aegis-hud-work-mode-hint",
                &chrome_place(
                    Rect {
                        x: rect.x + pad_h,
                        y: hint_y,
                        w: content_w,
                        h: hint_h,
                    },
                    transparent(),
                ),
                |f| {
                    f.row_ex(
                        &LayoutOpts {
                            width: content_w,
                            height: hint_h,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            display_label(f, hint_text, type_scale.caption);
                        },
                    );
                },
            );
        }
        f.set_theme(original);
    }

    /// The bottom-right power & session actions panel:
    /// quick actions for Lock Screen, Suspend, Restart, and Power Off (Shutdown).
    pub(super) fn render_power_session_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        if rect.w < 120.0 || rect.h < 40.0 {
            return;
        }
        let hud = Hud::classic();
        let type_scale = self.design.typography;
        let slide = (1.0 - ease_out_cubic(progress)) * 24.0;
        let rect = Rect {
            x: rect.x + slide,
            ..rect
        };

        // Floating glass backing
        f.place(
            "aegis-hud-power-session-glass",
            &chrome_place(
                rect,
                LayoutOpts {
                    bg: Color::rgba(24, 26, 36, 38),
                    border: Color::rgba(255, 255, 255, 16),
                    border_width: 0.75,
                    radius: self.design.radii.glass_panel,
                    pad: 0.0,
                    ..surface_layout()
                },
            ),
            |f| {
                f.column_ex(&sized(rect.w, rect.h), |_| {});
            },
        );

        let pad_h = 14.0;
        let pad_v = 10.0;
        let original = f.theme();
        let base_theme = themes::hud(&hud);

        // Header: "POWER & SESSION" / "电源与会话"
        f.set_theme(base_theme);
        let header_h = 22.0;
        f.place(
            "aegis-hud-power-session-header",
            &chrome_place(
                Rect {
                    x: rect.x + pad_h,
                    y: rect.y + pad_v,
                    w: (rect.w - pad_h * 2.0).max(1.0),
                    h: header_h,
                },
                transparent(),
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: (rect.w - pad_h * 2.0).max(1.0),
                        height: header_h,
                        gap: 8.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        f.icon(Icon::Radio, 15.0);
                        display_label(f, i18n.text(Message::PowerAndSession), type_scale.footnote);
                    },
                );
            },
        );

        // Action Buttons: 2x2 grid
        // [ 🔒 Lock Now ]    [ 💤 Suspend ]
        // [ 🔄 Restart ]     [ ⏻ Power Off ]
        let actions_top = rect.y + pad_v + header_h + 6.0;
        let available_h = (rect.h - pad_v * 2.0 - header_h - 10.0).max(20.0);
        let btn_h = 32.0_f32.min(((available_h - 6.0) * 0.5).max(18.0));
        let btn_gap = 8.0;
        let content_w = (rect.w - pad_h * 2.0).max(1.0);
        let btn_w = ((content_w - btn_gap) * 0.5).max(30.0);

        enum PowerActionKind {
            Lock,
            Suspend,
            Restart,
            PowerOff,
        }

        let buttons = [
            (
                PowerActionKind::Lock,
                i18n.text(Message::LockNow),
                Icon::Shield,
                false,
            ),
            (
                PowerActionKind::Suspend,
                i18n.text(Message::Suspend),
                Icon::Pause,
                false,
            ),
            (
                PowerActionKind::Restart,
                i18n.text(Message::Restart),
                Icon::RefreshCw,
                false,
            ),
            (
                PowerActionKind::PowerOff,
                i18n.text(Message::PowerOff),
                Icon::Slash,
                true,
            ),
        ];

        for (index, (kind, label, icon, is_destructive)) in buttons.into_iter().enumerate() {
            let row = index / 2;
            let col = index % 2;
            let btn_rect = Rect {
                x: rect.x + pad_h + col as f32 * (btn_w + btn_gap),
                y: actions_top + row as f32 * (btn_h + 6.0),
                w: btn_w,
                h: btn_h,
            };

            let bg = if is_destructive {
                Color::rgba(255, 60, 60, 20)
            } else {
                Color::rgba(255, 255, 255, 12)
            };
            let border = if is_destructive {
                Color::rgba(255, 80, 80, 50)
            } else {
                Color::rgba(255, 255, 255, 20)
            };
            let fg = if is_destructive {
                Color::rgba(255, 170, 170, 240)
            } else {
                Color::rgba(200, 210, 230, 220)
            };
            let icon_color = if is_destructive {
                Color::rgba(255, 100, 100, 255)
            } else {
                hud.accent
            };

            let btn_theme = themes::hud(&hud).with_fg(fg);
            f.set_theme(btn_theme);

            f.place(
                &format!("aegis-hud-power-btn-place-{index}"),
                &chrome_place(btn_rect, transparent()),
                |f| {
                    let (response, _) = f.pressable_row(
                        &format!("aegis-hud-power-btn-{index}"),
                        label,
                        &LayoutOpts {
                            width: btn_w,
                            height: btn_h,
                            bg,
                            border,
                            border_width: 0.75,
                            radius: 8.0,
                            cross: Align::Center,
                            gap: 6.0,
                            pad: 6.0,
                            ..Default::default()
                        },
                        |f, _| {
                            f.set_theme(btn_theme.with_fg(icon_color));
                            f.icon(icon, 13.0);
                            f.set_theme(btn_theme);
                            let short_label = truncate(label, (btn_w / 8.0).max(3.0) as usize);
                            display_label(f, &short_label, type_scale.caption);
                        },
                    );

                    if response.clicked {
                        match kind {
                            PowerActionKind::Lock => {
                                out.lock = true;
                            }
                            PowerActionKind::Suspend => {
                                out.system_actions.push(SystemAction::Suspend);
                            }
                            PowerActionKind::Restart => {
                                out.system_actions.push(SystemAction::Reboot);
                            }
                            PowerActionKind::PowerOff => {
                                out.system_actions.push(SystemAction::PowerOff);
                            }
                        }
                    }
                },
            );
        }
        f.set_theme(original);
    }
}

/// Project the two Quick Controls toggles onto the session power mode
/// (ADR-0140).
///
/// `keep_awake` is the display axis ("never blank the screen"); `auto_lock`
/// is the security axis ("lock on schedule"). The one combination the
/// security boundary forbids — an unlocked session that blanks — projects
/// onto [`aegis_model::power::PowerMode::Awake`]: with no automatic lock there is nothing to
/// power off or suspend behind, so dimming is the strongest idle response
/// the pipeline may keep. The toggles then read the mode back honestly:
/// "keep awake" shows on (the display indeed never blanks) even though the
/// user turned it off, telling them the security axis won the conflict.
pub(crate) fn power_mode_for(keep_awake: bool, auto_lock: bool) -> aegis_model::power::PowerMode {
    use aegis_model::power::PowerMode;
    match (keep_awake, auto_lock) {
        (false, true) => PowerMode::Balanced,
        (true, true) => PowerMode::Secure,
        (_, false) => PowerMode::Awake,
    }
}
