use super::*;

// ----- text-input-unstable-v3 ---------------------------------------------

struct TextInputRec {
    state: *mut State,
    seat: aegis_core::realm::SeatId,
    current_surface: *mut ffi::wl_resource,
    pending: aegis_core::input::TextInputState,
    current: aegis_core::input::TextInputState,
    commit_serial: u32,
}

static TEXT_INPUT_MANAGER_IMPL: ffi::zwp_text_input_manager_v3_interface_impl =
    ffi::zwp_text_input_manager_v3_interface_impl {
        destroy: crate::res_destroy,
        get_text_input: text_input_manager_get,
    };

static TEXT_INPUT_IMPL: ffi::zwp_text_input_v3_interface_impl =
    ffi::zwp_text_input_v3_interface_impl {
        destroy: text_input_destroy,
        enable: text_input_enable,
        disable: text_input_disable,
        set_surrounding_text: text_input_set_surrounding_text,
        set_text_change_cause: text_input_set_text_change_cause,
        set_content_type: text_input_set_content_type,
        set_cursor_rectangle: text_input_set_cursor_rectangle,
        commit: text_input_commit,
    };

pub(crate) unsafe extern "C" fn text_input_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        // Bind at v1: the v2 request set is larger but we only implement v1.
        let res = ffi::wl_resource_create(
            client,
            &ffi::zwp_text_input_manager_v3_interface,
            version.min(1) as c_int,
            id,
        );
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &TEXT_INPUT_MANAGER_IMPL as *const _ as *const c_void,
            data,
            None,
        );
    }
}

unsafe extern "C" fn text_input_manager_get(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    seat: *mut ffi::wl_resource,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(seat) as *mut State;
        if state.is_null() {
            return;
        }
        let Some(advertised_seat) = (*state).seat_for_resource(seat) else {
            return;
        };
        let Some(_guard) =
            crate::ActiveSeatGuard::for_client_seat_resource(state, client, seat, true)
        else {
            return;
        };
        let seat_id = (*state).active_seat;
        let ver = ffi::wl_resource_get_version(mgr).min(1);
        let res = ffi::wl_resource_create(client, &ffi::zwp_text_input_v3_interface, ver, id);
        if res.is_null() {
            return;
        }
        let current_surface = if !state.is_null()
            && !(*state).keyboard_focus.is_null()
            && ffi::wl_resource_get_client((*state).keyboard_focus) == client
        {
            (*state).keyboard_focus
        } else {
            std::ptr::null_mut()
        };
        let rec = Box::into_raw(Box::new(TextInputRec {
            state,
            seat: seat_id,
            current_surface,
            pending: aegis_core::input::TextInputState::default(),
            current: aegis_core::input::TextInputState::default(),
            commit_serial: 0,
        }));
        ffi::wl_resource_set_implementation(
            res,
            &TEXT_INPUT_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            Some(text_input_resource_destroy),
        );
        (*state).track_routed_seat_resource(res, advertised_seat, seat_id);
        (*state).text_inputs.push(res);
        if !current_surface.is_null() {
            ffi::wl_resource_post_event(res, ffi::ZWP_TEXT_INPUT_V3_ENTER, current_surface);
        }
        let _ = mgr;
    }
}

unsafe extern "C" fn text_input_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn text_input_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut TextInputRec;
        if rec.is_null() {
            return;
        }
        let state = (*rec).state;
        if !state.is_null() {
            let _guard = crate::ActiveSeatGuard::enter_existing(&mut *state, (*rec).seat);
            (*state).text_inputs.retain(|r| *r != resource);
            if (*rec).current.enabled {
                route_or_queue_text_input_state(
                    state,
                    (*rec).seat,
                    aegis_core::input::TextInputState::default(),
                );
            }
            (*state).untrack_seat_resource(resource);
        }
        drop(Box::from_raw(rec));
    }
}

/// Mark a text_input as enabled (it now wants IME input) and associate it with
/// the focused surface so enter/leave route correctly.
unsafe extern "C" fn text_input_enable(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut TextInputRec;
        if rec.is_null() || (*rec).current_surface.is_null() {
            return;
        }
        (*rec).pending = aegis_core::input::TextInputState {
            enabled: true,
            ..Default::default()
        };
    }
}

unsafe extern "C" fn text_input_disable(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut TextInputRec;
        if rec.is_null() || (*rec).current_surface.is_null() {
            return;
        }
        (*rec).pending = aegis_core::input::TextInputState::default();
    }
}

unsafe extern "C" fn text_input_set_surrounding_text(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    text: *const std::os::raw::c_char,
    cursor: i32,
    anchor: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut TextInputRec;
        if rec.is_null() || (*rec).current_surface.is_null() || text.is_null() {
            return;
        }
        (*rec).pending.surrounding_text = Some(CStr::from_ptr(text).to_string_lossy().into_owned());
        (*rec).pending.cursor = cursor;
        (*rec).pending.anchor = anchor;
    }
}

unsafe extern "C" fn text_input_set_text_change_cause(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    cause: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut TextInputRec;
        if !rec.is_null() && !(*rec).current_surface.is_null() {
            (*rec).pending.change_cause = cause;
        }
    }
}

unsafe extern "C" fn text_input_set_content_type(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    hint: u32,
    purpose: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut TextInputRec;
        if !rec.is_null() && !(*rec).current_surface.is_null() {
            (*rec).pending.content_hint = hint;
            (*rec).pending.content_purpose = purpose;
        }
    }
}

unsafe extern "C" fn text_input_set_cursor_rectangle(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut TextInputRec;
        if !rec.is_null() && !(*rec).current_surface.is_null() {
            (*rec).pending.cursor_rect = Some((x, y, width, height));
        }
    }
}

unsafe extern "C" fn text_input_commit(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut TextInputRec;
        if rec.is_null() || (*rec).current_surface.is_null() {
            return;
        }
        (*rec).commit_serial = (*rec).commit_serial.wrapping_add(1);
        (*rec).current = (*rec).pending.clone();
        queue_text_input_state(rec);
    }
}

unsafe fn queue_text_input_state(rec: *mut TextInputRec) {
    unsafe {
        let state = (*rec).state;
        if state.is_null() {
            return;
        }
        let Some(_guard) = crate::ActiveSeatGuard::enter(&mut *state, (*rec).seat) else {
            return;
        };
        if (*rec).current_surface != (*state).keyboard_focus {
            return;
        }
        let value = text_input_state_in_compositor_space(rec);
        route_or_queue_text_input_state(state, (*rec).seat, value);
    }
}

unsafe fn route_or_queue_text_input_state(
    state: *mut State,
    seat: aegis_core::realm::SeatId,
    value: aegis_core::input::TextInputState,
) {
    unsafe {
        if !super::route_text_input_state(state, seat, value.clone()) {
            (*state).pending_text_input_states.push(value);
        }
    }
}

unsafe fn text_input_state_in_compositor_space(
    rec: *mut TextInputRec,
) -> aegis_core::input::TextInputState {
    unsafe {
        let mut value = (*rec).current.clone();
        if let Some((x, y, width, height)) = value.cursor_rect {
            let surface = ffi::wl_resource_get_user_data((*rec).current_surface) as *mut SurfaceRec;
            if !surface.is_null() {
                // The cursor rect is surface-local; publish it in compositor
                // space relative to the buffer's draw origin.
                let origin = crate::surface_draw_origin(&*surface);
                value.cursor_rect = Some((x + origin.x, y + origin.y, width, height));
            }
        }
        value
    }
}

pub(crate) unsafe fn current_text_input_state(
    state: *mut State,
    seat: aegis_core::realm::SeatId,
) -> Option<aegis_core::input::TextInputState> {
    unsafe {
        let _guard = crate::ActiveSeatGuard::enter_existing(&mut *state, seat)?;
        let focus = (*state).keyboard_focus;
        if focus.is_null() {
            return None;
        }
        let resources = (*state).text_inputs.clone();
        resources.into_iter().find_map(|resource| {
            let rec = ffi::wl_resource_get_user_data(resource) as *mut TextInputRec;
            if rec.is_null()
                || (*rec).seat != seat
                || (*rec).current_surface != focus
                || !(*rec).current.enabled
            {
                None
            } else {
                Some(text_input_state_in_compositor_space(rec))
            }
        })
    }
}

/// Send `enter` to the focused client's enabled text_inputs and `leave` to
/// those that lost focus. Called from `change_keyboard_focus`.
pub(crate) unsafe fn text_input_focus_changed(
    state: *mut State,
    old_focus: *mut ffi::wl_resource,
    new_focus: *mut ffi::wl_resource,
) {
    unsafe {
        if state.is_null() {
            return;
        }
        // Collect the active text_input resource pointers (cloning the slice to
        // avoid borrowing across libwayland calls).
        let entries: Vec<*mut ffi::wl_resource> = (*state).text_inputs.clone();
        for ti in entries {
            if ti.is_null() {
                continue;
            }
            let ti_client = ffi::wl_resource_get_client(ti);
            let rec = ffi::wl_resource_get_user_data(ti) as *mut TextInputRec;
            if rec.is_null() {
                continue;
            }
            // Leave: text_inputs of the old-focus client.
            if !old_focus.is_null() && ffi::wl_resource_get_client(old_focus) == ti_client {
                ffi::wl_resource_post_event(ti, ffi::ZWP_TEXT_INPUT_V3_LEAVE, old_focus);
                if (*rec).current.enabled {
                    route_or_queue_text_input_state(
                        state,
                        (*rec).seat,
                        aegis_core::input::TextInputState::default(),
                    );
                }
                (*rec).current_surface = std::ptr::null_mut();
                (*rec).pending = Default::default();
                (*rec).current = Default::default();
            }
            // Enter: text_inputs of the new-focus client.
            if !new_focus.is_null() && ffi::wl_resource_get_client(new_focus) == ti_client {
                (*rec).current_surface = new_focus;
                ffi::wl_resource_post_event(ti, ffi::ZWP_TEXT_INPUT_V3_ENTER, new_focus);
            }
        }
    }
}

pub(crate) unsafe fn forward_text_input_event(
    state: *mut State,
    event: &aegis_core::input::TextInputEvent,
) {
    unsafe {
        if state.is_null() || (*state).keyboard_focus.is_null() {
            return;
        }
        let focus = (*state).keyboard_focus;
        let client = ffi::wl_resource_get_client(focus);
        let targets: Vec<*mut ffi::wl_resource> = (*state)
            .text_inputs
            .iter()
            .copied()
            .filter(|r| !r.is_null() && ffi::wl_resource_get_client(*r) == client)
            .filter(|r| {
                let rec = ffi::wl_resource_get_user_data(*r) as *mut TextInputRec;
                !rec.is_null() && (*rec).current_surface == focus && (*rec).current.enabled
            })
            .collect();
        for target in targets {
            let rec = ffi::wl_resource_get_user_data(target) as *mut TextInputRec;
            match event {
                aegis_core::input::TextInputEvent::Preedit {
                    text,
                    cursor_begin,
                    cursor_end,
                } => {
                    let text = text.as_ref().and_then(|s| CString::new(s.as_str()).ok());
                    ffi::wl_resource_post_event(
                        target,
                        ffi::ZWP_TEXT_INPUT_V3_PREEDIT_STRING,
                        text.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                        *cursor_begin,
                        *cursor_end,
                    );
                }
                aegis_core::input::TextInputEvent::Commit(text) => {
                    let text = text.as_ref().and_then(|s| CString::new(s.as_str()).ok());
                    ffi::wl_resource_post_event(
                        target,
                        ffi::ZWP_TEXT_INPUT_V3_COMMIT_STRING,
                        text.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                    );
                }
                aegis_core::input::TextInputEvent::DeleteSurrounding {
                    before_length,
                    after_length,
                } => ffi::wl_resource_post_event(
                    target,
                    ffi::ZWP_TEXT_INPUT_V3_DELETE_SURROUNDING_TEXT,
                    *before_length,
                    *after_length,
                ),
                aegis_core::input::TextInputEvent::Done => ffi::wl_resource_post_event(
                    target,
                    ffi::ZWP_TEXT_INPUT_V3_DONE,
                    (*rec).commit_serial,
                ),
            }
        }
    }
}
