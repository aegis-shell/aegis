use crate::*;

// ----- wl_seat ------------------------------------------------------------

static SEAT_IMPL: ffi::wl_seat_interface_impl = ffi::wl_seat_interface_impl {
    get_pointer: seat_get_pointer,
    get_keyboard: seat_get_keyboard,
    get_touch: seat_get_touch,
    release: res_destroy,
};

// `wl_pointer` has two requests: `set_cursor` (v1) and `release` (v3). The
// previous code bound the pointer resource with a NULL implementation on the
// assumption that no request needed handling — but `set_cursor` is a regular
// request every client sends to change its cursor, and a NULL handler makes
// libwayland abort with "Implementation of resource N of wl_pointer is NULL".
// `set_cursor` assigns the cursor role and exposes the surface as an overlay;
// the nested backend hides the host cursor while that overlay is active.
// `release` destroys the resource, which then runs
// `pointer_resource_destroy` to remove it from the seat's resource list.
static POINTER_IMPL: ffi::wl_pointer_interface_impl = ffi::wl_pointer_interface_impl {
    set_cursor: pointer_set_cursor,
    release: res_destroy,
};

static KEYBOARD_IMPL: ffi::wl_keyboard_interface_impl = ffi::wl_keyboard_interface_impl {
    release: res_destroy,
};

static TOUCH_IMPL: ffi::wl_touch_interface_impl = ffi::wl_touch_interface_impl {
    release: res_destroy,
};

/// `wl_pointer.set_cursor`: assign/update a custom cursor surface, or hide the
/// cursor for a null surface. Only the client holding pointer focus may use the
/// serial from its most recent enter event.
unsafe extern "C" fn pointer_set_cursor(
    client: *mut ffi::wl_client,
    pointer: *mut ffi::wl_resource,
    serial: u32,
    surface: *mut ffi::wl_resource,
    hotspot_x: i32,
    hotspot_y: i32,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(pointer) as *mut State;
        if state.is_null() {
            return;
        }
        let Some(seat) = (*state).seat_for_resource(pointer) else {
            return;
        };
        let Some(runtime) = (*state).seat_runtime(seat) else {
            return;
        };
        if runtime.pointer_focus.is_null()
            || ffi::wl_resource_get_client(runtime.pointer_focus) != client
            || serial != runtime.last_pointer_enter_serial
        {
            return;
        }
        if !surface.is_null() {
            if ffi::wl_resource_get_client(surface) != client {
                ffi::wl_resource_post_error(
                    pointer,
                    0,
                    c"cursor surface belongs to another client".as_ptr(),
                );
                return;
            }
            let rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
            if rec.is_null() || (surface_has_role(&*rec) && !(*rec).cursor_role) {
                ffi::wl_resource_post_error(
                    pointer,
                    0,
                    c"surface already has a different role".as_ptr(),
                );
                return;
            }
            (*rec).cursor_role = true;
        }
        let Some(runtime) = (*state).seat_runtime_mut(seat) else {
            return;
        };
        runtime.cursor_surface = surface;
        runtime.cursor_hotspot = aegis_model::Point {
            x: hotspot_x,
            y: hotspot_y,
        };
        runtime.cursor_shape = 0;
        runtime.cursor_hidden = true;
        update_overlay_positions_for_seat(state, seat);
    }
}

pub(crate) unsafe extern "C" fn seat_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let record = data as *mut SeatGlobal;
        if record.is_null() || !(*record).active || (*record).state.is_null() {
            return;
        }
        let state = (*record).state;
        let seat_id = (*record).seat;
        let res = ffi::wl_resource_create(client, &ffi::wl_seat_interface, version as c_int, id);
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &SEAT_IMPL as *const _ as *const c_void,
            state as *mut c_void,
            Some(seat_resource_destroy),
        );
        (*state).track_seat_resource(res, seat_id);
        if let Some(runtime) = (*state).seat_runtime_mut(seat_id) {
            runtime.seat_resources.push(res);
        }
        let caps = seat_wire_capabilities(&*state, seat_id);
        ffi::wl_resource_post_event(res, ffi::WL_SEAT_CAPABILITIES, caps);
        if version >= 2 {
            let name = (*state)
                .authority
                .seat(seat_id)
                .map(|seat| seat.name.as_str())
                .unwrap_or("revoked");
            let name = CString::new(name).unwrap_or_else(|_| CString::new("invalid-seat").unwrap());
            ffi::wl_resource_post_event(res, ffi::WL_SEAT_NAME, name.as_ptr());
        }
    }
}

pub(crate) fn seat_wire_capabilities(state: &State, seat: SeatId) -> u32 {
    let Some(model) = state.authority.seat(seat) else {
        return 0;
    };
    let Some(runtime) = state.seat_runtime(seat) else {
        return 0;
    };
    if runtime.id != seat
        || runtime.interaction_domain != model.interaction_domain
        || runtime.principal != model.principal
        || !model.enabled
    {
        return 0;
    }
    let mut caps = 0;
    if runtime.capabilities.pointer {
        caps |= ffi::WL_SEAT_CAPABILITY_POINTER;
    }
    if runtime.capabilities.keyboard && runtime.keyboard.is_some() {
        caps |= ffi::WL_SEAT_CAPABILITY_KEYBOARD;
    }
    if runtime.capabilities.touch {
        caps |= ffi::WL_SEAT_CAPABILITY_TOUCH;
    }
    caps
}

unsafe extern "C" fn seat_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(resource) as *mut State;
        if state.is_null() {
            return;
        }
        let Some(seat) = (*state).untrack_seat_resource(resource) else {
            return;
        };
        if let Some(runtime) = (*state).seat_runtime_mut(seat) {
            runtime
                .seat_resources
                .retain(|candidate| *candidate != resource);
        }
    }
}

// Each pointer resource is tracked by its owning SeatRuntime so the main loop
// can fan events out without crossing interaction domain boundaries.
unsafe extern "C" fn seat_get_pointer(
    client: *mut ffi::wl_client,
    seat: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(seat) as *mut State;
        if state.is_null() {
            return;
        }
        let Some(advertised_seat) = (*state).seat_for_resource(seat) else {
            return;
        };
        (*state).note_client_used_seat(client, advertised_seat);
        let seat_id = (*state).client_routed_seat(client, advertised_seat);
        let ver = ffi::wl_resource_get_version(seat).min(9);
        let p = ffi::wl_resource_create(client, &ffi::wl_pointer_interface, ver, id);
        if p.is_null() {
            return;
        }
        // Real implementation: see POINTER_IMPL — both `set_cursor` and
        // `release` must have handlers or libwayland aborts the server.
        ffi::wl_resource_set_implementation(
            p,
            &POINTER_IMPL as *const _ as *const c_void,
            state as *mut c_void,
            Some(pointer_resource_destroy),
        );
        (*state).track_routed_seat_resource(p, advertised_seat, seat_id);
        if let Some(runtime) = (*state).seat_runtime_mut(seat_id) {
            runtime.pointer_resources.push(p);
        }
    }
}

unsafe extern "C" fn pointer_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(resource) as *mut State;
        if state.is_null() {
            return;
        }
        let Some(seat) = (*state).untrack_seat_resource(resource) else {
            return;
        };
        let Some(runtime) = (*state).seat_runtime_mut(seat) else {
            return;
        };
        // Remove the resource outright. Long sessions churn wl_pointer
        // resources; a tombstone here would be walked by every focus scan
        // for the rest of the session.
        if let Some(pos) = runtime
            .pointer_resources
            .iter()
            .position(|p| *p == resource)
        {
            runtime.pointer_resources.remove(pos);
        }
        // If the focused client no longer has any pointer resources, clear focus
        // so the next motion event re-evaluates enter against remaining clients.
        if !runtime.pointer_focus.is_null() {
            let focus_client = ffi::wl_resource_get_client(runtime.pointer_focus);
            let orphaned = runtime
                .pointer_resources
                .iter()
                .copied()
                .filter(|p| !p.is_null())
                .all(|p| ffi::wl_resource_get_client(p) != focus_client);
            if orphaned {
                runtime.pointer_focus = std::ptr::null_mut();
            }
        }
    }
}

unsafe extern "C" fn seat_get_keyboard(
    client: *mut ffi::wl_client,
    seat: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(seat) as *mut State;
        if state.is_null() {
            return;
        }
        let Some(advertised_seat) = (*state).seat_for_resource(seat) else {
            return;
        };
        (*state).note_client_used_seat(client, advertised_seat);
        let seat_id = (*state).client_routed_seat(client, advertised_seat);
        let ver = ffi::wl_resource_get_version(seat).min(7);
        let k = ffi::wl_resource_create(client, &ffi::wl_keyboard_interface, ver, id);
        if k.is_null() {
            return;
        }
        // Real implementation: `release` (v3+) must have a handler or
        // libwayland aborts when a client sends it. See KEYBOARD_IMPL.
        ffi::wl_resource_set_implementation(
            k,
            &KEYBOARD_IMPL as *const _ as *const c_void,
            state as *mut c_void,
            Some(keyboard_resource_destroy),
        );
        if let Some(runtime) = (*state).seat_runtime_mut(seat_id) {
            if let Some(kb) = &runtime.keyboard {
                // Send the keymap event immediately so the client can decode
                // subsequent key/modifier events. libwayland dups the fd
                // internally; the original stays open for the next client.
                match kb.dup_keymap_fd() {
                    Ok(fd) => {
                        ffi::wl_resource_post_event(
                            k,
                            ffi::WL_KEYBOARD_KEYMAP,
                            ffi::WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1,
                            fd,
                            kb.keymap_size() as u32,
                        );
                    }
                    Err(e) => {
                        log::warn!("[server] keymap fd dup failed: {e}");
                    }
                }
                if ver >= 4 {
                    // Default repeat: 25 cps after 250 ms delay, matching what
                    // Weston and Mutter ship with a fresh install.
                    ffi::wl_resource_post_event(k, ffi::WL_KEYBOARD_REPEAT_INFO, 25u32, 250u32);
                }
            }
            if !runtime.keyboard_focus.is_null()
                && ffi::wl_resource_get_client(runtime.keyboard_focus) == client
            {
                // A newly bound keyboard object joins the seat's existing
                // logical focus. Announce that state immediately; destroying a
                // sibling wl_keyboard resource never changes surface focus.
                let serial = ffi::wl_display_next_serial((*state).display);
                let pressed_keys = runtime
                    .client_pressed_keys
                    .iter()
                    .copied()
                    .collect::<Vec<_>>();
                let keys = keyboard::keycodes_wl_array(&pressed_keys);
                ffi::wl_resource_post_event(
                    k,
                    ffi::WL_KEYBOARD_ENTER,
                    serial,
                    runtime.keyboard_focus,
                    &keys as *const ffi::wl_array as *mut ffi::wl_array,
                );
                let modifiers = runtime
                    .keyboard
                    .as_ref()
                    .map(keyboard::Keyboard::modifiers)
                    .unwrap_or((runtime.depressed_mods.0, 0, 0, 0));
                ffi::wl_resource_post_event(
                    k,
                    ffi::WL_KEYBOARD_MODIFIERS,
                    serial,
                    modifiers.0,
                    modifiers.1,
                    modifiers.2,
                    modifiers.3,
                );
            }
            runtime.keyboard_resources.push(k);
        }
        (*state).track_routed_seat_resource(k, advertised_seat, seat_id);
    }
}

unsafe extern "C" fn keyboard_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(resource) as *mut State;
        if state.is_null() {
            return;
        }
        let Some(seat) = (*state).untrack_seat_resource(resource) else {
            return;
        };
        let Some(runtime) = (*state).seat_runtime_mut(seat) else {
            return;
        };
        if let Some(pos) = runtime
            .keyboard_resources
            .iter()
            .position(|p| *p == resource)
        {
            runtime.keyboard_resources.remove(pos);
        }
    }
}
unsafe extern "C" fn seat_get_touch(
    client: *mut ffi::wl_client,
    seat: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(seat) as *mut State;
        if state.is_null() {
            return;
        }
        let Some(advertised_seat) = (*state).seat_for_resource(seat) else {
            return;
        };
        (*state).note_client_used_seat(client, advertised_seat);
        let seat_id = (*state).client_routed_seat(client, advertised_seat);
        let ver = ffi::wl_resource_get_version(seat).min(8);
        let t = ffi::wl_resource_create(client, &ffi::wl_touch_interface, ver, id);
        if t.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            t,
            &TOUCH_IMPL as *const _ as *const c_void,
            state as *mut c_void,
            Some(touch_resource_destroy),
        );
        (*state).track_routed_seat_resource(t, advertised_seat, seat_id);
        if let Some(runtime) = (*state).seat_runtime_mut(seat_id) {
            runtime.touch_resources.push(t);
        }
    }
}

unsafe extern "C" fn touch_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(resource) as *mut State;
        if state.is_null() {
            return;
        }
        let Some(seat) = (*state).untrack_seat_resource(resource) else {
            return;
        };
        let Some(runtime) = (*state).seat_runtime_mut(seat) else {
            return;
        };
        if let Some(pos) = runtime.touch_resources.iter().position(|p| *p == resource) {
            runtime.touch_resources.remove(pos);
        }
    }
}
