//! Compositor chrome for ass, built on lens.
//!
//! The shell is split into a **core host** and pluggable **chrome components**.
//! The core ([`Shell`]) owns the lens context, the per-frame snapshot of
//! live toplevels, and the interaction sink ([`ChromeEvents`]); it knows
//! nothing about what the chrome looks like. Each piece of chrome — the window
//! list, server-side decorations, the dock, a future HUD bar — is a [`Chrome`]
//! implementation registered with [`Shell::add`], and renders itself each frame
//! from the shared snapshot and input. Adding or removing a chrome surface is
//! a component change, not a core change.
//!
//! Input the compositor captures is fed here as a snapshot before being routed
//! to clients; component-emitted intents are drained by the main loop into
//! server window-management actions.

use std::collections::HashMap;
use std::os::raw::c_void;

use lens::{Frame, Ui};

pub mod chrome;
pub mod i18n;
pub mod system;
pub use chrome::{
    ControlCenter, Decorations, Dock, DockApp, HudBar, Launcher, Overview, ScreenshotSelector,
    Toast, WorkspaceBar,
};
pub use i18n::{Language, Localizer, Message};
pub use system::{BatteryStatus, NetworkState, SystemAction, SystemStatus};

use ass_core::app::{ApplicationTarget, BuiltInApplication, Entry};
use ass_core::realm::{RealmId, RealmSnapshot, RealmState};
use ass_core::window::Window;
use ass_core::workspace::WorkspaceSnapshot;

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
    pub fn inset(self, r: ass_core::Rect) -> ass_core::Rect {
        ass_core::Rect {
            origin: ass_core::Point {
                x: r.origin.x + self.left,
                y: r.origin.y + self.top,
            },
            size: ass_core::Size {
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
    Focus(ass_core::window::WindowId),
    /// Hide a window while keeping its client and buffers alive.
    Minimize(ass_core::window::WindowId),
    /// Ask a client to close one of its toplevels gracefully.
    Close(ass_core::window::WindowId),
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
        window: ass_core::window::WindowId,
        target: RealmId,
        retain_source_as_observer: bool,
        expected_revision: u64,
    },
}

/// Interaction intents chrome components emit during a frame. The core
/// collects these and the main loop drains them into server window-management
/// actions (`focus_surface_by_id`, `close_toplevel`,
/// `start_interactive_move`) or, for [`ChromeEvents::spawn`], into
/// `ass-launch`. Scalar intents keep the latest value; application menus use
/// an ordered queue for multi-window actions.
#[derive(Debug, Default)]
pub struct ChromeEvents {
    /// The chrome requested the session to quit.
    pub quit: bool,
    /// Window id a component asked to focus/activate.
    pub clicked: Option<ass_core::window::WindowId>,
    /// Window id a component asked to close.
    pub closed: Option<ass_core::window::WindowId>,
    /// Window id a component asked to start an interactive move on.
    pub move_requested: Option<ass_core::window::WindowId>,
    /// Ordered window actions emitted by popup menus. A queue allows an
    /// application-level action such as "close all windows" to preserve one
    /// journal entry per affected toplevel.
    pub window_actions: Vec<WindowAction>,
    /// A desktop entry the chrome asked to launch (e.g. the launcher's
    /// clicked row). Drained into `ass-launch` by the main loop; carrying the
    /// full [`Entry`] keeps `ass-shell` free of any `ass-apps` dependency
    /// (ADR-0022).
    pub spawn: Option<Entry>,
    /// A trusted compositor-owned application to present. Built-ins share the
    /// launcher catalog with external apps but never pass through a shell
    /// command or process boundary.
    pub open_builtin: Option<BuiltInApplication>,
    /// A workspace the chrome asked to switch to (the workspace bar's clicked
    /// tile). Drained into `Server::switch_workspace_to` by the main loop
    /// (ADR-0025).
    pub switch_workspace: Option<ass_core::workspace::WorkspaceId>,
    /// The chrome asked to toggle the launcher this frame (the dock's
    /// Launchpad tile). Drained by the main loop, which calls [`Shell::toggle`]
    /// — the same path as the Super-tap hotkey — so the launcher flips open or
    /// closed.
    pub toggle_launcher: bool,
    /// Notification id the toast stack asked to dismiss. Drained through the
    /// same command/journal path as an IPC dismissal.
    pub dismissed_notification: Option<u64>,
    /// Window id the overview asked to focus this frame (a thumbnail click).
    /// Drained through the focus command/journal path; picking also closes
    /// the overview.
    pub overview_pick: Option<ass_core::window::WindowId>,
    /// Workspace id the overview's rail asked to switch to. Drained through
    /// the same command/journal path as `SwitchWorkspaceTo`.
    pub overview_switch: Option<ass_core::workspace::WorkspaceId>,
    /// Region the screenshot selector asked to capture this frame, if any.
    pub screenshot_region: Option<ass_core::Rect>,
    /// Ordered host-system mutations requested by compositor-owned UI.
    pub system_actions: Vec<SystemAction>,
    /// Desktop ids the dock asked to toggle in the pinned list. Drained by the
    /// main loop, which updates `[dock] pinned` in the config and refreshes the
    /// dock catalog.
    pub dock_pin_toggles: Vec<String>,
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

    /// Whether this component currently owns the keyboard (e.g. an open
    /// launcher). When any registered component returns true, the main loop
    /// routes resolved key events to [`Chrome::key_char`] and withholds them
    /// from the focused client. Default `false`; override in components that
    /// capture text input.
    fn captures_keyboard(&self) -> bool {
        false
    }

    /// Handle one resolved key event while [`Chrome::captures_keyboard`] is
    /// true. Default no-op; override to consume typed input (the launcher's
    /// search box).
    fn key_char(&mut self, _kc: &ass_core::input::KeyChar, _out: &mut ChromeEvents) {}

    /// The global launcher hotkey fired (a Super tap detected by the main
    /// loop's `TapDetector`). Default no-op; components with an open/closed
    /// state (the launcher) override this to flip it.
    fn toggle(&mut self, _out: &mut ChromeEvents) {}

    /// Present one compositor-owned application. Only the component backing
    /// the requested identity acts; ordinary chrome ignores it.
    fn open_builtin(&mut self, _app: BuiltInApplication) {}

    /// Receive a normalized host-system snapshot. Components keep their own
    /// presentation copy so the render trait remains focused on frame data.
    fn update_system_status(&mut self, _status: &SystemStatus) {}

    /// Receive the complete Realm authority snapshot. Only trusted
    /// compositor-owned components consume this high-level state.
    fn update_realms(&mut self, _snapshot: &RealmSnapshot) {}

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

    /// Whether the screenshot region selector is currently active. Default
    /// `false`; the screenshot selector overrides this.
    fn screenshot_active(&self) -> bool {
        false
    }

    /// Accessibility reduced-motion policy (ADR-0029). When enabled, every
    /// component transition (springs, fades, slides) resolves to its end
    /// state in at most one frame. The shell fans this out to all components
    /// and to lens itself; default no-op for static components.
    fn set_reduced_motion(&mut self, _reduced: bool) {}

    /// Refresh the host application catalog and decoded icon map. Components
    /// that do not display applications ignore it; the launcher and dock
    /// replace their snapshots in place.
    fn update_app_catalog(
        &mut self,
        _apps: &[Entry],
        _dock_apps: &[DockApp],
        _icons: &HashMap<String, *mut c_void>,
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
    events: ChromeEvents,
    components: Vec<Box<dyn Chrome>>,
    /// Accessibility reduced-motion policy (ADR-0029), fanned out to every
    /// registered component (including ones added later) and to lens.
    reduced_motion: bool,
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
        let ui =
            Ui::with_device(device as *mut lens::sys::flux_device).map_err(ShellError::Create)?;
        Ok(Shell {
            ui,
            windows: Vec::new(),
            workspaces: WorkspaceSnapshot {
                outputs: Vec::new(),
            },
            i18n: Localizer::from_env(),
            system_status: SystemStatus::default(),
            realms: ass_core::realm::RealmModel::new().snapshot(),
            events: ChromeEvents::default(),
            components: Vec::new(),
            reduced_motion: false,
        })
    }

    /// Register a chrome component. Components render once per frame, in
    /// registration order.
    pub fn add(&mut self, mut component: Box<dyn Chrome>) {
        component.update_system_status(&self.system_status);
        component.update_realms(&self.realms);
        component.set_reduced_motion(self.reduced_motion);
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
    }

    /// Replace the host's workspace snapshot. Called once per frame by the
    /// main loop with `server.workspace_snapshot()`.
    pub fn set_workspaces(&mut self, workspaces: WorkspaceSnapshot) {
        self.workspaces = workspaces;
    }

    /// Drain the surface id of the window a component asked to focus this
    /// frame, if any.
    pub fn take_clicked_window(&mut self) -> Option<ass_core::window::WindowId> {
        self.events.clicked.take()
    }

    /// Drain the surface id of the window a component asked to close this
    /// frame, if any.
    pub fn take_closed_window(&mut self) -> Option<ass_core::window::WindowId> {
        self.events.closed.take()
    }

    /// Drain the surface id of the window a component asked to move this
    /// frame, if any. The main loop forwards this to
    /// `Server::start_interactive_move`.
    pub fn take_move_requested(&mut self) -> Option<ass_core::window::WindowId> {
        self.events.move_requested.take()
    }

    /// Drain ordered window actions emitted by application context menus.
    pub fn take_window_actions(&mut self) -> Vec<WindowAction> {
        std::mem::take(&mut self.events.window_actions)
    }

    /// Drain the desktop entry the chrome asked to launch this frame, if any.
    /// The main loop hands it to `ass-launch`.
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
    /// Control Center before their next frame.
    pub fn set_realms(&mut self, snapshot: RealmSnapshot) {
        self.realms = snapshot;
        for component in self.components.iter_mut() {
            component.update_realms(&self.realms);
        }
    }

    /// Drain ordered system mutations requested by trusted shell UI.
    pub fn take_system_actions(&mut self) -> Vec<SystemAction> {
        std::mem::take(&mut self.events.system_actions)
    }

    /// Drain desktop ids the dock asked to pin/unpin this frame.
    pub fn take_dock_pin_toggles(&mut self) -> Vec<String> {
        std::mem::take(&mut self.events.dock_pin_toggles)
    }

    /// Drain trusted Realm-management intents in UI order.
    pub fn take_realm_intents(&mut self) -> Vec<RealmIntent> {
        std::mem::take(&mut self.events.realm_intents)
    }

    /// Drain the workspace id the chrome asked to switch to this frame, if
    /// any (the workspace bar's clicked tile). The main loop forwards it to
    /// `Server::switch_workspace_to`.
    pub fn take_switch_workspace(&mut self) -> Option<ass_core::workspace::WorkspaceId> {
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

    /// Window id the overview asked to focus this frame, if any.
    pub fn take_overview_pick(&mut self) -> Option<ass_core::window::WindowId> {
        self.events.overview_pick.take()
    }

    /// Workspace id the overview's rail asked to switch to, if any.
    pub fn take_overview_switch(&mut self) -> Option<ass_core::workspace::WorkspaceId> {
        self.events.overview_switch.take()
    }

    /// Open the screenshot region selector. No-op if no selector component is
    /// registered.
    pub fn start_screenshot(&mut self) {
        for component in self.components.iter_mut() {
            // Only the screenshot selector reacts; other components ignore it.
            component.open_builtin(ass_core::app::BuiltInApplication::ScreenshotSelector);
        }
    }

    /// Region the screenshot selector asked to capture this frame, if any.
    pub fn take_screenshot_region(&mut self) -> Option<ass_core::Rect> {
        self.events.screenshot_region.take()
    }

    /// Whether the screenshot selector is currently active.
    pub fn screenshot_active(&self) -> bool {
        self.components.iter().any(|c| c.screenshot_active())
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
            .filter(|component| !modal_active || component.visible_during_modal())
            .find(|component| {
                component.captures_pointer(x, y, display, &self.windows, &self.workspaces)
            })
            .and_then(|component| {
                component.cursor_shape_at(x, y, display, &self.windows, &self.workspaces)
            })
    }

    /// Push a newly scanned application catalog to interested components.
    pub fn update_app_catalog(
        &mut self,
        apps: &[Entry],
        dock_apps: &[DockApp],
        icons: &HashMap<String, *mut c_void>,
    ) {
        for component in self.components.iter_mut() {
            component.update_app_catalog(apps, dock_apps, icons);
        }
    }

    /// Feed one resolved key event to every registered component. Components
    /// with keyboard-owned state, such as the launcher or an application
    /// context menu, override [`Chrome::key_char`]; others no-op.
    pub fn key_char(&mut self, kc: ass_core::input::KeyChar) {
        let events = &mut self.events;
        for component in self.components.iter_mut() {
            component.key_char(&kc, events);
        }
    }

    /// Fire the global launcher hotkey (a Super tap the main loop detected).
    /// Components with an open/closed state flip themselves; others no-op.
    pub fn toggle(&mut self) {
        let events = &mut self.events;
        for component in self.components.iter_mut() {
            component.toggle(events);
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
        if self.components.iter().any(|c| c.anim_pending()) {
            return true;
        }
        self.ui.anim_pending()
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
        let windows = &self.windows;
        let workspaces = &self.workspaces;
        let i18n = &self.i18n;
        let events = &mut self.events;
        let components = &mut self.components;
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
        let out = r.inset(ass_core::Rect::new(0, 0, 1000, 800));
        assert_eq!(out.origin, ass_core::Point { x: 4, y: 10 });
        assert_eq!(out.size, ass_core::Size { w: 996, h: 714 }); // 800-10-76
    }

    #[test]
    fn reserved_inset_clamps_to_non_negative() {
        let r = Reserved {
            top: 0,
            bottom: 2000,
            left: 0,
            right: 0,
        };
        let out = r.inset(ass_core::Rect::new(0, 0, 100, 100));
        assert_eq!(out.size, ass_core::Size { w: 100, h: 0 });
    }
}
