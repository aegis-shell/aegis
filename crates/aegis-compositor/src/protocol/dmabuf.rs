use crate::*;

// ----- zwp_linux_dmabuf_v1 ------------------------------------------------

/// DRM fourccs advertised to clients. ARGB8888 and ABGR8888 are the two
/// 32-bit-per-pixel byte orderings clients actually use; the X-variants are
/// the alpha-undefined counterparts (the server forces alpha opaque).
const DRM_FORMAT_ARGB8888: u32 = 0x3432_5241;
const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258;
const DRM_FORMAT_ABGR8888: u32 = 0x3432_4241;
const DRM_FORMAT_XBGR8888: u32 = 0x3432_4258;
/// DRM modifier advertised. Flux imports an explicit single-plane layout, so
/// do not advertise INVALID/implicit and let clients select a layout we cannot
/// validate or sample.
const DRM_FORMAT_MOD_LINEAR: u64 = 0;

static DMABUF_IMPL: ffi::zwp_linux_dmabuf_v1_interface_impl =
    ffi::zwp_linux_dmabuf_v1_interface_impl {
        destroy: res_destroy,
        create_params: dmabuf_create_params,
        get_default_feedback: dmabuf_noop_id,
        get_surface_feedback: dmabuf_noop_id_obj,
    };

static PARAMS_IMPL: ffi::zwp_linux_buffer_params_v1_interface_impl =
    ffi::zwp_linux_buffer_params_v1_interface_impl {
        destroy: res_destroy,
        add: params_add,
        create: params_create,
        create_immed: params_create_immed,
    };

pub(crate) static WL_BUFFER_IMPL: ffi::wl_buffer_interface_impl = ffi::wl_buffer_interface_impl {
    destroy: res_destroy,
};

pub(crate) unsafe extern "C" fn dmabuf_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let res = ffi::wl_resource_create(
            client,
            &ffi::zwp_linux_dmabuf_v1_interface,
            version as c_int,
            id,
        );
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &DMABUF_IMPL as *const _ as *const c_void,
            data,
            None,
        );

        // Advertise the renderer's real format/modifier table so clients
        // allocate GPU-optimal (tiled/compressed) buffers. A modifier set is
        // mandatory for correct performance: advertising only LINEAR forces
        // clients onto uncompressed, untiled layouts. Fall back to the four
        // 32-bit fourccs with LINEAR when no table is present (tests and any
        // path that bypassed the renderer query) so every client keeps working.
        let state = data as *mut State;
        let formats = if !state.is_null() && !(*state).dmabuf_formats.is_empty() {
            &(*state).dmabuf_formats
        } else {
            &[] as &[aegis_core::dmabuf::DmabufFormat]
        };
        if formats.is_empty() {
            for fmt in [
                DRM_FORMAT_ARGB8888,
                DRM_FORMAT_XRGB8888,
                DRM_FORMAT_ABGR8888,
                DRM_FORMAT_XBGR8888,
            ] {
                ffi::wl_resource_post_event(res, ffi::ZWP_LINUX_DMABUF_V1_FORMAT, fmt);
                if version >= 3 {
                    let hi = (DRM_FORMAT_MOD_LINEAR >> 32) as u32;
                    let lo = (DRM_FORMAT_MOD_LINEAR & 0xffff_ffff) as u32;
                    ffi::wl_resource_post_event(
                        res,
                        ffi::ZWP_LINUX_DMABUF_V1_MODIFIER,
                        fmt,
                        hi,
                        lo,
                    );
                }
            }
        } else {
            for entry in formats {
                let fmt = entry.fourcc;
                ffi::wl_resource_post_event(res, ffi::ZWP_LINUX_DMABUF_V1_FORMAT, fmt);
                if version >= 3 {
                    for &modifier in &entry.modifiers {
                        let hi = (modifier >> 32) as u32;
                        let lo = (modifier & 0xffff_ffff) as u32;
                        ffi::wl_resource_post_event(
                            res,
                            ffi::ZWP_LINUX_DMABUF_V1_MODIFIER,
                            fmt,
                            hi,
                            lo,
                        );
                    }
                }
            }
        }
    }
}

unsafe extern "C" fn dmabuf_create_params(
    client: *mut ffi::wl_client,
    dmabuf: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let ver = ffi::wl_resource_get_version(dmabuf);
        let params =
            ffi::wl_resource_create(client, &ffi::zwp_linux_buffer_params_v1_interface, ver, id);
        if params.is_null() {
            return;
        }
        let state = ffi::wl_resource_get_user_data(dmabuf) as *mut State;
        let acc = Box::into_raw(Box::new(DmabufBuffer::empty(state)));
        ffi::wl_resource_set_implementation(
            params,
            &PARAMS_IMPL as *const _ as *const c_void,
            acc as *mut c_void,
            Some(params_resource_destroy),
        );
    }
}

unsafe extern "C" fn params_add(
    _client: *mut ffi::wl_client,
    params: *mut ffi::wl_resource,
    fd: i32,
    plane_idx: u32,
    offset: u32,
    stride: u32,
    mod_hi: u32,
    mod_lo: u32,
) {
    unsafe {
        let acc = ffi::wl_resource_get_user_data(params) as *mut DmabufBuffer;
        if acc.is_null() || plane_idx != 0 {
            // Multi-plane not supported; close the fd we will not use.
            if fd >= 0 {
                libc_close(fd);
            }
            return;
        }
        if (*acc).have_plane && (*acc).fd >= 0 {
            libc_close((*acc).fd);
        }
        (*acc).fd = fd;
        (*acc).offset = offset;
        (*acc).stride = stride;
        (*acc).modifier = ((mod_hi as u64) << 32) | (mod_lo as u64);
        (*acc).have_plane = true;
    }
}

/// Finalize an accumulated params object into a `wl_buffer`. `id` may be 0 to
/// have the server allocate the id (the `create` path posts `created`), or a
/// client-supplied id (`create_immed`). Returns the new buffer resource.
unsafe fn params_finalize(
    client: *mut ffi::wl_client,
    params: *mut ffi::wl_resource,
    id: u32,
    width: i32,
    height: i32,
    format: u32,
) -> *mut ffi::wl_resource {
    unsafe {
        let acc = ffi::wl_resource_get_user_data(params) as *mut DmabufBuffer;
        if acc.is_null() || !(*acc).have_plane || width <= 0 || height <= 0 {
            return std::ptr::null_mut();
        }
        (*acc).width = width;
        (*acc).height = height;
        (*acc).drm_format = format;

        let buffer = ffi::wl_resource_create(client, &ffi::wl_buffer_interface, 1, id);
        if buffer.is_null() {
            return std::ptr::null_mut();
        }
        // Transfer ownership of the DmabufBuffer from params to the buffer.
        ffi::wl_resource_set_user_data(params, std::ptr::null_mut());
        ffi::wl_resource_set_implementation(
            buffer,
            &WL_BUFFER_IMPL as *const _ as *const c_void,
            acc as *mut c_void,
            Some(buffer_resource_destroy),
        );
        (*acc).resource = buffer;
        buffer
    }
}

unsafe extern "C" fn params_create(
    client: *mut ffi::wl_client,
    params: *mut ffi::wl_resource,
    width: i32,
    height: i32,
    format: u32,
    _flags: u32,
) {
    unsafe {
        let buffer = params_finalize(client, params, 0, width, height, format);
        if buffer.is_null() {
            ffi::wl_resource_post_event(params, ffi::ZWP_LINUX_BUFFER_PARAMS_V1_FAILED);
        } else {
            ffi::wl_resource_post_event(params, ffi::ZWP_LINUX_BUFFER_PARAMS_V1_CREATED, buffer);
        }
    }
}

unsafe extern "C" fn params_create_immed(
    client: *mut ffi::wl_client,
    params: *mut ffi::wl_resource,
    buffer_id: u32,
    width: i32,
    height: i32,
    format: u32,
    _flags: u32,
) {
    unsafe {
        // Protocol: create_immed failure is fatal to the client. The async `create`
        // path can post `failed` and recover; create_immed cannot. Post the
        // protocol error rather than leaving the client's new-id dangling.
        let buffer = params_finalize(client, params, buffer_id, width, height, format);
        if buffer.is_null() {
            let msg = CString::new("create_immed: missing plane or invalid dimensions").unwrap();
            ffi::wl_resource_post_error(
                params,
                ffi::ZWP_LINUX_BUFFER_PARAMS_V1_ERROR_INVALID_WL_BUFFER,
                msg.as_ptr(),
            );
        }
    }
}

unsafe extern "C" fn params_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let acc = ffi::wl_resource_get_user_data(resource) as *mut DmabufBuffer;
        if !acc.is_null() {
            drop(Box::from_raw(acc));
        }
    }
}

unsafe extern "C" fn buffer_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let db = ffi::wl_resource_get_user_data(resource) as *mut DmabufBuffer;
        if !db.is_null() {
            let state = (*db).state;
            if !state.is_null() {
                for surface in (*state).live_surfaces() {
                    if (*surface).current_buffer == resource {
                        (*surface).current_buffer = std::ptr::null_mut();
                    }
                }
                for retired in &mut (*state).retired_buffer_releases {
                    if retired.buffer == resource {
                        retired.buffer = std::ptr::null_mut();
                    }
                }
            }
            drop(Box::from_raw(db));
        }
    }
}

unsafe extern "C" fn dmabuf_noop_id(_c: *mut ffi::wl_client, _r: *mut ffi::wl_resource, _id: u32) {}
unsafe extern "C" fn dmabuf_noop_id_obj(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _id: u32,
    _surf: *mut ffi::wl_resource,
) {
}
