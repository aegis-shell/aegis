//! Compositor backends.
//!
//! A backend owns the presentation target and the raw input stream. The
//! first backend is *nested* — ass runs as a client of an existing Wayland
//! session and presents into a host window. A later backend drives DRM/KMS
//! directly with libinput and seat management for bare-TTY operation.
//!
//! Both implement [`Backend`], so the server, renderer, and shell are written
//! once against the abstraction.

use ass_core::input::InputEvent;
use ass_core::Size;
use std::time::Duration;

/// A presentation + input target the compositor drives each frame.
pub trait Backend {
    /// Current size of the presentation target in compositor logical pixels.
    /// Backend-specific accessors expose the physical render extent when it
    /// differs (for example, a nested HiDPI Wayland surface).
    fn size(&self) -> Size;

    /// Pump backend events (input, resize, redraw requests). Returns `false`
    /// when the backend has been asked to shut down.
    fn dispatch(&mut self) -> bool;

    /// Drain already-readable backend events without blocking. Used while a
    /// chrome animation is in flight so the render loop keeps ticking frames
    /// to advance it instead of sleeping on the host event queue. Default
    /// falls back to the blocking [`dispatch`](Self::dispatch); backends that
    /// can poll non-blocking override this.
    fn dispatch_nonblocking(&mut self) -> bool {
        self.dispatch()
    }

    /// Wait for backend events for at most `timeout`. Timer-driven chrome
    /// (clock/status refresh) uses this to wake an otherwise idle compositor
    /// without forcing the animation-rate non-blocking loop. Backends without
    /// timed polling may keep the blocking default.
    fn dispatch_timeout(&mut self, _timeout: Duration) -> bool {
        self.dispatch()
    }

    /// Drain buffered input events since the last call. Drained events are
    /// routed by the main loop: the focused client receives them via
    /// `wl_seat`, with a copy to the chrome when the pointer is over it.
    fn take_input(&mut self) -> Vec<InputEvent>;

    /// Take a pending resize, if the host reconfigured the window since the
    /// last call. The size is in compositor logical pixels.
    fn take_resize(&mut self) -> Option<Size>;
}

pub mod nested;
