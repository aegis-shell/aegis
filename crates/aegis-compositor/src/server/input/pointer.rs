use crate::*;

impl Server {
    pub(crate) fn pointer_motion(&mut self, x: f32, y: f32) {
        // Relative delta from the previous motion event (for relative-pointer
        // clients). Computed before pointer_x/y are overwritten.
        let dx = x - self.state.raw_pointer_x;
        let dy = y - self.state.raw_pointer_y;
        self.state.raw_pointer_x = x;
        self.state.raw_pointer_y = y;
        let (x, y) = unsafe { extensions::constrain_pointer_motion(self.state.as_mut(), x, y) };
        self.state.pointer_x = x;
        self.state.pointer_y = y;
        if self.state.active_seat == HUMAN_SEAT {
            unsafe { update_overlay_positions(self.state.as_mut()) };
        }
        if self.state.drag.is_some() {
            let focus = self.hit_test_focus(x, y);
            let time = self.epoch.elapsed().as_millis() as u32;
            unsafe {
                update_drag_focus(self.state.as_mut(), focus, x, y, time);
            }
            return;
        }
        let time = self.epoch.elapsed().as_millis() as u32;
        // Push relative motion to bound zwp_relative_pointer_v1 resources of
        // the focused client (games, etc.).
        self.post_relative_motion(dx, dy);
        // If an interactive grab is active, update the window's geometry
        // before any hit-testing — motion goes to the grabbed surface, not
        // whatever is under the pointer.
        if self.state.interactive.is_some() && self.apply_interactive_motion(x, y) {
            // Fall through to normal motion forwarding so the client still
            // sees wl_pointer.motion events per protocol. The hit-test
            // stays pinned on the grabbed surface because pointer_focus
            // was set when the grab started.
            self.post_motion_to_focus(time);
            return;
        }
        let focus = self.hit_test_focus(x, y);
        if focus != self.state.pointer_focus {
            self.change_pointer_focus(focus);
        }
        // Post motion to whichever client now holds focus.
        self.post_motion_to_focus(time);
    }

    pub(crate) fn pointer_button(&mut self, button: u32, state: aegis_core::input::ButtonState) {
        if self.state.session_locked {
            if state.is_pressed() && !self.state.pointer_focus.is_null() {
                self.change_keyboard_focus(self.state.pointer_focus);
            }
            if self.state.pointer_focus.is_null() {
                return;
            }
            let serial = unsafe { ffi::wl_display_next_serial(self.state.display) };
            let time = self.epoch.elapsed().as_millis() as u32;
            self.state.last_button_serial = serial;
            self.state.implicit_grab_active = state.is_pressed();
            let focus_client = unsafe { ffi::wl_resource_get_client(self.state.pointer_focus) };
            for pointer in self.iter_focus_pointers(focus_client) {
                unsafe {
                    let ver = ffi::wl_resource_get_version(pointer);
                    ffi::wl_resource_post_event(
                        pointer,
                        ffi::WL_POINTER_BUTTON,
                        serial,
                        time,
                        button,
                        u32::from(state.is_pressed()),
                    );
                    if ver >= 5 {
                        ffi::wl_resource_post_event(pointer, ffi::WL_POINTER_FRAME);
                    }
                }
            }
            return;
        }
        if !state.is_pressed() {
            self.state.implicit_grab_active = false;
            if self.state.drag.is_some() {
                unsafe { finish_drag(self.state.as_mut()) };
                return;
            }
        }
        // Button release ends any active interactive grab. A compositor-side
        // border grab consumed its press, so consume the paired release too;
        // client-initiated grabs still receive the release as required.
        if !state.is_pressed() && self.state.interactive.is_some() {
            let consume = self.state.compositor_pointer_grab;
            self.finish_interactive();
            if consume {
                return;
            }
        }

        if state.is_pressed() {
            let grabbed_popup = self
                .state
                .live_surfaces()
                .filter(|surface| unsafe {
                    (**surface).mapped
                        && !(**surface).xdg_popup.is_null()
                        && (**surface).popup_grabbed
                })
                .last();
            if let Some(popup) = grabbed_popup
                && self.state.pointer_focus != unsafe { (*popup).resource }
            {
                unsafe {
                    ffi::wl_resource_post_event((*popup).xdg_popup, ffi::XDG_POPUP_POPUP_DONE);
                    (*popup).popup_grabbed = false;
                    (*popup).mapped = false;
                }
                return;
            }
        }

        // Floating windows expose an invisible inside border for direct
        // resize. This runs before client button delivery so dragging a border
        // never activates a widget under the same pixels. Tiled, maximized,
        // and fullscreen windows keep their layout-owned geometry.
        const BORDER: f32 = 8.0;
        const BTN_LEFT: u32 = 0x110;
        const BTN_RIGHT: u32 = 0x111;
        // Borderless windows still need compositor-owned gestures that start
        // anywhere in their content. Super+left moves; Super+right resizes
        // from the nearest edge/corner. Both detach a layout-owned window so
        // tiling policy does not overwrite the interactive geometry.
        if self.state.active_seat == HUMAN_SEAT
            && state.is_pressed()
            && (button == BTN_LEFT || button == BTN_RIGHT)
            && self.state.depressed_mods.has(aegis_core::input::Mods::SUPER)
            && self.state.interactive.is_none()
            && !self.state.pointer_focus.is_null()
        {
            let focused = unsafe {
                ffi::wl_resource_get_user_data(self.state.pointer_focus) as *mut SurfaceRec
            };
            let rec = unsafe { surface_root_toplevel(focused) };
            if !rec.is_null() && unsafe { !(*rec).xdg_toplevel.is_null() } {
                let id = unsafe { (*rec).window.id };
                let resize_edges = unsafe {
                    let mut window = (*rec).window.clone();
                    window.position = (*rec).position;
                    window.size = (*rec)
                        .window_geometry
                        .map(|geometry| geometry.size)
                        .unwrap_or_else(|| surface_logical_size(&*rec));
                    window.resize_edges_nearest(self.state.pointer_x, self.state.pointer_y)
                };
                if button != BTN_RIGHT || !resize_edges.is_none() {
                    unsafe {
                        (*rec).window.layout_role = aegis_core::layout::LayoutRole::Floating;
                        (*rec).layout_target = None;
                        let state_changed =
                            (*rec).window.state.maximized || (*rec).window.state.fullscreen;
                        (*rec).window.state.maximized = false;
                        (*rec).window.state.fullscreen = false;
                        if state_changed {
                            reconfigure_with_state(rec);
                        }
                    }
                    if button == BTN_LEFT {
                        self.start_interactive_move(id);
                    } else {
                        self.start_interactive_resize(id, resize_edges);
                    }
                    if self.state.interactive.is_some() {
                        self.state.compositor_pointer_grab = true;
                        return;
                    }
                }
            }
        }
        if self.state.active_seat == HUMAN_SEAT
            && state.is_pressed()
            && button == BTN_LEFT
            && self.state.interactive.is_none()
            && let Some((rec, edges)) =
                self.resize_target_at(self.state.pointer_x, self.state.pointer_y, BORDER)
        {
            let resource = unsafe { (*rec).resource };
            let id = unsafe { (*rec).window.id };
            self.change_keyboard_focus(resource);
            unsafe {
                (*rec).window.state.resizing = true;
                reconfigure_with_state(rec);
            }
            self.state.interactive = Some(aegis_core::window::Interactive::Resize {
                window_id: id,
                edges,
                origin: (self.state.pointer_x, self.state.pointer_y),
                start_position: unsafe { (*rec).position },
                start_size: unsafe {
                    (*rec)
                        .window_geometry
                        .map(|geometry| geometry.size)
                        .unwrap_or_else(|| surface_logical_size(&*rec))
                },
            });
            self.state.compositor_pointer_grab = true;
            return;
        }
        // Click-to-focus: when a button is pressed over a surface, that
        // surface also gains keyboard focus. Released edges do not change
        // focus (matches GTK/Qt click-to-focus expectations).
        if state.is_pressed() && !self.state.pointer_focus.is_null() {
            self.change_keyboard_focus(self.state.pointer_focus);
        }
        if self.state.pointer_focus.is_null() {
            return;
        }
        let serial = unsafe { ffi::wl_display_next_serial(self.state.display) };
        let time = self.epoch.elapsed().as_millis() as u32;
        if state.is_pressed() {
            // Only a press starts an implicit grab. Keeping the press serial
            // stable until release lets xdg_toplevel.move/resize and
            // wl_data_device.start_drag validate the exact triggering event.
            self.state.last_button_serial = serial;
            self.state.implicit_grab_active = true;
        }
        let state_u32 = if state.is_pressed() { 1u32 } else { 0u32 };
        let focus = self.state.pointer_focus;
        let focus_client = unsafe { ffi::wl_resource_get_client(focus) };
        for p in self.iter_focus_pointers(focus_client) {
            unsafe {
                let ver = ffi::wl_resource_get_version(p);
                ffi::wl_resource_post_event(
                    p,
                    ffi::WL_POINTER_BUTTON,
                    serial,
                    time,
                    button,
                    state_u32,
                );
                if ver >= 5 {
                    ffi::wl_resource_post_event(p, ffi::WL_POINTER_FRAME);
                }
            }
        }
    }

    /// Synthesized leave: e.g. when the host pointer leaves the nested window.
    pub(crate) fn pointer_leave_all(&mut self) {
        self.state.implicit_grab_active = false;
        if self.state.drag.is_some() {
            unsafe { cancel_drag(self.state.as_mut(), true) };
        }
        self.change_pointer_focus(std::ptr::null_mut());
    }

    pub(crate) fn keyboard_key(
        &mut self,
        evdev_code: u32,
        state: aegis_core::input::ButtonState,
        keymap: Option<&aegis_core::keybind::Keymap>,
    ) -> Option<aegis_core::keybind::Action> {
        // Always advance xkbcommon state so modifier tracking and global
        // bindings work even with no focused client (e.g. an empty desktop).
        // Always posting modifiers (even when unchanged) is simpler; the
        // client-side xkbcommon treats a no-op update cheaply. A delta check
        // can be added if profiling ever shows it matters.
        let outcome = if let Some(kb) = self.state.keyboard.as_mut() {
            kb.update_key(evdev_code, state.is_pressed())
        } else {
            return None;
        };
        self.state.depressed_mods = aegis_core::input::Mods(outcome.depressed);
        // Console VT switch (Ctrl+Alt+Fn → XF86Switch_VT_N): libinput owns
        // evdev on a direct backend, so the kernel's built-in handling never
        // runs — the compositor performs the session switch itself through
        // libseat. xkb only produces these keysyms with Ctrl+Alt held, and
        // they are consumed here (never posted to a client).
        const XF86_SWITCH_VT_1: u32 = 0x1008_FE01;
        const XF86_SWITCH_VT_12: u32 = 0x1008_FE0C;
        if self.state.active_seat == HUMAN_SEAT
            && state.is_pressed()
            && (XF86_SWITCH_VT_1..=XF86_SWITCH_VT_12).contains(&outcome.keysym)
        {
            self.state.pending_vt_switch = Some((outcome.keysym - XF86_SWITCH_VT_1 + 1) as i32);
            return None;
        }
        // A key that matches a global binding on press is consumed (not posted
        // to the focused client) and its action returned for the caller to
        // dispatch. Modifier-only keys never match, so modifiers still post.
        let shortcuts_inhibited =
            unsafe { extensions::keyboard_shortcuts_inhibited(self.state.as_mut()) };
        let matched = if state.is_pressed() && !shortcuts_inhibited && !self.state.session_locked {
            keymap.and_then(|keymap| {
                keymap.match_key(aegis_core::input::Mods(outcome.depressed), outcome.keysym)
            })
        } else {
            None
        };
        if matched.is_some() {
            return matched;
        }
        if self.state.keyboard_focus.is_null() {
            return None;
        }
        let serial = unsafe { ffi::wl_display_next_serial(self.state.display) };
        let state_u32 = if state.is_pressed() { 1u32 } else { 0u32 };
        let depressed = outcome.depressed;
        let latched = outcome.latched;
        let locked = outcome.locked;
        let group = outcome.group;
        let focus = self.state.keyboard_focus;
        let focus_client = unsafe { ffi::wl_resource_get_client(focus) };
        for k in self.iter_focus_keyboards(focus_client) {
            unsafe {
                ffi::wl_resource_post_event(
                    k,
                    ffi::WL_KEYBOARD_MODIFIERS,
                    serial,
                    depressed,
                    latched,
                    locked,
                    group,
                );
                ffi::wl_resource_post_event(
                    k,
                    ffi::WL_KEYBOARD_KEY,
                    serial,
                    0u32,
                    evdev_code,
                    state_u32,
                );
            }
        }
        None
    }

    /// Hit-test the current pointer position against mapped toplevels,
    /// returning the surface resource under the cursor or null if none. Uses
    /// each surface's authoritative `position` (the window-rect origin,
    /// assigned at map time); later surfaces in the surfaces Vec are
    /// considered "above" earlier ones.
    pub(crate) fn hit_test_focus(&self, x: f32, y: f32) -> *mut ffi::wl_resource {
        let visible = self.visible();
        let physical = self.state.active_seat == HUMAN_SEAT;
        let synthetic_target = self.state.synthetic_target;
        let mut hit: *mut ffi::wl_resource = std::ptr::null_mut();
        for p in self.state.live_surfaces() {
            let s = unsafe { &*p };
            if self.state.session_locked {
                if !unsafe {
                    extensions::is_active_session_lock_surface(
                        self.state.as_ref() as *const State as *mut State,
                        p,
                    )
                } || !s.mapped
                {
                    continue;
                }
                let sx = s.position.x as f32;
                let sy = s.position.y as f32;
                let logical = surface_logical_size(s);
                if x >= sx && y >= sy && x < sx + logical.w as f32 && y < sy + logical.h as f32 {
                    hit = s.resource;
                }
                continue;
            }
            let root = unsafe { surface_root_toplevel(p) };
            if !s.mapped
                || (s.xdg_toplevel.is_null() && s.xdg_popup.is_null())
                || root.is_null()
                || unsafe { (*root).window.minimized }
                || (physical && !visible.contains(unsafe { &(*root).window.id }))
                || synthetic_target.is_some_and(|target| target != unsafe { (*root).window.id })
            {
                continue;
            }
            let window = unsafe { (*root).window.id };
            if !self
                .state
                .authority
                .seat_controls_window(self.state.active_seat, window)
            {
                // A presentation-only mirror is an input barrier, not a hole
                // in the scene. If its input region covers the point, clear a
                // hit from a lower surface but never give the mirror focus.
                // This prevents a physical click on an agent-controlled
                // window from accidentally activating whatever is behind it.
                let observed = self
                    .state
                    .authority
                    .seat(self.state.active_seat)
                    .is_some_and(|seat| {
                        self.state
                            .authority
                            .realm_observes_window(seat.realm, window)
                    });
                if observed {
                    let mut barrier = std::ptr::null_mut();
                    Self::hit_test_tree(s, x, y, &mut barrier, 0);
                    if !barrier.is_null() {
                        hit = std::ptr::null_mut();
                    }
                }
                continue;
            }
            Self::hit_test_tree(s, x, y, &mut hit, 0);
        }
        hit
    }

    /// Walk one surface tree in render order (below-children subtrees, the
    /// surface itself, above-children subtrees), keeping the last surface
    /// that accepts `(x, y)` — the topmost. Subsurfaces therefore receive
    /// input directly when they are topmost under the pointer, per the core
    /// protocol, instead of the event falling through to the root toplevel.
    pub(crate) fn hit_test_tree(
        s: &SurfaceRec,
        x: f32,
        y: f32,
        hit: &mut *mut ffi::wl_resource,
        depth: u32,
    ) {
        if depth >= 32 {
            return;
        }
        for &child_ptr in &s.children {
            if child_ptr.is_null() {
                continue;
            }
            let child = unsafe { &*child_ptr };
            // An unmapped subsurface hides its whole subtree.
            if !child.subsurface_above_parent && child.mapped {
                Self::hit_test_tree(child, x, y, hit, depth + 1);
            }
        }
        if surface_accepts_point(s, x, y) {
            *hit = s.resource;
        }
        for &child_ptr in &s.children {
            if child_ptr.is_null() {
                continue;
            }
            let child = unsafe { &*child_ptr };
            if child.subsurface_above_parent && child.mapped {
                Self::hit_test_tree(child, x, y, hit, depth + 1);
            }
        }
    }

    /// Topmost floating toplevel whose inside border contains `(x, y)`.
    pub(crate) fn resize_target_at(
        &self,
        x: f32,
        y: f32,
        border: f32,
    ) -> Option<(*mut SurfaceRec, aegis_core::window::ResizeEdges)> {
        let visible = self.visible();
        let mut hit = None;
        for p in self.state.live_surfaces() {
            let s = unsafe { &*p };
            if !s.mapped
                || s.xdg_toplevel.is_null()
                || s.window.minimized
                || s.window.state.maximized
                || s.window.state.fullscreen
                || s.window.layout_role != aegis_core::layout::LayoutRole::Floating
                || !visible.contains(&s.window.id)
                || !self
                    .state
                    .authority
                    .seat_controls_window(self.state.active_seat, s.window.id)
            {
                continue;
            }
            let mut window = s.window.clone();
            window.position = s.position;
            window.size = s
                .window_geometry
                .map(|geometry| geometry.size)
                .unwrap_or_else(|| surface_logical_size(s));
            let edges = window.resize_edges_at(x, y, border);
            if !edges.is_none() {
                hit = Some((p, edges));
            }
        }
        hit
    }

    /// End an interactive move/resize, clearing the protocol resizing state
    /// and notifying the client once after the final geometry.
    pub(crate) fn finish_interactive(&mut self) {
        if let Some(aegis_core::window::Interactive::Resize { window_id, .. }) =
            self.state.interactive
        {
            let rec = self.find_surface_by_window_id(window_id);
            if !rec.is_null() {
                unsafe {
                    (*rec).window.state.resizing = false;
                    reconfigure_with_state(rec);
                }
            }
        }
        self.state.interactive = None;
        self.state.compositor_pointer_grab = false;
    }

    pub(crate) fn is_lock_resource(&self, resource: *mut ffi::wl_resource) -> bool {
        if resource.is_null() {
            return false;
        }
        let surface = unsafe { ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec };
        unsafe {
            extensions::is_active_session_lock_surface(
                self.state.as_ref() as *const State as *mut State,
                surface,
            )
        }
    }

    /// Transition focus: post leave to the old client's pointer resources and
    /// enter to the new client's, with a fresh serial.
    pub(crate) fn change_pointer_focus(&mut self, mut new_focus: *mut ffi::wl_resource) {
        let allowed = if self.state.session_locked {
            self.is_lock_resource(new_focus)
        } else {
            !new_focus.is_null() && self.active_seat_controls_resource(new_focus)
        };
        if !allowed {
            new_focus = std::ptr::null_mut();
        }
        let serial = unsafe { ffi::wl_display_next_serial(self.state.display) };
        let old = self.state.pointer_focus;

        if new_focus != old {
            self.state.cursor_surface = std::ptr::null_mut();
            self.state.cursor_shape = 1;
            self.state.cursor_hidden = false;
        }

        if !old.is_null() {
            let old_client = unsafe { ffi::wl_resource_get_client(old) };
            for p in self.iter_focus_pointers(old_client) {
                unsafe {
                    let ver = ffi::wl_resource_get_version(p);
                    ffi::wl_resource_post_event(p, ffi::WL_POINTER_LEAVE, serial, old);
                    if ver >= 5 {
                        ffi::wl_resource_post_event(p, ffi::WL_POINTER_FRAME);
                    }
                }
            }
        }
        self.state.pointer_focus = new_focus;
        self.state.last_pointer_enter_serial = if new_focus.is_null() { 0 } else { serial };
        if !new_focus.is_null() {
            let new_client = unsafe { ffi::wl_resource_get_client(new_focus) };
            let rec = unsafe { ffi::wl_resource_get_user_data(new_focus) as *mut SurfaceRec };
            let (local_x, local_y) = if rec.is_null() {
                (self.state.pointer_x, self.state.pointer_y)
            } else {
                let origin = unsafe { surface_draw_origin(&*rec) };
                (
                    self.state.pointer_x - origin.x as f32,
                    self.state.pointer_y - origin.y as f32,
                )
            };
            let x = ffi::wl_fixed_from_f32(local_x);
            let y = ffi::wl_fixed_from_f32(local_y);
            for p in self.iter_focus_pointers(new_client) {
                unsafe {
                    let ver = ffi::wl_resource_get_version(p);
                    ffi::wl_resource_post_event(p, ffi::WL_POINTER_ENTER, serial, new_focus, x, y);
                    if ver >= 5 {
                        ffi::wl_resource_post_event(p, ffi::WL_POINTER_FRAME);
                    }
                }
            }
        }
        unsafe {
            extensions::pointer_constraint_focus_changed(self.state.as_mut(), old, new_focus)
        };
    }

    pub(crate) fn post_motion_to_focus(&self, time: u32) {
        if self.state.pointer_focus.is_null() {
            return;
        }
        let focus = self.state.pointer_focus;
        let focus_client = unsafe { ffi::wl_resource_get_client(focus) };
        let rec = unsafe { ffi::wl_resource_get_user_data(focus) as *mut SurfaceRec };
        let (local_x, local_y) = if rec.is_null() {
            (self.state.pointer_x, self.state.pointer_y)
        } else {
            let origin = unsafe { surface_draw_origin(&*rec) };
            (
                self.state.pointer_x - origin.x as f32,
                self.state.pointer_y - origin.y as f32,
            )
        };
        let x = ffi::wl_fixed_from_f32(local_x);
        let y = ffi::wl_fixed_from_f32(local_y);
        for p in self.iter_focus_pointers(focus_client) {
            unsafe {
                let ver = ffi::wl_resource_get_version(p);
                ffi::wl_resource_post_event(p, ffi::WL_POINTER_MOTION, time, x, y);
                if ver >= 5 {
                    ffi::wl_resource_post_event(p, ffi::WL_POINTER_FRAME);
                }
            }
        }
    }

    /// Post `zwp_relative_pointer_v1.relative_motion` to every bound
    /// relative-pointer resource owned by the focused client. `dx`/`dy` are the
    /// unaccelerated pixel deltas since the last motion event; the protocol
    /// also wants accelerated deltas, which we do not model, so we send both
    /// fields equal to the unaccelerated value.
    pub(crate) fn post_relative_motion(&self, dx: f32, dy: f32) {
        if self.state.pointer_focus.is_null() || (dx == 0.0 && dy == 0.0) {
            return;
        }
        let focus_client = unsafe { ffi::wl_resource_get_client(self.state.pointer_focus) };
        // Monotonic microsecond timestamp, split hi/lo per the protocol.
        let utime = self.epoch.elapsed().as_micros() as u64;
        let utime_hi = (utime >> 32) as u32;
        let utime_lo = (utime & 0xffff_ffff) as u32;
        let fdx = ffi::wl_fixed_from_f32(dx);
        let fdy = ffi::wl_fixed_from_f32(dy);
        // Collect the live relative-pointer resources for this client.
        let targets: Vec<*mut ffi::wl_resource> = self
            .state
            .relative_pointers
            .iter()
            .copied()
            .filter(|p| !p.is_null() && unsafe { ffi::wl_resource_get_client(*p) == focus_client })
            .collect();
        for rp in targets {
            unsafe {
                ffi::wl_resource_post_event(
                    rp,
                    ffi::ZWP_RELATIVE_POINTER_V1_RELATIVE_MOTION,
                    utime_hi,
                    utime_lo,
                    fdx,
                    fdy,
                    fdx,
                    fdy,
                );
            }
        }
    }

    /// Post one backend-preserved scroll frame to the focused client.
    pub(crate) fn pointer_axis(&mut self, frame: aegis_core::input::PointerAxisFrame) {
        if self.state.pointer_focus.is_null() || !frame.has_data() {
            return;
        }
        let focus = self.state.pointer_focus;
        let focus_client = unsafe { ffi::wl_resource_get_client(focus) };
        for p in self.iter_focus_pointers(focus_client) {
            let ver = unsafe { ffi::wl_resource_get_version(p) };
            for event in pointer_axis_wire_events(ver, frame) {
                unsafe { post_pointer_axis_wire_event(p, event) };
            }
        }
    }

    /// Post `wl_touch.down`: a new contact on the focused surface. Touch
    /// events go to the pointer-focused client (touch and pointer share a
    /// seat). `id` is the contact id (0..). The `time` is the same monotonic
    /// millisecond clock pointer events use.
    pub(crate) fn touch_down(&mut self, time: u32, id: i32, x: f32, y: f32) {
        if self.state.pointer_focus.is_null() {
            return;
        }
        let focus = self.state.pointer_focus;
        let serial = unsafe { ffi::wl_display_next_serial(self.state.display) };
        let client = unsafe { ffi::wl_resource_get_client(focus) };
        let rec = unsafe { ffi::wl_resource_get_user_data(focus) as *mut SurfaceRec };
        let origin = if rec.is_null() {
            aegis_core::Point::default()
        } else {
            unsafe { surface_draw_origin(&*rec) }
        };
        let fx = ffi::wl_fixed_from_f32(x - origin.x as f32);
        let fy = ffi::wl_fixed_from_f32(y - origin.y as f32);
        for t in self.iter_client_touch(client) {
            unsafe {
                ffi::wl_resource_post_event(t, ffi::WL_TOUCH_DOWN, serial, time, focus, id, fx, fy);
            }
        }
    }

    /// Post `wl_touch.motion` for an existing contact.
    pub(crate) fn touch_motion(&mut self, time: u32, id: i32, x: f32, y: f32) {
        if self.state.pointer_focus.is_null() {
            return;
        }
        let focus = self.state.pointer_focus;
        let client = unsafe { ffi::wl_resource_get_client(focus) };
        let rec = unsafe { ffi::wl_resource_get_user_data(focus) as *mut SurfaceRec };
        let origin = if rec.is_null() {
            aegis_core::Point::default()
        } else {
            unsafe { surface_draw_origin(&*rec) }
        };
        let fx = ffi::wl_fixed_from_f32(x - origin.x as f32);
        let fy = ffi::wl_fixed_from_f32(y - origin.y as f32);
        for t in self.iter_client_touch(client) {
            unsafe {
                ffi::wl_resource_post_event(t, ffi::WL_TOUCH_MOTION, time, id, fx, fy);
            }
        }
    }

    /// Post `wl_touch.up`.
    pub(crate) fn touch_up(&mut self, time: u32, id: i32) {
        if self.state.pointer_focus.is_null() {
            return;
        }
        let serial = unsafe { ffi::wl_display_next_serial(self.state.display) };
        let client = unsafe { ffi::wl_resource_get_client(self.state.pointer_focus) };
        for t in self.iter_client_touch(client) {
            unsafe {
                ffi::wl_resource_post_event(t, ffi::WL_TOUCH_UP, serial, time, id);
            }
        }
    }

    /// Post `wl_touch.frame`: end of a touch event batch.
    pub(crate) fn touch_frame(&self) {
        if self.state.pointer_focus.is_null() {
            return;
        }
        let client = unsafe { ffi::wl_resource_get_client(self.state.pointer_focus) };
        for t in self.iter_client_touch(client) {
            unsafe { ffi::wl_resource_post_event(t, ffi::WL_TOUCH_FRAME) };
        }
    }

    /// Post `wl_touch.cancel`: all active contacts invalidated.
    pub(crate) fn touch_cancel(&self) {
        if self.state.pointer_focus.is_null() {
            return;
        }
        let client = unsafe { ffi::wl_resource_get_client(self.state.pointer_focus) };
        for t in self.iter_client_touch(client) {
            unsafe { ffi::wl_resource_post_event(t, ffi::WL_TOUCH_CANCEL) };
        }
    }

    pub(crate) fn iter_client_touch(
        &self,
        client: *mut ffi::wl_client,
    ) -> Vec<*mut ffi::wl_resource> {
        self.state
            .touch_resources
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .filter(|p| unsafe { ffi::wl_resource_get_client(*p) == client })
            .collect()
    }
}
