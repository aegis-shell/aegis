//! End-to-end exercise of the IPC: loopback `Server` with test `Handler`s
//! and a `Client` over a real unix socket on a process-unique temp path.
//! No Vulkan or Wayland dependency.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use aegis_ipc::{
    ActorCapability, Client, Command, ConnectionCapabilities, Event, Handler,
    InteractionDomainAction, InteractionDomainActionResult, PROTOCOL_VERSION, Scope, Server,
    SettingsAction, SettingsReceipt, SettingsSnapshot, SystemAction, SystemStatus,
};
use aegis_model::window::{Window, WindowId};

/// A unique throwaway socket path under the temp dir, namespaced by pid +
/// counter so parallel test processes do not collide.
fn scratch() -> PathBuf {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let pid = std::process::id();
    let mut p = std::env::temp_dir();
    p.push(format!("aegis-ipc-{pid}-{n}.sock"));
    p
}

/// One recorded pairing invocation: connection, display label, requested set.
type PairCall = (u64, Option<String>, Vec<ActorCapability>);

/// A handler returning a fixed window snapshot and recording the commands it
/// receives. `policy` selects what it grants.
struct TestHandler {
    windows: Vec<Window>,
    policy: ConnectionCapabilities,
    commands: Mutex<Vec<Command>>,
    command_connections: Mutex<Vec<u64>>,
    interaction_domain_actions: Mutex<Vec<InteractionDomainAction>>,
    interaction_domain_connections: Mutex<Vec<u64>>,
    settings: Mutex<SettingsSnapshot>,
    system_status: Mutex<SystemStatus>,
    system_actions: Mutex<Vec<SystemAction>>,
    system_connections: Mutex<Vec<u64>>,
    system_result: Mutex<Result<(), String>>,
    settings_actions: Mutex<Vec<SettingsAction>>,
    settings_connections: Mutex<Vec<u64>>,
    refusals: Mutex<Vec<(u64, aegis_ipc::JournalMutation, String)>>,
    scopes: Mutex<HashMap<String, Scope>>,
    capture_delay_ms: AtomicU64,
    capture_security_active: std::sync::atomic::AtomicBool,
    stream_starts: AtomicU64,
    stream_stops: Mutex<Vec<u64>>,
    stream_disconnects: Mutex<Vec<u64>>,
    stream_targets: Mutex<Vec<aegis_ipc::StreamTarget>>,
    picks: Mutex<Vec<(u64, aegis_ipc::PickKind)>>,
    app_picks: Mutex<Vec<(u64, Vec<String>)>>,
    secret_prompts: Mutex<Vec<(u64, String)>>,
    confirms: Mutex<Vec<(u64, String)>>,
    wallpapers: Mutex<Vec<(u64, std::path::PathBuf)>>,
    idle_inhibits: Mutex<Vec<(u64, bool)>>,
    idle_disconnects: Mutex<Vec<u64>>,
    pair_result: Mutex<Result<aegis_ipc::PairedAgent, String>>,
    pair_calls: Mutex<Vec<PairCall>>,
    lookup_result: Mutex<Option<aegis_ipc::AgentIdentity>>,
    refresh_result: Mutex<Result<Option<aegis_ipc::AgentIdentity>, String>>,
    lockdown_flag: std::sync::atomic::AtomicBool,
    grants: Mutex<Vec<(String, ActorCapability, bool)>>,
    grant_calls: Mutex<Vec<(u64, String, ActorCapability)>>,
    request_grant_result: Mutex<Result<bool, String>>,
    principal_infos: Mutex<Vec<aegis_ipc::AgentPrincipalInfo>>,
    grant_infos: Mutex<Vec<aegis_ipc::AgentGrantInfo>>,
    management_log: Mutex<Vec<String>>,
    register_result: Mutex<Result<(String, String), String>>,
    resource_grants: Mutex<aegis_security::authority::ResourceGrantRegistry>,
    transact_seq: AtomicU64,
}

impl TestHandler {
    /// Query-only policy (the default): no control, no session.
    fn query(windows: Vec<Window>) -> Self {
        TestHandler {
            windows,
            policy: ConnectionCapabilities::QUERY,
            commands: Mutex::new(Vec::new()),
            command_connections: Mutex::new(Vec::new()),
            interaction_domain_actions: Mutex::new(Vec::new()),
            interaction_domain_connections: Mutex::new(Vec::new()),
            settings: Mutex::new(SettingsSnapshot {
                revision: 7,
                ..SettingsSnapshot::default()
            }),
            system_status: Mutex::new(SystemStatus {
                volume: Some(42),
                brightness: Some(73),
                ..SystemStatus::default()
            }),
            system_actions: Mutex::new(Vec::new()),
            system_connections: Mutex::new(Vec::new()),
            system_result: Mutex::new(Ok(())),
            settings_actions: Mutex::new(Vec::new()),
            settings_connections: Mutex::new(Vec::new()),
            refusals: Mutex::new(Vec::new()),
            scopes: Mutex::new(test_scopes()),
            capture_delay_ms: AtomicU64::new(0),
            capture_security_active: std::sync::atomic::AtomicBool::new(true),
            stream_starts: AtomicU64::new(0),
            stream_stops: Mutex::new(Vec::new()),
            stream_disconnects: Mutex::new(Vec::new()),
            stream_targets: Mutex::new(Vec::new()),
            picks: Mutex::new(Vec::new()),
            app_picks: Mutex::new(Vec::new()),
            secret_prompts: Mutex::new(Vec::new()),
            confirms: Mutex::new(Vec::new()),
            wallpapers: Mutex::new(Vec::new()),
            idle_inhibits: Mutex::new(Vec::new()),
            idle_disconnects: Mutex::new(Vec::new()),
            pair_result: Mutex::new(Err("pairing not offered".into())),
            pair_calls: Mutex::new(Vec::new()),
            lookup_result: Mutex::new(None),
            refresh_result: Mutex::new(Ok(None)),
            lockdown_flag: std::sync::atomic::AtomicBool::new(false),
            grants: Mutex::new(Vec::new()),
            grant_calls: Mutex::new(Vec::new()),
            request_grant_result: Mutex::new(Err("no interactive grant".into())),
            principal_infos: Mutex::new(Vec::new()),
            grant_infos: Mutex::new(Vec::new()),
            management_log: Mutex::new(Vec::new()),
            register_result: Mutex::new(Err("no register".into())),
            resource_grants: Mutex::new(aegis_security::authority::ResourceGrantRegistry::default()),
            transact_seq: AtomicU64::new(0),
        }
    }
    /// Grants control and session, so command tests can exercise them.
    fn permissive(windows: Vec<Window>) -> Self {
        TestHandler {
            windows,
            policy: ConnectionCapabilities {
                query: true,
                control: true,
                input: true,
                session: true,
                interaction_domain: true,
            },
            commands: Mutex::new(Vec::new()),
            command_connections: Mutex::new(Vec::new()),
            interaction_domain_actions: Mutex::new(Vec::new()),
            interaction_domain_connections: Mutex::new(Vec::new()),
            settings: Mutex::new(SettingsSnapshot {
                revision: 7,
                ..SettingsSnapshot::default()
            }),
            system_status: Mutex::new(SystemStatus {
                volume: Some(42),
                brightness: Some(73),
                ..SystemStatus::default()
            }),
            system_actions: Mutex::new(Vec::new()),
            system_connections: Mutex::new(Vec::new()),
            system_result: Mutex::new(Ok(())),
            settings_actions: Mutex::new(Vec::new()),
            settings_connections: Mutex::new(Vec::new()),
            refusals: Mutex::new(Vec::new()),
            scopes: Mutex::new(test_scopes()),
            capture_delay_ms: AtomicU64::new(0),
            capture_security_active: std::sync::atomic::AtomicBool::new(true),
            stream_starts: AtomicU64::new(0),
            stream_stops: Mutex::new(Vec::new()),
            stream_disconnects: Mutex::new(Vec::new()),
            stream_targets: Mutex::new(Vec::new()),
            picks: Mutex::new(Vec::new()),
            app_picks: Mutex::new(Vec::new()),
            secret_prompts: Mutex::new(Vec::new()),
            confirms: Mutex::new(Vec::new()),
            wallpapers: Mutex::new(Vec::new()),
            idle_inhibits: Mutex::new(Vec::new()),
            idle_disconnects: Mutex::new(Vec::new()),
            pair_result: Mutex::new(Err("pairing not offered".into())),
            pair_calls: Mutex::new(Vec::new()),
            lookup_result: Mutex::new(None),
            refresh_result: Mutex::new(Ok(None)),
            lockdown_flag: std::sync::atomic::AtomicBool::new(false),
            grants: Mutex::new(Vec::new()),
            grant_calls: Mutex::new(Vec::new()),
            request_grant_result: Mutex::new(Err("no interactive grant".into())),
            principal_infos: Mutex::new(Vec::new()),
            grant_infos: Mutex::new(Vec::new()),
            management_log: Mutex::new(Vec::new()),
            register_result: Mutex::new(Err("no register".into())),
            resource_grants: Mutex::new(aegis_security::authority::ResourceGrantRegistry::default()),
            transact_seq: AtomicU64::new(0),
        }
    }
}

impl Handler for TestHandler {
    fn policy_caps(&self) -> ConnectionCapabilities {
        self.policy
    }
    fn issue_resource_grant(
        &self,
        session: aegis_security::authority::ActorSessionId,
        principal: Option<&str>,
        resource: aegis_security::authority::ActorResource,
        ttl: Duration,
        uses: u32,
        _confirm_exact_resource: bool,
    ) -> Result<aegis_security::authority::ResourceGrant, String> {
        let principal = principal
            .map(aegis_security::authority::ActorPrincipal::new)
            .transpose()
            .map_err(str::to_owned)?;
        self.resource_grants.lock().unwrap().issue(
            session,
            principal,
            resource.required_capability(),
            resource,
            ttl,
            uses,
        )
    }
    fn consume_resource_grant(
        &self,
        session: aegis_security::authority::ActorSessionId,
        principal: Option<&str>,
        id: &aegis_security::authority::ResourceGrantId,
        resource: &aegis_security::authority::ActorResource,
    ) -> Result<aegis_security::authority::ResourceGrant, String> {
        let principal = principal
            .map(aegis_security::authority::ActorPrincipal::new)
            .transpose()
            .map_err(str::to_owned)?;
        self.resource_grants
            .lock()
            .unwrap()
            .consume(session, principal.as_ref(), id, resource)
    }
    fn windows(&self) -> Vec<Window> {
        self.windows.clone()
    }
    fn workspaces(&self) -> aegis_model::workspace::WorkspaceSnapshot {
        // A minimal snapshot: one output, one empty workspace. Sufficient for
        // the IPC plumbing tests; the model itself is exercised in aegis-model.
        use aegis_model::workspace::{OutputSnapshot, WorkspaceEntry, WorkspaceId};
        aegis_model::workspace::WorkspaceSnapshot {
            outputs: vec![OutputSnapshot {
                id: aegis_model::workspace::OutputId(0),
                connector: "test".into(),
                current: Some(WorkspaceId(0)),
                workspaces: vec![WorkspaceEntry {
                    id: WorkspaceId(0),
                    label: None,
                    tiled: false,
                    toplevels: self.windows.iter().map(|w| w.id).collect(),
                }],
            }],
        }
    }
    fn notifications(&self) -> Vec<aegis_model::notify::Notification> {
        Vec::new()
    }
    fn outputs(&self) -> Vec<aegis_model::output::OutputInfo> {
        Vec::new()
    }
    fn journal_since(&self, _since: u64) -> aegis_ipc::JournalSnapshot {
        aegis_ipc::JournalSnapshot {
            entries: vec![],
            oldest_seq: 1,
            latest_seq: 0,
        }
    }
    fn command(&self, conn_id: u64, _subject: Option<&str>, cmd: Command) {
        self.command_connections.lock().unwrap().push(conn_id);
        self.commands.lock().unwrap().push(cmd);
    }
    fn transact(
        &self,
        conn_id: u64,
        subject: Option<&str>,
        expected_journal_seq: Option<u64>,
        ops: Vec<Command>,
    ) -> Result<aegis_ipc::TransactResult, String> {
        let before_seq = self.transact_seq.load(Ordering::SeqCst);
        if let Some(expected) = expected_journal_seq
            && expected != before_seq
        {
            return Ok(aegis_ipc::TransactResult::PreconditionConflict {
                expected,
                actual: before_seq,
            });
        }
        let mut after_seq = before_seq;
        let mut results = Vec::with_capacity(ops.len());
        for cmd in ops {
            self.command(conn_id, subject, cmd);
            after_seq += 1;
            results.push(aegis_ipc::TransactOpResult {
                seq: after_seq,
                effect: aegis_ipc::Effect::Applied,
            });
        }
        self.transact_seq.store(after_seq, Ordering::SeqCst);
        Ok(aegis_ipc::TransactResult::Committed {
            receipt: aegis_ipc::TransactReceipt {
                before_seq,
                after_seq,
                results,
            },
        })
    }
    fn interaction_domains(&self) -> aegis_model::interaction_domain::InteractionDomainSnapshot {
        let mut model = aegis_model::interaction_domain::InteractionDomainModel::new();
        model.create_agent_interaction_domain(
            "test",
            aegis_model::interaction_domain::SeatCapabilities::POINTER_KEYBOARD,
        );
        let mut snapshot = model.snapshot();
        snapshot.revision = 4;
        snapshot
    }
    fn settings(&self) -> SettingsSnapshot {
        self.settings.lock().unwrap().clone()
    }
    fn system_status(&self) -> SystemStatus {
        self.system_status.lock().unwrap().clone()
    }
    fn system_action(
        &self,
        conn_id: u64,
        _subject: Option<&str>,
        action: SystemAction,
    ) -> Result<(), String> {
        self.system_connections.lock().unwrap().push(conn_id);
        self.system_actions.lock().unwrap().push(action);
        self.system_result.lock().unwrap().clone()
    }
    fn settings_action(
        &self,
        conn_id: u64,
        _subject: Option<&str>,
        expected_revision: Option<u64>,
        action: SettingsAction,
    ) -> Result<SettingsReceipt, String> {
        let mut snapshot = self.settings.lock().unwrap();
        if expected_revision.is_some_and(|expected| expected != snapshot.revision) {
            return Err(format!(
                "settings revision conflict: expected {}, actual {}",
                expected_revision.unwrap(),
                snapshot.revision
            ));
        }
        self.settings_connections.lock().unwrap().push(conn_id);
        self.settings_actions.lock().unwrap().push(action.clone());
        match action {
            SettingsAction::SetTouchpad { config } => snapshot.touchpad.config = config,
            SettingsAction::SetDisplay { .. } => {}
            SettingsAction::SetDesktopPreferences { preferences } => {
                snapshot.preferences = preferences;
            }
            SettingsAction::SetIdle { settings } => snapshot.idle = settings,
            SettingsAction::SetDock { settings } => snapshot.dock = settings,
        }
        snapshot.revision += 1;
        Ok(SettingsReceipt {
            revision: snapshot.revision,
        })
    }
    fn interaction_domain_action(
        &self,
        conn_id: u64,
        _subject: Option<&str>,
        action: InteractionDomainAction,
    ) -> Result<InteractionDomainActionResult, String> {
        self.interaction_domain_connections
            .lock()
            .unwrap()
            .push(conn_id);
        self.interaction_domain_actions
            .lock()
            .unwrap()
            .push(action.clone());
        match action {
            InteractionDomainAction::Create { .. } => Ok(InteractionDomainActionResult::Created {
                bundle: aegis_model::interaction_domain::InteractionDomainBundle {
                    principal: aegis_model::interaction_domain::InteractionPrincipalId(2),
                    interaction_domain: aegis_model::interaction_domain::InteractionDomainId(2),
                    seat: aegis_model::interaction_domain::SeatId(2),
                    revision: 2,
                },
            }),
            InteractionDomainAction::Transact { .. } => {
                Ok(InteractionDomainActionResult::TransactionCommitted {
                    receipt: aegis_model::interaction_domain::InteractionDomainTransactionReceipt {
                        before_revision: 1,
                        after_revision: 2,
                        results: Vec::new(),
                    },
                })
            }
            InteractionDomainAction::Revoke {
                interaction_domain,
                fallback,
                ..
            } => Ok(InteractionDomainActionResult::Revoked {
                receipt: aegis_model::interaction_domain::InteractionDomainRevocation {
                    interaction_domain,
                    fallback,
                    transferred_groups: Vec::new(),
                    revision: 3,
                },
            }),
        }
    }
    fn capture_output(
        &self,
        _region: Option<aegis_model::Rect>,
    ) -> Result<aegis_ipc::CaptureOutputPayload, String> {
        std::thread::sleep(std::time::Duration::from_millis(
            self.capture_delay_ms.load(Ordering::Relaxed),
        ));
        Ok(aegis_ipc::CaptureOutputPayload {
            width: 2,
            height: 1,
            png: vec![1u8, 2, 3, 4, 5],
        })
    }
    fn resolve_scope(&self, name: &str) -> Option<Scope> {
        self.scopes.lock().unwrap().get(name).cloned()
    }
    fn agent_lookup(&self, _credential: &str) -> Option<aegis_ipc::AgentIdentity> {
        self.lookup_result.lock().unwrap().clone()
    }
    fn refresh_agent_identity(
        &self,
        _principal: &str,
    ) -> Result<Option<aegis_ipc::AgentIdentity>, String> {
        self.refresh_result.lock().unwrap().clone()
    }
    fn pair_agent(
        &self,
        conn_id: u64,
        label: Option<&str>,
        requested: &[ActorCapability],
    ) -> Result<aegis_ipc::PairedAgent, String> {
        self.pair_calls.lock().unwrap().push((
            conn_id,
            label.map(str::to_owned),
            requested.to_vec(),
        ));
        self.pair_result.lock().unwrap().clone()
    }
    fn lockdown(&self) -> bool {
        self.lockdown_flag.load(Ordering::Relaxed)
    }
    fn grant_for(&self, principal: &str, op: ActorCapability) -> Option<bool> {
        self.grants
            .lock()
            .unwrap()
            .iter()
            .find(|(p, o, _)| p == principal && *o == op)
            .map(|(_, _, decision)| *decision)
    }
    fn request_grant(
        &self,
        conn_id: u64,
        principal: &str,
        op: ActorCapability,
    ) -> Result<bool, String> {
        self.grant_calls
            .lock()
            .unwrap()
            .push((conn_id, principal.to_owned(), op));
        self.request_grant_result.lock().unwrap().clone()
    }
    fn agent_principals(&self) -> Vec<aegis_ipc::AgentPrincipalInfo> {
        self.principal_infos.lock().unwrap().clone()
    }
    fn agent_grants(&self, principal: Option<&str>) -> Vec<aegis_ipc::AgentGrantInfo> {
        self.grant_infos
            .lock()
            .unwrap()
            .iter()
            .filter(|grant| principal.is_none_or(|p| grant.principal == p))
            .cloned()
            .collect()
    }
    fn rename_agent_principal(&self, principal: &str, label: Option<&str>) -> Result<(), String> {
        self.management_log
            .lock()
            .unwrap()
            .push(format!("rename:{principal}:{label:?}"));
        Ok(())
    }
    fn forget_agent_principal(&self, principal: &str) -> Result<(), String> {
        self.management_log
            .lock()
            .unwrap()
            .push(format!("forget:{principal}"));
        Ok(())
    }
    fn set_agent_ceiling(
        &self,
        principal: &str,
        pregranted: &[ActorCapability],
        gated: &[ActorCapability],
    ) -> Result<(), String> {
        self.management_log.lock().unwrap().push(format!(
            "ceiling:{principal}:{}+{}",
            pregranted.len(),
            gated.len()
        ));
        Ok(())
    }
    fn register_agent(
        &self,
        label: Option<&str>,
        pregranted: &[ActorCapability],
        gated: &[ActorCapability],
    ) -> Result<(String, String), String> {
        self.management_log.lock().unwrap().push(format!(
            "register:{label:?}:{}+{}",
            pregranted.len(),
            gated.len()
        ));
        self.register_result.lock().unwrap().clone()
    }
    fn revoke_agent_grant(&self, principal: &str, op: ActorCapability) -> Result<(), String> {
        self.management_log
            .lock()
            .unwrap()
            .push(format!("revoke:{principal}:{op:?}"));
        Ok(())
    }
    fn audit_refusal(
        &self,
        conn_id: u64,
        _subject: Option<&str>,
        mutation: aegis_ipc::JournalMutation,
        reason: String,
    ) {
        self.refusals
            .lock()
            .unwrap()
            .push((conn_id, mutation, reason));
    }
    fn audit_capability_use(
        &self,
        conn_id: u64,
        _subject: Option<&str>,
        session: aegis_security::authority::ActorSessionId,
        capability: ActorCapability,
        action: aegis_ipc::CapabilityUseAction,
        effect: aegis_ipc::Effect,
    ) {
        if let aegis_ipc::Effect::Refused { reason } = effect {
            self.refusals.lock().unwrap().push((
                conn_id,
                aegis_ipc::JournalMutation::CapabilityUse {
                    session,
                    principal: None,
                    capability,
                    action,
                },
                reason,
            ));
        }
    }
    fn capture_security_active(&self) -> bool {
        self.capture_security_active.load(Ordering::Acquire)
    }
    fn stream_output_start(
        &self,
        _conn_id: u64,
        _max_fps: Option<u32>,
        target: aegis_ipc::StreamTarget,
        _allow_dmabuf: bool,
    ) -> Result<aegis_ipc::StreamInfo, String> {
        self.stream_targets.lock().unwrap().push(target);
        let stream_id = self.stream_starts.fetch_add(1, Ordering::AcqRel) + 1;
        Ok(aegis_ipc::StreamInfo {
            stream_id,
            width: 2,
            height: 2,
            format: aegis_ipc::StreamPixelFormat::Bgra8,
            slots: None,
        })
    }
    fn stream_output_stop(&self, stream_id: u64) {
        self.stream_stops.lock().unwrap().push(stream_id);
    }
    fn streams_disconnected(&self, conn_id: u64) {
        self.stream_disconnects.lock().unwrap().push(conn_id);
    }
    fn set_idle_inhibit(&self, conn_id: u64, inhibit: bool) -> Result<bool, String> {
        self.idle_inhibits.lock().unwrap().push((conn_id, inhibit));
        Ok(inhibit)
    }
    fn idle_inhibit_disconnected(&self, conn_id: u64) {
        self.idle_disconnects.lock().unwrap().push(conn_id);
    }
    fn pick_target(
        &self,
        conn_id: u64,
        kind: aegis_ipc::PickKind,
    ) -> Result<aegis_ipc::PickResult, String> {
        self.picks.lock().unwrap().push((conn_id, kind));
        Ok(aegis_ipc::PickResult::Region {
            rect: aegis_model::Rect::new(1, 2, 30, 40),
        })
    }
    fn pick_app(
        &self,
        conn_id: u64,
        choices: Vec<String>,
        _subject: Option<String>,
        _last_choice: Option<String>,
    ) -> Result<aegis_ipc::AppPickResult, String> {
        self.app_picks.lock().unwrap().push((conn_id, choices));
        Ok(aegis_ipc::AppPickResult::App {
            id: "org.example.Chosen.desktop".to_string(),
        })
    }
    fn prompt_secret(
        &self,
        conn_id: u64,
        title: String,
        _reason: Option<String>,
    ) -> Result<aegis_ipc::SecretPromptResult, String> {
        self.secret_prompts.lock().unwrap().push((conn_id, title));
        Ok(aegis_ipc::SecretPromptResult::Secret {
            value: "hunter2".to_string(),
        })
    }
    fn pick_confirm(
        &self,
        conn_id: u64,
        title: String,
        _body: String,
        _accept_label: Option<String>,
    ) -> Result<aegis_ipc::ConfirmPickResult, String> {
        self.confirms.lock().unwrap().push((conn_id, title));
        Ok(aegis_ipc::ConfirmPickResult::Confirmed)
    }
    fn set_wallpaper(&self, conn_id: u64, path: std::path::PathBuf) -> Result<(), String> {
        self.wallpapers.lock().unwrap().push((conn_id, path));
        Ok(())
    }
    fn capture_interaction_domain(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
        region: Option<aegis_model::Rect>,
    ) -> Result<aegis_ipc::CaptureInteractionDomainPayload, String> {
        std::thread::sleep(std::time::Duration::from_millis(
            self.capture_delay_ms.load(Ordering::Relaxed),
        ));
        Ok(aegis_ipc::CaptureInteractionDomainPayload {
            capture: aegis_ipc::InteractionDomainCapture {
                interaction_domain,
                width: 2,
                height: 1,
                scale_milli: 1250,
                region: region.unwrap_or_else(|| aegis_model::Rect::new(0, 0, 2, 1)),
                placements: vec![
                    aegis_model::interaction_domain::InteractionDomainWindowPlacement {
                        window: WindowId(1),
                        output_rect: aegis_model::Rect::new(0, 0, 2, 1),
                        surface_size: aegis_model::Size { w: 20, h: 10 },
                    },
                ],
                observation: aegis_ipc::SemanticObservation {
                    token: aegis_ipc::ObservationToken("a".repeat(64)),
                    ttl_ms: 15_000,
                    snapshot: aegis_model::semantic::SemanticSnapshot {
                        interaction_domain,
                        authority_revision: 4,
                        objects: Vec::new(),
                    },
                },
                png_bytes: 3,
                revision: 4,
            },
            png: vec![9u8, 8, 7],
        })
    }
    fn capture_window(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        window: WindowId,
    ) -> Result<aegis_ipc::CaptureWindowPayload, String> {
        std::thread::sleep(std::time::Duration::from_millis(
            self.capture_delay_ms.load(Ordering::Relaxed),
        ));
        Ok(aegis_ipc::CaptureWindowPayload {
            capture: aegis_ipc::WindowCapture {
                window,
                width: 2,
                height: 1,
                scale_milli: 1000,
                rect: aegis_model::Rect::new(0, 0, 2, 1),
                png_bytes: 3,
            },
            png: vec![6u8, 5, 4],
        })
    }
    fn observe_interaction_domain(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
    ) -> Result<aegis_ipc::SemanticObservation, String> {
        Ok(aegis_ipc::SemanticObservation {
            token: aegis_ipc::ObservationToken("b".repeat(64)),
            ttl_ms: 15_000,
            snapshot: aegis_model::semantic::SemanticSnapshot {
                interaction_domain,
                authority_revision: 4,
                objects: vec![aegis_model::semantic::SemanticObject {
                    id: aegis_model::semantic::SemanticObjectId::for_window(WindowId(1)),
                    parent: None,
                    window: WindowId(1),
                    source: aegis_model::semantic::SemanticSource::Compositor,
                    role: aegis_model::semantic::SemanticRole::Window,
                    name: Some("first".into()),
                    description: None,
                    value: None,
                    app_id: Some("org.example.first".into()),
                    bounds: aegis_model::Rect::new(0, 0, 20, 10),
                    local_size: aegis_model::Size { w: 20, h: 10 },
                    state: aegis_model::semantic::SemanticState {
                        visible: true,
                        enabled: true,
                        ..Default::default()
                    },
                    actions: vec![aegis_model::semantic::SemanticAction::Pointer],
                    revision: 9,
                }],
            },
        })
    }
    fn act_in_interaction_domain(
        &self,
        _conn_id: u64,
        _subject: Option<&str>,
        _scope_name: Option<&str>,
        _scope: Scope,
        intent: aegis_ipc::ActorActionIntent,
    ) -> Result<aegis_ipc::ActorActionReceipt, String> {
        if intent.observation != aegis_ipc::ObservationToken("b".repeat(64)) {
            return Err("unknown observation".into());
        }
        Ok(aegis_ipc::ActorActionReceipt {
            action_id: 12,
            interaction_domain: intent.interaction_domain,
            target: intent.target,
            window: WindowId(1),
            authority_revision: 4,
            actions_applied: intent.actions.len() as u32,
            committed_mono_ms: 99,
        })
    }
}

fn test_scopes() -> HashMap<String, Scope> {
    HashMap::from([
        (
            aegis_ipc::LOCAL_AGENT_ADMIN_SCOPE.into(),
            Scope {
                ops: Some(Vec::new()),
                ..Scope::default()
            },
        ),
        (
            "focus-first".into(),
            Scope {
                windows: Some(vec![WindowId(1)]),
                ops: Some(vec![ActorCapability::Focus]),
                ..Scope::default()
            },
        ),
        (
            "input-first".into(),
            Scope {
                windows: Some(vec![WindowId(1)]),
                ops: Some(vec![ActorCapability::InjectInput]),
                ..Scope::default()
            },
        ),
        (
            "capture".into(),
            Scope {
                ops: Some(vec![ActorCapability::CaptureOutput]),
                ..Scope::default()
            },
        ),
        (
            "capture-window".into(),
            Scope {
                windows: Some(vec![WindowId(1)]),
                ops: Some(vec![ActorCapability::CaptureWindow]),
                ..Scope::default()
            },
        ),
        (
            "stream".into(),
            Scope {
                ops: Some(vec![ActorCapability::StreamOutput]),
                ..Scope::default()
            },
        ),
        (
            "stream-first".into(),
            Scope {
                windows: Some(vec![WindowId(1)]),
                ops: Some(vec![ActorCapability::StreamOutput]),
                ..Scope::default()
            },
        ),
        (
            "file-resource".into(),
            Scope {
                ops: Some(vec![ActorCapability::ReadFile]),
                ..Scope::default()
            },
        ),
        (
            "idle".into(),
            Scope {
                ops: Some(vec![ActorCapability::IdleInhibit]),
                ..Scope::default()
            },
        ),
        (
            "system".into(),
            Scope {
                ops: Some(vec![ActorCapability::SystemControl]),
                ..Scope::default()
            },
        ),
        (
            "pick".into(),
            Scope {
                ops: Some(vec![ActorCapability::PickTarget]),
                ..Scope::default()
            },
        ),
        (
            "app-pick".into(),
            Scope {
                ops: Some(vec![ActorCapability::PickApp]),
                ..Scope::default()
            },
        ),
        (
            "secret-prompt".into(),
            Scope {
                ops: Some(vec![ActorCapability::PromptSecret]),
                ..Scope::default()
            },
        ),
        (
            "confirm".into(),
            Scope {
                ops: Some(vec![ActorCapability::PickConfirm]),
                ..Scope::default()
            },
        ),
        (
            "wallpaper".into(),
            Scope {
                ops: Some(vec![ActorCapability::SetWallpaper]),
                ..Scope::default()
            },
        ),
        (
            "interaction_domain".into(),
            Scope {
                interaction_domains: Some(vec![
                    aegis_model::interaction_domain::HUMAN_INTERACTION_DOMAIN,
                    aegis_model::interaction_domain::InteractionDomainId(2),
                ]),
                ops: Some(vec![
                    ActorCapability::CreateInteractionDomain,
                    ActorCapability::TransactInteractionDomain,
                    ActorCapability::RevokeInteractionDomain,
                    ActorCapability::CaptureInteractionDomain,
                    ActorCapability::ObserveInteractionDomain,
                    ActorCapability::InjectInteractionDomainInput,
                ]),
                ..Scope::default()
            },
        ),
    ])
}

fn sample_windows() -> Vec<Window> {
    let mut a = Window::new(WindowId(1));
    a.title = Some("first".into());
    a.app_id = Some("org.example.first".into());
    a.state.activated = true;
    let mut b = Window::new(WindowId(2));
    b.title = Some("second".into());
    b.state.maximized = true;
    vec![a, b]
}

#[path = "ipc/agent.rs"]
mod agent;
#[path = "ipc/authority.rs"]
mod authority;
#[path = "ipc/basics.rs"]
mod basics;
#[path = "ipc/portal.rs"]
mod portal;
#[path = "ipc/primitives.rs"]
mod primitives;
#[path = "ipc/protocol.rs"]
mod protocol;
