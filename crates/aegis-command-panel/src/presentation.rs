use super::*;

// ---- rendering -----------------------------------------------------------

impl CommandPanel {
    /// Bounds of the currently open dbusmenu popover, if any.
    pub(super) fn open_popover_bounds(&mut self, display: (f32, f32)) -> Option<Rect> {
        let key = self.menu_open_for.clone()?;
        let menu = self.menu_snapshot().filter(|menu| menu.key == key)?;
        aegis_tray::visible_children(&menu.root, &self.menu_path)
            .map(|visible| menu_bounds(self.menu_owner, visible, display))
    }

    /// The header band: identity zone (ringed avatar, display name,
    /// `@username · groups`) on the left and the machine monitor (chassis
    /// glyph plus utilization gauges) on the right, separated by a hairline
    /// divider. Slides in from the left like the old menu panel.
    pub(super) fn render_header_band(
        &self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        i18n: &Localizer,
    ) {
        let sao = Sao::classic();
        let slide = (1.0 - ease_out_cubic(progress)) * -24.0;
        let rect = Rect {
            x: rect.x + slide,
            ..rect
        };
        f.layer(
            "aegis-sao-header-panel",
            rect,
            &OverlayOpts {
                bg: fade_color(sao.surface, progress),
                border: fade_color(sao.border, progress),
                border_width: 1.0,
                radius: 16.0,
                pad: 0.0,
                ..Default::default()
            },
            |f| {
                f.column_ex(&sized(rect.w, rect.h), |_| {});
            },
        );

        let pad = 16.0;
        let inner_y = rect.y + pad;
        let inner_h = (rect.h - pad * 2.0).max(1.0);
        let center_y = rect.y + rect.h * 0.5;
        let base_theme = themes::sao(&sao);
        let muted_theme = themes::sao_muted(base_theme, &sao);
        let original = f.theme();

        // -- identity zone: ringed avatar + name lines (~270px) ------------
        let avatar_center = (rect.x + pad + 42.0, center_y);
        // The avatar crate supplies only the portrait texture. Its surrounding
        // chrome belongs to this host: a flat warm graphite disc replaces the
        // old blue-white gradient fallback, while a quiet amber keyline keeps
        // the identity tied to the panel's accent language.
        render_disc(
            f,
            "aegis-sao-avatar-backdrop",
            avatar_center,
            72.0,
            Color::rgba(24, 23, 22, fade_alpha(246, progress)),
        );
        render_ring(
            f,
            "aegis-sao-avatar-ring",
            avatar_center,
            80.0,
            fade_color(sao.accent.with_alpha(132), progress),
            1.0,
        );
        let avatar_rect = Rect {
            x: avatar_center.0 - 36.0,
            y: avatar_center.1 - 36.0,
            w: 72.0,
            h: 72.0,
        };
        match &self.avatar {
            Some(avatar) => {
                let texture = avatar.texture().as_raw();
                f.layer("aegis-sao-avatar", avatar_rect, &transparent(), |f| {
                    f.row_ex(&sized(72.0, 72.0), |f| {
                        unsafe {
                            f.image_tinted(
                                texture as *mut lens::sys::flux_image,
                                72.0,
                                72.0,
                                Color::rgba(255, 255, 255, fade_alpha(255, progress)),
                            )
                        };
                    });
                });
            }
            None => {
                f.set_theme(base_theme.with_fg(Color::rgba(
                    236,
                    232,
                    222,
                    fade_alpha(238, progress),
                )));
                f.layer(
                    "aegis-sao-avatar-initials",
                    avatar_rect,
                    &transparent(),
                    |f| {
                        f.row_ex(
                            &LayoutOpts {
                                width: 72.0,
                                height: 72.0,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            |f| {
                                f.flex(1.0);
                                f.spacer(0.0);
                                f.label_compact_sized(&self.identity.initials, 22.0);
                                f.flex(1.0);
                                f.spacer(0.0);
                            },
                        );
                    },
                );
            }
        }

        let text_x = rect.x + pad + 84.0 + 14.0;
        let text_w = (rect.x + pad + 270.0 - text_x).max(40.0);
        let display_name = truncate(
            &self.identity.display_name,
            (text_w / 9.0).max(4.0) as usize,
        );
        f.set_theme(faded_theme(base_theme, progress));
        f.layer(
            "aegis-sao-identity-name",
            Rect {
                x: text_x,
                y: center_y - 21.0,
                w: text_w,
                h: 22.0,
            },
            &transparent(),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: text_w,
                        height: 22.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| f.label_compact_sized(&display_name, 16.0),
                );
            },
        );
        let mut sub_line = format!("@{}", self.identity.username);
        if !self.identity.groups.is_empty() {
            sub_line.push_str(" · ");
            sub_line.push_str(&self.identity.groups.join(", "));
        }
        let sub_line = truncate(&sub_line, (text_w / 5.8).max(8.0) as usize);
        f.set_theme(faded_theme(muted_theme, progress));
        f.layer(
            "aegis-sao-identity-sub",
            Rect {
                x: text_x,
                y: center_y + 3.0,
                w: text_w,
                h: 15.0,
            },
            &transparent(),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: text_w,
                        height: 15.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| f.label_compact_sized(&sub_line, 10.5),
                );
            },
        );

        // -- divider ---------------------------------------------------------
        let divider_x = rect.x + pad + 270.0 + 12.0;
        f.layer(
            "aegis-sao-header-divider",
            Rect {
                x: divider_x,
                y: rect.y + 22.0,
                w: 1.0,
                h: (rect.h - 44.0).max(1.0),
            },
            &OverlayOpts {
                bg: fade_color(sao.border, progress),
                border: Color::TRANSPARENT,
                radius: 0.0,
                pad: 0.0,
                ..Default::default()
            },
            |_| {},
        );

        // -- machine zone: chassis glyph + gauge rows ------------------------
        let machine_x = divider_x + 12.0;
        let machine_right = rect.x + rect.w - pad;
        if machine_right - machine_x < 200.0 {
            f.set_theme(original);
            return;
        }

        // Chassis glyph: a thin-line machine pictogram built from layer rects.
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
        let muted_line = fade_color(sao.text_muted, progress);
        let outline = |radius: f32| OverlayOpts {
            bg: Color::TRANSPARENT,
            border: muted_line,
            border_width: 1.2,
            radius,
            pad: 0.0,
            ..Default::default()
        };
        let filled = |radius: f32| OverlayOpts {
            bg: muted_line,
            border: Color::TRANSPARENT,
            radius,
            pad: 0.0,
            ..Default::default()
        };
        match self.stats.chassis {
            ChassisKind::Laptop => {
                let screen = Rect {
                    x: glyph_cx - 18.0,
                    y: glyph_top,
                    w: 36.0,
                    h: 24.0,
                };
                f.layer("aegis-sao-chassis-screen", screen, &outline(3.0), |_| {});
                let base = Rect {
                    x: glyph_cx - 22.0,
                    y: glyph_top + 24.0 + 2.0,
                    w: 44.0,
                    h: 2.5,
                };
                f.layer("aegis-sao-chassis-base", base, &filled(1.25), |_| {});
            }
            ChassisKind::Desktop => {
                let monitor = Rect {
                    x: glyph_cx - 17.0,
                    y: glyph_top,
                    w: 34.0,
                    h: 22.0,
                };
                f.layer("aegis-sao-chassis-screen", monitor, &outline(2.0), |_| {});
                let stand = Rect {
                    x: glyph_cx - 1.0,
                    y: glyph_top + 22.0,
                    w: 2.0,
                    h: 7.0,
                };
                f.layer("aegis-sao-chassis-stand", stand, &filled(0.0), |_| {});
                let base = Rect {
                    x: glyph_cx - 8.0,
                    y: glyph_top + 29.0,
                    w: 16.0,
                    h: 2.0,
                };
                f.layer("aegis-sao-chassis-base", base, &filled(1.0), |_| {});
            }
        }
        f.set_theme(faded_theme(muted_theme, progress));
        f.layer(
            "aegis-sao-chassis-label",
            Rect {
                x: machine_x,
                y: rect.y + rect.h - pad - 13.0,
                w: 56.0,
                h: 13.0,
            },
            &transparent(),
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
                        f.label_compact_sized(chassis_label, 9.0);
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
        // The band fits five 14px rows; when every source applies the
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
                progress,
                i18n,
            );
            row_y += ROW_H + ROW_GAP;
        }
        f.set_theme(original);
    }

    /// One gauge row of the header band's machine monitor: a 40px label
    /// cell, the bar/sparkline zone, and a 58px right-aligned value cell.
    pub(super) fn render_gauge_row(
        &self,
        f: &mut Frame,
        gauge: &Gauge,
        index: usize,
        row: Rect,
        progress: f32,
        i18n: &Localizer,
    ) {
        let sao = Sao::classic();
        let base_theme = themes::sao(&sao);
        let muted_theme = themes::sao_muted(base_theme, &sao);
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

        // Label cell: a 9.5pt caption, or a 10px icon for NET/BAT.
        let icon_label: Option<(Icon, Color)> = match gauge {
            Gauge::Net { .. } => Some((Icon::Globe, sao.text_muted)),
            Gauge::Battery { charging, .. } => Some((
                Icon::Zap,
                if *charging {
                    sao.accent
                } else {
                    sao.text_muted
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
            f.set_theme(faded_theme(muted_theme, progress));
        } else if let Some((_, color)) = icon_label {
            f.set_theme(faded_theme(base_theme.with_fg(color), progress));
        }
        f.layer(
            &format!("aegis-sao-gauge-label-{index}"),
            label_rect,
            &transparent(),
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
                            f.label_compact_sized(text, 9.5);
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
                    progress,
                );
                (format!("{:.0}%", self.stats.cpu_percent), false)
            }
            Gauge::Gpu(gpu) => {
                gauge_bar(
                    f,
                    &format!("aegis-sao-gauge-bar-{index}"),
                    Rect {
                        x: bar_x,
                        y: row.y + (row.h - 4.0) * 0.5,
                        w: bar_w,
                        h: 4.0,
                    },
                    gpu / 100.0,
                    progress,
                );
                (format!("{gpu:.0}%"), false)
            }
            Gauge::Ram { fraction, value } => {
                gauge_bar(
                    f,
                    &format!("aegis-sao-gauge-bar-{index}"),
                    Rect {
                        x: bar_x,
                        y: row.y + (row.h - 4.0) * 0.5,
                        w: bar_w,
                        h: 4.0,
                    },
                    *fraction,
                    progress,
                );
                (value.clone(), false)
            }
            Gauge::Net { value } => (value.clone(), true),
            Gauge::Disk { fraction, value } => {
                gauge_bar(
                    f,
                    &format!("aegis-sao-gauge-bar-{index}"),
                    Rect {
                        x: bar_x,
                        y: row.y + (row.h - 4.0) * 0.5,
                        w: bar_w,
                        h: 4.0,
                    },
                    *fraction,
                    progress,
                );
                (value.clone(), false)
            }
            Gauge::Battery {
                fraction, value, ..
            } => {
                gauge_bar(
                    f,
                    &format!("aegis-sao-gauge-bar-{index}"),
                    Rect {
                        x: bar_x,
                        y: row.y + (row.h - 4.0) * 0.5,
                        w: bar_w,
                        h: 4.0,
                    },
                    *fraction,
                    progress,
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
        f.set_theme(faded_theme(base_theme, progress));
        f.layer(
            &format!("aegis-sao-gauge-value-{index}"),
            value_rect,
            &transparent(),
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
                        f.label_compact_sized(&value, 9.5);
                    },
                );
            },
        );
        f.set_theme(original);
    }

    /// The icon rail: one 44px circular button per section (ring + accent
    /// glyph at rest, solid accent disc when selected, a soft disc on
    /// hover), plus the panel's close button at the bottom. Fades in.
    pub(super) fn render_icon_rail(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        cursor: (f32, f32),
        pressed: bool,
    ) {
        let sao = Sao::classic();
        f.layer(
            "aegis-sao-rail-panel",
            rect,
            &OverlayOpts {
                bg: fade_color(sao.surface, progress),
                border: fade_color(sao.border, progress),
                border_width: 1.0,
                radius: 16.0,
                pad: 0.0,
                ..Default::default()
            },
            |f| {
                f.column_ex(&sized(rect.w, rect.h), |_| {});
            },
        );

        let cx = rect.x + rect.w * 0.5;
        let mut rail_action = None;
        for (index, section) in Section::ALL.iter().enumerate() {
            let center = (cx, rect.y + 18.0 + 22.0 + index as f32 * 56.0);
            let hit = Rect {
                x: center.0 - 22.0,
                y: center.1 - 22.0,
                w: 44.0,
                h: 44.0,
            };
            let hovered = contains(hit, cursor.0, cursor.1);
            let selected = self.section == *section;
            let glyph_color = if selected {
                render_disc(
                    f,
                    &format!("aegis-sao-rail-disc-{index}"),
                    center,
                    44.0,
                    fade_color(sao.accent, progress),
                );
                sao.on_accent
            } else {
                if hovered {
                    render_disc(
                        f,
                        &format!("aegis-sao-rail-hover-{index}"),
                        center,
                        44.0,
                        fade_color(sao.accent_soft, progress),
                    );
                }
                render_ring(
                    f,
                    &format!("aegis-sao-rail-ring-{index}"),
                    center,
                    44.0,
                    fade_color(sao.accent, progress),
                    1.5,
                );
                sao.accent
            };
            let original = f.theme();
            f.set_theme(faded_theme(
                themes::sao(&sao).with_fg(glyph_color),
                progress,
            ));
            f.layer(
                &format!("aegis-sao-rail-icon-{index}"),
                Rect {
                    x: center.0 - 15.0,
                    y: center.1 - 15.0,
                    w: 30.0,
                    h: 30.0,
                },
                &transparent(),
                |f| {
                    f.row_ex(
                        &LayoutOpts {
                            width: 30.0,
                            height: 30.0,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            f.flex(1.0);
                            f.spacer(0.0);
                            f.icon(section.icon(), 15.0);
                            f.flex(1.0);
                            f.spacer(0.0);
                        },
                    );
                },
            );
            f.set_theme(original);
            if pressed && hovered {
                rail_action = Some(RailAction::Select(*section));
            }
        }

        // Close button at the rail bottom: ring + X, same idiom.
        let center = (cx, rect.y + rect.h - 30.0);
        let hit = Rect {
            x: center.0 - 22.0,
            y: center.1 - 22.0,
            w: 44.0,
            h: 44.0,
        };
        let hovered = contains(hit, cursor.0, cursor.1);
        if hovered {
            render_disc(
                f,
                "aegis-sao-rail-close-hover",
                center,
                44.0,
                fade_color(sao.accent_soft, progress),
            );
        }
        render_ring(
            f,
            "aegis-sao-rail-close-ring",
            center,
            44.0,
            fade_color(sao.accent, progress),
            1.5,
        );
        let original = f.theme();
        f.set_theme(faded_theme(themes::sao(&sao).with_fg(sao.accent), progress));
        f.layer(
            "aegis-sao-rail-close-icon",
            Rect {
                x: center.0 - 15.0,
                y: center.1 - 15.0,
                w: 30.0,
                h: 30.0,
            },
            &transparent(),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: 30.0,
                        height: 30.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        f.flex(1.0);
                        f.spacer(0.0);
                        f.icon(Icon::X, 14.0);
                        f.flex(1.0);
                        f.spacer(0.0);
                    },
                );
            },
        );
        f.set_theme(original);
        if pressed && hovered {
            rail_action = Some(RailAction::Close);
        }

        match rail_action {
            Some(RailAction::Select(section)) => self.select_section(section),
            Some(RailAction::Close) => self.close(),
            None => {}
        }
    }

    /// The white content panel: section title header plus the active
    /// section's body, sliding up slightly as it reveals.
    pub(super) fn render_content_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        cursor: (f32, f32),
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let sao = Sao::classic();
        let rise = (1.0 - progress) * 16.0;
        let rect = Rect {
            y: rect.y + rise,
            ..rect
        };
        f.layer(
            "aegis-sao-content-panel",
            rect,
            &OverlayOpts {
                bg: fade_color(sao.surface, progress),
                border: fade_color(sao.border, progress),
                border_width: 1.0,
                radius: 16.0,
                pad: 0.0,
                ..Default::default()
            },
            |f| {
                f.column_ex(&sized(rect.w, rect.h), |_| {});
            },
        );

        // Header: the section title alone; the close button lives on the
        // icon rail now.
        let original = f.theme();
        f.set_theme(faded_theme(themes::sao(&sao), progress));
        let header = Rect {
            x: rect.x + 18.0,
            y: rect.y + 10.0,
            w: rect.w - 36.0,
            h: 34.0,
        };
        let section_label = self.section.label(i18n);
        f.layer("aegis-sao-content-header", header, &transparent(), |f| {
            f.row_ex(
                &LayoutOpts {
                    width: header.w,
                    height: header.h,
                    gap: 10.0,
                    cross: Align::Center,
                    ..Default::default()
                },
                |f| {
                    f.label_compact_sized(section_label, 15.0);
                },
            );
        });
        f.set_theme(original);

        let area = Rect {
            x: rect.x + 18.0,
            y: rect.y + 52.0,
            w: rect.w - 36.0,
            h: rect.h - 70.0,
        };
        match self.section {
            Section::System => self.render_system_section(f, area, progress, i18n, out),
            Section::Tray => self.render_tray_section(f, area, progress, cursor, i18n),
            Section::Messages => self.render_messages_section(f, area, progress, i18n, out),
        }
    }

    /// Quick settings, ported from the status bar's old status-and-controls
    /// panel and laid out as full-width SAO groups.
    pub(super) fn render_system_section(
        &mut self,
        f: &mut Frame,
        area: Rect,
        progress: f32,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let sao = Sao::classic();
        let original = f.theme();
        f.set_theme(faded_theme(themes::sao(&sao), progress));
        let status = self.status.clone();
        let volume_themed = self.themed_icon(volume_icon_name(&status));
        let network_themed = self.themed_icon(network_icon_name(status.network));
        let network_label = network_text(&status, i18n);
        let agent_indicator = agent_workspace_indicator(&self.interaction_domains, i18n);
        let agent_status_text = if agent_indicator.state == AgentWorkspaceState::Idle {
            agent_workspace_state_label(agent_indicator.state, i18n).to_string()
        } else {
            agent_indicator.label.clone()
        };
        // Group headers: a small muted caption above each control group,
        // replacing the old bare separators.
        let base_theme = faded_theme(themes::sao(&sao), progress);
        let muted_theme = faded_theme(themes::sao_muted(themes::sao(&sao), &sao), progress);
        let group_header = move |f: &mut Frame, label: &str| {
            f.set_theme(muted_theme);
            f.label_compact_sized(label, 10.5);
            f.set_theme(base_theme);
        };
        f.layer("aegis-sao-system", area, &transparent(), |f| {
            f.column_ex(&sized(area.w, area.h), |f| {
                f.flex(1.0);
                f.scroll("aegis-sao-system-scroll", |f| {
                    f.column_ex(
                        &LayoutOpts {
                            gap: 8.0,
                            cross: Align::Stretch,
                            ..Default::default()
                        },
                        |f| {
                            // Sound group.
                            group_header(f, i18n.text(Message::Sound));
                            f.row_ex(
                                &LayoutOpts {
                                    height: 22.0,
                                    gap: 8.0,
                                    cross: Align::Center,
                                    ..Default::default()
                                },
                                |f| {
                                    match volume_themed {
                                        Some(icon) => unsafe {
                                            f.image(icon as *mut lens::sys::flux_image, 16.0, 16.0)
                                        },
                                        None => f.icon(volume_icon(&status), 15.0),
                                    }
                                    f.label_compact_sized(i18n.text(Message::Sound), 12.5);
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                    f.label_compact_sized(
                                        &status
                                            .volume
                                            .map(|level| format!("{level}%"))
                                            .unwrap_or_else(|| "--".into()),
                                        11.0,
                                    );
                                },
                            );
                            if status.volume.is_some() {
                                let mut volume = status.volume.unwrap_or(0) as f32;
                                if f.slider("##sao-volume", &mut volume, 0.0, 100.0) {
                                    out.system_actions.push(SystemAction::SetVolume {
                                        level: volume.round().clamp(0.0, 100.0) as u8,
                                    });
                                }
                                let mut muted = status.muted;
                                if f.checkbox(i18n.text(Message::Muted), &mut muted) {
                                    out.system_actions.push(SystemAction::ToggleMute);
                                }
                            } else {
                                unavailable_control(f, i18n.text(Message::Volume), i18n);
                            }
                            f.spacer(2.0);

                            // Brightness group.
                            group_header(f, i18n.text(Message::Brightness));
                            f.row_ex(
                                &LayoutOpts {
                                    height: 22.0,
                                    gap: 8.0,
                                    cross: Align::Center,
                                    ..Default::default()
                                },
                                |f| {
                                    f.icon(Icon::Zap, 15.0);
                                    f.label_compact_sized(i18n.text(Message::Brightness), 12.5);
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                    f.label_compact_sized(
                                        &status
                                            .brightness
                                            .map(|level| format!("{level}%"))
                                            .unwrap_or_else(|| "--".into()),
                                        11.0,
                                    );
                                },
                            );
                            if status.brightness.is_some() {
                                let mut brightness = status.brightness.unwrap_or(1) as f32;
                                if f.slider("##sao-brightness", &mut brightness, 1.0, 100.0) {
                                    out.system_actions.push(SystemAction::SetBrightness {
                                        level: brightness.round().clamp(1.0, 100.0) as u8,
                                    });
                                }
                            } else {
                                unavailable_control(f, i18n.text(Message::Brightness), i18n);
                            }
                            f.spacer(2.0);

                            // Connectivity group.
                            group_header(f, i18n.text(Message::Connectivity));
                            f.row_ex(
                                &LayoutOpts {
                                    height: 22.0,
                                    gap: 8.0,
                                    cross: Align::Center,
                                    ..Default::default()
                                },
                                |f| {
                                    match network_themed {
                                        Some(icon) => unsafe {
                                            f.image(icon as *mut lens::sys::flux_image, 16.0, 16.0)
                                        },
                                        None => f.icon(Icon::Globe, 15.0),
                                    }
                                    f.label_compact_sized(i18n.text(Message::Connectivity), 12.5);
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                    f.label_compact_sized(network_label, 11.0);
                                },
                            );
                            if status.wifi_enabled.is_some() {
                                let mut wifi = status.wifi_enabled.unwrap_or(false);
                                if f.checkbox(i18n.text(Message::Wifi), &mut wifi) {
                                    out.system_actions
                                        .push(SystemAction::SetWifi { enabled: wifi });
                                }
                            } else {
                                unavailable_control(f, i18n.text(Message::Wifi), i18n);
                            }
                            if status.bluetooth_enabled.is_some() {
                                let mut bluetooth = status.bluetooth_enabled.unwrap_or(false);
                                if f.checkbox(i18n.text(Message::Bluetooth), &mut bluetooth) {
                                    out.system_actions
                                        .push(SystemAction::SetBluetooth { enabled: bluetooth });
                                }
                            } else {
                                unavailable_control(f, i18n.text(Message::Bluetooth), i18n);
                            }
                            f.spacer(2.0);

                            // Desktop group.
                            group_header(f, i18n.text(Message::Desktop));
                            f.row_ex(
                                &LayoutOpts {
                                    height: 22.0,
                                    gap: 8.0,
                                    cross: Align::Center,
                                    ..Default::default()
                                },
                                |f| {
                                    f.icon(Icon::Grid, 15.0);
                                    f.label_compact_sized(i18n.text(Message::Desktop), 12.5);
                                },
                            );
                            let mut do_not_disturb = status.do_not_disturb;
                            if f.checkbox(i18n.text(Message::DoNotDisturb), &mut do_not_disturb) {
                                out.system_actions.push(SystemAction::SetDoNotDisturb {
                                    enabled: do_not_disturb,
                                });
                            }
                            let mut tiled = status.tiled;
                            if f.checkbox(i18n.text(Message::TiledLayout), &mut tiled) {
                                out.system_actions
                                    .push(SystemAction::SetTiling { enabled: tiled });
                            }
                            f.spacer(2.0);

                            // Agent Workspaces group: display-only aggregate of
                            // the live Agent Interaction Domains (moved here from the HUD's right
                            // chip, ADR-0083).
                            group_header(f, i18n.text(Message::InteractionManager));
                            f.row_ex(
                                &LayoutOpts {
                                    height: 22.0,
                                    gap: 8.0,
                                    cross: Align::Center,
                                    ..Default::default()
                                },
                                |f| {
                                    f.icon(Icon::Users, 15.0);
                                    f.label_compact_sized(
                                        i18n.text(Message::InteractionManager),
                                        12.5,
                                    );
                                    f.flex(1.0);
                                    f.spacer(0.0);
                                    f.label_compact_sized(&agent_status_text, 11.0);
                                },
                            );
                            f.spacer(2.0);

                            // Session group: an immediate lock trigger and the
                            // "always on" idle inhibitor, which suspends automatic
                            // dimming, locking, and display power-off while held.
                            group_header(f, i18n.text(Message::Session));
                            if f.button(i18n.text(Message::LockNow)) {
                                out.lock = true;
                            }
                            let mut always_on = status.idle_inhibited;
                            if f.checkbox(i18n.text(Message::AlwaysOn), &mut always_on) {
                                out.system_actions
                                    .push(SystemAction::SetIdleInhibit { inhibit: always_on });
                            }
                        },
                    );
                });
            });
        });
        f.set_theme(original);
    }

    /// The interactive tray grid: left-click activates, right-click opens
    /// the host-rendered dbusmenu popover (or `SecondaryActivate`).
    pub(super) fn render_tray_section(
        &mut self,
        f: &mut Frame,
        area: Rect,
        progress: f32,
        cursor: (f32, f32),
        i18n: &Localizer,
    ) {
        let sao = Sao::classic();
        let mut cells = self.sni_cells();
        if cells.is_empty() {
            let original = f.theme();
            let muted = themes::sao_muted(themes::sao(&sao), &sao);
            f.set_theme(faded_theme(muted, progress));
            f.layer("aegis-sao-tray-empty", area, &transparent(), |f| {
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
                        f.label_compact_sized(i18n.text(Message::NoTrayItems), 12.0);
                        f.flex(1.0);
                        f.spacer(0.0);
                    },
                );
            });
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
        f.set_theme(faded_theme(themes::sao(&sao), progress));
        f.layer("aegis-sao-tray", area, &transparent(), |f| {
            f.column_ex(&sized(area.w, area.h), |f| {
                f.flex(1.0);
                f.scroll("aegis-sao-tray-scroll", |f| {
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
                                                &format!("aegis-sao-tray-cell-{}", cell.key),
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
                                                            f.label_compact_sized(&cell.title, 9.0);
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

    /// The notification list, newest first, as SAO "quest item" cards in a
    /// scroll area; a card click dismisses.
    pub(super) fn render_messages_section(
        &mut self,
        f: &mut Frame,
        area: Rect,
        progress: f32,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let sao = Sao::classic();
        let notifications = self.notification_snapshot();
        let original = f.theme();
        if notifications.is_empty() {
            let muted = themes::sao_muted(themes::sao(&sao), &sao);
            f.set_theme(faded_theme(muted, progress));
            f.layer("aegis-sao-messages-empty", area, &transparent(), |f| {
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
                        f.label_compact_sized(i18n.text(Message::NoNotifications), 12.0);
                        f.flex(1.0);
                        f.spacer(0.0);
                    },
                );
            });
            f.set_theme(original);
            return;
        }
        let base = themes::sao(&sao);
        let row_theme = faded_theme(base, progress);
        let muted_theme = faded_theme(themes::sao_muted(base, &sao), progress);
        f.set_theme(row_theme);
        f.layer("aegis-sao-messages", area, &transparent(), |f| {
            f.column_ex(&sized(area.w, area.h), |f| {
                f.flex(1.0);
                f.scroll("aegis-sao-messages-scroll", |f| {
                    f.column_ex(
                        &LayoutOpts {
                            gap: 6.0,
                            cross: Align::Stretch,
                            ..Default::default()
                        },
                        |f| {
                            for notification in notifications.iter().rev() {
                                let summary = truncate(&notification.summary, 48);
                                let body = truncate(&notification.body, 72);
                                let (response, _) = f.pressable_row(
                                    &format!("aegis-sao-message-{}", notification.id),
                                    &summary,
                                    &LayoutOpts {
                                        height: 58.0,
                                        gap: 2.0,
                                        pad: 10.0,
                                        radius: 12.0,
                                        cross: Align::Center,
                                        bg: fade_color(sao.surface_dim, progress),
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
                                                f.label_compact_sized(&summary, 12.5);
                                                if !body.is_empty() {
                                                    f.set_theme(muted_theme);
                                                    f.label_compact_sized(&body, 10.5);
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
        });
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

        let sao = Sao::classic();
        let original_theme = f.theme();
        let menu_theme = themes::sao(&sao);
        let dim_theme = themes::sao_muted(menu_theme, &sao);

        let header_visible = self.menu_path.len() > 1;
        let mut action: Option<MenuRowAction> = None;
        f.set_theme(menu_theme);
        f.layer(
            "aegis-sao-sni-menu",
            popover_bounds,
            &materials::sao_panel(&sao),
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
                            f.push_id("sao-menu-back");
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
                            f.push_id(&format!("sao-menu-row-{}", row.id));
                            if !row.enabled {
                                // Disabled rows render as inert labels with a
                                // dim foreground — selectable would still
                                // capture the click, which the dbusmenu spec
                                // forbids.
                                f.set_theme(dim_theme);
                                f.label_compact_sized(&truncate(&menu_row_label(row), 32), 11.5);
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
}
