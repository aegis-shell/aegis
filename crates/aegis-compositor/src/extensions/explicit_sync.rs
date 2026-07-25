use super::*;

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
    unsafe {
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
}

unsafe extern "C" fn explicit_sync_get_surface(
    client: *mut ffi::wl_client,
    manager: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
) {
    unsafe {
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
}

unsafe extern "C" fn surface_sync_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn surface_sync_set_acquire(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    fd: i32,
) {
    unsafe {
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
}

unsafe extern "C" fn surface_sync_get_release(
    client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
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
}

unsafe extern "C" fn explicit_release_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
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
}

unsafe extern "C" fn surface_sync_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
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
            ffi::wl_resource_post_event(
                release,
                ffi::ZWP_LINUX_BUFFER_RELEASE_V1_IMMEDIATE_RELEASE,
            );
            ffi::wl_resource_destroy(release);
        }
        if !(*record).surface.is_null() {
            (*(*record).surface).explicit_sync = std::ptr::null_mut();
        }
        drop(Box::from_raw(record));
    }
}

pub(crate) unsafe fn explicit_sync_surface_committed(
    surface: *mut SurfaceRec,
    buffer_set: bool,
    buffer: *mut ffi::wl_resource,
) -> bool {
    unsafe {
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
}

pub(crate) unsafe fn explicit_sync_surface_destroyed(surface: *mut SurfaceRec) {
    unsafe {
        if surface.is_null() || (*surface).explicit_sync.is_null() {
            return;
        }
        let record = (*surface).explicit_sync as *mut SurfaceSyncRec;
        (*record).surface = std::ptr::null_mut();
        (*surface).explicit_sync = std::ptr::null_mut();
    }
}
