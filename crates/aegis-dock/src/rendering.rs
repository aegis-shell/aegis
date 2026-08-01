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
        // it. Maximized windows force the Dock into its collapsed overlay;
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

        // Pointer activation band for magnification and autohide reveal.
        let in_band = if self.collapse_pending {
            false
        } else if effective_autohide && self.autohide_reveal < 0.2 {
            Self::hidden_reveal_requested(
                &mut self.hidden_trigger_armed,
                (cursor.x, cursor.y),
                (disp.x, disp.y),
            )
        } else {
            cursor.x >= rest_bounds.x
                && cursor.y >= rest_bounds.y
                && cursor.x < rest_bounds.x + rest_bounds.w
                && cursor.y < rest_bounds.y + rest_bounds.h
        };
        let menu_open = self.app_menu.is_open();

        if effective_autohide {
            if in_band || menu_open {
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

        // The bar and collapsed indicator are the same layer. As it drains,
        // the glass grows more opaque and becomes the pale stadium handle
        // while staying anchored to the same bottom edge. Edge definition
        // comes from the compositor's glass rim, not a painted border.
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
        if down
            && !self.prev_down
            && !menu_was_open
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

        // Reveal an app name only after a short dwell, then keep it centred
        // above the current animated icon. Switching tiles resets the dwell so
        // a sweep across the dock does not produce a trail of labels.
        let hovered_tile = hit.map(|i| tiles[i].key.clone());
        if self.hovered_tile != hovered_tile {
            self.hovered_tile = hovered_tile;
            self.hover_elapsed = 0.0;
            self.tooltip_tile = None;
            self.tooltip_alpha = 0.0;
        } else if self.hovered_tile.is_some() && !self.app_menu.is_open() {
            self.hover_elapsed += dt;
        }

        if self.app_menu.is_open() {
            self.tooltip_tile = None;
            self.tooltip_alpha = 0.0;
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
            render_tooltip(
                f,
                &tile.label,
                icon_rects[index],
                (disp.x, disp.y),
                self.tooltip_alpha,
            );
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

    fn backdrop_blur_sigma(&self) -> f32 {
        if self.fullscreen_locked() || (self.effective_autohide() && self.autohide_reveal <= 0.05) {
            0.0
        } else {
            12.0
        }
    }

    fn backdrop_regions(
        &self,
        display: (f32, f32),
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        self.liquid_glass_region(display, windows)
            .map(|region| vec![region.bounds])
            .unwrap_or_default()
    }

    fn liquid_glass_regions(
        &self,
        display: (f32, f32),
        windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<LiquidGlassRegion> {
        self.liquid_glass_region(display, windows)
            .into_iter()
            .collect()
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
        if self.effective_autohide()
            && Self::collapse_content_progress(self.autohide_reveal)
                <= AUTOHIDE_CONTENT_INTERACTION_MIN
        {
            return Self::hidden_trigger_contains((x, y), display);
        }
        let rest = self.pointer_bounds(windows, display);
        let r = if self.effective_autohide() {
            Self::collapsed_panel_rect(display, rest.w, self.autohide_reveal)
        } else {
            rest
        };
        x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h
    }

    fn visible_during_modal(&self) -> bool {
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
        if self.fullscreen_locked() || (self.effective_autohide() && self.autohide_reveal <= 0.05) {
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
        Some(LiquidGlassRegion {
            bounds: BackdropRegion {
                x: bounds.x,
                y: bounds.y,
                w: bounds.w,
                h: bounds.h,
            },
            corner_radius: collapsing_radius(surface_progress, bounds.h),
            opacity: 1.0,
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
        mix_channel(150, 12, surface_progress),
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

/// A compact app-name bubble that follows the owning dock icon. It is kept
/// visually quieter than a context menu and never obscures the icon itself.
fn render_tooltip(frame: &mut Frame, label: &str, owner: Rect, display: (f32, f32), alpha: f32) {
    let label = truncate(label, 32);
    let text = frame.measure_text(&label, 12.5);
    let width = (text.width + 22.0).clamp(54.0, 224.0);
    let x = (owner.x + owner.w * 0.5 - width * 0.5).clamp(8.0, (display.0 - width - 8.0).max(8.0));
    let y = (owner.y - TOOLTIP_GAP - TOOLTIP_HEIGHT - (1.0 - alpha) * 3.0).max(8.0);
    let opacity = |base: u8| (base as f32 * alpha.clamp(0.0, 1.0)).round() as u8;
    let rect = Rect {
        x,
        y,
        w: width,
        h: TOOLTIP_HEIGHT,
    };
    let original = frame.theme();
    frame.set_theme(original.with_fg(Color::rgba(242, 244, 250, opacity(255))));
    frame.layer(
        "aegis-dock-app-name",
        rect,
        &OverlayOpts {
            // Frosted glass over the dock's backdrop-blur band: a light tint
            // with a bright edge, matching the bar's material instead of the
            // old opaque dark bubble.
            bg: Color::rgba(255, 255, 255, opacity(40)),
            border: Color::rgba(255, 255, 255, opacity(78)),
            border_width: 1.0,
            radius: TOOLTIP_HEIGHT * 0.5,
            pad: 0.0,
            cross: Align::Center,
            ..Default::default()
        },
        |frame| {
            frame.row_ex(
                &LayoutOpts {
                    height: TOOLTIP_HEIGHT,
                    cross: Align::Center,
                    ..Default::default()
                },
                |frame| frame.label_compact_sized(&label, 12.5),
            );
        },
    );
    frame.set_theme(original);
}
