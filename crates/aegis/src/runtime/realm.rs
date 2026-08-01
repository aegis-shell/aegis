use super::*;

/// One pixel-capture request from an IPC connection thread, answered by the
/// main loop after it copies the exact output frame being submitted.
pub(super) struct CaptureRequest {
    pub(super) reply: std::sync::mpsc::Sender<Result<aegis_ipc::CaptureOutputPayload, String>>,
    /// Logical-pixel region to capture, or `None` for the full output.
    pub(super) region: Option<aegis_core::Rect>,
}

pub(super) struct RealmCaptureRequest {
    pub(super) realm: aegis_core::realm::RealmId,
    pub(super) reply: std::sync::mpsc::Sender<Result<aegis_ipc::CaptureRealmPayload, String>>,
    pub(super) region: Option<aegis_core::Rect>,
}

pub(super) struct IpcCommandRequest {
    pub(super) origin: aegis_ipc::Origin,
    pub(super) command: aegis_ipc::Command,
}

/// One live-system mutation that must return the main loop's authoritative
/// apply result rather than only acknowledging IPC queueing.
pub(super) struct SystemControlRequest {
    pub(super) origin: aegis_ipc::Origin,
    pub(super) action: aegis_ipc::SystemAction,
    pub(super) reply: std::sync::mpsc::Sender<Result<(), String>>,
}

pub(super) struct RealmControlRequest {
    pub(super) origin: aegis_ipc::Origin,
    /// Credential-bound agent principal. `None` denotes a trusted built-in
    /// or compositor-local caller.
    pub(super) subject: Option<String>,
    pub(super) action: aegis_ipc::RealmAction,
    pub(super) reply: std::sync::mpsc::Sender<Result<aegis_ipc::RealmActionResult, String>>,
}

pub(super) struct SettingsControlRequest {
    pub(super) origin: aegis_ipc::Origin,
    pub(super) expected_revision: Option<u64>,
    pub(super) action: aegis_ipc::SettingsAction,
    pub(super) reply: std::sync::mpsc::Sender<Result<aegis_ipc::SettingsReceipt, String>>,
}

/// One wallpaper mutation from an IPC connection thread, applied on the
/// main loop; the reply carries the authoritative decode-and-swap receipt.
pub(super) struct WallpaperControlRequest {
    pub(super) path: std::path::PathBuf,
    pub(super) reply: std::sync::mpsc::Sender<Result<(), String>>,
}

pub(super) struct JournalRefusalRequest {
    pub(super) origin: aegis_ipc::Origin,
    pub(super) mutation: aegis_ipc::JournalMutation,
    pub(super) reason: String,
}

/// One positive agent-authorization lifecycle event (ADR-0088) from an IPC
/// connection thread, journaled on the main loop with `Effect::Applied` —
/// the applied counterpart of [`JournalRefusalRequest`].
pub(super) struct AuthEventRequest {
    pub(super) origin: aegis_ipc::Origin,
    pub(super) mutation: aegis_ipc::JournalMutation,
}

#[derive(Default)]
pub(super) struct RealmProcesses {
    launches:
        std::collections::BTreeMap<aegis_core::realm::RealmId, Vec<aegis_launcher::ManagedLaunch>>,
}

impl RealmProcesses {
    pub(super) fn insert(
        &mut self,
        realm: aegis_core::realm::RealmId,
        launch: aegis_launcher::ManagedLaunch,
    ) {
        self.launches.entry(realm).or_default().push(launch);
    }

    pub(super) fn pause(&mut self, realm: aegis_core::realm::RealmId) {
        if let Some(launches) = self.launches.get_mut(&realm) {
            launches.retain_mut(|launch| {
                if let Err(error) = launch.pause() {
                    log::error!(
                        "Realm {} sandbox {} could not be paused; terminating: {error}",
                        realm.0,
                        launch.report().pid
                    );
                    false
                } else {
                    true
                }
            });
        }
    }

    pub(super) fn resume(&mut self, realm: aegis_core::realm::RealmId) {
        if let Some(launches) = self.launches.get_mut(&realm) {
            launches.retain_mut(|launch| {
                if let Err(error) = launch.resume() {
                    log::error!(
                        "Realm {} sandbox {} could not be resumed; terminating: {error}",
                        realm.0,
                        launch.report().pid
                    );
                    false
                } else {
                    true
                }
            });
        }
    }

    pub(super) fn revoke(&mut self, realm: aegis_core::realm::RealmId) {
        // Dropping ManagedLaunch kills the complete sandbox cgroup and reaps
        // bubblewrap before this method returns.
        self.launches.remove(&realm);
    }

    pub(super) fn apply_committed_action(&mut self, action: &aegis_ipc::RealmAction) {
        match action {
            aegis_ipc::RealmAction::Create { .. } => {}
            aegis_ipc::RealmAction::Transact { mutations, .. } => {
                for mutation in mutations {
                    if let aegis_core::realm::RealmMutation::SetState { realm, state } = mutation {
                        match state {
                            aegis_core::realm::RealmState::Active => self.resume(*realm),
                            aegis_core::realm::RealmState::Paused => self.pause(*realm),
                            aegis_core::realm::RealmState::Revoked => self.revoke(*realm),
                        }
                    }
                }
            }
            aegis_ipc::RealmAction::Revoke { realm, .. } => self.revoke(*realm),
        }
    }
}

pub(super) struct RealmRenderTarget {
    pub(super) output: aegis_core::realm::VirtualOutput,
    pub(super) surface: flux::Surface,
    pub(super) canvas: flux::Canvas,
}

pub(super) struct RealmCaptureContext {
    pub(super) realm: aegis_core::realm::RealmId,
    pub(super) revision: u64,
    pub(super) scale_milli: u32,
    pub(super) region: aegis_core::Rect,
    pub(super) placements: Vec<aegis_core::realm::RealmWindowPlacement>,
}

pub(super) struct PendingRealmCapture {
    pub(super) readback: PendingReadback,
    pub(super) context: RealmCaptureContext,
    pub(super) reply: std::sync::mpsc::Sender<Result<aegis_ipc::CaptureRealmPayload, String>>,
}

pub(super) struct PreparedRealmCapture {
    pub(super) readback: PendingReadback,
    pub(super) context: RealmCaptureContext,
}

pub(super) fn virtual_output_physical_size(
    output: aegis_core::realm::VirtualOutput,
) -> Result<(u32, u32), String> {
    if !output.validate() {
        return Err("virtual output parameters are invalid".into());
    }
    let scaled = |value: u32| {
        u64::from(value)
            .saturating_mul(u64::from(output.scale_milli))
            .div_ceil(1000)
    };
    let width = u32::try_from(scaled(output.width)).map_err(|_| "virtual output is too wide")?;
    let height = u32::try_from(scaled(output.height)).map_err(|_| "virtual output is too tall")?;
    Ok((width.max(1), height.max(1)))
}

pub(super) fn begin_realm_capture(
    targets: &mut std::collections::BTreeMap<aegis_core::realm::RealmId, RealmRenderTarget>,
    device: &flux::Device,
    renderer: &mut aegis_render::Renderer,
    server: &aegis_compositor::Server,
    realm: aegis_core::realm::RealmId,
    region: Option<aegis_core::Rect>,
    security_generation: u64,
) -> Result<PreparedRealmCapture, String> {
    let snapshot = server.realm_snapshot();
    let realm_state = snapshot
        .realms
        .iter()
        .find(|record| record.id == realm)
        .ok_or_else(|| format!("unknown realm {}", realm.0))?
        .state;
    if realm_state != aegis_core::realm::RealmState::Active {
        return Err(format!("realm {} is not active ({realm_state:?})", realm.0));
    }
    let output = server
        .realm_output(realm)
        .ok_or_else(|| format!("realm {} has no virtual output", realm.0))?;
    let region = match region {
        Some(region) => {
            clamp_logical_region(region, output.width, output.height).ok_or_else(|| {
                "Realm capture region does not intersect the virtual output".to_owned()
            })?
        }
        None => aegis_core::Rect::new(0, 0, output.width as i32, output.height as i32),
    };
    let placements = server.realm_window_placements(realm);
    let physical_size = virtual_output_physical_size(output)?;
    if targets
        .get(&realm)
        .is_none_or(|target| target.output != output)
    {
        let surface = flux::Surface::offscreen_readback(device, physical_size.0, physical_size.1)
            .map_err(|error| {
            format!(
                "allocate realm {} render target: {error}{}",
                realm.0,
                flux_last_error_detail()
            )
        })?;
        surface.prepare_readback().map_err(|error| {
            format!(
                "prepare realm {} readback: {error}{}",
                realm.0,
                flux_last_error_detail()
            )
        })?;
        let canvas = flux::Canvas::new(&surface).map_err(|error| {
            format!(
                "create realm {} canvas: {error}{}",
                realm.0,
                flux_last_error_detail()
            )
        })?;
        targets.insert(
            realm,
            RealmRenderTarget {
                output,
                surface,
                canvas,
            },
        );
    }
    let target = targets
        .get_mut(&realm)
        .expect("realm render target was just installed");
    let mut frame = target.surface.begin_frame().map_err(|error| {
        format!(
            "begin realm {} frame: {error}{}",
            realm.0,
            flux_last_error_detail()
        )
    })?;
    renderer.begin_frame();
    begin_opaque_frame(&target.canvas, &frame, flux::rgba(17, 20, 27, 255)).map_err(|error| {
        format!(
            "begin realm {} canvas: {error}{}",
            realm.0,
            flux_last_error_detail()
        )
    })?;
    let scale = output.scale_milli as f32 / 1000.0;
    target.canvas.save();
    if scale != 1.0 {
        target.canvas.scale(scale, scale);
    }
    let shm = server.realm_client_surface_frames(realm);
    let dmabuf = server.realm_client_surface_dmabuf_frames(realm);
    let surface_order = server.realm_client_surface_frame_order(realm);
    renderer.draw_surfaces_ordered(device, &target.canvas, &surface_order, &shm, &dmabuf);
    target.canvas.restore();
    target.canvas.end();
    frame.request_readback().map_err(|error| {
        format!(
            "request realm {} readback: {error}{}",
            realm.0,
            flux_last_error_detail()
        )
    })?;
    frame
        .submit()
        .and_then(flux::SubmittedFrame::present)
        .map_err(|error| {
            format!(
                "submit realm {} frame: {error}{}",
                realm.0,
                flux_last_error_detail()
            )
        })?;
    let full_region = aegis_core::Rect::new(0, 0, output.width as i32, output.height as i32);
    Ok(PreparedRealmCapture {
        readback: PendingReadback {
            width: physical_size.0,
            height: physical_size.1,
            crop: (region != full_region)
                .then(|| logical_rect_to_physical(region, scale, physical_size.0, physical_size.1)),
            cursor: None,
            security_generation,
        },
        context: RealmCaptureContext {
            realm,
            revision: snapshot.revision,
            scale_milli: output.scale_milli,
            region,
            placements,
        },
    })
}
