use crate::*;

// ----- shared no-op handlers ----------------------------------------------

pub(crate) unsafe extern "C" fn res_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_destroy(resource);
    }
}
#[allow(dead_code)]
unsafe extern "C" fn xdg_noop_none(_c: *mut ffi::wl_client, _r: *mut ffi::wl_resource) {}
pub(crate) unsafe extern "C" fn xdg_noop_serial(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _serial: u32,
) {
}
#[allow(dead_code)]
unsafe extern "C" fn xdg_noop_obj(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _obj: *mut ffi::wl_resource,
) {
}
#[allow(dead_code)]
unsafe extern "C" fn xdg_noop_str(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _s: *const std::os::raw::c_char,
) {
}
#[allow(dead_code)]
unsafe extern "C" fn xdg_noop_ii(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _a: i32,
    _b: i32,
) {
}
#[allow(dead_code)]
unsafe extern "C" fn xdg_noop_seat_serial(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _seat: *mut ffi::wl_resource,
    _serial: u32,
) {
}
#[allow(dead_code)]
unsafe extern "C" fn xdg_noop_resize(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _seat: *mut ffi::wl_resource,
    _serial: u32,
    _edges: u32,
) {
}
pub(crate) unsafe extern "C" fn xdg_noop_menu(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _seat: *mut ffi::wl_resource,
    _serial: u32,
    _x: i32,
    _y: i32,
) {
}

// ----- no-op handlers shared with the extensions module --------------------
//
// These are `pub(crate)` so `extensions.rs` can wire them into request
// vtables without duplicating each trivial handler. They accept the protocol
// arguments and do nothing (or only resource-lifecycle work).

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn noop_none(_c: *mut ffi::wl_client, _r: *mut ffi::wl_resource) {}

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn noop_obj_serial(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _obj: *mut ffi::wl_resource,
    _serial: u32,
) {
}

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn noop_str_ii(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _s: *const std::os::raw::c_char,
    _a: i32,
    _b: i32,
) {
}

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn noop_ii(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _a: i32,
    _b: i32,
) {
}

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn noop_uu(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _a: u32,
    _b: u32,
) {
}

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn noop_rect(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _x: i32,
    _y: i32,
    _w: i32,
    _h: i32,
) {
}

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn noop_region(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _reg: *mut ffi::wl_resource,
) {
}

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn noop_fixed2(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _a: i32,
    _b: i32,
) {
}

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn noop_serial_shape(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _serial: u32,
    _shape: u32,
) {
}

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn noop_uu_one(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _a: u32,
) {
}

// ----- accessors for the extensions module --------------------------------

/// Construct an `ass_core::Point` (re-exported so extensions.rs does not name
/// the crate).
pub(crate) fn ass_core_point(x: i32, y: i32) -> ass_core::Point {
    ass_core::Point { x, y }
}

pub(crate) fn ass_core_size(w: i32, h: i32) -> ass_core::Size {
    ass_core::Size { w, h }
}
