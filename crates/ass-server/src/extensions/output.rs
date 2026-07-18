use super::*;

// ----- xdg-output-unstable-v1 ---------------------------------------------

static XDG_OUTPUT_MANAGER_IMPL: ffi::zxdg_output_manager_v1_interface_impl =
    ffi::zxdg_output_manager_v1_interface_impl {
        destroy: crate::res_destroy,
        get_xdg_output: xdg_output_manager_get_xdg_output,
    };

static XDG_OUTPUT_IMPL: ffi::zxdg_output_v1_interface_impl = ffi::zxdg_output_v1_interface_impl {
    destroy: crate::res_destroy,
};

pub(crate) unsafe extern "C" fn xdg_output_manager_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let res = ffi::wl_resource_create(
            client,
            &ffi::zxdg_output_manager_v1_interface,
            version as c_int,
            id,
        );
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &XDG_OUTPUT_MANAGER_IMPL as *const _ as *const c_void,
            data,
            None,
        );
    }
}

unsafe extern "C" fn xdg_output_manager_get_xdg_output(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    output: *mut ffi::wl_resource,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
        let ver = ffi::wl_resource_get_version(mgr);
        let res = ffi::wl_resource_create(client, &ffi::zxdg_output_v1_interface, ver, id);
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &XDG_OUTPUT_IMPL as *const _ as *const c_void,
            state as *mut c_void,
            Some(xdg_output_resource_destroy),
        );
        // Track so geometry changes can re-send.
        if !state.is_null() {
            (*state).xdg_output_resources.push(res);
            (*state).xdg_output_links.insert(res as usize, output);
        }
        send_xdg_output_geometry(res, output, state);
        // The xdg-output spec requires a final `done`; for v3+ that is the
        // xdg_output.done, for v1/v2 it is the paired wl_output.done which the
        // client already received. We send done on v3+ here.
        if ver >= 3 {
            ffi::wl_resource_post_event(res, ffi::ZXDG_OUTPUT_V1_DONE);
        }
    }
}

unsafe extern "C" fn xdg_output_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(resource) as *mut State;
        if state.is_null() {
            return;
        }
        for slot in (*state).xdg_output_resources.iter_mut() {
            if *slot == resource {
                *slot = std::ptr::null_mut();
                break;
            }
        }
        (*state).xdg_output_links.remove(&(resource as usize));
    }
}

/// Post the logical_position / logical_size / (name) events for one
/// xdg_output resource paired with one wl_output resource.
pub(crate) unsafe fn send_xdg_output_geometry(
    res: *mut ffi::wl_resource,
    output: *mut ffi::wl_resource,
    state: *mut State,
) {
    unsafe {
        let info = crate::output_info_for_resource(output);
        let geometry = info
            .as_ref()
            .map(|info| info.geometry)
            .or_else(|| (!state.is_null()).then(|| (*state).output_geometry));
        let origin = geometry
            .map(|geometry| geometry.logical_origin)
            .unwrap_or_else(|| crate::ass_core_point(0, 0));
        let size = geometry
            .map(|geometry| geometry.logical_size())
            .unwrap_or_else(|| crate::ass_core_size(1280, 720));
        ffi::wl_resource_post_event(
            res,
            ffi::ZXDG_OUTPUT_V1_LOGICAL_POSITION,
            origin.x,
            origin.y,
        );
        ffi::wl_resource_post_event(res, ffi::ZXDG_OUTPUT_V1_LOGICAL_SIZE, size.w, size.h);
        let ver = ffi::wl_resource_get_version(res);
        if ver >= 2 {
            let connector = info
                .as_ref()
                .map(|info| info.connector.as_str())
                .unwrap_or("unknown");
            let name = CString::new(connector).unwrap_or_else(|_| CString::new("output").unwrap());
            ffi::wl_resource_post_event(res, ffi::ZXDG_OUTPUT_V1_NAME, name.as_ptr());
        }
    }
}
