use super::*;

/// Shared live window snapshot for the IPC (ADR-0027). The main loop writes
/// the same `Vec<Window>` it hands the shell; connection threads read it.
/// `query`-capability commands never mutate, so the lock is an `RwLock` and
/// reads from several connections do not block each other. `control`/
/// `session` commands arrive through [`ass_ipc::Handler::command`] and are forwarded
/// to the main loop via the channel the binary owns — the Wayland server
/// state is not `Send`, so connection threads must not touch it directly.
pub(super) struct LiveChannels {
    pub(super) commands: std::sync::mpsc::Sender<IpcCommandRequest>,
    pub(super) capture: std::sync::mpsc::Sender<CaptureRequest>,
    pub(super) realm_controls: std::sync::mpsc::Sender<RealmControlRequest>,
    pub(super) realm_capture: std::sync::mpsc::Sender<RealmCaptureRequest>,
    pub(super) journal_refusals: std::sync::mpsc::Sender<JournalRefusalRequest>,
}

pub(super) struct LiveState {
    windows: std::sync::RwLock<Vec<ass_core::window::Window>>,
    workspaces: std::sync::RwLock<ass_core::workspace::WorkspaceSnapshot>,
    outputs: std::sync::RwLock<Vec<ass_core::output::OutputInfo>>,
    realms: std::sync::RwLock<ass_core::realm::RealmSnapshot>,
    notifications: std::sync::Arc<std::sync::Mutex<ass_core::notify::NotificationQueue>>,
    journal: std::sync::Arc<std::sync::Mutex<ass_ipc::Journal>>,
    commands: std::sync::Mutex<std::sync::mpsc::Sender<IpcCommandRequest>>,
    capture: std::sync::Mutex<std::sync::mpsc::Sender<CaptureRequest>>,
    realm_controls: std::sync::Mutex<std::sync::mpsc::Sender<RealmControlRequest>>,
    realm_capture: std::sync::Mutex<std::sync::mpsc::Sender<RealmCaptureRequest>>,
    journal_refusals: std::sync::mpsc::Sender<JournalRefusalRequest>,
    capture_delivery_gate: std::sync::Arc<std::sync::atomic::AtomicBool>,
    scopes: std::sync::RwLock<std::collections::HashMap<String, ass_ipc::Scope>>,
}

impl LiveState {
    pub(super) fn new(
        channels: LiveChannels,
        capture_delivery_gate: std::sync::Arc<std::sync::atomic::AtomicBool>,
        notifications: std::sync::Arc<std::sync::Mutex<ass_core::notify::NotificationQueue>>,
        journal: std::sync::Arc<std::sync::Mutex<ass_ipc::Journal>>,
        scopes: std::collections::HashMap<String, ass_ipc::Scope>,
    ) -> LiveState {
        LiveState {
            windows: std::sync::RwLock::new(Vec::new()),
            workspaces: std::sync::RwLock::new(
                ass_core::workspace::WorkspaceModel::new().snapshot(),
            ),
            outputs: std::sync::RwLock::new(Vec::new()),
            realms: std::sync::RwLock::new(ass_core::realm::RealmModel::new().snapshot()),
            notifications,
            journal,
            commands: std::sync::Mutex::new(channels.commands),
            capture: std::sync::Mutex::new(channels.capture),
            realm_controls: std::sync::Mutex::new(channels.realm_controls),
            realm_capture: std::sync::Mutex::new(channels.realm_capture),
            journal_refusals: channels.journal_refusals,
            capture_delivery_gate,
            scopes: std::sync::RwLock::new(scopes),
        }
    }

    pub(super) fn set_windows(&self, windows: Vec<ass_core::window::Window>) {
        *self.windows.write().unwrap() = windows;
    }

    pub(super) fn set_workspaces(&self, snapshot: ass_core::workspace::WorkspaceSnapshot) {
        *self.workspaces.write().unwrap() = snapshot;
    }

    pub(super) fn set_outputs(&self, outputs: Vec<ass_core::output::OutputInfo>) {
        *self.outputs.write().unwrap() = outputs;
    }

    pub(super) fn set_realms(&self, snapshot: ass_core::realm::RealmSnapshot) {
        *self.realms.write().unwrap() = snapshot;
    }

    pub(super) fn set_scopes(&self, scopes: std::collections::HashMap<String, ass_ipc::Scope>) {
        *self.scopes.write().unwrap() = scopes;
    }
}

impl ass_ipc::Handler for LiveState {
    /// The socket lives in `$XDG_RUNTIME_DIR` (user-only), so every local
    /// client is the user; grant all capabilities. The capability boundary
    /// becomes load-bearing for the M10 agent phase, where a scope narrows it.
    fn policy_caps(&self) -> ass_ipc::Capabilities {
        ass_ipc::Capabilities {
            query: true,
            control: true,
            input: true,
            session: true,
            realm: true,
        }
    }

    fn windows(&self) -> Vec<ass_core::window::Window> {
        self.windows.read().unwrap().clone()
    }

    fn workspaces(&self) -> ass_core::workspace::WorkspaceSnapshot {
        self.workspaces.read().unwrap().clone()
    }

    fn notifications(&self) -> Vec<ass_core::notify::Notification> {
        self.notifications.lock().unwrap().snapshot()
    }

    fn outputs(&self) -> Vec<ass_core::output::OutputInfo> {
        self.outputs.read().unwrap().clone()
    }

    fn journal_since(&self, since: u64) -> ass_ipc::JournalSnapshot {
        self.journal.lock().unwrap().since(since)
    }

    fn realms(&self) -> ass_core::realm::RealmSnapshot {
        self.realms.read().unwrap().clone()
    }

    fn authorize_realm_action(
        &self,
        scope: &ass_ipc::Scope,
        action: &ass_ipc::RealmAction,
    ) -> Result<(), String> {
        let snapshot = self.realms.read().unwrap();
        authorize_realm_action_against_snapshot(scope, action, &snapshot)
    }

    fn audit_refusal(&self, conn_id: u64, mutation: ass_ipc::JournalMutation, reason: String) {
        let _ = self.journal_refusals.send(JournalRefusalRequest {
            origin: ass_ipc::Origin::Ipc { conn_id },
            mutation,
            reason,
        });
    }

    fn capture_security_active(&self) -> bool {
        self.capture_delivery_gate
            .load(std::sync::atomic::Ordering::Acquire)
    }

    fn command(&self, conn_id: u64, cmd: ass_ipc::Command) {
        // Best-effort: a send fails only if the main loop has dropped the
        // receiver (compositor shutting down); the command is then lost,
        // which is the right outcome.
        let _ = self.commands.lock().unwrap().send(IpcCommandRequest {
            origin: ass_ipc::Origin::Ipc { conn_id },
            command: cmd,
        });
    }

    fn resolve_scope(&self, name: &str) -> Option<ass_ipc::Scope> {
        self.scopes.read().unwrap().get(name).cloned()
    }

    fn capture_output(
        &self,
        region: Option<ass_core::Rect>,
    ) -> Result<ass_ipc::CaptureOutputPayload, String> {
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
        action: ass_ipc::RealmAction,
    ) -> Result<ass_ipc::RealmActionResult, String> {
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        self.realm_controls
            .lock()
            .unwrap()
            .send(RealmControlRequest {
                origin: ass_ipc::Origin::Ipc { conn_id },
                action,
                reply: reply_tx,
            })
            .map_err(|_| "compositor is shutting down".to_owned())?;
        reply_rx
            .recv_timeout(std::time::Duration::from_secs(2))
            .map_err(|_| "realm operation timed out".to_owned())?
    }

    fn capture_realm(
        &self,
        realm: ass_core::realm::RealmId,
        region: Option<ass_core::Rect>,
    ) -> Result<ass_ipc::CaptureRealmPayload, String> {
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
}
