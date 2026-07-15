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
pub use chrome::{Decorations, Dock, DockApp, Launcher, Toast, WorkspaceBar};

use ass_core::app::Entry;
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

/// Interaction intents chrome components emit during a frame. The core
/// collects these and the main loop drains them into server window-management
/// actions (`focus_surface_by_id`, `close_toplevel`,
/// `start_interactive_move`) or, for [`ChromeEvents::spawn`], into
/// `ass-launch`. Each field is set at most once per frame; components share
/// the sink.
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
    /// A desktop entry the chrome asked to launch (e.g. the launcher's
    /// clicked row). Drained into `ass-launch` by the main loop; carrying the
    /// full [`Entry`] keeps `ass-shell` free of any `ass-apps` dependency
    /// (ADR-0022).
    pub spawn: Option<Entry>,
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
    events: ChromeEvents,
    components: Vec<Box<dyn Chrome>>,
}

impl Shell {
    /// Bind to the compositor's flux device. The host starts with no chrome
    /// registered; add components with [`Shell::add`].
    ///
    /// # Safety
    /// `device` must be a live `flux_device` (from [`flux::Device::as_raw`]) and
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
            events: ChromeEvents::default(),
            components: Vec::new(),
        })
    }

    /// Register a chrome component. Components render once per frame, in
    /// registration order.
    pub fn add(&mut self, component: Box<dyn Chrome>) {
        self.components.push(component);
    }

    /// Set the device-pixel (HiDPI) scale for the chrome. Layout and input
    /// stay in logical pixels; lens scales the canvas transform on render so
    /// chrome rasterises crisply on a scaled output. The main loop reports the
    /// backend's output scale here each time it changes.
    pub fn set_scale(&mut self, scale: f32) {
        self.ui.set_scale(scale);
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

    /// Drain the desktop entry the chrome asked to launch this frame, if any.
    /// The main loop hands it to `ass-launch`.
    pub fn take_spawn(&mut self) -> Option<Entry> {
        self.events.spawn.take()
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

    /// Feed one resolved key event to every registered component. Only
    /// components that override [`Chrome::key_char`] (the launcher) act on it;
    /// others take the default no-op.
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

    /// Run every registered component and render the chrome into `canvas`,
    /// using `input` for interaction.
    ///
    /// # Safety
    /// `canvas` must be a live `flux_canvas` (from [`flux::Canvas::as_raw`])
    /// currently inside a `begin`/`end` recording pair on the active frame.
    pub unsafe fn render(&mut self, canvas: *mut c_void, input: &Input) -> Result<(), ShellError> {
        let windows = &self.windows;
        let workspaces = &self.workspaces;
        let events = &mut self.events;
        let components = &mut self.components;
        self.ui.frame(input, |f| {
            let modal_active = components.iter().any(|component| component.modal_active());
            for component in components.iter_mut() {
                if !modal_active || component.visible_during_modal() {
                    component.render(f, input, windows, workspaces, events);
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
