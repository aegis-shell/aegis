use super::*;

impl AegisPlatform {
    /// Probe the compositor grant and acquire the per-scope Interaction Domain recovery lock.
    pub fn connect(config: BridgeConfig) -> Result<Self, PlatformError> {
        config.validate()?;
        let identity =
            crate::identity::IdentityStore::load(config.data_dir.clone(), &config.instance_id);
        let client = connect_with(&config, &identity, config.io_timeout)?;
        let principal = identity
            .principal()
            .ok_or(PlatformError::MissingAuthenticatedIdentity)?;
        let grant = ToolGrant {
            capabilities: client.caps(),
            scope: client.scope().clone(),
        };
        Ok(Self {
            interaction_domain: InteractionDomainSession::acquire(&config, &principal)?,
            config,
            grant,
            identity,
            pending_observations: std::collections::BTreeMap::new(),
        })
    }

    pub fn grant(&self) -> &ToolGrant {
        &self.grant
    }

    /// Names exposed through `tools/list` under the current startup grant.
    pub fn tool_names(&self) -> Vec<&'static str> {
        self.definitions()
            .into_iter()
            .map(|definition| definition.name)
            .collect()
    }

    /// Exercise a real, reversible compositor mutation and Interaction Domain lifecycle.
    ///
    /// The notification is re-read from compositor state rather than treating
    /// an IPC `Ok` as proof. A newly created test Interaction Domain is synchronously
    /// transitioned through paused and active states, left visible for
    /// `observation`, then revoked. A recovered pre-existing Interaction Domain is only
    /// observed and preserved so smoke testing never takes authority away from
    /// user work.
    pub fn smoke(&mut self, observation: Duration) -> Result<SmokeReport, PlatformError> {
        self.smoke_with_input(observation, None)
    }

    /// Run the live smoke test and optionally transfer one explicitly selected
    /// human-controlled window long enough to apply a harmless pointer move.
    /// The move is verified through the compositor journal and the window is
    /// returned to the human Interaction Domain during cleanup.
    pub fn smoke_with_input(
        &mut self,
        observation: Duration,
        input_window: Option<WindowId>,
    ) -> Result<SmokeReport, PlatformError> {
        let mut required = vec![
            ToolKind::PostNotification,
            ToolKind::InteractionDomainEnsure,
            ToolKind::InteractionDomainSetState,
            ToolKind::InteractionDomainReset,
        ];
        if input_window.is_some() {
            required.extend([
                ToolKind::InteractionDomainTransferWindow,
                ToolKind::InteractionDomainObserve,
                ToolKind::InteractionDomainInput,
            ]);
        }
        for kind in required {
            if !kind.allowed(&self.grant) {
                return Err(PlatformError::NotGranted(
                    kind.definition().name.to_string(),
                ));
            }
        }

        let mut client = self.connect_ipc_grant()?;
        let (_, existing) = self.interaction_domain.locate(&mut client)?;
        let marker = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tag = format!("{:06x}", marker & 0xff_ffff);
        let started_summary = format!("aegis-mcp ↔ Aegis · {tag}");
        client.command(Command::Notify {
            summary: started_summary.clone(),
            body: "Live notification verified. Agent Interaction Domain smoke is running.".into(),
            app_id: Some("aegis-mcp".into()),
            external_id: None,
        })?;
        let started_notification = self.wait_for_notification(&mut client, &started_summary)?;

        let created_by_smoke = existing.is_none();
        let managed = match existing {
            Some(managed) => managed,
            None => self.ensure_interaction_domain(&mut client)?,
        };
        let mut lifecycle = vec![self.verified_interaction_domain_state(&mut client, managed.id)?];
        let mut input_probe = None;

        let cleanup = if created_by_smoke {
            let paused_for = observation.min(Duration::from_secs(2));
            let active_for = observation.saturating_sub(paused_for);
            let exercise = (|| {
                self.set_smoke_interaction_domain_state(
                    &mut client,
                    managed.id,
                    InteractionDomainState::Paused,
                )?;
                lifecycle.push(self.verified_interaction_domain_state(&mut client, managed.id)?);
                if !paused_for.is_zero() {
                    std::thread::sleep(paused_for);
                }
                self.set_smoke_interaction_domain_state(
                    &mut client,
                    managed.id,
                    InteractionDomainState::Active,
                )?;
                lifecycle.push(self.verified_interaction_domain_state(&mut client, managed.id)?);
                if let Some(window) = input_window {
                    input_probe =
                        Some(self.exercise_agent_pointer(&mut client, managed.id, window)?);
                }
                if !active_for.is_zero() {
                    std::thread::sleep(active_for);
                }
                Ok::<(), PlatformError>(())
            })();

            // Cleanup is attempted even if a transition or verification
            // failed, so a diagnostic run does not strand authority.
            let revoked = self.interaction_domain.revoke(&mut client);
            if let Err(error) = exercise {
                return match revoked {
                    Ok(_) => Err(error),
                    Err(cleanup) => Err(PlatformError::SmokeVerification(format!(
                        "{error}; cleanup also failed: {cleanup}"
                    ))),
                };
            }
            if !revoked? {
                return Err(PlatformError::SmokeVerification(
                    "new smoke Interaction Domain was not present during cleanup".into(),
                ));
            }
            let snapshot = client.interaction_domains()?;
            if snapshot
                .interaction_domains
                .iter()
                .any(|interaction_domain| {
                    interaction_domain.id == managed.id
                        && interaction_domain.state != InteractionDomainState::Revoked
                })
            {
                return Err(PlatformError::SmokeVerification(
                    "smoke Interaction Domain remained live after revocation".into(),
                ));
            }
            lifecycle.push("revoked".into());
            if let Some(probe) = input_probe.as_mut() {
                probe.window_restored_to_human = self
                    .window_control_interaction_domain(&mut client, WindowId(probe.window_id))?
                    == Some(HUMAN_INTERACTION_DOMAIN);
                if !probe.window_restored_to_human {
                    return Err(PlatformError::SmokeVerification(format!(
                        "window {} did not return to the human Interaction Domain after smoke revocation",
                        probe.window_id
                    )));
                }
            }
            "revoked_test_interaction_domain"
        } else {
            let exercise = (|| {
                if let Some(window) = input_window {
                    if self.verified_interaction_domain_state(&mut client, managed.id)? != "active"
                    {
                        return Err(PlatformError::SmokeVerification(
                            "the recovered managed Interaction Domain is paused; resume or reset it before an input smoke probe"
                                .into(),
                        ));
                    }
                    input_probe =
                        Some(self.exercise_agent_pointer(&mut client, managed.id, window)?);
                }
                if !observation.is_zero() {
                    std::thread::sleep(observation);
                }
                Ok::<(), PlatformError>(())
            })();
            let restore = input_window
                .map(|window| self.restore_smoke_window(&mut client, window))
                .transpose();
            if let Err(error) = exercise {
                return match restore {
                    Ok(_) => Err(error),
                    Err(cleanup) => Err(PlatformError::SmokeVerification(format!(
                        "{error}; window cleanup also failed: {cleanup}"
                    ))),
                };
            }
            restore?;
            if let Some(probe) = input_probe.as_mut() {
                probe.window_restored_to_human = self
                    .window_control_interaction_domain(&mut client, WindowId(probe.window_id))?
                    == Some(HUMAN_INTERACTION_DOMAIN);
                if !probe.window_restored_to_human {
                    return Err(PlatformError::SmokeVerification(format!(
                        "window {} did not return to the human Interaction Domain after the smoke probe",
                        probe.window_id
                    )));
                }
            }
            "preserved_existing_interaction_domain"
        };

        let summary = format!("aegis-mcp ↔ Aegis · passed · {tag}");
        client.command(Command::Notify {
            summary: summary.clone(),
            body: "Notification and Agent Interaction Domain controls were applied and verified."
                .into(),
            app_id: Some("aegis-mcp".into()),
            external_id: None,
        })?;
        let notification = self.wait_for_notification(&mut client, &summary)?;

        Ok(SmokeReport {
            status: "passed",
            mode: "live",
            label: self.config.label.clone(),
            notification: SmokeNotificationReport {
                started_id: started_notification.id,
                id: notification.id,
                summary,
                observed_in_compositor_state: true,
            },
            interaction_domain: SmokeInteractionDomainReport {
                id: managed.id.0,
                created_by_smoke,
                lifecycle,
                cleanup,
            },
            visual: SmokeVisualReport {
                status_indicator: "persistent while the Agent Interaction Domain is live",
                details_surface: "click the status indicator to open Agent Workspaces",
                observation_millis: observation.as_millis(),
                input_probe,
            },
        })
    }

    fn exercise_agent_pointer(
        &self,
        client: &mut Client,
        interaction_domain: InteractionDomainId,
        window: WindowId,
    ) -> Result<SmokeInputReport, PlatformError> {
        let target = client
            .windows()?
            .into_iter()
            .find(|candidate| candidate.id == window)
            .ok_or_else(|| {
                PlatformError::SmokeVerification(format!(
                    "window {} is not visible on the physical desktop",
                    window.0
                ))
            })?;
        if target.read_only || target.size.w <= 0 || target.size.h <= 0 {
            return Err(PlatformError::SmokeVerification(format!(
                "window {} is not a live human-controlled input target",
                window.0
            )));
        }
        if self.window_control_interaction_domain(client, window)? != Some(HUMAN_INTERACTION_DOMAIN)
        {
            return Err(PlatformError::SmokeVerification(format!(
                "window {} is not currently controlled by the human Interaction Domain",
                window.0
            )));
        }

        let local_position = Point {
            x: (target.size.w / 2).clamp(0, target.size.w - 1),
            y: (target.size.h / 2).clamp(0, target.size.h - 1),
        };
        let snapshot = client.interaction_domains()?;
        let result = client.interaction_domain_action(InteractionDomainAction::Transact {
            expected_revision: Some(snapshot.revision),
            mutations: vec![InteractionDomainMutation::TransferWindow {
                window,
                target: interaction_domain,
                retain_source_as_observer: true,
            }],
        })?;
        if !matches!(
            result,
            InteractionDomainActionResult::TransactionCommitted { .. }
        ) {
            return Err(PlatformError::UnexpectedResponse);
        }

        let observation = client.observe_interaction_domain(interaction_domain)?;
        let baseline = client.journal(0)?.latest_seq;
        let actions = vec![SyntheticInputAction::PointerMove {
            position: local_position,
        }];
        let semantic_actions = vec![
            aegis_model::semantic::SemanticActionIntent::SyntheticInput {
                actions: actions.clone(),
            },
        ];
        let audited_actions = aegis_ipc::audit_semantic_actions(&semantic_actions);
        client.inject_interaction_domain_input(
            interaction_domain,
            SemanticObjectId::for_window(window),
            observation.token,
            actions.clone(),
        )?;
        let deadline = Instant::now() + self.config.io_timeout;
        loop {
            let journal = client.journal(baseline)?;
            if let Some(entry) = journal.entries.into_iter().find(|entry| {
                matches!(
                    &entry.mutation,
                    JournalMutation::ActorAction {
                        interaction_domain: event_interaction_domain,
                        target,
                        actions: event_actions,
                        ..
                    } if *event_interaction_domain == interaction_domain
                        && *target == SemanticObjectId::for_window(window)
                        && event_actions == &audited_actions
                )
            }) {
                return match entry.effect {
                    Effect::Applied => Ok(SmokeInputReport {
                        window_id: window.0,
                        action: "pointer_move",
                        local_position,
                        journal_sequence: entry.seq,
                        applied: true,
                        window_restored_to_human: false,
                    }),
                    Effect::Refused { reason } => Err(PlatformError::SmokeVerification(format!(
                        "Agent pointer smoke was refused: {reason}"
                    ))),
                    Effect::NoOp => Err(PlatformError::SmokeVerification(
                        "Agent pointer smoke was recorded as a no-op".into(),
                    )),
                };
            }
            if Instant::now() >= deadline {
                return Err(PlatformError::SmokeVerification(
                    "Agent pointer smoke was queued but no journal decision appeared".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn restore_smoke_window(
        &self,
        client: &mut Client,
        window: WindowId,
    ) -> Result<(), PlatformError> {
        let snapshot = client.interaction_domains()?;
        if self.window_control_interaction_domain(client, window)? == Some(HUMAN_INTERACTION_DOMAIN)
        {
            return Ok(());
        }
        let result = client.interaction_domain_action(InteractionDomainAction::Transact {
            expected_revision: Some(snapshot.revision),
            mutations: vec![InteractionDomainMutation::TransferWindow {
                window,
                target: HUMAN_INTERACTION_DOMAIN,
                retain_source_as_observer: false,
            }],
        })?;
        if !matches!(
            result,
            InteractionDomainActionResult::TransactionCommitted { .. }
        ) {
            return Err(PlatformError::UnexpectedResponse);
        }
        Ok(())
    }

    fn window_control_interaction_domain(
        &self,
        client: &mut Client,
        window: WindowId,
    ) -> Result<Option<InteractionDomainId>, PlatformError> {
        Ok(client
            .interaction_domains()?
            .interaction_groups
            .into_iter()
            .find(|group| group.windows.contains(&window))
            .map(|group| group.control_interaction_domain))
    }

    pub(crate) fn definitions(&self) -> Vec<ToolDefinition> {
        ToolKind::ALL
            .iter()
            .copied()
            .filter(|kind| kind.allowed(&self.grant))
            .map(ToolKind::definition)
            .collect()
    }

    /// Refresh the credential-bound ceiling before publishing a catalog.
    /// This makes administrative ceiling changes visible on the next
    /// `tools/list` instead of retaining the process-start snapshot.
    pub(crate) fn refreshed_definitions(&mut self) -> Result<Vec<ToolDefinition>, PlatformError> {
        let client = self.connect_ipc()?;
        self.grant = ToolGrant {
            capabilities: client.caps(),
            scope: client.scope().clone(),
        };
        Ok(self.definitions())
    }

    pub(crate) fn call(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolCallResult, PlatformError> {
        let kind = ToolKind::from_name(name)
            .ok_or_else(|| PlatformError::UnknownTool(name.to_string()))?;
        if !kind.allowed(&self.grant) {
            return Err(PlatformError::NotGranted(name.to_string()));
        }
        self.invoke(kind, arguments)
    }

    /// Best-effort normal shutdown. Failure is returned so the CLI can report
    /// that the recovery record was intentionally retained for the next run.
    pub fn shutdown(&mut self) -> Result<(), PlatformError> {
        self.pending_observations.clear();
        if !self.config.revoke_on_exit {
            return Ok(());
        }
        let mut client = self.connect_ipc_grant()?;
        let (_, managed) = self.interaction_domain.locate(&mut client)?;
        if managed.is_none() {
            return Ok(());
        }
        if !self.can_revoke_interaction_domain() {
            return Err(PlatformError::InteractionDomainCleanupNotGranted);
        }
        self.interaction_domain.revoke(&mut client)?;
        Ok(())
    }

    fn invoke(
        &mut self,
        kind: ToolKind,
        arguments: Value,
    ) -> Result<ToolCallResult, PlatformError> {
        match kind {
            ToolKind::DesktopSnapshot => {
                parse::<NoArgs>(arguments)?;
                let mut client = self.connect_ipc()?;
                Ok(ToolCallResult::json(json!({
                    "grant": self.grant,
                    "windows": client.windows()?,
                    "workspaces": client.workspaces()?,
                    "outputs": client.outputs()?,
                    "interaction_domains": client.interaction_domains()?
                })))
            }
            ToolKind::DesktopJournal => {
                let args: JournalArgs = parse(arguments)?;
                let limit = args.limit.unwrap_or(MAX_JOURNAL_ENTRIES);
                if !(1..=MAX_JOURNAL_ENTRIES).contains(&limit) {
                    return Err(invalid(format!(
                        "limit must be from 1 through {MAX_JOURNAL_ENTRIES}"
                    )));
                }
                let mut client = self.connect_ipc()?;
                let mut snapshot = client.journal(args.since.unwrap_or(0))?;
                snapshot.entries.truncate(limit);
                let next_since = snapshot
                    .entries
                    .last()
                    .map_or(args.since.unwrap_or(0), |entry| entry.seq);
                Ok(ToolCallResult::json(json!({
                    "snapshot": snapshot,
                    "next_since": next_since
                })))
            }
            ToolKind::AppsList => self.list_apps(arguments),
            ToolKind::LaunchApp => {
                let args: LaunchAppArgs = parse(arguments)?;
                self.command(launch_app_command(args)?, "launch_app")
            }
            ToolKind::FocusWindow => {
                let args: WindowArgs = parse(arguments)?;
                self.command(focus_command(args)?, "focus_window")
            }
            ToolKind::MinimizeWindow => self.command(
                window_command(arguments, |id| Command::Minimize { id })?,
                "minimize_window",
            ),
            ToolKind::CloseWindow => self.command(
                window_command(arguments, |id| Command::Close { id })?,
                "close_window",
            ),
            ToolKind::MoveWindowToWorkspace => {
                let args: MoveArgs = parse(arguments)?;
                self.command(
                    Command::MoveToWorkspace {
                        window: window_id(args.window_id)?,
                        workspace: workspace_id(args.workspace_id)?,
                    },
                    "move_window_to_workspace",
                )
            }
            ToolKind::SwitchWorkspace => {
                let args: DirectionArgs = parse(arguments)?;
                let dir = match args.direction.as_str() {
                    "next" => Switch::Next,
                    "previous" => Switch::Prev,
                    _ => return Err(invalid("direction must be `next` or `previous`")),
                };
                self.command(Command::SwitchWorkspace { dir }, "switch_workspace")
            }
            ToolKind::SwitchWorkspaceTo => {
                let args: WorkspaceArgs = parse(arguments)?;
                self.command(
                    Command::SwitchWorkspaceTo {
                        id: workspace_id(args.workspace_id)?,
                    },
                    "switch_workspace_to",
                )
            }
            ToolKind::SetWindowGeometry => {
                let args: GeometryArgs = parse(arguments)?;
                let rect = rect(args.x, args.y, args.width, args.height)?;
                self.command(
                    Command::SetWindowGeometry {
                        id: window_id(args.window_id)?,
                        rect,
                    },
                    "set_window_geometry",
                )
            }
            ToolKind::ToggleTiling => {
                parse::<NoArgs>(arguments)?;
                self.command(Command::ToggleTiling, "toggle_tiling")
            }
            ToolKind::ToggleOverview => {
                parse::<NoArgs>(arguments)?;
                self.command(Command::ToggleOverview, "toggle_overview")
            }
            ToolKind::PostNotification => {
                let args: NotificationArgs = parse(arguments)?;
                if args.summary.trim().is_empty() {
                    return Err(invalid("summary must not be empty"));
                }
                self.command(
                    Command::Notify {
                        summary: args.summary,
                        body: args.body.unwrap_or_default(),
                        app_id: Some("aegis-mcp".into()),
                        external_id: None,
                    },
                    "post_notification",
                )
            }
            ToolKind::InteractionDomainStatus => self.interaction_domain_status(arguments),
            ToolKind::InteractionDomainEnsure => self.interaction_domain_ensure(arguments),
            ToolKind::InteractionDomainLaunchApp => self.interaction_domain_launch(arguments),
            ToolKind::InteractionDomainTransferWindow => {
                self.interaction_domain_transfer(arguments)
            }
            ToolKind::InteractionDomainSetState => self.interaction_domain_set_state(arguments),
            ToolKind::InteractionDomainObserve => self.interaction_domain_observe(arguments),
            ToolKind::InteractionDomainCapture => self.interaction_domain_capture(arguments),
            ToolKind::InteractionDomainInput => self.interaction_domain_input(arguments),
            ToolKind::InteractionDomainReset => self.interaction_domain_reset(arguments),
            ToolKind::WindowCapture => self.window_capture(arguments),
        }
    }

    fn connect_ipc(&self) -> Result<Client, PlatformError> {
        connect_with(&self.config, &self.identity, self.config.io_timeout)
    }

    /// Connect for a mutation call that may block on an interactive runtime
    /// grant (ADR-0088). The compositor's interaction timeout is 300 s, so
    /// these calls get a bound beyond it; query calls keep the configured
    /// I/O timeout.
    fn connect_ipc_grant(&self) -> Result<Client, PlatformError> {
        connect_with(&self.config, &self.identity, GRANT_TIMEOUT)
    }

    fn retain_observation(&mut self, token: &ObservationToken, ttl_ms: u64, client: Client) {
        let now = Instant::now();
        self.pending_observations
            .retain(|_, pending| pending.expires_at > now);
        while self.pending_observations.len() >= MAX_PENDING_OBSERVATIONS {
            let Some(oldest) = self
                .pending_observations
                .iter()
                .min_by_key(|(_, pending)| pending.expires_at)
                .map(|(token, _)| token.clone())
            else {
                break;
            };
            self.pending_observations.remove(&oldest);
        }
        let expires_at = now
            .checked_add(Duration::from_millis(ttl_ms))
            .unwrap_or(now);
        self.pending_observations
            .insert(token.0.clone(), PendingObservation { expires_at, client });
    }

    fn take_observation_client(&mut self, token: &str) -> Result<Client, PlatformError> {
        let now = Instant::now();
        self.pending_observations
            .retain(|_, pending| pending.expires_at > now);
        self.pending_observations
            .remove(token)
            .map(|pending| pending.client)
            .ok_or(PlatformError::UnknownObservation)
    }

    fn wait_for_notification(
        &self,
        client: &mut Client,
        summary: &str,
    ) -> Result<aegis_model::notify::Notification, PlatformError> {
        let deadline = Instant::now() + self.config.io_timeout;
        loop {
            if let Some(notification) = client.notifications()?.into_iter().find(|notification| {
                notification.summary == summary
                    && notification.app_id.as_deref() == Some("aegis-mcp")
            }) {
                return Ok(notification);
            }
            if Instant::now() >= deadline {
                return Err(PlatformError::SmokeVerification(
                    "notification was acknowledged but did not appear in compositor state".into(),
                ));
            }
            std::thread::sleep(Duration::from_millis(25));
        }
    }

    fn set_smoke_interaction_domain_state(
        &self,
        client: &mut Client,
        interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
        state: InteractionDomainState,
    ) -> Result<(), PlatformError> {
        let snapshot = client.interaction_domains()?;
        let result = client.interaction_domain_action(InteractionDomainAction::Transact {
            expected_revision: Some(snapshot.revision),
            mutations: vec![InteractionDomainMutation::SetState {
                interaction_domain,
                state,
            }],
        })?;
        if !matches!(
            result,
            InteractionDomainActionResult::TransactionCommitted { .. }
        ) {
            return Err(PlatformError::UnexpectedResponse);
        }
        Ok(())
    }

    fn verified_interaction_domain_state(
        &self,
        client: &mut Client,
        interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
    ) -> Result<String, PlatformError> {
        let snapshot = client.interaction_domains()?;
        let state = snapshot
            .interaction_domains
            .iter()
            .find(|candidate| candidate.id == interaction_domain)
            .map(|candidate| candidate.state)
            .ok_or_else(|| {
                PlatformError::SmokeVerification(format!(
                    "Interaction Domain {} was committed but was not queryable",
                    interaction_domain.0
                ))
            })?;
        Ok(match state {
            InteractionDomainState::Active => "active",
            InteractionDomainState::Paused => "paused",
            InteractionDomainState::Revoked => "revoked",
        }
        .into())
    }

    fn command(
        &self,
        command: Command,
        operation: &'static str,
    ) -> Result<ToolCallResult, PlatformError> {
        let mut client = self.connect_ipc_grant()?;
        client.command(command)?;
        Ok(ToolCallResult::json(json!({
            "status": "queued",
            "operation": operation,
            "verified": false
        })))
    }

    fn list_apps(&self, arguments: Value) -> Result<ToolCallResult, PlatformError> {
        let args: AppsArgs = parse(arguments)?;
        let limit = args.limit.unwrap_or(50);
        if !(1..=MAX_APP_RESULTS).contains(&limit) {
            return Err(invalid(format!(
                "limit must be from 1 through {MAX_APP_RESULTS}"
            )));
        }
        let query = args.query.unwrap_or_default().trim().to_ascii_lowercase();
        let apps = aegis_desktop_entries::enumerate()
            .into_iter()
            .filter(|entry| {
                query.is_empty()
                    || entry.id.to_ascii_lowercase().contains(&query)
                    || entry.name.to_ascii_lowercase().contains(&query)
                    || entry.summary().to_ascii_lowercase().contains(&query)
                    || entry
                        .keywords
                        .iter()
                        .any(|keyword| keyword.to_ascii_lowercase().contains(&query))
            })
            .take(limit)
            .map(|entry| {
                json!({
                    "desktop_id": entry.id,
                    "name": entry.name,
                    "summary": entry.summary(),
                    "categories": entry.categories,
                    "terminal": entry.terminal
                })
            })
            .collect::<Vec<_>>();
        Ok(ToolCallResult::json(json!({
            "count": apps.len(),
            "apps": apps
        })))
    }

    fn interaction_domain_status(
        &mut self,
        arguments: Value,
    ) -> Result<ToolCallResult, PlatformError> {
        parse::<NoArgs>(arguments)?;
        let mut client = self.connect_ipc()?;
        let (snapshot, managed) = self.interaction_domain.locate(&mut client)?;
        let interaction_domain = managed.and_then(|managed| {
            snapshot
                .interaction_domains
                .iter()
                .find(|interaction_domain| interaction_domain.id == managed.id)
                .cloned()
        });
        let groups = managed.map_or_else(Vec::new, |managed| {
            snapshot
                .interaction_groups
                .iter()
                .filter(|group| {
                    group.control_interaction_domain == managed.id
                        || group.observer_interaction_domains.contains(&managed.id)
                })
                .cloned()
                .collect::<Vec<_>>()
        });
        Ok(ToolCallResult::json(json!({
            "managed": interaction_domain.is_some(),
            "interaction_domain": interaction_domain,
            "interaction_groups": groups,
            "revision": snapshot.revision
        })))
    }

    fn interaction_domain_ensure(
        &mut self,
        arguments: Value,
    ) -> Result<ToolCallResult, PlatformError> {
        parse::<NoArgs>(arguments)?;
        let mut client = self.connect_ipc_grant()?;
        let managed = self.ensure_interaction_domain(&mut client)?;
        Ok(ToolCallResult::json(json!({
            "status": "active_or_recovered",
            "interaction_domain_id": managed.id.0,
            "revision": managed.revision,
            "label": self.config.interaction_domain_label
        })))
    }

    fn interaction_domain_launch(
        &mut self,
        arguments: Value,
    ) -> Result<ToolCallResult, PlatformError> {
        let args: LaunchArgs = parse(arguments)?;
        if args.desktop_id.trim().is_empty() {
            return Err(invalid("desktop_id must not be empty"));
        }
        let known = aegis_desktop_entries::enumerate()
            .iter()
            .any(|entry| entry.id == args.desktop_id);
        if !known {
            return Err(invalid(format!(
                "desktop_id {:?} is not in the current XDG application catalog; call apps_list first",
                args.desktop_id
            )));
        }
        let mut client = self.connect_ipc_grant()?;
        let managed = self.ensure_interaction_domain(&mut client)?;
        client.launch_in_interaction_domain(managed.id, &args.desktop_id)?;
        Ok(ToolCallResult::json(json!({
            "status": "queued",
            "operation": "interaction_domain_launch_app",
            "interaction_domain_id": managed.id.0,
            "desktop_id": args.desktop_id,
            "verified": false
        })))
    }

    fn interaction_domain_transfer(
        &mut self,
        arguments: Value,
    ) -> Result<ToolCallResult, PlatformError> {
        let args: TransferArgs = parse(arguments)?;
        let window = window_id(args.window_id)?;
        let mut client = self.connect_ipc_grant()?;
        let (target, retain_source_as_observer) = match args.target.as_str() {
            "agent" => {
                let managed = self.ensure_interaction_domain(&mut client)?;
                (managed.id, args.retain_source_as_observer.unwrap_or(true))
            }
            "human" => {
                let (_, managed) = self.interaction_domain.locate(&mut client)?;
                if managed.is_none() {
                    return Err(PlatformError::NoManagedInteractionDomain);
                }
                (
                    HUMAN_INTERACTION_DOMAIN,
                    args.retain_source_as_observer.unwrap_or(false),
                )
            }
            _ => return Err(invalid("target must be `agent` or `human`")),
        };
        let snapshot = client.interaction_domains()?;
        let result = client.interaction_domain_action(InteractionDomainAction::Transact {
            expected_revision: Some(snapshot.revision),
            mutations: vec![InteractionDomainMutation::TransferWindow {
                window,
                target,
                retain_source_as_observer,
            }],
        })?;
        let InteractionDomainActionResult::TransactionCommitted { receipt } = result else {
            return Err(PlatformError::UnexpectedResponse);
        };
        Ok(ToolCallResult::json(json!({
            "status": "committed",
            "target": args.target,
            "receipt": receipt
        })))
    }

    fn interaction_domain_set_state(
        &mut self,
        arguments: Value,
    ) -> Result<ToolCallResult, PlatformError> {
        let args: StateArgs = parse(arguments)?;
        let state = match args.state.as_str() {
            "active" => InteractionDomainState::Active,
            "paused" => InteractionDomainState::Paused,
            _ => return Err(invalid("state must be `active` or `paused`")),
        };
        let mut client = self.connect_ipc_grant()?;
        let managed = self.existing_interaction_domain(&mut client)?;
        let result = client.interaction_domain_action(InteractionDomainAction::Transact {
            expected_revision: Some(managed.revision),
            mutations: vec![InteractionDomainMutation::SetState {
                interaction_domain: managed.id,
                state,
            }],
        })?;
        let InteractionDomainActionResult::TransactionCommitted { receipt } = result else {
            return Err(PlatformError::UnexpectedResponse);
        };
        Ok(ToolCallResult::json(json!({
            "status": "committed",
            "state": args.state,
            "receipt": receipt
        })))
    }

    fn interaction_domain_capture(
        &mut self,
        arguments: Value,
    ) -> Result<ToolCallResult, PlatformError> {
        let args: CaptureArgs = parse(arguments)?;
        let region = args.region.map(TryInto::try_into).transpose()?;
        let mut client = self.connect_ipc_grant()?;
        let managed = self.existing_interaction_domain(&mut client)?;
        let capture = client.capture_interaction_domain(managed.id, region)?;
        let image_path = self.interaction_domain.store_capture(&capture.png)?;
        let image_bytes = capture.png.len();
        let image_png = (image_bytes <= MAX_INLINE_MCP_IMAGE_BYTES).then_some(capture.png);
        let observation_token = capture.observation.token.clone();
        let observation_ttl_ms = capture.observation.ttl_ms;
        let semantic = capture.observation.snapshot;
        self.retain_observation(&observation_token, observation_ttl_ms, client);
        Ok(ToolCallResult {
            value: json!({
                "interaction_domain_id": capture.interaction_domain.0,
                "width": capture.width,
                "height": capture.height,
                "scale_milli": capture.scale_milli,
                "region": capture.region,
                "placements": capture.placements,
                "revision": capture.revision,
                "observation_token": observation_token.0,
                "observation_ttl_ms": observation_ttl_ms,
                "semantic": semantic,
                "image_bytes": image_bytes,
                "image_attached": image_png.is_some(),
                "image_path": image_path
            }),
            image_png,
        })
    }

    fn window_capture(&mut self, arguments: Value) -> Result<ToolCallResult, PlatformError> {
        let args: WindowArgs = parse(arguments)?;
        let window = window_id(args.window_id)?;
        let mut client = self.connect_ipc_grant()?;
        let capture = client.capture_window(window)?;
        let image_path = self.interaction_domain.store_window_capture(&capture.png)?;
        let image_bytes = capture.png.len();
        let image_png = (image_bytes <= MAX_INLINE_MCP_IMAGE_BYTES).then_some(capture.png);
        Ok(ToolCallResult {
            value: json!({
                "window_id": capture.window.0,
                "width": capture.width,
                "height": capture.height,
                "scale_milli": capture.scale_milli,
                "rect": capture.rect,
                "image_bytes": image_bytes,
                "image_attached": image_png.is_some(),
                "image_path": image_path
            }),
            image_png,
        })
    }

    fn interaction_domain_observe(
        &mut self,
        arguments: Value,
    ) -> Result<ToolCallResult, PlatformError> {
        parse::<NoArgs>(arguments)?;
        let mut client = self.connect_ipc_grant()?;
        let managed = self.existing_interaction_domain(&mut client)?;
        let observation = client.observe_interaction_domain(managed.id)?;
        let token = observation.token.clone();
        let ttl_ms = observation.ttl_ms;
        let snapshot = observation.snapshot;
        self.retain_observation(&token, ttl_ms, client);
        Ok(ToolCallResult::json(json!({
            "interaction_domain_id": managed.id.0,
            "observation_token": token.0,
            "observation_ttl_ms": ttl_ms,
            "semantic": snapshot
        })))
    }

    fn interaction_domain_input(
        &mut self,
        arguments: Value,
    ) -> Result<ToolCallResult, PlatformError> {
        let args: InputArgs = parse(arguments)?;
        if args.actions.is_empty() || args.actions.len() > MAX_INPUT_ACTIONS {
            return Err(invalid(format!(
                "actions must contain from 1 through {MAX_INPUT_ACTIONS} entries"
            )));
        }
        let actions = args
            .actions
            .into_iter()
            .map(semantic_action)
            .collect::<Result<Vec<_>, PlatformError>>()?;
        let mut client = self.take_observation_client(&args.observation_token)?;
        let managed = self.existing_interaction_domain(&mut client)?;
        let target = semantic_object_id(args.target_window_id, args.target_local_id)?;
        let receipt = client.act_in_interaction_domain(aegis_ipc::ActorActionIntent {
            interaction_domain: managed.id,
            target,
            observation: ObservationToken(args.observation_token),
            actions,
        })?;
        Ok(ToolCallResult::json(json!({
            "status": "committed",
            "operation": "interaction_domain_input",
            "interaction_domain_id": managed.id.0,
            "target": target,
            "verified": true,
            "receipt": receipt
        })))
    }

    fn interaction_domain_reset(
        &mut self,
        arguments: Value,
    ) -> Result<ToolCallResult, PlatformError> {
        parse::<NoArgs>(arguments)?;
        self.pending_observations.clear();
        let mut client = self.connect_ipc_grant()?;
        let revoked = self.interaction_domain.revoke(&mut client)?;
        Ok(ToolCallResult::json(json!({
            "status": if revoked { "revoked" } else { "not_initialized" },
            "fallback_interaction_domain_id": HUMAN_INTERACTION_DOMAIN.0
        })))
    }

    fn ensure_interaction_domain(
        &mut self,
        client: &mut Client,
    ) -> Result<ManagedInteractionDomain, PlatformError> {
        let (_, managed) = self.interaction_domain.locate(client)?;
        if let Some(managed) = managed {
            return Ok(managed);
        }
        if !self.interaction_domain_op_allowed(ActorCapability::CreateInteractionDomain) {
            return Err(PlatformError::InteractionDomainCreationNotGranted);
        }
        self.interaction_domain.ensure(client).map_err(Into::into)
    }

    fn existing_interaction_domain(
        &mut self,
        client: &mut Client,
    ) -> Result<ManagedInteractionDomain, PlatformError> {
        let (_, managed) = self.interaction_domain.locate(client)?;
        managed.ok_or(PlatformError::NoManagedInteractionDomain)
    }

    fn interaction_domain_op_allowed(&self, op: ActorCapability) -> bool {
        let listed =
            |ops: &Option<Vec<ActorCapability>>| ops.as_ref().is_some_and(|ops| ops.contains(&op));
        self.grant.capabilities.interaction_domain
            && (listed(&self.grant.scope.ops) || listed(&self.grant.scope.ask_ops))
    }

    fn can_revoke_interaction_domain(&self) -> bool {
        self.interaction_domain_op_allowed(ActorCapability::RevokeInteractionDomain)
    }
}

fn connect_with(
    config: &BridgeConfig,
    identity: &crate::identity::IdentityStore,
    post_timeout: Duration,
) -> Result<Client, PlatformError> {
    // The first connection may block on the interactive pairing prompt, so
    // the handshake gets a generous bound; per-request I/O falls back to the
    // configured timeout right after.
    let handshake_timeout = config.io_timeout.max(GRANT_TIMEOUT);
    let client = Client::connect_agent_with_timeout(
        &config.socket_path,
        ConnectionCapabilities {
            query: true,
            control: true,
            input: true,
            session: false,
            interaction_domain: true,
        },
        None,
        aegis_ipc::AgentHello {
            label: Some(config.label.clone()),
            requested: catalog_ops(),
            credential: identity.credential(),
        },
        handshake_timeout,
    )
    .map_err(|source| PlatformError::Connect {
        socket: config.socket_path.clone(),
        label: config.label.clone(),
        source,
    })?;
    client
        .set_io_timeout(Some(post_timeout))
        .map_err(|source| PlatformError::Connect {
            socket: config.socket_path.clone(),
            label: config.label.clone(),
            source,
        })?;
    if let Some(issued) = client.agent_issued() {
        if let Some(credential) = &issued.credential {
            identity
                .store(&issued.principal, credential)
                .map_err(PlatformError::Identity)?;
        } else {
            identity
                .confirm_principal(&issued.principal)
                .map_err(PlatformError::Identity)?;
        }
    }
    Ok(client)
}
