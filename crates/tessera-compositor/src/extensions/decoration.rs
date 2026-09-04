use super::*;

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
    unsafe {
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
}

unsafe extern "C" fn xdg_decoration_get_toplevel(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    id: u32,
    toplevel: *mut ffi::wl_resource,
) {
    unsafe {
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
        configure_decoration(resource, rec);
    }
}

fn protocol_mode(policy: tessera_model::window::DecorationPolicy) -> u32 {
    match policy {
        tessera_model::window::DecorationPolicy::Borderless => {
            // Borderless windows are compositor-owned. Tessera supplies move,
            // resize, close, and state controls outside a per-window frame,
            // so the client must not add its own title bar.
            ffi::ZXDG_TOPLEVEL_DECORATION_V1_MODE_SERVER_SIDE
        }
        tessera_model::window::DecorationPolicy::ClientSide => {
            ffi::ZXDG_TOPLEVEL_DECORATION_V1_MODE_CLIENT_SIDE
        }
    }
}

pub(crate) unsafe fn configure_decoration(resource: *mut ffi::wl_resource, rec: *mut SurfaceRec) {
    unsafe {
        if resource.is_null() || rec.is_null() || (*rec).state.is_null() {
            return;
        }
        let policy = (*(*rec).state).decoration_policy;
        ffi::wl_resource_post_event(
            resource,
            ffi::ZXDG_TOPLEVEL_DECORATION_V1_CONFIGURE,
            protocol_mode(policy),
        );
        crate::reconfigure_with_state(rec);
    }
}

unsafe extern "C" fn xdg_toplevel_decoration_set_mode(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    mode: u32,
) {
    unsafe {
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
            // set_mode is a preference, not an instruction. The compositor's
            // explicit desktop policy selects the effective mode.
            configure_decoration(resource, rec);
        }
    }
}

unsafe extern "C" fn xdg_toplevel_decoration_unset_mode(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if !rec.is_null() {
            configure_decoration(resource, rec);
        }
    }
}

unsafe extern "C" fn xdg_toplevel_decoration_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        xdg_toplevel_decoration_resource_destroy(resource);
        ffi::wl_resource_set_user_data(resource, std::ptr::null_mut());
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn xdg_toplevel_decoration_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if !rec.is_null() && (*rec).xdg_decoration == resource {
            (*rec).xdg_decoration = std::ptr::null_mut();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn borderless_policy_uses_server_side_protocol_ownership() {
        assert_eq!(
            protocol_mode(tessera_model::window::DecorationPolicy::Borderless),
            ffi::ZXDG_TOPLEVEL_DECORATION_V1_MODE_SERVER_SIDE
        );
    }

    #[test]
    fn client_side_policy_uses_client_side_protocol_mode() {
        assert_eq!(
            protocol_mode(tessera_model::window::DecorationPolicy::ClientSide),
            ffi::ZXDG_TOPLEVEL_DECORATION_V1_MODE_CLIENT_SIDE
        );
    }
}
