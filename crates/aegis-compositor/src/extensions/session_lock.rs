use super::*;

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
    pending_configures: Vec<(u32, aegis_model::Size)>,
    acked_configure: Option<(u32, aegis_model::Size)>,
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

unsafe fn restore_unlocked_state(state: *mut State) {
    unsafe {
        if state.is_null() {
            return;
        }
        (*state).session_lock_phase.unlock();
        (*state).pending_lock_focus = (*state).pre_lock_keyboard_focus;
        (*state).pre_lock_keyboard_focus = std::ptr::null_mut();
        (*state).lock_focus_dirty = true;
    }
}

pub(crate) unsafe extern "C" fn session_lock_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
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
}

unsafe extern "C" fn session_lock_manager_lock(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
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
        if state.is_null() || !(*state).session_lock.is_null() {
            (*record_ptr).finished_sent = true;
            ffi::wl_resource_post_event(res, ffi::EXT_SESSION_LOCK_V1_FINISHED);
            return;
        }
        if (*state).session_lock_phase.is_confirmed() {
            // A confirmed lock outlives its client. Allow a new lock object to
            // assume responsibility without passing through Unlocked or
            // exposing normal content between the two clients.
            (*state).session_lock = record_ptr as *mut c_void;
            (*record_ptr).locked_sent = true;
            ffi::wl_resource_post_event(res, ffi::EXT_SESSION_LOCK_V1_LOCKED);
            log::info!("[server] replacement client assumed the fail-closed session lock");
            return;
        }
        if (*state).session_lock_phase.is_active() {
            // A securing phase without its object should be transient only
            // inside resource destruction. Refuse re-entry rather than
            // assigning two owners to one protocol transaction.
            (*record_ptr).finished_sent = true;
            ffi::wl_resource_post_event(res, ffi::EXT_SESSION_LOCK_V1_FINISHED);
            return;
        }
        (*state).session_lock = record_ptr as *mut c_void;
        (*state).session_lock_phase.begin(std::time::Instant::now());
        log::info!("[server] session lock requested; normal content hidden");
        (*state).pre_lock_keyboard_focus = (*state).keyboard_focus;
        (*state).pending_lock_focus = std::ptr::null_mut();
        (*state).lock_focus_dirty = true;
        (*state).interactive = None;
        (*state).compositor_pointer_grab = false;
        (*state).implicit_grab_active = false;
        if (*state).drag.is_some() {
            crate::cancel_drag(state, true);
        }
        if (*state).output_infos.is_empty() {
            // With no active outputs there is no sensitive scanout to replace.
            // The ext-session-lock presentation condition is vacuously met.
            (*state).session_lock_phase.request_secure_frame();
            session_lock_presented(state);
        }
    }
}

unsafe extern "C" fn session_lock_get_surface(
    client: *mut ffi::wl_client,
    lock: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
    output: *mut ffi::wl_resource,
) {
    unsafe {
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
        if (*surface_rec).mapped
            || (*surface_rec).pending_buffer_set
            || (*surface_rec).generation != 0
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
        let res =
            ffi::wl_resource_create(client, &ffi::ext_session_lock_surface_v1_interface, ver, id);
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
}

unsafe extern "C" fn session_lock_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let record = ffi::wl_resource_get_user_data(resource) as *mut SessionLockRec;
        if !record.is_null() && (*record).locked_sent {
            ffi::wl_resource_post_error(resource, 0, c"locked session requires unlock".as_ptr());
            return;
        }
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn session_lock_unlock(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let record = ffi::wl_resource_get_user_data(resource) as *mut SessionLockRec;
        if record.is_null() || !(*record).locked_sent {
            ffi::wl_resource_post_error(resource, 1, c"lock was never acknowledged".as_ptr());
            return;
        }
        let state = (*record).state;
        if !state.is_null() && (*state).session_lock == record as *mut c_void {
            (*state).session_lock = std::ptr::null_mut();
            restore_unlocked_state(state);
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
}

unsafe extern "C" fn session_lock_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let record = ffi::wl_resource_get_user_data(resource) as *mut SessionLockRec;
        if record.is_null() {
            return;
        }
        let state = (*record).state;
        if !state.is_null() && (*state).session_lock == record as *mut c_void {
            (*state).session_lock = std::ptr::null_mut();
            if !(*record).locked_sent {
                restore_unlocked_state(state);
                log::warn!("[server] session lock client exited before lock confirmation");
            } else {
                // Fail closed, retire the vanished client's pixels with an
                // opaque compositor frame, and permit a replacement lock
                // object to authenticate and unlock later.
                (*state).session_lock_phase.request_secure_frame();
                (*state).pending_lock_focus = std::ptr::null_mut();
                (*state).lock_focus_dirty = true;
                log::error!(
                    "[server] session lock client exited while locked; retaining fail-closed state for replacement"
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
}

unsafe extern "C" fn session_lock_surface_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn session_lock_surface_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
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
            if !state.is_null() && (*state).session_lock_phase.is_active() {
                // A destroyed lock surface immediately reveals the compositor's
                // solid fallback on its output. Confirm that replacement frame
                // before acknowledging an in-flight lock request.
                (*state).session_lock_phase.request_secure_frame();
            }
            let lock_surfaces = &mut (*(*record).lock).surfaces;
            if let Some(pos) = lock_surfaces.iter().position(|p| *p == record) {
                lock_surfaces.remove(pos);
            }
        }
        drop(Box::from_raw(record));
    }
}

unsafe extern "C" fn session_lock_surface_ack(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    serial: u32,
) {
    unsafe {
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
}

pub(crate) unsafe fn session_lock_surface_committed(surface: *mut SurfaceRec) {
    unsafe {
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
            (*state).session_lock_phase.request_secure_frame();
            (*state).pending_lock_focus = (*surface).resource;
            (*state).lock_focus_dirty = true;
        }
    }
}

pub(crate) unsafe fn session_lock_presented(state: *mut State) {
    unsafe {
        if state.is_null() || !(*state).session_lock_phase.frame_pending() {
            return;
        }
        let lock = (*state).session_lock as *mut SessionLockRec;
        let first_secure_frame = (*state).session_lock_phase.secure_frame_presented();
        if !first_secure_frame || lock.is_null() || (*lock).locked_sent || (*lock).finished_sent {
            return;
        }
        (*lock).locked_sent = true;
        ffi::wl_resource_post_event((*lock).resource, ffi::EXT_SESSION_LOCK_V1_LOCKED);
        log::info!("[server] secure frame confirmed on all outputs; session locked");
    }
}

/// Detach the role record before a `wl_surface` allocation is reclaimed.
/// The protocol role object may outlive its surface resource, so keeping its
/// raw back-pointer would otherwise turn a later destroy into a use-after-free.
pub(crate) unsafe fn session_lock_surface_destroyed(surface: *mut SurfaceRec) {
    unsafe {
        if surface.is_null() || (*surface).session_lock_surface.is_null() {
            return;
        }
        let record = (*surface).session_lock_surface as *mut LockSurfaceRec;
        (*record).surface = std::ptr::null_mut();
        (*surface).session_lock_surface = std::ptr::null_mut();
        if !(*record).lock.is_null() {
            let state = (*(*record).lock).state;
            if !state.is_null() && (*state).session_lock_phase.is_active() {
                (*state).session_lock_phase.request_secure_frame();
            }
        }
    }
}

pub(crate) unsafe fn is_active_session_lock_surface(
    state: *mut State,
    surface: *mut SurfaceRec,
) -> bool {
    unsafe {
        if state.is_null()
            || surface.is_null()
            || !(*state).session_lock_phase.is_active()
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
}

/// Preferred fractional scale (in 120ths, the wire unit) for a lock surface,
/// resolved from the connector it was created for. Returns `None` for
/// non-lock or already-destroyed surfaces so callers keep their fallback.
pub(crate) unsafe fn session_lock_surface_preferred_scale_120(
    surface: *mut SurfaceRec,
) -> Option<u32> {
    unsafe {
        if surface.is_null() || (*surface).session_lock_surface.is_null() {
            return None;
        }
        let record = (*surface).session_lock_surface as *mut LockSurfaceRec;
        if (*record).lock.is_null() || (*(*record).lock).state.is_null() {
            return None;
        }
        let state = (*(*record).lock).state;
        let output = (*state)
            .output_infos
            .iter()
            .find(|output| output.connector == (*record).connector)?;
        Some((output.geometry.scale.0 * 120.0).round().max(1.0) as u32)
    }
}

pub(crate) unsafe fn is_active_session_lock_client_resource(
    state: *mut State,
    resource: *mut ffi::wl_resource,
) -> bool {
    unsafe {
        if state.is_null() || resource.is_null() || (*state).session_lock.is_null() {
            return false;
        }
        let lock = (*state).session_lock as *mut SessionLockRec;
        ffi::wl_resource_get_client(resource) == ffi::wl_resource_get_client((*lock).resource)
    }
}

/// Reposition and reconfigure lock surfaces after an output mode/layout
/// change. Removed-output role objects remain valid but are not rendered;
/// clients normally destroy them after the `wl_output` global disappears.
pub(crate) unsafe fn session_lock_outputs_changed(state: *mut State) {
    unsafe {
        if state.is_null() || !(*state).session_lock_phase.is_active() {
            return;
        }
        (*state).session_lock_phase.request_secure_frame();
        let lock = (*state).session_lock as *mut SessionLockRec;
        if (*state).output_infos.is_empty() {
            // Removing the last output leaves no scanout that could expose
            // normal content, so an in-flight lock can be acknowledged
            // without waiting for an unavailable presentation target.
            session_lock_presented(state);
        }
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
}
