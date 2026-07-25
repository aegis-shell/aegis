use super::*;

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
    unsafe {
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
}

unsafe extern "C" fn tablet_manager_get_tablet_seat(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    seat: *mut ffi::wl_resource,
) {
    unsafe {
        if seat.is_null() || ffi::wl_resource_get_client(seat) != client {
            ffi::wl_resource_post_error(mgr, 0, c"seat belongs to another client".as_ptr());
            return;
        }
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
        (*state).track_routed_seat_resource(res, advertised_seat, seat_id);
        (*state).tablet_seats.push(res);
        announce_tablets(state, res);
    }
}

unsafe extern "C" fn tablet_seat_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut TabletSeatRec;
        if rec.is_null() {
            return;
        }
        let state = (*rec).state;
        if let Some(_guard) = crate::ActiveSeatGuard::for_resource(state, resource, false) {
            (*state).tablet_seats.retain(|r| *r != resource);
            (*state).untrack_seat_resource(resource);
        }
        drop(Box::from_raw(rec));
    }
}

unsafe extern "C" fn tablet_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut TabletRec;
        if rec.is_null() {
            return;
        }
        let state = (*rec).state;
        if let Some(_guard) = crate::ActiveSeatGuard::for_resource(state, resource, false) {
            (*state).tablet_devices.retain(|r| *r != resource);
            (*state).untrack_seat_resource(resource);
        }
        drop(Box::from_raw(rec));
    }
}

unsafe extern "C" fn tablet_tool_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut TabletToolRec;
        if rec.is_null() {
            return;
        }
        let state = (*rec).state;
        if let Some(_guard) = crate::ActiveSeatGuard::for_resource(state, resource, false) {
            (*state).tablet_tools.retain(|r| *r != resource);
            (*state).untrack_seat_resource(resource);
        }
        drop(Box::from_raw(rec));
    }
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
    unsafe {
        if (*state).tablet_device_seen {
            announce_tablet(state, seat);
        }
        // Clone the list so the loop can re-borrow `state` freely.
        let tools = (*state).known_tools.clone();
        for (tool, info) in tools {
            announce_tool(state, seat, tool, &info);
        }
    }
}

/// Create the seat's `zwp_tablet_v2` object for the compositor's single
/// synthetic tablet and post `tablet_added` + the name/id/done burst.
pub(crate) unsafe fn announce_tablet(state: *mut State, seat: *mut ffi::wl_resource) {
    unsafe {
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
        (*state).track_seat_resource(res, (*state).active_seat);
        (*state).tablet_devices.push(res);
        ffi::wl_resource_post_event(seat, ffi::ZWP_TABLET_SEAT_V2_TABLET_ADDED, res);
        ffi::wl_resource_post_event(res, ffi::ZWP_TABLET_V2_NAME, c"ass tablet".as_ptr());
        // No real hardware: vid/pid are 0/0.
        ffi::wl_resource_post_event(res, ffi::ZWP_TABLET_V2_ID, 0u32, 0u32);
        ffi::wl_resource_post_event(res, ffi::ZWP_TABLET_V2_DONE);
    }
}

/// Create a `zwp_tablet_tool_v2` resource on `seat`'s client for physical
/// tool `tool`. The caller posts `tool_added` and the describe burst.
pub(crate) unsafe fn tablet_tool_resource(
    state: *mut State,
    seat: *mut ffi::wl_resource,
    tool: u64,
) -> *mut ffi::wl_resource {
    unsafe {
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
        (*state).track_seat_resource(res, (*state).active_seat);
        (*state).tablet_tools.push(res);
        res
    }
}

/// Announce tool `tool` to `seat`: `tool_added` with a fresh tool resource,
/// then type/hardware serial/id/capabilities and `done`. The 64-bit ids are
/// split high word first, per the protocol.
pub(crate) unsafe fn announce_tool(
    state: *mut State,
    seat: *mut ffi::wl_resource,
    tool: u64,
    info: &aegis_core::input::TabletToolInfo,
) {
    unsafe {
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
}
