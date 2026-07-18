use crate::*;

// ----- wl_subcompositor ---------------------------------------------------

static SUBCOMPOSITOR_IMPL: ffi::wl_subcompositor_interface_impl =
    ffi::wl_subcompositor_interface_impl {
        destroy: res_destroy,
        get_subsurface: subcompositor_get_subsurface,
    };

static SUBSURFACE_IMPL: ffi::wl_subsurface_interface_impl = ffi::wl_subsurface_interface_impl {
    destroy: subsurface_destroy,
    set_position: subsurface_set_position,
    place_above: subsurface_place_above,
    place_below: subsurface_place_below,
    set_sync: subsurface_set_sync,
    set_desync: subsurface_set_desync,
};

pub(crate) unsafe extern "C" fn subcompositor_bind(
    client: *mut ffi::wl_client,
    _data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let res = ffi::wl_resource_create(
            client,
            &ffi::wl_subcompositor_interface,
            version as c_int,
            id,
        );
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &SUBCOMPOSITOR_IMPL as *const _ as *const c_void,
            std::ptr::null_mut(),
            None,
        );
    }
}

unsafe extern "C" fn subcompositor_get_subsurface(
    client: *mut ffi::wl_client,
    parent_res: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
    parent: *mut ffi::wl_resource,
) {
    unsafe {
        let child_rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
        let parent_rec = ffi::wl_resource_get_user_data(parent) as *mut SurfaceRec;
        if child_rec.is_null()
            || parent_rec.is_null()
            || child_rec == parent_rec
            || surface_has_role(&*child_rec)
            || ffi::wl_resource_get_client(surface) != client
            || ffi::wl_resource_get_client(parent) != client
        {
            ffi::wl_resource_post_error(
                parent_res,
                0,
                c"invalid wl_subsurface child or parent".as_ptr(),
            );
            return;
        }
        let ver = ffi::wl_resource_get_version(parent_res);
        let sub = ffi::wl_resource_create(client, &ffi::wl_subsurface_interface, ver, id);
        if sub.is_null() {
            return;
        }
        // Link the child into the parent's children list. The rec pointer is
        // shared; both surface and subsurface resource reference it.
        (*child_rec).parent = parent_rec;
        (*parent_rec).children.push(child_rec);
        ffi::wl_resource_set_implementation(
            sub,
            &SUBSURFACE_IMPL as *const _ as *const c_void,
            child_rec as *mut c_void,
            None,
        );
    }
}

// `wl_subsurface` request handlers. Synchronized children cache their pending
// surface state until the parent commits; desynchronized children apply
// immediately. Parent commits recursively release cached child commits.

unsafe extern "C" fn subsurface_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if !rec.is_null() {
            detach_from_parent(rec);
        }
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn subsurface_set_position(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if !rec.is_null() {
            (*rec).subsurface_offset = ass_core::Point { x, y };
        }
    }
}

unsafe extern "C" fn subsurface_place_above(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    sibling: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        let sibling_rec = ffi::wl_resource_get_user_data(sibling) as *mut SurfaceRec;
        if rec.is_null() || (*rec).parent.is_null() {
            return;
        }
        let parent = (*rec).parent;
        (*parent).children.retain(|child| *child != rec);
        if sibling_rec == parent {
            (*rec).subsurface_above_parent = true;
            let index = (*parent)
                .children
                .iter()
                .position(|child| !child.is_null() && (**child).subsurface_above_parent)
                .unwrap_or((*parent).children.len());
            (*parent).children.insert(index, rec);
        } else if !sibling_rec.is_null() && (*sibling_rec).parent == parent {
            (*rec).subsurface_above_parent = (*sibling_rec).subsurface_above_parent;
            let index = (*parent)
                .children
                .iter()
                .position(|child| *child == sibling_rec)
                .map_or((*parent).children.len(), |index| index + 1);
            (*parent).children.insert(index, rec);
        } else {
            (*parent).children.push(rec);
        }
    }
}

unsafe extern "C" fn subsurface_place_below(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    sibling: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        let sibling_rec = ffi::wl_resource_get_user_data(sibling) as *mut SurfaceRec;
        if rec.is_null() || (*rec).parent.is_null() {
            return;
        }
        let parent = (*rec).parent;
        (*parent).children.retain(|child| *child != rec);
        if sibling_rec == parent {
            (*rec).subsurface_above_parent = false;
            let index = (*parent)
                .children
                .iter()
                .position(|child| !child.is_null() && (**child).subsurface_above_parent)
                .unwrap_or((*parent).children.len());
            (*parent).children.insert(index, rec);
        } else if !sibling_rec.is_null() && (*sibling_rec).parent == parent {
            (*rec).subsurface_above_parent = (*sibling_rec).subsurface_above_parent;
            let index = (*parent)
                .children
                .iter()
                .position(|child| *child == sibling_rec)
                .unwrap_or(0);
            (*parent).children.insert(index, rec);
        } else {
            (*parent).children.insert(0, rec);
        }
    }
}

unsafe extern "C" fn subsurface_set_sync(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if !rec.is_null() {
            (*rec).subsurface_sync = true;
        }
    }
}

unsafe extern "C" fn subsurface_set_desync(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        (*rec).subsurface_sync = false;
        if (*rec).subsurface_cached_commit {
            surface_commit(std::ptr::null_mut(), (*rec).resource);
        }
    }
}

/// Detach a subsurface from its parent's children list. Called from
/// `subsurface.destroy` and from `surface_resource_destroy` (the latter
/// because a surface can be destroyed without its subsurface role being
/// explicitly destroyed first).
pub(crate) unsafe fn detach_from_parent(rec: *mut SurfaceRec) {
    unsafe {
        let parent = (*rec).parent;
        if parent.is_null() {
            return;
        }
        let target = rec;
        (*parent).children.retain(|c| *c != target);
        (*rec).parent = std::ptr::null_mut();
    }
}
