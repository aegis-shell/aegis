use super::*;

use aegis_design::materials::{chrome_place, sized, transparent};
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
    /// name, `@username · groups`, hostname) drawn frameless — no chip
    /// background or border — straight onto the solid canvas.
    /// Slides in from the top-left.
    pub(super) fn render_profile_panel(
        &self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        _i18n: &Localizer,
    ) {
        let hud = self.panel_colors();
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
                            unsafe { f.image(texture, avatar_size, avatar_size) };
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

    /// The central command surface. Navigation and page content share one
    /// coherent card so the middle of the screen reads as a single object,
    /// not a loose rail beside an unrelated panel.
    pub(super) fn render_main_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let rise = (1.0 - progress) * 16.0;
        let rect = Rect {
            y: rect.y + rise,
            ..rect
        };

        const SHELL_PAD: f32 = 12.0;
        const INNER_GAP: f32 = 12.0;
        let nav_w: f32 = if rect.w < 620.0 { 132.0 } else { 184.0 };
        let nav_w = nav_w.min((rect.w * 0.36).max(96.0));
        let inner_h = (rect.h - SHELL_PAD * 2.0).max(1.0);
        let view_w = (rect.w - SHELL_PAD * 2.0 - nav_w - INNER_GAP).max(1.0);

        // One strong silhouette owns the centre. The recessed navigation
        // well supplies hierarchy without creating a second floating card.
        f.place(
            "aegis-hud-main-surface",
            &chrome_place(
                rect,
                LayoutOpts {
                    radius: self.design.radii.glass_panel,
                    border: hud.border,
                    border_width: 1.0,
                    ..materials::hud_panel(&hud)
                },
            ),
            |f| {
                f.column_ex(&sized(rect.w, rect.h), |_| {});
            },
        );

        let nav_rect = Rect {
            x: rect.x + SHELL_PAD,
            y: rect.y + SHELL_PAD,
            w: nav_w,
            h: inner_h,
        };
        let view_rect = Rect {
            x: nav_rect.x + nav_w + INNER_GAP,
            y: rect.y + SHELL_PAD,
            w: view_w,
            h: inner_h,
        };

        f.place(
            "aegis-hud-nav-surface",
            &chrome_place(
                nav_rect,
                LayoutOpts {
                    radius: (self.design.radii.glass_panel - 4.0).max(8.0),
                    bg: hud.surface_recessed,
                    ..Default::default()
                },
            ),
            |f| {
                f.column_ex(&sized(nav_rect.w, nav_rect.h), |_| {});
            },
        );

        self.render_nav_rail(f, nav_rect, i18n);

        let pad_h = 14.0;
        let pad_v = 10.0;
        let header_h = 42.0;
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
                        gap: 12.0,
                        ..Default::default()
                    },
                    |f| {
                        f.icon(Self::tab_icon(self.tab), 20.0);
                        display_label(f, active_title, type_scale.title);
                    },
                );
            },
        );
        f.set_theme(original);

        let body_area = Rect {
            x: view_rect.x + pad_h,
            y: view_rect.y + pad_v + header_h + 8.0,
            w: (view_rect.w - pad_h * 2.0).max(1.0),
            h: (view_rect.h - pad_v * 2.0 - header_h - 8.0).max(1.0),
        };
        match self.tab {
            Tab::QuickControls => self.render_quick_controls_section(f, body_area, i18n, out),
            Tab::Settings(id) => self.render_settings_tab(f, id, body_area, i18n, out),
        }
    }

    /// The navigation list inside the main card's recessed rail.
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

    pub(super) fn render_nav_rail(&mut self, f: &mut Frame, rect: Rect, i18n: &Localizer) {
        let hud = self.panel_colors();
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

        const ROW_H: f32 = 44.0;
        const ROW_GAP: f32 = 6.0;
        const RAIL_PAD: f32 = 8.0;

        let tab_theme = themes::hud(&hud);

        f.place(
            "aegis-hud-nav-rail",
            &chrome_place(rect, transparent()),
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: (rect.w - RAIL_PAD * 2.0).max(1.0),
                        height: (rect.h - RAIL_PAD * 2.0).max(1.0),
                        gap: ROW_GAP,
                        pad: RAIL_PAD,
                        cross: Align::Stretch,
                        ..Default::default()
                    },
                    |f| {
                        for (index, (tab, label)) in tabs.iter().enumerate() {
                            let selected = self.tab == *tab;

                            let (bg, border, text_color, icon_color) = if selected {
                                (
                                    hud.selection_surface,
                                    hud.accent.with_alpha(70),
                                    hud.text,
                                    hud.accent,
                                )
                            } else {
                                (
                                    Color::TRANSPARENT,
                                    Color::TRANSPARENT,
                                    hud.text,
                                    hud.text_muted,
                                )
                            };

                            let icon = Self::tab_icon(*tab);
                            let label_text =
                                truncate(label, ((rect.w - 58.0) / 7.2).max(3.0) as usize);

                            f.set_theme(tab_theme.with_fg(text_color));
                            let (response, _) = f.pressable_row(
                                &format!("aegis-hud-tab-{index}"),
                                &label_text,
                                &LayoutOpts {
                                    height: ROW_H,
                                    pad: 12.0,
                                    radius: 12.0,
                                    cross: Align::Center,
                                    gap: 10.0,
                                    bg,
                                    border,
                                    border_width: if selected { 1.0 } else { 0.0 },
                                    ..Default::default()
                                },
                                |f, _| {
                                    f.set_theme(tab_theme.with_fg(icon_color));
                                    f.icon(icon, 18.0);
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
        let hud = self.panel_colors();
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

    /// The top-right notifications stream: each notification as its own
    /// recessed card — a distinct background, hairline border, and 10px
    /// gaps so individuals read clearly — newest first, with the tail
    /// fading out toward the bottom of the region. The scrollbar stays
    /// hidden until the user wheels over the stream and fades back out
    /// when idle. Slides in from the top-right.
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
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let original = f.theme();
        let base_theme = themes::hud(&hud);
        let muted_theme = themes::hud_muted(base_theme, &hud);
        let notifications = self.notification_snapshot();

        // Small muted section header, still frameless.
        f.set_theme(muted_theme);
        f.place(
            "aegis-hud-notifications-header",
            &chrome_place(
                Rect {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w,
                    h: 18.0,
                },
                transparent(),
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: rect.w,
                        height: 18.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |f| {
                        display_label(f, i18n.text(Message::Notifications), type_scale.label);
                    },
                );
            },
        );

        let body = Rect {
            x: rect.x,
            y: rect.y + 24.0,
            w: rect.w,
            h: (rect.h - 24.0).max(1.0),
        };

        if notifications.is_empty() {
            f.place(
                "aegis-hud-messages-empty",
                &chrome_place(body, transparent()),
                |f| {
                    f.row_ex(
                        &LayoutOpts {
                            width: body.w,
                            height: body.h,
                            cross: Align::Center,
                            ..Default::default()
                        },
                        |f| {
                            display_label(f, i18n.text(Message::NoNotifications), type_scale.body);
                        },
                    );
                },
            );
            f.set_theme(original);
            return;
        }

        // Scrollbar only while wheel activity keeps it revealed.
        let scroll_id = "aegis-hud-messages-scroll";
        let scrollbar_theme = if self.notif_scrollbar_reveal > 0.01 {
            base_theme
                .with_scrollbar_width(4.0)
                .with_scrollbar_radius(2.0)
                .with_scrollbar_track_color(hud.background.with_alpha(0))
                .with_scrollbar_thumb_color(hud.surface.with_alpha(150))
                .with_scrollbar_thumb_hover_color(hud.surface.with_alpha(220))
        } else {
            base_theme.with_scrollbar_width(0.0)
        };
        f.set_theme(scrollbar_theme);

        f.place(
            "aegis-hud-messages",
            &chrome_place(body, transparent()),
            |f| {
                f.column_ex(&sized(body.w, body.h), |f| {
                    f.flex(1.0);
                    f.scroll(scroll_id, |f| {
                        f.column_ex(
                            &LayoutOpts {
                                width: body.w,
                                gap: 10.0,
                                cross: Align::Stretch,
                                ..Default::default()
                            },
                            |f| {
                                for notification in notifications.iter() {
                                    let id = notification.id;
                                    let (response, _) = f.pressable_row(
                                        &format!("aegis-hud-message-{id}"),
                                        "",
                                        &LayoutOpts {
                                            width: body.w,
                                            cross: Align::Center,
                                            gap: 10.0,
                                            pad: 10.0,
                                            radius: 10.0,
                                            bg: hud.surface_recessed,
                                            border: hud.border.with_alpha(60),
                                            border_width: 1.0,
                                            ..Default::default()
                                        },
                                        |f, _| {
                                            // One item per card: a distinct
                                            // recessed background, hairline
                                            // border, and 10px of breathing
                                            // room from its neighbours.
                                            f.icon(Icon::Bell, 14.0);
                                            f.column_ex(
                                                &LayoutOpts {
                                                    flex: 1.0,
                                                    gap: 2.0,
                                                    ..Default::default()
                                                },
                                                |f| {
                                                    f.set_theme(base_theme);
                                                    display_label(
                                                        f,
                                                        &notification.summary,
                                                        type_scale.body,
                                                    );
                                                    f.set_theme(muted_theme);
                                                    display_label(
                                                        f,
                                                        &notification.body,
                                                        type_scale.footnote,
                                                    );
                                                },
                                            );
                                        },
                                    );
                                    if response.clicked {
                                        out.dismissed_notification = Some(id);
                                    }
                                }
                            },
                        );
                    });
                });
            },
        );

        // Tail fade: a canvas-colored gradient plate masks the last stretch
        // of the stream so items dissolve toward the region's bottom edge.
        let fade_h = (body.h * 0.22).clamp(24.0, 72.0).min(body.h);
        f.place(
            "aegis-hud-messages-fade",
            &chrome_place(
                Rect {
                    x: body.x,
                    y: body.y + body.h - fade_h,
                    w: body.w,
                    h: fade_h,
                },
                transparent(),
            ),
            |f| {
                let steps = 10;
                for step in 0..steps {
                    let t = step as f32 / steps as f32;
                    let alpha = (t * t * 255.0) as u8;
                    f.place(
                        &format!("aegis-hud-messages-fade-{step}"),
                        &chrome_place(
                            Rect {
                                x: body.x,
                                y: body.y + body.h - fade_h + fade_h * (step as f32 / steps as f32),
                                w: body.w,
                                h: fade_h / steps as f32 + 0.5,
                            },
                            LayoutOpts {
                                bg: hud.background.with_alpha(alpha),
                                ..Default::default()
                            },
                        ),
                        |f| {
                            f.row_ex(
                                &LayoutOpts {
                                    width: body.w,
                                    height: fade_h / steps as f32 + 0.5,
                                    ..Default::default()
                                },
                                |_| {},
                            );
                        },
                    );
                }
            },
        );
        f.set_theme(original);
    }

    /// The frameless clock surface at top-center: large locale time with
    /// the weekday and date beneath. Redrawn when the wall clock's minute
    /// advances; between minutes the cached strings render unchanged.
    pub(super) fn render_clock_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        _i18n: &Localizer,
    ) {
        if rect.w < 80.0 || rect.h < 40.0 {
            return;
        }
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let fall = (1.0 - ease_out_cubic(progress)) * 16.0;
        let rect = Rect {
            y: rect.y + fall,
            ..rect
        };
        let (time_text, date_text) = clock_strings();

        let original = f.theme();
        let base_theme = themes::hud(&hud);
        let muted_theme = themes::hud_muted(base_theme, &hud);

        f.set_theme(base_theme);
        let time_h = (rect.h * 0.62).max(24.0);
        f.place(
            "aegis-hud-clock-time",
            &chrome_place(
                Rect {
                    x: rect.x,
                    y: rect.y,
                    w: rect.w,
                    h: time_h,
                },
                transparent(),
            ),
            |f| {
                f.centered(rect.w, time_h, |f| {
                    display_label(f, &time_text, type_scale.hero);
                });
            },
        );
        f.set_theme(muted_theme);
        let date_h = (rect.h - time_h).max(16.0);
        f.place(
            "aegis-hud-clock-date",
            &chrome_place(
                Rect {
                    x: rect.x,
                    y: rect.y + time_h,
                    w: rect.w,
                    h: date_h,
                },
                transparent(),
            ),
            |f| {
                f.centered(rect.w, date_h, |f| {
                    display_label(f, &date_text, type_scale.headline);
                });
            },
        );
        f.set_theme(original);
    }

    /// The first tab: a compact Control Center grid. Four direct-action
    /// tiles occupy a 2×2 cluster while sound and brightness become thick
    /// vertical faders. This keeps glanceable state and manipulation close
    /// together without falling back to a settings-form row stack.
    pub(super) fn render_quick_controls_section(
        &mut self,
        f: &mut Frame,
        area: Rect,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let original = f.theme();
        let status = self.status.clone();
        let gap = 12.0;
        let tile_gap = 10.0;
        let grid_h = area.h.clamp(1.0, 320.0);
        let fader_w = (area.w * 0.20).clamp(82.0, 116.0);
        let tile_cluster_w = (area.w - fader_w * 2.0 - gap * 2.0).max(1.0);
        let tile_w = ((tile_cluster_w - tile_gap) * 0.5).max(1.0);
        let tile_h = ((grid_h - tile_gap) * 0.5).max(1.0);
        let mode = status.power_mode;
        let keep_awake = !mode.blanks_display();
        let auto_lock = mode.locks_automatically();

        f.set_theme(themes::hud(&hud));
        f.place("aegis-hud-quick", &chrome_place(area, transparent()), |f| {
            f.column_ex(&sized(area.w, area.h), |f| {
                f.flex(1.0);
                f.spacer(0.0);
                f.row_ex(
                    &LayoutOpts {
                        width: area.w,
                        height: grid_h,
                        gap,
                        cross: Align::Stretch,
                        ..Default::default()
                    },
                    |f| {
                        f.column_ex(
                            &LayoutOpts {
                                width: tile_cluster_w,
                                height: grid_h,
                                gap: tile_gap,
                                cross: Align::Stretch,
                                ..Default::default()
                            },
                            |f| {
                                f.row_ex(
                                    &LayoutOpts {
                                        width: tile_cluster_w,
                                        height: tile_h,
                                        gap: tile_gap,
                                        cross: Align::Stretch,
                                        ..Default::default()
                                    },
                                    |f| {
                                        if render_control_tile(
                                            f,
                                            "aegis-hud-control-mute",
                                            i18n.text(Message::Muted),
                                            volume_icon(&status),
                                            status.muted,
                                            status.volume.is_some(),
                                            (tile_w, tile_h),
                                            hud,
                                            type_scale,
                                        ) {
                                            out.system_actions.push(SystemAction::ToggleMute);
                                        }
                                        if render_control_tile(
                                            f,
                                            "aegis-hud-control-dnd",
                                            i18n.text(Message::DoNotDisturb),
                                            Icon::Bell,
                                            status.do_not_disturb,
                                            true,
                                            (tile_w, tile_h),
                                            hud,
                                            type_scale,
                                        ) {
                                            out.system_actions.push(
                                                SystemAction::SetDoNotDisturb {
                                                    enabled: !status.do_not_disturb,
                                                },
                                            );
                                        }
                                    },
                                );
                                f.row_ex(
                                    &LayoutOpts {
                                        width: tile_cluster_w,
                                        height: tile_h,
                                        gap: tile_gap,
                                        cross: Align::Stretch,
                                        ..Default::default()
                                    },
                                    |f| {
                                        if render_control_tile(
                                            f,
                                            "aegis-hud-control-awake",
                                            i18n.text(Message::KeepAwake),
                                            Icon::Zap,
                                            keep_awake,
                                            true,
                                            (tile_w, tile_h),
                                            hud,
                                            type_scale,
                                        ) {
                                            let next = power_mode_for(
                                                !keep_awake,
                                                mode.locks_automatically(),
                                            );
                                            out.system_actions
                                                .push(SystemAction::SetPowerMode { mode: next });
                                        }
                                        if render_control_tile(
                                            f,
                                            "aegis-hud-control-lock",
                                            i18n.text(Message::AutoLock),
                                            Icon::Shield,
                                            auto_lock,
                                            true,
                                            (tile_w, tile_h),
                                            hud,
                                            type_scale,
                                        ) {
                                            let next = power_mode_for(keep_awake, !auto_lock);
                                            out.system_actions
                                                .push(SystemAction::SetPowerMode { mode: next });
                                        }
                                    },
                                );
                            },
                        );

                        if let Some(level) = render_control_fader(
                            f,
                            "aegis-hud-quick-volume",
                            i18n.text(Message::Sound),
                            volume_icon(&status),
                            status.volume,
                            (0, 100),
                            (fader_w, grid_h),
                            hud.accent,
                            hud,
                            type_scale,
                        ) {
                            out.system_actions.push(SystemAction::SetVolume { level });
                        }
                        if let Some(level) = render_control_fader(
                            f,
                            "aegis-hud-quick-brightness",
                            i18n.text(Message::Brightness),
                            Icon::Zap,
                            status.brightness,
                            (1, 100),
                            (fader_w, grid_h),
                            hud.text,
                            hud,
                            type_scale,
                        ) {
                            out.system_actions
                                .push(SystemAction::SetBrightness { level });
                        }
                    },
                );
                f.flex(1.0);
                f.spacer(0.0);
            });
        });
        f.set_theme(original);
    }

    /// The tray icon column at the left-middle anchor: StatusNotifierItem
    /// icons stacked vertically with no panel background, compact 22px
    /// glyphs. Left-click activates an item; right-click opens the
    /// host-rendered dbusmenu popover to the icon's right (or
    /// `SecondaryActivate` when the item has no Menu object); hover raises a
    /// rounded accent plate BEHIND the icon — drawn first and larger than
    /// the glyph, so the icon never disappears, only its backing becomes
    /// prominent. The column scrolls when icons overflow its height — the
    /// scrollbar fades in on wheel movement and decays away once idle.
    pub(super) fn render_tray_column(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        cursor: (f32, f32),
        _i18n: &Localizer,
    ) {
        if rect.w < 32.0 || rect.h < 32.0 {
            return;
        }
        let hud = self.panel_colors();
        let rise = (1.0 - progress) * 16.0;
        let rect = Rect {
            y: rect.y + rise,
            ..rect
        };

        let cells = self.sni_cells();
        if cells.is_empty() {
            return;
        }

        // Distill the per-cell visuals before the layout closures: those
        // capture disjoint borrows, so `self` method calls happen here.
        let fallback_themed = self.themed_icon("application-x-executable-symbolic");
        let cells: Vec<TrayCellVisual> = cells
            .iter()
            .map(|cell| TrayCellVisual {
                key: cell.key.clone(),
                title: truncate(&cell.title, 12),
                has_menu: cell.has_menu,
                texture: if cell.textured {
                    self.tray
                        .as_ref()
                        .and_then(|tray| tray.textures.get(&cell.key))
                        .map(|(_, image)| image.as_raw())
                } else {
                    None
                },
                fallback: fallback_themed.map(|icon| icon as *mut lens::sys::flux_image),
            })
            .collect();

        let cell_w = TRAY_CELL.min(rect.w).max(24.0);
        let column_x = rect.x + (rect.w - cell_w) * 0.5;

        let original = f.theme();
        let base_theme = themes::hud(&hud);

        // Interactions captured inside the layout closures for dispatch
        // after them (opening a popover mutates `self`).
        let mut activations: Vec<String> = Vec::new();
        let mut secondary: Vec<(String, bool)> = Vec::new();
        let mut resolved: Vec<(String, Rect)> = Vec::new();

        let scroll_id = "aegis-hud-tray-column-scroll";
        let needs_scroll = cells.len() as f32 * (TRAY_CELL + TRAY_GAP) > rect.h;
        let scrollbar_theme = if needs_scroll && self.tray_scrollbar_reveal > 0.01 {
            base_theme
                .with_scrollbar_width(4.0)
                .with_scrollbar_radius(2.0)
                .with_scrollbar_track_color(hud.background.with_alpha(0))
                .with_scrollbar_thumb_color(hud.surface.with_alpha(150))
                .with_scrollbar_thumb_hover_color(hud.surface.with_alpha(220))
        } else {
            // Zero width removes the scrollbar entirely when idle.
            base_theme.with_scrollbar_width(0.0)
        };

        f.set_theme(scrollbar_theme);
        f.place(
            "aegis-hud-tray-column",
            &chrome_place(
                Rect {
                    x: column_x - 10.0,
                    y: rect.y,
                    w: cell_w + 20.0,
                    h: rect.h,
                },
                transparent(),
            ),
            |f| {
                f.column_ex(&sized(cell_w + 20.0, rect.h), |f| {
                    f.flex(1.0);
                    f.scroll(scroll_id, |f| {
                        f.column_ex(
                            &LayoutOpts {
                                width: cell_w,
                                gap: TRAY_GAP,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            |f| {
                                for (index, cell) in cells.iter().enumerate() {
                                    // Hover raises a rounded-square plate BEHIND
                                    // the icon: the plate is drawn first and
                                    // sized larger than the glyph, so the icon
                                    // itself never disappears or dims — only its
                                    // backing becomes prominent.
                                    let est_y = rect.y + index as f32 * (TRAY_CELL + TRAY_GAP);
                                    let est_rect = Rect {
                                        x: column_x,
                                        y: est_y,
                                        w: cell_w,
                                        h: TRAY_CELL,
                                    };
                                    let hover = contains(est_rect, cursor.0, cursor.1);
                                    if hover {
                                        f.place(
                                            &format!("aegis-hud-tray-plate-{}", cell.key),
                                            &chrome_place(
                                                Rect {
                                                    x: column_x,
                                                    y: est_y,
                                                    w: cell_w,
                                                    h: TRAY_CELL,
                                                },
                                                LayoutOpts {
                                                    radius: cell_w * 0.32,
                                                    bg: hud.accent_surface_hover,
                                                    border: hud.accent.with_alpha(60),
                                                    border_width: 1.0,
                                                    ..Default::default()
                                                },
                                            ),
                                            |f| {
                                                f.row_ex(
                                                    &LayoutOpts {
                                                        width: cell_w,
                                                        height: TRAY_CELL,
                                                        ..Default::default()
                                                    },
                                                    |_| {},
                                                );
                                            },
                                        );
                                    }
                                    let texture = cell.texture;
                                    let fallback = cell.fallback;
                                    let title = cell.title.clone();
                                    let key = cell.key.clone();
                                    let has_menu = cell.has_menu;
                                    let (response, _) = f.pressable_row(
                                        &format!("aegis-hud-tray-item-{key}"),
                                        &title,
                                        &LayoutOpts {
                                            width: cell_w,
                                            height: TRAY_CELL,
                                            cross: Align::Center,
                                            bg: Color(0),
                                            ..Default::default()
                                        },
                                        |f, _| match texture {
                                            Some(texture) => unsafe {
                                                f.image(texture, TRAY_ICON, TRAY_ICON)
                                            },
                                            None => match fallback {
                                                Some(icon) => unsafe {
                                                    f.image(icon, TRAY_ICON, TRAY_ICON)
                                                },
                                                None => f.icon(Icon::FileText, TRAY_ICON),
                                            },
                                        },
                                    );
                                    resolved.push((key.clone(), response.rect));
                                    if response.clicked {
                                        activations.push(key.clone());
                                    } else if response.right_clicked {
                                        secondary.push((key.clone(), has_menu));
                                    }
                                }
                            },
                        );
                    });
                    f.spacer(0.0);
                });
            },
        );

        f.set_theme(original);
        let (x, y) = (cursor.0 as i32, cursor.1 as i32);
        for key in activations {
            self.send_tray_command(TrayCommand::Activate { key, x, y });
        }
        for (key, has_menu) in secondary {
            // Items that expose a Menu object path get the host-rendered
            // popover; everything else keeps the SNI `SecondaryActivate`.
            if has_menu {
                self.menu_owner = resolved
                    .iter()
                    .find(|(k, _)| k == &key)
                    .map(|(_, rect)| *rect)
                    .unwrap_or(Rect {
                        x: 0.0,
                        y: 0.0,
                        w: 0.0,
                        h: 0.0,
                    });
                self.menu_open_for = Some(key.clone());
                self.menu_path.clear();
                self.menu_just_opened = true;
                self.send_tray_command(TrayCommand::FetchMenu { key });
            } else {
                self.send_tray_command(TrayCommand::SecondaryActivate { key, x, y });
            }
        }

        // Keep the owner rect fresh against relayout or item movement.
        if let Some(key) = self.menu_open_for.clone() {
            if let Some((_, rect)) = resolved.iter().find(|(k, _)| k == &key) {
                self.menu_owner = *rect;
            } else {
                self.menu_open_for = None;
                self.menu_path.clear();
                self.send_tray_command(TrayCommand::CloseMenu { key });
            }
        }
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

        let hud = self.panel_colors();
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

    /// Compact MPRIS now-playing card at the left-bottom anchor. The card
    /// remains useful as an honest empty state when no player owns MPRIS;
    /// when one appears, transport actions are forwarded to the worker and
    /// never block the render thread.
    pub(super) fn render_media_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        i18n: &Localizer,
    ) {
        if rect.w < 150.0 || rect.h < 80.0 {
            return;
        }
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let slide = (1.0 - ease_out_cubic(progress)) * -20.0;
        let rect = Rect {
            x: rect.x + slide,
            ..rect
        };
        let snapshot = self
            .media
            .as_ref()
            .map(MediaHandle::snapshot)
            .unwrap_or_default();
        let original = f.theme();
        let base_theme = themes::hud(&hud);
        let muted_theme = themes::hud_muted(base_theme, &hud);
        let mut command = None;
        let identity_label = if snapshot.available {
            truncate(
                if snapshot.identity.is_empty() {
                    i18n.text(Message::NowPlaying)
                } else {
                    &snapshot.identity
                },
                ((rect.w - 58.0) / 6.5).max(8.0) as usize,
            )
        } else {
            i18n.text(Message::NowPlaying).to_owned()
        };
        let title_label = if snapshot.available && !snapshot.title.is_empty() {
            truncate(&snapshot.title, ((rect.w - 58.0) / 7.0).max(8.0) as usize)
        } else {
            i18n.text(Message::NotPlaying).to_owned()
        };
        let artist_label = truncate(&snapshot.artist, ((rect.w - 58.0) / 6.5).max(8.0) as usize);

        f.set_theme(base_theme);
        f.place(
            "aegis-hud-media",
            &chrome_place(
                rect,
                LayoutOpts {
                    radius: 18.0,
                    bg: hud.surface_recessed,
                    border: hud.border,
                    border_width: 1.0,
                    ..Default::default()
                },
            ),
            |f| {
                f.column_ex(
                    &LayoutOpts {
                        width: rect.w,
                        height: rect.h,
                        gap: 4.0,
                        pad: 12.0,
                        cross: Align::Stretch,
                        ..Default::default()
                    },
                    |f| {
                        f.row_ex(
                            &LayoutOpts {
                                height: 44.0,
                                gap: 10.0,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            |f| {
                                f.icon(
                                    if snapshot.playing {
                                        Icon::Pause
                                    } else {
                                        Icon::Play
                                    },
                                    18.0,
                                );
                                f.column_ex(
                                    &LayoutOpts {
                                        flex: 1.0,
                                        gap: 2.0,
                                        ..Default::default()
                                    },
                                    |f| {
                                        f.set_theme(muted_theme);
                                        display_label(f, &identity_label, type_scale.footnote);
                                        f.set_theme(base_theme);
                                        display_label(f, &title_label, type_scale.body);
                                        if snapshot.available && !snapshot.artist.is_empty() {
                                            f.set_theme(muted_theme);
                                            display_label(f, &artist_label, type_scale.footnote);
                                            f.set_theme(base_theme);
                                        }
                                    },
                                );
                            },
                        );

                        f.row_ex(
                            &LayoutOpts {
                                height: 36.0,
                                gap: 6.0,
                                cross: Align::Center,
                                ..Default::default()
                            },
                            |f| {
                                f.flex(1.0);
                                f.spacer(0.0);
                                f.set_theme(if snapshot.can_previous {
                                    base_theme
                                } else {
                                    muted_theme
                                });
                                let (previous, _) = f.pressable_row(
                                    "aegis-hud-media-previous",
                                    "Previous",
                                    &LayoutOpts {
                                        width: 36.0,
                                        height: 36.0,
                                        radius: 12.0,
                                        cross: Align::Center,
                                        bg: Color::TRANSPARENT,
                                        ..Default::default()
                                    },
                                    |f, _| f.centered(36.0, 36.0, |f| f.icon(Icon::SkipBack, 17.0)),
                                );
                                if previous.clicked && snapshot.can_previous {
                                    command = Some(MediaCommand::Previous);
                                }
                                f.set_theme(if snapshot.available {
                                    base_theme
                                } else {
                                    muted_theme
                                });
                                let (play_pause, _) = f.pressable_row(
                                    "aegis-hud-media-play-pause",
                                    "Play or pause",
                                    &LayoutOpts {
                                        width: 40.0,
                                        height: 36.0,
                                        radius: 12.0,
                                        cross: Align::Center,
                                        bg: hud.selection_surface,
                                        ..Default::default()
                                    },
                                    |f, _| {
                                        f.centered(40.0, 36.0, |f| {
                                            f.icon(
                                                if snapshot.playing {
                                                    Icon::Pause
                                                } else {
                                                    Icon::Play
                                                },
                                                18.0,
                                            )
                                        })
                                    },
                                );
                                if play_pause.clicked && snapshot.available {
                                    command = Some(MediaCommand::PlayPause);
                                }
                                f.set_theme(if snapshot.can_next {
                                    base_theme
                                } else {
                                    muted_theme
                                });
                                let (next, _) = f.pressable_row(
                                    "aegis-hud-media-next",
                                    "Next",
                                    &LayoutOpts {
                                        width: 36.0,
                                        height: 36.0,
                                        radius: 12.0,
                                        cross: Align::Center,
                                        bg: Color::TRANSPARENT,
                                        ..Default::default()
                                    },
                                    |f, _| {
                                        f.centered(36.0, 36.0, |f| f.icon(Icon::SkipForward, 17.0))
                                    },
                                );
                                if next.clicked && snapshot.can_next {
                                    command = Some(MediaCommand::Next);
                                }
                                f.flex(1.0);
                                f.spacer(0.0);
                            },
                        );
                    },
                );
            },
        );
        f.set_theme(original);
        if let Some(command) = command
            && let Some(media) = &self.media
        {
            media.send(command);
        }
    }

    /// The first of the three right-bottom components: a stable segmented
    /// mode selector. Its visible segments and interaction targets share
    /// the same absolute geometry, preventing the indicator and labels from
    /// drifting into different layout flows.
    pub(super) fn render_work_mode_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        if rect.w < 120.0 || rect.h < 32.0 {
            return;
        }
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let slide = (1.0 - ease_out_cubic(progress)) * 24.0;
        let rect = Rect {
            x: rect.x + slide,
            ..rect
        };

        let current_mode = self.status.power_mode;
        let original = f.theme();
        let base_theme = themes::hud(&hud);
        const INSET: f32 = 4.0;
        let control = Rect {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        };
        let inner = Rect {
            x: control.x + INSET,
            y: control.y + INSET,
            w: (control.w - INSET * 2.0).max(1.0),
            h: (control.h - INSET * 2.0).max(1.0),
        };
        let seg_w = inner.w / PowerMode::ALL.len() as f32;

        f.set_theme(base_theme);
        f.place(
            "aegis-hud-work-mode-surface",
            &chrome_place(
                control,
                LayoutOpts {
                    radius: control.h * 0.5,
                    bg: hud.surface_recessed,
                    border: hud.border,
                    border_width: 1.0,
                    ..Default::default()
                },
            ),
            |f| {
                f.row_ex(&sized(control.w, control.h), |_| {});
            },
        );

        // Sliding indicator: position from the spring (segment units), so
        // it overshoots slightly and settles with elastic bounce.
        let spring_pos = self
            .work_mode_spring
            .value
            .clamp(0.0, PowerMode::ALL.len().saturating_sub(1) as f32);
        let indicator_x = inner.x + spring_pos * seg_w;
        f.place(
            "aegis-hud-work-mode-indicator",
            &chrome_place(
                Rect {
                    x: indicator_x,
                    y: inner.y,
                    w: seg_w,
                    h: inner.h,
                },
                LayoutOpts {
                    radius: inner.h * 0.5,
                    bg: hud.selection_surface,
                    border: hud.accent.with_alpha(80),
                    border_width: 1.0,
                    ..Default::default()
                },
            ),
            |f| {
                f.row_ex(
                    &LayoutOpts {
                        width: seg_w,
                        height: inner.h,
                        ..Default::default()
                    },
                    |_| {},
                );
            },
        );

        // Segment hit areas over the indicator.
        self.work_mode_hover = None;
        for (index, mode) in PowerMode::ALL.iter().enumerate() {
            let active = *mode == current_mode;
            let label = match mode {
                PowerMode::Balanced => i18n.text(Message::PowerModeBalanced),
                PowerMode::Awake => i18n.text(Message::PowerModeAwake),
                PowerMode::Secure => i18n.text(Message::PowerModeSecure),
            };
            let hint = power_mode_hint(*mode);
            let seg_rect = Rect {
                x: inner.x + index as f32 * seg_w,
                y: inner.y,
                w: seg_w,
                h: inner.h,
            };
            let hovered = contains(seg_rect, self.cursor_hint.0, self.cursor_hint.1);
            if hovered {
                self.work_mode_hover = Some(index);
            }
            let fg = if active { hud.accent } else { hud.text_muted };
            let seg_theme = themes::hud(&hud).with_fg(fg);
            f.set_theme(seg_theme);
            let mut clicked = false;
            f.place(
                &format!("aegis-hud-work-seg-place-{index}"),
                &chrome_place(seg_rect, transparent()),
                |f| {
                    let (response, _) = f.pressable_row(
                        &format!("aegis-hud-work-seg-{index}"),
                        label,
                        &LayoutOpts {
                            width: seg_rect.w,
                            height: seg_rect.h,
                            cross: Align::Center,
                            bg: Color::TRANSPARENT,
                            ..Default::default()
                        },
                        |f, _| {
                            f.centered(seg_rect.w, seg_rect.h, |f| {
                                display_label(
                                    f,
                                    &truncate(label, (seg_rect.w / 7.0).max(3.0) as usize),
                                    type_scale.footnote,
                                );
                            });
                        },
                    );
                    clicked = response.clicked;
                },
            );
            if clicked && !active {
                out.system_actions
                    .push(SystemAction::SetPowerMode { mode: *mode });
            }
            // Segment tooltip on hover.
            if hovered && self.work_mode_tooltip_reveal > 0.02 {
                render_tooltip(
                    f,
                    &format!("aegis-hud-work-seg-tip-{index}"),
                    Rect {
                        x: seg_rect.x,
                        y: control.y - 32.0,
                        w: seg_w.max(120.0),
                        h: 26.0,
                    },
                    hint,
                    hud,
                    type_scale,
                    self.work_mode_tooltip_reveal,
                );
            }
        }

        f.set_theme(original);
    }

    /// The second and third right-bottom components: explicit lock and
    /// power buttons. Both the painted button and its hit target occupy the
    /// same fixed rect; destructive power-off still leaves through the
    /// runtime's system-level confirmation flow.
    pub(super) fn render_power_session_panel(
        &mut self,
        f: &mut Frame,
        rect: Rect,
        progress: f32,
        cursor: (f32, f32),
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let group_w = SESSION_BUTTON * 2.0 + SESSION_BUTTON_GAP;
        if rect.w < group_w || rect.h < SESSION_BUTTON {
            return;
        }
        let hud = self.panel_colors();
        let type_scale = self.design.typography;
        let slide = (1.0 - ease_out_cubic(progress)) * 24.0;
        let rect = Rect {
            x: rect.x + slide,
            ..rect
        };
        let original = f.theme();
        let button_y = rect.y + (rect.h - SESSION_BUTTON) * 0.5;
        let group_x = rect.x + (rect.w - group_w) * 0.5;
        let lock_rect = Rect {
            x: group_x,
            y: button_y,
            w: SESSION_BUTTON,
            h: SESSION_BUTTON,
        };
        let power_rect = Rect {
            x: group_x + SESSION_BUTTON + SESSION_BUTTON_GAP,
            y: button_y,
            w: SESSION_BUTTON,
            h: SESSION_BUTTON,
        };
        let lock_hover = contains(lock_rect, cursor.0, cursor.1);
        let power_hover = contains(power_rect, cursor.0, cursor.1);

        let mut lock_clicked = false;
        f.set_theme(themes::hud(&hud).with_fg(if lock_hover {
            hud.accent
        } else {
            hud.text_muted
        }));
        f.place(
            "aegis-hud-session-lock-place",
            &chrome_place(
                lock_rect,
                LayoutOpts {
                    radius: 14.0,
                    bg: hud.surface_recessed,
                    border: if lock_hover { hud.accent } else { hud.border },
                    border_width: 1.0,
                    ..Default::default()
                },
            ),
            |f| {
                let (response, _) = f.pressable_row(
                    "aegis-hud-session-lock",
                    i18n.text(Message::LockNow),
                    &LayoutOpts {
                        width: lock_rect.w,
                        height: lock_rect.h,
                        cross: Align::Center,
                        bg: Color::TRANSPARENT,
                        ..Default::default()
                    },
                    |f, _| {
                        f.centered(lock_rect.w, lock_rect.h, |f| {
                            f.icon(Icon::Shield, 18.0);
                        });
                    },
                );
                lock_clicked = response.clicked;
            },
        );
        if lock_clicked {
            out.lock = true;
        }

        let mut power_clicked = false;
        f.set_theme(themes::hud(&hud).with_fg(if power_hover { hud.accent } else { hud.text }));
        f.place(
            "aegis-hud-session-power-place",
            &chrome_place(
                power_rect,
                LayoutOpts {
                    radius: 14.0,
                    bg: hud.selection_surface,
                    border: hud.accent.with_alpha(if power_hover { 180 } else { 80 }),
                    border_width: 1.0,
                    ..Default::default()
                },
            ),
            |f| {
                let (response, _) = f.pressable_row(
                    "aegis-hud-session-power",
                    i18n.text(Message::PowerOff),
                    &LayoutOpts {
                        width: power_rect.w,
                        height: power_rect.h,
                        cross: Align::Center,
                        bg: Color::TRANSPARENT,
                        ..Default::default()
                    },
                    |f, _| {
                        f.centered(power_rect.w, power_rect.h, |f| {
                            f.icon(Icon::Zap, 18.0);
                        });
                    },
                );
                power_clicked = response.clicked;
            },
        );
        if power_clicked {
            self.request_system_confirm(SystemAction::PowerOff, out);
        }

        self.session_hover = if power_hover {
            Some("power")
        } else if lock_hover {
            Some("lock")
        } else {
            None
        };
        if let Some(kind) = self.session_hover
            && self.session_tooltip_reveal > 0.02
        {
            let (anchor, label) = if kind == "power" {
                (power_rect, i18n.text(Message::PowerOff))
            } else {
                (lock_rect, i18n.text(Message::LockNow))
            };
            render_tooltip(
                f,
                &format!("aegis-hud-session-{kind}-tip"),
                Rect {
                    x: anchor.x - 34.0,
                    y: anchor.y - 32.0,
                    w: 120.0,
                    h: 26.0,
                },
                label,
                hud,
                type_scale,
                self.session_tooltip_reveal,
            );
        }
        f.set_theme(original);
    }
    /// Route a destructive session action through the system-level
    /// confirmation. The panel never performs power transitions itself:
    /// like the power notification paths, the request leaves through
    /// `ChromeEvents` and the compositor runtime opens the consent chrome
    /// (`StartConfirmPick`) before executing anything.
    pub(super) fn request_system_confirm(&mut self, action: SystemAction, out: &mut ChromeEvents) {
        if self.power_pending_confirm.is_some() {
            return;
        }
        self.power_pending_confirm = Some(action.clone());
        out.system_actions.push(action);
    }
}

/// The one-line behavior hint under each work-mode segment.
fn power_mode_hint(mode: PowerMode) -> &'static str {
    match mode {
        PowerMode::Balanced => "Dim → Lock → Blank → Suspend",
        PowerMode::Awake => "Screen stays lit & unlocked",
        PowerMode::Secure => "Lock on idle, screen stays lit",
    }
}

/// Render a small frameless tooltip pill with `reveal` opacity over the
/// anchor's top edge. Purely presentational; hover state is caller-owned.
fn render_tooltip(
    f: &mut Frame,
    id: &str,
    anchor: Rect,
    text: &str,
    hud: CommandPanelColors,
    type_scale: TypeScale,
    reveal: f32,
) {
    let original_opacity = f.opacity();
    f.set_opacity(reveal * original_opacity);
    f.place(
        id,
        &chrome_place(
            anchor,
            LayoutOpts {
                radius: 8.0,
                bg: hud.surface_recessed,
                border: hud.border,
                border_width: 1.0,
                ..Default::default()
            },
        ),
        |f| {
            f.row_ex(
                &LayoutOpts {
                    width: anchor.w,
                    height: anchor.h,
                    cross: Align::Center,
                    pad: 6.0,
                    ..Default::default()
                },
                |f| {
                    display_label(f, text, type_scale.footnote);
                },
            );
        },
    );
    f.set_opacity(original_opacity);
}

/// Locale wall-clock strings for the top-center surface: `("21:47",
/// "Saturday, June 21")`. libc `localtime_r` keeps this off any additional
/// dependency, matching the lock screen's clock.
pub(crate) fn clock_strings() -> (String, String) {
    use std::ffi::{CStr, c_char};

    let mut timestamp = 0;
    unsafe {
        libc::time(&mut timestamp);
    }
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    let local = unsafe {
        if libc::localtime_r(&timestamp, local.as_mut_ptr()).is_null() {
            return ("--:--".into(), String::new());
        }
        local.assume_init()
    };
    let strftime = |time: &libc::tm, format: &CStr| -> String {
        let mut output = [0 as c_char; 128];
        let len =
            unsafe { libc::strftime(output.as_mut_ptr(), output.len(), format.as_ptr(), time) };
        if len == 0 {
            String::new()
        } else {
            unsafe { CStr::from_ptr(output.as_ptr()) }
                .to_string_lossy()
                .trim()
                .to_owned()
        }
    };
    (strftime(&local, c"%H:%M"), strftime(&local, c"%A, %B %e"))
}
/// Project the two Quick Controls toggles onto the session power mode
/// (ADR-0140).
///
/// `keep_awake` is the display axis ("never blank the screen"); `auto_lock`
/// is the security axis ("lock on schedule"). The one combination the
/// power pipeline cannot honor — keep awake but never lock — is unsafe on a
/// shared machine (anyone can walk up), so it projects onto
/// [`aegis_model::power::PowerMode::Awake`]: with no automatic lock there is nothing to
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
