use super::*;

// ----- ext-foreign-toplevel-list-v1 ---------------------------------------

static FOREIGN_TOPLEVEL_LIST_IMPL: ffi::ext_foreign_toplevel_list_v1_interface_impl =
    ffi::ext_foreign_toplevel_list_v1_interface_impl {
        stop: foreign_toplevel_stop,
        destroy: crate::res_destroy,
    };

static FOREIGN_TOPLEVEL_HANDLE_IMPL: ffi::ext_foreign_toplevel_handle_v1_interface_impl =
    ffi::ext_foreign_toplevel_handle_v1_interface_impl {
        destroy: foreign_toplevel_handle_destroy,
    };

pub(crate) unsafe extern "C" fn foreign_toplevel_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let res = ffi::wl_resource_create(
            client,
            &ffi::ext_foreign_toplevel_list_v1_interface,
            version as c_int,
            id,
        );
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &FOREIGN_TOPLEVEL_LIST_IMPL as *const _ as *const c_void,
            data,
            Some(foreign_toplevel_list_resource_destroy),
        );
        let state = data as *mut State;
        if !state.is_null() {
            (*state).foreign_toplevel_lists.push(res);
        }
        // Advertise each currently-live toplevel. `finished` is sent only after
        // the client explicitly calls stop; live lists keep receiving additions.
        if !state.is_null() {
            for p in (*state).live_surfaces_pub() {
                let s = &*p;
                if s.xdg_toplevel.is_null()
                    || !s.mapped
                    || !(*state).client_observes_window(client, s.window.id)
                {
                    continue;
                }
                create_foreign_handle(res, s as *const SurfaceRec as *mut SurfaceRec, state);
            }
        }
    }
}

unsafe extern "C" fn foreign_toplevel_stop(
    _client: *mut ffi::wl_client,
    list: *mut ffi::wl_resource,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(list) as *mut State;
        if !state.is_null() {
            (*state).foreign_toplevel_lists.retain(|r| *r != list);
        }
        ffi::wl_resource_post_event(list, ffi::EXT_FOREIGN_TOPLEVEL_LIST_V1_FINISHED);
    }
}

unsafe extern "C" fn foreign_toplevel_list_resource_destroy(list: *mut ffi::wl_resource) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(list) as *mut State;
        if !state.is_null() {
            (*state).foreign_toplevel_lists.retain(|r| *r != list);
        }
    }
}

/// Create a `ext_foreign_toplevel_handle_v1` for `rec`, advertise it on `list`,
/// register it in `state.foreign_handles`, and send title/app_id/identifier/done.
unsafe fn create_foreign_handle(
    list: *mut ffi::wl_resource,
    rec: *mut SurfaceRec,
    state: *mut State,
) {
    unsafe {
        let client = ffi::wl_resource_get_client(list);
        let ver = ffi::wl_resource_get_version(list);
        let handle = ffi::wl_resource_create(
            client,
            &ffi::ext_foreign_toplevel_handle_v1_interface,
            ver,
            0,
        );
        if handle.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            handle,
            &FOREIGN_TOPLEVEL_HANDLE_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            Some(foreign_toplevel_handle_resource_destroy),
        );
        ffi::wl_resource_post_event(list, ffi::EXT_FOREIGN_TOPLEVEL_LIST_V1_TOPLEVEL, handle);
        let wid = (*rec).window.id.0;
        if let Some(title) = &(*rec).window.title
            && let Ok(c) = CString::new(title.as_str())
        {
            ffi::wl_resource_post_event(
                handle,
                ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_TITLE,
                c.as_ptr(),
            );
        }
        if let Some(app_id) = &(*rec).window.app_id
            && let Ok(c) = CString::new(app_id.as_str())
        {
            ffi::wl_resource_post_event(
                handle,
                ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_APP_ID,
                c.as_ptr(),
            );
        }
        if let Ok(c) = CString::new(format!("aegis:{wid}")) {
            ffi::wl_resource_post_event(
                handle,
                ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_IDENTIFIER,
                c.as_ptr(),
            );
        }
        ffi::wl_resource_post_event(handle, ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_DONE);
        if !state.is_null() {
            (*state)
                .foreign_handles
                .entry(wid)
                .or_default()
                .push(handle);
        }
    }
}

unsafe extern "C" fn foreign_toplevel_handle_destroy(
    _client: *mut ffi::wl_client,
    handle: *mut ffi::wl_resource,
) {
    unsafe {
        foreign_toplevel_handle_resource_destroy(handle);
        ffi::wl_resource_set_user_data(handle, std::ptr::null_mut());
        ffi::wl_resource_destroy(handle);
    }
}

unsafe extern "C" fn foreign_toplevel_handle_resource_destroy(handle: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(handle) as *mut SurfaceRec;
        if rec.is_null() || (*rec).state.is_null() {
            return;
        }
        let state = (*rec).state;
        let wid = (*rec).window.id.0;
        if let Some(handles) = (*state).foreign_handles.get_mut(&wid) {
            handles.retain(|r| *r != handle);
            if handles.is_empty() {
                (*state).foreign_handles.remove(&wid);
            }
        }
    }
}

/// Push a new toplevel to every bound foreign-toplevel-list (live update).
/// Called when a toplevel first maps.
pub(crate) unsafe fn foreign_toplevel_added(rec: *mut SurfaceRec, state: *mut State) {
    unsafe {
        if state.is_null() {
            return;
        }
        let lists: Vec<*mut ffi::wl_resource> = (*state)
            .foreign_toplevel_lists
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .collect();
        for list in lists {
            let client = ffi::wl_resource_get_client(list);
            if (*state).client_observes_window(client, (*rec).window.id) {
                create_foreign_handle(list, rec, state);
            }
        }
    }
}

/// Reconcile foreign-toplevel capability objects after Interaction Domain presentation
/// authority changes. Closing and re-advertising keeps already-bound physical
/// clients from retaining a handle to a window they may no longer observe.
pub(crate) unsafe fn foreign_toplevel_authority_changed(rec: *mut SurfaceRec, state: *mut State) {
    unsafe {
        if rec.is_null() || state.is_null() {
            return;
        }
        foreign_toplevel_removed((*rec).window.id.0, state);
        if (*rec).mapped {
            foreign_toplevel_added(rec, state);
        }
    }
}

/// Push a title/app_id update for a toplevel to its handle.
pub(crate) unsafe fn foreign_toplevel_updated(rec: *mut SurfaceRec, state: *mut State) {
    unsafe {
        if state.is_null() {
            return;
        }
        let wid = (*rec).window.id.0;
        let Some(handles) = (*state).foreign_handles.get(&wid).cloned() else {
            return;
        };
        for handle in handles.into_iter().filter(|r| !r.is_null()) {
            if let Some(title) = &(*rec).window.title
                && let Ok(c) = CString::new(title.as_str())
            {
                ffi::wl_resource_post_event(
                    handle,
                    ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_TITLE,
                    c.as_ptr(),
                );
            }
            if let Some(app_id) = &(*rec).window.app_id
                && let Ok(c) = CString::new(app_id.as_str())
            {
                ffi::wl_resource_post_event(
                    handle,
                    ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_APP_ID,
                    c.as_ptr(),
                );
            }
            ffi::wl_resource_post_event(handle, ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_DONE);
        }
    }
}

/// Notify `closed` on the handle and drop it from the tracking map.
pub(crate) unsafe fn foreign_toplevel_removed(wid: u64, state: *mut State) {
    unsafe {
        if state.is_null() {
            return;
        }
        if let Some(handles) = (*state).foreign_handles.remove(&wid) {
            for handle in handles.into_iter().filter(|r| !r.is_null()) {
                ffi::wl_resource_post_event(handle, ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_CLOSED);
                ffi::wl_resource_destroy(handle);
            }
        }
    }
}
