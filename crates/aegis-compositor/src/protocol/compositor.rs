use crate::*;

// ----- wl_compositor ------------------------------------------------------

static COMPOSITOR_IMPL: ffi::wl_compositor_interface_impl = ffi::wl_compositor_interface_impl {
    create_surface: compositor_create_surface,
    create_region: compositor_create_region,
};

pub(crate) unsafe extern "C" fn compositor_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let state = data as *mut State;
        if state.is_null() {
            return;
        }
        (*state).ensure_client(client);
        let res =
            ffi::wl_resource_create(client, &ffi::wl_compositor_interface, version as c_int, id);
        if res.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            res,
            &COMPOSITOR_IMPL as *const _ as *const c_void,
            data,
            None,
        );
    }
}

unsafe extern "C" fn compositor_create_surface(
    client: *mut ffi::wl_client,
    compositor: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(compositor) as *mut State;
        let ver = ffi::wl_resource_get_version(compositor);
        let surface = ffi::wl_resource_create(client, &ffi::wl_surface_interface, ver, id);
        if surface.is_null() {
            return;
        }
        let rec = Box::into_raw(Box::new(SurfaceRec::new(surface)));
        (*rec).state = state;
        (*rec).client_id = (*state).ensure_client(client);
        (*rec).index = (*state).surfaces.len();
        ffi::wl_resource_set_implementation(
            surface,
            &SURFACE_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            Some(surface_resource_destroy),
        );
        (*state).surfaces.push(rec);
    }
}

unsafe extern "C" fn compositor_create_region(
    client: *mut ffi::wl_client,
    compositor: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let ver = ffi::wl_resource_get_version(compositor);
        let region = ffi::wl_resource_create(client, &ffi::wl_region_interface, ver, id);
        if region.is_null() {
            return;
        }
        ffi::wl_resource_set_implementation(
            region,
            &REGION_IMPL as *const _ as *const c_void,
            Box::into_raw(Box::new(RegionRec::default())) as *mut c_void,
            Some(region_resource_destroy),
        );
    }
}

// ----- wl_surface ---------------------------------------------------------

static SURFACE_IMPL: ffi::wl_surface_interface_impl = ffi::wl_surface_interface_impl {
    destroy: surface_destroy,
    attach: surface_attach,
    damage: surface_damage,
    frame: surface_frame,
    set_opaque_region: surface_noop_region,
    set_input_region: surface_set_input_region,
    commit: surface_commit,
    set_buffer_transform: surface_set_buffer_transform,
    set_buffer_scale: surface_set_buffer_scale,
    damage_buffer: surface_damage_buffer,
};

/// Merge one commit's damage into the not-yet-presented surface damage.
/// Keep a single bounding box: KMS consumes a conservative box and the
/// renderer's row upload is cheaper with one bounded region than with an
/// unbounded list after a burst of commits.
pub(crate) fn accumulate_committed_damage(
    rec: &mut SurfaceRec,
    pending: Vec<aegis_core::Rect>,
    unknown_full: bool,
) {
    if rec.committed_damage_full {
        return;
    }
    if unknown_full {
        rec.committed_damage.clear();
        rec.committed_damage_full = true;
        return;
    }
    let mut bbox = rec.committed_damage.first().copied();
    for rect in rec
        .committed_damage
        .iter()
        .skip(1)
        .chain(pending.iter())
        .copied()
    {
        bbox = Some(match bbox {
            Some(old) => {
                let x0 = old.origin.x.min(rect.origin.x);
                let y0 = old.origin.y.min(rect.origin.y);
                let x1 = old
                    .origin
                    .x
                    .saturating_add(old.size.w)
                    .max(rect.origin.x.saturating_add(rect.size.w));
                let y1 = old
                    .origin
                    .y
                    .saturating_add(old.size.h)
                    .max(rect.origin.y.saturating_add(rect.size.h));
                aegis_core::Rect::new(x0, y0, x1.saturating_sub(x0), y1.saturating_sub(y0))
            }
            None => rect,
        });
    }
    rec.committed_damage.clear();
    if let Some(rect) = bbox {
        rec.committed_damage.push(rect);
    }
}

pub(crate) fn reset_xdg_configure_state_after_unmap(rec: &mut SurfaceRec) {
    rec.xdg_configured = false;
    rec.xdg_configure_acked = false;
    rec.pending_xdg_configures.clear();
}

unsafe extern "C" fn surface_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn surface_attach(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    buffer: *mut ffi::wl_resource,
    x: i32,
    y: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        (*rec).pending_buffer = buffer;
        (*rec).pending_buffer_set = true;
        (*rec).pending_attach_offset = aegis_core::Point { x, y };
    }
}

unsafe fn retire_surface_buffer(rec: *mut SurfaceRec) {
    unsafe {
        if rec.is_null() || (*rec).state.is_null() {
            return;
        }
        let buffer = std::mem::replace(&mut (*rec).current_buffer, std::ptr::null_mut());
        let explicit_release =
            std::mem::replace(&mut (*rec).current_explicit_release, std::ptr::null_mut());
        if !buffer.is_null() || !explicit_release.is_null() {
            (*(*rec).state)
                .retired_buffer_releases
                .push(RetiredBufferRelease {
                    buffer,
                    explicit_release,
                });
        }
    }
}

fn valid_policy_size(size: aegis_core::Size) -> Option<aegis_core::Size> {
    (size.w >= 100 && size.h >= 100).then_some(size)
}

/// Size to advertise in the first toplevel configure.
///
/// At the initial no-buffer commit, xdg-shell metadata (including
/// `set_parent`) is complete. This is the first safe point to restore a
/// main-window size: doing it from `set_app_id` races the transient
/// relationship, while doing it after the first buffer maps visibly resizes
/// the window.
pub(crate) unsafe fn initial_toplevel_size(rec: *mut SurfaceRec) -> Option<aegis_core::Size> {
    unsafe {
        if rec.is_null() || (*rec).state.is_null() {
            return None;
        }
        if (*rec).window.state.maximized || (*rec).window.state.fullscreen {
            return valid_policy_size((*rec).window.size);
        }
        let state = &*(*rec).state;
        let rule = state
            .window_rules
            .iter()
            .find(|rule| {
                rule.matches(
                    (*rec).window.app_id.as_deref(),
                    (*rec).window.title.as_deref(),
                )
            })
            .cloned();
        if let Some(size) = rule.as_ref().and_then(|rule| rule.size) {
            return valid_policy_size(size);
        }
        // Application-level remembered state belongs to primary windows,
        // never to an xdg_toplevel transient. Dialogs commonly share the
        // exact app_id of their parent.
        if (*rec).window.parent.is_some()
            || !state.remember_window_positions
            || !rule.as_ref().and_then(|rule| rule.remember).unwrap_or(true)
        {
            return None;
        }
        (*rec)
            .window
            .app_id
            .as_deref()
            .and_then(|app_id| state.window_state_store.get(app_id))
            .and_then(|saved| saved.size)
            .and_then(valid_policy_size)
    }
}

/// Center a transient in its parent and keep it inside the output whenever
/// the transient fits. Both rectangles use compositor-logical coordinates.
pub(crate) fn centered_transient_position(
    parent: aegis_core::Rect,
    child: aegis_core::Size,
    output: aegis_core::Rect,
) -> aegis_core::Point {
    let centered = aegis_core::Point {
        x: parent.origin.x + (parent.size.w - child.w) / 2,
        y: parent.origin.y + (parent.size.h - child.h) / 2,
    };
    let max_x = output
        .origin
        .x
        .saturating_add(output.size.w)
        .saturating_sub(child.w)
        .max(output.origin.x);
    let max_y = output
        .origin
        .y
        .saturating_add(output.size.h)
        .saturating_sub(child.h)
        .max(output.origin.y);
    aegis_core::Point {
        x: centered.x.clamp(output.origin.x, max_x),
        y: centered.y.clamp(output.origin.y, max_y),
    }
}

unsafe fn transient_parent_rect(rec: *mut SurfaceRec) -> Option<aegis_core::Rect> {
    unsafe {
        let parent = (*rec).window.parent? as *mut SurfaceRec;
        if parent.is_null() || (*rec).state.is_null() {
            return None;
        }
        // Verify that the protocol-object pointer still names a live surface
        // before dereferencing it; a parent may be destroyed first.
        let live = (*(*rec).state)
            .live_surfaces()
            .any(|candidate| candidate == parent);
        if !live || !(*parent).mapped || (*parent).xdg_toplevel.is_null() {
            return None;
        }
        Some(aegis_core::Rect {
            origin: (*parent).position,
            size: (*parent).window.size,
        })
    }
}

pub(crate) fn should_focus_mapped_toplevel(
    visible: bool,
    human_controls: bool,
    minimized: bool,
) -> bool {
    visible && human_controls && !minimized
}

pub(crate) unsafe extern "C" fn surface_commit(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if !(*rec).parent.is_null() && (*rec).subsurface_sync && !(*rec).subsurface_applying_cached
        {
            (*rec).subsurface_cached_commit = true;
            return;
        }
        (*rec).subsurface_cached_commit = false;
        let visual_metadata_changed = (*rec).pending_window_geometry.is_some()
            || (*rec).pending_viewport_src.is_some()
            || (*rec).pending_viewport_dst.is_some()
            || (*rec).pending_transform != (*rec).buffer_transform
            || (*rec).pending_scale != (*rec).buffer_scale
            || !(*rec).pending_damage.is_empty();
        let old_window_size = (*rec).window.size;
        if let Some(region) = (*rec).pending_input_region.take() {
            (*rec).input_region = region;
        }
        if let Some(geometry) = (*rec).pending_window_geometry.take() {
            (*rec).window_geometry = Some(geometry);
        }
        if let Some(source) = (*rec).pending_viewport_src.take() {
            (*rec).viewport_src = source;
        }
        if let Some(destination) = (*rec).pending_viewport_dst.take() {
            (*rec).viewport_dst = destination;
        }
        (*rec).buffer_transform = (*rec).pending_transform;
        (*rec).buffer_scale = (*rec).pending_scale;

        // xdg-shell initial configure: on the first commit of a surface that has an
        // xdg role, send a configure and wait for the client to ack and attach a
        // buffer. The initial commit carries no buffer, so mapping happens on a
        // later commit.
        if !(*rec).xdg_surface.is_null() && !(*rec).xdg_configured {
            if !(*rec).xdg_toplevel.is_null() {
                let initial_size = initial_toplevel_size(rec);
                if let Some(size) = initial_size {
                    (*rec).window.size = size;
                }
                let mut states = ffi::wl_array::empty();
                ffi::wl_resource_post_event(
                    (*rec).xdg_toplevel,
                    ffi::XDG_TOPLEVEL_CONFIGURE,
                    initial_size.map(|size| size.w).unwrap_or(0),
                    initial_size.map(|size| size.h).unwrap_or(0),
                    &mut states as *mut ffi::wl_array,
                );
            }
            send_xdg_surface_configure(rec);
            (*rec).xdg_configured = true;
        }

        let was_mapped = (*rec).mapped;
        let buffer = (*rec).pending_buffer;
        let buffer_set = std::mem::take(&mut (*rec).pending_buffer_set);
        let scene_changed = buffer_set || visual_metadata_changed;
        if !extensions::explicit_sync_surface_committed(rec, buffer_set, buffer) {
            return;
        }
        if buffer_set
            && !buffer.is_null()
            && !(*rec).xdg_surface.is_null()
            && !(*rec).xdg_configure_acked
        {
            ffi::wl_resource_post_error(
                (*rec).xdg_surface,
                3,
                c"buffer committed before the initial xdg_surface.configure was acknowledged"
                    .as_ptr(),
            );
            return;
        }
        if buffer_set {
            (*rec).attach_offset = (*rec).pending_attach_offset;
        }
        // The pending transform and scale are surfaced to the renderer via
        // `SurfaceGeometry` (see toplevel_*_frames below); the renderer applies
        // them at composite time.
        // Accumulate until a compositor frame is actually presented. The
        // event loop can dispatch several commits before rendering once; only
        // retaining the newest commit's damage would leave earlier pixels
        // stale in the retained shm snapshot/GPU texture and under-report the
        // KMS damage hint. Empty damage on a new buffer carries no usable
        // information and therefore poisons the aggregate to full.
        let pending_damage = std::mem::take(&mut (*rec).pending_damage);
        let unknown_full = buffer_set && !buffer.is_null() && pending_damage.is_empty();
        accumulate_committed_damage(&mut *rec, pending_damage, unknown_full);
        if buffer_set && buffer.is_null() {
            retire_surface_buffer(rec);
            (*rec).dmabuf = None;
            (*rec).mapped = false;
            if was_mapped && !(*rec).xdg_surface.is_null() {
                reset_xdg_configure_state_after_unmap(&mut *rec);
            }
            (*rec).pixels.clear();
            (*rec).content_is_dmabuf = false;
            (*rec).generation = (*rec).generation.wrapping_add(1);
        } else if !buffer.is_null() {
            let is_dmabuf = ffi::wl_resource_instance_of(
                buffer,
                &ffi::wl_buffer_interface,
                &WL_BUFFER_IMPL as *const _ as *const c_void,
            ) != 0;

            if is_dmabuf {
                // Duplicate the dma-buf fd into compositor ownership before
                // releasing the client wl_buffer. Clients are then free to destroy
                // that protocol object without invalidating the surface contents.
                let db = ffi::wl_resource_get_user_data(buffer) as *const DmabufBuffer;
                if !db.is_null()
                    && (*db).have_plane
                    && let Some(owned) = (*db).duplicate()
                {
                    retire_surface_buffer(rec);
                    let mut owned = owned;
                    if (*rec).committed_acquire_fence >= 0 {
                        if owned.acquire_fence >= 0 {
                            libc_close(owned.acquire_fence);
                        }
                        owned.acquire_fence =
                            std::mem::replace(&mut (*rec).committed_acquire_fence, -1);
                    }
                    (*rec).width = owned.width;
                    (*rec).height = owned.height;
                    (*rec).dmabuf = Some(owned);
                    // Invalidate the shm snapshot: if a later commit returns
                    // to shm, its incremental-copy size check must fail so
                    // the new frame is copied in full rather than blended
                    // into these stale pixels.
                    (*rec).pixels.clear();
                    (*rec).content_is_dmabuf = true;
                    (*rec).generation = (*rec).generation.wrapping_add(1);
                    (*rec).mapped = true;
                    (*rec).current_buffer = buffer;
                    (*rec).current_explicit_release = std::mem::replace(
                        &mut (*rec).committed_explicit_release,
                        std::ptr::null_mut(),
                    );
                }
                (*rec).pending_buffer = std::ptr::null_mut();
            } else {
                // shm: copy the contents out into our own tightly packed BGRA store
                // and release the buffer immediately so the client can reuse it.
                //
                // The copy is damage-driven: for a same-size frame with usable
                // damage the protocol guarantees the new buffer differs from the
                // previous frame only inside the damaged region, so copying the
                // damaged rows onto the retained snapshot yields the new frame
                // without a full-buffer memcpy (and without the per-commit
                // allocation — the snapshot Vec is reused while its size holds).
                // Empty damage carries no information and forces a full copy, as
                // do a transform or buffer scale (damage is surface-local and
                // would not map 1:1 onto buffer pixels). The guard mirrors the
                // renderer's incremental-upload guard exactly; the two paths
                // must always agree or the texture would tear.
                let shm = ffi::wl_shm_buffer_get(buffer);
                if !shm.is_null() {
                    let w = ffi::wl_shm_buffer_get_width(shm);
                    let h = ffi::wl_shm_buffer_get_height(shm);
                    let stride = ffi::wl_shm_buffer_get_stride(shm) as usize;
                    let format = ffi::wl_shm_buffer_get_format(shm);
                    let src = ffi::wl_shm_buffer_get_data(shm) as *const u8;
                    if !src.is_null() && w > 0 && h > 0 {
                        let tight = (w as usize) * 4;
                        let needed = tight * h as usize;
                        let incremental = (*rec).width == w
                            && (*rec).height == h
                            && (*rec).pixels.len() == needed
                            && (*rec).buffer_transform == aegis_core::Transform::Normal
                            && (*rec).buffer_scale <= 1
                            && !(*rec).committed_damage.is_empty();
                        if (*rec).pixels.len() != needed {
                            (*rec).pixels = vec![0u8; needed];
                        }
                        // Read the damage list locally: the copy mutates
                        // `pixels` while the rects are consulted.
                        let damage = if incremental {
                            std::mem::take(&mut (*rec).committed_damage)
                        } else {
                            Vec::new()
                        };
                        ffi::wl_shm_buffer_begin_access(shm);
                        // One explicit mutable borrow for the whole copy: raw
                        // pointer field indexing would implicitly autoref each
                        // access.
                        let pixels = &mut (*rec).pixels;
                        if incremental {
                            for d in &damage {
                                let x = d.origin.x.max(0).min(w - 1) as usize;
                                let y = d.origin.y.max(0).min(h - 1) as usize;
                                let cw = (d.size.w.max(0)).min(w - x as i32) as usize;
                                let ch = (d.size.h.max(0)).min(h - y as i32) as usize;
                                if cw == 0 || ch == 0 {
                                    continue;
                                }
                                for row in 0..ch {
                                    std::ptr::copy_nonoverlapping(
                                        src.add((y + row) * stride + x * 4),
                                        pixels.as_mut_ptr().add((y + row) * tight + x * 4),
                                        cw * 4,
                                    );
                                }
                                // XRGB8888 has undefined alpha; force opaque on
                                // the refreshed rows.
                                if format == 1 {
                                    for row in 0..ch {
                                        let base = (y + row) * tight + x * 4;
                                        for px in 0..cw {
                                            pixels[base + px * 4 + 3] = 0xff;
                                        }
                                    }
                                }
                            }
                            (*rec).committed_damage = damage;
                        } else {
                            for row in 0..h as usize {
                                std::ptr::copy_nonoverlapping(
                                    src.add(row * stride),
                                    pixels.as_mut_ptr().add(row * tight),
                                    tight,
                                );
                            }
                            // XRGB8888 has undefined alpha; force opaque.
                            if format == 1 {
                                let mut i = 3;
                                while i < needed {
                                    pixels[i] = 0xff;
                                    i += 4;
                                }
                            }
                        }
                        ffi::wl_shm_buffer_end_access(shm);
                        retire_surface_buffer(rec);
                        (*rec).dmabuf = None;
                        (*rec).content_is_dmabuf = false;
                        (*rec).width = w;
                        (*rec).height = h;
                        (*rec).generation = (*rec).generation.wrapping_add(1);
                        (*rec).mapped = true;
                    }
                }
                ffi::wl_resource_post_event(buffer, ffi::WL_BUFFER_RELEASE);
                if !(*rec).committed_explicit_release.is_null() {
                    let release = std::mem::replace(
                        &mut (*rec).committed_explicit_release,
                        std::ptr::null_mut(),
                    );
                    ffi::wl_resource_post_event(
                        release,
                        ffi::ZWP_LINUX_BUFFER_RELEASE_V1_IMMEDIATE_RELEASE,
                    );
                    ffi::wl_resource_destroy(release);
                }
                (*rec).pending_buffer = std::ptr::null_mut();
            }
        }

        // Buffer transform/scale/viewport/window-geometry commits alter the
        // composed pixels even when no new wl_buffer is attached. Give the
        // render/damage generation tracker an edge to observe; its geometry
        // guard conservatively promotes these cases to full damage.
        if visual_metadata_changed && !buffer_set {
            (*rec).generation = (*rec).generation.wrapping_add(1);
        }

        if (*rec).mapped && !was_mapped && !(*rec).xdg_toplevel.is_null() {
            let app_id = (*rec).window.app_id.clone();
            let title = (*rec).window.title.clone();
            let rule = if !(*rec).state.is_null() {
                let st = &mut *(*rec).state;
                st.window_rules
                    .iter()
                    .find(|r| r.matches(app_id.as_deref(), title.as_deref()))
                    .cloned()
            } else {
                None
            };

            let rule_pos = rule.as_ref().and_then(|r| r.position);
            let rule_size = rule.as_ref().and_then(|r| r.size);
            let rule_remember = rule.as_ref().and_then(|r| r.remember);
            let is_transient = (*rec).window.parent.is_some();

            let allow_remember = !is_transient
                && rule_remember.unwrap_or(true)
                && if !(*rec).state.is_null() {
                    (*(*rec).state).remember_window_positions
                } else {
                    true
                };

            let remembered_store_entry = if allow_remember && !(*rec).state.is_null() {
                app_id
                    .as_deref()
                    .and_then(|id| (*(*rec).state).window_state_store.get(id).cloned())
            } else {
                None
            };
            let last_app_rect = if allow_remember {
                app_id.as_deref().and_then(|id| {
                    if !(*rec).state.is_null() {
                        (*(*rec).state).last_app_geometries.get(id).copied()
                    } else {
                        None
                    }
                })
            } else {
                None
            };
            let parent_rect = transient_parent_rect(rec);

            let target_pos =
                rule_pos.or_else(|| remembered_store_entry.as_ref().and_then(|s| s.position));
            if let Some(pos) = target_pos {
                let output = if !(*rec).state.is_null() {
                    (*(*rec).state).output_geometry.logical_rect()
                } else {
                    aegis_core::Rect::new(0, 0, 1920, 1080)
                };
                let max_x = output
                    .origin
                    .x
                    .saturating_add(output.size.w)
                    .saturating_sub(100)
                    .max(output.origin.x);
                let max_y = output
                    .origin
                    .y
                    .saturating_add(output.size.h)
                    .saturating_sub(100)
                    .max(output.origin.y);
                let clamped_pos = aegis_core::Point {
                    x: pos.x.clamp(output.origin.x, max_x),
                    y: pos.y.clamp(output.origin.y, max_y),
                };
                (*rec).position = clamped_pos;
                (*rec).window.position = clamped_pos;
            } else if let Some(rect) = last_app_rect {
                (*rec).position = rect.origin;
                (*rec).window.position = rect.origin;
            } else if parent_rect.is_none() {
                let count = if (*rec).state.is_null() {
                    0
                } else {
                    (*(*rec).state)
                        .live_surfaces()
                        .filter(|p| !(**p).xdg_toplevel.is_null() && (**p).mapped)
                        .count()
                };
                let idx = count.min(8) as i32;
                (*rec).position = aegis_core::Point {
                    x: 60 + idx * 32,
                    y: 60 + idx * 32,
                };
                (*rec).window.position = (*rec).position;
            }

            let target_size =
                rule_size.or_else(|| remembered_store_entry.as_ref().and_then(|s| s.size));
            let mapped_size = (*rec)
                .window_geometry
                .map(|geometry| geometry.size)
                .unwrap_or_else(|| surface_logical_size(&*rec));
            if let Some(size) = target_size.and_then(valid_policy_size) {
                (*rec).window.size = size;
                // Normally this was already advertised in the initial
                // configure. Only correct a client that mapped a different
                // size; do not send the redundant post-map configure that
                // made windows visibly "open, then shrink".
                if mapped_size != size {
                    reconfigure_with_size(rec, size.w, size.h);
                }
            } else {
                (*rec).window.size = mapped_size;
            }

            if rule_pos.is_none()
                && let Some(parent) = parent_rect
            {
                let output = if !(*rec).state.is_null() {
                    (*(*rec).state).output_geometry.logical_rect()
                } else {
                    aegis_core::Rect::new(0, 0, 1920, 1080)
                };
                let centered = centered_transient_position(parent, (*rec).window.size, output);
                (*rec).position = centered;
                (*rec).window.position = centered;
            }

            log::info!(
                "[server] surface mapped at {:?}: {}x{}",
                (*rec).position,
                (*rec).width,
                (*rec).height
            );

            if !(*rec).state.is_null() {
                let id = (*rec).window.id;
                let st = &mut *(*rec).state;
                if let Some(wid) = st.workspaces.current_workspace(st.output) {
                    st.workspaces.place_toplevel(wid, id);
                }
                let target_workspace = rule.as_ref().and_then(|r| r.workspace).or_else(|| {
                    if allow_remember {
                        remembered_store_entry.as_ref().and_then(|s| s.workspace)
                    } else {
                        None
                    }
                });
                if let Some(ws_idx1) = target_workspace {
                    let idx = (ws_idx1 as usize).saturating_sub(1);
                    if let Some(o) = st.workspaces.output(st.output)
                        && let Some(&target) = o.workspaces.get(idx)
                    {
                        st.workspaces.move_toplevel(id, target);
                    }
                }

                let rule_role = rule.as_ref().and_then(|r| r.role).or_else(|| {
                    if allow_remember {
                        remembered_store_entry.as_ref().and_then(|s| s.layout_role)
                    } else {
                        None
                    }
                });

                let workspace_tiled = st
                    .workspaces
                    .workspace_of(id)
                    .and_then(|wid| st.workspaces.workspace(wid))
                    .map(|ws| ws.tiled)
                    .unwrap_or(false);
                (*rec).window.layout_role =
                    resolve_layout_role(workspace_tiled, (*rec).window.parent.is_some(), rule_role);
            }
            // Live-update the foreign-toplevel list so taskbars see the new window.
            if !(*rec).state.is_null() {
                extensions::foreign_toplevel_added(rec, (*rec).state);
                let state = &mut *(*rec).state;
                let id = (*rec).window.id;
                let visible = state.workspaces.visible_toplevels().contains(&id);
                let human_controls = state.authority.seat_controls_window(HUMAN_SEAT, id);
                // Mapping happens inside libwayland's dispatch callback, where
                // constructing a second mutable `Server` would alias state.
                // Use the same deferred handoff as xdg-activation; `dispatch`
                // applies it after protocol callbacks finish.
                if state.pending_activation.is_none()
                    && should_focus_mapped_toplevel(
                        visible,
                        human_controls,
                        (*rec).window.minimized,
                    )
                {
                    state.pending_activation = Some((HUMAN_SEAT, (*rec).resource));
                }
            }
        } else if (*rec).mapped && !(*rec).xdg_toplevel.is_null() {
            (*rec).window.size = (*rec)
                .window_geometry
                .map(|geometry| geometry.size)
                .unwrap_or_else(|| surface_logical_size(&*rec));
        }
        if !(*rec).state.is_null() && !(*rec).xdg_toplevel.is_null() {
            let window = (*rec).window.id;
            if was_mapped != (*rec).mapped || old_window_size != (*rec).window.size {
                (*(*rec).state).queue_realm_layouts_for_window(window);
            }
        }
        if !(*rec).mapped && was_mapped && !(*rec).xdg_popup.is_null() && (*rec).popup_grabbed {
            let focus_after_dismissal = popup_keyboard_focus_after_dismissal(rec);
            if let Some(seat) = (*rec).popup_grab_seat.take()
                && !(*rec).state.is_null()
            {
                (*(*rec).state)
                    .pending_popup_focus
                    .insert(seat, focus_after_dismissal);
            }
            (*rec).popup_grabbed = false;
        }
        if (*rec).mapped
            && !was_mapped
            && !(*rec).xdg_popup.is_null()
            && (*rec).popup_grabbed
            && let Some(seat) = (*rec).popup_grab_seat
            && !(*rec).state.is_null()
        {
            // The xdg-shell grab contract requires the topmost grabbing popup
            // to hold keyboard focus. Defer out of this libwayland callback so
            // no second mutable `Server` aliases `State`.
            (*(*rec).state)
                .pending_popup_focus
                .insert(seat, (*rec).resource);
        }
        if (*rec).mapped && !was_mapped && !(*rec).state.is_null() {
            let root = surface_root_toplevel(rec);
            if !root.is_null() {
                let state = &*(*rec).state;
                for realm in output_realms_for_window(state, (*root).window.id) {
                    post_surface_output_event(state, (*rec).resource, realm, ffi::WL_SURFACE_ENTER);
                }
            }
        } else if !(*rec).mapped && was_mapped && !(*rec).state.is_null() {
            let root = surface_root_toplevel(rec);
            if !root.is_null() {
                let state = &*(*rec).state;
                for realm in output_realms_for_window(state, (*root).window.id) {
                    post_surface_output_event(state, (*rec).resource, realm, ffi::WL_SURFACE_LEAVE);
                }
            }
        }
        let children = (*rec).children.clone();
        for child in children {
            if child.is_null() || !(*child).subsurface_cached_commit {
                continue;
            }
            (*child).subsurface_applying_cached = true;
            surface_commit(std::ptr::null_mut(), (*child).resource);
            (*child).subsurface_applying_cached = false;
        }
        if !(*rec).state.is_null() && ((*rec).cursor_role || (*rec).drag_icon_role) {
            update_overlay_positions((*rec).state);
        }
        extensions::input_popup_surface_committed(rec);
        if scene_changed && (was_mapped || (*rec).mapped) && !(*rec).state.is_null() {
            let root = surface_root_toplevel(rec);
            if !root.is_null() && (*root).window.id.0 != 0 {
                (*(*rec).state).damaged_windows.insert((*root).window.id);
            }
        }
        extensions::session_lock_surface_committed(rec);
    }
}

unsafe extern "C" fn surface_frame(
    client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    callback_id: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        let cb = ffi::wl_resource_create(client, &ffi::wl_callback_interface, 1, callback_id);
        if !cb.is_null() {
            (*rec).frame_callbacks.push(cb);
        }
    }
}

unsafe extern "C" fn surface_noop_region(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _reg: *mut ffi::wl_resource,
) {
}

unsafe extern "C" fn surface_set_input_region(
    _client: *mut ffi::wl_client,
    surface: *mut ffi::wl_resource,
    region: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        let value = if region.is_null() {
            None
        } else {
            let region = ffi::wl_resource_get_user_data(region) as *mut RegionRec;
            if region.is_null() {
                Some(Vec::new())
            } else {
                Some((*region).rects.clone())
            }
        };
        (*rec).pending_input_region = Some(value);
    }
}

/// `wl_surface.set_buffer_transform` (v2+): records how the client has
/// pre-rotated the buffer. The renderer applies the inverse at composite
/// time via CPU staging (aegis-render) — a GPU-side transform in flux's image
/// shader is the long-term path.
unsafe extern "C" fn surface_set_buffer_transform(
    _c: *mut ffi::wl_client,
    r: *mut ffi::wl_resource,
    value: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(r) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        let transform = match value as u32 {
            0 => aegis_core::Transform::Normal,
            1 => aegis_core::Transform::Rotate90,
            2 => aegis_core::Transform::Rotate180,
            3 => aegis_core::Transform::Rotate270,
            4 => aegis_core::Transform::FlipHorizontal,
            5 => aegis_core::Transform::FlipRotate90,
            6 => aegis_core::Transform::FlipRotate180,
            7 => aegis_core::Transform::FlipRotate270,
            _ => {
                ffi::wl_resource_post_error(r, 1, c"invalid wl_output.transform value".as_ptr());
                return;
            }
        };
        (*rec).pending_transform = transform;
    }
}

/// `wl_surface.set_buffer_scale` (v2+): records the HiDPI scale.
unsafe extern "C" fn surface_set_buffer_scale(
    _c: *mut ffi::wl_client,
    r: *mut ffi::wl_resource,
    value: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(r) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        if value < 1 {
            ffi::wl_resource_post_error(r, 0, c"buffer scale must be positive".as_ptr());
            return;
        }
        (*rec).pending_scale = value;
    }
}
#[allow(dead_code)]
unsafe extern "C" fn surface_noop_i32(_c: *mut ffi::wl_client, _r: *mut ffi::wl_resource, _v: i32) {
}

/// `wl_surface.damage` (v1): damage in surface-local coords. The renderer's
/// texture is in buffer pixel coords, so under buffer_scale > 1 these rects
/// cover only a fraction of the buffer. The renderer bypasses the
/// incremental-upload path when `buffer_scale != 1` (see aegis-render); a
/// generation change still triggers a correct full upload.
unsafe extern "C" fn surface_damage(
    _c: *mut ffi::wl_client,
    r: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(r) as *mut SurfaceRec;
        if rec.is_null() || w <= 0 || h <= 0 {
            return;
        }
        (*rec)
            .pending_damage
            .push(aegis_core::Rect::new(x, y, w, h));
    }
}

/// `wl_surface.damage_buffer` (v4): damage in buffer coords (post-scale,
/// post-transform). Accumulated into the same Vec as surface damage; the
/// renderer's incremental-upload path uses these directly because the
/// cached texture lives in buffer pixel space.
unsafe extern "C" fn surface_damage_buffer(
    _c: *mut ffi::wl_client,
    r: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(r) as *mut SurfaceRec;
        if rec.is_null() || w <= 0 || h <= 0 {
            return;
        }
        (*rec)
            .pending_damage
            .push(aegis_core::Rect::new(x, y, w, h));
    }
}

unsafe extern "C" fn surface_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        if !(*rec).viewport_resource.is_null() {
            let viewport =
                ffi::wl_resource_get_user_data((*rec).viewport_resource) as *mut ViewportRec;
            if !viewport.is_null() {
                (*viewport).surface = std::ptr::null_mut();
            }
            (*rec).viewport_resource = std::ptr::null_mut();
        }
        extensions::fractional_scale_surface_destroyed(rec);
        extensions::session_lock_surface_destroyed(rec);
        extensions::idle_inhibit_surface_destroyed(rec);
        extensions::explicit_sync_surface_destroyed(rec);
        extensions::input_popup_surface_destroyed(rec);
        retire_surface_buffer(rec);
        if (*rec).committed_acquire_fence >= 0 {
            libc_close((*rec).committed_acquire_fence);
            (*rec).committed_acquire_fence = -1;
        }
        if !(*rec).committed_explicit_release.is_null() {
            let release =
                std::mem::replace(&mut (*rec).committed_explicit_release, std::ptr::null_mut());
            ffi::wl_resource_post_event(
                release,
                ffi::ZWP_LINUX_BUFFER_RELEASE_V1_IMMEDIATE_RELEASE,
            );
            ffi::wl_resource_destroy(release);
        }
        // Drop the toplevel from its workspace (ADR-0025). Idempotent: a no-op
        // for surfaces that never mapped or had no toplevel role. Run before the
        // slot is nulled so the resource address is still readable.
        if !(*rec).state.is_null() {
            let state = &mut *(*rec).state;
            if (*rec).window.parent.is_none()
                && let Some(app_id) = (*rec).window.app_id.as_deref()
                && !app_id.is_empty()
            {
                let rect = (*rec).saved_floating_rect.unwrap_or(aegis_core::Rect {
                    origin: (*rec).position,
                    size: (*rec).window.size,
                });
                if rect.size.w > 0 && rect.size.h > 0 {
                    let ws_idx = state.workspace_number_for_window((*rec).window.id);

                    state.persist_app_geometry(
                        app_id,
                        rect,
                        ws_idx,
                        Some((*rec).window.layout_role),
                    );
                }
            }
            let seats = state.seats.keys().copied().collect::<Vec<_>>();
            for seat in seats {
                let Some(_guard) = ActiveSeatGuard::enter_existing(state, seat) else {
                    continue;
                };
                extensions::keyboard_shortcuts_surface_destroyed(state, resource);
                if state.pointer_focus == resource {
                    extensions::pointer_constraint_focus_changed(
                        state,
                        resource,
                        std::ptr::null_mut(),
                    );
                }
                if state.keyboard_focus == resource {
                    keyboard_focus_dependencies_changed(state, resource, std::ptr::null_mut());
                }
            }
            for runtime in state.seats.values_mut().map(Box::as_mut) {
                if runtime.cursor_surface == resource {
                    runtime.cursor_surface = std::ptr::null_mut();
                    runtime.cursor_hidden = false;
                    runtime.cursor_shape = 1;
                }
                if let Some(drag) = runtime.drag.as_mut()
                    && drag.icon == resource
                {
                    drag.icon = std::ptr::null_mut();
                }
                if runtime.pointer_focus == resource {
                    runtime.pointer_focus = std::ptr::null_mut();
                }
                if runtime.keyboard_focus == resource {
                    runtime.keyboard_focus = std::ptr::null_mut();
                }
                if runtime.tablet_focus == resource {
                    runtime.tablet_focus = std::ptr::null_mut();
                }
            }
            let id = (*rec).window.id;
            state.unregister_window(id);
            if state
                .pending_activation
                .is_some_and(|(_, pending)| pending == resource)
            {
                state.pending_activation = None;
            }
            state
                .pending_popup_focus
                .retain(|_, pending| *pending != resource);
            state.workspaces.remove_toplevel(id);
            // Notify foreign-toplevel listeners the window is gone.
            if !(*rec).xdg_toplevel.is_null() {
                extensions::foreign_toplevel_removed(id.0, state);
            }
            for child in state.live_surfaces() {
                if (*child).popup_parent == rec {
                    (*child).popup_parent = std::ptr::null_mut();
                }
            }
        }
        (*rec).dmabuf = None;
        // Detach from the parent's children list and orphan any children of this
        // surface so they do not keep a dangling parent pointer. Children stay in
        // the surfaces Vec and remain mapped (the client may re-parent them).
        detach_from_parent(rec);
        for child in std::mem::take(&mut (*rec).children) {
            (*child).parent = std::ptr::null_mut();
        }
        // Detach from the surfaces list so iterators stop visiting this rec, then
        // reclaim the allocation. The slot is left null and never reused: stable
        // indices are not load-bearing here, but the bookkeeping is simplest this
        // way and the Vec stops growing once churn settles.
        if !(*rec).state.is_null() {
            let idx = (*rec).index;
            let slot = (*(*rec).state).surfaces.as_mut_ptr().add(idx);
            std::ptr::write(slot, std::ptr::null_mut());
        }
        drop(Box::from_raw(rec));
    }
}

// ----- wl_region ----------------------------------------------------------

static REGION_IMPL: ffi::wl_region_interface_impl = ffi::wl_region_interface_impl {
    destroy: region_destroy,
    add: region_add,
    subtract: region_subtract,
};

unsafe extern "C" fn region_destroy(_client: *mut ffi::wl_client, resource: *mut ffi::wl_resource) {
    unsafe {
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn region_add(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    unsafe {
        let region = ffi::wl_resource_get_user_data(resource) as *mut RegionRec;
        if !region.is_null() && width > 0 && height > 0 {
            (*region)
                .rects
                .push(aegis_core::Rect::new(x, y, width, height));
        }
    }
}

pub(crate) fn subtract_rect(
    source: aegis_core::Rect,
    cut: aegis_core::Rect,
) -> Vec<aegis_core::Rect> {
    let sx1 = source.origin.x;
    let sy1 = source.origin.y;
    let sx2 = sx1.saturating_add(source.size.w);
    let sy2 = sy1.saturating_add(source.size.h);
    let cx1 = cut.origin.x.max(sx1);
    let cy1 = cut.origin.y.max(sy1);
    let cx2 = cut.origin.x.saturating_add(cut.size.w).min(sx2);
    let cy2 = cut.origin.y.saturating_add(cut.size.h).min(sy2);
    if cx1 >= cx2 || cy1 >= cy2 {
        return vec![source];
    }
    let candidates = [
        aegis_core::Rect::new(sx1, sy1, source.size.w, cy1 - sy1),
        aegis_core::Rect::new(sx1, cy2, source.size.w, sy2 - cy2),
        aegis_core::Rect::new(sx1, cy1, cx1 - sx1, cy2 - cy1),
        aegis_core::Rect::new(cx2, cy1, sx2 - cx2, cy2 - cy1),
    ];
    candidates
        .into_iter()
        .filter(|rect| rect.size.w > 0 && rect.size.h > 0)
        .collect()
}

unsafe extern "C" fn region_subtract(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    unsafe {
        let region = ffi::wl_resource_get_user_data(resource) as *mut RegionRec;
        if region.is_null() || width <= 0 || height <= 0 {
            return;
        }
        let cut = aegis_core::Rect::new(x, y, width, height);
        (*region).rects = std::mem::take(&mut (*region).rects)
            .into_iter()
            .flat_map(|rect| subtract_rect(rect, cut))
            .collect();
    }
}

unsafe extern "C" fn region_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let region = ffi::wl_resource_get_user_data(resource) as *mut RegionRec;
        if !region.is_null() {
            drop(Box::from_raw(region));
        }
    }
}
