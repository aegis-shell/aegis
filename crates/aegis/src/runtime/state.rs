use super::*;

type IconSnapshot = std::collections::BTreeMap<std::path::PathBuf, Option<IconFileStamp>>;
pub(super) struct AppScanRequest {
    pub(super) icon_theme: String,
    pub(super) scale: u32,
}
pub(super) type AppScanResult = (
    String,
    u32,
    Vec<aegis_core::app::Entry>,
    IconSnapshot,
    Vec<DecodedIcon>,
);
pub(super) type WindowEventSignature = Vec<(
    aegis_core::window::WindowId,
    bool,
    bool,
    bool,
    bool,
    bool,
    Option<String>,
)>;

/// Per-gesture state of an in-flight compositor-owned touchpad swipe:
/// finger count, per-axis pixel accumulators, the latched axis, and the
/// bookkeeping of bindings that hold state across one gesture.
#[derive(Default)]
pub(super) struct SwipeState {
    pub(super) fingers: u8,
    pub(super) dx: f32,
    pub(super) dy: f32,
    pub(super) axis: Option<aegis_core::gesture::GestureAxis>,
    /// Whether this swipe opened the window switcher (`WindowCycle`).
    pub(super) switcher: bool,
    /// Whether the command-panel binding already fired (`CommandPanel`);
    /// latches until SwipeEnd so a long swipe cannot oscillate the panel.
    pub(super) panel_fired: bool,
}

/// Owns the mutable composition state used by the compositor event loop.
///
/// Startup builds this value once, after which event-loop phases borrow only
/// the state they need. Keeping ownership here makes those phases independently
/// testable and prevents the composition root from accumulating more locals.
pub(super) struct CompositorRuntime {
    pub(super) notif_queue: std::sync::Arc<std::sync::Mutex<aegis_core::notify::NotificationQueue>>,
    pub(super) config_path: Option<std::path::PathBuf>,
    pub(super) config: Option<aegis_config::Config>,
    // Rust drops fields in declaration order. The canvas and its Vulkan
    // surface must disappear before a nested host tears down the Wayland
    // display that owns the swapchain's wl_buffers. Flux resources retain the
    // device themselves, so keeping the explicit device handle last is safe.
    pub(super) canvas: flux::Canvas,
    pub(super) surface: flux::Surface,
    pub(super) host: Host,
    pub(super) device: flux::Device,
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
    /// Touchpad swipe bindings: the built-in defaults layered with the
    /// configuration's `[[gesture]]` entries, consulted when a swipe begins
    /// (ADR-0080, ADR-0082).
    pub(super) gesture_map: aegis_core::gesture::GestureMap,
    /// State of the in-flight compositor-owned swipe; `None` when no claimed
    /// gesture is running.
    pub(super) swipe: Option<SwipeState>,
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
    /// Whether the previous frame took the direct-scanout fast path. Used only
    /// to log the activation once per scanout session; compositing resets it.
    pub(super) scanout_taken: bool,
    pub(super) keyboard_capture: aegis_core::input::KeyboardCaptureState,
    pub(super) keymap: aegis_core::keybind::Keymap,
    pub(super) system_status: aegis_shell::SystemStatus,
    pub(super) status_rx: std::sync::mpsc::Receiver<aegis_shell::SystemStatus>,
    /// Wakes the status poller for an out-of-cycle refresh after a system
    /// action, so the HUD reconciles optimistic values without the main loop
    /// ever blocking on a probe subprocess.
    pub(super) status_refresh_tx: std::sync::mpsc::Sender<()>,
    /// Handle to the serialized config-write worker. Typed edits share one
    /// path-bound store and reach the TOML file in submission order. Dock
    /// edits are fire-and-forget; settings edits wait for an accurate IPC
    /// receipt.
    pub(super) config_writer: ConfigWriter,
    pub(super) reload: Option<aegis_config::ReloadWatcher>,
    /// Supervised ext-idle-notify policy client for this session.
    pub(super) idle_process: session::IdleProcess,
    pub(super) quit_requested: bool,
    pub(super) ipc_cmd_rx: std::sync::mpsc::Receiver<IpcCommandRequest>,
    pub(super) system_control_rx: std::sync::mpsc::Receiver<SystemControlRequest>,
    pub(super) capture_rx: std::sync::mpsc::Receiver<CaptureRequest>,
    pub(super) realm_control_rx: std::sync::mpsc::Receiver<RealmControlRequest>,
    pub(super) settings_control_rx: std::sync::mpsc::Receiver<SettingsControlRequest>,
    pub(super) wallpaper_control_rx: std::sync::mpsc::Receiver<WallpaperControlRequest>,
    pub(super) realm_capture_rx: std::sync::mpsc::Receiver<RealmCaptureRequest>,
    pub(super) stream_control_rx: std::sync::mpsc::Receiver<StreamControlRequest>,
    pub(super) idle_control_rx: std::sync::mpsc::Receiver<IdleControlRequest>,
    /// Interactive-pick controls from IPC connection threads (ADR-0054), the
    /// pick waiting for user interaction, and the pick kind whose chrome
    /// opens once the freeze holds.
    pub(super) pick_rx: std::sync::mpsc::Receiver<PickControlRequest>,
    pub(super) pending_pick: Option<PendingPick>,
    pub(super) pending_pick_open: Option<aegis_ipc::PickKind>,
    /// File-pick controls from IPC connection threads (the FileChooser
    /// portal's compositor side) and the pick waiting for user interaction.
    /// Unlike the target pick above, the file picker never arms the
    /// screenshot freeze.
    pub(super) file_pick_rx: std::sync::mpsc::Receiver<FilePickControlRequest>,
    pub(super) pending_file_pick: Option<PendingFilePick>,
    pub(super) app_pick_rx: std::sync::mpsc::Receiver<AppPickControlRequest>,
    pub(super) pending_app_pick: Option<PendingAppPick>,
    pub(super) secret_prompt_rx: std::sync::mpsc::Receiver<SecretPromptControlRequest>,
    pub(super) pending_secret_prompt: Option<PendingSecretPrompt>,
    pub(super) confirm_pick_rx: std::sync::mpsc::Receiver<ConfirmPickControlRequest>,
    pub(super) pending_confirm_pick: Option<PendingConfirmPick>,
    pub(super) capability_pick_rx: std::sync::mpsc::Receiver<CapabilityPickControlRequest>,
    pub(super) pending_capability_pick: Option<PendingCapabilityPick>,
    /// IPC connections currently holding a surfaceless idle inhibitor.
    pub(super) ipc_idle_inhibits: IdleInhibits,
    pub(super) journal_refusal_rx: std::sync::mpsc::Receiver<JournalRefusalRequest>,
    pub(super) auth_event_rx: std::sync::mpsc::Receiver<AuthEventRequest>,
    pub(super) journal: std::sync::Arc<std::sync::Mutex<aegis_ipc::Journal>>,
    pub(super) live: std::sync::Arc<LiveState>,
    pub(super) ipc: Option<aegis_ipc::Server>,
    pub(super) last_win_sig: Option<WindowEventSignature>,
    /// Last output-space state announced to IPC subscribers.
    pub(super) last_space_use: Option<aegis_core::window::SpaceUse>,
    /// Content hash of the last fanned-out window snapshot; the full
    /// `Server::windows()` clone only happens when this changes.
    pub(super) last_windows_hash: Option<u64>,
    pub(super) last_ws_sig: Option<u64>,
    pub(super) last_realm_revision: Option<u64>,
    pub(super) last_outputs_revision: Option<u64>,
    // ----- output damage pipeline (see runtime/damage.rs) -----
    /// Per-surface content generations at the last damage assessment; a
    /// mismatch marks that surface's region damaged.
    pub(super) last_surface_gens: std::collections::HashMap<usize, SurfaceDamageBaseline>,
    pub(super) last_notif_revision: Option<u64>,
    /// (overview, window switcher, keyboard capture, screenshot selector) at
    /// the last assessment — modal chrome changes outside signed paths.
    pub(super) last_chrome_mode: Option<(bool, bool, bool, bool)>,
    pub(super) last_session_locked: bool,
    /// (shape, hidden) as of the last presented frame.
    pub(super) last_presented_cursor: Option<(u32, bool)>,
    pub(super) last_presented_cursor_position: Option<(i32, i32)>,
    /// Damage each Flux ring slot has missed since it was last presented.
    /// Partial repaint unions the current frame's damage with this history so
    /// a three-buffer compositor never exposes stale pixels.
    pub(super) composite_slot_damage: Vec<FrameDamage>,
    /// Wall-clock minute of the last presented frame; a rollover forces one
    /// frame so the status-bar clock cannot go stale while idle.
    pub(super) last_present_minute: Option<u64>,
    /// Shell mutations applied outside the signed paths (status poller,
    /// config reload, app rescan, IPC settings/Realm control) since the last
    /// assessment.
    pub(super) chrome_dirty: bool,
    /// Set when the output was resized/recreated; the next frame must render
    /// in full regardless of damage.
    pub(super) force_full_redraw: bool,
    /// Explicit redraw/presentation lifecycle for the host's atomic commit
    /// domain, plus input edges accumulated while a frame is in flight.
    pub(super) presentation: PresentationScheduler,
    pub(super) pending_frame: Option<FrameState>,
    pub(super) settings_revision: u64,
    pub(super) previous_agent_suspended: bool,
    pub(super) automatically_paused_realms: std::collections::BTreeSet<aegis_core::realm::RealmId>,
    pub(super) animating: bool,
    pub(super) chrome_pointer_captured: bool,
    pub(super) synthetic_pointer_active: bool,
    pub(super) last_cursor_shape: u32,
    pub(super) last_cursor_hidden: bool,
    pub(super) next_app_scan: std::time::Instant,
    pub(super) scan_req_tx: std::sync::mpsc::Sender<AppScanRequest>,
    pub(super) scan_result_rx: std::sync::mpsc::Receiver<AppScanResult>,
    pub(super) previous_render_at: std::time::Instant,
}
