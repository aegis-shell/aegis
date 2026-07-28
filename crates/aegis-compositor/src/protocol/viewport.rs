use crate::*;

// ----- wp_viewporter ------------------------------------------------------
// Crop/scale state is double-buffered with wl_surface.commit and surfaced to
// the renderer through SurfaceGeometry.

pub(crate) struct ViewportRec {
    pub(crate) surface: *mut SurfaceRec,
}

static VIEWPORTER_IMPL: ffi::wp_viewporter_interface_impl = ffi::wp_viewporter_interface_impl {
    destroy: res_destroy,
    get_viewport: viewporter_get_viewport,
};

static VIEWPORT_IMPL: ffi::wp_viewport_interface_impl = ffi::wp_viewport_interface_impl {
    destroy: viewport_destroy,
    set_source: viewport_set_source,
    set_destination: viewport_set_destination,
};

pub(crate) unsafe extern "C" fn viewporter_bind(
    client: *mut ffi::wl_client,
    _data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let res =
            ffi::wl_resource_create(client, &ffi::wp_viewporter_interface, version as c_int, id);
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &VIEWPORTER_IMPL as *const _ as *const c_void,
            std::ptr::null_mut(),
            None,
        );
    }
}

unsafe extern "C" fn viewporter_get_viewport(
    client: *mut ffi::wl_client,
    viewporter: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        if !(*rec).viewport_resource.is_null() {
            ffi::wl_resource_post_error(
                viewporter,
                0,
                c"wl_surface already has a wp_viewport".as_ptr(),
            );
            return;
        }
        let ver = ffi::wl_resource_get_version(viewporter);
        let vp = ffi::wl_resource_create(client, &ffi::wp_viewport_interface, ver, id);
        if vp.is_null() {
            return;
        }
        let viewport_rec = Box::into_raw(Box::new(ViewportRec { surface: rec }));
        ffi::wl_resource_set_implementation(
            vp,
            &VIEWPORT_IMPL as *const _ as *mut c_void,
            viewport_rec as *mut c_void,
            Some(viewport_resource_destroy),
        );
        (*rec).viewport_resource = vp;
    }
}

unsafe extern "C" fn viewport_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let viewport = ffi::wl_resource_get_user_data(resource) as *mut ViewportRec;
        if !viewport.is_null() && !(*viewport).surface.is_null() {
            let surface = (*viewport).surface;
            (*surface).pending_viewport_src = Some(None);
            (*surface).pending_viewport_dst = Some(None);
            (*surface).viewport_resource = std::ptr::null_mut();
            (*viewport).surface = std::ptr::null_mut();
        }
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn viewport_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let viewport = ffi::wl_resource_get_user_data(resource) as *mut ViewportRec;
        if viewport.is_null() {
            return;
        }
        if !(*viewport).surface.is_null() {
            (*(*viewport).surface).viewport_resource = std::ptr::null_mut();
        }
        drop(Box::from_raw(viewport));
    }
}

unsafe fn viewport_surface(resource: *mut ffi::wl_resource) -> *mut SurfaceRec {
    unsafe {
        let viewport = ffi::wl_resource_get_user_data(resource) as *mut ViewportRec;
        if viewport.is_null() || (*viewport).surface.is_null() {
            ffi::wl_resource_post_error(
                resource,
                3,
                c"associated wl_surface was destroyed".as_ptr(),
            );
            std::ptr::null_mut()
        } else {
            (*viewport).surface
        }
    }
}

/// `wl_fixed_t` (24.8 signed) to `f32`. Matches the helper the nested backend
/// uses; duplicated here so aegis-compositor stays independent of aegis-backend.
fn fixed_to_f32(v: i32) -> f32 {
    (v as f32) / 256.0
}

const WL_FIXED_NEGATIVE_ONE: i32 = -256;

pub(crate) fn decode_viewport_source(
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) -> Result<Option<aegis_core::Rect>, ()> {
    if [x, y, w, h]
        .into_iter()
        .all(|value| value == WL_FIXED_NEGATIVE_ONE)
    {
        return Ok(None);
    }
    if x < 0 || y < 0 || w <= 0 || h <= 0 {
        return Err(());
    }
    Ok(Some(aegis_core::Rect::new(
        fixed_to_f32(x).round() as i32,
        fixed_to_f32(y).round() as i32,
        fixed_to_f32(w).round().max(1.0) as i32,
        fixed_to_f32(h).round().max(1.0) as i32,
    )))
}

/// `wp_viewport.set_source`: sets the source rectangle in surface-local
/// pixel coords (24.8 fixed-point). A value of -1 for every field resets the
/// source to "whole buffer".
unsafe extern "C" fn viewport_set_source(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    unsafe {
        let rec = viewport_surface(resource);
        if rec.is_null() {
            return;
        }
        match decode_viewport_source(x, y, w, h) {
            Ok(source) => (*rec).pending_viewport_src = Some(source),
            Err(()) => {
                ffi::wl_resource_post_error(
                    resource,
                    0,
                    c"invalid viewport source rectangle".as_ptr(),
                );
            }
        }
    }
}

/// `wp_viewport.set_destination`: sets the destination size in integer
/// logical pixels. A value of -1 for either field resets.
unsafe extern "C" fn viewport_set_destination(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    w: i32,
    h: i32,
) {
    unsafe {
        let rec = viewport_surface(resource);
        if rec.is_null() {
            return;
        }
        if w == -1 && h == -1 {
            (*rec).pending_viewport_dst = Some(None);
            return;
        }
        if w <= 0 || h <= 0 {
            ffi::wl_resource_post_error(resource, 0, c"invalid viewport destination size".as_ptr());
            return;
        }
        (*rec).pending_viewport_dst = Some(Some(aegis_core::Size { w, h }));
    }
}
