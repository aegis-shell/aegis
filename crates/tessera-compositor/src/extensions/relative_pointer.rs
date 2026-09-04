use super::*;

// ----- relative-pointer-unstable-v1 ---------------------------------------

struct RelativePointerRec {
    state: *mut State,
    pointer: *mut ffi::wl_resource,
}

static RELATIVE_POINTER_MANAGER_IMPL: ffi::zwp_relative_pointer_manager_v1_interface_impl =
    ffi::zwp_relative_pointer_manager_v1_interface_impl {
        destroy: crate::res_destroy,
        get_relative_pointer: relative_pointer_manager_get,
    };

static RELATIVE_POINTER_IMPL: ffi::zwp_relative_pointer_v1_interface_impl =
    ffi::zwp_relative_pointer_v1_interface_impl {
        destroy: relative_pointer_destroy,
    };

pub(crate) unsafe extern "C" fn relative_pointer_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let res = ffi::wl_resource_create(
            client,
            &ffi::zwp_relative_pointer_manager_v1_interface,
            version as c_int,
            id,
        );
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &RELATIVE_POINTER_MANAGER_IMPL as *const _ as *const c_void,
            data,
            None,
        );
    }
}

unsafe extern "C" fn relative_pointer_manager_get(
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
        let res = ffi::wl_resource_create(client, &ffi::zwp_relative_pointer_v1_interface, ver, id);
        if res.is_null() {
            return;
        }
        let rec = Box::into_raw(Box::new(RelativePointerRec { state, pointer }));
        ffi::wl_resource_set_implementation(
            res,
            &RELATIVE_POINTER_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            Some(relative_pointer_resource_destroy),
        );
        (*state).track_routed_seat_resource(res, origin.unwrap_or(seat), seat);
        (*state).relative_pointers.push(res);
    }
}

unsafe extern "C" fn relative_pointer_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn relative_pointer_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut RelativePointerRec;
        if rec.is_null() {
            return;
        }
        if let Some(_guard) = crate::ActiveSeatGuard::for_resource((*rec).state, resource, false) {
            (*(*rec).state).relative_pointers.retain(|r| *r != resource);
            (*(*rec).state).untrack_seat_resource(resource);
        }
        let _ = (*rec).pointer;
        drop(Box::from_raw(rec));
    }
}
