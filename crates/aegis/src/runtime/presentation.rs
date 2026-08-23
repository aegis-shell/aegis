use super::*;

pub(super) enum PresentationOutcome {
    Submitted,
    NoDamage { callbacks_sent: bool },
    Retry,
}

mod capture;
use capture::FrameCapture;

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
            input_pointer_only,
            mut pending_screenshots,
        } = state;
        // Scene colors track the desktop appearance: the shell's design
        // snapshot carries the resolved scheme, refreshed by every
        // preferences reload, so re-derive the opaque clear from it.
        let color_scheme = self.shell.design().scheme;
        self.clear = clear_color(color_scheme);
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
        // The largest theme-sprite edge, for pointer-only partial repaints
        // (damage assessment).
        let cursor_extent = self
            .cursor_cache
            .sprite_extent(cursor_shape, scale)
            .unwrap_or(48.0); // Upload and place compositor-owned theme cursors on dedicated KMS
        // planes before deciding whether a full-output client can scan out
        // directly. The cheap Arc clone ends the CursorCache borrow before
        // mutating the host; cursor buffers themselves are cached by exact
        // pixels in the DRM backend.
        // A previously rejected cursor plane re-arms itself once its failure
        // backoff elapses; the ordinary cursor commit below is the probe.
        let cursor_probe_armed = self.host.poll_hardware_cursor_retry();
        // Sprite identity this frame wants on the hardware plane. Written
        // whenever a cursor commit is actually attempted and read by the
        // present-side baseline updates below; the zero default means the
        // plane was hidden, disabled, or failed to load, in which case no
        // baseline may claim the plane still shows a particular sprite.
        let mut last_committed_hotspot: (u32, u32) = (0, 0);
        let mut last_committed_sprite_size: (u32, u32) = (0, 0);
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
                    // Same sprite, hotspot, and position as the last
                    // committed cursor: skip the backend cache lookup and
                    // the KMS property diff entirely. This is the steady
                    // state of a static pointer while the frame loop still
                    // runs (animations, streams) without any cursor change.
                    // A just-re-armed disabled plane must not skip: its
                    // probe is exactly this commit, and skipping it would
                    // leave the cursor invisible until the next change.
                    let unchanged = self.damage.last_presented_cursor
                        == Some((cursor_shape, cursor_hidden))
                        && self.damage.last_presented_cursor_position == Some(cursor_position)
                        && self.damage.last_presented_cursor_hotspot == Some(hotspot)
                        && self.damage.last_presented_cursor_pixels == Some(size);
                    if !unchanged || cursor_probe_armed {
                        self.host.set_hardware_cursor(Some(HardwareCursor {
                            pixels: &pixels,
                            size,
                            hotspot,
                            position: cursor_position,
                        }));
                    }
                    last_committed_hotspot = hotspot;
                    last_committed_sprite_size = size;
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
        // A due dmabuf stream (ADR-0130) forces a composite present exactly
        // like a bound readback: its capture-surface slot is filled from the
        // presented frame right after the commit. An active stream paces
        // presentation at its negotiated max-fps, so its consumer observes
        // real frames at that rate even on a static screen.
        let now = std::time::Instant::now();
        let dmabuf_stream_forcing_due =
            !session_locked && self.host.is_active() && self.streams.forcing_due_dmabuf(now);
        let shm_stream_forcing_due =
            !session_locked && self.host.is_active() && self.streams.forcing_due_shm(now);
        let stream_forcing_due = dmabuf_stream_forcing_due || shm_stream_forcing_due;
        let dmabuf_stream_capture_due =
            !session_locked && self.host.is_active() && self.streams.any_dmabuf_due(now);
        let screenshot_include_cursor = self
            .config
            .as_ref()
            .is_none_or(|config| config.screenshot.include_cursor);
        let bound_saved_screenshot = frame_capture
            .as_ref()
            .is_some_and(|capture| matches!(&capture.target, CaptureTarget::Screenshot { .. }));
        let bound_screenshot_cursor = frame_capture
            .as_ref()
            .filter(|capture| matches!(&capture.target, CaptureTarget::Screenshot { .. }))
            .and_then(|capture| capture.cursor);
        // Print-key screenshots keep the freeze armed through confirmation.
        // Portal pick sessions also use the freeze, but retain their own
        // cursor-free capture contract and are not governed by this setting.
        let human_screenshot_session = self.screenshot_freeze.armed && self.pending_pick.is_none();
        let frozen_trigger_cursor = human_screenshot_session
            .then(|| self.screenshot_freeze.trigger_cursor())
            .flatten();
        let failed_human_freeze = human_screenshot_session && self.screenshot_freeze.failed;
        let mut capturing_frozen_screenshot = false;
        // Resolve chrome's next-frame geometry before damage assessment and
        // plane assignment. An input-only hidden Dock can therefore turn a
        // bottom-edge entry into visible animation even while the previous
        // frames were direct-scanned-out and no canvas pass was running.
        self.shell.prepare_backdrop(&input);
        let assessed_damage = self.assess_frame_damage(DamageAssessment {
            had_input,
            input_pointer_only,
            session_locked,
            cursor_hidden,
            cursor_shape,
            software_cursor: self.host.uses_software_cursor(),
            cursor_position: self.input_acc.cursor,
            cursor_extent,
            scale,
            physical_size,
        });
        let mut damage = assessed_damage.output;
        let backdrop_source_damage = assessed_damage.backdrop_source;
        let cursor_plane_changed = self.host.supports_hardware_cursor()
            && (self.damage.last_presented_cursor != Some((cursor_shape, cursor_hidden))
                || (!cursor_hidden
                    && self.damage.last_presented_cursor_position != Some(cursor_position)));
        let cursor_only_eligible = matches!(damage, FrameDamage::None)
            && cursor_plane_changed
            && frame_capture.is_none()
            && !stream_forcing_due
            && !self.streams.any_output_live()
            && self.pending_capture.is_none()
            && self.pending_interaction_domain_capture.is_none()
            && !self.screenshot_freeze.armed
            && !self.server.lock_confirmation_pending()
            && !self.server.retired_buffers_pending();
        if cursor_only_eligible {
            match self.host.present_cursor() {
                Ok(()) => {
                    self.server
                        .send_frame_callbacks(self.start.elapsed().as_millis() as u32);
                    self.damage.last_present_minute = Some(wall_clock_minute());
                    self.damage.last_presented_cursor = Some((cursor_shape, cursor_hidden));
                    self.damage.last_presented_cursor_position = Some(cursor_position);
                    self.damage.last_presented_cursor_hotspot =
                        (!cursor_hidden).then_some(last_committed_hotspot);
                    self.damage.last_presented_cursor_pixels =
                        (!cursor_hidden).then_some(last_committed_sprite_size);
                    self.renderer.gc(self.server.live_surface_ids());
                    self.renderer.begin_frame();
                    self.frame_count += 1;
                    return Ok(PresentationOutcome::Submitted);
                }
                Err(HostError::Drm(DrmError::CursorFallback)) => {
                    // The backend disabled the cursor plane. Repaint this
                    // frame with the software cursor rather than showing one
                    // cursorless refresh.
                    damage = FrameDamage::Full;
                }
                Err(HostError::Drm(
                    DrmError::Busy
                    | DrmError::FlipTimeout
                    | DrmError::Inactive
                    | DrmError::Reconfigured,
                )) => {
                    // Nothing was presented and no damage baseline was
                    // consumed, so the retry stays on the cheap cursor-only
                    // path: the unpresented position keeps
                    // `cursor_plane_changed` true next frame. The scheduler
                    // paces the retry to the next estimated vblank.
                    return Ok(PresentationOutcome::Retry);
                }
                Err(HostError::Drm(DrmError::ScanoutUnsupported)) => {
                    damage = FrameDamage::Full;
                }
                Err(error) => return Err(error.into()),
            }
        }
        if matches!(damage, FrameDamage::None)
            && frame_capture.is_none()
            && !stream_forcing_due
            && self.pending_capture.is_none()
            && self.pending_interaction_domain_capture.is_none()
            && !self.screenshot_freeze.armed
            && !self.server.lock_confirmation_pending()
            && !self.server.retired_buffers_pending()
        {
            // Nothing visible changed: skip the render and the atomic commit
            // — the scanout contents are already correct, so presenting
            // would be pure waste. A frame callback may complete without a
            // GPU submission once per estimated refresh cycle; callbacks
            // arriving before that boundary remain pending instead of
            // forcing empty composites.
            let callbacks_sent = self.presentation.frame_callbacks_allowed()
                && self
                    .server
                    .send_frame_callbacks(self.start.elapsed().as_millis() as u32)
                    > 0;
            self.renderer.gc(self.server.live_surface_ids());
            self.renderer.begin_frame();
            return Ok(PresentationOutcome::NoDamage { callbacks_sent });
        }
        // Direct-scanout fast path: a single opaque dma-buf client whose actual
        // geometry covers the whole output (and nothing else needing
        // compositing) can
        // be page-flipped directly onto the primary plane, skipping the Vulkan
        // composite. This is the primary-plane zero-GPU-cost path for games,
        // maximized video, and semantic fullscreen alike. A miss or
        // a kernel rejection falls through to the normal composite below.
        // Scanout stays disqualified while any output stream lives
        // (ADR-0130): a page-flipped frame never passes through the
        // compositor, so streams could not capture it.
        //
        // While scanout is active the renderer never composites, so the damage
        // tracker's per-surface generation baselines go stale: a client that
        // committed frames entirely through the scanout path looks "unchanged"
        // to it. Leaving scanout must therefore force a full redraw the next
        // composite, or the fallback frame could skip rendering and show a
        // frozen image.
        let was_scanout = self.primary_plane_state.is_direct();
        let primary_plane_plan = self.plan_primary_plane(
            physical_size,
            cursor_hidden,
            frame_capture.is_some() || self.streams.any_output_live(),
        );
        if let Some(candidate) = primary_plane_plan.direct_candidate() {
            match self.host.present_scanout(candidate, damage.area_rects()) {
                Ok(completion_fence) => {
                    let entered_scanout = self.primary_plane_state.commit_direct(candidate.id);
                    self.scanout_telemetry.record_success();
                    // No backdrop slot was maintained while the compositor
                    // was bypassed. Re-entering composition must rebuild from
                    // the then-current desktop rather than reuse an old effect.
                    self.launcher_backdrop.invalidate();
                    if entered_scanout {
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
                    // disqualified by the primary-plane plan, so none run here.
                    // Live output streams disqualify scanout outright
                    // (ADR-0130), so this path never runs while one exists;
                    // a stopped stream's leftover damage still accumulates
                    // for the next captured frame (ADR-0127).
                    self.streams.accumulate_damage(&damage);
                    self.server.acknowledge_presented_surface_damage();
                    if self.server.retired_buffers_pending() {
                        self.server.release_retired_buffers(
                            completion_fence.as_ref().map(AsRawFd::as_raw_fd),
                        );
                    }
                    self.server
                        .send_frame_callbacks(self.start.elapsed().as_millis() as u32);
                    self.damage.last_present_minute = Some(wall_clock_minute());
                    self.damage.last_presented_cursor = Some((cursor_shape, cursor_hidden));
                    self.damage.last_presented_cursor_position = Some(cursor_position);
                    self.damage.last_presented_cursor_hotspot =
                        (!cursor_hidden).then_some(last_committed_hotspot);
                    self.damage.last_presented_cursor_pixels =
                        (!cursor_hidden).then_some(last_committed_sprite_size);
                    self.damage.force_full_redraw = false;
                    self.renderer.gc(self.server.live_surface_ids());
                    self.renderer.begin_frame();
                    self.frame_count += 1;
                    return Ok(PresentationOutcome::Submitted);
                }
                Err(error) => {
                    // Any rejection (unsupported format, EACCES, reconfigure,
                    // flip timeout) falls through to compositing. Do not change
                    // primary-plane state yet: the previous framebuffer still
                    // owns the plane until the fallback commit succeeds.
                    if matches!(error, HostError::Drm(DrmError::ScanoutUnsupported)) {
                        self.scanout_telemetry.record_rejection(
                            self.host.name(),
                            ScanoutRejectReason::KmsRejected,
                            true,
                        );
                    }
                    if matches!(error, HostError::Drm(DrmError::CursorFallback)) {
                        // The backend disabled the KMS cursor after rejecting
                        // this commit. The same frame must include the
                        // software cursor, outside any client-only damage
                        // scissor. The plane is disabled until its retry
                        // backoff elapses; dropping the sprite baseline
                        // guarantees the re-arm probe commit is issued.
                        damage = FrameDamage::Full;
                        self.damage.last_presented_cursor = None;
                        self.damage.last_presented_cursor_position = None;
                        self.damage.last_presented_cursor_hotspot = None;
                        self.damage.last_presented_cursor_pixels = None;
                    }
                    if matches!(
                        error,
                        HostError::Drm(
                            DrmError::Busy
                                | DrmError::FlipTimeout
                                | DrmError::Inactive
                                | DrmError::Reconfigured
                        )
                    ) {
                        self.damage.force_full_redraw = true;
                        return Ok(PresentationOutcome::Retry);
                    }
                    if !matches!(error, HostError::Drm(DrmError::ScanoutUnsupported)) {
                        log::warn!(
                            "{}: direct scanout failed; compositing instead: {error}",
                            self.host.name()
                        );
                    }
                }
            }
        } else if let Some(rejection) = primary_plane_plan.rejection() {
            self.scanout_telemetry.record_rejection(
                self.host.name(),
                rejection.reason,
                rejection.plausible_candidate,
            );
        }
        // Scanout was active last frame but is no longer eligible (a window
        // appeared, the cursor moved, chrome opened…): force a full composite so
        // the resumed render is correct rather than skipped as "unchanged".
        if was_scanout {
            self.damage.force_full_redraw = true;
            damage = FrameDamage::Full;
            self.launcher_backdrop.invalidate();
        }
        // Opportunistic stream capture: this frame composites for real
        // damage, so every due SHM stream may piggyback on it with the
        // shared readback — between the forced frames its cadence already
        // guarantees (ADR-0130). One-shot captures keep priority; a locked
        // or inactive session simply produces no stream frames (the streams
        // survive). When any due SHM stream negotiated the embedded cursor
        // mode, the readback carries a cursor snapshot along so the worker
        // can produce a composited twin next to the pristine frame
        // (ADR-0127).
        if frame_capture.is_none()
            && self.pending_capture.is_none()
            && !self.stream_job_in_flight
            && !session_locked
            && self.host.is_active()
            && !self.capture_worker.is_busy()
            && self.streams.any_shm_due(std::time::Instant::now())
        {
            frame_capture = Some(FrameCapture {
                crop: None,
                target: CaptureTarget::Stream,
                cursor: self.stream_shm_cursor_state(),
            });
        }
        match self.surface.begin_frame() {
            Ok(mut frame) => {
                let frame_slot = frame.index() as usize;
                // `repaint` also contains damage missed by this swapchain
                // slot; only this logical frame's damage may be fanned out to
                // the other slots after present, otherwise first-use Full
                // damage circulates around the ring forever.
                let mut presented_damage = damage.clone();
                let mut repaint = composite_repaint_for_slot(
                    &mut self.damage.composite_slot_damage,
                    frame_slot,
                    damage,
                );
                // Exact physical render area of the currently open output
                // base pass. Full-output and offscreen-target passes leave it
                // as `None`. If visible Lens chrome is drawn below, the base
                // no-stencil pass is ended and a stencil-capable LOAD pass is
                // reopened with this same area.
                let mut output_render_area: Option<flux::CanvasRenderArea> = None;
                self.renderer.begin_frame();
                let render_geometry = RenderGeometry {
                    logical_size,
                    scale,
                };
                let switcher_state = self.server.window_switcher_snapshot();
                // Only clone the window list when the switcher can actually
                // present: `windows()` walks every live surface and clones
                // each `Window` (title/app_id Strings included) — pure
                // per-frame heap churn in the common no-switcher case. The
                // shell check covers the closing animation, which outlives
                // the server-side session and still needs the live set to
                // keep its fading cards.
                let switcher_windows =
                    if switcher_state.is_some() || self.shell.window_switcher_active() {
                        self.server.windows()
                    } else {
                        Vec::new()
                    };
                let (switcher_order, switcher_selected) = switcher_state
                    .as_ref()
                    .map(|(order, selected)| (order.as_slice(), *selected))
                    .unwrap_or((&[], None));
                let switcher_display = self
                    .shell
                    .reserved()
                    .inset(self.server.output_logical_rect());
                let window_switcher = self.shell.prepare_window_switcher(
                    &input,
                    switcher_display,
                    &switcher_windows,
                    switcher_order,
                    switcher_selected,
                );
                let live_previews = if window_switcher.is_none() {
                    self.shell.live_preview_presentations()
                } else {
                    Vec::new()
                };
                let blur_sigma = self.shell.backdrop_blur_sigma();
                let backdrop_regions = self.shell.backdrop_regions(self.input_acc.display_size);
                let mut liquid_glass_regions =
                    self.shell.liquid_glass_regions(self.input_acc.display_size);
                // Region-level backdrop adaptation: fold the statistics this
                // frame slot submitted FLUX_MAX_FRAMES_IN_FLIGHT frames ago
                // into the smoothed policy, then write it back into the
                // declared regions before cache keys and prism groups are
                // built from them.
                let glass_slot = frame.index() as usize;
                let submitted = self
                    .submitted_glass_ids
                    .get(glass_slot)
                    .map(Vec::as_slice)
                    .unwrap_or(&[]);
                if !submitted.is_empty() {
                    let mut stats = [prism::BackdropStats::default(); 64];
                    if let Ok(count) = self.launcher_backdrop.glass_stats(&frame, &mut stats) {
                        let dt = input.as_raw().dt_seconds.max(0.0);
                        for (index, stat) in
                            stats.iter().take(count.min(submitted.len())).enumerate()
                        {
                            let id = submitted[index];
                            if id != 0 {
                                self.glass_adaptation.observe(
                                    id,
                                    stat.mean_luminance,
                                    stat.high_freq_energy,
                                    dt,
                                );
                            }
                        }
                    }
                }
                let mut live_glass_ids = Vec::with_capacity(liquid_glass_regions.len());
                for region in &mut liquid_glass_regions {
                    self.glass_adaptation.apply_to(region);
                    if region.id != 0 {
                        live_glass_ids.push(region.id);
                    }
                }
                self.glass_adaptation.retain(&live_glass_ids);
                let model_active = self
                    .wallpaper
                    .as_ref()
                    .is_some_and(aegis_wallpaper::Wallpaper::has_model);
                // Once frozen, the immutable screenshot image is the backdrop
                // source even if the live wallpaper owns a 3D model. Keep the
                // effect capture local to the glass footprint in that state.
                let backdrop_model_active = model_active && !self.screenshot_freeze.active();
                // Overview mode (M9) swaps the whole client scene for the
                // thumbnail grid and skips the launcher-blur capture path.
                // The grid stays up while the reveal animation runs in
                // either direction: keying the swap on `open` alone would
                // pop the desktop back the instant the close fade starts,
                // stranding the chrome's cell frames mid-flight.
                let overview_progress = self.shell.overview_progress();
                let overview_active = self.shell.overview_active() || overview_progress > 0.001;
                // A screenshot freeze session replaces the whole frame with
                // the trigger-frame snapshot: the capture frame renders the
                // desktop scene *and* the chrome into an offscreen target
                // below; later frames blit that snapshot and draw only the
                // selector on top. The launcher-blur path stays off for the
                // whole session.
                let freeze_capturing = self.screenshot_freeze.needs_capture();
                // Compositor blurred shadows (ADR-0139): render masks and
                // record the shadow passes at this pass boundary, before any
                // output pass opens. The borrowed filter outputs are placed
                // by `aegis-render` inside the client-scene pass below.
                let shadow_style = self
                    .config
                    .as_ref()
                    .map(|c| c.ui.window_shadow)
                    .unwrap_or_default();
                let rendered_shadows = if shadow_style
                    == aegis_model::window::WindowShadowStyle::Soft
                {
                    let windows = self.server.render_windows();
                    self.window_shadows.prepare(
                        &self.device,
                        &self.canvas,
                        &frame,
                        &windows,
                        scale,
                        shadow_style,
                    )
                } else {
                    Vec::new()
                };
                // Borrow the raw effect outputs as draw-only entries for the
                // renderer. The images stay valid for this frame: the filter
                // slot is not applied again until the next rotation, after
                // this frame submits (ADR-0074 lifetime).
                let soft_shadow_entries: Vec<aegis_render::SoftShadowEntry<'_>> =
                    rendered_shadows
                        .iter()
                        .map(|shadow| aegis_render::SoftShadowEntry {
                            window: shadow.window,
                            raw: shadow.raw,
                            _borrow: std::marker::PhantomData,
                            x: shadow.x,
                            y: shadow.y,
                            w: shadow.w,
                            h: shadow.h,
                        })
                        .collect();
                let soft_shadow_layer = (!soft_shadow_entries.is_empty())
                    .then(|| aegis_render::SoftShadowLayer {
                        entries: soft_shadow_entries.as_slice(),
                    });
                // Restrict the backdrop capture to the union of the declared
                // blur regions (plus the blur footprint): the offscreen pass
                // re-renders the scene every frame, so covering only what the
                // blur can sample avoids a second full-screen scene render.
                // A live 3D wallpaper still draws its model into the whole
                // capture image, so it keeps the full-frame extent.
                //
                // Capture coverage comes from the declared backdrop regions
                // plus each glass body's optional `capture_bounds` footprint:
                // a body whose shape animates every frame declares the stable
                // envelope its shapes stay inside, so the animation alone can
                // never invalidate the capture (only the effect is rebuilt).
                let mut capture_inputs = backdrop_regions.clone();
                capture_inputs.extend(
                    liquid_glass_regions
                        .iter()
                        .filter_map(|region| region.capture_bounds),
                );
                let capture_bounds =
                    (blur_sigma > 0.0 && !capture_inputs.is_empty() && !backdrop_model_active)
                        .then(|| {
                            blur_capture_bounds(
                                &capture_inputs,
                                logical_size,
                                physical_size,
                                scale,
                                blur_sigma,
                            )
                        });
                let (capture_origin, capture_extent) =
                    capture_bounds.unwrap_or(((0, 0), physical_size));
                // Keep disconnected chrome bodies disconnected all the way
                // through Optics' blur pyramid.  The capture image remains a
                // single allocation (needed by the shared frost/liquid
                // composition), but the compute passes no longer dispatch
                // the otherwise-empty bounding box between a top HUD and a
                // bottom Dock.
                let capture_regions = if blur_sigma > 0.0 && !capture_inputs.is_empty() {
                    if backdrop_model_active {
                        vec![BackdropCaptureRegion {
                            origin: (0, 0),
                            extent: physical_size,
                        }]
                    } else {
                        blur_capture_regions(
                            &capture_inputs,
                            logical_size,
                            physical_size,
                            scale,
                            blur_sigma,
                        )
                    }
                } else {
                    Vec::new()
                };
                let capture_target_extent = (
                    capture_extent.0.div_ceil(BACKDROP_DOWNSAMPLE).max(1),
                    capture_extent.1.div_ceil(BACKDROP_DOWNSAMPLE).max(1),
                );
                let frost_backdrop_regions: Vec<_> = backdrop_regions
                    .iter()
                    .copied()
                    .filter(|region| {
                        !liquid_glass_regions
                            .iter()
                            .any(|glass| glass.bounds == *region)
                    })
                    .collect();
                let backdrop_key = BackdropCacheKey::new(
                    capture_origin,
                    capture_extent,
                    physical_size,
                    blur_sigma,
                    scale,
                    backdrop_model_active,
                    &capture_regions,
                    window_switcher.as_ref(),
                );
                let backdrop_material_key = BackdropMaterialKey::new(
                    &frost_backdrop_regions,
                    &liquid_glass_regions,
                    glass_tint(color_scheme),
                );
                let backdrop_plan = if overview_active
                    || (self.screenshot_freeze.armed && !self.screenshot_freeze.active())
                {
                    self.launcher_backdrop.invalidate();
                    BackdropPlan::Direct
                } else {
                    self.launcher_backdrop.prepare(
                        blur_sigma > 0.0 && !capture_inputs.is_empty(),
                        &self.device,
                        &self.surface,
                        &frame,
                        backdrop_key,
                        backdrop_material_key,
                        &backdrop_source_damage,
                    )
                };
                let recompute_only = matches!(&backdrop_plan, BackdropPlan::Recompute);
                let refresh_capture_regions = match &backdrop_plan {
                    BackdropPlan::Refresh(regions) => regions.clone(),
                    BackdropPlan::Direct | BackdropPlan::Recompute | BackdropPlan::Cached => {
                        Vec::new()
                    }
                };
                let effect_work_regions = match &backdrop_plan {
                    BackdropPlan::Refresh(regions) => blur_regions_in_capture(
                        regions,
                        capture_origin,
                        capture_extent,
                        capture_target_extent,
                    ),
                    BackdropPlan::Recompute => blur_regions_in_capture(
                        &capture_regions,
                        capture_origin,
                        capture_extent,
                        capture_target_extent,
                    ),
                    BackdropPlan::Direct | BackdropPlan::Cached => Vec::new(),
                };
                let refresh_requested = matches!(&backdrop_plan, BackdropPlan::Refresh(_));
                // Only a Refresh renders the scene into the capture; a
                // Recompute reuses the still-valid capture image and skips
                // the capture passes entirely.
                let capture_started = refresh_requested
                    && effect_work_regions.first().is_some_and(|region| {
                        self.launcher_backdrop.begin_capture(
                            &self.canvas,
                            &frame,
                            self.clear,
                            flux::CanvasRenderArea {
                                x: region.x as i32,
                                y: region.y as i32,
                                width: region.width,
                                height: region.height,
                            },
                        )
                    });
                let recompute_ready = recompute_only && !effect_work_regions.is_empty();

                match backdrop_plan {
                    BackdropPlan::Refresh(_) | BackdropPlan::Recompute
                        if capture_started || recompute_ready =>
                    {
                        let capture_size = self
                            .launcher_backdrop
                            .capture_size(&frame)
                            .unwrap_or(capture_extent);
                        let capture_ratio = capture_size.0 as f32 / capture_extent.0.max(1) as f32;
                        let capture_scale = scale * capture_ratio;

                        let mut capture_ok = true;
                        // A Recompute skips the scene capture loop: the empty
                        // iterator keeps every downstream index/capture_ok
                        // assumption identical to a zero-region Refresh.
                        let capture_loop_regions: &[flux::BlurRegion] = if recompute_only {
                            &[]
                        } else {
                            &effect_work_regions
                        };
                        for (index, region) in capture_loop_regions.iter().enumerate() {
                            if index > 0 {
                                if let Err(error) = self.canvas.end_target_checked() {
                                    log::warn!(
                                        "launcher: backdrop capture pass failed ({error}); using translucent fallback"
                                    );
                                    capture_ok = false;
                                    break;
                                }
                                if !self.launcher_backdrop.begin_capture(
                                    &self.canvas,
                                    &frame,
                                    self.clear,
                                    flux::CanvasRenderArea {
                                        x: region.x as i32,
                                        y: region.y as i32,
                                        width: region.width,
                                        height: region.height,
                                    },
                                ) {
                                    capture_ok = false;
                                    break;
                                }
                            }

                            // The capture target only covers `capture_extent`:
                            // shift the scene so the capture origin lands at
                            // (0, 0). Render-area scissoring means disconnected
                            // HUD/Dock bands do not shade the empty pixels
                            // between them.
                            self.canvas.save();
                            self.canvas.translate(
                                -(capture_origin.0 as f32) * capture_ratio,
                                -(capture_origin.1 as f32) * capture_ratio,
                            );

                            if self.screenshot_freeze.active() {
                                // During selection the trigger frame is the
                                // effect's immutable source. Sampling the live
                                // scene here would make glass contents drift
                                // while the rest of the screenshot stays frozen.
                                if let Some(image) = self.screenshot_freeze.image() {
                                    self.canvas.draw_image(
                                        image,
                                        0.0,
                                        0.0,
                                        physical_size.0 as f32 * capture_ratio,
                                        physical_size.1 as f32 * capture_ratio,
                                    );
                                }
                            } else {
                                draw_wallpaper_background(
                                    &self.canvas,
                                    &self.device,
                                    &mut self.wallpaper,
                                    logical_size,
                                    capture_scale,
                                );

                                if model_active {
                                    if let Err(error) = self.canvas.end_target_checked() {
                                        log::warn!(
                                            "launcher: backdrop capture pass failed ({error}); using translucent fallback"
                                        );
                                        self.canvas.restore();
                                        capture_ok = false;
                                        break;
                                    }
                                    if let Some(target) = self.launcher_backdrop.target(&frame) {
                                        if let Some(wallpaper) = self.wallpaper.as_mut() {
                                            wallpaper.draw_model_to(
                                                &self.device,
                                                &mut frame,
                                                target,
                                            );
                                        }
                                        if let Err(error) = self.canvas.begin_target_pass(
                                            &frame,
                                            target,
                                            flux::CanvasPassOptions {
                                                clear: None,
                                                antialias: flux::CanvasAntialias::None,
                                                render_area: Some(flux::CanvasRenderArea {
                                                    x: region.x as i32,
                                                    y: region.y as i32,
                                                    width: region.width,
                                                    height: region.height,
                                                }),
                                                skip_stencil: true,
                                            },
                                        ) {
                                            log::warn!(
                                                "launcher: failed to resume backdrop capture ({error}); using translucent fallback"
                                            );
                                            self.canvas.restore();
                                            capture_ok = false;
                                            break;
                                        }
                                    }
                                }

                                draw_client_scene(
                                    &self.canvas,
                                    &self.device,
                                    &mut self.renderer,
                                    &self.server,
                                    capture_scale,
                                    window_switcher.is_some(),
                                    None,
                                    aegis_model::window::WindowShadowStyle::Resize,
                                );
                                if let Some(presentation) = window_switcher.as_ref() {
                                    draw_window_switcher_scrim(
                                        &self.canvas,
                                        logical_size,
                                        capture_scale,
                                        presentation,
                                        color_scheme,
                                    );
                                }
                            }
                            self.canvas.restore();
                        }
                        let glass_groups = liquid_glass_groups(
                            &liquid_glass_regions,
                            capture_origin,
                            scale,
                            capture_ratio,
                            color_scheme,
                        );
                        let capture_pixel_scale = scale * capture_ratio;
                        let capture_covers_output = capture_origin == (0, 0)
                            && capture_extent == physical_size
                            && capture_size == physical_size
                            && effect_work_regions.len() == 1
                            && effect_work_regions[0]
                                == (flux::BlurRegion {
                                    x: 0,
                                    y: 0,
                                    width: capture_size.0,
                                    height: capture_size.1,
                                });
                        let frost_capture_regions = backdrop_regions_in_capture(
                            &frost_backdrop_regions,
                            capture_origin,
                            capture_extent,
                            capture_size,
                            scale,
                        );
                        // The liquid-failure fallback frosts every visible
                        // effect body: the rectangular frost regions plus
                        // each glass body's live bounds. Capture-only
                        // footprints carry no material and stay out.
                        let all_backdrop_capture_regions = backdrop_regions_in_capture(
                            &frost_backdrop_regions
                                .iter()
                                .copied()
                                .chain(liquid_glass_regions.iter().map(|region| region.bounds))
                                .collect::<Vec<_>>(),
                            capture_origin,
                            capture_extent,
                            capture_size,
                            scale,
                        );
                        let refreshed = if recompute_only {
                            self.launcher_backdrop.recompute_effects(
                                &self.canvas,
                                &frame,
                                blur_sigma * capture_scale,
                                &effect_work_regions,
                                &frost_capture_regions,
                                &all_backdrop_capture_regions,
                                &glass_groups,
                                prism::LiquidGlassParams {
                                    refraction: 8.0 * capture_pixel_scale,
                                    chromatic_aberration: 1.25 * capture_pixel_scale,
                                    edge_width: 18.0 * capture_pixel_scale,
                                    ..Default::default()
                                },
                            )
                        } else {
                            capture_ok
                                && self.launcher_backdrop.finish_refresh(
                                    &self.canvas,
                                    &frame,
                                    blur_sigma * capture_scale,
                                    &effect_work_regions,
                                    &frost_capture_regions,
                                    &all_backdrop_capture_regions,
                                    &glass_groups,
                                    prism::LiquidGlassParams {
                                        refraction: 8.0 * capture_pixel_scale,
                                        chromatic_aberration: 1.25 * capture_pixel_scale,
                                        edge_width: 18.0 * capture_pixel_scale,
                                        ..Default::default()
                                    },
                                )
                        };

                        // Bookkeep which region ids this slot's submission
                        // carries so the frame-lagged prism statistics can be
                        // resolved back to regions. A failed effect pass
                        // leaves no trustworthy stats for the slot.
                        if self.submitted_glass_ids.len() <= glass_slot {
                            self.submitted_glass_ids
                                .resize_with(glass_slot + 1, Vec::new);
                        }
                        self.submitted_glass_ids[glass_slot] = if refreshed {
                            liquid_glass_regions
                                .iter()
                                .filter(|region| glass_region_active(region))
                                .map(|region| region.id)
                                .collect()
                        } else {
                            Vec::new()
                        };

                        // Refreshing the sampled input may change every visible
                        // effect pixel even when the source damage itself lies
                        // just outside the glass body (inside the blur radius).
                        // Add those regions to the final output repaint.
                        let effect_damage_source = if recompute_only {
                            &capture_regions
                        } else {
                            &refresh_capture_regions
                        };
                        let effect_damage: Vec<_> = effect_damage_source
                            .iter()
                            .map(|region| {
                                aegis_model::Rect::new(
                                    region.origin.0 as i32,
                                    region.origin.1 as i32,
                                    region.extent.0 as i32,
                                    region.extent.1 as i32,
                                )
                            })
                            .collect();
                        repaint = repaint.with_rects(effect_damage.clone());
                        presented_damage = presented_damage.with_rects(effect_damage);
                        output_render_area = frame_damage_render_area(&repaint);
                        begin_opaque_frame_repaint(
                            &self.canvas,
                            &frame,
                            physical_size,
                            self.clear,
                            &repaint,
                        )?;
                        if self.screenshot_freeze.active() {
                            if let Some(image) = self.screenshot_freeze.image() {
                                self.canvas.draw_image(
                                    image,
                                    0.0,
                                    0.0,
                                    physical_size.0 as f32,
                                    physical_size.1 as f32,
                                );
                            }
                        } else if capture_covers_output
                            && refreshed
                            && self.launcher_backdrop.draw_capture_opaque(
                                &self.canvas,
                                &frame,
                                physical_size,
                            )
                        {
                            // The capture pass already rendered this exact
                            // desktop.  Reuse it as the base output rather than
                            // acquiring every client dma-buf and drawing the
                            // complete scene a second time in the same frame.
                        } else {
                            // A local capture has no pixels for the rest of the
                            // output (or the effect failed), so draw the normal
                            // desktop base once before overlaying the effect.
                            draw_direct_desktop_scene(
                                &self.canvas,
                                &self.device,
                                &mut frame,
                                &mut self.wallpaper,
                                &mut self.renderer,
                                &self.server,
                                render_geometry,
                                output_render_area,
                                overview_active,
                                overview_progress,
                                window_switcher.as_ref(),
                                color_scheme,
                                soft_shadow_layer.as_ref(),
                                shadow_style,
                            )?;
                        }
                        if refreshed {
                            self.launcher_backdrop.draw_cached(
                                &self.canvas,
                                &frame,
                                capture_origin,
                                capture_extent,
                            );
                        }
                    }
                    BackdropPlan::Cached => {
                        // The desktop still repaints normally, but the effect
                        // image is only sampled. No capture, blur, or liquid
                        // compute is recorded for this frame.
                        output_render_area = frame_damage_render_area(&repaint);
                        begin_opaque_frame_repaint(
                            &self.canvas,
                            &frame,
                            physical_size,
                            self.clear,
                            &repaint,
                        )?;
                        if self.screenshot_freeze.active() {
                            if let Some(image) = self.screenshot_freeze.image() {
                                self.canvas.draw_image(
                                    image,
                                    0.0,
                                    0.0,
                                    physical_size.0 as f32,
                                    physical_size.1 as f32,
                                );
                            }
                        } else {
                            draw_direct_desktop_scene(
                                &self.canvas,
                                &self.device,
                                &mut frame,
                                &mut self.wallpaper,
                                &mut self.renderer,
                                &self.server,
                                render_geometry,
                                output_render_area,
                                overview_active,
                                overview_progress,
                                window_switcher.as_ref(),
                                color_scheme,
                                soft_shadow_layer.as_ref(),
                                shadow_style,
                            )?;
                        }
                        self.launcher_backdrop.draw_cached(
                            &self.canvas,
                            &frame,
                            capture_origin,
                            capture_extent,
                        );
                    }
                    BackdropPlan::Refresh(_) | BackdropPlan::Recompute | BackdropPlan::Direct => {
                        if refresh_requested {
                            // Capture setup failed. Repaint the previous effect
                            // footprint with the direct scene so stale cached
                            // glass cannot survive in this swapchain image.
                            let effect_damage: Vec<_> = refresh_capture_regions
                                .iter()
                                .map(|region| {
                                    aegis_model::Rect::new(
                                        region.origin.0 as i32,
                                        region.origin.1 as i32,
                                        region.extent.0 as i32,
                                        region.extent.1 as i32,
                                    )
                                })
                                .collect();
                            repaint = repaint.with_rects(effect_damage.clone());
                            presented_damage = presented_damage.with_rects(effect_damage);
                        }
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
                                            if let Err(error) = self.canvas.end_target_checked() {
                                                log::warn!(
                                                    "screenshot: freeze capture interrupted ({error}); falling back to live scene"
                                                );
                                                self.screenshot_freeze.failed = true;
                                                in_target = false;
                                            } else {
                                                if let Some(wallpaper) = self.wallpaper.as_mut() {
                                                    wallpaper.draw_model_to(
                                                        &self.device,
                                                        &mut frame,
                                                        target,
                                                    );
                                                }
                                                if let Err(error) = self.canvas.begin_target_pass(
                                                    &frame,
                                                    target,
                                                    flux::CanvasPassOptions {
                                                        clear: None,
                                                        antialias: flux::CanvasAntialias::None,
                                                        render_area: None,
                                                        skip_stencil: true,
                                                    },
                                                ) {
                                                    log::warn!(
                                                        "screenshot: freeze capture interrupted ({error}); falling back to live scene"
                                                    );
                                                    self.screenshot_freeze.failed = true;
                                                    in_target = false;
                                                }
                                            }
                                        }
                                        if in_target {
                                            draw_client_scene(
                                                &self.canvas,
                                                &self.device,
                                                &mut self.renderer,
                                                &self.server,
                                                scale,
                                                false,
                                                None,
                                                aegis_model::window::WindowShadowStyle::Resize,
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
                                    None,
                                    overview_active,
                                    overview_progress,
                                    window_switcher.as_ref(),
                                    color_scheme,
                                    soft_shadow_layer.as_ref(),
                                    shadow_style,
                                )?;
                            }
                        } else {
                            output_render_area = frame_damage_render_area(&repaint);
                            begin_opaque_frame_repaint(
                                &self.canvas,
                                &frame,
                                physical_size,
                                self.clear,
                                &repaint,
                            )?;
                            draw_direct_desktop_scene(
                                &self.canvas,
                                &self.device,
                                &mut frame,
                                &mut self.wallpaper,
                                &mut self.renderer,
                                &self.server,
                                render_geometry,
                                output_render_area,
                                overview_active,
                                overview_progress,
                                window_switcher.as_ref(),
                                color_scheme,
                                soft_shadow_layer.as_ref(),
                                shadow_style,
                            )?;
                        }
                    }
                }
                if !session_locked && !self.screenshot_freeze.active() {
                    if let Some(presentation) = window_switcher.as_ref() {
                        draw_window_switcher_cards(
                            &self.canvas,
                            &self.device,
                            &mut self.renderer,
                            &self.server,
                            scale,
                            presentation,
                        );
                    } else {
                        draw_live_preview_scenes(
                            &self.canvas,
                            &self.device,
                            &mut self.renderer,
                            &self.server,
                            scale,
                            &live_previews,
                        );
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
                                w.always_on_top,
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
                    let space_use = aegis_model::window::SpaceUse::from_windows(&win_snapshot);
                    if self.last_space_use != Some(space_use) {
                        self.last_space_use = Some(space_use);
                        if let Some(s) = self.ipc.as_ref() {
                            s.broadcast(aegis_ipc::Event::SpaceUseChanged { state: space_use });
                        }
                    }
                    let accessibility_windows = self.server.accessibility_window_bindings();
                    self.live
                        .set_windows(win_snapshot.clone(), accessibility_windows);
                    self.shell.set_windows(win_snapshot);
                }
                // The dock's strip is workspace-global: it additionally
                // receives every mapped toplevel, gated on its own hash so the
                // overview, switcher, IPC mirror, and SpaceUse above keep the
                // visible-set snapshot untouched.
                let all_windows_hash = self.server.all_windows_signature();
                if self.last_all_windows_hash != Some(all_windows_hash) {
                    self.last_all_windows_hash = Some(all_windows_hash);
                    let all_windows = self.server.all_windows();
                    // The window-capture delivery recheck reads the same
                    // workspace-global set from the IPC live state.
                    self.live.set_all_windows(all_windows.clone());
                    self.shell.set_all_windows(all_windows);
                }
                // Minimize flight targets follow the dock's resting tile
                // layout; pushing them every frame keeps even
                // client-initiated minimizes aimed at the real icon.
                let targets = self.shell.minimize_targets(self.input_acc.display_size);
                self.server.set_minimize_targets(targets);
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
                let interaction_domain_revision = self.server.interaction_domain_revision();
                if self.last_interaction_domain_revision != Some(interaction_domain_revision) {
                    self.last_interaction_domain_revision = Some(interaction_domain_revision);
                    let interaction_domain_snapshot = self.server.interaction_domain_snapshot();
                    self.live
                        .set_interaction_domains(interaction_domain_snapshot.clone());
                    self.shell
                        .set_interaction_domains(interaction_domain_snapshot);
                    if let Some(s) = self.ipc.as_ref() {
                        s.broadcast(aegis_ipc::Event::InteractionDomainsChanged {
                            revision: interaction_domain_revision,
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
                // The desktop/client pass intentionally has no stencil
                // attachment. Lens is an arbitrary vector renderer and may
                // emit winding/even-odd path fills, so visible shell chrome
                // must run in a separate stencil-capable LOAD pass. Query the
                // shell only after every snapshot/status update above: its
                // `requires_composition` contract describes exact next-frame
                // visible output (and defaults conservatively). When false,
                // skip Lens replay entirely and retain the cheaper no-stencil
                // base pass; this is the same policy direct scanout trusts.
                let shell_requires_composition = self.shell.requires_composition();
                if shell_requires_composition {
                    if freeze_capturing && !self.screenshot_freeze.failed {
                        self.canvas.end_target_checked()?;
                        let target = self
                            .screenshot_freeze
                            .target(&frame)
                            .expect("active freeze capture has an allocated target");
                        begin_stencil_target_overlay(&self.canvas, &frame, target, None)?;
                    } else {
                        self.canvas.end_checked()?;
                        begin_stencil_frame_overlay(&self.canvas, &frame, output_render_area)?;
                    }
                    // Lens replay draws chrome text through flux-text, whose
                    // glyph-atlas flushes upload outside any batch otherwise —
                    // each flush is its own vkQueueSubmit plus a transient
                    // command pool. The nesting semantics of v0.0.23 make this
                    // scope safe even alongside the aegis-render batches: an
                    // inner begin joins whatever batch is open, and only the
                    // outermost flush submits. Errors are ignored here to
                    // match the aegis-render call sites (worst case is the
                    // pre-batching behaviour, never a lost draw).
                    let _uploads = self.device.uploads_begin();
                    unsafe { self.shell.render(self.canvas.as_raw() as *mut _, &input)? };
                }
                // Protocol overlays sit above ordinary shell chrome. A modal
                // screenshot/picker owns the whole frame and suppresses live
                // client overlays without changing Wayland keyboard focus.
                if !session_locked
                    && !self.shell.screenshot_active()
                    && !self.screenshot_freeze.active()
                {
                    let captured_cursor = if freeze_capturing || failed_human_freeze {
                        frozen_trigger_cursor
                    } else if bound_saved_screenshot {
                        bound_screenshot_cursor
                    } else {
                        None
                    };
                    let (include_live_cursor, cursor_position) = if freeze_capturing
                        && !human_screenshot_session
                    {
                        // Portal pick sessions are cursor-free.
                        (false, None)
                    } else if freeze_capturing || failed_human_freeze || bound_saved_screenshot {
                        let client_cursor = screenshot_include_cursor
                            && captured_cursor.is_some_and(|cursor| cursor.client_surface);
                        (
                            client_cursor,
                            client_cursor
                                .then(|| captured_cursor.map(|cursor| cursor.position))
                                .flatten(),
                        )
                    } else {
                        (true, None)
                    };
                    draw_client_overlays(
                        &self.canvas,
                        &self.device,
                        &mut self.renderer,
                        &self.server,
                        scale,
                        include_live_cursor,
                        cursor_position,
                    );
                }
                // Finish the freeze snapshot pass: the chrome above rendered
                // into the target as well; protocol overlays are included
                // above it. Resolve that whole trigger frame into the
                // on-screen blit, then open the selector over the frozen
                // screen. On failure the selector opens right away over the
                // live scene instead.
                if freeze_capturing && !self.screenshot_freeze.failed {
                    if human_screenshot_session
                        && screenshot_include_cursor
                        && let Some(cursor) = frozen_trigger_cursor
                        && !cursor.hidden
                        && !cursor.client_surface
                    {
                        // Bake the trigger-time themed cursor into the frozen
                        // image itself. The selector then draws the current
                        // live cursor above this immutable cursor, so both are
                        // visible throughout region selection.
                        draw_software_cursor(
                            &self.canvas,
                            &self.device,
                            &mut self.cursor_cache,
                            cursor.position,
                            cursor.shape,
                            scale,
                        );
                    }
                    self.canvas.end_target_checked()?;
                    self.screenshot_freeze.mark_captured(&frame);
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
                        frame_capture = Some(FrameCapture {
                            crop: Some(region),
                            target: CaptureTarget::Screenshot {
                                path,
                                command,
                                ts_mono_ms: ts,
                                origin: aegis_ipc::Origin::Chrome,
                            },
                            cursor: self.screenshot_freeze.trigger_cursor(),
                        });
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
                        frame_capture = Some(FrameCapture {
                            crop: Some(aegis_model::Rect::new(point.x, point.y, 1, 1)),
                            target: CaptureTarget::Pixel {
                                point,
                                reply: pick.reply,
                            },
                            cursor: None,
                        });
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
                    // The window-mode whole-output answer keeps the legacy
                    // bare shape (no connector) for wire compatibility
                    // (ADR-0128).
                    let _ = pick
                        .reply
                        .send(Ok(aegis_ipc::PickResult::Output { connector: None }));
                }
                if let Some(connector) = self.shell.take_picked_output()
                    && let Some(pick) = self.pending_pick.take()
                {
                    let result = if self
                        .server
                        .output_infos()
                        .iter()
                        .any(|output| output.connector == connector)
                    {
                        Ok(aegis_ipc::PickResult::Output {
                            connector: Some(connector),
                        })
                    } else {
                        Err("the picked output is gone".to_owned())
                    };
                    let _ = pick.reply.send(result);
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
                // App-pick delivery (the AppChooser portal's compositor
                // side).
                if let Some(id) = self.shell.take_app_pick_confirmed()
                    && let Some(pick) = self.pending_app_pick.take()
                {
                    let _ = pick.reply.send(Ok(aegis_ipc::AppPickResult::App { id }));
                }
                if self.shell.take_app_pick_cancelled()
                    && let Some(pick) = self.pending_app_pick.take()
                {
                    let _ = pick.reply.send(Ok(aegis_ipc::AppPickResult::Cancelled));
                }
                if self.pending_app_pick.is_some()
                    && !self.shell.app_pick_active()
                    && let Some(pick) = self.pending_app_pick.take()
                {
                    let _ = pick.reply.send(Ok(aegis_ipc::AppPickResult::Cancelled));
                }
                // Secret-prompt delivery (the vault password unlock's
                // compositor side), same shape as the other picks above.
                if let Some(value) = self.shell.take_secret_prompt_confirmed()
                    && let Some(pick) = self.pending_secret_prompt.take()
                {
                    let _ = pick
                        .reply
                        .send(Ok(aegis_ipc::SecretPromptResult::Secret { value }));
                }
                if self.shell.take_secret_prompt_cancelled()
                    && let Some(pick) = self.pending_secret_prompt.take()
                {
                    let _ = pick
                        .reply
                        .send(Ok(aegis_ipc::SecretPromptResult::Cancelled));
                }
                if self.pending_secret_prompt.is_some()
                    && !self.shell.secret_prompt_active()
                    && let Some(pick) = self.pending_secret_prompt.take()
                {
                    let _ = pick
                        .reply
                        .send(Ok(aegis_ipc::SecretPromptResult::Cancelled));
                }
                // Confirmation delivery (portal consent dialogs and
                // ADR-0088 runtime grants), same shape as the other picks
                // above.
                if let Some(answer) = self.shell.take_confirm_pick_answered()
                    && let Some(pick) = self.pending_confirm_pick.take()
                {
                    let _ = pick.reply.send(Ok(answer));
                }
                if self.pending_confirm_pick.is_some()
                    && !self.shell.confirm_pick_active()
                    && let Some(pick) = self.pending_confirm_pick.take()
                {
                    let _ = pick.reply.send(Ok(aegis_shell::ConfirmAnswer::Cancelled));
                }
                // Capability-checklist delivery (ADR-0088 agent pairing),
                // same shape as the other picks above.
                if let Some(result) = self.shell.take_capability_pick_answered()
                    && let Some(pick) = self.pending_capability_pick.take()
                {
                    let _ = pick.reply.send(Ok(result));
                }
                if self.pending_capability_pick.is_some()
                    && !self.shell.capability_pick_active()
                    && let Some(pick) = self.pending_capability_pick.take()
                {
                    let _ = pick
                        .reply
                        .send(Ok(aegis_shell::CapabilityPickResult { approved: None }));
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
                if let Some(id) = self.shell.take_window_switcher_pick() {
                    self.server.cancel_window_switcher();
                    self.shell.finish_window_switcher();
                    apply_chrome_window_command(
                        &mut self.server,
                        &self.notif_queue,
                        &mut self.quit_requested,
                        aegis_ipc::Command::Focus { id, reveal: true },
                        &self.ipc,
                        &self.journal,
                        ts,
                    );
                }
                if self.shell.take_window_switcher_cancel() {
                    self.server.cancel_window_switcher();
                    self.shell.finish_window_switcher();
                }
                if let Some(id) = self.shell.take_clicked_window() {
                    apply_chrome_window_command(
                        &mut self.server,
                        &self.notif_queue,
                        &mut self.quit_requested,
                        aegis_ipc::Command::Focus { id, reveal: true },
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
                        aegis_ipc::Command::Focus { id, reveal: true },
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
                        origin.clone(),
                    );
                }
                for action in self.shell.take_window_actions() {
                    let cmd = match action {
                        aegis_shell::WindowAction::Focus(id) => {
                            aegis_ipc::Command::Focus { id, reveal: true }
                        }
                        aegis_shell::WindowAction::Minimize(id) => {
                            aegis_ipc::Command::Minimize { id }
                        }
                        aegis_shell::WindowAction::SetMaximized(id, maximized) => {
                            aegis_ipc::Command::SetMaximized { id, maximized }
                        }
                        aegis_shell::WindowAction::SetAlwaysOnTop(id, on_top) => {
                            aegis_ipc::Command::SetAlwaysOnTop { id, on_top }
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
                // Mirror-guard drags move a read-only mirror's presentation
                // position. Like a title-bar move grab this is a
                // chrome-owned gesture, not a journaled command.
                if let Some(mirror_move) = self.shell.take_mirror_move() {
                    self.server
                        .start_mirror_move(mirror_move.window, mirror_move.cursor);
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
                        origin.clone(),
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
                        origin.clone(),
                    );
                }
                if let Some(app) = self.shell.take_open_builtin() {
                    // The selector opens through the freeze session so the
                    // trigger frame (chrome included) is snapshotted first.
                    if app == aegis_model::app::BuiltInApplication::ScreenshotSelector {
                        let cursor = self.capture_cursor_state();
                        self.screenshot_freeze.request_open(Some(cursor));
                    } else {
                        self.shell.open_builtin(app);
                    }
                }
                for intent in self.shell.take_interaction_domain_intents() {
                    let action = interaction_domain_intent_to_action(intent);
                    let before_revision = self.server.interaction_domain_revision();
                    let result =
                        apply_interaction_domain_action(&mut self.server, None, action.clone());
                    match &result {
                        Ok(_) => {
                            for interaction_domain in
                                interaction_domains_explicitly_stopped(&action)
                            {
                                self.automatically_paused_interaction_domains
                                    .remove(&interaction_domain);
                            }
                            let invalidated =
                                interaction_domain_action_invalidates_capture(&action);
                            if !invalidated.is_empty() {
                                self.capture_worker.invalidate_security_context();
                                if self
                                    .pending_interaction_domain_capture
                                    .as_ref()
                                    .is_some_and(|pending| {
                                        invalidated.contains(&pending.context.interaction_domain)
                                    })
                                {
                                    let pending = self
                                        .pending_interaction_domain_capture
                                        .take()
                                        .expect("capture invalidation predicate checked");
                                    refuse_capture_target(
                                        &self.capture_worker,
                                        CaptureTarget::InteractionDomainReply {
                                            context: pending.context,
                                            reply: pending.reply,
                                        },
                                        "Interaction Domain authority changed before capture completed".into(),
                                        &self.journal,
                                        &self.ipc,
                                    );
                                }
                            }
                            if let aegis_ipc::InteractionDomainAction::Revoke {
                                interaction_domain,
                                ..
                            } = &action
                            {
                                self.interaction_domain_render_targets
                                    .remove(interaction_domain);
                            }
                            self.interaction_domain_processes
                                .apply_committed_action(&action);
                            let snapshot = self.server.interaction_domain_snapshot();
                            self.last_interaction_domain_revision = Some(snapshot.revision);
                            self.live.set_interaction_domains(snapshot.clone());
                            self.shell.set_interaction_domains(snapshot.clone());
                            if let Some(ipc) = &self.ipc {
                                ipc.broadcast(aegis_ipc::Event::InteractionDomainsChanged {
                                    revision: snapshot.revision,
                                });
                            }
                        }
                        Err(error) => {
                            log::warn!("Interaction Domain action from shell refused: {error}");
                            let notification = self.notif_queue.lock().unwrap().push(
                                "Interaction Domain",
                                error.clone(),
                                Some("aegis".into()),
                                ts,
                            );
                            if let Some(ipc) = &self.ipc {
                                ipc.broadcast(aegis_ipc::Event::Notified { notification });
                            }
                        }
                    }
                    let after_revision = self.server.interaction_domain_revision();
                    let effect = match result {
                        Ok(_) => aegis_ipc::Effect::Applied,
                        Err(reason) => aegis_ipc::Effect::Refused { reason },
                    };
                    journal_mutation_effect_and_broadcast(
                        &self.journal,
                        &self.ipc,
                        ts,
                        aegis_ipc::Origin::Chrome,
                        aegis_ipc::JournalMutation::InteractionDomain {
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
                            &mut self.host,
                            &self.notif_queue,
                            &mut self.system_status,
                            &mut self.ipc_idle_inhibits,
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
                            origin.clone(),
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
                // Persistent-settings mutations from compositor-owned settings
                // UI: the same commit/journal path as IPC settings requests.
                let settings_actions = self.shell.take_settings_actions();
                for (expected_revision, action) in settings_actions {
                    let appearance_changed = matches!(
                        &action,
                        aegis_ipc::SettingsAction::SetDesktopPreferences { .. }
                    );
                    let before_revision = self.settings_revision;
                    let result = if self.server.session_locked() {
                        Err("session is locked".into())
                    } else {
                        // Disjoint-field form: a Flux frame borrows
                        // `self.surface` across this whole section.
                        settings::commit_settings_parts(
                            expected_revision,
                            action.clone(),
                            &mut self.settings_revision,
                            self.config_path.as_deref(),
                            &self.config_writer,
                            &mut self.config,
                            &mut self.keymap,
                            &mut self.gesture_map,
                            &mut self.server,
                            &mut self.shell,
                            &mut self.cursor_cache,
                            &mut self.host,
                            &mut self.reload,
                            &mut self.idle_process,
                            &self.live,
                            &mut self.system_status,
                            &mut self.input_acc,
                            &self.ipc,
                        )
                    };
                    if result.is_ok() {
                        // A committed settings action may redraw status chrome
                        // outside the signed server-state paths.
                        self.damage.chrome_dirty = true;
                        if appearance_changed {
                            queue_app_scan_parts(
                                &self.server,
                                &self.host,
                                self.config.as_ref(),
                                &self.scan_req_tx,
                                &mut self.next_app_scan,
                            );
                        }
                    }
                    let after_revision = self.settings_revision;
                    let effect = match &result {
                        Ok(_) => aegis_ipc::Effect::Applied,
                        Err(reason) => aegis_ipc::Effect::Refused {
                            reason: reason.clone(),
                        },
                    };
                    journal_mutation_effect_and_broadcast(
                        &self.journal,
                        &self.ipc,
                        ts,
                        origin.clone(),
                        aegis_ipc::JournalMutation::Settings {
                            action,
                            before_revision,
                            after_revision,
                        },
                        effect,
                    );
                }
                // The command panel's Lock now row: lock immediately through
                // the same idle-process path as the Super+L binding.
                if self.shell.take_lock() {
                    self.idle_process.lock_now();
                }
                // The dock's Launchpad tile was clicked: toggle the launcher
                // through the same path as the Super+A hotkey.
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
                    let pinned = resolve_chrome_pins(
                        &self.launcher_apps,
                        &self.icon_cache.map,
                        &pinned_list,
                        false,
                    );
                    self.shell.set_app_catalog(aegis_shell::AppCatalog {
                        apps: self.launcher_apps.clone(),
                        pinned,
                        icons: aegis_shell::IconSet::from_raw(self.icon_cache.map.clone()),
                        position: self
                            .config
                            .as_ref()
                            .map(|c| c.dock.position)
                            .unwrap_or_default(),
                    });
                }
                // A committed drag reorder carries the complete pinned list
                // in dock order (entry ids, already materialized from the
                // visible strip): persist it through the same path as a
                // pin/unpin edit. The dock applied the order optimistically;
                // this push reconciles it.
                if let Some(pinned_list) = self.shell.take_dock_reorder() {
                    if let Err(error) =
                        self.config_writer
                            .enqueue(aegis_config::ConfigEdit::SetDockPinned {
                                pinned: pinned_list.clone(),
                            })
                    {
                        log::warn!("dock: reordered pins not saved: {error}");
                    }
                    if let Some(c) = self.config.as_mut() {
                        c.dock.pinned = pinned_list.clone();
                        c.dock.autopopulate = false;
                    }
                    let pinned = resolve_chrome_pins(
                        &self.launcher_apps,
                        &self.icon_cache.map,
                        &pinned_list,
                        false,
                    );
                    self.shell.set_app_catalog(aegis_shell::AppCatalog {
                        apps: self.launcher_apps.clone(),
                        pinned,
                        icons: aegis_shell::IconSet::from_raw(self.icon_cache.map.clone()),
                        position: self
                            .config
                            .as_ref()
                            .map(|c| c.dock.position)
                            .unwrap_or_default(),
                    });
                }
                // A dock edge drag commits the edge it landed on: persist
                // it and reconcile the catalog-carried position. The dock
                // already switched edges optimistically during the gesture.
                if let Some(position) = self.shell.take_dock_position() {
                    if let Err(error) = self
                        .config_writer
                        .enqueue(aegis_config::ConfigEdit::SetDockPosition { position })
                    {
                        log::warn!("dock: position not saved: {error}");
                    }
                    if let Some(c) = self.config.as_mut() {
                        c.dock.position = position;
                    }
                    let pinned = resolve_chrome_pins(
                        &self.launcher_apps,
                        &self.icon_cache.map,
                        self.config
                            .as_ref()
                            .map(|c| c.dock.pinned.as_slice())
                            .unwrap_or(&[]),
                        self.config
                            .as_ref()
                            .map(|c| c.dock.autopopulate)
                            .unwrap_or(false),
                    );
                    self.shell.set_app_catalog(aegis_shell::AppCatalog {
                        apps: self.launcher_apps.clone(),
                        pinned,
                        icons: aegis_shell::IconSet::from_raw(self.icon_cache.map.clone()),
                        position,
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
                if self.host.uses_software_cursor() && !capturing_frozen_screenshot {
                    let saved_cursor = frame_capture
                        .as_ref()
                        .filter(|capture| {
                            matches!(&capture.target, CaptureTarget::Screenshot { .. })
                        })
                        .and_then(|capture| capture.cursor);
                    if let Some(cursor) = saved_cursor {
                        if screenshot_include_cursor && !cursor.hidden && !cursor.client_surface {
                            draw_software_cursor(
                                &self.canvas,
                                &self.device,
                                &mut self.cursor_cache,
                                cursor.position,
                                cursor.shape,
                                scale,
                            );
                        }
                    } else if !cursor_hidden {
                        draw_software_cursor(
                            &self.canvas,
                            &self.device,
                            &mut self.cursor_cache,
                            self.input_acc.cursor,
                            cursor_shape,
                            scale,
                        );
                    }
                }
                self.canvas.end_checked()?;
                let mut capture_for_present = frame_capture.take().and_then(|capture| {
                    let FrameCapture {
                        crop,
                        target,
                        cursor: trigger_cursor,
                    } = capture;
                    let cursor = if matches!(&target, CaptureTarget::Stream) {
                        // Embedded cursor streams (ADR-0127): the binding
                        // carried a filtered cursor state; rasterize the
                        // theme sprite so the worker can blend a composited
                        // twin of this frame next to the pristine one.
                        trigger_cursor.and_then(|cursor| {
                            capture_cursor_snapshot(
                                &self.device,
                                &mut self.cursor_cache,
                                cursor.position,
                                cursor.shape,
                                scale,
                            )
                        })
                    } else if screenshot_include_cursor
                        && matches!(&target, CaptureTarget::Screenshot { .. })
                    {
                        if capturing_frozen_screenshot {
                            // The trigger-time cursor is already part of the
                            // frozen image. Adding a readback overlay here
                            // would duplicate it in the saved PNG.
                            None
                        } else if !self.host.uses_software_cursor() {
                            trigger_cursor
                                .filter(|cursor| !cursor.hidden && !cursor.client_surface)
                                .and_then(|cursor| {
                                    capture_cursor_snapshot(
                                        &self.device,
                                        &mut self.cursor_cache,
                                        cursor.position,
                                        cursor.shape,
                                        scale,
                                    )
                                })
                        } else {
                            None
                        }
                    } else {
                        None
                    };
                    let physical_crop = crop.map(|rect| {
                        logical_rect_to_physical(
                            rect,
                            render_geometry.scale,
                            physical_size.0,
                            physical_size.1,
                        )
                    });
                    match request_frame_readback(
                        &mut frame,
                        physical_size,
                        physical_crop,
                        cursor,
                        self.capture_worker.security_generation(),
                    ) {
                        Ok(readback) => Some(PendingCapture { readback, target }),
                        Err(reason) => {
                            refuse_capture_target(
                                &self.capture_worker,
                                target,
                                reason,
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
                    match self
                        .host
                        .present(&self.surface, submitted, repaint.area_rects())
                    {
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
                                    DrmError::Busy
                                        | DrmError::FlipTimeout
                                        | DrmError::Inactive
                                        | DrmError::Reconfigured
                                        | DrmError::CursorFallback
                                )
                            ) {
                                if matches!(&error, HostError::Drm(DrmError::Busy)) {
                                    log::debug!(
                                        "{}: previous atomic commit still busy; coalescing frame",
                                        self.host.name()
                                    );
                                } else {
                                    log::warn!(
                                        "{}: transient present failure; skipping frame: {error}",
                                        self.host.name()
                                    );
                                }
                                // Damage/revision baselines were assessed
                                // before this frame was rendered. They must
                                // not let the retry skip content that never
                                // reached scanout.
                                self.damage.force_full_redraw = true;
                                return Ok(PresentationOutcome::Retry);
                            }
                            return Err(error.into());
                        }
                    };
                let left_scanout = self.primary_plane_state.commit_composited();
                if left_scanout {
                    log::info!(
                        "{}: compositor reclaimed the primary plane",
                        self.host.name()
                    );
                }
                // Fold this frame's damage into every stream's accumulator
                // before any stream-facing sampling below, so a frame
                // captured from this composite carries exactly the damage
                // that produced it (ADR-0127).
                self.streams.accumulate_damage(&presented_damage);
                if capture_for_present
                    .as_ref()
                    .is_some_and(|capture| matches!(capture.target, CaptureTarget::Stream))
                {
                    // A stream readback is bound to this frame: sample each
                    // due SHM stream's damage for its delivery.
                    self.stash_shm_stream_damage();
                }
                record_composite_present(
                    &mut self.damage.composite_slot_damage,
                    frame_slot,
                    presented_damage,
                );
                self.launcher_backdrop
                    .record_present(frame_slot, backdrop_source_damage);
                if let Some(capture) = capture_for_present {
                    debug_assert!(self.pending_capture.is_none());
                    self.pending_capture = Some(capture);
                }
                // The frame is on its way to scanout: copy it into every due
                // dmabuf stream's capture-surface slot (IPC protocol 25). The
                // frame events go out once each slot's acquire fence signals.
                // Due-ness alone only rides this already-happening composite
                // (ADR-0126); it never caused the frame.
                if dmabuf_stream_capture_due {
                    self.blit_dmabuf_stream_frames(completion_fence.as_ref());
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
                self.damage.last_present_minute = Some(wall_clock_minute());
                self.damage.last_presented_cursor = Some((cursor_shape, cursor_hidden));
                self.damage.last_presented_cursor_position = Some(cursor_position);
                self.damage.last_presented_cursor_hotspot =
                    (!cursor_hidden).then_some(last_committed_hotspot);
                self.damage.last_presented_cursor_pixels =
                    (!cursor_hidden).then_some(last_committed_sprite_size);
                self.damage.force_full_redraw = false;

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
                if let Some(capture) = frame_capture.take() {
                    refuse_capture_target(
                        &self.capture_worker,
                        capture.target,
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
                self.damage.force_full_redraw = true;
                return Ok(PresentationOutcome::Retry);
            }
            Err(_) => {
                if let Some(capture) = frame_capture.take() {
                    refuse_capture_target(
                        &self.capture_worker,
                        capture.target,
                        "output changed before capture".to_owned(),
                        &self.journal,
                        &self.ipc,
                    );
                }
                let (nw, nh) = self.host.physical_size();
                self.surface.resize(nw, nh)?;
                // The frame size a stream negotiated at start no longer
                // matches the output: freeze each affected stream with
                // `StreamGeometryChanged` so consumers renegotiate at the
                // new geometry instead of compositing mismatched frames
                // (ADR-0126). Runs after the resize so the new surface size
                // is the geometry streams are compared against.
                self.handle_output_geometry_change();
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
                self.damage.composite_slot_damage.clear();
                // Damage tracked against the old framebuffer does not
                // describe the rebuilt one; render the next frame in full.
                self.damage.force_full_redraw = true;
                if let Err(error) = self.surface.prepare_readback() {
                    log::warn!(
                        "capture: could not preallocate resized readback staging: {error}{}",
                        flux_last_error_detail()
                    );
                }
                return Ok(PresentationOutcome::Retry);
            }
        }

        Ok(PresentationOutcome::Submitted)
    }
}

pub(super) fn capture_cursor_snapshot(
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
