use super::*;

impl Chrome for Dock {
    fn render(
        &mut self,
        f: &mut Frame,
        input: &Input,
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    ) {
        let disp = input.as_raw().display_size;
        let dt = input.as_raw().dt_seconds.max(0.0);
        let cursor = input.as_raw().cursor;
        let down = input.as_raw().mouse_down.first().copied().unwrap_or(false);
        self.last_display = Some((disp.x, disp.y));

        // A fullscreen client owns the whole output edge: no animation,
        // handle, hover target, popup, or residual tooltip may surface above
        // it. Maximized windows force the Dock into its collapsed capsule;
        // other non-fullscreen windows use the geometric intersection policy.
        if self.fullscreen_locked() {
            self.dismiss_transient_ui();
            self.autohide_reveal = 0.0;
            self.autohide_idle = self.autohide_timeout;
            self.anim_active = false;
            self.prev_down = down;
            return;
        }

        let dock_obscured = self.obscured_by_windows(windows, (disp.x, disp.y));
        self.set_dock_obscured(dock_obscured);

        let menu_was_open = self.app_menu.is_open();

        // The Launchpad tile always leads the strip (macOS-style), followed by
        // the pinned apps and any unpinned running windows. The strip comes
        // from the cache shared with `pointer_bounds`, so it is rebuilt only
        // when the window set, the catalog, or the localized label changes.
        let application_label = i18n.text(Message::Applications);
        let tiles = Self::frame_tiles(
            &self.tile_cache,
            &self.apps,
            &self.icons,
            self.catalog_revision,
            windows,
            Some(application_label),
        );
        let n = tiles.len();
        let pinned_count = tiles.iter().filter(|t| t.pinned).count();
        let unpinned_count = n.saturating_sub(pinned_count);
        let section_gap = if pinned_count > 0 && unpinned_count > 0 {
            DOCK_SECTION_GAP
        } else {
            0.0
        };
        let rest_bounds = Self::rest_bounds(n, pinned_count, (disp.x, disp.y));

        // Drop eased sizes for tiles no longer present so the map does not
        // grow unbounded across long sessions.
        let live_keys: std::collections::HashSet<&str> =
            tiles.iter().map(|t| t.key.as_str()).collect();
        self.sizes.retain(|key, _| live_keys.contains(key.as_str()));

        let rest_panel_y = disp.y - DOCK_PANEL_HEIGHT - DOCK_BOTTOM_MARGIN;
        let effective_autohide = self.effective_autohide();

        // Pointer activation band for magnification. A visible hover surface
        // independently keeps autohide revealed while the pointer travels
        // from the icon into a live preview card.
        let over_hover_surface = self.hover_surface_contains(cursor.x, cursor.y);
        let over_rest_bounds = cursor.x >= rest_bounds.x
            && cursor.y >= rest_bounds.y
            && cursor.x < rest_bounds.x + rest_bounds.w
            && cursor.y < rest_bounds.y + rest_bounds.h;
        // The old resting rectangle must stay inert while collapsed. It is
        // enabled for magnification only after the capsule has begun revealing
        // the Dock.
        let in_band = !self.collapse_pending
            && over_rest_bounds
            && (!effective_autohide || self.autohide_reveal >= 0.2);
        let capsule_entry =
            if self.collapse_pending || !effective_autohide || self.autohide_reveal >= 0.2 {
                false
            } else {
                Self::hidden_reveal_requested(
                    &mut self.hidden_trigger_armed,
                    (cursor.x, cursor.y),
                    (disp.x, disp.y),
                )
            };
        let over_dock_trigger = !self.collapse_pending
            && Self::pointer_keeps_revealed(
                effective_autohide,
                self.autohide_reveal,
                capsule_entry,
                (cursor.x, cursor.y),
                rest_bounds,
                disp.y,
            );
        let keeps_revealed = over_dock_trigger || over_hover_surface;
        let menu_open = self.app_menu.is_open();

        if effective_autohide {
            if keeps_revealed || menu_open {
                self.autohide_idle = 0.0;
            } else {
                self.autohide_idle += dt;
            }
        }

        let target_reveal = if effective_autohide {
            if self.autohide_idle >= self.autohide_timeout && !menu_open {
                0.0
            } else {
                1.0
            }
        } else {
            1.0
        };

        if self.reduced_motion {
            self.autohide_reveal = target_reveal;
        } else {
            let blend = 1.0 - (-12.0 * dt.min(1.0 / 30.0)).exp();
            self.autohide_reveal += (target_reveal - self.autohide_reveal) * blend;
            if (target_reveal - self.autohide_reveal).abs() < 0.002 {
                self.autohide_reveal = target_reveal;
            }
        }
        let autohide_moving = (target_reveal - self.autohide_reveal).abs() > 0.002;
        if self.collapse_pending && target_reveal == 0.0 && self.autohide_reveal <= 0.002 {
            self.autohide_reveal = 0.0;
            self.collapse_pending = false;
        }

        // ---- contiguous reflow layout -------------------------------------
        // Unlike a fixed-rest dock, the bar widens to fit the magnified tiles
        // and neighbouring tiles spread apart around the cursor — the classic
        // macOS squeeze-and-lift. Because the total width changes, centres are
        // derived *from* the eased widths rather than from fixed slots, so the
        // cursor → tile mapping tracks the live layout.
        //
        // First ease each tile's size toward its magnification target. A
        // first-seen key springs up from DOCK_TILE_BIRTH (grow-in) instead of
        // snapping to rest size.
        let mut eased: Vec<f32> = Vec::with_capacity(n);
        // Track per-tile (target, velocity) so the anim-pending check below can
        // tell when every spring has fully rested.
        let mut unsettled = false;
        for (i, t) in tiles.iter().enumerate() {
            let factor = if in_band {
                Self::magnify_factor(
                    cursor.x - Self::rest_centre_estimate(i, n, pinned_count, disp.x),
                )
            } else {
                0.0
            };
            let target = DOCK_TILE + (DOCK_TILE_MAX - DOCK_TILE) * factor;
            // Look up before inserting so an existing tile does not pay a
            // key clone every frame.
            let state = match self.sizes.get_mut(&t.key) {
                Some(state) => state,
                None => self.sizes.entry(t.key.clone()).or_insert(SpringState {
                    value: if menu_was_open {
                        DOCK_TILE
                    } else {
                        DOCK_TILE_BIRTH
                    },
                    vel: 0.0,
                }),
            };
            // A context menu must not become a moving target. Freeze the
            // complete wave exactly where it was opened; once the menu closes,
            // the same springs resume toward the live pointer targets.
            if menu_was_open {
                state.vel = 0.0;
                eased.push(state.value);
                continue;
            }
            if self.reduced_motion {
                // ADR-0029: springs resolve to their target in one frame.
                state.value = target;
                state.vel = 0.0;
                eased.push(target);
                continue;
            }
            eased.push(Self::spring(state, target, dt));
            // A spring is still animating while it is meaningfully off its
            // target or still moving. Sub-pixel drift is ignored so we don't
            // tick forever chasing float noise.
            let drifting = (state.value - target).abs() > 0.15 || state.vel.abs() > 0.5;
            unsettled |= drifting;
        }
        self.anim_active = unsettled || autohide_moving;

        // Sum the eased widths (plus the inter-tile gap) to get the live bar
        // width. The gap is constant; only the tiles widen. Centred horizontally.
        let total_tiles: f32 = eased.iter().sum();
        let bar_w = total_tiles + (n as f32 - 1.0) * DOCK_TILE_GAP + section_gap + 2.0 * DOCK_PAD;
        let bar_x = (disp.x - bar_w) * 0.5;

        // The running x-offset of each tile's centre, left to right. The
        // pinned strip and the transient running section are separated by the
        // wider section gap instead of the ordinary tile gap.
        let mut centres = Vec::with_capacity(n);
        let mut x = bar_x + DOCK_PAD;
        for (i, s) in eased.iter().enumerate() {
            if i > 0 {
                let gap = if !tiles[i].pinned && tiles[i - 1].pinned {
                    section_gap
                } else {
                    DOCK_TILE_GAP
                };
                x += gap;
            }
            centres.push(x + s * 0.5);
            x += *s;
        }
        let centre = |i: usize| centres[i];

        let surface_progress = if effective_autohide {
            Self::collapse_surface_progress(self.autohide_reveal)
        } else {
            1.0
        };
        let content_progress = if effective_autohide {
            Self::collapse_content_progress(self.autohide_reveal)
        } else {
            1.0
        };
        let panel_rect = if effective_autohide {
            Self::collapsed_panel_rect((disp.x, disp.y), bar_w, self.autohide_reveal)
        } else {
            Rect {
                x: bar_x,
                y: rest_panel_y,
                w: bar_w,
                h: DOCK_PANEL_HEIGHT,
            }
        };

        // Icons are pulled toward the same bottom-centre sink as the panel:
        // their centres converge horizontally, their baseline follows the
        // shrinking surface, and their size reaches zero before the final
        // stadium settles.
        let icon_bottom = panel_rect.y + panel_rect.h - DOCK_BASELINE_INSET * content_progress;
        let icon_rects: Vec<Rect> = (0..n)
            .map(|i| {
                let s = (eased[i] * content_progress).max(0.0);
                let centre_x = disp.x * 0.5 + (centre(i) - disp.x * 0.5) * content_progress;
                Rect {
                    x: centre_x - s * 0.5,
                    y: icon_bottom - s,
                    w: s,
                    h: s,
                }
            })
            .collect();

        // The popup belongs to a tile rather than the pointer coordinate.
        // Re-anchor every frame so it follows the tile's live spring geometry.
        if self.app_menu.is_open() {
            if let Some((index, _)) = self
                .menu_tile
                .as_ref()
                .and_then(|key| tiles.iter().enumerate().find(|(_, tile)| &tile.key == key))
            {
                self.app_menu.set_owner(icon_rects[index]);
            } else {
                self.app_menu.dismiss();
                self.menu_tile = None;
            }
        }

        // The bar and collapsed indicator are the same layer, and both are
        // analytic glass bodies: as it drains, the bar morphs into the
        // stadium handle while keeping lensing, tint and its drop shadow.
        // Edge definition comes from the glass rim, not a painted border.
        let dock_material = collapsing_dock_material(surface_progress, panel_rect.h);
        // A layer with an empty body collapses to ~0 (the rect is only an
        // anchor, not a size); a fixed-size child forces it to the bar size.
        f.layer("aegis-dock", panel_rect, &dock_material, |f| {
            f.column_ex(&sized(panel_rect.w, panel_rect.h), |_| {});
        });

        // Hit-test only content that still exists in the morphing surface.
        // The resting bounds remain the outer ownership limit, while the
        // current panel and transformed icon geometry prevent the collapsed
        // handle (or an icon's former position) from impersonating a tile.
        let hit = hit_test_tiles(
            (cursor.x, cursor.y),
            rest_bounds,
            panel_rect,
            content_progress,
            &icon_rects,
        );

        // Draw each tile's icon, then its running dot. Once content has
        // reached the sink, no tile layers remain behind the stadium.
        if content_progress > AUTOHIDE_CONTENT_INTERACTION_MIN {
            for (i, t) in tiles.iter().enumerate() {
                let s = icon_rects[i].w;
                let cx = icon_rects[i].x + s * 0.5;
                let rect = icon_rects[i];
                let icon_id = format!("aegis-dock-icon-{}", t.key);
                if t.launchpad {
                    // A rounded "app tile" with a 3×3 grid, so it reads as macOS's
                    // Launchpad button. The grid (real content) sizes the layer;
                    // the layer paints the rounded background behind it.
                    let bg = OverlayOpts {
                        bg: Color::rgba(70, 78, 110, scaled_alpha(240, content_progress)),
                        border: Color::rgba(150, 160, 195, scaled_alpha(180, content_progress)),
                        border_width: 1.0,
                        radius: s * 0.22,
                        pad: s * 0.2,
                        cross: Align::Center,
                        ..Default::default()
                    };
                    let gap = s * 0.1;
                    let d = (s - 2.0 * (s * 0.2) - 2.0 * gap) / 3.0;
                    f.layer(&icon_id, rect, &bg, |f| {
                        f.column_ex(&grid(gap), |f| {
                            for _ in 0..3 {
                                f.row_ex(&grid(gap), |f| {
                                    for _ in 0..3 {
                                        f.column_ex(
                                            &sized_fill(
                                                d,
                                                d,
                                                Color::rgba(
                                                    236,
                                                    238,
                                                    248,
                                                    scaled_alpha(245, content_progress),
                                                ),
                                                d * 0.3,
                                            ),
                                            |_| {},
                                        );
                                    }
                                });
                            }
                        });
                    });
                } else {
                    f.layer(&icon_id, rect, &tile_opts(), |f| match t.icon {
                        // The pointer crosses from the binary's flux binding type to
                        // lens's ABI-identical flux_image.
                        Some(ptr) => unsafe { f.image(ptr as *mut lens::sys::flux_image, s, s) },
                        None => f.icon(Icon::FileText, s * 0.6),
                    });
                }

                if t.running {
                    // Centre the dot in the flat strip between the icon baseline
                    // and the panel bottom, so it never falls into the rounded
                    // corner region (and outside the bar) on the leftmost or
                    // rightmost tiles.
                    let dot_w = if t.windows.len() > 1 {
                        DOCK_DOT_STADIUM
                    } else {
                        DOCK_DOT
                    } * content_progress;
                    let dot_h = DOCK_DOT * content_progress;
                    let strip_h = DOCK_BASELINE_INSET.max(DOCK_DOT) * content_progress;
                    let dot_y = icon_bottom + (strip_h - dot_h) * 0.5 + dot_h * 0.5;
                    let dot_rect = Rect {
                        x: cx - dot_w * 0.5,
                        y: dot_y - dot_h * 0.5,
                        w: dot_w,
                        h: dot_h,
                    };
                    let color = if t.activated {
                        Color::rgba(236, 238, 245, scaled_alpha(255, content_progress))
                    } else {
                        Color::rgba(200, 204, 220, scaled_alpha(170, content_progress))
                    };
                    let dot_id = format!("aegis-dock-dot-{}", t.key);
                    f.layer(&dot_id, dot_rect, &tile_opts(), |f| {
                        f.column_ex(&sized_fill(dot_w, dot_h, color, dot_h * 0.5), |_| {});
                    });
                }
            }
        }

        // A slim divider in the section gap separates the kept strip from the
        // transient running apps, like macOS's Dock.
        if section_gap > 0.0 && content_progress > AUTOHIDE_CONTENT_INTERACTION_MIN {
            let normal_divider_x = (centre(pinned_count - 1) + centre(pinned_count)) * 0.5;
            let divider_x = disp.x * 0.5 + (normal_divider_x - disp.x * 0.5) * content_progress;
            let divider_h = DOCK_TILE * 0.55 * content_progress;
            let divider_rect = Rect {
                x: divider_x - 0.5,
                y: panel_rect.y + (panel_rect.h - divider_h) * 0.5,
                w: 1.0,
                h: divider_h,
            };
            f.layer(
                "aegis-dock-section-divider",
                divider_rect,
                &OverlayOpts::default(),
                |f| {
                    f.column_ex(
                        &sized_fill(
                            1.0,
                            divider_h,
                            Color::rgba(255, 255, 255, scaled_alpha(56, content_progress)),
                            0.5,
                        ),
                        |_| {},
                    );
                },
            );
        }

        // Fire a click once on the press edge (the host does not clear the
        // per-frame pressed flag, so track the button-down level transition).
        let pressed_edge = down && !self.prev_down;
        let preview_hit = self
            .live_preview
            .as_ref()
            .and_then(|presentation| live_preview_hit(presentation, cursor.x, cursor.y));
        self.hovered_preview = preview_hit.filter(|id| {
            windows
                .iter()
                .find(|window| window.id == *id)
                .is_some_and(|window| !window.read_only)
        });
        let clicked_preview = pressed_edge && !menu_was_open && self.hovered_preview.is_some();
        if clicked_preview {
            out.clicked = self.hovered_preview;
            self.hovered_tile = None;
            self.hover_elapsed = 0.0;
            self.tooltip_tile = None;
            self.tooltip_alpha = 0.0;
            self.live_preview = None;
            self.hover_surface_bounds = None;
            self.hover_owner_bounds = None;
            self.hovered_preview = None;
        }

        if pressed_edge
            && !menu_was_open
            && !clicked_preview
            && let Some(i) = hit
        {
            let t = &tiles[i];
            if t.launchpad {
                out.toggle_launcher = true;
            } else if let Some(id) = t.focus {
                out.clicked = Some(id);
            } else if let Some(ai) = t.spawn {
                out.activate_entry(self.apps[ai].entry.clone());
            }
        }
        let right_pressed = input
            .as_raw()
            .mouse_pressed
            .get(1)
            .copied()
            .unwrap_or(false);
        if right_pressed && let Some(i) = hit {
            let tile = &tiles[i];
            if !tile.launchpad {
                let pin_action = if let Some(ai) = tile.app {
                    // A pinned tile always offers removal from the strip.
                    Some(PinAction::Unpin(self.apps[ai].entry.id.clone()))
                } else {
                    // A transient running window offers "Keep in Dock"
                    // only when its app_id resolves to an enumerated
                    // desktop entry.
                    let window_app_id = tile
                        .windows
                        .first()
                        .and_then(|id| windows.iter().find(|w| w.id == *id))
                        .and_then(|w| w.app_id.as_deref());
                    window_app_id.and_then(|app_id| {
                        self.all_apps
                            .iter()
                            .find(|entry| entry_matches_app_id(entry, app_id))
                            .map(|entry| PinAction::Pin(entry.id.clone()))
                    })
                };
                self.app_menu.open(
                    tile.label.clone(),
                    tile.app.map(|app| self.apps[app].entry.clone()),
                    tile.windows.iter().copied(),
                    icon_rects[i],
                    pin_action,
                );
                self.menu_tile = Some(tile.key.clone());
            }
        }

        // Reveal an app name or running-window previews after a short dwell.
        // Once open, the popover retains its owner while the pointer crosses
        // the gap above the Dock and enters a preview card.
        let hovered_tile = hit.map(|i| tiles[i].key.clone()).or_else(|| {
            over_hover_surface
                .then(|| self.tooltip_tile.clone())
                .flatten()
        });
        if self.hovered_tile != hovered_tile {
            self.hovered_tile = hovered_tile;
            self.hover_elapsed = 0.0;
            self.tooltip_tile = None;
            self.tooltip_alpha = 0.0;
            self.live_preview = None;
            self.hover_surface_bounds = None;
            self.hover_owner_bounds = None;
            self.hovered_preview = None;
        } else if self.hovered_tile.is_some() && !self.app_menu.is_open() {
            self.hover_elapsed += dt;
        }

        if self.app_menu.is_open() {
            self.tooltip_tile = None;
            self.tooltip_alpha = 0.0;
            self.live_preview = None;
            self.hover_surface_bounds = None;
            self.hover_owner_bounds = None;
            self.hovered_preview = None;
        } else {
            let wants_tooltip = self.hovered_tile.is_some() && self.hover_elapsed >= TOOLTIP_DWELL;
            if wants_tooltip {
                self.tooltip_tile.clone_from(&self.hovered_tile);
            }
            let target = if wants_tooltip { 1.0 } else { 0.0 };
            if self.reduced_motion {
                // ADR-0029: no fade; the tooltip appears/disappears at once.
                self.tooltip_alpha = target;
            } else {
                let blend = 1.0 - (-TOOLTIP_FADE_SPEED * dt.min(1.0 / 30.0)).exp();
                self.tooltip_alpha += (target - self.tooltip_alpha) * blend;
            }
            if target == 0.0 && self.tooltip_alpha < 0.01 {
                self.tooltip_alpha = 0.0;
                self.tooltip_tile = None;
            }
            let waiting = self.hovered_tile.is_some() && self.hover_elapsed < TOOLTIP_DWELL;
            let fading = (target - self.tooltip_alpha).abs() > 0.01;
            self.anim_active |= waiting || fading;
        }

        if let Some((index, tile)) = self
            .tooltip_tile
            .as_ref()
            .and_then(|key| tiles.iter().enumerate().find(|(_, tile)| &tile.key == key))
        {
            // Once the popover exists, freeze its owner geometry. The Dock's
            // magnification spring can keep settling underneath it, but the
            // compositor preview, glass body, labels, and pointer bridge must
            // remain on one exact rectangle instead of chasing that spring a
            // frame apart.
            let owner = self.hover_owner_bounds.unwrap_or(icon_rects[index]);
            self.hover_owner_bounds = Some(owner);
            if tile.windows.is_empty() {
                let rect =
                    tooltip_rect(f, &tile.label, owner, (disp.x, disp.y), self.tooltip_alpha);
                self.hover_surface_bounds = Some(rect);
                self.live_preview = None;
                self.hovered_preview = None;
                render_tooltip(f, &tile.label, rect, self.tooltip_alpha);
            } else {
                let presentation =
                    live_preview_layout((disp.x, disp.y), owner, &tile.windows, self.tooltip_alpha);
                self.hover_surface_bounds = Some(to_lens_rect(presentation.panel));
                self.hovered_preview =
                    live_preview_hit(&presentation, cursor.x, cursor.y).filter(|id| {
                        windows
                            .iter()
                            .find(|window| window.id == *id)
                            .is_some_and(|window| !window.read_only)
                    });
                render_live_preview_chrome(f, &presentation, windows, self.hovered_preview);
                self.live_preview = Some(presentation);
            }
        } else {
            self.live_preview = None;
            self.hover_surface_bounds = None;
            self.hover_owner_bounds = None;
            self.hovered_preview = None;
        }
        self.app_menu.render(f, input, windows, i18n, out);
        if !self.app_menu.is_open() {
            self.menu_tile = None;
        }
        self.prev_down = down;
    }

    /// The dock reserves the bottom edge so tiled windows do not render under
    /// the bar (ADR-0024 chrome-aware work-area). The magnified-icon overshoot
    /// above the bar is intentionally not reserved — chrome draws over windows.
    fn reserved(&self) -> Reserved {
        if self.effective_autohide() || self.fullscreen_locked() {
            Reserved::default()
        } else {
            Reserved {
                top: 0,
                bottom: (DOCK_PANEL_HEIGHT + DOCK_BOTTOM_MARGIN) as i32,
                left: 0,
                right: 0,
            }
        }
    }

    fn prepare_backdrop(
        &mut self,
        input: &Input,
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) {
        if self.fullscreen_locked() {
            return;
        }
        let raw = input.as_raw();
        let display = (raw.display_size.x, raw.display_size.y);
        self.last_display = Some(display);
        let obscured = self.obscured_by_windows(windows, display);
        self.set_dock_obscured(obscured);

        // Resolve capsule hover before backdrop and damage policy are queried.
        // A forced maximize/collision collapse remains latched until the
        // pointer exits, preserving its anti-reopen contract.
        if !self.effective_autohide() || self.collapse_pending || self.autohide_reveal >= 0.2 {
            return;
        }
        let requested = Self::hidden_reveal_requested(
            &mut self.hidden_trigger_armed,
            (raw.cursor.x, raw.cursor.y),
            display,
        );
        if requested {
            self.autohide_idle = 0.0;
            self.anim_active = true;
            if self.reduced_motion {
                self.autohide_reveal = 1.0;
            }
        }
    }

    fn backdrop_blur_sigma(&self) -> f32 {
        if self.fullscreen_locked() { 0.0 } else { 12.0 }
    }

    fn backdrop_regions(
        &self,
        display: (f32, f32),
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        let mut regions = Vec::with_capacity(2);
        if let Some(region) = self.liquid_glass_region(display, windows) {
            regions.push(region.bounds);
        }
        if let Some(region) = self.hover_liquid_glass_region() {
            regions.push(region.bounds);
        }
        regions
    }

    fn liquid_glass_regions(
        &self,
        display: (f32, f32),
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<LiquidGlassRegion> {
        self.liquid_glass_region(display, windows)
            .into_iter()
            .chain(self.hover_liquid_glass_region())
            .collect()
    }

    fn live_preview_presentation(&self) -> Option<LivePreviewPresentation> {
        self.live_preview.clone()
    }

    fn captures_keyboard(&self) -> bool {
        self.app_menu.is_open()
    }

    fn key_char(&mut self, key: &KeyChar, _out: &mut ChromeEvents) {
        if matches!(key_action(key.keysym, key.ch), KeyAction::Escape) {
            self.app_menu.dismiss();
            self.menu_tile = None;
        }
    }

    /// The dock's magnify wave eases over many frames; report it as pending so
    /// the main loop keeps rendering (instead of blocking on the host queue)
    /// until every spring has rested.
    fn anim_pending(&self) -> bool {
        if self.fullscreen_locked() {
            return false;
        }
        let effective_autohide = self.effective_autohide();
        let target = if effective_autohide {
            if self.autohide_idle >= self.autohide_timeout && !self.app_menu.is_open() {
                0.0
            } else {
                1.0
            }
        } else {
            1.0
        };
        self.anim_active || (effective_autohide && (target - self.autohide_reveal).abs() > 0.002)
    }

    fn requires_composition(&self) -> bool {
        !self.fullscreen_locked()
    }

    fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
    }

    fn captures_pointer(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        if self.fullscreen_locked() {
            return false;
        }
        if self.app_menu.contains(x, y, display) {
            return true;
        }
        if self.hover_surface_contains(x, y) {
            return true;
        }
        let rest = self.pointer_bounds(windows, display);
        let effective_autohide = self.effective_autohide();
        let collapsed_indicator = Self::collapsed_indicator_contains((x, y), display);
        if Self::pointer_keeps_revealed(
            effective_autohide,
            self.autohide_reveal,
            collapsed_indicator,
            (x, y),
            rest,
            display.1,
        ) {
            return true;
        }
        if effective_autohide && self.autohide_reveal < 0.2 {
            return false;
        }
        let r = if effective_autohide {
            Self::collapsed_panel_rect(display, rest.w, self.autohide_reveal)
        } else {
            rest
        };
        x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h
    }

    fn persistent_decoration(&self) -> bool {
        true
    }

    fn cursor_shape_at(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        Some(CursorShape::Pointer)
    }

    fn update_windows(&mut self, windows: &[Window]) {
        if self.live_preview.as_ref().is_some_and(|presentation| {
            presentation
                .cards
                .iter()
                .any(|card| !windows.iter().any(|window| window.id == card.window))
        }) {
            self.dismiss_hover_surface();
        }
        let space_use = SpaceUse::from_windows(windows);
        let previous_space_use = self.space_use;
        self.space_use = space_use;
        if space_use == SpaceUse::Fullscreen {
            // Lock immediately; fullscreen must not expose even the
            // hidden-dock edge trigger between snapshot and render.
            self.dismiss_transient_ui();
            self.autohide_reveal = 0.0;
            self.autohide_idle = self.autohide_timeout;
            self.hidden_trigger_armed = false;
            self.collapse_pending = false;
            self.anim_active = false;
            return;
        }

        if let Some(display) = self.last_display {
            let obscured = self.obscured_by_windows(windows, display);
            self.set_dock_obscured(obscured);
        }
        if space_use == SpaceUse::Maximized && previous_space_use != SpaceUse::Maximized {
            // Maximized mode gains the complete work area by default. Start
            // one uninterrupted collapse so a stationary pointer inside the
            // old Dock rectangle cannot cancel the transition.
            self.dismiss_transient_ui();
            self.autohide_idle = self.autohide_timeout;
            self.hidden_trigger_armed = false;
            self.collapse_pending = true;
            self.anim_active = true;
        }
        if space_use == SpaceUse::Available
            && previous_space_use != SpaceUse::Available
            && !self.dock_obscured
        {
            self.collapse_pending = false;
            self.anim_active = true;
            if !self.autohide {
                self.autohide_idle = 0.0;
                self.hidden_trigger_armed = true;
            }
        }
    }

    fn update_app_catalog(&mut self, catalog: &AppCatalog) {
        self.app_menu.dismiss();
        self.menu_tile = None;
        self.dismiss_hover_surface();
        self.all_apps = catalog.apps.clone();
        self.apps = catalog
            .pinned
            .iter()
            .map(|e| DockApp {
                entry: e.clone(),
                keys: e.match_keys(),
            })
            .collect();
        self.icons = catalog.icons.clone();
        self.catalog_revision = self.catalog_revision.wrapping_add(1);
    }
}

impl Dock {
    /// Resolve the single animated Dock body once for both capture bounds and
    /// the analytic glass pass. The foreground material uses the same radius
    /// through `collapsing_dock_material`, eliminating the old two-rectangle
    /// blur cross and its hard corner discontinuities.
    fn liquid_glass_region(
        &self,
        display: (f32, f32),
        windows: &[Window],
    ) -> Option<LiquidGlassRegion> {
        if self.fullscreen_locked() {
            return None;
        }
        let expanded = self.visual_panel_bounds(windows, display);
        let bounds = if self.effective_autohide() {
            Self::collapsed_panel_rect(display, expanded.w, self.autohide_reveal)
        } else {
            expanded
        };
        let surface_progress = if self.effective_autohide() {
            Self::collapse_surface_progress(self.autohide_reveal)
        } else {
            1.0
        };
        // The shadow follows the morph in logical pixels: the full bar
        // casts the deep Dock shadow, the collapsed handle keeps a
        // proportionally tight one.
        let shadow_factor = (bounds.h / DOCK_PANEL_HEIGHT).clamp(0.35, 1.0);
        Some(LiquidGlassRegion {
            bounds: BackdropRegion {
                x: bounds.x,
                y: bounds.y,
                w: bounds.w,
                h: bounds.h,
            },
            corner_radius: collapsing_radius(surface_progress, bounds.h),
            opacity: 1.0,
            shadow_alpha: 0.20,
            shadow_blur: 12.0 * shadow_factor,
            shadow_offset_y: 6.0 * shadow_factor,
        })
    }

    fn hover_liquid_glass_region(&self) -> Option<LiquidGlassRegion> {
        let bounds = self.hover_surface_bounds?;
        if self.tooltip_alpha <= 0.01 || bounds.w <= 0.0 || bounds.h <= 0.0 {
            return None;
        }
        let preview = self.live_preview.is_some();
        Some(LiquidGlassRegion {
            bounds: BackdropRegion {
                x: bounds.x,
                y: bounds.y,
                w: bounds.w,
                h: bounds.h,
            },
            corner_radius: if preview {
                PREVIEW_PANEL_RADIUS
            } else {
                TOOLTIP_HEIGHT * 0.5
            },
            opacity: self.tooltip_alpha,
            shadow_alpha: if preview { 0.20 } else { 0.14 },
            shadow_blur: if preview { 16.0 } else { 10.0 },
            shadow_offset_y: if preview { 8.0 } else { 5.0 },
        })
    }
}

pub(super) fn hit_test_tiles(
    cursor: (f32, f32),
    rest_bounds: Rect,
    panel_rect: Rect,
    content_progress: f32,
    icon_rects: &[Rect],
) -> Option<usize> {
    let contains = |rect: Rect| {
        cursor.0 >= rect.x
            && cursor.1 >= rect.y
            && cursor.0 < rect.x + rect.w
            && cursor.1 < rect.y + rect.h
    };
    if content_progress <= AUTOHIDE_CONTENT_INTERACTION_MIN
        || !contains(rest_bounds)
        || !contains(panel_rect)
    {
        return None;
    }

    let mut hit = None;
    let mut best = f32::MAX;
    for (i, rect) in icon_rects.iter().enumerate() {
        let centre = rect.x + rect.w * 0.5;
        let half = rect.w * 0.5 + DOCK_TILE_GAP * content_progress * 0.5;
        let distance = (cursor.0 - centre).abs();
        if distance <= half && distance < best {
            best = distance;
            hit = Some(i);
        }
    }
    hit
}

/// Whether `entry` is the desktop entry a running `app_id` belongs to. The
/// match mirrors the launcher's running-app heuristic: `StartupWMClass`,
/// the desktop-id stem, or the icon name (case-insensitive).
pub(super) fn entry_matches_app_id(entry: &Entry, app_id: &str) -> bool {
    let want = app_id.to_ascii_lowercase();
    if want.is_empty() {
        return false;
    }
    entry
        .startup_wm_class
        .as_deref()
        .is_some_and(|wm| wm.to_ascii_lowercase() == want)
        || entry
            .id
            .trim_end_matches(".desktop")
            .eq_ignore_ascii_case(app_id)
        || entry
            .icon
            .as_deref()
            .is_some_and(|icon| icon.eq_ignore_ascii_case(app_id))
}

/// A fixed-size, transparent container used to force a layer (whose `rect` is
/// only an anchor, not a size) to a known width and height.
fn sized(w: f32, h: f32) -> LayoutOpts {
    LayoutOpts {
        width: w,
        height: h,
        ..Default::default()
    }
}

/// A fixed-size container that paints a rounded `bg` — the reliable filled-rect
/// primitive (lens paints a container's background at its solved size).
fn sized_fill(w: f32, h: f32, bg: Color, radius: f32) -> LayoutOpts {
    LayoutOpts {
        width: w,
        height: h,
        bg,
        radius,
        ..Default::default()
    }
}

fn scaled_alpha(alpha: u8, progress: f32) -> u8 {
    (f32::from(alpha) * progress.clamp(0.0, 1.0)).round() as u8
}

fn mix_channel(collapsed: u8, expanded: u8, progress: f32) -> u8 {
    let progress = progress.clamp(0.0, 1.0);
    (f32::from(collapsed) + (f32::from(expanded) - f32::from(collapsed)) * progress)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn collapsing_radius(surface_progress: f32, height: f32) -> f32 {
    let radius = AUTOHIDE_HANDLE_HEIGHT * 0.5
        + (Design::dark().radii.dock - AUTOHIDE_HANDLE_HEIGHT * 0.5) * surface_progress;
    radius.min(height * 0.5)
}

fn collapsing_dock_material(surface_progress: f32, height: f32) -> OverlayOpts {
    let mut material = materials::dock(&Design::dark());
    material.bg = Color::rgba(
        mix_channel(240, 255, surface_progress),
        mix_channel(243, 255, surface_progress),
        mix_channel(252, 255, surface_progress),
        mix_channel(64, 12, surface_progress),
    );
    material.radius = collapsing_radius(surface_progress, height);
    material
}

/// A centred grid row/column with the given gap, for the Launchpad glyph.
fn grid(gap: f32) -> LayoutOpts {
    LayoutOpts {
        gap,
        cross: Align::Center,
        ..Default::default()
    }
}

/// A single icon tile: no fill, no border, no padding — just the raster icon
/// (or glyph fallback), centred so a glyph smaller than the cell is centred.
fn tile_opts() -> OverlayOpts {
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

/// Resolve the name bubble once so the painted foreground and compositor
/// liquid-glass body use identical geometry on the following frame.
fn tooltip_rect(
    frame: &mut Frame,
    label: &str,
    owner: Rect,
    display: (f32, f32),
    _alpha: f32,
) -> Rect {
    let label = ellipsize(frame, label, 12.5, 224.0 - 22.0);
    let text = frame.measure_text(&label, 12.5);
    let width = (text.width + 22.0).clamp(54.0, 224.0);
    let x = (owner.x + owner.w * 0.5 - width * 0.5).clamp(8.0, (display.0 - width - 8.0).max(8.0));
    let y = (owner.y - TOOLTIP_GAP - TOOLTIP_HEIGHT).max(8.0);
    Rect {
        x,
        y,
        w: width,
        h: TOOLTIP_HEIGHT,
    }
}

/// A compact app-name bubble that follows the owning Dock icon. Its physical
/// body comes from the compositor's analytic glass pass; this foreground only
/// supplies a minimal tint and the text.
fn render_tooltip(frame: &mut Frame, label: &str, rect: Rect, alpha: f32) {
    let label = ellipsize(frame, label, 12.5, (rect.w - 22.0).max(0.0));
    let opacity = |base: u8| (base as f32 * alpha.clamp(0.0, 1.0)).round() as u8;
    let original = frame.theme();
    frame.set_theme(original.with_fg(Color::rgba(242, 244, 250, opacity(255))));
    let mut material = materials::dock(&Design::dark());
    material.bg = Color::rgba(255, 255, 255, opacity(12));
    material.radius = TOOLTIP_HEIGHT * 0.5;
    frame.layer("aegis-dock-app-name", rect, &material, |frame| {
        frame.row_ex(
            &LayoutOpts {
                height: TOOLTIP_HEIGHT,
                cross: Align::Center,
                ..Default::default()
            },
            |frame| frame.label_compact_sized(&label, 12.5),
        );
    });
    frame.set_theme(original);
}

/// Lay out every running window for one Dock application. Typical groups stay
/// in a single row; large groups wrap into a centred grid while preserving a
/// usable card width and staying inside the output margins.
pub(super) fn live_preview_layout(
    display: (f32, f32),
    owner: Rect,
    windows: &[aegis_core::window::WindowId],
    visibility: f32,
) -> LivePreviewPresentation {
    let count = windows.len().max(1);
    let available_w = (display.0 - PREVIEW_SCREEN_MARGIN * 2.0).max(1.0);
    let max_columns = (((available_w - PREVIEW_PANEL_PAD * 2.0 + PREVIEW_CARD_GAP)
        / (PREVIEW_CARD_MIN_WIDTH + PREVIEW_CARD_GAP))
        .floor() as usize)
        .clamp(1, count);
    let columns = count.min(max_columns);
    let rows = count.div_ceil(columns);
    let horizontal_width = (available_w
        - PREVIEW_PANEL_PAD * 2.0
        - PREVIEW_CARD_GAP * columns.saturating_sub(1) as f32)
        / columns as f32;
    let vertical_space = (owner.y
        - PREVIEW_PANEL_GAP
        - PREVIEW_SCREEN_MARGIN
        - PREVIEW_PANEL_PAD * 2.0
        - PREVIEW_CARD_GAP * rows.saturating_sub(1) as f32)
        .max(1.0);
    let vertical_width =
        ((vertical_space / rows as f32 - PREVIEW_LABEL_HEIGHT) / PREVIEW_ASPECT).max(1.0);
    let card_w = PREVIEW_CARD_MAX_WIDTH
        .min(horizontal_width)
        .min(vertical_width)
        .max(56.0);
    let preview_h = (card_w * PREVIEW_ASPECT).max(44.0);
    let card_h = preview_h + PREVIEW_LABEL_HEIGHT;
    let panel_w = card_w * columns as f32
        + PREVIEW_CARD_GAP * columns.saturating_sub(1) as f32
        + PREVIEW_PANEL_PAD * 2.0;
    let panel_h = card_h * rows as f32
        + PREVIEW_CARD_GAP * rows.saturating_sub(1) as f32
        + PREVIEW_PANEL_PAD * 2.0;
    let panel_x = (owner.x + owner.w * 0.5 - panel_w * 0.5).clamp(
        PREVIEW_SCREEN_MARGIN,
        (display.0 - panel_w - PREVIEW_SCREEN_MARGIN).max(PREVIEW_SCREEN_MARGIN),
    );
    let panel_y = (owner.y - PREVIEW_PANEL_GAP - panel_h).max(PREVIEW_SCREEN_MARGIN);
    let panel = Rect {
        x: panel_x,
        y: panel_y,
        w: panel_w,
        h: panel_h,
    };

    let cards = windows
        .iter()
        .copied()
        .enumerate()
        .map(|(index, window)| {
            let row = index / columns;
            let column = index % columns;
            let row_count = windows.len().saturating_sub(row * columns).min(columns);
            let row_w =
                card_w * row_count as f32 + PREVIEW_CARD_GAP * row_count.saturating_sub(1) as f32;
            let row_x = panel.x + (panel.w - row_w) * 0.5;
            let x = row_x + column as f32 * (card_w + PREVIEW_CARD_GAP);
            let y = panel.y + PREVIEW_PANEL_PAD + row as f32 * (card_h + PREVIEW_CARD_GAP);
            WindowSwitcherCard {
                window,
                geometry: aegis_core::window_switcher::Card {
                    outer: core_rect(Rect {
                        x,
                        y,
                        w: card_w,
                        h: card_h,
                    }),
                    preview: core_rect(Rect {
                        x,
                        y,
                        w: card_w,
                        h: preview_h,
                    }),
                    label: core_rect(Rect {
                        x,
                        y: y + preview_h,
                        w: card_w,
                        h: PREVIEW_LABEL_HEIGHT,
                    }),
                },
            }
        })
        .collect();
    LivePreviewPresentation {
        panel: core_rect(panel),
        cards,
        visibility: visibility.clamp(0.0, 1.0),
    }
}

fn render_live_preview_chrome(
    frame: &mut Frame,
    presentation: &LivePreviewPresentation,
    windows: &[Window],
    hovered: Option<aegis_core::window::WindowId>,
) {
    let opacity = |base: u8| (base as f32 * presentation.visibility.clamp(0.0, 1.0)).round() as u8;
    let panel = to_lens_rect(presentation.panel);
    let mut material = materials::dock(&Design::dark());
    material.bg = Color::rgba(255, 255, 255, opacity(12));
    material.radius = PREVIEW_PANEL_RADIUS;
    frame.layer("aegis-dock-live-previews", panel, &material, |frame| {
        frame.column_ex(&sized(panel.w, panel.h), |_| {});
    });

    let original = frame.theme();
    frame.set_theme(original.with_fg(Color::rgba(242, 244, 250, opacity(255))));
    for (index, card) in presentation.cards.iter().enumerate() {
        let Some(window) = windows.iter().find(|window| window.id == card.window) else {
            continue;
        };
        let outer = to_lens_rect(card.geometry.outer);
        let is_hovered = hovered == Some(window.id) && !window.read_only;
        frame.layer(
            &format!("aegis-dock-live-preview-card-{index}"),
            outer,
            &OverlayOpts {
                bg: Color::rgba(255, 255, 255, opacity(if is_hovered { 14 } else { 4 })),
                border: if is_hovered {
                    Color::rgba(126, 178, 255, opacity(245))
                } else {
                    Color::rgba(255, 255, 255, opacity(42))
                },
                border_width: if is_hovered { 2.0 } else { 1.0 },
                radius: 11.0,
                pad: 0.0,
                ..Default::default()
            },
            |frame| frame.column_ex(&sized(outer.w, outer.h), |_| {}),
        );

        let label_rect = to_lens_rect(card.geometry.label);
        let title = window
            .title
            .as_deref()
            .or(window.app_id.as_deref())
            .unwrap_or("Untitled");
        let label = ellipsize(frame, title, 11.5, (label_rect.w - 16.0).max(0.0));
        frame.layer(
            &format!("aegis-dock-live-preview-label-{index}"),
            label_rect,
            &OverlayOpts {
                bg: Color::rgba(255, 255, 255, opacity(9)),
                radius: 8.0,
                pad: 0.0,
                cross: Align::Center,
                ..Default::default()
            },
            |frame| {
                frame.row_ex(
                    &LayoutOpts {
                        width: label_rect.w,
                        height: label_rect.h,
                        pad: 8.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |frame| frame.label_compact_sized(&label, 11.5),
                );
            },
        );
    }
    frame.set_theme(original);
}

fn contains_core_rect(rect: aegis_core::Rect, x: f32, y: f32) -> bool {
    x >= rect.origin.x as f32
        && y >= rect.origin.y as f32
        && x < (rect.origin.x + rect.size.w) as f32
        && y < (rect.origin.y + rect.size.h) as f32
}

pub(super) fn live_preview_hit(
    presentation: &LivePreviewPresentation,
    x: f32,
    y: f32,
) -> Option<aegis_core::window::WindowId> {
    presentation
        .cards
        .iter()
        .rev()
        .find(|card| contains_core_rect(card.geometry.outer, x, y))
        .map(|card| card.window)
}

fn core_rect(rect: Rect) -> aegis_core::Rect {
    aegis_core::Rect::new(
        rect.x.round() as i32,
        rect.y.round() as i32,
        rect.w.round().max(1.0) as i32,
        rect.h.round().max(1.0) as i32,
    )
}

fn to_lens_rect(rect: aegis_core::Rect) -> Rect {
    Rect {
        x: rect.origin.x as f32,
        y: rect.origin.y as f32,
        w: rect.size.w as f32,
        h: rect.size.h as f32,
    }
}
