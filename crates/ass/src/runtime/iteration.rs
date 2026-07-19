use super::*;

const APP_RESCAN_INTERVAL: std::time::Duration = std::time::Duration::from_secs(5);

pub(super) struct IterationWork {
    pub(super) frame_dt: f32,
    pub(super) pending_synthetic_input: Vec<(ass_ipc::Command, u64, ass_ipc::Origin)>,
    pub(super) pending_screenshots: Vec<(ass_ipc::Command, u64, ass_ipc::Origin)>,
}

impl CompositorRuntime {
    pub(super) fn prepare_iteration(
        &mut self,
    ) -> Result<Option<IterationWork>, Box<dyn std::error::Error>> {
        // Process security-sensitive protocol transitions before completing
        // any pixel readback. In particular, an ext-session-lock request that
        // woke this iteration must become visible before a pending Realm or
        // desktop capture can be handed to its requester.
        self.server.dispatch();
        self.capture_worker
            .set_allowed(!self.server.session_locked() && self.host.is_active());
        let agent_suspended = self.server.session_locked() || !self.host.is_active();
        if agent_suspended && !self.previous_agent_suspended {
            let snapshot = self.server.realm_snapshot();
            for realm in snapshot.realms.into_iter().filter(|realm| {
                realm.kind == ass_core::realm::RealmKind::Agent
                    && realm.state == ass_core::realm::RealmState::Active
            }) {
                if self.server.pause_realm(realm.id).is_ok() {
                    self.realm_processes.pause(realm.id);
                    self.automatically_paused_realms.insert(realm.id);
                }
            }
        } else if !agent_suspended && self.previous_agent_suspended {
            for realm in std::mem::take(&mut self.automatically_paused_realms) {
                if self.server.resume_realm(realm).is_ok() {
                    self.realm_processes.resume(realm);
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
            if let Some(pending) = self.pending_realm_capture.take() {
                refuse_capture_target(
                    &self.capture_worker,
                    CaptureTarget::RealmReply {
                        context: pending.context,
                        reply: pending.reply,
                    },
                    "session locked before Realm capture completed".into(),
                    &self.journal,
                    &self.ipc,
                );
            }
        }
        while let Ok(completion) = self.capture_worker.completions.try_recv() {
            match completion {
                CaptureCompletion::Screenshot {
                    path,
                    command,
                    ts_mono_ms,
                    origin,
                    security_generation,
                    encoded,
                } => {
                    let result = if self.capture_worker.permits(security_generation) {
                        encoded.and_then(|png| {
                            atomic_write_capture(&path, &png)?;
                            if screenshot_updates_human_clipboard(origin) {
                                let mut payloads = vec![("image/png".to_owned(), png)];
                                match screenshot_uri_list(&path) {
                                    Ok(uri_list) => {
                                        payloads.push(("text/uri-list".to_owned(), uri_list));
                                    }
                                    Err(error) => {
                                        log::warn!("screenshot clipboard URI unavailable: {error}");
                                    }
                                }
                                if let Err(error) = self
                                    .server
                                    .set_clipboard_data(ass_core::realm::HUMAN_SEAT, payloads)
                                {
                                    // Saving the screenshot remains successful. Clipboard
                                    // publication is an additional desktop convenience and
                                    // must not rewrite an already-applied capture as refused.
                                    log::warn!("screenshot clipboard publication failed: {error}");
                                }
                            }
                            Ok(())
                        })
                    } else {
                        Err("capture authority changed before delivery".into())
                    };
                    let effect = match result {
                        Ok(()) => {
                            log::info!("screenshot: wrote {path}");
                            ass_ipc::Effect::Applied
                        }
                        Err(reason) => ass_ipc::Effect::Refused { reason },
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
                CaptureCompletion::RealmReply {
                    reply,
                    security_generation,
                    encoded,
                } => {
                    let current_revision = self.server.realm_snapshot().revision;
                    let result = if !self.capture_worker.permits(security_generation) {
                        Err("Realm capture authority changed before delivery".into())
                    } else {
                        encoded.and_then(|capture| {
                            let captured_revision = capture.capture.revision;
                            (captured_revision == current_revision)
                                .then_some(capture)
                                .ok_or_else(|| {
                                    format!(
                                        "Realm authority changed before delivery \
                                         (captured r{}, current r{current_revision})",
                                        captured_revision
                                    )
                                })
                        })
                    };
                    let _ = reply.send(result);
                }
            }
            self.capture_worker.release();
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
                    match read_captured_pixels(&self.surface, pending.readback) {
                        Ok(capture) => queue_captured_pixels(
                            &self.capture_worker,
                            capture,
                            pending.target,
                            &self.journal,
                            &self.ipc,
                        ),
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
        if self.pending_realm_capture.is_some() {
            let realm = self
                .pending_realm_capture
                .as_ref()
                .expect("checked above")
                .context
                .realm;
            let readiness = self
                .realm_render_targets
                .get(&realm)
                .ok_or_else(|| format!("realm {} render target disappeared", realm.0))
                .and_then(|target| {
                    target.surface.read_pixels_ready().map_err(|error| {
                        format!(
                            "realm {} readback readiness: {error}{}",
                            realm.0,
                            flux_last_error_detail()
                        )
                    })
                });
            match readiness {
                Ok(false) => {}
                Ok(true) => {
                    let pending = self.pending_realm_capture.take().expect("checked above");
                    let target = self
                        .realm_render_targets
                        .get(&realm)
                        .expect("readiness target disappeared");
                    let reply_target = CaptureTarget::RealmReply {
                        context: pending.context,
                        reply: pending.reply,
                    };
                    match read_captured_pixels(&target.surface, pending.readback) {
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
                    let pending = self.pending_realm_capture.take().expect("checked above");
                    refuse_capture_target(
                        &self.capture_worker,
                        CaptureTarget::RealmReply {
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
        let frame_at = std::time::Instant::now();
        let frame_dt = (frame_at - self.previous_frame_at)
            .as_secs_f32()
            .clamp(0.0, 1.0 / 15.0);
        self.previous_frame_at = frame_at;

        while let Ok(mut detected) = self.status_rx.try_recv() {
            detected.do_not_disturb = self.notif_queue.lock().unwrap().do_not_disturb();
            detected.tiled = self.server.tiling();
            detected.touchpad = self.host.touchpad_status();
            detected.display = self.system_status.display.clone();
            if detected != self.system_status {
                self.system_status = detected;
                self.shell.set_system_status(self.system_status.clone());
            }
        }
        let touchpad_status = self.host.touchpad_status();
        if touchpad_status != self.system_status.touchpad {
            self.system_status.touchpad = touchpad_status;
            self.shell.set_system_status(self.system_status.clone());
        }
        // Hot-reload the configuration when its mtime moves (ADR-0026). One
        // `stat` per frame is cheap and keeps the reload on this loop, where
        // the keymap rebuild must happen anyway. A failed reload keeps the
        // previous configuration rather than reverting silently.
        if let Some(path) = self.config_path.as_deref()
            && self.reload.as_mut().is_some_and(|w| w.changed(path))
            && reload_config(
                path,
                &mut self.config,
                &mut self.keymap,
                &mut self.server,
                &mut self.shell,
                &mut self.cursor_cache,
            )
        {
            self.screenshot_dir = self
                .config
                .as_ref()
                .map(|c| std::path::PathBuf::from(&c.screenshot.save_dir))
                .unwrap_or_else(ass_config::default_screenshot_dir);
            // Output follow-through: hand the backend the fresh mode
            // requests, then re-feed its current geometries so a policy
            // *removed* from the file reverts to the backend-reported value
            // instead of lingering in the live output set. Direct DRM queues
            // a live re-modeset after its in-flight page flip retires.
            self.host
                .set_configured_modes(configured_output_modes(self.config.as_ref()));
            self.system_status.touchpad = self.host.set_touchpad_config(
                self.config
                    .as_ref()
                    .map(|c| c.input.touchpad)
                    .unwrap_or_default(),
            );
            self.shell.set_system_status(self.system_status.clone());
            self.server.set_outputs(self.host.output_infos());
            if let Some(logical) = self
                .server
                .output_infos()
                .first()
                .map(|output| output.geometry.logical_size())
            {
                self.input_acc.display_size = (logical.w.max(1) as f32, logical.h.max(1) as f32);
            }
            self.system_status.display = ass_shell::DisplayStatus {
                configurable: self.host.name() == "drm",
                outputs: self.server.output_infos(),
                error: None,
            };
            self.live.set_scopes(build_ipc_scopes(self.config.as_ref()));
            let pinned = resolve_pinned(
                &self.launcher_apps,
                &self.icon_cache.map,
                self.config
                    .as_ref()
                    .map(|c| c.dock.pinned.as_slice())
                    .unwrap_or(&[]),
                self.config
                    .as_ref()
                    .map(|c| c.dock.autopopulate)
                    .unwrap_or(true),
            );
            self.shell.set_app_catalog(ass_shell::AppCatalog {
                apps: self.launcher_apps.clone(),
                pinned,
                icons: ass_shell::IconSet::from_raw(self.icon_cache.map.clone()),
            });
        }

        if std::time::Instant::now() >= self.next_app_scan {
            self.next_app_scan = std::time::Instant::now() + APP_RESCAN_INTERVAL;
            self.pending_scan_scale = self.host.scale().ceil().max(1.0) as u32;
            let _ = self.scan_req_tx.send(self.pending_scan_scale);
        }
        while let Ok((refreshed_theme, refreshed, refreshed_snapshot)) =
            self.scan_result_rx.try_recv()
        {
            let refreshed_scale = self.pending_scan_scale;
            let catalog_changed = refreshed != self.launcher_apps;
            let icons_changed = refreshed_snapshot != self.icon_snapshot;
            let theme_changed = refreshed_theme != self.icon_theme;
            let scale_changed = refreshed_scale != self.icon_scale;
            if catalog_changed || icons_changed || theme_changed || scale_changed {
                log::info!(
                    "launcher: application catalog/icons changed ({} -> {}, theme {} -> {})",
                    self.launcher_apps.len(),
                    refreshed.len(),
                    self.icon_theme,
                    refreshed_theme
                );
                let refreshed_icons =
                    build_icon_cache(&self.device, &refreshed, &refreshed_theme, refreshed_scale);
                let pinned = resolve_pinned(
                    &refreshed,
                    &refreshed_icons.map,
                    self.config
                        .as_ref()
                        .map(|c| c.dock.pinned.as_slice())
                        .unwrap_or(&[]),
                    self.config
                        .as_ref()
                        .map(|c| c.dock.autopopulate)
                        .unwrap_or(true),
                );
                self.shell.set_app_catalog(ass_shell::AppCatalog {
                    apps: refreshed.clone(),
                    pinned,
                    icons: ass_shell::IconSet::from_raw(refreshed_icons.map.clone()),
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

        // Drain IPC control/session commands and apply them here on the main
        // loop — the Wayland server state is not `Send`, so connection
        // threads forward through the channel rather than touching it
        // directly. Mirrors the chrome-intent drain below (ADR-0016/0027).
        for request in self.journal_refusal_rx.try_iter() {
            journal_mutation_effect_and_broadcast(
                &self.journal,
                &self.ipc,
                self.start.elapsed().as_millis() as u64,
                request.origin,
                request.mutation,
                ass_ipc::Effect::Refused {
                    reason: request.reason,
                },
            );
        }
        while let Ok(request) = self.realm_control_rx.try_recv() {
            let committed_action = request.action.clone();
            let before_revision = self.server.realm_snapshot().revision;
            let allowed_while_locked = match &request.action {
                ass_ipc::RealmAction::Revoke { .. } => true,
                ass_ipc::RealmAction::Transact { mutations, .. } => {
                    !mutations.is_empty()
                        && mutations.iter().all(|mutation| {
                            matches!(
                                mutation,
                                ass_core::realm::RealmMutation::SetState {
                                    state: ass_core::realm::RealmState::Paused,
                                    ..
                                }
                            )
                        })
                }
                ass_ipc::RealmAction::Create { .. } => false,
            };
            let result = if self.server.session_locked() && !allowed_while_locked {
                Err("session is locked".into())
            } else {
                apply_realm_action(&mut self.server, request.action)
            };
            if result.is_ok() {
                for realm in realms_explicitly_stopped(&committed_action) {
                    self.automatically_paused_realms.remove(&realm);
                }
                let invalidated = realm_action_invalidates_capture(&committed_action);
                if !invalidated.is_empty() {
                    self.capture_worker.invalidate_security_context();
                    if self
                        .pending_realm_capture
                        .as_ref()
                        .is_some_and(|pending| invalidated.contains(&pending.context.realm))
                    {
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
                if let ass_ipc::RealmAction::Revoke { realm, .. } = &committed_action {
                    self.realm_render_targets.remove(realm);
                }
                self.realm_processes
                    .apply_committed_action(&committed_action);
                let snapshot = self.server.realm_snapshot();
                self.last_realm_revision = Some(snapshot.revision);
                self.live.set_realms(snapshot.clone());
                if let Some(ipc) = &self.ipc {
                    ipc.broadcast(ass_ipc::Event::RealmsChanged {
                        revision: snapshot.revision,
                    });
                }
            }
            let after_revision = self.server.realm_snapshot().revision;
            let effect = match &result {
                Ok(_) => ass_ipc::Effect::Applied,
                Err(reason) => ass_ipc::Effect::Refused {
                    reason: reason.clone(),
                },
            };
            journal_mutation_effect_and_broadcast(
                &self.journal,
                &self.ipc,
                self.start.elapsed().as_millis() as u64,
                request.origin,
                ass_ipc::JournalMutation::Realm {
                    action: committed_action,
                    before_revision,
                    after_revision,
                },
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
            if self.server.session_locked() {
                journal_effect_and_broadcast(
                    &self.journal,
                    &self.ipc,
                    ts,
                    origin,
                    cmd,
                    ass_ipc::Effect::Refused {
                        reason: "session is locked".into(),
                    },
                );
                continue;
            }
            if let ass_ipc::Command::LaunchInRealm { realm, desktop_id } = &cmd {
                let effect = match self
                    .launcher_apps
                    .iter()
                    .find(|entry| entry.id == *desktop_id)
                {
                    Some(entry) => {
                        let launched = (|| -> Result<ass_launch::ManagedLaunch, String> {
                            let portal = self
                                .server
                                .prepare_realm_portal(*realm)
                                .map_err(|error| error.to_string())?;
                            let wayland_listener = portal
                                .try_clone_listener()
                                .map_err(|error| error.to_string())?;
                            let wayland_socket_path = portal.path().to_path_buf();
                            let sandbox_policy = self
                                .config
                                .as_ref()
                                .map(|config| config.realm_sandbox.policy_for(&entry.id))
                                .unwrap_or_else(|| {
                                    ass_config::RealmSandboxConfig::default().policy_for(&entry.id)
                                });
                            let opts = ass_launch::LaunchOpts {
                                sandbox: Some(ass_launch::RealmSandbox {
                                    realm_id: realm.0,
                                    wayland_listener,
                                    wayland_socket_path,
                                    app_id: entry.id.clone(),
                                    network: sandbox_policy.network,
                                    writable_paths: sandbox_policy.writable_paths,
                                    readable_paths: sandbox_policy.readable_paths,
                                    limits: ass_launch::RealmResourceLimits {
                                        memory_max_bytes: sandbox_policy.memory_max_bytes,
                                        pids_max: sandbox_policy.pids_max,
                                        cpu_weight: sandbox_policy.cpu_weight,
                                    },
                                }),
                                ..Default::default()
                            };
                            let launch = ass_launch::launch_managed(entry, &opts)
                                .map_err(|error| error.to_string())?;
                            self.server
                                .activate_realm_portal(portal)
                                .map_err(|error| error.to_string())?;
                            Ok(launch)
                        })();
                        match launched {
                            Ok(launch) => {
                                log::info!(
                                    "Realm {}: launched {} in sandbox cgroup (supervisor {})",
                                    realm.0,
                                    entry.id,
                                    launch.report().pid
                                );
                                self.realm_processes.insert(*realm, launch);
                                ass_ipc::Effect::Applied
                            }
                            Err(reason) => ass_ipc::Effect::Refused { reason },
                        }
                    }
                    None => ass_ipc::Effect::Refused {
                        reason: format!("unknown desktop entry {desktop_id:?}"),
                    },
                };
                journal_effect_and_broadcast(&self.journal, &self.ipc, ts, origin, cmd, effect);
                continue;
            }
            if matches!(
                cmd,
                ass_ipc::Command::InjectInput { .. } | ass_ipc::Command::InjectRealmInput { .. }
            ) {
                pending_synthetic_input.push((cmd, ts, origin));
                continue;
            }
            // The overview lives in the shell; toggling it is a presentation
            // change, not a server mutation, but it still passes the journal.
            if matches!(cmd, ass_ipc::Command::ToggleOverview) {
                self.shell.toggle_overview();
                journal_and_broadcast(&self.journal, &self.ipc, ts, origin, cmd);
                continue;
            }
            // Screenshots render with the GPU objects below, not in the
            // generic command path.
            if matches!(cmd, ass_ipc::Command::Screenshot { .. }) {
                pending_screenshots.push((cmd, ts, origin));
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
        // Age out expired notifications once per frame.
        self.notif_queue
            .lock()
            .unwrap()
            .expire(self.start.elapsed().as_millis() as u64);

        Ok(Some(IterationWork {
            frame_dt,
            pending_synthetic_input,
            pending_screenshots,
        }))
    }
}

/// User-initiated compositor surfaces may update the physical clipboard.
/// IPC and internal captures remain side-effect-free across the Realm boundary.
pub(super) fn screenshot_updates_human_clipboard(origin: ass_ipc::Origin) -> bool {
    matches!(
        origin,
        ass_ipc::Origin::Chrome | ass_ipc::Origin::Keybinding
    )
}
