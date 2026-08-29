use super::*;

/// One pixel-capture request from an IPC connection thread, answered by the
/// main loop after it copies the exact output frame being submitted.
pub(super) struct CaptureRequest {
    pub(super) reply: std::sync::mpsc::Sender<Result<aegis_ipc::CaptureOutputPayload, String>>,
    /// Logical-pixel region to capture, or `None` for the full output.
    pub(super) region: Option<aegis_model::Rect>,
}

pub(super) struct InteractionDomainCaptureRequest {
    pub(super) actor: ActorBinding,
    pub(super) max_observations: usize,
    pub(super) interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
    pub(super) reply:
        std::sync::mpsc::Sender<Result<aegis_ipc::CaptureInteractionDomainPayload, String>>,
    pub(super) region: Option<aegis_model::Rect>,
}

pub(super) struct InteractionDomainObserveRequest {
    pub(super) actor: ActorBinding,
    pub(super) max_observations: usize,
    pub(super) interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
    pub(super) reply: std::sync::mpsc::Sender<Result<aegis_ipc::SemanticObservation, String>>,
}

pub(super) struct InteractionDomainActorActionRequest {
    pub(super) actor: ActorBinding,
    pub(super) scope_name: Option<String>,
    pub(super) scope: aegis_ipc::Scope,
    pub(super) origin: aegis_ipc::Origin,
    pub(super) intent: aegis_ipc::ActorActionIntent,
    pub(super) reply: std::sync::mpsc::Sender<Result<aegis_ipc::ActorActionReceipt, String>>,
}

pub(super) struct SemanticTreeUpdateRequest {
    pub(super) provider: aegis_semantic::SemanticProviderId,
    pub(super) update: aegis_semantic::AccessibilityTreeUpdate,
    pub(super) reply: std::sync::mpsc::Sender<Result<(), String>>,
}

pub(super) struct PendingSemanticActorAction {
    pub(super) completion: std::sync::mpsc::Receiver<Result<(), String>>,
    pub(super) deadline: std::time::Instant,
    pub(super) origin: aegis_ipc::Origin,
    pub(super) intent: aegis_ipc::ActorActionIntent,
    pub(super) action_id: u64,
    pub(super) window: aegis_model::window::WindowId,
    pub(super) authority_revision: u64,
    pub(super) reply: std::sync::mpsc::Sender<Result<aegis_ipc::ActorActionReceipt, String>>,
}

pub(super) struct ObservationDiscardRequest {
    pub(super) actor: ActorBinding,
    pub(super) token: aegis_ipc::ObservationToken,
}

pub(super) struct IpcCommandRequest {
    pub(super) origin: aegis_ipc::Origin,
    pub(super) command: aegis_ipc::Command,
}

/// One pre-authorized transaction batch from an IPC connection thread
/// (ADR-0125), applied in order on the main loop; the reply carries the
/// authoritative per-op receipt or a journal precondition conflict.
pub(super) struct TransactRequest {
    pub(super) origin: aegis_ipc::Origin,
    pub(super) expected_journal_seq: Option<u64>,
    pub(super) expected_interaction_domain_revision: Option<u64>,
    pub(super) ops: Vec<aegis_ipc::Command>,
    pub(super) reply: std::sync::mpsc::Sender<Result<aegis_ipc::TransactResult, String>>,
}

/// One live-system mutation that must return the main loop's authoritative
/// apply result rather than only acknowledging IPC queueing.
pub(super) struct SystemControlRequest {
    pub(super) origin: aegis_ipc::Origin,
    pub(super) action: aegis_ipc::SystemAction,
    pub(super) reply: std::sync::mpsc::Sender<Result<(), String>>,
}

pub(super) struct InteractionDomainControlRequest {
    pub(super) origin: aegis_ipc::Origin,
    /// Credential-bound agent principal. `None` denotes a trusted built-in
    /// or compositor-local caller.
    pub(super) subject: Option<String>,
    pub(super) action: aegis_ipc::InteractionDomainAction,
    pub(super) reply:
        std::sync::mpsc::Sender<Result<aegis_ipc::InteractionDomainActionResult, String>>,
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

#[derive(Default)]
pub(super) struct InteractionDomainProcesses {
    launches: std::collections::BTreeMap<
        aegis_model::interaction_domain::InteractionDomainId,
        Vec<aegis_launcher::ManagedLaunch>,
    >,
}

impl InteractionDomainProcesses {
    pub(super) fn insert(
        &mut self,
        interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
        launch: aegis_launcher::ManagedLaunch,
    ) {
        self.launches
            .entry(interaction_domain)
            .or_default()
            .push(launch);
    }

    pub(super) fn pause(
        &mut self,
        interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
    ) {
        if let Some(launches) = self.launches.get_mut(&interaction_domain) {
            launches.retain_mut(|launch| {
                if let Err(error) = launch.pause() {
                    log::error!(
                        "InteractionDomain {} sandbox {} could not be paused; terminating: {error}",
                        interaction_domain.0,
                        launch.report().pid
                    );
                    false
                } else {
                    true
                }
            });
        }
    }

    pub(super) fn resume(
        &mut self,
        interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
    ) {
        if let Some(launches) = self.launches.get_mut(&interaction_domain) {
            launches.retain_mut(|launch| {
                if let Err(error) = launch.resume() {
                    log::error!(
                        "InteractionDomain {} sandbox {} could not be resumed; terminating: {error}",
                        interaction_domain.0,
                        launch.report().pid
                    );
                    false
                } else {
                    true
                }
            });
        }
    }

    pub(super) fn revoke(
        &mut self,
        interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
    ) {
        // Dropping ManagedLaunch kills the complete sandbox cgroup and reaps
        // bubblewrap before this method returns.
        self.launches.remove(&interaction_domain);
    }

    pub(super) fn apply_committed_action(&mut self, action: &aegis_ipc::InteractionDomainAction) {
        match action {
            aegis_ipc::InteractionDomainAction::Create { .. } => {}
            aegis_ipc::InteractionDomainAction::Transact { mutations, .. } => {
                for mutation in mutations {
                    if let aegis_model::interaction_domain::InteractionDomainMutation::SetState {
                        interaction_domain,
                        state,
                    } = mutation
                    {
                        match state {
                            aegis_model::interaction_domain::InteractionDomainState::Active => {
                                self.resume(*interaction_domain)
                            }
                            aegis_model::interaction_domain::InteractionDomainState::Paused => {
                                self.pause(*interaction_domain)
                            }
                            aegis_model::interaction_domain::InteractionDomainState::Revoked => {
                                self.revoke(*interaction_domain)
                            }
                        }
                    }
                }
            }
            aegis_ipc::InteractionDomainAction::Revoke {
                interaction_domain, ..
            } => self.revoke(*interaction_domain),
        }
    }
}

pub(super) struct InteractionDomainRenderTarget {
    pub(super) output: aegis_model::interaction_domain::VirtualOutput,
    pub(super) surface: flux::Surface,
    pub(super) canvas: flux::Canvas,
}

pub(super) struct InteractionDomainCaptureContext {
    pub(super) interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
    pub(super) revision: u64,
    pub(super) scale_milli: u32,
    pub(super) region: aegis_model::Rect,
    pub(super) placements: Vec<aegis_model::interaction_domain::InteractionDomainWindowPlacement>,
    pub(super) semantic: aegis_model::semantic::SemanticSnapshot,
    pub(super) observation: Option<aegis_ipc::SemanticObservation>,
}

pub(super) struct PendingInteractionDomainCapture {
    pub(super) readback: PendingReadback,
    pub(super) context: InteractionDomainCaptureContext,
    pub(super) reply:
        std::sync::mpsc::Sender<Result<aegis_ipc::CaptureInteractionDomainPayload, String>>,
}

pub(super) struct PreparedInteractionDomainCapture {
    pub(super) readback: PendingReadback,
    pub(super) context: InteractionDomainCaptureContext,
}

pub(super) fn virtual_output_physical_size(
    output: aegis_model::interaction_domain::VirtualOutput,
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

#[allow(clippy::too_many_arguments)]
pub(super) fn begin_interaction_domain_capture(
    targets: &mut std::collections::BTreeMap<
        aegis_model::interaction_domain::InteractionDomainId,
        InteractionDomainRenderTarget,
    >,
    device: &flux::Device,
    renderer: &mut aegis_render::Renderer,
    server: &aegis_compositor::Server,
    interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
    region: Option<aegis_model::Rect>,
    security_generation: u64,
    scheme: aegis_model::settings::ColorScheme,
) -> Result<PreparedInteractionDomainCapture, String> {
    let snapshot = server.interaction_domain_snapshot();
    let interaction_domain_state = snapshot
        .interaction_domains
        .iter()
        .find(|record| record.id == interaction_domain)
        .ok_or_else(|| format!("unknown interaction_domain {}", interaction_domain.0))?
        .state;
    if interaction_domain_state != aegis_model::interaction_domain::InteractionDomainState::Active {
        return Err(format!(
            "interaction_domain {} is not active ({interaction_domain_state:?})",
            interaction_domain.0
        ));
    }
    let output = server
        .interaction_domain_output(interaction_domain)
        .ok_or_else(|| {
            format!(
                "interaction_domain {} has no virtual output",
                interaction_domain.0
            )
        })?;
    let region = match region {
        Some(region) => {
            clamp_logical_region(region, output.width, output.height).ok_or_else(|| {
                "Interaction Domain capture region does not intersect the virtual output".to_owned()
            })?
        }
        None => aegis_model::Rect::new(0, 0, output.width as i32, output.height as i32),
    };
    let placements = server.interaction_domain_window_placements(interaction_domain);
    let semantic = server
        .interaction_domain_semantic_snapshot(interaction_domain)
        .map_err(|error| error.to_string())?;
    let physical_size = virtual_output_physical_size(output)?;
    if targets
        .get(&interaction_domain)
        .is_none_or(|target| target.output != output)
    {
        let surface = flux::Surface::offscreen_readback(device, physical_size.0, physical_size.1)
            .map_err(|error| {
            format!(
                "allocate interaction_domain {} render target: {error}{}",
                interaction_domain.0,
                flux_last_error_detail()
            )
        })?;
        surface.prepare_readback().map_err(|error| {
            format!(
                "prepare interaction_domain {} readback: {error}{}",
                interaction_domain.0,
                flux_last_error_detail()
            )
        })?;
        let canvas = flux::Canvas::new(&surface).map_err(|error| {
            format!(
                "create interaction_domain {} canvas: {error}{}",
                interaction_domain.0,
                flux_last_error_detail()
            )
        })?;
        targets.insert(
            interaction_domain,
            InteractionDomainRenderTarget {
                output,
                surface,
                canvas,
            },
        );
    }
    let target = targets
        .get_mut(&interaction_domain)
        .expect("interaction_domain render target was just installed");
    let mut frame = target.surface.begin_frame().map_err(|error| {
        format!(
            "begin interaction_domain {} frame: {error}{}",
            interaction_domain.0,
            flux_last_error_detail()
        )
    })?;
    begin_opaque_frame(&target.canvas, &frame, interaction_domain_clear(scheme)).map_err(
        |error| {
            format!(
                "begin interaction_domain {} canvas: {error}{}",
                interaction_domain.0,
                flux_last_error_detail()
            )
        },
    )?;
    let scale = output.scale_milli as f32 / 1000.0;
    target.canvas.save();
    if scale != 1.0 {
        target.canvas.scale(scale, scale);
    }
    let shm = server.interaction_domain_client_surface_frames(interaction_domain);
    let dmabuf = server.interaction_domain_client_surface_dmabuf_frames(interaction_domain);
    let surface_order = server.interaction_domain_client_surface_frame_order(interaction_domain);
    renderer.draw_surfaces_ordered(device, &target.canvas, &surface_order, &shm, &dmabuf);
    target.canvas.restore();
    target.canvas.end_frame_checked().map_err(|error| {
        format!(
            "end interaction_domain {} canvas: {error}{}",
            interaction_domain.0,
            flux_last_error_detail()
        )
    })?;
    frame.request_readback().map_err(|error| {
        format!(
            "request interaction_domain {} readback: {error}{}",
            interaction_domain.0,
            flux_last_error_detail()
        )
    })?;
    frame
        .submit()
        .and_then(flux::SubmittedFrame::present)
        .map_err(|error| {
            format!(
                "submit interaction_domain {} frame: {error}{}",
                interaction_domain.0,
                flux_last_error_detail()
            )
        })?;
    let full_region = aegis_model::Rect::new(0, 0, output.width as i32, output.height as i32);
    Ok(PreparedInteractionDomainCapture {
        readback: PendingReadback {
            width: physical_size.0,
            height: physical_size.1,
            crop: (region != full_region)
                .then(|| logical_rect_to_physical(region, scale, physical_size.0, physical_size.1)),
            cursor: None,
            security_generation,
        },
        context: InteractionDomainCaptureContext {
            interaction_domain,
            revision: snapshot.revision,
            scale_milli: output.scale_milli,
            region,
            placements,
            semantic,
            observation: None,
        },
    })
}
