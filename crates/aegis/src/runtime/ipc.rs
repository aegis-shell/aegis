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
    pub(super) realm_capture: std::sync::mpsc::Sender<RealmCaptureRequest>,
    pub(super) stream_controls: std::sync::mpsc::Sender<StreamControlRequest>,
    pub(super) idle_controls: std::sync::mpsc::Sender<IdleControlRequest>,
    pub(super) pick_controls: std::sync::mpsc::Sender<PickControlRequest>,
    pub(super) journal_refusals: std::sync::mpsc::Sender<JournalRefusalRequest>,
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
    realm_capture: std::sync::Mutex<std::sync::mpsc::Sender<RealmCaptureRequest>>,
    stream_controls: std::sync::Mutex<std::sync::mpsc::Sender<StreamControlRequest>>,
    idle_controls: std::sync::Mutex<std::sync::mpsc::Sender<IdleControlRequest>>,
    pick_controls: std::sync::Mutex<std::sync::mpsc::Sender<PickControlRequest>>,
    journal_refusals: std::sync::mpsc::Sender<JournalRefusalRequest>,
    capture_delivery_gate: std::sync::Arc<std::sync::atomic::AtomicBool>,
    scopes: std::sync::RwLock<std::collections::HashMap<String, aegis_ipc::Scope>>,
}

impl LiveState {
    pub(super) fn new(
        channels: LiveChannels,
        capture_delivery_gate: std::sync::Arc<std::sync::atomic::AtomicBool>,
        notifications: std::sync::Arc<std::sync::Mutex<aegis_core::notify::NotificationQueue>>,
        journal: std::sync::Arc<std::sync::Mutex<aegis_ipc::Journal>>,
        scopes: std::collections::HashMap<String, aegis_ipc::Scope>,
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
            realm_capture: std::sync::Mutex::new(channels.realm_capture),
            stream_controls: std::sync::Mutex::new(channels.stream_controls),
            idle_controls: std::sync::Mutex::new(channels.idle_controls),
            pick_controls: std::sync::Mutex::new(channels.pick_controls),
            journal_refusals: channels.journal_refusals,
            capture_delivery_gate,
            scopes: std::sync::RwLock::new(scopes),
        }
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

    pub(super) fn set_scopes(&self, scopes: std::collections::HashMap<String, aegis_ipc::Scope>) {
        *self.scopes.write().unwrap() = scopes;
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
        action: aegis_ipc::RealmAction,
    ) -> Result<aegis_ipc::RealmActionResult, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.realm_controls
            .lock()
            .unwrap()
            .send(RealmControlRequest {
                origin: aegis_ipc::Origin::Ipc { conn_id },
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
}
