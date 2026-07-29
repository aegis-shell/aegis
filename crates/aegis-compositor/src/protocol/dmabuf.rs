use crate::*;
use std::collections::HashSet;
use std::io::Write;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, RawFd};

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

// Linux memfd flags and seals. The feedback format table becomes immutable
// before its fd is transferred to libwayland/client code.
const MFD_CLOEXEC_AND_ALLOW_SEALING: u32 = 3;
const F_SEAL_SEAL: i32 = 0x0001;
const F_SEAL_SHRINK: i32 = 0x0002;
const F_SEAL_GROW: i32 = 0x0004;
const F_SEAL_WRITE: i32 = 0x0008;
const F_ADD_SEALS: i32 = 1033;

unsafe extern "C" {
    fn memfd_create(name: *const std::os::raw::c_char, flags: u32) -> i32;
    fn fcntl(fd: i32, cmd: i32, ...) -> i32;
}

static DMABUF_IMPL: ffi::zwp_linux_dmabuf_v1_interface_impl =
    ffi::zwp_linux_dmabuf_v1_interface_impl {
        destroy: res_destroy,
        create_params: dmabuf_create_params,
        get_default_feedback: dmabuf_get_default_feedback,
        get_surface_feedback: dmabuf_get_surface_feedback,
    };

static FEEDBACK_IMPL: ffi::zwp_linux_dmabuf_feedback_v1_interface_impl =
    ffi::zwp_linux_dmabuf_feedback_v1_interface_impl {
        destroy: res_destroy,
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

        // v4 clients receive this information through feedback objects. The
        // legacy events are forbidden starting at v4.
        if version >= 4 {
            return;
        }

        // Advertise the renderer's real format/modifier table so legacy
        // clients
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

/// Flatten renderer capabilities into the native format-table order, removing
/// duplicates while preserving preference order. The LINEAR fallback mirrors
/// the legacy bind path used by tests and renderers without a queried table.
fn dmabuf_format_pairs(formats: &[aegis_core::dmabuf::DmabufFormat]) -> Vec<(u32, u64)> {
    let mut pairs = Vec::new();
    let mut seen = HashSet::new();
    if formats.is_empty() {
        for fourcc in [
            DRM_FORMAT_ARGB8888,
            DRM_FORMAT_XRGB8888,
            DRM_FORMAT_ABGR8888,
            DRM_FORMAT_XBGR8888,
        ] {
            pairs.push((fourcc, DRM_FORMAT_MOD_LINEAR));
        }
        return pairs;
    }

    for entry in formats {
        for &modifier in &entry.modifiers {
            let pair = (entry.fourcc, modifier);
            if seen.insert(pair) {
                pairs.push(pair);
            }
        }
    }
    pairs
}

/// Serialize the linux-dmabuf v4 table ABI: u32 fourcc, four zero padding
/// bytes, u64 modifier, all in native endianness.
fn format_table_bytes(pairs: &[(u32, u64)]) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(pairs.len() * 16);
    for &(fourcc, modifier) in pairs {
        bytes.extend_from_slice(&fourcc.to_ne_bytes());
        bytes.extend_from_slice(&[0; 4]);
        bytes.extend_from_slice(&modifier.to_ne_bytes());
    }
    bytes
}

fn sealed_format_table_fd(bytes: &[u8]) -> std::io::Result<RawFd> {
    let fd = unsafe {
        memfd_create(
            c"aegis-dmabuf-feedback".as_ptr(),
            MFD_CLOEXEC_AND_ALLOW_SEALING,
        )
    };
    if fd < 0 {
        return Err(std::io::Error::last_os_error());
    }

    // SAFETY: `fd` is newly created and exclusively owned here. On every
    // error `File` closes it; success transfers ownership to libwayland.
    let mut file = unsafe { std::fs::File::from_raw_fd(fd) };
    file.write_all(bytes)?;
    let seals = F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE;
    if unsafe { fcntl(file.as_raw_fd(), F_ADD_SEALS, seals) } < 0 {
        log::warn!(
            "[dmabuf] format-table memfd sealing failed: {}",
            std::io::Error::last_os_error()
        );
    }
    Ok(file.into_raw_fd())
}

fn borrowed_wl_array<T>(values: &[T]) -> ffi::wl_array {
    ffi::wl_array {
        size: std::mem::size_of_val(values),
        alloc: std::mem::size_of_val(values),
        data: if values.is_empty() {
            std::ptr::null_mut()
        } else {
            values.as_ptr().cast_mut().cast()
        },
    }
}

unsafe fn create_dmabuf_feedback(
    client: *mut ffi::wl_client,
    dmabuf: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let version = ffi::wl_resource_get_version(dmabuf);
        let feedback = ffi::wl_resource_create(
            client,
            &ffi::zwp_linux_dmabuf_feedback_v1_interface,
            version,
            id,
        );
        if feedback.is_null() {
            return;
        }

        let state = ffi::wl_resource_get_user_data(dmabuf) as *mut State;
        ffi::wl_resource_set_implementation(
            feedback,
            &FEEDBACK_IMPL as *const _ as *const c_void,
            state.cast(),
            None,
        );
        let Some(state) = state.as_ref() else {
            log::error!("[dmabuf] feedback requested without compositor state");
            ffi::wl_resource_destroy(feedback);
            return;
        };
        let Some(main_device) = state.dmabuf_main_device else {
            log::error!("[dmabuf] v4 feedback requested without a main DRM device");
            ffi::wl_resource_destroy(feedback);
            return;
        };

        let mut pairs = dmabuf_format_pairs(&state.dmabuf_formats);
        // tranche indices are native-endian u16 values.
        let max_pairs = usize::from(u16::MAX) + 1;
        if pairs.len() > max_pairs {
            log::warn!(
                "[dmabuf] truncating feedback format table from {} to {max_pairs} entries",
                pairs.len()
            );
            pairs.truncate(max_pairs);
        }
        if pairs.is_empty() {
            log::error!("[dmabuf] renderer exposed no usable format/modifier pairs");
            ffi::wl_resource_destroy(feedback);
            return;
        }
        let table = format_table_bytes(&pairs);
        let table_size = u32::try_from(table.len()).expect("dmabuf feedback table fits u32");
        let table_fd = match sealed_format_table_fd(&table) {
            Ok(fd) => fd,
            Err(error) => {
                log::error!("[dmabuf] cannot create feedback format table: {error}");
                ffi::wl_resource_destroy(feedback);
                return;
            }
        };

        // libwayland consumes/closes the fd after marshalling the event.
        ffi::wl_resource_post_event(
            feedback,
            ffi::ZWP_LINUX_DMABUF_FEEDBACK_V1_FORMAT_TABLE,
            table_fd,
            table_size,
        );

        // `dev_t` is sent as an opaque native byte array. The host supplies
        // the node matching Flux's Vulkan physical device, preferring its
        // render node and falling back to the KMS primary node.
        let device = [main_device];
        let mut device_array = borrowed_wl_array(&device);
        ffi::wl_resource_post_event(
            feedback,
            ffi::ZWP_LINUX_DMABUF_FEEDBACK_V1_MAIN_DEVICE,
            &mut device_array as *mut ffi::wl_array,
        );
        ffi::wl_resource_post_event(
            feedback,
            ffi::ZWP_LINUX_DMABUF_FEEDBACK_V1_TRANCHE_TARGET_DEVICE,
            &mut device_array as *mut ffi::wl_array,
        );
        ffi::wl_resource_post_event(
            feedback,
            ffi::ZWP_LINUX_DMABUF_FEEDBACK_V1_TRANCHE_FLAGS,
            0u32,
        );

        let indices = (0..pairs.len())
            .map(|index| index as u16)
            .collect::<Vec<_>>();
        let mut indices_array = borrowed_wl_array(&indices);
        ffi::wl_resource_post_event(
            feedback,
            ffi::ZWP_LINUX_DMABUF_FEEDBACK_V1_TRANCHE_FORMATS,
            &mut indices_array as *mut ffi::wl_array,
        );
        ffi::wl_resource_post_event(feedback, ffi::ZWP_LINUX_DMABUF_FEEDBACK_V1_TRANCHE_DONE);
        ffi::wl_resource_post_event(feedback, ffi::ZWP_LINUX_DMABUF_FEEDBACK_V1_DONE);
    }
}

unsafe extern "C" fn dmabuf_get_default_feedback(
    client: *mut ffi::wl_client,
    dmabuf: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe { create_dmabuf_feedback(client, dmabuf, id) };
}

unsafe extern "C" fn dmabuf_get_surface_feedback(
    client: *mut ffi::wl_client,
    dmabuf: *mut ffi::wl_resource,
    id: u32,
    _surface: *mut ffi::wl_resource,
) {
    // Aegis currently has one render device and one format preference set.
    // Surface feedback still matters: Mesa requests it to select that device.
    unsafe { create_dmabuf_feedback(client, dmabuf, id) };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn feedback_table_has_native_16_byte_entries_and_zero_padding() {
        let pairs = [
            (DRM_FORMAT_XRGB8888, 0x0102_0304_0506_0708),
            (DRM_FORMAT_ARGB8888, DRM_FORMAT_MOD_LINEAR),
        ];
        let bytes = format_table_bytes(&pairs);
        assert_eq!(bytes.len(), 32);
        assert_eq!(&bytes[0..4], &DRM_FORMAT_XRGB8888.to_ne_bytes());
        assert_eq!(&bytes[4..8], &[0; 4]);
        assert_eq!(&bytes[8..16], &0x0102_0304_0506_0708u64.to_ne_bytes());
        assert_eq!(&bytes[16..20], &DRM_FORMAT_ARGB8888.to_ne_bytes());
        assert_eq!(&bytes[20..24], &[0; 4]);
        assert_eq!(&bytes[24..32], &DRM_FORMAT_MOD_LINEAR.to_ne_bytes());
    }

    #[test]
    fn feedback_pairs_preserve_order_and_remove_duplicates() {
        let formats = vec![
            aegis_core::dmabuf::DmabufFormat {
                fourcc: DRM_FORMAT_XRGB8888,
                modifiers: vec![5, 0, 5],
            },
            aegis_core::dmabuf::DmabufFormat {
                fourcc: DRM_FORMAT_ARGB8888,
                modifiers: vec![0],
            },
        ];
        assert_eq!(
            dmabuf_format_pairs(&formats),
            vec![
                (DRM_FORMAT_XRGB8888, 5),
                (DRM_FORMAT_XRGB8888, 0),
                (DRM_FORMAT_ARGB8888, 0),
            ]
        );
    }
}
