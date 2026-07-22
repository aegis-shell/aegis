use super::*;

pub(super) enum PresentationOutcome {
    Presented,
    Retry,
}

impl CompositorRuntime {
    pub(super) fn render_and_present(
        &mut self,
        state: FrameState,
    ) -> Result<PresentationOutcome, Box<dyn std::error::Error>> {
        let FrameState {
            input,
            session_locked,
            cursor_hidden,
            cursor_shape,
            mut pending_screenshots,
        } = state;
        match self.surface.begin_frame() {
            Ok(mut frame) => {
                self.renderer.begin_frame();
                // Render scale and logical extent come from the server's
                // output geometry (backend + `[[output]]` overrides), not
                // the host, so a configured scale actually changes the
                // desktop. Nested outputs report the host scale, so the
                // nested path is unchanged.
                let (scale, logical_size) = {
                    let geometry = self.server.output_infos().first().map(|o| o.geometry);
                    let scale = geometry
                        .map(|g| g.scale.as_f32())
                        .filter(|s| *s > 0.0)
                        .unwrap_or_else(|| self.host.scale());
                    let logical = geometry
                        .map(|g| g.logical_size())
                        .map(|s| (s.w.max(1) as u32, s.h.max(1) as u32))
                        .unwrap_or_else(|| self.host.size_u32());
                    (scale, logical)
                };
                let render_geometry = RenderGeometry {
                    logical_size,
                    scale,
                };
                let physical_size = self.surface.size();
                // Bind at most one pending request to this presentation
                // frame. The readback copy is recorded after every scene and
                // cursor draw, so it captures exactly the pixels submitted
                // below rather than a later re-render of mutable state.
                let mut frame_capture: Option<(Option<ass_core::Rect>, CaptureTarget)> = None;
                for req in self.capture_rx.try_iter() {
                    if session_locked || !self.host.is_active() {
                        let _ = req
                            .reply
                            .send(Err("session is locked or inactive".to_owned()));
                    } else if !self.capture_worker.reserve() {
                        let _ = req
                            .reply
                            .send(Err("another capture is still being processed".to_owned()));
                    } else {
                        frame_capture =
                            Some((req.region, CaptureTarget::Reply { reply: req.reply }));
                    }
                }
                for (cmd, ts, origin) in pending_screenshots.drain(..) {
                    let ass_ipc::Command::Screenshot { path, region } = &cmd else {
                        continue;
                    };
                    if session_locked || !self.host.is_active() {
                        journal_effect_and_broadcast(
                            &self.journal,
                            &self.ipc,
                            ts,
                            origin,
                            cmd,
                            ass_ipc::Effect::Refused {
                                reason: "session is locked or inactive".into(),
                            },
                        );
                    } else if !self.capture_worker.reserve() {
                        journal_effect_and_broadcast(
                            &self.journal,
                            &self.ipc,
                            ts,
                            origin,
                            cmd,
                            ass_ipc::Effect::Refused {
                                reason: "another capture is still being processed".into(),
                            },
                        );
                    } else {
                        frame_capture = Some((
                            *region,
                            CaptureTarget::Screenshot {
                                path: path.clone(),
                                command: cmd,
                                ts_mono_ms: ts,
                                origin,
                            },
                        ));
                    }
                }
                let blur_sigma = self.shell.backdrop_blur_sigma();
                let backdrop_regions = self.shell.backdrop_regions(self.input_acc.display_size);
                let model_active = self
                    .wallpaper
                    .as_ref()
                    .is_some_and(ass_wallpaper::Wallpaper::has_model);
                // Overview mode (M9) swaps the whole client scene for the
                // thumbnail grid and skips the launcher-blur capture path.
                let overview_active = self.shell.overview_active();
                let backdrop_plan = if overview_active {
                    BackdropPlan::Direct
                } else {
                    self.launcher_backdrop.prepare(
                        blur_sigma > 0.0 && !backdrop_regions.is_empty(),
                        &self.device,
                        &self.surface,
                        &frame,
                        physical_size,
                    )
                };

                match backdrop_plan {
                    BackdropPlan::Capture
                        if self.launcher_backdrop.begin_capture(
                            &self.canvas,
                            &frame,
                            self.clear,
                        ) =>
                    {
                        let capture_size = self
                            .launcher_backdrop
                            .capture_size(&frame)
                            .unwrap_or(physical_size);
                        let capture_ratio = capture_size.0 as f32 / physical_size.0.max(1) as f32;
                        let capture_scale = scale * capture_ratio;

                        draw_wallpaper_background(
                            &self.canvas,
                            &self.device,
                            &mut self.wallpaper,
                            logical_size,
                            capture_scale,
                        );

                        if model_active {
                            self.canvas.end_target();
                            if let Some(target) = self.launcher_backdrop.target(&frame) {
                                if let Some(wallpaper) = self.wallpaper.as_mut() {
                                    wallpaper.draw_model_to(&self.device, &mut frame, target);
                                }
                                self.canvas.begin_target(&frame, target, None)?;
                            }
                        }

                        draw_client_scene(
                            &self.canvas,
                            &self.device,
                            &mut self.renderer,
                            &self.server,
                            capture_scale,
                        );
                        let blurred = self.launcher_backdrop.end_capture_and_blur(
                            &self.canvas,
                            &frame,
                            blur_sigma * capture_scale,
                        );
                        self.canvas.begin(&frame, Some(self.clear))?;
                        // Preserve the live desktop everywhere, then replace
                        // only the component-declared glass regions with the
                        // shared blurred capture. This is a true backdrop
                        // effect rather than a full-screen blur hidden under
                        // an opaque top-bar colour.
                        draw_direct_desktop_scene(
                            &self.canvas,
                            &self.device,
                            &mut frame,
                            &mut self.wallpaper,
                            &mut self.renderer,
                            &self.server,
                            render_geometry,
                            overview_active,
                        )?;
                        if let Some(image) = blurred {
                            for region in &backdrop_regions {
                                let x = region.x.max(0.0) * scale;
                                let y = region.y.max(0.0) * scale;
                                let w = region
                                    .w
                                    .max(0.0)
                                    .min(logical_size.0 as f32 - region.x.max(0.0))
                                    * scale;
                                let h = region
                                    .h
                                    .max(0.0)
                                    .min(logical_size.1 as f32 - region.y.max(0.0))
                                    * scale;
                                if w <= 0.0 || h <= 0.0 {
                                    continue;
                                }
                                self.canvas.save();
                                self.canvas.clip_rect(x, y, w, h);
                                image.draw(
                                    &self.canvas,
                                    0.0,
                                    0.0,
                                    physical_size.0 as f32,
                                    physical_size.1 as f32,
                                );
                                self.canvas.restore();
                            }
                        }
                    }
                    BackdropPlan::Capture | BackdropPlan::Direct => {
                        self.canvas.begin(&frame, Some(self.clear))?;
                        draw_direct_desktop_scene(
                            &self.canvas,
                            &self.device,
                            &mut frame,
                            &mut self.wallpaper,
                            &mut self.renderer,
                            &self.server,
                            render_geometry,
                            overview_active,
                        )?;
                    }
                }
                // Hand the shell a snapshot of live toplevels so the chrome's
                // window list reflects the current set. The shell reads
                // title/app_id/activated off each Window to draw its buttons.
                // The same snapshot is mirrored to the IPC (ADR-0027) so the
                // chrome and external tools read identical state, and a
                // change broadcasts `WindowsChanged` to subscribers.
                let win_snapshot = self.server.windows();
                let sig: Vec<(ass_core::window::WindowId, bool, Option<String>)> = win_snapshot
                    .iter()
                    .map(|w| (w.id, w.state.activated, w.title.clone()))
                    .collect();
                if self.last_win_sig.as_ref() != Some(&sig) {
                    self.last_win_sig = Some(sig);
                    if let Some(s) = self.ipc.as_ref() {
                        s.broadcast(ass_ipc::Event::WindowsChanged);
                    }
                }
                self.live.set_windows(win_snapshot.clone());
                self.shell.set_windows(win_snapshot);
                // Mirror the workspace snapshot and broadcast `WorkspaceChanged`
                // on any model mutation (switch, place, remove, reap).
                let ws_snap = self.server.workspace_snapshot();
                let ws_changed = self.last_ws_snap.as_ref() != Some(&ws_snap);
                self.live.set_workspaces(ws_snap.clone());
                self.shell.set_workspaces(ws_snap.clone());
                let output_snapshot = self.server.output_infos();
                self.live.set_outputs(output_snapshot.clone());
                let display_changed = self.system_status.display.configurable
                    != (self.host.name() == "drm")
                    || self.system_status.display.outputs != output_snapshot;
                if display_changed {
                    self.system_status.display.configurable = self.host.name() == "drm";
                    self.system_status.display.outputs = output_snapshot;
                    self.shell.set_system_status(self.system_status.clone());
                }
                let realm_snapshot = self.server.realm_snapshot();
                self.live.set_realms(realm_snapshot.clone());
                self.shell.set_realms(realm_snapshot.clone());
                if self.last_realm_revision != Some(realm_snapshot.revision) {
                    self.last_realm_revision = Some(realm_snapshot.revision);
                    if let Some(s) = self.ipc.as_ref() {
                        s.broadcast(ass_ipc::Event::RealmsChanged {
                            revision: realm_snapshot.revision,
                        });
                    }
                }
                if ws_changed {
                    self.last_ws_snap = Some(ws_snap);
                    if let Some(s) = self.ipc.as_ref() {
                        s.broadcast(ass_ipc::Event::WorkspaceChanged);
                    }
                }
                let do_not_disturb = self.notif_queue.lock().unwrap().do_not_disturb();
                let tiled = self.server.tiling();
                if self.system_status.do_not_disturb != do_not_disturb
                    || self.system_status.tiled != tiled
                {
                    self.system_status.do_not_disturb = do_not_disturb;
                    self.system_status.tiled = tiled;
                    self.shell.set_system_status(self.system_status.clone());
                }
                // Report the output scale so lens rasterises chrome crisply on
                // a HiDPI host; layout and input stay in logical pixels.
                self.shell.set_scale(scale);
                unsafe { self.shell.render(self.canvas.as_raw() as *mut _, &input)? };
                // Confirmation resets the selector before it draws, so this
                // same presentation frame contains the desktop without the
                // selection overlay. Bind that exact frame to the request.
                if let Some(region) = self.shell.take_screenshot_region() {
                    let path = screenshot_path(&self.screenshot_dir);
                    let ts = self.start.elapsed().as_millis() as u64;
                    let command = ass_ipc::Command::Screenshot {
                        path: path.clone(),
                        region: Some(region),
                    };
                    if self.capture_worker.reserve() {
                        frame_capture = Some((
                            Some(region),
                            CaptureTarget::Screenshot {
                                path,
                                command,
                                ts_mono_ms: ts,
                                origin: ass_ipc::Origin::Chrome,
                            },
                        ));
                    } else {
                        journal_effect_and_broadcast(
                            &self.journal,
                            &self.ipc,
                            ts,
                            ass_ipc::Origin::Chrome,
                            command,
                            ass_ipc::Effect::Refused {
                                reason: "another capture is still being processed".into(),
                            },
                        );
                    }
                }
                if session_locked {
                    draw_lock_scene(
                        &self.canvas,
                        &self.device,
                        &mut self.renderer,
                        &self.server,
                        physical_size,
                        scale,
                    );
                }
                // Drain chrome interactions and forward through the apply
                // chokepoint (ADR-0033) so the journal records them.
                let ts = self.start.elapsed().as_millis() as u64;
                let origin = ass_ipc::Origin::Chrome;
                if let Some(id) = self.shell.take_clicked_window() {
                    apply_chrome_window_command(
                        &mut self.server,
                        &self.notif_queue,
                        &mut self.quit_requested,
                        ass_ipc::Command::Focus { id },
                        &self.ipc,
                        &self.journal,
                        ts,
                    );
                }
                // Overview intents (M9): a thumbnail pick focuses its window;
                // a rail tile switches workspace while the overview stays open.
                if let Some(id) = self.shell.take_overview_pick() {
                    apply_chrome_window_command(
                        &mut self.server,
                        &self.notif_queue,
                        &mut self.quit_requested,
                        ass_ipc::Command::Focus { id },
                        &self.ipc,
                        &self.journal,
                        ts,
                    );
                }
                if let Some(id) = self.shell.take_overview_switch() {
                    let cmd = ass_ipc::Command::SwitchWorkspaceTo { id };
                    apply_command_and_journal(
                        &mut self.server,
                        &self.notif_queue,
                        &mut self.quit_requested,
                        cmd,
                        &self.ipc,
                        &self.journal,
                        ts,
                        origin,
                    );
                }
                if let Some(id) = self.shell.take_closed_window() {
                    apply_chrome_window_command(
                        &mut self.server,
                        &self.notif_queue,
                        &mut self.quit_requested,
                        ass_ipc::Command::Close { id },
                        &self.ipc,
                        &self.journal,
                        ts,
                    );
                }
                if let Some(id) = self.shell.take_move_requested() {
                    apply_chrome_window_command(
                        &mut self.server,
                        &self.notif_queue,
                        &mut self.quit_requested,
                        ass_ipc::Command::Move { id },
                        &self.ipc,
                        &self.journal,
                        ts,
                    );
                }
                for action in self.shell.take_window_actions() {
                    let cmd = match action {
                        ass_shell::WindowAction::Focus(id) => ass_ipc::Command::Focus { id },
                        ass_shell::WindowAction::Minimize(id) => ass_ipc::Command::Minimize { id },
                        ass_shell::WindowAction::Close(id) => ass_ipc::Command::Close { id },
                    };
                    apply_chrome_window_command(
                        &mut self.server,
                        &self.notif_queue,
                        &mut self.quit_requested,
                        cmd,
                        &self.ipc,
                        &self.journal,
                        ts,
                    );
                }
                if let Some(id) = self.shell.take_switch_workspace() {
                    let cmd = ass_ipc::Command::SwitchWorkspaceTo { id };
                    apply_command_and_journal(
                        &mut self.server,
                        &self.notif_queue,
                        &mut self.quit_requested,
                        cmd,
                        &self.ipc,
                        &self.journal,
                        ts,
                        origin,
                    );
                }
                if let Some(id) = self.shell.take_dismissed_notification() {
                    let cmd = ass_ipc::Command::DismissNotification { id };
                    apply_command_and_journal(
                        &mut self.server,
                        &self.notif_queue,
                        &mut self.quit_requested,
                        cmd,
                        &self.ipc,
                        &self.journal,
                        ts,
                        origin,
                    );
                }
                if let Some(app) = self.shell.take_open_builtin() {
                    self.shell.open_builtin(app);
                }
                for intent in self.shell.take_realm_intents() {
                    let action = realm_intent_to_action(intent);
                    let before_revision = self.server.realm_snapshot().revision;
                    let result = apply_realm_action(&mut self.server, action.clone());
                    match &result {
                        Ok(_) => {
                            for realm in realms_explicitly_stopped(&action) {
                                self.automatically_paused_realms.remove(&realm);
                            }
                            let invalidated = realm_action_invalidates_capture(&action);
                            if !invalidated.is_empty() {
                                self.capture_worker.invalidate_security_context();
                                if self.pending_realm_capture.as_ref().is_some_and(|pending| {
                                    invalidated.contains(&pending.context.realm)
                                }) {
                                    let pending = self
                                        .pending_realm_capture
                                        .take()
                                        .expect("capture invalidation predicate checked");
                                    refuse_capture_target(
                                        &self.capture_worker,
                                        CaptureTarget::RealmReply {
                                            context: pending.context,
                                            reply: pending.reply,
                                        },
                                        "Realm authority changed before capture completed".into(),
                                        &self.journal,
                                        &self.ipc,
                                    );
                                }
                            }
                            if let ass_ipc::RealmAction::Revoke { realm, .. } = &action {
                                self.realm_render_targets.remove(realm);
                            }
                            self.realm_processes.apply_committed_action(&action);
                            let snapshot = self.server.realm_snapshot();
                            self.last_realm_revision = Some(snapshot.revision);
                            self.live.set_realms(snapshot.clone());
                            self.shell.set_realms(snapshot.clone());
                            if let Some(ipc) = &self.ipc {
                                ipc.broadcast(ass_ipc::Event::RealmsChanged {
                                    revision: snapshot.revision,
                                });
                            }
                        }
                        Err(error) => {
                            log::warn!("Realm action from shell refused: {error}");
                            let notification = self.notif_queue.lock().unwrap().push(
                                "AI Workspace",
                                error.clone(),
                                Some("ass-control-center".into()),
                                ts,
                            );
                            if let Some(ipc) = &self.ipc {
                                ipc.broadcast(ass_ipc::Event::Notified { notification });
                            }
                        }
                    }
                    let after_revision = self.server.realm_snapshot().revision;
                    let effect = match result {
                        Ok(_) => ass_ipc::Effect::Applied,
                        Err(reason) => ass_ipc::Effect::Refused { reason },
                    };
                    journal_mutation_effect_and_broadcast(
                        &self.journal,
                        &self.ipc,
                        ts,
                        ass_ipc::Origin::Chrome,
                        ass_ipc::JournalMutation::Realm {
                            action,
                            before_revision,
                            after_revision,
                        },
                        effect,
                    );
                }
                let system_actions = self.shell.take_system_actions();
                if !system_actions.is_empty() {
                    for action in system_actions {
                        let settings_action = match &action {
                            ass_shell::SystemAction::SetTouchpad(config) => {
                                Some(ass_ipc::SettingsAction::SetTouchpad { config: *config })
                            }
                            ass_shell::SystemAction::SetDisplay(settings) => {
                                Some(ass_ipc::SettingsAction::SetDisplay {
                                    settings: settings.clone(),
                                })
                            }
                            _ => None,
                        };
                        if let Some(settings_action) = settings_action {
                            let before_revision = self.settings_revision;
                            let result = commit_settings_parts(
                                None,
                                settings_action.clone(),
                                &mut self.settings_revision,
                                self.config_path.as_deref(),
                                &mut self.config,
                                &mut self.keymap,
                                &mut self.server,
                                &mut self.shell,
                                &mut self.cursor_cache,
                                &mut self.host,
                                &mut self.reload,
                                &self.live,
                                &mut self.system_status,
                                &mut self.input_acc,
                                &self.ipc,
                            );
                            let effect = match result {
                                Ok(_) => ass_ipc::Effect::Applied,
                                Err(reason) => {
                                    log::warn!("settings: {reason}");
                                    ass_ipc::Effect::Refused { reason }
                                }
                            };
                            journal_mutation_effect_and_broadcast(
                                &self.journal,
                                &self.ipc,
                                ts,
                                origin,
                                ass_ipc::JournalMutation::Settings {
                                    action: settings_action,
                                    before_revision,
                                    after_revision: self.settings_revision,
                                },
                                effect,
                            );
                            continue;
                        }
                        if let Some(cmd) = apply_system_action(
                            &mut self.server,
                            &self.notif_queue,
                            &mut self.system_status,
                            action,
                        ) {
                            apply_command_and_journal(
                                &mut self.server,
                                &self.notif_queue,
                                &mut self.quit_requested,
                                cmd,
                                &self.ipc,
                                &self.journal,
                                ts,
                                origin,
                            );
                        }
                    }
                    self.shell.set_system_status(self.system_status.clone());
                    // Reconcile optimistic hardware state right away: this
                    // path only fires on an explicit user action (volume,
                    // brightness, radios), so a one-off detect here is cheap
                    // and gives the HUD immediate feedback.
                    let mut detected = ass_shell::SystemStatus::detect();
                    detected.do_not_disturb = self.system_status.do_not_disturb;
                    detected.tiled = self.system_status.tiled;
                    detected.touchpad = self.host.touchpad_status();
                    detected.display = self.system_status.display.clone();
                    self.system_status = detected;
                    self.shell.set_system_status(self.system_status.clone());
                }
                // The dock's Launchpad tile was clicked: toggle the launcher,
                // the same path as the Super-tap hotkey.
                if self.shell.take_toggle_launcher() {
                    self.shell.toggle();
                }
                // Dock context-menu pin/unpin requests: apply the explicit,
                // idempotent action to `[dock] pinned`, write the config back,
                // and refresh immediately rather than waiting for live reload.
                let pin_actions = self.shell.take_dock_pin_actions();
                if !pin_actions.is_empty() {
                    let mut pinned_list = self
                        .config
                        .as_ref()
                        .map(|c| c.dock.pinned.clone())
                        .unwrap_or_default();
                    pinned_list = materialize_pins_for_manual_edit(
                        &self.launcher_apps,
                        &self.icon_cache.map,
                        &pinned_list,
                        self.config.as_ref().is_some_and(|c| c.dock.autopopulate),
                    );
                    pinned_list =
                        apply_pin_actions(&self.launcher_apps, &pinned_list, &pin_actions);
                    if let Some(path) = self.config_path.as_deref() {
                        if let Err(e) = ass_config::set_dock_pinned(path, &pinned_list) {
                            log::warn!("dock: failed to persist pins: {e}");
                        }
                    } else {
                        log::warn!("dock: cannot persist pins; no config path");
                    }
                    if let Some(c) = self.config.as_mut() {
                        c.dock.pinned = pinned_list.clone();
                        c.dock.autopopulate = false;
                    }
                    let pinned = resolve_pinned(
                        &self.launcher_apps,
                        &self.icon_cache.map,
                        &pinned_list,
                        false,
                    );
                    self.shell.set_app_catalog(ass_shell::AppCatalog {
                        apps: self.launcher_apps.clone(),
                        pinned,
                        icons: ass_shell::IconSet::from_raw(self.icon_cache.map.clone()),
                    });
                }
                // Launch the application the launcher's clicked row asked for.
                // The child is detached (setsid) and inherits the Wayland/XDG
                // environment, so it connects back to this compositor and
                // survives it exiting. See ass-launch / ADR-0022.
                if let Some(entry) = self.shell.take_spawn() {
                    let opts = ass_launch::LaunchOpts {
                        wayland_display: Some(self.server.socket().to_owned()),
                        ..Default::default()
                    };
                    match ass_launch::launch(&entry, &opts) {
                        Ok(report) => {
                            log::info!("launcher: spawned {} (pid {})", entry.id, report.pid)
                        }
                        Err(e) => log::warn!("launcher: failed to spawn {}: {e}", entry.id),
                    }
                }
                // Apply keyboard-grab transitions the chrome requested this
                // frame (launcher opened or closed). Done after the intent
                // drains so a launcher "focus running app" action (which sets
                // a new keyboard focus) takes precedence over restoring the
                // pre-grab focus. The grab sends `wl_keyboard.leave` to the
                // focused client and the release sends `wl_keyboard.enter`
                // back, keeping the focused client's state consistent with the
                // capture decision. See ADR-0022.
                let captured = !session_locked && self.shell.captures_keyboard();
                if captured && !self.prev_captured {
                    self.server.grab_keyboard_focus();
                } else if !captured && self.prev_captured {
                    self.server.release_keyboard_focus();
                }
                self.prev_captured = captured;
                if self.host.uses_software_cursor() && !cursor_hidden {
                    draw_software_cursor(
                        &self.canvas,
                        &self.device,
                        &mut self.cursor_cache,
                        self.input_acc.cursor,
                        cursor_shape,
                        scale,
                    );
                }
                self.canvas.end();
                let mut capture_for_present = frame_capture.take().and_then(|(crop, target)| {
                    let readback = PendingReadback {
                        width: physical_size.0,
                        height: physical_size.1,
                        crop: crop.map(|rect| {
                            logical_rect_to_physical(
                                rect,
                                render_geometry.scale,
                                physical_size.0,
                                physical_size.1,
                            )
                        }),
                        security_generation: self.capture_worker.security_generation(),
                    };
                    match frame.request_readback() {
                        Ok(()) => Some(PendingCapture { readback, target }),
                        Err(error) => {
                            refuse_capture_target(
                                &self.capture_worker,
                                target,
                                format!(
                                    "frame readback request: {error}{}",
                                    flux_last_error_detail()
                                ),
                                &self.journal,
                                &self.ipc,
                            );
                            None
                        }
                    }
                });
                let submitted = match frame.submit() {
                    Ok(submitted) => submitted,
                    Err(error) => {
                        if let Some(capture) = capture_for_present.take() {
                            refuse_capture_target(
                                &self.capture_worker,
                                capture.target,
                                format!("captured frame submission failed: {error}"),
                                &self.journal,
                                &self.ipc,
                            );
                        }
                        return Err(error.into());
                    }
                };
                let completion_fence = match self.host.present(&self.surface, submitted) {
                    Ok(fence) => fence,
                    Err(error) => {
                        if let Some(capture) = capture_for_present.take() {
                            refuse_capture_target(
                                &self.capture_worker,
                                capture.target,
                                format!("captured frame was not presented: {error}"),
                                &self.journal,
                                &self.ipc,
                            );
                        }
                        // Transient direct-display conditions (VT switch,
                        // hotplug reconfigure, flip timeout): drop this frame
                        // and keep the session alive instead of exiting.
                        if matches!(
                            error,
                            HostError::Drm(
                                DrmError::FlipTimeout | DrmError::Inactive | DrmError::Reconfigured
                            )
                        ) {
                            log::warn!(
                                "{}: transient present failure; skipping frame: {error}",
                                self.host.name()
                            );
                            return Ok(PresentationOutcome::Retry);
                        }
                        return Err(error.into());
                    }
                };
                if let Some(capture) = capture_for_present {
                    debug_assert!(self.pending_capture.is_none());
                    self.pending_capture = Some(capture);
                }
                if self.server.lock_confirmation_pending() {
                    match self.host.wait_presented(&self.device) {
                        Ok(()) => self.server.presentation_complete(),
                        Err(error) => log::error!(
                            "session lock: secure frame was not confirmed; keeping lock request pending: {error}"
                        ),
                    }
                }
                if self.server.retired_buffers_pending() {
                    if completion_fence.is_none() && !self.host.uses_software_cursor() {
                        // Nested swapchain presentation has no exportable
                        // completion fence. Rather than stalling the whole
                        // device on a wait_idle, release the buffers a few
                        // frames late: after more presented frames than
                        // flux's in-flight slots (3), the GPU can no longer
                        // reference their contents.
                        let since = self.retired_defer.get_or_insert(self.frame_count);
                        if self.frame_count.saturating_sub(*since) >= 4 {
                            self.server.release_retired_buffers(None);
                            self.retired_defer = None;
                        }
                    } else {
                        self.server.release_retired_buffers(
                            completion_fence.as_ref().map(AsRawFd::as_raw_fd),
                        );
                    }
                }

                // Pace clients: fire frame callbacks for this presentation.
                self.server
                    .send_frame_callbacks(self.start.elapsed().as_millis() as u32);

                self.frame_count += 1;
                if self.frame_count == 1 {
                    log::info!(
                        "{}: first frame presented (with shell chrome)",
                        self.host.name()
                    );
                }
            }
            Err(error) if error.0 == flux_sys::flux_result::FLUX_ERROR_TIMEOUT => {
                for (command, ts_mono_ms, origin) in pending_screenshots.drain(..) {
                    journal_effect_and_broadcast(
                        &self.journal,
                        &self.ipc,
                        ts_mono_ms,
                        origin,
                        command,
                        ass_ipc::Effect::Refused {
                            reason: "output frame timed out before capture".to_owned(),
                        },
                    );
                }
                // The previous frame's GPU work did not retire inside the
                // frame timeout (or the presentation engine released no
                // swapchain image): transient. Skip this iteration and let
                // the next wakeup retry instead of rebuilding the
                // swapchain against a busy device.
                return Ok(PresentationOutcome::Retry);
            }
            Err(_) => {
                for (command, ts_mono_ms, origin) in pending_screenshots.drain(..) {
                    journal_effect_and_broadcast(
                        &self.journal,
                        &self.ipc,
                        ts_mono_ms,
                        origin,
                        command,
                        ass_ipc::Effect::Refused {
                            reason: "output changed before capture".to_owned(),
                        },
                    );
                }
                // Out-of-date / lost: rebuild the swapchain at the current
                // physical size.
                if let Some(capture) = self.pending_capture.take() {
                    refuse_capture_target(
                        &self.capture_worker,
                        capture.target,
                        "output changed before the captured frame became readable".to_owned(),
                        &self.journal,
                        &self.ipc,
                    );
                }
                let (nw, nh) = self.host.physical_size();
                self.surface.resize(nw, nh)?;
                if let Err(error) = self.surface.prepare_readback() {
                    log::warn!(
                        "capture: could not preallocate resized readback staging: {error}{}",
                        flux_last_error_detail()
                    );
                }
            }
        }

        Ok(PresentationOutcome::Presented)
    }
}
