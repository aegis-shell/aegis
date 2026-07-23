use super::*;

pub(super) struct FrameState {
    pub(super) input: ass_shell::Input,
    pub(super) session_locked: bool,
    pub(super) cursor_hidden: bool,
    pub(super) cursor_shape: u32,
    pub(super) pending_screenshots: Vec<(ass_ipc::Command, u64, ass_ipc::Origin)>,
}

fn agent_activities_from_applied_input(
    realm: ass_core::realm::RealmId,
    realm_label: &str,
    window: ass_core::window::WindowId,
    actions: &[ass_core::input::SyntheticInputAction],
    events: &[ass_core::input::InputEvent],
    sequence: &mut u64,
) -> Vec<ass_shell::AgentActivity> {
    use ass_core::input::{InputEvent, SyntheticInputAction};

    // Every prepared pointer action contributes exactly one global motion
    // event before its button/axis events. Reading those positions preserves
    // the exact coordinates the server applied rather than remapping target-
    // local coordinates a second time in the presentation layer.
    let mut pointer_positions = events.iter().filter_map(|event| match *event {
        InputEvent::PointerMotion { x, y } => Some(ass_core::Point {
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
                    ass_shell::AgentInputKind::PointerMove,
                ),
                SyntheticInputAction::Click { button, .. } => (
                    Some(pointer_positions.next()?),
                    ass_shell::AgentInputKind::Click { button },
                ),
                SyntheticInputAction::Scroll { dx, dy, .. } => (
                    Some(pointer_positions.next()?),
                    ass_shell::AgentInputKind::Scroll { dx, dy },
                ),
                // Do not copy the key code into presentation state: the
                // feedback says only that keyboard input occurred.
                SyntheticInputAction::KeyPress { .. } => {
                    (None, ass_shell::AgentInputKind::Keyboard)
                }
            };
            *sequence = sequence.saturating_add(1);
            Some(ass_shell::AgentActivity {
                sequence: *sequence,
                realm,
                realm_label: realm_label.to_owned(),
                window,
                position,
                kind,
            })
        })
        .collect()
}

impl CompositorRuntime {
    pub(super) fn process_input(
        &mut self,
        work: IterationWork,
    ) -> Result<FrameState, Box<dyn std::error::Error>> {
        let IterationWork {
            frame_dt,
            pending_synthetic_input,
            pending_screenshots,
        } = work;
        // Process client protocol traffic.
        self.server.dispatch();
        let session_locked = self.server.session_locked();
        let realm_revision = self.server.realm_snapshot().revision;
        for (realm, damage) in self.server.take_realm_damage() {
            self.realm_damage_sequence = self.realm_damage_sequence.saturating_add(1);
            if let Some(ipc) = &self.ipc {
                ipc.broadcast(ass_ipc::Event::RealmDamaged {
                    realm,
                    sequence: self.realm_damage_sequence,
                    revision: realm_revision,
                    damage,
                });
            }
        }
        if session_locked {
            self.super_tap.cancel_current();
        }
        for state in self.server.take_text_input_states() {
            self.host.set_text_input_state(state);
        }
        for event in self.host.take_text_input() {
            self.server.text_input_event(&event);
        }
        for event in self.host.take_pointer_gestures() {
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
        let mut input = ass_shell::Input::default();
        input.set_display_size(self.input_acc.display_size.0, self.input_acc.display_size.1);
        input.set_cursor(self.input_acc.cursor.0, self.input_acc.cursor.1);
        input.set_mouse_down(lens::MouseButton::Left, self.input_acc.mouse_down[0]);
        input.set_mouse_down(lens::MouseButton::Right, self.input_acc.mouse_down[1]);
        input.set_mouse_down(lens::MouseButton::Middle, self.input_acc.mouse_down[2]);
        input.set_dt(frame_dt);
        let mut shell_scroll = (0.0_f32, 0.0_f32);
        let mut shell_scroll_pixels = (0.0_f32, 0.0_f32);
        let pointer_before = self.input_acc.cursor;
        let mut events = self.host.take_input();
        if !events.is_empty() {
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
                use ass_core::input::{InputEvent::*, TabletEvent};
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
        // When chrome (the launcher or a context menu) captures the keyboard,
        // key events go to chrome rather than the focused client. The shell
        // reports capture state from the previous frame's render / key
        // handling, so this is stable for the whole batch.
        let keyboard_captured = !session_locked && self.shell.captures_keyboard();
        if !events.is_empty() {
            for ev in &events {
                use ass_core::input::InputEvent::*;
                match *ev {
                    PointerMotion { x, y } => {
                        input.set_cursor(x, y);
                        self.input_acc.cursor = (x, y);
                    }
                    PointerButton { button, state } => {
                        if state.is_pressed() {
                            // A pointer gesture while Super is held is a
                            // modifier drag, not a bare launcher-key tap.
                            self.super_tap.cancel_current();
                        }
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
                        input.set_cursor(-1.0, -1.0);
                        self.input_acc.cursor = (-1.0, -1.0);
                    }
                    Key { code, state } if keyboard_captured => {
                        // Capture: advance the server's xkb state on every key
                        // event (press and release both keep modifier tracking
                        // consistent), and feed the launcher brain only on
                        // press so typed characters are not double-counted.
                        // Captured keys are withheld from clients below.
                        if !session_locked && self.super_tap.on_key(code, state.is_pressed()) {
                            self.shell.toggle();
                        }
                        if let Some(kc) = self.server.key_char(code, state.is_pressed())
                            && state.is_pressed()
                        {
                            self.shell.key_char(kc);
                        }
                        // VT switch keys stay compositor-owned while chrome
                        // holds the keyboard too.
                        if let Some(vt) = self.server.take_vt_switch() {
                            log::info!("{}: VT switch requested to tty{vt}", self.host.name());
                            self.host.switch_vt(vt);
                        }
                    }
                    Key { code, state } => {
                        // Not capturing: keys forward to the client normally
                        // (below). The Super-tap detector still observes every
                        // key so a clean tap can open the launcher.
                        if !session_locked && self.super_tap.on_key(code, state.is_pressed()) {
                            self.shell.toggle();
                        }
                    }
                    PointerAxis(frame) => {
                        use ass_core::input::PointerAxisSource;
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
            // from the client stream. Pointer motion into chrome becomes one
            // leave; buttons and scroll are consumed until the pointer exits.
            // This prevents a dock/workspace/launcher click from also clicking
            // the client window visually underneath it.
            let display = self.input_acc.display_size;
            let mut route_cursor = pointer_before;
            let mut forwarded = Vec::with_capacity(events.len() + 1);
            for ev in events.iter().copied() {
                use ass_core::input::InputEvent::*;
                match ev {
                    Key { .. } if keyboard_captured => {}
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
            let actions = self.server.forward_input(&forwarded, &self.keymap);
            // Ctrl+Alt+Fn: the compositor performs console VT switches itself
            // through libseat (the kernel never sees the key once libinput
            // owns evdev). No-op on the nested backend.
            if let Some(vt) = self.server.take_vt_switch() {
                log::info!("{}: VT switch requested to tty{vt}", self.host.name());
                self.host.switch_vt(vt);
            }
            // Dispatch matched global bindings. (Empty while the launcher
            // captures the keyboard — those keys went to the search box.)
            for action in actions {
                use ass_core::keybind::Action;
                let ts = self.start.elapsed().as_millis() as u64;
                let origin = ass_ipc::Origin::Keybinding;
                match action {
                    Action::ToggleLauncher => self.shell.toggle(),
                    Action::ToggleOverview => self.shell.toggle_overview(),
                    Action::CloseFocused => {
                        if let Some(id) = self.server.focused_toplevel_id() {
                            let cmd = ass_ipc::Command::Close { id };
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
                        let cmd = ass_ipc::Command::Cycle { forward: true };
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
                        let cmd = ass_ipc::Command::Cycle { forward: false };
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
                        let cmd = ass_ipc::Command::SwitchWorkspace {
                            dir: ass_core::workspace::Switch::Next,
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
                        let cmd = ass_ipc::Command::SwitchWorkspace {
                            dir: ass_core::workspace::Switch::Prev,
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
                        let cmd = ass_ipc::Command::ToggleTiling;
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
                    Action::Quit => {
                        let cmd = ass_ipc::Command::Quit;
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
                        // Refuse to open the selector while locked or inactive;
                        // the selector itself also suppresses confirmation in
                        // those states, but this avoids the modal entirely.
                        if session_locked || !self.host.is_active() {
                            log::debug!("screenshot: suppressed while locked or inactive");
                            continue;
                        }
                        self.shell.start_screenshot();
                    }
                }
            }
        }

        // Apply scoped synthetic actions only after physical input has updated
        // xkb modifier state. The target-local batch was authorized on the IPC
        // thread; this main-loop pass validates live geometry, z-order, and
        // shell occlusion before sending any event.
        for (cmd, ts, origin) in pending_synthetic_input {
            let effect = match &cmd {
                ass_ipc::Command::InjectInput { id, actions } => {
                    let prepared = self.server.prepare_synthetic_input(*id, actions);
                    if let Some(events) = prepared {
                        let has_key = events
                            .iter()
                            .any(|event| matches!(event, ass_core::input::InputEvent::Key { .. }));
                        let blocked_by_chrome = (has_key && self.shell.captures_keyboard())
                            || events.iter().any(|event| {
                                matches!(
                                    *event,
                                    ass_core::input::InputEvent::PointerMotion { x, y }
                                        if self.shell.captures_pointer_at(
                                            x,
                                            y,
                                            self.input_acc.display_size
                                        )
                                )
                            });
                        if blocked_by_chrome {
                            ass_ipc::Effect::Refused {
                                reason: "target is covered by compositor chrome".into(),
                            }
                        } else {
                            self.server.focus_surface_by_id(*id);
                            let no_bindings = ass_core::keybind::Keymap::default();
                            let actions = self.server.forward_input(&events, &no_bindings);
                            debug_assert!(actions.is_empty());
                            if events.iter().any(|event| {
                                matches!(event, ass_core::input::InputEvent::PointerMotion { .. })
                            }) {
                                self.synthetic_pointer_active = true;
                                self.chrome_pointer_captured = false;
                            }
                            ass_ipc::Effect::Applied
                        }
                    } else {
                        ass_ipc::Effect::Refused {
                            reason: "invalid, hidden, stale, or occluded target".into(),
                        }
                    }
                }
                ass_ipc::Command::InjectRealmInput { realm, id, actions } => {
                    let realm_snapshot = self.server.realm_snapshot();
                    let realm_label = realm_snapshot
                        .realms
                        .iter()
                        .find(|candidate| candidate.id == *realm)
                        .map(|candidate| candidate.label.clone())
                        .unwrap_or_else(|| format!("Realm {}", realm.0));
                    let seat = realm_snapshot
                        .seats
                        .iter()
                        .find(|seat| seat.realm == *realm && seat.enabled)
                        .map(|seat| seat.id);
                    let Some(seat) = seat else {
                        journal_effect_and_broadcast(
                            &self.journal,
                            &self.ipc,
                            ts,
                            origin,
                            cmd.clone(),
                            ass_ipc::Effect::Refused {
                                reason: "realm has no active seat".into(),
                            },
                        );
                        continue;
                    };
                    let Some(events) = self
                        .server
                        .prepare_agent_synthetic_input(seat, *id, actions)
                    else {
                        journal_effect_and_broadcast(
                            &self.journal,
                            &self.ipc,
                            ts,
                            origin,
                            cmd.clone(),
                            ass_ipc::Effect::Refused {
                                reason: "invalid, stale, or unauthorized realm target".into(),
                            },
                        );
                        continue;
                    };
                    match self.server.forward_agent_input_to(seat, *id, &events) {
                        Ok(()) => {
                            for activity in agent_activities_from_applied_input(
                                *realm,
                                &realm_label,
                                *id,
                                actions,
                                &events,
                                &mut self.agent_activity_sequence,
                            ) {
                                self.shell.report_agent_activity(activity);
                            }
                            ass_ipc::Effect::Applied
                        }
                        Err(error) => ass_ipc::Effect::Refused {
                            reason: error.to_string(),
                        },
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
            input = ass_shell::Input::default();
            input.set_display_size(self.input_acc.display_size.0, self.input_acc.display_size.1);
            input.set_cursor(-1.0, -1.0);
            input.set_dt(frame_dt);
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
            if let Err(error) = self.surface.prepare_readback() {
                log::warn!(
                    "capture: could not preallocate resized readback staging: {error}{}",
                    flux_last_error_detail()
                );
            }
            self.host.set_buffer_scale();
            self.server.set_outputs(self.host.output_infos());
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

        // Chrome owns the host cursor while it owns pointer routing. This is
        // what gives the launcher's search field a text caret and interactive
        // HUD/dock controls a pointing hand; leaving chrome restores the
        // focused client's requested cursor (including hidden cursors).
        let chrome_cursor = (!session_locked)
            .then(|| {
                self.shell.cursor_shape_at(
                    self.input_acc.cursor.0,
                    self.input_acc.cursor.1,
                    self.input_acc.display_size,
                )
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
        let cursor_hidden = owned_cursor.is_none() && self.server.cursor_hidden();
        let cursor_shape = owned_cursor.unwrap_or_else(|| self.server.cursor_shape().max(1));
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

        for request in self.realm_capture_rx.try_iter() {
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
            match begin_realm_capture(
                &mut self.realm_render_targets,
                &self.device,
                &mut self.renderer,
                &self.server,
                request.realm,
                request.region,
                self.capture_worker.security_generation(),
            ) {
                Ok(prepared) => {
                    self.pending_realm_capture = Some(PendingRealmCapture {
                        readback: prepared.readback,
                        context: prepared.context,
                        reply: request.reply,
                    });
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
            pending_screenshots,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ass_core::Point;
    use ass_core::input::{ButtonState, InputEvent, SyntheticInputAction};
    use ass_core::realm::RealmId;
    use ass_core::window::WindowId;

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
            RealmId(7),
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
            ass_shell::AgentInputKind::Click { button: 0x110 }
        );
        assert_eq!(feedback[2].sequence, 43);
        assert_eq!(feedback[2].position, None);
        assert_eq!(feedback[2].kind, ass_shell::AgentInputKind::Keyboard);
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
            RealmId(7),
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
