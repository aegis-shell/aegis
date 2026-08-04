use super::*;

pub(super) struct FrameState {
    pub(super) input: aegis_shell::Input,
    pub(super) session_locked: bool,
    pub(super) cursor_hidden: bool,
    pub(super) cursor_shape: u32,
    /// Any host input (physical, gesture, or text) or synthetic input was
    /// applied this iteration; conservatively forces a full-damage frame.
    pub(super) had_input: bool,
    pub(super) pending_screenshots: Vec<PendingScreenshot>,
}

#[derive(Debug, Clone, Copy)]
struct KeybindingInvocation {
    action: aegis_model::keybind::Action,
    cursor: (f32, f32),
    /// Modifier state at the triggering key press, not at the end of the
    /// backend batch. A batch may contain both Tab and the following Super
    /// release; collapsing those edges would turn a preview step into an
    /// immediate focus change.
    super_held: bool,
}

fn keybinding_invocation(
    action: aegis_model::keybind::Action,
    cursor: (f32, f32),
    key: aegis_model::input::KeyChar,
) -> KeybindingInvocation {
    KeybindingInvocation {
        action,
        cursor,
        super_held: key.mods.has(aegis_model::input::Mods::SUPER),
    }
}

impl FrameState {
    /// Preserve edge-triggered chrome input while the presentation backend
    /// owns an in-flight frame. Level state comes from the newest snapshot;
    /// button/key/text/scroll edges accumulate until the next redraw.
    pub(super) fn merge(&mut self, newer: FrameState) {
        let src = newer.input.as_raw();
        let dst = self.input.as_raw_mut();

        dst.cursor = src.cursor;
        dst.display_size = src.display_size;
        dst.mouse_down.copy_from_slice(&src.mouse_down);
        dst.mods = src.mods;
        for (dst, src) in dst.mouse_pressed.iter_mut().zip(&src.mouse_pressed) {
            *dst |= *src;
        }
        for (dst, src) in dst.mouse_released.iter_mut().zip(&src.mouse_released) {
            *dst |= *src;
        }
        dst.scroll_x += src.scroll_x;
        dst.scroll_y += src.scroll_y;
        dst.scroll_pixels_x += src.scroll_pixels_x;
        dst.scroll_pixels_y += src.scroll_pixels_y;
        dst.ime_delete_before = dst.ime_delete_before.saturating_add(src.ime_delete_before);
        dst.ime_delete_after = dst.ime_delete_after.saturating_add(src.ime_delete_after);
        append_c_text(&mut dst.text_utf8, &src.text_utf8);
        if src.preedit_utf8.first().copied().unwrap_or_default() != 0 {
            dst.preedit_utf8.copy_from_slice(&src.preedit_utf8);
            dst.preedit_cursor = src.preedit_cursor;
            dst.preedit_sel_lo = src.preedit_sel_lo;
            dst.preedit_sel_hi = src.preedit_sel_hi;
        }

        let dst_keys = (dst.key_count as usize).min(dst.keys.len());
        let src_keys = (src.key_count as usize).min(src.keys.len());
        let copy_keys = src_keys.min(dst.keys.len().saturating_sub(dst_keys));
        dst.keys[dst_keys..dst_keys + copy_keys].copy_from_slice(&src.keys[..copy_keys]);
        dst.key_count = (dst_keys + copy_keys) as u32;

        self.session_locked = newer.session_locked;
        self.cursor_hidden = newer.cursor_hidden;
        self.cursor_shape = newer.cursor_shape;
        self.had_input |= newer.had_input;
        self.pending_screenshots.extend(newer.pending_screenshots);
    }

    pub(super) fn set_dt(&mut self, dt: f32) {
        self.input.set_dt(dt);
    }
}

fn append_c_text(dst: &mut [std::os::raw::c_char], src: &[std::os::raw::c_char]) {
    let dst_len = dst
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(dst.len());
    let src_len = src
        .iter()
        .position(|value| *value == 0)
        .unwrap_or(src.len());
    if dst_len >= dst.len() || src_len == 0 {
        return;
    }
    let copied = src_len.min(dst.len() - dst_len - 1);
    dst[dst_len..dst_len + copied].copy_from_slice(&src[..copied]);
    dst[dst_len + copied] = 0;
}

pub(super) fn agent_activities_from_applied_input(
    interaction_domain: aegis_model::interaction_domain::InteractionDomainId,
    interaction_domain_label: &str,
    window: aegis_model::window::WindowId,
    actions: &[aegis_model::input::SyntheticInputAction],
    events: &[aegis_model::input::InputEvent],
    sequence: &mut u64,
) -> Vec<aegis_shell::AgentActivity> {
    use aegis_model::input::{InputEvent, SyntheticInputAction};

    // Every prepared pointer action contributes exactly one global motion
    // event before its button/axis events. Reading those positions preserves
    // the exact coordinates the server applied rather than remapping target-
    // local coordinates a second time in the presentation layer.
    let mut pointer_positions = events.iter().filter_map(|event| match *event {
        InputEvent::PointerMotion { x, y } => Some(aegis_model::Point {
            x: x.round() as i32,
            y: y.round() as i32,
        }),
        _ => None,
    });
    actions
        .iter()
        .filter_map(|action| {
            let (position, kind) = match *action {
                SyntheticInputAction::PointerMove { .. } => (
                    Some(pointer_positions.next()?),
                    aegis_shell::AgentInputKind::PointerMove,
                ),
                SyntheticInputAction::Click { button, .. } => (
                    Some(pointer_positions.next()?),
                    aegis_shell::AgentInputKind::Click { button },
                ),
                SyntheticInputAction::Scroll { dx, dy, .. } => (
                    Some(pointer_positions.next()?),
                    aegis_shell::AgentInputKind::Scroll { dx, dy },
                ),
                // Do not copy the key code into presentation state: the
                // feedback says only that keyboard input occurred.
                SyntheticInputAction::KeyPress { .. } => {
                    (None, aegis_shell::AgentInputKind::Keyboard)
                }
            };
            *sequence = sequence.saturating_add(1);
            Some(aegis_shell::AgentActivity {
                sequence: *sequence,
                interaction_domain,
                interaction_domain_label: interaction_domain_label.to_owned(),
                window,
                position,
                kind,
            })
        })
        .collect()
}

impl CompositorRuntime {
    /// Snapshot the physical-seat cursor state at the current logical
    /// position. Screenshot requests carry this value through to readback so
    /// later pointer motion cannot change the cursor that is saved.
    pub(super) fn capture_cursor_state(&self) -> CaptureCursorState {
        self.capture_cursor_state_at(self.input_acc.cursor)
    }

    fn capture_cursor_state_at(&self, position: (f32, f32)) -> CaptureCursorState {
        let chrome_cursor = (!self.server.session_locked())
            .then(|| {
                self.shell
                    .cursor_shape_at(position.0, position.1, self.input_acc.display_size)
            })
            .flatten();
        let compositor_cursor = self.server.compositor_cursor_shape();
        let owned_cursor = if self.server.interactive().is_some() {
            compositor_cursor
        } else {
            chrome_cursor
                .map(|shape| shape as u32)
                .or(compositor_cursor)
        };
        let hidden = owned_cursor.is_none() && self.server.cursor_hidden();
        CaptureCursorState {
            position,
            shape: owned_cursor.unwrap_or_else(|| self.server.cursor_shape().max(1)),
            hidden,
            client_surface: hidden && self.server.client_cursor_surface_active(),
        }
    }

    /// Claim a compositor-owned touchpad swipe per the active gesture map
    /// (ADR-0080, ADR-0082). Returns true when the event is consumed here
    /// and must not be forwarded to client pointer-gesture objects. A swipe
    /// whose finger count has any binding is claimed for its whole duration;
    /// the axis latches once either accumulator crosses `AXIS_LOCK_PX`, and
    /// the bound action then fires one step per `STEP_PX` of travel. Which
    /// action listens on which (fingers, axis) is configuration, not code —
    /// see `aegis_model::gesture`.
    fn claim_swipe(&mut self, event: &aegis_model::input::PointerGestureEvent) -> bool {
        use aegis_model::gesture::{GestureAction, GestureAxis};
        use aegis_model::input::PointerGestureEvent as G;
        const AXIS_LOCK_PX: f32 = 30.0;
        const STEP_PX: f32 = 120.0;
        if self.server.session_locked() {
            // Never run gestures on a locked session; drop the state of any
            // swipe the lock interrupted so the next one starts clean.
            // `process_input` already finishes any active switcher.
            self.swipe = None;
            return false;
        }
        match *event {
            G::SwipeBegin { fingers, .. } => {
                let Ok(fingers) = u8::try_from(fingers) else {
                    return false;
                };
                if !self.gesture_map.claims(fingers) {
                    return false;
                }
                self.swipe = Some(SwipeState {
                    fingers,
                    ..SwipeState::default()
                });
                true
            }
            G::SwipeUpdate { dx, dy, .. } => {
                let Some(swipe) = self.swipe.as_mut() else {
                    return false;
                };
                swipe.dx += dx;
                swipe.dy += dy;
                if swipe.axis.is_none()
                    && (swipe.dx.abs() >= AXIS_LOCK_PX || swipe.dy.abs() >= AXIS_LOCK_PX)
                {
                    swipe.axis = Some(if swipe.dx.abs() >= swipe.dy.abs() {
                        GestureAxis::Horizontal
                    } else {
                        GestureAxis::Vertical
                    });
                }
                let Some(axis) = swipe.axis else {
                    return true;
                };
                let Some(action) = self.gesture_map.lookup(swipe.fingers, axis) else {
                    // Claimed finger count with no binding on this axis:
                    // consume without acting.
                    return true;
                };
                match action {
                    GestureAction::None => {}
                    GestureAction::WorkspaceSwitch => {
                        // Swipe left = next workspace, right = previous. Fast
                        // swipes can fire several steps in one update.
                        let mut steps = 0i32;
                        while swipe.dx <= -STEP_PX {
                            steps += 1;
                            swipe.dx += STEP_PX;
                        }
                        while swipe.dx >= STEP_PX {
                            steps -= 1;
                            swipe.dx -= STEP_PX;
                        }
                        let ts = self.start.elapsed().as_millis() as u64;
                        for _ in 0..steps.unsigned_abs() {
                            let dir = if steps > 0 {
                                aegis_model::workspace::Switch::Next
                            } else {
                                aegis_model::workspace::Switch::Prev
                            };
                            apply_command_and_journal(
                                &mut self.server,
                                &self.notif_queue,
                                &mut self.quit_requested,
                                aegis_ipc::Command::SwitchWorkspace { dir },
                                &self.ipc,
                                &self.journal,
                                ts,
                                aegis_ipc::Origin::Gesture,
                            );
                        }
                    }
                    GestureAction::WindowCycle => {
                        // Swipe up = next window, down = previous; the
                        // switcher stays open until SwipeEnd.
                        let mut steps = 0i32;
                        while swipe.dy <= -STEP_PX {
                            steps += 1;
                            swipe.dy += STEP_PX;
                        }
                        while swipe.dy >= STEP_PX {
                            steps -= 1;
                            swipe.dy -= STEP_PX;
                        }
                        let start_switcher = steps != 0 && !swipe.switcher;
                        swipe.switcher |= start_switcher;
                        if start_switcher {
                            self.shell.start_window_switcher();
                            self.server.start_window_switcher();
                        }
                        let ts = self.start.elapsed().as_millis() as u64;
                        for _ in 0..steps.unsigned_abs() {
                            apply_command_and_journal(
                                &mut self.server,
                                &self.notif_queue,
                                &mut self.quit_requested,
                                aegis_ipc::Command::Cycle { forward: steps > 0 },
                                &self.ipc,
                                &self.journal,
                                ts,
                                aegis_ipc::Origin::Gesture,
                            );
                        }
                    }
                    GestureAction::CommandPanel => {
                        // Down opens the panel, up closes it; fires at most
                        // once per gesture so a long swipe cannot oscillate
                        // the panel (ADR-0080).
                        if !swipe.panel_fired {
                            let open = self.shell.command_panel_active();
                            if (!open && swipe.dy > STEP_PX) || (open && swipe.dy < -STEP_PX) {
                                self.shell.toggle_command_panel();
                                swipe.panel_fired = true;
                            }
                        }
                    }
                }
                true
            }
            G::SwipeEnd { .. } => {
                let Some(swipe) = self.swipe.take() else {
                    return false;
                };
                if swipe.switcher {
                    self.shell.finish_window_switcher();
                    self.server.finish_window_switcher();
                }
                true
            }
            _ => self.swipe.is_some(),
        }
    }

    fn dispatch_keybinding(&mut self, invocation: KeybindingInvocation, session_locked: bool) {
        use aegis_model::keybind::Action;

        let KeybindingInvocation {
            action,
            cursor,
            super_held,
        } = invocation;
        let ts = self.start.elapsed().as_millis() as u64;
        let origin = aegis_ipc::Origin::Keybinding;
        match action {
            Action::ToggleLauncher => self.shell.toggle(),
            Action::TogglePrism => self.shell.toggle_prism(),
            Action::ToggleOverview => self.shell.toggle_overview(),
            Action::ToggleCommandPanel => self.shell.toggle_command_panel(),
            Action::CloseFocused => {
                if let Some(id) = self.server.focused_toplevel_id() {
                    let cmd = aegis_ipc::Command::Close { id };
                    apply_command_and_journal(
                        &mut self.server,
                        &self.notif_queue,
                        &mut self.quit_requested,
                        cmd,
                        &self.ipc,
                        &self.journal,
                        ts,
                        origin,
                    );
                }
            }
            Action::CycleFocus => {
                if super_held {
                    self.shell.start_window_switcher();
                    self.server.start_window_switcher();
                }
                let cmd = aegis_ipc::Command::Cycle { forward: true };
                apply_command_and_journal(
                    &mut self.server,
                    &self.notif_queue,
                    &mut self.quit_requested,
                    cmd,
                    &self.ipc,
                    &self.journal,
                    ts,
                    origin,
                );
            }
            Action::CycleFocusBack => {
                if super_held {
                    self.shell.start_window_switcher();
                    self.server.start_window_switcher();
                }
                let cmd = aegis_ipc::Command::Cycle { forward: false };
                apply_command_and_journal(
                    &mut self.server,
                    &self.notif_queue,
                    &mut self.quit_requested,
                    cmd,
                    &self.ipc,
                    &self.journal,
                    ts,
                    origin,
                );
            }
            Action::WorkspaceNext => {
                let cmd = aegis_ipc::Command::SwitchWorkspace {
                    dir: aegis_model::workspace::Switch::Next,
                };
                apply_command_and_journal(
                    &mut self.server,
                    &self.notif_queue,
                    &mut self.quit_requested,
                    cmd,
                    &self.ipc,
                    &self.journal,
                    ts,
                    origin,
                );
            }
            Action::WorkspacePrev => {
                let cmd = aegis_ipc::Command::SwitchWorkspace {
                    dir: aegis_model::workspace::Switch::Prev,
                };
                apply_command_and_journal(
                    &mut self.server,
                    &self.notif_queue,
                    &mut self.quit_requested,
                    cmd,
                    &self.ipc,
                    &self.journal,
                    ts,
                    origin,
                );
            }
            Action::ToggleTiling => {
                let cmd = aegis_ipc::Command::ToggleTiling;
                apply_command_and_journal(
                    &mut self.server,
                    &self.notif_queue,
                    &mut self.quit_requested,
                    cmd,
                    &self.ipc,
                    &self.journal,
                    ts,
                    origin,
                );
            }
            Action::Lock => self.idle_process.lock_now(),
            Action::Quit => {
                let cmd = aegis_ipc::Command::Quit;
                apply_command_and_journal(
                    &mut self.server,
                    &self.notif_queue,
                    &mut self.quit_requested,
                    cmd,
                    &self.ipc,
                    &self.journal,
                    ts,
                    origin,
                );
            }
            Action::Screenshot => {
                // Refuse to open the selector while locked or inactive; the
                // selector itself also suppresses confirmation in those
                // states, but this avoids the modal entirely.
                if session_locked || !self.host.is_active() {
                    log::debug!("screenshot: suppressed while locked or inactive");
                    return;
                }
                if self.shell.screenshot_active() {
                    // Print toggles the selector closed again.
                    self.shell.start_screenshot();
                } else {
                    // Open through the freeze session: the next frame
                    // snapshots the whole trigger frame (chrome included)
                    // and the selector opens on top of it.
                    let cursor = self.capture_cursor_state_at(cursor);
                    self.screenshot_freeze.request_open(Some(cursor));
                }
            }
        }
    }

    fn flush_physical_input_segment(
        &mut self,
        forwarded: &mut Vec<aegis_model::input::InputEvent>,
        forwarded_keys: &mut Vec<Option<aegis_compositor::PreparedKeyboardEvent>>,
        candidates: &mut Vec<KeybindingInvocation>,
        session_locked: bool,
    ) {
        if forwarded.is_empty() {
            debug_assert!(forwarded_keys.is_empty());
            debug_assert!(candidates.is_empty());
            return;
        }

        let actions = self
            .server
            .forward_prepared_input(forwarded, forwarded_keys, &self.keymap);
        let fallback_super_held = self
            .server
            .depressed_modifiers()
            .has(aegis_model::input::Mods::SUPER);
        let mut candidate_at = 0;
        for action in actions {
            let invocation = candidates[candidate_at..]
                .iter()
                .position(|candidate| candidate.action == action)
                .map(|offset| {
                    candidate_at += offset + 1;
                    candidates[candidate_at - 1]
                })
                .unwrap_or(KeybindingInvocation {
                    action,
                    cursor: self.input_acc.cursor,
                    super_held: fallback_super_held,
                });
            self.dispatch_keybinding(invocation, session_locked);
        }
        forwarded.clear();
        forwarded_keys.clear();
        candidates.clear();
    }

    fn finish_keyboard_switcher_if_released(&mut self, super_held: bool) {
        // Swipe-driven switching has no held modifier and commits on
        // SwipeEnd instead.
        if self.swipe.as_ref().is_none_or(|swipe| !swipe.switcher)
            && (self.shell.window_switcher_active() || self.server.window_switcher_active())
            && !super_held
        {
            self.shell.finish_window_switcher();
            self.server.finish_window_switcher();
        }
    }

    pub(super) fn process_input(
        &mut self,
        work: IterationWork,
    ) -> Result<FrameState, Box<dyn std::error::Error>> {
        let IterationWork {
            pending_synthetic_input,
            pending_screenshots,
        } = work;
        // Process client protocol traffic.
        self.server.dispatch();
        let session_locked = self.server.session_locked();
        if session_locked {
            self.shell.finish_window_switcher();
            self.server.cancel_window_switcher();
        }
        let interaction_domain_revision = self.server.interaction_domain_snapshot().revision;
        for (interaction_domain, damage) in self.server.take_interaction_domain_damage() {
            self.interaction_domain_damage_sequence =
                self.interaction_domain_damage_sequence.saturating_add(1);
            if let Some(ipc) = &self.ipc {
                ipc.broadcast(aegis_ipc::Event::InteractionDomainDamaged {
                    interaction_domain,
                    sequence: self.interaction_domain_damage_sequence,
                    revision: interaction_domain_revision,
                    damage,
                });
            }
        }
        for state in self.server.take_text_input_states() {
            self.host.set_text_input_state(state);
        }
        let mut had_input = false;
        let mut non_cursor_input = false;
        for event in self.host.take_text_input() {
            had_input = true;
            non_cursor_input = true;
            self.server.text_input_event(&event);
        }
        for event in self.host.take_pointer_gestures() {
            had_input = true;
            non_cursor_input = true;
            // Swipes claimed by the gesture map (built-in defaults plus
            // `[[gesture]]` overrides, ADR-0082) are compositor-owned and
            // never reach client pointer-gesture objects; anything else
            // forwards unchanged.
            if self.claim_swipe(&event) {
                continue;
            }
            self.server.pointer_gesture_event(&event);
        }
        // Drain backend input: forward to clients (via the server's seat) and
        // mirror into the shell's input snapshot so chrome gets first dibs on
        // clicks (e.g. the Quit button). The chrome reads the same pointer
        // position; routing priority is decided by the shell's hit-test.
        //
        // The per-frame `Input` snapshot is rebuilt from the accumulator each
        // iteration: only level state (cursor position, button-held, display
        // size) is carried in; edge flags (pressed/released/scroll/keys/text)
        // start at zero so a press/release in one frame can never bleed into
        // the next and trigger phantom clicks in immediate-mode widgets.
        let mut input = aegis_shell::Input::default();
        input.set_display_size(self.input_acc.display_size.0, self.input_acc.display_size.1);
        input.set_cursor(self.input_acc.cursor.0, self.input_acc.cursor.1);
        input.set_mouse_down(lens::MouseButton::Left, self.input_acc.mouse_down[0]);
        input.set_mouse_down(lens::MouseButton::Right, self.input_acc.mouse_down[1]);
        input.set_mouse_down(lens::MouseButton::Middle, self.input_acc.mouse_down[2]);
        let mut shell_scroll = (0.0_f32, 0.0_f32);
        let mut shell_scroll_pixels = (0.0_f32, 0.0_f32);
        let pointer_before = self.input_acc.cursor;
        let mut events = self.host.take_input();
        let pointer_motion_only = !events.is_empty()
            && events
                .iter()
                .all(|event| matches!(event, aegis_model::input::InputEvent::PointerMotion { .. }));
        if !events.is_empty() {
            had_input = true;
            non_cursor_input |= !pointer_motion_only;
            self.server.note_user_activity();
        }
        // Coordinate contract: backends emit absolute coordinates in their
        // native space — the nested host in its already-scaled logical
        // pixels, direct KMS in physical panel pixels. The compositor's
        // logical space follows the server's output geometry, which may
        // carry a configured scale override, so convert to logical once
        // here instead of every consumer doing it. On nested the factor is
        // 1.0; on DRM with an unmodified backend scale it is 1.0 too —
        // only a configured override changes it.
        let effective_scale = self
            .server
            .output_infos()
            .first()
            .map(|o| o.geometry.scale.as_f32())
            .filter(|s| *s > 0.0)
            .unwrap_or_else(|| self.host.scale());
        let coord_factor = self.host.scale() / effective_scale;
        if (coord_factor - 1.0).abs() > f32::EPSILON {
            for ev in &mut events {
                use aegis_model::input::{InputEvent::*, TabletEvent};
                match ev {
                    PointerMotion { x, y }
                    | TouchDown { x, y, .. }
                    | TouchMotion { x, y, .. }
                    | Tablet {
                        event: TabletEvent::Proximity { x, y, .. } | TabletEvent::Axes { x, y, .. },
                    } => {
                        *x *= coord_factor;
                        *y *= coord_factor;
                    }
                    _ => {}
                }
            }
        }
        // Chrome capture chooses the owner of each new key press without
        // changing the focused Wayland surface. Ownership remains fixed until
        // the matching release, even when the overlay opens or closes between
        // those two events.
        let keyboard_captured = !session_locked && self.shell.captures_keyboard();
        let mut event_cursor = pointer_before;
        let mut chrome_owned_key_events = vec![false; events.len()];
        let mut chrome_key_chars = vec![None; events.len()];
        let mut chrome_actions = vec![None; events.len()];
        let mut client_action_candidates = vec![None; events.len()];
        let mut prepared_key_events = vec![None; events.len()];
        if !events.is_empty() {
            for (event_index, ev) in events.iter().enumerate() {
                use aegis_model::input::InputEvent::*;
                match *ev {
                    PointerMotion { x, y } => {
                        event_cursor = (x, y);
                        input.set_cursor(x, y);
                        self.input_acc.cursor = (x, y);
                    }
                    PointerButton { button, state } => {
                        // Map Linux BTN_* codes (0x110=left, 0x111=right,
                        // 0x112=middle) to lens's MouseButton. Other buttons
                        // are dropped; the chrome only consumes these three.
                        let mapped = match button {
                            0x110 => Some(lens::MouseButton::Left),
                            0x111 => Some(lens::MouseButton::Right),
                            0x112 => Some(lens::MouseButton::Middle),
                            _ => None,
                        };
                        if let Some(b) = mapped {
                            if state.is_pressed() {
                                input.set_mouse_pressed(b, true);
                                input.set_mouse_down(b, true);
                                self.input_acc.set_mouse_down(b, true);
                            } else {
                                input.set_mouse_released(b, true);
                                input.set_mouse_down(b, false);
                                self.input_acc.set_mouse_down(b, false);
                            }
                        }
                    }
                    PointerLeave => {
                        event_cursor = (-1.0, -1.0);
                        input.set_cursor(-1.0, -1.0);
                        self.input_acc.cursor = (-1.0, -1.0);
                    }
                    Key { code, state } => {
                        // Advance the physical XKB state here, in backend
                        // arrival order, before shell/client ownership can
                        // split this batch into separate delivery paths.
                        let prepared = self.server.prepare_keyboard_event(code, state);
                        prepared_key_events[event_index] = prepared;
                        let route = self.keyboard_capture.route(code, state, keyboard_captured);
                        let chrome_owned = route == aegis_model::input::KeyRoute::Chrome;
                        chrome_owned_key_events[event_index] = chrome_owned;
                        if !chrome_owned
                            && state.is_pressed()
                            && let Some(key) = prepared.and_then(|prepared| prepared.key_char())
                            && let Some(action) = self.keymap.match_key(key.mods, key.keysym)
                        {
                            // The compositor confirms below whether shortcut
                            // inhibition permits this candidate. Keeping the
                            // event-local pointer here lets an accepted
                            // screenshot binding retain its exact trigger
                            // coordinates even if this input batch contains a
                            // later motion event.
                            client_action_candidates[event_index] =
                                Some(keybinding_invocation(action, event_cursor, key));
                        }

                        if chrome_owned {
                            // XKB was already advanced above on both edges;
                            // only feed prepared presses to chrome so text is
                            // not duplicated and no route can reorder state.
                            if let Some(kc) = prepared.and_then(|event| event.key_char())
                                && state.is_pressed()
                            {
                                // A small explicit set of compositor controls
                                // remains reachable while modal chrome owns
                                // new key sequences.
                                let switcher_action = self
                                    .shell
                                    .window_switcher_active()
                                    .then(|| self.keymap.match_key(kc.mods, kc.keysym))
                                    .flatten()
                                    .filter(|action| {
                                        matches!(
                                            action,
                                            aegis_model::keybind::Action::CycleFocus
                                                | aegis_model::keybind::Action::CycleFocusBack
                                        )
                                    });
                                if let Some(action) = switcher_action.or_else(|| {
                                    self.keymap
                                        .match_key_during_keyboard_capture(kc.mods, kc.keysym)
                                }) {
                                    chrome_actions[event_index] =
                                        Some(keybinding_invocation(action, event_cursor, kc));
                                } else {
                                    chrome_key_chars[event_index] = Some(kc);
                                }
                            }
                        }
                    }
                    PointerAxis(frame) => {
                        use aegis_model::input::PointerAxisSource;
                        if matches!(
                            frame.source,
                            Some(PointerAxisSource::Wheel | PointerAxisSource::WheelTilt)
                        ) {
                            shell_scroll.0 += frame.horizontal.wheel_steps();
                            shell_scroll.1 += frame.vertical.wheel_steps();
                        } else {
                            shell_scroll_pixels.0 += frame.dx();
                            shell_scroll_pixels.1 += frame.dy();
                        }
                    }
                    // Touch events are not handled by the shell chrome yet;
                    // they route to clients via forward_input below.
                    TouchDown { .. }
                    | TouchMotion { .. }
                    | TouchUp { .. }
                    | TouchFrame
                    | TouchCancel
                    | Tablet { .. } => {}
                }
            }
            // Route the batch a second time with compositor overlays removed
            // from the client stream. Flush at shortcut and held-modifier
            // boundaries so their effects occur in physical event order. In
            // particular, a Super release commits the preview before a later
            // click or wheel frame in this same backend batch is routed.
            let display = self.input_acc.display_size;
            let mut route_cursor = pointer_before;
            let mut forwarded = Vec::with_capacity(events.len() + 1);
            let mut forwarded_keys = Vec::new();
            let mut forwarded_candidates = Vec::new();
            for (event_index, ev) in events.iter().copied().enumerate() {
                use aegis_model::input::InputEvent::*;
                match ev {
                    Key { code, state } if chrome_owned_key_events[event_index] => {
                        self.flush_physical_input_segment(
                            &mut forwarded,
                            &mut forwarded_keys,
                            &mut forwarded_candidates,
                            session_locked,
                        );
                        if let Some(invocation) = chrome_actions[event_index] {
                            self.dispatch_keybinding(invocation, session_locked);
                        } else if let Some(key) = chrome_key_chars[event_index] {
                            self.shell.key_char(key);
                        }
                        // Escape cancels instead of committing when the later
                        // Super release from this batch is observed.
                        if self.shell.take_window_switcher_cancel() {
                            self.server.cancel_window_switcher();
                            self.shell.finish_window_switcher();
                        }
                        let super_held_at_event = prepared_key_events[event_index]
                            .and_then(|prepared| prepared.key_char())
                            .is_some_and(|key| key.mods.has(aegis_model::input::Mods::SUPER));
                        if !state.is_pressed()
                            && matches!(
                                code,
                                aegis_model::input::KEY_LEFTMETA
                                    | aegis_model::input::KEY_RIGHTMETA
                            )
                            && !super_held_at_event
                        {
                            self.finish_keyboard_switcher_if_released(false);
                        }
                    }
                    Key { code, state } => {
                        forwarded.push(ev);
                        forwarded_keys.push(prepared_key_events[event_index]);
                        if let Some(candidate) = client_action_candidates[event_index] {
                            forwarded_candidates.push(candidate);
                        }
                        let super_held_at_event = prepared_key_events[event_index]
                            .and_then(|prepared| prepared.key_char())
                            .is_some_and(|key| key.mods.has(aegis_model::input::Mods::SUPER));
                        let super_released = !state.is_pressed()
                            && matches!(
                                code,
                                aegis_model::input::KEY_LEFTMETA
                                    | aegis_model::input::KEY_RIGHTMETA
                            )
                            && !super_held_at_event;
                        if client_action_candidates[event_index].is_some() || super_released {
                            self.flush_physical_input_segment(
                                &mut forwarded,
                                &mut forwarded_keys,
                                &mut forwarded_candidates,
                                session_locked,
                            );
                        }
                        if super_released {
                            self.finish_keyboard_switcher_if_released(false);
                        }
                    }
                    PointerMotion { x, y } => {
                        self.synthetic_pointer_active = false;
                        route_cursor = (x, y);
                        let captured =
                            !session_locked && self.shell.captures_pointer_at(x, y, display);
                        if captured {
                            if !self.chrome_pointer_captured {
                                forwarded.push(PointerLeave);
                            }
                            // A title-bar move or edge resize begins after
                            // chrome handles the press. Once active, pointer
                            // motion still has to reach the server even while
                            // the cursor remains inside that chrome region.
                            if self.server.interactive().is_some() || self.server.drag_active() {
                                forwarded.push(ev);
                            }
                        } else {
                            forwarded.push(ev);
                        }
                        self.chrome_pointer_captured = captured;
                    }
                    PointerButton { state, .. } => {
                        let captured = !session_locked
                            && self.shell.captures_pointer_at(
                                route_cursor.0,
                                route_cursor.1,
                                display,
                            );
                        if self.synthetic_pointer_active {
                            if !captured {
                                forwarded.push(PointerMotion {
                                    x: route_cursor.0,
                                    y: route_cursor.1,
                                });
                            }
                            self.synthetic_pointer_active = false;
                        }
                        if captured {
                            if !self.chrome_pointer_captured {
                                forwarded.push(PointerLeave);
                            }
                            // Chrome-initiated move/resize grabs still need a
                            // release edge to terminate even though ordinary
                            // clicks over the overlay are consumed.
                            if !state.is_pressed()
                                && (self.server.interactive().is_some()
                                    || self.server.drag_active())
                            {
                                forwarded.push(ev);
                            }
                        } else {
                            // A button/axis can be the first event after an
                            // overlay closes. Re-establish client focus before
                            // forwarding it because the enter-side motion was
                            // consumed while chrome owned the pointer.
                            if self.chrome_pointer_captured {
                                forwarded.push(PointerMotion {
                                    x: route_cursor.0,
                                    y: route_cursor.1,
                                });
                            }
                            forwarded.push(ev);
                        }
                        self.chrome_pointer_captured = captured;
                    }
                    PointerAxis(_) => {
                        let captured = !session_locked
                            && self.shell.captures_pointer_at(
                                route_cursor.0,
                                route_cursor.1,
                                display,
                            );
                        if self.synthetic_pointer_active {
                            if !captured {
                                forwarded.push(PointerMotion {
                                    x: route_cursor.0,
                                    y: route_cursor.1,
                                });
                            }
                            self.synthetic_pointer_active = false;
                        }
                        if captured {
                            if !self.chrome_pointer_captured {
                                forwarded.push(PointerLeave);
                            }
                        } else {
                            if self.chrome_pointer_captured {
                                forwarded.push(PointerMotion {
                                    x: route_cursor.0,
                                    y: route_cursor.1,
                                });
                            }
                            forwarded.push(ev);
                        }
                        self.chrome_pointer_captured = captured;
                    }
                    PointerLeave => {
                        self.synthetic_pointer_active = false;
                        route_cursor = (-1.0, -1.0);
                        self.chrome_pointer_captured = false;
                        forwarded.push(PointerLeave);
                    }
                    TouchDown { x, y, .. } if self.synthetic_pointer_active => {
                        // Touch delivery shares the server's pointer focus.
                        // Re-hit-test at the physical contact before routing
                        // the down event after a synthetic pointer move.
                        self.synthetic_pointer_active = false;
                        forwarded.push(PointerMotion { x, y });
                        forwarded.push(ev);
                    }
                    _ => forwarded.push(ev),
                }
            }
            self.flush_physical_input_segment(
                &mut forwarded,
                &mut forwarded_keys,
                &mut forwarded_candidates,
                session_locked,
            );
            // Safety net for a chrome implementation that raises cancellation
            // outside the key path above.
            if self.shell.take_window_switcher_cancel() {
                self.server.cancel_window_switcher();
                self.shell.finish_window_switcher();
            }
            let super_held = self
                .server
                .depressed_modifiers()
                .has(aegis_model::input::Mods::SUPER);
            self.finish_keyboard_switcher_if_released(super_held);
            // Ctrl+Alt+Fn: the compositor performs console VT switches itself
            // through libseat (the kernel never sees the key once libinput
            // owns evdev). No-op on the nested backend.
            if let Some(vt) = self.server.take_vt_switch() {
                log::info!("{}: VT switch requested to tty{vt}", self.host.name());
                self.host.switch_vt(vt);
            }
        }

        // Apply scoped synthetic actions only after physical input has updated
        // xkb modifier state. The target-local batch was authorized on the IPC
        // thread; this main-loop pass validates live geometry, z-order, and
        // shell occlusion before sending any event.
        had_input |= !pending_synthetic_input.is_empty();
        non_cursor_input |= !pending_synthetic_input.is_empty();
        for (cmd, ts, origin) in pending_synthetic_input {
            let effect = match &cmd {
                aegis_ipc::Command::InjectInput { id, actions } => {
                    let prepared = self.server.prepare_synthetic_input(*id, actions);
                    if let Some(events) = prepared {
                        let has_key = events.iter().any(|event| {
                            matches!(event, aegis_model::input::InputEvent::Key { .. })
                        });
                        let blocked_by_chrome = (has_key && self.shell.captures_keyboard())
                            || events.iter().any(|event| {
                                matches!(
                                    *event,
                                    aegis_model::input::InputEvent::PointerMotion { x, y }
                                        if self.shell.captures_pointer_at(
                                            x,
                                            y,
                                            self.input_acc.display_size
                                        )
                                )
                            });
                        if blocked_by_chrome {
                            aegis_ipc::Effect::Refused {
                                reason: "target is covered by compositor chrome".into(),
                            }
                        } else {
                            self.server.focus_surface_by_id(*id);
                            let no_bindings = aegis_model::keybind::Keymap::default();
                            let actions = self.server.forward_input(&events, &no_bindings);
                            debug_assert!(actions.is_empty());
                            if events.iter().any(|event| {
                                matches!(
                                    event,
                                    aegis_model::input::InputEvent::PointerMotion { .. }
                                )
                            }) {
                                self.synthetic_pointer_active = true;
                                self.chrome_pointer_captured = false;
                            }
                            aegis_ipc::Effect::Applied
                        }
                    } else {
                        aegis_ipc::Effect::Refused {
                            reason: "invalid, hidden, stale, or occluded target".into(),
                        }
                    }
                }
                _ => unreachable!(),
            };
            journal_effect_and_broadcast(&self.journal, &self.ipc, ts, origin, cmd, effect);
        }
        if session_locked {
            // Keep compositor chrome inert while it is hidden beneath the
            // secure frame. Physical events have already reached the lock
            // client through the server path above.
            input = aegis_shell::Input::default();
            input.set_display_size(self.input_acc.display_size.0, self.input_acc.display_size.1);
            input.set_cursor(-1.0, -1.0);
        } else {
            input.set_scroll(shell_scroll.0, shell_scroll.1);
            input.set_scroll_pixels(shell_scroll_pixels.0, shell_scroll_pixels.1);
        }

        // A host resize or an output-scale change (window moved to a monitor
        // with a different scale) reports the new *logical* size. The swapchain
        // follows the physical size; layout, input, and the advertised output
        // geometry stay logical. Re-advertise the buffer scale so the host
        // keeps mapping our pre-scaled buffer 1:1.
        if let Some(sz) = self.host.take_resize() {
            // The swapchain was rebuilt at a new size or modifier set; damage
            // from before the reconfigure does not describe the new
            // framebuffer, so the next frame renders in full.
            self.damage.force_full_redraw = true;
            if let Some(capture) = self.pending_capture.take() {
                refuse_capture_target(
                    &self.capture_worker,
                    capture.target,
                    "output changed before the captured frame became readable".to_owned(),
                    &self.journal,
                    &self.ipc,
                );
            }
            if self.host.surface_needs_recreate() {
                // Direct KMS: a hotplug changed the plane-modifier
                // intersection the surface was created with. Resize cannot
                // retcon a modifier, so recreate the surface and its canvas
                // at the new display set instead.
                self.surface = self.host.create_surface(&self.device)?;
                self.canvas = flux::Canvas::new(&self.surface)?;
            } else {
                let (pw, ph) = self.host.physical_size();
                self.surface.resize(pw, ph)?;
            }
            self.damage.composite_slot_damage.clear();
            if let Err(error) = self.surface.prepare_readback() {
                log::warn!(
                    "capture: could not preallocate resized readback staging: {error}{}",
                    flux_last_error_detail()
                );
            }
            self.host.set_buffer_scale();
            self.server.set_outputs(self.host.output_infos());
            // KMS plane capabilities can change when a connector is added,
            // removed, or remodeset. Existing linux-dmabuf v4 feedback
            // objects are subscriptions, so refresh their preferred scanout
            // tranche after the backend and advertised outputs agree on the
            // new topology. Semantically unchanged capabilities are a no-op.
            self.server.update_dmabuf_feedback(
                self.host.dmabuf_scanout_formats(),
                self.host.dmabuf_scanout_device(),
            );
            // The logical extent follows the fresh server geometry (backend
            // + overrides), so a scale override or a hotplug to a
            // different-scale output re-lays the chrome out correctly.
            let logical = self
                .server
                .output_infos()
                .first()
                .map(|o| o.geometry.logical_size())
                .map(|s| (s.w as f32, s.h as f32))
                .unwrap_or((sz.w as f32, sz.h as f32));
            self.input_acc.display_size = logical;
            input.set_display_size(logical.0, logical.1);
        }

        // Parallax samples only exposed wallpaper. Crossing a client window
        // deliberately withholds intermediate targets; the wallpaper's
        // time-based filter then connects the two exposed samples smoothly
        // when the pointer emerges on the other side.
        let pointer = self.input_acc.cursor;
        let pointer_over_wallpaper = !session_locked
            && !self
                .shell
                .captures_pointer_at(pointer.0, pointer.1, self.input_acc.display_size)
            && !self.server.client_occupies_point(pointer.0, pointer.1)
            && self.server.interactive().is_none()
            && !self.server.drag_active();
        if let Some(wallpaper) = self.wallpaper.as_mut() {
            wallpaper.set_pointer_position(
                pointer_over_wallpaper.then_some(pointer),
                self.input_acc.display_size,
            );
        }

        // Chrome owns the host cursor while it owns pointer routing. This is
        // what gives the launcher's search field a text caret and interactive
        // HUD/dock controls a pointing hand; leaving chrome restores the
        // focused client's requested cursor (including hidden cursors).
        let current_cursor = self.capture_cursor_state();
        let cursor_hidden = current_cursor.hidden;
        let cursor_shape = current_cursor.shape;
        // With a KMS cursor plane, plain pointer motion over client content
        // changes no compositor pixels. Let presentation issue a cursor-only
        // atomic commit instead of turning every mouse report into a full
        // Vulkan frame. Chrome hover, drags, client cursor surfaces, locks,
        // and every non-motion input remain conservative full-damage signals.
        let cursor_plane_only = had_input
            && !non_cursor_input
            && pointer_motion_only
            && pointer_before != self.input_acc.cursor
            && self.host.supports_hardware_cursor()
            && !session_locked
            && !self.shell.captures_pointer_at(
                self.input_acc.cursor.0,
                self.input_acc.cursor.1,
                self.input_acc.display_size,
            )
            && self.server.interactive().is_none()
            && !self.server.drag_active()
            && !self.server.client_cursor_surface_active()
            && !self
                .wallpaper
                .as_ref()
                .is_some_and(aegis_wallpaper::Wallpaper::parallax_pointer_active);
        if cursor_plane_only {
            had_input = false;
        }
        if cursor_hidden != self.last_cursor_hidden
            || (!cursor_hidden && cursor_shape != self.last_cursor_shape)
        {
            if cursor_hidden {
                self.host.hide_cursor();
            } else {
                self.host.set_cursor_shape(cursor_shape);
            }
            self.last_cursor_shape = cursor_shape;
            self.last_cursor_hidden = cursor_hidden;
        }

        // Apply the tiling policy to the current workspace when tiled mode is
        // on (ADR-0024). No-op when off; reconfigures only windows whose
        // target moved. The work-area is the focused output's logical rect
        // (ADR-0028) inset by the chrome's reserved edges, so tiles avoid
        // the dock (ADR-0024 chrome-aware work-area).
        self.server.apply_tiling(
            self.shell
                .reserved()
                .inset(self.server.output_logical_rect()),
        );

        for request in self.interaction_domain_capture_rx.try_iter() {
            if self.server.session_locked() || !self.host.is_active() {
                let _ = request
                    .reply
                    .send(Err("session is locked or inactive".into()));
                continue;
            }
            if !self.capture_worker.reserve() {
                let _ = request
                    .reply
                    .send(Err("another capture is still being processed".into()));
                continue;
            }
            match begin_interaction_domain_capture(
                &mut self.interaction_domain_render_targets,
                &self.device,
                &mut self.renderer,
                &self.server,
                request.interaction_domain,
                request.region,
                self.capture_worker.security_generation(),
            ) {
                Ok(prepared) => {
                    let mut context = prepared.context;
                    match self.observations.issue_bounded(
                        request.actor,
                        context.semantic.clone(),
                        request.max_observations,
                    ) {
                        Ok(observation) => {
                            context.observation = Some(observation);
                            self.pending_interaction_domain_capture =
                                Some(PendingInteractionDomainCapture {
                                    readback: prepared.readback,
                                    context,
                                    reply: request.reply,
                                });
                        }
                        Err(reason) => {
                            self.capture_worker.release();
                            let _ = request.reply.send(Err(reason));
                        }
                    }
                }
                Err(reason) => {
                    self.capture_worker.release();
                    let _ = request.reply.send(Err(reason));
                }
            }
        }

        Ok(FrameState {
            input,
            session_locked,
            cursor_hidden,
            cursor_shape,
            had_input,
            pending_screenshots,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aegis_model::Point;
    use aegis_model::input::{ButtonState, InputEvent, SyntheticInputAction};
    use aegis_model::interaction_domain::InteractionDomainId;
    use aegis_model::window::WindowId;

    fn frame(input: aegis_shell::Input, had_input: bool) -> FrameState {
        FrameState {
            input,
            session_locked: false,
            cursor_hidden: false,
            cursor_shape: 1,
            had_input,
            pending_screenshots: Vec::new(),
        }
    }

    #[test]
    fn frame_state_merge_preserves_edges_until_redraw() {
        let mut first = aegis_shell::Input::new((100.0, 80.0), 0.0);
        first
            .set_cursor(10.0, 20.0)
            .set_mouse_down(lens::MouseButton::Left, true)
            .set_mouse_pressed(lens::MouseButton::Left, true)
            .set_scroll(1.0, 2.0)
            .set_text("a")
            .push_key(lens::key::LEFT, true, false);
        first.as_raw_mut().ime_delete_before = 2;
        let mut merged = frame(first, true);

        let mut second = aegis_shell::Input::new((120.0, 90.0), 0.0);
        second
            .set_cursor(30.0, 40.0)
            .set_mouse_down(lens::MouseButton::Left, false)
            .set_mouse_released(lens::MouseButton::Left, true)
            .set_scroll(3.0, 4.0)
            .set_text("b")
            .push_key(lens::key::RIGHT, true, false);
        second.as_raw_mut().ime_delete_before = 3;
        second.as_raw_mut().ime_delete_after = 4;
        merged.merge(frame(second, true));

        let raw = merged.input.as_raw();
        assert_eq!((raw.cursor.x, raw.cursor.y), (30.0, 40.0));
        assert_eq!((raw.display_size.x, raw.display_size.y), (120.0, 90.0));
        assert!(!raw.mouse_down[lens_sys::lens_mouse_button::LENS_MOUSE_LEFT as usize]);
        assert!(raw.mouse_pressed[lens_sys::lens_mouse_button::LENS_MOUSE_LEFT as usize]);
        assert!(raw.mouse_released[lens_sys::lens_mouse_button::LENS_MOUSE_LEFT as usize]);
        assert_eq!((raw.scroll_x, raw.scroll_y), (4.0, 6.0));
        assert_eq!(raw.key_count, 2);
        assert_eq!(raw.keys[0].key, lens::key::LEFT);
        assert_eq!(raw.keys[1].key, lens::key::RIGHT);
        assert_eq!(raw.ime_delete_before, 5);
        assert_eq!(raw.ime_delete_after, 4);
        let text = raw
            .text_utf8
            .iter()
            .take_while(|value| **value != 0)
            .map(|value| *value as u8)
            .collect::<Vec<_>>();
        assert_eq!(text, b"ab");
    }

    #[test]
    fn keybinding_invocation_keeps_the_trigger_edge_modifier_state() {
        let invocation = keybinding_invocation(
            aegis_model::keybind::Action::CycleFocus,
            (20.0, 30.0),
            aegis_model::input::KeyChar {
                keysym: aegis_model::input::XKB_KEY_Tab,
                ch: None,
                mods: aegis_model::input::Mods::SUPER,
            },
        );
        let end_of_batch_mods = aegis_model::input::Mods::NONE;

        assert!(invocation.super_held);
        assert!(!end_of_batch_mods.has(aegis_model::input::Mods::SUPER));
    }

    #[test]
    fn applied_agent_actions_become_ordered_privacy_preserving_feedback() {
        let actions = [
            SyntheticInputAction::PointerMove {
                position: Point { x: 4, y: 5 },
            },
            SyntheticInputAction::Click {
                position: Point { x: 8, y: 9 },
                button: 0x110,
            },
            SyntheticInputAction::KeyPress { code: 30 },
        ];
        let events = [
            InputEvent::PointerMotion { x: 104.0, y: 205.0 },
            InputEvent::PointerMotion { x: 108.0, y: 209.0 },
            InputEvent::PointerButton {
                button: 0x110,
                state: ButtonState::Pressed,
            },
            InputEvent::PointerButton {
                button: 0x110,
                state: ButtonState::Released,
            },
            InputEvent::Key {
                code: 30,
                state: ButtonState::Pressed,
            },
            InputEvent::Key {
                code: 30,
                state: ButtonState::Released,
            },
        ];
        let mut sequence = 40;
        let feedback = agent_activities_from_applied_input(
            InteractionDomainId(7),
            "Fuji",
            WindowId(42),
            &actions,
            &events,
            &mut sequence,
        );

        assert_eq!(feedback.len(), 3);
        assert_eq!(feedback[0].sequence, 41);
        assert_eq!(feedback[0].position, Some(Point { x: 104, y: 205 }));
        assert_eq!(feedback[1].position, Some(Point { x: 108, y: 209 }));
        assert_eq!(
            feedback[1].kind,
            aegis_shell::AgentInputKind::Click { button: 0x110 }
        );
        assert_eq!(feedback[2].sequence, 43);
        assert_eq!(feedback[2].position, None);
        assert_eq!(feedback[2].kind, aegis_shell::AgentInputKind::Keyboard);
        assert_eq!(sequence, 43);
    }

    #[test]
    fn malformed_prepared_event_stream_does_not_invent_pointer_feedback() {
        let actions = [SyntheticInputAction::Click {
            position: Point { x: 8, y: 9 },
            button: 0x110,
        }];
        let mut sequence = 3;
        let feedback = agent_activities_from_applied_input(
            InteractionDomainId(7),
            "Fuji",
            WindowId(42),
            &actions,
            &[],
            &mut sequence,
        );
        assert!(feedback.is_empty());
        assert_eq!(sequence, 3);
    }
}
