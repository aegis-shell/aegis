use super::*;

// ----- pointer-gestures-unstable-v1 --------------------------------------

#[derive(Clone, Copy)]
enum PointerGestureKind {
    Swipe,
    Pinch,
    Hold,
}

struct PointerGestureRec {
    state: *mut State,
    kind: PointerGestureKind,
}

static POINTER_GESTURES_IMPL: ffi::zwp_pointer_gestures_v1_interface_impl =
    ffi::zwp_pointer_gestures_v1_interface_impl {
        get_swipe_gesture: pointer_gestures_get_swipe,
        get_pinch_gesture: pointer_gestures_get_pinch,
        release: crate::res_destroy,
        get_hold_gesture: pointer_gestures_get_hold,
    };
static POINTER_GESTURE_SWIPE_IMPL: ffi::zwp_pointer_gesture_swipe_v1_interface_impl =
    ffi::zwp_pointer_gesture_swipe_v1_interface_impl {
        destroy: pointer_gesture_destroy,
    };
static POINTER_GESTURE_PINCH_IMPL: ffi::zwp_pointer_gesture_pinch_v1_interface_impl =
    ffi::zwp_pointer_gesture_pinch_v1_interface_impl {
        destroy: pointer_gesture_destroy,
    };
static POINTER_GESTURE_HOLD_IMPL: ffi::zwp_pointer_gesture_hold_v1_interface_impl =
    ffi::zwp_pointer_gesture_hold_v1_interface_impl {
        destroy: pointer_gesture_destroy,
    };

pub(crate) unsafe extern "C" fn pointer_gestures_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let res = ffi::wl_resource_create(
            client,
            &ffi::zwp_pointer_gestures_v1_interface,
            version.min(3) as c_int,
            id,
        );
        if !res.is_null() {
            ffi::wl_resource_set_implementation(
                res,
                &POINTER_GESTURES_IMPL as *const _ as *const c_void,
                data,
                None,
            );
        }
    }
}

unsafe extern "C" fn pointer_gestures_get_swipe(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    id: u32,
    pointer: *mut ffi::wl_resource,
) {
    unsafe {
        create_pointer_gesture(client, manager, id, pointer, PointerGestureKind::Swipe);
    }
}

unsafe extern "C" fn pointer_gestures_get_pinch(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    id: u32,
    pointer: *mut ffi::wl_resource,
) {
    unsafe {
        create_pointer_gesture(client, manager, id, pointer, PointerGestureKind::Pinch);
    }
}

unsafe extern "C" fn pointer_gestures_get_hold(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    id: u32,
    pointer: *mut ffi::wl_resource,
) {
    unsafe {
        create_pointer_gesture(client, manager, id, pointer, PointerGestureKind::Hold);
    }
}

unsafe fn create_pointer_gesture(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    id: u32,
    pointer: *mut ffi::wl_resource,
    kind: PointerGestureKind,
) {
    unsafe {
        if pointer.is_null() || ffi::wl_resource_get_client(pointer) != client {
            ffi::wl_resource_post_error(manager, 0, c"pointer belongs to another client".as_ptr());
            return;
        }
        let state = ffi::wl_resource_get_user_data(manager) as *mut State;
        let origin = (*state).seat_origin_for_resource(pointer);
        let Some(_guard) = crate::ActiveSeatGuard::for_resource(state, pointer, true) else {
            return;
        };
        let seat = (*state).active_seat;
        let (interface, implementation): (&ffi::wl_interface, *const c_void) = match kind {
            PointerGestureKind::Swipe => (
                &ffi::zwp_pointer_gesture_swipe_v1_interface,
                &POINTER_GESTURE_SWIPE_IMPL as *const _ as *const c_void,
            ),
            PointerGestureKind::Pinch => (
                &ffi::zwp_pointer_gesture_pinch_v1_interface,
                &POINTER_GESTURE_PINCH_IMPL as *const _ as *const c_void,
            ),
            PointerGestureKind::Hold => (
                &ffi::zwp_pointer_gesture_hold_v1_interface,
                &POINTER_GESTURE_HOLD_IMPL as *const _ as *const c_void,
            ),
        };
        let res = ffi::wl_resource_create(client, interface, 1, id);
        if res.is_null() {
            return;
        }
        let rec = Box::into_raw(Box::new(PointerGestureRec { state, kind }));
        ffi::wl_resource_set_implementation(
            res,
            implementation,
            rec as *mut c_void,
            Some(pointer_gesture_resource_destroy),
        );
        (*state).track_routed_seat_resource(res, origin.unwrap_or(seat), seat);
        match kind {
            PointerGestureKind::Swipe => (*state).pointer_gesture_swipes.push(res),
            PointerGestureKind::Pinch => (*state).pointer_gesture_pinches.push(res),
            PointerGestureKind::Hold => (*state).pointer_gesture_holds.push(res),
        }
    }
}

unsafe extern "C" fn pointer_gesture_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn pointer_gesture_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut PointerGestureRec;
        if rec.is_null() {
            return;
        }
        let state = (*rec).state;
        if let Some(_guard) = crate::ActiveSeatGuard::for_resource(state, resource, false) {
            match (*rec).kind {
                PointerGestureKind::Swipe => {
                    (*state).pointer_gesture_swipes.retain(|r| *r != resource)
                }
                PointerGestureKind::Pinch => {
                    (*state).pointer_gesture_pinches.retain(|r| *r != resource)
                }
                PointerGestureKind::Hold => {
                    (*state).pointer_gesture_holds.retain(|r| *r != resource)
                }
            }
            (*state).untrack_seat_resource(resource);
        }
        drop(Box::from_raw(rec));
    }
}
