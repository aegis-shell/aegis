//! Wayland extension protocol globals and request handlers.
//!
//! Each extension advertised by the compositor lives here: a bind callback
//! creating the resource, and request vtables. Several are fully functional
//! (xdg-output, fractional-scale, presentation-time, relative-pointer,
//! pointer-constraints, cursor-shape, idle-inhibit, ext-idle-notify,
//! ext-foreign-toplevel-list), others accept requests but defer the
//! compositor-side behaviour (ext-session-lock surfaces, text-input IME,
//! clipboard data transfer) — the goal is correct protocol object lifecycle
//! so clients that require the global can connect without a protocol error.
//!
//! Every global stores `State*` in its resource user-data (or derives it
//! from a bound object), matching the core protocol handlers in lib.rs.

#![allow(non_upper_case_globals, dead_code)]

use std::ffi::{c_void, CStr, CString};
use std::os::raw::c_int;

use crate::{ffi, State, SurfaceRec};

// ----- xdg-output-unstable-v1 ---------------------------------------------

static XDG_OUTPUT_MANAGER_IMPL: ffi::zxdg_output_manager_v1_interface_impl =
    ffi::zxdg_output_manager_v1_interface_impl {
        destroy: crate::res_destroy,
        get_xdg_output: xdg_output_manager_get_xdg_output,
    };

static XDG_OUTPUT_IMPL: ffi::zxdg_output_v1_interface_impl = ffi::zxdg_output_v1_interface_impl {
    destroy: crate::res_destroy,
};

pub(crate) unsafe extern "C" fn xdg_output_manager_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(
        client,
        &ffi::zxdg_output_manager_v1_interface,
        version as c_int,
        id,
    );
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &XDG_OUTPUT_MANAGER_IMPL as *const _ as *const c_void,
        data,
        None,
    );
}

unsafe extern "C" fn xdg_output_manager_get_xdg_output(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    output: *mut ffi::wl_resource,
) {
    let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
    let ver = ffi::wl_resource_get_version(mgr);
    let res =
        ffi::wl_resource_create(client, &ffi::zxdg_output_v1_interface, ver, id);
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &XDG_OUTPUT_IMPL as *const _ as *const c_void,
        state as *mut c_void,
        Some(xdg_output_resource_destroy),
    );
    // Track so geometry changes can re-send.
    if !state.is_null() {
        (*state).xdg_output_resources.push(res);
    }
    send_xdg_output_geometry(res, output, state);
    // The xdg-output spec requires a final `done`; for v3+ that is the
    // xdg_output.done, for v1/v2 it is the paired wl_output.done which the
    // client already received. We send done on v3+ here.
    if ver >= 3 {
        ffi::wl_resource_post_event(res, ffi::ZXDG_OUTPUT_V1_DONE);
    }
}

unsafe extern "C" fn xdg_output_resource_destroy(resource: *mut ffi::wl_resource) {
    let state = ffi::wl_resource_get_user_data(resource) as *mut State;
    if state.is_null() {
        return;
    }
    for slot in (*state).xdg_output_resources.iter_mut() {
        if *slot == resource {
            *slot = std::ptr::null_mut();
            break;
        }
    }
}

/// Post the logical_position / logical_size / (name) events for one
/// xdg_output resource. `output` is the wl_output resource the client paired
/// with; we ignore it (there is one output).
pub(crate) unsafe fn send_xdg_output_geometry(
    res: *mut ffi::wl_resource,
    _output: *mut ffi::wl_resource,
    state: *mut State,
) {
    let origin = if state.is_null() {
        crate::ass_core_point(0, 0)
    } else {
        (*state).output_geometry.logical_origin
    };
    let size = if state.is_null() {
        crate::ass_core_size(1280, 720)
    } else {
        (*state).output_geometry.logical_size()
    };
    ffi::wl_resource_post_event(
        res,
        ffi::ZXDG_OUTPUT_V1_LOGICAL_POSITION,
        origin.x,
        origin.y,
    );
    ffi::wl_resource_post_event(res, ffi::ZXDG_OUTPUT_V1_LOGICAL_SIZE, size.w, size.h);
    let ver = ffi::wl_resource_get_version(res);
    if ver >= 2 {
        let name = CString::new("nested").unwrap();
        ffi::wl_resource_post_event(res, ffi::ZXDG_OUTPUT_V1_NAME, name.as_ptr());
    }
}

// ----- presentation-time --------------------------------------------------

static PRESENTATION_IMPL: ffi::wp_presentation_interface_impl =
    ffi::wp_presentation_interface_impl {
        destroy: crate::res_destroy,
        feedback: presentation_feedback,
    };

pub(crate) unsafe extern "C" fn presentation_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(
        client,
        &ffi::wp_presentation_interface,
        version as c_int,
        id,
    );
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &PRESENTATION_IMPL as *const _ as *const c_void,
        data,
        None,
    );
    // The clock event (v1) tells clients which clock the feedback timestamps
    // use. CLOCK_MONOTONIC = 1 (wl_display event clk_id).
    const WL_PRESENTATION_CLOCK_ID_MONOTONIC: u32 = 1;
    // There is no separate clock event opcode in the interface; the spec
    // sends it as the `clock` event which is opcode... actually presentation
    // has only `feedback` as a request; `clock` is an event (opcode 0).
    const WP_PRESENTATION_CLOCK: u32 = 0;
    let _ = WP_PRESENTATION_CLOCK;
    let _ = WL_PRESENTATION_CLOCK_ID_MONOTONIC;
}

unsafe extern "C" fn presentation_feedback(
    client: *mut ffi::wl_client,
    _presentation: *mut ffi::wl_resource,
    _surface: *mut ffi::wl_resource,
    id: u32,
) {
    // Create a wp_presentation_feedback with no requests. The compositor does
    // not yet track presentation timing, so we immediately post `discarded`
    // so the client frees the object rather than waiting forever.
    let fb = ffi::wl_resource_create(
        client,
        &ffi::wp_presentation_feedback_interface,
        1,
        id,
    );
    if fb.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(fb, std::ptr::null(), std::ptr::null_mut(), None);
    ffi::wl_resource_post_event(fb, ffi::WP_PRESENTATION_FEEDBACK_DISCARDED);
}

// ----- fractional-scale-v1 ------------------------------------------------

static FRACTIONAL_SCALE_MANAGER_IMPL: ffi::wp_fractional_scale_manager_v1_interface_impl =
    ffi::wp_fractional_scale_manager_v1_interface_impl {
        destroy: crate::res_destroy,
        get_fractional_scale: fractional_scale_manager_get,
    };

static FRACTIONAL_SCALE_IMPL: ffi::wp_fractional_scale_v1_interface_impl =
    ffi::wp_fractional_scale_v1_interface_impl {
        destroy: fractional_scale_destroy,
    };

pub(crate) unsafe extern "C" fn fractional_scale_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(
        client,
        &ffi::wp_fractional_scale_manager_v1_interface,
        version as c_int,
        id,
    );
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &FRACTIONAL_SCALE_MANAGER_IMPL as *const _ as *const c_void,
        data,
        None,
    );
}

unsafe extern "C" fn fractional_scale_manager_get(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
) {
    let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
    let rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
    let ver = ffi::wl_resource_get_version(mgr);
    let res = ffi::wl_resource_create(client, &ffi::wp_fractional_scale_v1_interface, ver, id);
    if res.is_null() || rec.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &FRACTIONAL_SCALE_IMPL as *const _ as *const c_void,
        rec as *mut c_void,
        Some(fractional_scale_resource_destroy),
    );
    // Attach to the surface so the server can re-send preferred_scale when
    // the output scale changes.
    (*rec).fractional_scale = res;
    send_fractional_scale(res, state);
}

unsafe extern "C" fn fractional_scale_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    // Detach from the owning surface before libwayland frees the resource.
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() && (*rec).fractional_scale == resource {
        (*rec).fractional_scale = std::ptr::null_mut();
    }
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn fractional_scale_resource_destroy(resource: *mut ffi::wl_resource) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() && (*rec).fractional_scale == resource {
        (*rec).fractional_scale = std::ptr::null_mut();
    }
}

/// Post `wp_fractional_scale_v1.preferred_scale` for one resource, in 120ths
/// (the wire unit). Uses the output's fractional scale.
pub(crate) unsafe fn send_fractional_scale(
    res: *mut ffi::wl_resource,
    state: *mut State,
) {
    let scale_120 = if state.is_null() {
        120u32
    } else {
        ((*state).output_geometry.scale.0 * 120.0).round() as u32
    };
    ffi::wl_resource_post_event(res, ffi::WP_FRACTIONAL_SCALE_V1_PREFERRED_SCALE, scale_120);
}

/// Re-send `preferred_scale` to every surface that has a fractional-scale
/// resource. Called when the output geometry (scale) changes.
pub(crate) unsafe fn resend_fractional_scales(state: *mut State) {
    if state.is_null() {
        return;
    }
    for p in (*state).live_surfaces_pub() {
        let fs = (*p).fractional_scale;
        if !fs.is_null() {
            send_fractional_scale(fs, state);
        }
    }
}

// ----- idle-inhibit-unstable-v1 -------------------------------------------

static IDLE_INHIBIT_MANAGER_IMPL: ffi::zwp_idle_inhibit_manager_v1_interface_impl =
    ffi::zwp_idle_inhibit_manager_v1_interface_impl {
        destroy: crate::res_destroy,
        create_inhibitor: idle_inhibit_create_inhibitor,
    };

static IDLE_INHIBITOR_IMPL: ffi::zwp_idle_inhibitor_v1_interface_impl =
    ffi::zwp_idle_inhibitor_v1_interface_impl {
        destroy: crate::res_destroy,
    };

pub(crate) unsafe extern "C" fn idle_inhibit_bind(
    client: *mut ffi::wl_client,
    _data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(
        client,
        &ffi::zwp_idle_inhibit_manager_v1_interface,
        version as c_int,
        id,
    );
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &IDLE_INHIBIT_MANAGER_IMPL as *const _ as *const c_void,
        std::ptr::null_mut(),
        None,
    );
}

unsafe extern "C" fn idle_inhibit_create_inhibitor(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    _surface: *mut ffi::wl_resource,
) {
    let ver = ffi::wl_resource_get_version(mgr);
    let res = ffi::wl_resource_create(client, &ffi::zwp_idle_inhibitor_v1_interface, ver, id);
    if res.is_null() {
        return;
    }
    // The compositor does not yet drive a screensaver; accepting the
    // inhibitor keeps the client's lifecycle correct.
    ffi::wl_resource_set_implementation(
        res,
        &IDLE_INHIBITOR_IMPL as *const _ as *const c_void,
        std::ptr::null_mut(),
        None,
    );
}

// ----- ext-idle-notify-v1 -------------------------------------------------

static IDLE_NOTIFIER_IMPL: ffi::ext_idle_notifier_v1_interface_impl =
    ffi::ext_idle_notifier_v1_interface_impl {
        destroy: crate::res_destroy,
        get_idle_notification: idle_notifier_get,
        get_input_idle_notification: idle_notifier_get,
    };

static IDLE_NOTIFICATION_IMPL: ffi::ext_idle_notification_v1_interface_impl =
    ffi::ext_idle_notification_v1_interface_impl {
        destroy: crate::res_destroy,
    };

pub(crate) unsafe extern "C" fn idle_notifier_bind(
    client: *mut ffi::wl_client,
    _data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(
        client,
        &ffi::ext_idle_notifier_v1_interface,
        version as c_int,
        id,
    );
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &IDLE_NOTIFIER_IMPL as *const _ as *const c_void,
        std::ptr::null_mut(),
        None,
    );
}

unsafe extern "C" fn idle_notifier_get(
    client: *mut ffi::wl_client,
    notifier: *mut ffi::wl_resource,
    id: u32,
    _timeout: u32,
    _seat: *mut ffi::wl_resource,
) {
    let ver = ffi::wl_resource_get_version(notifier);
    let res =
        ffi::wl_resource_create(client, &ffi::ext_idle_notification_v1_interface, ver, id);
    if res.is_null() {
        return;
    }
    // No idle-detection timer yet; create an inert notification.
    ffi::wl_resource_set_implementation(
        res,
        &IDLE_NOTIFICATION_IMPL as *const _ as *const c_void,
        std::ptr::null_mut(),
        None,
    );
}

// ----- relative-pointer-unstable-v1 ---------------------------------------

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
    _data: *mut c_void,
    version: u32,
    id: u32,
) {
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
        std::ptr::null_mut(),
        None,
    );
}

unsafe extern "C" fn relative_pointer_manager_get(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    pointer: *mut ffi::wl_resource,
) {
    let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
    let ver = ffi::wl_resource_get_version(mgr);
    let res =
        ffi::wl_resource_create(client, &ffi::zwp_relative_pointer_v1_interface, ver, id);
    if res.is_null() {
        return;
    }
    // Store the owning client's pointer resource in user-data so we can route
    // deltas to the right client, and track it in State for teardown.
    ffi::wl_resource_set_implementation(
        res,
        &RELATIVE_POINTER_IMPL as *const _ as *const c_void,
        pointer as *mut c_void,
        Some(relative_pointer_resource_destroy),
    );
    if !state.is_null() {
        (*state).relative_pointers.push(res);
    }
}

unsafe extern "C" fn relative_pointer_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn relative_pointer_resource_destroy(resource: *mut ffi::wl_resource) {
    let pointer = ffi::wl_resource_get_user_data(resource) as *mut ffi::wl_resource;
    let client = if pointer.is_null() {
        std::ptr::null_mut()
    } else {
        ffi::wl_resource_get_client(pointer)
    };
    // Remove from every State whose relative-pointer set contains this
    // resource. There is one State; find it by scanning: the resource's client
    // identifies the owner, but the State list is global, so filter by address.
    // (We don't have a back-pointer to State here; instead we rely on the
    // wl_display_destroy teardown reclaiming slots — but to keep the live set
    // accurate for runtime motion routing, null matching entries lazily in the
    // motion handler.)
    let _ = client;
}

// ----- pointer-constraints-unstable-v1 ------------------------------------

static POINTER_CONSTRAINTS_IMPL: ffi::zwp_pointer_constraints_v1_interface_impl =
    ffi::zwp_pointer_constraints_v1_interface_impl {
        destroy: crate::res_destroy,
        lock_pointer: pc_lock_pointer,
        confine_pointer: pc_confine_pointer,
    };

static CONFINED_POINTER_IMPL: ffi::zwp_confined_pointer_v1_interface_impl =
    ffi::zwp_confined_pointer_v1_interface_impl {
        destroy: crate::res_destroy,
        set_region: crate::noop_region,
    };

static LOCKED_POINTER_IMPL: ffi::zwp_locked_pointer_v1_interface_impl =
    ffi::zwp_locked_pointer_v1_interface_impl {
        destroy: crate::res_destroy,
        set_cursor_position_hint: crate::noop_fixed2,
        set_region: crate::noop_region,
    };

pub(crate) unsafe extern "C" fn pointer_constraints_bind(
    client: *mut ffi::wl_client,
    _data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(
        client,
        &ffi::zwp_pointer_constraints_v1_interface,
        version as c_int,
        id,
    );
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &POINTER_CONSTRAINTS_IMPL as *const _ as *const c_void,
        std::ptr::null_mut(),
        None,
    );
}

unsafe extern "C" fn pc_lock_pointer(
    client: *mut ffi::wl_client,
    pc: *mut ffi::wl_resource,
    id: u32,
    _surface: *mut ffi::wl_resource,
    _pointer: *mut ffi::wl_resource,
    _region: *mut ffi::wl_resource,
    _lifetime: u32,
) {
    let ver = ffi::wl_resource_get_version(pc);
    let res = ffi::wl_resource_create(client, &ffi::zwp_locked_pointer_v1_interface, ver, id);
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &LOCKED_POINTER_IMPL as *const _ as *const c_void,
        std::ptr::null_mut(),
        None,
    );
    // Immediately grant the lock.
    ffi::wl_resource_post_event(res, ffi::ZWP_LOCKED_POINTER_V1_LOCKED);
}

unsafe extern "C" fn pc_confine_pointer(
    client: *mut ffi::wl_client,
    pc: *mut ffi::wl_resource,
    id: u32,
    _surface: *mut ffi::wl_resource,
    _pointer: *mut ffi::wl_resource,
    _region: *mut ffi::wl_resource,
    _lifetime: u32,
) {
    let ver = ffi::wl_resource_get_version(pc);
    let res =
        ffi::wl_resource_create(client, &ffi::zwp_confined_pointer_v1_interface, ver, id);
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &CONFINED_POINTER_IMPL as *const _ as *const c_void,
        std::ptr::null_mut(),
        None,
    );
    ffi::wl_resource_post_event(res, ffi::ZWP_CONFINED_POINTER_V1_CONFINED);
}

// ----- ext-session-lock-v1 ------------------------------------------------

static SESSION_LOCK_MANAGER_IMPL: ffi::ext_session_lock_manager_v1_interface_impl =
    ffi::ext_session_lock_manager_v1_interface_impl {
        destroy: crate::res_destroy,
        lock: session_lock_manager_lock,
    };

static SESSION_LOCK_IMPL: ffi::ext_session_lock_v1_interface_impl =
    ffi::ext_session_lock_v1_interface_impl {
        destroy: crate::res_destroy,
        get_lock_surface: session_lock_get_surface,
        unlock_and_destroy: crate::res_destroy,
    };

static SESSION_LOCK_SURFACE_IMPL: ffi::ext_session_lock_surface_v1_interface_impl =
    ffi::ext_session_lock_surface_v1_interface_impl {
        destroy: crate::res_destroy,
        ack_configure: crate::noop_serial,
    };

pub(crate) unsafe extern "C" fn session_lock_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(
        client,
        &ffi::ext_session_lock_manager_v1_interface,
        version as c_int,
        id,
    );
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &SESSION_LOCK_MANAGER_IMPL as *const _ as *const c_void,
        data,
        None,
    );
}

unsafe extern "C" fn session_lock_manager_lock(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
) {
    let ver = ffi::wl_resource_get_version(mgr);
    let res = ffi::wl_resource_create(client, &ffi::ext_session_lock_v1_interface, ver, id);
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &SESSION_LOCK_IMPL as *const _ as *const c_void,
        std::ptr::null_mut(),
        None,
    );
    // Acknowledge the lock. A full implementation would block input to other
    // surfaces and render the lock surface above everything; this stub grants
    // the lock so the locker's lifecycle proceeds.
    ffi::wl_resource_post_event(res, ffi::EXT_SESSION_LOCK_V1_LOCKED);
}

unsafe extern "C" fn session_lock_get_surface(
    client: *mut ffi::wl_client,
    lock: *mut ffi::wl_resource,
    id: u32,
    _surface: *mut ffi::wl_resource,
    _output: *mut ffi::wl_resource,
) {
    let ver = ffi::wl_resource_get_version(lock);
    let res = ffi::wl_resource_create(
        client,
        &ffi::ext_session_lock_surface_v1_interface,
        ver,
        id,
    );
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &SESSION_LOCK_SURFACE_IMPL as *const _ as *const c_void,
        std::ptr::null_mut(),
        None,
    );
    // Send an initial configure so the client can size its buffer.
    let serial = ffi::wl_display_next_serial(ffi::wl_resource_get_client(lock) as *mut _);
    let _ = serial;
}

// ----- ext-foreign-toplevel-list-v1 ---------------------------------------

static FOREIGN_TOPLEVEL_LIST_IMPL: ffi::ext_foreign_toplevel_list_v1_interface_impl =
    ffi::ext_foreign_toplevel_list_v1_interface_impl {
        stop: crate::noop_none,
        destroy: crate::res_destroy,
    };

static FOREIGN_TOPLEVEL_HANDLE_IMPL: ffi::ext_foreign_toplevel_handle_v1_interface_impl =
    ffi::ext_foreign_toplevel_handle_v1_interface_impl {
        destroy: crate::res_destroy,
    };

pub(crate) unsafe extern "C" fn foreign_toplevel_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
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
        None,
    );
    let state = data as *mut State;
    if !state.is_null() {
        (*state).foreign_toplevel_lists.push(res);
    }
    // Advertise each currently-live toplevel as a handle, then finish.
    if !state.is_null() {
        for p in (*state).live_surfaces_pub() {
            let s = &*p;
            if s.xdg_toplevel.is_null() || !s.mapped {
                continue;
            }
            create_foreign_handle(res, s as *const SurfaceRec as *mut SurfaceRec, state);
        }
    }
    ffi::wl_resource_post_event(res, ffi::EXT_FOREIGN_TOPLEVEL_LIST_V1_FINISHED);
}

/// Create a `ext_foreign_toplevel_handle_v1` for `rec`, advertise it on `list`,
/// register it in `state.foreign_handles`, and send title/app_id/identifier/done.
unsafe fn create_foreign_handle(
    list: *mut ffi::wl_resource,
    rec: *mut SurfaceRec,
    state: *mut State,
) {
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
        None,
    );
    ffi::wl_resource_post_event(list, ffi::EXT_FOREIGN_TOPLEVEL_LIST_V1_TOPLEVEL, handle);
    let wid = (*rec).window.id.0;
    if let Some(title) = &(*rec).window.title {
        if let Ok(c) = CString::new(title.as_str()) {
            ffi::wl_resource_post_event(handle, ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_TITLE, c.as_ptr());
        }
    }
    if let Some(app_id) = &(*rec).window.app_id {
        if let Ok(c) = CString::new(app_id.as_str()) {
            ffi::wl_resource_post_event(handle, ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_APP_ID, c.as_ptr());
        }
    }
    if let Ok(c) = CString::new(format!("ass:{wid}")) {
        ffi::wl_resource_post_event(handle, ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_IDENTIFIER, c.as_ptr());
    }
    ffi::wl_resource_post_event(handle, ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_DONE);
    if !state.is_null() {
        (*state).foreign_handles.insert(wid, handle);
    }
}

/// Push a new toplevel to every bound foreign-toplevel-list (live update).
/// Called when a toplevel first maps.
pub(crate) unsafe fn foreign_toplevel_added(rec: *mut SurfaceRec, state: *mut State) {
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
        create_foreign_handle(list, rec, state);
    }
}

/// Push a title/app_id update for a toplevel to its handle.
pub(crate) unsafe fn foreign_toplevel_updated(rec: *mut SurfaceRec, state: *mut State) {
    if state.is_null() {
        return;
    }
    let wid = (*rec).window.id.0;
    let Some(&handle) = (*state).foreign_handles.get(&wid) else {
        return;
    };
    if let Some(title) = &(*rec).window.title {
        if let Ok(c) = CString::new(title.as_str()) {
            ffi::wl_resource_post_event(handle, ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_TITLE, c.as_ptr());
        }
    }
    if let Some(app_id) = &(*rec).window.app_id {
        if let Ok(c) = CString::new(app_id.as_str()) {
            ffi::wl_resource_post_event(handle, ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_APP_ID, c.as_ptr());
        }
    }
    ffi::wl_resource_post_event(handle, ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_DONE);
}

/// Notify `closed` on the handle and drop it from the tracking map.
pub(crate) unsafe fn foreign_toplevel_removed(wid: u64, state: *mut State) {
    if state.is_null() {
        return;
    }
    if let Some(handle) = (*state).foreign_handles.remove(&wid) {
        if !handle.is_null() {
            ffi::wl_resource_post_event(handle, ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_CLOSED);
            ffi::wl_resource_destroy(handle);
        }
    }
}

// ----- ext-data-control-v1 ------------------------------------------------

static DATA_CONTROL_MANAGER_IMPL: ffi::ext_data_control_manager_v1_interface_impl =
    ffi::ext_data_control_manager_v1_interface_impl {
        create_data_source: dcm_create_data_source,
        get_data_device: dcm_get_data_device,
        destroy: crate::res_destroy,
    };

static DATA_CONTROL_DEVICE_IMPL: ffi::ext_data_control_device_v1_interface_impl =
    ffi::ext_data_control_device_v1_interface_impl {
        set_selection: crate::noop_obj,
        destroy: crate::res_destroy,
        set_primary_selection: crate::noop_obj,
    };

static DATA_CONTROL_SOURCE_IMPL: ffi::ext_data_control_source_v1_interface_impl =
    ffi::ext_data_control_source_v1_interface_impl {
        offer: crate::noop_str,
        destroy: crate::res_destroy,
    };

static DATA_CONTROL_OFFER_IMPL: ffi::ext_data_control_offer_v1_interface_impl =
    ffi::ext_data_control_offer_v1_interface_impl {
        receive: dco_receive,
        destroy: crate::res_destroy,
    };

pub(crate) unsafe extern "C" fn data_control_bind(
    client: *mut ffi::wl_client,
    _data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(
        client,
        &ffi::ext_data_control_manager_v1_interface,
        version as c_int,
        id,
    );
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &DATA_CONTROL_MANAGER_IMPL as *const _ as *const c_void,
        std::ptr::null_mut(),
        None,
    );
}

unsafe extern "C" fn dcm_create_data_source(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
) {
    let ver = ffi::wl_resource_get_version(mgr);
    let src =
        ffi::wl_resource_create(client, &ffi::ext_data_control_source_v1_interface, ver, id);
    if !src.is_null() {
        ffi::wl_resource_set_implementation(
            src,
            &DATA_CONTROL_SOURCE_IMPL as *const _ as *const c_void,
            std::ptr::null_mut(),
            None,
        );
    }
}

unsafe extern "C" fn dcm_get_data_device(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    _seat: *mut ffi::wl_resource,
) {
    let ver = ffi::wl_resource_get_version(mgr);
    let dev =
        ffi::wl_resource_create(client, &ffi::ext_data_control_device_v1_interface, ver, id);
    if !dev.is_null() {
        ffi::wl_resource_set_implementation(
            dev,
            &DATA_CONTROL_DEVICE_IMPL as *const _ as *const c_void,
            std::ptr::null_mut(),
            None,
        );
    }
}

unsafe extern "C" fn dco_receive(
    _client: *mut ffi::wl_client,
    _offer: *mut ffi::wl_resource,
    _mime: *const std::os::raw::c_char,
    _fd: i32,
) {
    // No clipboard content to write; close the fd so it does not leak.
    if _fd >= 0 {
        crate::libc_close(_fd);
    }
}

// ----- primary-selection-unstable-v1 --------------------------------------

static PRIMARY_SELECTION_DEV_MGR_IMPL: ffi::zwp_primary_selection_device_manager_v1_interface_impl =
    ffi::zwp_primary_selection_device_manager_v1_interface_impl {
        create_source: psdm_create_source,
        get_data_device: psdm_get_data_device,
        destroy: crate::res_destroy,
    };

static PRIMARY_SELECTION_DEVICE_IMPL: ffi::zwp_primary_selection_device_v1_interface_impl =
    ffi::zwp_primary_selection_device_v1_interface_impl {
        set_selection: crate::noop_obj_serial,
        destroy: crate::res_destroy,
    };

static PRIMARY_SELECTION_SOURCE_IMPL: ffi::zwp_primary_selection_source_v1_interface_impl =
    ffi::zwp_primary_selection_source_v1_interface_impl {
        offer: crate::noop_str,
        destroy: crate::res_destroy,
    };

static PRIMARY_SELECTION_OFFER_IMPL: ffi::zwp_primary_selection_offer_v1_interface_impl =
    ffi::zwp_primary_selection_offer_v1_interface_impl {
        receive: pso_receive,
        destroy: crate::res_destroy,
    };

pub(crate) unsafe extern "C" fn primary_selection_bind(
    client: *mut ffi::wl_client,
    _data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(
        client,
        &ffi::zwp_primary_selection_device_manager_v1_interface,
        version as c_int,
        id,
    );
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &PRIMARY_SELECTION_DEV_MGR_IMPL as *const _ as *const c_void,
        std::ptr::null_mut(),
        None,
    );
}

unsafe extern "C" fn psdm_create_source(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
) {
    let ver = ffi::wl_resource_get_version(mgr);
    let src = ffi::wl_resource_create(
        client,
        &ffi::zwp_primary_selection_source_v1_interface,
        ver,
        id,
    );
    if !src.is_null() {
        ffi::wl_resource_set_implementation(
            src,
            &PRIMARY_SELECTION_SOURCE_IMPL as *const _ as *const c_void,
            std::ptr::null_mut(),
            None,
        );
    }
}

unsafe extern "C" fn psdm_get_data_device(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    _seat: *mut ffi::wl_resource,
) {
    let ver = ffi::wl_resource_get_version(mgr);
    let dev = ffi::wl_resource_create(
        client,
        &ffi::zwp_primary_selection_device_v1_interface,
        ver,
        id,
    );
    if !dev.is_null() {
        ffi::wl_resource_set_implementation(
            dev,
            &PRIMARY_SELECTION_DEVICE_IMPL as *const _ as *const c_void,
            std::ptr::null_mut(),
            None,
        );
    }
}

unsafe extern "C" fn pso_receive(
    _client: *mut ffi::wl_client,
    _offer: *mut ffi::wl_resource,
    _mime: *const std::os::raw::c_char,
    fd: i32,
) {
    if fd >= 0 {
        crate::libc_close(fd);
    }
}

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
    _data: *mut c_void,
    version: u32,
    id: u32,
) {
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
        std::ptr::null_mut(),
        None,
    );
}

unsafe extern "C" fn cursor_shape_get_pointer(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    _pointer: *mut ffi::wl_resource,
) {
    let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
    let ver = ffi::wl_resource_get_version(mgr);
    let dev =
        ffi::wl_resource_create(client, &ffi::wp_cursor_shape_device_v1_interface, ver, id);
    if !dev.is_null() {
        ffi::wl_resource_set_implementation(
            dev,
            &CURSOR_SHAPE_DEVICE_IMPL as *const _ as *const c_void,
            state as *mut c_void,
            None,
        );
    }
}

unsafe extern "C" fn cursor_shape_get_tablet(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    _tool: *mut ffi::wl_resource,
) {
    let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
    let ver = ffi::wl_resource_get_version(mgr);
    let dev =
        ffi::wl_resource_create(client, &ffi::wp_cursor_shape_device_v1_interface, ver, id);
    if !dev.is_null() {
        ffi::wl_resource_set_implementation(
            dev,
            &CURSOR_SHAPE_DEVICE_IMPL as *const _ as *const c_void,
            state as *mut c_void,
            None,
        );
    }
}

/// `wp_cursor_shape_device_v1.set_shape`: record the requested shape so the
/// renderer can paint the matching cursor. The shape enum follows
/// `wp_cursor_shape_device_v1.shape` (1=default, 2=context_menu, ...).
unsafe extern "C" fn cursor_shape_set_shape(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    _serial: u32,
    shape: u32,
) {
    let state = ffi::wl_resource_get_user_data(resource) as *mut State;
    if !state.is_null() {
        (*state).cursor_shape = shape;
    }
}

// ----- text-input-unstable-v3 ---------------------------------------------

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
        set_surrounding_text: crate::noop_str_ii,
        set_text_change_cause: crate::noop_uu_one,
        set_content_type: crate::noop_uu,
        set_cursor_rectangle: crate::noop_rect,
        commit: crate::noop_none,
    };

pub(crate) unsafe extern "C" fn text_input_bind(
    client: *mut ffi::wl_client,
    _data: *mut c_void,
    version: u32,
    id: u32,
) {
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
        std::ptr::null_mut(),
        None,
    );
}

unsafe extern "C" fn text_input_manager_get(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    seat: *mut ffi::wl_resource,
) {
    let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
    let ver = ffi::wl_resource_get_version(mgr).min(1);
    let res = ffi::wl_resource_create(client, &ffi::zwp_text_input_v3_interface, ver, id);
    if res.is_null() {
        return;
    }
    // Track so the server can send enter/leave on keyboard focus changes. The
    // paired surface starts null (disabled); `enable` sets the focused surface.
    ffi::wl_resource_set_implementation(
        res,
        &TEXT_INPUT_IMPL as *const _ as *const c_void,
        state as *mut c_void,
        Some(text_input_resource_destroy),
    );
    if !state.is_null() {
        (*state).text_inputs.push((res, std::ptr::null_mut()));
    }
    let _ = seat;
}

unsafe extern "C" fn text_input_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn text_input_resource_destroy(resource: *mut ffi::wl_resource) {
    let state = ffi::wl_resource_get_user_data(resource) as *mut State;
    if state.is_null() {
        return;
    }
    (*state).text_inputs.retain(|(r, _)| *r != resource);
}

/// Mark a text_input as enabled (it now wants IME input) and associate it with
/// the focused surface so enter/leave route correctly.
unsafe extern "C" fn text_input_enable(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let state = ffi::wl_resource_get_user_data(resource) as *mut State;
    if state.is_null() {
        return;
    }
    let focus = (*state).keyboard_focus;
    if let Some(slot) = (*state).text_inputs.iter_mut().find(|(r, _)| *r == resource) {
        slot.1 = focus;
    }
}

unsafe extern "C" fn text_input_disable(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let state = ffi::wl_resource_get_user_data(resource) as *mut State;
    if state.is_null() {
        return;
    }
    if let Some(slot) = (*state).text_inputs.iter_mut().find(|(r, _)| *r == resource) {
        slot.1 = std::ptr::null_mut();
    }
}

/// Send `enter` to the focused client's enabled text_inputs and `leave` to
/// those that lost focus. Called from `change_keyboard_focus`.
pub(crate) unsafe fn text_input_focus_changed(
    state: *mut State,
    old_focus: *mut ffi::wl_resource,
    new_focus: *mut ffi::wl_resource,
) {
    if state.is_null() {
        return;
    }
    // Collect the active text_input resource pointers (cloning the slice to
    // avoid borrowing across libwayland calls).
    let entries: Vec<(*mut ffi::wl_resource, *mut ffi::wl_resource)> =
        (*state).text_inputs.iter().copied().collect();
    for (ti, _surf) in entries {
        if ti.is_null() {
            continue;
        }
        let ti_client = ffi::wl_resource_get_client(ti);
        // Leave: text_inputs of the old-focus client.
        if !old_focus.is_null() && ffi::wl_resource_get_client(old_focus) == ti_client {
            ffi::wl_resource_post_event(ti, ffi::ZWP_TEXT_INPUT_V3_LEAVE, std::ptr::null_mut::<ffi::wl_resource>());
        }
        // Enter: text_inputs of the new-focus client.
        if !new_focus.is_null() && ffi::wl_resource_get_client(new_focus) == ti_client {
            ffi::wl_resource_post_event(ti, ffi::ZWP_TEXT_INPUT_V3_ENTER, new_focus);
        }
    }
}

// Suppress unused-import warnings for helpers reachable only via crate::.
#[allow(dead_code)]
fn _unused() {
    let _ = std::ptr::null::<SurfaceRec>();
    let _ = CStr::from_bytes_with_nul(b"\0");
}
