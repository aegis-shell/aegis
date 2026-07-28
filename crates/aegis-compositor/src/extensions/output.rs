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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum XdgOutputBatchDone {
    XdgOutput,
    WlOutput,
}

/// xdg-output v3 deprecated `zxdg_output_v1.done` in favour of a second
/// `wl_output.done` sent after the logical geometry. Clients such as SDL3
/// deliberately wait for that second core-output event before constructing
/// their display record.
fn xdg_output_batch_done(xdg_version: i32, wl_output_version: i32) -> XdgOutputBatchDone {
    if xdg_version >= 3 && wl_output_version >= 2 {
        XdgOutputBatchDone::WlOutput
    } else {
        // `zxdg_output_v1.done` exists since v1. Keep it as a compatibility
        // fallback when a v3 manager is paired with a legacy wl_output v1
        // resource, which has no core `done` event.
        XdgOutputBatchDone::XdgOutput
    }
}

pub(crate) unsafe fn finish_xdg_output_batch(
    xdg_output: *mut ffi::wl_resource,
    output: *mut ffi::wl_resource,
) {
    unsafe {
        if xdg_output.is_null() {
            return;
        }
        let xdg_version = ffi::wl_resource_get_version(xdg_output);
        let wl_output_version = if output.is_null() {
            0
        } else {
            ffi::wl_resource_get_version(output)
        };
        match xdg_output_batch_done(xdg_version, wl_output_version) {
            XdgOutputBatchDone::XdgOutput => {
                ffi::wl_resource_post_event(xdg_output, ffi::ZXDG_OUTPUT_V1_DONE);
            }
            XdgOutputBatchDone::WlOutput => {
                ffi::wl_resource_post_event(output, ffi::WL_OUTPUT_DONE);
            }
        }
    }
}

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
        finish_xdg_output_batch(res, output);
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
            .unwrap_or_else(|| crate::aegis_core_point(0, 0));
        let size = geometry
            .map(|geometry| geometry.logical_size())
            .unwrap_or_else(|| crate::aegis_core_size(1280, 720));
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_xdg_output_batches_end_on_the_xdg_resource() {
        assert_eq!(xdg_output_batch_done(1, 4), XdgOutputBatchDone::XdgOutput);
        assert_eq!(xdg_output_batch_done(2, 4), XdgOutputBatchDone::XdgOutput);
    }

    #[test]
    fn xdg_output_v3_batches_end_with_a_second_wl_output_done() {
        assert_eq!(xdg_output_batch_done(3, 4), XdgOutputBatchDone::WlOutput);
    }

    #[test]
    fn xdg_output_v3_falls_back_for_wl_output_v1() {
        assert_eq!(xdg_output_batch_done(3, 1), XdgOutputBatchDone::XdgOutput);
    }
}
