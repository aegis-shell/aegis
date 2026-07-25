use std::path::PathBuf;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use aegis_core::input::SyntheticInputAction;
use aegis_core::realm::{HUMAN_REALM, RealmId, RealmMutation, RealmState};
use aegis_core::window::WindowId;
use aegis_core::workspace::{Switch, WorkspaceId};
use aegis_core::{Point, Rect};
use aegis_ipc::{
    Capabilities, Client, Command, Effect, JournalMutation, OpClass, RealmAction,
    RealmActionResult, Scope,
};
use serde::Deserialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

use crate::bridge::BridgeConfig;
use crate::bridge::realm::{ManagedRealm, RealmSession, RealmSessionError};

const MAX_JOURNAL_ENTRIES: usize = 200;
const MAX_APP_RESULTS: usize = 200;
const MAX_INPUT_ACTIONS: usize = 64;
const MAX_INLINE_MCP_IMAGE_BYTES: usize = 32 * 1024 * 1024;

/// Capabilities and resource/operation allowlists observed at startup.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct ToolGrant {
    pub capabilities: Capabilities,
    pub scope: Scope,
}

/// Evidence returned by the live, low-risk compositor smoke test.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SmokeReport {
    pub status: &'static str,
    pub mode: &'static str,
    pub scope: String,
    pub notification: SmokeNotificationReport,
    pub realm: SmokeRealmReport,
    pub visual: SmokeVisualReport,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SmokeNotificationReport {
    pub started_id: u64,
    pub id: u64,
    pub summary: String,
    pub observed_in_compositor_state: bool,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SmokeRealmReport {
    pub id: u64,
    pub created_by_smoke: bool,
    pub lifecycle: Vec<String>,
    pub cleanup: &'static str,
}

#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SmokeVisualReport {
    pub status_indicator: &'static str,
    pub details_surface: &'static str,
    pub observation_millis: u128,
    pub input_probe: Option<SmokeInputReport>,
}

/// Evidence for the opt-in, non-clicking Agent input smoke probe.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub struct SmokeInputReport {
    pub window_id: u64,
    pub action: &'static str,
    pub local_position: Point,
    pub journal_sequence: u64,
    pub applied: bool,
    pub window_restored_to_human: bool,
}

/// One MCP-advertised tool definition.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Value,
    pub read_only: bool,
    pub destructive: bool,
}

impl ToolDefinition {
    pub fn to_mcp(&self) -> Value {
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": {
                "readOnlyHint": self.read_only,
                "destructiveHint": self.destructive,
                "idempotentHint": self.read_only,
                "openWorldHint": false
            }
        })
    }
}

/// Successful platform call plus optional MCP image content.
#[derive(Debug)]
pub(crate) struct ToolCallResult {
    pub value: Value,
    pub image_png: Option<Vec<u8>>,
}

impl ToolCallResult {
    pub(crate) fn json(value: Value) -> Self {
        Self {
            value,
            image_png: None,
        }
    }
}

/// Scoped ASS platform service consumed by the MCP transport.
pub struct AssPlatform {
    config: BridgeConfig,
    grant: ToolGrant,
    realm: RealmSession,
}

impl AssPlatform {
    /// Probe the compositor grant and acquire the per-scope Realm recovery lock.
    pub fn connect(config: BridgeConfig) -> Result<Self, PlatformError> {
        config.validate()?;
        let client = connect_with(&config)?;
        let grant = ToolGrant {
            capabilities: client.caps(),
            scope: client.scope().clone(),
        };
        Ok(Self {
            realm: RealmSession::acquire(&config)?,
            config,
            grant,
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

    /// Exercise a real, reversible compositor mutation and Realm lifecycle.
    ///
    /// The notification is re-read from compositor state rather than treating
    /// an IPC `Ok` as proof. A newly created test Realm is synchronously
    /// transitioned through paused and active states, left visible for
    /// `observation`, then revoked. A recovered pre-existing Realm is only
    /// observed and preserved so smoke testing never takes authority away from
    /// user work.
    pub fn smoke(&mut self, observation: Duration) -> Result<SmokeReport, PlatformError> {
        self.smoke_with_input(observation, None)
    }

    /// Run the live smoke test and optionally transfer one explicitly selected
    /// human-controlled window long enough to apply a harmless pointer move.
    /// The move is verified through the compositor journal and the window is
    /// returned to the human Realm during cleanup.
    pub fn smoke_with_input(
        &mut self,
        observation: Duration,
        input_window: Option<WindowId>,
    ) -> Result<SmokeReport, PlatformError> {
        let mut required = vec![
            ToolKind::PostNotification,
            ToolKind::RealmEnsure,
            ToolKind::RealmSetState,
            ToolKind::RealmReset,
        ];
        if input_window.is_some() {
            required.extend([ToolKind::RealmTransferWindow, ToolKind::RealmInput]);
        }
        for kind in required {
            if !kind.allowed(&self.grant) {
                return Err(PlatformError::NotGranted(
                    kind.definition().name.to_string(),
                ));
            }
        }

        let mut client = self.connect_ipc()?;
        let (_, existing) = self.realm.locate(&mut client)?;
        let marker = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tag = format!("{:06x}", marker & 0xff_ffff);
        let started_summary = format!("Fuji ↔ ASS · {tag}");
        client.command(Command::Notify {
            summary: started_summary.clone(),
            body: "Live notification verified. Agent Realm smoke is running.".into(),
            app_id: Some("fuji".into()),
        })?;
        let started_notification = self.wait_for_notification(&mut client, &started_summary)?;

        let created_by_smoke = existing.is_none();
        let managed = match existing {
            Some(managed) => managed,
            None => self.ensure_realm(&mut client)?,
        };
        let mut lifecycle = vec![self.verified_realm_state(&mut client, managed.id)?];
        let mut input_probe = None;

        let cleanup = if created_by_smoke {
            let paused_for = observation.min(Duration::from_secs(2));
            let active_for = observation.saturating_sub(paused_for);
            let exercise = (|| {
                self.set_smoke_realm_state(&mut client, managed.id, RealmState::Paused)?;
                lifecycle.push(self.verified_realm_state(&mut client, managed.id)?);
                if !paused_for.is_zero() {
                    std::thread::sleep(paused_for);
                }
                self.set_smoke_realm_state(&mut client, managed.id, RealmState::Active)?;
                lifecycle.push(self.verified_realm_state(&mut client, managed.id)?);
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
            let revoked = self.realm.revoke(&mut client);
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
                    "new smoke Realm was not present during cleanup".into(),
                ));
            }
            let snapshot = client.realms()?;
            if snapshot
                .realms
                .iter()
                .any(|realm| realm.id == managed.id && realm.state != RealmState::Revoked)
            {
                return Err(PlatformError::SmokeVerification(
                    "smoke Realm remained live after revocation".into(),
                ));
            }
            lifecycle.push("revoked".into());
            if let Some(probe) = input_probe.as_mut() {
                probe.window_restored_to_human = self
                    .window_control_realm(&mut client, WindowId(probe.window_id))?
                    == Some(HUMAN_REALM);
                if !probe.window_restored_to_human {
                    return Err(PlatformError::SmokeVerification(format!(
                        "window {} did not return to the human Realm after smoke revocation",
                        probe.window_id
                    )));
                }
            }
            "revoked_test_realm"
        } else {
            let exercise = (|| {
                if let Some(window) = input_window {
                    if self.verified_realm_state(&mut client, managed.id)? != "active" {
                        return Err(PlatformError::SmokeVerification(
                            "the recovered managed Realm is paused; resume or reset it before an input smoke probe"
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
                    .window_control_realm(&mut client, WindowId(probe.window_id))?
                    == Some(HUMAN_REALM);
                if !probe.window_restored_to_human {
                    return Err(PlatformError::SmokeVerification(format!(
                        "window {} did not return to the human Realm after the smoke probe",
                        probe.window_id
                    )));
                }
            }
            "preserved_existing_realm"
        };

        let summary = format!("Fuji ↔ ASS · passed · {tag}");
        client.command(Command::Notify {
            summary: summary.clone(),
            body: "Notification and Agent Realm controls were applied and verified.".into(),
            app_id: Some("fuji".into()),
        })?;
        let notification = self.wait_for_notification(&mut client, &summary)?;

        Ok(SmokeReport {
            status: "passed",
            mode: "live",
            scope: self.config.scope.clone(),
            notification: SmokeNotificationReport {
                started_id: started_notification.id,
                id: notification.id,
                summary,
                observed_in_compositor_state: true,
            },
            realm: SmokeRealmReport {
                id: managed.id.0,
                created_by_smoke,
                lifecycle,
                cleanup,
            },
            visual: SmokeVisualReport {
                status_indicator: "persistent while the Agent Realm is live",
                details_surface: "click the status indicator to open Control Center → AI Workspaces",
                observation_millis: observation.as_millis(),
                input_probe,
            },
        })
    }

    fn exercise_agent_pointer(
        &self,
        client: &mut Client,
        realm: RealmId,
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
        if self.window_control_realm(client, window)? != Some(HUMAN_REALM) {
            return Err(PlatformError::SmokeVerification(format!(
                "window {} is not currently controlled by the human Realm",
                window.0
            )));
        }

        let local_position = Point {
            x: (target.size.w / 2).clamp(0, target.size.w - 1),
            y: (target.size.h / 2).clamp(0, target.size.h - 1),
        };
        let snapshot = client.realms()?;
        let result = client.realm_action(RealmAction::Transact {
            expected_revision: Some(snapshot.revision),
            mutations: vec![RealmMutation::TransferWindow {
                window,
                target: realm,
                retain_source_as_observer: true,
            }],
        })?;
        if !matches!(result, RealmActionResult::TransactionCommitted { .. }) {
            return Err(PlatformError::UnexpectedResponse);
        }

        let baseline = client.journal(0)?.latest_seq;
        let command = Command::InjectRealmInput {
            realm,
            id: window,
            actions: vec![SyntheticInputAction::PointerMove {
                position: local_position,
            }],
        };
        client.command(command.clone())?;
        let deadline = Instant::now() + self.config.io_timeout;
        loop {
            let journal = client.journal(baseline)?;
            if let Some(entry) = journal.entries.into_iter().find(|entry| {
                matches!(
                    &entry.mutation,
                    JournalMutation::Command { cmd } if cmd == &command
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
        let snapshot = client.realms()?;
        if self.window_control_realm(client, window)? == Some(HUMAN_REALM) {
            return Ok(());
        }
        let result = client.realm_action(RealmAction::Transact {
            expected_revision: Some(snapshot.revision),
            mutations: vec![RealmMutation::TransferWindow {
                window,
                target: HUMAN_REALM,
                retain_source_as_observer: false,
            }],
        })?;
        if !matches!(result, RealmActionResult::TransactionCommitted { .. }) {
            return Err(PlatformError::UnexpectedResponse);
        }
        Ok(())
    }

    fn window_control_realm(
        &self,
        client: &mut Client,
        window: WindowId,
    ) -> Result<Option<RealmId>, PlatformError> {
        Ok(client
            .realms()?
            .interaction_groups
            .into_iter()
            .find(|group| group.windows.contains(&window))
            .map(|group| group.control_realm))
    }

    pub(crate) fn definitions(&self) -> Vec<ToolDefinition> {
        ToolKind::ALL
            .iter()
            .copied()
            .filter(|kind| kind.allowed(&self.grant))
            .map(ToolKind::definition)
            .collect()
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
        if !self.config.revoke_on_exit {
            return Ok(());
        }
        let mut client = self.connect_ipc()?;
        let (_, managed) = self.realm.locate(&mut client)?;
        if managed.is_none() {
            return Ok(());
        }
        if !self.can_revoke_realm() {
            return Err(PlatformError::RealmCleanupNotGranted);
        }
        self.realm.revoke(&mut client)?;
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
                    "realms": client.realms()?
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
            ToolKind::FocusWindow => self.command(
                window_command(arguments, |id| Command::Focus { id })?,
                "focus_window",
            ),
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
                        app_id: Some("fuji".into()),
                    },
                    "post_notification",
                )
            }
            ToolKind::RealmStatus => self.realm_status(arguments),
            ToolKind::RealmEnsure => self.realm_ensure(arguments),
            ToolKind::RealmLaunchApp => self.realm_launch(arguments),
            ToolKind::RealmTransferWindow => self.realm_transfer(arguments),
            ToolKind::RealmSetState => self.realm_set_state(arguments),
            ToolKind::RealmCapture => self.realm_capture(arguments),
            ToolKind::RealmInput => self.realm_input(arguments),
            ToolKind::RealmReset => self.realm_reset(arguments),
        }
    }

    fn connect_ipc(&self) -> Result<Client, PlatformError> {
        connect_with(&self.config)
    }

    fn wait_for_notification(
        &self,
        client: &mut Client,
        summary: &str,
    ) -> Result<aegis_core::notify::Notification, PlatformError> {
        let deadline = Instant::now() + self.config.io_timeout;
        loop {
            if let Some(notification) = client.notifications()?.into_iter().find(|notification| {
                notification.summary == summary && notification.app_id.as_deref() == Some("fuji")
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

    fn set_smoke_realm_state(
        &self,
        client: &mut Client,
        realm: aegis_core::realm::RealmId,
        state: RealmState,
    ) -> Result<(), PlatformError> {
        let snapshot = client.realms()?;
        let result = client.realm_action(RealmAction::Transact {
            expected_revision: Some(snapshot.revision),
            mutations: vec![RealmMutation::SetState { realm, state }],
        })?;
        if !matches!(result, RealmActionResult::TransactionCommitted { .. }) {
            return Err(PlatformError::UnexpectedResponse);
        }
        Ok(())
    }

    fn verified_realm_state(
        &self,
        client: &mut Client,
        realm: aegis_core::realm::RealmId,
    ) -> Result<String, PlatformError> {
        let snapshot = client.realms()?;
        let state = snapshot
            .realms
            .iter()
            .find(|candidate| candidate.id == realm)
            .map(|candidate| candidate.state)
            .ok_or_else(|| {
                PlatformError::SmokeVerification(format!(
                    "Realm {} was committed but was not queryable",
                    realm.0
                ))
            })?;
        Ok(match state {
            RealmState::Active => "active",
            RealmState::Paused => "paused",
            RealmState::Revoked => "revoked",
        }
        .into())
    }

    fn command(
        &self,
        command: Command,
        operation: &'static str,
    ) -> Result<ToolCallResult, PlatformError> {
        let mut client = self.connect_ipc()?;
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

    fn realm_status(&mut self, arguments: Value) -> Result<ToolCallResult, PlatformError> {
        parse::<NoArgs>(arguments)?;
        let mut client = self.connect_ipc()?;
        let (snapshot, managed) = self.realm.locate(&mut client)?;
        let realm = managed.and_then(|managed| {
            snapshot
                .realms
                .iter()
                .find(|realm| realm.id == managed.id)
                .cloned()
        });
        let groups = managed.map_or_else(Vec::new, |managed| {
            snapshot
                .interaction_groups
                .iter()
                .filter(|group| {
                    group.control_realm == managed.id || group.observer_realms.contains(&managed.id)
                })
                .cloned()
                .collect::<Vec<_>>()
        });
        Ok(ToolCallResult::json(json!({
            "managed": realm.is_some(),
            "realm": realm,
            "interaction_groups": groups,
            "revision": snapshot.revision
        })))
    }

    fn realm_ensure(&mut self, arguments: Value) -> Result<ToolCallResult, PlatformError> {
        parse::<NoArgs>(arguments)?;
        let mut client = self.connect_ipc()?;
        let managed = self.ensure_realm(&mut client)?;
        Ok(ToolCallResult::json(json!({
            "status": "active_or_recovered",
            "realm_id": managed.id.0,
            "revision": managed.revision,
            "label": self.config.realm_label
        })))
    }

    fn realm_launch(&mut self, arguments: Value) -> Result<ToolCallResult, PlatformError> {
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
        let mut client = self.connect_ipc()?;
        let managed = self.ensure_realm(&mut client)?;
        client.launch_in_realm(managed.id, &args.desktop_id)?;
        Ok(ToolCallResult::json(json!({
            "status": "queued",
            "operation": "realm_launch_app",
            "realm_id": managed.id.0,
            "desktop_id": args.desktop_id,
            "verified": false
        })))
    }

    fn realm_transfer(&mut self, arguments: Value) -> Result<ToolCallResult, PlatformError> {
        let args: TransferArgs = parse(arguments)?;
        let window = window_id(args.window_id)?;
        let mut client = self.connect_ipc()?;
        let (target, retain_source_as_observer) = match args.target.as_str() {
            "fuji" => {
                let managed = self.ensure_realm(&mut client)?;
                (managed.id, args.retain_source_as_observer.unwrap_or(true))
            }
            "human" => {
                let (_, managed) = self.realm.locate(&mut client)?;
                if managed.is_none() {
                    return Err(PlatformError::NoManagedRealm);
                }
                (HUMAN_REALM, args.retain_source_as_observer.unwrap_or(false))
            }
            _ => return Err(invalid("target must be `fuji` or `human`")),
        };
        let snapshot = client.realms()?;
        let result = client.realm_action(RealmAction::Transact {
            expected_revision: Some(snapshot.revision),
            mutations: vec![RealmMutation::TransferWindow {
                window,
                target,
                retain_source_as_observer,
            }],
        })?;
        let RealmActionResult::TransactionCommitted { receipt } = result else {
            return Err(PlatformError::UnexpectedResponse);
        };
        Ok(ToolCallResult::json(json!({
            "status": "committed",
            "target": args.target,
            "receipt": receipt
        })))
    }

    fn realm_set_state(&mut self, arguments: Value) -> Result<ToolCallResult, PlatformError> {
        let args: StateArgs = parse(arguments)?;
        let state = match args.state.as_str() {
            "active" => RealmState::Active,
            "paused" => RealmState::Paused,
            _ => return Err(invalid("state must be `active` or `paused`")),
        };
        let mut client = self.connect_ipc()?;
        let managed = self.existing_realm(&mut client)?;
        let result = client.realm_action(RealmAction::Transact {
            expected_revision: Some(managed.revision),
            mutations: vec![RealmMutation::SetState {
                realm: managed.id,
                state,
            }],
        })?;
        let RealmActionResult::TransactionCommitted { receipt } = result else {
            return Err(PlatformError::UnexpectedResponse);
        };
        Ok(ToolCallResult::json(json!({
            "status": "committed",
            "state": args.state,
            "receipt": receipt
        })))
    }

    fn realm_capture(&mut self, arguments: Value) -> Result<ToolCallResult, PlatformError> {
        let args: CaptureArgs = parse(arguments)?;
        let region = args.region.map(TryInto::try_into).transpose()?;
        let mut client = self.connect_ipc()?;
        let managed = self.existing_realm(&mut client)?;
        let capture = client.capture_realm(managed.id, region)?;
        let image_path = self.realm.store_capture(&capture.png)?;
        let image_bytes = capture.png.len();
        let image_png = (image_bytes <= MAX_INLINE_MCP_IMAGE_BYTES).then_some(capture.png);
        Ok(ToolCallResult {
            value: json!({
                "realm_id": capture.realm.0,
                "width": capture.width,
                "height": capture.height,
                "scale_milli": capture.scale_milli,
                "region": capture.region,
                "placements": capture.placements,
                "revision": capture.revision,
                "image_bytes": image_bytes,
                "image_attached": image_png.is_some(),
                "image_path": image_path
            }),
            image_png,
        })
    }

    fn realm_input(&mut self, arguments: Value) -> Result<ToolCallResult, PlatformError> {
        let args: InputArgs = parse(arguments)?;
        if args.actions.is_empty() || args.actions.len() > MAX_INPUT_ACTIONS {
            return Err(invalid(format!(
                "actions must contain from 1 through {MAX_INPUT_ACTIONS} entries"
            )));
        }
        let actions = args
            .actions
            .into_iter()
            .map(TryInto::try_into)
            .collect::<Result<Vec<_>, PlatformError>>()?;
        let mut client = self.connect_ipc()?;
        let managed = self.existing_realm(&mut client)?;
        client.inject_realm_input(managed.id, window_id(args.window_id)?, actions)?;
        Ok(ToolCallResult::json(json!({
            "status": "queued",
            "operation": "realm_input",
            "realm_id": managed.id.0,
            "window_id": args.window_id,
            "verified": false
        })))
    }

    fn realm_reset(&mut self, arguments: Value) -> Result<ToolCallResult, PlatformError> {
        parse::<NoArgs>(arguments)?;
        let mut client = self.connect_ipc()?;
        let revoked = self.realm.revoke(&mut client)?;
        Ok(ToolCallResult::json(json!({
            "status": if revoked { "revoked" } else { "not_initialized" },
            "fallback_realm_id": HUMAN_REALM.0
        })))
    }

    fn ensure_realm(&mut self, client: &mut Client) -> Result<ManagedRealm, PlatformError> {
        let (_, managed) = self.realm.locate(client)?;
        if let Some(managed) = managed {
            return Ok(managed);
        }
        if !self.realm_op_allowed(OpClass::CreateRealm) {
            return Err(PlatformError::RealmCreationNotGranted);
        }
        self.realm.ensure(client).map_err(Into::into)
    }

    fn existing_realm(&mut self, client: &mut Client) -> Result<ManagedRealm, PlatformError> {
        let (_, managed) = self.realm.locate(client)?;
        managed.ok_or(PlatformError::NoManagedRealm)
    }

    fn realm_op_allowed(&self, op: OpClass) -> bool {
        self.grant.capabilities.realm
            && self
                .grant
                .scope
                .ops
                .as_ref()
                .is_some_and(|ops| ops.contains(&op))
    }

    fn can_revoke_realm(&self) -> bool {
        self.realm_op_allowed(OpClass::RevokeRealm)
    }
}

fn connect_with(config: &BridgeConfig) -> Result<Client, PlatformError> {
    Client::connect_scoped_with_timeout(
        &config.socket_path,
        Capabilities {
            query: true,
            control: true,
            input: true,
            session: false,
            realm: true,
        },
        config.scope.clone(),
        config.io_timeout,
    )
    .map_err(|source| PlatformError::Connect {
        socket: config.socket_path.clone(),
        scope: config.scope.clone(),
        source,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ToolKind {
    DesktopSnapshot,
    DesktopJournal,
    AppsList,
    FocusWindow,
    MinimizeWindow,
    CloseWindow,
    MoveWindowToWorkspace,
    SwitchWorkspace,
    SwitchWorkspaceTo,
    SetWindowGeometry,
    ToggleTiling,
    ToggleOverview,
    PostNotification,
    RealmStatus,
    RealmEnsure,
    RealmLaunchApp,
    RealmTransferWindow,
    RealmSetState,
    RealmCapture,
    RealmInput,
    RealmReset,
}

impl ToolKind {
    const ALL: [Self; 21] = [
        Self::DesktopSnapshot,
        Self::DesktopJournal,
        Self::AppsList,
        Self::FocusWindow,
        Self::MinimizeWindow,
        Self::CloseWindow,
        Self::MoveWindowToWorkspace,
        Self::SwitchWorkspace,
        Self::SwitchWorkspaceTo,
        Self::SetWindowGeometry,
        Self::ToggleTiling,
        Self::ToggleOverview,
        Self::PostNotification,
        Self::RealmStatus,
        Self::RealmEnsure,
        Self::RealmLaunchApp,
        Self::RealmTransferWindow,
        Self::RealmSetState,
        Self::RealmCapture,
        Self::RealmInput,
        Self::RealmReset,
    ];

    fn from_name(name: &str) -> Option<Self> {
        Self::ALL
            .iter()
            .copied()
            .find(|kind| kind.definition().name == name)
    }

    fn allowed(self, grant: &ToolGrant) -> bool {
        if matches!(
            self,
            Self::DesktopSnapshot | Self::DesktopJournal | Self::AppsList | Self::RealmStatus
        ) {
            return grant.capabilities.query;
        }
        let (capability, op) = match self {
            Self::FocusWindow => (grant.capabilities.control, OpClass::Focus),
            Self::MinimizeWindow => (grant.capabilities.control, OpClass::Minimize),
            Self::CloseWindow => (grant.capabilities.control, OpClass::Close),
            Self::MoveWindowToWorkspace => (grant.capabilities.control, OpClass::MoveToWorkspace),
            Self::SwitchWorkspace => (grant.capabilities.control, OpClass::SwitchWorkspace),
            Self::SwitchWorkspaceTo => (grant.capabilities.control, OpClass::SwitchWorkspaceTo),
            Self::SetWindowGeometry => (grant.capabilities.control, OpClass::SetWindowGeometry),
            Self::ToggleTiling => (grant.capabilities.control, OpClass::ToggleTiling),
            Self::ToggleOverview => (grant.capabilities.control, OpClass::ToggleOverview),
            Self::PostNotification => (grant.capabilities.control, OpClass::Notify),
            Self::RealmEnsure => (grant.capabilities.realm, OpClass::CreateRealm),
            Self::RealmLaunchApp => (grant.capabilities.realm, OpClass::LaunchInRealm),
            Self::RealmTransferWindow | Self::RealmSetState => {
                (grant.capabilities.realm, OpClass::TransactRealm)
            }
            Self::RealmCapture => (grant.capabilities.realm, OpClass::CaptureRealm),
            Self::RealmInput => (grant.capabilities.input, OpClass::InjectRealmInput),
            Self::RealmReset => (grant.capabilities.realm, OpClass::RevokeRealm),
            Self::DesktopSnapshot | Self::DesktopJournal | Self::AppsList | Self::RealmStatus => {
                unreachable!("query tools returned above")
            }
        };
        capability
            && grant.scope.ops.as_ref().is_none_or(|ops| ops.contains(&op))
            && if matches!(
                self,
                Self::RealmEnsure
                    | Self::RealmLaunchApp
                    | Self::RealmTransferWindow
                    | Self::RealmSetState
                    | Self::RealmCapture
                    | Self::RealmInput
                    | Self::RealmReset
            ) {
                // Realm and input operations are never inherited from an
                // omitted op allowlist; mirror the compositor's fail-closed rule.
                grant
                    .scope
                    .ops
                    .as_ref()
                    .is_some_and(|ops| ops.contains(&op))
            } else {
                true
            }
    }

    #[allow(clippy::too_many_lines)]
    fn definition(self) -> ToolDefinition {
        let empty = || json!({"type": "object", "properties": {}, "additionalProperties": false});
        match self {
            Self::DesktopSnapshot => definition(
                "desktop_snapshot",
                "Read current ASS windows, workspaces, outputs, all Realms, and this connector's granted scope. Call before addressing desktop objects by id.",
                empty(),
                true,
                false,
            ),
            Self::DesktopJournal => definition(
                "desktop_journal",
                "Read ordered compositor mutations after a sequence number. Use this to verify queued desktop commands.",
                json!({"type":"object","properties":{"since":{"type":"integer","minimum":0},"limit":{"type":"integer","minimum":1,"maximum":MAX_JOURNAL_ENTRIES}},"additionalProperties":false}),
                true,
                false,
            ),
            Self::AppsList => definition(
                "apps_list",
                "Search the host XDG application catalog. Use the returned desktop_id with realm_launch_app; never invent desktop ids.",
                json!({"type":"object","properties":{"query":{"type":"string"},"limit":{"type":"integer","minimum":1,"maximum":MAX_APP_RESULTS}},"additionalProperties":false}),
                true,
                false,
            ),
            Self::FocusWindow => definition(
                "focus_window",
                "Queue focus for a live window id.",
                id_schema("window_id"),
                false,
                false,
            ),
            Self::MinimizeWindow => definition(
                "minimize_window",
                "Queue minimization for a live window id.",
                id_schema("window_id"),
                false,
                false,
            ),
            Self::CloseWindow => definition(
                "close_window",
                "Request that a live window close. Use only when the user explicitly asks to close it.",
                id_schema("window_id"),
                false,
                true,
            ),
            Self::MoveWindowToWorkspace => definition(
                "move_window_to_workspace",
                "Queue moving a window to a workspace by durable ids.",
                json!({"type":"object","properties":{"window_id":{"type":"integer","minimum":1},"workspace_id":{"type":"integer","minimum":1}},"required":["window_id","workspace_id"],"additionalProperties":false}),
                false,
                false,
            ),
            Self::SwitchWorkspace => definition(
                "switch_workspace",
                "Queue switching to the next or previous workspace.",
                json!({"type":"object","properties":{"direction":{"type":"string","enum":["next","previous"]}},"required":["direction"],"additionalProperties":false}),
                false,
                false,
            ),
            Self::SwitchWorkspaceTo => definition(
                "switch_workspace_to",
                "Queue switching to a workspace by durable id.",
                id_schema("workspace_id"),
                false,
                false,
            ),
            Self::SetWindowGeometry => definition(
                "set_window_geometry",
                "Queue floating-window geometry in compositor logical coordinates.",
                json!({"type":"object","properties":{"window_id":{"type":"integer","minimum":1},"x":{"type":"integer"},"y":{"type":"integer"},"width":{"type":"integer","minimum":1},"height":{"type":"integer","minimum":1}},"required":["window_id","x","y","width","height"],"additionalProperties":false}),
                false,
                false,
            ),
            Self::ToggleTiling => definition(
                "toggle_tiling",
                "Toggle the current workspace between tiled and floating layout.",
                empty(),
                false,
                false,
            ),
            Self::ToggleOverview => definition(
                "toggle_overview",
                "Toggle the ASS workspace overview.",
                empty(),
                false,
                false,
            ),
            Self::PostNotification => definition(
                "post_notification",
                "Post a user-visible notification from fuji.",
                json!({"type":"object","properties":{"summary":{"type":"string","minLength":1},"body":{"type":"string"}},"required":["summary"],"additionalProperties":false}),
                false,
                false,
            ),
            Self::RealmStatus => definition(
                "realm_status",
                "Inspect only the Agent Realm managed by this fuji connector and its controlled or observed interaction groups. Does not create a Realm.",
                empty(),
                true,
                false,
            ),
            Self::RealmEnsure => definition(
                "realm_ensure",
                "Create or recover this connector's private Agent Realm. The Realm id is managed internally and is never caller-selected.",
                empty(),
                false,
                false,
            ),
            Self::RealmLaunchApp => definition(
                "realm_launch_app",
                "Launch one catalogued desktop application inside fuji's private Agent Realm and sandbox. Call apps_list first.",
                json!({"type":"object","properties":{"desktop_id":{"type":"string","minLength":1,"maxLength":512}},"required":["desktop_id"],"additionalProperties":false}),
                false,
                false,
            ),
            Self::RealmTransferWindow => definition(
                "realm_transfer_window",
                "Atomically transfer interaction authority for a window into fuji's Realm or back to the human Realm. Human observation is retained by default when transferring to fuji.",
                json!({"type":"object","properties":{"window_id":{"type":"integer","minimum":1},"target":{"type":"string","enum":["fuji","human"]},"retain_source_as_observer":{"type":"boolean"}},"required":["window_id","target"],"additionalProperties":false}),
                false,
                false,
            ),
            Self::RealmSetState => definition(
                "realm_set_state",
                "Pause or resume fuji's managed Realm using an optimistic Realm transaction.",
                json!({"type":"object","properties":{"state":{"type":"string","enum":["active","paused"]}},"required":["state"],"additionalProperties":false}),
                false,
                false,
            ),
            Self::RealmCapture => definition(
                "realm_capture",
                "Capture only fuji's directed virtual output. Returns layout metadata, an owner-only PNG path, and an attached image when it is within the inline limit; never captures compositor chrome or another Realm.",
                json!({"type":"object","properties":{"region":{"type":"object","properties":{"x":{"type":"integer"},"y":{"type":"integer"},"width":{"type":"integer","minimum":1},"height":{"type":"integer","minimum":1}},"required":["x","y","width","height"],"additionalProperties":false}},"additionalProperties":false}),
                false,
                false,
            ),
            Self::RealmInput => definition(
                "realm_input",
                "Inject a bounded batch of target-local pointer, click, scroll, or evdev key-press actions through fuji's independent Realm seat. Call realm_capture first and use its placement metadata.",
                input_schema(),
                false,
                false,
            ),
            Self::RealmReset => definition(
                "realm_reset",
                "Permanently revoke fuji's managed Realm and atomically return its controlled windows to the human Realm. Use only when the user explicitly requests reset or shutdown.",
                empty(),
                false,
                true,
            ),
        }
    }
}

fn definition(
    name: &'static str,
    description: &'static str,
    input_schema: Value,
    read_only: bool,
    destructive: bool,
) -> ToolDefinition {
    ToolDefinition {
        name,
        description,
        input_schema,
        read_only,
        destructive,
    }
}

fn id_schema(name: &str) -> Value {
    json!({
        "type": "object",
        "properties": {name: {"type": "integer", "minimum": 1}},
        "required": [name],
        "additionalProperties": false
    })
}

fn input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "window_id": {"type": "integer", "minimum": 1},
            "actions": {
                "type": "array",
                "minItems": 1,
                "maxItems": MAX_INPUT_ACTIONS,
                "items": {
                    "oneOf": [
                        {"type":"object","properties":{"type":{"const":"pointer_move"},"x":{"type":"integer","minimum":0},"y":{"type":"integer","minimum":0}},"required":["type","x","y"],"additionalProperties":false},
                        {"type":"object","properties":{"type":{"const":"click"},"x":{"type":"integer","minimum":0},"y":{"type":"integer","minimum":0},"button":{"type":"string","enum":["left","right","middle","side","extra"]}},"required":["type","x","y","button"],"additionalProperties":false},
                        {"type":"object","properties":{"type":{"const":"scroll"},"x":{"type":"integer","minimum":0},"y":{"type":"integer","minimum":0},"dx":{"type":"number","minimum":-1000,"maximum":1000},"dy":{"type":"number","minimum":-1000,"maximum":1000}},"required":["type","x","y","dx","dy"],"additionalProperties":false},
                        {"type":"object","properties":{"type":{"const":"key_press"},"code":{"type":"integer","minimum":0,"maximum":767,"description":"Linux evdev key code"}},"required":["type","code"],"additionalProperties":false}
                    ]
                }
            }
        },
        "required": ["window_id", "actions"],
        "additionalProperties": false
    })
}

fn parse<T: DeserializeOwned>(arguments: Value) -> Result<T, PlatformError> {
    let arguments = if arguments.is_null() {
        json!({})
    } else {
        arguments
    };
    serde_json::from_value(arguments).map_err(|source| invalid(source.to_string()))
}

fn invalid(message: impl Into<String>) -> PlatformError {
    PlatformError::InvalidArguments(message.into())
}

fn window_id(id: u64) -> Result<WindowId, PlatformError> {
    (id != 0)
        .then_some(WindowId(id))
        .ok_or_else(|| invalid("window_id must be greater than zero"))
}

fn workspace_id(id: u64) -> Result<WorkspaceId, PlatformError> {
    (id != 0)
        .then_some(WorkspaceId(id))
        .ok_or_else(|| invalid("workspace_id must be greater than zero"))
}

fn rect(x: i32, y: i32, width: i32, height: i32) -> Result<Rect, PlatformError> {
    if width <= 0 || height <= 0 {
        return Err(invalid("width and height must be positive"));
    }
    Ok(Rect::new(x, y, width, height))
}

fn window_command(
    arguments: Value,
    command: impl FnOnce(WindowId) -> Command,
) -> Result<Command, PlatformError> {
    let args: WindowArgs = parse(arguments)?;
    Ok(command(window_id(args.window_id)?))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NoArgs {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct JournalArgs {
    #[serde(default)]
    since: Option<u64>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AppsArgs {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowArgs {
    window_id: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct WorkspaceArgs {
    workspace_id: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveArgs {
    window_id: u64,
    workspace_id: u64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DirectionArgs {
    direction: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GeometryArgs {
    window_id: u64,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NotificationArgs {
    summary: String,
    #[serde(default)]
    body: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LaunchArgs {
    desktop_id: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransferArgs {
    window_id: u64,
    target: String,
    #[serde(default)]
    retain_source_as_observer: Option<bool>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StateArgs {
    state: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureArgs {
    #[serde(default)]
    region: Option<RegionArgs>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RegionArgs {
    x: i32,
    y: i32,
    width: i32,
    height: i32,
}

impl TryFrom<RegionArgs> for Rect {
    type Error = PlatformError;

    fn try_from(value: RegionArgs) -> Result<Self, Self::Error> {
        rect(value.x, value.y, value.width, value.height)
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct InputArgs {
    window_id: u64,
    actions: Vec<InputActionArgs>,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum InputActionArgs {
    PointerMove { x: i32, y: i32 },
    Click { x: i32, y: i32, button: String },
    Scroll { x: i32, y: i32, dx: f32, dy: f32 },
    KeyPress { code: u32 },
}

impl TryFrom<InputActionArgs> for SyntheticInputAction {
    type Error = PlatformError;

    fn try_from(value: InputActionArgs) -> Result<Self, Self::Error> {
        let position = |x, y| {
            if x < 0 || y < 0 {
                Err(invalid("input coordinates must be non-negative"))
            } else {
                Ok(Point { x, y })
            }
        };
        match value {
            InputActionArgs::PointerMove { x, y } => Ok(Self::PointerMove {
                position: position(x, y)?,
            }),
            InputActionArgs::Click { x, y, button } => {
                let button = match button.as_str() {
                    "left" => 0x110,
                    "right" => 0x111,
                    "middle" => 0x112,
                    "side" => 0x113,
                    "extra" => 0x114,
                    _ => {
                        return Err(invalid(
                            "button must be left, right, middle, side, or extra",
                        ));
                    }
                };
                Ok(Self::Click {
                    position: position(x, y)?,
                    button,
                })
            }
            InputActionArgs::Scroll { x, y, dx, dy } => {
                if !dx.is_finite() || !dy.is_finite() || dx.abs() > 1_000.0 || dy.abs() > 1_000.0 {
                    return Err(invalid("scroll deltas must be finite and within ±1000"));
                }
                Ok(Self::Scroll {
                    position: position(x, y)?,
                    dx,
                    dy,
                })
            }
            InputActionArgs::KeyPress { code } => {
                if code > 0x2ff {
                    return Err(invalid("evdev key code must be at most 767"));
                }
                Ok(Self::KeyPress { code })
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("cannot connect to ASS IPC socket {socket:?} with named scope {scope:?}: {source}")]
    Connect {
        socket: PathBuf,
        scope: String,
        #[source]
        source: std::io::Error,
    },
    #[error("ASS IPC operation failed: {0}")]
    Ipc(#[from] std::io::Error),
    #[error("invalid tool arguments: {0}")]
    InvalidArguments(String),
    #[error("unknown tool {0:?}")]
    UnknownTool(String),
    #[error("tool {0:?} is not present in the compositor's granted named scope")]
    NotGranted(String),
    #[error("the managed fuji Realm does not exist")]
    NoManagedRealm,
    #[error("creating the managed fuji Realm requires CreateRealm in the named scope")]
    RealmCreationNotGranted,
    #[error("graceful Realm cleanup requires RevokeRealm in the named scope")]
    RealmCleanupNotGranted,
    #[error("ASS returned an unexpected Realm action response")]
    UnexpectedResponse,
    #[error("live smoke verification failed: {0}")]
    SmokeVerification(String),
    #[error(transparent)]
    Config(#[from] crate::bridge::ConfigError),
    #[error("managed Realm lifecycle failed: {0}")]
    Realm(String),
}

impl From<RealmSessionError> for PlatformError {
    fn from(error: RealmSessionError) -> Self {
        Self::Realm(error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn grant(ops: Option<Vec<OpClass>>) -> ToolGrant {
        ToolGrant {
            capabilities: Capabilities {
                query: true,
                control: true,
                input: true,
                session: false,
                realm: true,
            },
            scope: Scope {
                ops,
                ..Scope::default()
            },
        }
    }

    #[test]
    fn realm_tools_require_explicit_high_risk_operations() {
        let unscoped = grant(None);
        assert!(ToolKind::FocusWindow.allowed(&unscoped));
        assert!(!ToolKind::RealmCapture.allowed(&unscoped));
        assert!(!ToolKind::RealmInput.allowed(&unscoped));

        let scoped = grant(Some(vec![OpClass::CaptureRealm, OpClass::InjectRealmInput]));
        assert!(ToolKind::RealmCapture.allowed(&scoped));
        assert!(ToolKind::RealmInput.allowed(&scoped));
        assert!(!ToolKind::RealmReset.allowed(&scoped));
    }

    #[test]
    fn input_translation_is_bounded_and_semantic() {
        let action = InputActionArgs::Click {
            x: 10,
            y: 20,
            button: "left".into(),
        };
        assert_eq!(
            SyntheticInputAction::try_from(action).expect("action"),
            SyntheticInputAction::Click {
                position: Point { x: 10, y: 20 },
                button: 0x110
            }
        );
        assert!(
            SyntheticInputAction::try_from(InputActionArgs::PointerMove { x: -1, y: 0 }).is_err()
        );
    }

    #[test]
    fn tool_names_are_connector_local_and_stable() {
        let names = ToolKind::ALL
            .iter()
            .map(|kind| kind.definition().name)
            .collect::<Vec<_>>();
        assert!(names.contains(&"realm_capture"));
        assert!(names.contains(&"apps_list"));
        assert_eq!(names.len(), 21);
    }

    #[test]
    fn id_schema_uses_the_requested_property_name() {
        let schema = id_schema("window_id");
        assert!(schema["properties"].get("window_id").is_some());
        assert!(schema["properties"].get("name").is_none());
    }
}
