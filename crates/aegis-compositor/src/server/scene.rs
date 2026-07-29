use crate::*;

impl Server {
    /// The set of toplevel ids on the current workspace of each output — the
    /// only surfaces the renderer, chrome, and input may touch (ADR-0025).
    pub(crate) fn visible(&self) -> std::collections::HashSet<aegis_core::window::WindowId> {
        self.state
            .workspaces
            .visible_toplevels()
            .into_iter()
            .collect()
    }

    /// Compatibility alias for
    /// [`client_surface_frame_order`](Self::client_surface_frame_order).
    ///
    /// Callers that only provide xdg-role frame payloads safely ignore the
    /// subsurface ids in the returned order.
    pub fn toplevel_frame_order(&self) -> Vec<usize> {
        self.client_surface_frame_order()
    }

    /// Compatibility alias for
    /// [`realm_client_surface_frame_order`](Self::realm_client_surface_frame_order).
    pub fn realm_toplevel_frame_order(&self, realm: RealmId) -> Vec<usize> {
        self.realm_client_surface_frame_order(realm)
    }

    /// Every mapped client surface id in physical-desktop paint order.
    ///
    /// Each toplevel is emitted as an indivisible stacking unit: its
    /// below-parent subsurface trees, the toplevel, its above-parent trees,
    /// then its popup surface trees. Toplevel units remain in desktop z-order.
    /// Consumers must use this order to interleave both shm and dma-buf
    /// frames; global backing-type or subsurface passes break window
    /// occlusion.
    pub fn client_surface_frame_order(&self) -> Vec<usize> {
        let visible = self.visible();
        self.client_surface_frame_order_for_realm(HUMAN_REALM, Some(&visible))
    }

    /// Every mapped client surface id in paint order for a directed Realm.
    pub fn realm_client_surface_frame_order(&self, realm: RealmId) -> Vec<usize> {
        if self.state.session_locked || self.realm_output(realm).is_none() {
            return Vec::new();
        }
        self.client_surface_frame_order_for_realm(realm, None)
    }

    fn client_surface_frame_order_for_realm(
        &self,
        realm: RealmId,
        visible: Option<&std::collections::HashSet<aegis_core::window::WindowId>>,
    ) -> Vec<usize> {
        let roots = self
            .state
            .live_surfaces()
            .filter(|pointer| unsafe {
                let surface = &**pointer;
                surface.mapped
                    && !surface.xdg_toplevel.is_null()
                    && visible.is_none_or(|visible| visible.contains(&surface.window.id))
                    && self
                        .state
                        .authority
                        .realm_observes_window(realm, surface.window.id)
            })
            .collect::<Vec<_>>();
        let popups = self
            .state
            .live_surfaces()
            .filter(|pointer| unsafe {
                let surface = &**pointer;
                surface.mapped && !surface.xdg_popup.is_null()
            })
            .collect::<Vec<_>>();

        let mut order = Vec::new();
        for root in roots {
            unsafe {
                append_surface_tree_frame_order(root, &mut order, 0);
            }
            // xdg-popups are separate xdg surface trees rather than
            // wl_subsurfaces. Keep every popup with its owning toplevel so a
            // popup from a lower window cannot escape above a higher window.
            for popup in &popups {
                if unsafe { surface_root_toplevel(*popup) == root } {
                    unsafe {
                        append_surface_tree_frame_order(*popup, &mut order, 0);
                    }
                }
            }
        }
        order
    }

    /// Mapped xdg-toplevel and xdg-popup surfaces backed by shm (CPU pixels).
    pub fn toplevel_frames(&self) -> Vec<SurfacePixels<'_>> {
        let visible = self.visible();
        self.state
            .surfaces
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .map(|p| unsafe { &*p })
            .filter(|s| {
                let root =
                    unsafe { surface_root_toplevel(*s as *const SurfaceRec as *mut SurfaceRec) };
                s.mapped
                    && (!s.xdg_toplevel.is_null() || !s.xdg_popup.is_null())
                    && !root.is_null()
                    && unsafe { !(*root).window.minimized || (*root).window.transition.is_some() }
                    && !s.content_is_dmabuf
                    && !s.pixels.is_empty()
                    && visible.contains(unsafe { &(*root).window.id })
                    && self
                        .state
                        .authority
                        .realm_observes_window(HUMAN_REALM, unsafe { (*root).window.id })
            })
            .map(|s| {
                // ADR-0029: while a transition is in flight the frame renders
                // at the interpolated rect; the model stays at the target.
                // The origin delta carries the whole subsurface tree with it.
                let render_rect = self.transition_render_rect(s);
                let mut origin = surface_draw_origin(s);
                if let Some(r) = render_rect {
                    origin.x += r.origin.x - s.position.x;
                    origin.y += r.origin.y - s.position.y;
                }
                SurfacePixels {
                    id: s.resource as usize,
                    window: if s.xdg_toplevel.is_null() {
                        let root =
                            unsafe { surface_root_toplevel(s as *const SurfaceRec as *mut _) };
                        if root.is_null() {
                            None
                        } else {
                            Some(unsafe { &*root }.window.id)
                        }
                    } else {
                        Some(s.window.id)
                    },
                    width: s.width,
                    height: s.height,
                    generation: s.generation,
                    pixels: &s.pixels,
                    geometry: aegis_core::SurfaceGeometry {
                        position: origin,
                        transform: s.buffer_transform,
                        buffer_scale: s.buffer_scale,
                        viewport_src: s.viewport_src,
                        viewport_dst: s.viewport_dst,
                        transition_size: render_rect.map(|r| r.size),
                        ..Default::default()
                    },
                    damage: &s.committed_damage,
                }
            })
            .collect()
    }

    /// Mapped xdg-toplevel surfaces backed by a dma-buf, for the renderer to
    /// import zero-copy. The `fd` is borrowed; the renderer duplicates it
    /// before Flux consumes the duplicate. The server keeps ownership until
    /// the backing buffer is replaced or destroyed.
    pub fn toplevel_dmabuf_frames(&self) -> Vec<SurfaceDmabuf> {
        let visible = self.visible();
        self.state
            .surfaces
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .map(|p| unsafe { &*p })
            .filter(|s| {
                let root =
                    unsafe { surface_root_toplevel(*s as *const SurfaceRec as *mut SurfaceRec) };
                s.mapped
                    && (!s.xdg_toplevel.is_null() || !s.xdg_popup.is_null())
                    && !root.is_null()
                    && unsafe { !(*root).window.minimized }
                    && s.content_is_dmabuf
                    && s.dmabuf.is_some()
                    && visible.contains(unsafe { &(*root).window.id })
                    && self
                        .state
                        .authority
                        .realm_observes_window(HUMAN_REALM, unsafe { (*root).window.id })
            })
            .filter_map(|s| {
                let db = s.dmabuf.as_ref()?;
                let render_rect = self.transition_render_rect(s);
                let mut origin = surface_draw_origin(s);
                if let Some(r) = render_rect {
                    origin.x += r.origin.x - s.position.x;
                    origin.y += r.origin.y - s.position.y;
                }
                Some(SurfaceDmabuf {
                    id: s.resource as usize,
                    window: if s.xdg_toplevel.is_null() {
                        let root =
                            unsafe { surface_root_toplevel(s as *const SurfaceRec as *mut _) };
                        if root.is_null() {
                            None
                        } else {
                            Some(unsafe { &*root }.window.id)
                        }
                    } else {
                        Some(s.window.id)
                    },
                    width: s.width,
                    height: s.height,
                    generation: s.generation,
                    buffer_id: db.buffer_id,
                    fd: db.fd,
                    drm_format: db.drm_format,
                    modifier: db.modifier,
                    offset: db.offset,
                    stride: db.stride,
                    acquire_fence: db.acquire_fence,
                    geometry: aegis_core::SurfaceGeometry {
                        position: origin,
                        transform: s.buffer_transform,
                        buffer_scale: s.buffer_scale,
                        viewport_src: s.viewport_src,
                        viewport_dst: s.viewport_dst,
                        transition_size: render_rect.map(|r| r.size),
                        ..Default::default()
                    },
                })
            })
            .collect()
    }

    /// All mapped physical-desktop client surfaces backed by shm.
    ///
    /// The returned vector is an unordered backing store. Composite it
    /// against [`client_surface_frame_order`](Self::client_surface_frame_order).
    pub fn client_surface_frames(&self) -> Vec<SurfacePixels<'_>> {
        let mut frames = self.toplevel_frames();
        frames.extend(self.subsurface_frames_below());
        frames.extend(self.subsurface_frames_above());
        frames
    }

    /// All mapped physical-desktop client surfaces backed by dma-buf.
    ///
    /// The returned vector is an unordered backing store. Composite it
    /// against [`client_surface_frame_order`](Self::client_surface_frame_order).
    pub fn client_surface_dmabuf_frames(&self) -> Vec<SurfaceDmabuf> {
        let mut frames = self.toplevel_dmabuf_frames();
        frames.extend(self.subsurface_dmabuf_frames_below());
        frames.extend(self.subsurface_dmabuf_frames_above());
        frames
    }

    /// Directed offscreen scene for one realm. Unlike the physical desktop,
    /// virtual realms are independent of the user's currently visible
    /// workspace and use realm-local placements on their virtual output.
    pub fn realm_toplevel_frames(&self, realm: RealmId) -> Vec<SurfacePixels<'_>> {
        if self.state.session_locked || self.realm_output(realm).is_none() {
            return Vec::new();
        }
        self.state
            .live_surfaces()
            .map(|pointer| unsafe { &*pointer })
            .filter_map(|surface| {
                let root = unsafe {
                    surface_root_toplevel(surface as *const SurfaceRec as *mut SurfaceRec)
                };
                if !surface.mapped
                    || (surface.xdg_toplevel.is_null() && surface.xdg_popup.is_null())
                    || root.is_null()
                    || unsafe { (*root).window.minimized }
                    || surface.content_is_dmabuf
                    || surface.pixels.is_empty()
                    || !self
                        .state
                        .authority
                        .realm_observes_window(realm, unsafe { (*root).window.id })
                {
                    return None;
                }
                Some(SurfacePixels {
                    id: surface.resource as usize,
                    window: Some(unsafe { (*root).window.id }),
                    width: surface.width,
                    height: surface.height,
                    generation: surface.generation,
                    pixels: &surface.pixels,
                    geometry: self.realm_surface_geometry(surface, root, realm)?,
                    damage: &surface.committed_damage,
                })
            })
            .collect()
    }

    /// All mapped client surfaces backed by shm for one directed Realm.
    pub fn realm_client_surface_frames(&self, realm: RealmId) -> Vec<SurfacePixels<'_>> {
        let mut frames = self.realm_toplevel_frames(realm);
        frames.extend(self.realm_subsurface_frames_below(realm));
        frames.extend(self.realm_subsurface_frames_above(realm));
        frames
    }

    /// All mapped client surfaces backed by dma-buf for one directed Realm.
    pub fn realm_client_surface_dmabuf_frames(&self, realm: RealmId) -> Vec<SurfaceDmabuf> {
        let mut frames = self.realm_toplevel_dmabuf_frames(realm);
        frames.extend(self.realm_subsurface_dmabuf_frames_below(realm));
        frames.extend(self.realm_subsurface_dmabuf_frames_above(realm));
        frames
    }

    pub fn realm_toplevel_dmabuf_frames(&self, realm: RealmId) -> Vec<SurfaceDmabuf> {
        if self.state.session_locked || self.realm_output(realm).is_none() {
            return Vec::new();
        }
        self.state
            .live_surfaces()
            .map(|pointer| unsafe { &*pointer })
            .filter_map(|surface| {
                let root = unsafe {
                    surface_root_toplevel(surface as *const SurfaceRec as *mut SurfaceRec)
                };
                if !surface.mapped
                    || (surface.xdg_toplevel.is_null() && surface.xdg_popup.is_null())
                    || root.is_null()
                    || unsafe { (*root).window.minimized }
                    || !surface.content_is_dmabuf
                    || !self
                        .state
                        .authority
                        .realm_observes_window(realm, unsafe { (*root).window.id })
                {
                    return None;
                }
                let dmabuf = surface.dmabuf.as_ref()?;
                Some(SurfaceDmabuf {
                    id: surface.resource as usize,
                    window: Some(unsafe { (*root).window.id }),
                    width: surface.width,
                    height: surface.height,
                    generation: surface.generation,
                    buffer_id: dmabuf.buffer_id,
                    fd: dmabuf.fd,
                    drm_format: dmabuf.drm_format,
                    modifier: dmabuf.modifier,
                    offset: dmabuf.offset,
                    stride: dmabuf.stride,
                    acquire_fence: dmabuf.acquire_fence,
                    geometry: self.realm_surface_geometry(surface, root, realm)?,
                })
            })
            .collect()
    }

    pub(crate) fn realm_surface_geometry(
        &self,
        surface: &SurfaceRec,
        root: *mut SurfaceRec,
        realm: RealmId,
    ) -> Option<aegis_core::SurfaceGeometry> {
        let root = unsafe { &*root };
        let placement = self.state.realm_placements.get(&(realm, root.window.id))?;
        let root_size = if root.window.size.w > 0 && root.window.size.h > 0 {
            root.window.size
        } else {
            surface_logical_size(root)
        };
        if root_size.w <= 0 || root_size.h <= 0 {
            return None;
        }
        let scale = (placement.size.w as f32 / root_size.w as f32)
            .min(placement.size.h as f32 / root_size.h as f32);
        let source_origin = surface_draw_origin(surface);
        let relative_x = source_origin.x - root.position.x;
        let relative_y = source_origin.y - root.position.y;
        let logical_size = surface_logical_size(surface);
        Some(aegis_core::SurfaceGeometry {
            position: aegis_core::Point {
                x: placement.origin.x + (relative_x as f32 * scale).round() as i32,
                y: placement.origin.y + (relative_y as f32 * scale).round() as i32,
            },
            transform: surface.buffer_transform,
            buffer_scale: surface.buffer_scale,
            viewport_src: surface.viewport_src,
            viewport_dst: surface.viewport_dst,
            transition_size: Some(aegis_core::Size {
                w: (logical_size.w as f32 * scale).round().max(1.0) as i32,
                h: (logical_size.h as f32 * scale).round().max(1.0) as i32,
            }),
            ..Default::default()
        })
    }

    /// Mapped session-lock surfaces backed by shm. The compositor renders
    /// these over an opaque fallback and never mixes them with normal clients.
    pub fn lock_frames(&self) -> Vec<SurfacePixels<'_>> {
        let mut frames = self
            .state
            .live_surfaces()
            .map(|surface| unsafe { &*surface })
            .filter(|surface| unsafe {
                extensions::is_active_session_lock_surface(
                    self.state.as_ref() as *const State as *mut State,
                    *surface as *const SurfaceRec as *mut SurfaceRec,
                )
            })
            .filter(|surface| {
                surface.mapped && !surface.content_is_dmabuf && !surface.pixels.is_empty()
            })
            .map(|surface| SurfacePixels {
                window: None,
                id: surface.resource as usize,
                width: surface.width,
                height: surface.height,
                generation: surface.generation,
                pixels: &surface.pixels,
                geometry: aegis_core::SurfaceGeometry {
                    position: surface.position,
                    transform: surface.buffer_transform,
                    buffer_scale: surface.buffer_scale,
                    viewport_src: surface.viewport_src,
                    viewport_dst: surface.viewport_dst,
                    ..Default::default()
                },
                damage: &surface.committed_damage,
            })
            .collect::<Vec<_>>();
        let cursor = self.state.cursor_surface;
        if !cursor.is_null()
            && unsafe {
                extensions::is_active_session_lock_client_resource(
                    self.state.as_ref() as *const State as *mut State,
                    cursor,
                )
            }
        {
            let surface = unsafe { ffi::wl_resource_get_user_data(cursor) as *mut SurfaceRec };
            if !surface.is_null() {
                let surface = unsafe { &*surface };
                if surface.mapped && !surface.content_is_dmabuf && !surface.pixels.is_empty() {
                    frames.push(SurfacePixels {
                        window: None,
                        id: surface.resource as usize,
                        width: surface.width,
                        height: surface.height,
                        generation: surface.generation,
                        pixels: &surface.pixels,
                        geometry: aegis_core::SurfaceGeometry {
                            position: surface.position,
                            transform: surface.buffer_transform,
                            buffer_scale: surface.buffer_scale,
                            viewport_src: surface.viewport_src,
                            viewport_dst: surface.viewport_dst,
                            ..Default::default()
                        },
                        damage: &surface.committed_damage,
                    });
                }
            }
        }
        frames
    }

    /// dma-buf variant of [`lock_frames`](Self::lock_frames).
    pub fn lock_dmabuf_frames(&self) -> Vec<SurfaceDmabuf> {
        let mut frames = self
            .state
            .live_surfaces()
            .map(|surface| unsafe { &*surface })
            .filter(|surface| unsafe {
                extensions::is_active_session_lock_surface(
                    self.state.as_ref() as *const State as *mut State,
                    *surface as *const SurfaceRec as *mut SurfaceRec,
                )
            })
            .filter(|surface| surface.mapped && surface.content_is_dmabuf)
            .filter_map(|surface| {
                let buffer = surface.dmabuf.as_ref()?;
                Some(SurfaceDmabuf {
                    window: None,
                    id: surface.resource as usize,
                    width: surface.width,
                    height: surface.height,
                    generation: surface.generation,
                    buffer_id: buffer.buffer_id,
                    fd: buffer.fd,
                    drm_format: buffer.drm_format,
                    modifier: buffer.modifier,
                    offset: buffer.offset,
                    stride: buffer.stride,
                    acquire_fence: buffer.acquire_fence,
                    geometry: aegis_core::SurfaceGeometry {
                        position: surface.position,
                        transform: surface.buffer_transform,
                        buffer_scale: surface.buffer_scale,
                        viewport_src: surface.viewport_src,
                        viewport_dst: surface.viewport_dst,
                        ..Default::default()
                    },
                })
            })
            .collect::<Vec<_>>();
        let cursor = self.state.cursor_surface;
        if !cursor.is_null()
            && unsafe {
                extensions::is_active_session_lock_client_resource(
                    self.state.as_ref() as *const State as *mut State,
                    cursor,
                )
            }
        {
            let surface = unsafe { ffi::wl_resource_get_user_data(cursor) as *mut SurfaceRec };
            if !surface.is_null() {
                let surface = unsafe { &*surface };
                if surface.mapped
                    && surface.content_is_dmabuf
                    && let Some(buffer) = surface.dmabuf.as_ref()
                {
                    frames.push(SurfaceDmabuf {
                        window: None,
                        id: surface.resource as usize,
                        width: surface.width,
                        height: surface.height,
                        generation: surface.generation,
                        buffer_id: buffer.buffer_id,
                        fd: buffer.fd,
                        drm_format: buffer.drm_format,
                        modifier: buffer.modifier,
                        offset: buffer.offset,
                        stride: buffer.stride,
                        acquire_fence: buffer.acquire_fence,
                        geometry: aegis_core::SurfaceGeometry {
                            position: surface.position,
                            transform: surface.buffer_transform,
                            buffer_scale: surface.buffer_scale,
                            viewport_src: surface.viewport_src,
                            viewport_dst: surface.viewport_dst,
                            ..Default::default()
                        },
                    });
                }
            }
        }
        frames
    }

    /// Input-popup, drag-icon, and cursor role surfaces, composited above all
    /// client toplevels and subsurfaces in that order.
    pub fn overlay_frames(&self) -> Vec<SurfacePixels<'_>> {
        self.overlay_frames_with_cursor(true)
    }

    /// Overlay frames with optional client-cursor inclusion. Input-method
    /// popups and drag icons remain present when the cursor is excluded.
    pub fn overlay_frames_with_cursor(&self, include_cursor: bool) -> Vec<SurfacePixels<'_>> {
        self.overlay_frames_with_cursor_at(include_cursor, None)
    }

    /// Overlay frames with an optional logical cursor-position override.
    /// Screenshot capture uses the override sampled with its request so a
    /// later pointer event cannot move a client-provided cursor surface in
    /// the captured frame.
    pub fn overlay_frames_with_cursor_at(
        &self,
        include_cursor: bool,
        cursor_position: Option<(f32, f32)>,
    ) -> Vec<SurfacePixels<'_>> {
        let drag_icon = self
            .state
            .drag
            .as_ref()
            .map_or(std::ptr::null_mut(), |drag| drag.icon);
        let mut resources = unsafe {
            extensions::input_popup_resources(self.state.as_ref() as *const _ as *mut _, HUMAN_SEAT)
        };
        resources.push(drag_icon);
        if include_cursor {
            resources.push(self.state.cursor_surface);
        }
        resources
            .into_iter()
            .filter(|resource| !resource.is_null())
            .filter_map(|resource| {
                let rec = unsafe { ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec };
                if rec.is_null() {
                    return None;
                }
                let surface = unsafe { &*rec };
                if !surface.mapped || surface.content_is_dmabuf || surface.pixels.is_empty() {
                    return None;
                }
                let position = if resource == self.state.cursor_surface {
                    cursor_position
                        .map(|position| {
                            cursor_surface_position(
                                position,
                                self.state.cursor_hotspot,
                                surface.attach_offset,
                            )
                        })
                        .unwrap_or(surface.position)
                } else {
                    surface.position
                };
                Some(SurfacePixels {
                    window: None,
                    id: surface.resource as usize,
                    width: surface.width,
                    height: surface.height,
                    generation: surface.generation,
                    pixels: &surface.pixels,
                    geometry: aegis_core::SurfaceGeometry {
                        position,
                        transform: surface.buffer_transform,
                        buffer_scale: surface.buffer_scale,
                        viewport_src: surface.viewport_src,
                        viewport_dst: surface.viewport_dst,
                        ..Default::default()
                    },
                    damage: &surface.committed_damage,
                })
            })
            .collect()
    }

    /// dma-buf variant of [`overlay_frames`](Self::overlay_frames).
    pub fn overlay_dmabuf_frames(&self) -> Vec<SurfaceDmabuf> {
        self.overlay_dmabuf_frames_with_cursor(true)
    }

    /// dma-buf overlay frames with optional client-cursor inclusion.
    pub fn overlay_dmabuf_frames_with_cursor(&self, include_cursor: bool) -> Vec<SurfaceDmabuf> {
        self.overlay_dmabuf_frames_with_cursor_at(include_cursor, None)
    }

    /// dma-buf overlay frames with an optional logical cursor-position
    /// override. This is the dma-buf counterpart of
    /// [`Self::overlay_frames_with_cursor_at`].
    pub fn overlay_dmabuf_frames_with_cursor_at(
        &self,
        include_cursor: bool,
        cursor_position: Option<(f32, f32)>,
    ) -> Vec<SurfaceDmabuf> {
        let drag_icon = self
            .state
            .drag
            .as_ref()
            .map_or(std::ptr::null_mut(), |drag| drag.icon);
        let mut resources = unsafe {
            extensions::input_popup_resources(self.state.as_ref() as *const _ as *mut _, HUMAN_SEAT)
        };
        resources.push(drag_icon);
        if include_cursor {
            resources.push(self.state.cursor_surface);
        }
        resources
            .into_iter()
            .filter(|resource| !resource.is_null())
            .filter_map(|resource| {
                let rec = unsafe { ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec };
                if rec.is_null() {
                    return None;
                }
                let surface = unsafe { &*rec };
                if !surface.mapped || !surface.content_is_dmabuf {
                    return None;
                }
                let buffer = surface.dmabuf.as_ref()?;
                let position = if resource == self.state.cursor_surface {
                    cursor_position
                        .map(|position| {
                            cursor_surface_position(
                                position,
                                self.state.cursor_hotspot,
                                surface.attach_offset,
                            )
                        })
                        .unwrap_or(surface.position)
                } else {
                    surface.position
                };
                Some(SurfaceDmabuf {
                    window: None,
                    id: surface.resource as usize,
                    width: surface.width,
                    height: surface.height,
                    generation: surface.generation,
                    buffer_id: buffer.buffer_id,
                    fd: buffer.fd,
                    drm_format: buffer.drm_format,
                    modifier: buffer.modifier,
                    offset: buffer.offset,
                    stride: buffer.stride,
                    acquire_fence: buffer.acquire_fence,
                    geometry: aegis_core::SurfaceGeometry {
                        position,
                        transform: surface.buffer_transform,
                        buffer_scale: surface.buffer_scale,
                        viewport_src: surface.viewport_src,
                        viewport_dst: surface.viewport_dst,
                        ..Default::default()
                    },
                })
            })
            .collect()
    }

    /// Mapped subsurfaces backed by shm whose `place_below` was the most
    /// recent stacking request — these render *under* their parent toplevel.
    /// Nested subsurface chains are walked recursively: each entry carries
    /// its compositor-space draw origin, and the whole subtree of a
    /// below-child renders here, in render order.
    pub fn subsurface_frames_below(&self) -> Vec<SurfacePixels<'_>> {
        self.collect_subsurfaces_shm(false)
    }

    /// As [`subsurface_frames_below`](Self::subsurface_frames_below) for
    /// surfaces whose most recent stacking request was `place_above` (or the
    /// default). These render *over* their parent toplevel.
    pub fn subsurface_frames_above(&self) -> Vec<SurfacePixels<'_>> {
        self.collect_subsurfaces_shm(true)
    }

    /// Mapped dma-buf-backed subsurfaces below their parent.
    pub fn subsurface_dmabuf_frames_below(&self) -> Vec<SurfaceDmabuf> {
        self.collect_subsurfaces_dmabuf(false)
    }

    /// Mapped dma-buf-backed subsurfaces above their parent.
    pub fn subsurface_dmabuf_frames_above(&self) -> Vec<SurfaceDmabuf> {
        self.collect_subsurfaces_dmabuf(true)
    }

    pub fn realm_subsurface_frames_below(&self, realm: RealmId) -> Vec<SurfacePixels<'_>> {
        self.collect_realm_subsurfaces_shm(realm, false)
    }

    pub fn realm_subsurface_frames_above(&self, realm: RealmId) -> Vec<SurfacePixels<'_>> {
        self.collect_realm_subsurfaces_shm(realm, true)
    }

    pub fn realm_subsurface_dmabuf_frames_below(&self, realm: RealmId) -> Vec<SurfaceDmabuf> {
        self.collect_realm_subsurfaces_dmabuf(realm, false)
    }

    pub fn realm_subsurface_dmabuf_frames_above(&self, realm: RealmId) -> Vec<SurfaceDmabuf> {
        self.collect_realm_subsurfaces_dmabuf(realm, true)
    }

    pub(crate) fn collect_realm_subsurfaces_shm(
        &self,
        realm: RealmId,
        want_above: bool,
    ) -> Vec<SurfacePixels<'_>> {
        if self.state.session_locked || self.realm_output(realm).is_none() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for role_pointer in self.state.live_surfaces() {
            let role_surface = unsafe { &*role_pointer };
            if !role_surface.mapped
                || (role_surface.xdg_toplevel.is_null() && role_surface.xdg_popup.is_null())
            {
                continue;
            }
            let root = unsafe { surface_root_toplevel(role_pointer) };
            if root.is_null() {
                continue;
            }
            let root_surface = unsafe { &*root };
            if !root_surface.mapped
                || root_surface.window.minimized
                || !self
                    .state
                    .authority
                    .realm_observes_window(realm, root_surface.window.id)
            {
                continue;
            }
            for &child_ptr in &role_surface.children {
                if child_ptr.is_null() {
                    continue;
                }
                let child = unsafe { &*child_ptr };
                if child.subsurface_above_parent == want_above {
                    self.collect_realm_subtree_shm(
                        child,
                        root,
                        realm,
                        root_surface.window.id,
                        &mut out,
                        0,
                    );
                }
            }
        }
        out
    }

    pub(crate) fn collect_realm_subtree_shm<'a>(
        &'a self,
        surface: &'a SurfaceRec,
        root: *mut SurfaceRec,
        realm: RealmId,
        window: aegis_core::window::WindowId,
        out: &mut Vec<SurfacePixels<'a>>,
        depth: u32,
    ) {
        if !surface.mapped || depth >= 32 {
            return;
        }
        for &child_ptr in &surface.children {
            if !child_ptr.is_null() && unsafe { !(*child_ptr).subsurface_above_parent } {
                self.collect_realm_subtree_shm(
                    unsafe { &*child_ptr },
                    root,
                    realm,
                    window,
                    out,
                    depth + 1,
                );
            }
        }
        if !surface.content_is_dmabuf
            && !surface.pixels.is_empty()
            && let Some(geometry) = self.realm_surface_geometry(surface, root, realm)
        {
            out.push(SurfacePixels {
                window: Some(window),
                id: surface.resource as usize,
                width: surface.width,
                height: surface.height,
                generation: surface.generation,
                pixels: &surface.pixels,
                geometry,
                damage: &surface.committed_damage,
            });
        }
        for &child_ptr in &surface.children {
            if !child_ptr.is_null() && unsafe { (*child_ptr).subsurface_above_parent } {
                self.collect_realm_subtree_shm(
                    unsafe { &*child_ptr },
                    root,
                    realm,
                    window,
                    out,
                    depth + 1,
                );
            }
        }
    }

    pub(crate) fn collect_realm_subsurfaces_dmabuf(
        &self,
        realm: RealmId,
        want_above: bool,
    ) -> Vec<SurfaceDmabuf> {
        if self.state.session_locked || self.realm_output(realm).is_none() {
            return Vec::new();
        }
        let mut out = Vec::new();
        for role_pointer in self.state.live_surfaces() {
            let role_surface = unsafe { &*role_pointer };
            if !role_surface.mapped
                || (role_surface.xdg_toplevel.is_null() && role_surface.xdg_popup.is_null())
            {
                continue;
            }
            let root = unsafe { surface_root_toplevel(role_pointer) };
            if root.is_null() {
                continue;
            }
            let root_surface = unsafe { &*root };
            if !root_surface.mapped
                || root_surface.window.minimized
                || !self
                    .state
                    .authority
                    .realm_observes_window(realm, root_surface.window.id)
            {
                continue;
            }
            for &child_ptr in &role_surface.children {
                if child_ptr.is_null() {
                    continue;
                }
                let child = unsafe { &*child_ptr };
                if child.subsurface_above_parent == want_above {
                    self.collect_realm_subtree_dmabuf(
                        child,
                        root,
                        realm,
                        root_surface.window.id,
                        &mut out,
                        0,
                    );
                }
            }
        }
        out
    }

    pub(crate) fn collect_realm_subtree_dmabuf(
        &self,
        surface: &SurfaceRec,
        root: *mut SurfaceRec,
        realm: RealmId,
        window: aegis_core::window::WindowId,
        out: &mut Vec<SurfaceDmabuf>,
        depth: u32,
    ) {
        if !surface.mapped || depth >= 32 {
            return;
        }
        for &child_ptr in &surface.children {
            if !child_ptr.is_null() && unsafe { !(*child_ptr).subsurface_above_parent } {
                self.collect_realm_subtree_dmabuf(
                    unsafe { &*child_ptr },
                    root,
                    realm,
                    window,
                    out,
                    depth + 1,
                );
            }
        }
        if surface.content_is_dmabuf
            && let (Some(dmabuf), Some(geometry)) = (
                surface.dmabuf.as_ref(),
                self.realm_surface_geometry(surface, root, realm),
            )
        {
            out.push(SurfaceDmabuf {
                window: Some(window),
                id: surface.resource as usize,
                width: surface.width,
                height: surface.height,
                generation: surface.generation,
                buffer_id: dmabuf.buffer_id,
                fd: dmabuf.fd,
                drm_format: dmabuf.drm_format,
                modifier: dmabuf.modifier,
                offset: dmabuf.offset,
                stride: dmabuf.stride,
                acquire_fence: dmabuf.acquire_fence,
                geometry,
            });
        }
        for &child_ptr in &surface.children {
            if !child_ptr.is_null() && unsafe { (*child_ptr).subsurface_above_parent } {
                self.collect_realm_subtree_dmabuf(
                    unsafe { &*child_ptr },
                    root,
                    realm,
                    window,
                    out,
                    depth + 1,
                );
            }
        }
    }

    pub(crate) fn collect_subsurfaces_shm(&self, want_above: bool) -> Vec<SurfacePixels<'_>> {
        let visible = self.visible();
        let mut out = Vec::new();
        for role_pointer in self.state.live_surfaces() {
            let role_surface = unsafe { &*role_pointer };
            if !role_surface.mapped
                || (role_surface.xdg_toplevel.is_null() && role_surface.xdg_popup.is_null())
            {
                continue;
            }
            let root = unsafe { surface_root_toplevel(role_pointer) };
            if root.is_null() {
                continue;
            }
            let root_surface = unsafe { &*root };
            if !root_surface.mapped
                || root_surface.window.minimized
                || !visible.contains(&root_surface.window.id)
                || !self
                    .state
                    .authority
                    .realm_observes_window(HUMAN_REALM, root_surface.window.id)
            {
                continue;
            }
            // ADR-0029: the toplevel's in-flight transition shifts its whole
            // subsurface tree by the same delta.
            let delta = self
                .transition_render_rect(root_surface)
                .map(|r| {
                    (
                        r.origin.x - root_surface.position.x,
                        r.origin.y - root_surface.position.y,
                    )
                })
                .unwrap_or((0, 0));
            for &child_ptr in &role_surface.children {
                if child_ptr.is_null() {
                    continue;
                }
                let child = unsafe { &*child_ptr };
                if child.subsurface_above_parent != want_above {
                    continue;
                }
                Self::collect_subtree_shm(child, &mut out, delta, root_surface.window.id, 0);
            }
        }
        out
    }

    /// Emit one subsurface subtree in render order: the below-children
    /// subtrees, the surface itself, then the above-children subtrees. A
    /// subsurface's descendants render relative to it, so the whole subtree
    /// of a below-child still renders under the root toplevel. An unmapped
    /// node hides its entire subtree, per `wl_subsurface` mapping rules.
    /// `delta` shifts every emitted origin by the root's in-flight window
    /// transition (ADR-0029); (0, 0) outside transitions.
    pub(crate) fn collect_subtree_shm<'a>(
        s: &'a SurfaceRec,
        out: &mut Vec<SurfacePixels<'a>>,
        delta: (i32, i32),
        window: aegis_core::window::WindowId,
        depth: u32,
    ) {
        // The depth cap only breaks reference cycles defensively; children
        // are orphaned on destroy, so live child pointers are always valid.
        if !s.mapped || depth >= 32 {
            return;
        }
        for &child_ptr in &s.children {
            if child_ptr.is_null() {
                continue;
            }
            let child = unsafe { &*child_ptr };
            if !child.subsurface_above_parent {
                Self::collect_subtree_shm(child, out, delta, window, depth + 1);
            }
        }
        if !s.content_is_dmabuf && !s.pixels.is_empty() {
            let origin = surface_draw_origin(s);
            out.push(SurfacePixels {
                window: Some(window),
                id: s.resource as usize,
                width: s.width,
                height: s.height,
                generation: s.generation,
                pixels: &s.pixels,
                geometry: aegis_core::SurfaceGeometry {
                    position: aegis_core::Point {
                        x: origin.x + delta.0,
                        y: origin.y + delta.1,
                    },
                    transform: s.buffer_transform,
                    buffer_scale: s.buffer_scale,
                    viewport_src: s.viewport_src,
                    viewport_dst: s.viewport_dst,
                    ..Default::default()
                },
                damage: &s.committed_damage,
            });
        }
        for &child_ptr in &s.children {
            if child_ptr.is_null() {
                continue;
            }
            let child = unsafe { &*child_ptr };
            if child.subsurface_above_parent {
                Self::collect_subtree_shm(child, out, delta, window, depth + 1);
            }
        }
    }

    pub(crate) fn collect_subsurfaces_dmabuf(&self, want_above: bool) -> Vec<SurfaceDmabuf> {
        let visible = self.visible();
        let mut out = Vec::new();
        for role_pointer in self.state.live_surfaces() {
            let role_surface = unsafe { &*role_pointer };
            if !role_surface.mapped
                || (role_surface.xdg_toplevel.is_null() && role_surface.xdg_popup.is_null())
            {
                continue;
            }
            let root = unsafe { surface_root_toplevel(role_pointer) };
            if root.is_null() {
                continue;
            }
            let root_surface = unsafe { &*root };
            if !root_surface.mapped
                || root_surface.window.minimized
                || !visible.contains(&root_surface.window.id)
                || !self
                    .state
                    .authority
                    .realm_observes_window(HUMAN_REALM, root_surface.window.id)
            {
                continue;
            }
            let delta = self
                .transition_render_rect(root_surface)
                .map(|r| {
                    (
                        r.origin.x - root_surface.position.x,
                        r.origin.y - root_surface.position.y,
                    )
                })
                .unwrap_or((0, 0));
            for &child_ptr in &role_surface.children {
                if child_ptr.is_null() {
                    continue;
                }
                let child = unsafe { &*child_ptr };
                if child.subsurface_above_parent != want_above {
                    continue;
                }
                Self::collect_subtree_dmabuf(child, &mut out, delta, root_surface.window.id, 0);
            }
        }
        out
    }

    /// The dma-buf half of [`Self::collect_subtree_shm`]: same render-order
    /// tree walk, emitting only dma-buf-backed surfaces.
    pub(crate) fn collect_subtree_dmabuf(
        s: &SurfaceRec,
        out: &mut Vec<SurfaceDmabuf>,
        delta: (i32, i32),
        window: aegis_core::window::WindowId,
        depth: u32,
    ) {
        if !s.mapped || depth >= 32 {
            return;
        }
        for &child_ptr in &s.children {
            if child_ptr.is_null() {
                continue;
            }
            let child = unsafe { &*child_ptr };
            if !child.subsurface_above_parent {
                Self::collect_subtree_dmabuf(child, out, delta, window, depth + 1);
            }
        }
        if s.content_is_dmabuf
            && let Some(db) = s.dmabuf.as_ref()
        {
            let origin = surface_draw_origin(s);
            out.push(SurfaceDmabuf {
                window: Some(window),
                id: s.resource as usize,
                width: s.width,
                height: s.height,
                generation: s.generation,
                buffer_id: db.buffer_id,
                fd: db.fd,
                drm_format: db.drm_format,
                modifier: db.modifier,
                offset: db.offset,
                stride: db.stride,
                acquire_fence: db.acquire_fence,
                geometry: aegis_core::SurfaceGeometry {
                    position: aegis_core::Point {
                        x: origin.x + delta.0,
                        y: origin.y + delta.1,
                    },
                    transform: s.buffer_transform,
                    buffer_scale: s.buffer_scale,
                    viewport_src: s.viewport_src,
                    viewport_dst: s.viewport_dst,
                    ..Default::default()
                },
            });
        }
        for &child_ptr in &s.children {
            if child_ptr.is_null() {
                continue;
            }
            let child = unsafe { &*child_ptr };
            if child.subsurface_above_parent {
                Self::collect_subtree_dmabuf(child, out, delta, window, depth + 1);
            }
        }
    }

    /// Whether any client is waiting for an output-paced frame callback.
    pub fn frame_callbacks_pending(&self) -> bool {
        self.state
            .surfaces
            .iter()
            .copied()
            .any(|p| !p.is_null() && unsafe { !(*p).frame_callbacks.is_empty() })
    }

    /// Fire and clear all pending frame callbacks, pacing clients to the output.
    /// `time_ms` is a millisecond timestamp from a monotonic clock.
    pub fn send_frame_callbacks(&mut self, time_ms: u32) -> usize {
        let mut sent = 0;
        for &p in &self.state.surfaces {
            if p.is_null() {
                continue;
            }
            let rec = unsafe { &mut *p };
            for cb in rec.frame_callbacks.drain(..) {
                unsafe {
                    ffi::wl_resource_post_event(cb, ffi::WL_CALLBACK_DONE, time_ms);
                    ffi::wl_resource_destroy(cb);
                }
                sent += 1;
            }
        }
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
        sent
    }

    /// The last submitted frame reached the presentation backend, so every
    /// surface's accumulated damage is now represented by the renderer's
    /// retained texture and the scanout baseline.
    pub fn acknowledge_presented_surface_damage(&mut self) {
        for &p in &self.state.surfaces {
            if p.is_null() {
                continue;
            }
            let rec = unsafe { &mut *p };
            rec.committed_damage.clear();
            rec.committed_damage_full = false;
        }
    }

    pub fn retired_buffers_pending(&self) -> bool {
        !self.state.retired_buffer_releases.is_empty()
    }

    /// Release replaced dma-bufs after the renderer has submitted a frame
    /// that no longer references them. `completion_fence` is a borrowed Linux
    /// sync_file fd for that submission; `None` means completion was already
    /// waited on by the backend.
    pub fn release_retired_buffers(&mut self, completion_fence: Option<i32>) {
        let retired = std::mem::take(&mut self.state.retired_buffer_releases);
        let mut retry = Vec::new();
        for release in retired {
            let explicit_fd = if release.explicit_release.is_null() {
                None
            } else if let Some(fence) = completion_fence {
                let fd = unsafe { dup(fence) };
                if fd < 0 {
                    retry.push(release);
                    continue;
                }
                Some(fd)
            } else {
                None
            };
            unsafe {
                if !release.buffer.is_null() {
                    ffi::wl_resource_post_event(release.buffer, ffi::WL_BUFFER_RELEASE);
                }
                if !release.explicit_release.is_null() {
                    if let Some(fd) = explicit_fd {
                        ffi::wl_resource_post_event(
                            release.explicit_release,
                            ffi::ZWP_LINUX_BUFFER_RELEASE_V1_FENCED_RELEASE,
                            fd,
                        );
                    } else {
                        ffi::wl_resource_post_event(
                            release.explicit_release,
                            ffi::ZWP_LINUX_BUFFER_RELEASE_V1_IMMEDIATE_RELEASE,
                        );
                    }
                    ffi::wl_resource_destroy(release.explicit_release);
                }
            }
        }
        self.state.retired_buffer_releases.extend(retry);
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }
}

/// Flatten one xdg-role surface and its wl_subsurface descendants into Wayland
/// paint order. The server owns the tree; the renderer only receives resource
/// ids and frame payloads.
unsafe fn append_surface_tree_frame_order(
    surface: *mut SurfaceRec,
    order: &mut Vec<usize>,
    depth: u32,
) {
    unsafe {
        if surface.is_null() || !(*surface).mapped || depth >= 32 {
            return;
        }
        for &child in &(*surface).children {
            if !child.is_null() && !(*child).subsurface_above_parent {
                append_surface_tree_frame_order(child, order, depth + 1);
            }
        }
        order.push((*surface).resource as usize);
        for &child in &(*surface).children {
            if !child.is_null() && (*child).subsurface_above_parent {
                append_surface_tree_frame_order(child, order, depth + 1);
            }
        }
    }
}

fn cursor_surface_position(
    pointer: (f32, f32),
    hotspot: aegis_core::Point,
    attach_offset: aegis_core::Point,
) -> aegis_core::Point {
    aegis_core::Point {
        x: pointer.0.round() as i32 - hotspot.x + attach_offset.x,
        y: pointer.1.round() as i32 - hotspot.y + attach_offset.y,
    }
}

#[cfg(test)]
mod tests {
    use super::cursor_surface_position;
    use aegis_core::Point;

    #[test]
    fn cursor_surface_override_preserves_trigger_position_and_hotspot() {
        assert_eq!(
            cursor_surface_position((120.6, 79.4), Point { x: 7, y: 11 }, Point { x: 2, y: -3 },),
            Point { x: 116, y: 65 },
        );
    }
}
