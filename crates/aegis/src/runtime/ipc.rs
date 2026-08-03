use super::*;

type SemanticCompletion = std::sync::mpsc::Sender<Result<(), String>>;
type SemanticEnvelopeReceiver =
    std::sync::Arc<std::sync::Mutex<std::sync::mpsc::Receiver<SemanticDispatchEnvelope>>>;
type SemanticPendingKey = (aegis_semantic::SemanticProviderId, u64);
type SemanticPendingAction = (aegis_authority::ActorSessionId, SemanticCompletion);

struct SemanticDispatchEnvelope {
    request: aegis_semantic::SemanticActionRequest,
    completion: SemanticCompletion,
}

struct SemanticProviderLane {
    session: aegis_authority::ActorSessionId,
    sender: std::sync::mpsc::SyncSender<SemanticDispatchEnvelope>,
    receiver: SemanticEnvelopeReceiver,
}

#[derive(Default)]
struct SemanticDispatchBroker {
    providers: std::collections::HashMap<aegis_semantic::SemanticProviderId, SemanticProviderLane>,
    pending: std::collections::HashMap<SemanticPendingKey, SemanticPendingAction>,
}

impl SemanticDispatchBroker {
    fn receiver(
        &mut self,
        provider: aegis_semantic::SemanticProviderId,
        session: aegis_authority::ActorSessionId,
    ) -> Result<SemanticEnvelopeReceiver, String> {
        if let Some(lane) = self.providers.get(&provider) {
            if lane.session != session {
                return Err("semantic provider already has another live session".into());
            }
            return Ok(std::sync::Arc::clone(&lane.receiver));
        }
        let (sender, receiver) = std::sync::mpsc::sync_channel(64);
        let receiver = std::sync::Arc::new(std::sync::Mutex::new(receiver));
        self.providers.insert(
            provider,
            SemanticProviderLane {
                session,
                sender,
                receiver: std::sync::Arc::clone(&receiver),
            },
        );
        Ok(receiver)
    }

    fn revoke_session(&mut self, session: aegis_authority::ActorSessionId) {
        self.providers.retain(|_, lane| lane.session != session);
        let revoked = self
            .pending
            .iter()
            .filter_map(|(key, (owner, _))| (*owner == session).then_some(key.clone()))
            .collect::<Vec<_>>();
        for key in revoked {
            if let Some((_, completion)) = self.pending.remove(&key) {
                let _ = completion.send(Err("semantic provider session was revoked".into()));
            }
        }
    }
}

/// Shared live window snapshot for the IPC (ADR-0027). The main loop writes
/// the same `Vec<Window>` it hands the shell; connection threads read it.
/// `query`-capability commands never mutate, so the lock is an `RwLock` and
/// reads from several connections do not block each other. `control`/
/// `session` commands arrive through [`aegis_ipc::Handler::command`] and are forwarded
/// to the main loop via the channel the binary owns — the Wayland server
/// state is not `Send`, so connection threads must not touch it directly.
pub(super) struct LiveChannels {
    pub(super) commands: std::sync::mpsc::Sender<IpcCommandRequest>,
    pub(super) system_controls: std::sync::mpsc::Sender<SystemControlRequest>,
    pub(super) capture: std::sync::mpsc::Sender<CaptureRequest>,
    pub(super) interaction_domain_controls:
        std::sync::mpsc::Sender<InteractionDomainControlRequest>,
    pub(super) settings_controls: std::sync::mpsc::Sender<SettingsControlRequest>,
    pub(super) wallpaper_controls: std::sync::mpsc::Sender<WallpaperControlRequest>,
    pub(super) interaction_domain_capture: std::sync::mpsc::Sender<InteractionDomainCaptureRequest>,
    pub(super) interaction_domain_observe:
        std::sync::mpsc::SyncSender<InteractionDomainObserveRequest>,
    pub(super) actor_actions: std::sync::mpsc::SyncSender<InteractionDomainActorActionRequest>,
    pub(super) semantic_tree_updates: std::sync::mpsc::SyncSender<SemanticTreeUpdateRequest>,
    pub(super) semantic_provider_revocations:
        std::sync::mpsc::SyncSender<aegis_semantic::SemanticProviderId>,
    pub(super) observation_discards: std::sync::mpsc::SyncSender<ObservationDiscardRequest>,
    pub(super) actor_disconnects: std::sync::mpsc::Sender<u64>,
    pub(super) stream_controls: std::sync::mpsc::Sender<StreamControlRequest>,
    pub(super) idle_controls: std::sync::mpsc::Sender<IdleControlRequest>,
    pub(super) pick_controls: std::sync::mpsc::Sender<PickControlRequest>,
    pub(super) app_pick_controls: std::sync::mpsc::Sender<AppPickControlRequest>,
    pub(super) secret_prompt_controls: std::sync::mpsc::Sender<SecretPromptControlRequest>,
    pub(super) confirm_pick_controls: std::sync::mpsc::Sender<ConfirmPickControlRequest>,
    pub(super) capability_pick_controls: std::sync::mpsc::Sender<CapabilityPickControlRequest>,
}

pub(super) struct LiveState {
    windows: std::sync::RwLock<Vec<aegis_core::window::Window>>,
    accessibility_windows: std::sync::RwLock<Vec<aegis_semantic::AccessibilityWindowBinding>>,
    workspaces: std::sync::RwLock<aegis_core::workspace::WorkspaceSnapshot>,
    outputs: std::sync::RwLock<Vec<aegis_core::output::OutputInfo>>,
    interaction_domains:
        std::sync::RwLock<aegis_core::interaction_domain::InteractionDomainSnapshot>,
    settings: std::sync::RwLock<aegis_ipc::SettingsSnapshot>,
    system_status: std::sync::RwLock<aegis_ipc::SystemStatus>,
    notifications: std::sync::Arc<std::sync::Mutex<aegis_core::notify::NotificationQueue>>,
    journal: std::sync::Arc<std::sync::Mutex<aegis_ipc::Journal>>,
    commands: std::sync::Mutex<std::sync::mpsc::Sender<IpcCommandRequest>>,
    system_controls: std::sync::Mutex<std::sync::mpsc::Sender<SystemControlRequest>>,
    capture: std::sync::Mutex<std::sync::mpsc::Sender<CaptureRequest>>,
    interaction_domain_controls:
        std::sync::Mutex<std::sync::mpsc::Sender<InteractionDomainControlRequest>>,
    settings_controls: std::sync::Mutex<std::sync::mpsc::Sender<SettingsControlRequest>>,
    wallpaper_controls: std::sync::Mutex<std::sync::mpsc::Sender<WallpaperControlRequest>>,
    interaction_domain_capture:
        std::sync::Mutex<std::sync::mpsc::Sender<InteractionDomainCaptureRequest>>,
    interaction_domain_observe:
        std::sync::Mutex<std::sync::mpsc::SyncSender<InteractionDomainObserveRequest>>,
    actor_actions:
        std::sync::Mutex<std::sync::mpsc::SyncSender<InteractionDomainActorActionRequest>>,
    semantic_tree_updates: std::sync::Mutex<std::sync::mpsc::SyncSender<SemanticTreeUpdateRequest>>,
    semantic_provider_revocations: std::sync::mpsc::SyncSender<aegis_semantic::SemanticProviderId>,
    observation_discards: std::sync::mpsc::SyncSender<ObservationDiscardRequest>,
    actor_disconnects: std::sync::mpsc::Sender<u64>,
    stream_controls: std::sync::Mutex<std::sync::mpsc::Sender<StreamControlRequest>>,
    idle_controls: std::sync::Mutex<std::sync::mpsc::Sender<IdleControlRequest>>,
    pick_controls: std::sync::Mutex<std::sync::mpsc::Sender<PickControlRequest>>,
    app_pick_controls: std::sync::Mutex<std::sync::mpsc::Sender<AppPickControlRequest>>,
    secret_prompt_controls: std::sync::Mutex<std::sync::mpsc::Sender<SecretPromptControlRequest>>,
    confirm_pick_controls: std::sync::Mutex<std::sync::mpsc::Sender<ConfirmPickControlRequest>>,
    capability_pick_controls:
        std::sync::Mutex<std::sync::mpsc::Sender<CapabilityPickControlRequest>>,
    journal_broadcaster: aegis_ipc::JournalBroadcaster,
    capture_delivery_gate: std::sync::Arc<std::sync::atomic::AtomicBool>,
    scopes: std::sync::RwLock<std::collections::HashMap<String, aegis_ipc::Scope>>,
    /// Pairing registry for capability-borrowing agents (ADR-0088).
    agent_auth: std::sync::RwLock<PrincipalRegistry>,
    /// Runtime-grant decisions for paired agents (ADR-0088).
    grants: std::sync::RwLock<GrantStore>,
    /// Live execution contexts. Unlike paired principals these die on EOF,
    /// idle expiry, or explicit principal revocation.
    actor_sessions: std::sync::Mutex<aegis_authority::ActorSessionRegistry>,
    /// Exact, session-bound filesystem/network/secret/payment authorities.
    resource_grants: std::sync::Mutex<aegis_authority::ResourceGrantRegistry>,
    semantic_dispatch: std::sync::Mutex<SemanticDispatchBroker>,
    next_semantic_request: std::sync::atomic::AtomicU64,
    audit_start: std::time::Instant,
    /// `[agent] lockdown`: strip privileged capabilities from connections
    /// that neither present a built-in scope nor pair as an agent.
    lockdown: bool,
}

fn actor_action_scope_still_authorized(
    before: aegis_ipc::AuthorizationDecision,
    now: aegis_ipc::AuthorizationDecision,
    recorded: Option<bool>,
) -> bool {
    use aegis_ipc::AuthorizationDecision;
    match (before, now) {
        (
            AuthorizationDecision::Permit | AuthorizationDecision::Ask(_),
            AuthorizationDecision::Permit,
        ) => true,
        (AuthorizationDecision::Ask(before), AuthorizationDecision::Ask(now)) if before == now => {
            recorded != Some(false)
        }
        (AuthorizationDecision::Permit, AuthorizationDecision::Ask(_)) => recorded == Some(true),
        _ => false,
    }
}

fn resource_confirmation(
    resource: &aegis_authority::ActorResource,
) -> (String, String, Option<String>) {
    use aegis_authority::{ActorResource, FilesystemAccess};
    match resource {
        ActorResource::FilesystemPath { path, access } => {
            let operation = match access {
                FilesystemAccess::Read => "read",
                FilesystemAccess::Write => "write",
            };
            (
                "Allow file access?".into(),
                format!("Allow this Actor to {operation} the exact path {path:?}?"),
                Some("Allow once".into()),
            )
        }
        ActorResource::NetworkOrigin { scheme, host, port } => {
            let origin = port.map_or_else(
                || format!("{scheme}://{host}"),
                |port| format!("{scheme}://{host}:{port}"),
            );
            (
                "Allow network access?".into(),
                format!("Allow this Actor to access the exact origin {origin}?"),
                Some("Allow".into()),
            )
        }
        ActorResource::SecretPrompt { purpose } => (
            "Allow secret request?".into(),
            format!("Allow this Actor to request a secret for {purpose:?}?"),
            Some("Continue".into()),
        ),
        ActorResource::PaymentRequest {
            payee,
            currency,
            maximum_minor_units,
        } => (
            "Confirm payment authority".into(),
            format!(
                "Authorize a payment to {payee:?} up to {maximum_minor_units} minor units in {currency}?"
            ),
            Some("Authorize payment".into()),
        ),
    }
}

impl LiveState {
    #[allow(clippy::too_many_arguments)]
    pub(super) fn new(
        channels: LiveChannels,
        capture_delivery_gate: std::sync::Arc<std::sync::atomic::AtomicBool>,
        notifications: std::sync::Arc<std::sync::Mutex<aegis_core::notify::NotificationQueue>>,
        journal: std::sync::Arc<std::sync::Mutex<aegis_ipc::Journal>>,
        scopes: std::collections::HashMap<String, aegis_ipc::Scope>,
        agent_auth: PrincipalRegistry,
        grants: GrantStore,
        lockdown: bool,
        audit_start: std::time::Instant,
        journal_broadcaster: aegis_ipc::JournalBroadcaster,
    ) -> LiveState {
        LiveState {
            windows: std::sync::RwLock::new(Vec::new()),
            accessibility_windows: std::sync::RwLock::new(Vec::new()),
            workspaces: std::sync::RwLock::new(
                aegis_core::workspace::WorkspaceModel::new().snapshot(),
            ),
            outputs: std::sync::RwLock::new(Vec::new()),
            interaction_domains: std::sync::RwLock::new(
                aegis_core::interaction_domain::InteractionDomainModel::new().snapshot(),
            ),
            settings: std::sync::RwLock::new(aegis_ipc::SettingsSnapshot::default()),
            system_status: std::sync::RwLock::new(aegis_ipc::SystemStatus::default()),
            notifications,
            journal,
            commands: std::sync::Mutex::new(channels.commands),
            system_controls: std::sync::Mutex::new(channels.system_controls),
            capture: std::sync::Mutex::new(channels.capture),
            interaction_domain_controls: std::sync::Mutex::new(
                channels.interaction_domain_controls,
            ),
            settings_controls: std::sync::Mutex::new(channels.settings_controls),
            wallpaper_controls: std::sync::Mutex::new(channels.wallpaper_controls),
            interaction_domain_capture: std::sync::Mutex::new(channels.interaction_domain_capture),
            interaction_domain_observe: std::sync::Mutex::new(channels.interaction_domain_observe),
            actor_actions: std::sync::Mutex::new(channels.actor_actions),
            semantic_tree_updates: std::sync::Mutex::new(channels.semantic_tree_updates),
            semantic_provider_revocations: channels.semantic_provider_revocations,
            observation_discards: channels.observation_discards,
            actor_disconnects: channels.actor_disconnects,
            stream_controls: std::sync::Mutex::new(channels.stream_controls),
            idle_controls: std::sync::Mutex::new(channels.idle_controls),
            pick_controls: std::sync::Mutex::new(channels.pick_controls),
            app_pick_controls: std::sync::Mutex::new(channels.app_pick_controls),
            secret_prompt_controls: std::sync::Mutex::new(channels.secret_prompt_controls),
            confirm_pick_controls: std::sync::Mutex::new(channels.confirm_pick_controls),
            capability_pick_controls: std::sync::Mutex::new(channels.capability_pick_controls),
            journal_broadcaster,
            capture_delivery_gate,
            scopes: std::sync::RwLock::new(scopes),
            agent_auth: std::sync::RwLock::new(agent_auth),
            grants: std::sync::RwLock::new(grants),
            actor_sessions: std::sync::Mutex::new(aegis_authority::ActorSessionRegistry::default()),
            resource_grants: std::sync::Mutex::new(
                aegis_authority::ResourceGrantRegistry::default(),
            ),
            semantic_dispatch: std::sync::Mutex::new(SemanticDispatchBroker::default()),
            next_semantic_request: std::sync::atomic::AtomicU64::new(0),
            audit_start,
            lockdown,
        }
    }

    fn actor_binding(
        &self,
        connection_id: u64,
        subject: Option<&str>,
    ) -> Result<(ActorBinding, aegis_authority::ActorSessionSnapshot), String> {
        let principal = subject
            .map(aegis_authority::ActorPrincipal::new)
            .transpose()
            .map_err(str::to_owned)?;
        let snapshot = self
            .actor_sessions
            .lock()
            .unwrap()
            .authorize_connection(connection_id)?;
        if snapshot.principal.as_ref() != principal.as_ref() {
            return Err("Actor principal does not match the live session".into());
        }
        Ok((
            ActorBinding {
                session: snapshot.id,
                connection_id,
                principal,
            },
            snapshot,
        ))
    }

    /// Re-resolve an Actor's capability ceiling at the main-loop commit
    /// boundary. The IPC thread already obtained any one-shot interactive
    /// grant; this check detects principal removal, explicit denial, named
    /// scope changes, and resource narrowing without prompting a second time.
    pub(super) fn revalidate_actor_action_scope(
        &self,
        scope_name: Option<&str>,
        actor: &ActorBinding,
        authorized_scope: &aegis_ipc::Scope,
        interaction_domain: aegis_core::interaction_domain::InteractionDomainId,
    ) -> Result<aegis_ipc::Scope, String> {
        let current = if let Some(name) = scope_name {
            self.scopes
                .read()
                .unwrap()
                .get(name)
                .cloned()
                .ok_or_else(|| "Actor scope was revoked before action commit".to_owned())?
        } else if let Some(subject) = actor.principal.as_deref() {
            let identity = self
                .agent_auth
                .read()
                .unwrap()
                .identity_for_principal(subject)
                .ok_or_else(|| "Actor principal was forgotten before action commit".to_owned())?;
            aegis_ipc::Scope {
                ops: Some(identity.pregranted),
                ask_ops: Some(identity.gated),
                ..aegis_ipc::Scope::default()
            }
        } else {
            authorized_scope.clone()
        };

        let before = authorized_scope.decide_interaction_domain_input(interaction_domain);
        let now = current.decide_interaction_domain_input(interaction_domain);
        let recorded = actor.principal.as_deref().and_then(|subject| {
            self.grants.read().unwrap().decision_for(
                subject,
                aegis_ipc::ActorCapability::InjectInteractionDomainInput,
            )
        });
        let permitted = actor_action_scope_still_authorized(before, now, recorded);
        permitted.then_some(current).ok_or_else(|| {
            "Actor capability or InteractionDomain scope changed before action commit".into()
        })
    }

    /// Durably append one positive authorization lifecycle event before the
    /// request can report success. Append and broadcast stay under one lock
    /// so concurrent producers cannot publish sequence numbers out of order.
    fn audit_event(&self, origin: aegis_ipc::Origin, mutation: aegis_ipc::JournalMutation) {
        self.persist_and_broadcast(origin, mutation, aegis_ipc::Effect::Applied);
    }

    fn persist_and_broadcast(
        &self,
        origin: aegis_ipc::Origin,
        mutation: aegis_ipc::JournalMutation,
        effect: aegis_ipc::Effect,
    ) {
        let effect = mutation.privacy_minimize_effect(effect);
        let mut journal = self.journal.lock().unwrap();
        let entry = match journal.try_append(
            self.audit_start.elapsed().as_millis() as u64,
            origin,
            mutation,
            effect,
        ) {
            Ok(entry) => entry,
            Err(error) => {
                log::error!("durable audit append failed; fail-stopping compositor: {error}");
                std::process::abort();
            }
        };
        self.journal_broadcaster.broadcast(entry);
    }

    fn auth_event(
        &self,
        conn_id: Option<u64>,
        principal: &str,
        action: aegis_ipc::AgentAuthAction,
    ) {
        let origin = conn_id
            .map(|conn_id| aegis_ipc::Origin::ipc(conn_id, Some(principal)))
            .unwrap_or(aegis_ipc::Origin::Internal);
        self.audit_event(
            origin,
            aegis_ipc::JournalMutation::AgentAuth {
                principal: principal.to_owned(),
                action,
            },
        );
    }

    fn resource_grant_event(
        &self,
        origin: aegis_ipc::Origin,
        grant: &aegis_authority::ResourceGrant,
        action: aegis_ipc::ResourceGrantAuditAction,
    ) {
        self.audit_event(
            origin,
            aegis_ipc::JournalMutation::ResourceGrant {
                session: grant.session,
                principal: grant.principal.clone(),
                capability: grant.capability,
                resource_kind: (&grant.resource).into(),
                action,
            },
        );
    }

    fn terminate_actor_session(
        &self,
        snapshot: aegis_authority::ActorSessionSnapshot,
        action: aegis_ipc::ActorSessionAuditAction,
        origin: aegis_ipc::Origin,
    ) {
        let revoked_grants = self
            .resource_grants
            .lock()
            .unwrap()
            .revoke_session(snapshot.id);
        self.semantic_dispatch
            .lock()
            .unwrap()
            .revoke_session(snapshot.id);
        if let Some(principal) = snapshot.principal.as_ref()
            && let Ok(provider) = aegis_semantic::SemanticProviderId::new(principal.as_ref())
        {
            let _ = self.semantic_provider_revocations.try_send(provider);
        }
        let _ = self.actor_disconnects.send(snapshot.connection_id);
        self.audit_event(
            origin.clone(),
            aegis_ipc::JournalMutation::ActorSession {
                session: snapshot.id,
                principal: snapshot.principal,
                action,
            },
        );
        for grant in revoked_grants {
            self.resource_grant_event(
                origin.clone(),
                &grant,
                aegis_ipc::ResourceGrantAuditAction::Revoked,
            );
        }
    }

    /// Timer-driven cleanup for Actors that make no further request after
    /// reaching their TTL or idle deadline.
    pub(super) fn expire_due_actor_sessions(&self) {
        self.expire_due_resource_grants();
        let expired = self.actor_sessions.lock().unwrap().expire_due();
        for snapshot in expired {
            self.terminate_actor_session(
                snapshot,
                aegis_ipc::ActorSessionAuditAction::Expired,
                aegis_ipc::Origin::Internal,
            );
        }
    }

    fn expire_due_resource_grants(&self) {
        let expired = self.resource_grants.lock().unwrap().expire_due();
        for grant in expired {
            self.resource_grant_event(
                aegis_ipc::Origin::Internal,
                &grant,
                aegis_ipc::ResourceGrantAuditAction::Expired,
            );
        }
    }

    pub(super) fn set_windows(
        &self,
        windows: Vec<aegis_core::window::Window>,
        accessibility_windows: Vec<aegis_semantic::AccessibilityWindowBinding>,
    ) {
        *self.windows.write().unwrap() = windows;
        *self.accessibility_windows.write().unwrap() = accessibility_windows;
    }

    pub(super) fn set_workspaces(&self, snapshot: aegis_core::workspace::WorkspaceSnapshot) {
        *self.workspaces.write().unwrap() = snapshot;
    }

    pub(super) fn set_outputs(&self, outputs: Vec<aegis_core::output::OutputInfo>) {
        *self.outputs.write().unwrap() = outputs;
    }

    pub(super) fn set_interaction_domains(
        &self,
        snapshot: aegis_core::interaction_domain::InteractionDomainSnapshot,
    ) {
        *self.interaction_domains.write().unwrap() = snapshot;
    }

    pub(super) fn set_settings(&self, snapshot: aegis_ipc::SettingsSnapshot) {
        *self.settings.write().unwrap() = snapshot;
    }

    pub(super) fn set_system_status(&self, snapshot: aegis_ipc::SystemStatus) {
        *self.system_status.write().unwrap() = snapshot;
    }

    pub(super) fn dispatch_accessibility_action(
        &self,
        target: aegis_semantic::SemanticDispatchTarget,
        action: aegis_core::semantic::SemanticActionIntent,
    ) -> Result<std::sync::mpsc::Receiver<Result<(), String>>, String> {
        let previous = self
            .next_semantic_request
            .fetch_update(
                std::sync::atomic::Ordering::AcqRel,
                std::sync::atomic::Ordering::Acquire,
                |value| value.checked_add(1),
            )
            .map_err(|_| "semantic action request id space exhausted".to_owned())?;
        let request_id = previous + 1;
        let request = aegis_semantic::SemanticActionRequest {
            request_id,
            target: aegis_core::semantic::SemanticObjectId {
                window: target.window,
                local: target.provider_node_id,
            },
            provider_node_id: target.provider_node_id,
            tree_revision: target.tree_revision,
            action,
        };
        let (completion, receiver) = std::sync::mpsc::channel();
        let broker = self.semantic_dispatch.lock().unwrap();
        let lane = broker
            .providers
            .get(&target.provider)
            .ok_or_else(|| "semantic provider is not accepting actions".to_owned())?;
        lane.sender
            .try_send(SemanticDispatchEnvelope {
                request,
                completion,
            })
            .map_err(|error| match error {
                std::sync::mpsc::TrySendError::Full(_) => {
                    "semantic provider action queue is full".to_owned()
                }
                std::sync::mpsc::TrySendError::Disconnected(_) => {
                    "semantic provider disconnected".to_owned()
                }
            })?;
        Ok(receiver)
    }
}

impl aegis_ipc::Handler for LiveState {
    /// The socket lives in `$XDG_RUNTIME_DIR` (user-only), so every local
    /// client is the user; grant all capabilities. The capability boundary
    /// becomes load-bearing for the M10 agent phase, where a scope narrows it.
    fn policy_caps(&self) -> aegis_ipc::ConnectionCapabilities {
        aegis_ipc::ConnectionCapabilities {
            query: true,
            control: true,
            input: true,
            session: true,
            interaction_domain: true,
        }
    }

    fn windows(&self) -> Vec<aegis_core::window::Window> {
        self.windows.read().unwrap().clone()
    }

    fn accessibility_windows(&self) -> Vec<aegis_semantic::AccessibilityWindowBinding> {
        self.accessibility_windows.read().unwrap().clone()
    }

    fn workspaces(&self) -> aegis_core::workspace::WorkspaceSnapshot {
        self.workspaces.read().unwrap().clone()
    }

    fn notifications(&self) -> Vec<aegis_core::notify::Notification> {
        self.notifications.lock().unwrap().snapshot()
    }

    fn outputs(&self) -> Vec<aegis_core::output::OutputInfo> {
        self.outputs.read().unwrap().clone()
    }

    fn journal_since(&self, since: u64) -> aegis_ipc::JournalSnapshot {
        self.journal.lock().unwrap().since(since)
    }

    fn interaction_domains(&self) -> aegis_core::interaction_domain::InteractionDomainSnapshot {
        self.interaction_domains.read().unwrap().clone()
    }

    fn settings(&self) -> aegis_ipc::SettingsSnapshot {
        self.settings.read().unwrap().clone()
    }

    fn system_status(&self) -> aegis_ipc::SystemStatus {
        self.system_status.read().unwrap().clone()
    }

    fn authorize_interaction_domain_action(
        &self,
        scope: &aegis_ipc::Scope,
        action: &aegis_ipc::InteractionDomainAction,
    ) -> Result<(), String> {
        let snapshot = self.interaction_domains.read().unwrap();
        authorize_interaction_domain_action_against_snapshot(scope, action, &snapshot)
    }

    fn audit_refusal(
        &self,
        conn_id: u64,
        subject: Option<&str>,
        mutation: aegis_ipc::JournalMutation,
        reason: String,
    ) {
        self.persist_and_broadcast(
            aegis_ipc::Origin::ipc(conn_id, subject),
            mutation,
            aegis_ipc::Effect::Refused { reason },
        );
    }

    fn audit_capability_use(
        &self,
        conn_id: u64,
        subject: Option<&str>,
        session: aegis_authority::ActorSessionId,
        capability: aegis_ipc::ActorCapability,
        action: aegis_ipc::CapabilityUseAction,
        effect: aegis_ipc::Effect,
    ) {
        self.persist_and_broadcast(
            aegis_ipc::Origin::ipc(conn_id, subject),
            aegis_ipc::JournalMutation::CapabilityUse {
                session,
                principal: subject
                    .and_then(|value| aegis_authority::ActorPrincipal::new(value).ok()),
                capability,
                action,
            },
            effect,
        );
    }

    fn capture_security_active(&self) -> bool {
        self.capture_delivery_gate
            .load(std::sync::atomic::Ordering::Acquire)
    }

    fn command(&self, conn_id: u64, subject: Option<&str>, cmd: aegis_ipc::Command) {
        // Best-effort: a send fails only if the main loop has dropped the
        // receiver (compositor shutting down); the command is then lost,
        // which is the right outcome.
        let _ = self.commands.lock().unwrap().send(IpcCommandRequest {
            origin: aegis_ipc::Origin::ipc(conn_id, subject),
            command: cmd,
        });
    }

    fn system_action(
        &self,
        conn_id: u64,
        subject: Option<&str>,
        action: aegis_ipc::SystemAction,
    ) -> Result<(), String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.system_controls
            .lock()
            .unwrap()
            .send(SystemControlRequest {
                origin: aegis_ipc::Origin::ipc(conn_id, subject),
                action,
                reply: reply_tx,
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        reply_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "system control timed out".to_owned())?
    }

    fn resolve_scope(&self, name: &str) -> Option<aegis_ipc::Scope> {
        self.scopes.read().unwrap().get(name).cloned()
    }

    fn agent_lookup(&self, credential: &str) -> Option<aegis_ipc::AgentIdentity> {
        self.agent_auth.read().unwrap().lookup(credential)
    }

    fn refresh_agent_identity(
        &self,
        principal: &str,
    ) -> Result<Option<aegis_ipc::AgentIdentity>, String> {
        self.agent_auth
            .read()
            .unwrap()
            .identity_for_principal(principal)
            .map(Some)
            .ok_or_else(|| "agent principal was forgotten".into())
    }

    fn start_actor_session(
        &self,
        conn_id: u64,
        principal: Option<&str>,
        policy: aegis_authority::ActorSessionPolicy,
    ) -> Result<aegis_authority::ActorSessionSnapshot, String> {
        let principal = principal
            .map(aegis_authority::ActorPrincipal::new)
            .transpose()
            .map_err(str::to_owned)?;
        let snapshot = self
            .actor_sessions
            .lock()
            .unwrap()
            .start(conn_id, principal, policy)?;
        self.audit_event(
            aegis_ipc::Origin::ipc(
                conn_id,
                snapshot
                    .principal
                    .as_ref()
                    .map(|principal| principal.as_ref()),
            ),
            aegis_ipc::JournalMutation::ActorSession {
                session: snapshot.id,
                principal: snapshot.principal.clone(),
                action: aegis_ipc::ActorSessionAuditAction::Started,
            },
        );
        Ok(snapshot)
    }

    fn authorize_actor_session(
        &self,
        session: aegis_authority::ActorSessionId,
    ) -> Result<(), String> {
        let result = self
            .actor_sessions
            .lock()
            .unwrap()
            .authorize(session)
            .map(|_| ());
        if result.is_err() {
            self.expire_due_actor_sessions();
        }
        result
    }

    fn issue_resource_grant(
        &self,
        session: aegis_authority::ActorSessionId,
        principal: Option<&str>,
        resource: aegis_authority::ActorResource,
        ttl: std::time::Duration,
        uses: u32,
        confirm_exact_resource: bool,
    ) -> Result<aegis_authority::ResourceGrant, String> {
        self.expire_due_resource_grants();
        resource.validate().map_err(str::to_owned)?;
        let principal = principal
            .map(aegis_authority::ActorPrincipal::new)
            .transpose()
            .map_err(str::to_owned)?;
        let session_snapshot = self.actor_sessions.lock().unwrap().authorize(session)?;
        if session_snapshot.principal.as_ref() != principal.as_ref() {
            return Err("resource grant Actor binding does not match the live session".into());
        }
        let conn_id = session_snapshot.connection_id;
        if confirm_exact_resource {
            let (title, body, accept_label) = resource_confirmation(&resource);
            if self.pick_confirm(conn_id, title, body, accept_label)?
                != aegis_ipc::ConfirmPickResult::Confirmed
            {
                return Err("resource grant was not confirmed".into());
            }
        }
        let grant = self.resource_grants.lock().unwrap().issue(
            session,
            principal,
            resource.required_capability(),
            resource,
            ttl,
            uses,
        )?;
        self.resource_grant_event(
            aegis_ipc::Origin::ipc(
                conn_id,
                grant.principal.as_ref().map(|principal| principal.as_ref()),
            ),
            &grant,
            aegis_ipc::ResourceGrantAuditAction::Issued,
        );
        Ok(grant)
    }

    fn consume_resource_grant(
        &self,
        session: aegis_authority::ActorSessionId,
        principal: Option<&str>,
        id: &aegis_authority::ResourceGrantId,
        resource: &aegis_authority::ActorResource,
    ) -> Result<aegis_authority::ResourceGrant, String> {
        self.expire_due_resource_grants();
        let session_snapshot = self.actor_sessions.lock().unwrap().authorize(session)?;
        let principal = principal
            .map(aegis_authority::ActorPrincipal::new)
            .transpose()
            .map_err(str::to_owned)?;
        if session_snapshot.principal.as_ref() != principal.as_ref() {
            return Err("resource grant Actor binding does not match the live session".into());
        }
        let grant = self.resource_grants.lock().unwrap().consume(
            session,
            principal.as_ref(),
            id,
            resource,
        )?;
        self.resource_grant_event(
            aegis_ipc::Origin::ipc(
                session_snapshot.connection_id,
                grant.principal.as_ref().map(|principal| principal.as_ref()),
            ),
            &grant,
            aegis_ipc::ResourceGrantAuditAction::Consumed,
        );
        Ok(grant)
    }

    fn revoke_resource_grant(
        &self,
        session: aegis_authority::ActorSessionId,
        principal: Option<&str>,
        id: &aegis_authority::ResourceGrantId,
    ) -> Result<(), String> {
        let session_snapshot = self.actor_sessions.lock().unwrap().authorize(session)?;
        let principal = principal
            .map(aegis_authority::ActorPrincipal::new)
            .transpose()
            .map_err(str::to_owned)?;
        if session_snapshot.principal.as_ref() != principal.as_ref() {
            return Err("resource grant Actor binding does not match the live session".into());
        }
        let grant = self
            .resource_grants
            .lock()
            .unwrap()
            .revoke(session, principal.as_ref(), id)?;
        self.resource_grant_event(
            aegis_ipc::Origin::ipc(
                session_snapshot.connection_id,
                grant.principal.as_ref().map(|principal| principal.as_ref()),
            ),
            &grant,
            aegis_ipc::ResourceGrantAuditAction::Revoked,
        );
        Ok(())
    }

    fn publish_accessibility_tree(
        &self,
        principal: &str,
        update: aegis_semantic::AccessibilityTreeUpdate,
    ) -> Result<(), String> {
        let provider = aegis_semantic::SemanticProviderId::new(principal).map_err(str::to_owned)?;
        let (reply, response) = std::sync::mpsc::channel();
        self.semantic_tree_updates
            .lock()
            .unwrap()
            .send(SemanticTreeUpdateRequest {
                provider,
                update,
                reply,
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        response
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "accessibility tree publication timed out".to_owned())?
    }

    fn next_accessibility_action(
        &self,
        session: aegis_authority::ActorSessionId,
        principal: &str,
        timeout: std::time::Duration,
    ) -> Result<Option<aegis_semantic::SemanticActionRequest>, String> {
        let provider = aegis_semantic::SemanticProviderId::new(principal).map_err(str::to_owned)?;
        let session_snapshot = self.actor_sessions.lock().unwrap().authorize(session)?;
        if session_snapshot.principal.as_deref() != Some(principal) {
            return Err("semantic provider principal does not own the Actor session".into());
        }
        let max_pending = session_snapshot.max_pending_actions as usize;
        let receiver = {
            let mut broker = self.semantic_dispatch.lock().unwrap();
            let pending = broker
                .pending
                .keys()
                .filter(|(owner, _)| owner == &provider)
                .count();
            if pending >= max_pending {
                return Err("semantic provider pending-action quota exhausted".into());
            }
            broker.receiver(provider.clone(), session)?
        };
        let received = receiver.lock().unwrap().recv_timeout(timeout);
        match received {
            Ok(envelope) => {
                if let Err(message) = self.actor_sessions.lock().unwrap().authorize(session) {
                    let _ = envelope.completion.send(Err(message.clone()));
                    return Err(message);
                }
                self.semantic_dispatch.lock().unwrap().pending.insert(
                    (provider, envelope.request.request_id),
                    (session, envelope.completion),
                );
                Ok(Some(envelope.request))
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => Ok(None),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                Err("semantic provider queue was revoked".into())
            }
        }
    }

    fn complete_accessibility_action(
        &self,
        session: aegis_authority::ActorSessionId,
        principal: &str,
        request_id: u64,
        result: Result<(), String>,
    ) -> Result<(), String> {
        let provider = aegis_semantic::SemanticProviderId::new(principal).map_err(str::to_owned)?;
        let session_snapshot = self.actor_sessions.lock().unwrap().authorize(session)?;
        if session_snapshot.principal.as_deref() != Some(principal) {
            return Err("semantic provider principal does not own the Actor session".into());
        }
        let mut broker = self.semantic_dispatch.lock().unwrap();
        let key = (provider, request_id);
        let (owner, _) = broker
            .pending
            .get(&key)
            .ok_or_else(|| "unknown or already completed semantic action".to_owned())?;
        if *owner != session {
            return Err("semantic action belongs to another provider session".into());
        }
        let (_, completion) = broker
            .pending
            .remove(&key)
            .expect("pending semantic action was verified under the same lock");
        drop(broker);
        completion
            .send(result)
            .map_err(|_| "semantic action requester disconnected".to_owned())
    }

    fn lockdown(&self) -> bool {
        self.lockdown
    }

    fn pair_agent(
        &self,
        conn_id: u64,
        label: Option<&str>,
        requested: &[aegis_ipc::ActorCapability],
    ) -> Result<aegis_ipc::PairedAgent, String> {
        {
            let auth = self.agent_auth.read().unwrap();
            if auth.is_denied(label) {
                return Err("agent pairing was denied earlier in this session".into());
            }
        }
        let collision = label
            .map(|label| self.agent_auth.read().unwrap().label_collision(label))
            .unwrap_or(false);
        let title = format!(
            "{} wants to borrow desktop capabilities",
            label.unwrap_or("An agent")
        );
        let warning = collision
            .then(|| "A different installation already registered under this name.".to_owned());
        let groups = capability_groups(requested)
            .into_iter()
            .map(|group| aegis_shell::CapabilityGroup {
                key: group.key.to_owned(),
                label: group.label.to_owned(),
                gated: group.gated,
                enabled: true,
            })
            .collect();
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.capability_pick_controls
            .lock()
            .unwrap()
            .send(CapabilityPickControlRequest {
                conn_id,
                action: CapabilityPickControl::Start {
                    params: aegis_shell::CapabilityPickParams {
                        title,
                        warning,
                        groups,
                    },
                    reply: reply_tx,
                },
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        // The pairing prompt parks like the picks; the timeout closes the
        // chrome so it never lingers for a dead requester.
        match reply_rx.recv_timeout(PICK_TIMEOUT) {
            Ok(Ok(aegis_shell::CapabilityPickResult {
                approved: Some(keys),
            })) => {
                let ops: Vec<aegis_ipc::ActorCapability> = keys
                    .iter()
                    .filter_map(|key| aegis_ipc::ActorCapability::from_name(key))
                    .collect();
                let paired = self.agent_auth.write().unwrap().issue(label, &ops)?;
                self.auth_event(
                    Some(conn_id),
                    &paired.principal,
                    aegis_ipc::AgentAuthAction::Paired,
                );
                Ok(paired)
            }
            Ok(Ok(aegis_shell::CapabilityPickResult { approved: None })) => {
                self.agent_auth.write().unwrap().deny(label);
                Err("pairing denied by the user".into())
            }
            Ok(Err(message)) => Err(message),
            Err(_) => {
                let _ = self.capability_pick_controls.lock().unwrap().send(
                    CapabilityPickControlRequest {
                        conn_id,
                        action: CapabilityPickControl::Cancel,
                    },
                );
                Err("pairing timed out".into())
            }
        }
    }

    fn grant_for(&self, principal: &str, op: aegis_ipc::ActorCapability) -> Option<bool> {
        self.grants.read().unwrap().decision_for(principal, op)
    }

    fn request_grant(
        &self,
        conn_id: u64,
        principal: &str,
        op: aegis_ipc::ActorCapability,
    ) -> Result<bool, String> {
        let label = self
            .agent_auth
            .read()
            .unwrap()
            .principals()
            .iter()
            .find(|record| record.id == principal)
            .and_then(|record| record.label.clone());
        let title = format!(
            "Allow {} to borrow a sensitive capability?",
            label.as_deref().unwrap_or(principal)
        );
        let body = format!(
            "{}\n\nAllow once: just this time.\nThis session: until you log out.\nAlways: \
             remembered across sessions.\nDeny: refused for this session.",
            op.label()
        );
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.confirm_pick_controls
            .lock()
            .unwrap()
            .send(ConfirmPickControlRequest {
                conn_id,
                action: ConfirmPickControl::Start {
                    title,
                    body,
                    accept_label: None,
                    style: aegis_shell::ConfirmPickStyle::Grant,
                    reply: reply_tx,
                },
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        // The grant prompt parks like the picks; the timeout closes the
        // chrome so it never lingers for a dead requester.
        match reply_rx.recv_timeout(PICK_TIMEOUT) {
            Ok(Ok(aegis_shell::ConfirmAnswer::AllowOnce)) => {
                self.auth_event(
                    Some(conn_id),
                    principal,
                    aegis_ipc::AgentAuthAction::Granted {
                        op,
                        persistence: aegis_ipc::GrantPersistence::Once,
                    },
                );
                Ok(true)
            }
            Ok(Ok(aegis_shell::ConfirmAnswer::AllowSession)) => {
                self.grants
                    .write()
                    .unwrap()
                    .record(principal, op, true, true)?;
                self.auth_event(
                    Some(conn_id),
                    principal,
                    aegis_ipc::AgentAuthAction::Granted {
                        op,
                        persistence: aegis_ipc::GrantPersistence::Session,
                    },
                );
                Ok(true)
            }
            Ok(Ok(aegis_shell::ConfirmAnswer::AllowAlways)) => {
                self.grants
                    .write()
                    .unwrap()
                    .record(principal, op, true, false)?;
                self.auth_event(
                    Some(conn_id),
                    principal,
                    aegis_ipc::AgentAuthAction::Granted {
                        op,
                        persistence: aegis_ipc::GrantPersistence::Always,
                    },
                );
                Ok(true)
            }
            Ok(Ok(_)) => {
                self.grants
                    .write()
                    .unwrap()
                    .record(principal, op, false, true)?;
                self.auth_event(
                    Some(conn_id),
                    principal,
                    aegis_ipc::AgentAuthAction::Granted {
                        op,
                        persistence: aegis_ipc::GrantPersistence::DeniedSession,
                    },
                );
                Ok(false)
            }
            Ok(Err(message)) => Err(message),
            Err(_) => {
                let _ =
                    self.confirm_pick_controls
                        .lock()
                        .unwrap()
                        .send(ConfirmPickControlRequest {
                            conn_id,
                            action: ConfirmPickControl::Cancel,
                        });
                Err("grant request timed out".into())
            }
        }
    }

    fn authorize_interaction_domain_action_granted(
        &self,
        scope: &aegis_ipc::Scope,
        action: &aegis_ipc::InteractionDomainAction,
    ) -> Result<(), String> {
        let snapshot = self.interaction_domains.read().unwrap();
        authorize_interaction_domain_action_granted_against_snapshot(scope, action, &snapshot)
    }

    fn agent_principals(&self) -> Vec<aegis_ipc::AgentPrincipalInfo> {
        self.agent_auth
            .read()
            .unwrap()
            .principals()
            .iter()
            .map(|record| aegis_ipc::AgentPrincipalInfo {
                principal: record.id.clone(),
                label: record.label.clone(),
                pregranted: record.pregranted.clone(),
                gated: record.gated.clone(),
                created_at: record.created_at,
            })
            .collect()
    }

    fn agent_grants(&self, principal: Option<&str>) -> Vec<aegis_ipc::AgentGrantInfo> {
        self.grants.read().unwrap().list(principal)
    }

    fn rename_agent_principal(&self, principal: &str, label: Option<&str>) -> Result<(), String> {
        self.agent_auth.write().unwrap().rename(principal, label)?;
        self.auth_event(None, principal, aegis_ipc::AgentAuthAction::Renamed);
        Ok(())
    }

    fn forget_agent_principal(&self, principal: &str) -> Result<(), String> {
        self.agent_auth.write().unwrap().forget(principal)?;
        self.grants.write().unwrap().forget_principal(principal);
        let principal = aegis_authority::ActorPrincipal::new(principal).map_err(str::to_owned)?;
        let revoked = self
            .actor_sessions
            .lock()
            .unwrap()
            .revoke_principal(&principal);
        for snapshot in revoked {
            self.terminate_actor_session(
                snapshot,
                aegis_ipc::ActorSessionAuditAction::PrincipalRevoked,
                aegis_ipc::Origin::Internal,
            );
        }
        // Defensive cleanup for records from an older runtime that may not
        // have had a retained live-session entry. Current grants are always
        // session-bound, so this is normally empty.
        for grant in self
            .resource_grants
            .lock()
            .unwrap()
            .revoke_principal(&principal)
        {
            self.resource_grant_event(
                aegis_ipc::Origin::Internal,
                &grant,
                aegis_ipc::ResourceGrantAuditAction::Revoked,
            );
        }
        if let Ok(provider) = aegis_semantic::SemanticProviderId::new(principal.as_ref()) {
            let _ = self.semantic_provider_revocations.try_send(provider);
        }
        self.auth_event(
            None,
            principal.as_ref(),
            aegis_ipc::AgentAuthAction::Forgotten,
        );
        Ok(())
    }

    fn set_agent_ceiling(
        &self,
        principal: &str,
        pregranted: &[aegis_ipc::ActorCapability],
        gated: &[aegis_ipc::ActorCapability],
    ) -> Result<(), String> {
        self.agent_auth.write().unwrap().set_ceiling(
            principal,
            pregranted.to_vec(),
            gated.to_vec(),
        )?;
        self.auth_event(None, principal, aegis_ipc::AgentAuthAction::CeilingChanged);
        Ok(())
    }

    fn register_agent(
        &self,
        label: Option<&str>,
        pregranted: &[aegis_ipc::ActorCapability],
        gated: &[aegis_ipc::ActorCapability],
    ) -> Result<(String, String), String> {
        let (principal, credential) = self.agent_auth.write().unwrap().register(
            label,
            pregranted.to_vec(),
            gated.to_vec(),
        )?;
        self.auth_event(None, &principal, aegis_ipc::AgentAuthAction::Paired);
        Ok((principal, credential))
    }

    fn revoke_agent_grant(
        &self,
        principal: &str,
        op: aegis_ipc::ActorCapability,
    ) -> Result<(), String> {
        self.grants.write().unwrap().revoke(principal, op)?;
        self.auth_event(
            None,
            principal,
            aegis_ipc::AgentAuthAction::GrantRevoked { op },
        );
        Ok(())
    }

    fn capture_output(
        &self,
        region: Option<aegis_core::Rect>,
    ) -> Result<aegis_ipc::CaptureOutputPayload, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.capture
            .lock()
            .unwrap()
            .send(CaptureRequest {
                reply: reply_tx,
                region,
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        // The main loop answers after the next frame; two seconds is far
        // beyond any frame budget and bounds a wedged-GPU stall.
        reply_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "capture timed out".to_owned())?
    }

    fn interaction_domain_action(
        &self,
        conn_id: u64,
        subject: Option<&str>,
        action: aegis_ipc::InteractionDomainAction,
    ) -> Result<aegis_ipc::InteractionDomainActionResult, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.interaction_domain_controls
            .lock()
            .unwrap()
            .send(InteractionDomainControlRequest {
                origin: aegis_ipc::Origin::ipc(conn_id, subject),
                subject: subject.map(str::to_owned),
                action,
                reply: reply_tx,
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        reply_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "interaction_domain operation timed out".to_owned())?
    }

    fn settings_action(
        &self,
        conn_id: u64,
        subject: Option<&str>,
        expected_revision: Option<u64>,
        action: aegis_ipc::SettingsAction,
    ) -> Result<aegis_ipc::SettingsReceipt, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.settings_controls
            .lock()
            .unwrap()
            .send(SettingsControlRequest {
                origin: aegis_ipc::Origin::ipc(conn_id, subject),
                expected_revision,
                action,
                reply: reply_tx,
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        reply_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "settings operation timed out".to_owned())?
    }

    fn set_wallpaper(&self, _conn_id: u64, path: std::path::PathBuf) -> Result<(), String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.wallpaper_controls
            .lock()
            .unwrap()
            .send(WallpaperControlRequest {
                path,
                reply: reply_tx,
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        // A video wallpaper's first decode can take a moment.
        reply_rx
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| "wallpaper operation timed out".to_owned())?
    }

    fn capture_interaction_domain(
        &self,
        conn_id: u64,
        subject: Option<&str>,
        interaction_domain: aegis_core::interaction_domain::InteractionDomainId,
        region: Option<aegis_core::Rect>,
    ) -> Result<aegis_ipc::CaptureInteractionDomainPayload, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        let (actor, session) = self.actor_binding(conn_id, subject)?;
        self.interaction_domain_capture
            .lock()
            .unwrap()
            .send(InteractionDomainCaptureRequest {
                actor,
                max_observations: session.max_observations as usize,
                interaction_domain,
                reply: reply_tx,
                region,
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        reply_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "interaction_domain capture timed out".to_owned())?
    }

    fn observe_interaction_domain(
        &self,
        conn_id: u64,
        subject: Option<&str>,
        interaction_domain: aegis_core::interaction_domain::InteractionDomainId,
    ) -> Result<aegis_ipc::SemanticObservation, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        let (actor, session) = self.actor_binding(conn_id, subject)?;
        self.interaction_domain_observe
            .lock()
            .unwrap()
            .send(InteractionDomainObserveRequest {
                actor,
                max_observations: session.max_observations as usize,
                interaction_domain,
                reply: reply_tx,
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        reply_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "InteractionDomain observation timed out".to_owned())?
    }

    fn act_in_interaction_domain(
        &self,
        conn_id: u64,
        subject: Option<&str>,
        scope_name: Option<&str>,
        scope: aegis_ipc::Scope,
        intent: aegis_ipc::ActorActionIntent,
    ) -> Result<aegis_ipc::ActorActionReceipt, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        let (actor, _) = self.actor_binding(conn_id, subject)?;
        self.actor_actions
            .lock()
            .unwrap()
            .send(InteractionDomainActorActionRequest {
                actor,
                scope_name: scope_name.map(str::to_owned),
                scope,
                origin: aegis_ipc::Origin::ipc(conn_id, subject),
                intent,
                reply: reply_tx,
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        reply_rx
            .recv_timeout(std::time::Duration::from_secs(7))
            .map_err(|_| "Actor action timed out".to_owned())?
    }

    fn connection_disconnected(&self, conn_id: u64) {
        let revoked = self
            .actor_sessions
            .lock()
            .unwrap()
            .revoke_connection(conn_id);
        for snapshot in revoked {
            let origin = aegis_ipc::Origin::ipc(
                conn_id,
                snapshot
                    .principal
                    .as_ref()
                    .map(|principal| principal.as_ref()),
            );
            self.terminate_actor_session(
                snapshot,
                aegis_ipc::ActorSessionAuditAction::Disconnected,
                origin,
            );
        }
    }

    fn discard_observation(
        &self,
        conn_id: u64,
        subject: Option<&str>,
        token: &aegis_ipc::ObservationToken,
    ) {
        if let Ok((actor, _)) = self.actor_binding(conn_id, subject) {
            let _ = self.observation_discards.send(ObservationDiscardRequest {
                actor,
                token: token.clone(),
            });
        }
    }

    fn stream_output_start(
        &self,
        conn_id: u64,
        max_fps: Option<u32>,
        target: aegis_ipc::StreamTarget,
    ) -> Result<aegis_ipc::StreamInfo, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.stream_controls
            .lock()
            .unwrap()
            .send(StreamControlRequest {
                conn_id,
                action: StreamControl::Start {
                    max_fps,
                    target,
                    reply: reply_tx,
                },
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        reply_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "stream start timed out".to_owned())?
    }

    fn stream_output_stop(&self, stream_id: u64) {
        let _ = self
            .stream_controls
            .lock()
            .unwrap()
            .send(StreamControlRequest {
                conn_id: 0,
                action: StreamControl::Stop { stream_id },
            });
    }

    fn streams_disconnected(&self, conn_id: u64) {
        let _ = self
            .stream_controls
            .lock()
            .unwrap()
            .send(StreamControlRequest {
                conn_id,
                action: StreamControl::Disconnect,
            });
    }

    fn set_idle_inhibit(&self, conn_id: u64, inhibit: bool) -> Result<bool, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.idle_controls
            .lock()
            .unwrap()
            .send(IdleControlRequest {
                conn_id,
                action: IdleControl::Set {
                    inhibit,
                    reply: reply_tx,
                },
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        reply_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "idle inhibit timed out".to_owned())?
    }

    fn idle_inhibit_disconnected(&self, conn_id: u64) {
        let _ = self.idle_controls.lock().unwrap().send(IdleControlRequest {
            conn_id,
            action: IdleControl::Disconnect,
        });
    }

    fn pick_target(
        &self,
        conn_id: u64,
        kind: aegis_ipc::PickKind,
    ) -> Result<aegis_ipc::PickResult, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.pick_controls
            .lock()
            .unwrap()
            .send(PickControlRequest {
                conn_id,
                action: PickControl::Start {
                    kind,
                    reply: reply_tx,
                },
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        // The reply parks until the user confirms or cancels. The timeout
        // bounds an abandoned picker; expiring also cancels the chrome so
        // the overlay never lingers for a dead requester (ADR-0054).
        match reply_rx.recv_timeout(PICK_TIMEOUT) {
            Ok(result) => result,
            Err(_) => {
                let _ = self.pick_controls.lock().unwrap().send(PickControlRequest {
                    conn_id,
                    action: PickControl::Cancel,
                });
                Err("interactive pick timed out".to_owned())
            }
        }
    }

    fn pick_confirm(
        &self,
        conn_id: u64,
        title: String,
        body: String,
        accept_label: Option<String>,
    ) -> Result<aegis_ipc::ConfirmPickResult, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.confirm_pick_controls
            .lock()
            .unwrap()
            .send(ConfirmPickControlRequest {
                conn_id,
                action: ConfirmPickControl::Start {
                    title,
                    body,
                    accept_label,
                    style: aegis_shell::ConfirmPickStyle::YesNo,
                    reply: reply_tx,
                },
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        // The reply parks until the user confirms or cancels, exactly like
        // the other picks; the timeout bounds an abandoned dialog and
        // cancels the chrome so the panel never lingers for a dead
        // requester. The yes/no style only ever answers Confirmed or
        // Cancelled; map defensively anyway.
        match reply_rx.recv_timeout(PICK_TIMEOUT) {
            Ok(Ok(aegis_shell::ConfirmAnswer::Confirmed)) => {
                Ok(aegis_ipc::ConfirmPickResult::Confirmed)
            }
            Ok(Ok(_)) => Ok(aegis_ipc::ConfirmPickResult::Cancelled),
            Ok(Err(message)) => Err(message),
            Err(_) => {
                let _ =
                    self.confirm_pick_controls
                        .lock()
                        .unwrap()
                        .send(ConfirmPickControlRequest {
                            conn_id,
                            action: ConfirmPickControl::Cancel,
                        });
                Err("confirmation timed out".to_owned())
            }
        }
    }

    fn prompt_secret(
        &self,
        conn_id: u64,
        title: String,
        reason: Option<String>,
    ) -> Result<aegis_ipc::SecretPromptResult, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.secret_prompt_controls
            .lock()
            .unwrap()
            .send(SecretPromptControlRequest {
                conn_id,
                action: SecretPromptControl::Start {
                    title,
                    reason,
                    reply: reply_tx,
                },
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        // The reply parks until the user confirms or cancels, exactly like
        // the other picks; the timeout bounds an abandoned prompt and
        // cancels the chrome so the panel never lingers for a dead
        // requester.
        match reply_rx.recv_timeout(PICK_TIMEOUT) {
            Ok(result) => result,
            Err(_) => {
                let _ =
                    self.secret_prompt_controls
                        .lock()
                        .unwrap()
                        .send(SecretPromptControlRequest {
                            conn_id,
                            action: SecretPromptControl::Cancel,
                        });
                Err("secret prompt timed out".to_owned())
            }
        }
    }

    fn pick_app(
        &self,
        conn_id: u64,
        choices: Vec<String>,
        subject: Option<String>,
        last_choice: Option<String>,
    ) -> Result<aegis_ipc::AppPickResult, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.app_pick_controls
            .lock()
            .unwrap()
            .send(AppPickControlRequest {
                conn_id,
                action: AppPickControl::Start {
                    choices,
                    subject,
                    last_choice,
                    reply: reply_tx,
                },
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        // The reply parks until the user confirms or cancels. The timeout
        // bounds an abandoned picker and cancels the chrome so the panel
        // never lingers for a dead requester.
        match reply_rx.recv_timeout(PICK_TIMEOUT) {
            Ok(result) => result,
            Err(_) => {
                let _ = self
                    .app_pick_controls
                    .lock()
                    .unwrap()
                    .send(AppPickControlRequest {
                        conn_id,
                        action: AppPickControl::Cancel,
                    });
                Err("app pick timed out".to_owned())
            }
        }
    }
}

#[cfg(test)]
mod actor_action_scope_tests {
    use super::*;
    use aegis_ipc::{ActorCapability, AuthorizationDecision};

    #[test]
    fn commit_revalidation_fails_closed_on_ceiling_changes() {
        let permit = AuthorizationDecision::Permit;
        let ask = AuthorizationDecision::Ask(ActorCapability::InjectInteractionDomainInput);
        let deny = AuthorizationDecision::Deny;

        assert!(actor_action_scope_still_authorized(permit, permit, None));
        assert!(actor_action_scope_still_authorized(ask, ask, None));
        assert!(actor_action_scope_still_authorized(ask, permit, None));
        assert!(!actor_action_scope_still_authorized(ask, ask, Some(false)));
        assert!(!actor_action_scope_still_authorized(permit, ask, None));
        assert!(actor_action_scope_still_authorized(permit, ask, Some(true)));
        assert!(!actor_action_scope_still_authorized(
            permit,
            deny,
            Some(true)
        ));
    }

    fn semantic_request(request_id: u64) -> aegis_semantic::SemanticActionRequest {
        aegis_semantic::SemanticActionRequest {
            request_id,
            target: aegis_core::semantic::SemanticObjectId {
                window: aegis_core::window::WindowId(7),
                local: 2,
            },
            provider_node_id: 2,
            tree_revision: 3,
            action: aegis_core::semantic::SemanticActionIntent::Invoke,
        }
    }

    #[test]
    fn semantic_broker_is_single_session_bounded_and_revocation_completes_pending() {
        let provider = aegis_semantic::SemanticProviderId::new("atspi.test").unwrap();
        let session = aegis_authority::ActorSessionId(7);
        let mut broker = SemanticDispatchBroker::default();
        let receiver = broker.receiver(provider.clone(), session).unwrap();
        assert!(
            broker
                .receiver(provider.clone(), aegis_authority::ActorSessionId(8))
                .is_err()
        );

        for request_id in 1..=64 {
            let (completion, _result) = std::sync::mpsc::channel();
            broker
                .providers
                .get(&provider)
                .unwrap()
                .sender
                .try_send(SemanticDispatchEnvelope {
                    request: semantic_request(request_id),
                    completion,
                })
                .unwrap();
        }
        let (completion, _result) = std::sync::mpsc::channel();
        assert!(matches!(
            broker
                .providers
                .get(&provider)
                .unwrap()
                .sender
                .try_send(SemanticDispatchEnvelope {
                    request: semantic_request(65),
                    completion,
                }),
            Err(std::sync::mpsc::TrySendError::Full(_))
        ));

        let envelope = receiver.lock().unwrap().recv().unwrap();
        let (result_tx, result_rx) = std::sync::mpsc::channel();
        broker.pending.insert(
            (provider.clone(), envelope.request.request_id),
            (session, result_tx),
        );
        broker.revoke_session(session);
        assert!(result_rx.recv().unwrap().is_err());
        assert!(!broker.providers.contains_key(&provider));
    }
}
