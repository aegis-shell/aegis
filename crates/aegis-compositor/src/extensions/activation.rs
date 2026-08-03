use super::*;

// ----- xdg-activation-v1 --------------------------------------------------

struct ActivationTokenRec {
    state: *mut State,
    client: *mut ffi::wl_client,
    serial: Option<u32>,
    seat: Option<aegis_core::interaction_domain::SeatId>,
    surface: *mut ffi::wl_resource,
    committed: bool,
}

static XDG_ACTIVATION_IMPL: ffi::xdg_activation_v1_interface_impl =
    ffi::xdg_activation_v1_interface_impl {
        destroy: crate::res_destroy,
        get_activation_token: xdg_activation_get_token,
        activate: xdg_activation_activate,
    };

static XDG_ACTIVATION_TOKEN_IMPL: ffi::xdg_activation_token_v1_interface_impl =
    ffi::xdg_activation_token_v1_interface_impl {
        set_serial: activation_token_set_serial,
        set_app_id: activation_token_set_app_id,
        set_surface: activation_token_set_surface,
        commit: activation_token_commit,
        destroy: crate::res_destroy,
    };

pub(crate) unsafe extern "C" fn xdg_activation_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let resource = ffi::wl_resource_create(
            client,
            &ffi::xdg_activation_v1_interface,
            version.min(1) as c_int,
            id,
        );
        if !resource.is_null() {
            ffi::wl_resource_set_implementation(
                resource,
                &XDG_ACTIVATION_IMPL as *const _ as *const c_void,
                data,
                None,
            );
        }
    }
}

unsafe extern "C" fn xdg_activation_get_token(
    client: *mut ffi::wl_client,
    activation: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(activation) as *mut State;
        let resource =
            ffi::wl_resource_create(client, &ffi::xdg_activation_token_v1_interface, 1, id);
        if resource.is_null() {
            return;
        }
        let rec = Box::into_raw(Box::new(ActivationTokenRec {
            state,
            client,
            serial: None,
            seat: None,
            surface: std::ptr::null_mut(),
            committed: false,
        }));
        ffi::wl_resource_set_implementation(
            resource,
            &XDG_ACTIVATION_TOKEN_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            Some(activation_token_resource_destroy),
        );
    }
}

unsafe extern "C" fn activation_token_set_serial(
    client: *mut ffi::wl_client,
    token: *mut ffi::wl_resource,
    serial: u32,
    seat: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(token) as *mut ActivationTokenRec;
        if rec.is_null() || (*rec).committed || (*rec).state.is_null() {
            return;
        }
        let state = (*rec).state;
        let Some(_guard) =
            crate::ActiveSeatGuard::for_client_seat_resource(state, client, seat, true)
        else {
            return;
        };
        if ffi::wl_resource_get_client(seat) == (*rec).client
            && serial == (*state).last_button_serial
        {
            (*rec).serial = Some(serial);
            (*rec).seat = Some((*state).active_seat);
        }
    }
}

unsafe extern "C" fn activation_token_set_app_id(
    _client: *mut ffi::wl_client,
    token: *mut ffi::wl_resource,
    _app_id: *const std::os::raw::c_char,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(token) as *mut ActivationTokenRec;
        if !rec.is_null() && (*rec).committed {
            ffi::wl_resource_post_error(
                token,
                ffi::XDG_ACTIVATION_TOKEN_V1_ERROR_ALREADY_USED,
                c"activation token already committed".as_ptr(),
            );
        }
    }
}

unsafe extern "C" fn activation_token_set_surface(
    client: *mut ffi::wl_client,
    token: *mut ffi::wl_resource,
    surface: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(token) as *mut ActivationTokenRec;
        if rec.is_null() || (*rec).committed {
            return;
        }
        if !surface.is_null() && ffi::wl_resource_get_client(surface) == client {
            (*rec).surface = surface;
        }
    }
}

unsafe extern "C" fn activation_token_commit(
    _client: *mut ffi::wl_client,
    token_resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(token_resource) as *mut ActivationTokenRec;
        if rec.is_null() {
            return;
        }
        if (*rec).committed {
            ffi::wl_resource_post_error(
                token_resource,
                ffi::XDG_ACTIVATION_TOKEN_V1_ERROR_ALREADY_USED,
                c"activation token already committed".as_ptr(),
            );
            return;
        }
        (*rec).committed = true;
        let state = (*rec).state;
        let serial = if state.is_null() {
            0
        } else {
            ffi::wl_display_next_serial((*state).display)
        };
        let token = format!("aegis-activation-{serial:08x}");
        if !state.is_null()
            && let Some(seat) = (*rec).seat
            && let Some(_guard) = crate::ActiveSeatGuard::enter(&mut *state, seat)
        {
            let valid_focus = !(*state).keyboard_focus.is_null()
                && ffi::wl_resource_get_client((*state).keyboard_focus) == (*rec).client;
            let valid_surface = (*rec).surface.is_null()
                || ffi::wl_resource_get_client((*rec).surface) == (*rec).client;
            let valid_serial = (*rec)
                .serial
                .is_some_and(|serial| serial == (*state).last_button_serial);
            if valid_focus && valid_surface && valid_serial {
                (*state).activation_tokens.insert(token.clone(), seat);
            }
        }
        if let Ok(token) = CString::new(token) {
            ffi::wl_resource_post_event(
                token_resource,
                ffi::XDG_ACTIVATION_TOKEN_V1_DONE,
                token.as_ptr(),
            );
        }
    }
}

unsafe extern "C" fn activation_token_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut ActivationTokenRec;
        if !rec.is_null() {
            drop(Box::from_raw(rec));
        }
    }
}

unsafe extern "C" fn xdg_activation_activate(
    client: *mut ffi::wl_client,
    activation: *mut ffi::wl_resource,
    token: *const std::os::raw::c_char,
    surface: *mut ffi::wl_resource,
) {
    unsafe {
        if token.is_null() || surface.is_null() || ffi::wl_resource_get_client(surface) != client {
            return;
        }
        let state = ffi::wl_resource_get_user_data(activation) as *mut State;
        if state.is_null() {
            return;
        }
        let token = CStr::from_ptr(token).to_string_lossy();
        if let Some(seat) = (*state).activation_tokens.remove(token.as_ref()) {
            let Some(_guard) = crate::ActiveSeatGuard::enter(&mut *state, seat) else {
                return;
            };
            let rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
            let root = crate::surface_root_toplevel(rec);
            if !root.is_null()
                && (*state)
                    .authority
                    .seat_controls_window(seat, (*root).window.id)
            {
                (*state).pending_activation = Some((seat, surface));
            }
        }
    }
}
