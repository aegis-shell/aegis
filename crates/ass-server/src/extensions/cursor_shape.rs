use super::*;

// ----- cursor-shape-v1 ----------------------------------------------------

static CURSOR_SHAPE_MANAGER_IMPL: ffi::wp_cursor_shape_manager_v1_interface_impl =
    ffi::wp_cursor_shape_manager_v1_interface_impl {
        destroy: crate::res_destroy,
        get_pointer: cursor_shape_get_pointer,
        get_tablet_tool_v2: cursor_shape_get_tablet,
    };

static CURSOR_SHAPE_DEVICE_IMPL: ffi::wp_cursor_shape_device_v1_interface_impl =
    ffi::wp_cursor_shape_device_v1_interface_impl {
        destroy: crate::res_destroy,
        set_shape: cursor_shape_set_shape,
    };

pub(crate) unsafe extern "C" fn cursor_shape_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let res = ffi::wl_resource_create(
            client,
            &ffi::wp_cursor_shape_manager_v1_interface,
            version as c_int,
            id,
        );
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &CURSOR_SHAPE_MANAGER_IMPL as *const _ as *const c_void,
            data,
            None,
        );
    }
}

unsafe extern "C" fn cursor_shape_get_pointer(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    pointer: *mut ffi::wl_resource,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
        let origin = (*state).seat_origin_for_resource(pointer);
        let Some(_guard) = crate::ActiveSeatGuard::for_resource(state, pointer, true) else {
            return;
        };
        let seat = (*state).active_seat;
        let ver = ffi::wl_resource_get_version(mgr);
        let dev =
            ffi::wl_resource_create(client, &ffi::wp_cursor_shape_device_v1_interface, ver, id);
        if !dev.is_null() {
            ffi::wl_resource_set_implementation(
                dev,
                &CURSOR_SHAPE_DEVICE_IMPL as *const _ as *const c_void,
                state as *mut c_void,
                Some(cursor_shape_resource_destroy),
            );
            (*state).track_routed_seat_resource(dev, origin.unwrap_or(seat), seat);
            (*state).cursor_shape_devices.push(dev);
        }
    }
}

unsafe extern "C" fn cursor_shape_get_tablet(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    tool: *mut ffi::wl_resource,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
        let origin = (*state).seat_origin_for_resource(tool);
        let Some(_guard) = crate::ActiveSeatGuard::for_resource(state, tool, true) else {
            return;
        };
        let seat = (*state).active_seat;
        let ver = ffi::wl_resource_get_version(mgr);
        let dev =
            ffi::wl_resource_create(client, &ffi::wp_cursor_shape_device_v1_interface, ver, id);
        if !dev.is_null() {
            ffi::wl_resource_set_implementation(
                dev,
                &CURSOR_SHAPE_DEVICE_IMPL as *const _ as *const c_void,
                state as *mut c_void,
                Some(cursor_shape_resource_destroy),
            );
            (*state).track_routed_seat_resource(dev, origin.unwrap_or(seat), seat);
            (*state).cursor_shape_devices.push(dev);
        }
    }
}

/// `wp_cursor_shape_device_v1.set_shape`: record the requested shape so the
/// renderer can paint the matching cursor. The shape enum follows
/// `wp_cursor_shape_device_v1.shape` (1=default, 2=context_menu, ...).
unsafe extern "C" fn cursor_shape_set_shape(
    client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    serial: u32,
    shape: u32,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(resource) as *mut State;
        let Some(_guard) = crate::ActiveSeatGuard::for_resource(state, resource, true) else {
            return;
        };
        if !(*state).pointer_focus.is_null()
            && ffi::wl_resource_get_client((*state).pointer_focus) == client
            && serial == (*state).last_pointer_enter_serial
        {
            (*state).cursor_shape = shape;
            (*state).cursor_surface = std::ptr::null_mut();
            (*state).cursor_hidden = false;
        }
    }
}

unsafe extern "C" fn cursor_shape_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(resource) as *mut State;
        if let Some(_guard) = crate::ActiveSeatGuard::for_resource(state, resource, false) {
            (*state)
                .cursor_shape_devices
                .retain(|item| *item != resource);
            (*state).untrack_seat_resource(resource);
        }
    }
}
