use super::*;

const APP_RESCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

pub(super) struct PendingScreenshot {
    pub(super) command: aegis_ipc::Command,
    pub(super) ts_mono_ms: u64,
    pub(super) origin: aegis_ipc::Origin,
    pub(super) cursor: CaptureCursorState,
}

pub(super) struct IterationWork {
    pub(super) pending_synthetic_input: Vec<(aegis_ipc::Command, u64, aegis_ipc::Origin)>,
    pub(super) pending_screenshots: Vec<PendingScreenshot>,
}

/// Disjoint-field form used while a Flux frame borrows `surface` (the same
/// split as `commit_settings`/`commit_settings_parts`).
pub(super) fn queue_app_scan_parts(
    server: &aegis_compositor::Server,
    host: &Host,
    config: Option<&aegis_config::Config>,
    scan_req_tx: &std::sync::mpsc::Sender<AppScanRequest>,
    next_app_scan: &mut std::time::Instant,
) {
    let scale = effective_icon_scale(
        server
            .output_infos()
            .first()
            .map(|output| output.geometry.scale.as_f32()),
        host.scale(),
    );
    let icon_theme = effective_desktop_preferences(config).icon_theme;
    let _ = scan_req_tx.send(AppScanRequest { icon_theme, scale });
    *next_app_scan = std::time::Instant::now() + APP_RESCAN_INTERVAL;
}

impl CompositorRuntime {
    fn queue_app_scan(&mut self) {
        queue_app_scan_parts(
            &self.server,
            &self.host,
            self.config.as_ref(),
            &self.scan_req_tx,
            &mut self.next_app_scan,
        );
    }

    pub(super) fn prepare_iteration(
        &mut self,
    ) -> Result<Option<IterationWork>, Box<dyn std::error::Error>> {
        // Process security-sensitive protocol transitions before completing
        // any pixel readback. In particular, an ext-session-lock request that
        // woke this iteration must become visible before a pending Interaction Domain or
        // desktop capture can be handed to its requester.
        self.server.dispatch();
        // A transition is live state only until its animation deadline. The
        // animation scheduler wakes this terminal iteration even without a
        // client commit, so retire settled records before any scene,
        // occlusion, callback, or capture query observes them.
        self.server.settle_finished_transitions();
        // The host polls a mux containing both the Wayland event-loop fd and
        // the capture worker's completion eventfd. Drain readiness only after
        // server dispatch, then consume the completion channel below. This
        // preserves event-driven idle sleep without turning worker activity
        // into compositor animation.
        self.capture_worker.drain_wakeup();
        self.idle_process.maintain();
        self.evaluate_night_light();
        self.semantic_adapter_process.maintain();
        let outputs_powered = self.host.outputs_powered();
        if !self.server.session_locked() && !outputs_powered {
            self.idle_process
                .require_output_wake(session::OutputWakeReason::SessionUnlock);
        }
        if let Some(wake_reason) = self.idle_process.output_wake_due(outputs_powered) {
            match self.host.set_outputs_powered(true) {
                Ok(()) => self.idle_process.output_wake_succeeded(),
                Err(error) => {
                    self.idle_process.output_wake_failed();
                    log::warn!(
                        "session: could not wake outputs for {}: {error}",
                        wake_reason.description()
                    );
                }
            }
        }
        self.capture_worker
            .set_allowed(!self.server.session_locked() && self.host.is_active());
        let agent_suspended = self.server.session_locked() || !self.host.is_active();
        if agent_suspended && !self.previous_agent_suspended {
            self.observations.discard_all();
            let snapshot = self.server.interaction_domain_snapshot();
            for interaction_domain in
                snapshot
                    .interaction_domains
                    .into_iter()
                    .filter(|interaction_domain| {
                        interaction_domain.kind
                            == aegis_model::interaction_domain::InteractionDomainKind::Agent
                            && interaction_domain.state
                                == aegis_model::interaction_domain::InteractionDomainState::Active
                    })
            {
                if self
                    .server
                    .pause_interaction_domain(interaction_domain.id)
                    .is_ok()
                {
                    self.interaction_domain_processes
                        .pause(interaction_domain.id);
                    self.automatically_paused_interaction_domains
                        .insert(interaction_domain.id);
                }
            }
        } else if !agent_suspended && self.previous_agent_suspended {
            for interaction_domain in
                std::mem::take(&mut self.automatically_paused_interaction_domains)
            {
                if self
                    .server
                    .resume_interaction_domain(interaction_domain)
                    .is_ok()
                {
                    self.interaction_domain_processes.resume(interaction_domain);
                }
            }
        }
        self.previous_agent_suspended = agent_suspended;
        if self.server.session_locked() {
            if let Some(pending) = self.pending_capture.take() {
                refuse_capture_target(
                    &self.capture_worker,
                    pending.target,
                    "session locked before capture completed".into(),
                    &self.journal,
                    &self.ipc,
                );
            }
            if let Some(pending) = self.pending_interaction_domain_capture.take() {
                refuse_capture_target(
                    &self.capture_worker,
                    CaptureTarget::InteractionDomainReply {
                        context: pending.context,
                        reply: pending.reply,
                    },
                    "session locked before Interaction Domain capture completed".into(),
                    &self.journal,
                    &self.ipc,
                );
            }
            if let Some(pending) = self.pending_window_capture.take() {
                refuse_capture_target(
                    &self.capture_worker,
                    CaptureTarget::WindowReply {
                        context: pending.context,
                        reply: pending.reply,
                    },
                    "session locked before window capture completed".into(),
                    &self.journal,
                    &self.ipc,
                );
            }
            // A pick presents screen content through chrome; the lock must
            // not cover a live picker (ADR-0054).
            self.abandon_pending_pick("session locked before the pick completed");
            self.abandon_pending_app_pick("session locked before the app pick completed");
            self.abandon_pending_secret_prompt("session locked before the secret prompt completed");
            self.abandon_pending_confirm_pick("session locked before the confirmation completed");
            self.abandon_pending_capability_pick(
                "session locked before the capability pick completed",
            );
        }
        while let Ok(completion) = self.capture_worker.completions.try_recv() {
            let finishes_reserved_job = completion.finishes_reserved_job();
            match completion {
                CaptureCompletion::ScreenshotEncoded {
                    path,
                    origin,
                    security_generation,
                    encoded,
                } => {
                    if self.capture_worker.permits(security_generation)
                        && screenshot_updates_human_clipboard(&origin)
                    {
                        match encoded {
                            Ok(png) => {
                                // Publish the immutable PNG immediately. The
                                // worker retains the same Arc while it performs
                                // the independent atomic file write.
                                if let Err(error) = self.server.set_clipboard_data_shared(
                                    aegis_model::interaction_domain::HUMAN_SEAT,
                                    vec![("image/png".to_owned(), png)],
                                ) {
                                    log::warn!(
                                        "screenshot clipboard publication failed for {path}: {error}"
                                    );
                                }
                            }
                            Err(error) => {
                                log::warn!("screenshot encoding failed for {path}: {error}");
                            }
                        }
                    }
                }
                CaptureCompletion::ScreenshotSaved {
                    path,
                    command,
                    ts_mono_ms,
                    origin,
                    security_generation,
                    png,
                    written,
                } => {
                    let result = if self.capture_worker.permits(security_generation) {
                        written.map(|()| {
                            // The durable file now exists. Refresh the early
                            // image-only selection with its URI convenience,
                            // sharing the same PNG allocation rather than
                            // copying the multi-megabyte payload again.
                            if screenshot_updates_human_clipboard(&origin)
                                && let Some(png) = png
                            {
                                let mut payloads = vec![("image/png".to_owned(), png)];
                                match screenshot_uri_list(&path) {
                                    Ok(uri_list) => payloads.push((
                                        "text/uri-list".to_owned(),
                                        std::sync::Arc::from(uri_list),
                                    )),
                                    Err(error) => {
                                        log::warn!("screenshot clipboard URI unavailable: {error}");
                                    }
                                }
                                if let Err(error) = self.server.set_clipboard_data_shared(
                                    aegis_model::interaction_domain::HUMAN_SEAT,
                                    payloads,
                                ) {
                                    // Saving remains successful. Clipboard
                                    // publication is a desktop convenience
                                    // and cannot rewrite an applied capture.
                                    log::warn!("screenshot clipboard publication failed: {error}");
                                }
                            }
                        })
                    } else {
                        Err("capture authority changed before delivery".into())
                    };
                    let effect = match result {
                        Ok(()) => {
                            log::info!("screenshot: wrote {path}");
                            aegis_ipc::Effect::Applied
                        }
                        Err(reason) => aegis_ipc::Effect::Refused { reason },
                    };
                    journal_effect_and_broadcast(
                        &self.journal,
                        &self.ipc,
                        ts_mono_ms,
                        origin,
                        command,
                        effect,
                    );
                }
                CaptureCompletion::Reply {
                    reply,
                    security_generation,
                    encoded,
                } => {
                    let result = if self.capture_worker.permits(security_generation) {
                        encoded
                    } else {
                        Err("capture authority changed before delivery".into())
                    };
                    let _ = reply.send(result);
                }
                CaptureCompletion::InteractionDomainReply {
                    reply,
                    security_generation,
                    observation_token,
                    encoded,
                } => {
                    let current_revision = self.server.interaction_domain_snapshot().revision;
                    let result = if !self.capture_worker.permits(security_generation) {
                        Err("Interaction Domain capture authority changed before delivery".into())
                    } else {
                        encoded.and_then(|capture| {
                            let captured_revision = capture.capture.revision;
                            (captured_revision == current_revision)
                                .then_some(capture)
                                .ok_or_else(|| {
                                    format!(
                                        "Interaction Domain authority changed before delivery \
                                         (captured r{captured_revision}, current r{current_revision})"
                                    )
                                })
                        })
                    };
                    let result = match result {
                        Ok(mut capture) => self
                            .observations
                            .refresh_for_delivery(&observation_token)
                            .map(|observation| {
                                capture.capture.observation = observation;
                                capture
                            }),
                        Err(reason) => {
                            self.observations.discard(&observation_token);
                            Err(reason)
                        }
                    };
                    let delivered_token = result
                        .as_ref()
                        .ok()
                        .map(|capture| capture.capture.observation.token.clone());
                    if reply.send(result).is_err()
                        && let Some(token) = delivered_token
                    {
                        self.observations.discard(&token);
                    }
                }
                CaptureCompletion::WindowReply {
                    reply,
                    security_generation,
                    encoded,
                } => {
                    let result = if self.capture_worker.permits(security_generation) {
                        encoded
                    } else {
                        Err("capture authority changed before delivery".into())
                    };
                    let _ = reply.send(result);
                }
                CaptureCompletion::Pixel {
                    reply,
                    security_generation,
                    picked,
                } => {
                    let result = if self.capture_worker.permits(security_generation) {
                        picked
                    } else {
                        Err("capture authority changed before delivery".into())
                    };
                    let _ = reply.send(result);
                }
                CaptureCompletion::Stream {
                    security_generation,
                    pixels,
                } => {
                    self.stream_job_in_flight = false;
                    if self.capture_worker.permits(security_generation)
                        && let Ok(frame) = pixels
                    {
                        self.deliver_stream_frame(frame);
                    }
                }
                CaptureCompletion::StreamWindow {
                    stream_id,
                    security_generation,
                    pixels,
                } => {
                    self.deliver_window_stream_frame(stream_id, security_generation, pixels);
                }
            }
            if finishes_reserved_job {
                self.capture_worker.release();
            }
        }
        if self.pending_capture.is_some() {
            let readiness = self.surface.read_pixels_ready().map_err(|error| {
                format!(
                    "shot readback readiness: {error}{}",
                    flux_last_error_detail()
                )
            });
            match readiness {
                Ok(false) => {}
                Ok(true) => {
                    let pending = self.pending_capture.take().expect("checked above");
                    let is_stream = matches!(pending.target, CaptureTarget::Stream);
                    match read_captured_pixels(&self.surface, pending.readback) {
                        Ok(capture) => {
                            // The stream lane is one frame deep: no new
                            // stream readback binds until this job's
                            // completion arrives.
                            if is_stream {
                                self.stream_job_in_flight = true;
                            }
                            queue_captured_pixels(
                                &self.capture_worker,
                                capture,
                                pending.target,
                                &self.journal,
                                &self.ipc,
                            );
                        }
                        Err(reason) => refuse_capture_target(
                            &self.capture_worker,
                            pending.target,
                            reason,
                            &self.journal,
                            &self.ipc,
                        ),
                    }
                }
                Err(reason) => {
                    let pending = self.pending_capture.take().expect("checked above");
                    refuse_capture_target(
                        &self.capture_worker,
                        pending.target,
                        reason,
                        &self.journal,
                        &self.ipc,
                    );
                }
            }
        }
        // Zero-copy streams (IPC protocol 25): deliver every slot frame whose
        // acquire fence signaled since the last iteration.
        self.poll_dmabuf_stream_fences();
        if self.pending_window_capture.is_some() {
            let readiness = self
                .pending_window_capture
                .as_ref()
                .expect("checked above")
                .surface
                .read_pixels_ready()
                .map_err(|error| {
                    format!(
                        "window capture readback readiness: {error}{}",
                        flux_last_error_detail()
                    )
                });
            match readiness {
                Ok(false) => {}
                Ok(true) => {
                    let pending = self.pending_window_capture.take().expect("checked above");
                    let reply_target = CaptureTarget::WindowReply {
                        context: pending.context,
                        reply: pending.reply,
                    };
                    // The per-capture offscreen target is a require_readback
                    // surface: flux keeps its staging surface-owned and refuses
                    // take_readback there, so copy the completed frame into an
                    // owned buffer instead of detaching it.
                    match read_captured_pixels_owned(&pending.surface, pending.readback) {
                        Ok(capture) => queue_captured_pixels(
                            &self.capture_worker,
                            capture,
                            reply_target,
                            &self.journal,
                            &self.ipc,
                        ),
                        Err(reason) => refuse_capture_target(
                            &self.capture_worker,
                            reply_target,
                            reason,
                            &self.journal,
                            &self.ipc,
                        ),
                    }
                }
                Err(reason) => {
                    let pending = self.pending_window_capture.take().expect("checked above");
                    refuse_capture_target(
                        &self.capture_worker,
                        CaptureTarget::WindowReply {
                            context: pending.context,
                            reply: pending.reply,
                        },
                        reason,
                        &self.journal,
                        &self.ipc,
                    );
                }
            }
        }
        // Per-window streams (ADR-0127): completed offscreen readbacks leave
        // for the capture worker's conversion lane here; their deliveries
        // arrive as `CaptureCompletion::StreamWindow` above.
        self.poll_window_stream_readbacks();
        if self.pending_interaction_domain_capture.is_some() {
            let interaction_domain = self
                .pending_interaction_domain_capture
                .as_ref()
                .expect("checked above")
                .context
                .interaction_domain;
            let readiness = self
                .interaction_domain_render_targets
                .get(&interaction_domain)
                .ok_or_else(|| {
                    format!(
                        "interaction_domain {} render target disappeared",
                        interaction_domain.0
                    )
                })
                .and_then(|target| {
                    target.surface.read_pixels_ready().map_err(|error| {
                        format!(
                            "interaction_domain {} readback readiness: {error}{}",
                            interaction_domain.0,
                            flux_last_error_detail()
                        )
                    })
                });
            match readiness {
                Ok(false) => {}
                Ok(true) => {
                    let pending = self
                        .pending_interaction_domain_capture
                        .take()
                        .expect("checked above");
                    let target = self
                        .interaction_domain_render_targets
                        .get(&interaction_domain)
                        .expect("readiness target disappeared");
                    let reply_target = CaptureTarget::InteractionDomainReply {
                        context: pending.context,
                        reply: pending.reply,
                    };
                    // The Interaction Domain render target is a require_readback
                    // offscreen surface: flux keeps its staging surface-owned and
                    // refuses take_readback there, so copy the completed frame into
                    // an owned buffer instead of detaching it.
                    match read_captured_pixels_owned(&target.surface, pending.readback) {
                        Ok(capture) => queue_captured_pixels(
                            &self.capture_worker,
                            capture,
                            reply_target,
                            &self.journal,
                            &self.ipc,
                        ),
                        Err(reason) => refuse_capture_target(
                            &self.capture_worker,
                            reply_target,
                            reason,
                            &self.journal,
                            &self.ipc,
                        ),
                    }
                }
                Err(reason) => {
                    let pending = self
                        .pending_interaction_domain_capture
                        .take()
                        .expect("checked above");
                    refuse_capture_target(
                        &self.capture_worker,
                        CaptureTarget::InteractionDomainReply {
                            context: pending.context,
                            reply: pending.reply,
                        },
                        reason,
                        &self.journal,
                        &self.ipc,
                    );
                }
            }
        }
        // libseat revokes DRM/input devices while another VT owns the seat.
        // The backend continues dispatching seat events but no rendering or
        // client input may occur until the enable event restores ownership.
        if !self.host.is_active() {
            return Ok(None);
        }
        while let Ok(mut detected) = self.status_rx.try_recv() {
            detected.do_not_disturb = self.notif_queue.lock().unwrap().do_not_disturb();
            detected.tiled = self.server.tiling();
            detected.idle_inhibited = self.system_status.idle_inhibited;
            // Compositor-owned like `idle_inhibited`: the live capture
            // stream count (ADR-0128) must survive a host status sample.
            detected.capture_streams = self.system_status.capture_streams;
            detected.touchpad = self.host.touchpad_status();
            detected.display = self.system_status.display.clone();
            if detected != self.system_status {
                self.system_status = detected;
                publish_system_status_parts(
                    &self.system_status,
                    &mut self.shell,
                    &self.live,
                    &self.ipc,
                );
                self.damage.chrome_dirty = true;
            }
            // Evaluate every drained sample, not only changed ones: a warning
            // skipped behind a lock or another modal retries on the next tick.
            self.poll_battery_warning();
        }
        while let Ok(stats) = self.resource_rx.try_recv() {
            self.shell.set_resource_stats(stats);
            self.damage.chrome_dirty = true;
        }
        let touchpad_status = self.host.touchpad_status();
        if touchpad_status != self.system_status.touchpad {
            self.system_status.touchpad = touchpad_status;
            self.publish_settings();
        }
        // Hot-reload the configuration when its mtime moves (ADR-0026). One
        // `stat` per frame is cheap and keeps the reload on this loop, where
        // the keymap rebuild must happen anyway. A failed reload keeps the
        // previous configuration rather than reverting silently.
        let wallpaper_before_reload = self
            .config
            .as_ref()
            .map(|config| config.wallpaper.clone())
            .unwrap_or_default();
        if let Some(path) = self.config_path.as_deref()
            && self.reload.as_mut().is_some_and(|w| w.changed(path))
            && reload_config(
                path,
                &mut self.config,
                &mut self.keymap,
                &mut self.gesture_map,
                &mut self.server,
                &mut self.shell,
                &mut self.cursor_cache,
            )
        {
            self.damage.chrome_dirty = true;
            // Built-in scope executable allowlists follow the reload
            // (ADR-0128): the next connection handshake resolves against
            // the fresh table.
            self.live.set_scope_executables(
                self.config
                    .as_ref()
                    .map(|config| config.ipc.scope_executables.clone())
                    .unwrap_or_default(),
            );
            let wallpaper_after_reload = self
                .config
                .as_ref()
                .map(|config| config.wallpaper.clone())
                .unwrap_or_default();
            if wallpaper_before_reload != wallpaper_after_reload && !wallpaper_source_overridden() {
                let size = self.host.physical_size();
                match load_wallpaper(
                    self.config.as_ref(),
                    self.config_path.as_deref(),
                    &self.device,
                    &self.surface,
                    size,
                    DEFAULT_WALLPAPER,
                ) {
                    Ok((wallpaper, label)) => {
                        self.wallpaper = Some(wallpaper);
                        self.damage.force_full_redraw = true;
                        log::info!("wallpaper: reloaded ({label})");
                    }
                    Err(error) => {
                        log::warn!(
                            "wallpaper: configured reload failed; keeping previous scene: {error}"
                        );
                    }
                }
            }
            if let Some(wallpaper) = self.wallpaper.as_mut() {
                wallpaper.set_reduced_motion(
                    self.config
                        .as_ref()
                        .is_some_and(|config| config.ui.reduced_motion),
                );
            }
            self.screenshot_dir = self
                .config
                .as_ref()
                .map(|c| std::path::PathBuf::from(&c.screenshot.save_dir))
                .unwrap_or_else(aegis_config::default_screenshot_dir);
            // Output follow-through: hand the backend the fresh mode
            // requests, then re-feed its current geometries so a policy
            // *removed* from the file reverts to the backend-reported value
            // instead of lingering in the live output set. Direct DRM queues
            // a live re-modeset after its in-flight page flip retires.
            self.host
                .set_configured_modes(configured_output_modes(self.config.as_ref()));
            self.host
                .set_configured_color_policies(configured_color_policies(self.config.as_ref()));
            self.host
                .set_configured_icc_profiles(configured_icc_profiles(self.config.as_ref()));
            self.system_status.touchpad = self.host.set_touchpad_config(
                self.config
                    .as_ref()
                    .map(|c| c.input.touchpad)
                    .unwrap_or_default(),
            );
            publish_system_status_parts(
                &self.system_status,
                &mut self.shell,
                &self.live,
                &self.ipc,
            );
            self.server.set_outputs(self.host.output_infos());
            if let Some(logical) = self
                .server
                .output_infos()
                .first()
                .map(|output| output.geometry.logical_size())
            {
                self.input_acc.display_size = (logical.w.max(1) as f32, logical.h.max(1) as f32);
            }
            self.system_status.display = aegis_shell::DisplayStatus {
                configurable: self.host.name() == "drm",
                outputs: self.server.output_infos(),
                error: None,
            };
            self.settings_revision = self.settings_revision.saturating_add(1);
            self.idle_process.reconfigure(
                self.config
                    .as_ref()
                    .map(|config| config.idle)
                    .unwrap_or_default(),
            );
            self.publish_settings();
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
                position: self
                    .config
                    .as_ref()
                    .map(|c| c.dock.position)
                    .unwrap_or_default(),
            });
            // Theme selection follows the same live-reload contract as the
            // rest of the effective desktop-preferences snapshot.
            self.queue_app_scan();
        }

        if app_scan_due(
            std::time::Instant::now(),
            self.next_app_scan,
            self.server.session_locked(),
        ) {
            self.queue_app_scan();
        }
        while let Ok((
            refreshed_theme,
            refreshed_scale,
            refreshed,
            refreshed_snapshot,
            refreshed_decoded,
        )) = self.scan_result_rx.try_recv()
        {
            let catalog_changed = refreshed != self.launcher_apps;
            let icons_changed = refreshed_snapshot != self.icon_snapshot;
            let theme_changed = refreshed_theme != self.icon_theme;
            let scale_changed = refreshed_scale != self.icon_scale;
            if catalog_changed || icons_changed || theme_changed || scale_changed {
                self.damage.chrome_dirty = true;
                log::info!(
                    "launcher: application catalog/icons changed ({} -> {}, theme {} -> {})",
                    self.launcher_apps.len(),
                    refreshed.len(),
                    self.icon_theme,
                    refreshed_theme
                );
                // The worker already decoded every icon off the frame loop;
                // only the GPU texture upload happens here.
                let refreshed_icons = build_icon_cache(&self.device, &refreshed_decoded);
                let pinned = resolve_chrome_pins(
                    &refreshed,
                    &refreshed_icons.map,
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
                    apps: refreshed.clone(),
                    pinned,
                    icons: aegis_shell::IconSet::from_raw(refreshed_icons.map.clone()),
                    position: self
                        .config
                        .as_ref()
                        .map(|c| c.dock.position)
                        .unwrap_or_default(),
                });
                // Components now point only at refreshed_icons; dropping the
                // old cache after the update cannot leave dangling textures.
                self.icon_cache = refreshed_icons;
            }
            if theme_changed && !catalog_changed && !icons_changed && !scale_changed {
                log::info!(
                    "launcher: icon theme changed ({} -> {}), resolved icons unchanged",
                    self.icon_theme,
                    refreshed_theme
                );
            }
            self.launcher_apps = refreshed;
            self.icon_snapshot = refreshed_snapshot;
            self.icon_theme = refreshed_theme;
            self.icon_scale = refreshed_scale;
        }

        // Expiry is timer-driven, not request-driven. Cascade it before
        // other IPC work so idle Actors lose observations, resource grants,
        // and semantic-provider authority promptly.
        self.live.expire_due_actor_sessions();
        self.drain_idle_controls();
        self.drain_pick_controls();
        self.drain_app_pick_controls();
        self.drain_secret_prompt_controls();
        self.drain_confirm_pick_controls();
        self.drain_capability_pick_controls();
        while let Ok(request) = self.stream_control_rx.try_recv() {
            match request.action {
                StreamControl::Start {
                    max_fps,
                    target,
                    allow_dmabuf,
                    cursor,
                    reply,
                } => {
                    let result = if self.server.session_locked() || !self.host.is_active() {
                        Err("session is locked or inactive".to_owned())
                    } else {
                        let (width, height) = self.surface.size();
                        match &target {
                            aegis_ipc::StreamTarget::Output { output } => {
                                // A connector selector (IPC protocol 29)
                                // resolves to that output's sub-region of
                                // the desktop frame; an unknown connector is
                                // an error, not a fallback.
                                let region = match output {
                                    None => Some(aegis_model::Rect::new(
                                        0,
                                        0,
                                        width as i32,
                                        height as i32,
                                    )),
                                    Some(connector) => resolve_output_rect(
                                        &self.server.output_infos(),
                                        connector,
                                        output_render_scale(&self.server, &self.host),
                                        width,
                                        height,
                                    )
                                    .filter(|rect| rect.size.w > 0 && rect.size.h > 0),
                                };
                                match (output, region) {
                                    (Some(connector), None) => {
                                        Err(format!("unknown output connector '{connector}'"))
                                    }
                                    (_, Some(region)) => {
                                        let size = (region.size.w as u32, region.size.h as u32);
                                        // The zero-copy dmabuf transport (protocol 25)
                                        // needs an exportable presentation surface;
                                        // every other case announces SHM pixels.
                                        let mut capture = None;
                                        if allow_dmabuf {
                                            match self.create_dmabuf_capture(size.0, size.1) {
                                                Ok(created) => capture = Some(created),
                                                Err(reason) => log::warn!(
                                                    "stream: dmabuf capture unavailable ({reason}); \
                                                     falling back to shm"
                                                ),
                                            }
                                        }
                                        match capture {
                                            Some(capture) => Ok(self.streams.start_dmabuf(
                                                request.conn_id,
                                                max_fps,
                                                size,
                                                target.clone(),
                                                cursor,
                                                capture,
                                            )),
                                            None => Ok(self.streams.start(
                                                request.conn_id,
                                                max_fps,
                                                size,
                                                target.clone(),
                                                cursor,
                                            )),
                                        }
                                    }
                                    (None, None) => unreachable!("whole-desktop region is sized"),
                                }
                            }
                            aegis_ipc::StreamTarget::Window { window } => {
                                // Per-window independent rendering
                                // (ADR-0127): the negotiated extent is the
                                // window's physical size at the capture
                                // scale of the output it sits on; the
                                // per-stream target is created now and
                                // cached until the stream stops.
                                match window_tree_geometry(&self.server, *window) {
                                    Err(reason) => Err(reason),
                                    Ok(geometry) => {
                                        let size =
                                            (geometry.physical_width, geometry.physical_height);
                                        let window_state = |shm| WindowStream {
                                            shm,
                                            geometry_sig: (
                                                self.server.all_windows_signature(),
                                                self.server.outputs_revision(),
                                            ),
                                            origin: geometry.origin,
                                            logical_size: geometry.logical_size,
                                            scale_milli: geometry.scale_milli,
                                            generations: std::collections::HashMap::new(),
                                            dirty: true,
                                            stage: WindowStreamStage::Idle,
                                            held_since: None,
                                        };
                                        let mut capture = None;
                                        if allow_dmabuf {
                                            match self.create_dmabuf_capture(size.0, size.1) {
                                                Ok(created) => capture = Some(created),
                                                Err(reason) => log::warn!(
                                                    "stream: dmabuf capture unavailable ({reason}); \
                                                     falling back to shm"
                                                ),
                                            }
                                        }
                                        match capture {
                                            Some(capture) => {
                                                let info = self.streams.start_dmabuf(
                                                    request.conn_id,
                                                    max_fps,
                                                    size,
                                                    target.clone(),
                                                    cursor,
                                                    capture,
                                                );
                                                self.streams.attach_window(
                                                    info.stream_id,
                                                    window_state(None),
                                                );
                                                Ok(info)
                                            }
                                            None => match WindowShmTarget::new(
                                                &self.device,
                                                size.0,
                                                size.1,
                                            ) {
                                                Ok(target_shm) => {
                                                    let info = self.streams.start(
                                                        request.conn_id,
                                                        max_fps,
                                                        size,
                                                        target.clone(),
                                                        cursor,
                                                    );
                                                    self.streams.attach_window(
                                                        info.stream_id,
                                                        window_state(Some(target_shm)),
                                                    );
                                                    Ok(info)
                                                }
                                                Err(reason) => Err(reason),
                                            },
                                        }
                                    }
                                }
                            }
                        }
                    };
                    if let Ok(info) = &result {
                        log::info!(
                            "stream {}: started for IPC connection {} ({}x{}, {:?})",
                            info.stream_id,
                            request.conn_id,
                            info.width,
                            info.height,
                            target
                        );
                    }
                    let _ = reply.send(result);
                }
                StreamControl::Stop { stream_id } => {
                    log::info!("stream {stream_id}: stopped");
                    self.streams.stop(stream_id);
                }
                StreamControl::ReleaseSlot { stream_id, slot } => {
                    self.streams.release_slot(stream_id, slot);
                }
                StreamControl::Disconnect => {
                    self.streams.disconnect(request.conn_id);
                }
            }
        }
        // Window streams produce independently of presentation (ADR-0127):
        // geometry re-checks, dirty-driven and liveness renders into each
        // stream's cached offscreen target.
        self.drive_window_streams();
        // The recording indicator follows the live stream set after every
        // mutation above (start, stop, disconnect) — the same frame the
        // change drained in, so it paints promptly (ADR-0128).
        self.publish_capture_stream_count();
        while let Ok(request) = self.interaction_domain_control_rx.try_recv() {
            let committed_action = request.action.clone();
            let before_revision = self.server.interaction_domain_snapshot().revision;
            let allowed_while_locked = match &request.action {
                aegis_ipc::InteractionDomainAction::Revoke { .. } => true,
                aegis_ipc::InteractionDomainAction::Transact { mutations, .. } => {
                    !mutations.is_empty()
                        && mutations.iter().all(|mutation| {
                            matches!(
                            mutation,
                            aegis_model::interaction_domain::InteractionDomainMutation::SetState {
                                state:
                                    aegis_model::interaction_domain::InteractionDomainState::Paused,
                                ..
                            }
                        )
                        })
                }
                aegis_ipc::InteractionDomainAction::Create { .. } => false,
            };
            let result = if self.server.session_locked() && !allowed_while_locked {
                Err("session is locked".into())
            } else {
                apply_interaction_domain_action(&mut self.server, request.subject, request.action)
            };
            if result.is_ok() {
                // Every observation carries the global Interaction Domain authority
                // revision. Revoke all outstanding leases at the same commit
                // boundary instead of retaining tokens that can only fail.
                self.observations.discard_all();
                // This path updates the interaction domain snapshot and `last_interaction_domain_revision`
                // ahead of the presentation fanout, so the signed revision
                // compare cannot see the change; flag the chrome explicitly.
                self.damage.chrome_dirty = true;
                for interaction_domain in interaction_domains_explicitly_stopped(&committed_action)
                {
                    self.automatically_paused_interaction_domains
                        .remove(&interaction_domain);
                }
                let invalidated = interaction_domain_action_invalidates_capture(&committed_action);
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
                    interaction_domain, ..
                } = &committed_action
                {
                    self.interaction_domain_render_targets
                        .remove(interaction_domain);
                }
                self.interaction_domain_processes
                    .apply_committed_action(&committed_action);
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
            let after_revision = self.server.interaction_domain_revision();
            let effect = match &result {
                Ok(_) => aegis_ipc::Effect::Applied,
                Err(reason) => aegis_ipc::Effect::Refused {
                    reason: reason.clone(),
                },
            };
            journal_mutation_effect_and_broadcast(
                &self.journal,
                &self.ipc,
                self.start.elapsed().as_millis() as u64,
                request.origin,
                aegis_ipc::JournalMutation::InteractionDomain {
                    action: committed_action,
                    before_revision,
                    after_revision,
                },
                effect,
            );
            let _ = request.reply.send(result);
        }
        while let Ok(provider) = self.semantic_provider_revocation_rx.try_recv() {
            self.server.revoke_semantic_provider(&provider);
        }
        while let Ok(request) = self.semantic_tree_update_rx.try_recv() {
            let result = self
                .server
                .publish_accessibility_tree(request.provider, request.update);
            let _ = request.reply.send(result);
        }
        while let Ok(request) = self.interaction_domain_observe_rx.try_recv() {
            let result = if self.server.session_locked() || !self.host.is_active() {
                Err("session is locked or inactive".into())
            } else {
                self.server
                    .interaction_domain_semantic_snapshot(request.interaction_domain)
                    .map_err(|error| error.to_string())
                    .and_then(|snapshot| {
                        self.observations.issue_bounded(
                            request.actor,
                            snapshot,
                            request.max_observations,
                        )
                    })
            };
            let issued_token = result
                .as_ref()
                .ok()
                .map(|observation| observation.token.clone());
            if request.reply.send(result).is_err()
                && let Some(token) = issued_token
            {
                self.observations.discard(&token);
            }
        }
        // Drain disconnects after observations: if an observe request and
        // EOF were queued in the same iteration, the just-issued token is
        // revoked before any action request can consume it.
        while let Ok(conn_id) = self.actor_disconnect_rx.try_recv() {
            self.observations.discard_connection(conn_id);
        }
        while let Ok(request) = self.observation_discard_rx.try_recv() {
            self.observations
                .discard_for_actor(&request.actor, &request.token);
        }
        let mut pending_index = 0;
        while pending_index < self.pending_semantic_actions.len() {
            let completion = self.pending_semantic_actions[pending_index]
                .completion
                .try_recv();
            let timed_out =
                std::time::Instant::now() >= self.pending_semantic_actions[pending_index].deadline;
            let resolved = match completion {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(Err(
                    "semantic provider disconnected before completion".into(),
                )),
                Err(std::sync::mpsc::TryRecvError::Empty) if timed_out => {
                    Some(Err("semantic action dispatch timed out".into()))
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
            };
            let Some(provider_result) = resolved else {
                pending_index += 1;
                continue;
            };
            let pending = self.pending_semantic_actions.swap_remove(pending_index);
            let result = provider_result.map(|()| aegis_ipc::ActorActionReceipt {
                action_id: pending.action_id,
                interaction_domain: pending.intent.interaction_domain,
                target: pending.intent.target,
                window: pending.window,
                authority_revision: pending.authority_revision,
                actions_applied: 1,
                committed_mono_ms: self.start.elapsed().as_millis() as u64,
            });
            let timestamp = result.as_ref().map_or_else(
                |_| self.start.elapsed().as_millis() as u64,
                |receipt| receipt.committed_mono_ms,
            );
            let effect = result.as_ref().map_or_else(
                |reason| aegis_ipc::Effect::Refused {
                    reason: reason.clone(),
                },
                |_| aegis_ipc::Effect::Applied,
            );
            journal_mutation_effect_and_broadcast(
                &self.journal,
                &self.ipc,
                timestamp,
                pending.origin,
                aegis_ipc::JournalMutation::ActorAction {
                    action_id: result.as_ref().ok().map(|receipt| receipt.action_id),
                    interaction_domain: pending.intent.interaction_domain,
                    target: pending.intent.target,
                    window: result.as_ref().ok().map(|receipt| receipt.window),
                    actions: aegis_ipc::audit_semantic_actions(&pending.intent.actions),
                    actions_truncated: false,
                    authority_revision: result
                        .as_ref()
                        .ok()
                        .map(|receipt| receipt.authority_revision),
                },
                effect,
            );
            let _ = pending.reply.send(result);
        }

        enum ActorActionDispatch {
            Immediate(aegis_ipc::ActorActionReceipt),
            Deferred(PendingSemanticActorAction),
        }

        while let Ok(request) = self.actor_action_rx.try_recv() {
            let dispatch: Result<ActorActionDispatch, String> = if self.server.session_locked()
                || !self.host.is_active()
            {
                self.observations.discard(&request.intent.observation);
                Err("session is locked or inactive".into())
            } else {
                (|| {
                    let current_scope = match self.live.revalidate_actor_action_scope(
                        request.scope_name.as_deref(),
                        &request.actor,
                        &request.scope,
                        request.intent.interaction_domain,
                    ) {
                        Ok(scope) => scope,
                        Err(error) => {
                            self.observations.discard(&request.intent.observation);
                            return Err(error);
                        }
                    };
                    let current = match self
                        .server
                        .interaction_domain_semantic_snapshot(request.intent.interaction_domain)
                    {
                        Ok(current) => current,
                        Err(error) => {
                            self.observations.discard(&request.intent.observation);
                            return Err(error.to_string());
                        }
                    };
                    let validated = self.observations.consume(
                        &request.actor,
                        &request.intent,
                        &current,
                        |window| current_scope.permits_window(window),
                    )?;
                    let interaction_domain_snapshot = self.server.interaction_domain_snapshot();
                    let interaction_domain_label = interaction_domain_snapshot
                        .interaction_domains
                        .iter()
                        .find(|interaction_domain| {
                            interaction_domain.id == request.intent.interaction_domain
                        })
                        .map(|interaction_domain| interaction_domain.label.clone())
                        .unwrap_or_else(|| {
                            format!("InteractionDomain {}", request.intent.interaction_domain.0)
                        });
                    if validated.source == aegis_model::semantic::SemanticSource::Accessibility {
                        let target = self
                            .server
                            .resolve_semantic_dispatch(request.intent.target)
                            .ok_or_else(|| {
                                "accessibility target lost its provider before dispatch".to_owned()
                            })?;
                        let action = request
                            .intent
                            .actions
                            .first()
                            .cloned()
                            .ok_or_else(|| "semantic action is empty".to_owned())?;
                        let completion = self.live.dispatch_accessibility_action(target, action)?;
                        return Ok(ActorActionDispatch::Deferred(PendingSemanticActorAction {
                            completion,
                            deadline: std::time::Instant::now() + std::time::Duration::from_secs(5),
                            origin: request.origin.clone(),
                            intent: request.intent.clone(),
                            action_id: validated.action_id,
                            window: validated.window,
                            authority_revision: validated.authority_revision,
                            reply: request.reply.clone(),
                        }));
                    }
                    let seat = interaction_domain_snapshot
                        .seats
                        .iter()
                        .find(|seat| {
                            seat.interaction_domain == request.intent.interaction_domain
                                && seat.enabled
                        })
                        .map(|seat| seat.id)
                        .ok_or_else(|| "InteractionDomain has no active seat".to_owned())?;
                    let synthetic_actions =
                        request
                            .intent
                            .actions
                            .iter()
                            .map(|action| match action {
                                aegis_model::semantic::SemanticActionIntent::SyntheticInput {
                                    actions,
                                } => Ok(actions.as_slice()),
                                _ => Err("application semantic action dispatch is unavailable"
                                    .to_owned()),
                            })
                            .collect::<Result<Vec<_>, _>>()?
                            .into_iter()
                            .flatten()
                            .cloned()
                            .collect::<Vec<_>>();
                    let events = self
                        .server
                        .prepare_agent_synthetic_input(seat, validated.window, &synthetic_actions)
                        .ok_or_else(|| {
                            "semantic action became invalid before input preparation".to_owned()
                        })?;
                    self.server
                        .forward_agent_input_to(seat, validated.window, &events)
                        .map_err(|error| error.to_string())?;
                    for activity in agent_activities_from_applied_input(
                        request.intent.interaction_domain,
                        &interaction_domain_label,
                        validated.window,
                        &synthetic_actions,
                        &events,
                        &mut self.agent_activity_sequence,
                    ) {
                        self.shell.report_agent_activity(activity);
                    }
                    Ok(ActorActionDispatch::Immediate(
                        aegis_ipc::ActorActionReceipt {
                            action_id: validated.action_id,
                            interaction_domain: request.intent.interaction_domain,
                            target: request.intent.target,
                            window: validated.window,
                            authority_revision: validated.authority_revision,
                            actions_applied: request.intent.actions.len() as u32,
                            committed_mono_ms: self.start.elapsed().as_millis() as u64,
                        },
                    ))
                })()
            };
            let result: Result<aegis_ipc::ActorActionReceipt, String> = match dispatch {
                Ok(ActorActionDispatch::Deferred(pending)) => {
                    self.pending_semantic_actions.push(pending);
                    continue;
                }
                Ok(ActorActionDispatch::Immediate(receipt)) => Ok(receipt),
                Err(reason) => Err(reason),
            };
            let ts = result.as_ref().map_or_else(
                |_| self.start.elapsed().as_millis() as u64,
                |receipt| receipt.committed_mono_ms,
            );
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
                request.origin,
                aegis_ipc::JournalMutation::ActorAction {
                    action_id: result.as_ref().ok().map(|receipt| receipt.action_id),
                    interaction_domain: request.intent.interaction_domain,
                    target: request.intent.target,
                    window: result.as_ref().ok().map(|receipt| receipt.window),
                    actions: aegis_ipc::audit_semantic_actions(&request.intent.actions),
                    actions_truncated: false,
                    authority_revision: result
                        .as_ref()
                        .ok()
                        .map(|receipt| receipt.authority_revision),
                },
                effect,
            );
            let _ = request.reply.send(result);
        }
        while let Ok(request) = self.settings_control_rx.try_recv() {
            let action = request.action.clone();
            let appearance_changed = matches!(
                &action,
                aegis_ipc::SettingsAction::SetDesktopPreferences { .. }
            );
            let before_revision = self.settings_revision;
            let result = if self.server.session_locked() {
                Err("session is locked".into())
            } else {
                self.commit_settings(request.expected_revision, request.action)
            };
            if result.is_ok() {
                // A committed settings action may redraw status chrome
                // outside the signed server-state paths.
                self.damage.chrome_dirty = true;
                if appearance_changed {
                    self.queue_app_scan();
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
                self.start.elapsed().as_millis() as u64,
                request.origin,
                aegis_ipc::JournalMutation::Settings {
                    action,
                    before_revision,
                    after_revision,
                },
                effect,
            );
            let _ = request.reply.send(result);
        }
        while let Ok(request) = self.wallpaper_control_rx.try_recv() {
            let result = if self.server.session_locked() {
                Err("session is locked".into())
            } else {
                self.swap_wallpaper(&request.path)
            };
            let _ = request.reply.send(result);
        }
        while let Ok(request) = self.system_control_rx.try_recv() {
            let action = request.action;
            // Correlate every event produced while applying this live-system
            // control request. `debug_span` is elided at the default `info`
            // level, so it is free in production and active under RUST_LOG.
            let _span = tracing::debug_span!("ipc", kind = "system_control", ?action).entered();
            let command = aegis_ipc::Command::System {
                action: action.clone(),
            };
            let allowed_while_locked =
                matches!(&action, aegis_ipc::SystemAction::SetOutputPower { .. });
            let result = if self.server.session_locked() && !allowed_while_locked {
                Err("session is locked".into())
            } else {
                apply_system_action(
                    &mut self.server,
                    &mut self.host,
                    &self.notif_queue,
                    &mut self.system_status,
                    &mut self.ipc_idle_inhibits,
                    action,
                )
            };
            if result.is_ok() {
                publish_system_status_parts(
                    &self.system_status,
                    &mut self.shell,
                    &self.live,
                    &self.ipc,
                );
                self.damage.chrome_dirty = true;
                let _ = self.status_refresh_tx.send(());
            }
            let effect = match &result {
                Ok(()) => aegis_ipc::Effect::Applied,
                Err(reason) => aegis_ipc::Effect::Refused {
                    reason: reason.clone(),
                },
            };
            journal_effect_and_broadcast(
                &self.journal,
                &self.ipc,
                self.start.elapsed().as_millis() as u64,
                request.origin,
                command,
                effect,
            );
            let _ = request.reply.send(result);
        }
        let mut pending_synthetic_input = Vec::new();
        let mut pending_screenshots = Vec::new();
        while let Ok(request) = self.ipc_cmd_rx.try_recv() {
            let cmd = request.command;
            let origin = request.origin;
            let ts = self.start.elapsed().as_millis() as u64;
            let allowed_while_locked = matches!(
                &cmd,
                aegis_ipc::Command::System {
                    action: aegis_ipc::SystemAction::SetOutputPower { .. }
                }
            );
            if self.server.session_locked() && !allowed_while_locked {
                journal_effect_and_broadcast(
                    &self.journal,
                    &self.ipc,
                    ts,
                    origin,
                    cmd,
                    aegis_ipc::Effect::Refused {
                        reason: "session is locked".into(),
                    },
                );
                continue;
            }
            if let aegis_ipc::Command::System { action } = &cmd {
                let effect = match apply_system_action(
                    &mut self.server,
                    &mut self.host,
                    &self.notif_queue,
                    &mut self.system_status,
                    &mut self.ipc_idle_inhibits,
                    action.clone(),
                ) {
                    Ok(()) => {
                        publish_system_status_parts(
                            &self.system_status,
                            &mut self.shell,
                            &self.live,
                            &self.ipc,
                        );
                        self.damage.chrome_dirty = true;
                        let _ = self.status_refresh_tx.send(());
                        aegis_ipc::Effect::Applied
                    }
                    Err(reason) => aegis_ipc::Effect::Refused { reason },
                };
                journal_effect_and_broadcast(&self.journal, &self.ipc, ts, origin, cmd, effect);
                continue;
            }
            if let aegis_ipc::Command::LaunchInInteractionDomain {
                interaction_domain,
                desktop_id,
            } = &cmd
            {
                let effect = match self
                    .launcher_apps
                    .iter()
                    .find(|entry| entry.id == *desktop_id)
                {
                    Some(entry) => {
                        let launched = (|| -> Result<aegis_launcher::ManagedLaunch, String> {
                            let portal = self
                                .server
                                .prepare_interaction_domain_portal(*interaction_domain)
                                .map_err(|error| error.to_string())?;
                            let wayland_listener = portal
                                .try_clone_listener()
                                .map_err(|error| error.to_string())?;
                            let wayland_socket_path = portal.path().to_path_buf();
                            let sandbox_policy = self
                                .config
                                .as_ref()
                                .map(|config| {
                                    config.interaction_domain_sandbox.policy_for(&entry.id)
                                })
                                .unwrap_or_else(|| {
                                    aegis_config::InteractionDomainSandboxConfig::default()
                                        .policy_for(&entry.id)
                                });
                            let opts = aegis_launcher::LaunchOpts {
                                sandbox: Some(aegis_launcher::InteractionDomainSandbox {
                                    interaction_domain_id: interaction_domain.0,
                                    wayland_listener,
                                    wayland_socket_path,
                                    app_id: entry.id.clone(),
                                    limits: aegis_launcher::InteractionDomainResourceLimits {
                                        memory_max_bytes: sandbox_policy.memory_max_bytes,
                                        pids_max: sandbox_policy.pids_max,
                                        cpu_weight: sandbox_policy.cpu_weight,
                                    },
                                }),
                                ..Default::default()
                            };
                            let launch = aegis_launcher::launch_managed(entry, &opts)
                                .map_err(|error| error.to_string())?;
                            self.server
                                .activate_interaction_domain_portal(portal)
                                .map_err(|error| error.to_string())?;
                            Ok(launch)
                        })();
                        match launched {
                            Ok(launch) => {
                                log::info!(
                                    "InteractionDomain {}: launched {} in sandbox cgroup (supervisor {})",
                                    interaction_domain.0,
                                    entry.id,
                                    launch.report().pid
                                );
                                self.interaction_domain_processes
                                    .insert(*interaction_domain, launch);
                                aegis_ipc::Effect::Applied
                            }
                            Err(reason) => aegis_ipc::Effect::Refused { reason },
                        }
                    }
                    None => aegis_ipc::Effect::Refused {
                        reason: format!("unknown desktop entry {desktop_id:?}"),
                    },
                };
                journal_effect_and_broadcast(&self.journal, &self.ipc, ts, origin, cmd, effect);
                continue;
            }
            // A plain desktop launch; the optional placement is held pending
            // and applied to the app's first mapped window (ADR-0118).
            if let aegis_ipc::Command::LaunchApp {
                desktop_id,
                placement,
            } = &cmd
            {
                let effect = match self
                    .launcher_apps
                    .iter()
                    .find(|entry| entry.id == *desktop_id)
                {
                    Some(entry) => {
                        let opts = aegis_launcher::LaunchOpts {
                            wayland_display: Some(self.server.socket().to_owned()),
                            ..Default::default()
                        };
                        match aegis_launcher::launch(entry, &opts) {
                            Ok(report) => {
                                log::info!("launcher: spawned {} (pid {})", entry.id, report.pid);
                                if let Some(placement) = placement {
                                    // Cover the common app_id spellings the
                                    // case-sensitive first-map matcher sees.
                                    let mut app_ids: Vec<String> = Vec::new();
                                    for candidate in [
                                        entry.startup_wm_class.clone(),
                                        desktop_id.strip_suffix(".desktop").map(str::to_string),
                                        Some(desktop_id.clone()),
                                    ]
                                    .into_iter()
                                    .flatten()
                                    {
                                        if !candidate.is_empty() && !app_ids.contains(&candidate) {
                                            app_ids.push(candidate);
                                        }
                                    }
                                    self.server.register_launch_placement(
                                        app_ids,
                                        Some(report.pid),
                                        placement.clone(),
                                    );
                                }
                                aegis_ipc::Effect::Applied
                            }
                            Err(error) => aegis_ipc::Effect::Refused {
                                reason: error.to_string(),
                            },
                        }
                    }
                    None => aegis_ipc::Effect::Refused {
                        reason: format!("unknown desktop entry {desktop_id:?}"),
                    },
                };
                journal_effect_and_broadcast(&self.journal, &self.ipc, ts, origin, cmd, effect);
                continue;
            }
            if matches!(cmd, aegis_ipc::Command::InjectInput { .. }) {
                pending_synthetic_input.push((cmd, ts, origin));
                continue;
            }
            // The overview lives in the shell; toggling it is a presentation
            // change, not a server mutation, but it still passes the journal.
            if matches!(cmd, aegis_ipc::Command::ToggleOverview) {
                self.shell.toggle_overview();
                journal_and_broadcast(&self.journal, &self.ipc, ts, origin, cmd);
                continue;
            }
            // Screenshots render with the GPU objects below, not in the
            // generic command path.
            if matches!(cmd, aegis_ipc::Command::Screenshot { .. }) {
                pending_screenshots.push(PendingScreenshot {
                    command: cmd,
                    ts_mono_ms: ts,
                    origin,
                    cursor: self.capture_cursor_state(),
                });
                continue;
            }
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
        // Pre-authorized transaction batches (ADR-0125): preconditions are
        // checked at this commit boundary, then ops apply in order through
        // the same chokepoint as `Do` and each returns its own journal
        // sequence number and effect as the receipt.
        while let Ok(request) = self.transact_rx.try_recv() {
            let result = apply_transact_batch(
                &mut self.server,
                &self.notif_queue,
                &mut self.quit_requested,
                &self.ipc,
                &self.journal,
                self.start.elapsed().as_millis() as u64,
                request.origin,
                request.expected_journal_seq,
                request.expected_interaction_domain_revision,
                request.ops,
            );
            let _ = request.reply.send(result);
        }
        // Age out expired notifications once per frame.
        self.notif_queue
            .lock()
            .unwrap()
            .expire(self.start.elapsed().as_millis() as u64);

        Ok(Some(IterationWork {
            pending_synthetic_input,
            pending_screenshots,
        }))
    }

    /// Decode `path` and swap the live wallpaper (the Wallpaper portal's
    /// compositor side). glTF stays startup-only: its decode needs the GPU
    /// device and surface, which the IPC path deliberately does not touch.
    /// Night light: pace a 1 Hz evaluation of the configured schedule and
    /// fade the CRTC gamma ramp toward the target temperature. This is
    /// KMS-level channel gain — the render pipeline is untouched.
    pub(super) fn evaluate_night_light(&mut self) {
        use aegis_model::night_light as nl;
        if self.night_light_last_eval.elapsed() < std::time::Duration::from_secs(1) {
            return;
        }
        self.night_light_last_eval = std::time::Instant::now();
        let config = self
            .config
            .as_ref()
            .map(|c| c.night_light.clone())
            .unwrap_or_default();
        let scheduled = match (&config.start, &config.end) {
            (Some(start), Some(end)) => {
                match (
                    nl::ClockTime::from_hhmm(start),
                    nl::ClockTime::from_hhmm(end),
                ) {
                    (Some(start), Some(end)) => nl::schedule_active(start, end, local_clock_now()),
                    _ => false,
                }
            }
            // No schedule: the enable switch alone decides.
            _ => true,
        };
        let target = (config.enable && scheduled).then_some(config.temperature as f32);
        let fade_seconds = config.fade_seconds.max(1) as f32;
        let step = ((nl::NEUTRAL_KELVIN - config.temperature as f32).abs() / fade_seconds).max(1.0);
        if let Some(gains) = self.night_light.step(target, step) {
            self.host
                .set_gamma_gains(self.night_light.active.then_some(gains));
        }
    }

    pub(super) fn swap_wallpaper(&mut self, path: &std::path::Path) -> Result<(), String> {
        let is_gltf = path
            .extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("glb"));
        if is_gltf {
            return Err("glTF wallpapers are not supported over IPC".into());
        }
        let (width, height) = self.host.physical_size();
        let wallpaper = aegis_wallpaper::Wallpaper::from_path(path, width, height)
            .map_err(|error| format!("could not decode {}: {error}", path.display()))?;
        self.wallpaper = Some(wallpaper);
        // Full-output repaint for the swap: the damage path has no dedicated
        // wallpaper hook, so the out-of-band mutation signal stands in (the
        // same one config reloads and IPC settings use).
        self.damage.chrome_dirty = true;
        log::info!(
            "compositor: wallpaper replaced via IPC ({})",
            path.display()
        );
        Ok(())
    }
}

fn app_scan_due(
    now: std::time::Instant,
    next_scan: std::time::Instant,
    session_locked: bool,
) -> bool {
    !session_locked && now >= next_scan
}

/// User-initiated compositor surfaces may update the physical clipboard.
/// IPC and internal captures remain side-effect-free across the Interaction Domain boundary.
pub(super) fn screenshot_updates_human_clipboard(origin: &aegis_ipc::Origin) -> bool {
    matches!(
        origin,
        aegis_ipc::Origin::Chrome | aegis_ipc::Origin::Keybinding
    )
}

/// Local wall-clock time for the night-light schedule.
fn local_clock_now() -> aegis_model::night_light::ClockTime {
    unsafe {
        let now = libc::time(std::ptr::null_mut());
        let mut tm: libc::tm = std::mem::zeroed();
        libc::localtime_r(&now, &mut tm);
        aegis_model::night_light::ClockTime {
            minutes: (tm.tm_hour as u32) * 60 + tm.tm_min as u32,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn periodic_app_scan_is_suppressed_while_locked() {
        let now = std::time::Instant::now();
        let due = now - APP_RESCAN_INTERVAL;
        assert!(app_scan_due(now, due, false));
        assert!(!app_scan_due(now, due, true));
        assert!(!app_scan_due(now, now + APP_RESCAN_INTERVAL, false));
    }
}
