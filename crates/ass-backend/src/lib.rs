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

/// A presentation + input target the compositor drives each frame.
pub trait Backend {
    /// Current size of the presentation target, in physical pixels.
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

    /// Drain buffered input events since the last call. Drained events are
    /// routed by the main loop: the focused client receives them via
    /// `wl_seat`, with a copy to the chrome when the pointer is over it.
    fn take_input(&mut self) -> Vec<InputEvent>;

    /// Take a pending resize, if the host reconfigured the window since the
    /// last call. The size is in physical pixels.
    fn take_resize(&mut self) -> Option<Size>;
}

pub mod nested;
