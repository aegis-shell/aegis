use crate::*;

// ----- xdg_wm_base --------------------------------------------------------

static XDG_WM_BASE_IMPL: ffi::xdg_wm_base_interface_impl = ffi::xdg_wm_base_interface_impl {
    destroy: res_destroy,
    create_positioner: xdg_wm_base_create_positioner,
    get_xdg_surface: xdg_wm_base_get_xdg_surface,
    pong: xdg_noop_serial,
};

pub(crate) unsafe extern "C" fn xdg_wm_base_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let res =
            ffi::wl_resource_create(client, &ffi::xdg_wm_base_interface, version as c_int, id);
        if res.is_null() {
            return;
        }
        // The resource data is the server State, so get_xdg_surface can reach the
        // display for serials.
        ffi::wl_resource_set_implementation(
            res,
            &XDG_WM_BASE_IMPL as *const _ as *const c_void,
            data,
            None,
        );
    }
}

static POSITIONER_IMPL: ffi::xdg_positioner_interface_impl = ffi::xdg_positioner_interface_impl {
    destroy: res_destroy,
    set_size: positioner_set_size,
    set_anchor_rect: positioner_set_anchor_rect,
    set_anchor: positioner_set_anchor,
    set_gravity: positioner_set_gravity,
    set_constraint_adjustment: positioner_set_constraint_adjustment,
    set_offset: positioner_set_offset,
};

static POPUP_IMPL: ffi::xdg_popup_interface_impl = ffi::xdg_popup_interface_impl {
    destroy: popup_destroy,
    grab: popup_grab,
};

unsafe extern "C" fn xdg_wm_base_create_positioner(
    client: *mut ffi::wl_client,
    wm_base: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let ver = ffi::wl_resource_get_version(wm_base);
        let pos = ffi::wl_resource_create(client, &ffi::xdg_positioner_interface, ver, id);
        if pos.is_null() {
            return;
        }
        let st = Box::into_raw(Box::new(PositionerState::default()));
        ffi::wl_resource_set_implementation(
            pos,
            &POSITIONER_IMPL as *const _ as *const c_void,
            st as *mut c_void,
            Some(positioner_resource_destroy),
        );
    }
}

unsafe extern "C" fn positioner_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let st = ffi::wl_resource_get_user_data(resource) as *mut PositionerState;
        if !st.is_null() {
            drop(Box::from_raw(st));
        }
    }
}

unsafe extern "C" fn positioner_set_size(
    _c: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    w: i32,
    h: i32,
) {
    unsafe {
        let st = ffi::wl_resource_get_user_data(resource) as *mut PositionerState;
        if !st.is_null() && w > 0 && h > 0 {
            (*st).size = Some(aegis_core::Size { w, h });
        }
    }
}

unsafe extern "C" fn positioner_set_anchor_rect(
    _c: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    unsafe {
        let st = ffi::wl_resource_get_user_data(resource) as *mut PositionerState;
        if !st.is_null() {
            (*st).anchor_rect = Some(aegis_core::Rect::new(x, y, w, h));
        }
    }
}

unsafe extern "C" fn positioner_set_offset(
    _c: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
) {
    unsafe {
        let st = ffi::wl_resource_get_user_data(resource) as *mut PositionerState;
        if !st.is_null() {
            (*st).offset = aegis_core::Point { x, y };
        }
    }
}

unsafe extern "C" fn positioner_set_anchor(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    anchor: u32,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(resource) as *mut PositionerState;
        if !state.is_null() && anchor <= 8 {
            (*state).anchor = anchor;
        }
    }
}

unsafe extern "C" fn positioner_set_gravity(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    gravity: u32,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(resource) as *mut PositionerState;
        if !state.is_null() && gravity <= 8 {
            (*state).gravity = gravity;
        }
    }
}

unsafe extern "C" fn positioner_set_constraint_adjustment(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    adjustment: u32,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(resource) as *mut PositionerState;
        if !state.is_null() {
            (*state).constraint_adjustment = adjustment & 0x3f;
        }
    }
}

unsafe extern "C" fn xdg_wm_base_get_xdg_surface(
    client: *mut ffi::wl_client,
    wm_base: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(wm_base) as *mut State;
        let rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
        if rec.is_null() || surface_has_role(&*rec) {
            ffi::wl_resource_post_error(wm_base, 0, c"wl_surface already has a role".as_ptr());
            return;
        }
        let ver = ffi::wl_resource_get_version(wm_base);
        let xdg_surface = ffi::wl_resource_create(client, &ffi::xdg_surface_interface, ver, id);
        if xdg_surface.is_null() {
            return;
        }
        (*rec).xdg_surface = xdg_surface;
        (*rec).display = (*state).display;
        ffi::wl_resource_set_implementation(
            xdg_surface,
            &XDG_SURFACE_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            None,
        );
    }
}

// ----- xdg_surface --------------------------------------------------------

static XDG_SURFACE_IMPL: ffi::xdg_surface_interface_impl = ffi::xdg_surface_interface_impl {
    destroy: xdg_surface_destroy,
    get_toplevel: xdg_surface_get_toplevel,
    get_popup: xdg_surface_get_popup,
    set_window_geometry: xdg_surface_set_window_geometry,
    ack_configure: xdg_surface_ack_configure,
};

unsafe extern "C" fn xdg_surface_ack_configure(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    serial: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        let Some(index) = (*rec)
            .pending_xdg_configures
            .iter()
            .position(|candidate| *candidate == serial)
        else {
            ffi::wl_resource_post_error(
                resource,
                4,
                c"xdg_surface.ack_configure used an unknown serial".as_ptr(),
            );
            return;
        };
        (*rec).pending_xdg_configures.drain(..=index);
        (*rec).xdg_configure_acked = true;
    }
}

unsafe extern "C" fn xdg_surface_set_window_geometry(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if rec.is_null() || width <= 0 || height <= 0 {
            return;
        }
        (*rec).pending_window_geometry = Some(aegis_core::Rect::new(x, y, width, height));
    }
}

unsafe extern "C" fn xdg_surface_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if !rec.is_null() {
            if !(*rec).xdg_toplevel.is_null() || !(*rec).xdg_popup.is_null() {
                ffi::wl_resource_post_error(
                    resource,
                    6,
                    c"xdg_surface role object must be destroyed first".as_ptr(),
                );
                return;
            }
            (*rec).xdg_surface = std::ptr::null_mut();
            (*rec).xdg_configured = false;
            (*rec).xdg_configure_acked = false;
            (*rec).pending_xdg_configures.clear();
        }
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn xdg_surface_get_toplevel(
    client: *mut ffi::wl_client,
    xdg_surface: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(xdg_surface) as *mut SurfaceRec;
        if rec.is_null() || !(*rec).xdg_toplevel.is_null() || !(*rec).xdg_popup.is_null() {
            ffi::wl_resource_post_error(
                xdg_surface,
                2,
                c"xdg_surface already has a role object".as_ptr(),
            );
            return;
        }
        let ver = ffi::wl_resource_get_version(xdg_surface);
        let toplevel = ffi::wl_resource_create(client, &ffi::xdg_toplevel_interface, ver, id);
        if toplevel.is_null() {
            return;
        }
        (*rec).xdg_toplevel = toplevel;
        // Initialize the window metadata so subsequent state requests (set_title,
        // set_min_size, ...) have somewhere to write.
        // Allocate a durable window id (ADR-0032). The surface address is no
        // longer the window's identity; the id is monotonic and never reused.
        let window_id = if !(*rec).state.is_null() {
            (*(*rec).state).alloc_window_id()
        } else {
            aegis_core::window::WindowId(0)
        };
        (*rec).window = aegis_core::window::Window::new(window_id);
        if !(*rec).state.is_null()
            && let Err(error) = (*(*rec).state).register_window((*rec).client_id, window_id)
        {
            // A window without an interaction group cannot receive input. Keep
            // the surface alive for protocol correctness, but fail closed and
            // make the model failure visible in diagnostics.
            log::error!(
                "[realm] failed to register window {} for client {}: {error}",
                window_id.0,
                (*rec).client_id.0
            );
        }
        ffi::wl_resource_set_implementation(
            toplevel,
            &XDG_TOPLEVEL_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            None,
        );
    }
}

unsafe extern "C" fn xdg_surface_get_popup(
    client: *mut ffi::wl_client,
    xdg_surface: *mut ffi::wl_resource,
    id: u32,
    parent: *mut ffi::wl_resource,
    positioner: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(xdg_surface) as *mut SurfaceRec;
        if rec.is_null() || !(*rec).xdg_toplevel.is_null() || !(*rec).xdg_popup.is_null() {
            ffi::wl_resource_post_error(
                xdg_surface,
                2,
                c"xdg_surface already has a role object".as_ptr(),
            );
            return;
        }
        if positioner.is_null() {
            ffi::wl_resource_post_error(xdg_surface, 5, c"popup requires a positioner".as_ptr());
            return;
        }
        let ver = ffi::wl_resource_get_version(xdg_surface);
        let pop = ffi::wl_resource_create(client, &ffi::xdg_popup_interface, ver, id);
        if pop.is_null() {
            return;
        }
        // Compute the popup position from the positioner + parent.
        let pos_state = ffi::wl_resource_get_user_data(positioner) as *mut PositionerState;
        let parent_rec = if parent.is_null() {
            std::ptr::null_mut::<SurfaceRec>()
        } else {
            ffi::wl_resource_get_user_data(parent) as *mut SurfaceRec
        };
        let (px, py) = if parent_rec.is_null() {
            (0, 0)
        } else {
            // Positioner coordinates are parent-surface-local: anchor at the
            // parent's buffer draw origin, not its window-rect origin.
            let origin = surface_draw_origin(&*parent_rec);
            (origin.x, origin.y)
        };
        let anchor_rect = if !pos_state.is_null() {
            (*pos_state)
                .anchor_rect
                .unwrap_or_else(|| aegis_core::Rect::new(0, 0, 1, 1))
        } else {
            aegis_core::Rect::new(0, 0, 1, 1)
        };
        let anchor = if pos_state.is_null() {
            0
        } else {
            (*pos_state).anchor
        };
        let gravity = if pos_state.is_null() {
            0
        } else {
            (*pos_state).gravity
        };
        let offset = if pos_state.is_null() {
            aegis_core::Point::default()
        } else {
            (*pos_state).offset
        };
        let mut popup_size = if !pos_state.is_null() {
            (*pos_state).size.unwrap_or(aegis_core::Size { w: 0, h: 0 })
        } else {
            aegis_core::Size { w: 0, h: 0 }
        };
        let anchor_x = match anchor {
            3 | 5 | 6 => anchor_rect.origin.x,
            4 | 7 | 8 => anchor_rect.origin.x + anchor_rect.size.w,
            _ => anchor_rect.origin.x + anchor_rect.size.w / 2,
        };
        let anchor_y = match anchor {
            1 | 5 | 7 => anchor_rect.origin.y,
            2 | 6 | 8 => anchor_rect.origin.y + anchor_rect.size.h,
            _ => anchor_rect.origin.y + anchor_rect.size.h / 2,
        };
        let gravity_x = match gravity {
            3 | 5 | 6 => -popup_size.w,
            4 | 7 | 8 => 0,
            _ => -popup_size.w / 2,
        };
        let gravity_y = match gravity {
            1 | 5 | 7 => -popup_size.h,
            2 | 6 | 8 => 0,
            _ => -popup_size.h / 2,
        };
        let mut local_x = anchor_x + gravity_x + offset.x;
        let mut local_y = anchor_y + gravity_y + offset.y;
        let adjustment = if pos_state.is_null() {
            0
        } else {
            (*pos_state).constraint_adjustment
        };
        if !(*rec).state.is_null() {
            let bounds = (*(*rec).state).output_geometry.logical_rect();
            let min_x = bounds.origin.x - px;
            let min_y = bounds.origin.y - py;
            let max_x = bounds.origin.x + bounds.size.w - px;
            let max_y = bounds.origin.y + bounds.size.h - py;
            if adjustment & (1 | 4) != 0 {
                local_x = local_x.clamp(min_x, (max_x - popup_size.w).max(min_x));
            } else if adjustment & 16 != 0 {
                popup_size.w = popup_size.w.min((max_x - local_x).max(1));
            }
            if adjustment & (2 | 8) != 0 {
                local_y = local_y.clamp(min_y, (max_y - popup_size.h).max(min_y));
            } else if adjustment & 32 != 0 {
                popup_size.h = popup_size.h.min((max_y - local_y).max(1));
            }
        }
        let popup_pos = aegis_core::Point {
            x: px + local_x,
            y: py + local_y,
        };
        (*rec).position = popup_pos;
        (*rec).xdg_toplevel = std::ptr::null_mut(); // popups are not toplevels
        (*rec).xdg_popup = pop;
        (*rec).popup_parent = parent_rec;
        ffi::wl_resource_set_implementation(
            pop,
            &POPUP_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            None,
        );
        // Send the initial configure so the client sizes and maps its buffer.
        ffi::wl_resource_post_event(
            pop,
            ffi::XDG_POPUP_CONFIGURE,
            local_x,
            local_y,
            popup_size.w,
            popup_size.h,
        );
        // The xdg_surface configure serial must follow per xdg-shell.
        if !(*rec).xdg_surface.is_null() {
            send_xdg_surface_configure(rec);
            (*rec).xdg_configured = true;
        }
    }
}

unsafe extern "C" fn popup_destroy(_client: *mut ffi::wl_client, resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if !rec.is_null() {
            reset_xdg_configure_state_after_unmap(&mut *rec);
            (*rec).xdg_popup = std::ptr::null_mut();
            (*rec).popup_parent = std::ptr::null_mut();
            (*rec).popup_grabbed = false;
            (*rec).mapped = false;
        }
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn popup_grab(
    _client: *mut ffi::wl_client,
    popup: *mut ffi::wl_resource,
    _seat: *mut ffi::wl_resource,
    _serial: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(popup) as *mut SurfaceRec;
        if !rec.is_null() {
            (*rec).popup_grabbed = true;
        }
    }
}

// ----- xdg_toplevel -------------------------------------------------------

static XDG_TOPLEVEL_IMPL: ffi::xdg_toplevel_interface_impl = ffi::xdg_toplevel_interface_impl {
    destroy: xdg_toplevel_destroy,
    set_parent: toplevel_set_parent,
    set_title: toplevel_set_title,
    set_app_id: toplevel_set_app_id,
    show_window_menu: xdg_noop_menu,
    move_: toplevel_move,
    resize: toplevel_resize,
    set_max_size: toplevel_set_max_size,
    set_min_size: toplevel_set_min_size,
    set_maximized: toplevel_set_maximized,
    unset_maximized: toplevel_unset_maximized,
    set_fullscreen: toplevel_set_fullscreen,
    unset_fullscreen: toplevel_unset_fullscreen,
    set_minimized: toplevel_set_minimized,
};

unsafe extern "C" fn xdg_toplevel_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if !rec.is_null() {
            if !(*rec).xdg_decoration.is_null() {
                ffi::wl_resource_post_error(
                    (*rec).xdg_decoration,
                    ffi::ZXDG_TOPLEVEL_DECORATION_V1_ERROR_ORPHANED,
                    c"xdg_toplevel destroyed before its decoration object".as_ptr(),
                );
                return;
            }
            if !(*rec).state.is_null() {
                (*(*rec).state).unregister_window((*rec).window.id);
            }
            reset_xdg_configure_state_after_unmap(&mut *rec);
            (*rec).xdg_toplevel = std::ptr::null_mut();
            (*rec).mapped = false;
        }
        ffi::wl_resource_destroy(resource);
    }
}

// ----- xdg_toplevel metadata + state requests ----------------------------
//
// M3 partial: real handlers for the metadata and state requests. Interactive
// move and resize (`move_`, `resize`) remain no-ops pending the pointer-state
// machine that drives them; `show_window_menu` and `set_minimized` are
// accepted but inert until the shell grows window-list and menu chrome. The
// state-changing handlers (maximize, fullscreen) emit a fresh
// `xdg_toplevel.configure` with the appropriate states array so clients
// reconfigure themselves.

unsafe fn cstr_to_string(p: *const std::os::raw::c_char) -> Option<String> {
    unsafe {
        if p.is_null() {
            return None;
        }
        Some(CStr::from_ptr(p).to_string_lossy().into_owned())
    }
}

unsafe extern "C" fn toplevel_set_title(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    title: *const std::os::raw::c_char,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if !rec.is_null() {
            (*rec).window.title = cstr_to_string(title);
            if !(*rec).state.is_null() {
                extensions::foreign_toplevel_updated(rec, (*rec).state);
            }
        }
    }
}

unsafe extern "C" fn toplevel_set_app_id(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    app_id: *const std::os::raw::c_char,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if !rec.is_null() {
            let id_opt = cstr_to_string(app_id);
            (*rec).window.app_id = id_opt.clone();
            if !(*rec).state.is_null() {
                if let Some(id_str) = id_opt {
                    let st = &mut *(*rec).state;
                    let rule = st
                        .window_rules
                        .iter()
                        .find(|r| r.matches(Some(&id_str), (*rec).window.title.as_deref()))
                        .cloned();
                    let allow_remember = rule.as_ref().and_then(|r| r.remember).unwrap_or(true)
                        && st.remember_window_positions;

                    let saved = if allow_remember {
                        st.window_state_store.get(&id_str).cloned()
                    } else {
                        None
                    };

                    if !(*rec).window.state.maximized && !(*rec).window.state.fullscreen {
                        if let Some(pos) = rule
                            .as_ref()
                            .and_then(|r| r.position)
                            .or_else(|| saved.as_ref().and_then(|s| s.position))
                        {
                            let screen_rect = st.output_geometry.logical_rect();
                            let max_x = (screen_rect.size.w - 100).max(0);
                            let max_y = (screen_rect.size.h - 100).max(0);
                            let clamped_pos = aegis_core::Point {
                                x: pos.x.clamp(0, max_x),
                                y: pos.y.clamp(0, max_y),
                            };
                            (*rec).position = clamped_pos;
                            (*rec).window.position = clamped_pos;
                        }
                        if let Some(size) = rule
                            .as_ref()
                            .and_then(|r| r.size)
                            .or_else(|| saved.as_ref().and_then(|s| s.size))
                            && size.w >= 100
                            && size.h >= 100
                        {
                            (*rec).window.size = size;
                            reconfigure_with_size(rec, size.w, size.h);
                        }
                    }
                }
                extensions::foreign_toplevel_updated(rec, (*rec).state);
            }
        }
    }
}

unsafe extern "C" fn toplevel_set_parent(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    parent_resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        if parent_resource.is_null() {
            (*rec).window.parent = None;
            return;
        }
        let parent_rec = ffi::wl_resource_get_user_data(parent_resource) as *mut SurfaceRec;
        if parent_rec.is_null() {
            (*rec).window.parent = None;
        } else {
            (*rec).window.parent = Some(parent_rec as usize);
        }
    }
}

unsafe extern "C" fn toplevel_set_min_size(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    w: i32,
    h: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if !rec.is_null() {
            (*rec).window.size_hints.min_w = w.max(0);
            (*rec).window.size_hints.min_h = h.max(0);
        }
    }
}

unsafe extern "C" fn toplevel_set_max_size(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    w: i32,
    h: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if !rec.is_null() {
            (*rec).window.size_hints.max_w = w.max(0);
            (*rec).window.size_hints.max_h = h.max(0);
        }
    }
}

pub(crate) fn clamp_size_to_hints(
    requested: aegis_core::Size,
    hints: aegis_core::window::SizeHints,
) -> aegis_core::Size {
    let min_w = hints.min_w.max(1);
    let min_h = hints.min_h.max(1);
    let max_w = if hints.max_w > 0 {
        hints.max_w.max(min_w)
    } else {
        i32::MAX
    };
    let max_h = if hints.max_h > 0 {
        hints.max_h.max(min_h)
    } else {
        i32::MAX
    };
    aegis_core::Size {
        w: requested.w.clamp(min_w, max_w),
        h: requested.h.clamp(min_h, max_h),
    }
}

/// Common path for state transitions: set the bit, send a configure event
/// carrying the new states array. `width` / `height` of 0 means "let the
/// client pick" per xdg-shell; we pass 0,0 for state-only changes and reserve
/// non-zero dimensions for fullscreen-on-output (deferred).
pub(crate) unsafe fn reconfigure_with_state(rec: *mut SurfaceRec) {
    unsafe {
        if rec.is_null() {
            return;
        }
        let (w, h) = if (*rec).window.state.fullscreen {
            if (*rec).saved_floating_rect.is_none() {
                (*rec).saved_floating_rect = Some(aegis_core::Rect {
                    origin: (*rec).position,
                    size: (*rec).window.size,
                });
            }
            if !(*rec).state.is_null() {
                let rect = (*(*rec).state).output_geometry.logical_rect();
                (*rec).position = rect.origin;
                (*rec).window.position = rect.origin;
                (*rec).window.size = rect.size;
                (*rec).layout_target = Some(rect);
                (rect.size.w, rect.size.h)
            } else {
                (0, 0)
            }
        } else if (*rec).window.state.maximized {
            if (*rec).saved_floating_rect.is_none() {
                (*rec).saved_floating_rect = Some(aegis_core::Rect {
                    origin: (*rec).position,
                    size: (*rec).window.size,
                });
            }
            if !(*rec).state.is_null() {
                let last_work_area = (*(*rec).state).last_work_area;
                let rect = if last_work_area.size.w > 0 && last_work_area.size.h > 0 {
                    last_work_area
                } else {
                    (*(*rec).state).output_geometry.logical_rect()
                };
                (*rec).position = rect.origin;
                (*rec).window.position = rect.origin;
                (*rec).window.size = rect.size;
                (*rec).layout_target = Some(rect);
                (rect.size.w, rect.size.h)
            } else {
                (0, 0)
            }
        } else {
            if let Some(saved) = (*rec).saved_floating_rect.take() {
                (*rec).position = saved.origin;
                (*rec).window.position = saved.origin;
                (*rec).window.size = saved.size;
            }
            (*rec).layout_target = None;
            (0, 0)
        };
        reconfigure_with_size(rec, w, h);
    }
}

/// Post an `xdg_toplevel.configure` with an explicit width/height (the
/// tiling path forces a size, unlike the state-bit path). `w`/`h` of 0 mean
/// "client decides" per xdg-shell. The current window-state bits are sent
/// unchanged; tiling carries no protocol state of its own.
pub(crate) unsafe fn reconfigure_with_size(rec: *mut SurfaceRec, w: i32, h: i32) {
    unsafe {
        if rec.is_null() || (*rec).xdg_toplevel.is_null() {
            return;
        }
        let states = (*rec).window.state.to_state_array();
        let mut arr = ffi::wl_array::empty();
        // Pack the active state values into a wl_array as u32s. Each state entry
        // is one `uint` per the protocol, so the array's element size is 4.
        if !states.is_empty() {
            let bytes_needed = states.len() * std::mem::size_of::<u32>();
            let buf = malloc(bytes_needed) as *mut u32;
            if !buf.is_null() {
                for (i, s) in states.iter().enumerate() {
                    std::ptr::write(buf.add(i), *s);
                }
                arr.size = bytes_needed;
                arr.alloc = bytes_needed;
                arr.data = buf as *mut c_void;
            }
        }
        ffi::wl_resource_post_event(
            (*rec).xdg_toplevel,
            ffi::XDG_TOPLEVEL_CONFIGURE,
            w,
            h,
            &mut arr as *mut ffi::wl_array,
        );
        if !arr.data.is_null() {
            free(arr.data);
        }
        // Also fire xdg_surface.configure so the client acks the new state.
        if !(*rec).xdg_surface.is_null() {
            send_xdg_surface_configure(rec);
        }
    }
}

unsafe extern "C" fn toplevel_set_maximized(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        (*rec).window.state.maximized = true;
        reconfigure_with_state(rec);
    }
}

unsafe extern "C" fn toplevel_unset_maximized(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        (*rec).window.state.maximized = false;
        reconfigure_with_state(rec);
    }
}

unsafe extern "C" fn toplevel_set_fullscreen(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    _output: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        (*rec).window.state.fullscreen = true;
        reconfigure_with_state(rec);
    }
}

unsafe extern "C" fn toplevel_unset_fullscreen(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        (*rec).window.state.fullscreen = false;
        reconfigure_with_state(rec);
    }
}

/// `xdg_toplevel.set_minimized`: the client asks the compositor to hide it.
/// Unlike maximize/fullscreen this is not a configure state — the compositor
/// stops rendering and hit-testing the surface but keeps it mapped so the
/// client retains its buffers and can be restored. If the minimized toplevel
/// held keyboard focus, drop it (post `wl_keyboard.leave`, clear activated)
/// so typing no longer routes to an invisible window. Restore happens when a
/// later focus gain clears the flag (see `set_activated_for_surface`).
unsafe extern "C" fn toplevel_set_minimized(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        minimize_toplevel_record(rec);
    }
}

/// Apply compositor-internal minimization to one live toplevel record.
/// Kept separate from the protocol callback so shell/IPC actions follow the
/// exact same focus, configure, and client-flush semantics.
pub(crate) unsafe fn minimize_toplevel_record(rec: *mut SurfaceRec) {
    unsafe {
        if rec.is_null() || (*rec).xdg_toplevel.is_null() || (*rec).window.minimized {
            return;
        }
        let state = (*rec).state;
        if !state.is_null() && !(*state).reduced_motion {
            let old = aegis_core::Rect {
                origin: (*rec).position,
                size: (*rec).window.size,
            };
            let old_screen_h = (*state).output_geometry.logical_rect().size.h;
            let now = (*state).now_ms();
            (*rec).window.transition =
                Some(aegis_core::transition::WindowTransition::new(old, now));
            let target_origin = aegis_core::Point {
                x: old.origin.x + old.size.w / 4,
                y: old_screen_h - 20,
            };
            let target_size = aegis_core::Size {
                w: (old.size.w / 2).max(40),
                h: 20,
            };
            (*rec).position = target_origin;
            (*rec).window.position = target_origin;
            (*rec).window.size = target_size;
        }
        (*rec).window.minimized = true;
        if state.is_null() {
            return;
        }
        // A minimized toplevel is no longer a valid focus target on any seat.
        // This free protocol handler mirrors the complete dependency set in
        // `Server::change_keyboard_focus`, not just wl_keyboard.
        let root = surface_root_toplevel(rec);
        let seats = (*state).seats.keys().copied().collect::<Vec<_>>();
        let mut focus_dropped = false;
        for seat in seats {
            let Some(_guard) = ActiveSeatGuard::enter_existing(&mut *state, seat) else {
                continue;
            };
            let old_focus = (*state).keyboard_focus;
            if old_focus.is_null() {
                continue;
            }
            let focused_rec = ffi::wl_resource_get_user_data(old_focus) as *mut SurfaceRec;
            if focused_rec.is_null() || surface_root_toplevel(focused_rec) != root {
                continue;
            }
            let serial = ffi::wl_display_next_serial((*state).display);
            let old_client = ffi::wl_resource_get_client(old_focus);
            let keyboards = (*state)
                .keyboard_resources
                .iter()
                .copied()
                .filter(|keyboard| {
                    !keyboard.is_null() && ffi::wl_resource_get_client(*keyboard) == old_client
                })
                .collect::<Vec<_>>();
            for keyboard in keyboards {
                ffi::wl_resource_post_event(keyboard, ffi::WL_KEYBOARD_LEAVE, serial, old_focus);
            }
            (*state).keyboard_focus = std::ptr::null_mut();
            keyboard_focus_dependencies_changed(state, old_focus, std::ptr::null_mut());
            focus_dropped = true;
        }
        if focus_dropped {
            (*rec).window.state.activated = false;
            reconfigure_with_state(rec);
        }
        ffi::wl_display_flush_clients((*state).display);
    }
}

// ----- interactive move / resize -----------------------------------------
//
// `xdg_toplevel.move` / `resize` start an interactive grab when the supplied
// serial matches the last pointer-button press. While the grab is active,
// pointer motion updates the window's geometry (move: translate position;
// resize: grow/shrink along the requested edges, clamped to size hints).
// Button release ends the grab.
//
// The grab is compositor-side: the window moves whether or not the client
// cooperative-processes; the client still receives `wl_pointer.motion`
// events as normal during the grab (per protocol, the pointer stays with
// the surface that had it when the grab began).

unsafe extern "C" fn toplevel_move(
    client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    seat: *mut ffi::wl_resource,
    _serial: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        let state_ptr = (*rec).state;
        if state_ptr.is_null()
            || !(*state_ptr).implicit_grab_active
            || seat.is_null()
            || ffi::wl_resource_get_client(seat) != client
            || ffi::wl_resource_get_client((*rec).resource) != client
        {
            return;
        }
        if (*state_ptr).interactive.is_some() {
            return; // Already grabbing; ignore.
        }
        let layout_changed = (*rec).window.layout_role != aegis_core::layout::LayoutRole::Floating;
        (*rec).window.layout_role = aegis_core::layout::LayoutRole::Floating;
        (*rec).layout_target = None;
        let state_changed = (*rec).window.state.maximized || (*rec).window.state.fullscreen;
        (*rec).window.state.maximized = false;
        (*rec).window.state.fullscreen = false;
        if state_changed || layout_changed {
            reconfigure_with_state(rec);
        }
        (*state_ptr).interactive = Some(aegis_core::window::Interactive::Move {
            window_id: (*rec).window.id,
            origin: ((*state_ptr).pointer_x, (*state_ptr).pointer_y),
            start_position: (*rec).position,
        });
        (*state_ptr).compositor_pointer_grab = false;
    }
}

unsafe extern "C" fn toplevel_resize(
    client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    seat: *mut ffi::wl_resource,
    _serial: u32,
    edges: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        let state_ptr = (*rec).state;
        if state_ptr.is_null()
            || !(*state_ptr).implicit_grab_active
            || seat.is_null()
            || ffi::wl_resource_get_client(seat) != client
            || ffi::wl_resource_get_client((*rec).resource) != client
        {
            return;
        }
        if (*state_ptr).interactive.is_some() {
            return;
        }
        let edges = aegis_core::window::ResizeEdges(edges);
        if edges.is_none() {
            return;
        }
        (*rec).window.layout_role = aegis_core::layout::LayoutRole::Floating;
        (*rec).layout_target = None;
        (*rec).window.state.maximized = false;
        (*rec).window.state.fullscreen = false;
        (*rec).window.state.resizing = true;
        reconfigure_with_state(rec);
        (*state_ptr).interactive = Some(aegis_core::window::Interactive::Resize {
            window_id: (*rec).window.id,
            edges,
            origin: ((*state_ptr).pointer_x, (*state_ptr).pointer_y),
            start_position: (*rec).position,
            start_size: (*rec)
                .window_geometry
                .map(|geometry| geometry.size)
                .unwrap_or_else(|| surface_logical_size(&*rec)),
        });
        (*state_ptr).compositor_pointer_grab = false;
    }
}

/// Apply a pointer-motion delta to an active interactive grab. Returns
/// `true` if the motion was consumed by the grab (the position changed) and
/// the caller may want to still forward the motion event to the focused
/// client, or `false` if there was no active grab.
///
/// On resize, the new window size is clamped to the toplevel's `size_hints`
/// (min and max) and a minimum of 1×1. The server then posts a fresh
/// `xdg_toplevel.configure` with the new width/height so the client can
/// reallocate its buffer.
impl Server {
    pub(crate) fn apply_interactive_motion(&mut self, x: f32, y: f32) -> bool {
        let interactive = match self.state.interactive {
            Some(i) => i,
            None => return false,
        };
        // Find the surface rec backing this window id.
        let rec_ptr: *mut SurfaceRec = self.find_surface_by_window_id(interactive.window_id());
        if rec_ptr.is_null() {
            self.state.interactive = None;
            self.state.compositor_pointer_grab = false;
            return false;
        }
        match interactive {
            aegis_core::window::Interactive::Move {
                start_position,
                origin,
                ..
            } => {
                let new_x = start_position.x as f32 + (x - origin.0);
                let new_y = start_position.y as f32 + (y - origin.1);
                // Round to integer logical pixels.
                let pos = aegis_core::Point {
                    x: new_x.round() as i32,
                    y: new_y.round() as i32,
                };
                unsafe {
                    (*rec_ptr).position = pos;
                    (*rec_ptr).window.position = pos;
                }
                self.state.damaged_windows.insert(interactive.window_id());
                true
            }
            aegis_core::window::Interactive::Resize {
                edges,
                origin,
                start_position,
                start_size,
                ..
            } => {
                let dx = (x - origin.0).round() as i32;
                let dy = (y - origin.1).round() as i32;
                let mut new_x = start_position.x;
                let mut new_y = start_position.y;
                let mut new_w = start_size.w;
                let mut new_h = start_size.h;
                // Horizontal axis.
                if edges.has_right() {
                    new_w = start_size.w + dx;
                } else if edges.has_left() {
                    new_w = start_size.w - dx;
                    new_x = start_position.x + dx;
                }
                // Vertical axis.
                if edges.has_bottom() {
                    new_h = start_size.h + dy;
                } else if edges.has_top() {
                    new_h = start_size.h - dy;
                    new_y = start_position.y + dy;
                }
                // Clamp to size hints and a 1×1 floor. If a max constraint
                // forces a smaller size on the growing side, also pull back the
                // position to keep the opposite edge anchored.
                let hints = unsafe { (*rec_ptr).window.size_hints };
                let min_w = hints.min_w.max(1);
                let min_h = hints.min_h.max(1);
                let max_w = if hints.max_w > 0 {
                    hints.max_w
                } else {
                    i32::MAX
                };
                let max_h = if hints.max_h > 0 {
                    hints.max_h
                } else {
                    i32::MAX
                };
                if new_w < min_w {
                    if edges.has_left() {
                        new_x += min_w - new_w;
                    }
                    new_w = min_w;
                } else if new_w > max_w {
                    if edges.has_left() {
                        new_x += max_w - new_w;
                    }
                    new_w = max_w;
                }
                if new_h < min_h {
                    if edges.has_top() {
                        new_y += min_h - new_h;
                    }
                    new_h = min_h;
                } else if new_h > max_h {
                    if edges.has_top() {
                        new_y += max_h - new_h;
                    }
                    new_h = max_h;
                }
                // Apply and send a configure with the new dimensions so the
                // client reallocates its buffer.
                unsafe {
                    let pos = aegis_core::Point { x: new_x, y: new_y };
                    (*rec_ptr).position = pos;
                    (*rec_ptr).window.position = pos;
                    (*rec_ptr).window.size = aegis_core::Size { w: new_w, h: new_h };
                    if !(*rec_ptr).xdg_toplevel.is_null() {
                        let mut arr = ffi::wl_array::empty();
                        let states = (*rec_ptr).window.state.to_state_array();
                        if !states.is_empty() {
                            let bytes_needed = states.len() * std::mem::size_of::<u32>();
                            let buf = malloc(bytes_needed) as *mut u32;
                            if !buf.is_null() {
                                for (i, s) in states.iter().enumerate() {
                                    std::ptr::write(buf.add(i), *s);
                                }
                                arr.size = bytes_needed;
                                arr.alloc = bytes_needed;
                                arr.data = buf as *mut c_void;
                            }
                        }
                        ffi::wl_resource_post_event(
                            (*rec_ptr).xdg_toplevel,
                            ffi::XDG_TOPLEVEL_CONFIGURE,
                            new_w,
                            new_h,
                            &mut arr as *mut ffi::wl_array,
                        );
                        if !arr.data.is_null() {
                            free(arr.data);
                        }
                        if !(*rec_ptr).xdg_surface.is_null() {
                            send_xdg_surface_configure(rec_ptr);
                        }
                    }
                }
                self.state.damaged_windows.insert(interactive.window_id());
                true
            }
        }
    }

    pub(crate) fn find_surface_by_window_id(
        &self,
        id: aegis_core::window::WindowId,
    ) -> *mut SurfaceRec {
        for p in self.state.live_surfaces() {
            if unsafe { (*p).window.id } == id {
                return p;
            }
        }
        std::ptr::null_mut()
    }
}
