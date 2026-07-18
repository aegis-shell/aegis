//! Raw FFI to libwayland-server. The unsafe seam; safe orchestration lives in
//! the parent module.
//
// This module mirrors the complete libwayland-server ABI surface (opcodes,
// interface tables, extern fns, core ABI types). Not every symbol is wired
// into a caller yet — they are kept complete so wiring the next server-side
// request is local. The broad allows below reflect that this is a binding
// module, not application code.
#![allow(
    non_camel_case_types,
    non_upper_case_globals,
    dead_code,
    unused_imports
)]

use std::os::raw::{c_char, c_int, c_void};

// Canonical protocol ABI types and the xdg-shell interface tables come from the
// shared protocols crate.
pub use ass_protocols::{wl_array, wl_interface, wl_message};
pub use ass_protocols::{wp_viewport_interface, wp_viewporter_interface};
pub use ass_protocols::{
    xdg_popup_interface, xdg_positioner_interface, xdg_surface_interface, xdg_toplevel_interface,
    xdg_wm_base_interface,
};
pub use ass_protocols::{zwp_linux_buffer_params_v1_interface, zwp_linux_dmabuf_v1_interface};
pub use ass_protocols::{
    zwp_tablet_manager_v2_interface, zwp_tablet_pad_group_v2_interface,
    zwp_tablet_pad_ring_v2_interface, zwp_tablet_pad_strip_v2_interface,
    zwp_tablet_pad_v2_interface, zwp_tablet_seat_v2_interface, zwp_tablet_tool_v2_interface,
    zwp_tablet_v2_interface,
};

// ----- extension protocol interface tables -----
pub use ass_protocols::{
    ext_data_control_device_v1_interface, ext_data_control_manager_v1_interface,
    ext_data_control_offer_v1_interface, ext_data_control_source_v1_interface,
    ext_foreign_toplevel_handle_v1_interface, ext_foreign_toplevel_list_v1_interface,
    ext_idle_notification_v1_interface, ext_idle_notifier_v1_interface,
    ext_session_lock_manager_v1_interface, ext_session_lock_surface_v1_interface,
    ext_session_lock_v1_interface, wp_cursor_shape_device_v1_interface,
    wp_cursor_shape_manager_v1_interface, wp_fractional_scale_manager_v1_interface,
    wp_fractional_scale_v1_interface, wp_presentation_feedback_interface,
    wp_presentation_interface, xdg_activation_token_v1_interface, xdg_activation_v1_interface,
    zwp_confined_pointer_v1_interface, zwp_idle_inhibit_manager_v1_interface,
    zwp_idle_inhibitor_v1_interface, zwp_keyboard_shortcuts_inhibit_manager_v1_interface,
    zwp_keyboard_shortcuts_inhibitor_v1_interface, zwp_locked_pointer_v1_interface,
    zwp_pointer_constraints_v1_interface, zwp_pointer_gesture_hold_v1_interface,
    zwp_pointer_gesture_pinch_v1_interface, zwp_pointer_gesture_swipe_v1_interface,
    zwp_pointer_gestures_v1_interface, zwp_primary_selection_device_manager_v1_interface,
    zwp_primary_selection_device_v1_interface, zwp_primary_selection_offer_v1_interface,
    zwp_primary_selection_source_v1_interface, zwp_relative_pointer_manager_v1_interface,
    zwp_relative_pointer_v1_interface, zwp_text_input_manager_v3_interface,
    zwp_text_input_v3_interface, zxdg_decoration_manager_v1_interface,
    zxdg_output_manager_v1_interface, zxdg_output_v1_interface,
    zxdg_toplevel_decoration_v1_interface,
};
pub use ass_protocols::{
    zwp_linux_buffer_release_v1_interface, zwp_linux_explicit_synchronization_v1_interface,
    zwp_linux_surface_synchronization_v1_interface,
};

pub type wl_display = c_void;
pub type wl_client = c_void;
pub type wl_resource = c_void;
pub type wl_global = c_void;
pub type wl_event_loop = c_void;
pub type wl_shm_buffer = c_void;

#[repr(C)]
pub struct wl_list {
    pub prev: *mut wl_list,
    pub next: *mut wl_list,
}

pub type wl_notify_func_t =
    Option<unsafe extern "C" fn(listener: *mut wl_listener, data: *mut c_void)>;

#[repr(C)]
pub struct wl_listener {
    pub link: wl_list,
    pub notify: wl_notify_func_t,
}

/// Global bind callback: `void (*)(wl_client*, void *data, uint32_t version, uint32_t id)`.
pub type wl_global_bind_func = unsafe extern "C" fn(*mut wl_client, *mut c_void, u32, u32);
pub type wl_display_global_filter_func = Option<
    unsafe extern "C" fn(
        client: *const wl_client,
        global: *const wl_global,
        data: *mut c_void,
    ) -> bool,
>;

/// Resource destroy callback: `void (*)(wl_resource*)`.
pub type wl_resource_destroy_func = unsafe extern "C" fn(*mut wl_resource);

// Event opcodes for the events the server sends.
pub const WL_OUTPUT_GEOMETRY: u32 = 0;
pub const WL_OUTPUT_MODE: u32 = 1;
pub const WL_OUTPUT_DONE: u32 = 2;
pub const WL_OUTPUT_SCALE: u32 = 3;
pub const WL_OUTPUT_NAME: u32 = 4;
pub const WL_OUTPUT_DESCRIPTION: u32 = 5;
pub const WL_OUTPUT_MODE_CURRENT: u32 = 0x1;
pub const WL_SURFACE_ENTER: u32 = 0;
pub const WL_SURFACE_LEAVE: u32 = 1;
pub const WL_CALLBACK_DONE: u32 = 0;
pub const WL_BUFFER_RELEASE: u32 = 0;

/// `wl_data_device` event opcodes.
pub const WL_DATA_DEVICE_DATA_OFFER: u32 = 0;
pub const WL_DATA_DEVICE_ENTER: u32 = 1;
pub const WL_DATA_DEVICE_LEAVE: u32 = 2;
pub const WL_DATA_DEVICE_MOTION: u32 = 3;
pub const WL_DATA_DEVICE_DROP: u32 = 4;
pub const WL_DATA_DEVICE_SELECTION: u32 = 5;
/// `wl_data_offer` event opcodes.
pub const WL_DATA_OFFER_OFFER: u32 = 0;
pub const WL_DATA_OFFER_SOURCE_ACTIONS: u32 = 1;
pub const WL_DATA_OFFER_ACTION: u32 = 2;
/// `wl_data_source` event opcodes.
pub const WL_DATA_SOURCE_TARGET: u32 = 0;
pub const WL_DATA_SOURCE_SEND: u32 = 1;
pub const WL_DATA_SOURCE_CANCELLED: u32 = 2;
pub const WL_DATA_SOURCE_DND_DROP_PERFORMED: u32 = 3;
pub const WL_DATA_SOURCE_DND_FINISHED: u32 = 4;
pub const WL_DATA_SOURCE_ACTION: u32 = 5;
pub const WL_DATA_ACTION_NONE: u32 = 0;
pub const WL_DATA_ACTION_COPY: u32 = 1;
pub const WL_DATA_ACTION_MOVE: u32 = 2;
pub const WL_DATA_ACTION_ASK: u32 = 4;
pub const WL_DATA_ACTION_MASK: u32 = 7;
pub const WL_DATA_SOURCE_ERROR_INVALID_ACTION_MASK: u32 = 0;
pub const WL_DATA_SOURCE_ERROR_INVALID_SOURCE: u32 = 1;
pub const WL_DATA_OFFER_ERROR_INVALID_FINISH: u32 = 0;
pub const WL_DATA_OFFER_ERROR_INVALID_ACTION_MASK: u32 = 1;
pub const WL_DATA_OFFER_ERROR_INVALID_ACTION: u32 = 2;
pub const WL_DATA_OFFER_ERROR_INVALID_OFFER: u32 = 3;
pub const XDG_SURFACE_CONFIGURE: u32 = 0;
pub const XDG_TOPLEVEL_CONFIGURE: u32 = 0;
pub const XDG_TOPLEVEL_CLOSE: u32 = 1;
pub const XDG_WM_BASE_PING: u32 = 0;
/// `xdg_popup` event opcodes.
pub const XDG_POPUP_CONFIGURE: u32 = 0;
pub const XDG_POPUP_POPUP_DONE: u32 = 1;

/// `zxdg_output_v1` event opcodes.
/// `logical_position`/`logical_size`/`name`/`description` are sent on first
/// bind and whenever the output's logical extents change; `done` (added in
/// v2) flushes a batch of changes so clients apply them atomically.
pub const ZXDG_OUTPUT_V1_LOGICAL_POSITION: u32 = 0;
pub const ZXDG_OUTPUT_V1_LOGICAL_SIZE: u32 = 1;
pub const ZXDG_OUTPUT_V1_DONE: u32 = 2;
pub const ZXDG_OUTPUT_V1_NAME: u32 = 3;
pub const ZXDG_OUTPUT_V1_DESCRIPTION: u32 = 4;
pub const ZXDG_TOPLEVEL_DECORATION_V1_CONFIGURE: u32 = 0;
pub const ZXDG_TOPLEVEL_DECORATION_V1_MODE_CLIENT_SIDE: u32 = 1;
pub const ZXDG_TOPLEVEL_DECORATION_V1_MODE_SERVER_SIDE: u32 = 2;
pub const ZXDG_TOPLEVEL_DECORATION_V1_ERROR_ALREADY_CONSTRUCTED: u32 = 1;
pub const ZXDG_TOPLEVEL_DECORATION_V1_ERROR_ORPHANED: u32 = 2;
pub const ZXDG_TOPLEVEL_DECORATION_V1_ERROR_INVALID_MODE: u32 = 3;
pub const XDG_ACTIVATION_TOKEN_V1_DONE: u32 = 0;
pub const XDG_ACTIVATION_TOKEN_V1_ERROR_ALREADY_USED: u32 = 0;
pub const WL_SEAT_CAPABILITIES: u32 = 0;
pub const WL_SEAT_NAME: u32 = 1;
/// `wl_seat.capability.pointer` bit.
pub const WL_SEAT_CAPABILITY_POINTER: u32 = 1;
/// `wl_seat.capability.keyboard` bit.
pub const WL_SEAT_CAPABILITY_KEYBOARD: u32 = 2;
/// `wl_seat.capability.touch` bit.
pub const WL_SEAT_CAPABILITY_TOUCH: u32 = 4;

/// `wl_pointer` event opcodes.
pub const WL_POINTER_ENTER: u32 = 0;
pub const WL_POINTER_LEAVE: u32 = 1;
pub const WL_POINTER_MOTION: u32 = 2;
pub const WL_POINTER_BUTTON: u32 = 3;
pub const WL_POINTER_AXIS: u32 = 4;
/// `wl_pointer.frame` (v5+).
pub const WL_POINTER_FRAME: u32 = 5;
/// `wl_pointer.axis_source` (v5+).
pub const WL_POINTER_AXIS_SOURCE: u32 = 6;
/// `wl_pointer.axis_stop` (v5+).
pub const WL_POINTER_AXIS_STOP: u32 = 7;
/// `wl_pointer.axis_discrete` (v5+).
pub const WL_POINTER_AXIS_DISCRETE: u32 = 8;
/// `wl_pointer.axis_value120` (v8+).
pub const WL_POINTER_AXIS_VALUE120: u32 = 9;
/// `wl_pointer.axis_relative_direction` (v9+).
pub const WL_POINTER_AXIS_RELATIVE_DIRECTION: u32 = 10;

/// `wl_pointer.axis` axis enum values.
pub const WL_POINTER_AXIS_VERTICAL_SCROLL: u32 = 0;
pub const WL_POINTER_AXIS_HORIZONTAL_SCROLL: u32 = 1;
/// `wl_pointer.axis_source` enum values.
pub const WL_POINTER_AXIS_SOURCE_WHEEL: u32 = 0;
pub const WL_POINTER_AXIS_SOURCE_FINGER: u32 = 1;
pub const WL_POINTER_AXIS_SOURCE_CONTINUOUS: u32 = 2;
pub const WL_POINTER_AXIS_SOURCE_WHEEL_TILT: u32 = 3;
/// `wl_pointer.axis_relative_direction` enum values.
pub const WL_POINTER_AXIS_RELATIVE_DIRECTION_IDENTICAL: u32 = 0;
pub const WL_POINTER_AXIS_RELATIVE_DIRECTION_INVERTED: u32 = 1;

/// `wl_keyboard` event opcodes.
pub const WL_KEYBOARD_KEYMAP: u32 = 0;
pub const WL_KEYBOARD_ENTER: u32 = 1;
pub const WL_KEYBOARD_LEAVE: u32 = 2;
pub const WL_KEYBOARD_KEY: u32 = 3;
pub const WL_KEYBOARD_MODIFIERS: u32 = 4;
pub const WL_KEYBOARD_REPEAT_INFO: u32 = 5;

/// `wl_touch` event opcodes.
pub const WL_TOUCH_DOWN: u32 = 0;
pub const WL_TOUCH_UP: u32 = 1;
pub const WL_TOUCH_MOTION: u32 = 2;
pub const WL_TOUCH_FRAME: u32 = 3;
pub const WL_TOUCH_CANCEL: u32 = 4;
pub const WL_TOUCH_SHAPE: u32 = 5;
pub const WL_TOUCH_ORIENTATION: u32 = 6;

/// `wl_keyboard.keymap.format`: 0 = no keymap, 1 = xkb string in fd.
pub const WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1: u32 = 1;

// ----- extension protocol event opcodes -----

// zxdg_output_v1 opcodes (ZXDG_OUTPUT_V1_*) are declared above near the
// output section; reused by the extensions module.

// wp_fractional_scale_v1
pub const WP_FRACTIONAL_SCALE_V1_PREFERRED_SCALE: u32 = 0;

// zwp_relative_pointer_v1
pub const ZWP_RELATIVE_POINTER_V1_RELATIVE_MOTION: u32 = 0;

// zwp_pointer_gesture_{swipe,pinch,hold}_v1
pub const ZWP_POINTER_GESTURE_SWIPE_V1_BEGIN: u32 = 0;
pub const ZWP_POINTER_GESTURE_SWIPE_V1_UPDATE: u32 = 1;
pub const ZWP_POINTER_GESTURE_SWIPE_V1_END: u32 = 2;
pub const ZWP_POINTER_GESTURE_PINCH_V1_BEGIN: u32 = 0;
pub const ZWP_POINTER_GESTURE_PINCH_V1_UPDATE: u32 = 1;
pub const ZWP_POINTER_GESTURE_PINCH_V1_END: u32 = 2;
pub const ZWP_POINTER_GESTURE_HOLD_V1_BEGIN: u32 = 0;
pub const ZWP_POINTER_GESTURE_HOLD_V1_END: u32 = 1;

// keyboard-shortcuts-inhibit-unstable-v1
pub const ZWP_KEYBOARD_SHORTCUTS_INHIBITOR_V1_ACTIVE: u32 = 0;
pub const ZWP_KEYBOARD_SHORTCUTS_INHIBITOR_V1_INACTIVE: u32 = 1;
pub const ZWP_KEYBOARD_SHORTCUTS_INHIBIT_MANAGER_V1_ERROR_ALREADY_INHIBITED: u32 = 0;

// zwp_confined_pointer_v1
pub const ZWP_CONFINED_POINTER_V1_CONFINED: u32 = 0;
pub const ZWP_CONFINED_POINTER_V1_UNCONFINED: u32 = 1;

// zwp_locked_pointer_v1
pub const ZWP_LOCKED_POINTER_V1_LOCKED: u32 = 0;
pub const ZWP_LOCKED_POINTER_V1_UNLOCKED: u32 = 1;

// ext_session_lock_v1
pub const EXT_SESSION_LOCK_V1_LOCKED: u32 = 0;
pub const EXT_SESSION_LOCK_V1_FINISHED: u32 = 1;
// ext_session_lock_surface_v1
pub const EXT_SESSION_LOCK_SURFACE_V1_CONFIGURE: u32 = 0;

// ext_idle_notification_v1
pub const EXT_IDLE_NOTIFICATION_V1_IDLED: u32 = 0;
pub const EXT_IDLE_NOTIFICATION_V1_RESUMED: u32 = 1;

// ext_foreign_toplevel_list_v1
pub const EXT_FOREIGN_TOPLEVEL_LIST_V1_TOPLEVEL: u32 = 0;
pub const EXT_FOREIGN_TOPLEVEL_LIST_V1_FINISHED: u32 = 1;
// ext_foreign_toplevel_handle_v1
pub const EXT_FOREIGN_TOPLEVEL_HANDLE_V1_CLOSED: u32 = 0;
pub const EXT_FOREIGN_TOPLEVEL_HANDLE_V1_DONE: u32 = 1;
pub const EXT_FOREIGN_TOPLEVEL_HANDLE_V1_TITLE: u32 = 2;
pub const EXT_FOREIGN_TOPLEVEL_HANDLE_V1_APP_ID: u32 = 3;
pub const EXT_FOREIGN_TOPLEVEL_HANDLE_V1_IDENTIFIER: u32 = 4;

// ext_data_control_device_v1
pub const EXT_DATA_CONTROL_DEVICE_V1_DATA_OFFER: u32 = 0;
pub const EXT_DATA_CONTROL_DEVICE_V1_SELECTION: u32 = 1;
pub const EXT_DATA_CONTROL_DEVICE_V1_FINISHED: u32 = 2;
pub const EXT_DATA_CONTROL_DEVICE_V1_PRIMARY_SELECTION: u32 = 3;
// ext_data_control_offer_v1
pub const EXT_DATA_CONTROL_OFFER_V1_OFFER: u32 = 0;
// ext_data_control_source_v1
pub const EXT_DATA_CONTROL_SOURCE_V1_SEND: u32 = 0;
pub const EXT_DATA_CONTROL_SOURCE_V1_CANCELLED: u32 = 1;

// zwp_primary_selection_device_v1
pub const ZWP_PRIMARY_SELECTION_DEVICE_V1_DATA_OFFER: u32 = 0;
pub const ZWP_PRIMARY_SELECTION_DEVICE_V1_SELECTION: u32 = 1;
// zwp_primary_selection_offer_v1
pub const ZWP_PRIMARY_SELECTION_OFFER_V1_OFFER: u32 = 0;
// zwp_primary_selection_source_v1
pub const ZWP_PRIMARY_SELECTION_SOURCE_V1_SEND: u32 = 0;
pub const ZWP_PRIMARY_SELECTION_SOURCE_V1_CANCELLED: u32 = 1;

// wp_presentation_feedback (bitfield flags, but event opcode is 0)
pub const WP_PRESENTATION_FEEDBACK_PRESENTED: u32 = 0;
pub const WP_PRESENTATION_FEEDBACK_DISCARDED: u32 = 1;

// zwp_text_input_v3
pub const ZWP_TEXT_INPUT_V3_ENTER: u32 = 0;
pub const ZWP_TEXT_INPUT_V3_LEAVE: u32 = 1;
pub const ZWP_TEXT_INPUT_V3_PREEDIT_STRING: u32 = 2;
pub const ZWP_TEXT_INPUT_V3_COMMIT_STRING: u32 = 3;
pub const ZWP_TEXT_INPUT_V3_DELETE_SURROUNDING_TEXT: u32 = 4;
pub const ZWP_TEXT_INPUT_V3_DONE: u32 = 5;

/// Convert a compositor-space `f32` to a `wl_fixed_t` (24.8) for event posting.
pub fn wl_fixed_from_f32(v: f32) -> i32 {
    (v * 256.0).round() as i32
}
pub const ZWP_LINUX_DMABUF_V1_FORMAT: u32 = 0;
pub const ZWP_LINUX_DMABUF_V1_MODIFIER: u32 = 1;
pub const ZWP_LINUX_BUFFER_PARAMS_V1_CREATED: u32 = 0;
pub const ZWP_LINUX_BUFFER_PARAMS_V1_FAILED: u32 = 1;
/// `zwp_linux_buffer_params_v1.error.invalid_wl_buffer` (protocol enum value 7):
/// fatal for `create_immed` when the params object cannot yield a valid buffer.
pub const ZWP_LINUX_BUFFER_PARAMS_V1_ERROR_INVALID_WL_BUFFER: u32 = 7;
pub const ZWP_LINUX_BUFFER_RELEASE_V1_FENCED_RELEASE: u32 = 0;
pub const ZWP_LINUX_BUFFER_RELEASE_V1_IMMEDIATE_RELEASE: u32 = 1;

pub const ZWP_TABLET_SEAT_V2_TABLET_ADDED: u32 = 0;
pub const ZWP_TABLET_SEAT_V2_TOOL_ADDED: u32 = 1;
pub const ZWP_TABLET_V2_NAME: u32 = 0;
pub const ZWP_TABLET_V2_ID: u32 = 1;
pub const ZWP_TABLET_V2_DONE: u32 = 3;
pub const ZWP_TABLET_TOOL_V2_TYPE: u32 = 0;
pub const ZWP_TABLET_TOOL_V2_HARDWARE_SERIAL: u32 = 1;
pub const ZWP_TABLET_TOOL_V2_HARDWARE_ID_WACOM: u32 = 2;
pub const ZWP_TABLET_TOOL_V2_CAPABILITY: u32 = 3;
pub const ZWP_TABLET_TOOL_V2_DONE: u32 = 4;
pub const ZWP_TABLET_TOOL_V2_PROXIMITY_IN: u32 = 6;
pub const ZWP_TABLET_TOOL_V2_PROXIMITY_OUT: u32 = 7;
pub const ZWP_TABLET_TOOL_V2_DOWN: u32 = 8;
pub const ZWP_TABLET_TOOL_V2_UP: u32 = 9;
pub const ZWP_TABLET_TOOL_V2_MOTION: u32 = 10;
pub const ZWP_TABLET_TOOL_V2_PRESSURE: u32 = 11;
pub const ZWP_TABLET_TOOL_V2_DISTANCE: u32 = 12;
pub const ZWP_TABLET_TOOL_V2_TILT: u32 = 13;
pub const ZWP_TABLET_TOOL_V2_ROTATION: u32 = 14;
pub const ZWP_TABLET_TOOL_V2_SLIDER: u32 = 15;
pub const ZWP_TABLET_TOOL_V2_WHEEL: u32 = 16;
pub const ZWP_TABLET_TOOL_V2_BUTTON: u32 = 17;
pub const ZWP_TABLET_TOOL_V2_FRAME: u32 = 18;

extern "C" {
    // Core interface tables (libwayland-server).
    pub static wl_compositor_interface: wl_interface;
    pub static wl_surface_interface: wl_interface;
    pub static wl_region_interface: wl_interface;
    pub static wl_callback_interface: wl_interface;
    pub static wl_output_interface: wl_interface;
    pub static wl_subcompositor_interface: wl_interface;
    pub static wl_subsurface_interface: wl_interface;
    pub static wl_seat_interface: wl_interface;
    pub static wl_pointer_interface: wl_interface;
    pub static wl_keyboard_interface: wl_interface;
    pub static wl_touch_interface: wl_interface;
    pub static wl_data_device_manager_interface: wl_interface;
    pub static wl_data_device_interface: wl_interface;
    pub static wl_data_source_interface: wl_interface;
    pub static wl_data_offer_interface: wl_interface;
    pub static wl_buffer_interface: wl_interface;

    // Display + event loop.
    pub fn wl_display_create() -> *mut wl_display;
    pub fn wl_display_destroy(display: *mut wl_display);
    pub fn wl_display_add_socket_auto(display: *mut wl_display) -> *const c_char;
    pub fn wl_display_init_shm(display: *mut wl_display) -> c_int;
    pub fn wl_display_next_serial(display: *mut wl_display) -> u32;
    pub fn wl_display_get_event_loop(display: *mut wl_display) -> *mut wl_event_loop;
    pub fn wl_display_flush_clients(display: *mut wl_display);
    pub fn wl_display_set_global_filter(
        display: *mut wl_display,
        filter: wl_display_global_filter_func,
        data: *mut c_void,
    );
    pub fn wl_event_loop_dispatch(loop_: *mut wl_event_loop, timeout: c_int) -> c_int;
    pub fn wl_event_loop_get_fd(loop_: *mut wl_event_loop) -> c_int;

    // Globals + resources.
    pub fn wl_global_create(
        display: *mut wl_display,
        interface: *const wl_interface,
        version: c_int,
        data: *mut c_void,
        bind: wl_global_bind_func,
    ) -> *mut wl_global;
    pub fn wl_global_destroy(global: *mut wl_global);
    pub fn wl_client_create(display: *mut wl_display, fd: c_int) -> *mut wl_client;
    pub fn wl_client_destroy(client: *mut wl_client);
    pub fn wl_client_add_destroy_listener(client: *mut wl_client, listener: *mut wl_listener);

    pub fn wl_resource_create(
        client: *mut wl_client,
        interface: *const wl_interface,
        version: c_int,
        id: u32,
    ) -> *mut wl_resource;
    pub fn wl_resource_set_implementation(
        resource: *mut wl_resource,
        implementation: *const c_void,
        data: *mut c_void,
        destroy: Option<wl_resource_destroy_func>,
    );
    pub fn wl_resource_post_event(resource: *mut wl_resource, opcode: u32, ...);
    pub fn wl_resource_post_error(
        resource: *mut wl_resource,
        code: u32,
        message: *const c_char,
        ...
    );
    pub fn wl_resource_destroy(resource: *mut wl_resource);
    pub fn wl_resource_get_user_data(resource: *mut wl_resource) -> *mut c_void;
    pub fn wl_resource_set_user_data(resource: *mut wl_resource, data: *mut c_void);
    pub fn wl_resource_get_version(resource: *mut wl_resource) -> c_int;
    pub fn wl_resource_get_id(resource: *mut wl_resource) -> u32;
    pub fn wl_resource_get_client(resource: *mut wl_resource) -> *mut wl_client;
    pub fn wl_resource_instance_of(
        resource: *mut wl_resource,
        interface: *const wl_interface,
        implementation: *const c_void,
    ) -> c_int;

    // shm buffer accessors.
    pub fn wl_shm_buffer_get(resource: *mut wl_resource) -> *mut wl_shm_buffer;
    pub fn wl_shm_buffer_get_data(buffer: *mut wl_shm_buffer) -> *mut c_void;
    pub fn wl_shm_buffer_get_width(buffer: *mut wl_shm_buffer) -> i32;
    pub fn wl_shm_buffer_get_height(buffer: *mut wl_shm_buffer) -> i32;
    pub fn wl_shm_buffer_get_stride(buffer: *mut wl_shm_buffer) -> i32;
    pub fn wl_shm_buffer_get_format(buffer: *mut wl_shm_buffer) -> u32;
    pub fn wl_shm_buffer_begin_access(buffer: *mut wl_shm_buffer);
    pub fn wl_shm_buffer_end_access(buffer: *mut wl_shm_buffer);
}

// ----- request implementation vtables ------------------------------------

/// `wl_compositor` requests: create_surface, create_region.
#[repr(C)]
pub struct wl_compositor_interface_impl {
    pub create_surface: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub create_region: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
}

/// `wl_surface` requests through version 4 (opcodes 0..=9).
#[repr(C)]
pub struct wl_surface_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub attach: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource, i32, i32),
    pub damage: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32, i32, i32, i32),
    pub frame: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub set_opaque_region: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource),
    pub set_input_region: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource),
    pub commit: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub set_buffer_transform: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32),
    pub set_buffer_scale: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32),
    pub damage_buffer: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32, i32, i32, i32),
}

/// `wl_region` requests: destroy, add, subtract.
#[repr(C)]
pub struct wl_region_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub add: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32, i32, i32, i32),
    pub subtract: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32, i32, i32, i32),
}

/// `xdg_wm_base` requests: destroy, create_positioner, get_xdg_surface, pong.
#[repr(C)]
pub struct xdg_wm_base_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub create_positioner: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub get_xdg_surface:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
    pub pong: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
}

/// `xdg_positioner` requests (v1): destroy, set_size, set_anchor_rect, set_anchor,
/// set_gravity, set_constraint_adjustment, set_offset.
#[repr(C)]
pub struct xdg_positioner_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub set_size: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32, i32),
    pub set_anchor_rect: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32, i32, i32, i32),
    pub set_anchor: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub set_gravity: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub set_constraint_adjustment: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub set_offset: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32, i32),
}

/// `xdg_popup` requests: destroy, grab. (configure and popup_done are events.)
#[repr(C)]
pub struct xdg_popup_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub grab: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource, u32),
}

/// `xdg_surface` requests: destroy, get_toplevel, get_popup, set_window_geometry,
/// ack_configure.
#[repr(C)]
pub struct xdg_surface_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub get_toplevel: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub get_popup: unsafe extern "C" fn(
        *mut wl_client,
        *mut wl_resource,
        u32,
        *mut wl_resource,
        *mut wl_resource,
    ),
    pub set_window_geometry:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32, i32, i32, i32),
    pub ack_configure: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
}

/// `wl_subcompositor` requests: destroy, get_subsurface.
#[repr(C)]
pub struct wl_subcompositor_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub get_subsurface: unsafe extern "C" fn(
        *mut wl_client,
        *mut wl_resource,
        u32,
        *mut wl_resource,
        *mut wl_resource,
    ),
}

/// `wl_subsurface` requests: destroy, set_position, place_above, place_below,
/// set_sync, set_desync.
#[repr(C)]
pub struct wl_subsurface_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub set_position: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32, i32),
    pub place_above: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource),
    pub place_below: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource),
    pub set_sync: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub set_desync: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `wl_output` v3+: release.
#[repr(C)]
pub struct wl_output_interface_impl {
    pub release: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `wl_data_device_manager` requests through version 3.
#[repr(C)]
pub struct wl_data_device_manager_interface_impl {
    pub create_data_source: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub get_data_device:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
}

/// `wl_data_device` v1 requests: start_drag, set_selection.
#[repr(C)]
pub struct wl_data_device_interface_impl {
    pub start_drag: unsafe extern "C" fn(
        *mut wl_client,
        *mut wl_resource,
        *mut wl_resource,
        *mut wl_resource,
        *mut wl_resource,
        u32,
    ),
    pub set_selection:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource, u32),
}

/// `wl_data_source` v3 requests: offer, destroy, set_actions.
#[repr(C)]
pub struct wl_data_source_interface_impl {
    pub offer: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *const c_char),
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub set_actions: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
}

/// `wl_data_offer` v3 requests: accept, receive, destroy, finish, set_actions.
#[repr(C)]
pub struct wl_data_offer_interface_impl {
    pub accept: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *const c_char),
    pub receive: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *const c_char, i32),
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub finish: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub set_actions: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, u32),
}

/// `wl_seat` requests: get_pointer, get_keyboard, get_touch, release.
#[repr(C)]
pub struct wl_seat_interface_impl {
    pub get_pointer: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub get_keyboard: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub get_touch: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub release: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `wl_pointer` requests: set_cursor (v1), release (v3).
/// Both must be implemented: `set_cursor` is a regular request every client
/// sends when changing its cursor, and libwayland aborts with "Implementation
/// of resource N of wl_pointer is NULL" if the function pointer is NULL.
#[repr(C)]
pub struct wl_pointer_interface_impl {
    pub set_cursor:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource, i32, i32),
    pub release: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `wl_keyboard` requests: release (v3).
#[repr(C)]
pub struct wl_keyboard_interface_impl {
    pub release: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `wl_touch` requests: release (v3).
#[repr(C)]
pub struct wl_touch_interface_impl {
    pub release: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `xdg_toplevel` requests (xdg_wm_base v1): 14 entries in protocol order.
#[repr(C)]
pub struct xdg_toplevel_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub set_parent: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource),
    pub set_title: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *const c_char),
    pub set_app_id: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *const c_char),
    pub show_window_menu:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource, u32, i32, i32),
    pub move_: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource, u32),
    pub resize: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource, u32, u32),
    pub set_max_size: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32, i32),
    pub set_min_size: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32, i32),
    pub set_maximized: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub unset_maximized: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub set_fullscreen: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource),
    pub unset_fullscreen: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub set_minimized: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `zwp_linux_dmabuf_v1` requests. The interface (v5) declares four: destroy,
/// create_params, get_default_feedback, get_surface_feedback. We bind at v3 so
/// only the first two are reachable; the feedback entries are inert.
#[repr(C)]
pub struct zwp_linux_dmabuf_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub create_params: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub get_default_feedback: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub get_surface_feedback:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
}

/// `zwp_linux_buffer_params_v1` requests: destroy, add, create, create_immed.
#[repr(C)]
pub struct zwp_linux_buffer_params_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub add: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32, u32, u32, u32, u32, u32),
    pub create: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32, i32, u32, u32),
    pub create_immed:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, i32, i32, u32, u32),
}

/// `wl_buffer` requests: destroy.
#[repr(C)]
pub struct wl_buffer_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `wp_viewporter` requests: destroy, get_viewport.
#[repr(C)]
pub struct wp_viewporter_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub get_viewport: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
}

/// `wp_viewport` requests: destroy, set_source, set_destination.
/// set_source uses 24.8 fixed-point (`wl_fixed_t` = i32).
#[repr(C)]
pub struct wp_viewport_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub set_source: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32, i32, i32, i32),
    pub set_destination: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32, i32),
}

// ----- vtable sizing discipline ------------------------------------------
//
// libwayland indexes an `*_interface_impl` struct by request opcode: a request
// with opcode N reads `impl[N]`. An under-sized struct therefore causes an
// out-of-bounds read on the highest opcodes the protocol advertises (which
// depends on the version we bind). Assert here, at compile time, that every
// impl struct has exactly as many function-pointer slots as the protocol
// defines for the version we bind — so adding a request to the protocol XML
// without adding a slot here becomes a hard build failure, not a latent UB.
//
// Sizes are function-pointer count; `*const ()` is one pointer slot wide.

/// Compile-time assert that an `*_interface_impl` struct carries exactly the
/// expected number of function-pointer entries. `count` is the number of
/// opcodes the protocol advertises for the version we bind.
macro_rules! assert_impl_opcode_count {
    ($ty:ty, $count:expr) => {
        const _: () = {
            assert!(std::mem::size_of::<$ty>() == $count * std::mem::size_of::<*const ()>(),);
        };
    };
}

// Opcode counts (request side) come from the protocol XML at the version we
// bind; they must not change without bumping the bind version and adding a
// matching slot to the impl struct.
assert_impl_opcode_count!(wl_compositor_interface_impl, 2);
assert_impl_opcode_count!(wl_surface_interface_impl, 10);
assert_impl_opcode_count!(wl_region_interface_impl, 3);
assert_impl_opcode_count!(xdg_wm_base_interface_impl, 4);
assert_impl_opcode_count!(xdg_positioner_interface_impl, 7);
assert_impl_opcode_count!(xdg_popup_interface_impl, 2);
assert_impl_opcode_count!(xdg_surface_interface_impl, 5);
assert_impl_opcode_count!(wl_subcompositor_interface_impl, 2);
assert_impl_opcode_count!(wl_subsurface_interface_impl, 6);
assert_impl_opcode_count!(wl_data_device_manager_interface_impl, 2);
assert_impl_opcode_count!(wl_data_device_interface_impl, 2);
assert_impl_opcode_count!(wl_data_source_interface_impl, 3);
assert_impl_opcode_count!(wl_data_offer_interface_impl, 5);
assert_impl_opcode_count!(wl_seat_interface_impl, 4);
assert_impl_opcode_count!(wl_pointer_interface_impl, 2);
assert_impl_opcode_count!(wl_keyboard_interface_impl, 1);
assert_impl_opcode_count!(wl_touch_interface_impl, 1);
assert_impl_opcode_count!(xdg_toplevel_interface_impl, 14);
assert_impl_opcode_count!(zwp_linux_dmabuf_v1_interface_impl, 4);
assert_impl_opcode_count!(zwp_linux_buffer_params_v1_interface_impl, 4);
assert_impl_opcode_count!(wl_buffer_interface_impl, 1);
assert_impl_opcode_count!(wp_viewporter_interface_impl, 2);
assert_impl_opcode_count!(wp_viewport_interface_impl, 3);

// ----- extension protocol request vtables ---------------------------------
//
// libwayland indexes each `*_interface_impl` struct by request opcode, so the
// struct must carry exactly as many function-pointer slots as the protocol
// advertises requests. The asserts at the bottom enforce this. Request
// signatures come verbatim from the protocol XML; argument types are the C ABI
// (object → *mut wl_resource, new_id → u32, fixed → i32, etc.).

/// `zxdg_output_manager_v1`: destroy, get_xdg_output.
#[repr(C)]
pub struct zxdg_output_manager_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub get_xdg_output:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
}

/// `zxdg_output_v1`: destroy.
#[repr(C)]
pub struct zxdg_output_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `zxdg_decoration_manager_v1`: destroy, get_toplevel_decoration.
#[repr(C)]
pub struct zxdg_decoration_manager_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub get_toplevel_decoration:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
}

/// `zxdg_toplevel_decoration_v1`: destroy, set_mode, unset_mode.
#[repr(C)]
pub struct zxdg_toplevel_decoration_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub set_mode: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub unset_mode: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `xdg_activation_v1`: destroy, get_activation_token, activate.
#[repr(C)]
pub struct xdg_activation_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub get_activation_token: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub activate:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *const c_char, *mut wl_resource),
}

/// `xdg_activation_token_v1`: set_serial, set_app_id, set_surface, commit,
/// destroy.
#[repr(C)]
pub struct xdg_activation_token_v1_interface_impl {
    pub set_serial: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
    pub set_app_id: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *const c_char),
    pub set_surface: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource),
    pub commit: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `wp_presentation`: destroy, feedback.
#[repr(C)]
pub struct wp_presentation_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub feedback: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource, u32),
}

/// `wp_fractional_scale_manager_v1`: destroy, get_fractional_scale.
#[repr(C)]
pub struct wp_fractional_scale_manager_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub get_fractional_scale:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
}

/// `wp_fractional_scale_v1`: destroy.
#[repr(C)]
pub struct wp_fractional_scale_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `zwp_idle_inhibit_manager_v1`: destroy, create_inhibitor.
#[repr(C)]
pub struct zwp_idle_inhibit_manager_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub create_inhibitor:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
}

/// `zwp_idle_inhibitor_v1`: destroy.
#[repr(C)]
pub struct zwp_idle_inhibitor_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `ext_idle_notifier_v1`: destroy, get_idle_notification, get_input_idle_notification.
#[repr(C)]
pub struct ext_idle_notifier_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub get_idle_notification:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, u32, *mut wl_resource),
    pub get_input_idle_notification:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, u32, *mut wl_resource),
}

/// `ext_idle_notification_v1`: destroy.
#[repr(C)]
pub struct ext_idle_notification_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `zwp_relative_pointer_manager_v1`: destroy, get_relative_pointer.
#[repr(C)]
pub struct zwp_relative_pointer_manager_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub get_relative_pointer:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
}

/// `zwp_relative_pointer_v1`: destroy.
#[repr(C)]
pub struct zwp_relative_pointer_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `zwp_pointer_gestures_v1`: destroy and create swipe/pinch/hold objects.
#[repr(C)]
pub struct zwp_pointer_gestures_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub get_swipe_gesture:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
    pub get_pinch_gesture:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
    pub get_hold_gesture:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
}

#[repr(C)]
pub struct zwp_pointer_gesture_swipe_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

#[repr(C)]
pub struct zwp_pointer_gesture_pinch_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

#[repr(C)]
pub struct zwp_pointer_gesture_hold_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `zwp_keyboard_shortcuts_inhibit_manager_v1`: destroy, inhibit_shortcuts.
#[repr(C)]
pub struct zwp_keyboard_shortcuts_inhibit_manager_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub inhibit_shortcuts: unsafe extern "C" fn(
        *mut wl_client,
        *mut wl_resource,
        u32,
        *mut wl_resource,
        *mut wl_resource,
    ),
}

/// `zwp_keyboard_shortcuts_inhibitor_v1`: destroy.
#[repr(C)]
pub struct zwp_keyboard_shortcuts_inhibitor_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `zwp_pointer_constraints_v1`: destroy, lock_pointer, confine_pointer.
#[repr(C)]
pub struct zwp_pointer_constraints_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub lock_pointer: unsafe extern "C" fn(
        *mut wl_client,
        *mut wl_resource,
        u32,
        *mut wl_resource,
        *mut wl_resource,
        *mut wl_resource,
        u32,
    ),
    pub confine_pointer: unsafe extern "C" fn(
        *mut wl_client,
        *mut wl_resource,
        u32,
        *mut wl_resource,
        *mut wl_resource,
        *mut wl_resource,
        u32,
    ),
}

/// `zwp_confined_pointer_v1`: destroy, set_region.
#[repr(C)]
pub struct zwp_confined_pointer_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub set_region: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource),
}

/// `zwp_locked_pointer_v1`: destroy, set_cursor_position_hint, set_region.
#[repr(C)]
pub struct zwp_locked_pointer_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub set_cursor_position_hint: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32, i32),
    pub set_region: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource),
}

/// `ext_session_lock_manager_v1`: destroy, lock.
#[repr(C)]
pub struct ext_session_lock_manager_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub lock: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
}

/// `ext_session_lock_v1`: destroy, get_lock_surface, unlock_and_destroy.
#[repr(C)]
pub struct ext_session_lock_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub get_lock_surface: unsafe extern "C" fn(
        *mut wl_client,
        *mut wl_resource,
        u32,
        *mut wl_resource,
        *mut wl_resource,
    ),
    pub unlock_and_destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `ext_session_lock_surface_v1`: destroy, ack_configure.
#[repr(C)]
pub struct ext_session_lock_surface_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub ack_configure: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
}

#[repr(C)]
pub struct zwp_linux_explicit_synchronization_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub get_synchronization:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
}

#[repr(C)]
pub struct zwp_linux_surface_synchronization_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub set_acquire_fence: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, c_int),
    pub get_release: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
}

#[repr(C)]
pub struct zwp_tablet_manager_v2_interface_impl {
    pub get_tablet_seat:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

#[repr(C)]
pub struct zwp_tablet_seat_v2_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

#[repr(C)]
pub struct zwp_tablet_v2_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

#[repr(C)]
pub struct zwp_tablet_tool_v2_interface_impl {
    pub set_cursor:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource, i32, i32),
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `ext_foreign_toplevel_list_v1`: stop, destroy.
#[repr(C)]
pub struct ext_foreign_toplevel_list_v1_interface_impl {
    pub stop: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `ext_foreign_toplevel_handle_v1`: destroy.
#[repr(C)]
pub struct ext_foreign_toplevel_handle_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `ext_data_control_manager_v1`: create_data_source, get_data_device, destroy.
#[repr(C)]
pub struct ext_data_control_manager_v1_interface_impl {
    pub create_data_source: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub get_data_device:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `ext_data_control_device_v1`: set_selection, destroy, set_primary_selection.
#[repr(C)]
pub struct ext_data_control_device_v1_interface_impl {
    pub set_selection: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource),
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub set_primary_selection:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource),
}

/// `ext_data_control_source_v1`: offer, destroy.
#[repr(C)]
pub struct ext_data_control_source_v1_interface_impl {
    pub offer: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *const c_char),
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `ext_data_control_offer_v1`: receive, destroy.
#[repr(C)]
pub struct ext_data_control_offer_v1_interface_impl {
    pub receive: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *const c_char, i32),
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `wp_cursor_shape_manager_v1`: destroy, get_pointer, get_tablet_tool_v2.
#[repr(C)]
pub struct wp_cursor_shape_manager_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub get_pointer: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
    pub get_tablet_tool_v2:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
}

/// `wp_cursor_shape_device_v1`: destroy, set_shape.
#[repr(C)]
pub struct wp_cursor_shape_device_v1_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub set_shape: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, u32),
}

/// `zwp_primary_selection_device_manager_v1`: create_source, get_device, destroy.
#[repr(C)]
pub struct zwp_primary_selection_device_manager_v1_interface_impl {
    pub create_source: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub get_data_device:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `zwp_primary_selection_device_v1`: set_selection, destroy.
#[repr(C)]
pub struct zwp_primary_selection_device_v1_interface_impl {
    pub set_selection:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *mut wl_resource, u32),
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `zwp_primary_selection_source_v1`: offer, destroy.
#[repr(C)]
pub struct zwp_primary_selection_source_v1_interface_impl {
    pub offer: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *const c_char),
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `zwp_primary_selection_offer_v1`: receive, destroy.
#[repr(C)]
pub struct zwp_primary_selection_offer_v1_interface_impl {
    pub receive: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *const c_char, i32),
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `zwp_text_input_manager_v3`: destroy, get_text_input.
#[repr(C)]
pub struct zwp_text_input_manager_v3_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub get_text_input:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
}

/// `zwp_text_input_v3` requests at v1 (8 opcodes). Opcode order: destroy,
/// enable, disable, set_surrounding_text, set_text_change_cause,
/// set_content_type, set_cursor_rectangle, commit. We bind at v1 so the three
/// v2-only requests (set_available_actions, show/hide_input_panel) are
/// unreachable.
#[repr(C)]
pub struct zwp_text_input_v3_interface_impl {
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub enable: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub disable: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
    pub set_surrounding_text:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, *const c_char, i32, i32),
    pub set_text_change_cause: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub set_content_type: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, u32),
    pub set_cursor_rectangle:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, i32, i32, i32, i32),
    pub commit: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

assert_impl_opcode_count!(zxdg_output_manager_v1_interface_impl, 2);
assert_impl_opcode_count!(zxdg_output_v1_interface_impl, 1);
assert_impl_opcode_count!(zxdg_decoration_manager_v1_interface_impl, 2);
assert_impl_opcode_count!(zxdg_toplevel_decoration_v1_interface_impl, 3);
assert_impl_opcode_count!(xdg_activation_v1_interface_impl, 3);
assert_impl_opcode_count!(xdg_activation_token_v1_interface_impl, 5);
assert_impl_opcode_count!(wp_presentation_interface_impl, 2);
assert_impl_opcode_count!(wp_fractional_scale_manager_v1_interface_impl, 2);
assert_impl_opcode_count!(wp_fractional_scale_v1_interface_impl, 1);
assert_impl_opcode_count!(zwp_idle_inhibit_manager_v1_interface_impl, 2);
assert_impl_opcode_count!(zwp_idle_inhibitor_v1_interface_impl, 1);
assert_impl_opcode_count!(ext_idle_notifier_v1_interface_impl, 3);
assert_impl_opcode_count!(ext_idle_notification_v1_interface_impl, 1);
assert_impl_opcode_count!(zwp_relative_pointer_manager_v1_interface_impl, 2);
assert_impl_opcode_count!(zwp_relative_pointer_v1_interface_impl, 1);
assert_impl_opcode_count!(zwp_pointer_gestures_v1_interface_impl, 4);
assert_impl_opcode_count!(zwp_pointer_gesture_swipe_v1_interface_impl, 1);
assert_impl_opcode_count!(zwp_pointer_gesture_pinch_v1_interface_impl, 1);
assert_impl_opcode_count!(zwp_pointer_gesture_hold_v1_interface_impl, 1);
assert_impl_opcode_count!(zwp_keyboard_shortcuts_inhibit_manager_v1_interface_impl, 2);
assert_impl_opcode_count!(zwp_keyboard_shortcuts_inhibitor_v1_interface_impl, 1);
assert_impl_opcode_count!(zwp_pointer_constraints_v1_interface_impl, 3);
assert_impl_opcode_count!(zwp_confined_pointer_v1_interface_impl, 2);
assert_impl_opcode_count!(zwp_locked_pointer_v1_interface_impl, 3);
assert_impl_opcode_count!(ext_session_lock_manager_v1_interface_impl, 2);
assert_impl_opcode_count!(ext_session_lock_v1_interface_impl, 3);
assert_impl_opcode_count!(ext_session_lock_surface_v1_interface_impl, 2);
assert_impl_opcode_count!(ext_foreign_toplevel_list_v1_interface_impl, 2);
assert_impl_opcode_count!(ext_foreign_toplevel_handle_v1_interface_impl, 1);
assert_impl_opcode_count!(ext_data_control_manager_v1_interface_impl, 3);
assert_impl_opcode_count!(ext_data_control_device_v1_interface_impl, 3);
assert_impl_opcode_count!(ext_data_control_source_v1_interface_impl, 2);
assert_impl_opcode_count!(ext_data_control_offer_v1_interface_impl, 2);
assert_impl_opcode_count!(wp_cursor_shape_manager_v1_interface_impl, 3);
assert_impl_opcode_count!(wp_cursor_shape_device_v1_interface_impl, 2);
assert_impl_opcode_count!(zwp_primary_selection_device_manager_v1_interface_impl, 3);
assert_impl_opcode_count!(zwp_primary_selection_device_v1_interface_impl, 2);
assert_impl_opcode_count!(zwp_primary_selection_source_v1_interface_impl, 2);
assert_impl_opcode_count!(zwp_primary_selection_offer_v1_interface_impl, 2);
assert_impl_opcode_count!(zwp_text_input_manager_v3_interface_impl, 2);
assert_impl_opcode_count!(zwp_text_input_v3_interface_impl, 8);
