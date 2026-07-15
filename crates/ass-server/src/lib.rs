//! Hand-rolled Wayland server for ass.
//!
//! Drives libwayland-server directly over FFI: it creates the display and
//! socket, advertises the core globals, and owns protocol object lifecycle. The
//! shm implementation and the core `wl_*` interface tables come from
//! libwayland-server; ass implements the request handlers.

mod extensions;
mod ffi;
mod keyboard;

use std::ffi::{c_void, CStr, CString};
use std::os::raw::c_int;

use ass_core::layout::Layout;
use ass_core::{SurfaceDmabuf, SurfacePixels};

/// Single-plane dma-buf parameters backing a `wl_buffer`, or accumulating in a
/// `zwp_linux_buffer_params_v1`. Owns the imported file descriptor.
struct DmabufBuffer {
    fd: i32,
    width: i32,
    height: i32,
    drm_format: u32,
    modifier: u64,
    offset: u32,
    stride: u32,
    /// Set once the plane has been added (`add`), so `create` can validate.
    have_plane: bool,
}

impl DmabufBuffer {
    fn empty() -> DmabufBuffer {
        DmabufBuffer {
            fd: -1,
            width: 0,
            height: 0,
            drm_format: 0,
            modifier: 0,
            offset: 0,
            stride: 0,
            have_plane: false,
        }
    }
}

impl Drop for DmabufBuffer {
    fn drop(&mut self) {
        if self.fd >= 0 {
            unsafe { libc_close(self.fd) };
        }
    }
}

/// The active clipboard selection: the `wl_data_source` that owns it and the
/// MIME types it advertised. Set by `wl_data_device.set_selection`, advertised
/// to every bound `wl_data_device` via `wl_data_offer`.
struct Selection {
    source: *mut ffi::wl_resource,
    mime_types: Vec<String>,
}

/// Accumulated `xdg_positioner` state used to compute a popup's placement
/// relative to its parent surface. Only the fields real-world clients set
/// (size, anchor rect, offset) are tracked; anchor edge and gravity default to
/// the common "top-left" so menus and tooltips place predictably.
#[derive(Default)]
struct PositionerState {
    size: Option<ass_core::Size>,
    anchor_rect: Option<ass_core::Rect>,
    offset: ass_core::Point,
}

// Minimal close() without pulling the libc crate.
extern "C" {
    fn close(fd: std::os::raw::c_int) -> std::os::raw::c_int;
    fn malloc(size: usize) -> *mut c_void;
    fn free(ptr: *mut c_void);
}
pub(crate) unsafe fn libc_close(fd: i32) {
    close(fd);
}

/// A client surface: its pending buffer, the last committed contents copied out
/// of shm, and its xdg role.
pub struct SurfaceRec {
    pub resource: *mut ffi::wl_resource,
    pending_buffer: *mut ffi::wl_resource,
    pub mapped: bool,
    pub width: i32,
    pub height: i32,
    /// Logical position of the surface's top-left corner in the output. M1
    /// assigns a placeholder cascade on map; M3's window manager will own
    /// placement policy.
    pub position: ass_core::Point,
    /// Last committed contents, tightly packed BGRA8, copied out of the client
    /// shm buffer at commit so the buffer can be released immediately.
    pixels: Vec<u8>,
    /// Bumped on every commit that updates content (shm or dma-buf).
    generation: u64,
    /// True when the committed content is a dma-buf (`dmabuf_buffer` holds it),
    /// false when it is the CPU-copied `pixels` from shm.
    content_is_dmabuf: bool,
    /// Held dma-buf-backed `wl_buffer` whose fd is sampled directly; released
    /// when replaced or on unmap. Null for shm content.
    dmabuf_buffer: *mut ffi::wl_resource,
    frame_callbacks: Vec<*mut ffi::wl_resource>,
    // xdg-shell role state.
    xdg_surface: *mut ffi::wl_resource,
    xdg_toplevel: *mut ffi::wl_resource,
    xdg_configured: bool,
    display: *mut ffi::wl_display,
    /// Back-pointer to the server State. Lets `surface_resource_destroy`
    /// detach this rec from `state.surfaces` and reclaim the box without
    /// holding a borrow across libwayland callbacks.
    state: *mut State,
    /// Slot index in `state.surfaces`. Used to null the entry in O(1) on
    /// destroy. Focus-driven raises update this index when they move a live
    /// pointer to the end of the stacking vector.
    index: usize,
    // ----- subsurface state -----
    /// Parent surface if this is a subsurface, null otherwise. A subsurface
    /// does not get an xdg role; its parent must be a mapped toplevel (or a
    /// subsurface of one) for it to appear on screen.
    parent: *mut SurfaceRec,
    /// Children that are subsurfaces of this surface. Order within the list is
    /// the stacking order protocol requests dictate via `place_above` /
    /// `place_below`; M2 approximates this with an "above_parent" flag per
    /// child rather than a true z-sorted tree.
    children: Vec<*mut SurfaceRec>,
    /// Offset relative to the parent's top-left, set by
    /// `wl_subsurface.set_position`. Defaults to (0, 0).
    subsurface_offset: ass_core::Point,
    /// Whether this subsurface renders above (true) or below (false) its
    /// parent. Defaults to above; `place_below` flips it.
    subsurface_above_parent: bool,
    // ----- toplevel window state -----
    /// Per-toplevel metadata. Populated when the toplevel role is acquired
    /// (`xdg_surface.get_toplevel`) and updated by `set_title`,
    /// `set_app_id`, `set_min_size`, `set_max_size`, `set_parent`, and the
    /// maximized/fullscreen transitions. Sent back to the client in the
    /// states array of subsequent `xdg_toplevel.configure` events.
    pub window: ass_core::window::Window,
    /// Tiling target (ADR-0024): the layout rect the tiling policy last
    /// configured this surface to, or `None` when not under active tiling.
    /// The apply path reconfigures only when the target moves.
    pub layout_target: Option<ass_core::Rect>,
    // ----- wp_viewport state -----
    /// Source rectangle in surface pixel coords, or None for "whole buffer".
    /// Set by `wp_viewport.set_source`. Coordinates arrive as 24.8
    /// fixed-point; we store them as f32.
    pub viewport_src: Option<ass_core::Rect>,
    /// Destination size in logical pixels, or None for "source size".
    /// Set by `wp_viewport.set_destination`.
    pub viewport_dst: Option<ass_core::Size>,
    // ----- wp_fractional_scale_v1 state -----
    /// The `wp_fractional_scale_v1` resource bound for this surface, if any.
    /// The server posts `preferred_scale` here when the output's scale changes.
    pub fractional_scale: *mut ffi::wl_resource,
    // ----- pending buffer transform / scale -----
    /// Pending buffer transform from `wl_surface.set_buffer_transform`,
    /// applied on the next commit.
    pending_transform: ass_core::Transform,
    /// Pending buffer scale from `wl_surface.set_buffer_scale`.
    pending_scale: i32,
    // ----- damage tracking -----
    /// Damage rectangles accumulated by `wl_surface.damage` /
    /// `damage_buffer` since the last commit. Surface-local pixel coords;
    /// empty means "client did not report damage, renderer should
    /// re-upload the whole texture on a generation change".
    pending_damage: Vec<ass_core::Rect>,
    /// Damage from the most recent commit, surfaced to the renderer via
    /// `Server::toplevel_frames`. Cleared at the next commit.
    committed_damage: Vec<ass_core::Rect>,
}

impl SurfaceRec {
    fn new(resource: *mut ffi::wl_resource) -> SurfaceRec {
        SurfaceRec {
            resource,
            pending_buffer: std::ptr::null_mut(),
            mapped: false,
            width: 0,
            height: 0,
            position: ass_core::Point::default(),
            pixels: Vec::new(),
            generation: 0,
            content_is_dmabuf: false,
            dmabuf_buffer: std::ptr::null_mut(),
            frame_callbacks: Vec::new(),
            xdg_surface: std::ptr::null_mut(),
            xdg_toplevel: std::ptr::null_mut(),
            xdg_configured: false,
            display: std::ptr::null_mut(),
            state: std::ptr::null_mut(),
            index: 0,
            parent: std::ptr::null_mut(),
            children: Vec::new(),
            subsurface_offset: ass_core::Point::default(),
            subsurface_above_parent: true,
            window: ass_core::window::Window::default(),
            viewport_src: None,
            viewport_dst: None,
            fractional_scale: std::ptr::null_mut(),
            pending_transform: ass_core::Transform::Normal,
            pending_scale: 1,
            pending_damage: Vec::new(),
            committed_damage: Vec::new(),
            // Tiling (ADR-0024): the last layout rect we configured this
            // surface to. `None` until applied; the apply path reconfigures
            // only when the target moves, so steady state sends no configures.
            layout_target: None,
        }
    }
}

/// Server-wide state. Its address is handed to the C bind callbacks, so it is
/// boxed and never moved out.
pub(crate) struct State {
    pub(crate) display: *mut ffi::wl_display,
    /// Surface pointers in stacking order (bottom to top). Entries are nulled
    /// when a surface's destroy notify fires; focusing a toplevel moves its
    /// pointer to the end and updates affected live records' slot indices.
    /// Iterators must skip null entries.
    surfaces: Vec<*mut SurfaceRec>,
    /// Every `wl_pointer` resource clients have bound. Entries null out when
    /// the resource's destroy notify fires. Pointer events are forwarded only
    /// to the resources owned by the currently focused surface's client.
    pointer_resources: Vec<*mut ffi::wl_resource>,
    /// Every `wl_keyboard` resource clients have bound. Same lifecycle as
    /// pointer_resources; the keymap event is sent to each on creation.
    keyboard_resources: Vec<*mut ffi::wl_resource>,
    /// Every `wl_touch` resource clients have bound. Same lifecycle as the
    /// pointer/keyboard lists.
    touch_resources: Vec<*mut ffi::wl_resource>,
    /// Every `wl_output` resource clients have bound. Resent in full when the
    /// output geometry (mode/scale/transform) changes so bound clients update.
    pub(crate) output_resources: Vec<*mut ffi::wl_resource>,
    /// Every `xdg_output` resource clients have bound (zxdg-output v1). Resent
    /// together with the wl_output reconfigure path.
    pub(crate) xdg_output_resources: Vec<*mut ffi::wl_resource>,
    /// Every `wl_data_device` resource clients have bound. A `set_selection`
    /// advertises a new `wl_data_offer` to each.
    data_devices: Vec<*mut ffi::wl_resource>,
    /// The current clipboard selection, if any.
    selection: Option<Selection>,
    /// Active `zwp_relative_pointer_v1` resources. Relative-motion deltas are
    /// posted to each (filtered to the focused client's set).
    relative_pointers: Vec<*mut ffi::wl_resource>,
    /// Bound `ext_foreign_toplevel_list_v1` resources. New toplevels, title
    /// changes, and removals are pushed to each.
    foreign_toplevel_lists: Vec<*mut ffi::wl_resource>,
    /// Per-toplevel foreign handle resources, keyed by window id. Lets the
    /// server push title/app_id/closed updates to the right handle.
    foreign_handles: std::collections::HashMap<u64, *mut ffi::wl_resource>,
    /// Active `zwp_text_input_v3` resources, with the surface they last
    /// targeted (null until the client enables it). The focused client's
    /// enabled text_input receives `enter`/`leave` on keyboard focus changes.
    text_inputs: Vec<(*mut ffi::wl_resource, *mut ffi::wl_resource)>,
    /// The last cursor shape requested by the focused client via
    /// `wp_cursor_shape_device_v1.set_shape` (or `wl_pointer.set_cursor`,
    /// once wired). 0 = default arrow. Exposed to the renderer.
    cursor_shape: u32,
    /// Surface resource currently under the pointer, or null when the pointer
    /// is outside any mapped toplevel. Drives enter/leave transitions.
    pointer_focus: *mut ffi::wl_resource,
    /// Surface resource that currently has keyboard focus, or null. Decoupled
    /// from `pointer_focus` because click-to-focus sets keyboard focus only on
    /// button press, not on motion.
    keyboard_focus: *mut ffi::wl_resource,
    /// Surface that had keyboard focus before chrome (the launcher) grabbed
    /// it, or null. While a grab is active, `keyboard_focus` is null and the
    /// saved surface receives a `wl_keyboard.leave`; releasing the grab
    /// restores it via `wl_keyboard.enter` — but only if nothing else took
    /// focus in the meantime. See `grab_keyboard_focus` / ADR-0022.
    saved_keyboard_focus: *mut ffi::wl_resource,
    /// Last reported pointer position in compositor logical space.
    pointer_x: f32,
    pointer_y: f32,
    /// Last serial handed to a `wl_pointer.button` event, for clients that
    /// gate interactive moves on a press serial (xdg_toplevel.move &c.).
    last_button_serial: u32,
    /// xkbcommon keymap and modifier state. Owned by the server so the
    /// keymap fd lives as long as clients may bind a keyboard.
    keyboard: Option<keyboard::Keyboard>,
    /// Ongoing interactive move or resize, started by `xdg_toplevel.move` /
    /// `resize` when the supplied serial matches the last pointer button
    /// press. Cleared on button release. While active, pointer motion
    /// updates the window's geometry instead of (only) being forwarded to
    /// the focused client.
    interactive: Option<ass_core::window::Interactive>,
    /// The active interactive grab was initiated by the compositor's
    /// invisible floating-window border. Its initiating button press was not
    /// sent to the client, so the matching release must also be consumed.
    compositor_pointer_grab: bool,
    /// Parameters for the tiling policy (gaps, master ratio). Per-workspace
    /// tiling on/off lives on each workspace in the model (ADR-0024).
    layout_params: ass_core::layout::LayoutParams,
    /// Config-driven window rules (ADR-0026). Evaluated on first map; the
    /// first match prescribes a workspace move and/or a forced layout role.
    window_rules: Vec<ass_core::window_rule::WindowRule>,
    /// The focused output's geometry (ADR-0028): the tiling work-area is its
    /// logical rect. Updated by the backend on resize; defaults to identity.
    pub(crate) output_geometry: ass_core::output::OutputGeometry,
    /// Dynamic per-output workspaces (ADR-0025). Toplevels are placed on the
    /// current workspace at first map; rendering and input see only the
    /// visible set (`visible_toplevels`).
    workspaces: ass_core::workspace::WorkspaceModel,
    /// The single output the nested backend presents. Multi-output lands in
    /// M7 (ADR-0028); until then there is exactly one.
    output: ass_core::workspace::OutputId,
    /// Monotonic counter for durable window identifiers (ADR-0032). Starts
    /// at 1 so `WindowId(0)` remains reserved for the `Window::default()`
    /// that non-toplevel surfaces carry.
    next_window_id: u64,
}

impl State {
    fn new(display: *mut ffi::wl_display) -> State {
        let mut workspaces = ass_core::workspace::WorkspaceModel::new();
        let output = workspaces.add_output("nested");
        State {
            display,
            surfaces: Vec::new(),
            pointer_resources: Vec::new(),
            keyboard_resources: Vec::new(),
            touch_resources: Vec::new(),
            output_resources: Vec::new(),
            xdg_output_resources: Vec::new(),
            data_devices: Vec::new(),
            selection: None,
            relative_pointers: Vec::new(),
            foreign_toplevel_lists: Vec::new(),
            foreign_handles: std::collections::HashMap::new(),
            text_inputs: Vec::new(),
            cursor_shape: 0,
            pointer_focus: std::ptr::null_mut(),
            keyboard_focus: std::ptr::null_mut(),
            saved_keyboard_focus: std::ptr::null_mut(),
            pointer_x: 0.0,
            pointer_y: 0.0,
            last_button_serial: 0,
            keyboard: None,
            interactive: None,
            compositor_pointer_grab: false,
            workspaces,
            output,
            layout_params: ass_core::layout::LayoutParams::default(),
            window_rules: Vec::new(),
            output_geometry: ass_core::output::OutputGeometry::default(),
            next_window_id: 1,
        }
    }

    /// Allocate a fresh, never-reused `WindowId` (ADR-0032). Called on the
    /// main loop when a toplevel role is acquired.
    fn alloc_window_id(&mut self) -> ass_core::window::WindowId {
        let id = self.next_window_id;
        self.next_window_id += 1;
        ass_core::window::WindowId(id)
    }

    /// Iterate live surface records, skipping nulled slots. The returned
    /// iterator yields raw pointers; callers must validate liveness for any
    /// operation that holds the pointer across re-entry into libwayland.
    fn live_surfaces(&self) -> impl Iterator<Item = *mut SurfaceRec> + '_ {
        self.surfaces.iter().copied().filter(|p| !p.is_null())
    }

    /// Crate-visible live-surface iterator for `extensions.rs`.
    pub(crate) fn live_surfaces_pub(&self) -> impl Iterator<Item = *mut SurfaceRec> + '_ {
        self.surfaces.iter().copied().filter(|p| !p.is_null())
    }
}

/// The Wayland server: socket, globals, and object lifecycle.
pub struct Server {
    state: Box<State>,
    socket: String,
    /// Monotonic epoch for `wl_pointer.axis` / `wl_pointer.button` time
    /// stamps. The nested backend's input events carry no time, so the server
    /// derives one from elapsed time since creation (millisecond granularity,
    /// matching `wl_pointer`'s `uint32 time` semantics).
    epoch: std::time::Instant,
}

impl Server {
    /// Create the display, bind an auto-named socket, and advertise the core
    /// globals.
    pub fn new() -> Result<Server, ServerError> {
        unsafe {
            let display = ffi::wl_display_create();
            if display.is_null() {
                return Err(ServerError::DisplayCreate);
            }
            let sock = ffi::wl_display_add_socket_auto(display);
            if sock.is_null() {
                ffi::wl_display_destroy(display);
                return Err(ServerError::Socket);
            }
            let socket = CStr::from_ptr(sock).to_string_lossy().into_owned();
            if ffi::wl_display_init_shm(display) != 0 {
                ffi::wl_display_destroy(display);
                return Err(ServerError::Shm);
            }

            let mut state = Box::new(State::new(display));
            // The keyboard is optional in the sense that its absence should
            // not crash the compositor — but a working keymap is needed for
            // interactive use, so a failure here is logged loudly. The seat
            // advertises keyboard capability only when this succeeded.
            match keyboard::Keyboard::new() {
                Ok(kb) => {
                    state.keyboard = Some(kb);
                }
                Err(e) => {
                    log::error!("[server] keyboard init failed: {e}; keyboard capability disabled");
                }
            }
            let data = &mut *state as *mut State as *mut c_void;

            ffi::wl_global_create(
                display,
                &ffi::wl_compositor_interface,
                4,
                data,
                compositor_bind,
            );
            ffi::wl_global_create(display, &ffi::wl_output_interface, 2, data, output_bind);
            ffi::wl_global_create(
                display,
                &ffi::xdg_wm_base_interface,
                1,
                data,
                xdg_wm_base_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::wl_subcompositor_interface,
                1,
                data,
                subcompositor_bind,
            );
            ffi::wl_global_create(display, &ffi::wl_seat_interface, 5, data, seat_bind);
            ffi::wl_global_create(
                display,
                &ffi::wl_data_device_manager_interface,
                3,
                data,
                ddm_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::zwp_linux_dmabuf_v1_interface,
                3,
                data,
                dmabuf_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::wp_viewporter_interface,
                1,
                data,
                viewporter_bind,
            );
            // Extension protocols. Each advertises its global so clients that
            // require it connect without a protocol error.
            ffi::wl_global_create(
                display,
                &ffi::zxdg_output_manager_v1_interface,
                3,
                data,
                extensions::xdg_output_manager_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::wp_presentation_interface,
                1,
                data,
                extensions::presentation_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::wp_fractional_scale_manager_v1_interface,
                1,
                data,
                extensions::fractional_scale_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::zwp_idle_inhibit_manager_v1_interface,
                1,
                data,
                extensions::idle_inhibit_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::ext_idle_notifier_v1_interface,
                1,
                data,
                extensions::idle_notifier_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::zwp_relative_pointer_manager_v1_interface,
                1,
                data,
                extensions::relative_pointer_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::zwp_pointer_constraints_v1_interface,
                1,
                data,
                extensions::pointer_constraints_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::ext_session_lock_manager_v1_interface,
                1,
                data,
                extensions::session_lock_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::ext_foreign_toplevel_list_v1_interface,
                1,
                data,
                extensions::foreign_toplevel_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::ext_data_control_manager_v1_interface,
                1,
                data,
                extensions::data_control_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::zwp_primary_selection_device_manager_v1_interface,
                1,
                data,
                extensions::primary_selection_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::wp_cursor_shape_manager_v1_interface,
                1,
                data,
                extensions::cursor_shape_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::zwp_text_input_manager_v3_interface,
                1,
                data,
                extensions::text_input_bind,
            );

            Ok(Server {
                state,
                socket,
                epoch: std::time::Instant::now(),
            })
        }
    }

    /// The socket name clients connect to (set `WAYLAND_DISPLAY` to this).
    pub fn socket(&self) -> &str {
        &self.socket
    }

    /// Process pending client events and flush queued events. Non-blocking.
    pub fn dispatch(&mut self) {
        unsafe {
            let loop_ = ffi::wl_display_get_event_loop(self.state.display);
            ffi::wl_event_loop_dispatch(loop_, 0);
            ffi::wl_display_flush_clients(self.state.display);
        }
    }

    /// Mapped xdg-toplevel surfaces backed by shm (CPU pixels), for the renderer
    /// to upload. Subsurfaces are skipped until subsurface placement is modeled.
    /// The set of toplevel ids on the current workspace of each output — the
    /// only surfaces the renderer, chrome, and input may touch (ADR-0025).
    fn visible(&self) -> std::collections::HashSet<ass_core::window::WindowId> {
        self.state
            .workspaces
            .visible_toplevels()
            .into_iter()
            .collect()
    }

    pub fn toplevel_frames(&self) -> Vec<SurfacePixels<'_>> {
        let visible = self.visible();
        self.state
            .surfaces
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .map(|p| unsafe { &*p })
            .filter(|s| {
                s.mapped
                    && !s.xdg_toplevel.is_null()
                    && !s.window.minimized
                    && !s.content_is_dmabuf
                    && !s.pixels.is_empty()
                    && visible.contains(&s.window.id)
            })
            .map(|s| SurfacePixels {
                id: s.resource as usize,
                width: s.width,
                height: s.height,
                generation: s.generation,
                pixels: &s.pixels,
                geometry: ass_core::SurfaceGeometry {
                    position: s.position,
                    transform: s.pending_transform,
                    buffer_scale: s.pending_scale,
                    viewport_src: s.viewport_src,
                    viewport_dst: s.viewport_dst,
                    ..Default::default()
                },
                damage: &s.committed_damage,
            })
            .collect()
    }

    /// Mapped xdg-toplevel surfaces backed by a dma-buf, for the renderer to
    /// import zero-copy. The `fd` is borrowed (flux dups it); the server keeps
    /// ownership until the backing buffer is replaced or destroyed.
    pub fn toplevel_dmabuf_frames(&self) -> Vec<SurfaceDmabuf> {
        let visible = self.visible();
        self.state
            .surfaces
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .map(|p| unsafe { &*p })
            .filter(|s| {
                s.mapped
                    && !s.xdg_toplevel.is_null()
                    && !s.window.minimized
                    && s.content_is_dmabuf
                    && !s.dmabuf_buffer.is_null()
                    && visible.contains(&s.window.id)
            })
            .filter_map(|s| {
                let db = unsafe {
                    ffi::wl_resource_get_user_data(s.dmabuf_buffer) as *const DmabufBuffer
                };
                if db.is_null() {
                    return None;
                }
                let db = unsafe { &*db };
                Some(SurfaceDmabuf {
                    id: s.resource as usize,
                    width: s.width,
                    height: s.height,
                    generation: s.generation,
                    fd: db.fd,
                    drm_format: db.drm_format,
                    modifier: db.modifier,
                    offset: db.offset,
                    stride: db.stride,
                    geometry: ass_core::SurfaceGeometry {
                        position: s.position,
                        transform: s.pending_transform,
                        buffer_scale: s.pending_scale,
                        viewport_src: s.viewport_src,
                        viewport_dst: s.viewport_dst,
                        ..Default::default()
                    },
                })
            })
            .collect()
    }

    /// Mapped subsurfaces backed by shm whose `place_below` was the most
    /// recent stacking request — these render *under* their parent toplevel.
    /// M2 surfaces only direct children of mapped toplevels; nested
    /// subsurface-of-subsurface chains are deferred. Within this list, order
    /// is the server's iteration order over the parent's children.
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

    fn collect_subsurfaces_shm(&self, want_above: bool) -> Vec<SurfacePixels<'_>> {
        let visible = self.visible();
        let mut out = Vec::new();
        for p in self.state.live_surfaces() {
            let parent = unsafe { &*p };
            if !parent.mapped
                || parent.xdg_toplevel.is_null()
                || !visible.contains(&parent.window.id)
            {
                continue;
            }
            for &child_ptr in &parent.children {
                if child_ptr.is_null() {
                    continue;
                }
                let child = unsafe { &*child_ptr };
                if child.subsurface_above_parent != want_above
                    || !child.mapped
                    || child.content_is_dmabuf
                    || child.pixels.is_empty()
                {
                    continue;
                }
                let absolute = ass_core::Point {
                    x: parent.position.x + child.subsurface_offset.x,
                    y: parent.position.y + child.subsurface_offset.y,
                };
                out.push(SurfacePixels {
                    id: child.resource as usize,
                    width: child.width,
                    height: child.height,
                    generation: child.generation,
                    pixels: &child.pixels,
                    geometry: ass_core::SurfaceGeometry {
                        position: absolute,
                        transform: child.pending_transform,
                        buffer_scale: child.pending_scale,
                        viewport_src: child.viewport_src,
                        viewport_dst: child.viewport_dst,
                        ..Default::default()
                    },
                    damage: &child.committed_damage,
                });
            }
        }
        out
    }

    fn collect_subsurfaces_dmabuf(&self, want_above: bool) -> Vec<SurfaceDmabuf> {
        let visible = self.visible();
        let mut out = Vec::new();
        for p in self.state.live_surfaces() {
            let parent = unsafe { &*p };
            if !parent.mapped
                || parent.xdg_toplevel.is_null()
                || !visible.contains(&parent.window.id)
            {
                continue;
            }
            for &child_ptr in &parent.children {
                if child_ptr.is_null() {
                    continue;
                }
                let child = unsafe { &*child_ptr };
                if child.subsurface_above_parent != want_above
                    || !child.mapped
                    || !child.content_is_dmabuf
                    || child.dmabuf_buffer.is_null()
                {
                    continue;
                }
                let db = unsafe {
                    ffi::wl_resource_get_user_data(child.dmabuf_buffer) as *const DmabufBuffer
                };
                if db.is_null() {
                    continue;
                }
                let db = unsafe { &*db };
                let absolute = ass_core::Point {
                    x: parent.position.x + child.subsurface_offset.x,
                    y: parent.position.y + child.subsurface_offset.y,
                };
                out.push(SurfaceDmabuf {
                    id: child.resource as usize,
                    width: child.width,
                    height: child.height,
                    generation: child.generation,
                    fd: db.fd,
                    drm_format: db.drm_format,
                    modifier: db.modifier,
                    offset: db.offset,
                    stride: db.stride,
                    geometry: ass_core::SurfaceGeometry {
                        position: absolute,
                        transform: child.pending_transform,
                        buffer_scale: child.pending_scale,
                        viewport_src: child.viewport_src,
                        viewport_dst: child.viewport_dst,
                        ..Default::default()
                    },
                });
            }
        }
        out
    }

    /// Fire and clear all pending frame callbacks, pacing clients to the output.
    /// `time_ms` is a millisecond timestamp from a monotonic clock.
    pub fn send_frame_callbacks(&mut self, time_ms: u32) {
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
            }
        }
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }

    /// Forward a drained batch of input events from the backend to the focused
    /// client. Pointer motion drives hit-testing and enter/leave transitions;
    /// pointer buttons go to the current focus and also drive click-to-focus
    /// for the keyboard. Key events update xkbcommon state and post
    /// `wl_keyboard.key` plus any resulting `wl_keyboard.modifiers` change.
    /// Pointer axis (scroll wheel) is posted as `wl_pointer.axis` followed by
    /// `wl_pointer.axis_source` / `wl_pointer.axis_stop`, and discrete clicks
    /// are posted as `wl_pointer.axis_discrete` for clients that prefer the
    /// coarse signal.
    pub fn forward_input(
        &mut self,
        events: &[ass_core::input::InputEvent],
        keymap: &ass_core::keybind::Keymap,
    ) -> Vec<ass_core::keybind::Action> {
        let mut actions = Vec::new();
        let time = self.epoch.elapsed().as_millis() as u32;
        for event in events {
            use ass_core::input::InputEvent::*;
            match *event {
                PointerMotion { x, y } => self.pointer_motion(x, y),
                PointerButton { button, state } => self.pointer_button(button, state),
                PointerLeave => self.pointer_leave_all(),
                PointerAxis { dx, dy } => self.pointer_axis(time, dx, dy),
                TouchDown { id, x, y } => self.touch_down(time, id, x, y),
                TouchMotion { id, x, y } => self.touch_motion(time, id, x, y),
                TouchUp { id } => self.touch_up(time, id),
                TouchFrame => self.touch_frame(),
                TouchCancel => self.touch_cancel(),
                Key { code, state } => {
                    if let Some(a) = self.keyboard_key(code, state, keymap) {
                        actions.push(a);
                    }
                }
            }
        }
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
        actions
    }

    /// Advance the xkbcommon state with one key event and return the keysym
    /// and printable character it produced, without forwarding anything to a
    /// client.
    ///
    /// Used by the main loop when compositor chrome owns the keyboard (the
    /// launcher overlay is open): the key event is consumed by the chrome
    /// rather than delivered to the focused client, but the server's xkb
    /// state still advances so modifier tracking stays consistent when the
    /// client resumes ownership. Returns `None` only when the server has no
    /// keyboard compiled. See ADR-0022.
    pub fn key_char(&mut self, evdev_code: u32, pressed: bool) -> Option<ass_core::input::KeyChar> {
        self.state.keyboard.as_mut().map(|kb| {
            let o = kb.update_key(evdev_code, pressed);
            ass_core::input::KeyChar {
                keysym: o.keysym,
                ch: o.utf8,
                mods: ass_core::input::Mods(o.depressed),
            }
        })
    }

    /// Grab the keyboard away from the focused client for compositor-side use
    /// (the launcher overlay): sends `wl_keyboard.leave` to the current focus
    /// and clears focus so no client receives keys while the grab is active.
    /// Idempotent: a second call while grabbed is a no-op. Pair with
    /// [`Server::release_keyboard_focus`]. See ADR-0022.
    pub fn grab_keyboard_focus(&mut self) {
        // Don't clobber the saved focus if already grabbed.
        if !self.state.saved_keyboard_focus.is_null() {
            return;
        }
        self.state.saved_keyboard_focus = self.state.keyboard_focus;
        if !self.state.keyboard_focus.is_null() {
            // change_keyboard_focus posts the leave and clears focus.
            self.change_keyboard_focus(std::ptr::null_mut());
        }
    }

    /// Release a keyboard grab taken by [`Server::grab_keyboard_focus`]:
    /// restores `wl_keyboard.enter` to the surface that had focus before the
    /// grab, but only if nothing else has since taken focus (e.g. the
    /// launcher focusing a running app, or a pointer click-to-focus). If focus
    /// moved during the grab, the current focus is left alone. No-op when no
    /// grab is active.
    pub fn release_keyboard_focus(&mut self) {
        let saved = self.state.saved_keyboard_focus;
        if saved.is_null() {
            return;
        }
        self.state.saved_keyboard_focus = std::ptr::null_mut();
        // Only restore if focus is still vacant; otherwise another path already
        // established a new focus and we must not override it.
        if self.state.keyboard_focus.is_null() {
            self.change_keyboard_focus(saved);
        }
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

    /// Enumerate live toplevel windows. The shell uses this for the overview
    /// and any chrome that needs a list of windows. Reads current metadata;
    /// mutation happens through xdg_toplevel requests from the owning client.
    pub fn windows(&self) -> Vec<ass_core::window::Window> {
        let visible = self.visible();
        self.state
            .live_surfaces()
            .map(|p| unsafe { &*p })
            .filter(|s| !s.xdg_toplevel.is_null() && visible.contains(&s.window.id))
            .map(|s| s.window.clone())
            .collect()
    }

    /// Ask a toplevel to close by posting `xdg_toplevel.close`. The client
    /// responds by destroying its `xdg_toplevel` (and usually the surface).
    /// No-op if `surface_id` does not name a live toplevel.
    pub fn close_toplevel(&mut self, surface_id: ass_core::window::WindowId) {
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

    /// Mark a toplevel as activated (or not) and emit a configure so the
    /// client updates its focus state. The shell calls this when keyboard
    /// focus changes; M1's click-to-focus already posts keyboard enter/leave,
    /// and this complements it with the toplevel-state side.
    pub fn set_toplevel_activated(
        &mut self,
        surface_id: ass_core::window::WindowId,
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

    /// Begin an interactive move from the shell (server-side decorations,
    /// overview drag, etc.). Unlike the client-initiated
    /// `xdg_toplevel.move` path, no serial validation is performed — the
    /// compositor is initiating the grab itself. No-op if a grab is already
    /// active or the surface is not a live toplevel.
    pub fn start_interactive_move(&mut self, surface_id: ass_core::window::WindowId) {
        if self.state.interactive.is_some() {
            return;
        }
        let rec = self.find_surface_by_window_id(surface_id);
        if rec.is_null() || unsafe { (*rec).xdg_toplevel.is_null() } {
            return;
        }
        self.change_keyboard_focus(unsafe { (*rec).resource });
        self.state.interactive = Some(ass_core::window::Interactive::Move {
            window_id: surface_id,
            origin: (self.state.pointer_x, self.state.pointer_y),
            start_position: unsafe { (*rec).position },
        });
        self.state.compositor_pointer_grab = false;
    }

    /// Begin an interactive resize from the shell. Same serial-less contract
    /// as [`start_interactive_move`](Self::start_interactive_move).
    pub fn start_interactive_resize(
        &mut self,
        surface_id: ass_core::window::WindowId,
        edges: ass_core::window::ResizeEdges,
    ) {
        if self.state.interactive.is_some() {
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
            (*rec).window.state.resizing = true;
            reconfigure_with_state(rec);
        }
        self.state.interactive = Some(ass_core::window::Interactive::Resize {
            window_id: surface_id,
            edges,
            origin: (self.state.pointer_x, self.state.pointer_y),
            start_position: unsafe { (*rec).position },
            start_size: ass_core::Size {
                w: unsafe { (*rec).width },
                h: unsafe { (*rec).height },
            },
        });
        self.state.compositor_pointer_grab = false;
    }

    /// Whether an interactive grab (move or resize) is currently active.
    /// The shell uses this to change the cursor or suppress overview
    /// animations during a grab.
    pub fn interactive(&self) -> Option<ass_core::window::Interactive> {
        self.state.interactive
    }

    /// Focus a toplevel by its surface id. Used by the shell's window list
    /// (click-to-focus from chrome) and by future overview / launcher
    /// surfaces. Equivalent to the click-to-focus path driven from a pointer
    /// button press, but initiated by the compositor. No-op if the id does
    /// not name a live toplevel.
    pub fn focus_surface_by_id(&mut self, surface_id: ass_core::window::WindowId) {
        let rec = self.find_surface_by_window_id(surface_id);
        if rec.is_null() {
            return;
        }
        unsafe {
            if (*rec).xdg_toplevel.is_null() || !(*rec).mapped {
                return;
            }
        }
        let resource = unsafe { (*rec).resource };
        // Use the same transition as a click on the surface: keyboard enter
        // for the new focus, leave for the old, plus activated-bit
        // reconfigure. We do not call pointer_motion here because the
        // pointer may be elsewhere; only the focus model changes.
        self.change_keyboard_focus(resource);
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }

    /// Surface id of the toplevel currently holding keyboard focus, if any.
    /// Returns `None` when no surface is focused or the focus is not a
    /// toplevel. Used by the keybind dispatcher to target the focused window.
    pub fn focused_toplevel_id(&self) -> Option<ass_core::window::WindowId> {
        let f = self.state.keyboard_focus;
        if f.is_null() {
            return None;
        }
        self.state
            .live_surfaces()
            .map(|p| unsafe { &*p })
            .find(|s| s.resource == f && !s.xdg_toplevel.is_null())
            .map(|s| s.window.id)
    }

    /// The last cursor shape requested by the focused client
    /// (`wp_cursor_shape_device_v1.set_shape`), or 0 for the default arrow.
    /// The renderer consults this to pick which cursor to paint.
    pub fn cursor_shape(&self) -> u32 {
        self.state.cursor_shape
    }

    /// Cycle keyboard focus among mapped, non-minimized toplevels in creation
    /// order. `forward` selects the next surface, `false` the previous. No-op
    /// if fewer than two eligible toplevels exist. Backs the `CycleFocus` /
    /// `CycleFocusBack` key bindings.
    pub fn cycle_focus(&mut self, forward: bool) {
        let visible = self.visible();
        let ids: Vec<ass_core::window::WindowId> = self
            .state
            .live_surfaces()
            .map(|p| unsafe { &*p })
            .filter(|s| {
                !s.xdg_toplevel.is_null()
                    && s.mapped
                    && !s.window.minimized
                    && visible.contains(&s.window.id)
            })
            .map(|s| s.window.id)
            .collect();
        if ids.len() < 2 {
            return;
        }
        let next = match self
            .focused_toplevel_id()
            .and_then(|id| ids.iter().position(|x| *x == id))
        {
            Some(i) => {
                let n = ids.len();
                if forward {
                    ids[(i + 1) % n]
                } else {
                    ids[(i + n - 1) % n]
                }
            }
            None => ids[0],
        };
        self.focus_surface_by_id(next);
    }

    /// Switch to an adjacent workspace on the focused output (ADR-0025). The
    /// visible set changes on the next frame; if the focused toplevel is no
    /// longer visible, keyboard focus is dropped (a `wl_keyboard.leave` is
    /// posted) so keystrokes do not route to a hidden window.
    pub fn switch_workspace(&mut self, dir: ass_core::workspace::Switch) {
        self.state.workspaces.switch(self.state.output, dir);
        self.drop_focus_if_hidden();
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }

    /// Switch directly to a workspace by id on the output that owns it. Same
    /// focus-drop contract as [`switch_workspace`](Self::switch_workspace).
    pub fn switch_workspace_to(&mut self, id: ass_core::workspace::WorkspaceId) {
        self.state.workspaces.switch_to(id);
        self.drop_focus_if_hidden();
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }

    /// Move a toplevel to a workspace by id (ADR-0025). If the target is not
    /// the current workspace, the window leaves the visible set. No-op if the
    /// window or workspace is unknown.
    pub fn move_to_workspace(
        &mut self,
        window_id: ass_core::window::WindowId,
        workspace: ass_core::workspace::WorkspaceId,
    ) {
        self.state.workspaces.move_toplevel(window_id, workspace);
        self.drop_focus_if_hidden();
    }

    /// The workspace/output snapshot for the IPC and chrome (ADR-0025/0027).
    pub fn workspace_snapshot(&self) -> ass_core::workspace::WorkspaceSnapshot {
        self.state.workspaces.snapshot()
    }

    /// Whether the current workspace is in tiled mode (ADR-0024).
    pub fn tiling(&self) -> bool {
        self.state
            .workspaces
            .current_workspace_tiled(self.state.output)
    }

    /// Toggle the current workspace between tiled and floating (ADR-0024).
    /// On, the workspace's windows are marked `Tiled` and laid out next
    /// `apply_tiling`; off, they revert to `Floating` and keep their current
    /// geometry. Layout targets are cleared so the next apply reconfigures.
    pub fn set_tiling(&mut self, on: bool) {
        if let Some(wid) = self.state.workspaces.current_workspace(self.state.output) {
            self.state.workspaces.set_tiled(wid, on);
        }
        let role = if on {
            ass_core::layout::LayoutRole::Tiled
        } else {
            ass_core::layout::LayoutRole::Floating
        };
        for id in self.state.workspaces.visible_toplevels() {
            let rec = self.find_surface_by_window_id(id);
            if rec.is_null() {
                continue;
            }
            unsafe {
                (*rec).window.layout_role = role;
                (*rec).layout_target = None;
            }
        }
        log::info!(
            "[server] workspace tiling {}",
            if on { "on" } else { "off" }
        );
    }

    /// Replace the window rules (ADR-0026). Called at startup and on config
    /// reload. Rules apply to windows mapped after they are set.
    pub fn set_window_rules(&mut self, rules: Vec<ass_core::window_rule::WindowRule>) {
        self.state.window_rules = rules;
    }

    /// Replace the tiling layout parameters (gaps, master ratio) from the
    /// config (ADR-0024/0026). Applied on the next `apply_tiling`.
    pub fn set_layout_params(&mut self, params: ass_core::layout::LayoutParams) {
        self.state.layout_params = params;
    }

    /// Replace the focused output's geometry (ADR-0028). The backend calls
    /// this on resize; the tiling work-area is the geometry's logical rect.
    /// Re-sends the wl_output geometry/mode/scale/done sequence to every bound
    /// client so they update their scale and surface buffer scale.
    pub fn set_output_geometry(&mut self, geo: ass_core::output::OutputGeometry) {
        self.state.output_geometry = geo;
        // Resend to every bound wl_output resource.
        let resources: Vec<*mut ffi::wl_resource> = self
            .state
            .output_resources
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .collect();
        for res in resources {
            unsafe { send_output_geometry(res, self.state.as_ref() as *const State as *mut State) };
        }
        // Refresh xdg-output logical extents too.
        self.resend_xdg_outputs();
        // Re-send fractional-scale hints so HiDPI-aware clients resize buffers.
        unsafe {
            extensions::resend_fractional_scales(self.state.as_ref() as *const State as *mut State)
        };
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
    }

    /// The focused output's logical rect (ADR-0028). The chrome-aware
    /// tiling work-area is this inset by the chrome's reserved edges.
    pub fn output_logical_rect(&self) -> ass_core::Rect {
        self.state.output_geometry.logical_rect()
    }

    /// Resend the `zxdg_output_v1` logical geometry events to every bound
    /// xdg-output resource. Called whenever the output's logical extents
    /// change (resize / scale / transform) so clients reposition. Pairs with
    /// the wl_output geometry re-send in [`Server::set_output_geometry`].
    fn resend_xdg_outputs(&self) {
        let rect = self.state.output_geometry.logical_rect();
        let resources: Vec<*mut ffi::wl_resource> = self
            .state
            .xdg_output_resources
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .collect();
        for res in resources {
            // SAFETY: res is a live zxdg_output_v1 resource we own. The logical
            // position/size events take i32 pairs; done (v2+) flushes the batch.
            unsafe {
                let version = ffi::wl_resource_get_version(res);
                ffi::wl_resource_post_event(
                    res,
                    ffi::ZXDG_OUTPUT_V1_LOGICAL_POSITION,
                    rect.origin.x,
                    rect.origin.y,
                );
                ffi::wl_resource_post_event(
                    res,
                    ffi::ZXDG_OUTPUT_V1_LOGICAL_SIZE,
                    rect.size.w,
                    rect.size.h,
                );
                if version >= 2 {
                    ffi::wl_resource_post_event(res, ffi::ZXDG_OUTPUT_V1_DONE);
                }
            }
        }
    }

    /// The live outputs (connector + geometry) for the IPC and chrome. One
    /// entry per workspace-model output, paired with the focused geometry
    /// (multi-output geometry tracking lands with M7).
    pub fn output_infos(&self) -> Vec<ass_core::output::OutputInfo> {
        self.state
            .workspaces
            .outputs()
            .iter()
            .map(|o| ass_core::output::OutputInfo {
                connector: o.connector.clone(),
                geometry: self.state.output_geometry,
            })
            .collect()
    }

    /// Apply the master-stack tiling policy to the current workspace's
    /// windows when tiled mode is on (ADR-0024). Runs the layout over
    /// `work_area` and reconfigures only the windows whose target rect moved,
    /// so steady state sends no configure events. No-op when tiling is off.
    /// Apply the master-stack tiling policy to the current workspace's
    /// windows when tiled mode is on (ADR-0024). Runs the layout over
    /// `work_area` (the chrome-aware logical rect) and reconfigures only the
    /// windows whose target rect moved, so steady state sends no configure
    /// events. No-op when tiling is off.
    pub fn apply_tiling(&mut self, work_area: ass_core::Rect) {
        if !self
            .state
            .workspaces
            .current_workspace_tiled(self.state.output)
        {
            return;
        }
        let tiled_ids: Vec<ass_core::window::WindowId> = self
            .state
            .workspaces
            .visible_toplevels()
            .into_iter()
            .filter(|id| {
                let rec = self.find_surface_by_window_id(*id);
                !rec.is_null()
                    && unsafe { (*rec).window.layout_role == ass_core::layout::LayoutRole::Tiled }
            })
            .collect();
        let rects = ass_core::layout::MasterStack.layout(
            work_area,
            tiled_ids.len(),
            &self.state.layout_params,
        );
        let mut flushed = false;
        for (id, rect) in tiled_ids.iter().zip(rects.iter()) {
            let rec = self.find_surface_by_window_id(*id);
            if rec.is_null() {
                continue;
            }
            unsafe {
                if (*rec).xdg_toplevel.is_null() || !(*rec).mapped {
                    continue;
                }
                if (*rec).layout_target == Some(*rect) {
                    continue; // already at the target; do not reconfigure
                }
                (*rec).position = rect.origin;
                (*rec).window.position = rect.origin;
                (*rec).window.size = rect.size;
                (*rec).window.layout_role = ass_core::layout::LayoutRole::Tiled;
                (*rec).layout_target = Some(*rect);
                reconfigure_with_size(rec, rect.size.w, rect.size.h);
                if !flushed {
                    ffi::wl_display_flush_clients(self.state.display);
                    flushed = true;
                }
            }
        }
    }

    /// If the keyboard-focused surface is not on a visible workspace, clear
    /// focus (post leave, deactivate). Idempotent.
    fn drop_focus_if_hidden(&mut self) {
        let visible = self.visible();
        let Some(wid) = self.focused_toplevel_id() else {
            return;
        };
        if !visible.contains(&wid) {
            self.change_keyboard_focus(std::ptr::null_mut());
        }
    }

    fn pointer_motion(&mut self, x: f32, y: f32) {
        // Relative delta from the previous motion event (for relative-pointer
        // clients). Computed before pointer_x/y are overwritten.
        let dx = x - self.state.pointer_x;
        let dy = y - self.state.pointer_y;
        self.state.pointer_x = x;
        self.state.pointer_y = y;
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
            self.post_motion_to_focus();
            return;
        }
        let focus = self.hit_test_focus(x, y);
        if focus != self.state.pointer_focus {
            self.change_pointer_focus(focus);
        }
        // Post motion to whichever client now holds focus.
        self.post_motion_to_focus();
    }

    fn pointer_button(&mut self, button: u32, state: ass_core::input::ButtonState) {
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

        // Floating windows expose an invisible inside border for direct
        // resize. This runs before client button delivery so dragging a border
        // never activates a widget under the same pixels. Tiled, maximized,
        // and fullscreen windows keep their layout-owned geometry.
        const BORDER: f32 = 8.0;
        const BTN_LEFT: u32 = 0x110;
        if state.is_pressed() && button == BTN_LEFT && self.state.interactive.is_none() {
            if let Some((rec, edges)) =
                self.resize_target_at(self.state.pointer_x, self.state.pointer_y, BORDER)
            {
                let resource = unsafe { (*rec).resource };
                let id = unsafe { (*rec).window.id };
                self.change_keyboard_focus(resource);
                unsafe {
                    (*rec).window.state.resizing = true;
                    reconfigure_with_state(rec);
                }
                self.state.interactive = Some(ass_core::window::Interactive::Resize {
                    window_id: id,
                    edges,
                    origin: (self.state.pointer_x, self.state.pointer_y),
                    start_position: unsafe { (*rec).position },
                    start_size: ass_core::Size {
                        w: unsafe { (*rec).width },
                        h: unsafe { (*rec).height },
                    },
                });
                self.state.compositor_pointer_grab = true;
                return;
            }
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
        self.state.last_button_serial = serial;
        let state_u32 = if state.is_pressed() { 1u32 } else { 0u32 };
        let focus = self.state.pointer_focus;
        let focus_client = unsafe { ffi::wl_resource_get_client(focus) };
        for p in self.iter_focus_pointers(focus_client) {
            unsafe {
                ffi::wl_resource_post_event(
                    p,
                    ffi::WL_POINTER_BUTTON,
                    serial,
                    0u32,
                    button,
                    state_u32,
                );
            }
        }
    }

    /// Synthesized leave: e.g. when the host pointer leaves the nested window.
    fn pointer_leave_all(&mut self) {
        self.change_pointer_focus(std::ptr::null_mut());
    }

    fn keyboard_key(
        &mut self,
        evdev_code: u32,
        state: ass_core::input::ButtonState,
        keymap: &ass_core::keybind::Keymap,
    ) -> Option<ass_core::keybind::Action> {
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
        // A key that matches a global binding on press is consumed (not posted
        // to the focused client) and its action returned for the caller to
        // dispatch. Modifier-only keys never match, so modifiers still post.
        let matched = if state.is_pressed() {
            keymap.match_key(ass_core::input::Mods(outcome.depressed), outcome.keysym)
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
    /// each surface's authoritative `position` (assigned at map time); later
    /// surfaces in the surfaces Vec are considered "above" earlier ones.
    fn hit_test_focus(&self, x: f32, y: f32) -> *mut ffi::wl_resource {
        let visible = self.visible();
        let mut hit: *mut ffi::wl_resource = std::ptr::null_mut();
        for p in self.state.live_surfaces() {
            let s = unsafe { &*p };
            if !s.mapped
                || s.xdg_toplevel.is_null()
                || s.window.minimized
                || !visible.contains(&s.window.id)
            {
                continue;
            }
            let sx = s.position.x as f32;
            let sy = s.position.y as f32;
            let sw = s.width as f32;
            let sh = s.height as f32;
            if x >= sx && y >= sy && x < sx + sw && y < sy + sh {
                // Higher-index surfaces paint on top; keep iterating so the
                // topmost match wins.
                hit = s.resource;
            }
        }
        hit
    }

    /// Topmost floating toplevel whose inside border contains `(x, y)`.
    fn resize_target_at(
        &self,
        x: f32,
        y: f32,
        border: f32,
    ) -> Option<(*mut SurfaceRec, ass_core::window::ResizeEdges)> {
        let visible = self.visible();
        let mut hit = None;
        for p in self.state.live_surfaces() {
            let s = unsafe { &*p };
            if !s.mapped
                || s.xdg_toplevel.is_null()
                || s.window.minimized
                || s.window.state.maximized
                || s.window.state.fullscreen
                || s.window.layout_role != ass_core::layout::LayoutRole::Floating
                || !visible.contains(&s.window.id)
            {
                continue;
            }
            let mut window = s.window.clone();
            window.position = s.position;
            window.size = ass_core::Size {
                w: s.width,
                h: s.height,
            };
            let edges = window.resize_edges_at(x, y, border);
            if !edges.is_none() {
                hit = Some((p, edges));
            }
        }
        hit
    }

    /// End an interactive move/resize, clearing the protocol resizing state
    /// and notifying the client once after the final geometry.
    fn finish_interactive(&mut self) {
        if let Some(ass_core::window::Interactive::Resize { window_id, .. }) =
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

    /// Transition focus: post leave to the old client's pointer resources and
    /// enter to the new client's, with a fresh serial.
    fn change_pointer_focus(&mut self, new_focus: *mut ffi::wl_resource) {
        let serial = unsafe { ffi::wl_display_next_serial(self.state.display) };
        let old = self.state.pointer_focus;
        let x = ffi::wl_fixed_from_f32(self.state.pointer_x);
        let y = ffi::wl_fixed_from_f32(self.state.pointer_y);

        if !old.is_null() {
            let old_client = unsafe { ffi::wl_resource_get_client(old) };
            for p in self.iter_focus_pointers(old_client) {
                unsafe {
                    ffi::wl_resource_post_event(p, ffi::WL_POINTER_LEAVE, serial, old);
                }
            }
        }
        self.state.pointer_focus = new_focus;
        if !new_focus.is_null() {
            let new_client = unsafe { ffi::wl_resource_get_client(new_focus) };
            for p in self.iter_focus_pointers(new_client) {
                unsafe {
                    ffi::wl_resource_post_event(p, ffi::WL_POINTER_ENTER, serial, new_focus, x, y);
                }
            }
        }
    }

    fn post_motion_to_focus(&self) {
        if self.state.pointer_focus.is_null() {
            return;
        }
        let focus = self.state.pointer_focus;
        let focus_client = unsafe { ffi::wl_resource_get_client(focus) };
        let x = ffi::wl_fixed_from_f32(self.state.pointer_x);
        let y = ffi::wl_fixed_from_f32(self.state.pointer_y);
        for p in self.iter_focus_pointers(focus_client) {
            unsafe {
                ffi::wl_resource_post_event(p, ffi::WL_POINTER_MOTION, 0u32, x, y);
            }
        }
    }

    /// Post `zwp_relative_pointer_v1.relative_motion` to every bound
    /// relative-pointer resource owned by the focused client. `dx`/`dy` are the
    /// unaccelerated pixel deltas since the last motion event; the protocol
    /// also wants accelerated deltas, which we do not model, so we send both
    /// fields equal to the unaccelerated value.
    fn post_relative_motion(&self, dx: f32, dy: f32) {
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

    /// Post `wl_pointer.axis` (scroll) to the focused client. `dx`/`dy` are
    /// horizontal/vertical scroll deltas in logical-pixel units (libinput's
    /// convention: discrete wheel clicks are ~10.0). Each axis with a non-zero
    /// delta is posted with `wl_pointer.axis`; for discrete deltas we also
    /// post `wl_pointer.axis_discrete` so scroll-stepping clients (browsers,
    /// terminals) step the right number of lines. A `frame` event (v5+) bundles
    /// the axis events for clients that consume them together.
    fn pointer_axis(&mut self, time: u32, dx: f32, dy: f32) {
        if self.state.pointer_focus.is_null() || (dx == 0.0 && dy == 0.0) {
            return;
        }
        let focus = self.state.pointer_focus;
        let focus_client = unsafe { ffi::wl_resource_get_client(focus) };
        // Discrete scroll: a delta of ~10.0 is one wheel click (libinput's
        // default DEG_PER_UNIT-derived value). Round to the nearest integer
        // click count per axis.
        let discrete_x = (dx.abs() / 10.0).round() as i32;
        let discrete_y = (dy.abs() / 10.0).round() as i32;
        for p in self.iter_focus_pointers(focus_client) {
            let ver = unsafe { ffi::wl_resource_get_version(p) };
            unsafe {
                if dx != 0.0 {
                    ffi::wl_resource_post_event(
                        p,
                        ffi::WL_POINTER_AXIS,
                        time,
                        ffi::WL_POINTER_AXIS_HORIZONTAL_SCROLL,
                        ffi::wl_fixed_from_f32(dx),
                    );
                    if ver >= 5 && discrete_x != 0 {
                        ffi::wl_resource_post_event(
                            p,
                            ffi::WL_POINTER_AXIS_DISCRETE,
                            ffi::WL_POINTER_AXIS_HORIZONTAL_SCROLL,
                            discrete_x,
                        );
                    }
                }
                if dy != 0.0 {
                    ffi::wl_resource_post_event(
                        p,
                        ffi::WL_POINTER_AXIS,
                        time,
                        ffi::WL_POINTER_AXIS_VERTICAL_SCROLL,
                        ffi::wl_fixed_from_f32(dy),
                    );
                    if ver >= 5 && discrete_y != 0 {
                        ffi::wl_resource_post_event(
                            p,
                            ffi::WL_POINTER_AXIS_DISCRETE,
                            ffi::WL_POINTER_AXIS_VERTICAL_SCROLL,
                            discrete_y,
                        );
                    }
                }
                // Source + stop + frame (v5+) let clients distinguish wheel
                // from touch/finger scroll and coalesce events.
                if ver >= 5 {
                    ffi::wl_resource_post_event(
                        p,
                        ffi::WL_POINTER_AXIS_SOURCE,
                        ffi::WL_POINTER_AXIS_SOURCE_WHEEL,
                    );
                    if dx != 0.0 {
                        ffi::wl_resource_post_event(
                            p,
                            ffi::WL_POINTER_AXIS_STOP,
                            time,
                            ffi::WL_POINTER_AXIS_HORIZONTAL_SCROLL,
                        );
                    }
                    if dy != 0.0 {
                        ffi::wl_resource_post_event(
                            p,
                            ffi::WL_POINTER_AXIS_STOP,
                            time,
                            ffi::WL_POINTER_AXIS_VERTICAL_SCROLL,
                        );
                    }
                    ffi::wl_resource_post_event(p, ffi::WL_POINTER_FRAME);
                }
            }
        }
    }

    /// Post `wl_touch.down`: a new contact on the focused surface. Touch
    /// events go to the pointer-focused client (touch and pointer share a
    /// seat). `id` is the contact id (0..). The `time` is the same monotonic
    /// millisecond clock pointer events use.
    fn touch_down(&mut self, time: u32, id: i32, x: f32, y: f32) {
        if self.state.pointer_focus.is_null() {
            return;
        }
        let focus = self.state.pointer_focus;
        let serial = unsafe { ffi::wl_display_next_serial(self.state.display) };
        let client = unsafe { ffi::wl_resource_get_client(focus) };
        let fx = ffi::wl_fixed_from_f32(x);
        let fy = ffi::wl_fixed_from_f32(y);
        for t in self.iter_client_touch(client) {
            unsafe {
                ffi::wl_resource_post_event(t, ffi::WL_TOUCH_DOWN, serial, time, focus, id, fx, fy);
            }
        }
    }

    /// Post `wl_touch.motion` for an existing contact.
    fn touch_motion(&mut self, time: u32, id: i32, x: f32, y: f32) {
        if self.state.pointer_focus.is_null() {
            return;
        }
        let client = unsafe { ffi::wl_resource_get_client(self.state.pointer_focus) };
        let fx = ffi::wl_fixed_from_f32(x);
        let fy = ffi::wl_fixed_from_f32(y);
        for t in self.iter_client_touch(client) {
            unsafe {
                ffi::wl_resource_post_event(t, ffi::WL_TOUCH_MOTION, time, id, fx, fy);
            }
        }
    }

    /// Post `wl_touch.up`.
    fn touch_up(&mut self, time: u32, id: i32) {
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
    fn touch_frame(&self) {
        if self.state.pointer_focus.is_null() {
            return;
        }
        let client = unsafe { ffi::wl_resource_get_client(self.state.pointer_focus) };
        for t in self.iter_client_touch(client) {
            unsafe { ffi::wl_resource_post_event(t, ffi::WL_TOUCH_FRAME) };
        }
    }

    /// Post `wl_touch.cancel`: all active contacts invalidated.
    fn touch_cancel(&self) {
        if self.state.pointer_focus.is_null() {
            return;
        }
        let client = unsafe { ffi::wl_resource_get_client(self.state.pointer_focus) };
        for t in self.iter_client_touch(client) {
            unsafe { ffi::wl_resource_post_event(t, ffi::WL_TOUCH_CANCEL) };
        }
    }

    fn iter_client_touch(&self, client: *mut ffi::wl_client) -> Vec<*mut ffi::wl_resource> {
        self.state
            .touch_resources
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .filter(|p| unsafe { ffi::wl_resource_get_client(*p) == client })
            .collect()
    }

    /// Transition keyboard focus: post leave to the old client's keyboard
    /// resources and enter to the new client's. The "keys" array passed to
    /// `enter` is empty — M1 does not track currently-held keys for resend on
    /// refocus; that's a polish item once the keyboard pipeline stabilizes.
    /// Also flips the `activated` toplevel state bit on the old and new
    /// surfaces so clients update their title-bar chrome to match focus.
    fn change_keyboard_focus(&mut self, new_focus: *mut ffi::wl_resource) {
        if !new_focus.is_null() {
            self.raise_toplevel(new_focus);
        }
        if new_focus == self.state.keyboard_focus {
            return;
        }
        let serial = unsafe { ffi::wl_display_next_serial(self.state.display) };
        let old = self.state.keyboard_focus;
        let empty = ffi::wl_array::empty();

        // Notify text-input clients of the focus transition (IME enter/leave).
        unsafe {
            extensions::text_input_focus_changed(
                self.state.as_ref() as *const State as *mut State,
                old,
                new_focus,
            )
        };

        if !old.is_null() {
            let old_client = unsafe { ffi::wl_resource_get_client(old) };
            for k in self.iter_focus_keyboards(old_client) {
                unsafe {
                    ffi::wl_resource_post_event(k, ffi::WL_KEYBOARD_LEAVE, serial, old);
                }
            }
            // Clear activated on the surface losing keyboard focus.
            self.set_activated_for_surface(old, false);
        }
        self.state.keyboard_focus = new_focus;
        if !new_focus.is_null() {
            let new_client = unsafe { ffi::wl_resource_get_client(new_focus) };
            for k in self.iter_focus_keyboards(new_client) {
                unsafe {
                    ffi::wl_resource_post_event(
                        k,
                        ffi::WL_KEYBOARD_ENTER,
                        serial,
                        new_focus,
                        &empty as *const ffi::wl_array as *mut ffi::wl_array,
                    );
                }
            }
            // Set activated on the surface gaining keyboard focus.
            self.set_activated_for_surface(new_focus, true);
        }
    }

    /// Move a focused toplevel to the top of the stacking order while keeping
    /// every live record's destroy-slot index correct. Raw `SurfaceRec`
    /// allocations do not move; only their pointers in the Vec do.
    fn raise_toplevel(&mut self, resource: *mut ffi::wl_resource) {
        let Some(pos) = self.state.surfaces.iter().position(|p| {
            !p.is_null() && unsafe { (**p).resource == resource && !(**p).xdg_toplevel.is_null() }
        }) else {
            return;
        };
        if self.state.surfaces[pos + 1..].iter().all(|p| p.is_null()) {
            return;
        }
        let rec = self.state.surfaces.remove(pos);
        self.state.surfaces.push(rec);
        for (index, ptr) in self.state.surfaces.iter().copied().enumerate().skip(pos) {
            if !ptr.is_null() {
                unsafe { (*ptr).index = index };
            }
        }
    }

    /// Flip the `activated` bit on a toplevel and reconfigure it. No-op if
    /// the surface has no toplevel role or the bit is already in the
    /// requested state.
    fn set_activated_for_surface(&mut self, surface: *mut ffi::wl_resource, activated: bool) {
        // Find the SurfaceRec backing this surface resource by walking the
        // Vec. The search is O(N) but N is small and this only fires on
        // focus transitions, not per frame.
        for p in self.state.live_surfaces() {
            let s = unsafe { &mut *p };
            if s.resource == surface && !s.xdg_toplevel.is_null() {
                if s.window.state.activated == activated {
                    return;
                }
                s.window.state.activated = activated;
                // Focusing a minimized toplevel restores it: clear the flag so
                // the renderer and hit-test pick it up again.
                if activated {
                    s.window.minimized = false;
                }
                unsafe { reconfigure_with_state(s as *mut SurfaceRec) };
                return;
            }
        }
    }

    /// Borrowed slice of live pointer resource pointers belonging to `client`.
    /// The slice is rebuilt per call (no lifetime issues across re-entry).
    fn iter_focus_pointers(&self, client: *mut ffi::wl_client) -> Vec<*mut ffi::wl_resource> {
        self.state
            .pointer_resources
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .filter(|p| unsafe { ffi::wl_resource_get_client(*p) == client })
            .collect()
    }

    fn iter_focus_keyboards(&self, client: *mut ffi::wl_client) -> Vec<*mut ffi::wl_resource> {
        self.state
            .keyboard_resources
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .filter(|p| unsafe { ffi::wl_resource_get_client(*p) == client })
            .collect()
    }
}

impl Drop for Server {
    fn drop(&mut self) {
        unsafe {
            // `wl_display_destroy` fires each live resource's destroy notify;
            // `surface_resource_destroy` frees each surface's box and nulls its
            // slot. This MUST run before the orphan-reclaim loop below — the
            // opposite order frees the boxes while the wl_resources still hold
            // dangling user_data pointers, so the notifys fired here would
            // dereference freed memory (use-after-free, observed as a flaky
            // shutdown segfault roughly one run in three).
            ffi::wl_display_destroy(self.state.display);
            // Reclaim any orphaned boxes whose destroy notify never fired
            // (slot still non-null). Boxes freed via their notify have a null
            // slot and are skipped, so there is no double-free.
            for &p in &self.state.surfaces {
                if !p.is_null() {
                    drop(Box::from_raw(p));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Server::new` brings up the display, binds an auto-named socket, and
    /// returns a non-empty socket name. The socket lives in `XDG_RUNTIME_DIR`
    /// (libwayland's convention) and is removed by `wl_display_destroy`.
    #[test]
    fn server_new_creates_socket() {
        // Skip on environments without an XDG runtime dir (CI sandboxes).
        if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
            eprintln!("skipping: XDG_RUNTIME_DIR not set");
            return;
        }
        let server = Server::new().expect("Server::new");
        let socket = server.socket();
        assert!(!socket.is_empty(), "socket name must not be empty");
        let path = std::env::var("XDG_RUNTIME_DIR").unwrap() + "/" + socket;
        assert!(
            std::path::Path::new(&path).exists(),
            "socket file missing: {path}"
        );
        // Drop runs destroy and should remove the socket.
        drop(server);
        assert!(
            !std::path::Path::new(&path).exists(),
            "socket file should be removed after drop: {path}"
        );
    }
}

/// Errors bringing up the server.
#[derive(Debug, thiserror::Error)]
pub enum ServerError {
    #[error("wl_display_create failed")]
    DisplayCreate,
    #[error("wl_display_add_socket_auto failed")]
    Socket,
    #[error("wl_display_init_shm failed")]
    Shm,
}

// ----- wl_compositor ------------------------------------------------------

static COMPOSITOR_IMPL: ffi::wl_compositor_interface_impl = ffi::wl_compositor_interface_impl {
    create_surface: compositor_create_surface,
    create_region: compositor_create_region,
};

unsafe extern "C" fn compositor_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(client, &ffi::wl_compositor_interface, version as c_int, id);
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &COMPOSITOR_IMPL as *const _ as *const c_void,
        data,
        None,
    );
}

unsafe extern "C" fn compositor_create_surface(
    client: *mut ffi::wl_client,
    compositor: *mut ffi::wl_resource,
    id: u32,
) {
    let state = ffi::wl_resource_get_user_data(compositor) as *mut State;
    let ver = ffi::wl_resource_get_version(compositor);
    let surface = ffi::wl_resource_create(client, &ffi::wl_surface_interface, ver, id);
    if surface.is_null() {
        return;
    }
    let rec = Box::into_raw(Box::new(SurfaceRec::new(surface)));
    (*rec).state = state;
    (*rec).index = (*state).surfaces.len();
    ffi::wl_resource_set_implementation(
        surface,
        &SURFACE_IMPL as *const _ as *const c_void,
        rec as *mut c_void,
        Some(surface_resource_destroy),
    );
    (*state).surfaces.push(rec);
}

unsafe extern "C" fn compositor_create_region(
    client: *mut ffi::wl_client,
    compositor: *mut ffi::wl_resource,
    id: u32,
) {
    let ver = ffi::wl_resource_get_version(compositor);
    let region = ffi::wl_resource_create(client, &ffi::wl_region_interface, ver, id);
    if region.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        region,
        &REGION_IMPL as *const _ as *const c_void,
        std::ptr::null_mut(),
        None,
    );
}

// ----- wl_surface ---------------------------------------------------------

static SURFACE_IMPL: ffi::wl_surface_interface_impl = ffi::wl_surface_interface_impl {
    destroy: surface_destroy,
    attach: surface_attach,
    damage: surface_damage,
    frame: surface_frame,
    set_opaque_region: surface_noop_region,
    set_input_region: surface_noop_region,
    commit: surface_commit,
    set_buffer_transform: surface_set_buffer_transform,
    set_buffer_scale: surface_set_buffer_scale,
    damage_buffer: surface_damage_buffer,
};

unsafe extern "C" fn surface_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn surface_attach(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    buffer: *mut ffi::wl_resource,
    _x: i32,
    _y: i32,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    (*rec).pending_buffer = buffer;
}

unsafe extern "C" fn surface_commit(_client: *mut ffi::wl_client, resource: *mut ffi::wl_resource) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;

    // xdg-shell initial configure: on the first commit of a surface that has an
    // xdg role, send a configure and wait for the client to ack and attach a
    // buffer. The initial commit carries no buffer, so mapping happens on a
    // later commit.
    if !(*rec).xdg_surface.is_null() && !(*rec).xdg_configured {
        if !(*rec).xdg_toplevel.is_null() {
            let mut states = ffi::wl_array::empty();
            ffi::wl_resource_post_event(
                (*rec).xdg_toplevel,
                ffi::XDG_TOPLEVEL_CONFIGURE,
                0i32,
                0i32,
                &mut states as *mut ffi::wl_array,
            );
        }
        let serial = ffi::wl_display_next_serial((*rec).display);
        ffi::wl_resource_post_event((*rec).xdg_surface, ffi::XDG_SURFACE_CONFIGURE, serial);
        (*rec).xdg_configured = true;
    }

    let was_mapped = (*rec).mapped;
    let buffer = (*rec).pending_buffer;
    // The pending transform and scale are surfaced to the renderer via
    // `SurfaceGeometry` (see toplevel_*_frames below); the renderer applies
    // them at composite time.
    // Rotate the pending damage into committed_damage for the renderer to
    // read this frame; clear pending so the next commit starts fresh.
    // Bounding boxes are clamped to surface bounds when surfaced.
    (*rec).committed_damage = std::mem::take(&mut (*rec).pending_damage);
    if !buffer.is_null() {
        let is_dmabuf = ffi::wl_resource_instance_of(
            buffer,
            &ffi::wl_buffer_interface,
            &WL_BUFFER_IMPL as *const _ as *const c_void,
        ) != 0;

        if is_dmabuf {
            // Zero-copy: hold the dma-buf-backed buffer and sample its fd
            // directly. Release the previously held one (if any). Do NOT send
            // wl_buffer.release until the buffer is replaced or the surface
            // unmaps, since flux re-imports the fd each frame.
            if !(*rec).dmabuf_buffer.is_null() && (*rec).dmabuf_buffer != buffer {
                ffi::wl_resource_post_event((*rec).dmabuf_buffer, ffi::WL_BUFFER_RELEASE);
            }
            let db = ffi::wl_resource_get_user_data(buffer) as *const DmabufBuffer;
            if !db.is_null() && (*db).have_plane {
                (*rec).dmabuf_buffer = buffer;
                (*rec).content_is_dmabuf = true;
                (*rec).width = (*db).width;
                (*rec).height = (*db).height;
                (*rec).generation = (*rec).generation.wrapping_add(1);
                (*rec).mapped = true;
            }
            (*rec).pending_buffer = std::ptr::null_mut();
        } else {
            // shm: copy the contents out into our own tightly packed BGRA store
            // and release the buffer immediately so the client can reuse it.
            let shm = ffi::wl_shm_buffer_get(buffer);
            if !shm.is_null() {
                let w = ffi::wl_shm_buffer_get_width(shm);
                let h = ffi::wl_shm_buffer_get_height(shm);
                let stride = ffi::wl_shm_buffer_get_stride(shm) as usize;
                let format = ffi::wl_shm_buffer_get_format(shm);
                let src = ffi::wl_shm_buffer_get_data(shm) as *const u8;
                if !src.is_null() && w > 0 && h > 0 {
                    let tight = (w as usize) * 4;
                    let mut pixels = vec![0u8; tight * h as usize];
                    ffi::wl_shm_buffer_begin_access(shm);
                    for row in 0..h as usize {
                        std::ptr::copy_nonoverlapping(
                            src.add(row * stride),
                            pixels.as_mut_ptr().add(row * tight),
                            tight,
                        );
                    }
                    ffi::wl_shm_buffer_end_access(shm);
                    // XRGB8888 has undefined alpha; force opaque.
                    if format == 1 {
                        let mut i = 3;
                        while i < pixels.len() {
                            pixels[i] = 0xff;
                            i += 4;
                        }
                    }
                    (*rec).pixels = pixels;
                    (*rec).content_is_dmabuf = false;
                    (*rec).width = w;
                    (*rec).height = h;
                    (*rec).generation = (*rec).generation.wrapping_add(1);
                    (*rec).mapped = true;
                }
            }
            ffi::wl_resource_post_event(buffer, ffi::WL_BUFFER_RELEASE);
            (*rec).pending_buffer = std::ptr::null_mut();
        }
    }

    if (*rec).mapped && !was_mapped {
        // First map of this surface: assign a placeholder position. M3's
        // window manager replaces this with a real policy (tile, place under
        // pointer, remember last position). Until then, a diagonal cascade
        // keeps multiple toplevels from perfectly overlapping.
        let count = if (*rec).state.is_null() {
            0
        } else {
            (*(*rec).state)
                .live_surfaces()
                .filter(|p| unsafe { !(**p).xdg_toplevel.is_null() && (**p).mapped })
                .count()
        };
        let idx = count.min(8) as i32;
        (*rec).position = ass_core::Point {
            x: 60 + idx * 32,
            y: 60 + idx * 32,
        };
        (*rec).window.position = (*rec).position;
        (*rec).window.size = ass_core::Size {
            w: (*rec).width,
            h: (*rec).height,
        };
        log::info!(
            "[server] surface mapped at {:?}: {}x{}",
            (*rec).position,
            (*rec).width,
            (*rec).height
        );
        // Place the new toplevel on the focused output's current workspace
        // (ADR-0025). The model's trailing-empty invariant appends a fresh
        // workspace if this one fills the last.
        if !(*rec).state.is_null() {
            let id = (*rec).window.id;
            let st = &mut *(*rec).state;
            if let Some(wid) = st.workspaces.current_workspace(st.output) {
                st.workspaces.place_toplevel(wid, id);
            }
            // Apply the first matching window rule (ADR-0026): a workspace
            // move and/or a forced layout role. `app_id`/`title` may not be
            // set yet at first map; rules re-evaluating on title changes is a
            // follow-up.
            let app_id = (*rec).window.app_id.clone();
            let title = (*rec).window.title.clone();
            if let Some(rule) = st
                .window_rules
                .iter()
                .find(|r| r.matches(app_id.as_deref(), title.as_deref()))
            {
                if let Some(ws_idx1) = rule.workspace {
                    let idx = (ws_idx1 as usize).saturating_sub(1);
                    if let Some(o) = st.workspaces.output(st.output) {
                        if let Some(&target) = o.workspaces.get(idx) {
                            st.workspaces.move_toplevel(id, target);
                        }
                    }
                }
                if let Some(role) = rule.role {
                    (*rec).window.layout_role = role;
                }
            }
        }
        // Live-update the foreign-toplevel list so taskbars see the new window.
        if !(*rec).state.is_null() {
            extensions::foreign_toplevel_added(rec, (*rec).state);
        }
    }
}

unsafe extern "C" fn surface_frame(
    client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    callback_id: u32,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    let cb = ffi::wl_resource_create(client, &ffi::wl_callback_interface, 1, callback_id);
    if !cb.is_null() {
        (*rec).frame_callbacks.push(cb);
    }
}

unsafe extern "C" fn surface_noop_rect(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _x: i32,
    _y: i32,
    _w: i32,
    _h: i32,
) {
}
unsafe extern "C" fn surface_noop_region(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _reg: *mut ffi::wl_resource,
) {
}

/// `wl_surface.set_buffer_transform` (v2+): records how the client has
/// pre-rotated the buffer. The renderer applies the inverse at composite
/// time via CPU staging (ass-render) — a GPU-side transform in flux's image
/// shader is the long-term path.
unsafe extern "C" fn surface_set_buffer_transform(
    _c: *mut ffi::wl_client,
    r: *mut ffi::wl_resource,
    value: i32,
) {
    let rec = ffi::wl_resource_get_user_data(r) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    let transform = match value as u32 {
        0 => ass_core::Transform::Normal,
        1 => ass_core::Transform::Rotate90,
        2 => ass_core::Transform::Rotate180,
        3 => ass_core::Transform::Rotate270,
        4 => ass_core::Transform::FlipHorizontal,
        5 => ass_core::Transform::FlipRotate90,
        6 => ass_core::Transform::FlipRotate180,
        7 => ass_core::Transform::FlipRotate270,
        _ => return,
    };
    (*rec).pending_transform = transform;
}

/// `wl_surface.set_buffer_scale` (v2+): records the HiDPI scale.
unsafe extern "C" fn surface_set_buffer_scale(
    _c: *mut ffi::wl_client,
    r: *mut ffi::wl_resource,
    value: i32,
) {
    let rec = ffi::wl_resource_get_user_data(r) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    if value >= 1 {
        (*rec).pending_scale = value;
    }
}
#[allow(dead_code)]
unsafe extern "C" fn surface_noop_i32(_c: *mut ffi::wl_client, _r: *mut ffi::wl_resource, _v: i32) {
}

/// `wl_surface.damage` (v1): damage in surface-local coords. The renderer's
/// texture is in buffer pixel coords, so under buffer_scale > 1 these rects
/// cover only a fraction of the buffer. The renderer bypasses the
/// incremental-upload path when `buffer_scale != 1` (see ass-render); a
/// generation change still triggers a correct full upload.
unsafe extern "C" fn surface_damage(
    _c: *mut ffi::wl_client,
    r: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    let rec = ffi::wl_resource_get_user_data(r) as *mut SurfaceRec;
    if rec.is_null() || w <= 0 || h <= 0 {
        return;
    }
    (*rec).pending_damage.push(ass_core::Rect::new(x, y, w, h));
}

/// `wl_surface.damage_buffer` (v4): damage in buffer coords (post-scale,
/// post-transform). Accumulated into the same Vec as surface damage; the
/// renderer's incremental-upload path uses these directly because the
/// cached texture lives in buffer pixel space.
unsafe extern "C" fn surface_damage_buffer(
    _c: *mut ffi::wl_client,
    r: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    let rec = ffi::wl_resource_get_user_data(r) as *mut SurfaceRec;
    if rec.is_null() || w <= 0 || h <= 0 {
        return;
    }
    (*rec).pending_damage.push(ass_core::Rect::new(x, y, w, h));
}

unsafe extern "C" fn surface_resource_destroy(resource: *mut ffi::wl_resource) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    // Drop the toplevel from its workspace (ADR-0025). Idempotent: a no-op
    // for surfaces that never mapped or had no toplevel role. Run before the
    // slot is nulled so the resource address is still readable.
    if !(*rec).state.is_null() {
        let id = (*rec).window.id;
        (*(*rec).state).workspaces.remove_toplevel(id);
        // Notify foreign-toplevel listeners the window is gone.
        if !(*rec).xdg_toplevel.is_null() {
            extensions::foreign_toplevel_removed(id.0, (*rec).state);
        }
    }
    // Release any held dma-buf-backed wl_buffer so the client can reclaim its
    // GPU memory; a surface destroyed while holding a zero-copy buffer would
    // otherwise never signal release.
    if !(*rec).dmabuf_buffer.is_null() {
        ffi::wl_resource_post_event((*rec).dmabuf_buffer, ffi::WL_BUFFER_RELEASE);
        (*rec).dmabuf_buffer = std::ptr::null_mut();
    }
    // Detach from the parent's children list and orphan any children of this
    // surface so they do not keep a dangling parent pointer. Children stay in
    // the surfaces Vec and remain mapped (the client may re-parent them).
    detach_from_parent(rec);
    for child in std::mem::take(&mut (*rec).children) {
        (*child).parent = std::ptr::null_mut();
    }
    // Detach from the surfaces list so iterators stop visiting this rec, then
    // reclaim the allocation. The slot is left null and never reused: stable
    // indices are not load-bearing here, but the bookkeeping is simplest this
    // way and the Vec stops growing once churn settles.
    if !(*rec).state.is_null() {
        let idx = (*rec).index;
        let slot = (*(*rec).state).surfaces.as_mut_ptr().add(idx);
        std::ptr::write(slot, std::ptr::null_mut());
    }
    drop(Box::from_raw(rec));
}

// ----- wl_region ----------------------------------------------------------

static REGION_IMPL: ffi::wl_region_interface_impl = ffi::wl_region_interface_impl {
    destroy: region_destroy,
    add: surface_noop_rect,
    subtract: surface_noop_rect,
};

unsafe extern "C" fn region_destroy(_client: *mut ffi::wl_client, resource: *mut ffi::wl_resource) {
    ffi::wl_resource_destroy(resource);
}

// ----- wl_output ----------------------------------------------------------

/// Compute the (mode, integer-scale, transform) tuple from the state's
/// `output_geometry`, with a sane default before the first resize.
unsafe fn output_params(state: *mut State) -> (ass_core::output::OutputMode, i32, i32) {
    let mut mode = ass_core::output::OutputMode {
        width: 1280,
        height: 720,
        refresh_mhz: 60000,
    };
    let mut scale_i = 1i32;
    let mut transform = 0i32;
    if !state.is_null() {
        let g = (*state).output_geometry;
        if g.mode.width > 0 && g.mode.height > 0 {
            mode = g.mode;
        }
        scale_i = g.scale.0.round().max(1.0) as i32;
        transform = g.transform as i32;
    }
    (mode, scale_i, transform)
}

/// Post the full geometry + mode + scale + done sequence to one wl_output
/// resource. Version-gated: scale/done require v2.
unsafe fn send_output_geometry(res: *mut ffi::wl_resource, state: *mut State) {
    let (mode, scale_i, transform) = output_params(state);
    let version = ffi::wl_resource_get_version(res);
    let make = CString::new("ass").unwrap();
    let model = CString::new("nested").unwrap();
    ffi::wl_resource_post_event(
        res,
        ffi::WL_OUTPUT_GEOMETRY,
        0i32,
        0i32,
        300i32,
        200i32,
        0i32,
        make.as_ptr(),
        model.as_ptr(),
        transform,
    );
    ffi::wl_resource_post_event(
        res,
        ffi::WL_OUTPUT_MODE,
        ffi::WL_OUTPUT_MODE_CURRENT,
        mode.width,
        mode.height,
        mode.refresh_mhz as i32,
    );
    if version >= 2 {
        ffi::wl_resource_post_event(res, ffi::WL_OUTPUT_SCALE, scale_i);
        ffi::wl_resource_post_event(res, ffi::WL_OUTPUT_DONE);
    }
}

unsafe extern "C" fn output_resource_destroy(resource: *mut ffi::wl_resource) {
    let state = ffi::wl_resource_get_user_data(resource) as *mut State;
    if state.is_null() {
        return;
    }
    for slot in (*state).output_resources.iter_mut() {
        if *slot == resource {
            *slot = std::ptr::null_mut();
            break;
        }
    }
}

unsafe extern "C" fn output_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(client, &ffi::wl_output_interface, version as c_int, id);
    if res.is_null() {
        return;
    }
    let state = data as *mut State;
    ffi::wl_resource_set_implementation(
        res,
        std::ptr::null(),
        state as *mut c_void,
        Some(output_resource_destroy),
    );
    if !state.is_null() {
        (*state).output_resources.push(res);
    }

    send_output_geometry(res, state);
}

// ----- xdg_wm_base --------------------------------------------------------

static XDG_WM_BASE_IMPL: ffi::xdg_wm_base_interface_impl = ffi::xdg_wm_base_interface_impl {
    destroy: res_destroy,
    create_positioner: xdg_wm_base_create_positioner,
    get_xdg_surface: xdg_wm_base_get_xdg_surface,
    pong: xdg_noop_serial,
};

unsafe extern "C" fn xdg_wm_base_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(client, &ffi::xdg_wm_base_interface, version as c_int, id);
    if res.is_null() {
        return;
    }
    // The resource data is the server State, so get_xdg_surface can reach the
    // display for serials.
    ffi::wl_resource_set_implementation(
        res,
        &XDG_WM_BASE_IMPL as *const _ as *const c_void,
        data,
        None,
    );
}

static POSITIONER_IMPL: ffi::xdg_positioner_interface_impl = ffi::xdg_positioner_interface_impl {
    destroy: res_destroy,
    set_size: positioner_set_size,
    set_anchor_rect: positioner_set_anchor_rect,
    set_anchor: noop_uu_one,
    set_gravity: noop_uu_one,
    set_constraint_adjustment: noop_uu_one,
    set_offset: positioner_set_offset,
};

static POPUP_IMPL: ffi::xdg_popup_interface_impl = ffi::xdg_popup_interface_impl {
    destroy: popup_destroy,
    grab: popup_grab,
};

unsafe extern "C" fn xdg_wm_base_create_positioner(
    client: *mut ffi::wl_client,
    wm_base: *mut ffi::wl_resource,
    id: u32,
) {
    let ver = ffi::wl_resource_get_version(wm_base);
    let pos = ffi::wl_resource_create(client, &ffi::xdg_positioner_interface, ver, id);
    if pos.is_null() {
        return;
    }
    let st = Box::into_raw(Box::new(PositionerState::default()));
    ffi::wl_resource_set_implementation(
        pos,
        &POSITIONER_IMPL as *const _ as *const c_void,
        st as *mut c_void,
        Some(positioner_resource_destroy),
    );
}

unsafe extern "C" fn positioner_resource_destroy(resource: *mut ffi::wl_resource) {
    let st = ffi::wl_resource_get_user_data(resource) as *mut PositionerState;
    if !st.is_null() {
        drop(Box::from_raw(st));
    }
}

unsafe extern "C" fn positioner_set_size(
    _c: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    w: i32,
    h: i32,
) {
    let st = ffi::wl_resource_get_user_data(resource) as *mut PositionerState;
    if !st.is_null() && w > 0 && h > 0 {
        (*st).size = Some(ass_core::Size { w, h });
    }
}

unsafe extern "C" fn positioner_set_anchor_rect(
    _c: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    let st = ffi::wl_resource_get_user_data(resource) as *mut PositionerState;
    if !st.is_null() {
        (*st).anchor_rect = Some(ass_core::Rect::new(x, y, w, h));
    }
}

unsafe extern "C" fn positioner_set_offset(
    _c: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
) {
    let st = ffi::wl_resource_get_user_data(resource) as *mut PositionerState;
    if !st.is_null() {
        (*st).offset = ass_core::Point { x, y };
    }
}

unsafe extern "C" fn xdg_wm_base_get_xdg_surface(
    client: *mut ffi::wl_client,
    wm_base: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
) {
    let state = ffi::wl_resource_get_user_data(wm_base) as *mut State;
    let rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
    let ver = ffi::wl_resource_get_version(wm_base);
    let xdg_surface = ffi::wl_resource_create(client, &ffi::xdg_surface_interface, ver, id);
    if xdg_surface.is_null() {
        return;
    }
    (*rec).xdg_surface = xdg_surface;
    (*rec).display = (*state).display;
    ffi::wl_resource_set_implementation(
        xdg_surface,
        &XDG_SURFACE_IMPL as *const _ as *const c_void,
        rec as *mut c_void,
        None,
    );
}

// ----- xdg_surface --------------------------------------------------------

static XDG_SURFACE_IMPL: ffi::xdg_surface_interface_impl = ffi::xdg_surface_interface_impl {
    destroy: xdg_surface_destroy,
    get_toplevel: xdg_surface_get_toplevel,
    get_popup: xdg_surface_get_popup,
    set_window_geometry: xdg_noop_rect,
    ack_configure: xdg_noop_serial,
};

unsafe extern "C" fn xdg_surface_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() {
        (*rec).xdg_surface = std::ptr::null_mut();
        (*rec).xdg_configured = false;
    }
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn xdg_surface_get_toplevel(
    client: *mut ffi::wl_client,
    xdg_surface: *mut ffi::wl_resource,
    id: u32,
) {
    let rec = ffi::wl_resource_get_user_data(xdg_surface) as *mut SurfaceRec;
    let ver = ffi::wl_resource_get_version(xdg_surface);
    let toplevel = ffi::wl_resource_create(client, &ffi::xdg_toplevel_interface, ver, id);
    if toplevel.is_null() {
        return;
    }
    (*rec).xdg_toplevel = toplevel;
    // Initialize the window metadata so subsequent state requests (set_title,
    // set_min_size, ...) have somewhere to write.
    // Allocate a durable window id (ADR-0032). The surface address is no
    // longer the window's identity; the id is monotonic and never reused.
    let window_id = if !(*rec).state.is_null() {
        (*(*rec).state).alloc_window_id()
    } else {
        ass_core::window::WindowId(0)
    };
    (*rec).window = ass_core::window::Window::new(window_id);
    ffi::wl_resource_set_implementation(
        toplevel,
        &XDG_TOPLEVEL_IMPL as *const _ as *const c_void,
        rec as *mut c_void,
        None,
    );
}

unsafe extern "C" fn xdg_surface_get_popup(
    client: *mut ffi::wl_client,
    xdg_surface: *mut ffi::wl_resource,
    id: u32,
    parent: *mut ffi::wl_resource,
    positioner: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(xdg_surface) as *mut SurfaceRec;
    let ver = ffi::wl_resource_get_version(xdg_surface);
    let pop = ffi::wl_resource_create(client, &ffi::xdg_popup_interface, ver, id);
    if pop.is_null() {
        return;
    }
    // Compute the popup position from the positioner + parent.
    let pos_state = ffi::wl_resource_get_user_data(positioner) as *mut PositionerState;
    let parent_rec = if parent.is_null() {
        std::ptr::null_mut::<SurfaceRec>()
    } else {
        ffi::wl_resource_get_user_data(parent) as *mut SurfaceRec
    };
    let (px, py) = if parent_rec.is_null() {
        (0, 0)
    } else {
        ((*parent_rec).position.x, (*parent_rec).position.y)
    };
    let anchor = if !pos_state.is_null() {
        (*pos_state).anchor_rect
    } else {
        None
    };
    let offset = if !pos_state.is_null() {
        (*pos_state).offset
    } else {
        ass_core::Point::default()
    };
    // Place at the anchor-rect's bottom-left by default (the common menu case),
    // else at the parent origin; apply the client offset.
    let (ax, ay) = match anchor {
        Some(r) => (r.origin.x, r.origin.y + r.size.h),
        None => (0, 0),
    };
    let popup_pos = ass_core::Point {
        x: px + ax + offset.x,
        y: py + ay + offset.y,
    };
    let popup_size = if !pos_state.is_null() {
        (*pos_state).size.unwrap_or(ass_core::Size { w: 0, h: 0 })
    } else {
        ass_core::Size { w: 0, h: 0 }
    };
    (*rec).position = popup_pos;
    (*rec).xdg_toplevel = std::ptr::null_mut(); // popups are not toplevels
    ffi::wl_resource_set_implementation(
        pop,
        &POPUP_IMPL as *const _ as *const c_void,
        rec as *mut c_void,
        None,
    );
    // Send the initial configure so the client sizes and maps its buffer.
    ffi::wl_resource_post_event(
        pop,
        ffi::XDG_POPUP_CONFIGURE,
        popup_pos.x,
        popup_pos.y,
        popup_size.w,
        popup_size.h,
    );
    // The xdg_surface configure serial must follow per xdg-shell.
    if !(*rec).xdg_surface.is_null() {
        let serial = ffi::wl_display_next_serial((*rec).display);
        ffi::wl_resource_post_event((*rec).xdg_surface, ffi::XDG_SURFACE_CONFIGURE, serial);
    }
}

unsafe extern "C" fn popup_destroy(_client: *mut ffi::wl_client, resource: *mut ffi::wl_resource) {
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn popup_grab(
    _client: *mut ffi::wl_client,
    _popup: *mut ffi::wl_resource,
    _seat: *mut ffi::wl_resource,
    _serial: u32,
) {
    // Popup grabs (keyboard/pointer grab semantics) are not enforced; accepting
    // the request keeps client lifecycle valid.
}

// ----- xdg_toplevel -------------------------------------------------------

static XDG_TOPLEVEL_IMPL: ffi::xdg_toplevel_interface_impl = ffi::xdg_toplevel_interface_impl {
    destroy: xdg_toplevel_destroy,
    set_parent: toplevel_set_parent,
    set_title: toplevel_set_title,
    set_app_id: toplevel_set_app_id,
    show_window_menu: xdg_noop_menu,
    move_: toplevel_move,
    resize: toplevel_resize,
    set_max_size: toplevel_set_max_size,
    set_min_size: toplevel_set_min_size,
    set_maximized: toplevel_set_maximized,
    unset_maximized: toplevel_unset_maximized,
    set_fullscreen: toplevel_set_fullscreen,
    unset_fullscreen: toplevel_unset_fullscreen,
    set_minimized: toplevel_set_minimized,
};

unsafe extern "C" fn xdg_toplevel_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() {
        (*rec).xdg_toplevel = std::ptr::null_mut();
        (*rec).mapped = false;
    }
    ffi::wl_resource_destroy(resource);
}

// ----- xdg_toplevel metadata + state requests ----------------------------
//
// M3 partial: real handlers for the metadata and state requests. Interactive
// move and resize (`move_`, `resize`) remain no-ops pending the pointer-state
// machine that drives them; `show_window_menu` and `set_minimized` are
// accepted but inert until the shell grows window-list and menu chrome. The
// state-changing handlers (maximize, fullscreen) emit a fresh
// `xdg_toplevel.configure` with the appropriate states array so clients
// reconfigure themselves.

unsafe fn cstr_to_string(p: *const std::os::raw::c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    Some(CStr::from_ptr(p).to_string_lossy().into_owned())
}

unsafe extern "C" fn toplevel_set_title(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    title: *const std::os::raw::c_char,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() {
        (*rec).window.title = cstr_to_string(title);
        if !(*rec).state.is_null() {
            extensions::foreign_toplevel_updated(rec, (*rec).state);
        }
    }
}

unsafe extern "C" fn toplevel_set_app_id(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    app_id: *const std::os::raw::c_char,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() {
        (*rec).window.app_id = cstr_to_string(app_id);
        if !(*rec).state.is_null() {
            extensions::foreign_toplevel_updated(rec, (*rec).state);
        }
    }
}

unsafe extern "C" fn toplevel_set_parent(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    parent_resource: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    if parent_resource.is_null() {
        (*rec).window.parent = None;
        return;
    }
    let parent_rec = ffi::wl_resource_get_user_data(parent_resource) as *mut SurfaceRec;
    if parent_rec.is_null() {
        (*rec).window.parent = None;
    } else {
        (*rec).window.parent = Some(parent_rec as usize);
    }
}

unsafe extern "C" fn toplevel_set_min_size(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    w: i32,
    h: i32,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() {
        (*rec).window.size_hints.min_w = w.max(0);
        (*rec).window.size_hints.min_h = h.max(0);
    }
}

unsafe extern "C" fn toplevel_set_max_size(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    w: i32,
    h: i32,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() {
        (*rec).window.size_hints.max_w = w.max(0);
        (*rec).window.size_hints.max_h = h.max(0);
    }
}

/// Common path for state transitions: set the bit, send a configure event
/// carrying the new states array. `width` / `height` of 0 means "let the
/// client pick" per xdg-shell; we pass 0,0 for state-only changes and reserve
/// non-zero dimensions for fullscreen-on-output (deferred).
unsafe fn reconfigure_with_state(rec: *mut SurfaceRec) {
    // The state-bit path leaves the size to the client (0, 0 = "you decide");
    // maximize/fullscreen are advisory on size this way.
    reconfigure_with_size(rec, 0, 0);
}

/// Post an `xdg_toplevel.configure` with an explicit width/height (the
/// tiling path forces a size, unlike the state-bit path). `w`/`h` of 0 mean
/// "client decides" per xdg-shell. The current window-state bits are sent
/// unchanged; tiling carries no protocol state of its own.
unsafe fn reconfigure_with_size(rec: *mut SurfaceRec, w: i32, h: i32) {
    if rec.is_null() || (*rec).xdg_toplevel.is_null() {
        return;
    }
    let states = (*rec).window.state.to_state_array();
    let mut arr = ffi::wl_array::empty();
    // Pack the active state values into a wl_array as u32s. Each state entry
    // is one `uint` per the protocol, so the array's element size is 4.
    if !states.is_empty() {
        let bytes_needed = states.len() * std::mem::size_of::<u32>();
        let buf = malloc(bytes_needed) as *mut u32;
        if !buf.is_null() {
            for (i, s) in states.iter().enumerate() {
                std::ptr::write(buf.add(i), *s);
            }
            arr.size = bytes_needed;
            arr.alloc = bytes_needed;
            arr.data = buf as *mut c_void;
        }
    }
    ffi::wl_resource_post_event(
        (*rec).xdg_toplevel,
        ffi::XDG_TOPLEVEL_CONFIGURE,
        w,
        h,
        &mut arr as *mut ffi::wl_array,
    );
    if !arr.data.is_null() {
        free(arr.data);
    }
    // Also fire xdg_surface.configure so the client acks the new state.
    if !(*rec).xdg_surface.is_null() {
        let serial = ffi::wl_display_next_serial((*rec).display);
        ffi::wl_resource_post_event((*rec).xdg_surface, ffi::XDG_SURFACE_CONFIGURE, serial);
    }
}

unsafe extern "C" fn toplevel_set_maximized(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    (*rec).window.state.maximized = true;
    reconfigure_with_state(rec);
}

unsafe extern "C" fn toplevel_unset_maximized(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    (*rec).window.state.maximized = false;
    reconfigure_with_state(rec);
}

unsafe extern "C" fn toplevel_set_fullscreen(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    _output: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    (*rec).window.state.fullscreen = true;
    reconfigure_with_state(rec);
}

unsafe extern "C" fn toplevel_unset_fullscreen(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    (*rec).window.state.fullscreen = false;
    reconfigure_with_state(rec);
}

/// `xdg_toplevel.set_minimized`: the client asks the compositor to hide it.
/// Unlike maximize/fullscreen this is not a configure state — the compositor
/// stops rendering and hit-testing the surface but keeps it mapped so the
/// client retains its buffers and can be restored. If the minimized toplevel
/// held keyboard focus, drop it (post `wl_keyboard.leave`, clear activated)
/// so typing no longer routes to an invisible window. Restore happens when a
/// later focus gain clears the flag (see `set_activated_for_surface`).
unsafe extern "C" fn toplevel_set_minimized(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    (*rec).window.minimized = true;
    let state = (*rec).state;
    if state.is_null() || (*state).keyboard_focus != (*rec).resource {
        return;
    }
    // The minimized toplevel held keyboard focus: mirror
    // `change_keyboard_focus(null)` + `set_activated_for_surface(old, false)`
    // inline, since the handler is a free function without a `Server` ref.
    let serial = ffi::wl_display_next_serial((*state).display);
    let old_client = ffi::wl_resource_get_client((*rec).resource);
    for k in (*state)
        .keyboard_resources
        .iter()
        .copied()
        .filter(|p| !p.is_null())
        .filter(|p| ffi::wl_resource_get_client(*p) == old_client)
    {
        ffi::wl_resource_post_event(k, ffi::WL_KEYBOARD_LEAVE, serial, (*rec).resource);
    }
    (*state).keyboard_focus = std::ptr::null_mut();
    (*rec).window.state.activated = false;
    reconfigure_with_state(rec);
    ffi::wl_display_flush_clients((*state).display);
}

// ----- interactive move / resize -----------------------------------------
//
// `xdg_toplevel.move` / `resize` start an interactive grab when the supplied
// serial matches the last pointer-button press. While the grab is active,
// pointer motion updates the window's geometry (move: translate position;
// resize: grow/shrink along the requested edges, clamped to size hints).
// Button release ends the grab.
//
// The grab is compositor-side: the window moves whether or not the client
// cooperative-processes; the client still receives `wl_pointer.motion`
// events as normal during the grab (per protocol, the pointer stays with
// the surface that had it when the grab began).

unsafe extern "C" fn toplevel_move(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    _seat: *mut ffi::wl_resource,
    serial: u32,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    // Validate the serial against the last button press. xdg-shell requires
    // the request be issued "in response to" a pointer button press; an
    // unmatched serial means the client tried to start a grab out of band.
    let state_ptr = (*rec).state;
    if state_ptr.is_null() || (*state_ptr).last_button_serial != serial {
        return;
    }
    if (*state_ptr).interactive.is_some() {
        return; // Already grabbing; ignore.
    }
    (*state_ptr).interactive = Some(ass_core::window::Interactive::Move {
        window_id: (*rec).window.id,
        origin: ((*state_ptr).pointer_x, (*state_ptr).pointer_y),
        start_position: (*rec).position,
    });
    (*state_ptr).compositor_pointer_grab = false;
}

unsafe extern "C" fn toplevel_resize(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    _seat: *mut ffi::wl_resource,
    serial: u32,
    edges: u32,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    let state_ptr = (*rec).state;
    if state_ptr.is_null() || (*state_ptr).last_button_serial != serial {
        return;
    }
    if (*state_ptr).interactive.is_some() {
        return;
    }
    let edges = ass_core::window::ResizeEdges(edges);
    if edges.is_none() {
        return;
    }
    (*rec).window.state.resizing = true;
    reconfigure_with_state(rec);
    (*state_ptr).interactive = Some(ass_core::window::Interactive::Resize {
        window_id: (*rec).window.id,
        edges,
        origin: ((*state_ptr).pointer_x, (*state_ptr).pointer_y),
        start_position: (*rec).position,
        start_size: ass_core::Size {
            w: (*rec).width,
            h: (*rec).height,
        },
    });
    (*state_ptr).compositor_pointer_grab = false;
}

/// Apply a pointer-motion delta to an active interactive grab. Returns
/// `true` if the motion was consumed by the grab (the position changed) and
/// the caller may want to still forward the motion event to the focused
/// client, or `false` if there was no active grab.
///
/// On resize, the new window size is clamped to the toplevel's `size_hints`
/// (min and max) and a minimum of 1×1. The server then posts a fresh
/// `xdg_toplevel.configure` with the new width/height so the client can
/// reallocate its buffer.
impl Server {
    fn apply_interactive_motion(&mut self, x: f32, y: f32) -> bool {
        let interactive = match self.state.interactive {
            Some(i) => i,
            None => return false,
        };
        // Find the surface rec backing this window id.
        let rec_ptr: *mut SurfaceRec = self.find_surface_by_window_id(interactive.window_id());
        if rec_ptr.is_null() {
            self.state.interactive = None;
            self.state.compositor_pointer_grab = false;
            return false;
        }
        match interactive {
            ass_core::window::Interactive::Move {
                start_position,
                origin,
                ..
            } => {
                let new_x = start_position.x as f32 + (x - origin.0);
                let new_y = start_position.y as f32 + (y - origin.1);
                // Round to integer logical pixels.
                let pos = ass_core::Point {
                    x: new_x.round() as i32,
                    y: new_y.round() as i32,
                };
                unsafe {
                    (*rec_ptr).position = pos;
                    (*rec_ptr).window.position = pos;
                }
                true
            }
            ass_core::window::Interactive::Resize {
                edges,
                origin,
                start_position,
                start_size,
                ..
            } => {
                let dx = (x - origin.0).round() as i32;
                let dy = (y - origin.1).round() as i32;
                let mut new_x = start_position.x;
                let mut new_y = start_position.y;
                let mut new_w = start_size.w;
                let mut new_h = start_size.h;
                // Horizontal axis.
                if edges.has_right() {
                    new_w = start_size.w + dx;
                } else if edges.has_left() {
                    new_w = start_size.w - dx;
                    new_x = start_position.x + dx;
                }
                // Vertical axis.
                if edges.has_bottom() {
                    new_h = start_size.h + dy;
                } else if edges.has_top() {
                    new_h = start_size.h - dy;
                    new_y = start_position.y + dy;
                }
                // Clamp to size hints and a 1×1 floor. If a max constraint
                // forces a smaller size on the growing side, also pull back the
                // position to keep the opposite edge anchored.
                let hints = unsafe { (*rec_ptr).window.size_hints };
                let min_w = hints.min_w.max(1);
                let min_h = hints.min_h.max(1);
                let max_w = if hints.max_w > 0 {
                    hints.max_w
                } else {
                    i32::MAX
                };
                let max_h = if hints.max_h > 0 {
                    hints.max_h
                } else {
                    i32::MAX
                };
                if new_w < min_w {
                    if edges.has_left() {
                        new_x += min_w - new_w;
                    }
                    new_w = min_w;
                } else if new_w > max_w {
                    if edges.has_left() {
                        new_x += max_w - new_w;
                    }
                    new_w = max_w;
                }
                if new_h < min_h {
                    if edges.has_top() {
                        new_y += min_h - new_h;
                    }
                    new_h = min_h;
                } else if new_h > max_h {
                    if edges.has_top() {
                        new_y += max_h - new_h;
                    }
                    new_h = max_h;
                }
                // Apply and send a configure with the new dimensions so the
                // client reallocates its buffer.
                unsafe {
                    let pos = ass_core::Point { x: new_x, y: new_y };
                    (*rec_ptr).position = pos;
                    (*rec_ptr).window.position = pos;
                    (*rec_ptr).window.size = ass_core::Size { w: new_w, h: new_h };
                    if !(*rec_ptr).xdg_toplevel.is_null() {
                        let mut arr = ffi::wl_array::empty();
                        let states = (*rec_ptr).window.state.to_state_array();
                        if !states.is_empty() {
                            let bytes_needed = states.len() * std::mem::size_of::<u32>();
                            let buf = malloc(bytes_needed) as *mut u32;
                            if !buf.is_null() {
                                for (i, s) in states.iter().enumerate() {
                                    std::ptr::write(buf.add(i), *s);
                                }
                                arr.size = bytes_needed;
                                arr.alloc = bytes_needed;
                                arr.data = buf as *mut c_void;
                            }
                        }
                        ffi::wl_resource_post_event(
                            (*rec_ptr).xdg_toplevel,
                            ffi::XDG_TOPLEVEL_CONFIGURE,
                            new_w,
                            new_h,
                            &mut arr as *mut ffi::wl_array,
                        );
                        if !arr.data.is_null() {
                            free(arr.data);
                        }
                        if !(*rec_ptr).xdg_surface.is_null() {
                            let serial = ffi::wl_display_next_serial((*rec_ptr).display);
                            ffi::wl_resource_post_event(
                                (*rec_ptr).xdg_surface,
                                ffi::XDG_SURFACE_CONFIGURE,
                                serial,
                            );
                        }
                    }
                }
                true
            }
        }
    }

    fn find_surface_by_window_id(&self, id: ass_core::window::WindowId) -> *mut SurfaceRec {
        for p in self.state.live_surfaces() {
            if unsafe { (*p).window.id } == id {
                return p;
            }
        }
        std::ptr::null_mut()
    }
}

// ----- wl_subcompositor ---------------------------------------------------

static SUBCOMPOSITOR_IMPL: ffi::wl_subcompositor_interface_impl =
    ffi::wl_subcompositor_interface_impl {
        destroy: res_destroy,
        get_subsurface: subcompositor_get_subsurface,
    };

static SUBSURFACE_IMPL: ffi::wl_subsurface_interface_impl = ffi::wl_subsurface_interface_impl {
    destroy: subsurface_destroy,
    set_position: subsurface_set_position,
    place_above: subsurface_place_above,
    place_below: subsurface_place_below,
    set_sync: subsurface_set_sync,
    set_desync: subsurface_set_desync,
};

unsafe extern "C" fn subcompositor_bind(
    client: *mut ffi::wl_client,
    _data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(
        client,
        &ffi::wl_subcompositor_interface,
        version as c_int,
        id,
    );
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &SUBCOMPOSITOR_IMPL as *const _ as *const c_void,
        std::ptr::null_mut(),
        None,
    );
}

unsafe extern "C" fn subcompositor_get_subsurface(
    client: *mut ffi::wl_client,
    parent_res: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
    parent: *mut ffi::wl_resource,
) {
    let child_rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
    let parent_rec = ffi::wl_resource_get_user_data(parent) as *mut SurfaceRec;
    let ver = ffi::wl_resource_get_version(parent_res);
    let sub = ffi::wl_resource_create(client, &ffi::wl_subsurface_interface, ver, id);
    if sub.is_null() || child_rec.is_null() || parent_rec.is_null() {
        return;
    }
    // Link the child into the parent's children list. The rec pointer is
    // shared; both surface and subsurface resource reference it.
    (*child_rec).parent = parent_rec;
    (*parent_rec).children.push(child_rec);
    ffi::wl_resource_set_implementation(
        sub,
        &SUBSURFACE_IMPL as *const _ as *const c_void,
        child_rec as *mut c_void,
        None,
    );
}

// `wl_subsurface` request handlers. M2 implements set_position, place_above,
// and place_below; sync/desync are accepted but treated as desync (children
// apply their own commits immediately). True sync-mode cascade is future work
// because it interacts with commit ordering and pending-state tracking.

unsafe extern "C" fn subsurface_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() {
        detach_from_parent(rec);
    }
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn subsurface_set_position(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() {
        (*rec).subsurface_offset = ass_core::Point { x, y };
    }
}

unsafe extern "C" fn subsurface_place_above(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    _sibling: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() {
        (*rec).subsurface_above_parent = true;
        // Sibling-relative ordering within the children list is not modeled
        // in M2; the boolean is enough to split below/above draws.
    }
}

unsafe extern "C" fn subsurface_place_below(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    _sibling: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() {
        (*rec).subsurface_above_parent = false;
    }
}

unsafe extern "C" fn subsurface_set_sync(
    _client: *mut ffi::wl_client,
    _resource: *mut ffi::wl_resource,
) {
    // Sync mode: child state applies on parent commit. M2 treats all
    // subsurfaces as desync (apply on own commit); true sync-mode cascade is
    // future work because it interacts with commit ordering and pending state.
}

unsafe extern "C" fn subsurface_set_desync(
    _client: *mut ffi::wl_client,
    _resource: *mut ffi::wl_resource,
) {
}

/// Detach a subsurface from its parent's children list. Called from
/// `subsurface.destroy` and from `surface_resource_destroy` (the latter
/// because a surface can be destroyed without its subsurface role being
/// explicitly destroyed first).
unsafe fn detach_from_parent(rec: *mut SurfaceRec) {
    let parent = (*rec).parent;
    if parent.is_null() {
        return;
    }
    let target = rec;
    (*parent).children.retain(|c| *c != target);
    (*rec).parent = std::ptr::null_mut();
}

// ----- wl_seat ------------------------------------------------------------

static SEAT_IMPL: ffi::wl_seat_interface_impl = ffi::wl_seat_interface_impl {
    get_pointer: seat_get_pointer,
    get_keyboard: seat_get_keyboard,
    get_touch: seat_get_touch,
    release: res_destroy,
};

// `wl_pointer` has two requests: `set_cursor` (v1) and `release` (v3). The
// previous code bound the pointer resource with a NULL implementation on the
// assumption that no request needed handling — but `set_cursor` is a regular
// request every client sends to change its cursor, and a NULL handler makes
// libwayland abort with "Implementation of resource N of wl_pointer is NULL".
// `set_cursor` is accepted and ignored: in the nested backend the host
// compositor already paints a cursor; a future DRM/KMS backend will store
// the cursor surface and paint it itself. `release` destroys the resource,
// which then runs `pointer_resource_destroy` to null out the slot.
static POINTER_IMPL: ffi::wl_pointer_interface_impl = ffi::wl_pointer_interface_impl {
    set_cursor: pointer_set_cursor,
    release: res_destroy,
};

static KEYBOARD_IMPL: ffi::wl_keyboard_interface_impl = ffi::wl_keyboard_interface_impl {
    release: res_destroy,
};

static TOUCH_IMPL: ffi::wl_touch_interface_impl = ffi::wl_touch_interface_impl {
    release: res_destroy,
};

/// `wl_pointer.set_cursor`: accept and ignore. See `POINTER_IMPL`.
unsafe extern "C" fn pointer_set_cursor(
    _client: *mut ffi::wl_client,
    _pointer: *mut ffi::wl_resource,
    _serial: u32,
    _surface: *mut ffi::wl_resource,
    _hotspot_x: i32,
    _hotspot_y: i32,
) {
}

unsafe extern "C" fn seat_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(client, &ffi::wl_seat_interface, version as c_int, id);
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(res, &SEAT_IMPL as *const _ as *const c_void, data, None);
    let state = data as *mut State;
    // Pointer cap is always advertised; keyboard only when the xkbcommon
    // keymap compiled successfully at startup. Touch is always advertised so
    // touch clients can bind.
    let mut caps = ffi::WL_SEAT_CAPABILITY_POINTER | ffi::WL_SEAT_CAPABILITY_TOUCH;
    if !state.is_null() && (*state).keyboard.is_some() {
        caps |= ffi::WL_SEAT_CAPABILITY_KEYBOARD;
    }
    ffi::wl_resource_post_event(res, ffi::WL_SEAT_CAPABILITIES, caps);
    if version >= 2 {
        let name = CString::new("seat0").unwrap();
        ffi::wl_resource_post_event(res, ffi::WL_SEAT_NAME, name.as_ptr());
    }
}

// Each pointer resource is tracked in `state.pointer_resources` so the main
// loop can fan out pointer events to the right client. A destroy notify nulls
// the slot when the client goes away or releases the pointer.
unsafe extern "C" fn seat_get_pointer(
    client: *mut ffi::wl_client,
    seat: *mut ffi::wl_resource,
    id: u32,
) {
    let state = ffi::wl_resource_get_user_data(seat) as *mut State;
    let ver = ffi::wl_resource_get_version(seat).min(7);
    let p = ffi::wl_resource_create(client, &ffi::wl_pointer_interface, ver, id);
    if p.is_null() {
        return;
    }
    // Real implementation: see POINTER_IMPL — both `set_cursor` and
    // `release` must have handlers or libwayland aborts the server.
    ffi::wl_resource_set_implementation(
        p,
        &POINTER_IMPL as *const _ as *const c_void,
        state as *mut c_void,
        Some(pointer_resource_destroy),
    );
    if !state.is_null() {
        (*state).pointer_resources.push(p);
    }
}

unsafe extern "C" fn pointer_resource_destroy(resource: *mut ffi::wl_resource) {
    let state = ffi::wl_resource_get_user_data(resource) as *mut State;
    if state.is_null() {
        return;
    }
    // Null the slot. Iterators skip null entries; the slot is never reused.
    for slot in (*state).pointer_resources.iter_mut() {
        if *slot == resource {
            *slot = std::ptr::null_mut();
            break;
        }
    }
    // If the focused client no longer has any pointer resources, clear focus
    // so the next motion event re-evaluates enter against remaining clients.
    if !(*state).pointer_focus.is_null() {
        let focus_client = ffi::wl_resource_get_client((*state).pointer_focus);
        let orphaned = (*state)
            .pointer_resources
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .all(|p| ffi::wl_resource_get_client(p) != focus_client);
        if orphaned {
            (*state).pointer_focus = std::ptr::null_mut();
        }
    }
}

unsafe extern "C" fn seat_get_keyboard(
    client: *mut ffi::wl_client,
    seat: *mut ffi::wl_resource,
    id: u32,
) {
    let state = ffi::wl_resource_get_user_data(seat) as *mut State;
    let ver = ffi::wl_resource_get_version(seat).min(7);
    let k = ffi::wl_resource_create(client, &ffi::wl_keyboard_interface, ver, id);
    if k.is_null() {
        return;
    }
    // Real implementation: `release` (v3+) must have a handler or
    // libwayland aborts when a client sends it. See KEYBOARD_IMPL.
    ffi::wl_resource_set_implementation(
        k,
        &KEYBOARD_IMPL as *const _ as *const c_void,
        state as *mut c_void,
        Some(keyboard_resource_destroy),
    );
    if !state.is_null() {
        if let Some(kb) = &(*state).keyboard {
            // Send the keymap event immediately so the client can decode
            // subsequent key/modifier events. libwayland dups the fd
            // internally; the original stays open for the next client.
            match kb.dup_keymap_fd() {
                Ok(fd) => {
                    ffi::wl_resource_post_event(
                        k,
                        ffi::WL_KEYBOARD_KEYMAP,
                        ffi::WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1,
                        fd,
                        kb.keymap_size() as u32,
                    );
                }
                Err(e) => {
                    log::warn!("[server] keymap fd dup failed: {e}");
                }
            }
            if ver >= 4 {
                // Default repeat: 25 cps after 250 ms delay, matching what
                // Weston and Mutter ship with a fresh install.
                ffi::wl_resource_post_event(k, ffi::WL_KEYBOARD_REPEAT_INFO, 25u32, 250u32);
            }
        }
        (*state).keyboard_resources.push(k);
    }
}

unsafe extern "C" fn keyboard_resource_destroy(resource: *mut ffi::wl_resource) {
    let state = ffi::wl_resource_get_user_data(resource) as *mut State;
    if state.is_null() {
        return;
    }
    for slot in (*state).keyboard_resources.iter_mut() {
        if *slot == resource {
            *slot = std::ptr::null_mut();
            break;
        }
    }
    if !(*state).keyboard_focus.is_null() {
        let focus_client = ffi::wl_resource_get_client((*state).keyboard_focus);
        let orphaned = (*state)
            .keyboard_resources
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .all(|p| ffi::wl_resource_get_client(p) != focus_client);
        if orphaned {
            (*state).keyboard_focus = std::ptr::null_mut();
        }
    }
}
unsafe extern "C" fn seat_get_touch(
    client: *mut ffi::wl_client,
    seat: *mut ffi::wl_resource,
    id: u32,
) {
    let state = ffi::wl_resource_get_user_data(seat) as *mut State;
    let ver = ffi::wl_resource_get_version(seat).min(8);
    let t = ffi::wl_resource_create(client, &ffi::wl_touch_interface, ver, id);
    if t.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        t,
        &TOUCH_IMPL as *const _ as *const c_void,
        state as *mut c_void,
        Some(touch_resource_destroy),
    );
    if !state.is_null() {
        (*state).touch_resources.push(t);
    }
}

unsafe extern "C" fn touch_resource_destroy(resource: *mut ffi::wl_resource) {
    let state = ffi::wl_resource_get_user_data(resource) as *mut State;
    if state.is_null() {
        return;
    }
    for slot in (*state).touch_resources.iter_mut() {
        if *slot == resource {
            *slot = std::ptr::null_mut();
            break;
        }
    }
}

// ----- wl_data_device_manager (clipboard) ---------------------------------
//
// A functional single-seat clipboard: `set_selection` records the source and
// advertises a `wl_data_offer` to every bound `wl_data_device`. Clients paste
// by `data_offer.receive`, which we service from the source's `send` request.
// The MIME types offered come from `wl_data_source.offer`.

static DDM_IMPL: ffi::wl_data_device_manager_interface_impl =
    ffi::wl_data_device_manager_interface_impl {
        create_data_source: ddm_create_data_source,
        get_data_device: ddm_get_data_device,
        destroy: res_destroy,
    };

static DATA_DEVICE_IMPL: ffi::wl_data_device_interface_impl = ffi::wl_data_device_interface_impl {
    start_drag: ddev_start_drag,
    set_selection: ddev_set_selection,
    release: res_destroy,
};

static DATA_SOURCE_IMPL: ffi::wl_data_source_interface_impl = ffi::wl_data_source_interface_impl {
    offer: data_source_offer,
    destroy: res_destroy,
};

static DATA_OFFER_IMPL: ffi::wl_data_offer_interface_impl = ffi::wl_data_offer_interface_impl {
    receive: data_offer_receive,
    destroy: res_destroy,
};

unsafe extern "C" fn ddm_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(
        client,
        &ffi::wl_data_device_manager_interface,
        version as c_int,
        id,
    );
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(res, &DDM_IMPL as *const _ as *const c_void, data, None);
}

/// Create a `wl_data_source` whose user-data is a boxed `Vec<String>` of MIME
/// types collected by `offer`.
unsafe extern "C" fn ddm_create_data_source(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
) {
    let ver = ffi::wl_resource_get_version(mgr);
    let src = ffi::wl_resource_create(client, &ffi::wl_data_source_interface, ver, id);
    if src.is_null() {
        return;
    }
    let mimes = Box::into_raw(Box::new(Vec::<String>::new()));
    ffi::wl_resource_set_implementation(
        src,
        &DATA_SOURCE_IMPL as *const _ as *const c_void,
        mimes as *mut c_void,
        Some(data_source_resource_destroy),
    );
}

unsafe extern "C" fn data_source_resource_destroy(resource: *mut ffi::wl_resource) {
    let mimes = ffi::wl_resource_get_user_data(resource) as *mut Vec<String>;
    if !mimes.is_null() {
        drop(Box::from_raw(mimes));
    }
}

unsafe extern "C" fn data_source_offer(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    mime_type: *const std::os::raw::c_char,
) {
    let mimes = ffi::wl_resource_get_user_data(resource) as *mut Vec<String>;
    if !mimes.is_null() && !mime_type.is_null() {
        if let Ok(s) = CStr::from_ptr(mime_type).to_str() {
            if !(*mimes).iter().any(|m| m == s) {
                (*mimes).push(s.to_string());
            }
        }
    }
}

unsafe extern "C" fn ddm_get_data_device(
    client: *mut ffi::wl_client,
    mgr: *mut ffi::wl_resource,
    id: u32,
    seat: *mut ffi::wl_resource,
) {
    let state = ffi::wl_resource_get_user_data(seat) as *mut State;
    let ver = ffi::wl_resource_get_version(mgr);
    let dev = ffi::wl_resource_create(client, &ffi::wl_data_device_interface, ver, id);
    if dev.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        dev,
        &DATA_DEVICE_IMPL as *const _ as *const c_void,
        state as *mut c_void,
        Some(data_device_resource_destroy),
    );
    if !state.is_null() {
        (*state).data_devices.push(dev);
        // If a selection is already active, advertise it to this new device.
        if let Some(sel) = &(*state).selection {
            advertise_offer(dev, sel);
        }
    }
}

unsafe extern "C" fn data_device_resource_destroy(resource: *mut ffi::wl_resource) {
    let state = ffi::wl_resource_get_user_data(resource) as *mut State;
    if state.is_null() {
        return;
    }
    for slot in (*state).data_devices.iter_mut() {
        if *slot == resource {
            *slot = std::ptr::null_mut();
            break;
        }
    }
}

/// `wl_data_device.set_selection`: record the source as the current selection
/// and advertise a `wl_data_offer` to every bound data device. A null source
/// clears the selection.
unsafe extern "C" fn ddev_set_selection(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    source: *mut ffi::wl_resource,
    _serial: u32,
) {
    let state = ffi::wl_resource_get_user_data(_r) as *mut State;
    if state.is_null() {
        return;
    }
    if source.is_null() {
        (*state).selection = None;
        // Notify devices of the empty selection.
        let devices: Vec<*mut ffi::wl_resource> = (*state)
            .data_devices
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .collect();
        for dev in devices {
            ffi::wl_resource_post_event(
                dev,
                ffi::WL_DATA_DEVICE_SELECTION,
                std::ptr::null_mut::<ffi::wl_resource>(),
            );
        }
        return;
    }
    // Build the selection record: the source resource + its collected MIMEs.
    let mimes_ptr = ffi::wl_resource_get_user_data(source) as *mut Vec<String>;
    let mimes = if mimes_ptr.is_null() {
        Vec::new()
    } else {
        (*mimes_ptr).clone()
    };
    let sel = Selection {
        source,
        mime_types: mimes,
    };
    (*state).selection = Some(sel);
    let devices: Vec<*mut ffi::wl_resource> = (*state)
        .data_devices
        .iter()
        .copied()
        .filter(|p| !p.is_null())
        .collect();
    for dev in devices {
        advertise_offer(dev, (*state).selection.as_ref().unwrap());
    }
}

/// Create a `wl_data_offer` for `sel`, send `data_offer` + its `offer` events
/// to `dev`, then `selection(offer)`.
unsafe fn advertise_offer(dev: *mut ffi::wl_resource, sel: &Selection) {
    let client = ffi::wl_resource_get_client(dev);
    let offer = ffi::wl_resource_create(client, &ffi::wl_data_offer_interface, 3, 0);
    if offer.is_null() {
        return;
    }
    let src = sel.source; // back-pointer so receive() can forward to send().
    ffi::wl_resource_set_implementation(
        offer,
        &DATA_OFFER_IMPL as *const _ as *const c_void,
        src as *mut c_void,
        Some(data_offer_resource_destroy),
    );
    ffi::wl_resource_post_event(dev, ffi::WL_DATA_DEVICE_DATA_OFFER, offer);
    for mime in &sel.mime_types {
        let c = CString::new(mime.as_str()).unwrap();
        ffi::wl_resource_post_event(offer, ffi::WL_DATA_OFFER_OFFER, c.as_ptr());
    }
    ffi::wl_resource_post_event(dev, ffi::WL_DATA_DEVICE_SELECTION, offer);
}

unsafe extern "C" fn data_offer_resource_destroy(resource: *mut ffi::wl_resource) {
    // user_data is the backing wl_data_source; not owned by the offer.
    let _ = ffi::wl_resource_get_user_data(resource);
}

/// `wl_data_offer.receive`: forward to the source's `send` request so the
/// owning client writes the content for `mime_type` into `fd`.
unsafe extern "C" fn data_offer_receive(
    _client: *mut ffi::wl_client,
    offer: *mut ffi::wl_resource,
    mime_type: *const std::os::raw::c_char,
    fd: i32,
) {
    let source = ffi::wl_resource_get_user_data(offer) as *mut ffi::wl_resource;
    if source.is_null() {
        if fd >= 0 {
            libc_close(fd);
        }
        return;
    }
    ffi::wl_resource_post_event(source, ffi::WL_DATA_SOURCE_SEND, mime_type, fd);
    // The source owns writing to fd; close our copy.
    if fd >= 0 {
        libc_close(fd);
    }
}

unsafe extern "C" fn ddev_start_drag(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _source: *mut ffi::wl_resource,
    _origin: *mut ffi::wl_resource,
    _icon: *mut ffi::wl_resource,
    _serial: u32,
) {
    // Drag-and-drop is not implemented; accepting the request keeps the
    // client's lifecycle valid without a protocol error.
}

// ----- zwp_linux_dmabuf_v1 ------------------------------------------------

/// DRM fourccs advertised to clients. ARGB8888 and ABGR8888 are the two
/// 32-bit-per-pixel byte orderings clients actually use; the X-variants are
/// the alpha-undefined counterparts (the server forces alpha opaque).
const DRM_FORMAT_ARGB8888: u32 = 0x3432_5241;
const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258;
const DRM_FORMAT_ABGR8888: u32 = 0x3432_4241;
const DRM_FORMAT_XBGR8888: u32 = 0x3432_4258;
/// DRM modifiers advertised. INVALID lets the driver pick (implicit modifier).
const DRM_FORMAT_MOD_LINEAR: u64 = 0;
const DRM_FORMAT_MOD_INVALID: u64 = 0x00ff_ffff_ffff_ffff;

static DMABUF_IMPL: ffi::zwp_linux_dmabuf_v1_interface_impl =
    ffi::zwp_linux_dmabuf_v1_interface_impl {
        destroy: res_destroy,
        create_params: dmabuf_create_params,
        get_default_feedback: dmabuf_noop_id,
        get_surface_feedback: dmabuf_noop_id_obj,
    };

static PARAMS_IMPL: ffi::zwp_linux_buffer_params_v1_interface_impl =
    ffi::zwp_linux_buffer_params_v1_interface_impl {
        destroy: res_destroy,
        add: params_add,
        create: params_create,
        create_immed: params_create_immed,
    };

static WL_BUFFER_IMPL: ffi::wl_buffer_interface_impl = ffi::wl_buffer_interface_impl {
    destroy: res_destroy,
};

unsafe extern "C" fn dmabuf_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(
        client,
        &ffi::zwp_linux_dmabuf_v1_interface,
        version as c_int,
        id,
    );
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(res, &DMABUF_IMPL as *const _ as *const c_void, data, None);

    // Advertise supported formats (v1+) and format/modifier pairs (v3+).
    for fmt in [
        DRM_FORMAT_ARGB8888,
        DRM_FORMAT_XRGB8888,
        DRM_FORMAT_ABGR8888,
        DRM_FORMAT_XBGR8888,
    ] {
        ffi::wl_resource_post_event(res, ffi::ZWP_LINUX_DMABUF_V1_FORMAT, fmt);
        if version >= 3 {
            for modifier in [DRM_FORMAT_MOD_LINEAR, DRM_FORMAT_MOD_INVALID] {
                let hi = (modifier >> 32) as u32;
                let lo = (modifier & 0xffff_ffff) as u32;
                ffi::wl_resource_post_event(res, ffi::ZWP_LINUX_DMABUF_V1_MODIFIER, fmt, hi, lo);
            }
        }
    }
}

unsafe extern "C" fn dmabuf_create_params(
    client: *mut ffi::wl_client,
    dmabuf: *mut ffi::wl_resource,
    id: u32,
) {
    let ver = ffi::wl_resource_get_version(dmabuf);
    let params =
        ffi::wl_resource_create(client, &ffi::zwp_linux_buffer_params_v1_interface, ver, id);
    if params.is_null() {
        return;
    }
    let acc = Box::into_raw(Box::new(DmabufBuffer::empty()));
    ffi::wl_resource_set_implementation(
        params,
        &PARAMS_IMPL as *const _ as *const c_void,
        acc as *mut c_void,
        Some(params_resource_destroy),
    );
}

unsafe extern "C" fn params_add(
    _client: *mut ffi::wl_client,
    params: *mut ffi::wl_resource,
    fd: i32,
    plane_idx: u32,
    offset: u32,
    stride: u32,
    mod_hi: u32,
    mod_lo: u32,
) {
    let acc = ffi::wl_resource_get_user_data(params) as *mut DmabufBuffer;
    if acc.is_null() || plane_idx != 0 {
        // Multi-plane not supported; close the fd we will not use.
        if fd >= 0 {
            libc_close(fd);
        }
        return;
    }
    if (*acc).have_plane && (*acc).fd >= 0 {
        libc_close((*acc).fd);
    }
    (*acc).fd = fd;
    (*acc).offset = offset;
    (*acc).stride = stride;
    (*acc).modifier = ((mod_hi as u64) << 32) | (mod_lo as u64);
    (*acc).have_plane = true;
}

/// Finalize an accumulated params object into a `wl_buffer`. `id` may be 0 to
/// have the server allocate the id (the `create` path posts `created`), or a
/// client-supplied id (`create_immed`). Returns the new buffer resource.
unsafe fn params_finalize(
    client: *mut ffi::wl_client,
    params: *mut ffi::wl_resource,
    id: u32,
    width: i32,
    height: i32,
    format: u32,
) -> *mut ffi::wl_resource {
    let acc = ffi::wl_resource_get_user_data(params) as *mut DmabufBuffer;
    if acc.is_null() || !(*acc).have_plane || width <= 0 || height <= 0 {
        return std::ptr::null_mut();
    }
    (*acc).width = width;
    (*acc).height = height;
    (*acc).drm_format = format;

    let buffer = ffi::wl_resource_create(client, &ffi::wl_buffer_interface, 1, id);
    if buffer.is_null() {
        return std::ptr::null_mut();
    }
    // Transfer ownership of the DmabufBuffer from params to the buffer.
    ffi::wl_resource_set_user_data(params, std::ptr::null_mut());
    ffi::wl_resource_set_implementation(
        buffer,
        &WL_BUFFER_IMPL as *const _ as *const c_void,
        acc as *mut c_void,
        Some(buffer_resource_destroy),
    );
    buffer
}

unsafe extern "C" fn params_create(
    client: *mut ffi::wl_client,
    params: *mut ffi::wl_resource,
    width: i32,
    height: i32,
    format: u32,
    _flags: u32,
) {
    let buffer = params_finalize(client, params, 0, width, height, format);
    if buffer.is_null() {
        ffi::wl_resource_post_event(params, ffi::ZWP_LINUX_BUFFER_PARAMS_V1_FAILED);
    } else {
        ffi::wl_resource_post_event(params, ffi::ZWP_LINUX_BUFFER_PARAMS_V1_CREATED, buffer);
    }
}

unsafe extern "C" fn params_create_immed(
    client: *mut ffi::wl_client,
    params: *mut ffi::wl_resource,
    buffer_id: u32,
    width: i32,
    height: i32,
    format: u32,
    _flags: u32,
) {
    // Protocol: create_immed failure is fatal to the client. The async `create`
    // path can post `failed` and recover; create_immed cannot. Post the
    // protocol error rather than leaving the client's new-id dangling.
    let buffer = params_finalize(client, params, buffer_id, width, height, format);
    if buffer.is_null() {
        let msg = CString::new("create_immed: missing plane or invalid dimensions").unwrap();
        ffi::wl_resource_post_error(
            params,
            ffi::ZWP_LINUX_BUFFER_PARAMS_V1_ERROR_INVALID_WL_BUFFER,
            msg.as_ptr(),
        );
    }
}

unsafe extern "C" fn params_resource_destroy(resource: *mut ffi::wl_resource) {
    let acc = ffi::wl_resource_get_user_data(resource) as *mut DmabufBuffer;
    if !acc.is_null() {
        drop(Box::from_raw(acc));
    }
}

unsafe extern "C" fn buffer_resource_destroy(resource: *mut ffi::wl_resource) {
    let db = ffi::wl_resource_get_user_data(resource) as *mut DmabufBuffer;
    if !db.is_null() {
        drop(Box::from_raw(db));
    }
}

unsafe extern "C" fn dmabuf_noop_id(_c: *mut ffi::wl_client, _r: *mut ffi::wl_resource, _id: u32) {}
unsafe extern "C" fn dmabuf_noop_id_obj(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _id: u32,
    _surf: *mut ffi::wl_resource,
) {
}

// ----- wp_viewporter ------------------------------------------------------
// Accepts viewport requests so clients that require the global (e.g. mpv) run.
// Source crop / destination scaling are not yet applied to compositing.

static VIEWPORTER_IMPL: ffi::wp_viewporter_interface_impl = ffi::wp_viewporter_interface_impl {
    destroy: res_destroy,
    get_viewport: viewporter_get_viewport,
};

static VIEWPORT_IMPL: ffi::wp_viewport_interface_impl = ffi::wp_viewport_interface_impl {
    destroy: res_destroy,
    set_source: viewport_set_source,
    set_destination: viewport_set_destination,
};

unsafe extern "C" fn viewporter_bind(
    client: *mut ffi::wl_client,
    _data: *mut c_void,
    version: u32,
    id: u32,
) {
    let res = ffi::wl_resource_create(client, &ffi::wp_viewporter_interface, version as c_int, id);
    if res.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        res,
        &VIEWPORTER_IMPL as *const _ as *const c_void,
        std::ptr::null_mut(),
        None,
    );
}

unsafe extern "C" fn viewporter_get_viewport(
    client: *mut ffi::wl_client,
    viewporter: *mut ffi::wl_resource,
    id: u32,
    surface: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
    let ver = ffi::wl_resource_get_version(viewporter);
    let vp = ffi::wl_resource_create(client, &ffi::wp_viewport_interface, ver, id);
    if vp.is_null() || rec.is_null() {
        return;
    }
    ffi::wl_resource_set_implementation(
        vp,
        &VIEWPORT_IMPL as *const _ as *mut c_void,
        rec as *mut c_void,
        None,
    );
}

/// `wl_fixed_t` (24.8 signed) to `f32`. Matches the helper the nested backend
/// uses; duplicated here so ass-server stays independent of ass-backend.
fn fixed_to_f32(v: i32) -> f32 {
    (v as f32) / 256.0
}

/// `wp_viewport.set_source`: sets the source rectangle in surface-local
/// pixel coords (24.8 fixed-point). A value of -1 for every field resets the
/// source to "whole buffer".
unsafe extern "C" fn viewport_set_source(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    if x == -1 && y == -1 && w == -1 && h == -1 {
        (*rec).viewport_src = None;
        return;
    }
    // The spec rejects non-positive width/height with a protocol error; we
    // just ignore the call to keep M2 simple.
    if w <= 0 || h <= 0 {
        return;
    }
    (*rec).viewport_src = Some(ass_core::Rect::new(
        fixed_to_f32(x).round() as i32,
        fixed_to_f32(y).round() as i32,
        fixed_to_f32(w).round() as i32,
        fixed_to_f32(h).round() as i32,
    ));
}

/// `wp_viewport.set_destination`: sets the destination size in integer
/// logical pixels. A value of -1 for either field resets.
unsafe extern "C" fn viewport_set_destination(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    w: i32,
    h: i32,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    if w == -1 && h == -1 {
        (*rec).viewport_dst = None;
        return;
    }
    if w <= 0 || h <= 0 {
        return;
    }
    (*rec).viewport_dst = Some(ass_core::Size { w, h });
}

#[allow(dead_code)]
unsafe extern "C" fn viewport_noop_source(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _x: i32,
    _y: i32,
    _w: i32,
    _h: i32,
) {
}

// ----- shared no-op handlers ----------------------------------------------

pub(crate) unsafe extern "C" fn res_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    ffi::wl_resource_destroy(resource);
}
#[allow(dead_code)]
unsafe extern "C" fn xdg_noop_none(_c: *mut ffi::wl_client, _r: *mut ffi::wl_resource) {}
unsafe extern "C" fn xdg_noop_serial(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _serial: u32,
) {
}
#[allow(dead_code)]
unsafe extern "C" fn xdg_noop_obj(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _obj: *mut ffi::wl_resource,
) {
}
#[allow(dead_code)]
unsafe extern "C" fn xdg_noop_str(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _s: *const std::os::raw::c_char,
) {
}
#[allow(dead_code)]
unsafe extern "C" fn xdg_noop_ii(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _a: i32,
    _b: i32,
) {
}
unsafe extern "C" fn xdg_noop_rect(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _x: i32,
    _y: i32,
    _w: i32,
    _h: i32,
) {
}
#[allow(dead_code)]
unsafe extern "C" fn xdg_noop_seat_serial(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _seat: *mut ffi::wl_resource,
    _serial: u32,
) {
}
#[allow(dead_code)]
unsafe extern "C" fn xdg_noop_resize(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _seat: *mut ffi::wl_resource,
    _serial: u32,
    _edges: u32,
) {
}
unsafe extern "C" fn xdg_noop_menu(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _seat: *mut ffi::wl_resource,
    _serial: u32,
    _x: i32,
    _y: i32,
) {
}

// ----- no-op handlers shared with the extensions module --------------------
//
// These are `pub(crate)` so `extensions.rs` can wire them into request
// vtables without duplicating each trivial handler. They accept the protocol
// arguments and do nothing (or only resource-lifecycle work).

pub(crate) unsafe extern "C" fn noop_none(_c: *mut ffi::wl_client, _r: *mut ffi::wl_resource) {}

pub(crate) unsafe extern "C" fn noop_obj(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _obj: *mut ffi::wl_resource,
) {
}

pub(crate) unsafe extern "C" fn noop_obj_serial(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _obj: *mut ffi::wl_resource,
    _serial: u32,
) {
}

pub(crate) unsafe extern "C" fn noop_serial(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _serial: u32,
) {
}

pub(crate) unsafe extern "C" fn noop_str(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _s: *const std::os::raw::c_char,
) {
}

pub(crate) unsafe extern "C" fn noop_str_ii(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _s: *const std::os::raw::c_char,
    _a: i32,
    _b: i32,
) {
}

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn noop_ii(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _a: i32,
    _b: i32,
) {
}

pub(crate) unsafe extern "C" fn noop_uu(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _a: u32,
    _b: u32,
) {
}

pub(crate) unsafe extern "C" fn noop_rect(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _x: i32,
    _y: i32,
    _w: i32,
    _h: i32,
) {
}

pub(crate) unsafe extern "C" fn noop_region(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _reg: *mut ffi::wl_resource,
) {
}

pub(crate) unsafe extern "C" fn noop_fixed2(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _a: i32,
    _b: i32,
) {
}

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn noop_serial_shape(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _serial: u32,
    _shape: u32,
) {
}

pub(crate) unsafe extern "C" fn noop_uu_one(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _a: u32,
) {
}

// ----- accessors for the extensions module --------------------------------

/// Construct an `ass_core::Point` (re-exported so extensions.rs does not name
/// the crate).
pub(crate) fn ass_core_point(x: i32, y: i32) -> ass_core::Point {
    ass_core::Point { x, y }
}

pub(crate) fn ass_core_size(w: i32, h: i32) -> ass_core::Size {
    ass_core::Size { w, h }
}
