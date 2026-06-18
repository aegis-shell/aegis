//! Raw FFI to libwayland-client and the scanner-generated xdg-shell interface
//! tables. This is the unsafe seam; the safe orchestration lives in the parent
//! module.
#![allow(non_camel_case_types)]

use std::os::raw::{c_char, c_int, c_void};

// Canonical protocol ABI types and the xdg-shell interface tables come from the
// shared protocols crate so client and server agree on one definition.
pub use ass_protocols::{wl_array, wl_interface, wl_message};
pub use ass_protocols::{xdg_surface_interface, xdg_toplevel_interface, xdg_wm_base_interface};

/// Opaque protocol object. Every Wayland object (display, registry, surface,
/// xdg_* …) is a `wl_proxy` at the C ABI level.
pub type wl_proxy = c_void;
pub type wl_display = c_void;

/// `WL_MARSHAL_FLAG_DESTROY` — marshal then destroy the proxy (destructor reqs).
pub const WL_MARSHAL_FLAG_DESTROY: u32 = 1;

// Request opcodes (from the protocol definitions).
pub const WL_DISPLAY_GET_REGISTRY: u32 = 1;
pub const WL_REGISTRY_BIND: u32 = 0;
pub const WL_COMPOSITOR_CREATE_SURFACE: u32 = 0;
pub const WL_SURFACE_COMMIT: u32 = 6;
pub const WL_SURFACE_DESTROY: u32 = 0;
pub const WL_SEAT_GET_POINTER: u32 = 0;
pub const WL_SEAT_GET_KEYBOARD: u32 = 1;
pub const WL_SEAT_GET_TOUCH: u32 = 2;
pub const WL_POINTER_RELEASE: u32 = 5;
pub const WL_KEYBOARD_RELEASE: u32 = 3;
pub const XDG_WM_BASE_DESTROY: u32 = 0;
pub const XDG_WM_BASE_GET_XDG_SURFACE: u32 = 2;
pub const XDG_WM_BASE_PONG: u32 = 3;
pub const XDG_SURFACE_DESTROY: u32 = 0;
pub const XDG_SURFACE_GET_TOPLEVEL: u32 = 1;
pub const XDG_SURFACE_ACK_CONFIGURE: u32 = 4;
pub const XDG_TOPLEVEL_DESTROY: u32 = 0;
pub const XDG_TOPLEVEL_SET_TITLE: u32 = 2;
pub const XDG_TOPLEVEL_SET_APP_ID: u32 = 3;

/// `wl_seat` capability bits.
pub const WL_SEAT_CAPABILITY_POINTER: u32 = 1;
pub const WL_SEAT_CAPABILITY_KEYBOARD: u32 = 2;
pub const WL_SEAT_CAPABILITY_TOUCH: u32 = 4;

/// 24.8 fixed-point conversion. wl_pointer events carry `wl_fixed_t`.
pub fn wl_fixed_to_f32(v: i32) -> f32 {
    (v as f32) / 256.0
}

/// `wl_registry` listener vtable.
#[repr(C)]
pub struct wl_registry_listener {
    pub global: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, u32, *const c_char, u32),
    pub global_remove: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, u32),
}

/// `xdg_wm_base` listener vtable.
#[repr(C)]
pub struct xdg_wm_base_listener {
    pub ping: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, u32),
}

/// `xdg_surface` listener vtable.
#[repr(C)]
pub struct xdg_surface_listener {
    pub configure: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, u32),
}

/// `xdg_toplevel` listener vtable (xdg_wm_base v1: configure + close).
#[repr(C)]
pub struct xdg_toplevel_listener {
    pub configure: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, c_int, c_int, *mut wl_array),
    pub close: unsafe extern "C" fn(*mut c_void, *mut wl_proxy),
}

/// `wl_seat` listener vtable.
#[repr(C)]
pub struct wl_seat_listener {
    pub capabilities: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, u32),
    pub name: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, *const c_char),
}

/// `wl_pointer` listener vtable. `frame` and later events are v5+; ass binds
/// at v4 so those slots exist for ABI layout but are never invoked by the host.
#[repr(C)]
pub struct wl_pointer_listener {
    pub enter: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, u32, *mut wl_proxy, i32, i32),
    pub leave: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, u32, *mut wl_proxy),
    pub motion: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, u32, i32, i32),
    pub button: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, u32, u32, u32, u32),
    pub axis: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, u32, u32, i32),
}

/// `wl_keyboard` listener vtable. The host sends the keymap once and then
/// key/modifier events as keys are pressed.
#[repr(C)]
pub struct wl_keyboard_listener {
    pub keymap: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, u32, i32, u32),
    pub enter: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, u32, *mut wl_proxy, *mut wl_array),
    pub leave: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, u32, *mut wl_proxy),
    pub key: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, u32, u32, u32, u32),
    pub modifiers: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, u32, u32, u32, u32, u32),
    pub repeat_info: unsafe extern "C" fn(*mut c_void, *mut wl_proxy, i32, i32),
}

extern "C" {
    // Core interfaces, exported by libwayland-client.
    pub static wl_registry_interface: wl_interface;
    pub static wl_compositor_interface: wl_interface;
    pub static wl_surface_interface: wl_interface;
    pub static wl_seat_interface: wl_interface;
    pub static wl_pointer_interface: wl_interface;
    pub static wl_keyboard_interface: wl_interface;
    pub static wl_touch_interface: wl_interface;
    pub static wl_output_interface: wl_interface;

    // libwayland-client entry points.
    pub fn wl_display_connect(name: *const c_char) -> *mut wl_display;
    pub fn wl_display_disconnect(display: *mut wl_display);
    pub fn wl_display_roundtrip(display: *mut wl_display) -> c_int;
    pub fn wl_display_dispatch_pending(display: *mut wl_display) -> c_int;
    pub fn wl_display_flush(display: *mut wl_display) -> c_int;
    pub fn wl_display_get_fd(display: *mut wl_display) -> c_int;

    pub fn wl_proxy_add_listener(
        proxy: *mut wl_proxy,
        implementation: *const c_void,
        data: *mut c_void,
    ) -> c_int;
    pub fn wl_proxy_destroy(proxy: *mut wl_proxy);
    pub fn wl_proxy_get_version(proxy: *mut wl_proxy) -> u32;
    pub fn wl_proxy_marshal_flags(
        proxy: *mut wl_proxy,
        opcode: u32,
        interface: *const wl_interface,
        version: u32,
        flags: u32,
        ...
    ) -> *mut wl_proxy;
}
