//! Compositor backends.
//!
//! A backend owns the presentation target and the raw input stream. The
//! first backend is *nested* — ass runs as a client of an existing Wayland or
//! X11 session and presents into a host window. A later backend drives DRM/KMS
//! directly with libinput and libseat for bare-TTY operation.
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

    /// Drain buffered input events since the last call. The vector is empty
    /// until a backend with real input lands (the nested backend wires host
    /// seat in milestone M1). Drained events are routed by the main loop: the
    /// focused client receives them via `wl_seat`, with a copy to the chrome
    /// when the pointer is over it.
    fn take_input(&mut self) -> Vec<InputEvent>;

    /// Take a pending resize, if the host reconfigured the window since the
    /// last call. The size is in physical pixels.
    fn take_resize(&mut self) -> Option<Size>;
}

pub mod nested;
