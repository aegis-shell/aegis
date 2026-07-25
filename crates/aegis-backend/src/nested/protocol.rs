use super::listeners::{HOLD_GESTURE_LISTENER, PINCH_GESTURE_LISTENER, SWIPE_GESTURE_LISTENER};
use super::*;

// ----- request marshalling helpers ---------------------------------------

pub(super) unsafe fn registry_bind(
    registry: *mut ffi::wl_proxy,
    name: u32,
    interface: &ffi::wl_interface,
    version: u32,
) -> *mut ffi::wl_proxy {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            registry,
            ffi::WL_REGISTRY_BIND,
            interface,
            version,
            0,
            name,
            interface.name,
            version,
            ptr::null::<c_void>(),
        )
    }
}

pub(super) unsafe fn seat_get_pointer(seat: *mut ffi::wl_proxy) -> *mut ffi::wl_proxy {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            seat,
            ffi::WL_SEAT_GET_POINTER,
            &ffi::wl_pointer_interface,
            ffi::wl_proxy_get_version(seat).min(9),
            0,
            ptr::null::<c_void>(),
        )
    }
}

pub(super) unsafe fn seat_get_keyboard(seat: *mut ffi::wl_proxy) -> *mut ffi::wl_proxy {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            seat,
            ffi::WL_SEAT_GET_KEYBOARD,
            &ffi::wl_keyboard_interface,
            ffi::wl_proxy_get_version(seat).min(4),
            0,
            ptr::null::<c_void>(),
        )
    }
}

pub(super) unsafe fn keyboard_release(keyboard: *mut ffi::wl_proxy) {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            keyboard,
            ffi::WL_KEYBOARD_RELEASE,
            ptr::null::<ffi::wl_interface>(),
            ffi::wl_proxy_get_version(keyboard),
            ffi::WL_MARSHAL_FLAG_DESTROY,
        );
    }
}

pub(super) unsafe fn pointer_release(pointer: *mut ffi::wl_proxy) {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            pointer,
            ffi::WL_POINTER_RELEASE,
            ptr::null::<ffi::wl_interface>(),
            ffi::wl_proxy_get_version(pointer),
            ffi::WL_MARSHAL_FLAG_DESTROY,
        );
    }
}

pub(super) unsafe fn pointer_hide_cursor(pointer: *mut ffi::wl_proxy, serial: u32) {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            pointer,
            ffi::WL_POINTER_SET_CURSOR,
            ptr::null::<ffi::wl_interface>(),
            ffi::wl_proxy_get_version(pointer),
            0,
            serial,
            ptr::null_mut::<ffi::wl_proxy>(),
            0i32,
            0i32,
        );
    }
}

pub(super) unsafe fn create_surface(compositor: *mut ffi::wl_proxy) -> *mut ffi::wl_proxy {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            compositor,
            ffi::WL_COMPOSITOR_CREATE_SURFACE,
            &ffi::wl_surface_interface,
            ffi::wl_proxy_get_version(compositor),
            0,
            ptr::null::<c_void>(),
        )
    }
}

pub(super) unsafe fn get_xdg_surface(
    wm_base: *mut ffi::wl_proxy,
    surface: *mut ffi::wl_proxy,
) -> *mut ffi::wl_proxy {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            wm_base,
            ffi::XDG_WM_BASE_GET_XDG_SURFACE,
            &ffi::xdg_surface_interface,
            ffi::wl_proxy_get_version(wm_base),
            0,
            ptr::null::<c_void>(),
            surface,
        )
    }
}

pub(super) unsafe fn get_toplevel(xdg_surface: *mut ffi::wl_proxy) -> *mut ffi::wl_proxy {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            xdg_surface,
            ffi::XDG_SURFACE_GET_TOPLEVEL,
            &ffi::xdg_toplevel_interface,
            ffi::wl_proxy_get_version(xdg_surface),
            0,
            ptr::null::<c_void>(),
        )
    }
}

pub(super) unsafe fn ack_configure(xdg_surface: *mut ffi::wl_proxy, serial: u32) {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            xdg_surface,
            ffi::XDG_SURFACE_ACK_CONFIGURE,
            ptr::null::<ffi::wl_interface>(),
            ffi::wl_proxy_get_version(xdg_surface),
            0,
            serial,
        );
    }
}

pub(super) unsafe fn pong(wm_base: *mut ffi::wl_proxy, serial: u32) {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            wm_base,
            ffi::XDG_WM_BASE_PONG,
            ptr::null::<ffi::wl_interface>(),
            ffi::wl_proxy_get_version(wm_base),
            0,
            serial,
        );
    }
}

pub(super) unsafe fn commit(surface: *mut ffi::wl_proxy) {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            surface,
            ffi::WL_SURFACE_COMMIT,
            ptr::null::<ffi::wl_interface>(),
            ffi::wl_proxy_get_version(surface),
            0,
        );
    }
}

/// `wl_surface.set_buffer_scale`: declare the attached buffer is pre-scaled by
/// `scale`, so the host maps it 1:1 instead of upscaling a logical-sized
/// buffer. Double-buffered; applies on the next commit (the next present).
pub(super) unsafe fn set_buffer_scale(surface: *mut ffi::wl_proxy, scale: i32) {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            surface,
            ffi::WL_SURFACE_SET_BUFFER_SCALE,
            ptr::null::<ffi::wl_interface>(),
            ffi::wl_proxy_get_version(surface),
            0,
            scale.max(1),
        );
    }
}

pub(super) unsafe fn get_viewport(
    viewporter: *mut ffi::wl_proxy,
    surface: *mut ffi::wl_proxy,
) -> *mut ffi::wl_proxy {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            viewporter,
            ffi::WP_VIEWPORTER_GET_VIEWPORT,
            &ffi::wp_viewport_interface,
            ffi::wl_proxy_get_version(viewporter),
            0,
            ptr::null::<c_void>(),
            surface,
        )
    }
}

pub(super) unsafe fn viewport_set_destination(
    viewport: *mut ffi::wl_proxy,
    width: i32,
    height: i32,
) {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            viewport,
            ffi::WP_VIEWPORT_SET_DESTINATION,
            ptr::null::<ffi::wl_interface>(),
            ffi::wl_proxy_get_version(viewport),
            0,
            width.max(1),
            height.max(1),
        );
    }
}

pub(super) unsafe fn get_fractional_scale(
    manager: *mut ffi::wl_proxy,
    surface: *mut ffi::wl_proxy,
) -> *mut ffi::wl_proxy {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            manager,
            ffi::WP_FRACTIONAL_SCALE_MANAGER_V1_GET_FRACTIONAL_SCALE,
            &ffi::wp_fractional_scale_v1_interface,
            ffi::wl_proxy_get_version(manager),
            0,
            ptr::null::<c_void>(),
            surface,
        )
    }
}

pub(super) unsafe fn get_cursor_shape_device(
    manager: *mut ffi::wl_proxy,
    pointer: *mut ffi::wl_proxy,
) -> *mut ffi::wl_proxy {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            manager,
            ffi::WP_CURSOR_SHAPE_MANAGER_V1_GET_POINTER,
            &ffi::wp_cursor_shape_device_v1_interface,
            ffi::wl_proxy_get_version(manager),
            0,
            ptr::null::<c_void>(),
            pointer,
        )
    }
}

pub(super) unsafe fn cursor_shape_set_shape(device: *mut ffi::wl_proxy, serial: u32, shape: u32) {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            device,
            ffi::WP_CURSOR_SHAPE_DEVICE_V1_SET_SHAPE,
            ptr::null::<ffi::wl_interface>(),
            ffi::wl_proxy_get_version(device),
            0,
            serial,
            shape.max(1),
        );
    }
}

pub(super) unsafe fn get_pointer_gesture(
    manager: *mut ffi::wl_proxy,
    opcode: u32,
    interface: &ffi::wl_interface,
    pointer: *mut ffi::wl_proxy,
) -> *mut ffi::wl_proxy {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            manager,
            opcode,
            interface,
            1,
            0,
            ptr::null::<c_void>(),
            pointer,
        )
    }
}

pub(super) unsafe fn destroy_pointer_gesture(gesture: *mut ffi::wl_proxy) {
    unsafe {
        if !gesture.is_null() {
            ffi::wl_proxy_marshal_flags(
                gesture,
                0,
                ptr::null::<ffi::wl_interface>(),
                ffi::wl_proxy_get_version(gesture),
                ffi::WL_MARSHAL_FLAG_DESTROY,
            );
        }
    }
}

pub(super) unsafe fn destroy_pointer_gestures(st: *mut State) {
    unsafe {
        destroy_pointer_gesture((*st).gesture_hold);
        destroy_pointer_gesture((*st).gesture_pinch);
        destroy_pointer_gesture((*st).gesture_swipe);
        (*st).gesture_hold = ptr::null_mut();
        (*st).gesture_pinch = ptr::null_mut();
        (*st).gesture_swipe = ptr::null_mut();
    }
}

pub(super) unsafe fn ensure_pointer_gestures(st: *mut State, data: *mut c_void) {
    unsafe {
        if st.is_null()
            || (*st).pointer.is_null()
            || (*st).pointer_gestures_manager.is_null()
            || !(*st).gesture_swipe.is_null()
        {
            return;
        }
        let manager = (*st).pointer_gestures_manager;
        let pointer = (*st).pointer;
        (*st).gesture_swipe = get_pointer_gesture(
            manager,
            ffi::ZWP_POINTER_GESTURES_V1_GET_SWIPE_GESTURE,
            &ffi::zwp_pointer_gesture_swipe_v1_interface,
            pointer,
        );
        if !(*st).gesture_swipe.is_null() {
            ffi::wl_proxy_add_listener(
                (*st).gesture_swipe,
                &SWIPE_GESTURE_LISTENER as *const _ as *const c_void,
                data,
            );
        }
        if ffi::wl_proxy_get_version(manager) >= 2 {
            (*st).gesture_pinch = get_pointer_gesture(
                manager,
                ffi::ZWP_POINTER_GESTURES_V1_GET_PINCH_GESTURE,
                &ffi::zwp_pointer_gesture_pinch_v1_interface,
                pointer,
            );
            if !(*st).gesture_pinch.is_null() {
                ffi::wl_proxy_add_listener(
                    (*st).gesture_pinch,
                    &PINCH_GESTURE_LISTENER as *const _ as *const c_void,
                    data,
                );
            }
        }
        if ffi::wl_proxy_get_version(manager) >= 3 {
            (*st).gesture_hold = get_pointer_gesture(
                manager,
                ffi::ZWP_POINTER_GESTURES_V1_GET_HOLD_GESTURE,
                &ffi::zwp_pointer_gesture_hold_v1_interface,
                pointer,
            );
            if !(*st).gesture_hold.is_null() {
                ffi::wl_proxy_add_listener(
                    (*st).gesture_hold,
                    &HOLD_GESTURE_LISTENER as *const _ as *const c_void,
                    data,
                );
            }
        }
    }
}

pub(super) unsafe fn get_text_input(
    manager: *mut ffi::wl_proxy,
    seat: *mut ffi::wl_proxy,
) -> *mut ffi::wl_proxy {
    unsafe {
        ffi::wl_proxy_marshal_flags(
            manager,
            ffi::ZWP_TEXT_INPUT_MANAGER_V3_GET_TEXT_INPUT,
            &ffi::zwp_text_input_v3_interface,
            ffi::wl_proxy_get_version(manager).min(1),
            0,
            ptr::null::<c_void>(),
            seat,
        )
    }
}

pub(super) unsafe fn send_text_input_state(st: *mut State) {
    unsafe {
        if st.is_null() || (*st).text_input.is_null() || !(*st).text_input_entered {
            return;
        }
        let text_input = (*st).text_input;
        let state = (*st).text_input_state.clone();
        let opcode = if state.enabled {
            ffi::ZWP_TEXT_INPUT_V3_ENABLE
        } else {
            ffi::ZWP_TEXT_INPUT_V3_DISABLE
        };
        ffi::wl_proxy_marshal_flags(
            text_input,
            opcode,
            ptr::null::<ffi::wl_interface>(),
            ffi::wl_proxy_get_version(text_input),
            0,
        );
        if state.enabled {
            if let Some(text) = state.surrounding_text
                && let Ok(text) = CString::new(text)
            {
                ffi::wl_proxy_marshal_flags(
                    text_input,
                    ffi::ZWP_TEXT_INPUT_V3_SET_SURROUNDING_TEXT,
                    ptr::null::<ffi::wl_interface>(),
                    ffi::wl_proxy_get_version(text_input),
                    0,
                    text.as_ptr(),
                    state.cursor,
                    state.anchor,
                );
            }
            ffi::wl_proxy_marshal_flags(
                text_input,
                ffi::ZWP_TEXT_INPUT_V3_SET_TEXT_CHANGE_CAUSE,
                ptr::null::<ffi::wl_interface>(),
                ffi::wl_proxy_get_version(text_input),
                0,
                state.change_cause,
            );
            ffi::wl_proxy_marshal_flags(
                text_input,
                ffi::ZWP_TEXT_INPUT_V3_SET_CONTENT_TYPE,
                ptr::null::<ffi::wl_interface>(),
                ffi::wl_proxy_get_version(text_input),
                0,
                state.content_hint,
                state.content_purpose,
            );
            if let Some((x, y, width, height)) = state.cursor_rect {
                ffi::wl_proxy_marshal_flags(
                    text_input,
                    ffi::ZWP_TEXT_INPUT_V3_SET_CURSOR_RECTANGLE,
                    ptr::null::<ffi::wl_interface>(),
                    ffi::wl_proxy_get_version(text_input),
                    0,
                    x,
                    y,
                    width,
                    height,
                );
            }
        }
        ffi::wl_proxy_marshal_flags(
            text_input,
            ffi::ZWP_TEXT_INPUT_V3_COMMIT,
            ptr::null::<ffi::wl_interface>(),
            ffi::wl_proxy_get_version(text_input),
            0,
        );
    }
}

pub(super) unsafe fn set_string(toplevel: *mut ffi::wl_proxy, opcode: u32, value: &str) {
    unsafe {
        let c = CString::new(value).unwrap_or_default();
        ffi::wl_proxy_marshal_flags(
            toplevel,
            opcode,
            ptr::null::<ffi::wl_interface>(),
            ffi::wl_proxy_get_version(toplevel),
            0,
            c.as_ptr(),
        );
    }
}
