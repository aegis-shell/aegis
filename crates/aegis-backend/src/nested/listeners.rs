use super::protocol::*;
use super::*;

// ----- event callbacks ----------------------------------------------------

pub(super) unsafe extern "C" fn on_global(
    data: *mut c_void,
    registry: *mut ffi::wl_proxy,
    name: u32,
    interface: *const std::os::raw::c_char,
    version: u32,
) {
    unsafe {
        let st = &mut *(data as *mut State);
        let iface = CStr::from_ptr(interface).to_bytes();
        if iface == b"wl_compositor" {
            st.compositor = registry_bind(
                registry,
                name,
                &ffi::wl_compositor_interface,
                version.min(4),
            );
        } else if iface == b"xdg_wm_base" {
            st.wm_base = registry_bind(registry, name, &ffi::xdg_wm_base_interface, 1);
        } else if iface == b"wl_seat" {
            // Pointer v9 preserves axis framing, source, true sequence stops,
            // high-resolution wheel values, and natural-scroll direction.
            st.seat = registry_bind(registry, name, &ffi::wl_seat_interface, version.min(9));
        } else if iface == b"wl_output" {
            // Bind at v2 for the `scale` event (added in v2); v4's name/description
            // are not requested, so the 4-slot vtable is never over-run. Track each
            // output so `wl_surface.enter` can resolve the scale of the output the
            // window is on.
            let output = registry_bind(registry, name, &ffi::wl_output_interface, version.min(2));
            if !output.is_null() {
                ffi::wl_proxy_add_listener(
                    output,
                    &OUTPUT_LISTENER as *const _ as *const c_void,
                    data,
                );
                st.outputs.push((output, 1));
            }
        } else if iface == b"wp_viewporter" {
            st.viewporter = registry_bind(
                registry,
                name,
                &ffi::wp_viewporter_interface,
                version.min(1),
            );
        } else if iface == b"wp_fractional_scale_manager_v1" {
            st.fractional_scale_manager = registry_bind(
                registry,
                name,
                &ffi::wp_fractional_scale_manager_v1_interface,
                version.min(1),
            );
        } else if iface == b"wp_cursor_shape_manager_v1" {
            st.cursor_shape_manager = registry_bind(
                registry,
                name,
                &ffi::wp_cursor_shape_manager_v1_interface,
                version.min(2),
            );
        } else if iface == b"zwp_pointer_gestures_v1" {
            st.pointer_gestures_manager = registry_bind(
                registry,
                name,
                &ffi::zwp_pointer_gestures_v1_interface,
                version.min(3),
            );
            ensure_pointer_gestures(st, data);
        } else if iface == b"zwp_text_input_manager_v3" {
            st.text_input_manager = registry_bind(
                registry,
                name,
                &ffi::zwp_text_input_manager_v3_interface,
                version.min(1),
            );
        }
    }
}

pub(super) unsafe extern "C" fn on_global_remove(
    _data: *mut c_void,
    _registry: *mut ffi::wl_proxy,
    _name: u32,
) {
}

pub(super) unsafe extern "C" fn on_ping(
    _data: *mut c_void,
    wm_base: *mut ffi::wl_proxy,
    serial: u32,
) {
    unsafe {
        pong(wm_base, serial);
    }
}

pub(super) unsafe extern "C" fn on_xdg_surface_configure(
    data: *mut c_void,
    xdg_surface: *mut ffi::wl_proxy,
    serial: u32,
) {
    unsafe {
        let st = &mut *(data as *mut State);
        ack_configure(xdg_surface, serial);
        st.configured = true;
    }
}

pub(super) unsafe extern "C" fn on_toplevel_configure(
    data: *mut c_void,
    _toplevel: *mut ffi::wl_proxy,
    width: std::os::raw::c_int,
    height: std::os::raw::c_int,
    _states: *mut ffi::wl_array,
) {
    unsafe {
        let st = &mut *(data as *mut State);
        if width > 0 && height > 0 && (width != st.width || height != st.height) {
            st.pending_width = width;
            st.pending_height = height;
            st.resized = true;
        }
    }
}

pub(super) unsafe extern "C" fn on_toplevel_close(
    data: *mut c_void,
    _toplevel: *mut ffi::wl_proxy,
) {
    unsafe {
        (*(data as *mut State)).should_close = true;
    }
}

/// Host seat capabilities changed. When the host gains a pointer or keyboard
/// we bind it and install listeners; the host then sends input events. Losing
/// capability pushes a synthetic leave so the focus model clears cleanly.
pub(super) unsafe extern "C" fn on_seat_capabilities(
    data: *mut c_void,
    seat: *mut ffi::wl_proxy,
    caps: u32,
) {
    unsafe {
        let st = &mut *(data as *mut State);
        let want_pointer = (caps & ffi::WL_SEAT_CAPABILITY_POINTER) != 0;
        let want_keyboard = (caps & ffi::WL_SEAT_CAPABILITY_KEYBOARD) != 0;
        if want_pointer && st.pointer.is_null() {
            let p = seat_get_pointer(seat);
            if !p.is_null() {
                ffi::wl_proxy_add_listener(p, &POINTER_LISTENER as *const _ as *const c_void, data);
                st.pointer = p;
                if !st.cursor_shape_manager.is_null() {
                    st.cursor_shape_device = get_cursor_shape_device(st.cursor_shape_manager, p);
                }
                ensure_pointer_gestures(st, data);
                log::debug!("nested: host pointer bound");
            }
        } else if !want_pointer && !st.pointer.is_null() {
            destroy_pointer_gestures(st);
            if !st.cursor_shape_device.is_null() {
                ffi::wl_proxy_destroy(st.cursor_shape_device);
                st.cursor_shape_device = std::ptr::null_mut();
            }
            pointer_release(st.pointer);
            st.pointer = std::ptr::null_mut();
            st.pending_pointer_axis = PointerAxisFrame::default();
            st.input_events.push(InputEvent::PointerLeave);
        }
        if want_keyboard && st.keyboard.is_null() {
            let k = seat_get_keyboard(seat);
            if !k.is_null() {
                ffi::wl_proxy_add_listener(
                    k,
                    &KEYBOARD_LISTENER as *const _ as *const c_void,
                    data,
                );
                st.keyboard = k;
                log::debug!("nested: host keyboard bound");
            }
        } else if !want_keyboard && !st.keyboard.is_null() {
            keyboard_release(st.keyboard);
            st.keyboard = std::ptr::null_mut();
        }
    }
}

pub(super) unsafe extern "C" fn on_seat_name(
    _data: *mut c_void,
    _seat: *mut ffi::wl_proxy,
    _name: *const std::os::raw::c_char,
) {
}

// Host output listeners. We only care about `scale`: it drives the buffer
// scale so a HiDPI host maps our pre-scaled buffer 1:1 instead of upscaling a
// logical-sized one. `geometry`/`mode`/`done` are accepted (the vtable must be
// complete) but ignored.
pub(super) unsafe extern "C" fn on_output_geometry(
    _data: *mut c_void,
    _output: *mut ffi::wl_proxy,
    _x: i32,
    _y: i32,
    _physical_width: i32,
    _physical_height: i32,
    _subpixel: i32,
    _make: *const std::os::raw::c_char,
    _model: *const std::os::raw::c_char,
    _transform: i32,
) {
}

pub(super) unsafe extern "C" fn on_output_mode(
    _data: *mut c_void,
    _output: *mut ffi::wl_proxy,
    _flags: u32,
    _width: i32,
    _height: i32,
    _refresh: i32,
) {
}

pub(super) unsafe extern "C" fn on_output_done(_data: *mut c_void, _output: *mut ffi::wl_proxy) {}

pub(super) unsafe extern "C" fn on_output_scale(
    data: *mut c_void,
    output: *mut ffi::wl_proxy,
    factor: i32,
) {
    unsafe {
        let st = &mut *(data as *mut State);
        let f = factor.max(1);
        if let Some(entry) = st.outputs.iter_mut().find(|o| o.0 == output) {
            entry.1 = f;
        } else {
            st.outputs.push((output, f));
        }
        // Adopt this scale when it belongs to the output we're on, or before the
        // first `enter` (so the very first frame already targets the right scale).
        if (output == st.current_output || st.current_output.is_null()) && f != st.scale {
            st.scale = f;
            if !st.fractional_active {
                st.scale_changed = true;
            }
        }
    }
}

pub(super) unsafe extern "C" fn on_preferred_scale(
    data: *mut c_void,
    _fractional_scale: *mut ffi::wl_proxy,
    scale_120: u32,
) {
    unsafe {
        let st = &mut *(data as *mut State);
        let scale_120 = scale_120.max(1);
        if scale_120 != st.preferred_scale_120 {
            st.preferred_scale_120 = scale_120;
            st.scale_changed = true;
        }
    }
}

// Host surface listeners. `enter` tells us which output the window sits on; we
// adopt that output's scale. `leave` keeps the last known scale.
pub(super) unsafe extern "C" fn on_surface_enter(
    data: *mut c_void,
    _surface: *mut ffi::wl_proxy,
    output: *mut ffi::wl_proxy,
) {
    unsafe {
        let st = &mut *(data as *mut State);
        st.current_output = output;
        if let Some(entry) = st.outputs.iter().find(|o| o.0 == output)
            && entry.1 != st.scale
        {
            st.scale = entry.1;
            st.scale_changed = true;
        }
    }
}

pub(super) unsafe extern "C" fn on_surface_leave(
    _data: *mut c_void,
    _surface: *mut ffi::wl_proxy,
    _output: *mut ffi::wl_proxy,
) {
}

// Host pointer listeners. We translate the host's pointer events into the
// backend-agnostic `InputEvent` stream consumed by the main loop. For a nested
// backend the window is the only host surface, so the host's enter/leave
// translate into pointer position rather than client focus changes — the
// server side decides which client is focused.
pub(super) unsafe extern "C" fn on_pointer_enter(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    serial: u32,
    _surface: *mut ffi::wl_proxy,
    x: i32,
    y: i32,
) {
    unsafe {
        let st = &mut *(data as *mut State);
        st.last_pointer_serial = serial;
        st.input_events.push(InputEvent::PointerMotion {
            x: ffi::wl_fixed_to_f32(x),
            y: ffi::wl_fixed_to_f32(y),
        });
    }
}

pub(super) unsafe extern "C" fn on_pointer_leave(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    _serial: u32,
    _surface: *mut ffi::wl_proxy,
) {
    unsafe {
        let st = &mut *(data as *mut State);
        st.input_events.push(InputEvent::PointerLeave);
    }
}

pub(super) unsafe extern "C" fn on_pointer_motion(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    _time: u32,
    x: i32,
    y: i32,
) {
    unsafe {
        let st = &mut *(data as *mut State);
        st.input_events.push(InputEvent::PointerMotion {
            x: ffi::wl_fixed_to_f32(x),
            y: ffi::wl_fixed_to_f32(y),
        });
    }
}

pub(super) unsafe extern "C" fn on_pointer_button(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    serial: u32,
    _time: u32,
    button: u32,
    state: u32,
) {
    unsafe {
        let st = &mut *(data as *mut State);
        st.last_pointer_serial = serial;
        // Host button codes are Linux input-event BTN_* codes already
        // (BTN_LEFT=0x110, BTN_RIGHT=0x111, BTN_MIDDLE=0x112). Pass through.
        st.input_events.push(InputEvent::PointerButton {
            button,
            state: aegis_model::input::ButtonState::from_wayland(state),
        });
    }
}

pub(super) unsafe extern "C" fn on_pointer_axis(
    data: *mut c_void,
    pointer: *mut ffi::wl_proxy,
    time: u32,
    axis: u32,
    value: i32,
) {
    unsafe {
        let st = &mut *(data as *mut State);
        let v = ffi::wl_fixed_to_f32(value);
        st.pending_pointer_axis.time = time;
        if v != 0.0 {
            let slot = pointer_axis_slot(&mut st.pending_pointer_axis, axis);
            if let Some(slot) = slot {
                slot.value = Some(slot.value.unwrap_or(0.0) + v);
            }
        }
        // Pointer v4 has no `frame`; preserve compatibility with an old host by
        // emitting each axis callback as its own source-unknown frame.
        if ffi::wl_proxy_get_version(pointer) < 5 {
            flush_pointer_axis(st);
        }
    }
}

pub(super) fn pointer_axis_slot(
    frame: &mut PointerAxisFrame,
    axis: u32,
) -> Option<&mut PointerAxis> {
    match axis {
        0 => Some(&mut frame.vertical),
        1 => Some(&mut frame.horizontal),
        _ => None,
    }
}

pub(super) fn flush_pointer_axis(st: &mut State) {
    let frame = std::mem::take(&mut st.pending_pointer_axis);
    if frame.has_data() {
        st.input_events.push(InputEvent::PointerAxis(frame));
    }
}

pub(super) unsafe extern "C" fn on_pointer_frame(data: *mut c_void, _pointer: *mut ffi::wl_proxy) {
    unsafe {
        flush_pointer_axis(&mut *(data as *mut State));
    }
}

pub(super) unsafe extern "C" fn on_pointer_axis_source(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    source: u32,
) {
    unsafe {
        (*(data as *mut State)).pending_pointer_axis.source = match source {
            0 => Some(PointerAxisSource::Wheel),
            1 => Some(PointerAxisSource::Finger),
            2 => Some(PointerAxisSource::Continuous),
            3 => Some(PointerAxisSource::WheelTilt),
            _ => None,
        };
    }
}

pub(super) unsafe extern "C" fn on_pointer_axis_stop(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    time: u32,
    axis: u32,
) {
    unsafe {
        let st = &mut *(data as *mut State);
        st.pending_pointer_axis.time = time;
        if let Some(slot) = pointer_axis_slot(&mut st.pending_pointer_axis, axis) {
            slot.stop = true;
        }
    }
}

pub(super) unsafe extern "C" fn on_pointer_axis_discrete(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    axis: u32,
    discrete: i32,
) {
    unsafe {
        let st = &mut *(data as *mut State);
        if let Some(slot) = pointer_axis_slot(&mut st.pending_pointer_axis, axis) {
            slot.discrete = Some(slot.discrete.unwrap_or(0) + discrete);
        }
    }
}

pub(super) unsafe extern "C" fn on_pointer_axis_value120(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    axis: u32,
    value120: i32,
) {
    unsafe {
        let st = &mut *(data as *mut State);
        if let Some(slot) = pointer_axis_slot(&mut st.pending_pointer_axis, axis) {
            slot.value120 = Some(slot.value120.unwrap_or(0) + value120);
        }
    }
}

pub(super) unsafe extern "C" fn on_pointer_axis_relative_direction(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    axis: u32,
    direction: u32,
) {
    unsafe {
        let st = &mut *(data as *mut State);
        if let Some(slot) = pointer_axis_slot(&mut st.pending_pointer_axis, axis) {
            slot.relative_direction = match direction {
                0 => Some(PointerAxisRelativeDirection::Identical),
                1 => Some(PointerAxisRelativeDirection::Inverted),
                _ => None,
            };
        }
    }
}

pub(super) unsafe extern "C" fn on_swipe_begin(
    data: *mut c_void,
    _gesture: *mut ffi::wl_proxy,
    _serial: u32,
    time: u32,
    _surface: *mut ffi::wl_proxy,
    fingers: u32,
) {
    unsafe {
        (*(data as *mut State))
            .pointer_gesture_events
            .push(PointerGestureEvent::SwipeBegin { time, fingers });
    }
}

pub(super) unsafe extern "C" fn on_swipe_update(
    data: *mut c_void,
    _gesture: *mut ffi::wl_proxy,
    time: u32,
    dx: i32,
    dy: i32,
) {
    unsafe {
        (*(data as *mut State))
            .pointer_gesture_events
            .push(PointerGestureEvent::SwipeUpdate {
                time,
                dx: ffi::wl_fixed_to_f32(dx),
                dy: ffi::wl_fixed_to_f32(dy),
            });
    }
}

pub(super) unsafe extern "C" fn on_swipe_end(
    data: *mut c_void,
    _gesture: *mut ffi::wl_proxy,
    _serial: u32,
    time: u32,
    cancelled: i32,
) {
    unsafe {
        (*(data as *mut State))
            .pointer_gesture_events
            .push(PointerGestureEvent::SwipeEnd {
                time,
                cancelled: cancelled != 0,
            });
    }
}

pub(super) unsafe extern "C" fn on_pinch_begin(
    data: *mut c_void,
    _gesture: *mut ffi::wl_proxy,
    _serial: u32,
    time: u32,
    _surface: *mut ffi::wl_proxy,
    fingers: u32,
) {
    unsafe {
        (*(data as *mut State))
            .pointer_gesture_events
            .push(PointerGestureEvent::PinchBegin { time, fingers });
    }
}

pub(super) unsafe extern "C" fn on_pinch_update(
    data: *mut c_void,
    _gesture: *mut ffi::wl_proxy,
    time: u32,
    dx: i32,
    dy: i32,
    scale: i32,
    rotation: i32,
) {
    unsafe {
        (*(data as *mut State))
            .pointer_gesture_events
            .push(PointerGestureEvent::PinchUpdate {
                time,
                dx: ffi::wl_fixed_to_f32(dx),
                dy: ffi::wl_fixed_to_f32(dy),
                scale: ffi::wl_fixed_to_f32(scale),
                rotation: ffi::wl_fixed_to_f32(rotation),
            });
    }
}

pub(super) unsafe extern "C" fn on_pinch_end(
    data: *mut c_void,
    _gesture: *mut ffi::wl_proxy,
    _serial: u32,
    time: u32,
    cancelled: i32,
) {
    unsafe {
        (*(data as *mut State))
            .pointer_gesture_events
            .push(PointerGestureEvent::PinchEnd {
                time,
                cancelled: cancelled != 0,
            });
    }
}

pub(super) unsafe extern "C" fn on_hold_begin(
    data: *mut c_void,
    _gesture: *mut ffi::wl_proxy,
    _serial: u32,
    time: u32,
    _surface: *mut ffi::wl_proxy,
    fingers: u32,
) {
    unsafe {
        (*(data as *mut State))
            .pointer_gesture_events
            .push(PointerGestureEvent::HoldBegin { time, fingers });
    }
}

pub(super) unsafe extern "C" fn on_hold_end(
    data: *mut c_void,
    _gesture: *mut ffi::wl_proxy,
    _serial: u32,
    time: u32,
    cancelled: i32,
) {
    unsafe {
        (*(data as *mut State))
            .pointer_gesture_events
            .push(PointerGestureEvent::HoldEnd {
                time,
                cancelled: cancelled != 0,
            });
    }
}

// Host keyboard listeners. The keymap event from the host is consumed only to
// close the file descriptor the host sent us; aegis runs its own xkbcommon
// compile and ships that keymap to clients (the host's keymap and ours match
// in practice since both use the default evdev/pc104/us RMLVO, but we do not
// depend on it). Key events are forwarded as `InputEvent::Key` with their
// raw evdev scancode; the server side has its own xkbcommon state for
// modifier tracking.
pub(super) unsafe extern "C" fn on_keyboard_keymap(
    _data: *mut c_void,
    _keyboard: *mut ffi::wl_proxy,
    _format: u32,
    fd: i32,
    _size: u32,
) {
    // We compile our own keymap; just close the host's fd so it does not leak.
    if fd >= 0 {
        unsafe { libc::close(fd) };
    }
}

pub(super) unsafe extern "C" fn on_keyboard_enter(
    _data: *mut c_void,
    _keyboard: *mut ffi::wl_proxy,
    _serial: u32,
    _surface: *mut ffi::wl_proxy,
    _keys: *mut ffi::wl_array,
) {
    // The host's enter tells us our nested window gained keyboard focus. We
    // do not need to mirror this to clients — the server's focus model
    // handles enter/leave from our side.
}

pub(super) unsafe extern "C" fn on_keyboard_leave(
    _data: *mut c_void,
    _keyboard: *mut ffi::wl_proxy,
    _serial: u32,
    _surface: *mut ffi::wl_proxy,
) {
}

pub(super) unsafe extern "C" fn on_keyboard_key(
    data: *mut c_void,
    _keyboard: *mut ffi::wl_proxy,
    _serial: u32,
    _time: u32,
    key: u32,
    state: u32,
) {
    unsafe {
        let st = &mut *(data as *mut State);
        // The host delivers evdev scancodes (offset 8 from xkbcommon keycodes,
        // but the Wayland `wl_keyboard.key` event uses evdev scancodes directly,
        // so no shift needed for forwarding).
        st.input_events.push(InputEvent::Key {
            code: key,
            state: aegis_model::input::ButtonState::from_wayland(state),
        });
    }
}

pub(super) unsafe extern "C" fn on_keyboard_modifiers(
    _data: *mut c_void,
    _keyboard: *mut ffi::wl_proxy,
    _serial: u32,
    _depressed: u32,
    _latched: u32,
    _locked: u32,
    _group: u32,
) {
    // We compute modifiers ourselves from xkb_state in the server; the host's
    // modifier event would double-count if we forwarded it.
}

pub(super) unsafe extern "C" fn on_keyboard_repeat_info(
    _data: *mut c_void,
    _keyboard: *mut ffi::wl_proxy,
    _rate: i32,
    _delay: i32,
) {
    // We advertise our own repeat_info (25/250) on client bind; the host's
    // rate is ignored.
}

pub(super) unsafe extern "C" fn on_text_input_enter(
    data: *mut c_void,
    _text_input: *mut ffi::wl_proxy,
    _surface: *mut ffi::wl_proxy,
) {
    unsafe {
        let st = data as *mut State;
        (*st).text_input_entered = true;
        send_text_input_state(st);
    }
}

pub(super) unsafe extern "C" fn on_text_input_leave(
    data: *mut c_void,
    _text_input: *mut ffi::wl_proxy,
    _surface: *mut ffi::wl_proxy,
) {
    unsafe {
        (*(data as *mut State)).text_input_entered = false;
    }
}

pub(super) unsafe extern "C" fn on_text_input_preedit(
    data: *mut c_void,
    _text_input: *mut ffi::wl_proxy,
    text: *const std::os::raw::c_char,
    cursor_begin: i32,
    cursor_end: i32,
) {
    unsafe {
        let text = if text.is_null() {
            None
        } else {
            Some(CStr::from_ptr(text).to_string_lossy().into_owned())
        };
        (*(data as *mut State))
            .text_input_events
            .push(TextInputEvent::Preedit {
                text,
                cursor_begin,
                cursor_end,
            });
    }
}

pub(super) unsafe extern "C" fn on_text_input_commit(
    data: *mut c_void,
    _text_input: *mut ffi::wl_proxy,
    text: *const std::os::raw::c_char,
) {
    unsafe {
        let text = if text.is_null() {
            None
        } else {
            Some(CStr::from_ptr(text).to_string_lossy().into_owned())
        };
        (*(data as *mut State))
            .text_input_events
            .push(TextInputEvent::Commit(text));
    }
}

pub(super) unsafe extern "C" fn on_text_input_delete(
    data: *mut c_void,
    _text_input: *mut ffi::wl_proxy,
    before_length: u32,
    after_length: u32,
) {
    unsafe {
        (*(data as *mut State))
            .text_input_events
            .push(TextInputEvent::DeleteSurrounding {
                before_length,
                after_length,
            });
    }
}

pub(super) unsafe extern "C" fn on_text_input_done(
    data: *mut c_void,
    _text_input: *mut ffi::wl_proxy,
    _serial: u32,
) {
    unsafe {
        (*(data as *mut State))
            .text_input_events
            .push(TextInputEvent::Done);
    }
}

pub(super) mod libc {
    #[repr(C)]
    pub struct pollfd {
        pub fd: i32,
        pub events: i16,
        pub revents: i16,
    }

    pub const POLLIN: i16 = 0x0001;

    unsafe extern "C" {
        pub fn close(fd: i32) -> i32;
        pub fn poll(fds: *mut pollfd, nfds: usize, timeout: i32) -> i32;
    }
}

pub(super) static REGISTRY_LISTENER: ffi::wl_registry_listener = ffi::wl_registry_listener {
    global: on_global,
    global_remove: on_global_remove,
};
pub(super) static WM_BASE_LISTENER: ffi::xdg_wm_base_listener =
    ffi::xdg_wm_base_listener { ping: on_ping };
pub(super) static XDG_SURFACE_LISTENER: ffi::xdg_surface_listener = ffi::xdg_surface_listener {
    configure: on_xdg_surface_configure,
};
pub(super) static TOPLEVEL_LISTENER: ffi::xdg_toplevel_listener = ffi::xdg_toplevel_listener {
    configure: on_toplevel_configure,
    close: on_toplevel_close,
};
pub(super) static SEAT_LISTENER: ffi::wl_seat_listener = ffi::wl_seat_listener {
    capabilities: on_seat_capabilities,
    name: on_seat_name,
};
pub(super) static OUTPUT_LISTENER: ffi::wl_output_listener = ffi::wl_output_listener {
    geometry: on_output_geometry,
    mode: on_output_mode,
    done: on_output_done,
    scale: on_output_scale,
};
pub(super) static SURFACE_LISTENER: ffi::wl_surface_listener = ffi::wl_surface_listener {
    enter: on_surface_enter,
    leave: on_surface_leave,
};
pub(super) static FRACTIONAL_SCALE_LISTENER: ffi::wp_fractional_scale_v1_listener =
    ffi::wp_fractional_scale_v1_listener {
        preferred_scale: on_preferred_scale,
    };
pub(super) static POINTER_LISTENER: ffi::wl_pointer_listener = ffi::wl_pointer_listener {
    enter: on_pointer_enter,
    leave: on_pointer_leave,
    motion: on_pointer_motion,
    button: on_pointer_button,
    axis: on_pointer_axis,
    frame: on_pointer_frame,
    axis_source: on_pointer_axis_source,
    axis_stop: on_pointer_axis_stop,
    axis_discrete: on_pointer_axis_discrete,
    axis_value120: on_pointer_axis_value120,
    axis_relative_direction: on_pointer_axis_relative_direction,
};
pub(super) static SWIPE_GESTURE_LISTENER: ffi::zwp_pointer_gesture_swipe_v1_listener =
    ffi::zwp_pointer_gesture_swipe_v1_listener {
        begin: on_swipe_begin,
        update: on_swipe_update,
        end: on_swipe_end,
    };
pub(super) static PINCH_GESTURE_LISTENER: ffi::zwp_pointer_gesture_pinch_v1_listener =
    ffi::zwp_pointer_gesture_pinch_v1_listener {
        begin: on_pinch_begin,
        update: on_pinch_update,
        end: on_pinch_end,
    };
pub(super) static HOLD_GESTURE_LISTENER: ffi::zwp_pointer_gesture_hold_v1_listener =
    ffi::zwp_pointer_gesture_hold_v1_listener {
        begin: on_hold_begin,
        end: on_hold_end,
    };
pub(super) static KEYBOARD_LISTENER: ffi::wl_keyboard_listener = ffi::wl_keyboard_listener {
    keymap: on_keyboard_keymap,
    enter: on_keyboard_enter,
    leave: on_keyboard_leave,
    key: on_keyboard_key,
    modifiers: on_keyboard_modifiers,
    repeat_info: on_keyboard_repeat_info,
};
pub(super) static TEXT_INPUT_LISTENER: ffi::zwp_text_input_v3_listener =
    ffi::zwp_text_input_v3_listener {
        enter: on_text_input_enter,
        leave: on_text_input_leave,
        preedit_string: on_text_input_preedit,
        commit_string: on_text_input_commit,
        delete_surrounding_text: on_text_input_delete,
        done: on_text_input_done,
    };
