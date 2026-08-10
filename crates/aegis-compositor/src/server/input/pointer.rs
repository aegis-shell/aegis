use crate::*;

const BTN_LEFT: u32 = 0x110;
const BTN_RIGHT: u32 = 0x111;
const TOP_BORDER_DOUBLE_CLICK_MS: u64 = 450;
const TOP_BORDER_CLICK_DISTANCE: f32 = 4.0;
const TOP_BORDER_DRAG_THRESHOLD: f32 = 3.0;

fn pointer_distance_squared(a: (f32, f32), b: (f32, f32)) -> f32 {
    let dx = a.0 - b.0;
    let dy = a.1 - b.1;
    dx * dx + dy * dy
}

fn is_top_edge_only(edges: aegis_model::window::ResizeEdges) -> bool {
    edges.has_top() && !edges.has_bottom() && !edges.has_left() && !edges.has_right()
}

fn is_matching_top_border_double_click(
    first: TopBorderClick,
    window_id: aegis_model::window::WindowId,
    pressed_at_ms: u64,
    position: (f32, f32),
) -> bool {
    first.window_id == window_id
        && pressed_at_ms.saturating_sub(first.released_at_ms) <= TOP_BORDER_DOUBLE_CLICK_MS
        && pointer_distance_squared(first.position, position)
            <= TOP_BORDER_CLICK_DISTANCE * TOP_BORDER_CLICK_DISTANCE
}

/// xdg-popup grabs use owner-events semantics: every surface belonging to
/// the client that owns the grab continues receiving pointer events normally.
/// The client decides whether a click on one of its other surfaces dismisses
/// the popup; the compositor dismisses only clicks outside that client.
pub(crate) fn popup_grab_allows_owner_event(
    grab_client: *mut ffi::wl_client,
    focus_client: *mut ffi::wl_client,
) -> bool {
    !grab_client.is_null() && grab_client == focus_client
}

/// The xdg role whose surface tree contains `surface`, or null for a
/// role-less tree. `wl_subsurface` parents are the only links followed here:
/// an xdg-popup's protocol parent is a separate role surface, not part of the
/// same input tree.
unsafe fn surface_xdg_role(mut surface: *mut SurfaceRec) -> *mut SurfaceRec {
    unsafe {
        for _ in 0..32 {
            if surface.is_null()
                || !(*surface).xdg_toplevel.is_null()
                || !(*surface).xdg_popup.is_null()
            {
                return surface;
            }
            surface = (*surface).parent;
        }
        std::ptr::null_mut()
    }
}

/// Resolve click-to-focus at the xdg role boundary.
///
/// A grabbing popup is pinned as keyboard focus. A non-grabbing popup never
/// receives keyboard focus: activate its owning toplevel. Likewise, a
/// `wl_subsurface` is only a pointer-focus target inside its parent's surface
/// tree; keyboard focus belongs to the xdg role surface. Chrome implements
/// browser bubbles as subsurfaces and closes them if the compositor focuses
/// the child directly.
unsafe fn xdg_role_aware_keyboard_target(
    pointer_surface: *mut SurfaceRec,
    current_keyboard_focus: *mut ffi::wl_resource,
    grabbed_popup: Option<*mut SurfaceRec>,
) -> *mut ffi::wl_resource {
    unsafe {
        if let Some(popup) = grabbed_popup {
            return (*popup).resource;
        }
        let role = surface_xdg_role(pointer_surface);
        if role.is_null() {
            return if pointer_surface.is_null() {
                std::ptr::null_mut()
            } else {
                (*pointer_surface).resource
            };
        }
        if (*role).xdg_popup.is_null() {
            return (*role).resource;
        }
        let popup_root = surface_root_toplevel(role);
        if popup_root.is_null() {
            return current_keyboard_focus;
        }
        (*popup_root).resource
    }
}

/// Resolve direct resize in bottom-to-top stacking order. Every visible
/// window rectangle is an input barrier: when a foreground window contains
/// the point, it clears any resize candidate contributed by a lower window.
/// A resizable window additionally owns its outer logical-pixel margin.
fn stacked_resize_target<T: Copy>(
    layers: impl IntoIterator<Item = (T, aegis_model::window::Window, bool)>,
    x: f32,
    y: f32,
    margin: f32,
) -> Option<(T, aegis_model::window::ResizeEdges)> {
    let mut target = None;
    for (id, window, resizable) in layers {
        let edges = if resizable {
            window.resize_edges_at(x, y, margin)
        } else {
            aegis_model::window::ResizeEdges::NONE
        };
        if window.contains_point(x, y) || !edges.is_none() {
            target = (!edges.is_none()).then_some((id, edges));
        }
    }
    target
}

impl Server {
    /// Resolve the pointer target after a compositor-side stacking change
    /// without inventing physical motion. `None` means an active grab owns
    /// focus and it must remain pinned; `Some(null)` means the stationary
    /// pointer is over no client surface.
    pub(crate) fn stationary_pointer_rehit_target(&self) -> Option<*mut ffi::wl_resource> {
        if self.state.drag.is_some()
            || self.state.interactive.is_some()
            || self.state.implicit_grab_active
            || self.state.compositor_pointer_grab
        {
            return None;
        }
        let (x, y) = (self.state.pointer_x, self.state.pointer_y);
        let focus = if self.state.active_seat == HUMAN_SEAT
            && self
                .resize_target_at(x, y, aegis_model::window::RESIZE_OUTER_MARGIN)
                .is_some()
        {
            std::ptr::null_mut()
        } else {
            self.hit_test_focus(x, y)
        };
        Some(focus)
    }

    /// Keep Wayland pointer focus coherent with a window raised under a
    /// stationary cursor. Without this enter/leave transition, the next
    /// button or axis event is delivered to the surface that was topmost
    /// before the keyboard switch.
    pub(crate) fn rehit_pointer_after_stack_change(&mut self) {
        let Some(focus) = self.stationary_pointer_rehit_target() else {
            return;
        };
        if focus != self.state.pointer_focus {
            self.change_pointer_focus(focus);
            unsafe { ffi::wl_display_flush_clients(self.state.display) };
        }
    }

    /// Deltas for absolute-position devices (tablet proximity/axis events
    /// emulating a pointer): the event carries no delta channel, so
    /// difference against the last unconstrained absolute position.
    /// `pointer_motion` then advances that baseline.
    pub(crate) fn absolute_motion_deltas(&self, x: f32, y: f32) -> (f64, f64) {
        (
            f64::from(x - self.state.raw_pointer_x),
            f64::from(y - self.state.raw_pointer_y),
        )
    }

    pub(crate) fn pointer_motion(
        &mut self,
        x: f32,
        y: f32,
        dx: f64,
        dy: f64,
        dx_unaccel: f64,
        dy_unaccel: f64,
    ) {
        // Track the last unconstrained absolute position; absolute-device
        // callers (tablet emulation) difference it to derive their deltas.
        // The deltas themselves arrive with the event — deriving them from
        // the clamped absolute position would freeze relative-pointer clients
        // (game cameras) at output edges and during pointer locks.
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
        // the focused client (games, etc.). The deltas came with the event,
        // so they keep flowing even while `constrain_pointer_motion` pins
        // the absolute position above.
        self.post_relative_motion(dx, dy, dx_unaccel, dy_unaccel);
        // A second top-border click stays pending until intent is clear:
        // release maximizes, while movement beyond the jitter threshold turns
        // the same held click into a move grab from its original press point.
        if self.state.active_seat == HUMAN_SEAT
            && let Some(pending) = self.state.pending_top_border_double_click
            && pointer_distance_squared(pending.press_position, (x, y))
                >= TOP_BORDER_DRAG_THRESHOLD * TOP_BORDER_DRAG_THRESHOLD
        {
            self.state.pending_top_border_double_click = None;
            self.state.compositor_pointer_grab = false;
            self.start_interactive_move(pending.window_id);
            if let Some(aegis_model::window::Interactive::Move {
                origin,
                start_position,
                ..
            }) = self.state.interactive.as_mut()
            {
                *origin = pending.press_position;
                *start_position = pending.start_position;
                self.state.compositor_pointer_grab = true;
            }
        }
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
        // The direct-resize halo is compositor-owned. Suppress client focus
        // there so a lower window cannot receive hover or button preparation
        // through the foreground window's resize affordance.
        let focus = if self.state.active_seat == HUMAN_SEAT
            && self
                .resize_target_at(x, y, aegis_model::window::RESIZE_OUTER_MARGIN)
                .is_some()
        {
            std::ptr::null_mut()
        } else {
            self.hit_test_focus(x, y)
        };
        if focus != self.state.pointer_focus {
            self.change_pointer_focus(focus);
        }
        // Post motion to whichever client now holds focus.
        self.post_motion_to_focus(time);
    }

    pub(crate) fn pointer_button(&mut self, button: u32, state: aegis_model::input::ButtonState) {
        if self.state.session_lock_phase.is_active() {
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
            let implicit_grab_held = self.state.implicit_grab_active;
            self.state.implicit_grab_active = false;
            if self.state.drag.is_some() {
                unsafe { finish_drag(self.state.as_mut()) };
                return;
            }
            if implicit_grab_held {
                // The implicit grab pinned pointer focus to the pressed
                // surface for the whole hold. Now that the hold ended, what
                // is under the cursor may differ — notably a popup that
                // opened on the press and could not take focus mid-grab.
                unsafe {
                    schedule_pointer_rehit(self.state.as_mut(), Some(self.state.active_seat));
                }
            }
        }
        if !state.is_pressed()
            && button == BTN_LEFT
            && let Some(pending) = self.state.pending_top_border_double_click.take()
        {
            self.state.compositor_pointer_grab = false;
            self.state.last_top_border_click = None;
            self.set_toplevel_maximized(pending.window_id, true);
            return;
        }
        // Button release ends any active interactive grab. A compositor-side
        // border grab consumed its press, so consume the paired release too;
        // client-initiated grabs still receive the release as required.
        if !state.is_pressed() && self.state.interactive.is_some() {
            let consume = self.state.compositor_pointer_grab;
            let top_border_click = match self.state.interactive {
                Some(aegis_model::window::Interactive::Resize {
                    window_id,
                    edges,
                    origin,
                    start_position,
                    start_size,
                }) if consume
                    && button == BTN_LEFT
                    && is_top_edge_only(edges)
                    && pointer_distance_squared(
                        origin,
                        (self.state.pointer_x, self.state.pointer_y),
                    ) <= TOP_BORDER_DRAG_THRESHOLD * TOP_BORDER_DRAG_THRESHOLD =>
                {
                    let started_inside = origin.0 >= start_position.x as f32
                        && origin.0 < (start_position.x + start_size.w) as f32
                        && origin.1 >= start_position.y as f32
                        && origin.1 < (start_position.y + start_size.h) as f32;
                    (!started_inside).then_some(TopBorderClick {
                        window_id,
                        released_at_ms: self.state.now_ms(),
                        position: (self.state.pointer_x, self.state.pointer_y),
                    })
                }
                _ => None,
            };
            self.finish_interactive();
            if consume {
                self.state.last_top_border_click = top_border_click;
                return;
            }
        }

        let grabbed_popup = state
            .is_pressed()
            .then(|| topmost_grabbed_popup(&self.state, self.state.active_seat))
            .flatten();
        if let Some(popup) = grabbed_popup {
            unsafe {
                let focus_client = if self.state.pointer_focus.is_null() {
                    std::ptr::null_mut()
                } else {
                    ffi::wl_resource_get_client(self.state.pointer_focus)
                };
                let grab_client = ffi::wl_resource_get_client((*popup).xdg_popup);
                // Owner events continue through the ordinary button path.
                // Clicking another client or empty desktop dismisses the
                // topmost grab and consumes that outside click.
                if !popup_grab_allows_owner_event(grab_client, focus_client) {
                    let focus_after_dismissal = popup_keyboard_focus_after_dismissal(popup);
                    ffi::wl_resource_post_event((*popup).xdg_popup, ffi::XDG_POPUP_POPUP_DONE);
                    (*popup).popup_grabbed = false;
                    (*popup).popup_grab_seat = None;
                    (*popup).mapped = false;
                    self.change_keyboard_focus(focus_after_dismissal);
                    // The dismissed popup may have held pointer focus; re-hit
                    // so the surface underneath sees the enter without
                    // waiting for motion.
                    schedule_pointer_rehit(self.state.as_mut(), Some(self.state.active_seat));
                    return;
                }
            }
        }

        // Floating windows expose a compositor-owned outer margin for direct
        // resize. This runs before client button delivery, so the resize halo
        // never activates content in this window or one behind it. Tiled,
        // maximized, and fullscreen windows keep layout-owned geometry.
        // Borderless windows still need compositor-owned gestures that start
        // anywhere in their content. Super+left moves; Super+right resizes
        // from the nearest edge/corner. Both detach a layout-owned window so
        // tiling policy does not overwrite the interactive geometry.
        if self.state.active_seat == HUMAN_SEAT
            && state.is_pressed()
            && (button == BTN_LEFT || button == BTN_RIGHT)
            && self
                .state
                .depressed_mods
                .has(aegis_model::input::Mods::SUPER)
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
                        (*rec).window.layout_role = aegis_model::layout::LayoutRole::Floating;
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
            && let Some((rec, edges)) = self.resize_target_at(
                self.state.pointer_x,
                self.state.pointer_y,
                aegis_model::window::RESIZE_OUTER_MARGIN,
            )
        {
            let resource = unsafe { (*rec).resource };
            let id = unsafe { (*rec).window.id };
            self.change_keyboard_focus(resource);
            let position = (self.state.pointer_x, self.state.pointer_y);
            let now_ms = self.state.now_ms();
            let double_click = is_top_edge_only(edges)
                && self
                    .state
                    .last_top_border_click
                    .take()
                    .is_some_and(|first| {
                        is_matching_top_border_double_click(first, id, now_ms, position)
                    });
            if double_click {
                self.state.pending_top_border_double_click = Some(PendingTopBorderDoubleClick {
                    window_id: id,
                    press_position: position,
                    start_position: unsafe { (*rec).position },
                });
                self.state.compositor_pointer_grab = true;
                return;
            }
            self.state.last_top_border_click = None;
            unsafe {
                (*rec).window.state.resizing = true;
                reconfigure_with_state(rec);
            }
            self.state.interactive = Some(aegis_model::window::Interactive::Resize {
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
        if state.is_pressed() && button == BTN_LEFT {
            self.state.last_top_border_click = None;
        }
        // Click-to-focus: when a button is pressed over a surface, that
        // surface also gains keyboard focus. An explicit xdg-popup grab is
        // different: the protocol requires its topmost popup to retain
        // keyboard focus even while owner-events deliver the click to another
        // surface of the same client. A non-grabbing popup must not receive
        // keyboard focus at all.
        if state.is_pressed() && !self.state.pointer_focus.is_null() {
            let pointer_surface = unsafe {
                ffi::wl_resource_get_user_data(self.state.pointer_focus) as *mut SurfaceRec
            };
            let keyboard_target = unsafe {
                xdg_role_aware_keyboard_target(
                    pointer_surface,
                    self.state.keyboard_focus,
                    grabbed_popup,
                )
            };
            self.change_keyboard_focus(keyboard_target);
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
        self.state.last_top_border_click = None;
        if self.state.pending_top_border_double_click.take().is_some() {
            self.state.compositor_pointer_grab = false;
        }
        if self.state.drag.is_some() {
            unsafe { cancel_drag(self.state.as_mut(), true) };
        }
        self.change_pointer_focus(std::ptr::null_mut());
    }

    pub(crate) fn keyboard_key(
        &mut self,
        evdev_code: u32,
        state: aegis_model::input::ButtonState,
        keymap: Option<&aegis_model::keybind::Keymap>,
    ) -> Option<aegis_model::keybind::Action> {
        let prepared = self.prepare_keyboard_event(evdev_code, state)?;
        self.deliver_prepared_keyboard_event(prepared, keymap)
    }

    /// Advance the active seat's physical XKB state exactly once.
    ///
    /// Preparation is deliberately separate from delivery. The compositor
    /// runtime can prepare a complete libinput batch in hardware order, then
    /// route individual key sequences to chrome or clients without either
    /// path mutating XKB again.
    pub fn prepare_keyboard_event(
        &mut self,
        evdev_code: u32,
        state: aegis_model::input::ButtonState,
    ) -> Option<PreparedKeyboardEvent> {
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
        self.state.depressed_mods = aegis_model::input::Mods(outcome.depressed);
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
            return Some(PreparedKeyboardEvent {
                evdev_code,
                state,
                outcome,
                consumed_by_vt_switch: true,
            });
        }
        Some(PreparedKeyboardEvent {
            evdev_code,
            state,
            outcome,
            consumed_by_vt_switch: false,
        })
    }

    /// Route a key whose physical XKB transition has already been applied.
    pub(crate) fn deliver_prepared_keyboard_event(
        &mut self,
        prepared: PreparedKeyboardEvent,
        keymap: Option<&aegis_model::keybind::Keymap>,
    ) -> Option<aegis_model::keybind::Action> {
        let PreparedKeyboardEvent {
            evdev_code,
            state,
            outcome,
            consumed_by_vt_switch,
        } = prepared;
        if consumed_by_vt_switch {
            return None;
        }
        if !state.is_pressed() && self.state.suppressed_shortcut_keys.remove(&evdev_code) {
            return None;
        }
        // A key that matches a global binding on press is consumed (not posted
        // to the focused client) and its action returned for the caller to
        // dispatch. Modifier-only keys never match, so modifiers still post.
        // While the session is locked, bindings stay swallowed — except for
        // the development-only Quit escape hatch (`[dev]
        // allow_quit_while_locked`), which still matches so a wedged lock
        // surface cannot trap the session. That hatch will be removed before
        // release.
        let shortcuts_inhibited =
            unsafe { extensions::keyboard_shortcuts_inhibited(self.state.as_mut()) };
        let locked = self.state.session_lock_phase.is_active();
        let matched = if state.is_pressed()
            && !shortcuts_inhibited
            && (!locked || self.state.allow_quit_while_locked)
        {
            keymap
                .and_then(|keymap| {
                    keymap.match_key(aegis_model::input::Mods(outcome.depressed), outcome.keysym)
                })
                .filter(|action| !locked || matches!(action, aegis_model::keybind::Action::Quit))
        } else {
            None
        };
        if matched.is_some() {
            self.state.suppressed_shortcut_keys.insert(evdev_code);
            return matched;
        }
        let time = self.epoch.elapsed().as_millis() as u32;
        if unsafe {
            extensions::input_method_grab_key(self.state.as_mut(), evdev_code, state, outcome, time)
        } {
            return None;
        }
        // Maintain the client-facing logical key stream independently from
        // physical xkb state. A compositor shortcut or input-method-consumed
        // key never enters this set; valid presses survive focus changes via
        // wl_keyboard.enter and pair with the later release in the new
        // client. Duplicate presses/releases are not legal wl_keyboard
        // transitions and are dropped.
        let valid_transition = keyboard::apply_logical_key_state(
            &mut self.state.client_pressed_keys,
            evdev_code,
            state.is_pressed(),
        );
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
                if valid_transition {
                    ffi::wl_resource_post_event(
                        k,
                        ffi::WL_KEYBOARD_KEY,
                        serial,
                        time,
                        evdev_code,
                        state_u32,
                    );
                }
                // Core Wayland requires the modifier state resulting from a
                // key transition to follow wl_keyboard.key. Sending the
                // authoritative snapshot even when unchanged also repairs a
                // client joining the stream at a routing boundary.
                ffi::wl_resource_post_event(
                    k,
                    ffi::WL_KEYBOARD_MODIFIERS,
                    serial,
                    depressed,
                    latched,
                    locked,
                    group,
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
            if self.state.session_lock_phase.is_active() {
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
                            .interaction_domain_observes_window(seat.interaction_domain, window)
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

    /// Topmost floating toplevel whose outer resize margin contains `(x, y)`.
    ///
    /// All visible foreground window rectangles act as barriers, including
    /// non-resizable and observation-only windows. This prevents an edge on a
    /// lower window from being selected through pixels occupied by a higher
    /// window.
    pub(crate) fn resize_target_at(
        &self,
        x: f32,
        y: f32,
        margin: f32,
    ) -> Option<(*mut SurfaceRec, aegis_model::window::ResizeEdges)> {
        let visible = self.visible();
        let mut layers = Vec::new();
        for p in self.state.live_surfaces() {
            let s = unsafe { &*p };
            if !s.mapped
                || s.xdg_toplevel.is_null()
                || s.window.minimized
                || !visible.contains(&s.window.id)
            {
                continue;
            }
            let controls = self
                .state
                .authority
                .seat_controls_window(self.state.active_seat, s.window.id);
            let observes = self
                .state
                .authority
                .seat(self.state.active_seat)
                .is_some_and(|seat| {
                    self.state
                        .authority
                        .interaction_domain_observes_window(seat.interaction_domain, s.window.id)
                });
            if !controls && !observes {
                continue;
            }
            let mut window = s.window.clone();
            window.position = s.position;
            window.size = s
                .window_geometry
                .map(|geometry| geometry.size)
                .unwrap_or_else(|| surface_logical_size(s));
            let resizable = controls
                && !window.state.maximized
                && !window.state.fullscreen
                && window.layout_role == aegis_model::layout::LayoutRole::Floating;
            layers.push((p, window, resizable));
        }
        stacked_resize_target(layers, x, y, margin)
    }

    /// End an interactive move/resize, clearing the protocol resizing state
    /// and notifying the client once after the final geometry.
    pub(crate) fn finish_interactive(&mut self) {
        let interactive = self.state.interactive;
        if let Some(interactive_grab) = interactive {
            let window_id = match interactive_grab {
                aegis_model::window::Interactive::Move { window_id, .. } => window_id,
                aegis_model::window::Interactive::Resize { window_id, .. } => window_id,
            };
            let rec = self.find_surface_by_window_id(window_id);
            if !rec.is_null() {
                unsafe {
                    if matches!(
                        interactive_grab,
                        aegis_model::window::Interactive::Resize { .. }
                    ) {
                        (*rec).window.state.resizing = false;
                        reconfigure_with_state(rec);
                    }
                    if (*rec).window.parent.is_none()
                        && let Some(app_id) = (*rec).window.app_id.as_deref()
                        && !app_id.is_empty()
                    {
                        let rect = (*rec).saved_floating_rect.unwrap_or(aegis_model::Rect {
                            origin: (*rec).position,
                            size: (*rec).window.size,
                        });
                        if rect.size.w > 0 && rect.size.h > 0 {
                            let ws_idx = self.state.workspace_number_for_window((*rec).window.id);

                            self.state.persist_app_geometry(
                                app_id,
                                rect,
                                ws_idx,
                                Some((*rec).window.layout_role),
                            );
                        }
                    }
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
        let allowed = if self.state.session_lock_phase.is_active() {
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
        // An active pointer lock freezes the absolute position; the protocol
        // forbids wl_pointer.motion while it holds. Relative motion keeps
        // flowing through post_relative_motion. Confined pointers are
        // unaffected and keep receiving absolute motion within their region.
        let locked = unsafe {
            extensions::pointer_lock_active(self.state.as_ref() as *const State as *mut State)
        };
        if locked {
            // wl_pointer.frame is a batch delimiter, not motion: it must still
            // terminate the relative-motion batch while the lock holds.
            // wl_pointer >= 5 clients (notably SDL) accumulate
            // zwp_relative_pointer_v1.relative_motion and dispatch it only on
            // frame, so withholding it freezes locked game cameras.
            for p in self.iter_focus_pointers(unsafe {
                ffi::wl_resource_get_client(self.state.pointer_focus)
            }) {
                unsafe {
                    if ffi::wl_resource_get_version(p) >= 5 {
                        ffi::wl_resource_post_event(p, ffi::WL_POINTER_FRAME);
                    }
                }
            }
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
    /// relative-pointer resource owned by the focused client. `dx`/`dy` are
    /// the accelerated deltas and `dx_unaccel`/`dy_unaccel` the raw device
    /// deltas, both as reported by the backend event — never derived from the
    /// clamped absolute position, so motion keeps flowing while the cursor is
    /// pinned at an output edge or frozen by a pointer lock.
    pub(crate) fn post_relative_motion(&self, dx: f64, dy: f64, dx_unaccel: f64, dy_unaccel: f64) {
        if self.state.pointer_focus.is_null()
            || (dx == 0.0 && dy == 0.0 && dx_unaccel == 0.0 && dy_unaccel == 0.0)
        {
            return;
        }
        let focus_client = unsafe { ffi::wl_resource_get_client(self.state.pointer_focus) };
        // Monotonic microsecond timestamp, split hi/lo per the protocol.
        let utime = self.epoch.elapsed().as_micros() as u64;
        let utime_hi = (utime >> 32) as u32;
        let utime_lo = (utime & 0xffff_ffff) as u32;
        let fdx = ffi::wl_fixed_from_f32(dx as f32);
        let fdy = ffi::wl_fixed_from_f32(dy as f32);
        let fdx_unaccel = ffi::wl_fixed_from_f32(dx_unaccel as f32);
        let fdy_unaccel = ffi::wl_fixed_from_f32(dy_unaccel as f32);
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
                    fdx_unaccel,
                    fdy_unaccel,
                );
            }
        }
    }

    /// Post one backend-preserved scroll frame to the focused client.
    pub(crate) fn pointer_axis(&mut self, frame: aegis_model::input::PointerAxisFrame) {
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
            aegis_model::Point::default()
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
            aegis_model::Point::default()
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

#[cfg(test)]
mod resize_tests {
    use super::*;

    #[test]
    fn popup_grab_routes_owner_events_and_dismisses_true_outside_clicks() {
        let chrome = 0x100usize as *mut ffi::wl_client;
        let another_client = 0x200usize as *mut ffi::wl_client;

        assert!(popup_grab_allows_owner_event(chrome, chrome));
        assert!(!popup_grab_allows_owner_event(chrome, another_client));
        assert!(!popup_grab_allows_owner_event(chrome, std::ptr::null_mut()));
    }

    #[test]
    fn click_focus_stays_on_xdg_roles_for_popups_and_subsurfaces() {
        let mut root = SurfaceRec::new(0x100usize as *mut ffi::wl_resource);
        root.xdg_toplevel = 0x101usize as *mut ffi::wl_resource;
        let mut popup = SurfaceRec::new(0x200usize as *mut ffi::wl_resource);
        popup.xdg_popup = 0x201usize as *mut ffi::wl_resource;
        popup.popup_parent = &mut root;

        assert_eq!(
            unsafe { xdg_role_aware_keyboard_target(&mut popup, std::ptr::null_mut(), None) },
            root.resource,
            "a non-grabbing browser bubble must not steal keyboard focus"
        );
        assert_eq!(
            unsafe {
                xdg_role_aware_keyboard_target(&mut popup, std::ptr::null_mut(), Some(&mut popup))
            },
            popup.resource,
            "an explicit grab pins focus to the popup"
        );

        let mut child = SurfaceRec::new(0x300usize as *mut ffi::wl_resource);
        child.parent = &mut popup;
        assert_eq!(
            unsafe { xdg_role_aware_keyboard_target(&mut child, std::ptr::null_mut(), None) },
            root.resource,
            "subsurfaces of a non-grabbing popup follow the same rule"
        );

        let mut chrome_bubble = SurfaceRec::new(0x400usize as *mut ffi::wl_resource);
        chrome_bubble.parent = &mut root;
        assert_eq!(
            unsafe {
                xdg_role_aware_keyboard_target(&mut chrome_bubble, std::ptr::null_mut(), None)
            },
            root.resource,
            "Chrome UI subsurfaces keep keyboard focus on their xdg_toplevel"
        );
    }

    fn window(id: u64, x: i32, y: i32, w: i32, h: i32) -> aegis_model::window::Window {
        let mut window = aegis_model::window::Window::new(aegis_model::window::WindowId(id));
        window.position = aegis_model::Point { x, y };
        window.size = aegis_model::Size { w, h };
        window
    }

    #[test]
    fn foreground_window_blocks_resize_of_a_lower_window() {
        // The pointer is in the lower window's right resize margin, but the
        // foreground window covers that location.
        let lower = window(1, 100, 100, 200, 200);
        let foreground = window(2, 290, 150, 120, 100);
        let target = stacked_resize_target(
            [(1usize, lower, true), (2usize, foreground, true)],
            301.0,
            200.0,
            aegis_model::window::RESIZE_OUTER_MARGIN,
        );
        assert_eq!(target, None);
    }

    #[test]
    fn topmost_outer_margin_wins_over_lower_content() {
        let lower = window(1, 100, 100, 300, 200);
        let foreground = window(2, 150, 150, 150, 100);
        let target = stacked_resize_target(
            [(1usize, lower, true), (2usize, foreground, true)],
            301.0,
            200.0,
            aegis_model::window::RESIZE_OUTER_MARGIN,
        );
        assert_eq!(target, Some((2, aegis_model::window::ResizeEdges::RIGHT)));
    }

    #[test]
    fn non_resizable_foreground_is_still_an_input_barrier() {
        let lower = window(1, 100, 100, 200, 200);
        let foreground = window(2, 290, 150, 120, 100);
        let target = stacked_resize_target(
            [(1usize, lower, true), (2usize, foreground, false)],
            301.0,
            200.0,
            aegis_model::window::RESIZE_OUTER_MARGIN,
        );
        assert_eq!(target, None);
    }

    #[test]
    fn only_the_plain_top_edge_owns_the_double_click_gesture() {
        use aegis_model::window::ResizeEdges;

        assert!(is_top_edge_only(ResizeEdges::TOP));
        assert!(!is_top_edge_only(ResizeEdges::LEFT));
        assert!(!is_top_edge_only(ResizeEdges(
            ResizeEdges::TOP.0 | ResizeEdges::LEFT.0
        )));
    }

    #[test]
    fn top_border_double_click_requires_same_window_time_and_position() {
        let first = TopBorderClick {
            window_id: aegis_model::window::WindowId(7),
            released_at_ms: 1_000,
            position: (120.0, 40.0),
        };
        assert!(is_matching_top_border_double_click(
            first,
            aegis_model::window::WindowId(7),
            1_400,
            (123.0, 42.0),
        ));
        assert!(!is_matching_top_border_double_click(
            first,
            aegis_model::window::WindowId(8),
            1_400,
            (123.0, 42.0),
        ));
        assert!(!is_matching_top_border_double_click(
            first,
            aegis_model::window::WindowId(7),
            1_451,
            (123.0, 42.0),
        ));
        assert!(!is_matching_top_border_double_click(
            first,
            aegis_model::window::WindowId(7),
            1_400,
            (125.0, 40.0),
        ));
    }
}
