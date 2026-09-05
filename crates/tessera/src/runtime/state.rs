use super::*;

type IconSnapshot = std::collections::BTreeMap<std::path::PathBuf, Option<IconFileStamp>>;
pub(super) struct AppScanRequest {
    pub(super) icon_theme: String,
    pub(super) scale: u32,
}
pub(super) type AppScanResult = (
    String,
    u32,
    Vec<tessera_model::app::Entry>,
    IconSnapshot,
    Vec<DecodedIcon>,
);
pub(super) type WindowEventSignature = Vec<(
    tessera_model::window::WindowId,
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
    pub(super) axis: Option<tessera_model::gesture::GestureAxis>,
    /// Whether this swipe opened the window switcher (`WindowCycle`).
    pub(super) switcher: bool,
    /// Whether the command-panel binding already fired (`CommandPanel`);
    /// latches until SwipeEnd so a long swipe cannot oscillate the panel.
    pub(super) panel_fired: bool,
    /// Whether the overview binding already fired (`Overview`); latches
    /// until SwipeEnd so a long swipe cannot oscillate the picker.
    pub(super) overview_fired: bool,
}

/// Baselines and carry-over owned by the output-damage pipeline.
///
/// Keeping these values together makes invalidation an explicit subsystem
/// boundary instead of a set of unrelated flags on the composition root.
#[derive(Default)]
pub(super) struct DamageTracking {
    /// Per-surface content generations at the last damage assessment; a
    /// mismatch marks that surface's region damaged.
    pub(super) last_surface_gens: std::collections::HashMap<usize, SurfaceDamageBaseline>,
    /// Scratch double-buffer for [`Self::last_surface_gens`]: `client_damage`
    /// swaps the two in place and clears the now-old one, so the per-frame
    /// generation map is never freshly heap-allocated.
    pub(super) surface_gens_scratch: std::collections::HashMap<usize, SurfaceDamageBaseline>,
    pub(super) last_notif_revision: Option<u64>,
    /// (overview, window switcher, keyboard capture, screenshot selector) at
    /// the last assessment — modal chrome changes outside signed paths.
    pub(super) last_chrome_mode: Option<(bool, bool, bool, bool)>,
    pub(super) last_session_locked: bool,
    /// (shape, hidden) as of the last presented frame.
    pub(super) last_presented_cursor: Option<(u32, bool)>,
    pub(super) last_presented_cursor_position: Option<(i32, i32)>,
    /// Sprite identity committed to the hardware cursor plane at the last
    /// successful present: (hotspot, sprite size). The presentation path
    /// uses these to skip redundant backend cache lookups when the cursor
    /// is fully unchanged between frames. These mirror the KMS plane state
    /// and must be reset whenever the plane is reprogrammed externally
    /// (VT switch, backend reconfigure).
    pub(super) last_presented_cursor_hotspot: Option<(u32, u32)>,
    pub(super) last_presented_cursor_pixels: Option<(u32, u32)>,
    /// Damage each Flux ring slot has missed since it was last presented.
    /// Partial repaint unions the current frame's damage with this history so
    /// a three-buffer compositor never exposes stale pixels.
    pub(super) composite_slot_damage: Vec<FrameDamage>,
    /// Wall-clock minute of the last presented frame; a rollover forces one
    /// frame so the status-bar clock cannot go stale while idle.
    pub(super) last_present_minute: Option<u64>,
    /// Shell mutations applied outside the signed paths (status poller,
    /// config reload, app rescan, IPC settings/Interaction Domain control) since the last
    /// assessment.
    pub(super) chrome_dirty: bool,
    /// Set when the output was resized/recreated; the next frame must render
    /// in full regardless of damage.
    pub(super) force_full_redraw: bool,
}

/// Owns the mutable composition state used by the compositor event loop.
///
/// Startup builds this value once, after which event-loop phases borrow only
/// the state they need. Keeping ownership here makes those phases independently
/// testable and prevents the composition root from accumulating more locals.
pub(super) struct CompositorRuntime {
    pub(super) notif_queue:
        std::sync::Arc<std::sync::Mutex<tessera_model::notify::NotificationQueue>>,
    pub(super) config_path: Option<std::path::PathBuf>,
    pub(super) config: Option<tessera_config::Config>,
    // Rust drops fields in declaration order. The canvas and its Vulkan
    // surface must disappear before a nested host tears down the Wayland
    // display that owns the swapchain's wl_buffers. Flux resources retain the
    // device themselves, so keeping the explicit device handle last is safe.
    pub(super) canvas: flux::Canvas,
    pub(super) surface: flux::Surface,
    pub(super) host: Host,
    pub(super) device: flux::Device,
    pub(super) backdrop_graph: BackdropGraphExecutor,
    /// Compositor-side blurred window shadows (ADR-0139): owns the Optics
    /// shadow filter and per-slot mask targets; renders at the frame's pass
    /// boundary, composites inside the output pass.
    pub(super) window_shadows: WindowShadowRenderer,
    /// Region-level glass backdrop adaptation (smoothing + policy) and the
    /// per-frame-slot record of which region ids were submitted to prism, so
    /// the frame-lagged statistics align with the bodies that produced them.
    pub(super) glass_adaptation: GlassAdaptation,
    pub(super) submitted_glass_ids: Vec<Vec<u64>>,
    pub(super) screenshot_freeze: ScreenshotFreeze,
    pub(super) pending_capture: Option<PendingCapture>,
    pub(super) capture_worker: CaptureWorker,
    /// Live output frame streams (ADR-0052) and the in-flight flag bounding
    /// the main-loop → worker stream lane to one frame.
    pub(super) streams: OutputStreams,
    pub(super) stream_job_in_flight: bool,
    pub(super) cursor_cache: cursor::CursorCache,
    pub(super) screenshot_dir: std::path::PathBuf,
    pub(super) server: tessera_compositor::Server,
    pub(super) icon_theme: String,
    pub(super) icon_scale: u32,
    pub(super) launcher_apps: Vec<tessera_model::app::Entry>,
    pub(super) icon_cache: IconCache,
    pub(super) icon_snapshot: IconSnapshot,
    pub(super) shell: tessera_shell::Shell,
    pub(super) input_acc: InputAccumulator,
    /// Touchpad swipe bindings: the built-in defaults layered with the
    /// configuration's `[[gesture]]` entries, consulted when a swipe begins
    /// (ADR-0080, ADR-0082).
    pub(super) gesture_map: tessera_model::gesture::GestureMap,
    /// State of the in-flight compositor-owned swipe; `None` when no claimed
    /// gesture is running.
    pub(super) swipe: Option<SwipeState>,
    pub(super) renderer: tessera_render::Renderer,
    pub(super) interaction_domain_processes: InteractionDomainProcesses,
    pub(super) interaction_domain_render_targets: std::collections::BTreeMap<
        tessera_model::interaction_domain::InteractionDomainId,
        InteractionDomainRenderTarget,
    >,
    pub(super) pending_interaction_domain_capture: Option<PendingInteractionDomainCapture>,
    pub(super) pending_window_capture: Option<PendingWindowCapture>,
    pub(super) interaction_domain_damage_sequence: u64,
    pub(super) agent_activity_sequence: u64,
    pub(super) start: std::time::Instant,
    pub(super) wallpaper: Option<tessera_wallpaper::Wallpaper>,
    pub(super) clear: u32,
    pub(super) frame_count: u64,
    pub(super) retired_defer: Option<u64>,
    /// Content owner of the primary plane after the last successful present.
    /// Failed submissions never advance this state.
    pub(super) primary_plane_state: PrimaryPlaneState,
    /// Bounded primary-plane rejection counters and rate-limited diagnostics.
    /// Kept independently from `primary_plane_state` so a persistent video client
    /// explains why it is composited instead of failing silently every frame.
    pub(super) scanout_telemetry: ScanoutTelemetry,
    pub(super) keyboard_capture: tessera_model::input::KeyboardCaptureState,
    pub(super) keymap: tessera_model::keybind::Keymap,
    pub(super) system_status: tessera_shell::SystemStatus,
    pub(super) status_rx: std::sync::mpsc::Receiver<tessera_shell::SystemStatus>,
    /// Wakes the status poller for an out-of-cycle refresh after a system
    /// action, so the HUD reconciles optimistic values without the main loop
    /// ever blocking on a probe subprocess.
    pub(super) status_refresh_tx: std::sync::mpsc::Sender<()>,
    /// Handle to the serialized config-write worker. Typed edits share one
    /// path-bound store and reach the TOML file in submission order. Dock
    /// edits are fire-and-forget; settings edits wait for an accurate IPC
    /// receipt.
    pub(super) config_writer: ConfigWriter,
    pub(super) dock_state: tessera_compositor::DockStateStore,
    pub(super) dock_state_path: std::path::PathBuf,
    pub(super) reload: Option<tessera_config::ReloadWatcher>,
    /// Supervised ext-idle-notify policy client for this session.
    pub(super) idle_process: session::IdleProcess,
    /// Supervised out-of-process accessibility semantic adapter.
    pub(super) semantic_adapter_process: session::SemanticAdapterProcess,
    pub(super) quit_requested: bool,
    pub(super) ipc_cmd_rx: std::sync::mpsc::Receiver<IpcCommandRequest>,
    pub(super) transact_rx: std::sync::mpsc::Receiver<TransactRequest>,
    pub(super) system_control_rx: std::sync::mpsc::Receiver<SystemControlRequest>,
    pub(super) capture_rx: std::sync::mpsc::Receiver<CaptureRequest>,
    pub(super) interaction_domain_control_rx:
        std::sync::mpsc::Receiver<InteractionDomainControlRequest>,
    pub(super) settings_control_rx: std::sync::mpsc::Receiver<SettingsControlRequest>,
    pub(super) wallpaper_control_rx: std::sync::mpsc::Receiver<WallpaperControlRequest>,
    pub(super) interaction_domain_capture_rx:
        std::sync::mpsc::Receiver<InteractionDomainCaptureRequest>,
    pub(super) window_capture_rx: std::sync::mpsc::Receiver<WindowCaptureRequest>,
    pub(super) interaction_domain_observe_rx:
        std::sync::mpsc::Receiver<InteractionDomainObserveRequest>,
    pub(super) actor_action_rx: std::sync::mpsc::Receiver<InteractionDomainActorActionRequest>,
    pub(super) semantic_tree_update_rx: std::sync::mpsc::Receiver<SemanticTreeUpdateRequest>,
    pub(super) semantic_provider_revocation_rx:
        std::sync::mpsc::Receiver<tessera_semantic::SemanticProviderId>,
    pub(super) pending_semantic_actions: Vec<PendingSemanticActorAction>,
    pub(super) observation_discard_rx: std::sync::mpsc::Receiver<ObservationDiscardRequest>,
    pub(super) actor_disconnect_rx: std::sync::mpsc::Receiver<u64>,
    pub(super) observations: ObservationRegistry,
    pub(super) stream_control_rx: std::sync::mpsc::Receiver<StreamControlRequest>,
    pub(super) idle_control_rx: std::sync::mpsc::Receiver<IdleControlRequest>,
    /// Interactive-pick controls from IPC connection threads (ADR-0054), the
    /// pick waiting for user interaction, and the pick kind whose chrome
    /// opens once the freeze holds.
    pub(super) pick_rx: std::sync::mpsc::Receiver<PickControlRequest>,
    pub(super) pending_pick: Option<PendingPick>,
    pub(super) pending_pick_open: Option<tessera_ipc::PickKind>,
    pub(super) app_pick_rx: std::sync::mpsc::Receiver<AppPickControlRequest>,
    pub(super) pending_app_pick: Option<PendingAppPick>,
    pub(super) secret_prompt_rx: std::sync::mpsc::Receiver<SecretPromptControlRequest>,
    pub(super) pending_secret_prompt: Option<PendingSecretPrompt>,
    pub(super) confirm_pick_rx: std::sync::mpsc::Receiver<ConfirmPickControlRequest>,
    pub(super) pending_confirm_pick: Option<PendingConfirmPick>,
    /// Destructive session actions (power off / reboot / suspend) requested
    /// by chrome during a frame, waiting to open the system-level
    /// confirmation dialog. Collected inside the render pass (where the
    /// frame's borrow forbids the `&mut self` consent call) and drained by
    /// the iteration loop right after.
    pub(super) system_confirm_requests: Vec<tessera_model::system::SystemAction>,
    /// A destructive session action (power off / reboot / suspend) waiting
    /// behind the system-level confirmation dialog. Chrome surfaces only
    /// ever *request* these transitions; the runtime owns the consent
    /// architecture, so the action stays parked here until the user
    /// confirms or cancels.
    pub(super) pending_system_action: Option<tessera_model::system::SystemAction>,
    pub(super) capability_pick_rx: std::sync::mpsc::Receiver<CapabilityPickControlRequest>,
    pub(super) pending_capability_pick: Option<PendingCapabilityPick>,
    /// Once-per-discharge-cycle memory of the low-battery warnings already
    /// shown (`battery.rs`).
    pub(super) battery_latches: tessera_model::system::BatteryWarningLatches,
    /// IPC connections currently holding a surfaceless idle inhibitor.
    pub(super) ipc_idle_inhibits: IdleInhibits,
    pub(super) journal: std::sync::Arc<std::sync::Mutex<tessera_ipc::Journal>>,
    pub(super) live: std::sync::Arc<LiveState>,
    pub(super) ipc: Option<tessera_ipc::Server>,
    pub(super) last_win_sig: Option<WindowEventSignature>,
    /// Last output-space state announced to IPC subscribers.
    pub(super) last_space_use: Option<tessera_model::window::SpaceUse>,
    /// Content hash of the last fanned-out window snapshot; the full
    /// `Server::windows()` clone only happens when this changes.
    pub(super) last_windows_hash: Option<u64>,
    /// Same gate for the workspace-global `Server::all_windows()` snapshot
    /// pushed to the dock.
    pub(super) last_all_windows_hash: Option<u64>,
    pub(super) last_ws_sig: Option<u64>,
    pub(super) last_interaction_domain_revision: Option<u64>,
    pub(super) last_outputs_revision: Option<u64>,
    /// Cached `atomic_domain_interval` keyed by `outputs_revision`.
    /// `presentation_interval` sits on the per-frame pacing path (render
    /// follow-up and estimated-vblank wake) and used to clone the full
    /// `Vec<OutputInfo>` twice per frame through `Server::output_infos`;
    /// outputs only change on hotplug/mode switches, so the interval is
    /// recomputed only when the revision moves.
    pub(super) cached_presentation_interval: (u64, std::time::Duration),
    /// Output damage baselines and buffer-age carry-over (runtime/damage.rs).
    pub(super) damage: DamageTracking,
    /// Explicit redraw/presentation lifecycle for the host's atomic commit
    /// domain, plus input edges accumulated while a frame is in flight.
    pub(super) presentation: PresentationScheduler,
    pub(super) pending_frame: Option<FrameState>,
    pub(super) settings_revision: u64,
    pub(super) previous_agent_suspended: bool,
    pub(super) automatically_paused_interaction_domains:
        std::collections::BTreeSet<tessera_model::interaction_domain::InteractionDomainId>,
    pub(super) animating: bool,
    pub(super) chrome_pointer_captured: bool,
    pub(super) synthetic_pointer_active: bool,
    pub(super) last_cursor_shape: u32,
    pub(super) last_cursor_hidden: bool,
    pub(super) next_app_scan: std::time::Instant,
    pub(super) scan_req_tx: std::sync::mpsc::Sender<AppScanRequest>,
    pub(super) scan_result_rx: std::sync::mpsc::Receiver<AppScanResult>,
    pub(super) previous_render_at: std::time::Instant,
    /// Night-light live state and its 1 Hz evaluation cadence
    /// (tessera_model::night_light; applied through the backend's GAMMA_LUT).
    pub(super) night_light: tessera_model::night_light::NightLight,
    pub(super) night_light_last_eval: std::time::Instant,
    /// Last probe of `Host::input_status`. The probe interrogates every
    /// libinput device (several config queries each) and allocates a
    /// `Vec<String>` of device names, so it must not run per frame — the
    /// device set only moves on hotplug, which the same event iteration
    /// observes anyway. Probed at `TOUCHPAD_PROBE_INTERVAL` instead.
    pub(super) input_status_last_probe: std::time::Instant,
}
