use super::*;

use aegis_core::input::{ButtonState, TextInputEvent, TextInputState};
use aegis_core::realm::SeatId;

const MAX_SURROUNDING_TEXT_BYTES: usize = 4_000;

fn truncate_utf8_bytes(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

#[derive(Default)]
struct PendingInputMethodEdit {
    commit: Option<String>,
    preedit: Option<(String, i32, i32)>,
    delete: Option<(u32, u32)>,
}

struct InputMethodRec {
    state: *mut State,
    seat: SeatId,
    unavailable: bool,
    active: bool,
    done_count: u32,
    text_state: TextInputState,
    pending: PendingInputMethodEdit,
    popups: Vec<*mut ffi::wl_resource>,
    keyboard_grab: *mut ffi::wl_resource,
}

struct InputPopupRec {
    state: *mut State,
    seat: SeatId,
    input_method: *mut ffi::wl_resource,
    surface: *mut SurfaceRec,
}

struct InputMethodKeyboardGrabRec {
    state: *mut State,
    seat: SeatId,
    input_method: *mut ffi::wl_resource,
}

struct VirtualKeyboardRec {
    state: *mut State,
    seat: SeatId,
    keymap_set: bool,
}

static INPUT_METHOD_MANAGER_IMPL: ffi::zwp_input_method_manager_v2_interface_impl =
    ffi::zwp_input_method_manager_v2_interface_impl {
        get_input_method: input_method_manager_get,
        destroy: crate::res_destroy,
    };

static INPUT_METHOD_IMPL: ffi::zwp_input_method_v2_interface_impl =
    ffi::zwp_input_method_v2_interface_impl {
        commit_string: input_method_commit_string,
        set_preedit_string: input_method_set_preedit,
        delete_surrounding_text: input_method_delete_surrounding,
        commit: input_method_commit,
        get_input_popup_surface: input_method_get_popup,
        grab_keyboard: input_method_grab_keyboard,
        destroy: input_method_destroy,
    };

static INPUT_POPUP_IMPL: ffi::zwp_input_popup_surface_v2_interface_impl =
    ffi::zwp_input_popup_surface_v2_interface_impl {
        destroy: input_popup_destroy,
    };

static INPUT_METHOD_KEYBOARD_GRAB_IMPL: ffi::zwp_input_method_keyboard_grab_v2_interface_impl =
    ffi::zwp_input_method_keyboard_grab_v2_interface_impl {
        release: input_method_keyboard_grab_release,
    };

static VIRTUAL_KEYBOARD_MANAGER_IMPL: ffi::zwp_virtual_keyboard_manager_v1_interface_impl =
    ffi::zwp_virtual_keyboard_manager_v1_interface_impl {
        create_virtual_keyboard: virtual_keyboard_manager_create,
    };

static VIRTUAL_KEYBOARD_IMPL: ffi::zwp_virtual_keyboard_v1_interface_impl =
    ffi::zwp_virtual_keyboard_v1_interface_impl {
        keymap: virtual_keyboard_keymap,
        key: virtual_keyboard_key,
        modifiers: virtual_keyboard_modifiers,
        destroy: virtual_keyboard_destroy,
    };

pub(crate) unsafe extern "C" fn input_method_manager_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let resource = ffi::wl_resource_create(
            client,
            &ffi::zwp_input_method_manager_v2_interface,
            version.min(1) as c_int,
            id,
        );
        if resource.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            resource,
            &INPUT_METHOD_MANAGER_IMPL as *const _ as *const c_void,
            data,
            None,
        );
    }
}

pub(crate) unsafe extern "C" fn virtual_keyboard_manager_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let resource = ffi::wl_resource_create(
            client,
            &ffi::zwp_virtual_keyboard_manager_v1_interface,
            version.min(1) as c_int,
            id,
        );
        if resource.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            resource,
            &VIRTUAL_KEYBOARD_MANAGER_IMPL as *const _ as *const c_void,
            data,
            None,
        );
    }
}

unsafe extern "C" fn input_method_manager_get(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    seat_resource: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(manager) as *mut State;
        if state.is_null() {
            return;
        }
        let Some(advertised_seat) = (*state).seat_for_resource(seat_resource) else {
            return;
        };
        let Some(_guard) =
            crate::ActiveSeatGuard::for_client_seat_resource(state, client, seat_resource, true)
        else {
            return;
        };
        let seat = (*state).active_seat;
        let unavailable = !active_input_method(state, seat).is_null();
        let resource = ffi::wl_resource_create(client, &ffi::zwp_input_method_v2_interface, 1, id);
        if resource.is_null() {
            return;
        }
        let rec = Box::into_raw(Box::new(InputMethodRec {
            state,
            seat,
            unavailable,
            active: false,
            done_count: 0,
            text_state: TextInputState::default(),
            pending: PendingInputMethodEdit::default(),
            popups: Vec::new(),
            keyboard_grab: std::ptr::null_mut(),
        }));
        ffi::wl_resource_set_implementation(
            resource,
            &INPUT_METHOD_IMPL as *const _ as *const c_void,
            rec.cast(),
            Some(input_method_resource_destroy),
        );
        (*state).track_routed_seat_resource(resource, advertised_seat, seat);
        (*state).input_methods.push(resource);
        if unavailable {
            ffi::wl_resource_post_event(resource, ffi::ZWP_INPUT_METHOD_V2_UNAVAILABLE);
            return;
        }
        if let Some(current) = super::current_text_input_state(state, seat) {
            publish_text_input_state(resource, current);
        }
    }
}

unsafe fn active_input_method(state: *mut State, seat: SeatId) -> *mut ffi::wl_resource {
    unsafe {
        if state.is_null() {
            return std::ptr::null_mut();
        }
        (*state)
            .seat_runtime(seat)
            .into_iter()
            .flat_map(|runtime| runtime.input_methods.iter().copied())
            .find(|resource| {
                if resource.is_null() {
                    return false;
                }
                let rec = ffi::wl_resource_get_user_data(*resource) as *mut InputMethodRec;
                !rec.is_null() && !(*rec).unavailable
            })
            .unwrap_or(std::ptr::null_mut())
    }
}

pub(crate) unsafe fn route_text_input_state(
    state: *mut State,
    seat: SeatId,
    text_state: TextInputState,
) -> bool {
    unsafe {
        let input_method = active_input_method(state, seat);
        if input_method.is_null() {
            return false;
        }
        publish_text_input_state(input_method, text_state);
        true
    }
}

unsafe fn publish_text_input_state(
    input_method: *mut ffi::wl_resource,
    text_state: TextInputState,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(input_method) as *mut InputMethodRec;
        if rec.is_null() || (*rec).unavailable {
            return;
        }
        let was_active = (*rec).active;
        if !text_state.enabled {
            if was_active {
                ffi::wl_resource_post_event(input_method, ffi::ZWP_INPUT_METHOD_V2_DEACTIVATE);
                send_input_method_done(input_method, rec);
            }
            (*rec).active = false;
            (*rec).text_state = TextInputState::default();
            (*rec).pending = PendingInputMethodEdit::default();
            update_input_popup_positions((*rec).state, (*rec).seat);
            return;
        }

        if !was_active {
            (*rec).pending = PendingInputMethodEdit::default();
            ffi::wl_resource_post_event(input_method, ffi::ZWP_INPUT_METHOD_V2_ACTIVATE);
        }
        if let Some(text) = text_state.surrounding_text.as_deref()
            && let Ok(text) = CString::new(truncate_utf8_bytes(text, MAX_SURROUNDING_TEXT_BYTES))
        {
            let len = text.as_bytes().len() as i32;
            ffi::wl_resource_post_event(
                input_method,
                ffi::ZWP_INPUT_METHOD_V2_SURROUNDING_TEXT,
                text.as_ptr(),
                text_state.cursor.clamp(0, len) as u32,
                text_state.anchor.clamp(0, len) as u32,
            );
        }
        ffi::wl_resource_post_event(
            input_method,
            ffi::ZWP_INPUT_METHOD_V2_TEXT_CHANGE_CAUSE,
            text_state.change_cause,
        );
        ffi::wl_resource_post_event(
            input_method,
            ffi::ZWP_INPUT_METHOD_V2_CONTENT_TYPE,
            text_state.content_hint,
            text_state.content_purpose,
        );
        (*rec).active = true;
        (*rec).text_state = text_state;
        send_input_method_done(input_method, rec);
        update_input_popup_positions((*rec).state, (*rec).seat);
    }
}

unsafe fn send_input_method_done(input_method: *mut ffi::wl_resource, rec: *mut InputMethodRec) {
    unsafe {
        (*rec).done_count = (*rec).done_count.wrapping_add(1);
        ffi::wl_resource_post_event(input_method, ffi::ZWP_INPUT_METHOD_V2_DONE);
    }
}

unsafe extern "C" fn input_method_commit_string(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    text: *const std::os::raw::c_char,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut InputMethodRec;
        if rec.is_null() || (*rec).unavailable || text.is_null() {
            return;
        }
        (*rec).pending.commit = Some(CStr::from_ptr(text).to_string_lossy().into_owned());
    }
}

unsafe extern "C" fn input_method_set_preedit(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    text: *const std::os::raw::c_char,
    cursor_begin: i32,
    cursor_end: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut InputMethodRec;
        if rec.is_null() || (*rec).unavailable || text.is_null() {
            return;
        }
        (*rec).pending.preedit = Some((
            CStr::from_ptr(text).to_string_lossy().into_owned(),
            cursor_begin,
            cursor_end,
        ));
    }
}

unsafe extern "C" fn input_method_delete_surrounding(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    before_length: u32,
    after_length: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut InputMethodRec;
        if rec.is_null() || (*rec).unavailable {
            return;
        }
        (*rec).pending.delete = Some((before_length, after_length));
    }
}

unsafe extern "C" fn input_method_commit(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    serial: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut InputMethodRec;
        if rec.is_null() || (*rec).unavailable {
            return;
        }
        let pending = std::mem::take(&mut (*rec).pending);
        if !(*rec).active || serial != (*rec).done_count {
            return;
        }
        let state = (*rec).state;
        let Some(_guard) = crate::ActiveSeatGuard::enter(&mut *state, (*rec).seat) else {
            return;
        };
        if let Some((text, cursor_begin, cursor_end)) = pending.preedit {
            super::forward_text_input_event(
                state,
                &TextInputEvent::Preedit {
                    text: Some(text),
                    cursor_begin,
                    cursor_end,
                },
            );
        }
        if let Some((before_length, after_length)) = pending.delete {
            super::forward_text_input_event(
                state,
                &TextInputEvent::DeleteSurrounding {
                    before_length,
                    after_length,
                },
            );
        }
        if let Some(text) = pending.commit {
            super::forward_text_input_event(state, &TextInputEvent::Commit(Some(text)));
        }
        super::forward_text_input_event(state, &TextInputEvent::Done);
    }
}

unsafe extern "C" fn input_method_get_popup(
    client: *mut ffi::wl_client,
    input_method: *mut ffi::wl_resource,
    id: u32,
    surface_resource: *mut ffi::wl_resource,
) {
    unsafe {
        let input_method_rec = ffi::wl_resource_get_user_data(input_method) as *mut InputMethodRec;
        if input_method_rec.is_null() || (*input_method_rec).unavailable {
            return;
        }
        let surface = ffi::wl_resource_get_user_data(surface_resource) as *mut SurfaceRec;
        if surface.is_null() {
            return;
        }
        if crate::surface_has_role(&*surface) {
            ffi::wl_resource_post_error(
                input_method,
                ffi::ZWP_INPUT_METHOD_V2_ERROR_ROLE,
                c"wl_surface already has a role".as_ptr(),
            );
            return;
        }
        let resource =
            ffi::wl_resource_create(client, &ffi::zwp_input_popup_surface_v2_interface, 1, id);
        if resource.is_null() {
            return;
        }
        let rec = Box::into_raw(Box::new(InputPopupRec {
            state: (*input_method_rec).state,
            seat: (*input_method_rec).seat,
            input_method,
            surface,
        }));
        ffi::wl_resource_set_implementation(
            resource,
            &INPUT_POPUP_IMPL as *const _ as *const c_void,
            rec.cast(),
            Some(input_popup_resource_destroy),
        );
        (*surface).input_popup_role = true;
        (*surface).input_popup_surface = rec.cast();
        (*input_method_rec).popups.push(resource);
        (*(*input_method_rec).state).track_seat_resource(resource, (*input_method_rec).seat);
        update_input_popup_positions((*input_method_rec).state, (*input_method_rec).seat);
    }
}

unsafe extern "C" fn input_popup_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe { ffi::wl_resource_destroy(resource) };
}

unsafe extern "C" fn input_popup_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut InputPopupRec;
        if rec.is_null() {
            return;
        }
        if !(*rec).surface.is_null() {
            (*(*rec).surface).input_popup_surface = std::ptr::null_mut();
        }
        if !(*rec).input_method.is_null() {
            let input_method_rec =
                ffi::wl_resource_get_user_data((*rec).input_method) as *mut InputMethodRec;
            if !input_method_rec.is_null() {
                (*input_method_rec)
                    .popups
                    .retain(|candidate| *candidate != resource);
            }
        }
        if !(*rec).state.is_null() {
            (*(*rec).state).untrack_seat_resource(resource);
        }
        drop(Box::from_raw(rec));
    }
}

unsafe extern "C" fn input_method_grab_keyboard(
    client: *mut ffi::wl_client,
    input_method: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let input_method_rec = ffi::wl_resource_get_user_data(input_method) as *mut InputMethodRec;
        if input_method_rec.is_null() || (*input_method_rec).unavailable {
            return;
        }
        if !(*input_method_rec).keyboard_grab.is_null() {
            ffi::wl_resource_destroy((*input_method_rec).keyboard_grab);
        }
        let resource = ffi::wl_resource_create(
            client,
            &ffi::zwp_input_method_keyboard_grab_v2_interface,
            1,
            id,
        );
        if resource.is_null() {
            return;
        }
        let rec = Box::into_raw(Box::new(InputMethodKeyboardGrabRec {
            state: (*input_method_rec).state,
            seat: (*input_method_rec).seat,
            input_method,
        }));
        ffi::wl_resource_set_implementation(
            resource,
            &INPUT_METHOD_KEYBOARD_GRAB_IMPL as *const _ as *const c_void,
            rec.cast(),
            Some(input_method_keyboard_grab_resource_destroy),
        );
        (*input_method_rec).keyboard_grab = resource;
        (*(*input_method_rec).state).track_seat_resource(resource, (*input_method_rec).seat);
        let state = (*input_method_rec).state;
        let Some(_guard) = crate::ActiveSeatGuard::enter(&mut *state, (*input_method_rec).seat)
        else {
            return;
        };
        if let Some(keyboard) = (*state).keyboard.as_ref()
            && let Ok(fd) = keyboard.dup_keymap_fd()
        {
            ffi::wl_resource_post_event(
                resource,
                ffi::ZWP_INPUT_METHOD_KEYBOARD_GRAB_V2_KEYMAP,
                ffi::WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1,
                fd,
                keyboard.keymap_size() as u32,
            );
        }
        ffi::wl_resource_post_event(
            resource,
            ffi::ZWP_INPUT_METHOD_KEYBOARD_GRAB_V2_REPEAT_INFO,
            25i32,
            600i32,
        );
    }
}

unsafe extern "C" fn input_method_keyboard_grab_release(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe { ffi::wl_resource_destroy(resource) };
}

unsafe extern "C" fn input_method_keyboard_grab_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut InputMethodKeyboardGrabRec;
        if rec.is_null() {
            return;
        }
        if !(*rec).input_method.is_null() {
            let input_method_rec =
                ffi::wl_resource_get_user_data((*rec).input_method) as *mut InputMethodRec;
            if !input_method_rec.is_null() && (*input_method_rec).keyboard_grab == resource {
                (*input_method_rec).keyboard_grab = std::ptr::null_mut();
            }
        }
        if !(*rec).state.is_null() {
            (*(*rec).state).untrack_seat_resource(resource);
        }
        drop(Box::from_raw(rec));
    }
}

pub(crate) unsafe fn input_method_grab_key(
    state: *mut State,
    evdev_code: u32,
    button_state: ButtonState,
    outcome: crate::keyboard::KeyOutcome,
    time: u32,
) -> bool {
    unsafe {
        let seat = (*state).active_seat;
        let input_method = active_input_method(state, seat);
        if input_method.is_null() {
            return false;
        }
        let rec = ffi::wl_resource_get_user_data(input_method) as *mut InputMethodRec;
        if rec.is_null() || !(*rec).active || (*rec).keyboard_grab.is_null() {
            return false;
        }
        let serial = ffi::wl_display_next_serial((*state).display);
        ffi::wl_resource_post_event(
            (*rec).keyboard_grab,
            ffi::ZWP_INPUT_METHOD_KEYBOARD_GRAB_V2_MODIFIERS,
            serial,
            outcome.depressed,
            outcome.latched,
            outcome.locked,
            outcome.group,
        );
        ffi::wl_resource_post_event(
            (*rec).keyboard_grab,
            ffi::ZWP_INPUT_METHOD_KEYBOARD_GRAB_V2_KEY,
            serial,
            time,
            evdev_code,
            u32::from(button_state.is_pressed()),
        );
        true
    }
}

unsafe extern "C" fn virtual_keyboard_manager_create(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    seat_resource: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(manager) as *mut State;
        if state.is_null() {
            return;
        }
        let Some(advertised_seat) = (*state).seat_for_resource(seat_resource) else {
            return;
        };
        let Some(_guard) =
            crate::ActiveSeatGuard::for_client_seat_resource(state, client, seat_resource, true)
        else {
            return;
        };
        let seat = (*state).active_seat;
        let resource =
            ffi::wl_resource_create(client, &ffi::zwp_virtual_keyboard_v1_interface, 1, id);
        if resource.is_null() {
            return;
        }
        let rec = Box::into_raw(Box::new(VirtualKeyboardRec {
            state,
            seat,
            keymap_set: false,
        }));
        ffi::wl_resource_set_implementation(
            resource,
            &VIRTUAL_KEYBOARD_IMPL as *const _ as *const c_void,
            rec.cast(),
            Some(virtual_keyboard_resource_destroy),
        );
        (*state).track_routed_seat_resource(resource, advertised_seat, seat);
        (*state).virtual_keyboards.push(resource);
    }
}

unsafe fn virtual_keyboard_authorized(
    resource: *mut ffi::wl_resource,
) -> Option<*mut VirtualKeyboardRec> {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut VirtualKeyboardRec;
        if rec.is_null() || (*rec).state.is_null() {
            return None;
        }
        let input_method = active_input_method((*rec).state, (*rec).seat);
        if input_method.is_null()
            || ffi::wl_resource_get_client(input_method) != ffi::wl_resource_get_client(resource)
        {
            return None;
        }
        Some(rec)
    }
}

unsafe extern "C" fn virtual_keyboard_keymap(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    format: u32,
    fd: i32,
    size: u32,
) {
    unsafe {
        let rec = virtual_keyboard_authorized(resource);
        if let Some(rec) = rec {
            (*rec).keymap_set =
                format == ffi::WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1 && size > 0 && size <= 1_048_576;
        }
        crate::libc_close(fd);
    }
}

unsafe extern "C" fn virtual_keyboard_key(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    time: u32,
    key: u32,
    key_state: u32,
) {
    unsafe {
        let Some(rec) = virtual_keyboard_authorized(resource) else {
            return;
        };
        if !(*rec).keymap_set {
            ffi::wl_resource_post_error(
                resource,
                ffi::ZWP_VIRTUAL_KEYBOARD_V1_ERROR_NO_KEYMAP,
                c"virtual keyboard keymap was not set".as_ptr(),
            );
            return;
        }
        if key_state > 1 || key > 0x2ff {
            return;
        }
        let state = (*rec).state;
        let Some(_guard) = crate::ActiveSeatGuard::enter(&mut *state, (*rec).seat) else {
            return;
        };
        if (*state).keyboard_focus.is_null() {
            return;
        }
        let serial = ffi::wl_display_next_serial((*state).display);
        let focus_client = ffi::wl_resource_get_client((*state).keyboard_focus);
        for keyboard in (*state)
            .keyboard_resources
            .iter()
            .copied()
            .filter(|keyboard| ffi::wl_resource_get_client(*keyboard) == focus_client)
        {
            ffi::wl_resource_post_event(
                keyboard,
                ffi::WL_KEYBOARD_KEY,
                serial,
                time,
                key,
                key_state,
            );
        }
    }
}

unsafe extern "C" fn virtual_keyboard_modifiers(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    depressed: u32,
    latched: u32,
    locked: u32,
    group: u32,
) {
    unsafe {
        let Some(rec) = virtual_keyboard_authorized(resource) else {
            return;
        };
        if !(*rec).keymap_set {
            ffi::wl_resource_post_error(
                resource,
                ffi::ZWP_VIRTUAL_KEYBOARD_V1_ERROR_NO_KEYMAP,
                c"virtual keyboard keymap was not set".as_ptr(),
            );
            return;
        }
        let state = (*rec).state;
        let Some(_guard) = crate::ActiveSeatGuard::enter(&mut *state, (*rec).seat) else {
            return;
        };
        if (*state).keyboard_focus.is_null() {
            return;
        }
        let serial = ffi::wl_display_next_serial((*state).display);
        let focus_client = ffi::wl_resource_get_client((*state).keyboard_focus);
        for keyboard in (*state)
            .keyboard_resources
            .iter()
            .copied()
            .filter(|keyboard| ffi::wl_resource_get_client(*keyboard) == focus_client)
        {
            ffi::wl_resource_post_event(
                keyboard,
                ffi::WL_KEYBOARD_MODIFIERS,
                serial,
                depressed,
                latched,
                locked,
                group,
            );
        }
    }
}

unsafe extern "C" fn virtual_keyboard_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe { ffi::wl_resource_destroy(resource) };
}

unsafe extern "C" fn virtual_keyboard_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut VirtualKeyboardRec;
        if rec.is_null() {
            return;
        }
        if !(*rec).state.is_null() {
            let _guard = crate::ActiveSeatGuard::enter_existing(&mut *(*rec).state, (*rec).seat);
            (*(*rec).state)
                .virtual_keyboards
                .retain(|candidate| *candidate != resource);
            (*(*rec).state).untrack_seat_resource(resource);
        }
        drop(Box::from_raw(rec));
    }
}

unsafe extern "C" fn input_method_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe { ffi::wl_resource_destroy(resource) };
}

unsafe extern "C" fn input_method_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut InputMethodRec;
        if rec.is_null() {
            return;
        }
        let state = (*rec).state;
        let seat = (*rec).seat;
        let client = ffi::wl_resource_get_client(resource);
        let grab = std::mem::replace(&mut (*rec).keyboard_grab, std::ptr::null_mut());
        let popups = std::mem::take(&mut (*rec).popups);
        if !grab.is_null() {
            ffi::wl_resource_destroy(grab);
        }
        for popup in popups {
            if !popup.is_null() {
                ffi::wl_resource_destroy(popup);
            }
        }
        if !state.is_null() {
            let _guard = crate::ActiveSeatGuard::enter_existing(&mut *state, seat);
            (*state)
                .input_methods
                .retain(|candidate| *candidate != resource);
            let virtual_keyboards = (*state)
                .virtual_keyboards
                .iter()
                .copied()
                .filter(|keyboard| ffi::wl_resource_get_client(*keyboard) == client)
                .collect::<Vec<_>>();
            for keyboard in virtual_keyboards {
                ffi::wl_resource_destroy(keyboard);
            }
            (*state).untrack_seat_resource(resource);
            if !(*rec).unavailable
                && let Some(current) = super::current_text_input_state(state, seat)
            {
                (*state).pending_text_input_states.push(current);
            }
        }
        drop(Box::from_raw(rec));
    }
}

pub(crate) unsafe fn input_popup_surface_destroyed(surface: *mut SurfaceRec) {
    unsafe {
        if surface.is_null() || (*surface).input_popup_surface.is_null() {
            return;
        }
        let popup = (*surface).input_popup_surface as *mut InputPopupRec;
        (*popup).surface = std::ptr::null_mut();
        (*surface).input_popup_surface = std::ptr::null_mut();
    }
}

pub(crate) unsafe fn input_popup_surface_committed(surface: *mut SurfaceRec) {
    unsafe {
        if surface.is_null() || (*surface).input_popup_surface.is_null() {
            return;
        }
        let popup = (*surface).input_popup_surface as *mut InputPopupRec;
        update_input_popup_positions((*popup).state, (*popup).seat);
    }
}

pub(crate) unsafe fn input_popup_resources(
    state: *mut State,
    seat: SeatId,
) -> Vec<*mut ffi::wl_resource> {
    unsafe {
        let input_method = active_input_method(state, seat);
        if input_method.is_null() {
            return Vec::new();
        }
        let rec = ffi::wl_resource_get_user_data(input_method) as *mut InputMethodRec;
        if rec.is_null() || !(*rec).active {
            return Vec::new();
        }
        (*rec)
            .popups
            .iter()
            .copied()
            .filter_map(|popup_resource| {
                let popup = ffi::wl_resource_get_user_data(popup_resource) as *mut InputPopupRec;
                if popup.is_null() || (*popup).surface.is_null() {
                    None
                } else {
                    Some((*(*popup).surface).resource)
                }
            })
            .collect()
    }
}

pub(crate) unsafe fn input_popup_surface_visible(surface: *const SurfaceRec) -> bool {
    unsafe {
        if surface.is_null() || (*surface).input_popup_surface.is_null() {
            return false;
        }
        let popup = (*surface).input_popup_surface as *mut InputPopupRec;
        let input_method_rec =
            ffi::wl_resource_get_user_data((*popup).input_method) as *mut InputMethodRec;
        !input_method_rec.is_null() && (*input_method_rec).active
    }
}

fn input_popup_position(
    anchor: aegis_core::Rect,
    popup_size: aegis_core::Size,
    output: aegis_core::Rect,
) -> aegis_core::Point {
    let max_x = output
        .origin
        .x
        .saturating_add(output.size.w.saturating_sub(popup_size.w).max(0));
    let max_y = output
        .origin
        .y
        .saturating_add(output.size.h.saturating_sub(popup_size.h).max(0));
    let below = anchor.origin.y.saturating_add(anchor.size.h);
    let above = anchor.origin.y.saturating_sub(popup_size.h);
    let output_bottom = output.origin.y.saturating_add(output.size.h);
    let popup_y = if below.saturating_add(popup_size.h) <= output_bottom {
        below
    } else {
        above
    };
    aegis_core::Point {
        x: anchor.origin.x.clamp(output.origin.x, max_x),
        y: popup_y.clamp(output.origin.y, max_y),
    }
}

unsafe fn update_input_popup_positions(state: *mut State, seat: SeatId) {
    unsafe {
        let input_method = active_input_method(state, seat);
        if input_method.is_null() {
            return;
        }
        let rec = ffi::wl_resource_get_user_data(input_method) as *mut InputMethodRec;
        if rec.is_null() || !(*rec).active {
            return;
        }
        let Some((x, y, width, height)) = (*rec).text_state.cursor_rect else {
            return;
        };
        let anchor = aegis_core::Rect::new(x, y, width.max(1), height.max(1));
        let output = (*state)
            .output_infos
            .iter()
            .map(|output| output.geometry.logical_rect())
            .find(|output| output.contains(anchor.origin))
            .unwrap_or_else(|| (*state).output_geometry.logical_rect());
        for popup_resource in (*rec).popups.clone() {
            let popup = ffi::wl_resource_get_user_data(popup_resource) as *mut InputPopupRec;
            if popup.is_null() || (*popup).surface.is_null() {
                continue;
            }
            let surface = (*popup).surface;
            let popup_size = crate::surface_logical_size(&*surface);
            (*surface).position = input_popup_position(anchor, popup_size, output);
            ffi::wl_resource_post_event(
                popup_resource,
                ffi::ZWP_INPUT_POPUP_SURFACE_V2_TEXT_INPUT_RECTANGLE,
                anchor.origin.x - (*surface).position.x,
                anchor.origin.y - (*surface).position.y,
                anchor.size.w,
                anchor.size.h,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surrounding_text_truncation_preserves_utf8_boundaries() {
        let text = format!("{}界", "a".repeat(MAX_SURROUNDING_TEXT_BYTES - 1));
        let truncated = truncate_utf8_bytes(&text, MAX_SURROUNDING_TEXT_BYTES);
        assert_eq!(truncated.len(), MAX_SURROUNDING_TEXT_BYTES - 1);
        assert!(truncated.is_char_boundary(truncated.len()));
    }

    #[test]
    fn popup_prefers_below_the_cursor_and_clamps_horizontally() {
        let output = aegis_core::Rect::new(100, 50, 800, 600);
        let anchor = aegis_core::Rect::new(880, 200, 2, 20);
        let popup = aegis_core::Size { w: 240, h: 100 };
        assert_eq!(
            input_popup_position(anchor, popup, output),
            aegis_core::Point { x: 660, y: 220 }
        );
    }

    #[test]
    fn popup_moves_above_the_cursor_when_below_would_overflow() {
        let output = aegis_core::Rect::new(0, 0, 800, 600);
        let anchor = aegis_core::Rect::new(320, 575, 2, 20);
        let popup = aegis_core::Size { w: 240, h: 100 };
        assert_eq!(
            input_popup_position(anchor, popup, output),
            aegis_core::Point { x: 320, y: 475 }
        );
    }
}
