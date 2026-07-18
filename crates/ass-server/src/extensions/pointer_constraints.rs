use super::*;

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
    unsafe {
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
    unsafe {
        create_pointer_constraint(client, pc, id, surface, pointer, region, lifetime, true);
    }
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
    unsafe {
        create_pointer_constraint(client, pc, id, surface, pointer, region, lifetime, false);
    }
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
    unsafe {
        let state = ffi::wl_resource_get_user_data(manager) as *mut State;
        let origin = (*state).seat_origin_for_resource(pointer);
        let Some(_guard) = crate::ActiveSeatGuard::for_resource(state, pointer, true) else {
            return;
        };
        let seat = (*state).active_seat;
        if lifetime != 1 && lifetime != 2 {
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
        (*state).track_routed_seat_resource(res, origin.unwrap_or(seat), seat);
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
}

unsafe fn copy_region(region: *mut ffi::wl_resource) -> Option<Vec<ass_core::Rect>> {
    unsafe {
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
}

unsafe extern "C" fn pointer_constraint_set_region(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    region: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut PointerConstraintRec;
        if !rec.is_null() {
            (*rec).region = copy_region(region);
        }
    }
}

unsafe extern "C" fn locked_pointer_set_cursor_hint(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut PointerConstraintRec;
        if !rec.is_null() && (*rec).locked {
            (*rec).cursor_hint = Some((x as f32 / 256.0, y as f32 / 256.0));
        }
    }
}

unsafe extern "C" fn pointer_constraint_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn pointer_constraint_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut PointerConstraintRec;
        if rec.is_null() {
            return;
        }
        if let Some(_guard) = crate::ActiveSeatGuard::for_resource((*rec).state, resource, false) {
            (*(*rec).state)
                .pointer_constraints
                .retain(|r| *r != resource);
            (*(*rec).state).untrack_seat_resource(resource);
        }
        drop(Box::from_raw(rec));
    }
}

pub(crate) unsafe fn pointer_constraint_focus_changed(
    state: *mut State,
    old_focus: *mut ffi::wl_resource,
    new_focus: *mut ffi::wl_resource,
) {
    unsafe {
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
                if (*rec).locked
                    && let Some((x, y)) = (*rec).cursor_hint
                {
                    let surface = ffi::wl_resource_get_user_data(old_focus) as *mut SurfaceRec;
                    if !surface.is_null() {
                        // The hint is surface-local; restore it relative to
                        // the buffer's draw origin (window-geometry insets).
                        let origin = crate::surface_draw_origin(&*surface);
                        (*state).pointer_x = origin.x as f32 + x;
                        (*state).pointer_y = origin.y as f32 + y;
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
}

pub(crate) unsafe fn constrain_pointer_motion(state: *mut State, x: f32, y: f32) -> (f32, f32) {
    unsafe {
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
}
