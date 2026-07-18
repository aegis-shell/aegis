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
use ass_core::input::{
    InputEvent, PointerAxis, PointerAxisFrame, PointerAxisRelativeDirection, PointerAxisSource,
    PointerGestureEvent, TextInputEvent, TextInputState,
};
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
    viewporter: *mut ffi::wl_proxy,
    fractional_scale_manager: *mut ffi::wl_proxy,
    cursor_shape_manager: *mut ffi::wl_proxy,
    cursor_shape_device: *mut ffi::wl_proxy,
    pointer_gestures_manager: *mut ffi::wl_proxy,
    gesture_swipe: *mut ffi::wl_proxy,
    gesture_pinch: *mut ffi::wl_proxy,
    gesture_hold: *mut ffi::wl_proxy,
    text_input_manager: *mut ffi::wl_proxy,
    text_input: *mut ffi::wl_proxy,
    seat: *mut ffi::wl_proxy,
    pointer: *mut ffi::wl_proxy,
    keyboard: *mut ffi::wl_proxy,
    last_pointer_serial: u32,
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
    /// Preferred surface scale in 120ths, supplied by
    /// `wp_fractional_scale_v1`. Used only while `fractional_active` is true.
    preferred_scale_120: u32,
    /// True once both a fractional-scale object and viewport have been
    /// created for the host surface.
    fractional_active: bool,
    /// Set when `scale` changed (output scale event, or the window moved to a
    /// differently-scaled output); drained by `take_resize` so the main loop
    /// rebuilds the swapchain at the new physical size.
    scale_changed: bool,
    /// Input events drained by `take_input`. Pointer motion and button state
    /// changes accumulate here each dispatch; the main loop drains once per
    /// frame.
    input_events: Vec<InputEvent>,
    /// Axis callbacks accumulated until the host's `wl_pointer.frame`.
    pending_pointer_axis: PointerAxisFrame,
    pointer_gesture_events: Vec<PointerGestureEvent>,
    text_input_events: Vec<TextInputEvent>,
    text_input_entered: bool,
    text_input_state: TextInputState,
}

/// A nested host window and its Vulkan surface.
pub struct NestedHost {
    display: *mut ffi::wl_display,
    registry: *mut ffi::wl_proxy,
    surface: *mut ffi::wl_proxy,
    xdg_surface: *mut ffi::wl_proxy,
    toplevel: *mut ffi::wl_proxy,
    viewport: *mut ffi::wl_proxy,
    fractional_scale: *mut ffi::wl_proxy,
    // Boxed so the address handed to the C callbacks stays stable across moves.
    state: Box<State>,
    // Retained so the surface can be destroyed on drop. The `ash::Instance` is
    // `load`ed (not created) from flux's instance, so dropping it does not
    // destroy flux's instance.
    ash: Option<(ash::Entry, ash::Instance)>,
    vk_surface: u64,
    /// Persisted profile for direct-display sessions. The outer compositor
    /// owns the physical device while this backend is nested.
    touchpad_config: ass_core::input::TouchpadConfig,
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
        ffi::wl_proxy_get_version(seat).min(9),
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

unsafe fn pointer_hide_cursor(pointer: *mut ffi::wl_proxy, serial: u32) {
    ffi::wl_proxy_marshal_flags(
        pointer,
        ffi::WL_POINTER_SET_CURSOR,
        ptr::null::<ffi::wl_interface>(),
        ffi::wl_proxy_get_version(pointer),
        0,
        serial,
        ptr::null_mut::<ffi::wl_proxy>(),
        0i32,
        0i32,
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

unsafe fn get_viewport(
    viewporter: *mut ffi::wl_proxy,
    surface: *mut ffi::wl_proxy,
) -> *mut ffi::wl_proxy {
    ffi::wl_proxy_marshal_flags(
        viewporter,
        ffi::WP_VIEWPORTER_GET_VIEWPORT,
        &ffi::wp_viewport_interface,
        ffi::wl_proxy_get_version(viewporter),
        0,
        ptr::null::<c_void>(),
        surface,
    )
}

unsafe fn viewport_set_destination(viewport: *mut ffi::wl_proxy, width: i32, height: i32) {
    ffi::wl_proxy_marshal_flags(
        viewport,
        ffi::WP_VIEWPORT_SET_DESTINATION,
        ptr::null::<ffi::wl_interface>(),
        ffi::wl_proxy_get_version(viewport),
        0,
        width.max(1),
        height.max(1),
    );
}

unsafe fn get_fractional_scale(
    manager: *mut ffi::wl_proxy,
    surface: *mut ffi::wl_proxy,
) -> *mut ffi::wl_proxy {
    ffi::wl_proxy_marshal_flags(
        manager,
        ffi::WP_FRACTIONAL_SCALE_MANAGER_V1_GET_FRACTIONAL_SCALE,
        &ffi::wp_fractional_scale_v1_interface,
        ffi::wl_proxy_get_version(manager),
        0,
        ptr::null::<c_void>(),
        surface,
    )
}

unsafe fn get_cursor_shape_device(
    manager: *mut ffi::wl_proxy,
    pointer: *mut ffi::wl_proxy,
) -> *mut ffi::wl_proxy {
    ffi::wl_proxy_marshal_flags(
        manager,
        ffi::WP_CURSOR_SHAPE_MANAGER_V1_GET_POINTER,
        &ffi::wp_cursor_shape_device_v1_interface,
        ffi::wl_proxy_get_version(manager),
        0,
        ptr::null::<c_void>(),
        pointer,
    )
}

unsafe fn cursor_shape_set_shape(device: *mut ffi::wl_proxy, serial: u32, shape: u32) {
    ffi::wl_proxy_marshal_flags(
        device,
        ffi::WP_CURSOR_SHAPE_DEVICE_V1_SET_SHAPE,
        ptr::null::<ffi::wl_interface>(),
        ffi::wl_proxy_get_version(device),
        0,
        serial,
        shape.max(1),
    );
}

unsafe fn get_pointer_gesture(
    manager: *mut ffi::wl_proxy,
    opcode: u32,
    interface: &ffi::wl_interface,
    pointer: *mut ffi::wl_proxy,
) -> *mut ffi::wl_proxy {
    ffi::wl_proxy_marshal_flags(
        manager,
        opcode,
        interface,
        1,
        0,
        ptr::null::<c_void>(),
        pointer,
    )
}

unsafe fn destroy_pointer_gesture(gesture: *mut ffi::wl_proxy) {
    if !gesture.is_null() {
        ffi::wl_proxy_marshal_flags(
            gesture,
            0,
            ptr::null::<ffi::wl_interface>(),
            ffi::wl_proxy_get_version(gesture),
            ffi::WL_MARSHAL_FLAG_DESTROY,
        );
    }
}

unsafe fn destroy_pointer_gestures(st: *mut State) {
    destroy_pointer_gesture((*st).gesture_hold);
    destroy_pointer_gesture((*st).gesture_pinch);
    destroy_pointer_gesture((*st).gesture_swipe);
    (*st).gesture_hold = ptr::null_mut();
    (*st).gesture_pinch = ptr::null_mut();
    (*st).gesture_swipe = ptr::null_mut();
}

unsafe fn ensure_pointer_gestures(st: *mut State, data: *mut c_void) {
    if st.is_null()
        || (*st).pointer.is_null()
        || (*st).pointer_gestures_manager.is_null()
        || !(*st).gesture_swipe.is_null()
    {
        return;
    }
    let manager = (*st).pointer_gestures_manager;
    let pointer = (*st).pointer;
    (*st).gesture_swipe = get_pointer_gesture(
        manager,
        ffi::ZWP_POINTER_GESTURES_V1_GET_SWIPE_GESTURE,
        &ffi::zwp_pointer_gesture_swipe_v1_interface,
        pointer,
    );
    if !(*st).gesture_swipe.is_null() {
        ffi::wl_proxy_add_listener(
            (*st).gesture_swipe,
            &SWIPE_GESTURE_LISTENER as *const _ as *const c_void,
            data,
        );
    }
    if ffi::wl_proxy_get_version(manager) >= 2 {
        (*st).gesture_pinch = get_pointer_gesture(
            manager,
            ffi::ZWP_POINTER_GESTURES_V1_GET_PINCH_GESTURE,
            &ffi::zwp_pointer_gesture_pinch_v1_interface,
            pointer,
        );
        if !(*st).gesture_pinch.is_null() {
            ffi::wl_proxy_add_listener(
                (*st).gesture_pinch,
                &PINCH_GESTURE_LISTENER as *const _ as *const c_void,
                data,
            );
        }
    }
    if ffi::wl_proxy_get_version(manager) >= 3 {
        (*st).gesture_hold = get_pointer_gesture(
            manager,
            ffi::ZWP_POINTER_GESTURES_V1_GET_HOLD_GESTURE,
            &ffi::zwp_pointer_gesture_hold_v1_interface,
            pointer,
        );
        if !(*st).gesture_hold.is_null() {
            ffi::wl_proxy_add_listener(
                (*st).gesture_hold,
                &HOLD_GESTURE_LISTENER as *const _ as *const c_void,
                data,
            );
        }
    }
}

unsafe fn get_text_input(
    manager: *mut ffi::wl_proxy,
    seat: *mut ffi::wl_proxy,
) -> *mut ffi::wl_proxy {
    ffi::wl_proxy_marshal_flags(
        manager,
        ffi::ZWP_TEXT_INPUT_MANAGER_V3_GET_TEXT_INPUT,
        &ffi::zwp_text_input_v3_interface,
        ffi::wl_proxy_get_version(manager).min(1),
        0,
        ptr::null::<c_void>(),
        seat,
    )
}

unsafe fn send_text_input_state(st: *mut State) {
    if st.is_null() || (*st).text_input.is_null() || !(*st).text_input_entered {
        return;
    }
    let text_input = (*st).text_input;
    let state = (*st).text_input_state.clone();
    let opcode = if state.enabled {
        ffi::ZWP_TEXT_INPUT_V3_ENABLE
    } else {
        ffi::ZWP_TEXT_INPUT_V3_DISABLE
    };
    ffi::wl_proxy_marshal_flags(
        text_input,
        opcode,
        ptr::null::<ffi::wl_interface>(),
        ffi::wl_proxy_get_version(text_input),
        0,
    );
    if state.enabled {
        if let Some(text) = state.surrounding_text {
            if let Ok(text) = CString::new(text) {
                ffi::wl_proxy_marshal_flags(
                    text_input,
                    ffi::ZWP_TEXT_INPUT_V3_SET_SURROUNDING_TEXT,
                    ptr::null::<ffi::wl_interface>(),
                    ffi::wl_proxy_get_version(text_input),
                    0,
                    text.as_ptr(),
                    state.cursor,
                    state.anchor,
                );
            }
        }
        ffi::wl_proxy_marshal_flags(
            text_input,
            ffi::ZWP_TEXT_INPUT_V3_SET_TEXT_CHANGE_CAUSE,
            ptr::null::<ffi::wl_interface>(),
            ffi::wl_proxy_get_version(text_input),
            0,
            state.change_cause,
        );
        ffi::wl_proxy_marshal_flags(
            text_input,
            ffi::ZWP_TEXT_INPUT_V3_SET_CONTENT_TYPE,
            ptr::null::<ffi::wl_interface>(),
            ffi::wl_proxy_get_version(text_input),
            0,
            state.content_hint,
            state.content_purpose,
        );
        if let Some((x, y, width, height)) = state.cursor_rect {
            ffi::wl_proxy_marshal_flags(
                text_input,
                ffi::ZWP_TEXT_INPUT_V3_SET_CURSOR_RECTANGLE,
                ptr::null::<ffi::wl_interface>(),
                ffi::wl_proxy_get_version(text_input),
                0,
                x,
                y,
                width,
                height,
            );
        }
    }
    ffi::wl_proxy_marshal_flags(
        text_input,
        ffi::ZWP_TEXT_INPUT_V3_COMMIT,
        ptr::null::<ffi::wl_interface>(),
        ffi::wl_proxy_get_version(text_input),
        0,
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
            ffi::wl_proxy_add_listener(output, &OUTPUT_LISTENER as *const _ as *const c_void, data);
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

unsafe extern "C" fn on_output_scale(data: *mut c_void, output: *mut ffi::wl_proxy, factor: i32) {
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

unsafe extern "C" fn on_preferred_scale(
    data: *mut c_void,
    _fractional_scale: *mut ffi::wl_proxy,
    scale_120: u32,
) {
    let st = &mut *(data as *mut State);
    let scale_120 = scale_120.max(1);
    if scale_120 != st.preferred_scale_120 {
        st.preferred_scale_120 = scale_120;
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
    serial: u32,
    _surface: *mut ffi::wl_proxy,
    x: i32,
    y: i32,
) {
    let st = &mut *(data as *mut State);
    st.last_pointer_serial = serial;
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
    serial: u32,
    _time: u32,
    button: u32,
    state: u32,
) {
    let st = &mut *(data as *mut State);
    st.last_pointer_serial = serial;
    // Host button codes are Linux input-event BTN_* codes already
    // (BTN_LEFT=0x110, BTN_RIGHT=0x111, BTN_MIDDLE=0x112). Pass through.
    st.input_events.push(InputEvent::PointerButton {
        button,
        state: ass_core::input::ButtonState::from_wayland(state),
    });
}

unsafe extern "C" fn on_pointer_axis(
    data: *mut c_void,
    pointer: *mut ffi::wl_proxy,
    time: u32,
    axis: u32,
    value: i32,
) {
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

fn pointer_axis_slot(frame: &mut PointerAxisFrame, axis: u32) -> Option<&mut PointerAxis> {
    match axis {
        0 => Some(&mut frame.vertical),
        1 => Some(&mut frame.horizontal),
        _ => None,
    }
}

fn flush_pointer_axis(st: &mut State) {
    let frame = std::mem::take(&mut st.pending_pointer_axis);
    if frame.has_data() {
        st.input_events.push(InputEvent::PointerAxis(frame));
    }
}

unsafe extern "C" fn on_pointer_frame(data: *mut c_void, _pointer: *mut ffi::wl_proxy) {
    flush_pointer_axis(&mut *(data as *mut State));
}

unsafe extern "C" fn on_pointer_axis_source(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    source: u32,
) {
    (*(data as *mut State)).pending_pointer_axis.source = match source {
        0 => Some(PointerAxisSource::Wheel),
        1 => Some(PointerAxisSource::Finger),
        2 => Some(PointerAxisSource::Continuous),
        3 => Some(PointerAxisSource::WheelTilt),
        _ => None,
    };
}

unsafe extern "C" fn on_pointer_axis_stop(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    time: u32,
    axis: u32,
) {
    let st = &mut *(data as *mut State);
    st.pending_pointer_axis.time = time;
    if let Some(slot) = pointer_axis_slot(&mut st.pending_pointer_axis, axis) {
        slot.stop = true;
    }
}

unsafe extern "C" fn on_pointer_axis_discrete(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    axis: u32,
    discrete: i32,
) {
    let st = &mut *(data as *mut State);
    if let Some(slot) = pointer_axis_slot(&mut st.pending_pointer_axis, axis) {
        slot.discrete = Some(slot.discrete.unwrap_or(0) + discrete);
    }
}

unsafe extern "C" fn on_pointer_axis_value120(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    axis: u32,
    value120: i32,
) {
    let st = &mut *(data as *mut State);
    if let Some(slot) = pointer_axis_slot(&mut st.pending_pointer_axis, axis) {
        slot.value120 = Some(slot.value120.unwrap_or(0) + value120);
    }
}

unsafe extern "C" fn on_pointer_axis_relative_direction(
    data: *mut c_void,
    _pointer: *mut ffi::wl_proxy,
    axis: u32,
    direction: u32,
) {
    let st = &mut *(data as *mut State);
    if let Some(slot) = pointer_axis_slot(&mut st.pending_pointer_axis, axis) {
        slot.relative_direction = match direction {
            0 => Some(PointerAxisRelativeDirection::Identical),
            1 => Some(PointerAxisRelativeDirection::Inverted),
            _ => None,
        };
    }
}

unsafe extern "C" fn on_swipe_begin(
    data: *mut c_void,
    _gesture: *mut ffi::wl_proxy,
    _serial: u32,
    time: u32,
    _surface: *mut ffi::wl_proxy,
    fingers: u32,
) {
    (*(data as *mut State))
        .pointer_gesture_events
        .push(PointerGestureEvent::SwipeBegin { time, fingers });
}

unsafe extern "C" fn on_swipe_update(
    data: *mut c_void,
    _gesture: *mut ffi::wl_proxy,
    time: u32,
    dx: i32,
    dy: i32,
) {
    (*(data as *mut State))
        .pointer_gesture_events
        .push(PointerGestureEvent::SwipeUpdate {
            time,
            dx: ffi::wl_fixed_to_f32(dx),
            dy: ffi::wl_fixed_to_f32(dy),
        });
}

unsafe extern "C" fn on_swipe_end(
    data: *mut c_void,
    _gesture: *mut ffi::wl_proxy,
    _serial: u32,
    time: u32,
    cancelled: i32,
) {
    (*(data as *mut State))
        .pointer_gesture_events
        .push(PointerGestureEvent::SwipeEnd {
            time,
            cancelled: cancelled != 0,
        });
}

unsafe extern "C" fn on_pinch_begin(
    data: *mut c_void,
    _gesture: *mut ffi::wl_proxy,
    _serial: u32,
    time: u32,
    _surface: *mut ffi::wl_proxy,
    fingers: u32,
) {
    (*(data as *mut State))
        .pointer_gesture_events
        .push(PointerGestureEvent::PinchBegin { time, fingers });
}

unsafe extern "C" fn on_pinch_update(
    data: *mut c_void,
    _gesture: *mut ffi::wl_proxy,
    time: u32,
    dx: i32,
    dy: i32,
    scale: i32,
    rotation: i32,
) {
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

unsafe extern "C" fn on_pinch_end(
    data: *mut c_void,
    _gesture: *mut ffi::wl_proxy,
    _serial: u32,
    time: u32,
    cancelled: i32,
) {
    (*(data as *mut State))
        .pointer_gesture_events
        .push(PointerGestureEvent::PinchEnd {
            time,
            cancelled: cancelled != 0,
        });
}

unsafe extern "C" fn on_hold_begin(
    data: *mut c_void,
    _gesture: *mut ffi::wl_proxy,
    _serial: u32,
    time: u32,
    _surface: *mut ffi::wl_proxy,
    fingers: u32,
) {
    (*(data as *mut State))
        .pointer_gesture_events
        .push(PointerGestureEvent::HoldBegin { time, fingers });
}

unsafe extern "C" fn on_hold_end(
    data: *mut c_void,
    _gesture: *mut ffi::wl_proxy,
    _serial: u32,
    time: u32,
    cancelled: i32,
) {
    (*(data as *mut State))
        .pointer_gesture_events
        .push(PointerGestureEvent::HoldEnd {
            time,
            cancelled: cancelled != 0,
        });
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

unsafe extern "C" fn on_text_input_enter(
    data: *mut c_void,
    _text_input: *mut ffi::wl_proxy,
    _surface: *mut ffi::wl_proxy,
) {
    let st = data as *mut State;
    (*st).text_input_entered = true;
    send_text_input_state(st);
}

unsafe extern "C" fn on_text_input_leave(
    data: *mut c_void,
    _text_input: *mut ffi::wl_proxy,
    _surface: *mut ffi::wl_proxy,
) {
    (*(data as *mut State)).text_input_entered = false;
}

unsafe extern "C" fn on_text_input_preedit(
    data: *mut c_void,
    _text_input: *mut ffi::wl_proxy,
    text: *const std::os::raw::c_char,
    cursor_begin: i32,
    cursor_end: i32,
) {
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

unsafe extern "C" fn on_text_input_commit(
    data: *mut c_void,
    _text_input: *mut ffi::wl_proxy,
    text: *const std::os::raw::c_char,
) {
    let text = if text.is_null() {
        None
    } else {
        Some(CStr::from_ptr(text).to_string_lossy().into_owned())
    };
    (*(data as *mut State))
        .text_input_events
        .push(TextInputEvent::Commit(text));
}

unsafe extern "C" fn on_text_input_delete(
    data: *mut c_void,
    _text_input: *mut ffi::wl_proxy,
    before_length: u32,
    after_length: u32,
) {
    (*(data as *mut State))
        .text_input_events
        .push(TextInputEvent::DeleteSurrounding {
            before_length,
            after_length,
        });
}

unsafe extern "C" fn on_text_input_done(
    data: *mut c_void,
    _text_input: *mut ffi::wl_proxy,
    _serial: u32,
) {
    (*(data as *mut State))
        .text_input_events
        .push(TextInputEvent::Done);
}

mod libc {
    #[repr(C)]
    pub struct pollfd {
        pub fd: i32,
        pub events: i16,
        pub revents: i16,
    }

    pub const POLLIN: i16 = 0x0001;

    extern "C" {
        pub fn close(fd: i32) -> i32;
        pub fn poll(fds: *mut pollfd, nfds: usize, timeout: i32) -> i32;
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
static FRACTIONAL_SCALE_LISTENER: ffi::wp_fractional_scale_v1_listener =
    ffi::wp_fractional_scale_v1_listener {
        preferred_scale: on_preferred_scale,
    };
static POINTER_LISTENER: ffi::wl_pointer_listener = ffi::wl_pointer_listener {
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
static SWIPE_GESTURE_LISTENER: ffi::zwp_pointer_gesture_swipe_v1_listener =
    ffi::zwp_pointer_gesture_swipe_v1_listener {
        begin: on_swipe_begin,
        update: on_swipe_update,
        end: on_swipe_end,
    };
static PINCH_GESTURE_LISTENER: ffi::zwp_pointer_gesture_pinch_v1_listener =
    ffi::zwp_pointer_gesture_pinch_v1_listener {
        begin: on_pinch_begin,
        update: on_pinch_update,
        end: on_pinch_end,
    };
static HOLD_GESTURE_LISTENER: ffi::zwp_pointer_gesture_hold_v1_listener =
    ffi::zwp_pointer_gesture_hold_v1_listener {
        begin: on_hold_begin,
        end: on_hold_end,
    };
static KEYBOARD_LISTENER: ffi::wl_keyboard_listener = ffi::wl_keyboard_listener {
    keymap: on_keyboard_keymap,
    enter: on_keyboard_enter,
    leave: on_keyboard_leave,
    key: on_keyboard_key,
    modifiers: on_keyboard_modifiers,
    repeat_info: on_keyboard_repeat_info,
};
static TEXT_INPUT_LISTENER: ffi::zwp_text_input_v3_listener = ffi::zwp_text_input_v3_listener {
    enter: on_text_input_enter,
    leave: on_text_input_leave,
    preedit_string: on_text_input_preedit,
    commit_string: on_text_input_commit,
    delete_surrounding_text: on_text_input_delete,
    done: on_text_input_done,
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
                viewporter: ptr::null_mut(),
                fractional_scale_manager: ptr::null_mut(),
                cursor_shape_manager: ptr::null_mut(),
                cursor_shape_device: ptr::null_mut(),
                pointer_gestures_manager: ptr::null_mut(),
                gesture_swipe: ptr::null_mut(),
                gesture_pinch: ptr::null_mut(),
                gesture_hold: ptr::null_mut(),
                text_input_manager: ptr::null_mut(),
                text_input: ptr::null_mut(),
                seat: ptr::null_mut(),
                pointer: ptr::null_mut(),
                keyboard: ptr::null_mut(),
                last_pointer_serial: 0,
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
                preferred_scale_120: 120,
                fractional_active: false,
                scale_changed: false,
                input_events: Vec::new(),
                pending_pointer_axis: PointerAxisFrame::default(),
                pointer_gesture_events: Vec::new(),
                text_input_events: Vec::new(),
                text_input_entered: false,
                text_input_state: TextInputState::default(),
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
                if !state.text_input_manager.is_null() {
                    let text_input = get_text_input(state.text_input_manager, state.seat);
                    if !text_input.is_null() {
                        ffi::wl_proxy_add_listener(
                            text_input,
                            &TEXT_INPUT_LISTENER as *const _ as *const c_void,
                            data,
                        );
                        state.text_input = text_input;
                    }
                }
            }

            // Surface + xdg roles. Listen for enter/leave so the buffer scale
            // tracks the output the window is shown on.
            let surface = create_surface(state.compositor);
            ffi::wl_proxy_add_listener(
                surface,
                &SURFACE_LISTENER as *const _ as *const c_void,
                data,
            );
            // Fractional scaling is useful only as a pair: the scale protocol
            // recommends a buffer size, and viewporter maps that buffer back
            // to the xdg-configured logical surface size. If either global is
            // absent, retain the core integer buffer-scale path.
            let (viewport, fractional_scale) =
                if !state.viewporter.is_null() && !state.fractional_scale_manager.is_null() {
                    let viewport = get_viewport(state.viewporter, surface);
                    let fractional_scale =
                        get_fractional_scale(state.fractional_scale_manager, surface);
                    if !viewport.is_null() && !fractional_scale.is_null() {
                        ffi::wl_proxy_add_listener(
                            fractional_scale,
                            &FRACTIONAL_SCALE_LISTENER as *const _ as *const c_void,
                            data,
                        );
                        state.fractional_active = true;
                        state.preferred_scale_120 = (state.scale.max(1) as u32) * 120;
                        (viewport, fractional_scale)
                    } else {
                        if !viewport.is_null() {
                            ffi::wl_proxy_destroy(viewport);
                        }
                        if !fractional_scale.is_null() {
                            ffi::wl_proxy_destroy(fractional_scale);
                        }
                        (ptr::null_mut(), ptr::null_mut())
                    }
                } else {
                    (ptr::null_mut(), ptr::null_mut())
                };
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
            // Collect the initial preferred_scale before the swapchain is
            // created, avoiding a needless 1x first frame followed by resize.
            if state.fractional_active && ffi::wl_display_roundtrip(display) < 0 {
                return Err(NestedError::Roundtrip);
            }

            Ok(NestedHost {
                display,
                registry,
                surface,
                xdg_surface,
                toplevel,
                viewport,
                fractional_scale,
                state,
                ash: None,
                vk_surface: 0,
                touchpad_config: ass_core::input::TouchpadConfig::default(),
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

    /// Preferred render scale for the host surface. Fractional when the host
    /// supports fractional-scale + viewporter, otherwise the core integer
    /// `wl_output.scale` value.
    pub fn scale(&self) -> f32 {
        if self.state.fractional_active {
            self.state.preferred_scale_120.max(1) as f32 / 120.0
        } else {
            self.state.scale.max(1) as f32
        }
    }

    /// Physical (device-pixel) size = logical size × [`scale`](Self::scale).
    /// This is the swapchain extent; the buffer is divisible by `scale`, so
    /// `wl_surface.set_buffer_scale(scale)` is always valid.
    pub fn physical_size(&self) -> (u32, u32) {
        let s = self.scale();
        (
            (self.state.width.max(1) as f32 * s).round().max(1.0) as u32,
            (self.state.height.max(1) as f32 * s).round().max(1.0) as u32,
        )
    }

    /// Advertise the current buffer scale to the host. Applies on the next
    /// surface commit (the next present); call after sizing the swapchain to
    /// the matching physical size.
    pub fn set_buffer_scale(&self) {
        unsafe {
            if self.state.fractional_active {
                // fractional-scale-v1 requires buffer_scale=1. The Vulkan
                // buffer is rendered at logical*preferred_scale and the
                // viewport declares its logical surface-local destination.
                set_buffer_scale(self.surface, 1);
                viewport_set_destination(self.viewport, self.state.width, self.state.height);
            } else {
                set_buffer_scale(self.surface, self.state.scale);
            }
        };
    }

    /// Apply the focused inner client's committed text-input state to the
    /// host compositor. State is retained while the outer window is
    /// unfocused and replayed on the next host text-input `enter` event.
    pub fn set_text_input_state(&mut self, state: TextInputState) {
        self.state.text_input_state = state;
        unsafe { send_text_input_state(self.state.as_mut()) };
    }

    /// Drain IME events produced by the host compositor.
    pub fn take_text_input(&mut self) -> Vec<TextInputEvent> {
        std::mem::take(&mut self.state.text_input_events)
    }

    /// Drain touchpad gestures received from the host compositor.
    pub fn take_pointer_gestures(&mut self) -> Vec<PointerGestureEvent> {
        std::mem::take(&mut self.state.pointer_gesture_events)
    }

    /// Forward a client cursor-shape request to the host cursor. The most
    /// recent pointer enter/button serial authorizes the request.
    pub fn set_cursor_shape(&mut self, shape: u32) {
        if self.state.cursor_shape_device.is_null() || self.state.last_pointer_serial == 0 {
            return;
        }
        unsafe {
            cursor_shape_set_shape(
                self.state.cursor_shape_device,
                self.state.last_pointer_serial,
                shape.max(1),
            )
        };
    }

    /// Hide the host compositor cursor while an inner custom cursor surface
    /// is composited into the nested window (or the client explicitly asks
    /// for no cursor).
    pub fn hide_cursor(&mut self) {
        if self.state.pointer.is_null() || self.state.last_pointer_serial == 0 {
            return;
        }
        unsafe { pointer_hide_cursor(self.state.pointer, self.state.last_pointer_serial) };
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

    fn physical_size(&self) -> (u32, u32) {
        NestedHost::physical_size(self)
    }

    fn scale(&self) -> f32 {
        NestedHost::scale(self)
    }

    fn size_u32(&self) -> (u32, u32) {
        NestedHost::size_u32(self)
    }

    fn output_infos(&self) -> Vec<ass_core::output::OutputInfo> {
        let (width, height) = self.physical_size();
        vec![ass_core::output::OutputInfo {
            connector: "nested".to_owned(),
            geometry: ass_core::output::OutputGeometry {
                mode: ass_core::output::OutputMode {
                    width: width as i32,
                    height: height as i32,
                    refresh_mhz: 0,
                },
                scale: ass_core::output::Scale(self.scale()),
                transform: ass_core::Transform::Normal,
                logical_origin: ass_core::Point::default(),
            },
            // The outer compositor owns modesetting; there is nothing to
            // enumerate here.
            available_modes: Vec::new(),
        }]
    }

    fn set_touchpad_config(
        &mut self,
        config: ass_core::input::TouchpadConfig,
    ) -> ass_core::input::TouchpadStatus {
        self.touchpad_config = config;
        self.touchpad_status()
    }

    fn touchpad_status(&self) -> ass_core::input::TouchpadStatus {
        ass_core::input::TouchpadStatus {
            configurable: false,
            config: self.touchpad_config,
            ..ass_core::input::TouchpadStatus::default()
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

    /// Non-blocking drain of already-buffered host events. Used while a chrome
    /// animation is in flight so the loop renders the next frame without
    /// sleeping on the host. Returns false only on a hard error; an idle queue
    /// still returns true (no events, but alive).
    fn dispatch_nonblocking(&mut self) -> bool {
        unsafe {
            // `wl_display_dispatch_pending` processes events already read into
            // the display's internal queue without blocking for new ones. A
            // negative return is a fatal connection error, not "no events".
            if ffi::wl_display_dispatch_pending(self.display) < 0 {
                self.state.should_close = true;
            }
        }
        !self.state.should_close
    }

    fn dispatch_timeout(&mut self, timeout: std::time::Duration) -> bool {
        unsafe {
            if ffi::wl_display_dispatch_pending(self.display) < 0 {
                self.state.should_close = true;
                return false;
            }
            // Flush requests before waiting. A would-block result is benign:
            // POLLOUT is unnecessary here because the regular frame loop will
            // retry and our small request stream fits the Wayland socket.
            let _ = ffi::wl_display_flush(self.display);
            let mut fd = crate::nested::libc::pollfd {
                fd: ffi::wl_display_get_fd(self.display),
                events: crate::nested::libc::POLLIN,
                revents: 0,
            };
            let timeout_ms = timeout.as_millis().min(i32::MAX as u128) as i32;
            let ready = crate::nested::libc::poll(&mut fd, 1, timeout_ms);
            let dispatch_failed =
                ready > 0 && fd.revents != 0 && ffi::wl_display_dispatch(self.display) < 0;
            if ready < 0 || dispatch_failed {
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

    fn set_text_input_state(&mut self, state: TextInputState) {
        NestedHost::set_text_input_state(self, state);
    }

    fn take_text_input(&mut self) -> Vec<TextInputEvent> {
        NestedHost::take_text_input(self)
    }

    fn take_pointer_gestures(&mut self) -> Vec<PointerGestureEvent> {
        NestedHost::take_pointer_gestures(self)
    }

    fn set_cursor_shape(&mut self, shape: u32) {
        NestedHost::set_cursor_shape(self, shape);
    }

    fn hide_cursor(&mut self) {
        NestedHost::hide_cursor(self);
    }

    fn set_buffer_scale(&self) {
        NestedHost::set_buffer_scale(self);
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
                destroy_pointer_gestures(self.state.as_mut());
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
                self.state.text_input,
                self.state.cursor_shape_device,
                self.fractional_scale,
                self.viewport,
                self.toplevel,
                self.xdg_surface,
                self.surface,
                self.wm_base_ptr(),
                self.state.fractional_scale_manager,
                self.state.viewporter,
                self.state.text_input_manager,
                self.state.cursor_shape_manager,
                self.state.pointer_gestures_manager,
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
