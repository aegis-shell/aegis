//! Nested backend: aegis as a client of a host Wayland session.
//!
//! Brings up an xdg-shell toplevel over raw libwayland-client and creates a
//! `VkSurfaceKHR` on flux's Vulkan instance via `ash`, so flux can present into
//! the host window. This is the development backend; a DRM/KMS backend replaces
//! it for bare-TTY operation.

mod ffi;
mod listeners;
mod protocol;
mod runtime;

use std::ffi::{CStr, CString, c_void};
use std::ptr;

use aegis_model::Size;
use aegis_model::input::{
    InputEvent, PointerAxis, PointerAxisFrame, PointerAxisRelativeDirection, PointerAxisSource,
    PointerGestureEvent, TextInputEvent, TextInputState,
};
use ash::vk::Handle;

use crate::Backend;

/// Vulkan instance extensions flux must enable for a nested Wayland surface.
pub const INSTANCE_EXTENSIONS: [&CStr; 2] = [c"VK_KHR_surface", c"VK_KHR_wayland_surface"];

/// Vulkan device extensions flux must enable to present (swapchain).
pub const DEVICE_EXTENSIONS: [&CStr; 1] = [c"VK_KHR_swapchain"];

/// Errors bringing up the nested backend.
#[derive(Debug, thiserror::Error)]
pub enum NestedError {
    /// Could not connect to the host Wayland display (`$WAYLAND_DISPLAY`).
    #[error("cannot connect to host Wayland display (is WAYLAND_DISPLAY set?)")]
    Connect,
    /// A required global was not advertised by the host.
    #[error("host does not advertise required global: {0}")]
    MissingGlobal(&'static str),
    /// A `wl_display_roundtrip` failed.
    #[error("wl_display_roundtrip failed")]
    Roundtrip,
    /// Vulkan surface creation failed.
    #[error("VkSurfaceKHR creation failed")]
    Vulkan,
}

/// Mutable state shared with the C event callbacks via a stable heap pointer.
struct State {
    compositor: *mut ffi::wl_proxy,
    wm_base: *mut ffi::wl_proxy,
    viewporter: *mut ffi::wl_proxy,
    fractional_scale_manager: *mut ffi::wl_proxy,
    cursor_shape_manager: *mut ffi::wl_proxy,
    cursor_shape_device: *mut ffi::wl_proxy,
    pointer_gestures_manager: *mut ffi::wl_proxy,
    gesture_swipe: *mut ffi::wl_proxy,
    gesture_pinch: *mut ffi::wl_proxy,
    gesture_hold: *mut ffi::wl_proxy,
    text_input_manager: *mut ffi::wl_proxy,
    text_input: *mut ffi::wl_proxy,
    seat: *mut ffi::wl_proxy,
    pointer: *mut ffi::wl_proxy,
    keyboard: *mut ffi::wl_proxy,
    last_pointer_serial: u32,
    /// Last absolute position the host pointer reported, used to derive
    /// relative deltas for `InputEvent::PointerMotion`. `None` before the
    /// first enter and after the pointer leaves the window.
    last_pointer_position: Option<(f32, f32)>,
    configured: bool,
    width: i32,
    height: i32,
    pending_width: i32,
    pending_height: i32,
    resized: bool,
    should_close: bool,
    /// Bound `wl_output` globals and the integer scale each last advertised.
    /// The nested window reads the scale of the output it currently sits on
    /// (`current_output`) to size its buffer for HiDPI.
    outputs: Vec<(*mut ffi::wl_proxy, i32)>,
    /// The output the surface most recently entered, or null before the first
    /// `wl_surface.enter`.
    current_output: *mut ffi::wl_proxy,
    /// Effective integer buffer scale (>= 1) for the current output.
    scale: i32,
    /// Preferred surface scale in 120ths, supplied by
    /// `wp_fractional_scale_v1`. Used only while `fractional_active` is true.
    preferred_scale_120: u32,
    /// True once both a fractional-scale object and viewport have been
    /// created for the host surface.
    fractional_active: bool,
    /// Set when `scale` changed (output scale event, or the window moved to a
    /// differently-scaled output); drained by `take_resize` so the main loop
    /// rebuilds the swapchain at the new physical size.
    scale_changed: bool,
    /// Input events drained by `take_input`. Pointer motion and button state
    /// changes accumulate here each dispatch; the main loop drains once per
    /// frame.
    input_events: Vec<InputEvent>,
    /// Axis callbacks accumulated until the host's `wl_pointer.frame`.
    pending_pointer_axis: PointerAxisFrame,
    pointer_gesture_events: Vec<PointerGestureEvent>,
    text_input_events: Vec<TextInputEvent>,
    text_input_entered: bool,
    text_input_state: TextInputState,
}

/// A nested host window and its Vulkan surface.
pub struct NestedHost {
    display: *mut ffi::wl_display,
    registry: *mut ffi::wl_proxy,
    surface: *mut ffi::wl_proxy,
    xdg_surface: *mut ffi::wl_proxy,
    toplevel: *mut ffi::wl_proxy,
    viewport: *mut ffi::wl_proxy,
    fractional_scale: *mut ffi::wl_proxy,
    // Boxed so the address handed to the C callbacks stays stable across moves.
    state: Box<State>,
    // Retained so the surface can be destroyed on drop. The `ash::Instance` is
    // `load`ed (not created) from flux's instance, so dropping it does not
    // destroy flux's instance.
    ash: Option<(ash::Entry, ash::Instance)>,
    vk_surface: u64,
    /// Persisted profile for direct-display sessions. The outer compositor
    /// owns the physical device while this backend is nested.
    touchpad_config: aegis_model::input::TouchpadConfig,
    /// The compositor's Wayland server event-loop fd, registered via
    /// `Backend::set_wakeup_fd`. Polled for readability only — the main loop
    /// dispatches the server itself once the wait wakes.
    wakeup_fd: Option<std::os::fd::RawFd>,
}
