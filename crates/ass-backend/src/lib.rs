//! Compositor backends.
//!
//! A backend owns the presentation target and the raw input stream. The
//! *nested* backend runs as a client of an existing Wayland session. The
//! *drm* backend drives atomic KMS directly, with libinput and libseat for
//! bare-TTY operation.
//!
//! Both implement [`Backend`], so the server, renderer, and shell are written
//! once against the abstraction.

use ass_core::input::{InputEvent, PointerGestureEvent, TextInputEvent, TextInputState};
use ass_core::output::{OutputGeometry, OutputInfo, OutputMode, Scale};
use ass_core::Size;
use std::time::Duration;

/// A presentation + input target the compositor drives each frame.
pub trait Backend {
    /// Current size of the presentation target in compositor logical pixels.
    /// Backend-specific accessors expose the physical render extent when it
    /// differs (for example, a nested HiDPI Wayland surface).
    fn size(&self) -> Size;

    /// Current physical render extent in device pixels.
    fn physical_size(&self) -> (u32, u32) {
        let size = self.size();
        (size.w.max(1) as u32, size.h.max(1) as u32)
    }

    /// Logical-to-physical scale for this target.
    fn scale(&self) -> f32 {
        1.0
    }

    /// Logical extent as unsigned dimensions.
    fn size_u32(&self) -> (u32, u32) {
        let size = self.size();
        (size.w.max(1) as u32, size.h.max(1) as u32)
    }

    /// Connected outputs and their global logical geometry.
    fn output_infos(&self) -> Vec<OutputInfo> {
        let (width, height) = self.physical_size();
        vec![OutputInfo {
            connector: "unknown".to_owned(),
            geometry: OutputGeometry {
                mode: OutputMode {
                    width: width as i32,
                    height: height as i32,
                    refresh_mhz: 0,
                },
                scale: Scale(self.scale()),
                transform: ass_core::Transform::Normal,
                logical_origin: ass_core::Point::default(),
            },
            available_modes: Vec::new(),
        }]
    }

    /// Install the per-connector display-mode requests from the config's
    /// `[[output]]` entries (ADR-0028), keyed by connector name. Only the
    /// DRM backend can act on them; the default is a no-op so backends
    /// without modesetting (nested) ignore the policy.
    fn set_configured_modes(
        &mut self,
        _modes: std::collections::HashMap<String, ass_core::output::ModeSpec>,
    ) {
    }

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

    /// Whether the presentation surface must be recreated rather than resized
    /// because backend buffer constraints changed. Direct KMS reports this
    /// when a hotplug alters the plane-modifier intersection the Flux surface
    /// was created with; nested surfaces only ever resize, hence the default.
    fn surface_needs_recreate(&self) -> bool {
        false
    }

    /// Apply text-input state to an outer IME. Direct-display backends leave
    /// this to a future compositor-owned input-method implementation.
    fn set_text_input_state(&mut self, _state: TextInputState) {}

    /// Drain IME events from the backend.
    fn take_text_input(&mut self) -> Vec<TextInputEvent> {
        Vec::new()
    }

    /// Drain high-level touchpad gesture events.
    fn take_pointer_gestures(&mut self) -> Vec<PointerGestureEvent> {
        Vec::new()
    }

    /// Forward a cursor-shape request to an outer compositor. A direct KMS
    /// backend renders or scans out its own cursor and therefore ignores this.
    fn set_cursor_shape(&mut self, _shape: u32) {}

    /// Hide an outer compositor cursor. No-op for direct display.
    fn hide_cursor(&mut self) {}

    /// Advertise a nested buffer scale. No-op for direct display.
    fn set_buffer_scale(&self) {}

    /// Whether the backend currently owns an active presentation session.
    /// Direct-display sessions become inactive while switched away from their
    /// VT; nested sessions remain active until their host closes the window.
    fn is_active(&self) -> bool {
        true
    }

    /// Ask the session manager to switch to virtual terminal `vt` (1-based).
    /// Direct-display backends forward to libseat; the nested backend is a
    /// client of the host session and cannot switch VTs, so it ignores this.
    fn switch_vt(&mut self, _vt: i32) {}
}

pub mod drm;
pub mod host;
pub mod nested;
