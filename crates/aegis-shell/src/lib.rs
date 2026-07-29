//! Compositor chrome for aegis, built on lens.
//!
//! The shell is split into a **core host** and pluggable **chrome components**.
//! The core ([`Shell`]) owns the lens context, the per-frame snapshot of
//! live toplevels, and the interaction sink ([`ChromeEvents`]); it knows
//! nothing about what the chrome looks like. Each piece of chrome — the
//! launcher, the overview — is a [`Chrome`] implementation registered with
//! [`Shell::add`], and renders itself each frame from the shared snapshot
//! and input. Larger components live in their own crates on top of the same
//! contract (the dock in `aegis-dock`, Prism in `aegis-prism`, AI Workspaces
//! in `aegis-ai-workspaces`, and the status bar in `aegis-statusbar`). Adding
//! or removing a chrome surface is a component change, not a core change.
//!
//! Input the compositor captures is fed here as a snapshot before being routed
//! to clients; component-emitted intents are drained by the main loop into
//! server window-management actions.

use std::collections::HashMap;
use std::os::raw::c_void;

use lens::{Frame, Ui};

pub mod chrome;
pub mod i18n;
pub mod modal;
mod popup;
pub mod system;
mod text;
pub use chrome::{
    AgentFeedback, AppMenu, Launcher, Overview, PickerMode, PinAction, ScreenshotSelector, Toast,
    WindowSwitcher,
};
pub use i18n::{Language, Localizer, Message};
pub use modal::ModalApplicationSpec;
pub use popup::{POPUP_GAP, POPUP_MARGIN, place_popup};
pub use system::{
    BatteryStatus, DisplaySettings, DisplayStatus, NetworkState, SystemAction, SystemStatus,
    detect_system_status,
};
pub use text::truncate;

/// Logical height of the top status bar (the `aegis-statusbar` component).
/// Defined here, at the shell seam, so shell-resident chrome that must align
/// with the bar (the notification toast stack) can share the value without
/// depending on the component crate.
pub const HUD_HEIGHT: f32 = 32.0;

use aegis_core::app::{ApplicationTarget, BuiltInApplication, Entry};
use aegis_core::realm::{RealmId, RealmSnapshot, RealmState};
use aegis_core::window::Window;
use aegis_core::workspace::WorkspaceSnapshot;

/// One successfully applied Agent input operation, projected onto trusted
/// compositor chrome for the physical user.
///
/// This is deliberately presentation-only: it grants no authority, carries
/// no key contents, and is never part of an Agent Realm capture.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentActivity {
    /// Monotonic compositor-local ordering token.
    pub sequence: u64,
    /// Realm whose independent logical seat applied the operation.
    pub realm: RealmId,
    /// Human-readable Realm label captured with the operation.
    pub realm_label: String,
    /// Toplevel that received the operation.
    pub window: aegis_core::window::WindowId,
    /// Applied compositor-global pointer position, when applicable.
    pub position: Option<aegis_core::Point>,
    /// Privacy-preserving operation class.
    pub kind: AgentInputKind,
}

/// Visual class of a successfully applied Agent input operation.
///
/// Keyboard feedback intentionally omits the key code so passwords and typed
/// content cannot leak through trusted chrome or screenshots of the desktop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AgentInputKind {
    PointerMove,
    Click { button: u32 },
    Scroll { dx: f32, dy: f32 },
    Keyboard,
}

/// Edge space a chrome component reserves; tiled windows avoid it. Summed
/// across components by [`Shell::reserved`] and subtracted from the tiling
/// work-area so tiles do not render under the dock or panels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Reserved {
    pub top: i32,
    pub bottom: i32,
    pub left: i32,
    pub right: i32,
}

/// Logical output-space rectangle whose already-composited desktop should be
/// sampled and blurred before chrome is drawn over it. Components declare
/// only the area occupied by their glass material; the executable shares one
/// desktop capture across every request.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct BackdropRegion {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

/// One live preview target in the compositor-owned window switcher.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct WindowSwitcherCard {
    pub window: aegis_core::window::WindowId,
    pub geometry: aegis_core::window_switcher::Card,
}

/// Shared switcher presentation prepared once per frame.
///
/// The executable uses these exact targets for live client previews and the
/// shell uses them for borders, labels, hit-testing, and animation. Keeping a
/// single snapshot prevents the chrome and client scene from drifting apart.
#[derive(Debug, Clone, PartialEq)]
pub struct WindowSwitcherPresentation {
    pub panel: aegis_core::Rect,
    pub cards: Vec<WindowSwitcherCard>,
    pub selected: Option<aegis_core::window::WindowId>,
    pub visibility: f32,
}

/// Decoded icon texture handles shared with chrome components.
///
/// The textures are owned by the composition root's icon cache; an `IconSet`
/// is a map of borrowed raw `flux_image` pointers keyed by every lowercase id
/// an application may run as (StartupWMClass, desktop-id stem, icon name).
/// The owner must keep the textures alive until it has pushed a replacement
/// catalog via [`Shell::set_app_catalog`]; components only dereference handles
/// from the most recently pushed set.
#[derive(Clone, Default)]
pub struct IconSet {
    map: HashMap<String, *mut c_void>,
}

impl IconSet {
    /// Wrap a raw handle map (`app_id` → borrowed `flux_image` pointer).
    pub fn from_raw(map: HashMap<String, *mut c_void>) -> IconSet {
        IconSet { map }
    }

    /// The borrowed texture handle filed under `key`, if any.
    pub fn get(&self, key: &str) -> Option<*mut c_void> {
        self.map.get(key).copied()
    }
}

/// One immutable snapshot of the host application catalog pushed to chrome.
#[derive(Clone, Default)]
pub struct AppCatalog {
    /// Every launchable entry: enumerated XDG applications plus
    /// compositor-owned built-ins.
    pub apps: Vec<Entry>,
    /// The user's pinned favorites, resolved against `apps` by the
    /// composition root from the `[dock] pinned` configuration (or
    /// auto-populated when unconfigured).
    pub pinned: Vec<Entry>,
    /// Decoded icons keyed by every id an entry might run as.
    pub icons: IconSet,
}

/// Cursor shapes chrome can request from the nested host. Values deliberately
/// mirror `wp_cursor_shape_device_v1.shape`, so the executable can pass them
/// through without maintaining a second translation table.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
#[repr(u32)]
pub enum CursorShape {
    #[default]
    Default = 1,
    Pointer = 4,
    Crosshair = 3,
    Text = 9,
}

impl Reserved {
    /// Shrink `rect` by these margins, clamped so size never goes negative.
    pub fn inset(self, r: aegis_core::Rect) -> aegis_core::Rect {
        aegis_core::Rect {
            origin: aegis_core::Point {
                x: r.origin.x + self.left,
                y: r.origin.y + self.top,
            },
            size: aegis_core::Size {
                w: (r.size.w - self.left - self.right).max(0),
                h: (r.size.h - self.top - self.bottom).max(0),
            },
        }
    }
}

/// Re-export so callers can construct input snapshots without depending on
/// lens directly.
pub use lens::Input;

/// One ordered window-management action emitted by compositor chrome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAction {
    /// Focus and raise a window; focusing a minimized window restores it.
    Focus(aegis_core::window::WindowId),
    /// Hide a window while keeping its client and buffers alive.
    Minimize(aegis_core::window::WindowId),
    /// Set or clear compositor-managed maximization.
    SetMaximized(aegis_core::window::WindowId, bool),
    /// Ask a client to close one of its toplevels gracefully.
    Close(aegis_core::window::WindowId),
}

/// Trusted Realm-management intent emitted by compositor-owned chrome.
///
/// The shell never mutates compositor authority directly. The main loop
/// translates these values into the same optimistic Realm transactions used
/// by IPC clients, preserving one validation and commit path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RealmIntent {
    Create {
        label: String,
    },
    SetState {
        realm: RealmId,
        state: RealmState,
        expected_revision: u64,
    },
    Revoke {
        realm: RealmId,
        expected_revision: u64,
    },
    TransferWindow {
        window: aegis_core::window::WindowId,
        target: RealmId,
        retain_source_as_observer: bool,
        expected_revision: u64,
    },
}

/// Interaction intents chrome components emit during a frame. The core
/// collects these and the main loop drains them into server window-management
/// actions or, for [`ChromeEvents::spawn`], into `aegis-launch`. Scalar intents
/// keep the latest value; application menus use an ordered queue for
/// multi-window actions.
#[derive(Debug, Default)]
pub struct ChromeEvents {
    /// The chrome requested the session to quit.
    pub quit: bool,
    /// Window id a component asked to focus/activate.
    pub clicked: Option<aegis_core::window::WindowId>,
    /// Ordered window actions emitted by popup menus. A queue allows an
    /// application-level action such as "close all windows" to preserve one
    /// journal entry per affected toplevel.
    pub window_actions: Vec<WindowAction>,
    /// A desktop entry the chrome asked to launch (e.g. the launcher's
    /// clicked row). Drained into `aegis-launch` by the main loop; carrying the
    /// full [`Entry`] keeps `aegis-shell` free of any `aegis-apps` dependency
    /// (ADR-0022).
    pub spawn: Option<Entry>,
    /// A trusted compositor-owned application to present. Built-ins share the
    /// launcher catalog with external apps but never pass through a shell
    /// command or process boundary.
    pub open_builtin: Option<BuiltInApplication>,
    /// A workspace the chrome asked to switch to (the workspace bar's clicked
    /// tile). Drained into `Server::switch_workspace_to` by the main loop
    /// (ADR-0025).
    pub switch_workspace: Option<aegis_core::workspace::WorkspaceId>,
    /// The chrome asked to toggle the launcher this frame (the dock's
    /// Launchpad tile). Drained by the main loop, which calls [`Shell::toggle`]
    /// — the same path as the Super+A hotkey — so the launcher flips open or
    /// closed.
    pub toggle_launcher: bool,
    /// Notification id the toast stack asked to dismiss. Drained through the
    /// same command/journal path as an IPC dismissal.
    pub dismissed_notification: Option<u64>,
    /// Window id the overview asked to focus this frame (a thumbnail click).
    /// Drained through the focus command/journal path; picking also closes
    /// the overview.
    pub overview_pick: Option<aegis_core::window::WindowId>,
    /// Window id clicked in the held-modifier switcher.
    pub window_switcher_pick: Option<aegis_core::window::WindowId>,
    /// The switcher was dismissed by clicking outside its cards.
    pub window_switcher_cancel: bool,
    /// Workspace id the overview's rail asked to switch to. Drained through
    /// the same command/journal path as `SwitchWorkspaceTo`.
    pub overview_switch: Option<aegis_core::workspace::WorkspaceId>,
    /// Region the screenshot selector asked to capture this frame, if any.
    pub screenshot_region: Option<aegis_core::Rect>,
    /// Point the pixel picker was clicked at this frame (ADR-0054), in
    /// compositor logical pixels.
    pub picked_point: Option<aegis_core::Point>,
    /// Window id the window picker was clicked on this frame (ADR-0054).
    pub picked_window: Option<aegis_core::window::WindowId>,
    /// The window-picker user chose the whole output instead of a window:
    /// Enter/Space, or a click on empty desktop (ADR-0054).
    pub pick_output: bool,
    /// The user dismissed an IPC picker session without picking (Escape, or
    /// a confirm with no staged region). The main loop answers the waiting
    /// request with a cancellation.
    pub pick_cancelled: bool,
    /// Ordered host-system mutations requested by compositor-owned UI.
    pub system_actions: Vec<SystemAction>,
    /// Ordered, idempotent pin mutations requested by application menus.
    /// Drained by the main loop, which updates `[dock] pinned` in the config
    /// and refreshes the dock catalog.
    pub dock_pin_actions: Vec<PinAction>,
    /// Ordered Realm lifecycle and authority mutations requested by trusted
    /// shell surfaces.
    pub realm_intents: Vec<RealmIntent>,
}

impl ChromeEvents {
    /// Activate one catalog entry through its declared target.
    pub fn activate_entry(&mut self, entry: Entry) {
        match entry.target {
            ApplicationTarget::External => self.spawn = Some(entry),
            ApplicationTarget::BuiltIn(app) => self.open_builtin = Some(app),
        }
    }
}

/// One piece of compositor chrome.
///
/// A component renders itself for one frame from the shared window and
/// workspace snapshots and the input, drawing through `frame` and pushing
/// any user intents into `out`. The core owns the lens context, the
/// snapshots, and the sink; the component owns only its own appearance and
/// state. Register implementations with [`Shell::add`].
pub trait Chrome {
    /// Draw the component for this frame. Called inside the core's
    /// `Ui::frame` envelope, so `frame` is a live builder. `workspaces` is
    /// the live workspace/output snapshot (ADR-0025); components that don't
    /// care ignore it.
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        windows: &[Window],
        workspaces: &WorkspaceSnapshot,
        i18n: &Localizer,
        out: &mut ChromeEvents,
    );

    /// Whether this component currently captures new key sequences (e.g. an
    /// open launcher). When any registered component returns true, the main
    /// loop routes each new press and its matching release through chrome
    /// instead of the focused client. This routing policy does not change the
    /// Wayland keyboard or text-input focus. Default `false`; override in
    /// components that capture text input.
    fn captures_keyboard(&self) -> bool {
        false
    }

    /// Handle one resolved key event while [`Chrome::captures_keyboard`] is
    /// true. Default no-op; override to consume typed input (the launcher's
    /// search box).
    fn key_char(&mut self, _kc: &aegis_core::input::KeyChar, _out: &mut ChromeEvents) {}

    /// The global application-launcher hotkey fired. Default no-op;
    /// components with an open/closed launcher state override this to flip it.
    fn toggle(&mut self, _out: &mut ChromeEvents) {}

    /// Whether this component owns the application launcher state.
    fn launcher_active(&self) -> bool {
        false
    }

    /// Close the application launcher without activating a result.
    fn close_launcher(&mut self) {}

    /// The global Prism hotkey fired. Only the Prism component overrides this.
    fn toggle_prism(&mut self, _out: &mut ChromeEvents) {}

    /// Whether this component owns an open Prism surface.
    fn prism_active(&self) -> bool {
        false
    }

    /// Close Prism without activating a result.
    fn close_prism(&mut self) {}

    /// Present one compositor-owned application. Only the component backing
    /// the requested identity acts; ordinary chrome ignores it.
    fn open_builtin(&mut self, _app: BuiltInApplication) {}

    /// Receive a normalized host-system snapshot. Components keep their own
    /// presentation copy so the render trait remains focused on frame data.
    fn update_system_status(&mut self, _status: &SystemStatus) {}

    /// Receive the complete Realm authority snapshot. Only trusted
    /// compositor-owned components consume this high-level state.
    fn update_realms(&mut self, _snapshot: &RealmSnapshot) {}

    /// Receive one Agent input operation after the compositor successfully
    /// applied it. Components must treat this as ephemeral presentation data,
    /// not as authorization or an input source.
    fn update_agent_activity(&mut self, _activity: &AgentActivity) {}

    /// Whether this component owns pointer input at the given output-space
    /// position. The main loop uses this before client routing so clicks on
    /// overlays never fall through to a window underneath them.
    fn captures_pointer(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> bool {
        false
    }

    /// Whether this component temporarily owns the chrome presentation layer.
    /// While a modal component is active, the shell skips ordinary components
    /// so visually covered controls cannot still respond to pointer input.
    fn modal_active(&self) -> bool {
        false
    }

    /// Whether this component remains visible and interactive while another
    /// component is modal. Modal components opt in themselves; persistent
    /// surfaces such as the dock may opt in as well.
    fn visible_during_modal(&self) -> bool {
        false
    }

    /// Cursor shape to use while this component owns the pointer at `(x, y)`.
    /// Return `None` when the component only captures input and the cursor
    /// presentation should remain unchanged. The shell asks only after
    /// [`Chrome::captures_pointer`] returned true.
    fn cursor_shape_at(
        &self,
        _x: f32,
        _y: f32,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Option<CursorShape> {
        None
    }

    /// Inform modal chrome about edge space belonging to persistent chrome.
    /// A full-screen launcher still paints the whole output, but lays out its
    /// usable content above a dock that remains visible during the modal.
    fn set_modal_reserved(&mut self, _reserved: Reserved) {}

    /// Toggle the component's overview mode (M9). Default no-op; the
    /// overview component flips its open state. Fanned out by
    /// [`Shell::toggle_overview`], mirroring [`Chrome::toggle`].
    fn toggle_overview(&mut self, _out: &mut ChromeEvents) {}

    /// Whether the component's overview mode is currently open — the main
    /// loop swaps the desktop scene for the overview thumbnail grid.
    /// Default `false`.
    fn overview_active(&self) -> bool {
        false
    }

    /// Begin the held-modifier window switcher. Static components ignore it;
    /// the switcher component keeps its preview strip visible until
    /// [`Chrome::finish_window_switcher`] arrives.
    fn start_window_switcher(&mut self) {}

    /// Advance and cache the switcher's shared live-preview layout.
    ///
    /// Only the switcher component returns a presentation. The shell calls
    /// this before the compositor paints client previews, then the component
    /// reuses the same snapshot during its chrome render.
    fn prepare_window_switcher(
        &mut self,
        _input: &Input,
        _display: aegis_core::Rect,
        _windows: &[Window],
    ) -> Option<WindowSwitcherPresentation> {
        None
    }

    /// Close the held-modifier window switcher when Super is released.
    fn finish_window_switcher(&mut self) {}

    /// Whether the Super+Tab preview strip is currently active.
    fn window_switcher_active(&self) -> bool {
        false
    }

    /// Whether the screenshot region selector is currently active. Default
    /// `false`; the screenshot selector overrides this.
    fn screenshot_active(&self) -> bool {
        false
    }

    /// Open an interactive picker session for a portal IPC request
    /// (ADR-0054). Default no-op; the screenshot selector overrides this to
    /// open in the requested mode. Results arrive through the
    /// `picked_point`/`picked_window`/`pick_output`/`pick_cancelled` events
    /// (and `screenshot_region` for region picks).
    fn start_pick(&mut self, _mode: PickerMode) {}

    /// Force-close an IPC picker session whose requester went away (lock,
    /// timeout, disconnect). Must not interrupt the Print-key flow; default
    /// no-op.
    fn cancel_pick(&mut self) {}

    /// Accessibility reduced-motion policy (ADR-0029). When enabled, every
    /// component transition (springs, fades, slides) resolves to its end
    /// state in at most one frame. The shell fans this out to all components
    /// and to lens itself; default no-op for static components.
    fn set_reduced_motion(&mut self, _reduced: bool) {}

    /// Receive the host application catalog: every launchable entry, the
    /// resolved pinned favorites, and the decoded icon set. Components that
    /// display applications replace their snapshots in place; others ignore
    /// it. Fanned out by [`Shell::set_app_catalog`] and seeded by
    /// [`Shell::add`].
    fn update_app_catalog(&mut self, _catalog: &AppCatalog) {}

    /// Receive the current visible-window snapshot outside the render pass.
    /// Components normally read `windows` directly from [`Chrome::render`],
    /// but presentation policy that also affects reserved edges or backdrop
    /// capture must be available before rendering begins. Fanned out by
    /// [`Shell::set_windows`] and seeded by [`Shell::add`].
    fn update_windows(&mut self, _windows: &[Window]) {}

    /// Edge space this component reserves; tiled windows avoid it (ADR-0024).
    /// Default none; overridden by chrome that should not be covered (the
    /// dock reserves the bottom edge). Summed by [`Shell::reserved`].
    fn reserved(&self) -> Reserved {
        Reserved::default()
    }

    /// Whether this component has a multi-frame animation in flight (a dock
    /// spring still settling, a fade mid-transition, …). When any registered
    /// component returns true the main loop keeps ticking frames instead of
    /// blocking on the host event queue, so the animation can advance even
    /// when the pointer is still. Default `false`; override in components that
    /// run their own easing.
    fn anim_pending(&self) -> bool {
        false
    }

    /// Whether this component would draw visible pixels in the next frame.
    /// The default is conservative: third-party chrome blocks direct scanout
    /// until it explicitly proves that its inactive state is visually empty.
    fn requires_composition(&self) -> bool {
        true
    }

    /// Blur width requested for the desktop behind compositor
    /// chrome, in logical pixels. The host takes the maximum across
    /// components and applies one shared backdrop capture before rendering
    /// chrome. A zero value disables the capture path.
    fn backdrop_blur_sigma(&self) -> f32 {
        0.0
    }

    /// Regions covered by this component's glass material, in logical output
    /// coordinates. A component requesting blur without a region is treated
    /// as full-screen for compatibility.
    fn backdrop_regions(
        &self,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<BackdropRegion> {
        Vec::new()
    }
}

/// Errors from the shell.
#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    #[error("shell create: {0:?}")]
    Create(#[source] lens::Error),
    #[error("shell render: {0:?}")]
    Render(#[source] lens::Error),
}

/// The core chrome host.
///
/// Owns a lens context bound to the compositor's flux device, the per-frame
/// window snapshot, the interaction sink, and a registry of [`Chrome`]
/// components. The host renders the chrome into the output canvas each frame by
/// running every registered component inside one `Ui::frame` envelope. It has
/// no built-in chrome of its own; the binary composes it from components.
pub struct Shell {
    ui: Ui,
    windows: Vec<Window>,
    workspaces: WorkspaceSnapshot,
    i18n: Localizer,
    system_status: SystemStatus,
    realms: RealmSnapshot,
    /// The most recently pushed host application catalog, seeded into every
    /// registered component (including ones added later) by [`Shell::add`].
    catalog: AppCatalog,
    events: ChromeEvents,
    components: Vec<Box<dyn Chrome>>,
    /// Accessibility reduced-motion policy (ADR-0029), fanned out to every
    /// registered component (including ones added later) and to lens.
    reduced_motion: bool,
    /// While the screenshot freeze holds the screen, only the selector
    /// itself renders; every other component is part of the frozen snapshot
    /// and must not draw (or advance its animations) on top of it.
    screenshot_freeze: bool,
}

impl Shell {
    /// Bind to the compositor's flux device. The host starts with no chrome
    /// registered; add components with [`Shell::add`].
    ///
    /// # Safety
    /// `device` must be a live `flux_device` (from `flux::Device::as_raw`) and
    /// outlive the `Shell`. The pointer crosses from the `flux` bindings' type
    /// to lens's distinct-but-ABI-identical `flux_device`.
    pub unsafe fn new(device: *mut c_void) -> Result<Shell, ShellError> {
        unsafe {
            let ui = Ui::with_device(device as *mut lens::sys::flux_device)
                .map_err(ShellError::Create)?;
            Ok(Shell {
                ui,
                windows: Vec::new(),
                workspaces: WorkspaceSnapshot {
                    outputs: Vec::new(),
                },
                i18n: Localizer::from_env(),
                system_status: SystemStatus::default(),
                realms: aegis_core::realm::RealmModel::new().snapshot(),
                catalog: AppCatalog::default(),
                events: ChromeEvents::default(),
                components: Vec::new(),
                reduced_motion: false,
                screenshot_freeze: false,
            })
        }
    }

    /// Register a chrome component. Components render once per frame, in
    /// registration order.
    pub fn add(&mut self, mut component: Box<dyn Chrome>) {
        component.update_system_status(&self.system_status);
        component.update_realms(&self.realms);
        component.set_reduced_motion(self.reduced_motion);
        component.update_app_catalog(&self.catalog);
        component.update_windows(&self.windows);
        self.components.push(component);
    }

    /// Set the shell-wide reduced-motion policy (ADR-0029): every component
    /// transition and every lens eased value resolves in one frame when on.
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
        self.ui.set_reduced_motion(reduced);
        for component in &mut self.components {
            component.set_reduced_motion(reduced);
        }
    }

    /// Whether the reduced-motion policy is currently enabled.
    pub fn reduced_motion(&self) -> bool {
        self.reduced_motion
    }

    /// Set the device-pixel (HiDPI) scale for the chrome. Layout and input
    /// stay in logical pixels; lens scales the canvas transform on render so
    /// chrome rasterises crisply on a scaled output. The main loop reports the
    /// backend's output scale here each time it changes.
    pub fn set_scale(&mut self, scale: f32) {
        self.ui.set_scale(scale);
    }

    /// Select chrome translations from a POSIX locale or BCP-47 language tag.
    /// Unsupported locales use the English fallback catalog. The shell starts
    /// with the process message locale (`LC_ALL` > `LC_MESSAGES` > `LANG`).
    pub fn set_locale(&mut self, locale: &str) {
        self.i18n = Localizer::new(locale);
    }

    /// Canonical locale tag of the active shell translation catalog.
    pub fn locale(&self) -> &'static str {
        self.i18n.locale()
    }

    /// Whether any component requested the session to quit this frame.
    pub fn should_quit(&self) -> bool {
        self.events.quit
    }

    /// Replace the host's snapshot of live toplevels. Called once per frame
    /// by the main loop with `server.windows()`.
    pub fn set_windows(&mut self, windows: Vec<Window>) {
        self.windows = windows;
        for component in self.components.iter_mut() {
            component.update_windows(&self.windows);
        }
    }

    /// Replace the host's workspace snapshot. Called once per frame by the
    /// main loop with `server.workspace_snapshot()`.
    pub fn set_workspaces(&mut self, workspaces: WorkspaceSnapshot) {
        self.workspaces = workspaces;
    }

    /// Drain the surface id of the window a component asked to focus this
    /// frame, if any.
    pub fn take_clicked_window(&mut self) -> Option<aegis_core::window::WindowId> {
        self.events.clicked.take()
    }

    /// Drain ordered window actions emitted by application context menus.
    pub fn take_window_actions(&mut self) -> Vec<WindowAction> {
        std::mem::take(&mut self.events.window_actions)
    }

    /// Drain the desktop entry the chrome asked to launch this frame, if any.
    /// The main loop hands it to `aegis-launch`.
    pub fn take_spawn(&mut self) -> Option<Entry> {
        self.events.spawn.take()
    }

    /// Drain the compositor-owned application requested this frame.
    pub fn take_open_builtin(&mut self) -> Option<BuiltInApplication> {
        self.events.open_builtin.take()
    }

    /// Present a compositor-owned application through its registered chrome
    /// component.
    pub fn open_builtin(&mut self, app: BuiltInApplication) {
        for component in self.components.iter_mut() {
            component.open_builtin(app);
        }
    }

    /// Replace the normalized system snapshot and notify interested shell
    /// applications and compact status surfaces.
    pub fn set_system_status(&mut self, status: SystemStatus) {
        self.system_status = status;
        for component in self.components.iter_mut() {
            component.update_system_status(&self.system_status);
        }
    }

    /// Replace the Realm authority snapshot and notify the overview and
    /// AI Workspaces before their next frame.
    pub fn set_realms(&mut self, snapshot: RealmSnapshot) {
        self.realms = snapshot;
        for component in self.components.iter_mut() {
            component.update_realms(&self.realms);
        }
    }

    /// Publish one successfully applied Agent input operation to interested
    /// trusted chrome components.
    pub fn report_agent_activity(&mut self, activity: AgentActivity) {
        for component in self.components.iter_mut() {
            component.update_agent_activity(&activity);
        }
    }

    /// Drain ordered system mutations requested by trusted shell UI.
    pub fn take_system_actions(&mut self) -> Vec<SystemAction> {
        std::mem::take(&mut self.events.system_actions)
    }

    /// Drain ordered pin/unpin mutations requested this frame.
    pub fn take_dock_pin_actions(&mut self) -> Vec<PinAction> {
        std::mem::take(&mut self.events.dock_pin_actions)
    }

    /// Drain trusted Realm-management intents in UI order.
    pub fn take_realm_intents(&mut self) -> Vec<RealmIntent> {
        std::mem::take(&mut self.events.realm_intents)
    }

    /// Drain the workspace id the chrome asked to switch to this frame, if
    /// any (the workspace bar's clicked tile). The main loop forwards it to
    /// `Server::switch_workspace_to`.
    pub fn take_switch_workspace(&mut self) -> Option<aegis_core::workspace::WorkspaceId> {
        self.events.switch_workspace.take()
    }

    /// Drain a notification dismissal requested by the toast stack.
    pub fn take_dismissed_notification(&mut self) -> Option<u64> {
        self.events.dismissed_notification.take()
    }

    /// Whether the chrome asked to toggle the launcher this frame (the dock's
    /// Launchpad tile). The main loop calls [`Shell::toggle`] when set.
    pub fn take_toggle_launcher(&mut self) -> bool {
        std::mem::take(&mut self.events.toggle_launcher)
    }

    /// Toggle overview mode on the component that owns it (M9). Mirrors
    /// [`Shell::toggle`]: fanned out to every component; static components
    /// ignore it.
    pub fn toggle_overview(&mut self) {
        let events = &mut self.events;
        for component in self.components.iter_mut() {
            component.toggle_overview(events);
        }
    }

    /// Whether overview mode is currently open — the main loop swaps the
    /// desktop scene for the overview thumbnail grid while this holds.
    pub fn overview_active(&self) -> bool {
        self.components.iter().any(|c| c.overview_active())
    }

    /// Open the compositor-owned Super+Tab preview strip.
    pub fn start_window_switcher(&mut self) {
        for component in self.components.iter_mut() {
            component.start_window_switcher();
        }
    }

    /// Advance the switcher once and return the exact layout shared by the
    /// compositor's live-preview pass and shell chrome.
    pub fn prepare_window_switcher(
        &mut self,
        input: &Input,
        display: aegis_core::Rect,
        windows: &[Window],
    ) -> Option<WindowSwitcherPresentation> {
        self.components
            .iter_mut()
            .find_map(|component| component.prepare_window_switcher(input, display, windows))
    }

    /// Close the preview strip after the held Super modifier is released.
    pub fn finish_window_switcher(&mut self) {
        for component in self.components.iter_mut() {
            component.finish_window_switcher();
        }
    }

    /// Whether the Super+Tab preview strip is active.
    pub fn window_switcher_active(&self) -> bool {
        self.components
            .iter()
            .any(|component| component.window_switcher_active())
    }

    /// Window id the overview asked to focus this frame, if any.
    pub fn take_overview_pick(&mut self) -> Option<aegis_core::window::WindowId> {
        self.events.overview_pick.take()
    }

    /// Window clicked in the held-modifier switcher, if any.
    pub fn take_window_switcher_pick(&mut self) -> Option<aegis_core::window::WindowId> {
        self.events.window_switcher_pick.take()
    }

    /// Whether a click-away dismissed the window switcher this frame.
    pub fn take_window_switcher_cancel(&mut self) -> bool {
        std::mem::take(&mut self.events.window_switcher_cancel)
    }

    /// Workspace id the overview's rail asked to switch to, if any.
    pub fn take_overview_switch(&mut self) -> Option<aegis_core::workspace::WorkspaceId> {
        self.events.overview_switch.take()
    }

    /// Open the screenshot region selector. No-op if no selector component is
    /// registered.
    pub fn start_screenshot(&mut self) {
        for component in self.components.iter_mut() {
            // Only the screenshot selector reacts; other components ignore it.
            component.open_builtin(aegis_core::app::BuiltInApplication::ScreenshotSelector);
        }
    }

    /// Open an interactive picker session for a portal IPC request
    /// (ADR-0054). No-op if no picker component is registered.
    pub fn start_pick(&mut self, mode: PickerMode) {
        for component in self.components.iter_mut() {
            component.start_pick(mode);
        }
    }

    /// Force-close any IPC picker session (requester gone); the Print-key
    /// flow is unaffected.
    pub fn cancel_pick(&mut self) {
        for component in self.components.iter_mut() {
            component.cancel_pick();
        }
    }

    /// Region the screenshot selector asked to capture this frame, if any.
    pub fn take_screenshot_region(&mut self) -> Option<aegis_core::Rect> {
        self.events.screenshot_region.take()
    }

    /// Point the pixel picker was clicked at this frame, if any (ADR-0054).
    pub fn take_picked_point(&mut self) -> Option<aegis_core::Point> {
        self.events.picked_point.take()
    }

    /// Window id the window picker was clicked on this frame, if any
    /// (ADR-0054).
    pub fn take_picked_window(&mut self) -> Option<aegis_core::window::WindowId> {
        self.events.picked_window.take()
    }

    /// Whether the window-picker user chose the whole output this frame.
    pub fn take_pick_output(&mut self) -> bool {
        std::mem::take(&mut self.events.pick_output)
    }

    /// Whether an IPC picker session was dismissed without a pick this frame.
    pub fn take_pick_cancelled(&mut self) -> bool {
        std::mem::take(&mut self.events.pick_cancelled)
    }

    /// Whether the screenshot selector is currently active.
    pub fn screenshot_active(&self) -> bool {
        self.components.iter().any(|c| c.screenshot_active())
    }

    /// Hold or release the screenshot freeze. While held, [`Shell::render`]
    /// draws only the screenshot selector; every other component is part of
    /// the frozen snapshot the compositor presents underneath.
    pub fn set_screenshot_freeze(&mut self, frozen: bool) {
        self.screenshot_freeze = frozen;
    }

    /// Whether any registered component currently captures keyboard input
    /// (e.g. an open launcher). The main loop checks this to decide whether to
    /// route key events to [`Shell::key_char`] or forward them to the focused
    /// client.
    pub fn captures_keyboard(&self) -> bool {
        self.components.iter().any(|c| c.captures_keyboard())
    }

    /// Whether compositor chrome owns pointer input at `(x, y)`. Components
    /// use the same window/workspace snapshot they render, so routing and
    /// visuals agree for the frame.
    pub fn captures_pointer_at(&self, x: f32, y: f32, display: (f32, f32)) -> bool {
        self.components
            .iter()
            .any(|c| c.captures_pointer(x, y, display, &self.windows, &self.workspaces))
    }

    /// Cursor requested by the topmost chrome component at `(x, y)`, or
    /// `None` when the pointer belongs to a client. Components are visited in
    /// reverse registration order because that is their visual stacking order.
    pub fn cursor_shape_at(&self, x: f32, y: f32, display: (f32, f32)) -> Option<CursorShape> {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        self.components
            .iter()
            .rev()
            // Same filter as `render`: while the screenshot freeze holds the
            // screen, frozen components are inert — the dock under the
            // snapshot must not turn the cursor clickable.
            .filter(|component| !self.screenshot_freeze || component.screenshot_active())
            .filter(|component| !modal_active || component.visible_during_modal())
            .find(|component| {
                component.captures_pointer(x, y, display, &self.windows, &self.workspaces)
            })
            .and_then(|component| {
                component.cursor_shape_at(x, y, display, &self.windows, &self.workspaces)
            })
    }

    /// Replace the host application catalog and push it to every registered
    /// component. The shell keeps a copy to seed components added later.
    pub fn set_app_catalog(&mut self, catalog: AppCatalog) {
        self.catalog = catalog;
        for component in self.components.iter_mut() {
            component.update_app_catalog(&self.catalog);
        }
    }

    /// Feed one resolved key event to every registered component. Components
    /// with keyboard-owned state, such as the launcher or an application
    /// context menu, override [`Chrome::key_char`]; others no-op.
    pub fn key_char(&mut self, kc: aegis_core::input::KeyChar) {
        let events = &mut self.events;
        for component in self.components.iter_mut() {
            component.key_char(&kc, events);
        }
    }

    /// Fire the global application-launcher hotkey. Opening the launcher
    /// closes Prism first so the two catalog surfaces cannot capture input at
    /// the same time.
    pub fn toggle(&mut self) {
        let opening = !self
            .components
            .iter()
            .any(|component| component.launcher_active());
        if opening {
            for component in self.components.iter_mut() {
                component.close_prism();
            }
        }
        let events = &mut self.events;
        for component in self.components.iter_mut() {
            component.toggle(events);
        }
    }

    /// Fire the global Prism hotkey. Opening Prism closes the application
    /// launcher first so only one catalog surface owns keyboard input.
    pub fn toggle_prism(&mut self) {
        let opening = !self
            .components
            .iter()
            .any(|component| component.prism_active());
        if opening {
            for component in self.components.iter_mut() {
                component.close_launcher();
            }
        }
        let events = &mut self.events;
        for component in self.components.iter_mut() {
            component.toggle_prism(events);
        }
    }

    /// The union of every component's [`Chrome::reserved`] edges — the space
    /// tiled windows should avoid. Summed per edge.
    pub fn reserved(&self) -> Reserved {
        let mut r = Reserved::default();
        for c in &self.components {
            let c = c.reserved();
            r.top += c.top;
            r.bottom += c.bottom;
            r.left += c.left;
            r.right += c.right;
        }
        r
    }

    /// Whether any registered component has a multi-frame animation in flight.
    /// The main loop consults this to decide whether to keep ticking frames
    /// (advance the animation) or block on the host event queue for the next
    /// wakeup. Also folds in lens's own eased-value state so hover/active
    /// fades on lens widgets (buttons, etc.) settle correctly.
    pub fn anim_pending(&self) -> bool {
        if self
            .components
            .iter()
            // Frozen components do not render, so their animations never
            // advance; excluding them keeps the loop from spinning on an
            // animation that cannot settle until the freeze lifts.
            .filter(|c| !self.screenshot_freeze || c.screenshot_active())
            .any(|c| c.anim_pending())
        {
            return true;
        }
        self.ui.anim_pending()
    }

    /// Whether any component that would participate in the next chrome pass
    /// has visible output. Direct scanout is safe only when this is false.
    pub fn requires_composition(&self) -> bool {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        self.components
            .iter()
            .filter(|component| !self.screenshot_freeze || component.screenshot_active())
            .filter(|component| !modal_active || component.visible_during_modal())
            .any(|component| component.requires_composition())
    }

    /// Strongest backdrop blur requested by any registered component, in
    /// logical pixels. The executable converts it to physical pixels before
    /// invoking flux's realtime multi-resolution filter.
    pub fn backdrop_blur_sigma(&self) -> f32 {
        self.components
            .iter()
            .map(|component| component.backdrop_blur_sigma())
            .fold(0.0_f32, f32::max)
    }

    /// Glass regions contributed by components that will render this frame.
    /// Ordinary chrome is excluded while a modal is active, matching the
    /// render path below.
    pub fn backdrop_regions(&self, display: (f32, f32)) -> Vec<BackdropRegion> {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let mut regions = Vec::new();
        for component in &self.components {
            if (!modal_active || component.visible_during_modal())
                && component.backdrop_blur_sigma() > 0.0
            {
                let mut requested =
                    component.backdrop_regions(display, &self.windows, &self.workspaces);
                if requested.is_empty() {
                    requested.push(BackdropRegion {
                        x: 0.0,
                        y: 0.0,
                        w: display.0,
                        h: display.1,
                    });
                }
                regions.extend(requested);
            }
        }
        regions
    }

    /// Run every registered component and render the chrome into `canvas`,
    /// using `input` for interaction.
    ///
    /// # Safety
    /// `canvas` must be a live `flux_canvas` (from `flux::Canvas::as_raw`)
    /// currently inside a `begin`/`end` recording pair on the active frame.
    pub unsafe fn render(&mut self, canvas: *mut c_void, input: &Input) -> Result<(), ShellError> {
        unsafe {
            let windows = &self.windows;
            let workspaces = &self.workspaces;
            let i18n = &self.i18n;
            let events = &mut self.events;
            let components = &mut self.components;
            let freeze = self.screenshot_freeze;
            let modal_reserved = components
                .iter()
                .filter(|component| component.visible_during_modal())
                .fold(Reserved::default(), |mut total, component| {
                    let edge = component.reserved();
                    total.top += edge.top;
                    total.bottom += edge.bottom;
                    total.left += edge.left;
                    total.right += edge.right;
                    total
                });
            for component in components.iter_mut() {
                component.set_modal_reserved(modal_reserved);
            }
            self.ui.frame(input, |f| {
                let modal_active = components.iter().any(|component| component.modal_active());
                for component in components.iter_mut() {
                    // While the screenshot freeze holds the screen, only the
                    // selector draws; everything else is baked into the
                    // frozen snapshot underneath.
                    if freeze && !component.screenshot_active() {
                        continue;
                    }
                    if !modal_active || component.visible_during_modal() {
                        component.render(f, input, windows, workspaces, i18n, events);
                    }
                }
            });
            self.ui
                .render(canvas as *mut lens::sys::flux_canvas)
                .map_err(ShellError::Render)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reserved_inset_shrinks_and_clamps() {
        let r = Reserved {
            top: 10,
            bottom: 76,
            left: 4,
            right: 0,
        };
        let out = r.inset(aegis_core::Rect::new(0, 0, 1000, 800));
        assert_eq!(out.origin, aegis_core::Point { x: 4, y: 10 });
        assert_eq!(out.size, aegis_core::Size { w: 996, h: 714 }); // 800-10-76
    }

    #[test]
    fn reserved_inset_clamps_to_non_negative() {
        let r = Reserved {
            top: 0,
            bottom: 2000,
            left: 0,
            right: 0,
        };
        let out = r.inset(aegis_core::Rect::new(0, 0, 100, 100));
        assert_eq!(out.size, aegis_core::Size { w: 100, h: 0 });
    }
}
