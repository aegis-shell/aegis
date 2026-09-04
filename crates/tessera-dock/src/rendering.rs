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

        // The optimistic half of a reorder commit: apply the dragged order
        // locally so the strip never waits on the catalog round-trip. The
        // reconciling catalog push clears the flag in `update_app_catalog`.
        if let Some(order) = self.pending_order.take() {
            self.apps.sort_by_key(|app| {
                order
                    .iter()
                    .position(|id| id == &app.entry.id)
                    .unwrap_or(usize::MAX)
            });
            self.catalog_revision = self.catalog_revision.wrapping_add(1);
        }

        let dock_obscured = self.obscured_by_windows(windows, (disp.x, disp.y));
        self.set_dock_obscured(dock_obscured);

        let menu_was_open = self.app_menu.is_open();

        // The Launchpad tile always leads the strip (macOS-style), followed by
        // the pinned apps and any unpinned running windows. The strip is
        // workspace-global: it is built from the retained `all_windows`
        // snapshot, so pinned running-state and transient tiles reflect every
        // workspace, and clicking a tile on another workspace jumps to it.
        // The strip comes from the cache shared with `pointer_bounds`, so it
        // is rebuilt only when the window set, the catalog, or the localized
        // label changes.
        let application_label = i18n.text(Message::Applications);
        let tiles = Self::frame_tiles(
            &self.tile_cache,
            &self.apps,
            &self.all_apps,
            &self.icons,
            self.catalog_revision,
            &self.all_windows,
            Some(application_label),
        );
        let n = tiles.len();
        let pinned_count = tiles.iter().filter(|t| t.pinned).count();
        let position = self.position;
        let vertical = position.is_vertical();
        // The strip's long axis: x for a bottom dock, y for a side dock.
        // All tile layout runs along this axis and maps to x/y at the end.
        let axis_disp = if vertical { disp.y } else { disp.x };
        let cursor_axis = if vertical { cursor.y } else { cursor.x };
        let rest_bounds = Self::rest_bounds(n, pinned_count, position, (disp.x, disp.y));

        // Drop eased sizes for tiles no longer present so the map does not
        // grow unbounded across long sessions.
        let live_keys: std::collections::HashSet<&str> =
            tiles.iter().map(|t| t.key.as_str()).collect();
        self.sizes.retain(|key, _| live_keys.contains(key.as_str()));

        // ---- press & drag state machines --------------------------------
        // The press itself is armed after hit-testing below; this block
        // promotes an already-armed press past the drag threshold, drives
        // the live drag preview, and commits on the release edge. Clicks
        // fire on release (never on press) so the threshold gets first say.
        // Only disjoint fields are mutated while the tile strip borrow is
        // live — no `&mut self` calls here.
        let pressed_edge = down && !self.prev_down;
        let released_edge = !down && self.prev_down;
        let mut suppress_release_click = false;
        let mut released_target: Option<PressTarget> = None;
        let mut drag_strip: Option<usize> = None;
        let mut drag_insert: Option<usize> = None;
        let mut drag_section: Option<DropSection> = None;

        if let Some(mut press) = self.press.take() {
            let mut keep_press = true;
            if down {
                if !press.dragging
                    && Self::drag_threshold_exceeded(press.origin, (cursor.x, cursor.y))
                {
                    press.dragging = match &press.target {
                        PressTarget::PinnedTile(key) => tiles
                            .iter()
                            .any(|tile| &tile.key == key && tile.pinned && tile.app.is_some()),
                        PressTarget::Panel => true,
                        // A transient tile that resolves to a desktop entry
                        // drags across the section divider to pin.
                        PressTarget::OtherTile(key) => tiles.iter().any(|tile| {
                            &tile.key == key && !tile.pinned && tile.pin_entry.is_some()
                        }),
                    };
                }
                if press.dragging {
                    self.anim_active = true;
                    match &press.target {
                        PressTarget::PinnedTile(key) => {
                            let strip = tiles.iter().position(|tile| &tile.key == key);
                            match strip {
                                Some(strip)
                                    if tiles[strip].pinned && tiles[strip].app.is_some() =>
                                {
                                    drag_strip = Some(strip);
                                    let section = Self::drop_section_at(
                                        cursor_axis,
                                        n,
                                        pinned_count,
                                        axis_disp,
                                    );
                                    press.section = section;
                                    drag_section = Some(section);
                                    if section == DropSection::Pinned {
                                        // The insertion slot over the pinned-app
                                        // rest centres, the dragged tile excluded.
                                        let pinned_centres: Vec<f32> = (1..pinned_count)
                                            .filter(|slot| *slot != strip)
                                            .map(|slot| {
                                                Self::rest_centre_estimate(
                                                    slot,
                                                    n,
                                                    pinned_count,
                                                    axis_disp,
                                                )
                                            })
                                            .collect();
                                        let insert =
                                            Self::drop_insert_index(&pinned_centres, cursor_axis);
                                        press.insert = Some(insert);
                                        drag_insert = Some(insert);
                                    } else {
                                        press.insert = None;
                                    }
                                }
                                // The dragged tile vanished mid-gesture (a
                                // window or catalog change): cancel the press.
                                _ => keep_press = false,
                            }
                        }
                        PressTarget::Panel => {
                            if let Some(target) =
                                Self::edge_drag_target((cursor.x, cursor.y), (disp.x, disp.y))
                                && target != self.position
                            {
                                // Inline `set_position`: the strip borrow
                                // rules out `&mut self` calls here.
                                self.position = target;
                                self.app_menu.set_side(Self::popup_side_for(target));
                                self.anim_active = true;
                            }
                        }
                        PressTarget::OtherTile(key) => {
                            let strip = tiles.iter().position(|tile| &tile.key == key);
                            match strip {
                                Some(strip)
                                    if !tiles[strip].pinned && tiles[strip].pin_entry.is_some() =>
                                {
                                    drag_strip = Some(strip);
                                    let section = Self::drop_section_at(
                                        cursor_axis,
                                        n,
                                        pinned_count,
                                        axis_disp,
                                    );
                                    press.section = section;
                                    drag_section = Some(section);
                                    if section == DropSection::Pinned {
                                        // The insertion slot over every
                                        // pinned-app rest centre.
                                        let pinned_centres: Vec<f32> = (1..pinned_count)
                                            .map(|slot| {
                                                Self::rest_centre_estimate(
                                                    slot,
                                                    n,
                                                    pinned_count,
                                                    axis_disp,
                                                )
                                            })
                                            .collect();
                                        let insert =
                                            Self::drop_insert_index(&pinned_centres, cursor_axis);
                                        press.insert = Some(insert);
                                        drag_insert = Some(insert);
                                    } else {
                                        press.insert = None;
                                    }
                                }
                                _ => keep_press = false,
                            }
                        }
                    }
                }
            }
            if released_edge {
                released_target = Some(press.target.clone());
                if press.dragging {
                    suppress_release_click = true;
                    match &press.target {
                        PressTarget::PinnedTile(key) => {
                            let strip = tiles.iter().position(|tile| &tile.key == key);
                            if let Some(strip) = strip
                                && strip >= 1
                                && tiles[strip].pinned
                                && let Some(ai) = tiles[strip].app
                            {
                                match press.section {
                                    DropSection::Pinned => {
                                        if let Some(insert) = press.insert {
                                            let mut ids: Vec<String> = self
                                                .apps
                                                .iter()
                                                .map(|app| app.entry.id.clone())
                                                .collect();
                                            // Strip index 0 is the Launchpad; the
                                            // pinned apps sequence starts at 1.
                                            if Self::move_element(&mut ids, strip - 1, insert) {
                                                out.dock_reorder = Some(ids.clone());
                                                self.pending_order = Some(ids);
                                            }
                                        }
                                    }
                                    // Dropped past the divider: unpin. A still
                                    // running app reappears as a transient tile.
                                    DropSection::Transient => {
                                        let id = self.apps[ai].entry.id.clone();
                                        let ids: Vec<String> = self
                                            .apps
                                            .iter()
                                            .map(|app| app.entry.id.clone())
                                            .filter(|entry| entry != &id)
                                            .collect();
                                        self.apps.retain(|app| app.entry.id != id);
                                        out.dock_reorder = Some(ids.clone());
                                        self.pending_order = Some(ids);
                                    }
                                }
                            }
                        }
                        PressTarget::Panel => {
                            if self.position != press.start_position {
                                out.dock_position = Some(self.position);
                            }
                        }
                        PressTarget::OtherTile(key) => {
                            let strip = tiles.iter().position(|tile| &tile.key == key);
                            // Dropped into the pinned strip: pin at the
                            // previewed slot.
                            if let Some(strip) = strip
                                && !tiles[strip].pinned
                                && press.section == DropSection::Pinned
                                && let (Some(insert), Some(entry_id)) =
                                    (press.insert, tiles[strip].pin_entry.clone())
                                && let Some(entry) = self
                                    .all_apps
                                    .iter()
                                    .find(|entry| entry.id == entry_id)
                                    .cloned()
                            {
                                let pos = insert.min(self.apps.len());
                                let mut ids: Vec<String> =
                                    self.apps.iter().map(|app| app.entry.id.clone()).collect();
                                ids.insert(pos, entry_id);
                                self.apps.insert(
                                    pos,
                                    DockApp {
                                        keys: entry.match_keys(),
                                        entry,
                                    },
                                );
                                out.dock_reorder = Some(ids.clone());
                                self.pending_order = Some(ids);
                            }
                        }
                    }
                }
            } else if keep_press {
                self.press = Some(press);
            }
        }

        let drag_active = self.press.as_ref().is_some_and(|press| press.dragging);

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
        // the Dock. An active drag freezes the wave: the drop preview and the
        // magnification spring would fight over the same slots.
        let in_band = !self.collapse_pending
            && !drag_active
            && over_rest_bounds
            && (!effective_autohide || self.autohide_reveal >= 0.2);
        let capsule_entry =
            if self.collapse_pending || !effective_autohide || self.autohide_reveal >= 0.2 {
                false
            } else {
                Self::hidden_reveal_requested(
                    position,
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
                position,
                (disp.x, disp.y),
            );
        let keeps_revealed = over_dock_trigger || over_hover_surface;
        let menu_open = self.app_menu.is_open();

        // A held drag gesture keeps the Dock revealed even when the cursor
        // leaves the trigger corridor (an edge drag travels to another edge).
        if effective_autohide {
            if keeps_revealed || menu_open || drag_active {
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
                    cursor_axis - Self::rest_centre_estimate(i, n, pinned_count, axis_disp),
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
                    velocity: 0.0,
                }),
            };
            // A context menu must not become a moving target. Freeze the
            // complete wave exactly where it was opened; once the menu closes,
            // the same springs resume toward the live pointer targets.
            if menu_was_open {
                state.velocity = 0.0;
                eased.push(state.value);
                continue;
            }
            if self.reduced_motion {
                // ADR-0029: springs resolve to their target in one frame.
                state.value = target;
                state.velocity = 0.0;
                eased.push(target);
                continue;
            }
            eased.push(Self::spring(state, target, dt));
            // A spring is still animating while it is meaningfully off its
            // target or still moving. Sub-pixel drift is ignored so we don't
            // tick forever chasing float noise.
            let drifting = (state.value - target).abs() > 0.15 || state.velocity.abs() > 0.5;
            unsettled |= drifting;
        }
        // An active drag keeps frames ticking: the preview follows the cursor
        // even when every spring has rested.
        self.anim_active = unsettled || autohide_moving || drag_active;

        // Sum the eased widths (plus the inter-tile gap) to get the live bar
        // length along the strip axis. Ordinary neighbours sit one tile gap
        // apart; the section boundary replaces that gap with the wider
        // section gap. Centred on the dock's edge.
        let total_tiles: f32 = eased.iter().sum();

        // The drop-preview permutation: during a tile drag the dragged tile
        // still occupies a strip slot (the bar length is unchanged), but the
        // other tiles shift aside to open the insertion gap at the preview
        // slot. The Launchpad holds slot 0. A pinned tile re-enters within
        // the pinned range; dragged past the divider it previews as the
        // first transient tile; a transient tile dragged into the pinned
        // range previews at the hovered pinned slot. `section_slot` is where
        // the transient section begins in preview order.
        let (order, section_slot): (Vec<usize>, usize) = match (drag_strip, drag_section) {
            (Some(dragged), Some(DropSection::Transient)) if tiles[dragged].pinned => {
                let mut order: Vec<usize> = (0..n).filter(|slot| *slot != dragged).collect();
                order.insert((pinned_count - 1).min(order.len()), dragged);
                (order, pinned_count - 1)
            }
            (Some(dragged), Some(DropSection::Pinned)) => {
                let insert = drag_insert.unwrap_or(0);
                let mut order: Vec<usize> = (0..n).filter(|slot| *slot != dragged).collect();
                order.insert((insert + 1).min(order.len()), dragged);
                let section_slot = if tiles[dragged].pinned {
                    pinned_count
                } else {
                    pinned_count + 1
                };
                (order, section_slot)
            }
            _ => ((0..n).collect(), pinned_count),
        };
        let live_section_gap = if section_slot < order.len() {
            DOCK_SECTION_GAP
        } else {
            0.0
        };
        let bar_len = total_tiles
            + (n as f32 - 1.0) * DOCK_TILE_GAP
            + (live_section_gap - DOCK_TILE_GAP).max(0.0)
            + 2.0 * DOCK_PAD;
        let bar_axis_origin = (axis_disp - bar_len) * 0.5;

        // The running axis offset of each tile's centre, first to last in
        // preview order, from the shared strip geometry helper below.
        let centres = strip_centres(
            &eased,
            &order,
            bar_axis_origin,
            section_slot,
            live_section_gap,
        );
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
            Self::collapsed_panel_rect(position, (disp.x, disp.y), bar_len, self.autohide_reveal)
        } else {
            Self::panel_rect_for(position, bar_len, (disp.x, disp.y))
        };

        // Icons are anchored to the baseline strip on the panel's inner side
        // and grow toward the screen centre; as content drains into the
        // collapsed handle they converge on the same edge-centre sink as the
        // panel and their size reaches zero before the stadium settles.
        let icon_baseline = match position {
            DockPosition::Bottom => {
                panel_rect.y + panel_rect.h - DOCK_BASELINE_INSET * content_progress
            }
            DockPosition::Left => panel_rect.x + DOCK_BASELINE_INSET * content_progress,
            DockPosition::Right => {
                panel_rect.x + panel_rect.w - DOCK_BASELINE_INSET * content_progress
            }
        };
        let mut icon_rects: Vec<Rect> = (0..n)
            .map(|i| {
                let s = (eased[i] * content_progress).max(0.0);
                let centre_axis =
                    axis_disp * 0.5 + (centre(i) - axis_disp * 0.5) * content_progress;
                match position {
                    DockPosition::Bottom => Rect {
                        x: centre_axis - s * 0.5,
                        y: icon_baseline - s,
                        w: s,
                        h: s,
                    },
                    DockPosition::Left => Rect {
                        x: icon_baseline,
                        y: centre_axis - s * 0.5,
                        w: s,
                        h: s,
                    },
                    DockPosition::Right => Rect {
                        x: icon_baseline - s,
                        y: centre_axis - s * 0.5,
                        w: s,
                        h: s,
                    },
                }
            })
            .collect();

        // During a reorder drag the dragged tile leaves its slot and floats
        // at the cursor, slightly enlarged; the permutation opened the gap.
        if let Some(dragged) = drag_strip {
            let s = icon_rects[dragged].w * DRAG_LIFT_SCALE;
            icon_rects[dragged] = Rect {
                x: cursor.x - s * 0.5,
                y: cursor.y - s * 0.5,
                w: s,
                h: s,
            };
        }

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

        // The bar and collapsed indicator are the same placed surface, and
        // both are analytic glass bodies: as it drains, the bar morphs into
        // the stadium handle while keeping lensing, tint and its drop shadow.
        // Edge definition comes from the glass rim, not a painted border.
        let panel_thick = if vertical { panel_rect.w } else { panel_rect.h };
        let dock_material = collapsing_dock_material(&self.design, surface_progress, panel_thick);
        // A placed surface with an empty body collapses to ~0 (the rect is
        // only an anchor, not a size); a fixed-size child forces it to the
        // bar size.
        f.place(
            "tessera-dock",
            &chrome_place(panel_rect, dock_material),
            |f| {
                f.column_ex(&sized(panel_rect.w, panel_rect.h), |_| {});
            },
        );

        // Hit-test only content that still exists in the morphing surface.
        // The resting bounds remain the outer ownership limit, while the
        // current panel and transformed icon geometry prevent the collapsed
        // handle (or an icon's former position) from impersonating a tile.
        // An active drag owns the pointer: no hover, click, or menu target.
        let hit = if drag_active {
            None
        } else {
            hit_test_tiles(
                (cursor.x, cursor.y),
                rest_bounds,
                panel_rect,
                content_progress,
                &icon_rects,
                vertical,
            )
        };

        // Draw each tile's icon, then its running dot. Once content has
        // reached the sink, no tile placements remain behind the stadium.
        let dock_colors = self.design.dock;
        if content_progress > AUTOHIDE_CONTENT_INTERACTION_MIN {
            // The drained content fades through the lens opacity switch;
            // restored after the section divider below.
            f.set_opacity(content_progress);
            for (i, t) in tiles.iter().enumerate() {
                let s = icon_rects[i].w;
                let cx = icon_rects[i].x + s * 0.5;
                let cy = icon_rects[i].y + s * 0.5;
                let rect = icon_rects[i];
                let icon_id = format!("tessera-dock-icon-{}", t.key);
                if t.launchpad {
                    // A rounded "app tile" with a 3×3 grid, so it reads as macOS's
                    // Launchpad button. The grid (real content) sizes the surface;
                    // the surface paints the rounded background behind it.
                    let bg = LayoutOpts {
                        bg: dock_colors.launchpad_tile_bg,
                        border: dock_colors.launchpad_tile_border,
                        border_width: 1.0,
                        radius: s * 0.22,
                        pad: s * 0.2,
                        cross: Align::Center,
                        ..surface_layout()
                    };
                    let gap = s * 0.1;
                    let d = (s - 2.0 * (s * 0.2) - 2.0 * gap) / 3.0;
                    f.place(&icon_id, &chrome_place(rect, bg), |f| {
                        f.column_ex(&grid(gap), |f| {
                            for _ in 0..3 {
                                f.row_ex(&grid(gap), |f| {
                                    for _ in 0..3 {
                                        f.column_ex(
                                            &sized_fill(d, d, dock_colors.launchpad_grid, d * 0.3),
                                            |_| {},
                                        );
                                    }
                                });
                            }
                        });
                    });
                } else {
                    f.place(&icon_id, &chrome_place(rect, tile_opts()), |f| {
                        match t.icon.or_else(|| self.icons.default_icon()) {
                            // The pointer crosses from the binary's flux binding type to
                            // lens's ABI-identical flux_image.
                            Some(ptr) => unsafe {
                                f.image(ptr as *mut lens::sys::flux_image, s, s)
                            },
                            None => f.icon(Icon::FileText, s * 0.6),
                        }
                    });
                }

                if t.running && Some(i) != drag_strip {
                    // Centre the dot in the flat strip between the icon
                    // baseline and the panel's near edge, so it never falls
                    // into the rounded corner region (and outside the bar)
                    // on the first or last tile. On a side dock the strip —
                    // and the stadium's long axis — is vertical.
                    let dot_long = if t.windows.len() > 1 {
                        DOCK_DOT_STADIUM
                    } else {
                        DOCK_DOT
                    } * content_progress;
                    let dot_thick = DOCK_DOT * content_progress;
                    let strip_span = DOCK_BASELINE_INSET.max(DOCK_DOT) * content_progress;
                    let dot_rect = match position {
                        DockPosition::Bottom => Rect {
                            x: cx - dot_long * 0.5,
                            y: icon_baseline + (strip_span - dot_thick) * 0.5,
                            w: dot_long,
                            h: dot_thick,
                        },
                        DockPosition::Left => Rect {
                            x: icon_baseline + (strip_span - dot_thick) * 0.5,
                            y: cy - dot_long * 0.5,
                            w: dot_thick,
                            h: dot_long,
                        },
                        DockPosition::Right => Rect {
                            x: icon_baseline - strip_span + (strip_span - dot_thick) * 0.5,
                            y: cy - dot_long * 0.5,
                            w: dot_thick,
                            h: dot_long,
                        },
                    };
                    let color = if t.activated {
                        dock_colors.running_dot_active
                    } else {
                        dock_colors.running_dot_inactive
                    };
                    let dot_id = format!("tessera-dock-dot-{}", t.key);
                    f.place(&dot_id, &chrome_place(dot_rect, tile_opts()), |f| {
                        f.column_ex(
                            &sized_fill(dot_rect.w, dot_rect.h, color, dot_thick * 0.5),
                            |_| {},
                        );
                    });
                }
            }
        }

        // A slim divider in the section gap separates the kept strip from
        // the transient running apps, like macOS's Dock. On a side dock the
        // divider lies across the strip: a horizontal hairline. The boundary
        // follows the live preview order, so the divider itself takes part in
        // the reflow when a drag crosses it.
        if live_section_gap > 0.0 && content_progress > AUTOHIDE_CONTENT_INTERACTION_MIN {
            // Sit at the midpoint of the edge-to-edge gap rather than the
            // centre-to-centre midpoint: a boundary tile magnifying toward
            // the divider pushes it aside instead of swallowing it, so the
            // clearance stays one ordinary tile gap on both sides through
            // the wave.
            let before = order[section_slot - 1];
            let after = order[section_slot];
            let divider_axis =
                (centre(before) + eased[before] * 0.5 + centre(after) - eased[after] * 0.5) * 0.5;
            let divider_axis =
                axis_disp * 0.5 + (divider_axis - axis_disp * 0.5) * content_progress;
            let (divider_center, divider_thick) = snapped_hairline(divider_axis, self.scale);
            let divider_len = DOCK_TILE * 0.55 * content_progress;
            let divider_rect = match position {
                DockPosition::Bottom => Rect {
                    x: divider_center - divider_thick * 0.5,
                    y: panel_rect.y + (panel_rect.h - divider_len) * 0.5,
                    w: divider_thick,
                    h: divider_len,
                },
                DockPosition::Left | DockPosition::Right => Rect {
                    x: panel_rect.x + (panel_rect.w - divider_len) * 0.5,
                    y: divider_center - divider_thick * 0.5,
                    w: divider_len,
                    h: divider_thick,
                },
            };
            f.place(
                "tessera-dock-section-divider",
                &chrome_place(divider_rect, surface_layout()),
                |f| {
                    f.column_ex(
                        &sized_fill(
                            divider_rect.w,
                            divider_rect.h,
                            dock_colors.section_divider,
                            // Deliberately sharp: the divider is a hairline,
                            // and any radius at or below 0.5 falls back to the
                            // same sharp rect path in lens anyway.
                            0.0,
                        ),
                        |_| {},
                    );
                },
            );
        }
        f.set_opacity(1.0);

        // Preview cards keep press-edge activation (no drag gesture starts
        // on them); the per-frame pressed flag is not cleared by the host,
        // so the button-level transition was tracked at the top of render.
        let previous_hovered_preview = self.hovered_preview;
        let preview_hit = self
            .live_preview
            .as_ref()
            .and_then(|presentation| live_preview_hit(presentation, cursor.x, cursor.y));
        self.hovered_preview = preview_hit.filter(|id| {
            self.all_windows
                .iter()
                .find(|window| window.id == *id)
                .is_some_and(|window| !window.read_only)
        });
        self.anim_active |= self.hovered_preview != previous_hovered_preview;
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

        // Arm the press lifecycle: a pinned-app tile press may become a
        // reorder drag; a press on empty panel space may become an edge
        // drag. Anything else stays a pending click.
        if pressed_edge && !menu_was_open && !clicked_preview && self.press.is_none() {
            if let Some(i) = hit {
                let tile = &tiles[i];
                let target = if tile.pinned && tile.app.is_some() {
                    PressTarget::PinnedTile(tile.key.clone())
                } else {
                    PressTarget::OtherTile(tile.key.clone())
                };
                let section = if tile.pinned {
                    DropSection::Pinned
                } else {
                    DropSection::Transient
                };
                self.press = Some(PressState {
                    origin: (cursor.x, cursor.y),
                    target,
                    dragging: false,
                    section,
                    insert: None,
                    start_position: position,
                });
            } else if over_rest_bounds
                && content_progress > AUTOHIDE_CONTENT_INTERACTION_MIN
                && cursor.x >= panel_rect.x
                && cursor.y >= panel_rect.y
                && cursor.x < panel_rect.x + panel_rect.w
                && cursor.y < panel_rect.y + panel_rect.h
            {
                self.press = Some(PressState {
                    origin: (cursor.x, cursor.y),
                    target: PressTarget::Panel,
                    dragging: false,
                    section: DropSection::Pinned,
                    insert: None,
                    start_position: position,
                });
            }
        }

        // A click fires on the release edge, and only when the press began
        // on the same tile and never became a drag. A release after a drag,
        // or off the pressed tile, activates nothing.
        if released_edge
            && !suppress_release_click
            && !menu_was_open
            && let (Some(target), Some(i)) = (released_target, hit)
        {
            let pressed_this_tile = match &target {
                PressTarget::PinnedTile(key) | PressTarget::OtherTile(key) => key == &tiles[i].key,
                PressTarget::Panel => false,
            };
            if pressed_this_tile {
                let t = &tiles[i];
                if t.launchpad {
                    out.toggle_launcher = true;
                } else if let Some(id) = t.focus {
                    out.clicked = Some(id);
                } else if let Some(ai) = t.spawn {
                    out.activate_entry(self.apps[ai].entry.clone());
                }
            }
        }
        let right_pressed = input
            .as_raw()
            .mouse_pressed
            .get(1)
            .copied()
            .unwrap_or(false);
        // A held left press (pending click or drag) suppresses the menu for
        // its tile; the dragged tile in particular must not pop one up.
        if right_pressed
            && self.press.is_none()
            && let Some(i) = hit
        {
            let tile = &tiles[i];
            if !tile.launchpad {
                let pin_action = if let Some(ai) = tile.app {
                    // A pinned tile always offers removal from the strip.
                    Some(PinAction::Unpin(self.apps[ai].entry.id.clone()))
                } else {
                    // A transient tile offers "Keep in Dock" when its app
                    // resolved to an enumerated desktop entry at build time.
                    tile.pin_entry.clone().map(PinAction::Pin)
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
            let side = Self::popup_side_for(position);
            if tile.windows.is_empty() {
                let rect = tooltip_rect(
                    f,
                    &self.design,
                    &tile.label,
                    owner,
                    (disp.x, disp.y),
                    self.tooltip_alpha,
                    side,
                );
                self.hover_surface_bounds = Some(rect);
                self.live_preview = None;
                self.hovered_preview = None;
                render_tooltip(f, &self.design, &tile.label, rect, self.tooltip_alpha);
            } else {
                let mut presentation = live_preview_layout(
                    &self.design,
                    (disp.x, disp.y),
                    owner,
                    &tile.windows,
                    self.tooltip_alpha,
                    position,
                );
                self.hover_surface_bounds = Some(to_lens_rect(presentation.panel));
                let previous_hovered_preview = self.hovered_preview;
                self.hovered_preview =
                    live_preview_hit(&presentation, cursor.x, cursor.y).filter(|id| {
                        self.all_windows
                            .iter()
                            .find(|window| window.id == *id)
                            .is_some_and(|window| !window.read_only)
                    });
                self.anim_active |= self.hovered_preview != previous_hovered_preview;
                presentation.focused = self.hovered_preview;
                render_live_preview_chrome(
                    f,
                    &self.design,
                    &presentation,
                    &self.all_windows,
                    self.hovered_preview,
                );
                self.live_preview = Some(presentation);
            }
        } else {
            self.live_preview = None;
            self.hover_surface_bounds = None;
            self.hover_owner_bounds = None;
            self.hovered_preview = None;
        }
        self.app_menu.render(f, input, &self.all_windows, i18n, out);
        if !self.app_menu.is_open() {
            self.menu_tile = None;
        }
        self.prev_down = down;
    }

    /// The dock reserves its screen edge so tiled windows do not render under
    /// the bar (ADR-0024 chrome-aware work-area). The magnified-icon overshoot
    /// past the bar is intentionally not reserved — chrome draws over windows.
    fn reserved(&self) -> Reserved {
        if self.effective_autohide() || self.fullscreen_locked() {
            return Reserved::default();
        }
        let extent = (DOCK_PANEL_HEIGHT + DOCK_EDGE_MARGIN) as i32;
        match self.position {
            DockPosition::Bottom => Reserved {
                top: 0,
                bottom: extent,
                left: 0,
                right: 0,
            },
            DockPosition::Left => Reserved {
                top: 0,
                bottom: 0,
                left: extent,
                right: 0,
            },
            DockPosition::Right => Reserved {
                top: 0,
                bottom: 0,
                left: 0,
                right: extent,
            },
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
            self.position,
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
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        // The panel itself declares no backdrop region: its glass body
        // carries an animation-stable `capture_bounds` footprint instead, so
        // reveal and magnification morphs never invalidate the capture.
        let mut regions = Vec::with_capacity(2);
        if let Some(region) = self.hover_liquid_glass_region() {
            regions.push(region.bounds);
        }
        if let Some(region) = self.app_menu.liquid_glass_region(display) {
            regions.push(region.bounds);
        }
        regions
    }

    fn liquid_glass_regions(
        &self,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<LiquidGlassRegion> {
        self.liquid_glass_region(display)
            .into_iter()
            .chain(self.hover_liquid_glass_region())
            .chain(self.app_menu.liquid_glass_region(display))
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

    fn damage_region(&self, _windows: &[Window], display: (f32, f32)) -> Option<tessera_model::Rect> {
        if self.fullscreen_locked() {
            return None;
        }
        // The magnify wave, reveal morph, and spring overshoot all stay
        // inside the same envelope the backdrop capture uses — a strip one
        // panel thick on the dock's edge, wide enough for every tile at full
        // magnification plus overshoot.
        let mut region = {
            let rect = self.capture_footprint(display);
            tessera_model::Rect::new(rect.x as i32, rect.y as i32, rect.w as i32, rect.h as i32)
        };
        // A live-preview panel animates open above the strip (or beside it
        // for a side dock); its panel rect is computed for presentation each
        // frame, so the union with the strip covers both the panel and the
        // strip it emerges from.
        if let Some(preview) = self.live_preview_presentation() {
            region = region.union(preview.panel);
        }
        // Tooltips float above the hovered tile while their dwell fade runs;
        // a hover band reaching from the strip toward the screen centre is
        // wider than any tooltip, so it bounds the fade's footprint.
        if self.tooltip_alpha > 0.001 && self.tooltip_tile.is_some() {
            let band = match self.position {
                DockPosition::Bottom => {
                    let y1 = display.1 - DOCK_EDGE_MARGIN;
                    let y0 = (y1 - DOCK_PANEL_HEIGHT - DOCK_EDGE_MARGIN - TOOLTIP_BAND).max(0.0);
                    tessera_model::Rect::new(0, y0 as i32, display.0 as i32, (y1 - y0) as i32)
                }
                DockPosition::Left => {
                    let x1 = DOCK_EDGE_MARGIN + DOCK_PANEL_HEIGHT + TOOLTIP_BAND;
                    tessera_model::Rect::new(0, 0, x1.min(display.0) as i32, display.1 as i32)
                }
                DockPosition::Right => {
                    let x0 =
                        (display.0 - DOCK_EDGE_MARGIN - DOCK_PANEL_HEIGHT - TOOLTIP_BAND).max(0.0);
                    tessera_model::Rect::new(x0 as i32, 0, (display.0 - x0) as i32, display.1 as i32)
                }
            };
            region = region.union(band);
        }
        Some(region)
    }

    fn requires_composition(&self) -> bool {
        !self.fullscreen_locked()
    }

    fn update(&mut self, update: ChromeUpdate<'_>) {
        match update {
            ChromeUpdate::ReducedMotion(reduced) => self.reduced_motion = reduced,
            ChromeUpdate::Windows(windows) => self.update_windows(windows),
            ChromeUpdate::AllWindows(windows) => self.update_all_windows(windows),
            ChromeUpdate::Scale(scale) => self.scale = scale,
            ChromeUpdate::Appearance(design) => {
                self.design = *design;
                self.app_menu.update(update);
            }
            ChromeUpdate::AppCatalog(catalog) => self.update_app_catalog(catalog),
            _ => {}
        }
    }

    fn captures_pointer(
        &self,
        x: f32,
        y: f32,
        display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        if self.fullscreen_locked() {
            return false;
        }
        // A held drag gesture owns the pointer across the whole output: a
        // reorder drag previews across the strip and an edge drag travels to
        // another screen edge.
        if self.press.as_ref().is_some_and(|press| press.dragging) {
            return true;
        }
        if self.app_menu.contains(x, y, display) {
            return true;
        }
        if self.hover_surface_contains(x, y) {
            return true;
        }
        let rest = self.pointer_bounds(display);
        let effective_autohide = self.effective_autohide();
        let collapsed_indicator =
            Self::collapsed_indicator_contains(self.position, (x, y), display);
        if Self::pointer_keeps_revealed(
            effective_autohide,
            self.autohide_reveal,
            collapsed_indicator,
            (x, y),
            rest,
            self.position,
            display,
        ) {
            return true;
        }
        if effective_autohide && self.autohide_reveal < 0.2 {
            return false;
        }
        let rest_len = if self.position.is_vertical() {
            rest.h
        } else {
            rest.w
        };
        let r = if effective_autohide {
            Self::collapsed_panel_rect(self.position, display, rest_len, self.autohide_reveal)
        } else {
            rest
        };
        x >= r.x && y >= r.y && x < r.x + r.w && y < r.y + r.h
    }

    fn persistent_decoration(&self) -> bool {
        true
    }

    fn minimize_targets(
        &self,
        display: (f32, f32),
        out: &mut Vec<(tessera_model::window::WindowId, tessera_model::Rect)>,
    ) {
        out.extend(self.minimize_targets(display));
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
}

impl Dock {
    /// Retain the workspace-global window set the tile strip is built from.
    /// Preview cards are validated against this list (not the visible set) so
    /// a preview of a window on another workspace survives workspace switches
    /// and is dismissed only when the window actually goes away.
    pub(crate) fn update_all_windows(&mut self, windows: &[Window]) {
        if self.live_preview.as_ref().is_some_and(|presentation| {
            presentation
                .cards
                .iter()
                .any(|card| !windows.iter().any(|window| window.id == card.window))
        }) {
            self.dismiss_hover_surface();
        }
        self.all_windows = windows.to_vec();
    }

    pub(crate) fn update_windows(&mut self, windows: &[Window]) {
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

    pub(crate) fn update_app_catalog(&mut self, catalog: &AppCatalog) {
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
        // The push reconciles an optimistic drag reorder: the locally applied
        // order and the pushed order are the same list by construction.
        self.pending_order = None;
        // An edge drag in flight owns the live position; the catalog catches
        // up when the gesture ends.
        if !self
            .press
            .as_ref()
            .is_some_and(|press| press.dragging && matches!(press.target, PressTarget::Panel))
        {
            self.set_position(catalog.position);
        }
    }
    /// Resolve the single animated Dock body once for both capture bounds and
    /// the analytic glass pass. The foreground material uses the same radius
    /// through `collapsing_dock_material`, eliminating the old two-rectangle
    /// blur cross and its hard corner discontinuities.
    fn liquid_glass_region(&self, display: (f32, f32)) -> Option<LiquidGlassRegion> {
        if self.fullscreen_locked() {
            return None;
        }
        let expanded = self.visual_panel_bounds(display);
        let expanded_len = if self.position.is_vertical() {
            expanded.h
        } else {
            expanded.w
        };
        let bounds = if self.effective_autohide() {
            Self::collapsed_panel_rect(self.position, display, expanded_len, self.autohide_reveal)
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
        // proportionally tight one. The panel's thickness is its height on
        // the bottom edge and its width on a side edge.
        let thick = if self.position.is_vertical() {
            bounds.w
        } else {
            bounds.h
        };
        let shadow_factor = (thick / DOCK_PANEL_HEIGHT).clamp(0.35, 1.0);
        let design = self.design;
        let mut region = LiquidGlassRegion::from_role(
            &design,
            GlassRole::Dock,
            BackdropRegion::from(bounds),
            collapsing_radius(&design, surface_progress, thick),
            1.0,
        );
        region.shadow_blur *= shadow_factor;
        region.shadow_offset_y *= shadow_factor;
        Some(region.with_capture_bounds(BackdropRegion::from(self.capture_footprint(display))))
    }

    fn hover_liquid_glass_region(&self) -> Option<LiquidGlassRegion> {
        let bounds = self.hover_surface_bounds?;
        if self.tooltip_alpha <= 0.01 || bounds.w <= 0.0 || bounds.h <= 0.0 {
            return None;
        }
        let is_preview = self.live_preview.is_some();
        let design = self.design;
        let focus = self.live_preview.as_ref().and_then(|presentation| {
            preview::focus_field(&presentation.cards, presentation.focused, &design)
        });
        Some(
            LiquidGlassRegion::from_role(
                &design,
                if is_preview {
                    GlassRole::FloatingPanel
                } else {
                    GlassRole::Tooltip
                },
                BackdropRegion::from(bounds),
                if is_preview {
                    design.radii.glass_panel
                } else {
                    TOOLTIP_HEIGHT * 0.5
                },
                self.tooltip_alpha,
            )
            .with_id(liquid_glass_region_id("tessera-dock-hover"))
            .with_focus(focus),
        )
    }
}

/// The running axis offset of each tile's centre, first to last in preview
/// `order`. `section_slot` is the slot in `order` where the transient section
/// begins: that boundary keeps the wider section gap instead of the ordinary
/// tile gap — the same total the resting geometry (`rest_centre_estimate`,
/// `rest_bounds`, the bar length) assumes, so a settled strip lands exactly
/// on the interaction model's geometry and the section divider keeps equal
/// clearance on both sides.
pub(super) fn strip_centres(
    eased: &[f32],
    order: &[usize],
    bar_axis_origin: f32,
    section_slot: usize,
    section_gap: f32,
) -> Vec<f32> {
    let mut centres = vec![0.0_f32; eased.len()];
    let mut next_axis = bar_axis_origin + DOCK_PAD;
    for (slot, tile_index) in order.iter().copied().enumerate() {
        if slot > 0 {
            let gap = if slot == section_slot {
                section_gap
            } else {
                DOCK_TILE_GAP
            };
            next_axis += gap;
        }
        centres[tile_index] = next_axis + eased[tile_index] * 0.5;
        next_axis += eased[tile_index];
    }
    centres
}

/// Snap a hairline's logical center to the device pixel grid and give it
/// exactly one device pixel of width. At scale ≥ 2 a fractional 1-logical-px
/// line at low alpha rasterises to nothing; snapping keeps the section
/// divider crisp at any scale. At scale 1 this is the same 1 logical px line
/// at (within half a device pixel of) the same center.
pub(super) fn snapped_hairline(center: f32, scale: f32) -> (f32, f32) {
    let scale = scale.max(1.0);
    ((center * scale).round() / scale, 1.0 / scale)
}

/// Nearest-tile hit test along the strip axis (`vertical` picks y for a
/// side dock, x for a bottom dock). Gated by the resting bounds, the live
/// panel, and the content drain, so collapsed geometry owns no tiles.
pub(super) fn hit_test_tiles(
    cursor: (f32, f32),
    rest_bounds: Rect,
    panel_rect: Rect,
    content_progress: f32,
    icon_rects: &[Rect],
    vertical: bool,
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
    let cursor_axis = if vertical { cursor.1 } else { cursor.0 };
    for (i, rect) in icon_rects.iter().enumerate() {
        // Tiles are square, so either dimension reads the axis centre.
        let centre = if vertical {
            rect.y + rect.h * 0.5
        } else {
            rect.x + rect.w * 0.5
        };
        let half = rect.w * 0.5 + DOCK_TILE_GAP * content_progress * 0.5;
        let distance = (cursor_axis - centre).abs();
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

/// A fixed-size, transparent container used to force a placed surface (whose
/// `rect` is only an anchor, not a size) to a known width and height.
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

fn mix_channel(collapsed: u8, expanded: u8, progress: f32) -> u8 {
    let progress = progress.clamp(0.0, 1.0);
    (f32::from(collapsed) + (f32::from(expanded) - f32::from(collapsed)) * progress)
        .round()
        .clamp(0.0, 255.0) as u8
}

fn collapsing_radius(design: &Design, surface_progress: f32, height: f32) -> f32 {
    let radius = AUTOHIDE_HANDLE_HEIGHT * 0.5
        + (design.radii.glass_panel - AUTOHIDE_HANDLE_HEIGHT * 0.5) * surface_progress;
    radius.min(height * 0.5)
}

fn collapsing_dock_material(design: &Design, surface_progress: f32, height: f32) -> LayoutOpts {
    let mut material = materials::glass_panel(design);
    // The painted tint interpolates between the two palette endpoints as the
    // surface morphs between the collapsed handle and the expanded bar.
    let collapsed = design.dock.bar_surface_collapsed.components();
    let expanded = design.dock.bar_surface_expanded.components();
    material.bg = Color::rgba(
        mix_channel(collapsed.0, expanded.0, surface_progress),
        mix_channel(collapsed.1, expanded.1, surface_progress),
        mix_channel(collapsed.2, expanded.2, surface_progress),
        mix_channel(collapsed.3, expanded.3, surface_progress),
    );
    material.radius = collapsing_radius(design, surface_progress, height);
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
fn tile_opts() -> LayoutOpts {
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

/// Resolve the name bubble once so the painted foreground and compositor
/// liquid-glass body use identical geometry on the following frame. The
/// bubble opens toward `side` of its owner: above a bottom dock's tile, or
/// into the output beside a side dock's tile.
fn tooltip_rect(
    frame: &mut Frame,
    design: &Design,
    label: &str,
    owner: Rect,
    display: (f32, f32),
    _alpha: f32,
    side: PopupSide,
) -> Rect {
    let label = ellipsize(frame, label, design.typography.label, 224.0 - 22.0);
    let text = frame.measure_text(&label, design.typography.label);
    let width = (text.width + 22.0).clamp(54.0, 224.0);
    match side {
        PopupSide::Above => {
            let x = (owner.x + owner.w * 0.5 - width * 0.5)
                .clamp(8.0, (display.0 - width - 8.0).max(8.0));
            let y = (owner.y - TOOLTIP_GAP - TOOLTIP_HEIGHT).max(8.0);
            Rect {
                x,
                y,
                w: width,
                h: TOOLTIP_HEIGHT,
            }
        }
        PopupSide::Right | PopupSide::Left => {
            let x = if side == PopupSide::Right {
                (owner.x + owner.w + TOOLTIP_GAP).min((display.0 - width - 8.0).max(8.0))
            } else {
                (owner.x - TOOLTIP_GAP - width).max(8.0)
            };
            let y = (owner.y + owner.h * 0.5 - TOOLTIP_HEIGHT * 0.5)
                .clamp(8.0, (display.1 - TOOLTIP_HEIGHT - 8.0).max(8.0));
            Rect {
                x,
                y,
                w: width,
                h: TOOLTIP_HEIGHT,
            }
        }
    }
}

/// A compact app-name bubble that follows the owning Dock icon. Its physical
/// body comes from the compositor's analytic glass pass; this foreground only
/// supplies a minimal tint and the text.
fn render_tooltip(frame: &mut Frame, design: &Design, label: &str, rect: Rect, alpha: f32) {
    let label = ellipsize(
        frame,
        label,
        design.typography.label,
        (rect.w - 22.0).max(0.0),
    );
    let original = frame.theme();
    frame.set_theme(original.with_fg(design.colors.menu_text));
    frame.set_opacity(alpha);
    let mut material = materials::glass_panel(design);
    // The painted layer stays the design's glass whisper; the physical body
    // comes from the analytic pass, and the fade rides the opacity switch.
    material.radius = TOOLTIP_HEIGHT * 0.5;
    frame.place(
        "tessera-dock-app-name",
        &chrome_place(rect, material),
        |frame| {
            frame.row_ex(
                &LayoutOpts {
                    height: TOOLTIP_HEIGHT,
                    cross: Align::Center,
                    ..Default::default()
                },
                |frame| frame.label_compact_sized(&label, design.typography.label),
            );
        },
    );
    frame.set_theme(original);
    frame.set_opacity(1.0);
}

/// Lay out every running window for one Dock application. Typical groups stay
/// in a single row; large groups wrap into a centred grid while preserving a
/// usable card width and staying inside the output margins. The panel opens
/// toward the output interior — above a bottom dock's owner tile, beside a
/// side dock's — and the dock's edge decides which axis bounds the cards.
pub(super) fn live_preview_layout(
    design: &Design,
    display: (f32, f32),
    owner: Rect,
    windows: &[tessera_model::window::WindowId],
    visibility: f32,
    position: DockPosition,
) -> LivePreviewPresentation {
    let count = windows.len().max(1);
    // Room for the panel along each axis. For a bottom dock the horizontal
    // room is the output width and the vertical room is the space above the
    // owner; for a side dock the panel sits beside the owner, so the roles
    // swap to the room toward the far edge and the full output height.
    let horizontal_room = match position {
        DockPosition::Bottom => display.0 - PREVIEW_SCREEN_MARGIN * 2.0,
        DockPosition::Left => {
            display.0 - (owner.x + owner.w) - PREVIEW_PANEL_GAP - PREVIEW_SCREEN_MARGIN * 2.0
        }
        DockPosition::Right => owner.x - PREVIEW_PANEL_GAP - PREVIEW_SCREEN_MARGIN * 2.0,
    };
    let available_w = horizontal_room.max(1.0);
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
    let vertical_room = match position {
        DockPosition::Bottom => owner.y - PREVIEW_PANEL_GAP - PREVIEW_SCREEN_MARGIN,
        DockPosition::Left | DockPosition::Right => display.1 - PREVIEW_SCREEN_MARGIN * 2.0,
    };
    let vertical_space = (vertical_room
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
    let panel_x = match position {
        DockPosition::Bottom => (owner.x + owner.w * 0.5 - panel_w * 0.5).clamp(
            PREVIEW_SCREEN_MARGIN,
            (display.0 - panel_w - PREVIEW_SCREEN_MARGIN).max(PREVIEW_SCREEN_MARGIN),
        ),
        DockPosition::Left => (owner.x + owner.w + PREVIEW_PANEL_GAP)
            .min((display.0 - panel_w - PREVIEW_SCREEN_MARGIN).max(PREVIEW_SCREEN_MARGIN)),
        DockPosition::Right => (owner.x - PREVIEW_PANEL_GAP - panel_w).max(PREVIEW_SCREEN_MARGIN),
    };
    let panel_y = match position {
        DockPosition::Bottom => (owner.y - PREVIEW_PANEL_GAP - panel_h).max(PREVIEW_SCREEN_MARGIN),
        DockPosition::Left | DockPosition::Right => (owner.y + owner.h * 0.5 - panel_h * 0.5)
            .clamp(
                PREVIEW_SCREEN_MARGIN,
                (display.1 - panel_h - PREVIEW_SCREEN_MARGIN).max(PREVIEW_SCREEN_MARGIN),
            ),
    };
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
            PreviewCard {
                window,
                corner_radius: design.radii.control,
                geometry: tessera_model::window_switcher::Card {
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
        focused: None,
        inactive_content_brightness: design.preview.inactive_content_brightness,
        visibility: visibility.clamp(0.0, 1.0),
    }
}

fn render_live_preview_chrome(
    frame: &mut Frame,
    design: &Design,
    presentation: &LivePreviewPresentation,
    windows: &[Window],
    hovered: Option<tessera_model::window::WindowId>,
) {
    let panel = to_lens_rect(presentation.panel);
    let material = preview::panel_material(design);
    // The whole popover fades through the lens opacity switch; per-card
    // content dimming multiplies on top of it below.
    frame.set_opacity(presentation.visibility);
    frame.place(
        "tessera-dock-live-previews",
        &chrome_place(panel, material),
        |frame| {
            frame.column_ex(&sized(panel.w, panel.h), |_| {});
        },
    );

    let original = frame.theme();
    frame.set_theme(original.with_fg(design.colors.menu_text));
    for (index, card) in presentation.cards.iter().enumerate() {
        let Some(window) = windows.iter().find(|window| window.id == card.window) else {
            continue;
        };
        let outer = to_lens_rect(card.geometry.outer);
        let is_hovered = hovered == Some(window.id) && !window.read_only;
        let content_brightness = preview::content_brightness(
            presentation.focused,
            window.id,
            presentation.inactive_content_brightness,
        );
        frame.set_opacity(presentation.visibility * content_brightness);
        frame.place(
            &format!("tessera-dock-live-preview-card-{index}"),
            &chrome_place(
                outer,
                preview::card_material(
                    design,
                    if is_hovered {
                        preview::PreviewCardState::Selected
                    } else {
                        preview::PreviewCardState::Rest
                    },
                    card.corner_radius,
                ),
            ),
            |frame| frame.column_ex(&sized(outer.w, outer.h), |_| {}),
        );

        let label_rect = to_lens_rect(card.geometry.label);
        let title = window
            .title
            .as_deref()
            .or(window.app_id.as_deref())
            .unwrap_or("Untitled");
        let label = ellipsize(
            frame,
            title,
            design.typography.label,
            (label_rect.w - 16.0).max(0.0),
        );
        frame.place(
            &format!("tessera-dock-live-preview-label-{index}"),
            &chrome_place(
                label_rect,
                LayoutOpts {
                    bg: Color::TRANSPARENT,
                    radius: design.radii.control,
                    pad: 0.0,
                    cross: Align::Center,
                    ..surface_layout()
                },
            ),
            |frame| {
                frame.row_ex(
                    &LayoutOpts {
                        width: label_rect.w,
                        height: label_rect.h,
                        pad: 8.0,
                        cross: Align::Center,
                        ..Default::default()
                    },
                    |frame| frame.label_compact_sized(&label, design.typography.label),
                );
            },
        );
    }
    frame.set_theme(original);
    frame.set_opacity(1.0);
}

pub(super) fn live_preview_hit(
    presentation: &LivePreviewPresentation,
    x: f32,
    y: f32,
) -> Option<tessera_model::window::WindowId> {
    preview::hit_test(&presentation.cards, None, x, y)
}

fn core_rect(rect: Rect) -> tessera_model::Rect {
    tessera_model::Rect::new(
        rect.x.round() as i32,
        rect.y.round() as i32,
        rect.w.round().max(1.0) as i32,
        rect.h.round().max(1.0) as i32,
    )
}

fn to_lens_rect(rect: tessera_model::Rect) -> Rect {
    Rect {
        x: rect.origin.x as f32,
        y: rect.origin.y as f32,
        w: rect.size.w as f32,
        h: rect.size.h as f32,
    }
}
