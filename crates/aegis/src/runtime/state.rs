use super::*;

type IconSnapshot = std::collections::BTreeMap<std::path::PathBuf, Option<IconFileStamp>>;
type AppScanResult = (String, Vec<aegis_core::app::Entry>, IconSnapshot);

/// Owns the mutable composition state used by the compositor event loop.
///
/// Startup builds this value once, after which event-loop phases borrow only
/// the state they need. Keeping ownership here makes those phases independently
/// testable and prevents the composition root from accumulating more locals.
pub(super) struct CompositorRuntime {
    pub(super) notif_queue: std::sync::Arc<std::sync::Mutex<aegis_core::notify::NotificationQueue>>,
    pub(super) config_path: Option<std::path::PathBuf>,
    pub(super) config: Option<aegis_config::Config>,
    pub(super) device: flux::Device,
    pub(super) host: Host,
    pub(super) surface: flux::Surface,
    pub(super) canvas: flux::Canvas,
    pub(super) launcher_backdrop: LauncherBackdrop,
    pub(super) screenshot_freeze: ScreenshotFreeze,
    pub(super) pending_capture: Option<PendingCapture>,
    pub(super) capture_worker: CaptureWorker,
    /// Live output frame streams (ADR-0052) and the in-flight flag bounding
    /// the main-loop → worker stream lane to one frame.
    pub(super) streams: OutputStreams,
    pub(super) stream_job_in_flight: bool,
    pub(super) cursor_cache: cursor::CursorCache,
    pub(super) screenshot_dir: std::path::PathBuf,
    pub(super) server: aegis_compositor::Server,
    pub(super) icon_theme: String,
    pub(super) icon_scale: u32,
    pub(super) launcher_apps: Vec<aegis_core::app::Entry>,
    pub(super) icon_cache: IconCache,
    pub(super) icon_snapshot: IconSnapshot,
    pub(super) shell: aegis_shell::Shell,
    pub(super) input_acc: InputAccumulator,
    pub(super) renderer: aegis_render::Renderer,
    pub(super) realm_processes: RealmProcesses,
    pub(super) realm_render_targets:
        std::collections::BTreeMap<aegis_core::realm::RealmId, RealmRenderTarget>,
    pub(super) pending_realm_capture: Option<PendingRealmCapture>,
    pub(super) realm_damage_sequence: u64,
    pub(super) agent_activity_sequence: u64,
    pub(super) start: std::time::Instant,
    pub(super) wallpaper: Option<aegis_wallpaper::Wallpaper>,
    pub(super) clear: u32,
    pub(super) frame_count: u64,
    pub(super) retired_defer: Option<u64>,
    pub(super) super_tap: aegis_core::input::TapDetector,
    pub(super) prev_captured: bool,
    pub(super) keymap: aegis_core::keybind::Keymap,
    pub(super) system_status: aegis_shell::SystemStatus,
    pub(super) status_rx: std::sync::mpsc::Receiver<aegis_shell::SystemStatus>,
    pub(super) reload: Option<aegis_config::ReloadWatcher>,
    pub(super) quit_requested: bool,
    pub(super) ipc_cmd_rx: std::sync::mpsc::Receiver<IpcCommandRequest>,
    pub(super) capture_rx: std::sync::mpsc::Receiver<CaptureRequest>,
    pub(super) realm_control_rx: std::sync::mpsc::Receiver<RealmControlRequest>,
    pub(super) settings_control_rx: std::sync::mpsc::Receiver<SettingsControlRequest>,
    pub(super) realm_capture_rx: std::sync::mpsc::Receiver<RealmCaptureRequest>,
    pub(super) stream_control_rx: std::sync::mpsc::Receiver<StreamControlRequest>,
    pub(super) idle_control_rx: std::sync::mpsc::Receiver<IdleControlRequest>,
    /// Interactive-pick controls from IPC connection threads (ADR-0054), the
    /// pick waiting for user interaction, and the pick kind whose chrome
    /// opens once the freeze holds.
    pub(super) pick_rx: std::sync::mpsc::Receiver<PickControlRequest>,
    pub(super) pending_pick: Option<PendingPick>,
    pub(super) pending_pick_open: Option<aegis_ipc::PickKind>,
    /// IPC connections currently holding a portal idle inhibitor (ADR-0053).
    pub(super) ipc_idle_inhibits: IdleInhibits,
    pub(super) journal_refusal_rx: std::sync::mpsc::Receiver<JournalRefusalRequest>,
    pub(super) journal: std::sync::Arc<std::sync::Mutex<aegis_ipc::Journal>>,
    pub(super) live: std::sync::Arc<LiveState>,
    pub(super) ipc: Option<aegis_ipc::Server>,
    pub(super) last_win_sig: Option<Vec<(aegis_core::window::WindowId, bool, Option<String>)>>,
    pub(super) last_ws_snap: Option<aegis_core::workspace::WorkspaceSnapshot>,
    pub(super) last_realm_revision: Option<u64>,
    pub(super) settings_revision: u64,
    pub(super) previous_agent_suspended: bool,
    pub(super) automatically_paused_realms: std::collections::BTreeSet<aegis_core::realm::RealmId>,
    pub(super) animating: bool,
    pub(super) chrome_pointer_captured: bool,
    pub(super) synthetic_pointer_active: bool,
    pub(super) last_cursor_shape: u32,
    pub(super) last_cursor_hidden: bool,
    pub(super) next_app_scan: std::time::Instant,
    pub(super) scan_req_tx: std::sync::mpsc::Sender<u32>,
    pub(super) scan_result_rx: std::sync::mpsc::Receiver<AppScanResult>,
    pub(super) pending_scan_scale: u32,
    pub(super) previous_frame_at: std::time::Instant,
}
