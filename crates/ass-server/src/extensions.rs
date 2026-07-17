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
    let res = ffi::wl_resource_create(client, &ffi::zxdg_output_v1_interface, ver, id);
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
        (*state).xdg_output_links.insert(res as usize, output);
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
    (*state).xdg_output_links.remove(&(resource as usize));
}

/// Post the logical_position / logical_size / (name) events for one
/// xdg_output resource paired with one wl_output resource.
pub(crate) unsafe fn send_xdg_output_geometry(
    res: *mut ffi::wl_resource,
    output: *mut ffi::wl_resource,
    state: *mut State,
) {
    let info = crate::output_info_for_resource(output);
    let geometry = info
        .as_ref()
        .map(|info| info.geometry)
        .or_else(|| (!state.is_null()).then(|| (*state).output_geometry));
    let origin = geometry
        .map(|geometry| geometry.logical_origin)
        .unwrap_or_else(|| crate::ass_core_point(0, 0));
    let size = geometry
        .map(|geometry| geometry.logical_size())
        .unwrap_or_else(|| crate::ass_core_size(1280, 720));
    ffi::wl_resource_post_event(
        res,
        ffi::ZXDG_OUTPUT_V1_LOGICAL_POSITION,
        origin.x,
        origin.y,
    );
    ffi::wl_resource_post_event(res, ffi::ZXDG_OUTPUT_V1_LOGICAL_SIZE, size.w, size.h);
    let ver = ffi::wl_resource_get_version(res);
    if ver >= 2 {
        let connector = info
            .as_ref()
            .map(|info| info.connector.as_str())
            .unwrap_or("unknown");
        let name = CString::new(connector).unwrap_or_else(|_| CString::new("output").unwrap());
        ffi::wl_resource_post_event(res, ffi::ZXDG_OUTPUT_V1_NAME, name.as_ptr());
    }
}

// ----- xdg-decoration-unstable-v1 ----------------------------------------

static XDG_DECORATION_MANAGER_IMPL: ffi::zxdg_decoration_manager_v1_interface_impl =
    ffi::zxdg_decoration_manager_v1_interface_impl {
        destroy: crate::res_destroy,
        get_toplevel_decoration: xdg_decoration_get_toplevel,
    };

static XDG_TOPLEVEL_DECORATION_IMPL: ffi::zxdg_toplevel_decoration_v1_interface_impl =
    ffi::zxdg_toplevel_decoration_v1_interface_impl {
        destroy: xdg_toplevel_decoration_destroy,
        set_mode: xdg_toplevel_decoration_set_mode,
        unset_mode: xdg_toplevel_decoration_unset_mode,
    };

pub(crate) unsafe extern "C" fn xdg_decoration_manager_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    let resource = ffi::wl_resource_create(
        client,
        &ffi::zxdg_decoration_manager_v1_interface,
        version.min(2) as c_int,
        id,
    );
    if !resource.is_null() {
        ffi::wl_resource_set_implementation(
            resource,
            &XDG_DECORATION_MANAGER_IMPL as *const _ as *const c_void,
            data,
            None,
        );
    }
}

unsafe extern "C" fn xdg_decoration_get_toplevel(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    id: u32,
    toplevel: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(toplevel) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    if !(*rec).xdg_decoration.is_null() {
        ffi::wl_resource_post_error(
            manager,
            ffi::ZXDG_TOPLEVEL_DECORATION_V1_ERROR_ALREADY_CONSTRUCTED,
            c"xdg_toplevel already has a decoration object".as_ptr(),
        );
        return;
    };
    let resource = ffi::wl_resource_create(
        client,
        &ffi::zxdg_toplevel_decoration_v1_interface,
        ffi::wl_resource_get_version(manager),
        id,
    );
    if resource.is_null() {
        return;
    }
    (*rec).xdg_decoration = resource;
    ffi::wl_resource_set_implementation(
        resource,
        &XDG_TOPLEVEL_DECORATION_IMPL as *const _ as *const c_void,
        rec as *mut c_void,
        Some(xdg_toplevel_decoration_resource_destroy),
    );
    configure_client_side_decoration(resource, rec);
}

unsafe fn configure_client_side_decoration(resource: *mut ffi::wl_resource, rec: *mut SurfaceRec) {
    ffi::wl_resource_post_event(
        resource,
        ffi::ZXDG_TOPLEVEL_DECORATION_V1_CONFIGURE,
        ffi::ZXDG_TOPLEVEL_DECORATION_V1_MODE_CLIENT_SIDE,
    );
    crate::reconfigure_with_state(rec);
}

unsafe extern "C" fn xdg_toplevel_decoration_set_mode(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    mode: u32,
) {
    if mode != ffi::ZXDG_TOPLEVEL_DECORATION_V1_MODE_CLIENT_SIDE
        && mode != ffi::ZXDG_TOPLEVEL_DECORATION_V1_MODE_SERVER_SIDE
    {
        ffi::wl_resource_post_error(
            resource,
            ffi::ZXDG_TOPLEVEL_DECORATION_V1_ERROR_INVALID_MODE,
            c"invalid xdg-decoration mode".as_ptr(),
        );
        return;
    }
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() {
        // ASS currently renders client content without an out-of-band frame,
        // so client-side decorations are the only truthful effective mode.
        configure_client_side_decoration(resource, rec);
    }
}

unsafe extern "C" fn xdg_toplevel_decoration_unset_mode(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() {
        configure_client_side_decoration(resource, rec);
    }
}

unsafe extern "C" fn xdg_toplevel_decoration_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    xdg_toplevel_decoration_resource_destroy(resource);
    ffi::wl_resource_set_user_data(resource, std::ptr::null_mut());
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn xdg_toplevel_decoration_resource_destroy(resource: *mut ffi::wl_resource) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() && (*rec).xdg_decoration == resource {
        (*rec).xdg_decoration = std::ptr::null_mut();
    }
}

// ----- xdg-activation-v1 --------------------------------------------------

struct ActivationTokenRec {
    state: *mut State,
    client: *mut ffi::wl_client,
    serial: Option<u32>,
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

unsafe extern "C" fn xdg_activation_get_token(
    client: *mut ffi::wl_client,
    activation: *mut ffi::wl_resource,
    id: u32,
) {
    let state = ffi::wl_resource_get_user_data(activation) as *mut State;
    let resource = ffi::wl_resource_create(client, &ffi::xdg_activation_token_v1_interface, 1, id);
    if resource.is_null() {
        return;
    }
    let rec = Box::into_raw(Box::new(ActivationTokenRec {
        state,
        client,
        serial: None,
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

unsafe extern "C" fn activation_token_set_serial(
    _client: *mut ffi::wl_client,
    token: *mut ffi::wl_resource,
    serial: u32,
    _seat: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(token) as *mut ActivationTokenRec;
    if !rec.is_null() && !(*rec).committed {
        (*rec).serial = Some(serial);
    }
}

unsafe extern "C" fn activation_token_set_app_id(
    _client: *mut ffi::wl_client,
    token: *mut ffi::wl_resource,
    _app_id: *const std::os::raw::c_char,
) {
    let rec = ffi::wl_resource_get_user_data(token) as *mut ActivationTokenRec;
    if !rec.is_null() && (*rec).committed {
        ffi::wl_resource_post_error(
            token,
            ffi::XDG_ACTIVATION_TOKEN_V1_ERROR_ALREADY_USED,
            c"activation token already committed".as_ptr(),
        );
    }
}

unsafe extern "C" fn activation_token_set_surface(
    client: *mut ffi::wl_client,
    token: *mut ffi::wl_resource,
    surface: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(token) as *mut ActivationTokenRec;
    if rec.is_null() || (*rec).committed {
        return;
    }
    if !surface.is_null() && ffi::wl_resource_get_client(surface) == client {
        (*rec).surface = surface;
    }
}

unsafe extern "C" fn activation_token_commit(
    _client: *mut ffi::wl_client,
    token_resource: *mut ffi::wl_resource,
) {
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
    let token = format!("ass-activation-{serial:08x}");
    let valid_focus = !state.is_null()
        && !(*state).keyboard_focus.is_null()
        && ffi::wl_resource_get_client((*state).keyboard_focus) == (*rec).client;
    let valid_surface =
        (*rec).surface.is_null() || ffi::wl_resource_get_client((*rec).surface) == (*rec).client;
    let valid_serial = (*rec)
        .serial
        .is_none_or(|serial| !state.is_null() && serial == (*state).last_button_serial);
    if valid_focus && valid_surface && valid_serial {
        (*state).activation_tokens.insert(token.clone());
    }
    if let Ok(token) = CString::new(token) {
        ffi::wl_resource_post_event(
            token_resource,
            ffi::XDG_ACTIVATION_TOKEN_V1_DONE,
            token.as_ptr(),
        );
    }
}

unsafe extern "C" fn activation_token_resource_destroy(resource: *mut ffi::wl_resource) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut ActivationTokenRec;
    if !rec.is_null() {
        drop(Box::from_raw(rec));
    }
}

unsafe extern "C" fn xdg_activation_activate(
    client: *mut ffi::wl_client,
    activation: *mut ffi::wl_resource,
    token: *const std::os::raw::c_char,
    surface: *mut ffi::wl_resource,
) {
    if token.is_null() || surface.is_null() || ffi::wl_resource_get_client(surface) != client {
        return;
    }
    let state = ffi::wl_resource_get_user_data(activation) as *mut State;
    if state.is_null() {
        return;
    }
    let token = CStr::from_ptr(token).to_string_lossy();
    if (*state).activation_tokens.remove(token.as_ref()) {
        (*state).pending_activation = surface;
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
    ffi::wl_resource_post_event(
        res,
        WP_PRESENTATION_CLOCK,
        WL_PRESENTATION_CLOCK_ID_MONOTONIC,
    );
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
    let fb = ffi::wl_resource_create(client, &ffi::wp_presentation_feedback_interface, 1, id);
    if fb.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(fb, std::ptr::null(), std::ptr::null_mut(), None);
    ffi::wl_resource_post_event(fb, ffi::WP_PRESENTATION_FEEDBACK_DISCARDED);
}

// ----- fractional-scale-v1 ------------------------------------------------

struct FractionalScaleRec {
    surface: *mut SurfaceRec,
}

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
    if rec.is_null() {
        return;
    }
    if !(*rec).fractional_scale.is_null() {
        ffi::wl_resource_post_error(
            mgr,
            0,
            c"wl_surface already has a fractional-scale object".as_ptr(),
        );
        return;
    }
    let ver = ffi::wl_resource_get_version(mgr);
    let res = ffi::wl_resource_create(client, &ffi::wp_fractional_scale_v1_interface, ver, id);
    if res.is_null() {
        return;
    }
    let scale_rec = Box::into_raw(Box::new(FractionalScaleRec { surface: rec }));
    ffi::wl_resource_set_implementation(
        res,
        &FRACTIONAL_SCALE_IMPL as *const _ as *const c_void,
        scale_rec as *mut c_void,
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
    let scale = ffi::wl_resource_get_user_data(resource) as *mut FractionalScaleRec;
    if !scale.is_null() && !(*scale).surface.is_null() {
        let surface = (*scale).surface;
        if (*surface).fractional_scale == resource {
            (*surface).fractional_scale = std::ptr::null_mut();
        }
        (*scale).surface = std::ptr::null_mut();
    }
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn fractional_scale_resource_destroy(resource: *mut ffi::wl_resource) {
    let scale = ffi::wl_resource_get_user_data(resource) as *mut FractionalScaleRec;
    if scale.is_null() {
        return;
    }
    if !(*scale).surface.is_null() && (*(*scale).surface).fractional_scale == resource {
        (*(*scale).surface).fractional_scale = std::ptr::null_mut();
    }
    drop(Box::from_raw(scale));
}

pub(crate) unsafe fn fractional_scale_surface_destroyed(surface: *mut SurfaceRec) {
    if surface.is_null() || (*surface).fractional_scale.is_null() {
        return;
    }
    let scale =
        ffi::wl_resource_get_user_data((*surface).fractional_scale) as *mut FractionalScaleRec;
    if !scale.is_null() {
        (*scale).surface = std::ptr::null_mut();
    }
    (*surface).fractional_scale = std::ptr::null_mut();
}

/// Post `wp_fractional_scale_v1.preferred_scale` for one resource, in 120ths
/// (the wire unit). Uses the output's fractional scale.
pub(crate) unsafe fn send_fractional_scale(res: *mut ffi::wl_resource, state: *mut State) {
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

struct IdleInhibitorRec {
    state: *mut State,
    surface: *mut SurfaceRec,
    resource: *mut ffi::wl_resource,
    /// False when created after the session was already idle. The protocol
    /// says such a late inhibitor takes effect only after new user activity.
    honored: bool,
}

pub(crate) unsafe extern "C" fn idle_inhibit_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
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
        data,
        None,
    );
}

unsafe extern "C" fn idle_inhibit_create_inhibitor(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
) {
    let ver = ffi::wl_resource_get_version(mgr);
    let res = ffi::wl_resource_create(client, &ffi::zwp_idle_inhibitor_v1_interface, ver, id);
    if res.is_null() {
        return;
    }
    let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
    let surface = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
    if state.is_null() || surface.is_null() {
        ffi::wl_resource_destroy(res);
        return;
    }
    let already_idle = (*state).idle_notifications.iter().any(|resource| {
        if resource.is_null() {
            return false;
        }
        let notification = ffi::wl_resource_get_user_data(*resource) as *mut IdleNotificationRec;
        !notification.is_null() && !(*notification).ignore_inhibitors && (*notification).idle
    });
    let record = Box::new(IdleInhibitorRec {
        state,
        surface,
        resource: res,
        honored: !already_idle,
    });
    ffi::wl_resource_set_implementation(
        res,
        &IDLE_INHIBITOR_IMPL as *const _ as *const c_void,
        Box::into_raw(record) as *mut c_void,
        Some(idle_inhibitor_resource_destroy),
    );
    (*state).idle_inhibitors.push(res);
    update_idle_notifications(state);
}

unsafe extern "C" fn idle_inhibitor_resource_destroy(resource: *mut ffi::wl_resource) {
    let record = ffi::wl_resource_get_user_data(resource) as *mut IdleInhibitorRec;
    if record.is_null() {
        return;
    }
    let state = (*record).state;
    if !state.is_null() {
        for slot in &mut (*state).idle_inhibitors {
            if *slot == (*record).resource {
                *slot = std::ptr::null_mut();
                break;
            }
        }
    }
    drop(Box::from_raw(record));
    if !state.is_null() {
        update_idle_notifications(state);
    }
}

pub(crate) unsafe fn idle_inhibit_surface_destroyed(surface: *mut SurfaceRec) {
    if surface.is_null() || (*surface).state.is_null() {
        return;
    }
    let state = (*surface).state;
    for resource in &(*state).idle_inhibitors {
        if resource.is_null() {
            continue;
        }
        let record = ffi::wl_resource_get_user_data(*resource) as *mut IdleInhibitorRec;
        if !record.is_null() && (*record).surface == surface {
            (*record).surface = std::ptr::null_mut();
        }
    }
}

// ----- ext-idle-notify-v1 -------------------------------------------------

static IDLE_NOTIFIER_IMPL: ffi::ext_idle_notifier_v1_interface_impl =
    ffi::ext_idle_notifier_v1_interface_impl {
        destroy: crate::res_destroy,
        get_idle_notification: idle_notifier_get,
        get_input_idle_notification: idle_notifier_get_input,
    };

static IDLE_NOTIFICATION_IMPL: ffi::ext_idle_notification_v1_interface_impl =
    ffi::ext_idle_notification_v1_interface_impl {
        destroy: crate::res_destroy,
    };

pub(crate) unsafe extern "C" fn idle_notifier_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
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
        data,
        None,
    );
}

unsafe extern "C" fn idle_notifier_get(
    client: *mut ffi::wl_client,
    notifier: *mut ffi::wl_resource,
    id: u32,
    timeout: u32,
    seat: *mut ffi::wl_resource,
) {
    idle_notifier_create(client, notifier, id, timeout, seat, false);
}

unsafe extern "C" fn idle_notifier_get_input(
    client: *mut ffi::wl_client,
    notifier: *mut ffi::wl_resource,
    id: u32,
    timeout: u32,
    seat: *mut ffi::wl_resource,
) {
    idle_notifier_create(client, notifier, id, timeout, seat, true);
}

struct IdleNotificationRec {
    state: *mut State,
    resource: *mut ffi::wl_resource,
    timeout: std::time::Duration,
    activity_at: std::time::Instant,
    idle: bool,
    ignore_inhibitors: bool,
}

unsafe fn idle_notifier_create(
    client: *mut ffi::wl_client,
    notifier: *mut ffi::wl_resource,
    id: u32,
    timeout: u32,
    seat: *mut ffi::wl_resource,
    ignore_inhibitors: bool,
) {
    let ver = ffi::wl_resource_get_version(notifier);
    let res = ffi::wl_resource_create(client, &ffi::ext_idle_notification_v1_interface, ver, id);
    if res.is_null() {
        return;
    }
    let state = ffi::wl_resource_get_user_data(notifier) as *mut State;
    if state.is_null() || ffi::wl_resource_get_user_data(seat) as *mut State != state {
        ffi::wl_resource_destroy(res);
        return;
    }
    let record = Box::new(IdleNotificationRec {
        state,
        resource: res,
        timeout: std::time::Duration::from_millis(timeout as u64),
        activity_at: std::time::Instant::now(),
        idle: false,
        ignore_inhibitors,
    });
    ffi::wl_resource_set_implementation(
        res,
        &IDLE_NOTIFICATION_IMPL as *const _ as *const c_void,
        Box::into_raw(record) as *mut c_void,
        Some(idle_notification_resource_destroy),
    );
    (*state).idle_notifications.push(res);
}

unsafe extern "C" fn idle_notification_resource_destroy(resource: *mut ffi::wl_resource) {
    let record = ffi::wl_resource_get_user_data(resource) as *mut IdleNotificationRec;
    if record.is_null() {
        return;
    }
    if !(*record).state.is_null() {
        for slot in &mut (*(*record).state).idle_notifications {
            if *slot == (*record).resource {
                *slot = std::ptr::null_mut();
                break;
            }
        }
    }
    drop(Box::from_raw(record));
}

unsafe fn idle_inhibitor_is_active(record: *mut IdleInhibitorRec) -> bool {
    if record.is_null()
        || !(*record).honored
        || (*record).state.is_null()
        || (*record).surface.is_null()
        || (*(*record).state).session_locked
        || !(*(*record).surface).mapped
    {
        return false;
    }
    let root = crate::surface_root_toplevel((*record).surface);
    !root.is_null()
        && (*(*record).state)
            .workspaces
            .visible_toplevels()
            .contains(&(*root).window.id)
}

pub(crate) unsafe fn update_idle_notifications(state: *mut State) {
    if state.is_null() {
        return;
    }
    let inhibited = (*state).idle_inhibitors.iter().any(|resource| {
        if resource.is_null() {
            return false;
        }
        let record = ffi::wl_resource_get_user_data(*resource) as *mut IdleInhibitorRec;
        idle_inhibitor_is_active(record)
    });
    let now = std::time::Instant::now();
    for resource in &(*state).idle_notifications {
        if resource.is_null() {
            continue;
        }
        let record = ffi::wl_resource_get_user_data(*resource) as *mut IdleNotificationRec;
        if record.is_null() {
            continue;
        }
        let should_idle = now.duration_since((*record).activity_at) >= (*record).timeout
            && ((*record).ignore_inhibitors || !inhibited);
        if should_idle && !(*record).idle {
            (*record).idle = true;
            ffi::wl_resource_post_event((*record).resource, ffi::EXT_IDLE_NOTIFICATION_V1_IDLED);
        } else if !should_idle && (*record).idle {
            (*record).idle = false;
            ffi::wl_resource_post_event((*record).resource, ffi::EXT_IDLE_NOTIFICATION_V1_RESUMED);
        }
    }
}

pub(crate) unsafe fn idle_user_activity(state: *mut State) {
    if state.is_null() {
        return;
    }
    let now = std::time::Instant::now();
    for resource in &(*state).idle_notifications {
        if resource.is_null() {
            continue;
        }
        let record = ffi::wl_resource_get_user_data(*resource) as *mut IdleNotificationRec;
        if !record.is_null() {
            (*record).activity_at = now;
            if (*record).idle {
                (*record).idle = false;
                ffi::wl_resource_post_event(
                    (*record).resource,
                    ffi::EXT_IDLE_NOTIFICATION_V1_RESUMED,
                );
            }
        }
    }
    for resource in &(*state).idle_inhibitors {
        if resource.is_null() {
            continue;
        }
        let record = ffi::wl_resource_get_user_data(*resource) as *mut IdleInhibitorRec;
        if !record.is_null() {
            (*record).honored = true;
        }
    }
}

// ----- linux-explicit-synchronization-unstable-v1 -------------------------

struct SurfaceSyncRec {
    state: *mut State,
    resource: *mut ffi::wl_resource,
    surface: *mut SurfaceRec,
    pending_acquire_fence: i32,
    pending_release: *mut ffi::wl_resource,
}

struct ExplicitReleaseRec {
    owner: *mut SurfaceSyncRec,
    state: *mut State,
}

#[repr(C)]
struct SyncFileInfo {
    name: [u8; 32],
    status: i32,
    flags: u32,
    num_fences: u32,
    pad: u32,
    sync_fence_info: u64,
}

// _IOWR('>', 4, struct sync_file_info) from linux/sync_file.h.
const SYNC_IOC_FILE_INFO: std::os::raw::c_ulong = 0xc038_3e04;

static EXPLICIT_SYNC_MANAGER_IMPL: ffi::zwp_linux_explicit_synchronization_v1_interface_impl =
    ffi::zwp_linux_explicit_synchronization_v1_interface_impl {
        destroy: crate::res_destroy,
        get_synchronization: explicit_sync_get_surface,
    };

static SURFACE_SYNC_IMPL: ffi::zwp_linux_surface_synchronization_v1_interface_impl =
    ffi::zwp_linux_surface_synchronization_v1_interface_impl {
        destroy: surface_sync_destroy,
        set_acquire_fence: surface_sync_set_acquire,
        get_release: surface_sync_get_release,
    };

pub(crate) unsafe extern "C" fn explicit_sync_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    let resource = ffi::wl_resource_create(
        client,
        &ffi::zwp_linux_explicit_synchronization_v1_interface,
        version as c_int,
        id,
    );
    if !resource.is_null() {
        ffi::wl_resource_set_implementation(
            resource,
            &EXPLICIT_SYNC_MANAGER_IMPL as *const _ as *const c_void,
            data,
            None,
        );
    }
}

unsafe extern "C" fn explicit_sync_get_surface(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
) {
    let surface = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
    if surface.is_null() {
        return;
    }
    if !(*surface).explicit_sync.is_null() {
        ffi::wl_resource_post_error(manager, 0, c"surface already has explicit sync".as_ptr());
        return;
    }
    let resource = ffi::wl_resource_create(
        client,
        &ffi::zwp_linux_surface_synchronization_v1_interface,
        ffi::wl_resource_get_version(manager),
        id,
    );
    if resource.is_null() {
        return;
    }
    let state = ffi::wl_resource_get_user_data(manager) as *mut State;
    let record = Box::into_raw(Box::new(SurfaceSyncRec {
        state,
        resource,
        surface,
        pending_acquire_fence: -1,
        pending_release: std::ptr::null_mut(),
    }));
    (*surface).explicit_sync = record as *mut c_void;
    ffi::wl_resource_set_implementation(
        resource,
        &SURFACE_SYNC_IMPL as *const _ as *const c_void,
        record as *mut c_void,
        Some(surface_sync_resource_destroy),
    );
}

unsafe extern "C" fn surface_sync_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn surface_sync_set_acquire(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    fd: i32,
) {
    let record = ffi::wl_resource_get_user_data(resource) as *mut SurfaceSyncRec;
    if record.is_null() || (*record).surface.is_null() {
        if fd >= 0 {
            crate::libc_close(fd);
        }
        ffi::wl_resource_post_error(resource, 3, c"associated surface was destroyed".as_ptr());
        return;
    }
    if (*record).pending_acquire_fence >= 0 {
        crate::libc_close(fd);
        ffi::wl_resource_post_error(resource, 1, c"duplicate acquire fence".as_ptr());
        return;
    }
    let mut info = SyncFileInfo {
        name: [0; 32],
        status: 0,
        flags: 0,
        num_fences: 0,
        pad: 0,
        sync_fence_info: 0,
    };
    if fd < 0 || crate::ioctl(fd, SYNC_IOC_FILE_INFO, &mut info) != 0 {
        if fd >= 0 {
            crate::libc_close(fd);
        }
        ffi::wl_resource_post_error(resource, 0, c"invalid sync_file acquire fence".as_ptr());
        return;
    }
    (*record).pending_acquire_fence = fd;
}

unsafe extern "C" fn surface_sync_get_release(
    client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    id: u32,
) {
    let record = ffi::wl_resource_get_user_data(resource) as *mut SurfaceSyncRec;
    if record.is_null() || (*record).surface.is_null() {
        ffi::wl_resource_post_error(resource, 3, c"associated surface was destroyed".as_ptr());
        return;
    }
    if !(*record).pending_release.is_null() {
        ffi::wl_resource_post_error(resource, 2, c"duplicate buffer release".as_ptr());
        return;
    }
    let release =
        ffi::wl_resource_create(client, &ffi::zwp_linux_buffer_release_v1_interface, 1, id);
    if release.is_null() {
        return;
    }
    let release_record = Box::into_raw(Box::new(ExplicitReleaseRec {
        owner: record,
        state: (*record).state,
    }));
    ffi::wl_resource_set_implementation(
        release,
        std::ptr::null(),
        release_record as *mut c_void,
        Some(explicit_release_resource_destroy),
    );
    (*record).pending_release = release;
}

unsafe extern "C" fn explicit_release_resource_destroy(resource: *mut ffi::wl_resource) {
    let release = ffi::wl_resource_get_user_data(resource) as *mut ExplicitReleaseRec;
    if release.is_null() {
        return;
    }
    let owner = (*release).owner;
    if !owner.is_null() && (*owner).pending_release == resource {
        (*owner).pending_release = std::ptr::null_mut();
    }
    let state = (*release).state;
    if !state.is_null() {
        for surface in (*state).live_surfaces() {
            if (*surface).current_explicit_release == resource {
                (*surface).current_explicit_release = std::ptr::null_mut();
            }
        }
        for retired in &mut (*state).retired_buffer_releases {
            if retired.explicit_release == resource {
                retired.explicit_release = std::ptr::null_mut();
            }
        }
    }
    drop(Box::from_raw(release));
}

unsafe extern "C" fn surface_sync_resource_destroy(resource: *mut ffi::wl_resource) {
    let record = ffi::wl_resource_get_user_data(resource) as *mut SurfaceSyncRec;
    if record.is_null() {
        return;
    }
    if (*record).pending_acquire_fence >= 0 {
        crate::libc_close((*record).pending_acquire_fence);
    }
    if !(*record).pending_release.is_null() {
        let release = (*record).pending_release;
        let release_record = ffi::wl_resource_get_user_data(release) as *mut ExplicitReleaseRec;
        if !release_record.is_null() {
            (*release_record).owner = std::ptr::null_mut();
        }
        ffi::wl_resource_post_event(release, ffi::ZWP_LINUX_BUFFER_RELEASE_V1_IMMEDIATE_RELEASE);
        ffi::wl_resource_destroy(release);
    }
    if !(*record).surface.is_null() {
        (*(*record).surface).explicit_sync = std::ptr::null_mut();
    }
    drop(Box::from_raw(record));
}

pub(crate) unsafe fn explicit_sync_surface_committed(
    surface: *mut SurfaceRec,
    buffer_set: bool,
    buffer: *mut ffi::wl_resource,
) -> bool {
    if surface.is_null() || (*surface).explicit_sync.is_null() {
        return true;
    }
    let record = (*surface).explicit_sync as *mut SurfaceSyncRec;
    let has_fence = (*record).pending_acquire_fence >= 0;
    let has_release = !(*record).pending_release.is_null();
    if !has_fence && !has_release {
        return true;
    }
    if !buffer_set || buffer.is_null() {
        ffi::wl_resource_post_error(
            (*record).resource,
            5,
            c"explicit sync commit has no buffer".as_ptr(),
        );
        return false;
    }
    let is_dmabuf = ffi::wl_resource_instance_of(
        buffer,
        &ffi::wl_buffer_interface,
        &crate::WL_BUFFER_IMPL as *const _ as *const c_void,
    ) != 0;
    if has_fence && !is_dmabuf {
        ffi::wl_resource_post_error(
            (*record).resource,
            4,
            c"acquire fence buffer is not a supported dma-buf".as_ptr(),
        );
        return false;
    }
    (*surface).committed_acquire_fence =
        std::mem::replace(&mut (*record).pending_acquire_fence, -1);
    (*surface).committed_explicit_release =
        std::mem::replace(&mut (*record).pending_release, std::ptr::null_mut());
    if !(*surface).committed_explicit_release.is_null() {
        let release = ffi::wl_resource_get_user_data((*surface).committed_explicit_release)
            as *mut ExplicitReleaseRec;
        if !release.is_null() {
            (*release).owner = std::ptr::null_mut();
        }
    }
    true
}

pub(crate) unsafe fn explicit_sync_surface_destroyed(surface: *mut SurfaceRec) {
    if surface.is_null() || (*surface).explicit_sync.is_null() {
        return;
    }
    let record = (*surface).explicit_sync as *mut SurfaceSyncRec;
    (*record).surface = std::ptr::null_mut();
    (*surface).explicit_sync = std::ptr::null_mut();
}

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

unsafe extern "C" fn relative_pointer_manager_get(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    pointer: *mut ffi::wl_resource,
) {
    let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
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
    let rec = ffi::wl_resource_get_user_data(resource) as *mut RelativePointerRec;
    if rec.is_null() {
        return;
    }
    if !(*rec).state.is_null() {
        (*(*rec).state).relative_pointers.retain(|r| *r != resource);
    }
    let _ = (*rec).pointer;
    drop(Box::from_raw(rec));
}

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
        destroy: crate::res_destroy,
        get_swipe_gesture: pointer_gestures_get_swipe,
        get_pinch_gesture: pointer_gestures_get_pinch,
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

unsafe extern "C" fn pointer_gestures_get_swipe(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    id: u32,
    pointer: *mut ffi::wl_resource,
) {
    create_pointer_gesture(client, manager, id, pointer, PointerGestureKind::Swipe);
}

unsafe extern "C" fn pointer_gestures_get_pinch(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    id: u32,
    pointer: *mut ffi::wl_resource,
) {
    create_pointer_gesture(client, manager, id, pointer, PointerGestureKind::Pinch);
}

unsafe extern "C" fn pointer_gestures_get_hold(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    id: u32,
    pointer: *mut ffi::wl_resource,
) {
    create_pointer_gesture(client, manager, id, pointer, PointerGestureKind::Hold);
}

unsafe fn create_pointer_gesture(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    id: u32,
    pointer: *mut ffi::wl_resource,
    kind: PointerGestureKind,
) {
    if pointer.is_null() || ffi::wl_resource_get_client(pointer) != client {
        ffi::wl_resource_post_error(manager, 0, c"pointer belongs to another client".as_ptr());
        return;
    }
    let state = ffi::wl_resource_get_user_data(manager) as *mut State;
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
    match kind {
        PointerGestureKind::Swipe => (*state).pointer_gesture_swipes.push(res),
        PointerGestureKind::Pinch => (*state).pointer_gesture_pinches.push(res),
        PointerGestureKind::Hold => (*state).pointer_gesture_holds.push(res),
    }
}

unsafe extern "C" fn pointer_gesture_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn pointer_gesture_resource_destroy(resource: *mut ffi::wl_resource) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut PointerGestureRec;
    if rec.is_null() {
        return;
    }
    let state = (*rec).state;
    if !state.is_null() {
        match (*rec).kind {
            PointerGestureKind::Swipe => (*state).pointer_gesture_swipes.retain(|r| *r != resource),
            PointerGestureKind::Pinch => {
                (*state).pointer_gesture_pinches.retain(|r| *r != resource)
            }
            PointerGestureKind::Hold => (*state).pointer_gesture_holds.retain(|r| *r != resource),
        }
    }
    drop(Box::from_raw(rec));
}

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

unsafe extern "C" fn keyboard_shortcuts_inhibit(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
    seat: *mut ffi::wl_resource,
) {
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
    (*state).keyboard_shortcut_inhibitors.push(resource);
    if active {
        ffi::wl_resource_post_event(resource, ffi::ZWP_KEYBOARD_SHORTCUTS_INHIBITOR_V1_ACTIVE);
    }
}

unsafe extern "C" fn keyboard_shortcuts_inhibitor_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn keyboard_shortcuts_inhibitor_resource_destroy(
    resource: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut KeyboardShortcutsInhibitorRec;
    if rec.is_null() {
        return;
    }
    if !(*rec).state.is_null() {
        (*(*rec).state)
            .keyboard_shortcut_inhibitors
            .retain(|r| *r != resource);
    }
    drop(Box::from_raw(rec));
}

pub(crate) unsafe fn keyboard_shortcuts_focus_changed(
    state: *mut State,
    new_focus: *mut ffi::wl_resource,
) {
    for resource in (*state).keyboard_shortcut_inhibitors.clone() {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut KeyboardShortcutsInhibitorRec;
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

pub(crate) unsafe fn keyboard_shortcuts_inhibited(state: *mut State) -> bool {
    !(*state).keyboard_focus.is_null()
        && (*state)
            .keyboard_shortcut_inhibitors
            .iter()
            .copied()
            .any(|resource| {
                let rec =
                    ffi::wl_resource_get_user_data(resource) as *mut KeyboardShortcutsInhibitorRec;
                !rec.is_null() && (*rec).active && (*rec).surface == (*state).keyboard_focus
            })
}

pub(crate) unsafe fn keyboard_shortcuts_surface_destroyed(
    state: *mut State,
    surface: *mut ffi::wl_resource,
) {
    for resource in (*state).keyboard_shortcut_inhibitors.clone() {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut KeyboardShortcutsInhibitorRec;
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

// ----- pointer-constraints-unstable-v1 ------------------------------------

struct PointerConstraintRec {
    state: *mut State,
    surface: *mut ffi::wl_resource,
    pointer: *mut ffi::wl_resource,
    locked: bool,
    lifetime: u32,
    active: bool,
    consumed: bool,
    region: Option<Vec<ass_core::Rect>>,
    cursor_hint: Option<(f32, f32)>,
}

static POINTER_CONSTRAINTS_IMPL: ffi::zwp_pointer_constraints_v1_interface_impl =
    ffi::zwp_pointer_constraints_v1_interface_impl {
        destroy: crate::res_destroy,
        lock_pointer: pc_lock_pointer,
        confine_pointer: pc_confine_pointer,
    };

static CONFINED_POINTER_IMPL: ffi::zwp_confined_pointer_v1_interface_impl =
    ffi::zwp_confined_pointer_v1_interface_impl {
        destroy: pointer_constraint_destroy,
        set_region: pointer_constraint_set_region,
    };

static LOCKED_POINTER_IMPL: ffi::zwp_locked_pointer_v1_interface_impl =
    ffi::zwp_locked_pointer_v1_interface_impl {
        destroy: pointer_constraint_destroy,
        set_cursor_position_hint: locked_pointer_set_cursor_hint,
        set_region: pointer_constraint_set_region,
    };

pub(crate) unsafe extern "C" fn pointer_constraints_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
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
        data,
        None,
    );
}

unsafe extern "C" fn pc_lock_pointer(
    client: *mut ffi::wl_client,
    pc: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
    pointer: *mut ffi::wl_resource,
    region: *mut ffi::wl_resource,
    lifetime: u32,
) {
    create_pointer_constraint(client, pc, id, surface, pointer, region, lifetime, true);
}

unsafe extern "C" fn pc_confine_pointer(
    client: *mut ffi::wl_client,
    pc: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
    pointer: *mut ffi::wl_resource,
    region: *mut ffi::wl_resource,
    lifetime: u32,
) {
    create_pointer_constraint(client, pc, id, surface, pointer, region, lifetime, false);
}

#[allow(clippy::too_many_arguments)]
unsafe fn create_pointer_constraint(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
    pointer: *mut ffi::wl_resource,
    region: *mut ffi::wl_resource,
    lifetime: u32,
    locked: bool,
) {
    let state = ffi::wl_resource_get_user_data(manager) as *mut State;
    if state.is_null() || (lifetime != 1 && lifetime != 2) {
        return;
    }
    let duplicate = (*state)
        .pointer_constraints
        .iter()
        .copied()
        .any(|resource| {
            if resource.is_null() {
                return false;
            }
            let rec = ffi::wl_resource_get_user_data(resource) as *mut PointerConstraintRec;
            !rec.is_null() && (*rec).surface == surface && (*rec).pointer == pointer
        });
    if duplicate {
        ffi::wl_resource_post_error(
            manager,
            1,
            c"pointer is already constrained on this surface".as_ptr(),
        );
        return;
    }
    let ver = ffi::wl_resource_get_version(manager);
    let interface = if locked {
        &ffi::zwp_locked_pointer_v1_interface
    } else {
        &ffi::zwp_confined_pointer_v1_interface
    };
    let res = ffi::wl_resource_create(client, interface, ver, id);
    if res.is_null() {
        return;
    }
    let region = copy_region(region);
    let active = (*state).pointer_focus == surface;
    let rec = Box::into_raw(Box::new(PointerConstraintRec {
        state,
        surface,
        pointer,
        locked,
        lifetime,
        active,
        consumed: false,
        region,
        cursor_hint: None,
    }));
    let implementation = if locked {
        &LOCKED_POINTER_IMPL as *const _ as *const c_void
    } else {
        &CONFINED_POINTER_IMPL as *const _ as *const c_void
    };
    ffi::wl_resource_set_implementation(
        res,
        implementation,
        rec as *mut c_void,
        Some(pointer_constraint_resource_destroy),
    );
    (*state).pointer_constraints.push(res);
    if active {
        ffi::wl_resource_post_event(
            res,
            if locked {
                ffi::ZWP_LOCKED_POINTER_V1_LOCKED
            } else {
                ffi::ZWP_CONFINED_POINTER_V1_CONFINED
            },
        );
    }
}

unsafe fn copy_region(region: *mut ffi::wl_resource) -> Option<Vec<ass_core::Rect>> {
    if region.is_null() {
        return None;
    }
    let region = ffi::wl_resource_get_user_data(region) as *mut crate::RegionRec;
    if region.is_null() {
        Some(Vec::new())
    } else {
        Some((*region).rects.clone())
    }
}

unsafe extern "C" fn pointer_constraint_set_region(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    region: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut PointerConstraintRec;
    if !rec.is_null() {
        (*rec).region = copy_region(region);
    }
}

unsafe extern "C" fn locked_pointer_set_cursor_hint(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut PointerConstraintRec;
    if !rec.is_null() && (*rec).locked {
        (*rec).cursor_hint = Some((x as f32 / 256.0, y as f32 / 256.0));
    }
}

unsafe extern "C" fn pointer_constraint_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn pointer_constraint_resource_destroy(resource: *mut ffi::wl_resource) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut PointerConstraintRec;
    if rec.is_null() {
        return;
    }
    if !(*rec).state.is_null() {
        (*(*rec).state)
            .pointer_constraints
            .retain(|r| *r != resource);
    }
    drop(Box::from_raw(rec));
}

pub(crate) unsafe fn pointer_constraint_focus_changed(
    state: *mut State,
    old_focus: *mut ffi::wl_resource,
    new_focus: *mut ffi::wl_resource,
) {
    if state.is_null() {
        return;
    }
    for resource in (*state).pointer_constraints.clone() {
        if resource.is_null() {
            continue;
        }
        let rec = ffi::wl_resource_get_user_data(resource) as *mut PointerConstraintRec;
        if rec.is_null() {
            continue;
        }
        if (*rec).active && (*rec).surface == old_focus {
            (*rec).active = false;
            ffi::wl_resource_post_event(
                resource,
                if (*rec).locked {
                    ffi::ZWP_LOCKED_POINTER_V1_UNLOCKED
                } else {
                    ffi::ZWP_CONFINED_POINTER_V1_UNCONFINED
                },
            );
            if (*rec).locked {
                if let Some((x, y)) = (*rec).cursor_hint {
                    let surface = ffi::wl_resource_get_user_data(old_focus) as *mut SurfaceRec;
                    if !surface.is_null() {
                        // The hint is surface-local; restore it relative to
                        // the buffer's draw origin (window-geometry insets).
                        let origin = crate::surface_draw_origin(&*surface);
                        (*state).pointer_x = origin.x as f32 + x;
                        (*state).pointer_y = origin.y as f32 + y;
                    }
                }
            }
            if (*rec).lifetime == 1 {
                (*rec).consumed = true;
            }
        }
        if !(*rec).active && !(*rec).consumed && (*rec).surface == new_focus {
            (*rec).active = true;
            ffi::wl_resource_post_event(
                resource,
                if (*rec).locked {
                    ffi::ZWP_LOCKED_POINTER_V1_LOCKED
                } else {
                    ffi::ZWP_CONFINED_POINTER_V1_CONFINED
                },
            );
        }
    }
}

pub(crate) unsafe fn constrain_pointer_motion(state: *mut State, x: f32, y: f32) -> (f32, f32) {
    if state.is_null() || (*state).pointer_focus.is_null() {
        return (x, y);
    }
    for resource in (*state).pointer_constraints.clone() {
        if resource.is_null() {
            continue;
        }
        let rec = ffi::wl_resource_get_user_data(resource) as *mut PointerConstraintRec;
        if rec.is_null() || !(*rec).active || (*rec).surface != (*state).pointer_focus {
            continue;
        }
        if (*rec).locked {
            return ((*state).pointer_x, (*state).pointer_y);
        }
        let surface = ffi::wl_resource_get_user_data((*rec).surface) as *mut SurfaceRec;
        if surface.is_null() {
            return (x, y);
        }
        let local_x = x - crate::surface_draw_origin(&*surface).x as f32;
        let local_y = y - crate::surface_draw_origin(&*surface).y as f32;
        let bounds = (*rec).region.clone().unwrap_or_else(|| {
            let size = crate::surface_logical_size(&*surface);
            vec![ass_core::Rect::new(0, 0, size.w, size.h)]
        });
        if bounds.is_empty() {
            return ((*state).pointer_x, (*state).pointer_y);
        }
        if bounds.iter().any(|rect| {
            rect.contains(ass_core::Point {
                x: local_x.floor() as i32,
                y: local_y.floor() as i32,
            })
        }) {
            return (x, y);
        }
        let (cx, cy, _) = bounds
            .iter()
            .map(|rect| {
                let min_x = rect.origin.x as f32;
                let min_y = rect.origin.y as f32;
                let max_x = (rect.origin.x + rect.size.w).saturating_sub(1) as f32;
                let max_y = (rect.origin.y + rect.size.h).saturating_sub(1) as f32;
                let cx = local_x.clamp(min_x, max_x.max(min_x));
                let cy = local_y.clamp(min_y, max_y.max(min_y));
                let distance = (local_x - cx).powi(2) + (local_y - cy).powi(2);
                (cx, cy, distance)
            })
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .unwrap();
        let origin = crate::surface_draw_origin(&*surface);
        return (origin.x as f32 + cx, origin.y as f32 + cy);
    }
    (x, y)
}

// ----- ext-session-lock-v1 ------------------------------------------------

struct SessionLockRec {
    state: *mut State,
    resource: *mut ffi::wl_resource,
    surfaces: Vec<*mut LockSurfaceRec>,
    locked_sent: bool,
    finished_sent: bool,
}

struct LockSurfaceRec {
    lock: *mut SessionLockRec,
    resource: *mut ffi::wl_resource,
    surface: *mut SurfaceRec,
    connector: String,
    pending_configures: Vec<(u32, ass_core::Size)>,
    acked_configure: Option<(u32, ass_core::Size)>,
}

static SESSION_LOCK_MANAGER_IMPL: ffi::ext_session_lock_manager_v1_interface_impl =
    ffi::ext_session_lock_manager_v1_interface_impl {
        destroy: crate::res_destroy,
        lock: session_lock_manager_lock,
    };

static SESSION_LOCK_IMPL: ffi::ext_session_lock_v1_interface_impl =
    ffi::ext_session_lock_v1_interface_impl {
        destroy: session_lock_destroy,
        get_lock_surface: session_lock_get_surface,
        unlock_and_destroy: session_lock_unlock,
    };

static SESSION_LOCK_SURFACE_IMPL: ffi::ext_session_lock_surface_v1_interface_impl =
    ffi::ext_session_lock_surface_v1_interface_impl {
        destroy: session_lock_surface_destroy,
        ack_configure: session_lock_surface_ack,
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
    let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
    let mut record = Box::new(SessionLockRec {
        state,
        resource: res,
        surfaces: Vec::new(),
        locked_sent: false,
        finished_sent: false,
    });
    let record_ptr = record.as_mut() as *mut SessionLockRec;
    ffi::wl_resource_set_implementation(
        res,
        &SESSION_LOCK_IMPL as *const _ as *const c_void,
        Box::into_raw(record) as *mut c_void,
        Some(session_lock_resource_destroy),
    );
    if state.is_null() || !(*state).session_lock.is_null() || (*state).session_locked {
        (*record_ptr).finished_sent = true;
        ffi::wl_resource_post_event(res, ffi::EXT_SESSION_LOCK_V1_FINISHED);
        return;
    }
    (*state).session_lock = record_ptr as *mut c_void;
    (*state).session_locked = true;
    log::info!("[server] session lock requested; normal content hidden");
    (*state).session_lock_requested_at = Some(std::time::Instant::now());
    (*state).lock_frame_pending = false;
    (*state).pre_lock_keyboard_focus = (*state).keyboard_focus;
    (*state).pending_lock_focus = std::ptr::null_mut();
    (*state).lock_focus_dirty = true;
    (*state).interactive = None;
    (*state).compositor_pointer_grab = false;
    (*state).implicit_grab_active = false;
    if (*state).drag.is_some() {
        crate::cancel_drag(state, true);
    }
}

unsafe extern "C" fn session_lock_get_surface(
    client: *mut ffi::wl_client,
    lock: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
    output: *mut ffi::wl_resource,
) {
    let ver = ffi::wl_resource_get_version(lock);
    let lock_rec = ffi::wl_resource_get_user_data(lock) as *mut SessionLockRec;
    let surface_rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
    if lock_rec.is_null() || surface_rec.is_null() {
        return;
    }
    if crate::surface_has_role(&*surface_rec) {
        ffi::wl_resource_post_error(lock, 2, c"wl_surface already has a role".as_ptr());
        return;
    }
    if (*surface_rec).mapped || (*surface_rec).pending_buffer_set || (*surface_rec).generation != 0
    {
        ffi::wl_resource_post_error(lock, 4, c"wl_surface was already constructed".as_ptr());
        return;
    }
    let Some(output_info) = crate::output_info_for_resource(output) else {
        ffi::wl_resource_post_error(lock, 3, c"unknown lock output".as_ptr());
        return;
    };
    if (*lock_rec)
        .surfaces
        .iter()
        .any(|surface| !surface.is_null() && (**surface).connector == output_info.connector)
    {
        ffi::wl_resource_post_error(lock, 3, c"duplicate lock output".as_ptr());
        return;
    }
    let res = ffi::wl_resource_create(client, &ffi::ext_session_lock_surface_v1_interface, ver, id);
    if res.is_null() {
        return;
    }
    let state = (*lock_rec).state;
    let geometry = output_info.geometry;
    let size = geometry.logical_size();
    let serial = ffi::wl_display_next_serial((*state).display);
    let mut record = Box::new(LockSurfaceRec {
        lock: lock_rec,
        resource: res,
        surface: surface_rec,
        connector: output_info.connector,
        pending_configures: vec![(serial, size)],
        acked_configure: None,
    });
    let record_ptr = record.as_mut() as *mut LockSurfaceRec;
    ffi::wl_resource_set_implementation(
        res,
        &SESSION_LOCK_SURFACE_IMPL as *const _ as *const c_void,
        Box::into_raw(record) as *mut c_void,
        Some(session_lock_surface_resource_destroy),
    );
    (*surface_rec).session_lock_surface = record_ptr as *mut c_void;
    (*surface_rec).state = state;
    (*surface_rec).display = (*state).display;
    (*surface_rec).position = geometry.logical_origin;
    (*lock_rec).surfaces.push(record_ptr);
    ffi::wl_resource_post_event(
        res,
        ffi::EXT_SESSION_LOCK_SURFACE_V1_CONFIGURE,
        serial,
        size.w as u32,
        size.h as u32,
    );
}

unsafe extern "C" fn session_lock_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let record = ffi::wl_resource_get_user_data(resource) as *mut SessionLockRec;
    if !record.is_null() && (*record).locked_sent {
        ffi::wl_resource_post_error(resource, 0, c"locked session requires unlock".as_ptr());
        return;
    }
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn session_lock_unlock(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let record = ffi::wl_resource_get_user_data(resource) as *mut SessionLockRec;
    if record.is_null() || !(*record).locked_sent {
        ffi::wl_resource_post_error(resource, 1, c"lock was never acknowledged".as_ptr());
        return;
    }
    let state = (*record).state;
    if !state.is_null() && (*state).session_lock == record as *mut c_void {
        (*state).session_lock = std::ptr::null_mut();
        (*state).session_locked = false;
        (*state).session_lock_requested_at = None;
        (*state).lock_frame_pending = false;
        (*state).pending_lock_focus = (*state).pre_lock_keyboard_focus;
        (*state).pre_lock_keyboard_focus = std::ptr::null_mut();
        (*state).lock_focus_dirty = true;
        log::info!("[server] session unlocked by lock client");
    }
    for surface in &(*record).surfaces {
        if !surface.is_null() {
            (**surface).lock = std::ptr::null_mut();
            if !(**surface).surface.is_null() {
                (*(**surface).surface).session_lock_surface = std::ptr::null_mut();
                (*(**surface).surface).mapped = false;
            }
        }
    }
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn session_lock_resource_destroy(resource: *mut ffi::wl_resource) {
    let record = ffi::wl_resource_get_user_data(resource) as *mut SessionLockRec;
    if record.is_null() {
        return;
    }
    let state = (*record).state;
    if !state.is_null() && (*state).session_lock == record as *mut c_void {
        // Fail closed if the client dies after locking.
        (*state).session_lock = std::ptr::null_mut();
        if !(*record).locked_sent {
            (*state).session_locked = false;
            (*state).session_lock_requested_at = None;
            (*state).lock_frame_pending = false;
            log::warn!("[server] session lock client exited before lock confirmation");
        } else {
            log::error!(
                "[server] session lock client exited while locked; retaining fail-closed black screen"
            );
        }
    }
    for surface in &(*record).surfaces {
        if !surface.is_null() {
            (**surface).lock = std::ptr::null_mut();
        }
    }
    drop(Box::from_raw(record));
}

unsafe extern "C" fn session_lock_surface_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn session_lock_surface_resource_destroy(resource: *mut ffi::wl_resource) {
    let record = ffi::wl_resource_get_user_data(resource) as *mut LockSurfaceRec;
    if record.is_null() {
        return;
    }
    if !(*record).surface.is_null() {
        (*(*record).surface).session_lock_surface = std::ptr::null_mut();
        (*(*record).surface).mapped = false;
    }
    if !(*record).lock.is_null() {
        let state = (*(*record).lock).state;
        if !state.is_null() && (*state).session_locked {
            // A destroyed lock surface immediately reveals the compositor's
            // solid fallback on its output. Confirm that replacement frame
            // before acknowledging an in-flight lock request.
            (*state).lock_frame_pending = true;
        }
        for slot in &mut (*(*record).lock).surfaces {
            if *slot == record {
                *slot = std::ptr::null_mut();
                break;
            }
        }
    }
    drop(Box::from_raw(record));
}

unsafe extern "C" fn session_lock_surface_ack(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    serial: u32,
) {
    let record = ffi::wl_resource_get_user_data(resource) as *mut LockSurfaceRec;
    if record.is_null() {
        return;
    }
    let Some(index) = (*record)
        .pending_configures
        .iter()
        .position(|candidate| candidate.0 == serial)
    else {
        ffi::wl_resource_post_error(resource, 3, c"invalid configure serial".as_ptr());
        return;
    };
    let size = (&(*record).pending_configures)[index].1;
    (*record).pending_configures.drain(..=index);
    (*record).acked_configure = Some((serial, size));
}

pub(crate) unsafe fn session_lock_surface_committed(surface: *mut SurfaceRec) {
    if surface.is_null() || (*surface).session_lock_surface.is_null() {
        return;
    }
    let lock_surface = (*surface).session_lock_surface as *mut LockSurfaceRec;
    let Some((_, configured_size)) = (*lock_surface).acked_configure else {
        ffi::wl_resource_post_error(
            (*lock_surface).resource,
            0,
            c"commit before first ack_configure".as_ptr(),
        );
        return;
    };
    if !(*surface).mapped {
        ffi::wl_resource_post_error(
            (*lock_surface).resource,
            1,
            c"lock surface requires a buffer".as_ptr(),
        );
        return;
    }
    if crate::surface_logical_size(&*surface) != configured_size {
        ffi::wl_resource_post_error(
            (*lock_surface).resource,
            2,
            c"lock surface dimensions do not match configure".as_ptr(),
        );
        return;
    }
    let lock = (*lock_surface).lock;
    if lock.is_null() || (*lock).state.is_null() {
        return;
    }
    let state = (*lock).state;
    let all_mapped = (*state).output_infos.iter().all(|output| {
        (*lock).surfaces.iter().any(|candidate| {
            !candidate.is_null()
                && (**candidate).connector == output.connector
                && (**candidate).acked_configure.is_some()
                && !(**candidate).surface.is_null()
                && (*(**candidate).surface).mapped
        })
    });
    if all_mapped {
        (*state).lock_frame_pending = true;
        (*state).pending_lock_focus = (*surface).resource;
        (*state).lock_focus_dirty = true;
    }
}

pub(crate) unsafe fn session_lock_presented(state: *mut State) {
    if state.is_null() || !(*state).session_locked || !(*state).lock_frame_pending {
        return;
    }
    let lock = (*state).session_lock as *mut SessionLockRec;
    (*state).lock_frame_pending = false;
    if lock.is_null() || (*lock).locked_sent || (*lock).finished_sent {
        return;
    }
    (*state).session_lock_requested_at = None;
    (*lock).locked_sent = true;
    ffi::wl_resource_post_event((*lock).resource, ffi::EXT_SESSION_LOCK_V1_LOCKED);
    log::info!("[server] secure frame confirmed on all outputs; session locked");
}

/// Detach the role record before a `wl_surface` allocation is reclaimed.
/// The protocol role object may outlive its surface resource, so keeping its
/// raw back-pointer would otherwise turn a later destroy into a use-after-free.
pub(crate) unsafe fn session_lock_surface_destroyed(surface: *mut SurfaceRec) {
    if surface.is_null() || (*surface).session_lock_surface.is_null() {
        return;
    }
    let record = (*surface).session_lock_surface as *mut LockSurfaceRec;
    (*record).surface = std::ptr::null_mut();
    (*surface).session_lock_surface = std::ptr::null_mut();
    if !(*record).lock.is_null() {
        let state = (*(*record).lock).state;
        if !state.is_null() && (*state).session_locked {
            (*state).lock_frame_pending = true;
        }
    }
}

pub(crate) unsafe fn is_active_session_lock_surface(
    state: *mut State,
    surface: *mut SurfaceRec,
) -> bool {
    if state.is_null()
        || surface.is_null()
        || !(*state).session_locked
        || (*state).session_lock.is_null()
        || (*surface).session_lock_surface.is_null()
    {
        return false;
    }
    let record = (*surface).session_lock_surface as *mut LockSurfaceRec;
    (*record).lock as *mut c_void == (*state).session_lock
        && (*state)
            .output_infos
            .iter()
            .any(|output| output.connector == (*record).connector)
}

pub(crate) unsafe fn is_active_session_lock_client_resource(
    state: *mut State,
    resource: *mut ffi::wl_resource,
) -> bool {
    if state.is_null() || resource.is_null() || (*state).session_lock.is_null() {
        return false;
    }
    let lock = (*state).session_lock as *mut SessionLockRec;
    ffi::wl_resource_get_client(resource) == ffi::wl_resource_get_client((*lock).resource)
}

/// Reposition and reconfigure lock surfaces after an output mode/layout
/// change. Removed-output role objects remain valid but are not rendered;
/// clients normally destroy them after the `wl_output` global disappears.
pub(crate) unsafe fn session_lock_outputs_changed(state: *mut State) {
    if state.is_null() || !(*state).session_locked {
        return;
    }
    (*state).lock_frame_pending = true;
    let lock = (*state).session_lock as *mut SessionLockRec;
    if lock.is_null() {
        return;
    }
    for candidate in &(*lock).surfaces {
        if candidate.is_null() || (**candidate).surface.is_null() {
            continue;
        }
        let Some(output) = (*state)
            .output_infos
            .iter()
            .find(|output| output.connector == (**candidate).connector)
        else {
            continue;
        };
        let size = output.geometry.logical_size();
        (*(**candidate).surface).position = output.geometry.logical_origin;
        let already_configured = (**candidate)
            .pending_configures
            .last()
            .is_some_and(|(_, pending)| *pending == size)
            || (**candidate)
                .acked_configure
                .is_some_and(|(_, acked)| acked == size);
        if already_configured {
            continue;
        }
        let serial = ffi::wl_display_next_serial((*state).display);
        (**candidate).pending_configures.push((serial, size));
        ffi::wl_resource_post_event(
            (**candidate).resource,
            ffi::EXT_SESSION_LOCK_SURFACE_V1_CONFIGURE,
            serial,
            size.w as u32,
            size.h as u32,
        );
    }
}

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
            if s.xdg_toplevel.is_null() || !s.mapped {
                continue;
            }
            create_foreign_handle(res, s as *const SurfaceRec as *mut SurfaceRec, state);
        }
    }
}

unsafe extern "C" fn foreign_toplevel_stop(
    _client: *mut ffi::wl_client,
    list: *mut ffi::wl_resource,
) {
    let state = ffi::wl_resource_get_user_data(list) as *mut State;
    if !state.is_null() {
        (*state).foreign_toplevel_lists.retain(|r| *r != list);
    }
    ffi::wl_resource_post_event(list, ffi::EXT_FOREIGN_TOPLEVEL_LIST_V1_FINISHED);
}

unsafe extern "C" fn foreign_toplevel_list_resource_destroy(list: *mut ffi::wl_resource) {
    let state = ffi::wl_resource_get_user_data(list) as *mut State;
    if !state.is_null() {
        (*state).foreign_toplevel_lists.retain(|r| *r != list);
    }
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
        Some(foreign_toplevel_handle_resource_destroy),
    );
    ffi::wl_resource_post_event(list, ffi::EXT_FOREIGN_TOPLEVEL_LIST_V1_TOPLEVEL, handle);
    let wid = (*rec).window.id.0;
    if let Some(title) = &(*rec).window.title {
        if let Ok(c) = CString::new(title.as_str()) {
            ffi::wl_resource_post_event(
                handle,
                ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_TITLE,
                c.as_ptr(),
            );
        }
    }
    if let Some(app_id) = &(*rec).window.app_id {
        if let Ok(c) = CString::new(app_id.as_str()) {
            ffi::wl_resource_post_event(
                handle,
                ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_APP_ID,
                c.as_ptr(),
            );
        }
    }
    if let Ok(c) = CString::new(format!("ass:{wid}")) {
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

unsafe extern "C" fn foreign_toplevel_handle_destroy(
    _client: *mut ffi::wl_client,
    handle: *mut ffi::wl_resource,
) {
    foreign_toplevel_handle_resource_destroy(handle);
    ffi::wl_resource_set_user_data(handle, std::ptr::null_mut());
    ffi::wl_resource_destroy(handle);
}

unsafe extern "C" fn foreign_toplevel_handle_resource_destroy(handle: *mut ffi::wl_resource) {
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
    let Some(handles) = (*state).foreign_handles.get(&wid).cloned() else {
        return;
    };
    for handle in handles.into_iter().filter(|r| !r.is_null()) {
        if let Some(title) = &(*rec).window.title {
            if let Ok(c) = CString::new(title.as_str()) {
                ffi::wl_resource_post_event(
                    handle,
                    ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_TITLE,
                    c.as_ptr(),
                );
            }
        }
        if let Some(app_id) = &(*rec).window.app_id {
            if let Ok(c) = CString::new(app_id.as_str()) {
                ffi::wl_resource_post_event(
                    handle,
                    ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_APP_ID,
                    c.as_ptr(),
                );
            }
        }
        ffi::wl_resource_post_event(handle, ffi::EXT_FOREIGN_TOPLEVEL_HANDLE_V1_DONE);
    }
}

/// Notify `closed` on the handle and drop it from the tracking map.
pub(crate) unsafe fn foreign_toplevel_removed(wid: u64, state: *mut State) {
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
    let src = ffi::wl_resource_create(client, &ffi::ext_data_control_source_v1_interface, ver, id);
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
    let dev = ffi::wl_resource_create(client, &ffi::ext_data_control_device_v1_interface, ver, id);
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
        set_selection: primary_selection_set,
        destroy: crate::res_destroy,
    };

static PRIMARY_SELECTION_SOURCE_IMPL: ffi::zwp_primary_selection_source_v1_interface_impl =
    ffi::zwp_primary_selection_source_v1_interface_impl {
        offer: primary_source_offer,
        destroy: crate::res_destroy,
    };

static PRIMARY_SELECTION_OFFER_IMPL: ffi::zwp_primary_selection_offer_v1_interface_impl =
    ffi::zwp_primary_selection_offer_v1_interface_impl {
        receive: pso_receive,
        destroy: crate::res_destroy,
    };

pub(crate) unsafe extern "C" fn primary_selection_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
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
        data,
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
        let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
        let rec = Box::into_raw(Box::new(crate::PrimarySourceRec {
            state,
            mime_types: Vec::new(),
        }));
        ffi::wl_resource_set_implementation(
            src,
            &PRIMARY_SELECTION_SOURCE_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            Some(primary_source_resource_destroy),
        );
    }
}

unsafe extern "C" fn primary_source_offer(
    _client: *mut ffi::wl_client,
    source: *mut ffi::wl_resource,
    mime: *const std::os::raw::c_char,
) {
    let rec = ffi::wl_resource_get_user_data(source) as *mut crate::PrimarySourceRec;
    if rec.is_null() || mime.is_null() {
        return;
    }
    let mime = CStr::from_ptr(mime).to_string_lossy().into_owned();
    if !(*rec).mime_types.contains(&mime) {
        (*rec).mime_types.push(mime);
    }
}

unsafe extern "C" fn primary_source_resource_destroy(source: *mut ffi::wl_resource) {
    let rec = ffi::wl_resource_get_user_data(source) as *mut crate::PrimarySourceRec;
    if rec.is_null() {
        return;
    }
    let state = (*rec).state;
    if !state.is_null() {
        if (*state)
            .primary_selection
            .as_ref()
            .is_some_and(|s| s.source == source)
        {
            (*state).primary_selection = None;
            notify_primary_selection(state);
        }
        for offer in (*state)
            .primary_offers
            .iter()
            .copied()
            .filter(|r| !r.is_null())
        {
            let offer_rec = ffi::wl_resource_get_user_data(offer) as *mut crate::PrimaryOfferRec;
            if !offer_rec.is_null() && (*offer_rec).source == source {
                (*offer_rec).source = std::ptr::null_mut();
            }
        }
    }
    drop(Box::from_raw(rec));
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
        let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
        ffi::wl_resource_set_implementation(
            dev,
            &PRIMARY_SELECTION_DEVICE_IMPL as *const _ as *const c_void,
            state as *mut c_void,
            Some(primary_device_resource_destroy),
        );
        if !state.is_null() {
            (*state).primary_devices.push(dev);
            if !(*state).keyboard_focus.is_null()
                && ffi::wl_resource_get_client((*state).keyboard_focus) == client
            {
                advertise_primary_selection(dev, state);
            }
        }
    }
}

unsafe extern "C" fn primary_device_resource_destroy(device: *mut ffi::wl_resource) {
    let state = ffi::wl_resource_get_user_data(device) as *mut State;
    if !state.is_null() {
        (*state).primary_devices.retain(|r| *r != device);
    }
}

unsafe extern "C" fn primary_selection_set(
    client: *mut ffi::wl_client,
    device: *mut ffi::wl_resource,
    source: *mut ffi::wl_resource,
    _serial: u32,
) {
    let state = ffi::wl_resource_get_user_data(device) as *mut State;
    if state.is_null()
        || (!source.is_null() && ffi::wl_resource_get_client(source) != client)
        || (*state).keyboard_focus.is_null()
        || ffi::wl_resource_get_client((*state).keyboard_focus) != client
    {
        return;
    }
    let replacement = if source.is_null() {
        None
    } else {
        let rec = ffi::wl_resource_get_user_data(source) as *mut crate::PrimarySourceRec;
        if rec.is_null() {
            return;
        }
        Some(crate::PrimarySelection {
            source,
            mime_types: (*rec).mime_types.clone(),
        })
    };
    if let Some(old) = std::mem::replace(&mut (*state).primary_selection, replacement) {
        if old.source != source {
            ffi::wl_resource_post_event(old.source, ffi::ZWP_PRIMARY_SELECTION_SOURCE_V1_CANCELLED);
        }
    }
    notify_primary_selection(state);
}

unsafe fn create_primary_offer(
    device: *mut ffi::wl_resource,
    state: *mut State,
    selection: &crate::PrimarySelection,
) -> *mut ffi::wl_resource {
    let offer = ffi::wl_resource_create(
        ffi::wl_resource_get_client(device),
        &ffi::zwp_primary_selection_offer_v1_interface,
        1,
        0,
    );
    if offer.is_null() {
        return offer;
    }
    let rec = Box::into_raw(Box::new(crate::PrimaryOfferRec {
        state,
        source: selection.source,
    }));
    ffi::wl_resource_set_implementation(
        offer,
        &PRIMARY_SELECTION_OFFER_IMPL as *const _ as *const c_void,
        rec as *mut c_void,
        Some(primary_offer_resource_destroy),
    );
    (*state).primary_offers.push(offer);
    ffi::wl_resource_post_event(
        device,
        ffi::ZWP_PRIMARY_SELECTION_DEVICE_V1_DATA_OFFER,
        offer,
    );
    for mime in &selection.mime_types {
        if let Ok(mime) = CString::new(mime.as_str()) {
            ffi::wl_resource_post_event(
                offer,
                ffi::ZWP_PRIMARY_SELECTION_OFFER_V1_OFFER,
                mime.as_ptr(),
            );
        }
    }
    offer
}

unsafe fn advertise_primary_selection(device: *mut ffi::wl_resource, state: *mut State) {
    let offer = if let Some(selection) = &(*state).primary_selection {
        create_primary_offer(device, state, selection)
    } else {
        std::ptr::null_mut()
    };
    ffi::wl_resource_post_event(
        device,
        ffi::ZWP_PRIMARY_SELECTION_DEVICE_V1_SELECTION,
        offer,
    );
}

unsafe fn notify_primary_selection(state: *mut State) {
    let focus_client = if (*state).keyboard_focus.is_null() {
        std::ptr::null_mut()
    } else {
        ffi::wl_resource_get_client((*state).keyboard_focus)
    };
    for device in (*state).primary_devices.clone() {
        if device.is_null() {
            continue;
        }
        if !focus_client.is_null() && ffi::wl_resource_get_client(device) == focus_client {
            advertise_primary_selection(device, state);
        } else {
            ffi::wl_resource_post_event(
                device,
                ffi::ZWP_PRIMARY_SELECTION_DEVICE_V1_SELECTION,
                std::ptr::null_mut::<ffi::wl_resource>(),
            );
        }
    }
}

pub(crate) unsafe fn primary_selection_focus_changed(
    state: *mut State,
    old_focus: *mut ffi::wl_resource,
    new_focus: *mut ffi::wl_resource,
) {
    if state.is_null() {
        return;
    }
    let old_client = if old_focus.is_null() {
        std::ptr::null_mut()
    } else {
        ffi::wl_resource_get_client(old_focus)
    };
    let new_client = if new_focus.is_null() {
        std::ptr::null_mut()
    } else {
        ffi::wl_resource_get_client(new_focus)
    };
    for device in (*state).primary_devices.clone() {
        if device.is_null() {
            continue;
        }
        let client = ffi::wl_resource_get_client(device);
        if !old_client.is_null() && client == old_client {
            ffi::wl_resource_post_event(
                device,
                ffi::ZWP_PRIMARY_SELECTION_DEVICE_V1_SELECTION,
                std::ptr::null_mut::<ffi::wl_resource>(),
            );
        }
        if !new_client.is_null() && client == new_client {
            advertise_primary_selection(device, state);
        }
    }
}

unsafe extern "C" fn primary_offer_resource_destroy(offer: *mut ffi::wl_resource) {
    let rec = ffi::wl_resource_get_user_data(offer) as *mut crate::PrimaryOfferRec;
    if rec.is_null() {
        return;
    }
    if !(*rec).state.is_null() {
        (*(*rec).state).primary_offers.retain(|r| *r != offer);
    }
    drop(Box::from_raw(rec));
}

unsafe extern "C" fn pso_receive(
    _client: *mut ffi::wl_client,
    offer: *mut ffi::wl_resource,
    mime: *const std::os::raw::c_char,
    fd: i32,
) {
    let rec = ffi::wl_resource_get_user_data(offer) as *mut crate::PrimaryOfferRec;
    let source = if rec.is_null() {
        std::ptr::null_mut()
    } else {
        (*rec).source
    };
    if source.is_null() {
        if fd >= 0 {
            crate::libc_close(fd);
        }
        return;
    }
    ffi::wl_resource_post_event(source, ffi::ZWP_PRIMARY_SELECTION_SOURCE_V1_SEND, mime, fd);
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
    data: *mut c_void,
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
        data,
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
    let dev = ffi::wl_resource_create(client, &ffi::wp_cursor_shape_device_v1_interface, ver, id);
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
    let dev = ffi::wl_resource_create(client, &ffi::wp_cursor_shape_device_v1_interface, ver, id);
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
    client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    serial: u32,
    shape: u32,
) {
    let state = ffi::wl_resource_get_user_data(resource) as *mut State;
    if !state.is_null()
        && !(*state).pointer_focus.is_null()
        && ffi::wl_resource_get_client((*state).pointer_focus) == client
        && serial == (*state).last_pointer_enter_serial
    {
        (*state).cursor_shape = shape;
        (*state).cursor_surface = std::ptr::null_mut();
        (*state).cursor_hidden = false;
    }
}

// ----- text-input-unstable-v3 ---------------------------------------------

struct TextInputRec {
    state: *mut State,
    current_surface: *mut ffi::wl_resource,
    pending: ass_core::input::TextInputState,
    current: ass_core::input::TextInputState,
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
        current_surface,
        pending: ass_core::input::TextInputState::default(),
        current: ass_core::input::TextInputState::default(),
        commit_serial: 0,
    }));
    ffi::wl_resource_set_implementation(
        res,
        &TEXT_INPUT_IMPL as *const _ as *const c_void,
        rec as *mut c_void,
        Some(text_input_resource_destroy),
    );
    if !state.is_null() {
        (*state).text_inputs.push(res);
    }
    if !current_surface.is_null() {
        ffi::wl_resource_post_event(res, ffi::ZWP_TEXT_INPUT_V3_ENTER, current_surface);
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
    let rec = ffi::wl_resource_get_user_data(resource) as *mut TextInputRec;
    if rec.is_null() {
        return;
    }
    let state = (*rec).state;
    if !state.is_null() {
        (*state).text_inputs.retain(|r| *r != resource);
        if (*rec).current.enabled {
            (*state)
                .pending_text_input_states
                .push(ass_core::input::TextInputState::default());
        }
    }
    drop(Box::from_raw(rec));
}

/// Mark a text_input as enabled (it now wants IME input) and associate it with
/// the focused surface so enter/leave route correctly.
unsafe extern "C" fn text_input_enable(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut TextInputRec;
    if rec.is_null() || (*rec).current_surface.is_null() {
        return;
    }
    (*rec).pending = ass_core::input::TextInputState {
        enabled: true,
        ..Default::default()
    };
}

unsafe extern "C" fn text_input_disable(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut TextInputRec;
    if rec.is_null() || (*rec).current_surface.is_null() {
        return;
    }
    (*rec).pending = ass_core::input::TextInputState::default();
}

unsafe extern "C" fn text_input_set_surrounding_text(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    text: *const std::os::raw::c_char,
    cursor: i32,
    anchor: i32,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut TextInputRec;
    if rec.is_null() || (*rec).current_surface.is_null() || text.is_null() {
        return;
    }
    (*rec).pending.surrounding_text = Some(CStr::from_ptr(text).to_string_lossy().into_owned());
    (*rec).pending.cursor = cursor;
    (*rec).pending.anchor = anchor;
}

unsafe extern "C" fn text_input_set_text_change_cause(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    cause: u32,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut TextInputRec;
    if !rec.is_null() && !(*rec).current_surface.is_null() {
        (*rec).pending.change_cause = cause;
    }
}

unsafe extern "C" fn text_input_set_content_type(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    hint: u32,
    purpose: u32,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut TextInputRec;
    if !rec.is_null() && !(*rec).current_surface.is_null() {
        (*rec).pending.content_hint = hint;
        (*rec).pending.content_purpose = purpose;
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
    let rec = ffi::wl_resource_get_user_data(resource) as *mut TextInputRec;
    if !rec.is_null() && !(*rec).current_surface.is_null() {
        (*rec).pending.cursor_rect = Some((x, y, width, height));
    }
}

unsafe extern "C" fn text_input_commit(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut TextInputRec;
    if rec.is_null() || (*rec).current_surface.is_null() {
        return;
    }
    (*rec).commit_serial = (*rec).commit_serial.wrapping_add(1);
    (*rec).current = (*rec).pending.clone();
    queue_text_input_state(rec);
}

unsafe fn queue_text_input_state(rec: *mut TextInputRec) {
    let state = (*rec).state;
    if state.is_null() || (*rec).current_surface != (*state).keyboard_focus {
        return;
    }
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
    (*state).pending_text_input_states.push(value);
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
                (*state)
                    .pending_text_input_states
                    .push(ass_core::input::TextInputState::default());
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

pub(crate) unsafe fn forward_text_input_event(
    state: *mut State,
    event: &ass_core::input::TextInputEvent,
) {
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
            ass_core::input::TextInputEvent::Preedit {
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
            ass_core::input::TextInputEvent::Commit(text) => {
                let text = text.as_ref().and_then(|s| CString::new(s.as_str()).ok());
                ffi::wl_resource_post_event(
                    target,
                    ffi::ZWP_TEXT_INPUT_V3_COMMIT_STRING,
                    text.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
                );
            }
            ass_core::input::TextInputEvent::DeleteSurrounding {
                before_length,
                after_length,
            } => ffi::wl_resource_post_event(
                target,
                ffi::ZWP_TEXT_INPUT_V3_DELETE_SURROUNDING_TEXT,
                *before_length,
                *after_length,
            ),
            ass_core::input::TextInputEvent::Done => ffi::wl_resource_post_event(
                target,
                ffi::ZWP_TEXT_INPUT_V3_DONE,
                (*rec).commit_serial,
            ),
        }
    }
}

// ----- tablet-unstable-v2 ---------------------------------------------------

struct TabletSeatRec {
    state: *mut State,
}

struct TabletRec {
    state: *mut State,
}

pub(crate) struct TabletToolRec {
    state: *mut State,
    /// Physical tool id (from the backend) this resource proxies.
    pub(crate) tool: u64,
}

static TABLET_MANAGER_IMPL: ffi::zwp_tablet_manager_v2_interface_impl =
    ffi::zwp_tablet_manager_v2_interface_impl {
        get_tablet_seat: tablet_manager_get_tablet_seat,
        destroy: crate::res_destroy,
    };
static TABLET_SEAT_IMPL: ffi::zwp_tablet_seat_v2_interface_impl =
    ffi::zwp_tablet_seat_v2_interface_impl {
        destroy: crate::res_destroy,
    };
static TABLET_IMPL: ffi::zwp_tablet_v2_interface_impl = ffi::zwp_tablet_v2_interface_impl {
    destroy: crate::res_destroy,
};
static TABLET_TOOL_IMPL: ffi::zwp_tablet_tool_v2_interface_impl =
    ffi::zwp_tablet_tool_v2_interface_impl {
        set_cursor: tablet_tool_set_cursor,
        destroy: crate::res_destroy,
    };

pub(crate) unsafe extern "C" fn tablet_manager_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(
        client,
        &ffi::zwp_tablet_manager_v2_interface,
        version.min(1) as c_int,
        id,
    );
    if !res.is_null() {
        ffi::wl_resource_set_implementation(
            res,
            &TABLET_MANAGER_IMPL as *const _ as *const c_void,
            data,
            None,
        );
    }
}

unsafe extern "C" fn tablet_manager_get_tablet_seat(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    seat: *mut ffi::wl_resource,
) {
    if seat.is_null() || ffi::wl_resource_get_client(seat) != client {
        ffi::wl_resource_post_error(mgr, 0, c"seat belongs to another client".as_ptr());
        return;
    }
    let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
    let ver = ffi::wl_resource_get_version(mgr);
    let res = ffi::wl_resource_create(client, &ffi::zwp_tablet_seat_v2_interface, ver, id);
    if res.is_null() {
        return;
    }
    let rec = Box::into_raw(Box::new(TabletSeatRec { state }));
    ffi::wl_resource_set_implementation(
        res,
        &TABLET_SEAT_IMPL as *const _ as *const c_void,
        rec as *mut c_void,
        Some(tablet_seat_resource_destroy),
    );
    if !state.is_null() {
        (*state).tablet_seats.push(res);
        announce_tablets(state, res);
    }
}

unsafe extern "C" fn tablet_seat_resource_destroy(resource: *mut ffi::wl_resource) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut TabletSeatRec;
    if rec.is_null() {
        return;
    }
    let state = (*rec).state;
    if !state.is_null() {
        (*state).tablet_seats.retain(|r| *r != resource);
    }
    drop(Box::from_raw(rec));
}

unsafe extern "C" fn tablet_resource_destroy(resource: *mut ffi::wl_resource) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut TabletRec;
    if rec.is_null() {
        return;
    }
    let state = (*rec).state;
    if !state.is_null() {
        (*state).tablet_devices.retain(|r| *r != resource);
    }
    drop(Box::from_raw(rec));
}

unsafe extern "C" fn tablet_tool_resource_destroy(resource: *mut ffi::wl_resource) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut TabletToolRec;
    if rec.is_null() {
        return;
    }
    let state = (*rec).state;
    if !state.is_null() {
        (*state).tablet_tools.retain(|r| *r != resource);
    }
    drop(Box::from_raw(rec));
}

/// The compositor renders its own cursor, so a tool cursor surface is
/// accepted and ignored.
unsafe extern "C" fn tablet_tool_set_cursor(
    _client: *mut ffi::wl_client,
    _resource: *mut ffi::wl_resource,
    _serial: u32,
    _surface: *mut ffi::wl_resource,
    _hotspot_x: i32,
    _hotspot_y: i32,
) {
}

/// Announce the compositor's synthetic tablet (once one has been seen) and
/// every known tool to a newly bound tablet seat. The tablet goes first:
/// per the protocol a tool must always follow a tablet.
pub(crate) unsafe fn announce_tablets(state: *mut State, seat: *mut ffi::wl_resource) {
    if (*state).tablet_device_seen {
        announce_tablet(state, seat);
    }
    // Clone the list so the loop can re-borrow `state` freely.
    let tools = (*state).known_tools.clone();
    for (tool, info) in tools {
        announce_tool(state, seat, tool, &info);
    }
}

/// Create the seat's `zwp_tablet_v2` object for the compositor's single
/// synthetic tablet and post `tablet_added` + the name/id/done burst.
pub(crate) unsafe fn announce_tablet(state: *mut State, seat: *mut ffi::wl_resource) {
    let client = ffi::wl_resource_get_client(seat);
    let ver = ffi::wl_resource_get_version(seat);
    let res = ffi::wl_resource_create(client, &ffi::zwp_tablet_v2_interface, ver, 0);
    if res.is_null() {
        return;
    }
    let rec = Box::into_raw(Box::new(TabletRec { state }));
    ffi::wl_resource_set_implementation(
        res,
        &TABLET_IMPL as *const _ as *const c_void,
        rec as *mut c_void,
        Some(tablet_resource_destroy),
    );
    (*state).tablet_devices.push(res);
    ffi::wl_resource_post_event(seat, ffi::ZWP_TABLET_SEAT_V2_TABLET_ADDED, res);
    ffi::wl_resource_post_event(res, ffi::ZWP_TABLET_V2_NAME, c"ass tablet".as_ptr());
    // No real hardware: vid/pid are 0/0.
    ffi::wl_resource_post_event(res, ffi::ZWP_TABLET_V2_ID, 0u32, 0u32);
    ffi::wl_resource_post_event(res, ffi::ZWP_TABLET_V2_DONE);
}

/// Create a `zwp_tablet_tool_v2` resource on `seat`'s client for physical
/// tool `tool`. The caller posts `tool_added` and the describe burst.
pub(crate) unsafe fn tablet_tool_resource(
    state: *mut State,
    seat: *mut ffi::wl_resource,
    tool: u64,
) -> *mut ffi::wl_resource {
    let client = ffi::wl_resource_get_client(seat);
    let ver = ffi::wl_resource_get_version(seat);
    let res = ffi::wl_resource_create(client, &ffi::zwp_tablet_tool_v2_interface, ver, 0);
    if res.is_null() {
        return std::ptr::null_mut();
    }
    let rec = Box::into_raw(Box::new(TabletToolRec { state, tool }));
    ffi::wl_resource_set_implementation(
        res,
        &TABLET_TOOL_IMPL as *const _ as *const c_void,
        rec as *mut c_void,
        Some(tablet_tool_resource_destroy),
    );
    (*state).tablet_tools.push(res);
    res
}

/// Announce tool `tool` to `seat`: `tool_added` with a fresh tool resource,
/// then type/hardware serial/id/capabilities and `done`. The 64-bit ids are
/// split high word first, per the protocol.
pub(crate) unsafe fn announce_tool(
    state: *mut State,
    seat: *mut ffi::wl_resource,
    tool: u64,
    info: &ass_core::input::TabletToolInfo,
) {
    let res = tablet_tool_resource(state, seat, tool);
    if res.is_null() {
        return;
    }
    ffi::wl_resource_post_event(seat, ffi::ZWP_TABLET_SEAT_V2_TOOL_ADDED, res);
    ffi::wl_resource_post_event(res, ffi::ZWP_TABLET_TOOL_V2_TYPE, info.kind);
    ffi::wl_resource_post_event(
        res,
        ffi::ZWP_TABLET_TOOL_V2_HARDWARE_SERIAL,
        (info.serial >> 32) as u32,
        (info.serial & 0xffff_ffff) as u32,
    );
    ffi::wl_resource_post_event(
        res,
        ffi::ZWP_TABLET_TOOL_V2_HARDWARE_ID_WACOM,
        (info.hardware_id >> 32) as u32,
        (info.hardware_id & 0xffff_ffff) as u32,
    );
    // Capability bit N maps to protocol capability N (tilt=1..wheel=6).
    for capability in 1..=6u32 {
        if info.capabilities & (1 << capability) != 0 {
            ffi::wl_resource_post_event(res, ffi::ZWP_TABLET_TOOL_V2_CAPABILITY, capability);
        }
    }
    ffi::wl_resource_post_event(res, ffi::ZWP_TABLET_TOOL_V2_DONE);
}
