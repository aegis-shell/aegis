use crate::*;

// ----- wl_output ----------------------------------------------------------

static OUTPUT_IMPL: ffi::wl_output_interface_impl = ffi::wl_output_interface_impl {
    release: res_destroy,
};

fn interaction_domain_output_info(
    interaction_domain: InteractionDomainId,
    output: VirtualOutput,
) -> aegis_model::output::OutputInfo {
    let physical = |logical: u32| {
        u64::from(logical)
            .saturating_mul(u64::from(output.scale_milli))
            .div_ceil(1000)
            .min(i32::MAX as u64) as i32
    };
    aegis_model::output::OutputInfo {
        connector: format!("interaction_domain-{}", interaction_domain.0),
        geometry: aegis_model::output::OutputGeometry {
            mode: aegis_model::output::OutputMode {
                width: physical(output.width),
                height: physical(output.height),
                refresh_mhz: output.refresh_mhz,
            },
            scale: aegis_model::output::Scale(output.scale_milli as f32 / 1000.0),
            transform: aegis_model::Transform::Normal,
            logical_origin: aegis_model::Point::default(),
        },
        available_modes: Vec::new(),
        color_caps: aegis_model::edid::EdidColorCapabilities::default(),
    }
}

pub(crate) fn output_interaction_domains_for_window(
    state: &State,
    window: aegis_model::window::WindowId,
) -> std::collections::BTreeSet<InteractionDomainId> {
    state
        .authority
        .interaction_group_for_window(window)
        .map(|group| {
            std::iter::once(group.control_interaction_domain)
                .chain(group.observer_interaction_domains.iter().copied())
                .collect()
        })
        .unwrap_or_else(|| std::iter::once(HUMAN_INTERACTION_DOMAIN).collect())
}

unsafe fn output_global_matches_interaction_domain(
    state: &State,
    global: *mut OutputGlobal,
    interaction_domain: InteractionDomainId,
) -> bool {
    unsafe {
        if global.is_null() || !(*global).active {
            return false;
        }
        if interaction_domain == HUMAN_INTERACTION_DOMAIN {
            (*global).interaction_domain.is_none()
                && state
                    .output_infos
                    .first()
                    .is_some_and(|primary| primary.connector == (*global).info.connector)
        } else {
            (*global).interaction_domain == Some(interaction_domain)
        }
    }
}

pub(crate) unsafe fn post_surface_output_event(
    state: &State,
    surface: *mut ffi::wl_resource,
    interaction_domain: InteractionDomainId,
    opcode: u32,
) {
    unsafe {
        if surface.is_null() {
            return;
        }
        let client = ffi::wl_resource_get_client(surface);
        for output in state.output_resources.iter().copied().filter(|output| {
            if output.is_null() || ffi::wl_resource_get_client(*output) != client {
                return false;
            }
            let global = ffi::wl_resource_get_user_data(*output) as *mut OutputGlobal;
            output_global_matches_interaction_domain(state, global, interaction_domain)
        }) {
            ffi::wl_resource_post_event(surface, opcode, output);
        }
    }
}

pub(crate) unsafe fn update_windows_output_membership(
    state: &State,
    windows: &[aegis_model::window::WindowId],
    before: &std::collections::BTreeSet<InteractionDomainId>,
    after: &std::collections::BTreeSet<InteractionDomainId>,
) {
    unsafe {
        for pointer in state.live_surfaces() {
            let root = surface_root_toplevel(pointer);
            if root.is_null() || !windows.contains(&(*root).window.id) || !(*pointer).mapped {
                continue;
            }
            for interaction_domain in before.difference(after) {
                post_surface_output_event(
                    state,
                    (*pointer).resource,
                    *interaction_domain,
                    ffi::WL_SURFACE_LEAVE,
                );
            }
            for interaction_domain in after.difference(before) {
                post_surface_output_event(
                    state,
                    (*pointer).resource,
                    *interaction_domain,
                    ffi::WL_SURFACE_ENTER,
                );
            }
        }
    }
}

pub(crate) unsafe fn create_interaction_domain_output_global(
    state: &mut State,
    interaction_domain: InteractionDomainId,
    output: VirtualOutput,
) -> bool {
    unsafe {
        create_output_global(
            state,
            interaction_domain_output_info(interaction_domain, output),
            Some(interaction_domain),
        )
    }
}

pub(crate) unsafe fn update_interaction_domain_output_global(
    state: &mut State,
    interaction_domain: InteractionDomainId,
    output: VirtualOutput,
) {
    unsafe {
        let info = interaction_domain_output_info(interaction_domain, output);
        let mut found = false;
        for global in &mut state.output_globals {
            if global.interaction_domain == Some(interaction_domain) && global.active {
                global.info = info.clone();
                found = true;
            }
        }
        if !found {
            create_interaction_domain_output_global(state, interaction_domain, output);
        }
        for resource in state.output_resources.iter().copied().filter(|resource| {
            if resource.is_null() {
                return false;
            }
            let global = ffi::wl_resource_get_user_data(*resource) as *mut OutputGlobal;
            !global.is_null() && (*global).interaction_domain == Some(interaction_domain)
        }) {
            send_output_geometry(resource);
        }
    }
}

pub(crate) unsafe fn create_output_global(
    state: &mut State,
    info: aegis_model::output::OutputInfo,
    interaction_domain: Option<InteractionDomainId>,
) -> bool {
    unsafe {
        let mut record = Box::new(OutputGlobal {
            state: state as *mut State,
            info,
            interaction_domain,
            global: std::ptr::null_mut(),
            active: true,
        });
        let data = record.as_mut() as *mut OutputGlobal as *mut c_void;
        record.global = ffi::wl_global_create(
            state.display,
            &ffi::wl_output_interface,
            4,
            data,
            output_bind,
        );
        if record.global.is_null() {
            log::error!("[server] wl_output global creation failed");
            record.active = false;
        }
        let created = record.active;
        state.output_globals.push(record);
        created
    }
}

pub(crate) unsafe fn reconcile_output_globals(
    state: &mut State,
    outputs: &[aegis_model::output::OutputInfo],
) {
    unsafe {
        for global in &mut state.output_globals {
            if !global.active || global.interaction_domain.is_some() {
                continue;
            }
            if let Some(info) = outputs
                .iter()
                .find(|output| output.connector == global.info.connector)
            {
                global.info = info.clone();
            } else {
                ffi::wl_global_destroy(global.global);
                global.global = std::ptr::null_mut();
                global.active = false;
            }
        }
        for output in outputs {
            let exists = state.output_globals.iter().any(|global| {
                global.active
                    && global.interaction_domain.is_none()
                    && global.info.connector == output.connector
            });
            if !exists {
                create_output_global(state, output.clone(), None);
            }
        }
    }
}

pub(crate) unsafe fn output_info_for_resource(
    resource: *mut ffi::wl_resource,
) -> Option<aegis_model::output::OutputInfo> {
    unsafe {
        if resource.is_null() {
            return None;
        }
        let global = ffi::wl_resource_get_user_data(resource) as *mut OutputGlobal;
        (!global.is_null()).then(|| (*global).info.clone())
    }
}

/// Compute the (mode, integer-scale, transform) tuple from the state's
/// output record, with a sane default before the first backend update.
unsafe fn output_params(global: *mut OutputGlobal) -> (aegis_model::output::OutputMode, i32, i32) {
    unsafe {
        let mut mode = aegis_model::output::OutputMode {
            width: 1280,
            height: 720,
            refresh_mhz: 60000,
        };
        let mut scale_i = 1i32;
        let mut transform = 0i32;
        if !global.is_null() {
            let g = (*global).info.geometry;
            if g.mode.width > 0 && g.mode.height > 0 {
                mode = g.mode;
            }
            scale_i = integer_output_scale(g.scale.0);
            transform = g.transform as i32;
        }
        (mode, scale_i, transform)
    }
}

/// Legacy `wl_output.scale` is integer-only. Round upward so clients without
/// fractional-scale support render enough pixels for the compositor to
/// downsample instead of rendering at 1x and being blurred by upsampling.
pub(crate) fn integer_output_scale(scale: f32) -> i32 {
    scale.ceil().max(1.0) as i32
}

/// Post the full geometry + mode + scale + done sequence to one wl_output
/// resource. Version-gated: scale/done require v2.
pub(crate) unsafe fn send_output_geometry(res: *mut ffi::wl_resource) {
    unsafe {
        let global = ffi::wl_resource_get_user_data(res) as *mut OutputGlobal;
        let (mode, scale_i, transform) = output_params(global);
        let version = ffi::wl_resource_get_version(res);
        let make = CString::new("aegis").unwrap();
        let (origin, model_name) = if global.is_null() {
            (aegis_model::Point::default(), "unknown")
        } else {
            (
                (*global).info.geometry.logical_origin,
                (*global).info.connector.as_str(),
            )
        };
        let model = CString::new(model_name).unwrap_or_else(|_| CString::new("output").unwrap());
        ffi::wl_resource_post_event(
            res,
            ffi::WL_OUTPUT_GEOMETRY,
            origin.x,
            origin.y,
            300i32,
            200i32,
            0i32,
            make.as_ptr(),
            model.as_ptr(),
            transform,
        );
        ffi::wl_resource_post_event(
            res,
            ffi::WL_OUTPUT_MODE,
            ffi::WL_OUTPUT_MODE_CURRENT,
            mode.width,
            mode.height,
            mode.refresh_mhz as i32,
        );
        if version >= 2 {
            ffi::wl_resource_post_event(res, ffi::WL_OUTPUT_SCALE, scale_i);
        }
        if version >= 4 {
            let name = CString::new((*global).info.connector.as_str())
                .unwrap_or_else(|_| CString::new("unknown").unwrap());
            let description = CString::new(format!("aegis output {}", (*global).info.connector))
                .unwrap_or_else(|_| CString::new("aegis output").unwrap());
            ffi::wl_resource_post_event(res, ffi::WL_OUTPUT_NAME, name.as_ptr());
            ffi::wl_resource_post_event(res, ffi::WL_OUTPUT_DESCRIPTION, description.as_ptr());
        }
        if version >= 2 {
            ffi::wl_resource_post_event(res, ffi::WL_OUTPUT_DONE);
        }
    }
}

unsafe extern "C" fn output_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let global = ffi::wl_resource_get_user_data(resource) as *mut OutputGlobal;
        if global.is_null() || (*global).state.is_null() {
            return;
        }
        let state = (*global).state;
        if let Some(pos) = (*state)
            .output_resources
            .iter()
            .position(|p| *p == resource)
        {
            (*state).output_resources.remove(pos);
        }
    }
}

unsafe extern "C" fn output_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let res = ffi::wl_resource_create(client, &ffi::wl_output_interface, version as c_int, id);
        if res.is_null() {
            return;
        }
        let global = data as *mut OutputGlobal;
        ffi::wl_resource_set_implementation(
            res,
            &OUTPUT_IMPL as *const _ as *const c_void,
            global as *mut c_void,
            Some(output_resource_destroy),
        );
        if !global.is_null() && !(*global).state.is_null() {
            let state = (*global).state;
            (*state).output_resources.push(res);
        }

        send_output_geometry(res);
        if !global.is_null() && !(*global).state.is_null() {
            let state = &*(*global).state;
            for pointer in state.live_surfaces() {
                if !(*pointer).mapped || ffi::wl_resource_get_client((*pointer).resource) != client
                {
                    continue;
                }
                let root = surface_root_toplevel(pointer);
                if root.is_null() {
                    continue;
                }
                if output_interaction_domains_for_window(state, (*root).window.id)
                    .into_iter()
                    .any(|interaction_domain| {
                        output_global_matches_interaction_domain(state, global, interaction_domain)
                    })
                {
                    ffi::wl_resource_post_event((*pointer).resource, ffi::WL_SURFACE_ENTER, res);
                }
            }
        }
    }
}
