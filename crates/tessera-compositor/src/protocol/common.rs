use crate::*;

// ----- shared request handlers --------------------------------------------

pub(crate) unsafe extern "C" fn res_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_destroy(resource);
    }
}
pub(crate) unsafe extern "C" fn xdg_noop_serial(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _serial: u32,
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

// ----- accessors for the extensions module --------------------------------

/// Construct an `tessera_model::Point` (re-exported so extensions.rs does not name
/// the crate).
pub(crate) fn tessera_model_point(x: i32, y: i32) -> tessera_model::Point {
    tessera_model::Point { x, y }
}

pub(crate) fn tessera_model_size(w: i32, h: i32) -> tessera_model::Size {
    tessera_model::Size { w, h }
}
