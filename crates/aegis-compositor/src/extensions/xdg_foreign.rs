//! xdg-foreign-unstable-v2: capability handles for cross-client transient
//! relationships. The protocol carries window authority only; it never sees
//! portal options or filesystem paths.

use super::*;
use std::fmt::Write as _;

struct ExportedRec {
    state: *mut State,
    surface: *mut SurfaceRec,
    handle: String,
    imports: Vec<*mut ffi::wl_resource>,
}

struct ImportedRec {
    state: *mut State,
    export: *mut ExportedRec,
    child: *mut SurfaceRec,
}

static EXPORTER_IMPL: ffi::zxdg_exporter_v2_interface_impl = ffi::zxdg_exporter_v2_interface_impl {
    destroy: crate::res_destroy,
    export_toplevel,
};

static IMPORTER_IMPL: ffi::zxdg_importer_v2_interface_impl = ffi::zxdg_importer_v2_interface_impl {
    destroy: crate::res_destroy,
    import_toplevel,
};

static EXPORTED_IMPL: ffi::zxdg_exported_v2_interface_impl = ffi::zxdg_exported_v2_interface_impl {
    destroy: exported_destroy,
};

static IMPORTED_IMPL: ffi::zxdg_imported_v2_interface_impl = ffi::zxdg_imported_v2_interface_impl {
    destroy: imported_destroy,
    set_parent_of: imported_set_parent_of,
};

pub(crate) unsafe extern "C" fn xdg_exporter_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let resource = ffi::wl_resource_create(
            client,
            &ffi::zxdg_exporter_v2_interface,
            version.min(1) as c_int,
            id,
        );
        if !resource.is_null() {
            ffi::wl_resource_set_implementation(
                resource,
                &EXPORTER_IMPL as *const _ as *const c_void,
                data,
                None,
            );
        }
    }
}

pub(crate) unsafe extern "C" fn xdg_importer_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let resource = ffi::wl_resource_create(
            client,
            &ffi::zxdg_importer_v2_interface,
            version.min(1) as c_int,
            id,
        );
        if !resource.is_null() {
            ffi::wl_resource_set_implementation(
                resource,
                &IMPORTER_IMPL as *const _ as *const c_void,
                data,
                None,
            );
        }
    }
}

unsafe extern "C" fn export_toplevel(
    client: *mut ffi::wl_client,
    exporter: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(exporter) as *mut State;
        let surface_rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
        if state.is_null()
            || surface_rec.is_null()
            || (*surface_rec).state != state
            || (*surface_rec).xdg_toplevel.is_null()
        {
            ffi::wl_resource_post_error(
                exporter,
                ffi::ZXDG_EXPORTER_V2_ERROR_INVALID_SURFACE,
                c"surface is not an xdg_toplevel".as_ptr(),
            );
            return;
        }
        let Some(handle) = fresh_handle(&(*state).xdg_foreign_exports) else {
            ffi::wl_resource_post_error(
                exporter,
                ffi::ZXDG_EXPORTER_V2_ERROR_INVALID_SURFACE,
                c"could not create a secure export handle".as_ptr(),
            );
            return;
        };
        let resource = ffi::wl_resource_create(client, &ffi::zxdg_exported_v2_interface, 1, id);
        if resource.is_null() {
            return;
        }
        let record = Box::into_raw(Box::new(ExportedRec {
            state,
            surface: surface_rec,
            handle: handle.clone(),
            imports: Vec::new(),
        }));
        ffi::wl_resource_set_implementation(
            resource,
            &EXPORTED_IMPL as *const _ as *const c_void,
            record as *mut c_void,
            Some(exported_resource_destroy),
        );
        (*state)
            .xdg_foreign_exports
            .insert(handle.clone(), resource);
        let handle = CString::new(handle).expect("hex handle contains no NUL");
        ffi::wl_resource_post_event(resource, ffi::ZXDG_EXPORTED_V2_HANDLE, handle.as_ptr());
    }
}

unsafe extern "C" fn import_toplevel(
    client: *mut ffi::wl_client,
    importer: *mut ffi::wl_resource,
    id: u32,
    handle: *const std::os::raw::c_char,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(importer) as *mut State;
        if state.is_null() {
            return;
        }
        let resource = ffi::wl_resource_create(client, &ffi::zxdg_imported_v2_interface, 1, id);
        if resource.is_null() {
            return;
        }
        let key = if handle.is_null() {
            String::new()
        } else {
            CStr::from_ptr(handle).to_string_lossy().into_owned()
        };
        let export = (*state)
            .xdg_foreign_exports
            .get(&key)
            .copied()
            .map(|resource| ffi::wl_resource_get_user_data(resource) as *mut ExportedRec)
            .filter(|record| !record.is_null())
            .unwrap_or(std::ptr::null_mut());
        let record = Box::into_raw(Box::new(ImportedRec {
            state,
            export,
            child: std::ptr::null_mut(),
        }));
        ffi::wl_resource_set_implementation(
            resource,
            &IMPORTED_IMPL as *const _ as *const c_void,
            record as *mut c_void,
            Some(imported_resource_destroy),
        );
        (*state).xdg_foreign_imports.push(resource);
        if export.is_null() {
            ffi::wl_resource_post_event(resource, ffi::ZXDG_IMPORTED_V2_DESTROYED);
        } else {
            (*export).imports.push(resource);
        }
    }
}

unsafe extern "C" fn exported_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn imported_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn imported_set_parent_of(
    client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    surface: *mut ffi::wl_resource,
) {
    unsafe {
        let imported = ffi::wl_resource_get_user_data(resource) as *mut ImportedRec;
        let child = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
        if imported.is_null() || (*imported).export.is_null() {
            return;
        }
        let parent = (*(*imported).export).surface;
        if child.is_null()
            || parent.is_null()
            || (*child).xdg_toplevel.is_null()
            || ffi::wl_resource_get_client(surface) != client
            || child == parent
            || parent_chain_contains(parent, child)
        {
            ffi::wl_resource_post_error(
                resource,
                ffi::ZXDG_IMPORTED_V2_ERROR_INVALID_SURFACE,
                c"surface is not a valid xdg_toplevel child".as_ptr(),
            );
            return;
        }
        clear_imported_parent(resource, imported);
        if !(*child).foreign_parent_owner.is_null() && (*child).foreign_parent_owner != resource {
            let previous =
                ffi::wl_resource_get_user_data((*child).foreign_parent_owner) as *mut ImportedRec;
            if !previous.is_null() && (*previous).child == child {
                (*previous).child = std::ptr::null_mut();
            }
        }
        (*imported).child = child;
        (*child).window.parent = Some(parent as usize);
        (*child).foreign_parent_owner = resource;
    }
}

unsafe extern "C" fn exported_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let record = ffi::wl_resource_get_user_data(resource) as *mut ExportedRec;
        if record.is_null() {
            return;
        }
        ffi::wl_resource_set_user_data(resource, std::ptr::null_mut());
        let record = Box::from_raw(record);
        if !record.state.is_null() {
            let state = &mut *record.state;
            if state.xdg_foreign_exports.get(&record.handle) == Some(&resource) {
                state.xdg_foreign_exports.remove(&record.handle);
            }
        }
        for imported in record.imports {
            invalidate_import(imported);
        }
    }
}

unsafe extern "C" fn imported_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let record = ffi::wl_resource_get_user_data(resource) as *mut ImportedRec;
        if record.is_null() {
            return;
        }
        ffi::wl_resource_set_user_data(resource, std::ptr::null_mut());
        let mut record = Box::from_raw(record);
        clear_imported_parent(resource, Box::as_mut(&mut record));
        if !record.export.is_null() {
            (*record.export)
                .imports
                .retain(|candidate| *candidate != resource);
        }
        if !record.state.is_null() {
            (*record.state)
                .xdg_foreign_imports
                .retain(|candidate| *candidate != resource);
        }
    }
}

unsafe fn invalidate_import(resource: *mut ffi::wl_resource) {
    unsafe {
        if resource.is_null() {
            return;
        }
        let record = ffi::wl_resource_get_user_data(resource) as *mut ImportedRec;
        if record.is_null() || (*record).export.is_null() {
            return;
        }
        clear_imported_parent(resource, record);
        (*record).export = std::ptr::null_mut();
        ffi::wl_resource_post_event(resource, ffi::ZXDG_IMPORTED_V2_DESTROYED);
    }
}

unsafe fn clear_imported_parent(resource: *mut ffi::wl_resource, record: *mut ImportedRec) {
    unsafe {
        if record.is_null() {
            return;
        }
        let child = (*record).child;
        if !child.is_null() && (*child).foreign_parent_owner == resource {
            (*child).window.parent = None;
            (*child).foreign_parent_owner = std::ptr::null_mut();
        }
        (*record).child = std::ptr::null_mut();
    }
}

/// Clear a toplevel parent before xdg-shell replaces or destroys it. If the
/// relationship came from an import, both sides are detached so later import
/// destruction cannot dereference a reclaimed child surface.
pub(crate) unsafe fn xdg_foreign_clear_child_parent(surface: *mut SurfaceRec) {
    unsafe {
        if surface.is_null() {
            return;
        }
        if !(*surface).foreign_parent_owner.is_null() {
            let owner = (*surface).foreign_parent_owner;
            let imported = ffi::wl_resource_get_user_data(owner) as *mut ImportedRec;
            if !imported.is_null() && (*imported).child == surface {
                (*imported).child = std::ptr::null_mut();
            }
        }
        (*surface).window.parent = None;
        (*surface).foreign_parent_owner = std::ptr::null_mut();
    }
}

unsafe fn parent_chain_contains(mut parent: *mut SurfaceRec, child: *mut SurfaceRec) -> bool {
    unsafe {
        let mut depth = 0usize;
        while !parent.is_null() && depth < 1024 {
            if parent == child {
                return true;
            }
            parent = (*parent)
                .window
                .parent
                .map(|pointer| pointer as *mut SurfaceRec)
                .unwrap_or(std::ptr::null_mut());
            depth += 1;
        }
        depth == 1024
    }
}

fn fresh_handle(
    existing: &std::collections::HashMap<String, *mut ffi::wl_resource>,
) -> Option<String> {
    for _ in 0..16 {
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).ok()?;
        let mut handle = String::with_capacity(32);
        for byte in bytes {
            write!(&mut handle, "{byte:02x}").expect("writing to a String cannot fail");
        }
        if !existing.contains_key(&handle) {
            return Some(handle);
        }
    }
    None
}

/// Revoke every export/relationship that points at a surface before the
/// surface record is reclaimed.
pub(crate) unsafe fn xdg_foreign_surface_destroyed(surface: *mut SurfaceRec, state: *mut State) {
    unsafe {
        if surface.is_null() || state.is_null() {
            return;
        }
        xdg_foreign_clear_child_parent(surface);
        // Defensive sweep: no import may retain a surface record that is
        // about to be reclaimed, even if a future parent path forgets to
        // maintain the bidirectional link.
        for imported in &(*state).xdg_foreign_imports {
            let record = ffi::wl_resource_get_user_data(*imported) as *mut ImportedRec;
            if !record.is_null() && (*record).child == surface {
                (*record).child = std::ptr::null_mut();
            }
        }
        let exports: Vec<_> = (*state)
            .xdg_foreign_exports
            .values()
            .copied()
            .filter(|resource| {
                let record = ffi::wl_resource_get_user_data(*resource) as *mut ExportedRec;
                !record.is_null() && (*record).surface == surface
            })
            .collect();
        for export in exports {
            ffi::wl_resource_destroy(export);
        }
        for child in (*state).live_surfaces() {
            if (*child).window.parent == Some(surface as usize) {
                if !(*child).foreign_parent_owner.is_null() {
                    let imported = ffi::wl_resource_get_user_data((*child).foreign_parent_owner)
                        as *mut ImportedRec;
                    if !imported.is_null() && (*imported).child == child {
                        (*imported).child = std::ptr::null_mut();
                    }
                }
                (*child).window.parent = None;
                (*child).foreign_parent_owner = std::ptr::null_mut();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn handles_are_128_bit_lowercase_hex_and_unique() {
        let mut existing = std::collections::HashMap::new();
        let first = fresh_handle(&existing).unwrap();
        existing.insert(first.clone(), std::ptr::null_mut());
        let second = fresh_handle(&existing).unwrap();
        assert_eq!(first.len(), 32);
        assert!(
            first
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert_ne!(first, second);
    }

    #[test]
    fn parent_cycle_detection_walks_existing_transient_links() {
        let mut root = Box::new(SurfaceRec::new(std::ptr::null_mut()));
        let mut child = Box::new(SurfaceRec::new(std::ptr::null_mut()));
        let root_ptr = root.as_mut() as *mut SurfaceRec;
        let child_ptr = child.as_mut() as *mut SurfaceRec;
        child.window.parent = Some(root_ptr as usize);

        assert!(unsafe { parent_chain_contains(child_ptr, root_ptr) });
        assert!(!unsafe { parent_chain_contains(root_ptr, child_ptr) });
    }
}
