use crate::*;

impl Server {
    /// Transition keyboard focus: post leave to the old client's keyboard
    /// resources and enter to the new client's. The `enter` snapshot carries
    /// every key currently down in the client-facing logical stream. This is
    /// required when focus changes while a modifier is held: the new client
    /// must learn about the press before its eventual release. Also flips the
    /// `activated` toplevel state bit on the old and new surfaces so clients
    /// update their title-bar chrome to match focus.
    pub(crate) fn change_keyboard_focus(&mut self, mut new_focus: *mut ffi::wl_resource) {
        let allowed = if self.state.session_lock_phase.is_active() {
            self.is_lock_resource(new_focus)
        } else {
            !new_focus.is_null() && self.active_seat_controls_resource(new_focus)
        };
        if !allowed {
            new_focus = std::ptr::null_mut();
        }
        if !new_focus.is_null() && self.state.synthetic_target.is_none() {
            // The clicked surface may be a subsurface; raise its root
            // toplevel so the window comes forward as a unit. Targeted agent
            // input (`synthetic_target`) deliberately skips the raise: the
            // agent seat's focus is per-seat, so an agent-operated window
            // must not keep jumping above the physical user's windows on
            // every batch — stacking stays the human's.
            let rec = unsafe { ffi::wl_resource_get_user_data(new_focus) as *mut SurfaceRec };
            let root = unsafe { surface_root_toplevel(rec) };
            if !root.is_null() {
                let root_resource = unsafe { (*root).resource };
                self.raise_toplevel(root_resource);
            }
        }
        if new_focus == self.state.keyboard_focus {
            return;
        }
        let serial = unsafe { ffi::wl_display_next_serial(self.state.display) };
        let old = self.state.keyboard_focus;

        if !old.is_null() {
            let old_client = unsafe { ffi::wl_resource_get_client(old) };
            for k in self.iter_focus_keyboards(old_client) {
                unsafe {
                    ffi::wl_resource_post_event(k, ffi::WL_KEYBOARD_LEAVE, serial, old);
                }
            }
        }
        self.state.keyboard_focus = new_focus;
        if !old.is_null() && !self.any_seat_focuses_toplevel(old) {
            // `activated` is not seat-indexed in xdg-shell. Keep it set while
            // any logical seat still focuses this toplevel.
            self.set_activated_for_surface(old, false);
        }
        if !new_focus.is_null() {
            let new_client = unsafe { ffi::wl_resource_get_client(new_focus) };
            let pressed_keys = self
                .state
                .client_pressed_keys
                .iter()
                .copied()
                .collect::<Vec<_>>();
            let keys = keyboard::keycodes_wl_array(&pressed_keys);
            let modifiers = self
                .state
                .keyboard
                .as_ref()
                .map(keyboard::Keyboard::modifiers)
                .unwrap_or((self.state.depressed_mods.0, 0, 0, 0));
            for k in self.iter_focus_keyboards(new_client) {
                unsafe {
                    ffi::wl_resource_post_event(
                        k,
                        ffi::WL_KEYBOARD_ENTER,
                        serial,
                        new_focus,
                        &keys as *const ffi::wl_array as *mut ffi::wl_array,
                    );
                    ffi::wl_resource_post_event(
                        k,
                        ffi::WL_KEYBOARD_MODIFIERS,
                        serial,
                        modifiers.0,
                        modifiers.1,
                        modifiers.2,
                        modifiers.3,
                    );
                }
            }
            // Set activated on the surface gaining keyboard focus.
            self.set_activated_for_surface(new_focus, true);
        }

        // Text-input focus follows keyboard focus. Publish it after the
        // corresponding wl_keyboard leave/enter events so clients never
        // observe an IME enter for a surface their keyboard has not entered.
        unsafe {
            keyboard_focus_dependencies_changed(
                self.state.as_ref() as *const State as *mut State,
                old,
                new_focus,
            );
        }
    }

    /// Move a focused toplevel and its whole surface tree to the top of the
    /// stacking order while keeping every live record's destroy-slot index
    /// correct. Raw `SurfaceRec` allocations do not move; only their pointers
    /// in the Vec do.
    pub(crate) fn raise_toplevel(&mut self, resource: *mut ffi::wl_resource) {
        let Some(pos) = self.state.surfaces.iter().position(|p| {
            !p.is_null() && unsafe { (**p).resource == resource && !(**p).xdg_toplevel.is_null() }
        }) else {
            return;
        };
        let root = self.state.surfaces[pos];
        let surfaces = std::mem::take(&mut self.state.surfaces);
        let mut rest = Vec::with_capacity(surfaces.len());
        let mut raised_parents = Vec::new();
        let mut raised_descendants = Vec::new();
        for ptr in surfaces {
            if ptr.is_null() {
                continue;
            }
            let r = unsafe { surface_root_toplevel(ptr) };
            if r == root {
                raised_parents.push(ptr);
            } else if !r.is_null() && unsafe { is_transient_descendant_of(r, root, &self.state) } {
                raised_descendants.push(ptr);
            } else {
                rest.push(ptr);
            }
        }
        rest.append(&mut raised_parents);
        rest.append(&mut raised_descendants);
        self.state.surfaces = rest;
        for (index, ptr) in self.state.surfaces.iter().copied().enumerate() {
            if !ptr.is_null() {
                unsafe { (*ptr).index = index };
            }
        }
        self.restack_always_on_top_band();
    }

    /// Move every always-on-top toplevel's whole surface tree to the top of
    /// the stacking order, keeping each tree contiguous and preserving the
    /// relative order of both the normal windows and the always-on-top band.
    /// Called after every raise and after each dispatch batch so a normal
    /// window can never stack above an always-on-top one and a newly mapped
    /// but unfocused window cannot land above the band either. Raw
    /// `SurfaceRec` allocations do not move; only their pointers in the Vec
    /// do.
    pub(crate) fn restack_always_on_top_band(&mut self) {
        if !self.state.live_surfaces().any(|p| {
            let s = unsafe { &*p };
            !s.xdg_toplevel.is_null() && s.window.always_on_top
        }) {
            return;
        }
        let mut rest = Vec::with_capacity(self.state.surfaces.len());
        let mut band = Vec::new();
        for ptr in self.state.surfaces.drain(..) {
            let on_top = !ptr.is_null()
                && unsafe {
                    let root = surface_root_toplevel(ptr);
                    !root.is_null()
                        && !(*root).xdg_toplevel.is_null()
                        && (*root).window.always_on_top
                };
            if on_top {
                band.push(ptr);
            } else {
                rest.push(ptr);
            }
        }
        rest.append(&mut band);
        self.state.surfaces = rest;
        for (index, ptr) in self.state.surfaces.iter().copied().enumerate() {
            if !ptr.is_null() {
                unsafe { (*ptr).index = index };
            }
        }
    }

    /// Flip the `activated` bit on a toplevel and reconfigure it. No-op if
    /// the surface has no toplevel role or the bit is already in the
    /// requested state.
    pub(crate) fn set_activated_for_surface(
        &mut self,
        surface: *mut ffi::wl_resource,
        activated: bool,
    ) {
        // Find the SurfaceRec backing this surface resource by walking the
        // Vec, then resolve its root toplevel — keyboard focus may rest on
        // a subsurface, but activation is a property of the window. The
        // search is O(N) but N is small and this only fires on focus
        // transitions, not per frame.
        for p in self.state.live_surfaces() {
            let s = unsafe { &mut *p };
            if s.resource != surface {
                continue;
            }
            let root = unsafe { surface_root_toplevel(s as *mut SurfaceRec) };
            if root.is_null() {
                return;
            }
            let s = unsafe { &mut *root };
            if s.xdg_toplevel.is_null() {
                return;
            }
            if s.window.state.activated == activated {
                return;
            }
            s.window.state.activated = activated;
            // Focusing a minimized toplevel restores it: clear the flag so
            // the renderer and hit-test pick it up again.
            if activated {
                s.window.minimized = false;
                let s_ptr = s as *mut SurfaceRec;
                for p in self.state.live_surfaces() {
                    if p != s_ptr && unsafe { is_transient_descendant_of(p, s_ptr, &self.state) } {
                        unsafe { (*p).window.minimized = false };
                    }
                }
            }
            unsafe { reconfigure_with_state(s as *mut SurfaceRec) };
            return;
        }
    }

    /// Borrowed slice of live pointer resource pointers belonging to `client`.
    /// The slice is rebuilt per call (no lifetime issues across re-entry).
    pub(crate) fn iter_focus_pointers(
        &self,
        client: *mut ffi::wl_client,
    ) -> Vec<*mut ffi::wl_resource> {
        self.state
            .pointer_resources
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .filter(|p| unsafe { ffi::wl_resource_get_client(*p) == client })
            .collect()
    }

    pub(crate) fn iter_focus_keyboards(
        &self,
        client: *mut ffi::wl_client,
    ) -> Vec<*mut ffi::wl_resource> {
        self.state
            .keyboard_resources
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .filter(|p| unsafe { ffi::wl_resource_get_client(*p) == client })
            .collect()
    }

    pub(crate) fn active_seat_controls_resource(&self, resource: *mut ffi::wl_resource) -> bool {
        if resource.is_null() {
            return false;
        }
        let rec = unsafe { ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec };
        let root = unsafe { surface_root_toplevel(rec) };
        !root.is_null()
            && self
                .state
                .authority
                .seat_controls_window(self.state.active_seat, unsafe { (*root).window.id })
    }

    pub(crate) fn any_seat_focuses_toplevel(&self, resource: *mut ffi::wl_resource) -> bool {
        let rec = unsafe { ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec };
        let root = unsafe { surface_root_toplevel(rec) };
        if root.is_null() {
            return false;
        }
        self.state.seats.values().any(|runtime| {
            if runtime.keyboard_focus.is_null() {
                return false;
            }
            let focused = unsafe {
                ffi::wl_resource_get_user_data(runtime.keyboard_focus) as *mut SurfaceRec
            };
            unsafe { surface_root_toplevel(focused) == root }
        })
    }
}
