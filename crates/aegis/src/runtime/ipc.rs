use super::*;

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
    pub(super) realm_controls: std::sync::mpsc::Sender<RealmControlRequest>,
    pub(super) settings_controls: std::sync::mpsc::Sender<SettingsControlRequest>,
    pub(super) wallpaper_controls: std::sync::mpsc::Sender<WallpaperControlRequest>,
    pub(super) realm_capture: std::sync::mpsc::Sender<RealmCaptureRequest>,
    pub(super) stream_controls: std::sync::mpsc::Sender<StreamControlRequest>,
    pub(super) idle_controls: std::sync::mpsc::Sender<IdleControlRequest>,
    pub(super) pick_controls: std::sync::mpsc::Sender<PickControlRequest>,
    pub(super) file_pick_controls: std::sync::mpsc::Sender<FilePickControlRequest>,
    pub(super) app_pick_controls: std::sync::mpsc::Sender<AppPickControlRequest>,
    pub(super) secret_prompt_controls: std::sync::mpsc::Sender<SecretPromptControlRequest>,
    pub(super) confirm_pick_controls: std::sync::mpsc::Sender<ConfirmPickControlRequest>,
    pub(super) capability_pick_controls: std::sync::mpsc::Sender<CapabilityPickControlRequest>,
    pub(super) journal_refusals: std::sync::mpsc::Sender<JournalRefusalRequest>,
    pub(super) auth_events: std::sync::mpsc::Sender<AuthEventRequest>,
}

pub(super) struct LiveState {
    windows: std::sync::RwLock<Vec<aegis_core::window::Window>>,
    workspaces: std::sync::RwLock<aegis_core::workspace::WorkspaceSnapshot>,
    outputs: std::sync::RwLock<Vec<aegis_core::output::OutputInfo>>,
    realms: std::sync::RwLock<aegis_core::realm::RealmSnapshot>,
    settings: std::sync::RwLock<aegis_ipc::SettingsSnapshot>,
    system_status: std::sync::RwLock<aegis_ipc::SystemStatus>,
    notifications: std::sync::Arc<std::sync::Mutex<aegis_core::notify::NotificationQueue>>,
    journal: std::sync::Arc<std::sync::Mutex<aegis_ipc::Journal>>,
    commands: std::sync::Mutex<std::sync::mpsc::Sender<IpcCommandRequest>>,
    system_controls: std::sync::Mutex<std::sync::mpsc::Sender<SystemControlRequest>>,
    capture: std::sync::Mutex<std::sync::mpsc::Sender<CaptureRequest>>,
    realm_controls: std::sync::Mutex<std::sync::mpsc::Sender<RealmControlRequest>>,
    settings_controls: std::sync::Mutex<std::sync::mpsc::Sender<SettingsControlRequest>>,
    wallpaper_controls: std::sync::Mutex<std::sync::mpsc::Sender<WallpaperControlRequest>>,
    realm_capture: std::sync::Mutex<std::sync::mpsc::Sender<RealmCaptureRequest>>,
    stream_controls: std::sync::Mutex<std::sync::mpsc::Sender<StreamControlRequest>>,
    idle_controls: std::sync::Mutex<std::sync::mpsc::Sender<IdleControlRequest>>,
    pick_controls: std::sync::Mutex<std::sync::mpsc::Sender<PickControlRequest>>,
    file_pick_controls: std::sync::Mutex<std::sync::mpsc::Sender<FilePickControlRequest>>,
    app_pick_controls: std::sync::Mutex<std::sync::mpsc::Sender<AppPickControlRequest>>,
    secret_prompt_controls: std::sync::Mutex<std::sync::mpsc::Sender<SecretPromptControlRequest>>,
    confirm_pick_controls: std::sync::Mutex<std::sync::mpsc::Sender<ConfirmPickControlRequest>>,
    capability_pick_controls:
        std::sync::Mutex<std::sync::mpsc::Sender<CapabilityPickControlRequest>>,
    journal_refusals: std::sync::mpsc::Sender<JournalRefusalRequest>,
    auth_events: std::sync::mpsc::Sender<AuthEventRequest>,
    capture_delivery_gate: std::sync::Arc<std::sync::atomic::AtomicBool>,
    scopes: std::sync::RwLock<std::collections::HashMap<String, aegis_ipc::Scope>>,
    /// Pairing registry for capability-borrowing agents (ADR-0088).
    agent_auth: std::sync::RwLock<PrincipalRegistry>,
    /// Runtime-grant decisions for paired agents (ADR-0088).
    grants: std::sync::RwLock<GrantStore>,
    /// `[agent] lockdown`: strip privileged capabilities from connections
    /// that neither present a built-in scope nor pair as an agent.
    lockdown: bool,
}

impl LiveState {
    pub(super) fn new(
        channels: LiveChannels,
        capture_delivery_gate: std::sync::Arc<std::sync::atomic::AtomicBool>,
        notifications: std::sync::Arc<std::sync::Mutex<aegis_core::notify::NotificationQueue>>,
        journal: std::sync::Arc<std::sync::Mutex<aegis_ipc::Journal>>,
        scopes: std::collections::HashMap<String, aegis_ipc::Scope>,
        agent_auth: PrincipalRegistry,
        grants: GrantStore,
        lockdown: bool,
    ) -> LiveState {
        LiveState {
            windows: std::sync::RwLock::new(Vec::new()),
            workspaces: std::sync::RwLock::new(
                aegis_core::workspace::WorkspaceModel::new().snapshot(),
            ),
            outputs: std::sync::RwLock::new(Vec::new()),
            realms: std::sync::RwLock::new(aegis_core::realm::RealmModel::new().snapshot()),
            settings: std::sync::RwLock::new(aegis_ipc::SettingsSnapshot::default()),
            system_status: std::sync::RwLock::new(aegis_ipc::SystemStatus::default()),
            notifications,
            journal,
            commands: std::sync::Mutex::new(channels.commands),
            system_controls: std::sync::Mutex::new(channels.system_controls),
            capture: std::sync::Mutex::new(channels.capture),
            realm_controls: std::sync::Mutex::new(channels.realm_controls),
            settings_controls: std::sync::Mutex::new(channels.settings_controls),
            wallpaper_controls: std::sync::Mutex::new(channels.wallpaper_controls),
            realm_capture: std::sync::Mutex::new(channels.realm_capture),
            stream_controls: std::sync::Mutex::new(channels.stream_controls),
            idle_controls: std::sync::Mutex::new(channels.idle_controls),
            pick_controls: std::sync::Mutex::new(channels.pick_controls),
            file_pick_controls: std::sync::Mutex::new(channels.file_pick_controls),
            app_pick_controls: std::sync::Mutex::new(channels.app_pick_controls),
            secret_prompt_controls: std::sync::Mutex::new(channels.secret_prompt_controls),
            confirm_pick_controls: std::sync::Mutex::new(channels.confirm_pick_controls),
            capability_pick_controls: std::sync::Mutex::new(channels.capability_pick_controls),
            journal_refusals: channels.journal_refusals,
            auth_events: channels.auth_events,
            capture_delivery_gate,
            scopes: std::sync::RwLock::new(scopes),
            agent_auth: std::sync::RwLock::new(agent_auth),
            grants: std::sync::RwLock::new(grants),
            lockdown,
        }
    }

    /// Enqueue one positive agent-authorization lifecycle event
    /// (ADR-0088) for the main loop to journal with `Effect::Applied`.
    /// Best-effort, like [`aegis_ipc::Handler::audit_refusal`]: a send
    /// fails only when the compositor is shutting down.
    fn auth_event(
        &self,
        conn_id: Option<u64>,
        principal: &str,
        action: aegis_ipc::AgentAuthAction,
    ) {
        let origin = conn_id
            .map(|conn_id| aegis_ipc::Origin::Ipc { conn_id })
            .unwrap_or(aegis_ipc::Origin::Internal);
        let _ = self.auth_events.send(AuthEventRequest {
            origin,
            mutation: aegis_ipc::JournalMutation::AgentAuth {
                principal: principal.to_owned(),
                action,
            },
        });
    }

    pub(super) fn set_windows(&self, windows: Vec<aegis_core::window::Window>) {
        *self.windows.write().unwrap() = windows;
    }

    pub(super) fn set_workspaces(&self, snapshot: aegis_core::workspace::WorkspaceSnapshot) {
        *self.workspaces.write().unwrap() = snapshot;
    }

    pub(super) fn set_outputs(&self, outputs: Vec<aegis_core::output::OutputInfo>) {
        *self.outputs.write().unwrap() = outputs;
    }

    pub(super) fn set_realms(&self, snapshot: aegis_core::realm::RealmSnapshot) {
        *self.realms.write().unwrap() = snapshot;
    }

    pub(super) fn set_settings(&self, snapshot: aegis_ipc::SettingsSnapshot) {
        *self.settings.write().unwrap() = snapshot;
    }

    pub(super) fn set_system_status(&self, snapshot: aegis_ipc::SystemStatus) {
        *self.system_status.write().unwrap() = snapshot;
    }
}

impl aegis_ipc::Handler for LiveState {
    /// The socket lives in `$XDG_RUNTIME_DIR` (user-only), so every local
    /// client is the user; grant all capabilities. The capability boundary
    /// becomes load-bearing for the M10 agent phase, where a scope narrows it.
    fn policy_caps(&self) -> aegis_ipc::Capabilities {
        aegis_ipc::Capabilities {
            query: true,
            control: true,
            input: true,
            session: true,
            realm: true,
        }
    }

    fn windows(&self) -> Vec<aegis_core::window::Window> {
        self.windows.read().unwrap().clone()
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

    fn realms(&self) -> aegis_core::realm::RealmSnapshot {
        self.realms.read().unwrap().clone()
    }

    fn settings(&self) -> aegis_ipc::SettingsSnapshot {
        self.settings.read().unwrap().clone()
    }

    fn system_status(&self) -> aegis_ipc::SystemStatus {
        self.system_status.read().unwrap().clone()
    }

    fn authorize_realm_action(
        &self,
        scope: &aegis_ipc::Scope,
        action: &aegis_ipc::RealmAction,
    ) -> Result<(), String> {
        let snapshot = self.realms.read().unwrap();
        authorize_realm_action_against_snapshot(scope, action, &snapshot)
    }

    fn audit_refusal(&self, conn_id: u64, mutation: aegis_ipc::JournalMutation, reason: String) {
        let _ = self.journal_refusals.send(JournalRefusalRequest {
            origin: aegis_ipc::Origin::Ipc { conn_id },
            mutation,
            reason,
        });
    }

    fn capture_security_active(&self) -> bool {
        self.capture_delivery_gate
            .load(std::sync::atomic::Ordering::Acquire)
    }

    fn command(&self, conn_id: u64, cmd: aegis_ipc::Command) {
        // Best-effort: a send fails only if the main loop has dropped the
        // receiver (compositor shutting down); the command is then lost,
        // which is the right outcome.
        let _ = self.commands.lock().unwrap().send(IpcCommandRequest {
            origin: aegis_ipc::Origin::Ipc { conn_id },
            command: cmd,
        });
    }

    fn system_action(&self, conn_id: u64, action: aegis_ipc::SystemAction) -> Result<(), String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.system_controls
            .lock()
            .unwrap()
            .send(SystemControlRequest {
                origin: aegis_ipc::Origin::Ipc { conn_id },
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

    fn lockdown(&self) -> bool {
        self.lockdown
    }

    fn pair_agent(
        &self,
        conn_id: u64,
        label: Option<&str>,
        requested: &[aegis_ipc::OpClass],
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
                let ops: Vec<aegis_ipc::OpClass> = keys
                    .iter()
                    .filter_map(|key| aegis_ipc::OpClass::from_name(key))
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

    fn grant_for(&self, principal: &str, op: aegis_ipc::OpClass) -> Option<bool> {
        self.grants.read().unwrap().decision_for(principal, op)
    }

    fn request_grant(
        &self,
        conn_id: u64,
        principal: &str,
        op: aegis_ipc::OpClass,
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

    fn authorize_realm_action_granted(
        &self,
        scope: &aegis_ipc::Scope,
        action: &aegis_ipc::RealmAction,
    ) -> Result<(), String> {
        let snapshot = self.realms.read().unwrap();
        authorize_realm_action_granted_against_snapshot(scope, action, &snapshot)
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
        self.auth_event(None, principal, aegis_ipc::AgentAuthAction::Forgotten);
        Ok(())
    }

    fn set_agent_ceiling(
        &self,
        principal: &str,
        pregranted: &[aegis_ipc::OpClass],
        gated: &[aegis_ipc::OpClass],
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
        pregranted: &[aegis_ipc::OpClass],
        gated: &[aegis_ipc::OpClass],
    ) -> Result<(String, String), String> {
        let (principal, credential) = self.agent_auth.write().unwrap().register(
            label,
            pregranted.to_vec(),
            gated.to_vec(),
        )?;
        self.auth_event(None, &principal, aegis_ipc::AgentAuthAction::Paired);
        Ok((principal, credential))
    }

    fn revoke_agent_grant(&self, principal: &str, op: aegis_ipc::OpClass) -> Result<(), String> {
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

    fn realm_action(
        &self,
        conn_id: u64,
        subject: Option<&str>,
        action: aegis_ipc::RealmAction,
    ) -> Result<aegis_ipc::RealmActionResult, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.realm_controls
            .lock()
            .unwrap()
            .send(RealmControlRequest {
                origin: aegis_ipc::Origin::Ipc { conn_id },
                subject: subject.map(str::to_owned),
                action,
                reply: reply_tx,
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        reply_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "realm operation timed out".to_owned())?
    }

    fn settings_action(
        &self,
        conn_id: u64,
        expected_revision: Option<u64>,
        action: aegis_ipc::SettingsAction,
    ) -> Result<aegis_ipc::SettingsReceipt, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.settings_controls
            .lock()
            .unwrap()
            .send(SettingsControlRequest {
                origin: aegis_ipc::Origin::Ipc { conn_id },
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

    fn capture_realm(
        &self,
        realm: aegis_core::realm::RealmId,
        region: Option<aegis_core::Rect>,
    ) -> Result<aegis_ipc::CaptureRealmPayload, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.realm_capture
            .lock()
            .unwrap()
            .send(RealmCaptureRequest {
                realm,
                reply: reply_tx,
                region,
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        reply_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "realm capture timed out".to_owned())?
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
        // The reply parks until the user confirms or cancels, exactly like
        // the file pick; the timeout bounds an abandoned picker and cancels
        // the chrome so the panel never lingers for a dead requester.
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

    fn pick_file(
        &self,
        conn_id: u64,
        options: aegis_ipc::FilePickOptions,
    ) -> Result<aegis_ipc::FilePickResult, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.file_pick_controls
            .lock()
            .unwrap()
            .send(FilePickControlRequest {
                conn_id,
                action: FilePickControl::Start {
                    options,
                    reply: reply_tx,
                },
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        // The reply parks until the user confirms or cancels, exactly like
        // the target pick above; the timeout bounds an abandoned picker and
        // cancels the chrome so the panel never lingers for a dead
        // requester.
        match reply_rx.recv_timeout(PICK_TIMEOUT) {
            Ok(result) => result,
            Err(_) => {
                let _ = self
                    .file_pick_controls
                    .lock()
                    .unwrap()
                    .send(FilePickControlRequest {
                        conn_id,
                        action: FilePickControl::Cancel,
                    });
                Err("file pick timed out".to_owned())
            }
        }
    }
}
