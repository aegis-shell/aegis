//! Compositor chrome for ass, built on flux-ui.
//!
//! The shell is split into a **core host** and pluggable **chrome components**.
//! The core ([`Shell`]) owns the flux-ui context, the per-frame snapshot of
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

use flux_ui::{Frame, Ui};

pub mod chrome;
pub use chrome::{Decorations, Dock, WindowList};

use ass_core::window::Window;

/// Re-export so callers can construct input snapshots without depending on
/// flux-ui directly.
pub use flux_ui::Input;

/// Interaction intents chrome components emit during a frame. The core
/// collects these and the main loop drains them into server window-management
/// actions (`focus_surface_by_id`, `close_toplevel`, `start_interactive_move`).
/// Each field is set at most once per frame; components share the sink.
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
}

/// One piece of compositor chrome.
///
/// A component renders itself for one frame from the shared window snapshot
/// and input, drawing through `frame` and pushing any user intents into `out`.
/// The core owns the flux-ui context, the snapshot, and the sink; the component
/// owns only its own appearance and state. Register implementations with
/// [`Shell::add`].
pub trait Chrome {
    /// Draw the component for this frame. Called inside the core's
    /// `Ui::frame` envelope, so `frame` is a live builder.
    fn render(
        &mut self,
        frame: &mut Frame,
        input: &Input,
        windows: &[Window],
        out: &mut ChromeEvents,
    );
}

/// Errors from the shell.
#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    #[error("shell create: {0:?}")]
    Create(#[source] flux_ui::Error),
    #[error("shell render: {0:?}")]
    Render(#[source] flux_ui::Error),
}

/// The core chrome host.
///
/// Owns a flux-ui context bound to the compositor's flux device, the per-frame
/// window snapshot, the interaction sink, and a registry of [`Chrome`]
/// components. The host renders the chrome into the output canvas each frame by
/// running every registered component inside one `Ui::frame` envelope. It has
/// no built-in chrome of its own; the binary composes it from components.
pub struct Shell {
    ui: Ui,
    windows: Vec<Window>,
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
    /// to flux-ui's distinct-but-ABI-identical `flux_device`.
    pub unsafe fn new(device: *mut c_void) -> Result<Shell, ShellError> {
        let ui = Ui::with_device(device as *mut flux_ui::sys::flux_device)
            .map_err(ShellError::Create)?;
        Ok(Shell {
            ui,
            windows: Vec::new(),
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

    /// Run every registered component and render the chrome into `canvas`,
    /// using `input` for interaction.
    ///
    /// # Safety
    /// `canvas` must be a live `flux_canvas` (from [`flux::Canvas::as_raw`])
    /// currently inside a `begin`/`end` recording pair on the active frame.
    pub unsafe fn render(&mut self, canvas: *mut c_void, input: &Input) -> Result<(), ShellError> {
        let windows = &self.windows;
        let events = &mut self.events;
        let components = &mut self.components;
        self.ui.frame(input, |f| {
            for component in components.iter_mut() {
                component.render(f, input, windows, events);
            }
        });
        self.ui
            .render(canvas as *mut flux_ui::sys::flux_canvas)
            .map_err(ShellError::Render)
    }
}
