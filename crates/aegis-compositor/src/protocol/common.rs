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

/// Construct an `aegis_core::Point` (re-exported so extensions.rs does not name
/// the crate).
pub(crate) fn aegis_core_point(x: i32, y: i32) -> aegis_core::Point {
    aegis_core::Point { x, y }
}

pub(crate) fn aegis_core_size(w: i32, h: i32) -> aegis_core::Size {
    aegis_core::Size { w, h }
}
