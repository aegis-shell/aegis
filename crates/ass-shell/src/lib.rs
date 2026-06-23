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

use std::os::raw::c_void;

use lens::{Frame, Ui};

pub mod chrome;
pub use chrome::{Decorations, Dock, Launcher, Toast, WindowList, WorkspaceBar};

use ass_core::app::Entry;
use ass_core::window::Window;
use ass_core::workspace::WorkspaceSnapshot;

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
    /// Surface id of the window a component asked to focus/activate.
    pub clicked: Option<usize>,
    /// Surface id of the window a component asked to close.
    pub closed: Option<usize>,
    /// Surface id of the window a component asked to start an interactive
    /// move on.
    pub move_requested: Option<usize>,
    /// A desktop entry the chrome asked to launch (e.g. the launcher's
    /// clicked row). Drained into `ass-launch` by the main loop; carrying the
    /// full [`Entry`] keeps `ass-shell` free of any `ass-apps` dependency
    /// (ADR-0022).
    pub spawn: Option<Entry>,
    /// A workspace the chrome asked to switch to (the workspace bar's clicked
    /// tile). Drained into `Server::switch_workspace_to` by the main loop
    /// (ADR-0025).
    pub switch_workspace: Option<ass_core::workspace::WorkspaceId>,
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
            workspaces: WorkspaceSnapshot { outputs: Vec::new() },
            events: ChromeEvents::default(),
            components: Vec::new(),
        })
    }

    /// Register a chrome component. Components render once per frame, in
    /// registration order.
    pub fn add(&mut self, component: Box<dyn Chrome>) {
        self.components.push(component);
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
    pub fn take_clicked_window(&mut self) -> Option<usize> {
        self.events.clicked.take()
    }

    /// Drain the surface id of the window a component asked to close this
    /// frame, if any.
    pub fn take_closed_window(&mut self) -> Option<usize> {
        self.events.closed.take()
    }

    /// Drain the surface id of the window a component asked to move this
    /// frame, if any. The main loop forwards this to
    /// `Server::start_interactive_move`.
    pub fn take_move_requested(&mut self) -> Option<usize> {
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

    /// Whether any registered component currently captures keyboard input
    /// (e.g. an open launcher). The main loop checks this to decide whether to
    /// route key events to [`Shell::key_char`] or forward them to the focused
    /// client.
    pub fn captures_keyboard(&self) -> bool {
        self.components.iter().any(|c| c.captures_keyboard())
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
            for component in components.iter_mut() {
                component.render(f, input, windows, workspaces, events);
            }
        });
        self.ui
            .render(canvas as *mut lens::sys::flux_canvas)
            .map_err(ShellError::Render)
    }
}
