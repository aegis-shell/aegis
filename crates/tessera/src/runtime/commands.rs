use super::*;

/// Direct swapchain composition. A model wallpaper inserts one depth-tested
/// pass between the 2D background and client canvas draws.
#[derive(Clone, Copy)]
pub(super) struct RenderGeometry {
    pub(super) logical_size: (u32, u32),
    pub(super) scale: f32,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_direct_desktop_scene(
    canvas: &flux::Canvas,
    device: &flux::Device,
    frame: &mut flux::Frame<'_>,
    wallpaper: &mut Option<tessera_wallpaper::Wallpaper>,
    renderer: &mut tessera_render::Renderer,
    server: &tessera_compositor::Server,
    geometry: RenderGeometry,
    render_area: Option<flux::CanvasRenderArea>,
    overview: bool,
    overview_progress: f32,
    window_switcher: Option<&tessera_shell::WindowSwitcherPresentation>,
    scheme: tessera_model::settings::ColorScheme,
    soft_shadows: Option<&tessera_render::SoftShadowLayer<'_>>,
    shadow_style: tessera_model::window::WindowShadowStyle,
) -> Result<(), flux::Error> {
    let RenderGeometry {
        logical_size,
        scale,
    } = geometry;
    draw_wallpaper_background(canvas, device, wallpaper, logical_size, scale);
    if wallpaper
        .as_ref()
        .is_some_and(|wallpaper| wallpaper.has_model())
    {
        canvas.end_frame_checked()?;
        if let Some(wallpaper) = wallpaper.as_mut() {
            wallpaper.draw_model(device, frame);
        }
        canvas.begin_pass(
            frame,
            flux::CanvasPassOptions {
                clear: None,
                antialias: flux::CanvasAntialias::None,
                // Preserve the output pass' damage bound after the model's
                // depth pass temporarily leaves Canvas recording.
                render_area,
                skip_stencil: true,
            },
        )?;
    }
    if overview {
        draw_overview_scene(
            canvas,
            device,
            renderer,
            server,
            logical_size,
            scale,
            scheme,
            overview_progress,
        );
    } else {
        draw_client_scene(
            canvas,
            device,
            renderer,
            server,
            scale,
            window_switcher.is_some(),
            soft_shadows,
            shadow_style,
        );
        if let Some(presentation) = window_switcher {
            draw_window_switcher_scrim(canvas, logical_size, scale, presentation, scheme);
        }
    }
    Ok(())
}

pub(super) fn physical_window_target(
    cmd: &tessera_ipc::Command,
) -> Option<tessera_model::window::WindowId> {
    use tessera_ipc::Command;
    match cmd {
        Command::Focus { id, .. }
        | Command::Minimize { id }
        | Command::SetMaximized { id, .. }
        | Command::SetFullscreen { id, .. }
        | Command::SetAlwaysOnTop { id, .. }
        | Command::Close { id }
        | Command::Move { id }
        | Command::SetWindowGeometry { id, .. } => Some(*id),
        Command::MoveToWorkspace { window, .. } => Some(*window),
        _ => None,
    }
}

/// Dispatch an [`tessera_ipc::Command`] to the server and side-effect targets. Extracted
/// from the three mutation sources (IPC, keybindings, chrome) so both the
/// physical-seat authority check and journal chokepoint (ADR-0033) are shared.
pub(super) fn apply_command(
    server: &mut tessera_compositor::Server,
    notif_queue: &std::sync::Arc<std::sync::Mutex<tessera_model::notify::NotificationQueue>>,
    quit: &mut bool,
    cmd: &tessera_ipc::Command,
    ipc: &Option<tessera_ipc::Server>,
    ts_mono_ms: u64,
) -> Result<(), String> {
    if physical_window_target(cmd).is_some_and(|window| !server.human_controls_window(window)) {
        return Err("physical seat has observation-only authority for this window".into());
    }

    use tessera_ipc::Command;
    match cmd {
        Command::Focus { id, reveal } => server.focus_surface_by_id_reveal(*id, *reveal),
        Command::Minimize { id } => server.minimize_toplevel(*id),
        Command::SetMaximized { id, maximized } => {
            server.set_toplevel_maximized(*id, *maximized);
        }
        Command::SetFullscreen { id, fullscreen } => {
            server.set_toplevel_fullscreen(*id, *fullscreen);
        }
        Command::SetAlwaysOnTop { id, on_top } => {
            server.set_toplevel_always_on_top(*id, *on_top);
        }
        Command::Close { id } => server.close_toplevel(*id),
        Command::Move { id } => server.start_interactive_move(*id),
        Command::SetWindowGeometry { id, rect } => {
            server.set_window_geometry(*id, *rect);
        }
        Command::InjectInput { .. } => {
            // Synthetic input needs shell-occlusion validation and is handled
            // beside the physical-input router in the main loop.
            debug_assert!(false, "InjectInput reached the generic command path");
        }
        Command::LaunchInInteractionDomain { .. } => {
            debug_assert!(
                false,
                "LaunchInInteractionDomain reached the generic command path"
            );
        }
        Command::LaunchApp { .. } => {
            // Application launches go through the launcher and are handled
            // beside the IPC command drain in the main loop.
            debug_assert!(false, "LaunchApp reached the generic command path");
        }
        Command::Screenshot { .. } => {
            // Screenshots need the GPU objects and are handled beside the
            // frame renderer in the main loop.
            debug_assert!(false, "Screenshot reached the generic command path");
        }
        Command::ToggleOverview => {
            // The overview is shell-owned; toggled beside the IPC drain.
            debug_assert!(false, "ToggleOverview reached the generic command path");
        }
        Command::System { .. } => {
            // Live system controls need the normalized status snapshot and are
            // handled beside the IPC/chrome drains.
            debug_assert!(false, "System reached the generic command path");
        }
        Command::Cycle { forward } => server.cycle_focus(*forward),
        Command::SwitchWorkspace { dir } => server.switch_workspace(*dir),
        Command::SwitchWorkspaceTo { id } => server.switch_workspace_to(*id),
        Command::MoveToWorkspace { window, workspace } => {
            server.move_to_workspace(*window, *workspace)
        }
        Command::Notify {
            summary,
            body,
            app_id,
            external_id,
        } => {
            let n = notif_queue.lock().unwrap().push_external(
                summary.clone(),
                body.clone(),
                app_id.clone(),
                external_id.clone(),
                ts_mono_ms,
            );
            if let Some(s) = ipc.as_ref() {
                s.broadcast(tessera_ipc::Event::Notified { notification: n });
            }
        }
        Command::DismissNotification { id } => {
            notif_queue.lock().unwrap().dismiss(*id);
        }
        Command::Quit => *quit = true,
    }
    Ok(())
}

pub(super) fn apply_interaction_domain_action(
    server: &mut tessera_compositor::Server,
    subject: Option<String>,
    action: tessera_ipc::InteractionDomainAction,
) -> Result<tessera_ipc::InteractionDomainActionResult, String> {
    match action {
        tessera_ipc::InteractionDomainAction::Create {
            label,
            capabilities,
            output,
        } => {
            let bundle = server
                .create_agent_interaction_domain_for_subject(label, capabilities, subject)
                .map_err(|error| error.to_string())?;
            if let Some(output) = output
                && let Err(error) =
                    server.configure_interaction_domain_output(bundle.interaction_domain, output)
            {
                let _ = server.revoke_interaction_domain(
                    bundle.interaction_domain,
                    tessera_model::interaction_domain::HUMAN_INTERACTION_DOMAIN,
                );
                return Err(error.to_string());
            }
            Ok(tessera_ipc::InteractionDomainActionResult::Created { bundle })
        }
        tessera_ipc::InteractionDomainAction::Transact {
            expected_revision,
            mutations,
        } => server
            .transact_interaction_domains(expected_revision, &mutations)
            .map(
                |receipt| tessera_ipc::InteractionDomainActionResult::TransactionCommitted {
                    receipt,
                },
            )
            .map_err(|error| error.to_string()),
        tessera_ipc::InteractionDomainAction::Revoke {
            interaction_domain,
            fallback,
            expected_revision,
        } => {
            let actual = server.interaction_domain_revision();
            if expected_revision.is_some_and(|expected| expected != actual) {
                return Err(format!(
                    "InteractionDomain revision conflict: expected {}, actual {actual}",
                    expected_revision.unwrap()
                ));
            }
            server
                .revoke_interaction_domain(interaction_domain, fallback)
                .map(|receipt| tessera_ipc::InteractionDomainActionResult::Revoked { receipt })
                .map_err(|error| error.to_string())
        }
    }
}

pub(super) fn interaction_domain_intent_to_action(
    intent: tessera_shell::InteractionDomainIntent,
) -> tessera_ipc::InteractionDomainAction {
    match intent {
        tessera_shell::InteractionDomainIntent::TransferWindow {
            window,
            target,
            retain_source_as_observer,
            expected_revision,
        } => tessera_ipc::InteractionDomainAction::Transact {
            expected_revision: Some(expected_revision),
            mutations: vec![
                tessera_model::interaction_domain::InteractionDomainMutation::TransferWindow {
                    window,
                    target,
                    retain_source_as_observer,
                },
            ],
        },
    }
}

pub(super) fn interaction_domain_action_invalidates_capture(
    action: &tessera_ipc::InteractionDomainAction,
) -> std::collections::BTreeSet<tessera_model::interaction_domain::InteractionDomainId> {
    match action {
        tessera_ipc::InteractionDomainAction::Create { .. } => std::collections::BTreeSet::new(),
        tessera_ipc::InteractionDomainAction::Revoke {
            interaction_domain, ..
        } => std::collections::BTreeSet::from([*interaction_domain]),
        tessera_ipc::InteractionDomainAction::Transact { mutations, .. } => mutations
            .iter()
            .filter_map(|mutation| match mutation {
                tessera_model::interaction_domain::InteractionDomainMutation::SetState {
                    interaction_domain,
                    state:
                        tessera_model::interaction_domain::InteractionDomainState::Paused
                        | tessera_model::interaction_domain::InteractionDomainState::Revoked,
                } => Some(*interaction_domain),
                _ => None,
            })
            .collect(),
    }
}

pub(super) fn interaction_domains_explicitly_stopped(
    action: &tessera_ipc::InteractionDomainAction,
) -> std::collections::BTreeSet<tessera_model::interaction_domain::InteractionDomainId> {
    match action {
        tessera_ipc::InteractionDomainAction::Revoke {
            interaction_domain, ..
        } => std::collections::BTreeSet::from([*interaction_domain]),
        tessera_ipc::InteractionDomainAction::Transact { mutations, .. } => mutations
            .iter()
            .filter_map(|mutation| match mutation {
                tessera_model::interaction_domain::InteractionDomainMutation::SetState {
                    interaction_domain,
                    state: tessera_model::interaction_domain::InteractionDomainState::Paused,
                } => Some(*interaction_domain),
                _ => None,
            })
            .collect(),
        tessera_ipc::InteractionDomainAction::Create { .. } => std::collections::BTreeSet::new(),
    }
}

/// Record a mutation in the journal and push it to journal subscribers
/// (ADR-0033).
pub(super) fn journal_and_broadcast(
    journal: &std::sync::Arc<std::sync::Mutex<tessera_ipc::Journal>>,
    ipc: &Option<tessera_ipc::Server>,
    ts_mono_ms: u64,
    origin: tessera_ipc::Origin,
    cmd: tessera_ipc::Command,
) {
    journal_effect_and_broadcast(
        journal,
        ipc,
        ts_mono_ms,
        origin,
        cmd,
        tessera_ipc::Effect::Applied,
    );
}

pub(super) fn journal_effect_and_broadcast(
    journal: &std::sync::Arc<std::sync::Mutex<tessera_ipc::Journal>>,
    ipc: &Option<tessera_ipc::Server>,
    ts_mono_ms: u64,
    origin: tessera_ipc::Origin,
    cmd: tessera_ipc::Command,
    effect: tessera_ipc::Effect,
) -> tessera_ipc::JournalEntry {
    journal_mutation_effect_and_broadcast(
        journal,
        ipc,
        ts_mono_ms,
        origin,
        tessera_ipc::JournalMutation::Command {
            cmd: tessera_ipc::AuditedCommand::from(&cmd),
        },
        effect,
    )
}

pub(super) fn journal_mutation_effect_and_broadcast(
    journal: &std::sync::Arc<std::sync::Mutex<tessera_ipc::Journal>>,
    ipc: &Option<tessera_ipc::Server>,
    ts_mono_ms: u64,
    origin: tessera_ipc::Origin,
    mutation: tessera_ipc::JournalMutation,
    effect: tessera_ipc::Effect,
) -> tessera_ipc::JournalEntry {
    let effect = mutation.privacy_minimize_effect(effect);
    let mut journal = journal.lock().unwrap();
    let entry = match journal.try_append(ts_mono_ms, origin, mutation, effect) {
        Ok(entry) => entry,
        Err(error) => {
            log::error!("durable audit append failed; fail-stopping compositor: {error}");
            std::process::abort();
        }
    };
    if let Some(s) = ipc.as_ref() {
        s.broadcast_journal(entry.clone());
    }
    entry
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_command_and_journal(
    server: &mut tessera_compositor::Server,
    notifications: &std::sync::Arc<std::sync::Mutex<tessera_model::notify::NotificationQueue>>,
    quit: &mut bool,
    command: tessera_ipc::Command,
    ipc: &Option<tessera_ipc::Server>,
    journal: &std::sync::Arc<std::sync::Mutex<tessera_ipc::Journal>>,
    ts_mono_ms: u64,
    origin: tessera_ipc::Origin,
) {
    let effect = match apply_command(server, notifications, quit, &command, ipc, ts_mono_ms) {
        Ok(()) => tessera_ipc::Effect::Applied,
        Err(reason) => tessera_ipc::Effect::Refused { reason },
    };
    journal_effect_and_broadcast(journal, ipc, ts_mono_ms, origin, command, effect);
}

/// Apply one command and return its journal sequence number and recorded
/// effect — the per-op receipt currency of a Transact batch (ADR-0125).
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_command_journaled(
    server: &mut tessera_compositor::Server,
    notifications: &std::sync::Arc<std::sync::Mutex<tessera_model::notify::NotificationQueue>>,
    quit: &mut bool,
    command: tessera_ipc::Command,
    ipc: &Option<tessera_ipc::Server>,
    journal: &std::sync::Arc<std::sync::Mutex<tessera_ipc::Journal>>,
    ts_mono_ms: u64,
    origin: tessera_ipc::Origin,
) -> (u64, tessera_ipc::Effect) {
    let effect = match apply_command(server, notifications, quit, &command, ipc, ts_mono_ms) {
        Ok(()) => tessera_ipc::Effect::Applied,
        Err(reason) => tessera_ipc::Effect::Refused { reason },
    };
    let entry = journal_effect_and_broadcast(journal, ipc, ts_mono_ms, origin, command, effect);
    (entry.seq, entry.effect)
}

/// Commit one pre-authorized Transact batch at this commit boundary
/// (ADR-0125): every specified precondition currency is checked first —
/// a conflict applies nothing and journals nothing — then ops apply in
/// order through the same chokepoint as `Do`, each returning its journal
/// sequence number and effect as the receipt.
#[allow(clippy::too_many_arguments)]
pub(super) fn apply_transact_batch(
    server: &mut tessera_compositor::Server,
    notifications: &std::sync::Arc<std::sync::Mutex<tessera_model::notify::NotificationQueue>>,
    quit: &mut bool,
    ipc: &Option<tessera_ipc::Server>,
    journal: &std::sync::Arc<std::sync::Mutex<tessera_ipc::Journal>>,
    ts_mono_ms: u64,
    origin: tessera_ipc::Origin,
    expected_journal_seq: Option<u64>,
    expected_interaction_domain_revision: Option<u64>,
    ops: Vec<tessera_ipc::Command>,
) -> Result<tessera_ipc::TransactResult, String> {
    if server.session_locked() {
        return Err("session is locked".into());
    }
    let before_seq = journal.lock().unwrap().latest_seq();
    if let Some(expected) = expected_journal_seq
        && expected != before_seq
    {
        return Ok(tessera_ipc::TransactResult::PreconditionConflict {
            precondition: tessera_ipc::TransactPrecondition::JournalSeq,
            expected,
            actual: before_seq,
        });
    }
    if let Some(expected) = expected_interaction_domain_revision {
        let actual = server.interaction_domain_revision();
        if expected != actual {
            return Ok(tessera_ipc::TransactResult::PreconditionConflict {
                precondition: tessera_ipc::TransactPrecondition::InteractionDomainRevision,
                expected,
                actual,
            });
        }
    }
    let mut after_seq = before_seq;
    let mut results = Vec::with_capacity(ops.len());
    for cmd in ops {
        let (seq, effect) = apply_command_journaled(
            server,
            notifications,
            quit,
            cmd,
            ipc,
            journal,
            ts_mono_ms,
            origin.clone(),
        );
        after_seq = seq;
        results.push(tessera_ipc::TransactOpResult { seq, effect });
    }
    Ok(tessera_ipc::TransactResult::Committed {
        receipt: tessera_ipc::TransactReceipt {
            before_seq,
            after_seq,
            results,
        },
    })
}

#[allow(clippy::too_many_arguments)]
pub(super) fn apply_chrome_window_command(
    server: &mut tessera_compositor::Server,
    notifications: &std::sync::Arc<std::sync::Mutex<tessera_model::notify::NotificationQueue>>,
    quit: &mut bool,
    command: tessera_ipc::Command,
    ipc: &Option<tessera_ipc::Server>,
    journal: &std::sync::Arc<std::sync::Mutex<tessera_ipc::Journal>>,
    ts_mono_ms: u64,
) {
    debug_assert!(physical_window_target(&command).is_some());
    apply_command_and_journal(
        server,
        notifications,
        quit,
        command,
        ipc,
        journal,
        ts_mono_ms,
        tessera_ipc::Origin::Chrome,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        server: tessera_compositor::Server,
        notifications: std::sync::Arc<std::sync::Mutex<tessera_model::notify::NotificationQueue>>,
        journal: std::sync::Arc<std::sync::Mutex<tessera_ipc::Journal>>,
        quit: bool,
    }

    impl Fixture {
        fn new() -> Self {
            Self {
                server: tessera_compositor::Server::new().expect("Server::new"),
                notifications: std::sync::Arc::new(std::sync::Mutex::new(
                    tessera_model::notify::NotificationQueue::new(60_000),
                )),
                journal: std::sync::Arc::new(std::sync::Mutex::new(
                    tessera_ipc::Journal::default_capacity(),
                )),
                quit: false,
            }
        }

        fn transact(
            &mut self,
            expected_journal_seq: Option<u64>,
            expected_interaction_domain_revision: Option<u64>,
            ops: Vec<tessera_ipc::Command>,
        ) -> Result<tessera_ipc::TransactResult, String> {
            apply_transact_batch(
                &mut self.server,
                &self.notifications,
                &mut self.quit,
                &None,
                &self.journal,
                1,
                tessera_ipc::Origin::Ipc { conn_id: 1 },
                expected_journal_seq,
                expected_interaction_domain_revision,
                ops,
            )
        }
    }

    #[test]
    fn transact_batch_commits_in_order_with_per_op_receipts() {
        let mut fixture = Fixture::new();
        let result = fixture
            .transact(
                None,
                None,
                vec![
                    tessera_ipc::Command::Notify {
                        summary: "s1".into(),
                        body: "b1".into(),
                        app_id: None,
                        external_id: None,
                    },
                    tessera_ipc::Command::Notify {
                        summary: "s2".into(),
                        body: "b2".into(),
                        app_id: None,
                        external_id: None,
                    },
                ],
            )
            .expect("batch");
        let tessera_ipc::TransactResult::Committed { receipt } = result else {
            panic!("expected commit, got {result:?}");
        };
        assert_eq!((receipt.before_seq, receipt.after_seq), (0, 2));
        assert_eq!(
            receipt
                .results
                .iter()
                .map(|result| (result.seq, &result.effect))
                .collect::<Vec<_>>(),
            vec![
                (1, &tessera_ipc::Effect::Applied),
                (2, &tessera_ipc::Effect::Applied)
            ]
        );
        assert_eq!(
            fixture.notifications.lock().unwrap().len(),
            2,
            "both ops applied"
        );
        assert_eq!(fixture.journal.lock().unwrap().latest_seq(), 2);
    }

    #[test]
    fn transact_batch_precondition_conflicts_apply_nothing() {
        let mut fixture = Fixture::new();
        let result = fixture
            .transact(
                None,
                None,
                vec![tessera_ipc::Command::Notify {
                    summary: "n1".into(),
                    body: "b1".into(),
                    app_id: None,
                    external_id: None,
                }],
            )
            .expect("first batch");
        let tessera_ipc::TransactResult::Committed { receipt } = result else {
            panic!("expected commit, got {result:?}");
        };
        assert_eq!(receipt.after_seq, 1);

        let result = fixture
            .transact(
                Some(0),
                None,
                vec![tessera_ipc::Command::Notify {
                    summary: "n2".into(),
                    body: "b2".into(),
                    app_id: None,
                    external_id: None,
                }],
            )
            .expect("conflicting batch");
        assert_eq!(
            result,
            tessera_ipc::TransactResult::PreconditionConflict {
                precondition: tessera_ipc::TransactPrecondition::JournalSeq,
                expected: 0,
                actual: 1,
            }
        );
        assert_eq!(
            fixture.notifications.lock().unwrap().len(),
            1,
            "a conflicting batch applies nothing further"
        );
        assert_eq!(
            fixture.journal.lock().unwrap().latest_seq(),
            1,
            "a conflict journals nothing"
        );

        let revision = fixture.server.interaction_domain_revision();
        let result = fixture
            .transact(
                None,
                Some(revision + 1),
                vec![tessera_ipc::Command::Notify {
                    summary: "n3".into(),
                    body: "b3".into(),
                    app_id: None,
                    external_id: None,
                }],
            )
            .expect("revision-conflicting batch");
        assert_eq!(
            result,
            tessera_ipc::TransactResult::PreconditionConflict {
                precondition: tessera_ipc::TransactPrecondition::InteractionDomainRevision,
                expected: revision + 1,
                actual: revision,
            }
        );

        let result = fixture
            .transact(
                Some(1),
                Some(revision),
                vec![tessera_ipc::Command::Notify {
                    summary: "n4".into(),
                    body: "b4".into(),
                    app_id: None,
                    external_id: None,
                }],
            )
            .expect("batch at the fresh cursors");
        assert!(
            matches!(result, tessera_ipc::TransactResult::Committed { .. }),
            "the batch commits when every precondition holds: {result:?}"
        );
        assert_eq!(
            fixture.notifications.lock().unwrap().len(),
            2,
            "the fresh batch applied"
        );
    }

    #[test]
    fn transact_batch_reports_per_op_refusals_and_continues() {
        let mut fixture = Fixture::new();
        let result = fixture
            .transact(
                None,
                None,
                vec![
                    tessera_ipc::Command::Notify {
                        summary: "s1".into(),
                        body: "b1".into(),
                        app_id: None,
                        external_id: None,
                    },
                    // The window does not exist: the physical-seat authority
                    // check refuses this op while the batch continues.
                    tessera_ipc::Command::Focus {
                        id: tessera_model::window::WindowId(99),
                        reveal: true,
                    },
                    tessera_ipc::Command::Notify {
                        summary: "s2".into(),
                        body: "b2".into(),
                        app_id: None,
                        external_id: None,
                    },
                ],
            )
            .expect("batch");
        let tessera_ipc::TransactResult::Committed { receipt } = result else {
            panic!("expected commit, got {result:?}");
        };
        assert_eq!(receipt.results.len(), 3);
        assert!(matches!(
            receipt.results[0].effect,
            tessera_ipc::Effect::Applied
        ));
        assert!(matches!(
            receipt.results[1].effect,
            tessera_ipc::Effect::Refused { .. }
        ));
        assert!(matches!(
            receipt.results[2].effect,
            tessera_ipc::Effect::Applied
        ));
        assert_eq!(
            receipt
                .results
                .iter()
                .map(|result| result.seq)
                .collect::<Vec<_>>(),
            vec![1, 2, 3],
            "every op is journaled in order"
        );
    }
}
