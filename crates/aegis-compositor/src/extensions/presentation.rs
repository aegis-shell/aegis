use super::*;

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
    unsafe {
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
}

unsafe extern "C" fn presentation_feedback(
    client: *mut ffi::wl_client,
    _presentation: *mut ffi::wl_resource,
    _surface: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
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
}
