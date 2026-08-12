use crate::*;

impl Server {
    /// Forward a drained batch of input events from the backend to the focused
    /// client. Pointer motion drives hit-testing and enter/leave transitions;
    /// pointer buttons go to the current focus and also drive click-to-focus
    /// for the keyboard. Key events update xkbcommon state and post
    /// `wl_keyboard.key` plus any resulting `wl_keyboard.modifiers` change.
    /// Pointer-axis frames preserve source, high-resolution wheel data, and
    /// real sequence termination when translated to `wl_pointer`.
    pub fn forward_input(
        &mut self,
        events: &[aegis_model::input::InputEvent],
        keymap: &aegis_model::keybind::Keymap,
    ) -> Vec<aegis_model::keybind::Action> {
        let _guard = ActiveSeatGuard::enter(self.state.as_mut(), HUMAN_SEAT)
            .expect("bootstrap human seat must remain enabled");
        self.forward_input_active(events, Some(keymap))
    }

    /// Forward a physical-input batch whose client-owned key events were
    /// already prepared in the backend's original order.
    ///
    /// `prepared_keys` contains exactly one snapshot for each `Key` remaining
    /// in `events`; a missing snapshot means XKB initialization failed and the
    /// edge is safely dropped. Chrome-owned keys have already been removed
    /// from both slices. Pointer/touch events are still processed in their
    /// routed order.
    pub fn forward_prepared_input(
        &mut self,
        events: &[aegis_model::input::InputEvent],
        prepared_keys: &[Option<PreparedKeyboardEvent>],
        keymap: &aegis_model::keybind::Keymap,
    ) -> Vec<aegis_model::keybind::Action> {
        let _guard = ActiveSeatGuard::enter(self.state.as_mut(), HUMAN_SEAT)
            .expect("bootstrap human seat must remain enabled");
        self.forward_input_active_with_prepared(events, Some(keymap), Some(prepared_keys))
    }

    /// Route a synthetic batch through an agent's independent logical seat.
    /// Compositor-global key bindings and VT switching are deliberately
    /// disabled on this path; the batch can only become client protocol input.
    pub fn forward_agent_input(
        &mut self,
        seat: SeatId,
        events: &[aegis_model::input::InputEvent],
    ) -> Result<(), InteractionDomainRuntimeError> {
        let _guard = ActiveSeatGuard::enter(self.state.as_mut(), seat)
            .ok_or(InteractionDomainRuntimeError::SeatUnavailable(seat))?;
        self.forward_input_active(events, None);
        Ok(())
    }

    /// Deliver a previously validated target-local batch through one agent
    /// seat. The target is re-authorized on the main thread immediately
    /// before delivery, keyboard focus is scoped to that seat, and pointer
    /// hit-testing is pinned to the target root for the complete batch.
    pub fn forward_agent_input_to(
        &mut self,
        seat: SeatId,
        window: aegis_model::window::WindowId,
        events: &[aegis_model::input::InputEvent],
    ) -> Result<(), InteractionDomainRuntimeError> {
        if !self.state.authority.seat_controls_window(seat, window) {
            return Err(InteractionDomainError::UnknownWindow(window).into());
        }
        let rec = self.find_surface_by_window_id(window);
        if rec.is_null() || unsafe { (*rec).xdg_toplevel.is_null() || !(*rec).mapped } {
            return Err(InteractionDomainError::UnknownWindow(window).into());
        }
        let _guard = ActiveSeatGuard::enter(self.state.as_mut(), seat)
            .ok_or(InteractionDomainRuntimeError::SeatUnavailable(seat))?;
        self.state.synthetic_target = Some(window);
        self.change_keyboard_focus(unsafe { (*rec).resource });
        self.forward_input_active(events, None);
        self.state.synthetic_target = None;
        Ok(())
    }

    pub(crate) fn forward_input_active(
        &mut self,
        events: &[aegis_model::input::InputEvent],
        keymap: Option<&aegis_model::keybind::Keymap>,
    ) -> Vec<aegis_model::keybind::Action> {
        self.forward_input_active_with_prepared(events, keymap, None)
    }

    fn forward_input_active_with_prepared(
        &mut self,
        events: &[aegis_model::input::InputEvent],
        keymap: Option<&aegis_model::keybind::Keymap>,
        prepared_keys: Option<&[Option<PreparedKeyboardEvent>]>,
    ) -> Vec<aegis_model::keybind::Action> {
        let mut actions = Vec::new();
        let time = self.epoch.elapsed().as_millis() as u32;
        let mut prepared_keys = prepared_keys.map(|keys| keys.iter().copied());
        for event in events {
            use aegis_model::input::InputEvent::*;
            match *event {
                PointerMotion {
                    x,
                    y,
                    dx,
                    dy,
                    dx_unaccel,
                    dy_unaccel,
                } => self.pointer_motion(x, y, dx, dy, dx_unaccel, dy_unaccel),
                PointerButton { button, state } => self.pointer_button(button, state),
                PointerLeave => self.pointer_leave_all(),
                PointerAxis(frame) => self.pointer_axis(frame),
                TouchDown { id, x, y } => self.touch_down(time, id, x, y),
                TouchMotion { id, x, y } => self.touch_motion(time, id, x, y),
                TouchUp { id } => self.touch_up(time, id),
                TouchFrame => self.touch_frame(),
                TouchCancel => self.touch_cancel(),
                Key { code, state } => {
                    let action = if let Some(keys) = prepared_keys.as_mut() {
                        let prepared = keys
                            .next()
                            .expect("every routed physical key must have a prepared snapshot");
                        if let Some(prepared) = prepared {
                            debug_assert_eq!(prepared.evdev_code, code);
                            debug_assert_eq!(prepared.state, state);
                            self.deliver_prepared_keyboard_event(prepared, keymap)
                        } else {
                            None
                        }
                    } else {
                        self.keyboard_key(code, state, keymap)
                    };
                    if let Some(a) = action {
                        actions.push(a);
                    }
                }
                Tablet { event } => self.tablet_event(event),
            }
        }
        debug_assert!(
            prepared_keys
                .as_mut()
                .is_none_or(|prepared| prepared.next().is_none()),
            "prepared key snapshots must match the routed key stream"
        );
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
        actions
    }

    /// Mark real user input activity for ext-idle-notify. Synthetic IPC input
    /// intentionally does not call this method, so automation cannot keep a
    /// session awake indefinitely.
    pub fn note_user_activity(&mut self) {
        unsafe { extensions::idle_user_activity(self.state.as_mut()) };
    }

    /// Take a pending console VT switch request (Ctrl+Alt+Fn), if any.
    /// The main loop forwards it to the backend (libseat on DRM; a no-op
    /// nested).
    pub fn take_vt_switch(&mut self) -> Option<i32> {
        self.state.pending_vt_switch.take()
    }

    /// Current pointer focus, as the surface resource pointer. For the
    /// shell's hit-test of "is the pointer over chrome".
    pub fn pointer_focus_surface(&self) -> Option<*mut ffi::wl_resource> {
        if self.state.pointer_focus.is_null() {
            None
        } else {
            Some(self.state.pointer_focus)
        }
    }

    /// Last reported pointer position in compositor logical space.
    pub fn pointer_position(&self) -> (f32, f32) {
        (self.state.pointer_x, self.state.pointer_y)
    }

    /// Whether a logical output point is occupied by client content or its
    /// compositor-owned resize affordance. Consumers that react only to
    /// exposed desktop background use this semantic query instead of
    /// inferring visibility from the current Wayland focus pointer.
    pub fn client_occupies_point(&self, x: f32, y: f32) -> bool {
        !self.hit_test_focus(x, y).is_null()
            || self
                .resize_target_at(x, y, aegis_model::window::RESIZE_OUTER_MARGIN)
                .is_some()
    }

    /// Current depressed keyboard modifiers after the most recently routed
    /// key event. The composition root uses this to keep held-modifier chrome
    /// (notably Super+Tab) open until the modifier is actually released.
    pub fn depressed_modifiers(&self) -> aegis_model::input::Mods {
        self.state.depressed_mods
    }

    /// Validate and translate target-local automation actions into the same
    /// backend-agnostic events used by physical input. The method is pure with
    /// respect to compositor state: the caller can reject the complete batch
    /// (for example because shell chrome covers a point) before forwarding any
    /// event.
    pub fn prepare_synthetic_input(
        &self,
        window_id: aegis_model::window::WindowId,
        actions: &[aegis_model::input::SyntheticInputAction],
    ) -> Option<Vec<aegis_model::input::InputEvent>> {
        self.prepare_synthetic_input_for_seat(HUMAN_SEAT, window_id, actions, true)
    }

    /// Validate an agent's target-local batch without consulting the physical
    /// desktop workspace. The window must be controlled by `seat`; observation
    /// authority alone never permits input.
    pub fn prepare_agent_synthetic_input(
        &self,
        seat: SeatId,
        window_id: aegis_model::window::WindowId,
        actions: &[aegis_model::input::SyntheticInputAction],
    ) -> Option<Vec<aegis_model::input::InputEvent>> {
        self.prepare_synthetic_input_for_seat(seat, window_id, actions, false)
    }

    pub(crate) fn prepare_synthetic_input_for_seat(
        &self,
        seat: SeatId,
        window_id: aegis_model::window::WindowId,
        actions: &[aegis_model::input::SyntheticInputAction],
        require_physical_visibility: bool,
    ) -> Option<Vec<aegis_model::input::InputEvent>> {
        use aegis_model::input::{ButtonState, InputEvent, SyntheticInputAction};

        let runtime = self.state.seat_runtime(seat)?;
        if self.state.session_lock_phase.is_active()
            || actions.is_empty()
            || actions.len() > 64
            || runtime.interactive.is_some()
            || runtime.drag.is_some()
            || runtime.implicit_grab_active
            || runtime.depressed_mods != aegis_model::input::Mods::NONE
            || !self.state.authority.seat_controls_window(seat, window_id)
        {
            return None;
        }
        let rec = self.find_surface_by_window_id(window_id);
        if rec.is_null()
            || unsafe {
                (*rec).xdg_toplevel.is_null()
                    || !(*rec).mapped
                    || (*rec).window.minimized
                    || (require_physical_visibility && !self.visible().contains(&window_id))
            }
        {
            return None;
        }
        let (origin, size) = unsafe {
            let size = if (*rec).window.size.w > 0 && (*rec).window.size.h > 0 {
                (*rec).window.size
            } else {
                surface_logical_size(&*rec)
            };
            ((*rec).position, size)
        };
        if size.w <= 0 || size.h <= 0 {
            return None;
        }
        let to_global = |local: aegis_model::Point| -> Option<(f32, f32)> {
            if local.x < 0 || local.y < 0 || local.x >= size.w || local.y >= size.h {
                return None;
            }
            let x = origin.x.checked_add(local.x)?;
            let y = origin.y.checked_add(local.y)?;
            let mut hit = std::ptr::null_mut();
            unsafe { Self::hit_test_tree(&*rec, x as f32, y as f32, &mut hit, 0) };
            if hit.is_null() {
                return None;
            }
            let hit_rec = unsafe { ffi::wl_resource_get_user_data(hit) as *mut SurfaceRec };
            let root = unsafe { surface_root_toplevel(hit_rec) };
            (root == rec).then_some((x as f32, y as f32))
        };

        // Validate the complete action list before emitting any event.
        for action in actions.iter().copied() {
            if let Some(position) = action.pointer_position() {
                to_global(position)?;
            }
            match action {
                SyntheticInputAction::Click { button, .. }
                    if !(0x110..=0x117).contains(&button) =>
                {
                    return None;
                }
                SyntheticInputAction::Scroll { dx, dy, .. }
                    if !dx.is_finite()
                        || !dy.is_finite()
                        || dx.abs() > 1_000.0
                        || dy.abs() > 1_000.0 =>
                {
                    return None;
                }
                SyntheticInputAction::KeyPress { code } if code > 0x2ff => return None,
                _ => {}
            }
        }

        let mut events = Vec::with_capacity(actions.len() * 3);
        for action in actions.iter().copied() {
            match action {
                SyntheticInputAction::PointerMove { position } => {
                    let (x, y) = to_global(position)?;
                    events.push(InputEvent::pointer_move_to(x, y));
                }
                SyntheticInputAction::Click { position, button } => {
                    let (x, y) = to_global(position)?;
                    events.push(InputEvent::pointer_move_to(x, y));
                    events.push(InputEvent::PointerButton {
                        button,
                        state: ButtonState::Pressed,
                    });
                    events.push(InputEvent::PointerButton {
                        button,
                        state: ButtonState::Released,
                    });
                }
                SyntheticInputAction::Scroll { position, dx, dy } => {
                    let (x, y) = to_global(position)?;
                    events.push(InputEvent::pointer_move_to(x, y));
                    events.push(InputEvent::PointerAxis(
                        aegis_model::input::PointerAxisFrame::from_values(
                            self.epoch.elapsed().as_millis() as u32,
                            Some(aegis_model::input::PointerAxisSource::Continuous),
                            dx,
                            dy,
                        ),
                    ));
                }
                SyntheticInputAction::KeyPress { code } => {
                    events.push(InputEvent::Key {
                        code,
                        state: ButtonState::Pressed,
                    });
                    events.push(InputEvent::Key {
                        code,
                        state: ButtonState::Released,
                    });
                }
            }
        }
        Some(events)
    }

    /// Enumerate live toplevel windows. The shell uses this for the overview
    /// and any chrome that needs a list of windows. Reads current metadata;
    /// mutation happens through xdg_toplevel requests from the owning client.
    pub fn windows(&self) -> Vec<aegis_model::window::Window> {
        let visible = self.visible();
        self.windows_in_set(&visible)
    }

    /// Process-bound window identities for the first-party AT-SPI adapter.
    /// Keeping this separate from `windows()` prevents kernel process
    /// credentials from leaking into ordinary observation responses.
    pub fn accessibility_window_bindings(&self) -> Vec<aegis_semantic::AccessibilityWindowBinding> {
        let visible = self.visible();
        self.state
            .live_surfaces()
            .map(|surface| unsafe { &*surface })
            .filter(|surface| {
                surface.mapped
                    && !surface.xdg_toplevel.is_null()
                    && visible.contains(&surface.window.id)
                    && self.state.authority.interaction_domain_observes_window(
                        HUMAN_INTERACTION_DOMAIN,
                        surface.window.id,
                    )
            })
            .filter_map(|surface| {
                let process_id = self
                    .state
                    .client_process_ids
                    .get(&surface.client_id)
                    .copied()?;
                let mut window = surface.window.clone();
                window.read_only = !self
                    .state
                    .authority
                    .seat_controls_window(HUMAN_SEAT, window.id);
                window.state.activated = self.seat_focuses_window(HUMAN_SEAT, window.id);
                Some(aegis_semantic::AccessibilityWindowBinding { window, process_id })
            })
            .collect()
    }

    /// Enumerate the presentation-visible windows. During a workspace slide
    /// this includes retained source pages so their compositor-owned shadows
    /// remain stable until the page leaves the output.
    pub fn render_windows(&self) -> Vec<aegis_model::window::Window> {
        let visible = self.render_visible();
        self.windows_in_set(&visible)
    }

    /// Enumerate every mapped toplevel across all workspaces, not just the
    /// visible set. The dock consumes this so its running-window strip is
    /// global; ordering follows the same stable live-surface walk as
    /// [`Self::windows`], and the same interaction-domain observation filter
    /// applies.
    pub fn all_windows(&self) -> Vec<aegis_model::window::Window> {
        self.windows_filtered(|_| true)
    }

    fn windows_in_set(
        &self,
        visible: &std::collections::HashSet<aegis_model::window::WindowId>,
    ) -> Vec<aegis_model::window::Window> {
        self.windows_filtered(|s| visible.contains(&s.window.id))
    }

    fn windows_filtered(
        &self,
        in_set: impl Fn(&SurfaceRec) -> bool,
    ) -> Vec<aegis_model::window::Window> {
        self.state
            .live_surfaces()
            .map(|p| unsafe { &*p })
            .filter(|s| {
                s.mapped
                    && !s.xdg_toplevel.is_null()
                    && in_set(s)
                    && self
                        .state
                        .authority
                        .interaction_domain_observes_window(HUMAN_INTERACTION_DOMAIN, s.window.id)
            })
            .map(|s| {
                let mut w = s.window.clone();
                w.read_only = !self.state.authority.seat_controls_window(HUMAN_SEAT, w.id);
                w.state.activated = self.seat_focuses_window(HUMAN_SEAT, w.id);
                // Publish only in-flight transitions; settled ones are noise
                // to chrome and IPC consumers (ADR-0029).
                let target = aegis_model::Rect {
                    origin: w.position,
                    size: w.size,
                };
                if w.transition
                    .and_then(|t| t.rect_at(target, self.now_ms()))
                    .is_none()
                {
                    w.transition = None;
                }
                w
            })
            .collect()
    }

    /// Content hash of exactly what [`Self::windows`] would publish, computed
    /// without cloning any `Window`. The frame loop compares this per frame
    /// and rebuilds the owned snapshot only when it moves; a collision would
    /// only stall a refresh until the next change.
    pub fn windows_signature(&self) -> u64 {
        let visible = self.visible();
        self.windows_signature_filtered(|s| visible.contains(&s.window.id))
    }

    /// Content hash of exactly what [`Self::all_windows`] would publish, the
    /// global counterpart of [`Self::windows_signature`]. The frame loop gates
    /// the dock's snapshot push on it just like the visible-set hash.
    pub fn all_windows_signature(&self) -> u64 {
        self.windows_signature_filtered(|_| true)
    }

    fn windows_signature_filtered(&self, in_set: impl Fn(&SurfaceRec) -> bool) -> u64 {
        use std::hash::{Hash, Hasher};
        let now = self.now_ms();
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for s in self
            .state
            .live_surfaces()
            .map(|p| unsafe { &*p })
            .filter(|s| {
                s.mapped
                    && !s.xdg_toplevel.is_null()
                    && in_set(s)
                    && self
                        .state
                        .authority
                        .interaction_domain_observes_window(HUMAN_INTERACTION_DOMAIN, s.window.id)
            })
        {
            let w = &s.window;
            w.id.hash(&mut hasher);
            w.title.as_deref().hash(&mut hasher);
            w.app_id.as_deref().hash(&mut hasher);
            w.parent.hash(&mut hasher);
            w.size_hints.min_w.hash(&mut hasher);
            w.size_hints.min_h.hash(&mut hasher);
            w.size_hints.max_w.hash(&mut hasher);
            w.size_hints.max_h.hash(&mut hasher);
            w.state.maximized.hash(&mut hasher);
            w.state.fullscreen.hash(&mut hasher);
            w.state.resizing.hash(&mut hasher);
            // `windows()` overrides these two from live seat state.
            self.seat_focuses_window(HUMAN_SEAT, w.id).hash(&mut hasher);
            (!self.state.authority.seat_controls_window(HUMAN_SEAT, w.id)).hash(&mut hasher);
            w.minimized.hash(&mut hasher);
            w.always_on_top.hash(&mut hasher);
            (w.layout_role as u8).hash(&mut hasher);
            w.position.x.hash(&mut hasher);
            w.position.y.hash(&mut hasher);
            w.size.w.hash(&mut hasher);
            w.size.h.hash(&mut hasher);
            // Only in-flight transitions are published; settled ones read as
            // `None` in the snapshot (ADR-0029).
            let target = aegis_model::Rect {
                origin: w.position,
                size: w.size,
            };
            let published = w.transition.filter(|t| t.rect_at(target, now).is_some());
            published.is_some().hash(&mut hasher);
            if let Some(t) = published {
                t.from.origin.x.hash(&mut hasher);
                t.from.origin.y.hash(&mut hasher);
                t.from.size.w.hash(&mut hasher);
                t.from.size.h.hash(&mut hasher);
                t.started_ms.hash(&mut hasher);
                t.duration_ms.hash(&mut hasher);
            }
        }
        hasher.finish()
    }

    /// Whether compositor-owned physical chrome may mutate this window.
    /// Presentation-only mirrors deliberately return false even though they
    /// remain visible and can be transferred through Interaction Domain management.
    pub fn human_controls_window(&self, window: aegis_model::window::WindowId) -> bool {
        self.state
            .authority
            .seat_controls_window(HUMAN_SEAT, window)
    }

    pub(crate) fn seat_focuses_window(
        &self,
        seat: SeatId,
        window: aegis_model::window::WindowId,
    ) -> bool {
        let Some(runtime) = self.state.seat_runtime(seat) else {
            return false;
        };
        if runtime.keyboard_focus.is_null() {
            return false;
        }
        let focused =
            unsafe { ffi::wl_resource_get_user_data(runtime.keyboard_focus) as *mut SurfaceRec };
        let root = unsafe { surface_root_toplevel(focused) };
        !root.is_null() && unsafe { (*root).window.id == window }
    }

    /// Ask a toplevel to close by posting `xdg_toplevel.close`. The client
    /// responds by destroying its `xdg_toplevel` (and usually the surface).
    /// No-op if `surface_id` does not name a live toplevel or the physical
    /// human seat has observation-only authority for it.
    pub fn close_toplevel(&mut self, surface_id: aegis_model::window::WindowId) {
        if !self.human_controls_window(surface_id) {
            return;
        }
        for p in self.state.live_surfaces() {
            let s = unsafe { &*p };
            if s.window.id == surface_id && !s.xdg_toplevel.is_null() {
                unsafe {
                    ffi::wl_resource_post_event(s.xdg_toplevel, ffi::XDG_TOPLEVEL_CLOSE);
                }
                unsafe { ffi::wl_display_flush_clients(self.state.display) };
                return;
            }
        }
    }

    /// Minimize a toplevel from compositor chrome or IPC. This is the
    /// compositor-side counterpart of the client's
    /// `xdg_toplevel.set_minimized` request and shares the same focus cleanup.
    /// Presentation-only mirrors are immutable from the physical session.
    pub fn minimize_toplevel(&mut self, surface_id: aegis_model::window::WindowId) {
        if !self.human_controls_window(surface_id) {
            return;
        }
        let rec = self.find_surface_by_window_id(surface_id);
        if rec.is_null() || unsafe { (*rec).xdg_toplevel.is_null() } {
            return;
        }
        unsafe { minimize_toplevel_record(rec) };
    }

    /// Set or clear compositor-managed maximization. The existing
    /// `reconfigure_with_state` path owns work-area placement and restores the
    /// saved floating rectangle when maximization is cleared.
    pub fn set_toplevel_maximized(
        &mut self,
        surface_id: aegis_model::window::WindowId,
        maximized: bool,
    ) -> bool {
        if !self.human_controls_window(surface_id) {
            return false;
        }
        let rec = self.find_surface_by_window_id(surface_id);
        if rec.is_null()
            || unsafe {
                (*rec).xdg_toplevel.is_null()
                    || (*rec).window.minimized
                    || (*rec).window.state.fullscreen
                    || (*rec).window.state.maximized == maximized
            }
        {
            return false;
        }
        self.change_keyboard_focus(unsafe { (*rec).resource });
        unsafe {
            (*rec).window.state.maximized = maximized;
            reconfigure_with_state(rec);
            ffi::wl_display_flush_clients(self.state.display);
        }
        true
    }

    /// Set or clear the compositor-internal always-on-top flag. Enabling
    /// raises the window so its surface tree enters the always-on-top band at
    /// the top of the stacking order; disabling only clears the flag and
    /// leaves the stacking position untouched. No xdg configure is sent: the
    /// protocol has no always-on-top state. Idempotent, and a no-op when the
    /// physical human seat does not control the window.
    pub fn set_toplevel_always_on_top(
        &mut self,
        surface_id: aegis_model::window::WindowId,
        on_top: bool,
    ) {
        if !self.human_controls_window(surface_id) {
            return;
        }
        let rec = self.find_surface_by_window_id(surface_id);
        if rec.is_null()
            || unsafe { (*rec).xdg_toplevel.is_null() || (*rec).window.always_on_top == on_top }
        {
            return;
        }
        unsafe { (*rec).window.always_on_top = on_top };
        if on_top {
            let resource = unsafe { (*rec).resource };
            self.raise_toplevel(resource);
        }
    }

    /// Mark a toplevel as activated (or not) and emit a configure so the
    /// client updates its focus state. The shell calls this when keyboard
    /// focus changes; M1's click-to-focus already posts keyboard enter/leave,
    /// and this complements it with the toplevel-state side.
    pub fn set_toplevel_activated(
        &mut self,
        surface_id: aegis_model::window::WindowId,
        activated: bool,
    ) {
        for p in self.state.live_surfaces() {
            let s = unsafe { &mut *p };
            if s.window.id == surface_id && !s.xdg_toplevel.is_null() {
                if s.window.state.activated == activated {
                    return;
                }
                s.window.state.activated = activated;
                unsafe { reconfigure_with_state(s as *mut SurfaceRec) };
                unsafe { ffi::wl_display_flush_clients(self.state.display) };
                return;
            }
        }
    }

    /// Begin an interactive move from the shell (for example, an overview
    /// drag). Unlike the client-initiated
    /// `xdg_toplevel.move` path, no serial validation is performed — the
    /// compositor is initiating the grab itself. No-op if a grab is already
    /// active, the surface is not a live toplevel, or the active seat does not
    /// control the surface. The latter check is deliberately repeated below
    /// the main-loop command gateway so future compositor callers cannot turn
    /// an observation mirror into an input target.
    pub fn start_interactive_move(&mut self, surface_id: aegis_model::window::WindowId) {
        if self.state.interactive.is_some()
            || !self
                .state
                .authority
                .seat_controls_window(self.state.active_seat, surface_id)
        {
            return;
        }
        let rec = self.find_surface_by_window_id(surface_id);
        if rec.is_null() || unsafe { (*rec).xdg_toplevel.is_null() } {
            return;
        }
        self.change_keyboard_focus(unsafe { (*rec).resource });
        unsafe {
            let layout_changed =
                (*rec).window.layout_role != aegis_model::layout::LayoutRole::Floating;
            (*rec).window.layout_role = aegis_model::layout::LayoutRole::Floating;
            (*rec).layout_target = None;
            let state_changed = (*rec).window.state.maximized || (*rec).window.state.fullscreen;
            (*rec).window.state.maximized = false;
            (*rec).window.state.fullscreen = false;
            if state_changed || layout_changed {
                reconfigure_with_state(rec);
            }
        }
        self.state.interactive = Some(aegis_model::window::Interactive::Move {
            window_id: surface_id,
            origin: (self.state.pointer_x, self.state.pointer_y),
            start_position: unsafe { (*rec).position },
        });
        self.state.compositor_pointer_grab = false;
    }

    /// Begin an interactive move of a read-only mirror, initiated by trusted
    /// shell chrome (the mirror guard's drag). Unlike
    /// [`start_interactive_move`](Self::start_interactive_move) this targets a
    /// window the human only observes: keyboard focus and stacking stay
    /// untouched — focusing a mirror would defocus the human's own window,
    /// and moving must not grant the Agent's window any new prominence.
    /// `origin` is the physical cursor position supplied by chrome, because
    /// pointer motion is not forwarded to the server while the guard captures
    /// the pointer. Agent input uses target-local coordinates, so an applied
    /// operation stays correct while the mirror moves.
    pub fn start_mirror_move(
        &mut self,
        surface_id: aegis_model::window::WindowId,
        origin: (f32, f32),
    ) {
        let observed_mirror = !self
            .state
            .authority
            .seat_controls_window(HUMAN_SEAT, surface_id)
            && self.state.authority.interaction_domain_observes_window(
                aegis_model::interaction_domain::HUMAN_INTERACTION_DOMAIN,
                surface_id,
            );
        if self.state.interactive.is_some() || !observed_mirror {
            return;
        }
        let rec = self.find_surface_by_window_id(surface_id);
        if rec.is_null() || unsafe { (*rec).xdg_toplevel.is_null() } {
            return;
        }
        unsafe {
            let layout_changed =
                (*rec).window.layout_role != aegis_model::layout::LayoutRole::Floating;
            (*rec).window.layout_role = aegis_model::layout::LayoutRole::Floating;
            (*rec).layout_target = None;
            let state_changed = (*rec).window.state.maximized || (*rec).window.state.fullscreen;
            (*rec).window.state.maximized = false;
            (*rec).window.state.fullscreen = false;
            if state_changed || layout_changed {
                reconfigure_with_state(rec);
            }
        }
        self.state.interactive = Some(aegis_model::window::Interactive::Move {
            window_id: surface_id,
            origin,
            start_position: unsafe { (*rec).position },
        });
        self.state.compositor_pointer_grab = false;
    }

    /// Begin an interactive resize from the shell. Same serial-less contract
    /// as [`start_interactive_move`](Self::start_interactive_move).
    pub fn start_interactive_resize(
        &mut self,
        surface_id: aegis_model::window::WindowId,
        edges: aegis_model::window::ResizeEdges,
    ) {
        if self.state.interactive.is_some()
            || !self
                .state
                .authority
                .seat_controls_window(self.state.active_seat, surface_id)
        {
            return;
        }
        let rec = self.find_surface_by_window_id(surface_id);
        if rec.is_null() || unsafe { (*rec).xdg_toplevel.is_null() } {
            return;
        }
        if edges.is_none() {
            return;
        }
        self.change_keyboard_focus(unsafe { (*rec).resource });
        unsafe {
            (*rec).window.layout_role = aegis_model::layout::LayoutRole::Floating;
            (*rec).layout_target = None;
            (*rec).window.state.maximized = false;
            (*rec).window.state.fullscreen = false;
            (*rec).window.state.resizing = true;
            reconfigure_with_state(rec);
        }
        self.state.interactive = Some(aegis_model::window::Interactive::Resize {
            window_id: surface_id,
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
        self.state.compositor_pointer_grab = false;
    }

    /// Apply an explicit floating-window rectangle without simulating a
    /// pointer grab. Invalid or stale targets are no-ops. Client size hints
    /// are authoritative; callers observe the clamped result through the next
    /// window snapshot or journal event.
    pub fn set_window_geometry(
        &mut self,
        window_id: aegis_model::window::WindowId,
        rect: aegis_model::Rect,
    ) -> bool {
        if !self.human_controls_window(window_id) || rect.size.w <= 0 || rect.size.h <= 0 {
            return false;
        }
        let rec = self.find_surface_by_window_id(window_id);
        if rec.is_null() || unsafe { (*rec).xdg_toplevel.is_null() || !(*rec).mapped } {
            return false;
        }
        if self.state.interactive.is_some()
            || self.state.drag.is_some()
            || self.state.implicit_grab_active
        {
            return false;
        }
        let hints = unsafe { (*rec).window.size_hints };
        let size = clamp_size_to_hints(rect.size, hints);
        unsafe {
            let unchanged = (*rec).position == rect.origin
                && (*rec).window.size == size
                && (*rec).window.layout_role == aegis_model::layout::LayoutRole::Floating
                && !(*rec).window.state.maximized
                && !(*rec).window.state.fullscreen;
            if unchanged {
                return false;
            }
            let old = aegis_model::Rect {
                origin: (*rec).position,
                size: (*rec).window.size,
            };
            (*rec).position = rect.origin;
            (*rec).window.position = rect.origin;
            (*rec).window.size = size;
            (*rec).window.layout_role = aegis_model::layout::LayoutRole::Floating;
            (*rec).layout_target = None;
            (*rec).window.state.maximized = false;
            (*rec).window.state.fullscreen = false;
            self.note_transition(rec, old);
            reconfigure_with_size(rec, size.w, size.h);
            ffi::wl_display_flush_clients(self.state.display);
        }
        true
    }

    /// Whether an interactive grab (move or resize) is currently active.
    /// The shell uses this to change the cursor or suppress overview
    /// animations during a grab.
    pub fn interactive(&self) -> Option<aegis_model::window::Interactive> {
        self.state.interactive
    }

    /// Whether a client currently owns the data-device pointer grab. The
    /// shell uses this to keep forwarding motion/release while the pointer is
    /// visually over compositor chrome, allowing the server to emit leave or
    /// cancel the drag instead of stranding it.
    pub fn drag_active(&self) -> bool {
        self.state.drag.is_some()
    }

    /// Focus a toplevel by its surface id. Used by the shell's window list
    /// (click-to-focus from chrome) and by future overview / launcher
    /// surfaces. Equivalent to the click-to-focus path driven from a pointer
    /// button press, but initiated by the compositor. No-op if the id does
    /// not name a live toplevel.
    ///
    /// When the target lives on a hidden workspace, its output first switches
    /// to that workspace (the usual slide animation and hidden-focus cleanup
    /// in [`Self::switch_workspace_to`]), so a focus request never lands on a
    /// window the user cannot see.
    pub fn focus_surface_by_id(&mut self, surface_id: aegis_model::window::WindowId) {
        self.focus_surface_by_id_reveal(surface_id, true);
    }

    /// Focus a toplevel by id like [`Self::focus_surface_by_id`], but with
    /// explicit control over view switching. With `reveal = false` the output
    /// never switches workspace: a window on a hidden workspace is only raised
    /// within its own workspace's z-order and unminimized — it does NOT receive
    /// keyboard focus (the physical seat must never type into an invisible
    /// window). A window on the current workspace is focused normally.
    pub fn focus_surface_by_id_reveal(
        &mut self,
        surface_id: aegis_model::window::WindowId,
        reveal: bool,
    ) {
        let rec = self.find_surface_by_window_id(surface_id);
        if rec.is_null() {
            return;
        }
        let off_workspace = self
            .state
            .workspaces
            .workspace_of(surface_id)
            .filter(|workspace| {
                self.state
                    .workspaces
                    .workspace(*workspace)
                    .and_then(|ws| self.state.workspaces.current_workspace(ws.output))
                    != Some(*workspace)
            });
        if let Some(workspace) = off_workspace {
            if reveal {
                // The switch ends with `drop_focus_if_hidden`; the focus below is
                // applied afterwards so it survives the cleanup.
                self.switch_workspace_to(workspace);
            } else {
                // Agent workspace isolation (ADR-0118): raise and
                // unminimize the window in place so it is
                // ready when the user switches over, but never switch the view
                // or type into an invisible window.
                unsafe {
                    if (*rec).xdg_toplevel.is_null() || !(*rec).mapped {
                        return;
                    }
                }
                // `WorkspaceModel` has no raise method; re-placing on the same
                // workspace removes then re-pushes the id, landing it at the
                // most-recently-placed (top) end of the workspace's z-order.
                // The workspace stays non-empty, so the invariant/reap work in
                // `place_toplevel` is a no-op here.
                self.state.workspaces.place_toplevel(workspace, surface_id);
                unsafe { self.unminimize_toplevel(rec) };
                unsafe { ffi::wl_display_flush_clients(self.state.display) };
                return;
            }
        }
        unsafe {
            if (*rec).xdg_toplevel.is_null() || !(*rec).mapped {
                return;
            }
            self.unminimize_toplevel(rec);
        }
        let resource = unsafe { (*rec).resource };
        self.change_keyboard_focus(resource);
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }

    /// Reverse a minimize in place: restore the saved floating rect, clear
    /// the flag, and fly back out of the same dock icon (or stub point) the
    /// minimize flight landed on.
    unsafe fn unminimize_toplevel(&mut self, rec: *mut SurfaceRec) {
        unsafe {
            if !(*rec).window.minimized {
                return;
            }
            let saved = (*rec).saved_floating_rect.unwrap_or(aegis_model::Rect {
                origin: aegis_model::Point { x: 100, y: 100 },
                size: aegis_model::Size { w: 800, h: 600 },
            });
            let window_id = (*rec).window.id;
            // The flight leaves from the same dock icon (or stub point)
            // the minimize flight landed on.
            let from = minimize_flight_target(&self.state, window_id, saved);
            let styled = self.state.minimize_targets.contains_key(&window_id);
            (*rec).position = saved.origin;
            (*rec).window.position = saved.origin;
            (*rec).window.size = saved.size;
            (*rec).window.minimized = false;
            if styled && !self.state.reduced_motion {
                // Carry the minimize effect so a genie flight warps back
                // out of the icon; the scene derives the reversed
                // direction from the cleared minimized flag.
                (*rec).window.transition = Some(minimize_transition(&self.state, window_id, from));
            } else {
                self.note_transition(rec, from);
            }
        }
    }

    /// Surface id of the toplevel currently holding keyboard focus, if any.
    /// Returns `None` when no surface is focused or the focus is not a
    /// toplevel. Used by the keybind dispatcher to target the focused window.
    pub fn focused_toplevel_id(&self) -> Option<aegis_model::window::WindowId> {
        let f = self.state.keyboard_focus;
        if f.is_null() {
            return None;
        }
        self.state
            .live_surfaces()
            .map(|p| unsafe { &*p })
            .find(|s| s.resource == f)
            .and_then(|s| {
                // Keyboard focus may rest on a subsurface (it received the
                // pointer click); the window it belongs to is the root.
                let root =
                    unsafe { surface_root_toplevel(s as *const SurfaceRec as *mut SurfaceRec) };
                if root.is_null() {
                    return None;
                }
                let root = unsafe { &*root };
                if root.xdg_toplevel.is_null() {
                    None
                } else {
                    Some(root.window.id)
                }
            })
    }

    /// The last cursor shape requested by the focused client
    /// (`wp_cursor_shape_device_v1.set_shape`), or 0 for the default arrow.
    /// The renderer consults this to pick which cursor to paint.
    pub fn cursor_shape(&self) -> u32 {
        self.state.cursor_shape
    }

    /// Whether the outer host cursor must be hidden because the focused
    /// client supplied a custom cursor surface or explicitly selected no
    /// cursor.
    pub fn cursor_hidden(&self) -> bool {
        self.state.cursor_hidden
    }

    /// Whether the focused client supplied a wl_surface cursor. Unlike an
    /// explicitly hidden cursor, this surface moves with the pointer and must
    /// participate in compositor damage; it cannot be represented by the
    /// compositor-owned KMS cursor sprite.
    pub fn client_cursor_surface_active(&self) -> bool {
        !self.state.cursor_surface.is_null()
    }

    /// Cursor shape owned by compositor-side window manipulation. Active
    /// grabs take precedence; otherwise a floating window's outer resize
    /// margin advertises the edge/corner before the user presses it.
    pub fn compositor_cursor_shape(&self) -> Option<u32> {
        match self.state.interactive {
            Some(aegis_model::window::Interactive::Move { .. }) => Some(17), // grabbing
            Some(aegis_model::window::Interactive::Resize { edges, .. }) => {
                Some(resize_cursor_shape(edges))
            }
            None => self
                .resize_target_at(
                    self.state.pointer_x,
                    self.state.pointer_y,
                    aegis_model::window::RESIZE_OUTER_MARGIN,
                )
                .map(|(_, edges)| resize_cursor_shape(edges)),
        }
    }

    /// Drain text-input state committed by the focused inner client. The
    /// nested backend mirrors each state to the host compositor's IME.
    pub fn take_text_input_states(&mut self) -> Vec<aegis_model::input::TextInputState> {
        std::mem::take(&mut self.state.pending_text_input_states)
    }

    /// Route one host IME event to the enabled text-input object belonging to
    /// the keyboard-focused inner client.
    pub fn text_input_event(&mut self, event: &aegis_model::input::TextInputEvent) {
        unsafe { extensions::forward_text_input_event(self.state.as_mut(), event) };
    }

    /// Forward a host touchpad gesture to gesture objects belonging to the
    /// client that held pointer focus when the gesture began.
    pub fn pointer_gesture_event(&mut self, event: &aegis_model::input::PointerGestureEvent) {
        use aegis_model::input::PointerGestureEvent::*;
        unsafe {
            match *event {
                SwipeBegin { time, fingers } => {
                    let surface = self.state.pointer_focus;
                    if surface.is_null() {
                        return;
                    }
                    let client = ffi::wl_resource_get_client(surface);
                    self.state.swipe_gesture_client = client;
                    let serial = ffi::wl_display_next_serial(self.state.display);
                    for resource in self
                        .state
                        .pointer_gesture_swipes
                        .iter()
                        .copied()
                        .filter(|r| ffi::wl_resource_get_client(*r) == client)
                    {
                        ffi::wl_resource_post_event(
                            resource,
                            ffi::ZWP_POINTER_GESTURE_SWIPE_V1_BEGIN,
                            serial,
                            time,
                            surface,
                            fingers,
                        );
                    }
                }
                SwipeUpdate { time, dx, dy } => {
                    let client = self.state.swipe_gesture_client;
                    if client.is_null() {
                        return;
                    }
                    let dx = ffi::wl_fixed_from_f32(dx);
                    let dy = ffi::wl_fixed_from_f32(dy);
                    for resource in self
                        .state
                        .pointer_gesture_swipes
                        .iter()
                        .copied()
                        .filter(|r| ffi::wl_resource_get_client(*r) == client)
                    {
                        ffi::wl_resource_post_event(
                            resource,
                            ffi::ZWP_POINTER_GESTURE_SWIPE_V1_UPDATE,
                            time,
                            dx,
                            dy,
                        );
                    }
                }
                SwipeEnd { time, cancelled } => {
                    let client = std::mem::replace(
                        &mut self.state.swipe_gesture_client,
                        std::ptr::null_mut(),
                    );
                    if client.is_null() {
                        return;
                    }
                    let serial = ffi::wl_display_next_serial(self.state.display);
                    for resource in self
                        .state
                        .pointer_gesture_swipes
                        .iter()
                        .copied()
                        .filter(|r| ffi::wl_resource_get_client(*r) == client)
                    {
                        ffi::wl_resource_post_event(
                            resource,
                            ffi::ZWP_POINTER_GESTURE_SWIPE_V1_END,
                            serial,
                            time,
                            cancelled as i32,
                        );
                    }
                }
                PinchBegin { time, fingers } => {
                    let surface = self.state.pointer_focus;
                    if surface.is_null() {
                        return;
                    }
                    let client = ffi::wl_resource_get_client(surface);
                    self.state.pinch_gesture_client = client;
                    let serial = ffi::wl_display_next_serial(self.state.display);
                    for resource in self
                        .state
                        .pointer_gesture_pinches
                        .iter()
                        .copied()
                        .filter(|r| ffi::wl_resource_get_client(*r) == client)
                    {
                        ffi::wl_resource_post_event(
                            resource,
                            ffi::ZWP_POINTER_GESTURE_PINCH_V1_BEGIN,
                            serial,
                            time,
                            surface,
                            fingers,
                        );
                    }
                }
                PinchUpdate {
                    time,
                    dx,
                    dy,
                    scale,
                    rotation,
                } => {
                    let client = self.state.pinch_gesture_client;
                    if client.is_null() {
                        return;
                    }
                    let values = [dx, dy, scale, rotation].map(ffi::wl_fixed_from_f32);
                    for resource in self
                        .state
                        .pointer_gesture_pinches
                        .iter()
                        .copied()
                        .filter(|r| ffi::wl_resource_get_client(*r) == client)
                    {
                        ffi::wl_resource_post_event(
                            resource,
                            ffi::ZWP_POINTER_GESTURE_PINCH_V1_UPDATE,
                            time,
                            values[0],
                            values[1],
                            values[2],
                            values[3],
                        );
                    }
                }
                PinchEnd { time, cancelled } => {
                    let client = std::mem::replace(
                        &mut self.state.pinch_gesture_client,
                        std::ptr::null_mut(),
                    );
                    if client.is_null() {
                        return;
                    }
                    let serial = ffi::wl_display_next_serial(self.state.display);
                    for resource in self
                        .state
                        .pointer_gesture_pinches
                        .iter()
                        .copied()
                        .filter(|r| ffi::wl_resource_get_client(*r) == client)
                    {
                        ffi::wl_resource_post_event(
                            resource,
                            ffi::ZWP_POINTER_GESTURE_PINCH_V1_END,
                            serial,
                            time,
                            cancelled as i32,
                        );
                    }
                }
                HoldBegin { time, fingers } => {
                    let surface = self.state.pointer_focus;
                    if surface.is_null() {
                        return;
                    }
                    let client = ffi::wl_resource_get_client(surface);
                    self.state.hold_gesture_client = client;
                    let serial = ffi::wl_display_next_serial(self.state.display);
                    for resource in self
                        .state
                        .pointer_gesture_holds
                        .iter()
                        .copied()
                        .filter(|r| ffi::wl_resource_get_client(*r) == client)
                    {
                        ffi::wl_resource_post_event(
                            resource,
                            ffi::ZWP_POINTER_GESTURE_HOLD_V1_BEGIN,
                            serial,
                            time,
                            surface,
                            fingers,
                        );
                    }
                }
                HoldEnd { time, cancelled } => {
                    let client = std::mem::replace(
                        &mut self.state.hold_gesture_client,
                        std::ptr::null_mut(),
                    );
                    if client.is_null() {
                        return;
                    }
                    let serial = ffi::wl_display_next_serial(self.state.display);
                    for resource in self
                        .state
                        .pointer_gesture_holds
                        .iter()
                        .copied()
                        .filter(|r| ffi::wl_resource_get_client(*r) == client)
                    {
                        ffi::wl_resource_post_event(
                            resource,
                            ffi::ZWP_POINTER_GESTURE_HOLD_V1_END,
                            serial,
                            time,
                            cancelled as i32,
                        );
                    }
                }
            }
        }
    }
}
