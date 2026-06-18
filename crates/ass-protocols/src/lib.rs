//! Shared Wayland protocol types and interface tables.
//!
//! This crate defines the canonical `wl_interface` / `wl_message` / `wl_array`
//! C ABI types used across ass, and exposes the xdg-shell interface tables
//! compiled by its build script. The core `wl_*_interface` symbols are not
//! declared here: they are provided by whichever libwayland (client or server)
//! the consuming crate links, and each consumer declares the externs it needs.
#![allow(non_camel_case_types, non_upper_case_globals)]

use std::os::raw::{c_char, c_int, c_void};

/// `struct wl_message` (wayland-util.h): a request or event signature.
#[repr(C)]
pub struct wl_message {
    pub name: *const c_char,
    pub signature: *const c_char,
    pub types: *const *const wl_interface,
}

/// `struct wl_interface`: an object type's request/event tables.
#[repr(C)]
pub struct wl_interface {
    pub name: *const c_char,
    pub version: c_int,
    pub method_count: c_int,
    pub methods: *const wl_message,
    pub event_count: c_int,
    pub events: *const wl_message,
}

// Interface tables are immutable data; sharing references across FFI is sound.
unsafe impl Sync for wl_interface {}

/// `struct wl_array`: payload of array-typed arguments (e.g. xdg_toplevel states).
#[repr(C)]
pub struct wl_array {
    pub size: usize,
    pub alloc: usize,
    pub data: *mut c_void,
}

impl wl_array {
    /// An empty array, for events that take a (possibly empty) array argument.
    pub const fn empty() -> wl_array {
        wl_array {
            size: 0,
            alloc: 0,
            data: std::ptr::null_mut(),
        }
    }
}

extern "C" {
    pub static xdg_wm_base_interface: wl_interface;
    pub static xdg_positioner_interface: wl_interface;
    pub static xdg_surface_interface: wl_interface;
    pub static xdg_toplevel_interface: wl_interface;
    pub static xdg_popup_interface: wl_interface;

    pub static zwp_linux_dmabuf_v1_interface: wl_interface;
    pub static zwp_linux_buffer_params_v1_interface: wl_interface;
    pub static zwp_linux_dmabuf_feedback_v1_interface: wl_interface;

    pub static wp_viewporter_interface: wl_interface;
    pub static wp_viewport_interface: wl_interface;
}
