use super::*;

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
    unsafe {
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
}

unsafe extern "C" fn idle_inhibit_create_inhibitor(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
) {
    unsafe {
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
            let notification =
                ffi::wl_resource_get_user_data(*resource) as *mut IdleNotificationRec;
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
}

unsafe extern "C" fn idle_inhibitor_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
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
}

pub(crate) unsafe fn idle_inhibit_surface_destroyed(surface: *mut SurfaceRec) {
    unsafe {
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
    unsafe {
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
}

unsafe extern "C" fn idle_notifier_get(
    client: *mut ffi::wl_client,
    notifier: *mut ffi::wl_resource,
    id: u32,
    timeout: u32,
    seat: *mut ffi::wl_resource,
) {
    unsafe {
        idle_notifier_create(client, notifier, id, timeout, seat, false);
    }
}

unsafe extern "C" fn idle_notifier_get_input(
    client: *mut ffi::wl_client,
    notifier: *mut ffi::wl_resource,
    id: u32,
    timeout: u32,
    seat: *mut ffi::wl_resource,
) {
    unsafe {
        idle_notifier_create(client, notifier, id, timeout, seat, true);
    }
}

struct IdleNotificationRec {
    state: *mut State,
    seat: aegis_core::interaction_domain::SeatId,
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
    unsafe {
        let ver = ffi::wl_resource_get_version(notifier);
        let res =
            ffi::wl_resource_create(client, &ffi::ext_idle_notification_v1_interface, ver, id);
        if res.is_null() {
            return;
        }
        let state = ffi::wl_resource_get_user_data(notifier) as *mut State;
        let Some(_guard) =
            crate::ActiveSeatGuard::for_client_seat_resource(state, client, seat, true)
        else {
            ffi::wl_resource_destroy(res);
            return;
        };
        let record = Box::new(IdleNotificationRec {
            state,
            seat: (*state).active_seat,
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
}

unsafe extern "C" fn idle_notification_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
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
}

unsafe fn idle_inhibitor_is_active(record: *mut IdleInhibitorRec) -> bool {
    unsafe {
        if record.is_null()
            || !(*record).honored
            || (*record).state.is_null()
            || (*record).surface.is_null()
            || (*(*record).state).session_lock_phase.is_active()
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
            && (*(*record).state).authority.seat_controls_window(
                aegis_core::interaction_domain::HUMAN_SEAT,
                (*root).window.id,
            )
    }
}

pub(crate) unsafe fn update_idle_notifications(state: *mut State) {
    unsafe {
        if state.is_null() {
            return;
        }
        // A scoped IPC surfaceless inhibitor counts like an active
        // per-surface one, gated on the session being unlocked the same way.
        let inhibited = (!(*state).session_lock_phase.is_active() && (*state).ipc_idle_inhibit)
            || (*state).idle_inhibitors.iter().any(|resource| {
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
                ffi::wl_resource_post_event(
                    (*record).resource,
                    ffi::EXT_IDLE_NOTIFICATION_V1_IDLED,
                );
            } else if !should_idle && (*record).idle {
                (*record).idle = false;
                ffi::wl_resource_post_event(
                    (*record).resource,
                    ffi::EXT_IDLE_NOTIFICATION_V1_RESUMED,
                );
            }
        }
    }
}

pub(crate) unsafe fn idle_user_activity(state: *mut State) {
    unsafe {
        if state.is_null() {
            return;
        }
        let now = std::time::Instant::now();
        for resource in &(*state).idle_notifications {
            if resource.is_null() {
                continue;
            }
            let record = ffi::wl_resource_get_user_data(*resource) as *mut IdleNotificationRec;
            if !record.is_null() && (*record).seat == aegis_core::interaction_domain::HUMAN_SEAT {
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
}
