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
            had_input,
            mut pending_screenshots,
        } = state;
        // Render scale and logical extent come from the server's output
        // geometry (backend + `[[output]]` overrides), not the host, so a
        // configured scale actually changes the desktop. Nested outputs
        // report the host scale, so the nested path is unchanged.
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
        let physical_size = self.surface.size();
        let cursor_position = (
            (self.input_acc.cursor.0 * scale).round() as i32,
            (self.input_acc.cursor.1 * scale).round() as i32,
        );
        // Upload and place compositor-owned theme cursors on dedicated KMS
        // planes before deciding whether a fullscreen client can scan out
        // directly. The cheap Arc clone ends the CursorCache borrow before
        // mutating the host; cursor buffers themselves are cached by exact
        // pixels in the DRM backend.
        if self.host.supports_hardware_cursor() {
            if cursor_hidden {
                self.host.set_hardware_cursor(None);
            } else {
                let loaded = self
                    .cursor_cache
                    .get(&self.device, cursor_shape, scale)
                    .map(|cursor| {
                        (
                            std::sync::Arc::clone(&cursor.pixels),
                            (cursor.width as u32, cursor.height as u32),
                            (cursor.xhot as u32, cursor.yhot as u32),
                        )
                    });
                if let Some((pixels, size, hotspot)) = loaded {
                    self.host.set_hardware_cursor(Some(HardwareCursor {
                        pixels: &pixels,
                        size,
                        hotspot,
                        position: cursor_position,
                    }));
                } else {
                    log::warn!("cursor: theme rasterization failed; disabling hardware cursor");
                    self.host.disable_hardware_cursor();
                }
            }
        }
        // Bind capture requests before the render/skip decision: a bound
        // capture always forces a presentation frame.
        let mut frame_capture =
            self.prepare_frame_capture(session_locked, &mut pending_screenshots);
        let screenshot_include_cursor = self
            .config
            .as_ref()
            .is_none_or(|config| config.screenshot.include_cursor);
        let bound_saved_screenshot = frame_capture
            .as_ref()
            .is_some_and(|(_, target)| matches!(target, CaptureTarget::Screenshot { .. }));
        // Print-key screenshots keep the freeze armed through confirmation.
        // Portal pick sessions also use the freeze, but retain their own
        // cursor-free capture contract and are not governed by this setting.
        let human_screenshot_session = self.screenshot_freeze.armed && self.pending_pick.is_none();
        let frozen_screenshot_cursor = human_screenshot_session
            .then(|| self.screenshot_freeze.cursor().cloned())
            .flatten();
        let cursorless_saved_frame = !screenshot_include_cursor && bound_saved_screenshot;
        let mut capturing_frozen_screenshot = false;
        let mut damage = self.assess_frame_damage(DamageAssessment {
            had_input,
            session_locked,
            cursor_hidden,
            cursor_shape,
            software_cursor: self.host.uses_software_cursor(),
            scale,
            physical_size,
        });
        let cursor_plane_changed = self.host.supports_hardware_cursor()
            && (self.last_presented_cursor != Some((cursor_shape, cursor_hidden))
                || (!cursor_hidden
                    && self.last_presented_cursor_position != Some(cursor_position)));
        let cursor_only_eligible = matches!(damage, FrameDamage::None)
            && cursor_plane_changed
            && frame_capture.is_none()
            && self.pending_capture.is_none()
            && self.pending_realm_capture.is_none()
            && !self.capture_worker.is_busy()
            && !self.screenshot_freeze.armed
            && !self.server.lock_confirmation_pending()
            && !self.server.retired_buffers_pending();
        if cursor_only_eligible {
            match self.host.present_cursor() {
                Ok(()) => {
                    self.server
                        .send_frame_callbacks(self.start.elapsed().as_millis() as u32);
                    self.last_present_minute = Some(wall_clock_minute());
                    self.last_presented_cursor = Some((cursor_shape, cursor_hidden));
                    self.last_presented_cursor_position = Some(cursor_position);
                    self.frame_count += 1;
                    return Ok(PresentationOutcome::Presented);
                }
                Err(HostError::Drm(DrmError::CursorFallback)) => {
                    // The backend disabled the cursor plane. Repaint this
                    // frame with the software cursor rather than showing one
                    // cursorless refresh.
                    damage = FrameDamage::Full;
                }
                Err(HostError::Drm(
                    DrmError::FlipTimeout | DrmError::Inactive | DrmError::Reconfigured,
                )) => {
                    self.force_full_redraw = true;
                    return Ok(PresentationOutcome::Retry);
                }
                Err(HostError::Drm(DrmError::ScanoutUnsupported)) => {
                    damage = FrameDamage::Full;
                }
                Err(error) => return Err(error.into()),
            }
        }
        let present_damage = match damage {
            FrameDamage::Area(rect) => Some(rect),
            _ => None,
        };
        if matches!(damage, FrameDamage::None)
            && frame_capture.is_none()
            && self.pending_capture.is_none()
            && self.pending_realm_capture.is_none()
            && !self.capture_worker.is_busy()
            && !self.screenshot_freeze.armed
            && !self.server.lock_confirmation_pending()
            && !self.server.retired_buffers_pending()
            && !self.server.frame_callbacks_pending()
        {
            // Nothing visible changed: skip the render and the atomic commit
            // — the scanout contents are already correct, so presenting
            // would be pure waste. Pending frame callbacks deliberately
            // disable the skip: completing them immediately on every
            // Wayland-fd wake would let a frame-only client spin without
            // output/vblank pacing.
            return Ok(PresentationOutcome::Presented);
        }
        // Direct-scanout fast path: a single fullscreen, opaque dmabuf client
        // covering the whole output (and nothing else needing compositing) can
        // be page-flipped directly onto the primary plane, skipping the Vulkan
        // composite. This is the fullscreen-game zero-GPU-cost path. A miss or
        // a kernel rejection falls through to the normal composite below.
        //
        // While scanout is active the renderer never composites, so the damage
        // tracker's per-surface generation baselines go stale: a client that
        // committed frames entirely through the scanout path looks "unchanged"
        // to it. Leaving scanout must therefore force a full redraw the next
        // composite, or the fallback frame could skip rendering and show a
        // frozen image.
        let was_scanout = self.scanout_taken;
        let scanout_candidate = frame_capture
            .is_none()
            .then(|| self.pick_scanout_candidate(physical_size, cursor_hidden))
            .flatten();
        if let Some(candidate) = scanout_candidate {
            match self.host.present_scanout(&candidate, present_damage) {
                Ok(completion_fence) => {
                    self.scanout_taken = true;
                    if !was_scanout {
                        log::info!(
                            "{}: direct scanout active for {:#x} (mod {:#x}); composite bypassed",
                            self.host.name(),
                            candidate.drm_format,
                            candidate.modifier,
                        );
                    }
                    // The buffer was scanned out directly: mark the surface
                    // damage acknowledged so the server's damage baseline
                    // stays correct, fire client frame callbacks to keep
                    // pacing, and re-anchor the cursor/minute baselines the
                    // skip-render path consults. Captures and screenshots are
                    // disqualified by pick_scanout_candidate, so none run here.
                    self.server.acknowledge_presented_surface_damage();
                    if self.server.retired_buffers_pending() {
                        self.server.release_retired_buffers(
                            completion_fence.as_ref().map(AsRawFd::as_raw_fd),
                        );
                    }
                    self.server
                        .send_frame_callbacks(self.start.elapsed().as_millis() as u32);
                    self.last_present_minute = Some(wall_clock_minute());
                    self.last_presented_cursor = Some((cursor_shape, cursor_hidden));
                    self.last_presented_cursor_position = Some(cursor_position);
                    self.force_full_redraw = false;
                    self.frame_count += 1;
                    return Ok(PresentationOutcome::Presented);
                }
                Err(error) => {
                    // Any rejection (unsupported format, EACCES, reconfigure,
                    // flip timeout) disables scanout for this frame and falls
                    // through to compositing. A non-transient ScanoutUnsupported
                    // just composites; transient DRM errors retry below.
                    self.scanout_taken = false;
                    if matches!(error, HostError::Drm(DrmError::CursorFallback)) {
                        // The backend disabled the KMS cursor after rejecting
                        // this commit. The same frame must include the
                        // software cursor, outside any client-only damage
                        // scissor.
                        damage = FrameDamage::Full;
                    }
                    if !matches!(error, HostError::Drm(DrmError::ScanoutUnsupported)) {
                        log::warn!(
                            "{}: direct scanout failed; compositing instead: {error}",
                            self.host.name()
                        );
                    }
                }
            }
        } else {
            self.scanout_taken = false;
        }
        // Scanout was active last frame but is no longer eligible (a window
        // appeared, the cursor moved, chrome opened…): force a full composite so
        // the resumed render is correct rather than skipped as "unchanged".
        if was_scanout {
            self.force_full_redraw = true;
            damage = FrameDamage::Full;
        }
        match self.surface.begin_frame() {
            Ok(mut frame) => {
                let frame_slot = frame.index() as usize;
                let repaint =
                    composite_repaint_for_slot(&mut self.composite_slot_damage, frame_slot, damage);
                self.renderer.begin_frame();
                let render_geometry = RenderGeometry {
                    logical_size,
                    scale,
                };
                let blur_sigma = self.shell.backdrop_blur_sigma();
                let backdrop_regions = self.shell.backdrop_regions(self.input_acc.display_size);
                let model_active = self
                    .wallpaper
                    .as_ref()
                    .is_some_and(aegis_wallpaper::Wallpaper::has_model);
                // Overview mode (M9) swaps the whole client scene for the
                // thumbnail grid and skips the launcher-blur capture path.
                let overview_active = self.shell.overview_active();
                let window_switcher_active = self.shell.window_switcher_active();
                // A screenshot freeze session replaces the whole frame with
                // the trigger-frame snapshot: the capture frame renders the
                // desktop scene *and* the chrome into an offscreen target
                // below; later frames blit that snapshot and draw only the
                // selector on top. The launcher-blur path stays off for the
                // whole session.
                let freeze_capturing = self.screenshot_freeze.needs_capture();
                // Restrict the backdrop capture to the union of the declared
                // blur regions (plus the blur footprint): the offscreen pass
                // re-renders the scene every frame, so covering only what the
                // blur can sample avoids a second full-screen scene render.
                // A live 3D wallpaper still draws its model into the whole
                // capture image, so it keeps the full-frame extent.
                let capture_bounds = (blur_sigma > 0.0
                    && !backdrop_regions.is_empty()
                    && !model_active)
                    .then(|| {
                        blur_capture_bounds(
                            &backdrop_regions,
                            logical_size,
                            physical_size,
                            scale,
                            blur_sigma,
                        )
                    });
                let (capture_origin, capture_extent) =
                    capture_bounds.unwrap_or(((0, 0), physical_size));
                let backdrop_plan =
                    if overview_active || window_switcher_active || self.screenshot_freeze.armed {
                        BackdropPlan::Direct
                    } else {
                        self.launcher_backdrop.prepare(
                            blur_sigma > 0.0 && !backdrop_regions.is_empty(),
                            &self.device,
                            &self.surface,
                            &frame,
                            capture_extent,
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
                            .unwrap_or(capture_extent);
                        let capture_ratio = capture_size.0 as f32 / capture_extent.0.max(1) as f32;
                        let capture_scale = scale * capture_ratio;

                        // The capture target only covers `capture_extent`:
                        // shift the scene so the capture origin lands at
                        // (0, 0) of the target. The origin is (0, 0) for a
                        // live 3D wallpaper, whose model pass below re-begins
                        // the target and so drops this offset.
                        self.canvas.save();
                        self.canvas.translate(
                            -(capture_origin.0 as f32) * capture_ratio,
                            -(capture_origin.1 as f32) * capture_ratio,
                        );

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
                        self.canvas.restore();
                        let blurred = self.launcher_backdrop.end_capture_and_blur(
                            &self.canvas,
                            &frame,
                            blur_sigma * capture_scale,
                        );
                        begin_opaque_frame(&self.canvas, &frame, self.clear)?;
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
                            window_switcher_active,
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
                                // The blurred image covers exactly the
                                // capture bounds; stretching it over that
                                // rect (instead of the full frame) keeps the
                                // scene-to-blur sampling identity.
                                image.draw(
                                    &self.canvas,
                                    capture_origin.0 as f32,
                                    capture_origin.1 as f32,
                                    capture_extent.0 as f32,
                                    capture_extent.1 as f32,
                                );
                                self.canvas.restore();
                            }
                        }
                    }
                    BackdropPlan::Capture | BackdropPlan::Direct => {
                        if self.screenshot_freeze.active() {
                            // Frozen: present only the trigger-frame
                            // snapshot; the selector draws on top below.
                            begin_opaque_frame(&self.canvas, &frame, self.clear)?;
                            if let Some(image) = self.screenshot_freeze.image() {
                                self.canvas.draw_image(
                                    image,
                                    0.0,
                                    0.0,
                                    physical_size.0 as f32,
                                    physical_size.1 as f32,
                                );
                            }
                        } else if freeze_capturing {
                            // Capture frame: the desktop scene starts the
                            // snapshot pass; the chrome render below joins
                            // the same pass, which then resolves into the
                            // blit of the frozen frame.
                            if !self.screenshot_freeze.ensure_target(
                                &self.device,
                                &self.surface,
                                &frame,
                                physical_size,
                            ) {
                                self.screenshot_freeze.failed = true;
                            }
                            let mut in_target = false;
                            if !self.screenshot_freeze.failed {
                                let target = self
                                    .screenshot_freeze
                                    .target(&frame)
                                    .expect("ensure_target succeeded");
                                match begin_opaque_target(&self.canvas, &frame, target, self.clear)
                                {
                                    Ok(()) => {
                                        in_target = true;
                                        draw_wallpaper_background(
                                            &self.canvas,
                                            &self.device,
                                            &mut self.wallpaper,
                                            logical_size,
                                            scale,
                                        );
                                        if model_active {
                                            self.canvas.end_target();
                                            if let Some(wallpaper) = self.wallpaper.as_mut() {
                                                wallpaper.draw_model_to(
                                                    &self.device,
                                                    &mut frame,
                                                    target,
                                                );
                                            }
                                            if let Err(error) =
                                                self.canvas.begin_target(&frame, target, None)
                                            {
                                                log::warn!(
                                                    "screenshot: freeze capture interrupted ({error}); falling back to live scene"
                                                );
                                                self.screenshot_freeze.failed = true;
                                                in_target = false;
                                            }
                                        }
                                        if in_target {
                                            draw_client_scene(
                                                &self.canvas,
                                                &self.device,
                                                &mut self.renderer,
                                                &self.server,
                                                scale,
                                            );
                                        }
                                    }
                                    Err(error) => {
                                        log::warn!(
                                            "screenshot: failed to begin freeze capture ({error}); falling back to live scene"
                                        );
                                        self.screenshot_freeze.failed = true;
                                    }
                                }
                            }
                            if !in_target {
                                begin_opaque_frame(&self.canvas, &frame, self.clear)?;
                                draw_direct_desktop_scene(
                                    &self.canvas,
                                    &self.device,
                                    &mut frame,
                                    &mut self.wallpaper,
                                    &mut self.renderer,
                                    &self.server,
                                    render_geometry,
                                    overview_active,
                                    window_switcher_active,
                                )?;
                            }
                        } else {
                            begin_opaque_frame_repaint(
                                &self.canvas,
                                &frame,
                                physical_size,
                                self.clear,
                                repaint,
                            )?;
                            draw_direct_desktop_scene(
                                &self.canvas,
                                &self.device,
                                &mut frame,
                                &mut self.wallpaper,
                                &mut self.renderer,
                                &self.server,
                                render_geometry,
                                overview_active,
                                window_switcher_active,
                            )?;
                        }
                    }
                }
                // Hand the shell a snapshot of live toplevels so the chrome's
                // window list reflects the current set. The shell reads
                // title/app_id/activated off each Window to draw its buttons.
                // The same snapshot is mirrored to the IPC (ADR-0027) so the
                // chrome and external tools read identical state, and a
                // change broadcasts `WindowsChanged` to subscribers.
                //
                // Every snapshot below is rebuilt only when its cheap
                // revision/signature moves; chrome and IPC keep the copy
                // pushed last time otherwise, so steady state pays no clone.
                let windows_hash = self.server.windows_signature();
                if self.last_windows_hash != Some(windows_hash) {
                    self.last_windows_hash = Some(windows_hash);
                    let win_snapshot = self.server.windows();
                    let sig: WindowEventSignature = win_snapshot
                        .iter()
                        .map(|w| {
                            (
                                w.id,
                                w.state.activated,
                                w.state.maximized,
                                w.state.fullscreen,
                                w.minimized,
                                w.title.clone(),
                            )
                        })
                        .collect();
                    if self.last_win_sig.as_ref() != Some(&sig) {
                        self.last_win_sig = Some(sig);
                        if let Some(s) = self.ipc.as_ref() {
                            s.broadcast(aegis_ipc::Event::WindowsChanged);
                        }
                    }
                    let space_use = aegis_core::window::SpaceUse::from_windows(&win_snapshot);
                    if self.last_space_use != Some(space_use) {
                        self.last_space_use = Some(space_use);
                        if let Some(s) = self.ipc.as_ref() {
                            s.broadcast(aegis_ipc::Event::SpaceUseChanged { state: space_use });
                        }
                    }
                    self.live.set_windows(win_snapshot.clone());
                    self.shell.set_windows(win_snapshot);
                }
                // Mirror the workspace snapshot and broadcast `WorkspaceChanged`
                // on any model mutation (switch, place, remove, reap).
                let ws_sig = self.server.workspace_signature();
                if self.last_ws_sig != Some(ws_sig) {
                    self.last_ws_sig = Some(ws_sig);
                    let ws_snap = self.server.workspace_snapshot();
                    self.live.set_workspaces(ws_snap.clone());
                    self.shell.set_workspaces(ws_snap);
                    if let Some(s) = self.ipc.as_ref() {
                        s.broadcast(aegis_ipc::Event::WorkspaceChanged);
                    }
                }
                let outputs_revision = self.server.outputs_revision();
                if self.last_outputs_revision != Some(outputs_revision) {
                    self.last_outputs_revision = Some(outputs_revision);
                    let output_snapshot = self.server.output_infos();
                    self.live.set_outputs(output_snapshot.clone());
                    let display_changed = self.system_status.display.configurable
                        != (self.host.name() == "drm")
                        || self.system_status.display.outputs != output_snapshot;
                    if display_changed {
                        self.system_status.display.configurable = self.host.name() == "drm";
                        self.system_status.display.outputs = output_snapshot;
                        publish_system_status_parts(
                            &self.system_status,
                            &mut self.shell,
                            &self.live,
                            &self.ipc,
                        );
                    }
                }
                let realm_revision = self.server.realm_revision();
                if self.last_realm_revision != Some(realm_revision) {
                    self.last_realm_revision = Some(realm_revision);
                    let realm_snapshot = self.server.realm_snapshot();
                    self.live.set_realms(realm_snapshot.clone());
                    self.shell.set_realms(realm_snapshot);
                    if let Some(s) = self.ipc.as_ref() {
                        s.broadcast(aegis_ipc::Event::RealmsChanged {
                            revision: realm_revision,
                        });
                    }
                }
                let do_not_disturb = self.notif_queue.lock().unwrap().do_not_disturb();
                let tiled = self.server.tiling();
                if self.system_status.do_not_disturb != do_not_disturb
                    || self.system_status.tiled != tiled
                {
                    self.system_status.do_not_disturb = do_not_disturb;
                    self.system_status.tiled = tiled;
                    publish_system_status_parts(
                        &self.system_status,
                        &mut self.shell,
                        &self.live,
                        &self.ipc,
                    );
                }
                // Report the output scale so lens rasterises chrome crisply on
                // a HiDPI host; layout and input stay in logical pixels.
                self.shell.set_scale(scale);
                unsafe { self.shell.render(self.canvas.as_raw() as *mut _, &input)? };
                // Protocol overlays sit above ordinary shell chrome. A modal
                // screenshot/picker owns the whole frame and suppresses live
                // client overlays without changing Wayland keyboard focus.
                if !session_locked
                    && !self.shell.screenshot_active()
                    && !self.screenshot_freeze.active()
                {
                    let include_live_cursor = if freeze_capturing {
                        human_screenshot_session && screenshot_include_cursor
                    } else {
                        !cursorless_saved_frame
                    };
                    draw_client_overlays(
                        &self.canvas,
                        &self.device,
                        &mut self.renderer,
                        &self.server,
                        scale,
                        include_live_cursor,
                    );
                }
                // Finish the freeze snapshot pass: the chrome above rendered
                // into the target as well; protocol overlays are included
                // above it. Resolve that whole trigger frame into the
                // on-screen blit, then open the selector over the frozen
                // screen. On failure the selector opens right away over the
                // live scene instead.
                if freeze_capturing && !self.screenshot_freeze.failed {
                    self.canvas.end_target();
                    let frozen_cursor = if human_screenshot_session
                        && screenshot_include_cursor
                        && !cursor_hidden
                    {
                        capture_cursor_snapshot(
                            &self.device,
                            &mut self.cursor_cache,
                            self.input_acc.cursor,
                            cursor_shape,
                            scale,
                        )
                    } else {
                        None
                    };
                    self.screenshot_freeze.mark_captured(&frame, frozen_cursor);
                    begin_opaque_frame(&self.canvas, &frame, self.clear)?;
                    if let Some(image) = self.screenshot_freeze.image() {
                        self.canvas.draw_image(
                            image,
                            0.0,
                            0.0,
                            physical_size.0 as f32,
                            physical_size.1 as f32,
                        );
                    }
                }
                if self.screenshot_freeze.pending_open
                    && (self.screenshot_freeze.captured || self.screenshot_freeze.failed)
                {
                    // A pending IPC pick opens the selector in its picker
                    // mode (ADR-0054); otherwise this is the Print key.
                    match self.pending_pick_open.take() {
                        Some(kind) => self.shell.start_pick(picker_mode(kind)),
                        None => self.shell.start_screenshot(),
                    }
                    self.shell
                        .set_screenshot_freeze(self.screenshot_freeze.captured);
                    self.screenshot_freeze.mark_opened();
                }
                // Confirmation resets the selector before it draws, so this
                // same presentation frame contains the frozen desktop
                // without the selection overlay. Bind that exact frame to
                // the request: the saved pixels are the trigger frame's.
                //
                // A picker-mode confirm (ADR-0054) answers the waiting IPC
                // request instead of the Print-key file path below.
                let screenshot_region = self.shell.take_screenshot_region();
                let pick_consumed_region = if let Some(region) = screenshot_region
                    && let Some(pick) = self.pending_pick.take()
                {
                    let _ = pick
                        .reply
                        .send(Ok(aegis_ipc::PickResult::Region { rect: region }));
                    true
                } else {
                    false
                };
                if let Some(region) = screenshot_region.filter(|_| !pick_consumed_region) {
                    capturing_frozen_screenshot = self.screenshot_freeze.active();
                    let path = screenshot_path(&self.screenshot_dir);
                    let ts = self.start.elapsed().as_millis() as u64;
                    let command = aegis_ipc::Command::Screenshot {
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
                                origin: aegis_ipc::Origin::Chrome,
                            },
                        ));
                    } else {
                        journal_effect_and_broadcast(
                            &self.journal,
                            &self.ipc,
                            ts,
                            aegis_ipc::Origin::Chrome,
                            command,
                            aegis_ipc::Effect::Refused {
                                reason: "another capture is still being processed".into(),
                            },
                        );
                    }
                }
                // The remaining picker results (ADR-0054): a pixel click
                // binds a 1x1 readback of this exact frame; window, whole-
                // output, and cancellation answer immediately.
                if let Some(point) = self.shell.take_picked_point()
                    && let Some(pick) = self.pending_pick.take()
                {
                    if pick.kind == aegis_ipc::PickKind::Pixel && self.capture_worker.reserve() {
                        frame_capture = Some((
                            Some(aegis_core::Rect::new(point.x, point.y, 1, 1)),
                            CaptureTarget::Pixel {
                                point,
                                reply: pick.reply,
                            },
                        ));
                    } else {
                        let _ = pick
                            .reply
                            .send(Err("another capture is still being processed".into()));
                    }
                }
                if let Some(id) = self.shell.take_picked_window()
                    && let Some(pick) = self.pending_pick.take()
                {
                    let result = if self.server.windows().iter().any(|w| w.id == id) {
                        Ok(aegis_ipc::PickResult::Window { id })
                    } else {
                        Err("the picked window is gone".to_owned())
                    };
                    let _ = pick.reply.send(result);
                }
                if self.shell.take_pick_output()
                    && let Some(pick) = self.pending_pick.take()
                {
                    let _ = pick.reply.send(Ok(aegis_ipc::PickResult::Output));
                }
                if self.shell.take_pick_cancelled()
                    && let Some(pick) = self.pending_pick.take()
                {
                    let _ = pick.reply.send(Ok(aegis_ipc::PickResult::Cancelled));
                }
                // Safety net: a picker overlay that closed without emitting
                // any event still answers its request (ADR-0054).
                if self.pending_pick.is_some()
                    && self.screenshot_freeze.opened
                    && !self.shell.screenshot_active()
                    && let Some(pick) = self.pending_pick.take()
                {
                    let _ = pick.reply.send(Ok(aegis_ipc::PickResult::Cancelled));
                }
                // The selector closed this frame (confirmed above or
                // cancelled). This frame still presented the frozen
                // snapshot — exactly what the bound readback captures — so
                // live rendering resumes from the next frame.
                if self
                    .screenshot_freeze
                    .should_disarm(self.shell.screenshot_active())
                {
                    self.screenshot_freeze.disarm();
                    self.shell.set_screenshot_freeze(false);
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
                let origin = aegis_ipc::Origin::Chrome;
                if let Some(id) = self.shell.take_clicked_window() {
                    apply_chrome_window_command(
                        &mut self.server,
                        &self.notif_queue,
                        &mut self.quit_requested,
                        aegis_ipc::Command::Focus { id },
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
                        aegis_ipc::Command::Focus { id },
                        &self.ipc,
                        &self.journal,
                        ts,
                    );
                }
                if let Some(id) = self.shell.take_overview_switch() {
                    let cmd = aegis_ipc::Command::SwitchWorkspaceTo { id };
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
                for action in self.shell.take_window_actions() {
                    let cmd = match action {
                        aegis_shell::WindowAction::Focus(id) => aegis_ipc::Command::Focus { id },
                        aegis_shell::WindowAction::Minimize(id) => {
                            aegis_ipc::Command::Minimize { id }
                        }
                        aegis_shell::WindowAction::SetMaximized(id, maximized) => {
                            aegis_ipc::Command::SetMaximized { id, maximized }
                        }
                        aegis_shell::WindowAction::Close(id) => aegis_ipc::Command::Close { id },
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
                    let cmd = aegis_ipc::Command::SwitchWorkspaceTo { id };
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
                    let cmd = aegis_ipc::Command::DismissNotification { id };
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
                    // The selector opens through the freeze session so the
                    // trigger frame (chrome included) is snapshotted first.
                    if app == aegis_core::app::BuiltInApplication::ScreenshotSelector {
                        self.screenshot_freeze.request_open();
                    } else {
                        self.shell.open_builtin(app);
                    }
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
                            if let aegis_ipc::RealmAction::Revoke { realm, .. } = &action {
                                self.realm_render_targets.remove(realm);
                            }
                            self.realm_processes.apply_committed_action(&action);
                            let snapshot = self.server.realm_snapshot();
                            self.last_realm_revision = Some(snapshot.revision);
                            self.live.set_realms(snapshot.clone());
                            self.shell.set_realms(snapshot.clone());
                            if let Some(ipc) = &self.ipc {
                                ipc.broadcast(aegis_ipc::Event::RealmsChanged {
                                    revision: snapshot.revision,
                                });
                            }
                        }
                        Err(error) => {
                            log::warn!("Realm action from shell refused: {error}");
                            let notification = self.notif_queue.lock().unwrap().push(
                                "AI Workspace",
                                error.clone(),
                                Some(aegis_core::app::AI_WORKSPACES_ID.into()),
                                ts,
                            );
                            if let Some(ipc) = &self.ipc {
                                ipc.broadcast(aegis_ipc::Event::Notified { notification });
                            }
                        }
                    }
                    let after_revision = self.server.realm_revision();
                    let effect = match result {
                        Ok(_) => aegis_ipc::Effect::Applied,
                        Err(reason) => aegis_ipc::Effect::Refused { reason },
                    };
                    journal_mutation_effect_and_broadcast(
                        &self.journal,
                        &self.ipc,
                        ts,
                        aegis_ipc::Origin::Chrome,
                        aegis_ipc::JournalMutation::Realm {
                            action,
                            before_revision,
                            after_revision,
                        },
                        effect,
                    );
                }
                let system_actions = self.shell.take_system_actions();
                if !system_actions.is_empty() {
                    let mut applied = false;
                    for action in system_actions {
                        let command = aegis_ipc::Command::System {
                            action: action.clone(),
                        };
                        let effect = match apply_system_action(
                            &mut self.server,
                            &self.notif_queue,
                            &mut self.system_status,
                            action,
                        ) {
                            Ok(()) => {
                                applied = true;
                                aegis_ipc::Effect::Applied
                            }
                            Err(reason) => {
                                log::warn!("system control: {reason}");
                                aegis_ipc::Effect::Refused { reason }
                            }
                        };
                        journal_effect_and_broadcast(
                            &self.journal,
                            &self.ipc,
                            ts,
                            origin,
                            command,
                            effect,
                        );
                    }
                    if applied {
                        publish_system_status_parts(
                            &self.system_status,
                            &mut self.shell,
                            &self.live,
                            &self.ipc,
                        );
                        // Reconcile the optimistic hardware state out of
                        // cycle: `apply_system_action` already updated the HUD
                        // with optimistic values, and the poller thread
                        // re-probes the host right away so the main loop never
                        // blocks on a wpctl/nmcli subprocess.
                        let _ = self.status_refresh_tx.send(());
                    }
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
                    // Persist off the frame loop: the single worker applies
                    // the full list in send order, so rapid pin/unpin clicks
                    // cannot overwrite each other out of order.
                    if let Err(error) =
                        self.config_writer
                            .enqueue(aegis_config::ConfigEdit::SetDockPinned {
                                pinned: pinned_list.clone(),
                            })
                    {
                        log::warn!("dock: pins not saved: {error}");
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
                    self.shell.set_app_catalog(aegis_shell::AppCatalog {
                        apps: self.launcher_apps.clone(),
                        pinned,
                        icons: aegis_shell::IconSet::from_raw(self.icon_cache.map.clone()),
                    });
                }
                // Launch the application the launcher's clicked row asked for.
                // The child is detached (setsid) and inherits the Wayland/XDG
                // environment, so it connects back to this compositor and
                // survives it exiting. See aegis-launch / ADR-0022.
                if let Some(entry) = self.shell.take_spawn() {
                    let opts = aegis_launcher::LaunchOpts {
                        wayland_display: Some(self.server.socket().to_owned()),
                        ..Default::default()
                    };
                    match aegis_launcher::launch(&entry, &opts) {
                        Ok(report) => {
                            log::info!("launcher: spawned {} (pid {})", entry.id, report.pid)
                        }
                        Err(e) => log::warn!("launcher: failed to spawn {}: {e}", entry.id),
                    }
                }
                if self.host.uses_software_cursor()
                    && !cursor_hidden
                    && !cursorless_saved_frame
                    && !capturing_frozen_screenshot
                {
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
                    let cursor = if screenshot_include_cursor
                        && matches!(&target, CaptureTarget::Screenshot { .. })
                    {
                        if capturing_frozen_screenshot {
                            frozen_screenshot_cursor.clone()
                        } else if !self.host.uses_software_cursor() && !cursor_hidden {
                            capture_cursor_snapshot(
                                &self.device,
                                &mut self.cursor_cache,
                                self.input_acc.cursor,
                                cursor_shape,
                                scale,
                            )
                        } else {
                            None
                        }
                    } else {
                        None
                    };
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
                        cursor,
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
                let completion_fence =
                    match self.host.present(&self.surface, submitted, present_damage) {
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
                                    DrmError::FlipTimeout
                                        | DrmError::Inactive
                                        | DrmError::Reconfigured
                                        | DrmError::CursorFallback
                                )
                            ) {
                                log::warn!(
                                    "{}: transient present failure; skipping frame: {error}",
                                    self.host.name()
                                );
                                // Damage/revision baselines were assessed
                                // before this frame was rendered. They must
                                // not let the retry skip content that never
                                // reached scanout.
                                self.force_full_redraw = true;
                                return Ok(PresentationOutcome::Retry);
                            }
                            return Err(error.into());
                        }
                    };
                record_composite_present(&mut self.composite_slot_damage, frame_slot, damage);
                if let Some(capture) = capture_for_present {
                    debug_assert!(self.pending_capture.is_none());
                    self.pending_capture = Some(capture);
                }
                self.server.acknowledge_presented_surface_damage();
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

                // The frame landed: re-anchor the present-side damage
                // baselines (clock minute, cursor, post-resize full redraw).
                self.last_present_minute = Some(wall_clock_minute());
                self.last_presented_cursor = Some((cursor_shape, cursor_hidden));
                self.last_presented_cursor_position = Some(cursor_position);
                self.force_full_redraw = false;

                self.frame_count += 1;
                if self.frame_count == 1 {
                    log::info!(
                        "{}: first frame presented (with shell chrome)",
                        self.host.name()
                    );
                }
            }
            Err(error) if error.0 == flux_sys::flux_result::FLUX_ERROR_TIMEOUT => {
                // A capture bound by the pre-pass never got its frame; refuse
                // it so the worker lane is released for the next iteration.
                if let Some((_, target)) = frame_capture.take() {
                    refuse_capture_target(
                        &self.capture_worker,
                        target,
                        "output frame timed out before capture".to_owned(),
                        &self.journal,
                        &self.ipc,
                    );
                }
                // The previous frame's GPU work did not retire inside the
                // frame timeout (or the presentation engine released no
                // swapchain image): transient. Skip this iteration and let
                // the next wakeup retry instead of rebuilding the
                // swapchain against a busy device.
                self.force_full_redraw = true;
                return Ok(PresentationOutcome::Retry);
            }
            Err(_) => {
                if let Some((_, target)) = frame_capture.take() {
                    refuse_capture_target(
                        &self.capture_worker,
                        target,
                        "output changed before capture".to_owned(),
                        &self.journal,
                        &self.ipc,
                    );
                }
                // The frame size a stream negotiated at start no longer
                // matches the output; end every stream so consumers tear
                // down their PipeWire sessions instead of compositing
                // mismatched frames (ADR-0052).
                self.end_all_streams("output geometry changed");
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
                // The frozen snapshot no longer matches the output geometry;
                // end the session and close the selector rather than
                // presenting a stretched frame or cropping stale pixels.
                if self.screenshot_freeze.armed {
                    self.screenshot_freeze.disarm();
                    self.shell.set_screenshot_freeze(false);
                    if self.shell.screenshot_active() {
                        self.shell.start_screenshot();
                    }
                }
                let (nw, nh) = self.host.physical_size();
                self.surface.resize(nw, nh)?;
                self.composite_slot_damage.clear();
                // Damage tracked against the old framebuffer does not
                // describe the rebuilt one; render the next frame in full.
                self.force_full_redraw = true;
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

    /// Drain one-shot and stream capture requests and bind at most one
    /// readback to the upcoming presentation frame. Runs before the
    /// render/skip decision so a bound capture always forces presentation.
    /// The readback copy is recorded after every scene and cursor draw, so it
    /// captures exactly the pixels submitted rather than a later re-render of
    /// mutable state.
    fn prepare_frame_capture(
        &mut self,
        session_locked: bool,
        pending_screenshots: &mut Vec<(aegis_ipc::Command, u64, aegis_ipc::Origin)>,
    ) -> Option<(Option<aegis_core::Rect>, CaptureTarget)> {
        let mut frame_capture: Option<(Option<aegis_core::Rect>, CaptureTarget)> = None;
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
                frame_capture = Some((req.region, CaptureTarget::Reply { reply: req.reply }));
            }
        }
        for (cmd, ts, origin) in pending_screenshots.drain(..) {
            let aegis_ipc::Command::Screenshot { path, region } = &cmd else {
                continue;
            };
            if session_locked || !self.host.is_active() {
                journal_effect_and_broadcast(
                    &self.journal,
                    &self.ipc,
                    ts,
                    origin,
                    cmd,
                    aegis_ipc::Effect::Refused {
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
                    aegis_ipc::Effect::Refused {
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
        // Stream fan-out (ADR-0052): when no one-shot capture claimed this
        // frame's readback and the staging slot and worker lane are free,
        // bind one readback shared by every due stream. One-shots keep
        // priority; a locked or inactive session simply produces no stream
        // frames (the stream survives).
        if frame_capture.is_none()
            && self.pending_capture.is_none()
            && !self.stream_job_in_flight
            && !session_locked
            && self.host.is_active()
            && !self.capture_worker.is_busy()
            && !self.streams.due_ids(std::time::Instant::now()).is_empty()
        {
            frame_capture = Some((None, CaptureTarget::Stream));
        }
        frame_capture
    }
}

fn capture_cursor_snapshot(
    device: &flux::Device,
    cache: &mut cursor::CursorCache,
    position: (f32, f32),
    shape: u32,
    scale: f32,
) -> Option<CaptureCursor> {
    cache.get(device, shape, scale).map(|loaded| CaptureCursor {
        x: (position.0 * scale - loaded.xhot).round() as i32,
        y: (position.1 * scale - loaded.yhot).round() as i32,
        width: loaded.width.round().max(1.0) as u32,
        height: loaded.height.round().max(1.0) as u32,
        bgra: std::sync::Arc::clone(&loaded.pixels),
    })
}
