//! Nested backend: ass as a client of a host Wayland session.
//!
//! Brings up an xdg-shell toplevel over raw libwayland-client and creates a
//! `VkSurfaceKHR` on flux's Vulkan instance via `ash`, so flux can present into
//! the host window. This is the development backend; a DRM/KMS backend replaces
//! it for bare-TTY operation.

mod ffi;

use std::ffi::{c_void, CStr, CString};
use std::ptr;

use ash::vk::Handle;
use ass_core::input::InputEvent;
use ass_core::Size;

use crate::Backend;

/// Vulkan instance extensions flux must enable for a nested Wayland surface.
pub const INSTANCE_EXTENSIONS: [&CStr; 2] = [c"VK_KHR_surface", c"VK_KHR_wayland_surface"];

/// Vulkan device extensions flux must enable to present (swapchain).
pub const DEVICE_EXTENSIONS: [&CStr; 1] = [c"VK_KHR_swapchain"];

/// Errors bringing up the nested backend.
#[derive(Debug, thiserror::Error)]
pub enum NestedError {
    /// Could not connect to the host Wayland display (`$WAYLAND_DISPLAY`).
    #[error("cannot connect to host Wayland display (is WAYLAND_DISPLAY set?)")]
    Connect,
    /// A required global was not advertised by the host.
    #[error("host does not advertise required global: {0}")]
    MissingGlobal(&'static str),
    /// A `wl_display_roundtrip` failed.
    #[error("wl_display_roundtrip failed")]
    Roundtrip,
    /// Vulkan surface creation failed.
    #[error("VkSurfaceKHR creation failed")]
    Vulkan,
}

/// Mutable state shared with the C event callbacks via a stable heap pointer.
struct State {
    compositor: *mut ffi::wl_proxy,
    wm_base: *mut ffi::wl_proxy,
    seat: *mut ffi::wl_proxy,
    pointer: *mut ffi::wl_proxy,
    keyboard: *mut ffi::wl_proxy,
    configured: bool,
    width: i32,
    height: i32,
    pending_width: i32,
    pending_height: i32,
    resized: bool,
    should_close: bool,
    /// Bound `wl_output` globals and the integer scale each last advertised.
    /// The nested window reads the scale of the output it currently sits on
    /// (`current_output`) to size its buffer for HiDPI.
    outputs: Vec<(*mut ffi::wl_proxy, i32)>,
    /// The output the surface most recently entered, or null before the first
    /// `wl_surface.enter`.
    current_output: *mut ffi::wl_proxy,
    /// Effective integer buffer scale (>= 1) for the current output.
    scale: i32,
    /// Set when `scale` changed (output scale event, or the window moved to a
    /// differently-scaled output); drained by `take_resize` so the main loop
    /// rebuilds the swapchain at the new physical size.
    scale_changed: bool,
    /// Input events drained by `take_input`. Pointer motion and button state
    /// changes accumulate here each dispatch; the main loop drains once per
    /// frame.
    input_events: Vec<InputEvent>,
}

/// A nested host window and its Vulkan surface.
pub struct NestedHost {
    display: *mut ffi::wl_display,
    registry: *mut ffi::wl_proxy,
    surface: *mut ffi::wl_proxy,
    xdg_surface: *mut ffi::wl_proxy,
    toplevel: *mut ffi::wl_proxy,
    // Boxed so the address handed to the C callbacks stays stable across moves.
    state: Box<State>,
    // Retained so the surface can be destroyed on drop. The `ash::Instance` is
    // `load`ed (not created) from flux's instance, so dropping it does not
    // destroy flux's instance.
    ash: Option<(ash::Entry, ash::Instance)>,
    vk_surface: u64,
}

// ----- request marshalling helpers ---------------------------------------

unsafe fn registry_bind(
    registry: *mut ffi::wl_proxy,
    name: u32,
    interface: &ffi::wl_interface,
    version: u32,
) -> *mut ffi::wl_proxy {
    ffi::wl_proxy_marshal_flags(
        registry,
        ffi::WL_REGISTRY_BIND,
        interface,
        version,
        0,
        name,
        interface.name,
        version,
        ptr::null::<c_void>(),
    )
}

unsafe fn seat_get_pointer(seat: *mut ffi::wl_proxy) -> *mut ffi::wl_proxy {
    ffi::wl_proxy_marshal_flags(
        seat,
        ffi::WL_SEAT_GET_POINTER,
        &ffi::wl_pointer_interface,
        ffi::wl_proxy_get_version(seat).min(4),
        0,
        ptr::null::<c_void>(),
    )
}

unsafe fn seat_get_keyboard(seat: *mut ffi::wl_proxy) -> *mut ffi::wl_proxy {
    ffi::wl_proxy_marshal_flags(
        seat,
        ffi::WL_SEAT_GET_KEYBOARD,
        &ffi::wl_keyboard_interface,
        ffi::wl_proxy_get_version(seat).min(4),
        0,
        ptr::null::<c_void>(),
    )
}

unsafe fn keyboard_release(keyboard: *mut ffi::wl_proxy) {
    ffi::wl_proxy_marshal_flags(
        keyboard,
        ffi::WL_KEYBOARD_RELEASE,
        ptr::null::<ffi::wl_interface>(),
        ffi::wl_proxy_get_version(keyboard),
        ffi::WL_MARSHAL_FLAG_DESTROY,
    );
}

unsafe fn pointer_release(pointer: *mut ffi::wl_proxy) {
    ffi::wl_proxy_marshal_flags(
        pointer,
        ffi::WL_POINTER_RELEASE,
        ptr::null::<ffi::wl_interface>(),
        ffi::wl_proxy_get_version(pointer),
        ffi::WL_MARSHAL_FLAG_DESTROY,
    );
}

unsafe fn create_surface(compositor: *mut ffi::wl_proxy) -> *mut ffi::wl_proxy {
    ffi::wl_proxy_marshal_flags(
        compositor,
        ffi::WL_COMPOSITOR_CREATE_SURFACE,
        &ffi::wl_surface_interface,
        ffi::wl_proxy_get_version(compositor),
        0,
        ptr::null::<c_void>(),
    )
}

unsafe fn get_xdg_surface(
    wm_base: *mut ffi::wl_proxy,
    surface: *mut ffi::wl_proxy,
) -> *mut ffi::wl_proxy {
    ffi::wl_proxy_marshal_flags(
        wm_base,
        ffi::XDG_WM_BASE_GET_XDG_SURFACE,
        &ffi::xdg_surface_interface,
        ffi::wl_proxy_get_version(wm_base),
        0,
        ptr::null::<c_void>(),
        surface,
    )
}

unsafe fn get_toplevel(xdg_surface: *mut ffi::wl_proxy) -> *mut ffi::wl_proxy {
    ffi::wl_proxy_marshal_flags(
        xdg_surface,
        ffi::XDG_SURFACE_GET_TOPLEVEL,
        &ffi::xdg_toplevel_interface,
        ffi::wl_proxy_get_version(xdg_surface),
        0,
        ptr::null::<c_void>(),
    )
}

unsafe fn ack_configure(xdg_surface: *mut ffi::wl_proxy, serial: u32) {
    ffi::wl_proxy_marshal_flags(
        xdg_surface,
        ffi::XDG_SURFACE_ACK_CONFIGURE,
        ptr::null::<ffi::wl_interface>(),
        ffi::wl_proxy_get_version(xdg_surface),
        0,
        serial,
    );
}

unsafe fn pong(wm_base: *mut ffi::wl_proxy, serial: u32) {
    ffi::wl_proxy_marshal_flags(
        wm_base,
        ffi::XDG_WM_BASE_PONG,
        ptr::null::<ffi::wl_interface>(),
        ffi::wl_proxy_get_version(wm_base),
        0,
        serial,
    );
}

unsafe fn commit(surface: *mut ffi::wl_proxy) {
    ffi::wl_proxy_marshal_flags(
        surface,
        ffi::WL_SURFACE_COMMIT,
        ptr::null::<ffi::wl_interface>(),
        ffi::wl_proxy_get_version(surface),
        0,
    );
}

/// `wl_surface.set_buffer_scale`: declare the attached buffer is pre-scaled by
/// `scale`, so the host maps it 1:1 instead of upscaling a logical-sized
/// buffer. Double-buffered; applies on the next commit (the next present).
unsafe fn set_buffer_scale(surface: *mut ffi::wl_proxy, scale: i32) {
    ffi::wl_proxy_marshal_flags(
        surface,
        ffi::WL_SURFACE_SET_BUFFER_SCALE,
        ptr::null::<ffi::wl_interface>(),
        ffi::wl_proxy_get_version(surface),
        0,
        scale.max(1),
    );
}

unsafe fn set_string(toplevel: *mut ffi::wl_proxy, opcode: u32, value: &str) {
    let c = CString::new(value).unwrap_or_default();
    ffi::wl_proxy_marshal_flags(
        toplevel,
        opcode,
        ptr::null::<ffi::wl_interface>(),
        ffi::wl_proxy_get_version(toplevel),
        0,
        c.as_ptr(),
    );
}

// ----- event callbacks ----------------------------------------------------

unsafe extern "C" fn on_global(
    data: *mut c_void,
    registry: *mut ffi::wl_proxy,
    name: u32,
    interface: *const std::os::raw::c_char,
    version: u32,
) {
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
        // Bind at v4: pointer v4 covers enter/leave/motion/button/axis without
        // the v5+ frame/axis_value120 group — sufficient for M1 forwarding.
        st.seat = registry_bind(registry, name, &ffi::wl_seat_interface, version.min(4));
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
    }
}

unsafe extern "C" fn on_global_remove(
    _data: *mut c_void,
    _registry: *mut ffi::wl_proxy,
    _name: u32,
) {
}

unsafe extern "C" fn on_ping(_data: *mut c_void, wm_base: *mut ffi::wl_proxy, serial: u32) {
    pong(wm_base, serial);
}

unsafe extern "C" fn on_xdg_surface_configure(
    data: *mut c_void,
    xdg_surface: *mut ffi::wl_proxy,
    serial: u32,
) {
    let st = &mut *(data as *mut State);
    ack_configure(xdg_surface, serial);
    st.configured = true;
}

unsafe extern "C" fn on_toplevel_configure(
    data: *mut c_void,
    _toplevel: *mut ffi::wl_proxy,
    width: std::os::raw::c_int,
    height: std::os::raw::c_int,
    _states: *mut ffi::wl_array,
) {
    let st = &mut *(data as *mut State);
    if width > 0 && height > 0 && (width != st.width || height != st.height) {
        st.pending_width = width;
        st.pending_height = height;
        st.resized = true;
    }
}

unsafe extern "C" fn on_toplevel_close(data: *mut c_void, _toplevel: *mut ffi::wl_proxy) {
    (*(data as *mut State)).should_close = true;
}

/// Host seat capabilities changed. When the host gains a pointer or keyboard
/// we bind it and install listeners; the host then sends input events. Losing
/// capability pushes a synthetic leave so the focus model clears cleanly.
unsafe extern "C" fn on_seat_capabilities(data: *mut c_void, seat: *mut ffi::wl_proxy, caps: u32) {
    let st = &mut *(data as *mut State);
    let want_pointer = (caps & ffi::WL_SEAT_CAPABILITY_POINTER) != 0;
    let want_keyboard = (caps & ffi::WL_SEAT_CAPABILITY_KEYBOARD) != 0;
    if want_pointer && st.pointer.is_null() {
        let p = seat_get_pointer(seat);
        if !p.is_null() {
            ffi::wl_proxy_add_listener(p, &POINTER_LISTENER as *const _ as *const c_void, data);
            st.pointer = p;
            log::debug!("nested: host pointer bound");
        }
    } else if !want_pointer && !st.pointer.is_null() {
        pointer_release(st.pointer);
        st.pointer = std::ptr::null_mut();
        st.input_events.push(InputEvent::PointerLeave);
    }
    if want_keyboard && st.keyboard.is_null() {
        let k = seat_get_keyboard(seat);
        if !k.is_null() {
            ffi::wl_proxy_add_listener(k, &KEYBOARD_LISTENER as *const _ as *const c_void, data);
            st.keyboard = k;
            log::debug!("nested: host keyboard bound");
        }
    } else if !want_keyboard && !st.keyboard.is_null() {
        keyboard_release(st.keyboard);
        st.keyboard = std::ptr::null_mut();
    }
}

unsafe extern "C" fn on_seat_name(
    _data: *mut c_void,
    _seat: *mut ffi::wl_proxy,
    _name: *const std::os::raw::c_char,
) {
}

// Host output listeners. We only care about `scale`: it drives the buffer
// scale so a HiDPI host maps our pre-scaled buffer 1:1 instead of upscaling a
// logical-sized one. `geometry`/`mode`/`done` are accepted (the vtable must be
// complete) but ignored.
unsafe extern "C" fn on_output_geometry(
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

unsafe extern "C" fn on_output_mode(
    _data: *mut c_void,
    _output: *mut ffi::wl_proxy,
    _flags: u32,
    _width: i32,
    _height: i32,
    _refresh: i32,
) {
}

unsafe extern "C" fn on_output_done(_data: *mut c_void, _output: *mut ffi::wl_proxy) {}

unsafe extern "C" fn on_output_scale(
    data: *mut c_void,
    output: *mut ffi::wl_proxy,
    factor: i32,
) {
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
        st.scale_changed = true;
    }
}

// Host surface listeners. `enter` tells us which output the window sits on; we
// adopt that output's scale. `leave` keeps the last known scale.
unsafe extern "C" fn on_surface_enter(
    data: *mut c_void,
    _surface: *mut ffi::wl_proxy,
    output: *mut ffi::wl_proxy,
) {
    let st = &mut *(data as *mut State);
    st.current_output = output;
    if let Some(entry) = st.outputs.iter().find(|o| o.0 == output) {
        if entry.1 != st.scale {
            st.scale = entry.1;
            st.scale_changed = true;
        }
    }
}

unsafe extern "C" fn on_surface_leave(
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
unsafe extern "C" fn on_pointer_enter(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    _serial: u32,
    _surface: *mut ffi::wl_proxy,
    x: i32,
    y: i32,
) {
    let st = &mut *(data as *mut State);
    st.input_events.push(InputEvent::PointerMotion {
        x: ffi::wl_fixed_to_f32(x),
        y: ffi::wl_fixed_to_f32(y),
    });
}

unsafe extern "C" fn on_pointer_leave(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    _serial: u32,
    _surface: *mut ffi::wl_proxy,
) {
    let st = &mut *(data as *mut State);
    st.input_events.push(InputEvent::PointerLeave);
}

unsafe extern "C" fn on_pointer_motion(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    _time: u32,
    x: i32,
    y: i32,
) {
    let st = &mut *(data as *mut State);
    st.input_events.push(InputEvent::PointerMotion {
        x: ffi::wl_fixed_to_f32(x),
        y: ffi::wl_fixed_to_f32(y),
    });
}

unsafe extern "C" fn on_pointer_button(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    _serial: u32,
    _time: u32,
    button: u32,
    state: u32,
) {
    let st = &mut *(data as *mut State);
    // Host button codes are Linux input-event BTN_* codes already
    // (BTN_LEFT=0x110, BTN_RIGHT=0x111, BTN_MIDDLE=0x112). Pass through.
    st.input_events.push(InputEvent::PointerButton {
        button,
        state: ass_core::input::ButtonState::from_wayland(state),
    });
}

unsafe extern "C" fn on_pointer_axis(
    _data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    _time: u32,
    _axis: u32,
    _value: i32,
) {
    // Scroll wheel handling deferred — not on the M1 critical path.
}

// Host keyboard listeners. The keymap event from the host is consumed only to
// close the file descriptor the host sent us; ass runs its own xkbcommon
// compile and ships that keymap to clients (the host's keymap and ours match
// in practice since both use the default evdev/pc104/us RMLVO, but we do not
// depend on it). Key events are forwarded as `InputEvent::Key` with their
// raw evdev scancode; the server side has its own xkbcommon state for
// modifier tracking.
unsafe extern "C" fn on_keyboard_keymap(
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

unsafe extern "C" fn on_keyboard_enter(
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

unsafe extern "C" fn on_keyboard_leave(
    _data: *mut c_void,
    _keyboard: *mut ffi::wl_proxy,
    _serial: u32,
    _surface: *mut ffi::wl_proxy,
) {
}

unsafe extern "C" fn on_keyboard_key(
    data: *mut c_void,
    _keyboard: *mut ffi::wl_proxy,
    _serial: u32,
    _time: u32,
    key: u32,
    state: u32,
) {
    let st = &mut *(data as *mut State);
    // The host delivers evdev scancodes (offset 8 from xkbcommon keycodes,
    // but the Wayland `wl_keyboard.key` event uses evdev scancodes directly,
    // so no shift needed for forwarding).
    st.input_events.push(InputEvent::Key {
        code: key,
        state: ass_core::input::ButtonState::from_wayland(state),
    });
}

unsafe extern "C" fn on_keyboard_modifiers(
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

unsafe extern "C" fn on_keyboard_repeat_info(
    _data: *mut c_void,
    _keyboard: *mut ffi::wl_proxy,
    _rate: i32,
    _delay: i32,
) {
    // We advertise our own repeat_info (25/250) on client bind; the host's
    // rate is ignored.
}

mod libc {
    extern "C" {
        pub fn close(fd: i32) -> i32;
    }
}

static REGISTRY_LISTENER: ffi::wl_registry_listener = ffi::wl_registry_listener {
    global: on_global,
    global_remove: on_global_remove,
};
static WM_BASE_LISTENER: ffi::xdg_wm_base_listener = ffi::xdg_wm_base_listener { ping: on_ping };
static XDG_SURFACE_LISTENER: ffi::xdg_surface_listener = ffi::xdg_surface_listener {
    configure: on_xdg_surface_configure,
};
static TOPLEVEL_LISTENER: ffi::xdg_toplevel_listener = ffi::xdg_toplevel_listener {
    configure: on_toplevel_configure,
    close: on_toplevel_close,
};
static SEAT_LISTENER: ffi::wl_seat_listener = ffi::wl_seat_listener {
    capabilities: on_seat_capabilities,
    name: on_seat_name,
};
static OUTPUT_LISTENER: ffi::wl_output_listener = ffi::wl_output_listener {
    geometry: on_output_geometry,
    mode: on_output_mode,
    done: on_output_done,
    scale: on_output_scale,
};
static SURFACE_LISTENER: ffi::wl_surface_listener = ffi::wl_surface_listener {
    enter: on_surface_enter,
    leave: on_surface_leave,
};
static POINTER_LISTENER: ffi::wl_pointer_listener = ffi::wl_pointer_listener {
    enter: on_pointer_enter,
    leave: on_pointer_leave,
    motion: on_pointer_motion,
    button: on_pointer_button,
    axis: on_pointer_axis,
};
static KEYBOARD_LISTENER: ffi::wl_keyboard_listener = ffi::wl_keyboard_listener {
    keymap: on_keyboard_keymap,
    enter: on_keyboard_enter,
    leave: on_keyboard_leave,
    key: on_keyboard_key,
    modifiers: on_keyboard_modifiers,
    repeat_info: on_keyboard_repeat_info,
};

impl NestedHost {
    /// Open a nested toplevel of the given initial size and title.
    pub fn open(title: &str, width: i32, height: i32) -> Result<NestedHost, NestedError> {
        unsafe {
            let display = ffi::wl_display_connect(ptr::null());
            if display.is_null() {
                return Err(NestedError::Connect);
            }

            let mut state = Box::new(State {
                compositor: ptr::null_mut(),
                wm_base: ptr::null_mut(),
                seat: ptr::null_mut(),
                pointer: ptr::null_mut(),
                keyboard: ptr::null_mut(),
                configured: false,
                width,
                height,
                pending_width: 0,
                pending_height: 0,
                resized: false,
                should_close: false,
                outputs: Vec::new(),
                current_output: ptr::null_mut(),
                scale: 1,
                scale_changed: false,
                input_events: Vec::new(),
            });
            let data = &mut *state as *mut State as *mut c_void;

            // Registry → bind globals.
            let registry = ffi::wl_proxy_marshal_flags(
                display as *mut ffi::wl_proxy,
                ffi::WL_DISPLAY_GET_REGISTRY,
                &ffi::wl_registry_interface,
                ffi::wl_proxy_get_version(display as *mut ffi::wl_proxy),
                0,
                ptr::null::<c_void>(),
            );
            ffi::wl_proxy_add_listener(
                registry,
                &REGISTRY_LISTENER as *const _ as *const c_void,
                data,
            );
            ffi::wl_display_roundtrip(display);

            if state.compositor.is_null() {
                return Err(NestedError::MissingGlobal("wl_compositor"));
            }
            if state.wm_base.is_null() {
                return Err(NestedError::MissingGlobal("xdg_wm_base"));
            }
            ffi::wl_proxy_add_listener(
                state.wm_base,
                &WM_BASE_LISTENER as *const _ as *const c_void,
                data,
            );
            // Install the host seat listener if the registry bound one. The
            // capabilities event arrives on the roundtrips below, at which
            // point we create the host pointer proxy.
            if !state.seat.is_null() {
                ffi::wl_proxy_add_listener(
                    state.seat,
                    &SEAT_LISTENER as *const _ as *const c_void,
                    data,
                );
            }

            // Surface + xdg roles. Listen for enter/leave so the buffer scale
            // tracks the output the window is shown on.
            let surface = create_surface(state.compositor);
            ffi::wl_proxy_add_listener(
                surface,
                &SURFACE_LISTENER as *const _ as *const c_void,
                data,
            );
            let xdg_surface = get_xdg_surface(state.wm_base, surface);
            ffi::wl_proxy_add_listener(
                xdg_surface,
                &XDG_SURFACE_LISTENER as *const _ as *const c_void,
                data,
            );
            let toplevel = get_toplevel(xdg_surface);
            ffi::wl_proxy_add_listener(
                toplevel,
                &TOPLEVEL_LISTENER as *const _ as *const c_void,
                data,
            );
            set_string(toplevel, ffi::XDG_TOPLEVEL_SET_TITLE, title);
            set_string(toplevel, ffi::XDG_TOPLEVEL_SET_APP_ID, "ass");

            // Initial buffer-less commit to provoke the first configure, then
            // wait for it (Vulkan WSI provides the buffer on first present).
            // `state.configured` is flipped by the `on_xdg_surface_configure`
            // C callback during `wl_display_roundtrip`; clippy cannot see that
            // mutation across the FFI boundary, so the immutability check is a
            // false positive here.
            commit(surface);
            #[allow(clippy::while_immutable_condition)]
            while !state.configured {
                if ffi::wl_display_roundtrip(display) < 0 {
                    return Err(NestedError::Roundtrip);
                }
            }
            if state.resized {
                state.width = state.pending_width;
                state.height = state.pending_height;
                state.resized = false;
            }

            Ok(NestedHost {
                display,
                registry,
                surface,
                xdg_surface,
                toplevel,
                state,
                ash: None,
                vk_surface: 0,
            })
        }
    }

    /// Create a `VkSurfaceKHR` on `device`'s instance for this window. Returns
    /// the raw handle as a `*mut c_void` suitable for `flux::Surface::from_vk`.
    pub fn create_vk_surface(&mut self, device: &flux::Device) -> Result<*mut c_void, NestedError> {
        unsafe {
            let entry = ash::Entry::load().map_err(|_| NestedError::Vulkan)?;
            let raw_instance = device.vk_instance() as usize as u64;
            let instance =
                ash::Instance::load(entry.static_fn(), ash::vk::Instance::from_raw(raw_instance));

            let wl = ash::khr::wayland_surface::Instance::new(&entry, &instance);
            let info = ash::vk::WaylandSurfaceCreateInfoKHR::default()
                .display(self.display as *mut _)
                .surface(self.surface as *mut _);
            let surface = wl
                .create_wayland_surface(&info, None)
                .map_err(|_| NestedError::Vulkan)?;

            let raw = surface.as_raw();
            self.ash = Some((entry, instance));
            self.vk_surface = raw;
            Ok(raw as usize as *mut c_void)
        }
    }

    /// Logical window size as `u32` (the configured size, scale-independent).
    /// Use [`physical_size`](Self::physical_size) for swapchain extents.
    pub fn size_u32(&self) -> (u32, u32) {
        (
            self.state.width.max(1) as u32,
            self.state.height.max(1) as u32,
        )
    }

    /// Integer buffer scale of the output the window is on (>= 1). 1 on a
    /// non-HiDPI host or before the first output `scale` / surface `enter`.
    pub fn scale(&self) -> i32 {
        self.state.scale.max(1)
    }

    /// Physical (device-pixel) size = logical size × [`scale`](Self::scale).
    /// This is the swapchain extent; the buffer is divisible by `scale`, so
    /// `wl_surface.set_buffer_scale(scale)` is always valid.
    pub fn physical_size(&self) -> (u32, u32) {
        let s = self.scale();
        (
            (self.state.width.max(1) * s) as u32,
            (self.state.height.max(1) * s) as u32,
        )
    }

    /// Advertise the current buffer scale to the host. Applies on the next
    /// surface commit (the next present); call after sizing the swapchain to
    /// the matching physical size.
    pub fn set_buffer_scale(&self) {
        unsafe { set_buffer_scale(self.surface, self.state.scale) };
    }

    pub fn should_close(&self) -> bool {
        self.state.should_close
    }
}

impl Backend for NestedHost {
    fn size(&self) -> Size {
        Size {
            w: self.state.width,
            h: self.state.height,
        }
    }

    fn dispatch(&mut self) -> bool {
        unsafe {
            if ffi::wl_display_roundtrip(self.display) < 0 {
                self.state.should_close = true;
            }
        }
        !self.state.should_close
    }

    /// Drain input events buffered since the last call. Empty until the host
    /// seat advertises pointer capability and the host starts sending events.
    fn take_input(&mut self) -> Vec<ass_core::input::InputEvent> {
        std::mem::take(&mut self.state.input_events)
    }

    /// Reports a new *logical* size when the host resized us or the output
    /// scale changed (the window moved to a differently-scaled monitor). The
    /// main loop derives the physical swapchain extent from
    /// [`physical_size`](NestedHost::physical_size); on a pure scale change the
    /// logical size is unchanged but the physical size is not.
    fn take_resize(&mut self) -> Option<Size> {
        if self.state.resized || self.state.scale_changed {
            if self.state.resized {
                self.state.width = self.state.pending_width;
                self.state.height = self.state.pending_height;
            }
            self.state.resized = false;
            self.state.scale_changed = false;
            Some(Size {
                w: self.state.width,
                h: self.state.height,
            })
        } else {
            None
        }
    }
}

impl Drop for NestedHost {
    fn drop(&mut self) {
        unsafe {
            if let Some((entry, instance)) = &self.ash {
                if self.vk_surface != 0 {
                    let surf = ash::khr::surface::Instance::new(entry, instance);
                    surf.destroy_surface(ash::vk::SurfaceKHR::from_raw(self.vk_surface), None);
                }
            }
            // Children before parents, display last. The pointer and keyboard
            // (if bound) are children of the seat; release them first. The
            // `wl_compositor` proxy is stored only on `state` and has no
            // destructor request, but destroying it explicitly keeps the
            // struct's teardown contract complete instead of relying on
            // disconnect to reap it.
            if !self.state.pointer.is_null() {
                ffi::wl_proxy_destroy(self.state.pointer);
            }
            if !self.state.keyboard.is_null() {
                ffi::wl_proxy_destroy(self.state.keyboard);
            }
            // Bound outputs are independent globals; reap them before the
            // registry.
            for (output, _) in self.state.outputs.iter() {
                if !output.is_null() {
                    ffi::wl_proxy_destroy(*output);
                }
            }
            let compositor = self.state.compositor;
            let seat = self.state.seat;
            for p in [
                self.toplevel,
                self.xdg_surface,
                self.surface,
                self.wm_base_ptr(),
                compositor,
                seat,
                self.registry,
            ] {
                if !p.is_null() {
                    ffi::wl_proxy_destroy(p);
                }
            }
            ffi::wl_display_disconnect(self.display);
        }
    }
}

impl NestedHost {
    fn wm_base_ptr(&self) -> *mut ffi::wl_proxy {
        self.state.wm_base
    }
}
