//! Compositor chrome for aegis, built on lens.
//!
//! The shell is split into a **core host** and pluggable **chrome components**.
//! The core ([`Shell`]) owns the lens context, the per-frame snapshot of
//! live toplevels, and the interaction sink ([`ChromeEvents`]); it knows
//! nothing about what the chrome looks like. Each piece of chrome — the
//! launcher, the overview — is a [`Chrome`] implementation registered with
//! [`Shell::add`], and renders itself each frame from the shared snapshot
//! and input. Larger components live in their own crates on top of the same
//! contract (the dock in `aegis-dock`, Prism in `aegis-prism`, the HUD in
//! `aegis-hud`, and the command panel in `aegis-command-panel`). Adding
//! or removing a chrome surface is a component change, not a core change.
//! [`persona`] owns the lightweight personalized-profile convention; its
//! optional `persona` feature adds shared still/VRM portrait and motion
//! handling without imposing that dependency set on every shell consumer.
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
pub mod persona;
mod popup;
pub mod preview;
pub mod system;
mod text;
pub use chrome::{
    AgentFeedback, AppMenu, AppPickParams, AppPicker, BatteryAlert, BatteryAlertParams,
    CapabilityFamily, CapabilityGroup, CapabilityPickParams, CapabilityPickResult,
    CapabilityPrompt, ConfirmAnswer, ConfirmPickParams, ConfirmPickStyle, ConfirmPrompt,
    ControlledWindowGuard, Launcher, Overview, PickerMode, PinAction, ScreenshotSelector,
    SecretPrompt, SecretPromptParams, Toast, WindowSwitcher,
};
pub use i18n::{Language, Localizer, Message};
pub use modal::ModalApplicationSpec;
pub use popup::{POPUP_GAP, POPUP_MARGIN, PopupSide, place_popup, place_popup_side};
pub use preview::{LivePreviewPresentation, PreviewCard, WindowSwitcherPresentation};
pub use system::{
    BatteryStatus, ChassisKind, DisplaySettings, DisplayStatus, NetworkState, ResourceProbe,
    ResourceStats, SystemAction, SystemStatus, detect_forked_status, detect_system_status,
    detect_system_status_lightweight,
};
pub use text::{ellipsize, truncate};

/// Logical height of the HUD chips (the `aegis-hud` component).
/// Defined here, at the shell seam, so shell-resident chrome that must align
/// with the chips (the notification toast stack) can share the value without
/// depending on the component crate.
pub const HUD_HEIGHT: f32 = 32.0;

use aegis_model::app::{ApplicationTarget, BuiltInApplication, Entry};
use aegis_model::interaction_domain::{InteractionDomainId, InteractionDomainSnapshot};
use aegis_model::window::Window;
use aegis_model::workspace::WorkspaceSnapshot;

/// One successfully applied Agent input operation, projected onto trusted
/// compositor chrome for the physical user.
///
/// This is deliberately presentation-only: it grants no authority, carries
/// no key contents, and is never part of an Agent Interaction Domain capture.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentActivity {
    /// Monotonic compositor-local ordering token.
    pub sequence: u64,
    /// Interaction Domain whose independent logical seat applied the operation.
    pub interaction_domain: InteractionDomainId,
    /// Human-readable Interaction Domain label captured with the operation.
    pub interaction_domain_label: String,
    /// Toplevel that received the operation.
    pub window: aegis_model::window::WindowId,
    /// Applied compositor-global pointer position, when applicable.
    pub position: Option<aegis_model::Point>,
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

impl From<aegis_model::Rect> for BackdropRegion {
    fn from(rect: aegis_model::Rect) -> Self {
        Self {
            x: rect.origin.x as f32,
            y: rect.origin.y as f32,
            w: rect.size.w as f32,
            h: rect.size.h as f32,
        }
    }
}

impl From<lens::Rect> for BackdropRegion {
    fn from(rect: lens::Rect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        }
    }
}

/// One analytic liquid-glass body backed by a [`BackdropRegion`] capture.
/// Radius and opacity participate in the same SDF composite as refraction,
/// blur and edge lighting, so rounded corners and visibility cannot diverge.
/// The drop shadow is cast by the body's own SDF; the component configures
/// it in logical pixels and `shadow_alpha` 0 disables it.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LiquidGlassRegion {
    pub bounds: BackdropRegion,
    pub corner_radius: f32,
    pub opacity: f32,
    pub shadow_alpha: f32,
    pub shadow_blur: f32,
    pub shadow_offset_y: f32,
    /// Optional optical emphasis inside this body. This is one field in the
    /// parent's material, not a nested glass body.
    pub focus: Option<LiquidGlassFocus>,
}

impl LiquidGlassRegion {
    /// Construct a body from one semantic product role.
    #[must_use]
    pub fn from_role(
        design: &aegis_design::Design,
        role: aegis_design::GlassRole,
        bounds: BackdropRegion,
        corner_radius: f32,
        opacity: f32,
    ) -> Self {
        let style = design.glass.for_role(role);
        Self {
            bounds,
            corner_radius,
            opacity: opacity.clamp(0.0, 1.0),
            shadow_alpha: style.shadow_alpha,
            shadow_blur: style.shadow_blur,
            shadow_offset_y: style.shadow_offset_y,
            focus: None,
        }
    }

    /// Attach the parent's single optical interaction focus.
    #[must_use]
    pub fn with_focus(mut self, focus: Option<LiquidGlassFocus>) -> Self {
        self.focus = focus;
        self
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct LiquidGlassFocus {
    pub bounds: BackdropRegion,
    pub corner_radius: f32,
    pub strength: f32,
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
    /// The dock's configured screen edge from `[dock] position`. Carried on
    /// the catalog so the dock learns about live config edits through the
    /// same push that already carries its pinned list; other components
    /// ignore it.
    pub position: aegis_model::dock::DockPosition,
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
    NotAllowed = 15,
}

impl Reserved {
    /// Shrink `rect` by these margins, clamped so size never goes negative.
    pub fn inset(self, r: aegis_model::Rect) -> aegis_model::Rect {
        aegis_model::Rect {
            origin: aegis_model::Point {
                x: r.origin.x + self.left,
                y: r.origin.y + self.top,
            },
            size: aegis_model::Size {
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
    Focus(aegis_model::window::WindowId),
    /// Hide a window while keeping its client and buffers alive.
    Minimize(aegis_model::window::WindowId),
    /// Set or clear compositor-managed maximization.
    SetMaximized(aegis_model::window::WindowId, bool),
    /// Set or clear the compositor-internal always-on-top flag.
    SetAlwaysOnTop(aegis_model::window::WindowId, bool),
    /// Ask a client to close one of its toplevels gracefully.
    Close(aegis_model::window::WindowId),
}

/// Trusted Interaction Domain-management intent emitted by compositor-owned chrome.
///
/// The shell never mutates compositor authority directly. The main loop
/// translates these values into the same optimistic Interaction Domain transactions used
/// by IPC clients, preserving one validation and commit path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InteractionDomainIntent {
    TransferWindow {
        window: aegis_model::window::WindowId,
        target: InteractionDomainId,
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
    /// The chrome asked to lock the session immediately (the command panel's
    /// Lock now row). Drained by the main loop into the same lock path as the
    /// Super+L binding.
    pub lock: bool,
    /// Window id a component asked to focus/activate.
    pub clicked: Option<aegis_model::window::WindowId>,
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
    pub switch_workspace: Option<aegis_model::workspace::WorkspaceId>,
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
    pub overview_pick: Option<aegis_model::window::WindowId>,
    /// Window id clicked in the held-modifier switcher.
    pub window_switcher_pick: Option<aegis_model::window::WindowId>,
    /// The switcher was dismissed by clicking outside its cards.
    pub window_switcher_cancel: bool,
    /// Workspace id the overview's rail asked to switch to. Drained through
    /// the same command/journal path as `SwitchWorkspaceTo`.
    pub overview_switch: Option<aegis_model::workspace::WorkspaceId>,
    /// Region the screenshot selector asked to capture this frame, if any.
    pub screenshot_region: Option<aegis_model::Rect>,
    /// Point the pixel picker was clicked at this frame (ADR-0054), in
    /// compositor logical pixels.
    pub picked_point: Option<aegis_model::Point>,
    /// Window id the window picker was clicked on this frame (ADR-0054).
    pub picked_window: Option<aegis_model::window::WindowId>,
    /// The window-picker user chose the whole output instead of a window:
    /// Enter/Space, or a click on empty desktop (ADR-0054).
    pub pick_output: bool,
    /// The user dismissed an IPC picker session without picking (Escape, or
    /// a confirm with no staged region). The main loop answers the waiting
    /// request with a cancellation.
    pub pick_cancelled: bool,
    /// The desktop file id the app picker confirmed this frame (the
    /// AppChooser portal's compositor side). The main loop answers the
    /// waiting `PickApp` IPC request with it.
    pub app_pick_confirmed: Option<String>,
    /// The user dismissed the app picker without confirming.
    pub app_pick_cancelled: bool,
    /// The secret value the user confirmed at the secret prompt this frame
    /// (the vault password unlock's compositor side). The main loop answers
    /// the waiting `PromptSecret` IPC request with it.
    pub secret_prompt_confirmed: Option<String>,
    /// The user dismissed the secret prompt without confirming.
    pub secret_prompt_cancelled: bool,
    /// The answer the user gave at the confirmation dialog this frame
    /// (portal consent flows, or the ADR-0088 runtime-grant consent). The
    /// main loop answers the waiting `PickConfirm` IPC request or grant
    /// request with it.
    pub confirm_pick_answered: Option<ConfirmAnswer>,
    /// The checklist answer the user gave at the capability-borrowing
    /// dialog this frame (ADR-0088 agent pairing). The main loop answers
    /// the waiting `PairAgent` IPC request with it.
    pub capability_pick_answered: Option<CapabilityPickResult>,
    /// Ordered host-system mutations requested by compositor-owned UI.
    pub system_actions: Vec<SystemAction>,
    /// Persistent-settings mutations requested by compositor-owned UI; the
    /// `Option` is the expected snapshot revision for optimistic concurrency.
    pub settings_actions: Vec<(Option<u64>, aegis_model::settings::SettingsAction)>,
    /// Ordered, idempotent pin mutations requested by application menus.
    /// Drained by the main loop, which updates `[dock] pinned` in the config
    /// and refreshes the dock catalog.
    pub dock_pin_actions: Vec<PinAction>,
    /// The complete pinned order the dock committed this frame when the user
    /// finished dragging a tile to a new slot (entry ids in dock order).
    /// Drained by the main loop into `ConfigEdit::SetDockPinned`, like pin
    /// actions; the dock has already applied the order optimistically, and
    /// the resulting catalog push reconciles it.
    pub dock_reorder: Option<Vec<String>>,
    /// The screen edge the user dragged the dock to this frame. Drained by
    /// the main loop into `ConfigEdit::SetDockPosition`; the dock has
    /// already switched edges optimistically.
    pub dock_position: Option<aegis_model::dock::DockPosition>,
    /// Ordered Interaction Domain lifecycle and authority mutations requested by trusted
    /// shell surfaces.
    pub interaction_domain_intents: Vec<InteractionDomainIntent>,
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

/// Discrete lifecycle request broadcast by the shell host.
///
/// Components match only the variants they own, keeping the base [`Chrome`]
/// trait independent of every built-in application's control surface.
#[derive(Debug)]
pub enum ChromeCommand<'a> {
    ToggleLauncher,
    CloseLauncher,
    TogglePrism,
    ClosePrism,
    OpenBuiltIn(BuiltInApplication),
    ToggleOverview,
    ToggleCommandPanel,
    StartWindowSwitcher,
    FinishWindowSwitcher,
    StartPick(PickerMode),
    CancelPick,
    StartAppPick(&'a AppPickParams),
    CancelAppPick,
    StartSecretPrompt(&'a SecretPromptParams),
    CancelSecretPrompt,
    StartConfirmPick(&'a ConfirmPickParams),
    CancelConfirmPick,
    StartCapabilityPick(&'a CapabilityPickParams),
    CancelCapabilityPick,
    StartBatteryAlert(&'a BatteryAlertParams),
    CancelBatteryAlert,
}

/// Borrowed host snapshot or presentation-policy update.
///
/// Components that retain an update clone only the variant they consume.
#[derive(Clone, Copy)]
pub enum ChromeUpdate<'a> {
    SystemStatus(&'a SystemStatus),
    ResourceStats(&'a ResourceStats),
    InteractionDomains(&'a InteractionDomainSnapshot),
    AgentActivity(&'a AgentActivity),
    AppCatalog(&'a AppCatalog),
    Windows(&'a [Window]),
    /// Every mapped toplevel across all workspaces (the global counterpart of
    /// [`ChromeUpdate::Windows`], which carries only the visible set). Only
    /// components whose strip is workspace-global — the dock — consume this.
    AllWindows(&'a [Window]),
    /// Device-pixel (HiDPI) scale of the output the chrome renders on, so a
    /// component can snap hairline geometry to the device pixel grid.
    Scale(f32),
    /// The shell-wide design snapshot for the resolved desktop color scheme.
    /// Components that paint from design tokens retain a copy instead of
    /// constructing one inline; seeded by [`Shell::add`] and broadcast by
    /// [`Shell::set_color_scheme`].
    Appearance(&'a aegis_design::Design),
    /// The persistent-settings snapshot, seeded by [`Shell::add`] and
    /// broadcast by [`Shell::set_settings`]. Only chrome hosting settings UI
    /// consumes it.
    Settings(&'a aegis_model::settings::SettingsSnapshot),
    ReducedMotion(bool),
    ModalReserved(Reserved),
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
    fn key_char(&mut self, _kc: &aegis_model::input::KeyChar, _out: &mut ChromeEvents) {}

    /// Receive a discrete host lifecycle command.
    fn command(&mut self, _command: &ChromeCommand<'_>, _out: &mut ChromeEvents) {}

    /// Receive a host-owned snapshot or presentation-policy update.
    fn update(&mut self, _update: ChromeUpdate<'_>) {}

    /// Whether this component owns the application launcher state.
    fn launcher_active(&self) -> bool {
        false
    }

    /// Whether this component owns an open Prism surface.
    fn prism_active(&self) -> bool {
        false
    }

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

    /// Whether this component temporarily owns the chrome presentation band.
    /// While a modal component is active, the shell skips ordinary components
    /// so visually covered controls cannot still respond to pointer input.
    fn modal_active(&self) -> bool {
        false
    }

    /// Whether this component temporarily owns the complete chrome band.
    ///
    /// Full-output presentations such as the window switcher and command
    /// panel suppress every other component, including persistent HUD and
    /// Dock decorations, until their exit animation has finished.
    fn exclusive_presentation_active(&self) -> bool {
        self.window_switcher_active()
    }

    /// Whether this component is part of the persistent desktop decoration
    /// layer. Persistent decorations stay present above ordinary overlays such
    /// as Prism and the launcher. An exclusive presentation such as the window
    /// switcher may still suppress the whole decoration layer temporarily.
    fn persistent_decoration(&self) -> bool {
        false
    }

    /// Resting tile-icon rectangles for every running window, in output
    /// coordinates — the compositor's minimize-animation flight targets
    /// (ADR-0029). Only components with a window tile strip (the dock)
    /// contribute; the default is a no-op.
    fn minimize_targets(
        &self,
        _display: (f32, f32),
        _out: &mut Vec<(aegis_model::window::WindowId, aegis_model::Rect)>,
    ) {
    }

    /// Whether this component remains visible and interactive while another
    /// component is modal. Modal components opt in themselves; persistent
    /// decorations share the default opt-in through
    /// [`Chrome::persistent_decoration`].
    fn visible_during_modal(&self) -> bool {
        self.persistent_decoration()
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

    /// Whether the component's command panel is currently open.
    /// Default `false`.
    fn command_panel_active(&self) -> bool {
        false
    }

    /// Whether the component's overview mode is currently open — the main
    /// loop swaps the desktop scene for the overview thumbnail grid.
    /// Default `false`.
    fn overview_active(&self) -> bool {
        false
    }

    /// The component's overview reveal progress (0 = hidden, 1 = fully
    /// open). The compositor feeds it to the thumbnail pass so windows fly
    /// in from their real positions instead of popping onto the grid.
    /// Default `0`.
    fn overview_progress(&self) -> f32 {
        0.0
    }

    /// Advance and cache the switcher's shared live-preview layout.
    ///
    /// Only the switcher component returns a presentation. The shell calls
    /// this before the compositor paints client previews, then the component
    /// reuses the same snapshot during its chrome render.
    fn prepare_window_switcher(
        &mut self,
        _input: &Input,
        _display: aegis_model::Rect,
        _windows: &[Window],
        _order: &[aegis_model::window::WindowId],
        _selected: Option<aegis_model::window::WindowId>,
    ) -> Option<WindowSwitcherPresentation> {
        None
    }

    /// Return a live client-preview popover prepared by this component.
    /// Geometry is read after [`Chrome::prepare_backdrop`] and shared by the
    /// compositor preview pass, analytic glass declarations, shell rendering,
    /// and pointer hit-testing.
    fn live_preview_presentation(&self) -> Option<LivePreviewPresentation> {
        None
    }

    /// Whether the Super+Tab preview strip is currently active.
    fn window_switcher_active(&self) -> bool {
        false
    }

    /// Whether the screenshot region selector is currently active. Default
    /// `false`; the screenshot selector overrides this.
    fn screenshot_active(&self) -> bool {
        false
    }

    /// Whether the app picker is currently open. Default `false`; the
    /// app-picker component overrides this.
    fn app_pick_active(&self) -> bool {
        false
    }

    /// Whether the secret prompt is currently open. Default `false`; the
    /// secret-prompt component overrides this.
    fn secret_prompt_active(&self) -> bool {
        false
    }

    /// Whether the confirmation dialog is currently open. Default `false`;
    /// the confirmation component overrides this.
    fn confirm_pick_active(&self) -> bool {
        false
    }

    /// Whether the capability checklist is currently open. Default `false`;
    /// the capability-prompt component overrides this.
    fn capability_pick_active(&self) -> bool {
        false
    }

    /// Whether the low-battery alert is currently open. Default `false`; the
    /// battery-alert component overrides this.
    fn battery_alert_active(&self) -> bool {
        false
    }

    /// Prepare geometry/visibility that the backdrop capture must consume in
    /// the same frame as [`Chrome::render`]. Components with cursor-driven
    /// glass animations use this prepass so their SDF opacity never trails
    /// foreground content by one frame.
    fn prepare_backdrop(
        &mut self,
        _input: &Input,
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) {
    }

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

    /// Subset of [`Chrome::backdrop_regions`] that should use the analytic
    /// thick-glass compositor instead of a rectangular frosted-blur clip.
    fn liquid_glass_regions(
        &self,
        _display: (f32, f32),
        _windows: &[Window],
        _workspaces: &WorkspaceSnapshot,
    ) -> Vec<LiquidGlassRegion> {
        Vec::new()
    }
}

/// Exact compositor-owned output requirements for primary-plane policy.
///
/// This deliberately describes what would affect the *next rendered frame*,
/// rather than broad component state such as "the Dock exists" or "an
/// animation timer is running".  A hidden/fully transparent component must
/// not prevent direct scanout, while any visible chrome pixel or live
/// backdrop sample must.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompositionRequirements {
    /// At least one eligible chrome component would draw visible pixels.
    pub visible_pixels: bool,
    /// Visible chrome samples the client scene through a backdrop effect.
    pub live_backdrop_effect: bool,
}

/// Whether one component participates in the current shell pass. Ordinary
/// modal overlays preserve persistent decorations; an exclusive presentation
/// temporarily owns the complete chrome band. A held screenshot freeze
/// outranks every other state: the frozen snapshot already contains the other
/// components (an open command panel included), so only the selector itself
/// may draw and receive input until it closes.
fn participates_in_shell_pass(
    component: &dyn Chrome,
    screenshot_freeze: bool,
    modal_active: bool,
    window_switcher_active: bool,
    exclusive_presentation_active: bool,
) -> bool {
    if screenshot_freeze {
        return component.screenshot_active();
    }
    (!window_switcher_active || component.window_switcher_active())
        && (!exclusive_presentation_active || component.exclusive_presentation_active())
        && (!modal_active || component.visible_during_modal())
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
    /// Every mapped toplevel across all workspaces; pushed by the host
    /// alongside [`Self::windows`] and fanned out as
    /// [`ChromeUpdate::AllWindows`] for workspace-global components (the dock).
    all_windows: Vec<Window>,
    workspaces: WorkspaceSnapshot,
    i18n: Localizer,
    system_status: SystemStatus,
    /// The most recently published persistent-settings snapshot, seeded into
    /// every registered component (including ones added later) by
    /// [`Shell::add`] and fanned out as [`ChromeUpdate::Settings`].
    settings: Option<aegis_model::settings::SettingsSnapshot>,
    resource_stats: ResourceStats,
    interaction_domains: InteractionDomainSnapshot,
    /// The most recently pushed host application catalog, seeded into every
    /// registered component (including ones added later) by [`Shell::add`].
    catalog: AppCatalog,
    events: ChromeEvents,
    components: Vec<Box<dyn Chrome>>,
    /// Accessibility reduced-motion policy (ADR-0029), fanned out to every
    /// registered component (including ones added later) and to lens.
    reduced_motion: bool,
    /// Most recently reported device-pixel scale, fanned out to components as
    /// [`ChromeUpdate::Scale`].
    scale: f32,
    /// The resolved appearance every component paints from, seeded by
    /// [`Shell::add`] and fanned out as [`ChromeUpdate::Appearance`]. Starts
    /// dark; the compositor pushes the configured scheme after startup.
    design: aegis_design::Design,
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
                all_windows: Vec::new(),
                workspaces: WorkspaceSnapshot {
                    outputs: Vec::new(),
                },
                i18n: Localizer::from_env(),
                system_status: SystemStatus::default(),
                settings: None,
                resource_stats: ResourceStats::default(),
                interaction_domains: aegis_model::interaction_domain::InteractionDomainModel::new()
                    .snapshot(),
                catalog: AppCatalog::default(),
                events: ChromeEvents::default(),
                components: Vec::new(),
                reduced_motion: false,
                scale: 1.0,
                design: aegis_design::Design::dark(),
                screenshot_freeze: false,
            })
        }
    }

    /// Register a chrome component. Components render once per frame, in
    /// registration order.
    pub fn add(&mut self, mut component: Box<dyn Chrome>) {
        component.update(ChromeUpdate::SystemStatus(&self.system_status));
        component.update(ChromeUpdate::ResourceStats(&self.resource_stats));
        component.update(ChromeUpdate::InteractionDomains(&self.interaction_domains));
        component.update(ChromeUpdate::ReducedMotion(self.reduced_motion));
        component.update(ChromeUpdate::Scale(self.scale));
        component.update(ChromeUpdate::Appearance(&self.design));
        component.update(ChromeUpdate::AppCatalog(&self.catalog));
        component.update(ChromeUpdate::Windows(&self.windows));
        component.update(ChromeUpdate::AllWindows(&self.all_windows));
        if let Some(settings) = &self.settings {
            component.update(ChromeUpdate::Settings(settings));
        }
        self.components.push(component);
    }

    fn broadcast_command(&mut self, command: ChromeCommand<'_>) {
        let events = &mut self.events;
        for component in &mut self.components {
            component.command(&command, events);
        }
    }

    /// Set the shell-wide reduced-motion policy (ADR-0029): every component
    /// transition and every lens eased value resolves in one frame when on.
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.reduced_motion = reduced;
        self.ui.set_reduced_motion(reduced);
        for component in &mut self.components {
            component.update(ChromeUpdate::ReducedMotion(reduced));
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
        if self.scale != scale {
            self.scale = scale;
            for component in self.components.iter_mut() {
                component.update(ChromeUpdate::Scale(scale));
            }
        }
    }

    /// Set the shell-wide desktop color scheme (`[appearance] color_scheme`).
    /// `System` resolves to the dark fallback inside
    /// [`aegis_design::Design::for_scheme`]; every component receives the
    /// resulting design snapshot through [`ChromeUpdate::Appearance`] when it
    /// actually changes.
    pub fn set_color_scheme(&mut self, scheme: aegis_model::settings::ColorScheme) {
        let design = aegis_design::Design::for_scheme(scheme);
        if self.design != design {
            self.design = design;
            for component in self.components.iter_mut() {
                component.update(ChromeUpdate::Appearance(&self.design));
            }
        }
    }

    /// The design snapshot components currently paint from.
    pub fn design(&self) -> &aegis_design::Design {
        &self.design
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

    /// Drain an immediate-lock request from the chrome (the command panel's
    /// Lock now row). The main loop feeds it to `IdleProcess::lock_now`.
    pub fn take_lock(&mut self) -> bool {
        std::mem::take(&mut self.events.lock)
    }

    /// Replace the host's snapshot of live toplevels. Called once per frame
    /// by the main loop with `server.windows()`.
    pub fn set_windows(&mut self, windows: Vec<Window>) {
        self.windows = windows;
        for component in self.components.iter_mut() {
            component.update(ChromeUpdate::Windows(&self.windows));
        }
    }

    /// Replace the host's workspace-global snapshot of live toplevels — every
    /// mapped window across all workspaces, from `server.all_windows()`. Only
    /// components with a workspace-global strip (the dock) consume it; the
    /// overview, window switcher, and other chrome keep the visible-set
    /// [`Self::set_windows`] snapshot.
    pub fn set_all_windows(&mut self, windows: Vec<Window>) {
        self.all_windows = windows;
        for component in self.components.iter_mut() {
            component.update(ChromeUpdate::AllWindows(&self.all_windows));
        }
    }

    /// Replace the host's workspace snapshot. Called once per frame by the
    /// main loop with `server.workspace_snapshot()`.
    pub fn set_workspaces(&mut self, workspaces: WorkspaceSnapshot) {
        self.workspaces = workspaces;
    }

    /// Drain the surface id of the window a component asked to focus this
    /// frame, if any.
    pub fn take_clicked_window(&mut self) -> Option<aegis_model::window::WindowId> {
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
        self.broadcast_command(ChromeCommand::OpenBuiltIn(app));
    }

    /// Replace the normalized system snapshot and notify interested shell
    /// applications and compact status surfaces.
    pub fn set_system_status(&mut self, status: SystemStatus) {
        self.system_status = status;
        for component in self.components.iter_mut() {
            component.update(ChromeUpdate::SystemStatus(&self.system_status));
        }
    }

    /// Replace the persistent-settings snapshot and notify chrome hosting
    /// settings UI.
    pub fn set_settings(&mut self, snapshot: aegis_model::settings::SettingsSnapshot) {
        self.settings = Some(snapshot);
        if let Some(settings) = self.settings.as_ref() {
            for component in self.components.iter_mut() {
                component.update(ChromeUpdate::Settings(settings));
            }
        }
    }

    /// Replace the host resource-utilisation sample and notify interested
    /// status surfaces.
    pub fn set_resource_stats(&mut self, stats: ResourceStats) {
        self.resource_stats = stats;
        for component in self.components.iter_mut() {
            component.update(ChromeUpdate::ResourceStats(&self.resource_stats));
        }
    }

    /// Replace the Interaction Domain authority snapshot and notify the overview and
    /// Agent Workspaces before their next frame.
    pub fn set_interaction_domains(&mut self, snapshot: InteractionDomainSnapshot) {
        self.interaction_domains = snapshot;
        for component in self.components.iter_mut() {
            component.update(ChromeUpdate::InteractionDomains(&self.interaction_domains));
        }
    }

    /// Publish one successfully applied Agent input operation to interested
    /// trusted chrome components.
    pub fn report_agent_activity(&mut self, activity: AgentActivity) {
        for component in self.components.iter_mut() {
            component.update(ChromeUpdate::AgentActivity(&activity));
        }
    }

    /// Drain ordered system mutations requested by trusted shell UI.
    pub fn take_system_actions(&mut self) -> Vec<SystemAction> {
        std::mem::take(&mut self.events.system_actions)
    }

    /// Drain persistent-settings mutations requested by compositor-owned UI.
    pub fn take_settings_actions(
        &mut self,
    ) -> Vec<(Option<u64>, aegis_model::settings::SettingsAction)> {
        std::mem::take(&mut self.events.settings_actions)
    }

    /// Drain ordered pin/unpin mutations requested this frame.
    pub fn take_dock_pin_actions(&mut self) -> Vec<PinAction> {
        std::mem::take(&mut self.events.dock_pin_actions)
    }

    /// Drain the pinned order committed by a dock tile drag this frame, if
    /// any. The main loop persists it through `ConfigEdit::SetDockPinned`.
    pub fn take_dock_reorder(&mut self) -> Option<Vec<String>> {
        self.events.dock_reorder.take()
    }

    /// Drain the screen edge the dock was dragged to this frame, if any. The
    /// main loop persists it through `ConfigEdit::SetDockPosition`.
    pub fn take_dock_position(&mut self) -> Option<aegis_model::dock::DockPosition> {
        self.events.dock_position.take()
    }

    /// Drain trusted Interaction Domain-management intents in UI order.
    pub fn take_interaction_domain_intents(&mut self) -> Vec<InteractionDomainIntent> {
        std::mem::take(&mut self.events.interaction_domain_intents)
    }

    /// Drain the workspace id the chrome asked to switch to this frame, if
    /// any (the workspace bar's clicked tile). The main loop forwards it to
    /// `Server::switch_workspace_to`.
    pub fn take_switch_workspace(&mut self) -> Option<aegis_model::workspace::WorkspaceId> {
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
        self.broadcast_command(ChromeCommand::ToggleOverview);
    }

    /// Whether overview mode is currently open — the main loop swaps the
    /// desktop scene for the overview thumbnail grid while this holds.
    pub fn overview_active(&self) -> bool {
        self.components.iter().any(|c| c.overview_active())
    }

    /// The overview's reveal progress (0 = hidden, 1 = fully open), the
    /// maximum across components. Drives the thumbnail pass fly-in.
    pub fn overview_progress(&self) -> f32 {
        self.components
            .iter()
            .map(|c| c.overview_progress())
            .fold(0.0_f32, f32::max)
    }

    /// Toggle the command panel on the component that owns it
    /// (ADR-0080). Mirrors [`Shell::toggle_overview`]: fanned out to every
    /// component; static components ignore it.
    pub fn toggle_command_panel(&mut self) {
        self.broadcast_command(ChromeCommand::ToggleCommandPanel);
    }

    /// Whether the command panel is currently open.
    pub fn command_panel_active(&self) -> bool {
        self.components.iter().any(|c| c.command_panel_active())
    }

    /// Open the compositor-owned Super+Tab preview strip.
    pub fn start_window_switcher(&mut self) {
        self.broadcast_command(ChromeCommand::StartWindowSwitcher);
    }

    /// Advance the switcher once and return the exact layout shared by the
    /// compositor's live-preview pass and shell chrome.
    pub fn prepare_window_switcher(
        &mut self,
        input: &Input,
        display: aegis_model::Rect,
        windows: &[Window],
        order: &[aegis_model::window::WindowId],
        selected: Option<aegis_model::window::WindowId>,
    ) -> Option<WindowSwitcherPresentation> {
        self.components.iter_mut().find_map(|component| {
            component.prepare_window_switcher(input, display, windows, order, selected)
        })
    }

    /// Collect compositor-rendered live-preview popovers contributed by
    /// ordinary chrome. A vector keeps the contract composable even though the
    /// Dock is currently the only producer.
    pub fn live_preview_presentations(&self) -> Vec<LivePreviewPresentation> {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        self.components
            .iter()
            .filter(|component| {
                participates_in_shell_pass(
                    component.as_ref(),
                    self.screenshot_freeze,
                    modal_active,
                    window_switcher_active,
                    exclusive_presentation_active,
                )
            })
            .filter_map(|component| component.live_preview_presentation())
            .collect()
    }

    /// Close the preview strip after the held Super modifier is released.
    pub fn finish_window_switcher(&mut self) {
        self.broadcast_command(ChromeCommand::FinishWindowSwitcher);
    }

    /// Whether the Super+Tab preview strip is active.
    pub fn window_switcher_active(&self) -> bool {
        self.components
            .iter()
            .any(|component| component.window_switcher_active())
    }

    fn exclusive_presentation_active(&self) -> bool {
        self.components
            .iter()
            .any(|component| component.exclusive_presentation_active())
    }

    /// Window id the overview asked to focus this frame, if any.
    pub fn take_overview_pick(&mut self) -> Option<aegis_model::window::WindowId> {
        self.events.overview_pick.take()
    }

    /// Window clicked in the held-modifier switcher, if any.
    pub fn take_window_switcher_pick(&mut self) -> Option<aegis_model::window::WindowId> {
        self.events.window_switcher_pick.take()
    }

    /// Whether a click-away dismissed the window switcher this frame.
    pub fn take_window_switcher_cancel(&mut self) -> bool {
        std::mem::take(&mut self.events.window_switcher_cancel)
    }

    /// Workspace id the overview's rail asked to switch to, if any.
    pub fn take_overview_switch(&mut self) -> Option<aegis_model::workspace::WorkspaceId> {
        self.events.overview_switch.take()
    }

    /// Open the screenshot region selector. No-op if no selector component is
    /// registered.
    pub fn start_screenshot(&mut self) {
        self.broadcast_command(ChromeCommand::OpenBuiltIn(
            aegis_model::app::BuiltInApplication::ScreenshotSelector,
        ));
    }

    /// Open an interactive picker session for a portal IPC request
    /// (ADR-0054). No-op if no picker component is registered.
    pub fn start_pick(&mut self, mode: PickerMode) {
        self.broadcast_command(ChromeCommand::StartPick(mode));
    }

    /// Force-close any IPC picker session (requester gone); the Print-key
    /// flow is unaffected.
    pub fn cancel_pick(&mut self) {
        self.broadcast_command(ChromeCommand::CancelPick);
    }

    /// Region the screenshot selector asked to capture this frame, if any.
    pub fn take_screenshot_region(&mut self) -> Option<aegis_model::Rect> {
        self.events.screenshot_region.take()
    }

    /// Point the pixel picker was clicked at this frame, if any (ADR-0054).
    pub fn take_picked_point(&mut self) -> Option<aegis_model::Point> {
        self.events.picked_point.take()
    }

    /// Window id the window picker was clicked on this frame, if any
    /// (ADR-0054).
    pub fn take_picked_window(&mut self) -> Option<aegis_model::window::WindowId> {
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

    /// Open the user-consent application picker for a `PickApp` IPC request.
    /// No-op if no app-picker component is registered.
    pub fn start_app_pick(&mut self, params: AppPickParams) {
        self.broadcast_command(ChromeCommand::StartAppPick(&params));
    }

    /// Force-close the app picker (requester gone: lock, timeout,
    /// disconnect).
    pub fn cancel_app_pick(&mut self) {
        self.broadcast_command(ChromeCommand::CancelAppPick);
    }

    /// Whether the app picker is currently open.
    pub fn app_pick_active(&self) -> bool {
        self.components.iter().any(|c| c.app_pick_active())
    }

    /// The desktop file id the app picker confirmed this frame, if any.
    pub fn take_app_pick_confirmed(&mut self) -> Option<String> {
        self.events.app_pick_confirmed.take()
    }

    /// Whether the app picker was dismissed without a pick this frame.
    pub fn take_app_pick_cancelled(&mut self) -> bool {
        std::mem::take(&mut self.events.app_pick_cancelled)
    }

    /// Open the masked secret prompt for a `PromptSecret` IPC request.
    /// No-op if no secret-prompt component is registered.
    pub fn start_secret_prompt(&mut self, params: SecretPromptParams) {
        self.broadcast_command(ChromeCommand::StartSecretPrompt(&params));
    }

    /// Force-close the secret prompt (requester gone: lock, timeout,
    /// disconnect).
    pub fn cancel_secret_prompt(&mut self) {
        self.broadcast_command(ChromeCommand::CancelSecretPrompt);
    }

    /// Whether the secret prompt is currently open.
    pub fn secret_prompt_active(&self) -> bool {
        self.components.iter().any(|c| c.secret_prompt_active())
    }

    /// The secret value the prompt confirmed this frame, if any.
    pub fn take_secret_prompt_confirmed(&mut self) -> Option<String> {
        self.events.secret_prompt_confirmed.take()
    }

    /// Whether the secret prompt was dismissed without a secret this frame.
    pub fn take_secret_prompt_cancelled(&mut self) -> bool {
        std::mem::take(&mut self.events.secret_prompt_cancelled)
    }

    /// Open the yes/no confirmation dialog for a `PickConfirm` IPC request.
    /// No-op if no confirmation component is registered.
    pub fn start_confirm_pick(&mut self, params: ConfirmPickParams) {
        self.broadcast_command(ChromeCommand::StartConfirmPick(&params));
    }

    /// Force-close the confirmation dialog (requester gone: lock, timeout,
    /// disconnect).
    pub fn cancel_confirm_pick(&mut self) {
        self.broadcast_command(ChromeCommand::CancelConfirmPick);
    }

    /// Whether the confirmation dialog is currently open.
    pub fn confirm_pick_active(&self) -> bool {
        self.components.iter().any(|c| c.confirm_pick_active())
    }

    /// The answer the user gave at the confirmation dialog this frame, if
    /// any.
    pub fn take_confirm_pick_answered(&mut self) -> Option<ConfirmAnswer> {
        self.events.confirm_pick_answered.take()
    }

    /// Open the capability-borrowing checklist for a `PairAgent` IPC
    /// request. No-op if no capability-prompt component is registered.
    pub fn start_capability_pick(&mut self, params: CapabilityPickParams) {
        self.broadcast_command(ChromeCommand::StartCapabilityPick(&params));
    }

    /// Force-close the capability checklist (requester gone: lock, timeout,
    /// disconnect).
    pub fn cancel_capability_pick(&mut self) {
        self.broadcast_command(ChromeCommand::CancelCapabilityPick);
    }

    /// Whether the capability checklist is currently open.
    pub fn capability_pick_active(&self) -> bool {
        self.components.iter().any(|c| c.capability_pick_active())
    }

    /// The checklist answer the user gave this frame, if any (`approved`
    /// carries the checked group keys; `None` = denied).
    pub fn take_capability_pick_answered(&mut self) -> Option<CapabilityPickResult> {
        self.events.capability_pick_answered.take()
    }

    /// Open or update the low-battery alert. Compositor-owned: dismissal
    /// produces no answer event. No-op if no battery-alert component is
    /// registered.
    pub fn start_battery_alert(&mut self, params: BatteryAlertParams) {
        self.broadcast_command(ChromeCommand::StartBatteryAlert(&params));
    }

    /// Force-close the low-battery alert.
    pub fn cancel_battery_alert(&mut self) {
        self.broadcast_command(ChromeCommand::CancelBatteryAlert);
    }

    /// Whether the low-battery alert is currently open.
    pub fn battery_alert_active(&self) -> bool {
        self.components.iter().any(|c| c.battery_alert_active())
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
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        self.components.iter().any(|component| {
            participates_in_shell_pass(
                component.as_ref(),
                self.screenshot_freeze,
                modal_active,
                window_switcher_active,
                exclusive_presentation_active,
            ) && component.captures_keyboard()
        })
    }

    /// Whether compositor chrome owns pointer input at `(x, y)`. Components
    /// use the same window/workspace snapshot they render, so routing and
    /// visuals agree for the frame.
    pub fn captures_pointer_at(&self, x: f32, y: f32, display: (f32, f32)) -> bool {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        self.components
            .iter()
            .filter(|component| {
                participates_in_shell_pass(
                    component.as_ref(),
                    self.screenshot_freeze,
                    modal_active,
                    window_switcher_active,
                    exclusive_presentation_active,
                )
            })
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
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        self.components
            .iter()
            .rev()
            .filter(|component| {
                participates_in_shell_pass(
                    component.as_ref(),
                    self.screenshot_freeze,
                    modal_active,
                    window_switcher_active,
                    exclusive_presentation_active,
                )
            })
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
            component.update(ChromeUpdate::AppCatalog(&self.catalog));
        }
    }

    /// Feed one resolved key event to every registered component. Components
    /// with keyboard-owned state, such as the launcher or an application
    /// context menu, override [`Chrome::key_char`]; others no-op.
    pub fn key_char(&mut self, kc: aegis_model::input::KeyChar) {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        let freeze = self.screenshot_freeze;
        let events = &mut self.events;
        for component in self.components.iter_mut() {
            if participates_in_shell_pass(
                component.as_ref(),
                freeze,
                modal_active,
                window_switcher_active,
                exclusive_presentation_active,
            ) {
                component.key_char(&kc, events);
            }
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
            self.broadcast_command(ChromeCommand::ClosePrism);
        }
        self.broadcast_command(ChromeCommand::ToggleLauncher);
    }

    /// Fire the global Prism hotkey. Opening Prism closes the application
    /// launcher first so only one catalog surface owns keyboard input.
    pub fn toggle_prism(&mut self) {
        let opening = !self
            .components
            .iter()
            .any(|component| component.prism_active());
        if opening {
            self.broadcast_command(ChromeCommand::CloseLauncher);
        }
        self.broadcast_command(ChromeCommand::TogglePrism);
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
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        if self
            .components
            .iter()
            .filter(|component| {
                participates_in_shell_pass(
                    component.as_ref(),
                    self.screenshot_freeze,
                    modal_active,
                    window_switcher_active,
                    exclusive_presentation_active,
                )
            })
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
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        self.components
            .iter()
            .filter(|component| {
                participates_in_shell_pass(
                    component.as_ref(),
                    self.screenshot_freeze,
                    modal_active,
                    window_switcher_active,
                    exclusive_presentation_active,
                )
            })
            .any(|component| component.requires_composition())
    }

    /// Return the precise compositor-owned work that would affect the next
    /// output frame.  Direct-scanout policy consumes this instead of treating
    /// registered components, dormant animations, or a non-zero blur setting
    /// as global blockers.
    pub fn composition_requirements(&self) -> CompositionRequirements {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        let mut requirements = CompositionRequirements::default();
        for component in self.components.iter().filter(|component| {
            participates_in_shell_pass(
                component.as_ref(),
                self.screenshot_freeze,
                modal_active,
                window_switcher_active,
                exclusive_presentation_active,
            )
        }) {
            let visible = component.requires_composition();
            requirements.visible_pixels |= visible;
            // A blur request belonging to visually empty chrome is not live:
            // there are no output pixels that could consume the backdrop.
            requirements.live_backdrop_effect |= visible && component.backdrop_blur_sigma() > 0.0;
        }
        requirements
    }

    /// Strongest backdrop blur requested by any registered component, in
    /// logical pixels. The executable converts it to physical pixels before
    /// invoking flux's realtime multi-resolution filter.
    pub fn backdrop_blur_sigma(&self) -> f32 {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        self.components
            .iter()
            .filter(|component| {
                participates_in_shell_pass(
                    component.as_ref(),
                    self.screenshot_freeze,
                    modal_active,
                    window_switcher_active,
                    exclusive_presentation_active,
                )
            })
            .map(|component| component.backdrop_blur_sigma())
            .fold(0.0_f32, f32::max)
    }

    /// Run the backdrop prepass for the components eligible to render this
    /// frame. Call once after input is built and before querying blur regions.
    pub fn prepare_backdrop(&mut self, input: &Input) {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        let freeze = self.screenshot_freeze;
        for component in &mut self.components {
            if participates_in_shell_pass(
                component.as_ref(),
                freeze,
                modal_active,
                window_switcher_active,
                exclusive_presentation_active,
            ) {
                component.prepare_backdrop(input, &self.windows, &self.workspaces);
            }
        }
    }

    /// Aggregate every component's resting minimize-flight targets (the
    /// dock's tile icons), in output coordinates. The main loop pushes these
    /// into the server each frame so even a client-initiated minimize
    /// animates toward the real icon instead of a hardcoded point.
    pub fn minimize_targets(
        &self,
        display: (f32, f32),
    ) -> Vec<(aegis_model::window::WindowId, aegis_model::Rect)> {
        let mut targets = Vec::new();
        for component in &self.components {
            component.minimize_targets(display, &mut targets);
        }
        targets
    }

    /// Glass regions contributed by components that will render this frame.
    /// Ordinary chrome is excluded while a modal is active, matching the
    /// render path below.
    pub fn backdrop_regions(&self, display: (f32, f32)) -> Vec<BackdropRegion> {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        let mut regions = Vec::new();
        for component in &self.components {
            if participates_in_shell_pass(
                component.as_ref(),
                self.screenshot_freeze,
                modal_active,
                window_switcher_active,
                exclusive_presentation_active,
            ) && component.backdrop_blur_sigma() > 0.0
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

    /// Analytic liquid-glass bodies contributed by components that will
    /// render this frame. Visibility filtering mirrors [`Self::backdrop_regions`].
    pub fn liquid_glass_regions(&self, display: (f32, f32)) -> Vec<LiquidGlassRegion> {
        let modal_active = self
            .components
            .iter()
            .any(|component| component.modal_active());
        let window_switcher_active = self.window_switcher_active();
        let exclusive_presentation_active = self.exclusive_presentation_active();
        let mut regions = Vec::new();
        for component in &self.components {
            if participates_in_shell_pass(
                component.as_ref(),
                self.screenshot_freeze,
                modal_active,
                window_switcher_active,
                exclusive_presentation_active,
            ) && component.backdrop_blur_sigma() > 0.0
            {
                regions.extend(component.liquid_glass_regions(
                    display,
                    &self.windows,
                    &self.workspaces,
                ));
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
            let modal_active = components.iter().any(|component| component.modal_active());
            let window_switcher_active = components
                .iter()
                .any(|component| component.window_switcher_active());
            let exclusive_presentation_active = components
                .iter()
                .any(|component| component.exclusive_presentation_active());
            let modal_reserved = components
                .iter()
                .filter(|component| component.persistent_decoration())
                .filter(|component| {
                    participates_in_shell_pass(
                        component.as_ref(),
                        freeze,
                        modal_active,
                        window_switcher_active,
                        exclusive_presentation_active,
                    )
                })
                .fold(Reserved::default(), |mut total, component| {
                    let edge = component.reserved();
                    total.top += edge.top;
                    total.bottom += edge.bottom;
                    total.left += edge.left;
                    total.right += edge.right;
                    total
                });
            for component in components.iter_mut() {
                component.update(ChromeUpdate::ModalReserved(modal_reserved));
            }
            self.ui.frame(input, |f| {
                for component in components.iter_mut() {
                    if participates_in_shell_pass(
                        component.as_ref(),
                        freeze,
                        modal_active,
                        window_switcher_active,
                        exclusive_presentation_active,
                    ) {
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

    struct OrdinaryChrome;

    impl Chrome for OrdinaryChrome {
        fn render(
            &mut self,
            _frame: &mut Frame,
            _input: &Input,
            _windows: &[Window],
            _workspaces: &WorkspaceSnapshot,
            _i18n: &Localizer,
            _out: &mut ChromeEvents,
        ) {
        }
    }

    struct PersistentDecoration;

    impl Chrome for PersistentDecoration {
        fn render(
            &mut self,
            _frame: &mut Frame,
            _input: &Input,
            _windows: &[Window],
            _workspaces: &WorkspaceSnapshot,
            _i18n: &Localizer,
            _out: &mut ChromeEvents,
        ) {
        }

        fn persistent_decoration(&self) -> bool {
            true
        }
    }

    struct ExclusivePresentation;

    impl Chrome for ExclusivePresentation {
        fn render(
            &mut self,
            _frame: &mut Frame,
            _input: &Input,
            _windows: &[Window],
            _workspaces: &WorkspaceSnapshot,
            _i18n: &Localizer,
            _out: &mut ChromeEvents,
        ) {
        }

        fn modal_active(&self) -> bool {
            true
        }

        fn visible_during_modal(&self) -> bool {
            true
        }

        fn exclusive_presentation_active(&self) -> bool {
            true
        }
    }

    struct SwitcherPresentation;

    impl Chrome for SwitcherPresentation {
        fn render(
            &mut self,
            _frame: &mut Frame,
            _input: &Input,
            _windows: &[Window],
            _workspaces: &WorkspaceSnapshot,
            _i18n: &Localizer,
            _out: &mut ChromeEvents,
        ) {
        }

        fn visible_during_modal(&self) -> bool {
            true
        }

        fn window_switcher_active(&self) -> bool {
            true
        }
    }

    struct ScreenshotSessionChrome;

    impl Chrome for ScreenshotSessionChrome {
        fn render(
            &mut self,
            _frame: &mut Frame,
            _input: &Input,
            _windows: &[Window],
            _workspaces: &WorkspaceSnapshot,
            _i18n: &Localizer,
            _out: &mut ChromeEvents,
        ) {
        }

        fn screenshot_active(&self) -> bool {
            true
        }
    }

    #[test]
    fn ordinary_overlays_preserve_decorations_but_exclusive_presentations_do_not() {
        let ordinary = OrdinaryChrome;
        let decoration = PersistentDecoration;
        let exclusive = ExclusivePresentation;
        let switcher = SwitcherPresentation;

        assert!(!participates_in_shell_pass(
            &ordinary, false, true, false, false
        ));
        assert!(participates_in_shell_pass(
            &decoration,
            false,
            true,
            false,
            false
        ));
        assert!(!participates_in_shell_pass(
            &decoration,
            false,
            true,
            false,
            true
        ));
        assert!(participates_in_shell_pass(
            &exclusive, false, true, false, true
        ));
        assert!(!participates_in_shell_pass(
            &exclusive, false, true, true, true
        ));
        assert!(participates_in_shell_pass(
            &switcher, false, true, true, true
        ));
    }

    #[test]
    fn screenshot_freeze_outranks_modal_and_exclusive_presentations() {
        let selector = ScreenshotSessionChrome;
        let decoration = PersistentDecoration;
        let exclusive = ExclusivePresentation;

        // While the freeze holds, only the selector participates — even when
        // an exclusive presentation such as the command panel is still open
        // underneath (its pixels are already part of the frozen snapshot).
        assert!(participates_in_shell_pass(
            &selector, true, true, false, true
        ));
        assert!(!participates_in_shell_pass(
            &exclusive, true, true, false, true
        ));
        assert!(!participates_in_shell_pass(
            &decoration, true, true, false, true
        ));
    }

    #[test]
    fn reserved_inset_shrinks_and_clamps() {
        let r = Reserved {
            top: 10,
            bottom: 76,
            left: 4,
            right: 0,
        };
        let out = r.inset(aegis_model::Rect::new(0, 0, 1000, 800));
        assert_eq!(out.origin, aegis_model::Point { x: 4, y: 10 });
        assert_eq!(out.size, aegis_model::Size { w: 996, h: 714 }); // 800-10-76
    }

    #[test]
    fn reserved_inset_clamps_to_non_negative() {
        let r = Reserved {
            top: 0,
            bottom: 2000,
            left: 0,
            right: 0,
        };
        let out = r.inset(aegis_model::Rect::new(0, 0, 100, 100));
        assert_eq!(out.size, aegis_model::Size { w: 100, h: 0 });
    }
}
