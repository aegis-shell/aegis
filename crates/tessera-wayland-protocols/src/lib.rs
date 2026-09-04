//! Shared Wayland protocol types and generated interface tables.
//!
//! This crate defines the canonical `wl_interface` / `wl_message` / `wl_array`
//! C ABI types used across tessera, and exposes the shell and extension-protocol
//! interface tables compiled by its build script. The core `wl_*_interface`
//! symbols are not declared here: they are provided by whichever libwayland
//! (client or server) the consuming crate links, and each consumer declares the
//! externs it needs.
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

unsafe extern "C" {
    pub static xdg_wm_base_interface: wl_interface;
    pub static xdg_positioner_interface: wl_interface;
    pub static xdg_surface_interface: wl_interface;
    pub static xdg_toplevel_interface: wl_interface;
    pub static xdg_popup_interface: wl_interface;

    pub static zwp_linux_dmabuf_v1_interface: wl_interface;
    pub static zwp_linux_buffer_params_v1_interface: wl_interface;
    pub static zwp_linux_dmabuf_feedback_v1_interface: wl_interface;
    pub static zwp_linux_explicit_synchronization_v1_interface: wl_interface;
    pub static zwp_linux_surface_synchronization_v1_interface: wl_interface;
    pub static zwp_linux_buffer_release_v1_interface: wl_interface;

    pub static zwp_tablet_manager_v2_interface: wl_interface;
    pub static zwp_tablet_seat_v2_interface: wl_interface;
    pub static zwp_tablet_v2_interface: wl_interface;
    pub static zwp_tablet_tool_v2_interface: wl_interface;
    pub static zwp_tablet_pad_v2_interface: wl_interface;
    pub static zwp_tablet_pad_group_v2_interface: wl_interface;
    pub static zwp_tablet_pad_ring_v2_interface: wl_interface;
    pub static zwp_tablet_pad_strip_v2_interface: wl_interface;

    pub static wp_viewporter_interface: wl_interface;
    pub static wp_viewport_interface: wl_interface;

    // presentation-time
    pub static wp_presentation_interface: wl_interface;
    pub static wp_presentation_feedback_interface: wl_interface;

    // xdg-output-unstable-v1
    pub static zxdg_output_manager_v1_interface: wl_interface;
    pub static zxdg_output_v1_interface: wl_interface;

    // xdg-decoration-unstable-v1
    pub static zxdg_decoration_manager_v1_interface: wl_interface;
    pub static zxdg_toplevel_decoration_v1_interface: wl_interface;

    // xdg-foreign-unstable-v2
    pub static zxdg_exporter_v2_interface: wl_interface;
    pub static zxdg_importer_v2_interface: wl_interface;
    pub static zxdg_exported_v2_interface: wl_interface;
    pub static zxdg_imported_v2_interface: wl_interface;

    // idle-inhibit-unstable-v1
    pub static zwp_idle_inhibit_manager_v1_interface: wl_interface;
    pub static zwp_idle_inhibitor_v1_interface: wl_interface;

    // relative-pointer-unstable-v1
    pub static zwp_relative_pointer_manager_v1_interface: wl_interface;
    pub static zwp_relative_pointer_v1_interface: wl_interface;

    // pointer-gestures-unstable-v1
    pub static zwp_pointer_gestures_v1_interface: wl_interface;
    pub static zwp_pointer_gesture_swipe_v1_interface: wl_interface;
    pub static zwp_pointer_gesture_pinch_v1_interface: wl_interface;
    pub static zwp_pointer_gesture_hold_v1_interface: wl_interface;

    // keyboard-shortcuts-inhibit-unstable-v1
    pub static zwp_keyboard_shortcuts_inhibit_manager_v1_interface: wl_interface;
    pub static zwp_keyboard_shortcuts_inhibitor_v1_interface: wl_interface;

    // pointer-constraints-unstable-v1
    pub static zwp_pointer_constraints_v1_interface: wl_interface;
    pub static zwp_confined_pointer_v1_interface: wl_interface;
    pub static zwp_locked_pointer_v1_interface: wl_interface;

    // text-input-unstable-v3
    pub static zwp_text_input_manager_v3_interface: wl_interface;
    pub static zwp_text_input_v3_interface: wl_interface;

    // input-method-unstable-v2
    pub static zwp_input_method_manager_v2_interface: wl_interface;
    pub static zwp_input_method_v2_interface: wl_interface;
    pub static zwp_input_popup_surface_v2_interface: wl_interface;
    pub static zwp_input_method_keyboard_grab_v2_interface: wl_interface;

    // virtual-keyboard-unstable-v1
    pub static zwp_virtual_keyboard_manager_v1_interface: wl_interface;
    pub static zwp_virtual_keyboard_v1_interface: wl_interface;

    // fractional-scale-v1
    pub static wp_fractional_scale_manager_v1_interface: wl_interface;
    pub static wp_fractional_scale_v1_interface: wl_interface;

    // ext-session-lock-v1
    pub static ext_session_lock_manager_v1_interface: wl_interface;
    pub static ext_session_lock_v1_interface: wl_interface;
    pub static ext_session_lock_surface_v1_interface: wl_interface;

    // ext-idle-notify-v1
    pub static ext_idle_notifier_v1_interface: wl_interface;
    pub static ext_idle_notification_v1_interface: wl_interface;

    // ext-foreign-toplevel-list-v1
    pub static ext_foreign_toplevel_list_v1_interface: wl_interface;
    pub static ext_foreign_toplevel_handle_v1_interface: wl_interface;

    // ext-data-control-v1
    pub static ext_data_control_manager_v1_interface: wl_interface;
    pub static ext_data_control_device_v1_interface: wl_interface;
    pub static ext_data_control_source_v1_interface: wl_interface;
    pub static ext_data_control_offer_v1_interface: wl_interface;

    // cursor-shape-v1
    pub static wp_cursor_shape_manager_v1_interface: wl_interface;
    pub static wp_cursor_shape_device_v1_interface: wl_interface;

    // xdg-activation-v1
    pub static xdg_activation_v1_interface: wl_interface;
    pub static xdg_activation_token_v1_interface: wl_interface;

    // color-management-v1
    pub static wp_color_manager_v1_interface: wl_interface;
    pub static wp_color_management_output_v1_interface: wl_interface;
    pub static wp_color_management_surface_v1_interface: wl_interface;
    pub static wp_color_management_surface_feedback_v1_interface: wl_interface;
    pub static wp_image_description_creator_icc_v1_interface: wl_interface;
    pub static wp_image_description_creator_params_v1_interface: wl_interface;
    pub static wp_image_description_v1_interface: wl_interface;
    pub static wp_image_description_info_v1_interface: wl_interface;
    pub static wp_image_description_reference_v1_interface: wl_interface;
}
