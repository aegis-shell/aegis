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

pub type wl_display = c_void;
pub type wl_client = c_void;
pub type wl_resource = c_void;
pub type wl_global = c_void;
pub type wl_event_loop = c_void;
pub type wl_shm_buffer = c_void;

/// Global bind callback: `void (*)(wl_client*, void *data, uint32_t version, uint32_t id)`.
pub type wl_global_bind_func = unsafe extern "C" fn(*mut wl_client, *mut c_void, u32, u32);

/// Resource destroy callback: `void (*)(wl_resource*)`.
pub type wl_resource_destroy_func = unsafe extern "C" fn(*mut wl_resource);

// Event opcodes for the events the server sends.
pub const WL_OUTPUT_GEOMETRY: u32 = 0;
pub const WL_OUTPUT_MODE: u32 = 1;
pub const WL_OUTPUT_DONE: u32 = 2;
pub const WL_OUTPUT_SCALE: u32 = 3;
pub const WL_OUTPUT_MODE_CURRENT: u32 = 0x1;
pub const WL_CALLBACK_DONE: u32 = 0;
pub const WL_BUFFER_RELEASE: u32 = 0;
pub const XDG_SURFACE_CONFIGURE: u32 = 0;
pub const XDG_TOPLEVEL_CONFIGURE: u32 = 0;
pub const XDG_TOPLEVEL_CLOSE: u32 = 1;
pub const XDG_WM_BASE_PING: u32 = 0;
pub const WL_SEAT_CAPABILITIES: u32 = 0;
pub const WL_SEAT_NAME: u32 = 1;
/// `wl_seat.capability.pointer` bit.
pub const WL_SEAT_CAPABILITY_POINTER: u32 = 1;
/// `wl_seat.capability.keyboard` bit.
pub const WL_SEAT_CAPABILITY_KEYBOARD: u32 = 2;

/// `wl_pointer` event opcodes.
pub const WL_POINTER_ENTER: u32 = 0;
pub const WL_POINTER_LEAVE: u32 = 1;
pub const WL_POINTER_MOTION: u32 = 2;
pub const WL_POINTER_BUTTON: u32 = 3;
pub const WL_POINTER_AXIS: u32 = 4;

/// `wl_keyboard` event opcodes.
pub const WL_KEYBOARD_KEYMAP: u32 = 0;
pub const WL_KEYBOARD_ENTER: u32 = 1;
pub const WL_KEYBOARD_LEAVE: u32 = 2;
pub const WL_KEYBOARD_KEY: u32 = 3;
pub const WL_KEYBOARD_MODIFIERS: u32 = 4;
pub const WL_KEYBOARD_REPEAT_INFO: u32 = 5;

/// `wl_keyboard.keymap.format`: 0 = no keymap, 1 = xkb string in fd.
pub const WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1: u32 = 1;

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
    pub static wl_buffer_interface: wl_interface;

    // Display + event loop.
    pub fn wl_display_create() -> *mut wl_display;
    pub fn wl_display_destroy(display: *mut wl_display);
    pub fn wl_display_add_socket_auto(display: *mut wl_display) -> *const c_char;
    pub fn wl_display_init_shm(display: *mut wl_display) -> c_int;
    pub fn wl_display_next_serial(display: *mut wl_display) -> u32;
    pub fn wl_display_get_event_loop(display: *mut wl_display) -> *mut wl_event_loop;
    pub fn wl_display_flush_clients(display: *mut wl_display);
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

/// `wl_data_device_manager` requests. v1 declares `create_data_source` and
/// `get_data_device`; v2 adds `destroy` as opcode 2. The global is bound at v3,
/// so all three are reachable. The struct must carry all three slots or
/// libwayland reads past the end on opcode 2.
#[repr(C)]
pub struct wl_data_device_manager_interface_impl {
    pub create_data_source: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub get_data_device:
        unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32, *mut wl_resource),
    pub destroy: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `wl_data_device` requests: start_drag, set_selection, release.
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
    pub release: unsafe extern "C" fn(*mut wl_client, *mut wl_resource),
}

/// `wl_seat` requests: get_pointer, get_keyboard, get_touch, release.
#[repr(C)]
pub struct wl_seat_interface_impl {
    pub get_pointer: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub get_keyboard: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
    pub get_touch: unsafe extern "C" fn(*mut wl_client, *mut wl_resource, u32),
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
assert_impl_opcode_count!(xdg_surface_interface_impl, 5);
assert_impl_opcode_count!(wl_subcompositor_interface_impl, 2);
assert_impl_opcode_count!(wl_subsurface_interface_impl, 6);
assert_impl_opcode_count!(wl_data_device_manager_interface_impl, 3);
assert_impl_opcode_count!(wl_data_device_interface_impl, 3);
assert_impl_opcode_count!(wl_seat_interface_impl, 4);
assert_impl_opcode_count!(xdg_toplevel_interface_impl, 14);
assert_impl_opcode_count!(zwp_linux_dmabuf_v1_interface_impl, 4);
assert_impl_opcode_count!(zwp_linux_buffer_params_v1_interface_impl, 4);
assert_impl_opcode_count!(wl_buffer_interface_impl, 1);
assert_impl_opcode_count!(wp_viewporter_interface_impl, 2);
assert_impl_opcode_count!(wp_viewport_interface_impl, 3);
