use super::*;

// ----- fractional-scale-v1 ------------------------------------------------

struct FractionalScaleRec {
    surface: *mut SurfaceRec,
}

static FRACTIONAL_SCALE_MANAGER_IMPL: ffi::wp_fractional_scale_manager_v1_interface_impl =
    ffi::wp_fractional_scale_manager_v1_interface_impl {
        destroy: crate::res_destroy,
        get_fractional_scale: fractional_scale_manager_get,
    };

static FRACTIONAL_SCALE_IMPL: ffi::wp_fractional_scale_v1_interface_impl =
    ffi::wp_fractional_scale_v1_interface_impl {
        destroy: fractional_scale_destroy,
    };

pub(crate) unsafe extern "C" fn fractional_scale_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let res = ffi::wl_resource_create(
            client,
            &ffi::wp_fractional_scale_manager_v1_interface,
            version as c_int,
            id,
        );
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &FRACTIONAL_SCALE_MANAGER_IMPL as *const _ as *const c_void,
            data,
            None,
        );
    }
}

unsafe extern "C" fn fractional_scale_manager_get(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
        let rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        if !(*rec).fractional_scale.is_null() {
            ffi::wl_resource_post_error(
                mgr,
                0,
                c"wl_surface already has a fractional-scale object".as_ptr(),
            );
            return;
        }
        let ver = ffi::wl_resource_get_version(mgr);
        let res = ffi::wl_resource_create(client, &ffi::wp_fractional_scale_v1_interface, ver, id);
        if res.is_null() {
            return;
        }
        let scale_rec = Box::into_raw(Box::new(FractionalScaleRec { surface: rec }));
        ffi::wl_resource_set_implementation(
            res,
            &FRACTIONAL_SCALE_IMPL as *const _ as *const c_void,
            scale_rec as *mut c_void,
            Some(fractional_scale_resource_destroy),
        );
        // Attach to the surface so the server can re-send preferred_scale when
        // the output scale changes.
        (*rec).fractional_scale = res;
        send_fractional_scale(res, state);
    }
}

unsafe extern "C" fn fractional_scale_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        // Detach from the owning surface before libwayland frees the resource.
        let scale = ffi::wl_resource_get_user_data(resource) as *mut FractionalScaleRec;
        if !scale.is_null() && !(*scale).surface.is_null() {
            let surface = (*scale).surface;
            if (*surface).fractional_scale == resource {
                (*surface).fractional_scale = std::ptr::null_mut();
            }
            (*scale).surface = std::ptr::null_mut();
        }
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn fractional_scale_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let scale = ffi::wl_resource_get_user_data(resource) as *mut FractionalScaleRec;
        if scale.is_null() {
            return;
        }
        if !(*scale).surface.is_null() && (*(*scale).surface).fractional_scale == resource {
            (*(*scale).surface).fractional_scale = std::ptr::null_mut();
        }
        drop(Box::from_raw(scale));
    }
}

pub(crate) unsafe fn fractional_scale_surface_destroyed(surface: *mut SurfaceRec) {
    unsafe {
        if surface.is_null() || (*surface).fractional_scale.is_null() {
            return;
        }
        let scale =
            ffi::wl_resource_get_user_data((*surface).fractional_scale) as *mut FractionalScaleRec;
        if !scale.is_null() {
            (*scale).surface = std::ptr::null_mut();
        }
        (*surface).fractional_scale = std::ptr::null_mut();
    }
}

/// Post `wp_fractional_scale_v1.preferred_scale` for one resource, in 120ths
/// (the wire unit). Lock surfaces follow the output they were created for —
/// which may not be the focused one; other surfaces use the focused output's
/// fractional scale.
pub(crate) unsafe fn send_fractional_scale(res: *mut ffi::wl_resource, state: *mut State) {
    unsafe {
        let scale_120 = if state.is_null() {
            120u32
        } else {
            let scale_rec = ffi::wl_resource_get_user_data(res) as *mut FractionalScaleRec;
            let surface = if scale_rec.is_null() {
                std::ptr::null_mut()
            } else {
                (*scale_rec).surface
            };
            session_lock_surface_preferred_scale_120(surface).unwrap_or_else(|| {
                ((*state).output_geometry.scale.0 * 120.0).round() as u32
            })
        };
        ffi::wl_resource_post_event(res, ffi::WP_FRACTIONAL_SCALE_V1_PREFERRED_SCALE, scale_120);
    }
}

/// Re-send `preferred_scale` to every surface that has a fractional-scale
/// resource. Called when the output geometry (scale) changes.
pub(crate) unsafe fn resend_fractional_scales(state: *mut State) {
    unsafe {
        if state.is_null() {
            return;
        }
        for p in (*state).live_surfaces_pub() {
            let fs = (*p).fractional_scale;
            if !fs.is_null() {
                send_fractional_scale(fs, state);
            }
        }
    }
}
