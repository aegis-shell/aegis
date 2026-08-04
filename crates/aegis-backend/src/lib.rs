//! Compositor backends.
//!
//! A backend owns the presentation target and the raw input stream. The
//! *nested* backend runs as a client of an existing Wayland session. The
//! *drm* backend drives atomic KMS directly, with libinput and libseat for
//! bare-TTY operation.
//!
//! Both implement [`Backend`], so the server, renderer, and shell are written
//! once against the abstraction.

use aegis_model::Size;
use aegis_model::input::{
    InputEvent, PointerGestureEvent, TextInputEvent, TextInputState, TouchpadConfig, TouchpadStatus,
};
use aegis_model::output::{OutputGeometry, OutputInfo, OutputMode, Scale};
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
                transform: aegis_model::Transform::Normal,
                logical_origin: aegis_model::Point::default(),
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
        _modes: std::collections::HashMap<String, aegis_model::output::ModeSpec>,
    ) {
    }

    /// Install the touchpad profile and return the resulting live status.
    fn set_touchpad_config(&mut self, config: TouchpadConfig) -> TouchpadStatus {
        TouchpadStatus {
            config,
            ..TouchpadStatus::default()
        }
    }

    /// Describe attached touchpads and the profile currently selected.
    fn touchpad_status(&self) -> TouchpadStatus {
        TouchpadStatus::default()
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

    /// Register the compositor's Wayland server event-loop fd so client
    /// requests (surface commits) wake the timed/blocking dispatches
    /// alongside input and hotplug. Readability is only a wakeup — the main
    /// loop dispatches the server itself. The default ignores the fd for
    /// backends that cannot multiplex extra fds.
    fn set_wakeup_fd(&mut self, _fd: std::os::fd::RawFd) {}

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

    /// Whether scanout is currently powered. This is independent from
    /// [`Backend::is_active`]: input and Wayland dispatch must continue while
    /// outputs are off so physical activity can wake a locked session.
    fn outputs_powered(&self) -> bool {
        true
    }

    /// Whether the backend currently has a renderable presentation target.
    /// A direct-display backend can remain active for input while all
    /// connectors are unplugged or a replacement target is being prepared.
    fn presentation_target_ready(&self) -> bool {
        true
    }

    /// Enable or disable physical scanout without changing output topology.
    /// Nested backends cannot control their host compositor's monitors.
    fn set_outputs_powered(&mut self, powered: bool) -> Result<(), String> {
        if powered {
            Ok(())
        } else {
            Err("output power control is unavailable on this backend".into())
        }
    }

    /// Whether this presentation domain has an atomic commit in flight.
    ///
    /// The runtime uses this as an ownership boundary: it may continue
    /// dispatching input and client protocol traffic, but it must not submit
    /// another frame until the backend reports completion. Nested WSI owns
    /// its own queueing and therefore keeps the default `false`.
    fn presentation_pending(&self) -> bool {
        false
    }

    /// Ask the session manager to switch to virtual terminal `vt` (1-based).
    /// Direct-display backends forward to libseat; the nested backend is a
    /// client of the host session and cannot switch VTs, so it ignores this.
    fn switch_vt(&mut self, _vt: i32) {}
}

pub mod drm;
pub mod host;
pub mod nested;
