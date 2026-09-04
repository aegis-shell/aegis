use super::*;

// ----- keyboard-shortcuts-inhibit-unstable-v1 ----------------------------

struct KeyboardShortcutsInhibitorRec {
    state: *mut State,
    surface: *mut ffi::wl_resource,
    active: bool,
}

static KEYBOARD_SHORTCUTS_INHIBIT_MANAGER_IMPL:
    ffi::zwp_keyboard_shortcuts_inhibit_manager_v1_interface_impl =
    ffi::zwp_keyboard_shortcuts_inhibit_manager_v1_interface_impl {
        destroy: crate::res_destroy,
        inhibit_shortcuts: keyboard_shortcuts_inhibit,
    };
static KEYBOARD_SHORTCUTS_INHIBITOR_IMPL: ffi::zwp_keyboard_shortcuts_inhibitor_v1_interface_impl =
    ffi::zwp_keyboard_shortcuts_inhibitor_v1_interface_impl {
        destroy: keyboard_shortcuts_inhibitor_destroy,
    };

pub(crate) unsafe extern "C" fn keyboard_shortcuts_inhibit_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let resource = ffi::wl_resource_create(
            client,
            &ffi::zwp_keyboard_shortcuts_inhibit_manager_v1_interface,
            version.min(1) as c_int,
            id,
        );
        if !resource.is_null() {
            ffi::wl_resource_set_implementation(
                resource,
                &KEYBOARD_SHORTCUTS_INHIBIT_MANAGER_IMPL as *const _ as *const c_void,
                data,
                None,
            );
        }
    }
}

unsafe extern "C" fn keyboard_shortcuts_inhibit(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
    seat: *mut ffi::wl_resource,
) {
    unsafe {
        if surface.is_null()
            || seat.is_null()
            || ffi::wl_resource_get_client(surface) != client
            || ffi::wl_resource_get_client(seat) != client
        {
            ffi::wl_resource_post_error(
                manager,
                0,
                c"surface or seat belongs to another client".as_ptr(),
            );
            return;
        }
        let state = ffi::wl_resource_get_user_data(manager) as *mut State;
        let Some(advertised_seat) = (*state).seat_for_resource(seat) else {
            return;
        };
        let Some(_guard) =
            crate::ActiveSeatGuard::for_client_seat_resource(state, client, seat, true)
        else {
            return;
        };
        let seat_id = (*state).active_seat;
        let duplicate = (*state)
            .keyboard_shortcut_inhibitors
            .iter()
            .copied()
            .any(|resource| {
                let rec =
                    ffi::wl_resource_get_user_data(resource) as *mut KeyboardShortcutsInhibitorRec;
                !rec.is_null() && (*rec).surface == surface
            });
        if duplicate {
            ffi::wl_resource_post_error(
                manager,
                ffi::ZWP_KEYBOARD_SHORTCUTS_INHIBIT_MANAGER_V1_ERROR_ALREADY_INHIBITED,
                c"shortcuts are already inhibited for this surface and seat".as_ptr(),
            );
            return;
        }
        let resource = ffi::wl_resource_create(
            client,
            &ffi::zwp_keyboard_shortcuts_inhibitor_v1_interface,
            1,
            id,
        );
        if resource.is_null() {
            return;
        }
        let active = (*state).keyboard_focus == surface;
        let rec = Box::into_raw(Box::new(KeyboardShortcutsInhibitorRec {
            state,
            surface,
            active,
        }));
        ffi::wl_resource_set_implementation(
            resource,
            &KEYBOARD_SHORTCUTS_INHIBITOR_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            Some(keyboard_shortcuts_inhibitor_resource_destroy),
        );
        (*state).track_routed_seat_resource(resource, advertised_seat, seat_id);
        (*state).keyboard_shortcut_inhibitors.push(resource);
        if active {
            ffi::wl_resource_post_event(resource, ffi::ZWP_KEYBOARD_SHORTCUTS_INHIBITOR_V1_ACTIVE);
        }
    }
}

unsafe extern "C" fn keyboard_shortcuts_inhibitor_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn keyboard_shortcuts_inhibitor_resource_destroy(
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut KeyboardShortcutsInhibitorRec;
        if rec.is_null() {
            return;
        }
        if let Some(_guard) = crate::ActiveSeatGuard::for_resource((*rec).state, resource, false) {
            (*(*rec).state)
                .keyboard_shortcut_inhibitors
                .retain(|r| *r != resource);
            (*(*rec).state).untrack_seat_resource(resource);
        }
        drop(Box::from_raw(rec));
    }
}

pub(crate) unsafe fn keyboard_shortcuts_focus_changed(
    state: *mut State,
    new_focus: *mut ffi::wl_resource,
) {
    unsafe {
        for resource in (*state).keyboard_shortcut_inhibitors.clone() {
            let rec =
                ffi::wl_resource_get_user_data(resource) as *mut KeyboardShortcutsInhibitorRec;
            if rec.is_null() {
                continue;
            }
            let active = !new_focus.is_null() && (*rec).surface == new_focus;
            if active == (*rec).active {
                continue;
            }
            (*rec).active = active;
            ffi::wl_resource_post_event(
                resource,
                if active {
                    ffi::ZWP_KEYBOARD_SHORTCUTS_INHIBITOR_V1_ACTIVE
                } else {
                    ffi::ZWP_KEYBOARD_SHORTCUTS_INHIBITOR_V1_INACTIVE
                },
            );
        }
    }
}

pub(crate) unsafe fn keyboard_shortcuts_inhibited(state: *mut State) -> bool {
    unsafe {
        !(*state).keyboard_focus.is_null()
            && (*state)
                .keyboard_shortcut_inhibitors
                .iter()
                .copied()
                .any(|resource| {
                    let rec = ffi::wl_resource_get_user_data(resource)
                        as *mut KeyboardShortcutsInhibitorRec;
                    !rec.is_null() && (*rec).active && (*rec).surface == (*state).keyboard_focus
                })
    }
}

pub(crate) unsafe fn keyboard_shortcuts_surface_destroyed(
    state: *mut State,
    surface: *mut ffi::wl_resource,
) {
    unsafe {
        for resource in (*state).keyboard_shortcut_inhibitors.clone() {
            let rec =
                ffi::wl_resource_get_user_data(resource) as *mut KeyboardShortcutsInhibitorRec;
            if rec.is_null() || (*rec).surface != surface {
                continue;
            }
            if (*rec).active {
                (*rec).active = false;
                ffi::wl_resource_post_event(
                    resource,
                    ffi::ZWP_KEYBOARD_SHORTCUTS_INHIBITOR_V1_INACTIVE,
                );
            }
            (*rec).surface = std::ptr::null_mut();
        }
    }
}
