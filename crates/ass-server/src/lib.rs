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
use std::os::raw::{c_int, c_ulong};

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
    acquire_fence: i32,
    resource: *mut ffi::wl_resource,
    state: *mut State,
}

impl DmabufBuffer {
    fn empty(state: *mut State) -> DmabufBuffer {
        DmabufBuffer {
            fd: -1,
            width: 0,
            height: 0,
            drm_format: 0,
            modifier: 0,
            offset: 0,
            stride: 0,
            have_plane: false,
            acquire_fence: -1,
            resource: std::ptr::null_mut(),
            state,
        }
    }

    fn duplicate(&self) -> Option<DmabufBuffer> {
        let fd = unsafe { dup(self.fd) };
        let acquire_fence = if self.acquire_fence >= 0 {
            unsafe { dup(self.acquire_fence) }
        } else {
            -1
        };
        if fd < 0 || (self.acquire_fence >= 0 && acquire_fence < 0) {
            if fd >= 0 {
                unsafe { libc_close(fd) };
            }
            return None;
        }
        Some(DmabufBuffer {
            fd,
            width: self.width,
            height: self.height,
            drm_format: self.drm_format,
            modifier: self.modifier,
            offset: self.offset,
            stride: self.stride,
            have_plane: self.have_plane,
            acquire_fence,
            resource: self.resource,
            state: self.state,
        })
    }
}

impl Drop for DmabufBuffer {
    fn drop(&mut self) {
        if self.fd >= 0 {
            unsafe { libc_close(self.fd) };
        }
        if self.acquire_fence >= 0 {
            unsafe { libc_close(self.acquire_fence) };
        }
    }
}

struct RetiredBufferRelease {
    buffer: *mut ffi::wl_resource,
    explicit_release: *mut ffi::wl_resource,
}

/// The active clipboard selection: the `wl_data_source` that owns it and the
/// MIME types it advertised. Set by `wl_data_device.set_selection`, advertised
/// to every bound `wl_data_device` via `wl_data_offer`.
struct Selection {
    source: *mut ffi::wl_resource,
    mime_types: Vec<String>,
}

/// Server-owned state attached to each `wl_data_source`. Keeping the back
/// pointer here lets the destroy callback invalidate clipboard/drag state
/// before freeing the MIME list.
struct DataSourceRec {
    state: *mut State,
    mime_types: Vec<String>,
    actions: u32,
    actions_set: bool,
    used_for_drag: bool,
}

/// One offer introduced to a destination data device. Selection offers and
/// drag offers share the transfer path, while `is_drag` gates target feedback.
struct DataOfferRec {
    state: *mut State,
    source: *mut ffi::wl_resource,
    is_drag: bool,
    accepted: bool,
    destination_actions: u32,
    preferred_action: u32,
    selected_action: u32,
    dropped: bool,
    finished: bool,
}

struct PrimarySelection {
    source: *mut ffi::wl_resource,
    mime_types: Vec<String>,
}

struct PrimarySourceRec {
    state: *mut State,
    mime_types: Vec<String>,
}

struct PrimaryOfferRec {
    state: *mut State,
    source: *mut ffi::wl_resource,
}

/// Active version-1 drag-and-drop implicit grab.
#[derive(Clone, Copy)]
struct DragState {
    source: *mut ffi::wl_resource,
    origin: *mut ffi::wl_resource,
    focus: *mut ffi::wl_resource,
    target_device: *mut ffi::wl_resource,
    offer: *mut ffi::wl_resource,
    icon: *mut ffi::wl_resource,
}

/// Accumulated `xdg_positioner` state used to compute a popup's placement
/// relative to its parent surface. Only the fields real-world clients set
/// (size, anchor rect, offset) are tracked; anchor edge and gravity default to
/// the common "top-left" so menus and tooltips place predictably.
#[derive(Default)]
struct PositionerState {
    size: Option<ass_core::Size>,
    anchor_rect: Option<ass_core::Rect>,
    anchor: u32,
    gravity: u32,
    constraint_adjustment: u32,
    offset: ass_core::Point,
}

#[derive(Default)]
struct RegionRec {
    rects: Vec<ass_core::Rect>,
}

// Minimal close() without pulling the libc crate.
extern "C" {
    fn close(fd: std::os::raw::c_int) -> std::os::raw::c_int;
    fn dup(fd: std::os::raw::c_int) -> std::os::raw::c_int;
    fn malloc(size: usize) -> *mut c_void;
    fn free(ptr: *mut c_void);
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}
pub(crate) unsafe fn libc_close(fd: i32) {
    close(fd);
}

/// A client surface: its pending buffer, the last committed contents copied out
/// of shm, and its xdg role.
pub struct SurfaceRec {
    pub resource: *mut ffi::wl_resource,
    pending_buffer: *mut ffi::wl_resource,
    pending_buffer_set: bool,
    pending_attach_offset: ass_core::Point,
    attach_offset: ass_core::Point,
    pub mapped: bool,
    pub width: i32,
    pub height: i32,
    /// Logical position of the window rect's top-left corner in compositor
    /// space. For surfaces without a client-declared window geometry this is
    /// also the buffer's draw origin; for CSD surfaces that exclude shadows
    /// via `set_window_geometry` the buffer is drawn up-left of this point
    /// (see [`surface_draw_origin`]). M1 assigns a placeholder cascade on
    /// map; M3's window manager will own placement policy.
    pub position: ass_core::Point,
    /// Last committed contents, tightly packed BGRA8, copied out of the client
    /// shm buffer at commit so the buffer can be released immediately.
    pixels: Vec<u8>,
    /// Bumped on every commit that updates content (shm or dma-buf).
    generation: u64,
    /// True when the committed content is a dma-buf (`dmabuf` holds it),
    /// false when it is the CPU-copied `pixels` from shm.
    content_is_dmabuf: bool,
    /// Compositor-owned duplicate of the committed dma-buf metadata and fd.
    /// The client wl_buffer is released immediately after this duplicate is
    /// made, so its later destruction cannot leave a dangling resource ptr.
    dmabuf: Option<DmabufBuffer>,
    current_buffer: *mut ffi::wl_resource,
    current_explicit_release: *mut ffi::wl_resource,
    explicit_sync: *mut c_void,
    committed_acquire_fence: i32,
    committed_explicit_release: *mut ffi::wl_resource,
    frame_callbacks: Vec<*mut ffi::wl_resource>,
    // xdg-shell role state.
    xdg_surface: *mut ffi::wl_resource,
    xdg_toplevel: *mut ffi::wl_resource,
    xdg_decoration: *mut ffi::wl_resource,
    xdg_popup: *mut ffi::wl_resource,
    popup_parent: *mut SurfaceRec,
    popup_grabbed: bool,
    cursor_role: bool,
    drag_icon_role: bool,
    /// ext-session-lock role record, owned by the protocol resource.
    session_lock_surface: *mut c_void,
    xdg_configured: bool,
    xdg_configure_acked: bool,
    pending_xdg_configures: Vec<u32>,
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
    subsurface_sync: bool,
    subsurface_cached_commit: bool,
    subsurface_applying_cached: bool,
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
    pending_viewport_src: Option<Option<ass_core::Rect>>,
    /// Destination size in logical pixels, or None for "source size".
    /// Set by `wp_viewport.set_destination`.
    pub viewport_dst: Option<ass_core::Size>,
    pending_viewport_dst: Option<Option<ass_core::Size>>,
    viewport_resource: *mut ffi::wl_resource,
    // ----- wp_fractional_scale_v1 state -----
    /// The `wp_fractional_scale_v1` resource bound for this surface, if any.
    /// The server posts `preferred_scale` here when the output's scale changes.
    pub fractional_scale: *mut ffi::wl_resource,
    /// Committed xdg-shell window geometry (excluding client shadows). Its
    /// size is the window rect's size; its origin is the frame inset by
    /// which the buffer sits up-left of the window rect (see
    /// [`surface_draw_origin`]).
    window_geometry: Option<ass_core::Rect>,
    pending_window_geometry: Option<ass_core::Rect>,
    /// `None` means the whole surface accepts input; `Some` is the union of
    /// rectangles copied from the last committed `wl_region`.
    input_region: Option<Vec<ass_core::Rect>>,
    pending_input_region: Option<Option<Vec<ass_core::Rect>>>,
    // ----- pending buffer transform / scale -----
    /// Pending buffer transform from `wl_surface.set_buffer_transform`,
    /// applied on the next commit.
    pending_transform: ass_core::Transform,
    buffer_transform: ass_core::Transform,
    /// Pending buffer scale from `wl_surface.set_buffer_scale`.
    pending_scale: i32,
    buffer_scale: i32,
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
            pending_buffer_set: false,
            pending_attach_offset: ass_core::Point::default(),
            attach_offset: ass_core::Point::default(),
            mapped: false,
            width: 0,
            height: 0,
            position: ass_core::Point::default(),
            pixels: Vec::new(),
            generation: 0,
            content_is_dmabuf: false,
            dmabuf: None,
            current_buffer: std::ptr::null_mut(),
            current_explicit_release: std::ptr::null_mut(),
            explicit_sync: std::ptr::null_mut(),
            committed_acquire_fence: -1,
            committed_explicit_release: std::ptr::null_mut(),
            frame_callbacks: Vec::new(),
            xdg_surface: std::ptr::null_mut(),
            xdg_toplevel: std::ptr::null_mut(),
            xdg_decoration: std::ptr::null_mut(),
            xdg_popup: std::ptr::null_mut(),
            popup_parent: std::ptr::null_mut(),
            popup_grabbed: false,
            cursor_role: false,
            drag_icon_role: false,
            session_lock_surface: std::ptr::null_mut(),
            xdg_configured: false,
            xdg_configure_acked: false,
            pending_xdg_configures: Vec::new(),
            display: std::ptr::null_mut(),
            state: std::ptr::null_mut(),
            index: 0,
            parent: std::ptr::null_mut(),
            children: Vec::new(),
            subsurface_offset: ass_core::Point::default(),
            subsurface_above_parent: true,
            subsurface_sync: true,
            subsurface_cached_commit: false,
            subsurface_applying_cached: false,
            window: ass_core::window::Window::default(),
            viewport_src: None,
            pending_viewport_src: None,
            viewport_dst: None,
            pending_viewport_dst: None,
            viewport_resource: std::ptr::null_mut(),
            fractional_scale: std::ptr::null_mut(),
            window_geometry: None,
            pending_window_geometry: None,
            input_region: None,
            pending_input_region: None,
            pending_transform: ass_core::Transform::Normal,
            buffer_transform: ass_core::Transform::Normal,
            pending_scale: 1,
            buffer_scale: 1,
            pending_damage: Vec::new(),
            committed_damage: Vec::new(),
            // Tiling (ADR-0024): the last layout rect we configured this
            // surface to. `None` until applied; the apply path reconfigures
            // only when the target moves, so steady state sends no configures.
            layout_target: None,
        }
    }
}

fn surface_logical_size(surface: &SurfaceRec) -> ass_core::Size {
    if let Some(destination) = surface.viewport_dst {
        return destination;
    }
    let scale = surface.buffer_scale.max(1) as f32;
    if let Some(source) = surface.viewport_src {
        return source.size;
    }
    let (width, height) = if surface.buffer_transform.swap_axes() {
        (surface.height, surface.width)
    } else {
        (surface.width, surface.height)
    };
    ass_core::Size {
        w: (width as f32 / scale).round().max(1.0) as i32,
        h: (height as f32 / scale).round().max(1.0) as i32,
    }
}

/// Draw origin of the surface's buffer in compositor space. For surfaces
/// with a client-declared window geometry (xdg-shell `set_window_geometry`,
/// used by client-side-decorated windows to exclude shadows), the buffer is
/// drawn up-left of the window rect by the geometry's insets. A subsurface
/// is anchored in its parent's buffer space, so its origin resolves through
/// the parent chain — this is what makes nested subsurfaces (a subsurface
/// with its own subsurfaces) land at the right compositor position.
pub(crate) fn surface_draw_origin(surface: &SurfaceRec) -> ass_core::Point {
    surface_draw_origin_depth(surface, 0)
}

fn surface_draw_origin_depth(surface: &SurfaceRec, depth: u32) -> ass_core::Point {
    // The depth cap only breaks reference cycles defensively; the destroy
    // path orphans children, so a live parent pointer is always valid.
    if !surface.parent.is_null() && depth < 32 {
        let parent = unsafe { &*surface.parent };
        let origin = surface_draw_origin_depth(parent, depth + 1);
        return ass_core::Point {
            x: origin.x + surface.subsurface_offset.x,
            y: origin.y + surface.subsurface_offset.y,
        };
    }
    match surface.window_geometry {
        Some(geometry) => ass_core::Point {
            x: surface.position.x - geometry.origin.x,
            y: surface.position.y - geometry.origin.y,
        },
        None => surface.position,
    }
}

/// Whether compositor-space `(x, y)` lands on the surface: inside its rect
/// and accepted by its input region. An xdg-role surface presents its
/// declared window geometry at `position` (the window-rect origin, so CSD
/// shadows neither paint nor take input); a subsurface presents its buffer
/// at its draw origin. The input region is always buffer-local, anchored at
/// the draw origin.
fn surface_accepts_point(s: &SurfaceRec, x: f32, y: f32) -> bool {
    let draw_origin = surface_draw_origin(s);
    let (sx, sy, logical) = if !s.xdg_toplevel.is_null() || !s.xdg_popup.is_null() {
        (
            s.position.x as f32,
            s.position.y as f32,
            s.window_geometry
                .map(|geometry| geometry.size)
                .unwrap_or_else(|| surface_logical_size(s)),
        )
    } else {
        (
            draw_origin.x as f32,
            draw_origin.y as f32,
            surface_logical_size(s),
        )
    };
    if x < sx || y < sy || x >= sx + logical.w as f32 || y >= sy + logical.h as f32 {
        return false;
    }
    let local_x = x - draw_origin.x as f32;
    let local_y = y - draw_origin.y as f32;
    s.input_region.as_ref().is_none_or(|rects| {
        rects.iter().any(|rect| {
            rect.contains(ass_core::Point {
                x: local_x as i32,
                y: local_y as i32,
            })
        })
    })
}

/// `wp_cursor_shape_device_v1.shape` value for one xdg-shell resize edge set.
fn resize_cursor_shape(edges: ass_core::window::ResizeEdges) -> u32 {
    use ass_core::window::ResizeEdges;
    match (
        edges.has_top(),
        edges.has_bottom(),
        edges.has_left(),
        edges.has_right(),
    ) {
        (true, false, true, false) => 21,  // nw_resize
        (true, false, false, true) => 20,  // ne_resize
        (false, true, true, false) => 24,  // sw_resize
        (false, true, false, true) => 23,  // se_resize
        (true, false, false, false) => 19, // n_resize
        (false, true, false, false) => 22, // s_resize
        (false, false, true, false) => 25, // w_resize
        (false, false, false, true) => 18, // e_resize
        _ if edges == ResizeEdges::NONE => 1,
        _ => 1,
    }
}

fn surface_has_role(surface: &SurfaceRec) -> bool {
    !surface.xdg_surface.is_null()
        || !surface.parent.is_null()
        || surface.cursor_role
        || surface.drag_icon_role
        || !surface.session_lock_surface.is_null()
}

/// Resolve a newly mapped toplevel's layout role: an explicit window
/// rule wins; a transient (dialog) always floats (ADR-0024 floating
/// exception); otherwise the workspace's tiled flag decides.
fn resolve_layout_role(
    workspace_tiled: bool,
    is_transient: bool,
    rule_role: Option<ass_core::layout::LayoutRole>,
) -> ass_core::layout::LayoutRole {
    use ass_core::layout::LayoutRole;
    if let Some(role) = rule_role {
        return role;
    }
    if is_transient {
        return LayoutRole::Floating;
    }
    if workspace_tiled {
        LayoutRole::Tiled
    } else {
        LayoutRole::Floating
    }
}

unsafe fn update_overlay_positions(state: *mut State) {
    if !(*state).cursor_surface.is_null() {
        let rec = ffi::wl_resource_get_user_data((*state).cursor_surface) as *mut SurfaceRec;
        if !rec.is_null() {
            (*rec).position = ass_core::Point {
                x: (*state).pointer_x.round() as i32 - (*state).cursor_hotspot.x
                    + (*rec).attach_offset.x,
                y: (*state).pointer_y.round() as i32 - (*state).cursor_hotspot.y
                    + (*rec).attach_offset.y,
            };
        }
    }
    if let Some(drag) = (*state).drag {
        if !drag.icon.is_null() {
            let rec = ffi::wl_resource_get_user_data(drag.icon) as *mut SurfaceRec;
            if !rec.is_null() {
                (*rec).position = ass_core::Point {
                    x: (*state).pointer_x.round() as i32 + (*rec).attach_offset.x,
                    y: (*state).pointer_y.round() as i32 + (*rec).attach_offset.y,
                };
            }
        }
    }
}

unsafe fn send_xdg_surface_configure(rec: *mut SurfaceRec) -> Option<u32> {
    if rec.is_null() || (*rec).xdg_surface.is_null() || (*rec).display.is_null() {
        return None;
    }
    let serial = ffi::wl_display_next_serial((*rec).display);
    (*rec).pending_xdg_configures.push(serial);
    ffi::wl_resource_post_event((*rec).xdg_surface, ffi::XDG_SURFACE_CONFIGURE, serial);
    Some(serial)
}

unsafe fn surface_root_toplevel(mut surface: *mut SurfaceRec) -> *mut SurfaceRec {
    for _ in 0..32 {
        if surface.is_null() || !(*surface).xdg_toplevel.is_null() {
            return surface;
        }
        surface = if !(*surface).popup_parent.is_null() {
            (*surface).popup_parent
        } else {
            (*surface).parent
        };
    }
    std::ptr::null_mut()
}

/// One dynamically advertised wl_output global. Boxes remain allocated after
/// hot-unplug until server teardown because clients may retain resources whose
/// user-data points here even after the registry global is removed.
pub(crate) struct OutputGlobal {
    state: *mut State,
    info: ass_core::output::OutputInfo,
    global: *mut ffi::wl_global,
    active: bool,
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
    // Each address is wl_global callback data and must survive Vec growth.
    #[allow(clippy::vec_box)]
    output_globals: Vec<Box<OutputGlobal>>,
    /// Every `xdg_output` resource clients have bound (zxdg-output v1). Resent
    /// together with the wl_output reconfigure path.
    pub(crate) xdg_output_resources: Vec<*mut ffi::wl_resource>,
    pub(crate) xdg_output_links: std::collections::HashMap<usize, *mut ffi::wl_resource>,
    /// Every `wl_data_device` resource clients have bound. A `set_selection`
    /// advertises a new `wl_data_offer` to each.
    data_devices: Vec<*mut ffi::wl_resource>,
    /// Every live `wl_data_offer`; destroy callbacks null their slot. Tracking
    /// lets source teardown invalidate late receive requests safely.
    data_offers: Vec<*mut ffi::wl_resource>,
    /// The current clipboard selection, if any.
    selection: Option<Selection>,
    primary_devices: Vec<*mut ffi::wl_resource>,
    primary_offers: Vec<*mut ffi::wl_resource>,
    primary_selection: Option<PrimarySelection>,
    /// Active data-device drag, if a client owns the pointer implicit grab.
    drag: Option<DragState>,
    /// Active `zwp_relative_pointer_v1` resources. Relative-motion deltas are
    /// posted to each (filtered to the focused client's set).
    relative_pointers: Vec<*mut ffi::wl_resource>,
    pointer_constraints: Vec<*mut ffi::wl_resource>,
    pointer_gesture_swipes: Vec<*mut ffi::wl_resource>,
    pointer_gesture_pinches: Vec<*mut ffi::wl_resource>,
    pointer_gesture_holds: Vec<*mut ffi::wl_resource>,
    swipe_gesture_client: *mut ffi::wl_client,
    pinch_gesture_client: *mut ffi::wl_client,
    hold_gesture_client: *mut ffi::wl_client,
    keyboard_shortcut_inhibitors: Vec<*mut ffi::wl_resource>,
    /// Live ext-idle-notify and idle-inhibit protocol resources. Per-object
    /// timer/role state is owned by resource user data in `extensions.rs`.
    pub(crate) idle_notifications: Vec<*mut ffi::wl_resource>,
    pub(crate) idle_inhibitors: Vec<*mut ffi::wl_resource>,
    pub(crate) tablet_seats: Vec<*mut ffi::wl_resource>,
    /// Live `zwp_tablet_v2` resources, one per seat that has been told about
    /// the compositor's single synthetic tablet device.
    pub(crate) tablet_devices: Vec<*mut ffi::wl_resource>,
    /// Live `zwp_tablet_tool_v2` resources. Each carries a `TabletToolRec`
    /// naming the physical tool id it proxies.
    pub(crate) tablet_tools: Vec<*mut ffi::wl_resource>,
    /// Physical tablet tools seen so far, with their announced info. A tool
    /// is announced to every seat the first time it enters proximity.
    pub(crate) known_tools: Vec<(u64, ass_core::input::TabletToolInfo)>,
    /// Whether the synthetic tablet device has been announced yet (set on
    /// first proximity). Gates `tablet_added` for late-binding seats.
    pub(crate) tablet_device_seen: bool,
    /// Surface holding tablet proximity focus, or null when no tool is in
    /// proximity over a tablet-aware client (the pen emulates the pointer).
    pub(crate) tablet_focus: *mut ffi::wl_resource,
    retired_buffer_releases: Vec<RetiredBufferRelease>,
    /// Bound `ext_foreign_toplevel_list_v1` resources. New toplevels, title
    /// changes, and removals are pushed to each.
    foreign_toplevel_lists: Vec<*mut ffi::wl_resource>,
    /// Per-toplevel foreign handle resources, keyed by window id. Lets the
    /// server push title/app_id/closed updates to the right handle.
    foreign_handles: std::collections::HashMap<u64, Vec<*mut ffi::wl_resource>>,
    /// Active `zwp_text_input_v3` resources. Per-object double-buffered state
    /// lives in the resource's `TextInputRec` user data.
    text_inputs: Vec<*mut ffi::wl_resource>,
    /// Committed state waiting to be mirrored to the nested host's text-input
    /// object. The main loop drains this after dispatching client requests.
    pending_text_input_states: Vec<ass_core::input::TextInputState>,
    activation_tokens: std::collections::HashSet<String>,
    pending_activation: *mut ffi::wl_resource,
    /// Active ext-session-lock object and fail-closed visibility state.
    pub(crate) session_lock: *mut c_void,
    pub(crate) session_locked: bool,
    pub(crate) lock_focus_dirty: bool,
    pub(crate) pending_lock_focus: *mut ffi::wl_resource,
    pub(crate) pre_lock_keyboard_focus: *mut ffi::wl_resource,
    pub(crate) session_lock_requested_at: Option<std::time::Instant>,
    pub(crate) lock_frame_pending: bool,
    /// The last cursor shape requested by the focused client via
    /// `wp_cursor_shape_device_v1.set_shape` (or `wl_pointer.set_cursor`,
    /// once wired). 0 = default arrow. Exposed to the renderer.
    cursor_shape: u32,
    cursor_surface: *mut ffi::wl_resource,
    cursor_hotspot: ass_core::Point,
    cursor_hidden: bool,
    last_pointer_enter_serial: u32,
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
    raw_pointer_x: f32,
    raw_pointer_y: f32,
    /// Last serial handed to a `wl_pointer.button` event, for clients that
    /// gate interactive moves on a press serial (xdg_toplevel.move &c.).
    last_button_serial: u32,
    /// Whether the press that produced `last_button_serial` still owns an
    /// implicit pointer grab. Move/resize/start_drag requests require it.
    implicit_grab_active: bool,
    /// Current depressed modifier mask from the compositor's xkb state. Used
    /// for compositor-owned pointer gestures such as Super+drag.
    depressed_mods: ass_core::input::Mods,
    /// Pending console VT switch requested by a Ctrl+Alt+Fn key press
    /// (XF86Switch_VT_N). The kernel never sees these keys once libinput owns
    /// evdev, so the compositor performs the session switch through libseat.
    /// Drained by the main loop via [`Server::take_vt_switch`].
    pending_vt_switch: Option<i32>,
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
    /// Accessibility reduced-motion policy (ADR-0029): when true, window
    /// transitions resolve in one frame and none are recorded.
    reduced_motion: bool,
    /// Config-driven window rules (ADR-0026). Evaluated on first map; the
    /// first match prescribes a workspace move and/or a forced layout role.
    window_rules: Vec<ass_core::window_rule::WindowRule>,
    /// The focused output's geometry (ADR-0028): the tiling work-area is its
    /// logical rect. Updated by the backend on resize; defaults to identity.
    pub(crate) output_geometry: ass_core::output::OutputGeometry,
    /// Backend-reported connector geometry in global logical coordinates.
    /// The first entry is the primary/focused output exposed through the
    /// legacy single wl_output global until per-global resources are split.
    output_infos: Vec<ass_core::output::OutputInfo>,
    /// Per-connector output policies from `[[output]]` config entries
    /// (ADR-0028). Applied to every backend-reported output set in
    /// `set_outputs`.
    output_policies: std::collections::HashMap<String, ass_core::output::OutputPolicy>,
    /// Dynamic per-output workspaces (ADR-0025). Toplevels are placed on the
    /// current workspace at first map; rendering and input see only the
    /// visible set (`visible_toplevels`).
    workspaces: ass_core::workspace::WorkspaceModel,
    /// Focused output for new surfaces and workspace commands.
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
            output_globals: Vec::new(),
            xdg_output_resources: Vec::new(),
            xdg_output_links: std::collections::HashMap::new(),
            data_devices: Vec::new(),
            data_offers: Vec::new(),
            selection: None,
            primary_devices: Vec::new(),
            primary_offers: Vec::new(),
            primary_selection: None,
            drag: None,
            relative_pointers: Vec::new(),
            pointer_constraints: Vec::new(),
            pointer_gesture_swipes: Vec::new(),
            pointer_gesture_pinches: Vec::new(),
            pointer_gesture_holds: Vec::new(),
            swipe_gesture_client: std::ptr::null_mut(),
            pinch_gesture_client: std::ptr::null_mut(),
            hold_gesture_client: std::ptr::null_mut(),
            keyboard_shortcut_inhibitors: Vec::new(),
            idle_notifications: Vec::new(),
            idle_inhibitors: Vec::new(),
            tablet_seats: Vec::new(),
            tablet_devices: Vec::new(),
            tablet_tools: Vec::new(),
            known_tools: Vec::new(),
            tablet_device_seen: false,
            tablet_focus: std::ptr::null_mut(),
            retired_buffer_releases: Vec::new(),
            foreign_toplevel_lists: Vec::new(),
            foreign_handles: std::collections::HashMap::new(),
            text_inputs: Vec::new(),
            pending_text_input_states: Vec::new(),
            activation_tokens: std::collections::HashSet::new(),
            pending_activation: std::ptr::null_mut(),
            session_lock: std::ptr::null_mut(),
            session_locked: false,
            lock_focus_dirty: false,
            pending_lock_focus: std::ptr::null_mut(),
            pre_lock_keyboard_focus: std::ptr::null_mut(),
            session_lock_requested_at: None,
            lock_frame_pending: false,
            cursor_shape: 0,
            cursor_surface: std::ptr::null_mut(),
            cursor_hotspot: ass_core::Point::default(),
            cursor_hidden: false,
            last_pointer_enter_serial: 0,
            pointer_focus: std::ptr::null_mut(),
            keyboard_focus: std::ptr::null_mut(),
            saved_keyboard_focus: std::ptr::null_mut(),
            pointer_x: 0.0,
            pointer_y: 0.0,
            raw_pointer_x: 0.0,
            raw_pointer_y: 0.0,
            last_button_serial: 0,
            implicit_grab_active: false,
            depressed_mods: ass_core::input::Mods::NONE,
            pending_vt_switch: None,
            keyboard: None,
            interactive: None,
            compositor_pointer_grab: false,
            workspaces,
            output,
            layout_params: ass_core::layout::LayoutParams::default(),
            reduced_motion: false,
            window_rules: Vec::new(),
            output_geometry: ass_core::output::OutputGeometry::default(),
            output_infos: vec![ass_core::output::OutputInfo {
                connector: "nested".to_owned(),
                geometry: ass_core::output::OutputGeometry::default(),
                available_modes: Vec::new(),
            }],
            output_policies: std::collections::HashMap::new(),
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
        Self::new_with_render_caps(true, true)
    }

    /// Create a server whose advertised buffer protocols match the actual
    /// Vulkan device. Clients must never discover dma-buf or explicit-sync
    /// globals that the renderer cannot honor.
    pub fn new_with_render_caps(
        dmabuf_supported: bool,
        explicit_sync_supported: bool,
    ) -> Result<Server, ServerError> {
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
            let initial_output = state.output_infos[0].clone();
            create_output_global(state.as_mut(), initial_output);
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
            if dmabuf_supported {
                ffi::wl_global_create(
                    display,
                    &ffi::zwp_linux_dmabuf_v1_interface,
                    3,
                    data,
                    dmabuf_bind,
                );
            }
            if dmabuf_supported && explicit_sync_supported {
                ffi::wl_global_create(
                    display,
                    &ffi::zwp_linux_explicit_synchronization_v1_interface,
                    1,
                    data,
                    extensions::explicit_sync_bind,
                );
            }
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
                &ffi::zxdg_decoration_manager_v1_interface,
                2,
                data,
                extensions::xdg_decoration_manager_bind,
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
                2,
                data,
                extensions::idle_notifier_bind,
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
                &ffi::zwp_pointer_gestures_v1_interface,
                3,
                data,
                extensions::pointer_gestures_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::zwp_tablet_manager_v2_interface,
                1,
                data,
                extensions::tablet_manager_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::zwp_keyboard_shortcuts_inhibit_manager_v1_interface,
                1,
                data,
                extensions::keyboard_shortcuts_inhibit_bind,
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
                &ffi::zwp_primary_selection_device_manager_v1_interface,
                1,
                data,
                extensions::primary_selection_bind,
            );
            ffi::wl_global_create(
                display,
                &ffi::wp_cursor_shape_manager_v1_interface,
                2,
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
            ffi::wl_global_create(
                display,
                &ffi::xdg_activation_v1_interface,
                1,
                data,
                extensions::xdg_activation_bind,
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
        if !self.state.pending_activation.is_null() {
            let surface =
                std::mem::replace(&mut self.state.pending_activation, std::ptr::null_mut());
            self.change_keyboard_focus(surface);
        }
        if self.state.lock_focus_dirty {
            self.state.lock_focus_dirty = false;
            let focus = self.state.pending_lock_focus;
            self.change_pointer_focus(focus);
            self.change_keyboard_focus(focus);
        }
        if self.state.session_locked
            && self
                .state
                .session_lock_requested_at
                .is_some_and(|started| started.elapsed() >= std::time::Duration::from_secs(1))
        {
            // The compositor-rendered opaque fallback is sufficient after the
            // bounded grace period even if the locker has not mapped every
            // output yet. The event is deferred until presentation_complete.
            self.state.lock_frame_pending = true;
        }
        unsafe { extensions::update_idle_notifications(self.state.as_mut()) };
    }

    /// Whether normal client content and compositor chrome must be hidden.
    /// This becomes true as soon as a lock request is accepted, before the
    /// protocol's `locked` event, so the next frame fails closed.
    pub fn session_locked(&self) -> bool {
        self.state.session_locked
    }

    /// A newly blanked/locked frame must be confirmed on every output before
    /// the protocol lock request can be acknowledged.
    pub fn lock_confirmation_pending(&self) -> bool {
        self.state.session_locked && self.state.lock_frame_pending
    }

    /// Confirm that the just-submitted secure frame reached all outputs.
    pub fn presentation_complete(&mut self) {
        unsafe { extensions::session_lock_presented(self.state.as_mut()) };
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
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
                let root =
                    unsafe { surface_root_toplevel(*s as *const SurfaceRec as *mut SurfaceRec) };
                s.mapped
                    && (!s.xdg_toplevel.is_null() || !s.xdg_popup.is_null())
                    && !root.is_null()
                    && unsafe { !(*root).window.minimized }
                    && !s.content_is_dmabuf
                    && !s.pixels.is_empty()
                    && visible.contains(unsafe { &(*root).window.id })
            })
            .map(|s| {
                // ADR-0029: while a transition is in flight the frame renders
                // at the interpolated rect; the model stays at the target.
                // The origin delta carries the whole subsurface tree with it.
                let render_rect = self.transition_render_rect(s);
                let mut origin = surface_draw_origin(s);
                if let Some(r) = render_rect {
                    origin.x += r.origin.x - s.position.x;
                    origin.y += r.origin.y - s.position.y;
                }
                SurfacePixels {
                    id: s.resource as usize,
                    window: if s.xdg_toplevel.is_null() {
                        let root =
                            unsafe { surface_root_toplevel(s as *const SurfaceRec as *mut _) };
                        if root.is_null() {
                            None
                        } else {
                            Some(unsafe { &*root }.window.id)
                        }
                    } else {
                        Some(s.window.id)
                    },
                    width: s.width,
                    height: s.height,
                    generation: s.generation,
                    pixels: &s.pixels,
                    geometry: ass_core::SurfaceGeometry {
                        position: origin,
                        transform: s.buffer_transform,
                        buffer_scale: s.buffer_scale,
                        viewport_src: s.viewport_src,
                        viewport_dst: s.viewport_dst,
                        transition_size: render_rect.map(|r| r.size),
                        ..Default::default()
                    },
                    damage: &s.committed_damage,
                }
            })
            .collect()
    }

    /// Mapped xdg-toplevel surfaces backed by a dma-buf, for the renderer to
    /// import zero-copy. The `fd` is borrowed; the renderer duplicates it
    /// before Flux consumes the duplicate. The server keeps ownership until
    /// the backing buffer is replaced or destroyed.
    pub fn toplevel_dmabuf_frames(&self) -> Vec<SurfaceDmabuf> {
        let visible = self.visible();
        self.state
            .surfaces
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .map(|p| unsafe { &*p })
            .filter(|s| {
                let root =
                    unsafe { surface_root_toplevel(*s as *const SurfaceRec as *mut SurfaceRec) };
                s.mapped
                    && (!s.xdg_toplevel.is_null() || !s.xdg_popup.is_null())
                    && !root.is_null()
                    && unsafe { !(*root).window.minimized }
                    && s.content_is_dmabuf
                    && s.dmabuf.is_some()
                    && visible.contains(unsafe { &(*root).window.id })
            })
            .filter_map(|s| {
                let db = s.dmabuf.as_ref()?;
                let render_rect = self.transition_render_rect(s);
                let mut origin = surface_draw_origin(s);
                if let Some(r) = render_rect {
                    origin.x += r.origin.x - s.position.x;
                    origin.y += r.origin.y - s.position.y;
                }
                Some(SurfaceDmabuf {
                    id: s.resource as usize,
                    window: if s.xdg_toplevel.is_null() {
                        let root =
                            unsafe { surface_root_toplevel(s as *const SurfaceRec as *mut _) };
                        if root.is_null() {
                            None
                        } else {
                            Some(unsafe { &*root }.window.id)
                        }
                    } else {
                        Some(s.window.id)
                    },
                    width: s.width,
                    height: s.height,
                    generation: s.generation,
                    fd: db.fd,
                    drm_format: db.drm_format,
                    modifier: db.modifier,
                    offset: db.offset,
                    stride: db.stride,
                    acquire_fence: db.acquire_fence,
                    geometry: ass_core::SurfaceGeometry {
                        position: origin,
                        transform: s.buffer_transform,
                        buffer_scale: s.buffer_scale,
                        viewport_src: s.viewport_src,
                        viewport_dst: s.viewport_dst,
                        transition_size: render_rect.map(|r| r.size),
                        ..Default::default()
                    },
                })
            })
            .collect()
    }

    /// Mapped session-lock surfaces backed by shm. The compositor renders
    /// these over an opaque fallback and never mixes them with normal clients.
    pub fn lock_frames(&self) -> Vec<SurfacePixels<'_>> {
        let mut frames = self
            .state
            .live_surfaces()
            .map(|surface| unsafe { &*surface })
            .filter(|surface| unsafe {
                extensions::is_active_session_lock_surface(
                    self.state.as_ref() as *const State as *mut State,
                    *surface as *const SurfaceRec as *mut SurfaceRec,
                )
            })
            .filter(|surface| {
                surface.mapped && !surface.content_is_dmabuf && !surface.pixels.is_empty()
            })
            .map(|surface| SurfacePixels {
                window: None,
                id: surface.resource as usize,
                width: surface.width,
                height: surface.height,
                generation: surface.generation,
                pixels: &surface.pixels,
                geometry: ass_core::SurfaceGeometry {
                    position: surface.position,
                    transform: surface.buffer_transform,
                    buffer_scale: surface.buffer_scale,
                    viewport_src: surface.viewport_src,
                    viewport_dst: surface.viewport_dst,
                    ..Default::default()
                },
                damage: &surface.committed_damage,
            })
            .collect::<Vec<_>>();
        let cursor = self.state.cursor_surface;
        if !cursor.is_null()
            && unsafe {
                extensions::is_active_session_lock_client_resource(
                    self.state.as_ref() as *const State as *mut State,
                    cursor,
                )
            }
        {
            let surface = unsafe { ffi::wl_resource_get_user_data(cursor) as *mut SurfaceRec };
            if !surface.is_null() {
                let surface = unsafe { &*surface };
                if surface.mapped && !surface.content_is_dmabuf && !surface.pixels.is_empty() {
                    frames.push(SurfacePixels {
                        window: None,
                        id: surface.resource as usize,
                        width: surface.width,
                        height: surface.height,
                        generation: surface.generation,
                        pixels: &surface.pixels,
                        geometry: ass_core::SurfaceGeometry {
                            position: surface.position,
                            transform: surface.buffer_transform,
                            buffer_scale: surface.buffer_scale,
                            viewport_src: surface.viewport_src,
                            viewport_dst: surface.viewport_dst,
                            ..Default::default()
                        },
                        damage: &surface.committed_damage,
                    });
                }
            }
        }
        frames
    }

    /// dma-buf variant of [`lock_frames`](Self::lock_frames).
    pub fn lock_dmabuf_frames(&self) -> Vec<SurfaceDmabuf> {
        let mut frames = self
            .state
            .live_surfaces()
            .map(|surface| unsafe { &*surface })
            .filter(|surface| unsafe {
                extensions::is_active_session_lock_surface(
                    self.state.as_ref() as *const State as *mut State,
                    *surface as *const SurfaceRec as *mut SurfaceRec,
                )
            })
            .filter(|surface| surface.mapped && surface.content_is_dmabuf)
            .filter_map(|surface| {
                let buffer = surface.dmabuf.as_ref()?;
                Some(SurfaceDmabuf {
                    window: None,
                    id: surface.resource as usize,
                    width: surface.width,
                    height: surface.height,
                    generation: surface.generation,
                    fd: buffer.fd,
                    drm_format: buffer.drm_format,
                    modifier: buffer.modifier,
                    offset: buffer.offset,
                    stride: buffer.stride,
                    acquire_fence: buffer.acquire_fence,
                    geometry: ass_core::SurfaceGeometry {
                        position: surface.position,
                        transform: surface.buffer_transform,
                        buffer_scale: surface.buffer_scale,
                        viewport_src: surface.viewport_src,
                        viewport_dst: surface.viewport_dst,
                        ..Default::default()
                    },
                })
            })
            .collect::<Vec<_>>();
        let cursor = self.state.cursor_surface;
        if !cursor.is_null()
            && unsafe {
                extensions::is_active_session_lock_client_resource(
                    self.state.as_ref() as *const State as *mut State,
                    cursor,
                )
            }
        {
            let surface = unsafe { ffi::wl_resource_get_user_data(cursor) as *mut SurfaceRec };
            if !surface.is_null() {
                let surface = unsafe { &*surface };
                if surface.mapped && surface.content_is_dmabuf {
                    if let Some(buffer) = surface.dmabuf.as_ref() {
                        frames.push(SurfaceDmabuf {
                            window: None,
                            id: surface.resource as usize,
                            width: surface.width,
                            height: surface.height,
                            generation: surface.generation,
                            fd: buffer.fd,
                            drm_format: buffer.drm_format,
                            modifier: buffer.modifier,
                            offset: buffer.offset,
                            stride: buffer.stride,
                            acquire_fence: buffer.acquire_fence,
                            geometry: ass_core::SurfaceGeometry {
                                position: surface.position,
                                transform: surface.buffer_transform,
                                buffer_scale: surface.buffer_scale,
                                viewport_src: surface.viewport_src,
                                viewport_dst: surface.viewport_dst,
                                ..Default::default()
                            },
                        });
                    }
                }
            }
        }
        frames
    }

    /// Cursor and drag-icon role surfaces, composited above all client
    /// toplevels and subsurfaces in drag-icon-then-cursor order.
    pub fn overlay_frames(&self) -> Vec<SurfacePixels<'_>> {
        let drag_icon = self
            .state
            .drag
            .as_ref()
            .map_or(std::ptr::null_mut(), |drag| drag.icon);
        [drag_icon, self.state.cursor_surface]
            .into_iter()
            .filter(|resource| !resource.is_null())
            .filter_map(|resource| {
                let rec = unsafe { ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec };
                if rec.is_null() {
                    return None;
                }
                let surface = unsafe { &*rec };
                if !surface.mapped || surface.content_is_dmabuf || surface.pixels.is_empty() {
                    return None;
                }
                Some(SurfacePixels {
                    window: None,
                    id: surface.resource as usize,
                    width: surface.width,
                    height: surface.height,
                    generation: surface.generation,
                    pixels: &surface.pixels,
                    geometry: ass_core::SurfaceGeometry {
                        position: surface.position,
                        transform: surface.buffer_transform,
                        buffer_scale: surface.buffer_scale,
                        viewport_src: surface.viewport_src,
                        viewport_dst: surface.viewport_dst,
                        ..Default::default()
                    },
                    damage: &surface.committed_damage,
                })
            })
            .collect()
    }

    /// dma-buf variant of [`overlay_frames`](Self::overlay_frames).
    pub fn overlay_dmabuf_frames(&self) -> Vec<SurfaceDmabuf> {
        let drag_icon = self
            .state
            .drag
            .as_ref()
            .map_or(std::ptr::null_mut(), |drag| drag.icon);
        [drag_icon, self.state.cursor_surface]
            .into_iter()
            .filter(|resource| !resource.is_null())
            .filter_map(|resource| {
                let rec = unsafe { ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec };
                if rec.is_null() {
                    return None;
                }
                let surface = unsafe { &*rec };
                if !surface.mapped || !surface.content_is_dmabuf {
                    return None;
                }
                let buffer = surface.dmabuf.as_ref()?;
                Some(SurfaceDmabuf {
                    window: None,
                    id: surface.resource as usize,
                    width: surface.width,
                    height: surface.height,
                    generation: surface.generation,
                    fd: buffer.fd,
                    drm_format: buffer.drm_format,
                    modifier: buffer.modifier,
                    offset: buffer.offset,
                    stride: buffer.stride,
                    acquire_fence: buffer.acquire_fence,
                    geometry: ass_core::SurfaceGeometry {
                        position: surface.position,
                        transform: surface.buffer_transform,
                        buffer_scale: surface.buffer_scale,
                        viewport_src: surface.viewport_src,
                        viewport_dst: surface.viewport_dst,
                        ..Default::default()
                    },
                })
            })
            .collect()
    }

    /// Mapped subsurfaces backed by shm whose `place_below` was the most
    /// recent stacking request — these render *under* their parent toplevel.
    /// Nested subsurface chains are walked recursively: each entry carries
    /// its compositor-space draw origin, and the whole subtree of a
    /// below-child renders here, in render order.
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
            // ADR-0029: the toplevel's in-flight transition shifts its whole
            // subsurface tree by the same delta.
            let delta = self
                .transition_render_rect(parent)
                .map(|r| {
                    (
                        r.origin.x - parent.position.x,
                        r.origin.y - parent.position.y,
                    )
                })
                .unwrap_or((0, 0));
            for &child_ptr in &parent.children {
                if child_ptr.is_null() {
                    continue;
                }
                let child = unsafe { &*child_ptr };
                if child.subsurface_above_parent != want_above {
                    continue;
                }
                Self::collect_subtree_shm(child, &mut out, delta, parent.window.id, 0);
            }
        }
        out
    }

    /// Emit one subsurface subtree in render order: the below-children
    /// subtrees, the surface itself, then the above-children subtrees. A
    /// subsurface's descendants render relative to it, so the whole subtree
    /// of a below-child still renders under the root toplevel. An unmapped
    /// node hides its entire subtree, per `wl_subsurface` mapping rules.
    /// `delta` shifts every emitted origin by the root's in-flight window
    /// transition (ADR-0029); (0, 0) outside transitions.
    fn collect_subtree_shm<'a>(
        s: &'a SurfaceRec,
        out: &mut Vec<SurfacePixels<'a>>,
        delta: (i32, i32),
        window: ass_core::window::WindowId,
        depth: u32,
    ) {
        // The depth cap only breaks reference cycles defensively; children
        // are orphaned on destroy, so live child pointers are always valid.
        if !s.mapped || depth >= 32 {
            return;
        }
        for &child_ptr in &s.children {
            if child_ptr.is_null() {
                continue;
            }
            let child = unsafe { &*child_ptr };
            if !child.subsurface_above_parent {
                Self::collect_subtree_shm(child, out, delta, window, depth + 1);
            }
        }
        if !s.content_is_dmabuf && !s.pixels.is_empty() {
            let origin = surface_draw_origin(s);
            out.push(SurfacePixels {
                window: Some(window),
                id: s.resource as usize,
                width: s.width,
                height: s.height,
                generation: s.generation,
                pixels: &s.pixels,
                geometry: ass_core::SurfaceGeometry {
                    position: ass_core::Point {
                        x: origin.x + delta.0,
                        y: origin.y + delta.1,
                    },
                    transform: s.buffer_transform,
                    buffer_scale: s.buffer_scale,
                    viewport_src: s.viewport_src,
                    viewport_dst: s.viewport_dst,
                    ..Default::default()
                },
                damage: &s.committed_damage,
            });
        }
        for &child_ptr in &s.children {
            if child_ptr.is_null() {
                continue;
            }
            let child = unsafe { &*child_ptr };
            if child.subsurface_above_parent {
                Self::collect_subtree_shm(child, out, delta, window, depth + 1);
            }
        }
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
            let delta = self
                .transition_render_rect(parent)
                .map(|r| {
                    (
                        r.origin.x - parent.position.x,
                        r.origin.y - parent.position.y,
                    )
                })
                .unwrap_or((0, 0));
            for &child_ptr in &parent.children {
                if child_ptr.is_null() {
                    continue;
                }
                let child = unsafe { &*child_ptr };
                if child.subsurface_above_parent != want_above {
                    continue;
                }
                Self::collect_subtree_dmabuf(child, &mut out, delta, parent.window.id, 0);
            }
        }
        out
    }

    /// The dma-buf half of [`Self::collect_subtree_shm`]: same render-order
    /// tree walk, emitting only dma-buf-backed surfaces.
    fn collect_subtree_dmabuf(
        s: &SurfaceRec,
        out: &mut Vec<SurfaceDmabuf>,
        delta: (i32, i32),
        window: ass_core::window::WindowId,
        depth: u32,
    ) {
        if !s.mapped || depth >= 32 {
            return;
        }
        for &child_ptr in &s.children {
            if child_ptr.is_null() {
                continue;
            }
            let child = unsafe { &*child_ptr };
            if !child.subsurface_above_parent {
                Self::collect_subtree_dmabuf(child, out, delta, window, depth + 1);
            }
        }
        if s.content_is_dmabuf {
            if let Some(db) = s.dmabuf.as_ref() {
                let origin = surface_draw_origin(s);
                out.push(SurfaceDmabuf {
                    window: Some(window),
                    id: s.resource as usize,
                    width: s.width,
                    height: s.height,
                    generation: s.generation,
                    fd: db.fd,
                    drm_format: db.drm_format,
                    modifier: db.modifier,
                    offset: db.offset,
                    stride: db.stride,
                    acquire_fence: db.acquire_fence,
                    geometry: ass_core::SurfaceGeometry {
                        position: ass_core::Point {
                            x: origin.x + delta.0,
                            y: origin.y + delta.1,
                        },
                        transform: s.buffer_transform,
                        buffer_scale: s.buffer_scale,
                        viewport_src: s.viewport_src,
                        viewport_dst: s.viewport_dst,
                        ..Default::default()
                    },
                });
            }
        }
        for &child_ptr in &s.children {
            if child_ptr.is_null() {
                continue;
            }
            let child = unsafe { &*child_ptr };
            if child.subsurface_above_parent {
                Self::collect_subtree_dmabuf(child, out, delta, window, depth + 1);
            }
        }
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

    pub fn retired_buffers_pending(&self) -> bool {
        !self.state.retired_buffer_releases.is_empty()
    }

    /// Release replaced dma-bufs after the renderer has submitted a frame
    /// that no longer references them. `completion_fence` is a borrowed Linux
    /// sync_file fd for that submission; `None` means completion was already
    /// waited on by the backend.
    pub fn release_retired_buffers(&mut self, completion_fence: Option<i32>) {
        let retired = std::mem::take(&mut self.state.retired_buffer_releases);
        let mut retry = Vec::new();
        for release in retired {
            let explicit_fd = if release.explicit_release.is_null() {
                None
            } else if let Some(fence) = completion_fence {
                let fd = unsafe { dup(fence) };
                if fd < 0 {
                    retry.push(release);
                    continue;
                }
                Some(fd)
            } else {
                None
            };
            unsafe {
                if !release.buffer.is_null() {
                    ffi::wl_resource_post_event(release.buffer, ffi::WL_BUFFER_RELEASE);
                }
                if !release.explicit_release.is_null() {
                    if let Some(fd) = explicit_fd {
                        ffi::wl_resource_post_event(
                            release.explicit_release,
                            ffi::ZWP_LINUX_BUFFER_RELEASE_V1_FENCED_RELEASE,
                            fd,
                        );
                    } else {
                        ffi::wl_resource_post_event(
                            release.explicit_release,
                            ffi::ZWP_LINUX_BUFFER_RELEASE_V1_IMMEDIATE_RELEASE,
                        );
                    }
                    ffi::wl_resource_destroy(release.explicit_release);
                }
            }
        }
        self.state.retired_buffer_releases.extend(retry);
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
                Tablet { event } => self.tablet_event(event),
            }
        }
        unsafe { ffi::wl_display_flush_clients(self.state.display) };
        actions
    }

    /// Mark real user input activity for ext-idle-notify. Synthetic IPC input
    /// intentionally does not call this method, so automation cannot keep a
    /// session awake indefinitely.
    pub fn note_user_activity(&mut self) {
        unsafe { extensions::idle_user_activity(self.state.as_mut()) };
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
        let o = self
            .state
            .keyboard
            .as_mut()?
            .update_key(evdev_code, pressed);
        self.state.depressed_mods = ass_core::input::Mods(o.depressed);
        // VT switch keys stay compositor-owned even while chrome holds the
        // keyboard (launcher/overview open); see `keyboard_key`.
        const XF86_SWITCH_VT_1: u32 = 0x1008_FE01;
        const XF86_SWITCH_VT_12: u32 = 0x1008_FE0C;
        if pressed && (XF86_SWITCH_VT_1..=XF86_SWITCH_VT_12).contains(&o.keysym) {
            self.state.pending_vt_switch = Some((o.keysym - XF86_SWITCH_VT_1 + 1) as i32);
            return None;
        }
        Some(ass_core::input::KeyChar {
            keysym: o.keysym,
            ch: o.utf8,
            mods: self.state.depressed_mods,
        })
    }

    /// Take a pending console VT switch request (Ctrl+Alt+Fn), if any.
    /// The main loop forwards it to the backend (libseat on DRM; a no-op
    /// nested).
    pub fn take_vt_switch(&mut self) -> Option<i32> {
        self.state.pending_vt_switch.take()
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

    /// Validate and translate target-local automation actions into the same
    /// backend-agnostic events used by physical input. The method is pure with
    /// respect to compositor state: the caller can reject the complete batch
    /// (for example because shell chrome covers a point) before forwarding any
    /// event.
    pub fn prepare_synthetic_input(
        &self,
        window_id: ass_core::window::WindowId,
        actions: &[ass_core::input::SyntheticInputAction],
    ) -> Option<Vec<ass_core::input::InputEvent>> {
        use ass_core::input::{ButtonState, InputEvent, SyntheticInputAction};

        if self.state.session_locked
            || actions.is_empty()
            || actions.len() > 64
            || self.state.interactive.is_some()
            || self.state.drag.is_some()
            || self.state.implicit_grab_active
            || self.state.depressed_mods != ass_core::input::Mods::NONE
        {
            return None;
        }
        let rec = self.find_surface_by_window_id(window_id);
        if rec.is_null()
            || unsafe {
                (*rec).xdg_toplevel.is_null()
                    || !(*rec).mapped
                    || (*rec).window.minimized
                    || !self.visible().contains(&window_id)
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
        let to_global = |local: ass_core::Point| -> Option<(f32, f32)> {
            if local.x < 0 || local.y < 0 || local.x >= size.w || local.y >= size.h {
                return None;
            }
            let x = origin.x.checked_add(local.x)?;
            let y = origin.y.checked_add(local.y)?;
            let hit = self.hit_test_focus(x as f32, y as f32);
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
                    events.push(InputEvent::PointerMotion { x, y });
                }
                SyntheticInputAction::Click { position, button } => {
                    let (x, y) = to_global(position)?;
                    events.push(InputEvent::PointerMotion { x, y });
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
                    events.push(InputEvent::PointerMotion { x, y });
                    events.push(InputEvent::PointerAxis { dx, dy });
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
    pub fn windows(&self) -> Vec<ass_core::window::Window> {
        let visible = self.visible();
        self.state
            .live_surfaces()
            .map(|p| unsafe { &*p })
            .filter(|s| !s.xdg_toplevel.is_null() && visible.contains(&s.window.id))
            .map(|s| {
                let mut w = s.window.clone();
                // Publish only in-flight transitions; settled ones are noise
                // to chrome and IPC consumers (ADR-0029).
                let target = ass_core::Rect {
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

    /// Minimize a toplevel from compositor chrome or IPC. This is the
    /// compositor-side counterpart of the client's
    /// `xdg_toplevel.set_minimized` request and shares the same focus cleanup.
    pub fn minimize_toplevel(&mut self, surface_id: ass_core::window::WindowId) {
        let rec = self.find_surface_by_window_id(surface_id);
        if rec.is_null() || unsafe { (*rec).xdg_toplevel.is_null() } {
            return;
        }
        unsafe { minimize_toplevel_record(rec) };
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
        window_id: ass_core::window::WindowId,
        rect: ass_core::Rect,
    ) -> bool {
        if rect.size.w <= 0 || rect.size.h <= 0 {
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
                && (*rec).window.layout_role == ass_core::layout::LayoutRole::Floating
                && !(*rec).window.state.maximized
                && !(*rec).window.state.fullscreen;
            if unchanged {
                return false;
            }
            let old = ass_core::Rect {
                origin: (*rec).position,
                size: (*rec).window.size,
            };
            (*rec).position = rect.origin;
            (*rec).window.position = rect.origin;
            (*rec).window.size = size;
            (*rec).window.layout_role = ass_core::layout::LayoutRole::Floating;
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
    pub fn interactive(&self) -> Option<ass_core::window::Interactive> {
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

    /// Cursor shape owned by compositor-side window manipulation. Active
    /// grabs take precedence; otherwise a floating window's invisible resize
    /// border advertises the edge/corner before the user presses it.
    pub fn compositor_cursor_shape(&self) -> Option<u32> {
        match self.state.interactive {
            Some(ass_core::window::Interactive::Move { .. }) => Some(17), // grabbing
            Some(ass_core::window::Interactive::Resize { edges, .. }) => {
                Some(resize_cursor_shape(edges))
            }
            None => self
                .resize_target_at(self.state.pointer_x, self.state.pointer_y, 8.0)
                .map(|(_, edges)| resize_cursor_shape(edges)),
        }
    }

    /// Drain text-input state committed by the focused inner client. The
    /// nested backend mirrors each state to the host compositor's IME.
    pub fn take_text_input_states(&mut self) -> Vec<ass_core::input::TextInputState> {
        std::mem::take(&mut self.state.pending_text_input_states)
    }

    /// Route one host IME event to the enabled text-input object belonging to
    /// the keyboard-focused inner client.
    pub fn text_input_event(&mut self, event: &ass_core::input::TextInputEvent) {
        unsafe { extensions::forward_text_input_event(self.state.as_mut(), event) };
    }

    /// Forward a host touchpad gesture to gesture objects belonging to the
    /// client that held pointer focus when the gesture began.
    pub fn pointer_gesture_event(&mut self, event: &ass_core::input::PointerGestureEvent) {
        use ass_core::input::PointerGestureEvent::*;
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
                // Transient dialogs (xdg_toplevel.set_parent) stay floating
                // (ADR-0024 floating exception); the sweep skips them.
                if on && (*rec).window.parent.is_some() {
                    log::debug!(
                        "[server] tiling sweep skips transient {:?}",
                        (*rec).window.id
                    );
                    continue;
                }
                (*rec).window.layout_role = role;
                (*rec).layout_target = None;
            }
        }
        log::info!(
            "[server] workspace tiling {}",
            if on { "on" } else { "off" }
        );
    }

    /// Set whether newly created workspaces start in tiled mode (ADR-0024),
    /// from the config's `[layout] default_tiled`. Existing workspaces keep
    /// their own flag. Called at startup and on config reload.
    pub fn set_tiling_default(&mut self, on: bool) {
        self.state.workspaces.set_default_tiled(on);
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

    /// Set the reduced-motion policy (ADR-0029, from `[ui] reduced_motion`).
    /// When enabled, in-flight transitions resolve immediately and no new
    /// ones are recorded.
    pub fn set_reduced_motion(&mut self, reduced: bool) {
        self.state.reduced_motion = reduced;
        if reduced {
            for p in self.state.live_surfaces() {
                unsafe { (*p).window.transition = None };
            }
        }
    }

    /// Compositor-relative millisecond timestamp for transition records.
    fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    /// Record a geometry transition for a non-interactive rect change
    /// (tiling, IPC geometry). The model moves to the target immediately;
    /// rendering interpolates from the current on-screen rect — the previous
    /// transition's mid-flight rect when changes come faster than the
    /// duration, else the previous model rect (ADR-0029).
    fn note_transition(&self, rec: *mut SurfaceRec, old: ass_core::Rect) {
        if self.state.reduced_motion || rec.is_null() {
            return;
        }
        let now = self.now_ms();
        unsafe {
            let target = ass_core::Rect {
                origin: (*rec).position,
                size: (*rec).window.size,
            };
            if old == target {
                (*rec).window.transition = None;
                return;
            }
            let from = (*rec)
                .window
                .transition
                .and_then(|t| t.rect_at(old, now))
                .unwrap_or(old);
            (*rec).window.transition = Some(ass_core::transition::WindowTransition::new(from, now));
        }
    }

    /// The rect a surface renders at this frame: `Some(interpolated)` while
    /// its transition is in flight, `None` at the model target (ADR-0029).
    fn transition_render_rect(&self, s: &SurfaceRec) -> Option<ass_core::Rect> {
        let target = ass_core::Rect {
            origin: s.position,
            size: s.window.size,
        };
        s.window
            .transition
            .and_then(|t| t.rect_at(target, self.now_ms()))
    }

    /// Whether any toplevel has a transition still in flight — the main loop
    /// keeps ticking frames at cadence instead of blocking on the host queue.
    pub fn transitions_pending(&self) -> bool {
        self.state.live_surfaces().any(|p| unsafe {
            !(*p).xdg_toplevel.is_null() && self.transition_render_rect(&*p).is_some()
        })
    }

    /// Reconcile connector identities and geometries reported by the backend.
    /// Existing connector workspaces survive reordering; removed outputs are
    /// relocated by `WorkspaceModel`, and a replug restores their origin.
    pub fn set_outputs(&mut self, mut outputs: Vec<ass_core::output::OutputInfo>) {
        outputs.retain(|output| !output.connector.is_empty());
        let mut seen = std::collections::HashSet::new();
        outputs.retain(|output| seen.insert(output.connector.clone()));
        if outputs.is_empty() {
            return;
        }
        // Configured per-connector policy (ADR-0028) wins over the
        // backend-reported geometry: scale and position apply here. A
        // configured transform is accepted but not yet applied — the
        // renderer's output-transform support is still pending.
        for output in &mut outputs {
            let Some(policy) = self.state.output_policies.get(&output.connector) else {
                continue;
            };
            if let Some(scale) = policy.scale {
                output.geometry.scale = ass_core::output::Scale(scale as f32);
            }
            if let Some(position) = policy.position {
                output.geometry.logical_origin = position;
            }
            if let Some(transform) = policy.transform {
                if transform != ass_core::Transform::Normal {
                    log::warn!(
                        "[server] output '{}': transform configured but not yet applied \
                         (renderer output-transform support pending)",
                        output.connector
                    );
                }
            }
        }
        // A `primary` policy moves its output to the front: index 0 is the
        // primary/focused output below. When several entries claim primary,
        // the one that appears first in the backend's output order wins.
        if let Some(primary) = outputs.iter().position(|output| {
            self.state
                .output_policies
                .get(&output.connector)
                .is_some_and(|policy| policy.primary)
        }) {
            let output = outputs.remove(primary);
            outputs.insert(0, output);
        }

        let desired = outputs
            .iter()
            .map(|output| output.connector.as_str())
            .collect::<std::collections::HashSet<_>>();
        for output in &outputs {
            if !self
                .state
                .workspaces
                .outputs()
                .iter()
                .any(|current| current.connector == output.connector)
            {
                self.state.workspaces.add_output(&output.connector);
            }
        }
        let removed = self
            .state
            .workspaces
            .outputs()
            .iter()
            .filter(|output| !desired.contains(output.connector.as_str()))
            .map(|output| output.id)
            .collect::<Vec<_>>();
        for output in removed {
            self.state.workspaces.remove_output(output);
        }

        let primary = outputs[0].clone();
        if let Some(output) = self
            .state
            .workspaces
            .outputs()
            .iter()
            .find(|output| output.connector == primary.connector)
        {
            self.state.output = output.id;
        }
        unsafe { reconcile_output_globals(self.state.as_mut(), &outputs) };
        self.state.output_infos = outputs;
        self.set_output_geometry(primary.geometry);
    }

    /// Set per-connector output policies from the config's `[[output]]`
    /// entries (ADR-0028), and re-apply them to the current output set.
    /// Unmatched connectors are ignored with a log line, so a monitor that is
    /// not plugged in yet still applies once it appears.
    pub fn set_output_policies(
        &mut self,
        policies: std::collections::HashMap<String, ass_core::output::OutputPolicy>,
    ) {
        for connector in policies.keys() {
            if !self
                .state
                .output_infos
                .iter()
                .any(|o| &o.connector == connector)
            {
                log::info!("[server] output policy for '{connector}' matches no current output");
            }
        }
        self.state.output_policies = policies;
        let outputs = self.state.output_infos.clone();
        if !outputs.is_empty() {
            self.set_outputs(outputs);
        }
    }

    /// Replace the focused output's geometry (ADR-0028). The backend calls
    /// this on resize; the tiling work-area is the geometry's logical rect.
    /// Re-sends the wl_output geometry/mode/scale/done sequence to every bound
    /// client so they update their scale and surface buffer scale.
    pub fn set_output_geometry(&mut self, geo: ass_core::output::OutputGeometry) {
        self.state.output_geometry = geo;
        if let Some(primary) = self.state.output_infos.first_mut() {
            primary.geometry = geo;
        }
        let infos = self.state.output_infos.clone();
        unsafe { reconcile_output_globals(self.state.as_mut(), &infos) };
        // Resend to every bound wl_output resource.
        let resources: Vec<*mut ffi::wl_resource> = self
            .state
            .output_resources
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .collect();
        for res in resources {
            unsafe { send_output_geometry(res) };
        }
        // Refresh xdg-output logical extents too.
        self.resend_xdg_outputs();
        // Re-send fractional-scale hints so HiDPI-aware clients resize buffers.
        unsafe {
            extensions::resend_fractional_scales(self.state.as_ref() as *const State as *mut State)
        };
        unsafe { extensions::session_lock_outputs_changed(self.state.as_mut()) };
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
        let resources: Vec<*mut ffi::wl_resource> = self
            .state
            .xdg_output_resources
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .collect();
        for res in resources {
            unsafe {
                let output = self
                    .state
                    .xdg_output_links
                    .get(&(res as usize))
                    .copied()
                    .unwrap_or(std::ptr::null_mut());
                extensions::send_xdg_output_geometry(
                    res,
                    output,
                    self.state.as_ref() as *const State as *mut State,
                );
                let version = ffi::wl_resource_get_version(res);
                if version >= 2 {
                    ffi::wl_resource_post_event(res, ffi::ZXDG_OUTPUT_V1_DONE);
                }
            }
        }
    }

    /// The live backend-reported outputs for IPC and chrome.
    pub fn output_infos(&self) -> Vec<ass_core::output::OutputInfo> {
        self.state.output_infos.clone()
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
                let old = ass_core::Rect {
                    origin: (*rec).position,
                    size: (*rec).window.size,
                };
                (*rec).position = rect.origin;
                (*rec).window.position = rect.origin;
                (*rec).window.size = rect.size;
                (*rec).window.layout_role = ass_core::layout::LayoutRole::Tiled;
                (*rec).layout_target = Some(*rect);
                self.note_transition(rec, old);
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
        let dx = x - self.state.raw_pointer_x;
        let dy = y - self.state.raw_pointer_y;
        self.state.raw_pointer_x = x;
        self.state.raw_pointer_y = y;
        let (x, y) = unsafe { extensions::constrain_pointer_motion(self.state.as_mut(), x, y) };
        self.state.pointer_x = x;
        self.state.pointer_y = y;
        unsafe { update_overlay_positions(self.state.as_mut()) };
        if self.state.drag.is_some() {
            let focus = self.hit_test_focus(x, y);
            let time = self.epoch.elapsed().as_millis() as u32;
            unsafe {
                update_drag_focus(self.state.as_mut(), focus, x, y, time);
            }
            return;
        }
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
        if self.state.session_locked {
            if state.is_pressed() && !self.state.pointer_focus.is_null() {
                self.change_keyboard_focus(self.state.pointer_focus);
            }
            if self.state.pointer_focus.is_null() {
                return;
            }
            let serial = unsafe { ffi::wl_display_next_serial(self.state.display) };
            self.state.last_button_serial = serial;
            self.state.implicit_grab_active = state.is_pressed();
            let focus_client = unsafe { ffi::wl_resource_get_client(self.state.pointer_focus) };
            for pointer in self.iter_focus_pointers(focus_client) {
                unsafe {
                    ffi::wl_resource_post_event(
                        pointer,
                        ffi::WL_POINTER_BUTTON,
                        serial,
                        0u32,
                        button,
                        u32::from(state.is_pressed()),
                    );
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
            if let Some(popup) = grabbed_popup {
                if self.state.pointer_focus != unsafe { (*popup).resource } {
                    unsafe {
                        ffi::wl_resource_post_event((*popup).xdg_popup, ffi::XDG_POPUP_POPUP_DONE);
                        (*popup).popup_grabbed = false;
                        (*popup).mapped = false;
                    }
                    return;
                }
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
        if state.is_pressed()
            && (button == BTN_LEFT || button == BTN_RIGHT)
            && self.state.depressed_mods.has(ass_core::input::Mods::SUPER)
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
                        (*rec).window.layout_role = ass_core::layout::LayoutRole::Floating;
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
        self.state.implicit_grab_active = false;
        if self.state.drag.is_some() {
            unsafe { cancel_drag(self.state.as_mut(), true) };
        }
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
        self.state.depressed_mods = ass_core::input::Mods(outcome.depressed);
        // Console VT switch (Ctrl+Alt+Fn → XF86Switch_VT_N): libinput owns
        // evdev on a direct backend, so the kernel's built-in handling never
        // runs — the compositor performs the session switch itself through
        // libseat. xkb only produces these keysyms with Ctrl+Alt held, and
        // they are consumed here (never posted to a client).
        const XF86_SWITCH_VT_1: u32 = 0x1008_FE01;
        const XF86_SWITCH_VT_12: u32 = 0x1008_FE0C;
        if state.is_pressed() && (XF86_SWITCH_VT_1..=XF86_SWITCH_VT_12).contains(&outcome.keysym) {
            self.state.pending_vt_switch = Some((outcome.keysym - XF86_SWITCH_VT_1 + 1) as i32);
            return None;
        }
        // A key that matches a global binding on press is consumed (not posted
        // to the focused client) and its action returned for the caller to
        // dispatch. Modifier-only keys never match, so modifiers still post.
        let shortcuts_inhibited =
            unsafe { extensions::keyboard_shortcuts_inhibited(self.state.as_mut()) };
        let matched = if state.is_pressed() && !shortcuts_inhibited && !self.state.session_locked {
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
    /// each surface's authoritative `position` (the window-rect origin,
    /// assigned at map time); later surfaces in the surfaces Vec are
    /// considered "above" earlier ones.
    fn hit_test_focus(&self, x: f32, y: f32) -> *mut ffi::wl_resource {
        let visible = self.visible();
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
                || !visible.contains(unsafe { &(*root).window.id })
            {
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
    fn hit_test_tree(s: &SurfaceRec, x: f32, y: f32, hit: &mut *mut ffi::wl_resource, depth: u32) {
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

    fn is_lock_resource(&self, resource: *mut ffi::wl_resource) -> bool {
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
    fn change_pointer_focus(&mut self, mut new_focus: *mut ffi::wl_resource) {
        if self.state.session_locked && !self.is_lock_resource(new_focus) {
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
                    ffi::wl_resource_post_event(p, ffi::WL_POINTER_LEAVE, serial, old);
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
                    ffi::wl_resource_post_event(p, ffi::WL_POINTER_ENTER, serial, new_focus, x, y);
                }
            }
        }
        unsafe {
            extensions::pointer_constraint_focus_changed(self.state.as_mut(), old, new_focus)
        };
    }

    fn post_motion_to_focus(&self) {
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
        let rec = unsafe { ffi::wl_resource_get_user_data(focus) as *mut SurfaceRec };
        let origin = if rec.is_null() {
            ass_core::Point::default()
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
    fn touch_motion(&mut self, time: u32, id: i32, x: f32, y: f32) {
        if self.state.pointer_focus.is_null() {
            return;
        }
        let focus = self.state.pointer_focus;
        let client = unsafe { ffi::wl_resource_get_client(focus) };
        let rec = unsafe { ffi::wl_resource_get_user_data(focus) as *mut SurfaceRec };
        let origin = if rec.is_null() {
            ass_core::Point::default()
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

    /// Route one tablet tool event (zwp_tablet-unstable-v2). The first
    /// proximity of a physical tool announces the compositor's synthetic
    /// tablet device and the tool to every bound seat; from then on, a pen
    /// over a surface whose client holds a tablet seat receives the full
    /// protocol stream (proximity/axes/tip/button, each burst closed by
    /// `frame`) on that client's tool resource, with surface-local
    /// coordinates computed exactly like `touch_down`. Over any other
    /// surface the pen falls back to emulating the pointer — motion plus
    /// BTN_LEFT for the tip — so tablet-unaware clients still work.
    fn tablet_event(&mut self, event: ass_core::input::TabletEvent) {
        use ass_core::input::TabletEvent::*;
        match event {
            Proximity {
                tool,
                info,
                in_proximity: true,
                x,
                y,
                ..
            } => self.tablet_proximity_in(tool, info, x, y),
            Proximity {
                tool,
                in_proximity: false,
                time,
                ..
            } => self.tablet_proximity_out(tool, time),
            Axes {
                tool,
                x,
                y,
                pressure,
                distance,
                tilt,
                rotation,
                slider,
                wheel,
                time,
            } => self.tablet_axes(
                tool, x, y, pressure, distance, tilt, rotation, slider, wheel, time,
            ),
            Tip { tool, state, time } => self.tablet_tip(tool, state, time),
            Button {
                tool,
                button,
                state,
                time,
            } => self.tablet_button(tool, button, state, time),
        }
    }

    /// The `zwp_tablet_tool_v2` resource owned by `client` that proxies
    /// physical tool `tool`, or null.
    fn tablet_tool_for(&self, client: *mut ffi::wl_client, tool: u64) -> *mut ffi::wl_resource {
        self.state
            .tablet_tools
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .filter(|p| unsafe { ffi::wl_resource_get_client(*p) == client })
            .find(|p| unsafe {
                let rec = ffi::wl_resource_get_user_data(*p) as *mut extensions::TabletToolRec;
                !rec.is_null() && (*rec).tool == tool
            })
            .unwrap_or(std::ptr::null_mut())
    }

    /// Any live `zwp_tablet_v2` resource owned by `client`. One synthetic
    /// tablet exists, so the tablet/tool pairing is implicit per client.
    fn tablet_device_for(&self, client: *mut ffi::wl_client) -> *mut ffi::wl_resource {
        self.state
            .tablet_devices
            .iter()
            .copied()
            .filter(|p| !p.is_null())
            .find(|p| unsafe { ffi::wl_resource_get_client(*p) == client })
            .unwrap_or(std::ptr::null_mut())
    }

    /// A tool entered proximity: announce the device/tool to late-bound
    /// seats, then either open protocol proximity on the hit surface or
    /// emulate pointer motion when its client has no tablet seat.
    fn tablet_proximity_in(
        &mut self,
        tool: u64,
        info: ass_core::input::TabletToolInfo,
        x: f32,
        y: f32,
    ) {
        // Clone the seat list so the announce calls can re-borrow `state`.
        let seats: Vec<*mut ffi::wl_resource> = self
            .state
            .tablet_seats
            .iter()
            .copied()
            .filter(|s| !s.is_null())
            .collect();
        // A tool must follow a tablet, so a never-announced device goes out
        // to every seat before the tool itself.
        if !self.state.tablet_device_seen {
            self.state.tablet_device_seen = true;
            for seat in &seats {
                unsafe { extensions::announce_tablet(self.state.as_mut(), *seat) };
            }
        }
        if !self.state.known_tools.iter().any(|(id, _)| *id == tool) {
            self.state.known_tools.push((tool, info));
            for seat in &seats {
                unsafe { extensions::announce_tool(self.state.as_mut(), *seat, tool, &info) };
            }
        }
        // Keep chrome/cursor tracking the pen.
        self.state.pointer_x = x;
        self.state.pointer_y = y;
        let focus = self.hit_test_focus(x, y);
        let focus_client = if focus.is_null() {
            std::ptr::null_mut()
        } else {
            unsafe { ffi::wl_resource_get_client(focus) }
        };
        let has_seat = !focus_client.is_null()
            && seats
                .iter()
                .any(|s| unsafe { ffi::wl_resource_get_client(*s) } == focus_client);
        if !has_seat {
            self.state.tablet_focus = std::ptr::null_mut();
            self.pointer_motion(x, y);
            return;
        }
        self.state.tablet_focus = focus;
        let serial = unsafe { ffi::wl_display_next_serial(self.state.display) };
        let tablet = self.tablet_device_for(focus_client);
        let tool_res = self.tablet_tool_for(focus_client, tool);
        if tablet.is_null() || tool_res.is_null() {
            return;
        }
        unsafe {
            ffi::wl_resource_post_event(
                tool_res,
                ffi::ZWP_TABLET_TOOL_V2_PROXIMITY_IN,
                serial,
                tablet,
                focus,
            );
        }
    }

    /// The tool left proximity: close the burst on the focus client's tool
    /// resource and drop tablet focus.
    fn tablet_proximity_out(&mut self, tool: u64, time: u32) {
        if self.state.tablet_focus.is_null() {
            return;
        }
        let focus_client = unsafe { ffi::wl_resource_get_client(self.state.tablet_focus) };
        let tool_res = self.tablet_tool_for(focus_client, tool);
        self.state.tablet_focus = std::ptr::null_mut();
        if tool_res.is_null() {
            return;
        }
        unsafe {
            ffi::wl_resource_post_event(tool_res, ffi::ZWP_TABLET_TOOL_V2_PROXIMITY_OUT);
            ffi::wl_resource_post_event(tool_res, ffi::ZWP_TABLET_TOOL_V2_FRAME, time);
        }
    }

    /// Axis updates for the in-proximity tool: motion plus whichever of
    /// pressure/distance/tilt/rotation/slider/wheel the backend reported,
    /// closed by `frame`. Pressure and distance are normalized f32 0.0..1.0;
    /// the protocol wants uint 0..65535.
    #[allow(clippy::too_many_arguments)]
    fn tablet_axes(
        &mut self,
        tool: u64,
        x: f32,
        y: f32,
        pressure: Option<f32>,
        distance: Option<f32>,
        tilt: Option<(f32, f32)>,
        rotation: Option<f32>,
        slider: Option<f32>,
        wheel: Option<(f32, i32)>,
        time: u32,
    ) {
        if self.state.tablet_focus.is_null() {
            // Emulated path: the pen drives the pointer.
            self.pointer_motion(x, y);
            return;
        }
        self.state.pointer_x = x;
        self.state.pointer_y = y;
        let focus = self.state.tablet_focus;
        let focus_client = unsafe { ffi::wl_resource_get_client(focus) };
        let tool_res = self.tablet_tool_for(focus_client, tool);
        if tool_res.is_null() {
            return;
        }
        let rec = unsafe { ffi::wl_resource_get_user_data(focus) as *mut SurfaceRec };
        let origin = if rec.is_null() {
            ass_core::Point::default()
        } else {
            unsafe { surface_draw_origin(&*rec) }
        };
        let fx = ffi::wl_fixed_from_f32(x - origin.x as f32);
        let fy = ffi::wl_fixed_from_f32(y - origin.y as f32);
        unsafe {
            ffi::wl_resource_post_event(tool_res, ffi::ZWP_TABLET_TOOL_V2_MOTION, fx, fy);
            if let Some(p) = pressure {
                ffi::wl_resource_post_event(
                    tool_res,
                    ffi::ZWP_TABLET_TOOL_V2_PRESSURE,
                    (p.clamp(0.0, 1.0) * 65535.0) as u32,
                );
            }
            if let Some(d) = distance {
                ffi::wl_resource_post_event(
                    tool_res,
                    ffi::ZWP_TABLET_TOOL_V2_DISTANCE,
                    (d.clamp(0.0, 1.0) * 65535.0) as u32,
                );
            }
            if let Some((tx, ty)) = tilt {
                ffi::wl_resource_post_event(
                    tool_res,
                    ffi::ZWP_TABLET_TOOL_V2_TILT,
                    ffi::wl_fixed_from_f32(tx),
                    ffi::wl_fixed_from_f32(ty),
                );
            }
            if let Some(r) = rotation {
                ffi::wl_resource_post_event(
                    tool_res,
                    ffi::ZWP_TABLET_TOOL_V2_ROTATION,
                    ffi::wl_fixed_from_f32(r),
                );
            }
            if let Some(s) = slider {
                ffi::wl_resource_post_event(
                    tool_res,
                    ffi::ZWP_TABLET_TOOL_V2_SLIDER,
                    ffi::wl_fixed_from_f32(s),
                );
            }
            if let Some((degrees, clicks)) = wheel {
                ffi::wl_resource_post_event(
                    tool_res,
                    ffi::ZWP_TABLET_TOOL_V2_WHEEL,
                    ffi::wl_fixed_from_f32(degrees),
                    clicks,
                );
            }
            ffi::wl_resource_post_event(tool_res, ffi::ZWP_TABLET_TOOL_V2_FRAME, time);
        }
    }

    /// Tip down/up: protocol `down`/`up` on the focus client's tool resource
    /// (with click-to-focus parity on down), or a BTN_LEFT click on the
    /// emulated pointer path.
    fn tablet_tip(&mut self, tool: u64, state: ass_core::input::ButtonState, time: u32) {
        const BTN_LEFT: u32 = 0x110;
        if self.state.tablet_focus.is_null() {
            self.pointer_button(BTN_LEFT, state);
            return;
        }
        if state.is_pressed() {
            self.change_keyboard_focus(self.state.tablet_focus);
        }
        let focus_client = unsafe { ffi::wl_resource_get_client(self.state.tablet_focus) };
        let tool_res = self.tablet_tool_for(focus_client, tool);
        if tool_res.is_null() {
            return;
        }
        unsafe {
            if state.is_pressed() {
                let serial = ffi::wl_display_next_serial(self.state.display);
                ffi::wl_resource_post_event(tool_res, ffi::ZWP_TABLET_TOOL_V2_DOWN, serial);
            } else {
                ffi::wl_resource_post_event(tool_res, ffi::ZWP_TABLET_TOOL_V2_UP);
            }
            ffi::wl_resource_post_event(tool_res, ffi::ZWP_TABLET_TOOL_V2_FRAME, time);
        }
    }

    /// A stylus button: protocol `button` + `frame` on the focus client's
    /// tool resource. No-op without tablet focus (the emulated pointer path
    /// has no stylus buttons).
    fn tablet_button(
        &mut self,
        tool: u64,
        button: u32,
        state: ass_core::input::ButtonState,
        time: u32,
    ) {
        if self.state.tablet_focus.is_null() {
            return;
        }
        let focus_client = unsafe { ffi::wl_resource_get_client(self.state.tablet_focus) };
        let tool_res = self.tablet_tool_for(focus_client, tool);
        if tool_res.is_null() {
            return;
        }
        let serial = unsafe { ffi::wl_display_next_serial(self.state.display) };
        unsafe {
            ffi::wl_resource_post_event(
                tool_res,
                ffi::ZWP_TABLET_TOOL_V2_BUTTON,
                serial,
                button,
                u32::from(state.is_pressed()),
            );
            ffi::wl_resource_post_event(tool_res, ffi::ZWP_TABLET_TOOL_V2_FRAME, time);
        }
    }

    /// Transition keyboard focus: post leave to the old client's keyboard
    /// resources and enter to the new client's. The "keys" array passed to
    /// `enter` is empty — M1 does not track currently-held keys for resend on
    /// refocus; that's a polish item once the keyboard pipeline stabilizes.
    /// Also flips the `activated` toplevel state bit on the old and new
    /// surfaces so clients update their title-bar chrome to match focus.
    fn change_keyboard_focus(&mut self, mut new_focus: *mut ffi::wl_resource) {
        if self.state.session_locked && !self.is_lock_resource(new_focus) {
            new_focus = std::ptr::null_mut();
        }
        if !new_focus.is_null() {
            // The clicked surface may be a subsurface; raise its root
            // toplevel so the window comes forward as a unit.
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
        let empty = ffi::wl_array::empty();

        // Notify text-input clients of the focus transition (IME enter/leave).
        unsafe {
            extensions::text_input_focus_changed(
                self.state.as_ref() as *const State as *mut State,
                old,
                new_focus,
            );
            extensions::primary_selection_focus_changed(
                self.state.as_ref() as *const State as *mut State,
                old,
                new_focus,
            );
            data_device_focus_changed(
                self.state.as_ref() as *const State as *mut State,
                old,
                new_focus,
            );
            extensions::keyboard_shortcuts_focus_changed(
                self.state.as_ref() as *const State as *mut State,
                new_focus,
            );
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
            }
            unsafe { reconfigure_with_state(s as *mut SurfaceRec) };
            return;
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

    #[test]
    fn legacy_output_scale_rounds_fractional_values_up() {
        assert_eq!(integer_output_scale(1.0), 1);
        assert_eq!(integer_output_scale(1.25), 2);
        assert_eq!(integer_output_scale(1.5), 2);
        assert_eq!(integer_output_scale(2.0), 2);
    }

    #[test]
    fn compositor_resize_edges_map_to_protocol_cursor_shapes() {
        use ass_core::window::ResizeEdges;

        assert_eq!(resize_cursor_shape(ResizeEdges::LEFT), 25);
        assert_eq!(resize_cursor_shape(ResizeEdges::RIGHT), 18);
        assert_eq!(
            resize_cursor_shape(ResizeEdges(ResizeEdges::TOP.0 | ResizeEdges::LEFT.0)),
            21
        );
        assert_eq!(
            resize_cursor_shape(ResizeEdges(ResizeEdges::BOTTOM.0 | ResizeEdges::RIGHT.0)),
            23
        );
    }

    #[test]
    fn explicit_geometry_size_respects_client_hints() {
        let hints = ass_core::window::SizeHints {
            min_w: 320,
            min_h: 200,
            max_w: 1920,
            max_h: 1080,
        };
        assert_eq!(
            clamp_size_to_hints(ass_core::Size { w: 100, h: 2_000 }, hints),
            ass_core::Size { w: 320, h: 1080 }
        );
        assert_eq!(
            clamp_size_to_hints(
                ass_core::Size { w: 800, h: 600 },
                ass_core::window::SizeHints::default(),
            ),
            ass_core::Size { w: 800, h: 600 }
        );
    }

    #[test]
    fn logical_surface_size_applies_transform_scale_and_viewport_in_order() {
        let mut surface = SurfaceRec::new(std::ptr::null_mut());
        surface.width = 400;
        surface.height = 200;
        surface.buffer_scale = 2;
        assert_eq!(
            surface_logical_size(&surface),
            ass_core::Size { w: 200, h: 100 }
        );

        surface.buffer_transform = ass_core::Transform::Rotate90;
        assert_eq!(
            surface_logical_size(&surface),
            ass_core::Size { w: 100, h: 200 }
        );

        // Viewport source coordinates are after transform and buffer scale,
        // so they are already surface-local and must not be divided again.
        surface.viewport_src = Some(ass_core::Rect::new(5, 7, 80, 60));
        assert_eq!(
            surface_logical_size(&surface),
            ass_core::Size { w: 80, h: 60 }
        );

        surface.viewport_dst = Some(ass_core::Size { w: 123, h: 45 });
        assert_eq!(
            surface_logical_size(&surface),
            ass_core::Size { w: 123, h: 45 }
        );
    }

    #[test]
    fn draw_origin_subtracts_window_geometry_insets() {
        let mut surface = SurfaceRec::new(std::ptr::null_mut());
        surface.position = ass_core::Point { x: 100, y: 60 };

        // No declared geometry: the buffer draws at the window-rect origin.
        assert_eq!(
            surface_draw_origin(&surface),
            ass_core::Point { x: 100, y: 60 }
        );

        // CSD insets: the buffer extends up-left of the window rect.
        surface.window_geometry = Some(ass_core::Rect::new(20, 10, 400, 300));
        assert_eq!(
            surface_draw_origin(&surface),
            ass_core::Point { x: 80, y: 50 }
        );
    }

    #[test]
    fn draw_origin_walks_nested_subsurface_chains() {
        let mut root = SurfaceRec::new(std::ptr::null_mut());
        root.position = ass_core::Point { x: 100, y: 60 };
        // A CSD root: the chain anchors at the buffer draw origin.
        root.window_geometry = Some(ass_core::Rect::new(20, 10, 400, 300));

        let mut child = SurfaceRec::new(std::ptr::null_mut());
        child.parent = &mut root;
        child.subsurface_offset = ass_core::Point { x: 10, y: 5 };
        let mut grandchild = SurfaceRec::new(std::ptr::null_mut());
        grandchild.parent = &mut child;
        grandchild.subsurface_offset = ass_core::Point { x: 3, y: 2 };

        // 100-20+10+3, 60-10+5+2: offsets accumulate in each parent's
        // buffer space down to the root's draw origin.
        assert_eq!(
            surface_draw_origin(&grandchild),
            ass_core::Point { x: 93, y: 57 }
        );
        assert_eq!(
            surface_draw_origin(&child),
            ass_core::Point { x: 90, y: 55 }
        );

        // Detaching (wl_subsurface.destroy / parent destroyed) stops the walk.
        grandchild.parent = std::ptr::null_mut();
        assert_eq!(surface_draw_origin(&grandchild), ass_core::Point::default());
    }

    #[test]
    fn accepts_point_uses_buffer_space_for_subsurfaces() {
        let mut root = SurfaceRec::new(std::ptr::null_mut());
        root.position = ass_core::Point { x: 100, y: 60 };
        let mut child = SurfaceRec::new(std::ptr::null_mut());
        child.parent = &mut root;
        child.subsurface_offset = ass_core::Point { x: 10, y: 5 };
        child.width = 40;
        child.height = 30;
        child.buffer_scale = 1;

        // Inside the child's buffer rect (anchored at 110,65).
        assert!(surface_accepts_point(&child, 120.0, 70.0));
        // Outside it, but inside the parent's rect — the parent test would
        // catch that point instead.
        assert!(!surface_accepts_point(&child, 105.0, 62.0));

        // An input region further restricts the accepted area, in
        // buffer-local coordinates.
        child.input_region = Some(vec![ass_core::Rect::new(0, 0, 20, 30)]);
        assert!(surface_accepts_point(&child, 115.0, 70.0));
        assert!(!surface_accepts_point(&child, 135.0, 70.0));
    }

    #[test]
    fn region_subtraction_preserves_the_uncut_area() {
        let pieces = subtract_rect(
            ass_core::Rect::new(0, 0, 100, 100),
            ass_core::Rect::new(20, 20, 60, 60),
        );
        assert_eq!(pieces.len(), 4);
        let area: i32 = pieces.iter().map(|rect| rect.size.w * rect.size.h).sum();
        assert_eq!(area, 10_000 - 3_600);
        assert!(pieces
            .iter()
            .all(|rect| !rect.contains(ass_core::Point { x: 50, y: 50 })));
    }

    #[test]
    fn dnd_action_negotiation_honors_preference_then_fallback_order() {
        let all = ffi::WL_DATA_ACTION_COPY | ffi::WL_DATA_ACTION_MOVE | ffi::WL_DATA_ACTION_ASK;
        assert_eq!(
            choose_dnd_action(all, all, ffi::WL_DATA_ACTION_MOVE),
            ffi::WL_DATA_ACTION_MOVE
        );
        assert_eq!(
            choose_dnd_action(all, ffi::WL_DATA_ACTION_COPY | ffi::WL_DATA_ACTION_ASK, 0),
            ffi::WL_DATA_ACTION_COPY
        );
        assert_eq!(
            choose_dnd_action(ffi::WL_DATA_ACTION_MOVE, ffi::WL_DATA_ACTION_COPY, 0),
            ffi::WL_DATA_ACTION_NONE
        );
    }

    #[test]
    fn layout_role_resolution_prefers_rule_then_transient_then_workspace() {
        use ass_core::layout::LayoutRole;
        // An explicit window rule always wins (even over a transient).
        assert_eq!(
            resolve_layout_role(true, true, Some(LayoutRole::Tiled)),
            LayoutRole::Tiled
        );
        assert_eq!(
            resolve_layout_role(true, false, Some(LayoutRole::Floating)),
            LayoutRole::Floating
        );
        // No rule: a transient (dialog) floats even on a tiled workspace.
        assert_eq!(resolve_layout_role(true, true, None), LayoutRole::Floating);
        assert_eq!(resolve_layout_role(false, true, None), LayoutRole::Floating);
        // No rule, not transient: the workspace's tiled flag decides.
        assert_eq!(resolve_layout_role(true, false, None), LayoutRole::Tiled);
        assert_eq!(
            resolve_layout_role(false, false, None),
            LayoutRole::Floating
        );
    }

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

    #[test]
    fn backend_outputs_reconcile_workspace_connectors_and_geometry() {
        if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
            eprintln!("skipping: XDG_RUNTIME_DIR not set");
            return;
        }
        let mut server = Server::new().expect("Server::new");
        let geometry = |x, width| ass_core::output::OutputGeometry {
            mode: ass_core::output::OutputMode {
                width,
                height: 1080,
                refresh_mhz: 60_000,
            },
            scale: ass_core::output::Scale::IDENTITY,
            transform: ass_core::Transform::Normal,
            logical_origin: ass_core::Point { x, y: 0 },
        };
        server.set_outputs(vec![
            ass_core::output::OutputInfo {
                connector: "DP-1".into(),
                geometry: geometry(0, 1920),
                available_modes: Vec::new(),
            },
            ass_core::output::OutputInfo {
                connector: "HDMI-A-1".into(),
                geometry: geometry(1920, 2560),
                available_modes: Vec::new(),
            },
        ]);

        assert_eq!(
            server
                .output_infos()
                .iter()
                .map(|output| output.connector.as_str())
                .collect::<Vec<_>>(),
            vec!["DP-1", "HDMI-A-1"]
        );
        assert_eq!(server.output_logical_rect().size.w, 1920);
        assert_eq!(
            server
                .workspace_snapshot()
                .outputs
                .iter()
                .map(|output| output.connector.as_str())
                .collect::<std::collections::HashSet<_>>(),
            std::collections::HashSet::from(["DP-1", "HDMI-A-1"])
        );
    }

    /// `[[output]]` policy (ADR-0028) overrides the backend-reported
    /// geometry: scale and position apply per connector, and a `primary`
    /// entry moves its output to the front of the list (index 0 is the
    /// focused output whose geometry `output_logical_rect` reports).
    #[test]
    fn output_policies_apply_scale_position_and_primary() {
        if std::env::var_os("XDG_RUNTIME_DIR").is_none() {
            eprintln!("skipping: XDG_RUNTIME_DIR not set");
            return;
        }
        let mut server = Server::new().expect("Server::new");
        let geometry = |x, width| ass_core::output::OutputGeometry {
            mode: ass_core::output::OutputMode {
                width,
                height: 1080,
                refresh_mhz: 60_000,
            },
            scale: ass_core::output::Scale::IDENTITY,
            transform: ass_core::Transform::Normal,
            logical_origin: ass_core::Point { x, y: 0 },
        };
        server.set_outputs(vec![
            ass_core::output::OutputInfo {
                connector: "DP-1".into(),
                geometry: geometry(0, 1920),
                available_modes: Vec::new(),
            },
            ass_core::output::OutputInfo {
                connector: "HDMI-A-1".into(),
                geometry: geometry(1920, 2560),
                available_modes: Vec::new(),
            },
        ]);

        server.set_output_policies(std::collections::HashMap::from([(
            "HDMI-A-1".to_owned(),
            ass_core::output::OutputPolicy {
                scale: Some(2.0),
                position: Some(ass_core::Point { x: 1920, y: 0 }),
                primary: true,
                ..Default::default()
            },
        )]));

        let infos = server.output_infos();
        assert_eq!(
            infos
                .iter()
                .map(|output| output.connector.as_str())
                .collect::<Vec<_>>(),
            vec!["HDMI-A-1", "DP-1"],
            "the primary output leads the list"
        );
        assert_eq!(infos[0].geometry.scale.as_f32(), 2.0);
        assert_eq!(
            infos[0].geometry.logical_origin,
            ass_core::Point { x: 1920, y: 0 }
        );
        assert_eq!(
            server.output_logical_rect().origin,
            ass_core::Point { x: 1920, y: 0 },
            "the focused output geometry follows the primary policy"
        );
        // The other output keeps its backend-reported geometry.
        assert_eq!(infos[1].geometry.scale.as_f32(), 1.0);
        assert_eq!(
            infos[1].geometry.logical_origin,
            ass_core::Point { x: 0, y: 0 }
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
        Box::into_raw(Box::new(RegionRec::default())) as *mut c_void,
        Some(region_resource_destroy),
    );
}

// ----- wl_surface ---------------------------------------------------------

static SURFACE_IMPL: ffi::wl_surface_interface_impl = ffi::wl_surface_interface_impl {
    destroy: surface_destroy,
    attach: surface_attach,
    damage: surface_damage,
    frame: surface_frame,
    set_opaque_region: surface_noop_region,
    set_input_region: surface_set_input_region,
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
    x: i32,
    y: i32,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    (*rec).pending_buffer = buffer;
    (*rec).pending_buffer_set = true;
    (*rec).pending_attach_offset = ass_core::Point { x, y };
}

unsafe fn retire_surface_buffer(rec: *mut SurfaceRec) {
    if rec.is_null() || (*rec).state.is_null() {
        return;
    }
    let buffer = std::mem::replace(&mut (*rec).current_buffer, std::ptr::null_mut());
    let explicit_release =
        std::mem::replace(&mut (*rec).current_explicit_release, std::ptr::null_mut());
    if !buffer.is_null() || !explicit_release.is_null() {
        (*(*rec).state)
            .retired_buffer_releases
            .push(RetiredBufferRelease {
                buffer,
                explicit_release,
            });
    }
}

unsafe extern "C" fn surface_commit(_client: *mut ffi::wl_client, resource: *mut ffi::wl_resource) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !(*rec).parent.is_null() && (*rec).subsurface_sync && !(*rec).subsurface_applying_cached {
        (*rec).subsurface_cached_commit = true;
        return;
    }
    (*rec).subsurface_cached_commit = false;
    if let Some(region) = (*rec).pending_input_region.take() {
        (*rec).input_region = region;
    }
    if let Some(geometry) = (*rec).pending_window_geometry.take() {
        (*rec).window_geometry = Some(geometry);
    }
    if let Some(source) = (*rec).pending_viewport_src.take() {
        (*rec).viewport_src = source;
    }
    if let Some(destination) = (*rec).pending_viewport_dst.take() {
        (*rec).viewport_dst = destination;
    }
    (*rec).buffer_transform = (*rec).pending_transform;
    (*rec).buffer_scale = (*rec).pending_scale;

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
        send_xdg_surface_configure(rec);
        (*rec).xdg_configured = true;
    }

    let was_mapped = (*rec).mapped;
    let buffer = (*rec).pending_buffer;
    let buffer_set = std::mem::take(&mut (*rec).pending_buffer_set);
    if !extensions::explicit_sync_surface_committed(rec, buffer_set, buffer) {
        return;
    }
    if buffer_set
        && !buffer.is_null()
        && !(*rec).xdg_surface.is_null()
        && !(*rec).xdg_configure_acked
    {
        ffi::wl_resource_post_error(
            (*rec).xdg_surface,
            3,
            c"buffer committed before the initial xdg_surface.configure was acknowledged".as_ptr(),
        );
        return;
    }
    if buffer_set {
        (*rec).attach_offset = (*rec).pending_attach_offset;
    }
    // The pending transform and scale are surfaced to the renderer via
    // `SurfaceGeometry` (see toplevel_*_frames below); the renderer applies
    // them at composite time.
    // Rotate the pending damage into committed_damage for the renderer to
    // read this frame; clear pending so the next commit starts fresh.
    // Bounding boxes are clamped to surface bounds when surfaced.
    (*rec).committed_damage = std::mem::take(&mut (*rec).pending_damage);
    if buffer_set && buffer.is_null() {
        retire_surface_buffer(rec);
        (*rec).dmabuf = None;
        (*rec).mapped = false;
        (*rec).pixels.clear();
        (*rec).content_is_dmabuf = false;
        (*rec).generation = (*rec).generation.wrapping_add(1);
    } else if !buffer.is_null() {
        let is_dmabuf = ffi::wl_resource_instance_of(
            buffer,
            &ffi::wl_buffer_interface,
            &WL_BUFFER_IMPL as *const _ as *const c_void,
        ) != 0;

        if is_dmabuf {
            // Duplicate the dma-buf fd into compositor ownership before
            // releasing the client wl_buffer. Clients are then free to destroy
            // that protocol object without invalidating the surface contents.
            let db = ffi::wl_resource_get_user_data(buffer) as *const DmabufBuffer;
            if !db.is_null() && (*db).have_plane {
                if let Some(owned) = (*db).duplicate() {
                    retire_surface_buffer(rec);
                    let mut owned = owned;
                    if (*rec).committed_acquire_fence >= 0 {
                        if owned.acquire_fence >= 0 {
                            libc_close(owned.acquire_fence);
                        }
                        owned.acquire_fence =
                            std::mem::replace(&mut (*rec).committed_acquire_fence, -1);
                    }
                    (*rec).width = owned.width;
                    (*rec).height = owned.height;
                    (*rec).dmabuf = Some(owned);
                    // Invalidate the shm snapshot: if a later commit returns
                    // to shm, its incremental-copy size check must fail so
                    // the new frame is copied in full rather than blended
                    // into these stale pixels.
                    (*rec).pixels.clear();
                    (*rec).content_is_dmabuf = true;
                    (*rec).generation = (*rec).generation.wrapping_add(1);
                    (*rec).mapped = true;
                    (*rec).current_buffer = buffer;
                    (*rec).current_explicit_release = std::mem::replace(
                        &mut (*rec).committed_explicit_release,
                        std::ptr::null_mut(),
                    );
                }
            }
            (*rec).pending_buffer = std::ptr::null_mut();
        } else {
            // shm: copy the contents out into our own tightly packed BGRA store
            // and release the buffer immediately so the client can reuse it.
            //
            // The copy is damage-driven: for a same-size frame with usable
            // damage the protocol guarantees the new buffer differs from the
            // previous frame only inside the damaged region, so copying the
            // damaged rows onto the retained snapshot yields the new frame
            // without a full-buffer memcpy (and without the per-commit
            // allocation — the snapshot Vec is reused while its size holds).
            // Empty damage carries no information and forces a full copy, as
            // do a transform or buffer scale (damage is surface-local and
            // would not map 1:1 onto buffer pixels). The guard mirrors the
            // renderer's incremental-upload guard exactly; the two paths
            // must always agree or the texture would tear.
            let shm = ffi::wl_shm_buffer_get(buffer);
            if !shm.is_null() {
                let w = ffi::wl_shm_buffer_get_width(shm);
                let h = ffi::wl_shm_buffer_get_height(shm);
                let stride = ffi::wl_shm_buffer_get_stride(shm) as usize;
                let format = ffi::wl_shm_buffer_get_format(shm);
                let src = ffi::wl_shm_buffer_get_data(shm) as *const u8;
                if !src.is_null() && w > 0 && h > 0 {
                    let tight = (w as usize) * 4;
                    let needed = tight * h as usize;
                    let incremental = (*rec).width == w
                        && (*rec).height == h
                        && (*rec).pixels.len() == needed
                        && (*rec).buffer_transform == ass_core::Transform::Normal
                        && (*rec).buffer_scale <= 1
                        && !(*rec).committed_damage.is_empty();
                    if (*rec).pixels.len() != needed {
                        (*rec).pixels = vec![0u8; needed];
                    }
                    // Read the damage list locally: the copy mutates
                    // `pixels` while the rects are consulted.
                    let damage = if incremental {
                        std::mem::take(&mut (*rec).committed_damage)
                    } else {
                        Vec::new()
                    };
                    ffi::wl_shm_buffer_begin_access(shm);
                    // One explicit mutable borrow for the whole copy: raw
                    // pointer field indexing would implicitly autoref each
                    // access.
                    let pixels = &mut (*rec).pixels;
                    if incremental {
                        for d in &damage {
                            let x = d.origin.x.max(0).min(w - 1) as usize;
                            let y = d.origin.y.max(0).min(h - 1) as usize;
                            let cw = (d.size.w.max(0)).min(w - x as i32) as usize;
                            let ch = (d.size.h.max(0)).min(h - y as i32) as usize;
                            if cw == 0 || ch == 0 {
                                continue;
                            }
                            for row in 0..ch {
                                std::ptr::copy_nonoverlapping(
                                    src.add((y + row) * stride + x * 4),
                                    pixels.as_mut_ptr().add((y + row) * tight + x * 4),
                                    cw * 4,
                                );
                            }
                            // XRGB8888 has undefined alpha; force opaque on
                            // the refreshed rows.
                            if format == 1 {
                                for row in 0..ch {
                                    let base = (y + row) * tight + x * 4;
                                    for px in 0..cw {
                                        pixels[base + px * 4 + 3] = 0xff;
                                    }
                                }
                            }
                        }
                        (*rec).committed_damage = damage;
                    } else {
                        for row in 0..h as usize {
                            std::ptr::copy_nonoverlapping(
                                src.add(row * stride),
                                pixels.as_mut_ptr().add(row * tight),
                                tight,
                            );
                        }
                        // XRGB8888 has undefined alpha; force opaque.
                        if format == 1 {
                            let mut i = 3;
                            while i < needed {
                                pixels[i] = 0xff;
                                i += 4;
                            }
                        }
                    }
                    ffi::wl_shm_buffer_end_access(shm);
                    retire_surface_buffer(rec);
                    (*rec).dmabuf = None;
                    (*rec).content_is_dmabuf = false;
                    (*rec).width = w;
                    (*rec).height = h;
                    (*rec).generation = (*rec).generation.wrapping_add(1);
                    (*rec).mapped = true;
                }
            }
            ffi::wl_resource_post_event(buffer, ffi::WL_BUFFER_RELEASE);
            if !(*rec).committed_explicit_release.is_null() {
                let release =
                    std::mem::replace(&mut (*rec).committed_explicit_release, std::ptr::null_mut());
                ffi::wl_resource_post_event(
                    release,
                    ffi::ZWP_LINUX_BUFFER_RELEASE_V1_IMMEDIATE_RELEASE,
                );
                ffi::wl_resource_destroy(release);
            }
            (*rec).pending_buffer = std::ptr::null_mut();
        }
    }

    if (*rec).mapped && !was_mapped && !(*rec).xdg_toplevel.is_null() {
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
        (*rec).window.size = (*rec)
            .window_geometry
            .map(|geometry| geometry.size)
            .unwrap_or_else(|| surface_logical_size(&*rec));
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
            let rule = st
                .window_rules
                .iter()
                .find(|r| r.matches(app_id.as_deref(), title.as_deref()));
            let rule_role = rule.and_then(|r| r.role);
            if let Some(rule) = rule {
                if let Some(ws_idx1) = rule.workspace {
                    let idx = (ws_idx1 as usize).saturating_sub(1);
                    if let Some(o) = st.workspaces.output(st.output) {
                        if let Some(&target) = o.workspaces.get(idx) {
                            st.workspaces.move_toplevel(id, target);
                        }
                    }
                }
            }
            // Pick the layout role (ADR-0024/0026): an explicit rule role
            // wins; a transient dialog always floats; otherwise the tiled
            // flag of the workspace the toplevel landed on decides.
            let workspace_tiled = st
                .workspaces
                .workspace_of(id)
                .and_then(|wid| st.workspaces.workspace(wid))
                .map(|ws| ws.tiled)
                .unwrap_or(false);
            (*rec).window.layout_role =
                resolve_layout_role(workspace_tiled, (*rec).window.parent.is_some(), rule_role);
        }
        // Live-update the foreign-toplevel list so taskbars see the new window.
        if !(*rec).state.is_null() {
            extensions::foreign_toplevel_added(rec, (*rec).state);
        }
    } else if (*rec).mapped && !(*rec).xdg_toplevel.is_null() {
        (*rec).window.size = (*rec)
            .window_geometry
            .map(|geometry| geometry.size)
            .unwrap_or_else(|| surface_logical_size(&*rec));
    }
    let children = (*rec).children.clone();
    for child in children {
        if child.is_null() || !(*child).subsurface_cached_commit {
            continue;
        }
        (*child).subsurface_applying_cached = true;
        surface_commit(std::ptr::null_mut(), (*child).resource);
        (*child).subsurface_applying_cached = false;
    }
    if !(*rec).state.is_null() && ((*rec).cursor_role || (*rec).drag_icon_role) {
        update_overlay_positions((*rec).state);
    }
    extensions::session_lock_surface_committed(rec);
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

unsafe extern "C" fn surface_noop_region(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _reg: *mut ffi::wl_resource,
) {
}

unsafe extern "C" fn surface_set_input_region(
    _client: *mut ffi::wl_client,
    surface: *mut ffi::wl_resource,
    region: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    let value = if region.is_null() {
        None
    } else {
        let region = ffi::wl_resource_get_user_data(region) as *mut RegionRec;
        if region.is_null() {
            Some(Vec::new())
        } else {
            Some((*region).rects.clone())
        }
    };
    (*rec).pending_input_region = Some(value);
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
        _ => {
            ffi::wl_resource_post_error(r, 1, c"invalid wl_output.transform value".as_ptr());
            return;
        }
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
    if value < 1 {
        ffi::wl_resource_post_error(r, 0, c"buffer scale must be positive".as_ptr());
        return;
    }
    (*rec).pending_scale = value;
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
    if !(*rec).viewport_resource.is_null() {
        let viewport = ffi::wl_resource_get_user_data((*rec).viewport_resource) as *mut ViewportRec;
        if !viewport.is_null() {
            (*viewport).surface = std::ptr::null_mut();
        }
        (*rec).viewport_resource = std::ptr::null_mut();
    }
    extensions::fractional_scale_surface_destroyed(rec);
    extensions::session_lock_surface_destroyed(rec);
    extensions::idle_inhibit_surface_destroyed(rec);
    extensions::explicit_sync_surface_destroyed(rec);
    retire_surface_buffer(rec);
    if (*rec).committed_acquire_fence >= 0 {
        libc_close((*rec).committed_acquire_fence);
        (*rec).committed_acquire_fence = -1;
    }
    if !(*rec).committed_explicit_release.is_null() {
        let release =
            std::mem::replace(&mut (*rec).committed_explicit_release, std::ptr::null_mut());
        ffi::wl_resource_post_event(release, ffi::ZWP_LINUX_BUFFER_RELEASE_V1_IMMEDIATE_RELEASE);
        ffi::wl_resource_destroy(release);
    }
    // Drop the toplevel from its workspace (ADR-0025). Idempotent: a no-op
    // for surfaces that never mapped or had no toplevel role. Run before the
    // slot is nulled so the resource address is still readable.
    if !(*rec).state.is_null() {
        extensions::keyboard_shortcuts_surface_destroyed((*rec).state, resource);
        if (*(*rec).state).cursor_surface == resource {
            (*(*rec).state).cursor_surface = std::ptr::null_mut();
            (*(*rec).state).cursor_hidden = false;
            (*(*rec).state).cursor_shape = 1;
        }
        if let Some(drag) = (*(*rec).state).drag.as_mut() {
            if drag.icon == resource {
                drag.icon = std::ptr::null_mut();
            }
        }
        if (*(*rec).state).pointer_focus == resource {
            (*(*rec).state).pointer_focus = std::ptr::null_mut();
        }
        if (*(*rec).state).keyboard_focus == resource {
            (*(*rec).state).keyboard_focus = std::ptr::null_mut();
        }
        if (*(*rec).state).tablet_focus == resource {
            (*(*rec).state).tablet_focus = std::ptr::null_mut();
        }
        let id = (*rec).window.id;
        if (*(*rec).state).pending_activation == resource {
            (*(*rec).state).pending_activation = std::ptr::null_mut();
        }
        (*(*rec).state).workspaces.remove_toplevel(id);
        // Notify foreign-toplevel listeners the window is gone.
        if !(*rec).xdg_toplevel.is_null() {
            extensions::foreign_toplevel_removed(id.0, (*rec).state);
        }
        for child in (*(*rec).state).live_surfaces() {
            if (*child).popup_parent == rec {
                (*child).popup_parent = std::ptr::null_mut();
            }
        }
    }
    (*rec).dmabuf = None;
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
    add: region_add,
    subtract: region_subtract,
};

unsafe extern "C" fn region_destroy(_client: *mut ffi::wl_client, resource: *mut ffi::wl_resource) {
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn region_add(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    let region = ffi::wl_resource_get_user_data(resource) as *mut RegionRec;
    if !region.is_null() && width > 0 && height > 0 {
        (*region)
            .rects
            .push(ass_core::Rect::new(x, y, width, height));
    }
}

fn subtract_rect(source: ass_core::Rect, cut: ass_core::Rect) -> Vec<ass_core::Rect> {
    let sx1 = source.origin.x;
    let sy1 = source.origin.y;
    let sx2 = sx1.saturating_add(source.size.w);
    let sy2 = sy1.saturating_add(source.size.h);
    let cx1 = cut.origin.x.max(sx1);
    let cy1 = cut.origin.y.max(sy1);
    let cx2 = cut.origin.x.saturating_add(cut.size.w).min(sx2);
    let cy2 = cut.origin.y.saturating_add(cut.size.h).min(sy2);
    if cx1 >= cx2 || cy1 >= cy2 {
        return vec![source];
    }
    let candidates = [
        ass_core::Rect::new(sx1, sy1, source.size.w, cy1 - sy1),
        ass_core::Rect::new(sx1, cy2, source.size.w, sy2 - cy2),
        ass_core::Rect::new(sx1, cy1, cx1 - sx1, cy2 - cy1),
        ass_core::Rect::new(cx2, cy1, sx2 - cx2, cy2 - cy1),
    ];
    candidates
        .into_iter()
        .filter(|rect| rect.size.w > 0 && rect.size.h > 0)
        .collect()
}

unsafe extern "C" fn region_subtract(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    let region = ffi::wl_resource_get_user_data(resource) as *mut RegionRec;
    if region.is_null() || width <= 0 || height <= 0 {
        return;
    }
    let cut = ass_core::Rect::new(x, y, width, height);
    (*region).rects = std::mem::take(&mut (*region).rects)
        .into_iter()
        .flat_map(|rect| subtract_rect(rect, cut))
        .collect();
}

unsafe extern "C" fn region_resource_destroy(resource: *mut ffi::wl_resource) {
    let region = ffi::wl_resource_get_user_data(resource) as *mut RegionRec;
    if !region.is_null() {
        drop(Box::from_raw(region));
    }
}

// ----- wl_output ----------------------------------------------------------

static OUTPUT_IMPL: ffi::wl_output_interface_impl = ffi::wl_output_interface_impl {
    release: res_destroy,
};

unsafe fn create_output_global(state: &mut State, info: ass_core::output::OutputInfo) {
    let mut record = Box::new(OutputGlobal {
        state: state as *mut State,
        info,
        global: std::ptr::null_mut(),
        active: true,
    });
    let data = record.as_mut() as *mut OutputGlobal as *mut c_void;
    record.global = ffi::wl_global_create(
        state.display,
        &ffi::wl_output_interface,
        4,
        data,
        output_bind,
    );
    if record.global.is_null() {
        log::error!("[server] wl_output global creation failed");
        record.active = false;
    }
    state.output_globals.push(record);
}

unsafe fn reconcile_output_globals(state: &mut State, outputs: &[ass_core::output::OutputInfo]) {
    for global in &mut state.output_globals {
        if !global.active {
            continue;
        }
        if let Some(info) = outputs
            .iter()
            .find(|output| output.connector == global.info.connector)
        {
            global.info = info.clone();
        } else {
            ffi::wl_global_destroy(global.global);
            global.global = std::ptr::null_mut();
            global.active = false;
        }
    }
    for output in outputs {
        let exists = state
            .output_globals
            .iter()
            .any(|global| global.active && global.info.connector == output.connector);
        if !exists {
            create_output_global(state, output.clone());
        }
    }
}

pub(crate) unsafe fn output_info_for_resource(
    resource: *mut ffi::wl_resource,
) -> Option<ass_core::output::OutputInfo> {
    if resource.is_null() {
        return None;
    }
    let global = ffi::wl_resource_get_user_data(resource) as *mut OutputGlobal;
    (!global.is_null()).then(|| (*global).info.clone())
}

/// Compute the (mode, integer-scale, transform) tuple from the state's
/// output record, with a sane default before the first backend update.
unsafe fn output_params(global: *mut OutputGlobal) -> (ass_core::output::OutputMode, i32, i32) {
    let mut mode = ass_core::output::OutputMode {
        width: 1280,
        height: 720,
        refresh_mhz: 60000,
    };
    let mut scale_i = 1i32;
    let mut transform = 0i32;
    if !global.is_null() {
        let g = (*global).info.geometry;
        if g.mode.width > 0 && g.mode.height > 0 {
            mode = g.mode;
        }
        scale_i = integer_output_scale(g.scale.0);
        transform = g.transform as i32;
    }
    (mode, scale_i, transform)
}

/// Legacy `wl_output.scale` is integer-only. Round upward so clients without
/// fractional-scale support render enough pixels for the compositor to
/// downsample instead of rendering at 1x and being blurred by upsampling.
fn integer_output_scale(scale: f32) -> i32 {
    scale.ceil().max(1.0) as i32
}

/// Post the full geometry + mode + scale + done sequence to one wl_output
/// resource. Version-gated: scale/done require v2.
unsafe fn send_output_geometry(res: *mut ffi::wl_resource) {
    let global = ffi::wl_resource_get_user_data(res) as *mut OutputGlobal;
    let (mode, scale_i, transform) = output_params(global);
    let version = ffi::wl_resource_get_version(res);
    let make = CString::new("ass").unwrap();
    let (origin, model_name) = if global.is_null() {
        (ass_core::Point::default(), "unknown")
    } else {
        (
            (*global).info.geometry.logical_origin,
            (*global).info.connector.as_str(),
        )
    };
    let model = CString::new(model_name).unwrap_or_else(|_| CString::new("output").unwrap());
    ffi::wl_resource_post_event(
        res,
        ffi::WL_OUTPUT_GEOMETRY,
        origin.x,
        origin.y,
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
    }
    if version >= 4 {
        let name = CString::new((*global).info.connector.as_str())
            .unwrap_or_else(|_| CString::new("unknown").unwrap());
        let description = CString::new(format!("ass output {}", (*global).info.connector))
            .unwrap_or_else(|_| CString::new("ass output").unwrap());
        ffi::wl_resource_post_event(res, ffi::WL_OUTPUT_NAME, name.as_ptr());
        ffi::wl_resource_post_event(res, ffi::WL_OUTPUT_DESCRIPTION, description.as_ptr());
    }
    if version >= 2 {
        ffi::wl_resource_post_event(res, ffi::WL_OUTPUT_DONE);
    }
}

unsafe extern "C" fn output_resource_destroy(resource: *mut ffi::wl_resource) {
    let global = ffi::wl_resource_get_user_data(resource) as *mut OutputGlobal;
    if global.is_null() || (*global).state.is_null() {
        return;
    }
    let state = (*global).state;
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
    let global = data as *mut OutputGlobal;
    ffi::wl_resource_set_implementation(
        res,
        &OUTPUT_IMPL as *const _ as *const c_void,
        global as *mut c_void,
        Some(output_resource_destroy),
    );
    if !global.is_null() && !(*global).state.is_null() {
        let state = (*global).state;
        (*state).output_resources.push(res);
    }

    send_output_geometry(res);
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
    set_anchor: positioner_set_anchor,
    set_gravity: positioner_set_gravity,
    set_constraint_adjustment: positioner_set_constraint_adjustment,
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

unsafe extern "C" fn positioner_set_anchor(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    anchor: u32,
) {
    let state = ffi::wl_resource_get_user_data(resource) as *mut PositionerState;
    if !state.is_null() && anchor <= 8 {
        (*state).anchor = anchor;
    }
}

unsafe extern "C" fn positioner_set_gravity(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    gravity: u32,
) {
    let state = ffi::wl_resource_get_user_data(resource) as *mut PositionerState;
    if !state.is_null() && gravity <= 8 {
        (*state).gravity = gravity;
    }
}

unsafe extern "C" fn positioner_set_constraint_adjustment(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    adjustment: u32,
) {
    let state = ffi::wl_resource_get_user_data(resource) as *mut PositionerState;
    if !state.is_null() {
        (*state).constraint_adjustment = adjustment & 0x3f;
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
    if rec.is_null() || surface_has_role(&*rec) {
        ffi::wl_resource_post_error(wm_base, 0, c"wl_surface already has a role".as_ptr());
        return;
    }
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
    set_window_geometry: xdg_surface_set_window_geometry,
    ack_configure: xdg_surface_ack_configure,
};

unsafe extern "C" fn xdg_surface_ack_configure(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    serial: u32,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    let Some(index) = (*rec)
        .pending_xdg_configures
        .iter()
        .position(|candidate| *candidate == serial)
    else {
        ffi::wl_resource_post_error(
            resource,
            4,
            c"xdg_surface.ack_configure used an unknown serial".as_ptr(),
        );
        return;
    };
    (*rec).pending_xdg_configures.drain(..=index);
    (*rec).xdg_configure_acked = true;
}

unsafe extern "C" fn xdg_surface_set_window_geometry(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if rec.is_null() || width <= 0 || height <= 0 {
        return;
    }
    (*rec).pending_window_geometry = Some(ass_core::Rect::new(x, y, width, height));
}

unsafe extern "C" fn xdg_surface_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() {
        if !(*rec).xdg_toplevel.is_null() || !(*rec).xdg_popup.is_null() {
            ffi::wl_resource_post_error(
                resource,
                6,
                c"xdg_surface role object must be destroyed first".as_ptr(),
            );
            return;
        }
        (*rec).xdg_surface = std::ptr::null_mut();
        (*rec).xdg_configured = false;
        (*rec).xdg_configure_acked = false;
        (*rec).pending_xdg_configures.clear();
    }
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn xdg_surface_get_toplevel(
    client: *mut ffi::wl_client,
    xdg_surface: *mut ffi::wl_resource,
    id: u32,
) {
    let rec = ffi::wl_resource_get_user_data(xdg_surface) as *mut SurfaceRec;
    if rec.is_null() || !(*rec).xdg_toplevel.is_null() || !(*rec).xdg_popup.is_null() {
        ffi::wl_resource_post_error(
            xdg_surface,
            2,
            c"xdg_surface already has a role object".as_ptr(),
        );
        return;
    }
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
    if rec.is_null() || !(*rec).xdg_toplevel.is_null() || !(*rec).xdg_popup.is_null() {
        ffi::wl_resource_post_error(
            xdg_surface,
            2,
            c"xdg_surface already has a role object".as_ptr(),
        );
        return;
    }
    if positioner.is_null() {
        ffi::wl_resource_post_error(xdg_surface, 5, c"popup requires a positioner".as_ptr());
        return;
    }
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
        // Positioner coordinates are parent-surface-local: anchor at the
        // parent's buffer draw origin, not its window-rect origin.
        let origin = surface_draw_origin(&*parent_rec);
        (origin.x, origin.y)
    };
    let anchor_rect = if !pos_state.is_null() {
        (*pos_state)
            .anchor_rect
            .unwrap_or_else(|| ass_core::Rect::new(0, 0, 1, 1))
    } else {
        ass_core::Rect::new(0, 0, 1, 1)
    };
    let anchor = if pos_state.is_null() {
        0
    } else {
        (*pos_state).anchor
    };
    let gravity = if pos_state.is_null() {
        0
    } else {
        (*pos_state).gravity
    };
    let offset = if pos_state.is_null() {
        ass_core::Point::default()
    } else {
        (*pos_state).offset
    };
    let mut popup_size = if !pos_state.is_null() {
        (*pos_state).size.unwrap_or(ass_core::Size { w: 0, h: 0 })
    } else {
        ass_core::Size { w: 0, h: 0 }
    };
    let anchor_x = match anchor {
        3 | 5 | 6 => anchor_rect.origin.x,
        4 | 7 | 8 => anchor_rect.origin.x + anchor_rect.size.w,
        _ => anchor_rect.origin.x + anchor_rect.size.w / 2,
    };
    let anchor_y = match anchor {
        1 | 5 | 7 => anchor_rect.origin.y,
        2 | 6 | 8 => anchor_rect.origin.y + anchor_rect.size.h,
        _ => anchor_rect.origin.y + anchor_rect.size.h / 2,
    };
    let gravity_x = match gravity {
        3 | 5 | 6 => -popup_size.w,
        4 | 7 | 8 => 0,
        _ => -popup_size.w / 2,
    };
    let gravity_y = match gravity {
        1 | 5 | 7 => -popup_size.h,
        2 | 6 | 8 => 0,
        _ => -popup_size.h / 2,
    };
    let mut local_x = anchor_x + gravity_x + offset.x;
    let mut local_y = anchor_y + gravity_y + offset.y;
    let adjustment = if pos_state.is_null() {
        0
    } else {
        (*pos_state).constraint_adjustment
    };
    if !(*rec).state.is_null() {
        let bounds = (*(*rec).state).output_geometry.logical_rect();
        let min_x = bounds.origin.x - px;
        let min_y = bounds.origin.y - py;
        let max_x = bounds.origin.x + bounds.size.w - px;
        let max_y = bounds.origin.y + bounds.size.h - py;
        if adjustment & (1 | 4) != 0 {
            local_x = local_x.clamp(min_x, (max_x - popup_size.w).max(min_x));
        } else if adjustment & 16 != 0 {
            popup_size.w = popup_size.w.min((max_x - local_x).max(1));
        }
        if adjustment & (2 | 8) != 0 {
            local_y = local_y.clamp(min_y, (max_y - popup_size.h).max(min_y));
        } else if adjustment & 32 != 0 {
            popup_size.h = popup_size.h.min((max_y - local_y).max(1));
        }
    }
    let popup_pos = ass_core::Point {
        x: px + local_x,
        y: py + local_y,
    };
    (*rec).position = popup_pos;
    (*rec).xdg_toplevel = std::ptr::null_mut(); // popups are not toplevels
    (*rec).xdg_popup = pop;
    (*rec).popup_parent = parent_rec;
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
        local_x,
        local_y,
        popup_size.w,
        popup_size.h,
    );
    // The xdg_surface configure serial must follow per xdg-shell.
    if !(*rec).xdg_surface.is_null() {
        send_xdg_surface_configure(rec);
        (*rec).xdg_configured = true;
    }
}

unsafe extern "C" fn popup_destroy(_client: *mut ffi::wl_client, resource: *mut ffi::wl_resource) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() {
        (*rec).xdg_popup = std::ptr::null_mut();
        (*rec).popup_parent = std::ptr::null_mut();
        (*rec).popup_grabbed = false;
        (*rec).mapped = false;
    }
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn popup_grab(
    _client: *mut ffi::wl_client,
    popup: *mut ffi::wl_resource,
    _seat: *mut ffi::wl_resource,
    _serial: u32,
) {
    let rec = ffi::wl_resource_get_user_data(popup) as *mut SurfaceRec;
    if !rec.is_null() {
        (*rec).popup_grabbed = true;
    }
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
        if !(*rec).xdg_decoration.is_null() {
            ffi::wl_resource_post_error(
                (*rec).xdg_decoration,
                ffi::ZXDG_TOPLEVEL_DECORATION_V1_ERROR_ORPHANED,
                c"xdg_toplevel destroyed before its decoration object".as_ptr(),
            );
            return;
        }
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

fn clamp_size_to_hints(
    requested: ass_core::Size,
    hints: ass_core::window::SizeHints,
) -> ass_core::Size {
    let min_w = hints.min_w.max(1);
    let min_h = hints.min_h.max(1);
    let max_w = if hints.max_w > 0 {
        hints.max_w.max(min_w)
    } else {
        i32::MAX
    };
    let max_h = if hints.max_h > 0 {
        hints.max_h.max(min_h)
    } else {
        i32::MAX
    };
    ass_core::Size {
        w: requested.w.clamp(min_w, max_w),
        h: requested.h.clamp(min_h, max_h),
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
        send_xdg_surface_configure(rec);
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
    minimize_toplevel_record(rec);
}

/// Apply compositor-internal minimization to one live toplevel record.
/// Kept separate from the protocol callback so shell/IPC actions follow the
/// exact same focus, configure, and client-flush semantics.
unsafe fn minimize_toplevel_record(rec: *mut SurfaceRec) {
    if rec.is_null() || (*rec).xdg_toplevel.is_null() || (*rec).window.minimized {
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
    client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    seat: *mut ffi::wl_resource,
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
    if state_ptr.is_null()
        || !(*state_ptr).implicit_grab_active
        || (*state_ptr).last_button_serial != serial
        || seat.is_null()
        || ffi::wl_resource_get_client(seat) != client
        || (*state_ptr).pointer_focus.is_null()
        || ffi::wl_resource_get_client((*state_ptr).pointer_focus) != client
    {
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
    client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    seat: *mut ffi::wl_resource,
    serial: u32,
    edges: u32,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    let state_ptr = (*rec).state;
    if state_ptr.is_null()
        || !(*state_ptr).implicit_grab_active
        || (*state_ptr).last_button_serial != serial
        || seat.is_null()
        || ffi::wl_resource_get_client(seat) != client
        || (*state_ptr).pointer_focus.is_null()
        || ffi::wl_resource_get_client((*state_ptr).pointer_focus) != client
    {
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
        start_size: (*rec)
            .window_geometry
            .map(|geometry| geometry.size)
            .unwrap_or_else(|| surface_logical_size(&*rec)),
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
                            send_xdg_surface_configure(rec_ptr);
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
    if child_rec.is_null()
        || parent_rec.is_null()
        || child_rec == parent_rec
        || surface_has_role(&*child_rec)
        || ffi::wl_resource_get_client(surface) != client
        || ffi::wl_resource_get_client(parent) != client
    {
        ffi::wl_resource_post_error(
            parent_res,
            0,
            c"invalid wl_subsurface child or parent".as_ptr(),
        );
        return;
    }
    let ver = ffi::wl_resource_get_version(parent_res);
    let sub = ffi::wl_resource_create(client, &ffi::wl_subsurface_interface, ver, id);
    if sub.is_null() {
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

// `wl_subsurface` request handlers. Synchronized children cache their pending
// surface state until the parent commits; desynchronized children apply
// immediately. Parent commits recursively release cached child commits.

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
    sibling: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    let sibling_rec = ffi::wl_resource_get_user_data(sibling) as *mut SurfaceRec;
    if rec.is_null() || (*rec).parent.is_null() {
        return;
    }
    let parent = (*rec).parent;
    (*parent).children.retain(|child| *child != rec);
    if sibling_rec == parent {
        (*rec).subsurface_above_parent = true;
        let index = (*parent)
            .children
            .iter()
            .position(|child| !child.is_null() && (**child).subsurface_above_parent)
            .unwrap_or((*parent).children.len());
        (*parent).children.insert(index, rec);
    } else if !sibling_rec.is_null() && (*sibling_rec).parent == parent {
        (*rec).subsurface_above_parent = (*sibling_rec).subsurface_above_parent;
        let index = (*parent)
            .children
            .iter()
            .position(|child| *child == sibling_rec)
            .map_or((*parent).children.len(), |index| index + 1);
        (*parent).children.insert(index, rec);
    } else {
        (*parent).children.push(rec);
    }
}

unsafe extern "C" fn subsurface_place_below(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    sibling: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    let sibling_rec = ffi::wl_resource_get_user_data(sibling) as *mut SurfaceRec;
    if rec.is_null() || (*rec).parent.is_null() {
        return;
    }
    let parent = (*rec).parent;
    (*parent).children.retain(|child| *child != rec);
    if sibling_rec == parent {
        (*rec).subsurface_above_parent = false;
        let index = (*parent)
            .children
            .iter()
            .position(|child| !child.is_null() && (**child).subsurface_above_parent)
            .unwrap_or((*parent).children.len());
        (*parent).children.insert(index, rec);
    } else if !sibling_rec.is_null() && (*sibling_rec).parent == parent {
        (*rec).subsurface_above_parent = (*sibling_rec).subsurface_above_parent;
        let index = (*parent)
            .children
            .iter()
            .position(|child| *child == sibling_rec)
            .unwrap_or(0);
        (*parent).children.insert(index, rec);
    } else {
        (*parent).children.insert(0, rec);
    }
}

unsafe extern "C" fn subsurface_set_sync(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if !rec.is_null() {
        (*rec).subsurface_sync = true;
    }
}

unsafe extern "C" fn subsurface_set_desync(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
    if rec.is_null() {
        return;
    }
    (*rec).subsurface_sync = false;
    if (*rec).subsurface_cached_commit {
        surface_commit(std::ptr::null_mut(), (*rec).resource);
    }
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
// `set_cursor` assigns the cursor role and exposes the surface as an overlay;
// the nested backend hides the host cursor while that overlay is active.
// `release` destroys the resource, which then runs
// `pointer_resource_destroy` to null out the slot.
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

/// `wl_pointer.set_cursor`: assign/update a custom cursor surface, or hide the
/// cursor for a null surface. Only the client holding pointer focus may use the
/// serial from its most recent enter event.
unsafe extern "C" fn pointer_set_cursor(
    client: *mut ffi::wl_client,
    pointer: *mut ffi::wl_resource,
    serial: u32,
    surface: *mut ffi::wl_resource,
    hotspot_x: i32,
    hotspot_y: i32,
) {
    let state = ffi::wl_resource_get_user_data(pointer) as *mut State;
    if state.is_null()
        || (*state).pointer_focus.is_null()
        || ffi::wl_resource_get_client((*state).pointer_focus) != client
        || serial != (*state).last_pointer_enter_serial
    {
        return;
    }
    if !surface.is_null() {
        if ffi::wl_resource_get_client(surface) != client {
            ffi::wl_resource_post_error(
                pointer,
                0,
                c"cursor surface belongs to another client".as_ptr(),
            );
            return;
        }
        let rec = ffi::wl_resource_get_user_data(surface) as *mut SurfaceRec;
        if rec.is_null() || (surface_has_role(&*rec) && !(*rec).cursor_role) {
            ffi::wl_resource_post_error(
                pointer,
                0,
                c"surface already has a different role".as_ptr(),
            );
            return;
        }
        (*rec).cursor_role = true;
    }
    (*state).cursor_surface = surface;
    (*state).cursor_hotspot = ass_core::Point {
        x: hotspot_x,
        y: hotspot_y,
    };
    (*state).cursor_shape = 0;
    (*state).cursor_hidden = true;
    update_overlay_positions(state);
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
    // Pointer is available through the nested backend; keyboard only when the
    // xkbcommon keymap compiled successfully. Do not advertise touch until a
    // backend actually supplies touch events.
    let mut caps = ffi::WL_SEAT_CAPABILITY_POINTER;
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

// ----- wl_data_device_manager (clipboard + DnD v3) ------------------------
//
// A functional single-seat clipboard: `set_selection` records the source and
// advertises a `wl_data_offer` to every bound `wl_data_device`. Clients paste
// by `data_offer.receive`, which we service from the source's `send` request.
// The MIME types offered come from `wl_data_source.offer`; version 3 action
// negotiation is implemented for copy/move/ask drag operations.

static DDM_IMPL: ffi::wl_data_device_manager_interface_impl =
    ffi::wl_data_device_manager_interface_impl {
        create_data_source: ddm_create_data_source,
        get_data_device: ddm_get_data_device,
    };

static DATA_DEVICE_IMPL: ffi::wl_data_device_interface_impl = ffi::wl_data_device_interface_impl {
    start_drag: ddev_start_drag,
    set_selection: ddev_set_selection,
};

static DATA_SOURCE_IMPL: ffi::wl_data_source_interface_impl = ffi::wl_data_source_interface_impl {
    offer: data_source_offer,
    destroy: res_destroy,
    set_actions: data_source_set_actions,
};

static DATA_OFFER_IMPL: ffi::wl_data_offer_interface_impl = ffi::wl_data_offer_interface_impl {
    accept: data_offer_accept,
    receive: data_offer_receive,
    destroy: res_destroy,
    finish: data_offer_finish,
    set_actions: data_offer_set_actions,
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

/// Create a `wl_data_source` whose MIME types/actions are collected before use.
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
    let state = ffi::wl_resource_get_user_data(mgr) as *mut State;
    let rec = Box::into_raw(Box::new(DataSourceRec {
        state,
        mime_types: Vec::new(),
        actions: if ver >= 3 {
            ffi::WL_DATA_ACTION_NONE
        } else {
            ffi::WL_DATA_ACTION_COPY
        },
        actions_set: false,
        used_for_drag: false,
    }));
    ffi::wl_resource_set_implementation(
        src,
        &DATA_SOURCE_IMPL as *const _ as *const c_void,
        rec as *mut c_void,
        Some(data_source_resource_destroy),
    );
}

unsafe extern "C" fn data_source_resource_destroy(resource: *mut ffi::wl_resource) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut DataSourceRec;
    if rec.is_null() {
        return;
    }
    let state = (*rec).state;
    if !state.is_null() {
        if (*state)
            .selection
            .as_ref()
            .is_some_and(|selection| selection.source == resource)
        {
            (*state).selection = None;
            notify_selection_cleared(state);
        }
        if (*state)
            .drag
            .as_ref()
            .is_some_and(|drag| drag.source == resource)
        {
            cancel_drag(state, false);
        }
        // Offers are client-owned and can outlive their source. Make their
        // back-pointer inert so a late receive cannot address freed memory.
        for offer in (*state)
            .data_offers
            .iter()
            .copied()
            .filter(|p| !p.is_null())
        {
            let offer_rec = ffi::wl_resource_get_user_data(offer) as *mut DataOfferRec;
            if !offer_rec.is_null() && (*offer_rec).source == resource {
                (*offer_rec).source = std::ptr::null_mut();
            }
        }
    }
    drop(Box::from_raw(rec));
}

unsafe extern "C" fn data_source_offer(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    mime_type: *const std::os::raw::c_char,
) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut DataSourceRec;
    if !rec.is_null() && !mime_type.is_null() {
        if let Ok(s) = CStr::from_ptr(mime_type).to_str() {
            if !(*rec).mime_types.iter().any(|m| m == s) {
                (*rec).mime_types.push(s.to_string());
            }
        }
    }
}

unsafe extern "C" fn data_source_set_actions(
    _client: *mut ffi::wl_client,
    source: *mut ffi::wl_resource,
    actions: u32,
) {
    let rec = ffi::wl_resource_get_user_data(source) as *mut DataSourceRec;
    if rec.is_null() {
        return;
    }
    if actions & !ffi::WL_DATA_ACTION_MASK != 0 {
        let msg = c"wl_data_source.set_actions: invalid action mask";
        ffi::wl_resource_post_error(
            source,
            ffi::WL_DATA_SOURCE_ERROR_INVALID_ACTION_MASK,
            msg.as_ptr(),
        );
        return;
    }
    if (*rec).actions_set || (*rec).used_for_drag {
        let msg = c"wl_data_source.set_actions may only be called once before start_drag";
        ffi::wl_resource_post_error(
            source,
            ffi::WL_DATA_SOURCE_ERROR_INVALID_SOURCE,
            msg.as_ptr(),
        );
        return;
    }
    (*rec).actions = actions;
    (*rec).actions_set = true;
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
        if !(*state).keyboard_focus.is_null()
            && ffi::wl_resource_get_client((*state).keyboard_focus) == client
        {
            if let Some(sel) = &(*state).selection {
                advertise_selection_offer(dev, sel);
            }
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
    if let Some(drag) = (*state).drag.as_mut() {
        if drag.target_device == resource {
            drag.focus = std::ptr::null_mut();
            drag.target_device = std::ptr::null_mut();
            drag.offer = std::ptr::null_mut();
        }
    }
}

unsafe fn notify_selection_cleared(state: *mut State) {
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
}

/// `wl_data_device.set_selection`: record the source as the current selection
/// and advertise a `wl_data_offer` to every bound data device. A null source
/// clears the selection.
unsafe extern "C" fn ddev_set_selection(
    client: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    source: *mut ffi::wl_resource,
    _serial: u32,
) {
    let state = ffi::wl_resource_get_user_data(_r) as *mut State;
    if state.is_null() {
        return;
    }
    if (*state).keyboard_focus.is_null()
        || ffi::wl_resource_get_client((*state).keyboard_focus) != client
        || (!source.is_null() && ffi::wl_resource_get_client(source) != client)
    {
        return;
    }
    if source.is_null() {
        if let Some(old) = (*state).selection.take() {
            ffi::wl_resource_post_event(old.source, ffi::WL_DATA_SOURCE_CANCELLED);
        }
        notify_selection_cleared(state);
        return;
    }
    // Build the selection record: the source resource + its collected MIMEs.
    let source_rec = ffi::wl_resource_get_user_data(source) as *mut DataSourceRec;
    if !source_rec.is_null() && (*source_rec).actions_set {
        let msg = c"data source configured for drag-and-drop cannot own selection";
        ffi::wl_resource_post_error(
            source,
            ffi::WL_DATA_SOURCE_ERROR_INVALID_SOURCE,
            msg.as_ptr(),
        );
        return;
    }
    let mimes = if source_rec.is_null() {
        Vec::new()
    } else {
        (*source_rec).mime_types.clone()
    };
    let sel = Selection {
        source,
        mime_types: mimes,
    };
    if let Some(old) = (*state).selection.replace(sel) {
        if old.source != source {
            ffi::wl_resource_post_event(old.source, ffi::WL_DATA_SOURCE_CANCELLED);
        }
    }
    let devices: Vec<*mut ffi::wl_resource> = (*state)
        .data_devices
        .iter()
        .copied()
        .filter(|p| !p.is_null() && ffi::wl_resource_get_client(*p) == client)
        .collect();
    for dev in devices {
        advertise_selection_offer(dev, (*state).selection.as_ref().unwrap());
    }
}

unsafe fn data_device_focus_changed(
    state: *mut State,
    old_focus: *mut ffi::wl_resource,
    new_focus: *mut ffi::wl_resource,
) {
    if state.is_null() {
        return;
    }
    let old_client = if old_focus.is_null() {
        std::ptr::null_mut()
    } else {
        ffi::wl_resource_get_client(old_focus)
    };
    let new_client = if new_focus.is_null() {
        std::ptr::null_mut()
    } else {
        ffi::wl_resource_get_client(new_focus)
    };
    for device in (*state).data_devices.clone() {
        if device.is_null() {
            continue;
        }
        let client = ffi::wl_resource_get_client(device);
        if !old_client.is_null() && client == old_client {
            ffi::wl_resource_post_event(
                device,
                ffi::WL_DATA_DEVICE_SELECTION,
                std::ptr::null_mut::<ffi::wl_resource>(),
            );
        }
        if !new_client.is_null() && client == new_client {
            if let Some(selection) = &(*state).selection {
                advertise_selection_offer(device, selection);
            } else {
                ffi::wl_resource_post_event(
                    device,
                    ffi::WL_DATA_DEVICE_SELECTION,
                    std::ptr::null_mut::<ffi::wl_resource>(),
                );
            }
        }
    }
}

/// Create a `wl_data_offer` for `sel`, send `data_offer` + its `offer` events
/// to `dev`, then `selection(offer)`.
unsafe fn create_data_offer(
    state: *mut State,
    dev: *mut ffi::wl_resource,
    source: *mut ffi::wl_resource,
    mime_types: &[String],
    is_drag: bool,
) -> *mut ffi::wl_resource {
    let client = ffi::wl_resource_get_client(dev);
    let version = ffi::wl_resource_get_version(dev).min(3);
    let offer = ffi::wl_resource_create(client, &ffi::wl_data_offer_interface, version, 0);
    if offer.is_null() {
        return std::ptr::null_mut();
    }
    let rec = Box::into_raw(Box::new(DataOfferRec {
        state,
        source,
        is_drag,
        accepted: false,
        destination_actions: ffi::WL_DATA_ACTION_NONE,
        preferred_action: ffi::WL_DATA_ACTION_NONE,
        selected_action: if is_drag && version < 3 {
            ffi::WL_DATA_ACTION_COPY
        } else {
            ffi::WL_DATA_ACTION_NONE
        },
        dropped: false,
        finished: false,
    }));
    ffi::wl_resource_set_implementation(
        offer,
        &DATA_OFFER_IMPL as *const _ as *const c_void,
        rec as *mut c_void,
        Some(data_offer_resource_destroy),
    );
    (*state).data_offers.push(offer);
    ffi::wl_resource_post_event(dev, ffi::WL_DATA_DEVICE_DATA_OFFER, offer);
    for mime in mime_types {
        let c = CString::new(mime.as_str()).unwrap();
        ffi::wl_resource_post_event(offer, ffi::WL_DATA_OFFER_OFFER, c.as_ptr());
    }
    if is_drag && version >= 3 {
        let actions = if source.is_null() {
            ffi::WL_DATA_ACTION_COPY
        } else {
            let source_rec = ffi::wl_resource_get_user_data(source) as *mut DataSourceRec;
            if source_rec.is_null() {
                ffi::WL_DATA_ACTION_NONE
            } else {
                (*source_rec).actions
            }
        };
        ffi::wl_resource_post_event(offer, ffi::WL_DATA_OFFER_SOURCE_ACTIONS, actions);
        ffi::wl_resource_post_event(offer, ffi::WL_DATA_OFFER_ACTION, ffi::WL_DATA_ACTION_NONE);
    }
    offer
}

unsafe fn advertise_selection_offer(dev: *mut ffi::wl_resource, sel: &Selection) {
    let state = ffi::wl_resource_get_user_data(dev) as *mut State;
    let offer = create_data_offer(state, dev, sel.source, &sel.mime_types, false);
    if offer.is_null() {
        return;
    }
    ffi::wl_resource_post_event(dev, ffi::WL_DATA_DEVICE_SELECTION, offer);
}

unsafe extern "C" fn data_offer_resource_destroy(resource: *mut ffi::wl_resource) {
    let rec = ffi::wl_resource_get_user_data(resource) as *mut DataOfferRec;
    if rec.is_null() {
        return;
    }
    let state = (*rec).state;
    let cancel_unfinished = (*rec).is_drag
        && (*rec).dropped
        && !(*rec).finished
        && !(*rec).source.is_null()
        && ffi::wl_resource_get_version((*rec).source) >= 3;
    if !state.is_null() {
        for slot in (*state).data_offers.iter_mut() {
            if *slot == resource {
                *slot = std::ptr::null_mut();
                break;
            }
        }
        if let Some(drag) = (*state).drag.as_mut() {
            if drag.offer == resource {
                drag.offer = std::ptr::null_mut();
            }
        }
    }
    if cancel_unfinished {
        ffi::wl_resource_post_event((*rec).source, ffi::WL_DATA_SOURCE_CANCELLED);
    }
    drop(Box::from_raw(rec));
}

unsafe extern "C" fn data_offer_accept(
    _client: *mut ffi::wl_client,
    offer: *mut ffi::wl_resource,
    _serial: u32,
    mime_type: *const std::os::raw::c_char,
) {
    let rec = ffi::wl_resource_get_user_data(offer) as *mut DataOfferRec;
    if rec.is_null() || !(*rec).is_drag || (*rec).source.is_null() {
        return;
    }
    (*rec).accepted = !mime_type.is_null();
    ffi::wl_resource_post_event((*rec).source, ffi::WL_DATA_SOURCE_TARGET, mime_type);
}

fn choose_dnd_action(source: u32, destination: u32, preferred: u32) -> u32 {
    let available = source & destination & ffi::WL_DATA_ACTION_MASK;
    if preferred != 0 && available & preferred != 0 {
        preferred
    } else if available & ffi::WL_DATA_ACTION_COPY != 0 {
        ffi::WL_DATA_ACTION_COPY
    } else if available & ffi::WL_DATA_ACTION_MOVE != 0 {
        ffi::WL_DATA_ACTION_MOVE
    } else if available & ffi::WL_DATA_ACTION_ASK != 0 {
        ffi::WL_DATA_ACTION_ASK
    } else {
        ffi::WL_DATA_ACTION_NONE
    }
}

unsafe fn post_dnd_action(offer: *mut ffi::wl_resource, action: u32) {
    let rec = ffi::wl_resource_get_user_data(offer) as *mut DataOfferRec;
    if rec.is_null() || (*rec).selected_action == action {
        return;
    }
    (*rec).selected_action = action;
    if ffi::wl_resource_get_version(offer) >= 3 {
        ffi::wl_resource_post_event(offer, ffi::WL_DATA_OFFER_ACTION, action);
    }
    let source = (*rec).source;
    if !source.is_null() && ffi::wl_resource_get_version(source) >= 3 {
        ffi::wl_resource_post_event(source, ffi::WL_DATA_SOURCE_ACTION, action);
    }
}

unsafe extern "C" fn data_offer_set_actions(
    _client: *mut ffi::wl_client,
    offer: *mut ffi::wl_resource,
    actions: u32,
    preferred: u32,
) {
    let rec = ffi::wl_resource_get_user_data(offer) as *mut DataOfferRec;
    if rec.is_null() || !(*rec).is_drag || (*rec).finished {
        ffi::wl_resource_post_error(
            offer,
            ffi::WL_DATA_OFFER_ERROR_INVALID_OFFER,
            c"set_actions is only valid on an active drag offer".as_ptr(),
        );
        return;
    }
    if actions & !ffi::WL_DATA_ACTION_MASK != 0 {
        ffi::wl_resource_post_error(
            offer,
            ffi::WL_DATA_OFFER_ERROR_INVALID_ACTION_MASK,
            c"wl_data_offer.set_actions: invalid action mask".as_ptr(),
        );
        return;
    }
    if preferred & !ffi::WL_DATA_ACTION_MASK != 0
        || preferred.count_ones() > 1
        || (preferred != 0 && actions & preferred == 0)
    {
        ffi::wl_resource_post_error(
            offer,
            ffi::WL_DATA_OFFER_ERROR_INVALID_ACTION,
            c"wl_data_offer.set_actions: invalid preferred action".as_ptr(),
        );
        return;
    }
    (*rec).destination_actions = actions;
    (*rec).preferred_action = preferred;
    let source_actions = if (*rec).source.is_null() {
        ffi::WL_DATA_ACTION_COPY
    } else {
        let source_rec = ffi::wl_resource_get_user_data((*rec).source) as *mut DataSourceRec;
        if source_rec.is_null() {
            ffi::WL_DATA_ACTION_NONE
        } else {
            (*source_rec).actions
        }
    };
    post_dnd_action(offer, choose_dnd_action(source_actions, actions, preferred));
}

unsafe extern "C" fn data_offer_finish(_client: *mut ffi::wl_client, offer: *mut ffi::wl_resource) {
    let rec = ffi::wl_resource_get_user_data(offer) as *mut DataOfferRec;
    if rec.is_null()
        || !(*rec).is_drag
        || !(*rec).dropped
        || !(*rec).accepted
        || (*rec).selected_action == ffi::WL_DATA_ACTION_NONE
        || (*rec).finished
    {
        ffi::wl_resource_post_error(
            offer,
            ffi::WL_DATA_OFFER_ERROR_INVALID_FINISH,
            c"wl_data_offer.finish called before a successful drop".as_ptr(),
        );
        return;
    }
    (*rec).finished = true;
    if !(*rec).source.is_null() && ffi::wl_resource_get_version((*rec).source) >= 3 {
        ffi::wl_resource_post_event((*rec).source, ffi::WL_DATA_SOURCE_DND_FINISHED);
    }
}

/// `wl_data_offer.receive`: forward to the source's `send` request so the
/// owning client writes the content for `mime_type` into `fd`.
unsafe extern "C" fn data_offer_receive(
    _client: *mut ffi::wl_client,
    offer: *mut ffi::wl_resource,
    mime_type: *const std::os::raw::c_char,
    fd: i32,
) {
    let rec = ffi::wl_resource_get_user_data(offer) as *mut DataOfferRec;
    let source = if rec.is_null() {
        std::ptr::null_mut()
    } else {
        (*rec).source
    };
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
    client: *mut ffi::wl_client,
    data_device: *mut ffi::wl_resource,
    source: *mut ffi::wl_resource,
    origin: *mut ffi::wl_resource,
    icon: *mut ffi::wl_resource,
    serial: u32,
) {
    let state = ffi::wl_resource_get_user_data(data_device) as *mut State;
    if state.is_null()
        || !(*state).implicit_grab_active
        || serial != (*state).last_button_serial
        || origin.is_null()
        || ffi::wl_resource_get_client(origin) != client
        || origin != (*state).pointer_focus
        || (!source.is_null() && ffi::wl_resource_get_client(source) != client)
        || (!icon.is_null() && ffi::wl_resource_get_client(icon) != client)
    {
        return;
    }
    if !icon.is_null() {
        let icon_rec = ffi::wl_resource_get_user_data(icon) as *mut SurfaceRec;
        if icon_rec.is_null() || surface_has_role(&*icon_rec) {
            ffi::wl_resource_post_error(
                data_device,
                0,
                c"drag icon surface already has a role".as_ptr(),
            );
            return;
        }
        (*icon_rec).drag_icon_role = true;
    }
    if !source.is_null() {
        if (*state)
            .selection
            .as_ref()
            .is_some_and(|selection| selection.source == source)
        {
            ffi::wl_resource_post_error(
                source,
                ffi::WL_DATA_SOURCE_ERROR_INVALID_SOURCE,
                c"selection source cannot be reused for drag-and-drop".as_ptr(),
            );
            return;
        }
        let source_rec = ffi::wl_resource_get_user_data(source) as *mut DataSourceRec;
        if source_rec.is_null() || (*source_rec).used_for_drag {
            ffi::wl_resource_post_error(
                source,
                ffi::WL_DATA_SOURCE_ERROR_INVALID_SOURCE,
                c"data source has already been used for drag-and-drop".as_ptr(),
            );
            return;
        }
        (*source_rec).used_for_drag = true;
    }
    if (*state).drag.is_some() {
        cancel_drag(state, true);
    }
    (*state).drag = Some(DragState {
        source,
        origin,
        focus: std::ptr::null_mut(),
        target_device: std::ptr::null_mut(),
        offer: std::ptr::null_mut(),
        icon,
    });
    update_overlay_positions(state);
    update_drag_focus(
        state,
        (*state).pointer_focus,
        (*state).pointer_x,
        (*state).pointer_y,
        0,
    );
}

unsafe fn data_device_for_client(
    state: *mut State,
    client: *mut ffi::wl_client,
) -> *mut ffi::wl_resource {
    (*state)
        .data_devices
        .iter()
        .copied()
        .find(|device| !device.is_null() && ffi::wl_resource_get_client(*device) == client)
        .unwrap_or(std::ptr::null_mut())
}

/// Move the active DnD implicit grab to `focus` and emit the version-1
/// data-device enter/leave/motion sequence. Coordinates are converted from
/// compositor logical space to the destination surface's local space.
unsafe fn update_drag_focus(
    state: *mut State,
    mut focus: *mut ffi::wl_resource,
    x: f32,
    y: f32,
    time: u32,
) {
    let Some(mut drag) = (*state).drag else {
        return;
    };

    // A null source denotes client-internal DnD; the protocol restricts its
    // enter/motion events to surfaces belonging to the initiating client.
    if !focus.is_null()
        && drag.source.is_null()
        && ffi::wl_resource_get_client(focus) != ffi::wl_resource_get_client(drag.origin)
    {
        focus = std::ptr::null_mut();
    }
    let target_device = if focus.is_null() {
        std::ptr::null_mut()
    } else {
        data_device_for_client(state, ffi::wl_resource_get_client(focus))
    };
    if target_device.is_null() {
        focus = std::ptr::null_mut();
    }

    if focus == drag.focus && target_device == drag.target_device {
        if !target_device.is_null() {
            let surface = ffi::wl_resource_get_user_data(focus) as *mut SurfaceRec;
            if !surface.is_null() {
                let origin = surface_draw_origin(&*surface);
                ffi::wl_resource_post_event(
                    target_device,
                    ffi::WL_DATA_DEVICE_MOTION,
                    time,
                    ffi::wl_fixed_from_f32(x - origin.x as f32),
                    ffi::wl_fixed_from_f32(y - origin.y as f32),
                );
            }
        }
        return;
    }

    if !drag.target_device.is_null() {
        ffi::wl_resource_post_event(drag.target_device, ffi::WL_DATA_DEVICE_LEAVE);
    }
    if !drag.source.is_null() {
        ffi::wl_resource_post_event(
            drag.source,
            ffi::WL_DATA_SOURCE_TARGET,
            std::ptr::null::<std::os::raw::c_char>(),
        );
    }

    drag.focus = focus;
    drag.target_device = target_device;
    drag.offer = std::ptr::null_mut();

    if !target_device.is_null() {
        let mime_types = if drag.source.is_null() {
            Vec::new()
        } else {
            let source_rec = ffi::wl_resource_get_user_data(drag.source) as *mut DataSourceRec;
            if source_rec.is_null() {
                Vec::new()
            } else {
                (*source_rec).mime_types.clone()
            }
        };
        if !drag.source.is_null() {
            drag.offer = create_data_offer(state, target_device, drag.source, &mime_types, true);
        }
        let surface = ffi::wl_resource_get_user_data(focus) as *mut SurfaceRec;
        if !surface.is_null() {
            let serial = ffi::wl_display_next_serial((*state).display);
            let origin = surface_draw_origin(&*surface);
            ffi::wl_resource_post_event(
                target_device,
                ffi::WL_DATA_DEVICE_ENTER,
                serial,
                focus,
                ffi::wl_fixed_from_f32(x - origin.x as f32),
                ffi::wl_fixed_from_f32(y - origin.y as f32),
                drag.offer,
            );
        }
    }
    (*state).drag = Some(drag);
}

unsafe fn cancel_drag(state: *mut State, notify_source: bool) {
    let Some(drag) = (*state).drag.take() else {
        return;
    };
    if !drag.target_device.is_null() {
        ffi::wl_resource_post_event(drag.target_device, ffi::WL_DATA_DEVICE_LEAVE);
    }
    if notify_source && !drag.source.is_null() {
        ffi::wl_resource_post_event(drag.source, ffi::WL_DATA_SOURCE_CANCELLED);
    }
    clear_drag_icon(drag.icon);
}

unsafe fn clear_drag_icon(icon: *mut ffi::wl_resource) {
    if icon.is_null() {
        return;
    }
    let rec = ffi::wl_resource_get_user_data(icon) as *mut SurfaceRec;
    if !rec.is_null() {
        (*rec).drag_icon_role = false;
    }
}

unsafe fn finish_drag(state: *mut State) {
    let Some(drag) = (*state).drag.take() else {
        return;
    };
    let offer_rec = if drag.offer.is_null() {
        std::ptr::null_mut()
    } else {
        ffi::wl_resource_get_user_data(drag.offer) as *mut DataOfferRec
    };
    let accepted = !offer_rec.is_null()
        && (*offer_rec).accepted
        && (*offer_rec).selected_action != ffi::WL_DATA_ACTION_NONE;
    if drag.target_device.is_null() || !accepted {
        if !drag.source.is_null() {
            ffi::wl_resource_post_event(drag.source, ffi::WL_DATA_SOURCE_CANCELLED);
        }
    } else {
        (*offer_rec).dropped = true;
        if !drag.source.is_null() && ffi::wl_resource_get_version(drag.source) >= 3 {
            ffi::wl_resource_post_event(drag.source, ffi::WL_DATA_SOURCE_DND_DROP_PERFORMED);
        }
        ffi::wl_resource_post_event(drag.target_device, ffi::WL_DATA_DEVICE_DROP);
    }
    clear_drag_icon(drag.icon);
}

// ----- zwp_linux_dmabuf_v1 ------------------------------------------------

/// DRM fourccs advertised to clients. ARGB8888 and ABGR8888 are the two
/// 32-bit-per-pixel byte orderings clients actually use; the X-variants are
/// the alpha-undefined counterparts (the server forces alpha opaque).
const DRM_FORMAT_ARGB8888: u32 = 0x3432_5241;
const DRM_FORMAT_XRGB8888: u32 = 0x3432_5258;
const DRM_FORMAT_ABGR8888: u32 = 0x3432_4241;
const DRM_FORMAT_XBGR8888: u32 = 0x3432_4258;
/// DRM modifier advertised. Flux imports an explicit single-plane layout, so
/// do not advertise INVALID/implicit and let clients select a layout we cannot
/// validate or sample.
const DRM_FORMAT_MOD_LINEAR: u64 = 0;

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
            let hi = (DRM_FORMAT_MOD_LINEAR >> 32) as u32;
            let lo = (DRM_FORMAT_MOD_LINEAR & 0xffff_ffff) as u32;
            ffi::wl_resource_post_event(res, ffi::ZWP_LINUX_DMABUF_V1_MODIFIER, fmt, hi, lo);
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
    let state = ffi::wl_resource_get_user_data(dmabuf) as *mut State;
    let acc = Box::into_raw(Box::new(DmabufBuffer::empty(state)));
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
    (*acc).resource = buffer;
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
        let state = (*db).state;
        if !state.is_null() {
            for surface in (*state).live_surfaces() {
                if (*surface).current_buffer == resource {
                    (*surface).current_buffer = std::ptr::null_mut();
                }
            }
            for retired in &mut (*state).retired_buffer_releases {
                if retired.buffer == resource {
                    retired.buffer = std::ptr::null_mut();
                }
            }
        }
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
// Crop/scale state is double-buffered with wl_surface.commit and surfaced to
// the renderer through SurfaceGeometry.

struct ViewportRec {
    surface: *mut SurfaceRec,
}

static VIEWPORTER_IMPL: ffi::wp_viewporter_interface_impl = ffi::wp_viewporter_interface_impl {
    destroy: res_destroy,
    get_viewport: viewporter_get_viewport,
};

static VIEWPORT_IMPL: ffi::wp_viewport_interface_impl = ffi::wp_viewport_interface_impl {
    destroy: viewport_destroy,
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
    if rec.is_null() {
        return;
    }
    if !(*rec).viewport_resource.is_null() {
        ffi::wl_resource_post_error(
            viewporter,
            0,
            c"wl_surface already has a wp_viewport".as_ptr(),
        );
        return;
    }
    let ver = ffi::wl_resource_get_version(viewporter);
    let vp = ffi::wl_resource_create(client, &ffi::wp_viewport_interface, ver, id);
    if vp.is_null() {
        return;
    }
    let viewport_rec = Box::into_raw(Box::new(ViewportRec { surface: rec }));
    ffi::wl_resource_set_implementation(
        vp,
        &VIEWPORT_IMPL as *const _ as *mut c_void,
        viewport_rec as *mut c_void,
        Some(viewport_resource_destroy),
    );
    (*rec).viewport_resource = vp;
}

unsafe extern "C" fn viewport_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    let viewport = ffi::wl_resource_get_user_data(resource) as *mut ViewportRec;
    if !viewport.is_null() && !(*viewport).surface.is_null() {
        let surface = (*viewport).surface;
        (*surface).pending_viewport_src = Some(None);
        (*surface).pending_viewport_dst = Some(None);
        (*surface).viewport_resource = std::ptr::null_mut();
        (*viewport).surface = std::ptr::null_mut();
    }
    ffi::wl_resource_destroy(resource);
}

unsafe extern "C" fn viewport_resource_destroy(resource: *mut ffi::wl_resource) {
    let viewport = ffi::wl_resource_get_user_data(resource) as *mut ViewportRec;
    if viewport.is_null() {
        return;
    }
    if !(*viewport).surface.is_null() {
        (*(*viewport).surface).viewport_resource = std::ptr::null_mut();
    }
    drop(Box::from_raw(viewport));
}

unsafe fn viewport_surface(resource: *mut ffi::wl_resource) -> *mut SurfaceRec {
    let viewport = ffi::wl_resource_get_user_data(resource) as *mut ViewportRec;
    if viewport.is_null() || (*viewport).surface.is_null() {
        ffi::wl_resource_post_error(resource, 3, c"associated wl_surface was destroyed".as_ptr());
        std::ptr::null_mut()
    } else {
        (*viewport).surface
    }
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
    let rec = viewport_surface(resource);
    if rec.is_null() {
        return;
    }
    if x == -1 && y == -1 && w == -1 && h == -1 {
        (*rec).pending_viewport_src = Some(None);
        return;
    }
    if x < 0 || y < 0 || w <= 0 || h <= 0 {
        ffi::wl_resource_post_error(resource, 0, c"invalid viewport source rectangle".as_ptr());
        return;
    }
    (*rec).pending_viewport_src = Some(Some(ass_core::Rect::new(
        fixed_to_f32(x).round() as i32,
        fixed_to_f32(y).round() as i32,
        fixed_to_f32(w).round() as i32,
        fixed_to_f32(h).round() as i32,
    )));
}

/// `wp_viewport.set_destination`: sets the destination size in integer
/// logical pixels. A value of -1 for either field resets.
unsafe extern "C" fn viewport_set_destination(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    w: i32,
    h: i32,
) {
    let rec = viewport_surface(resource);
    if rec.is_null() {
        return;
    }
    if w == -1 && h == -1 {
        (*rec).pending_viewport_dst = Some(None);
        return;
    }
    if w <= 0 || h <= 0 {
        ffi::wl_resource_post_error(resource, 0, c"invalid viewport destination size".as_ptr());
        return;
    }
    (*rec).pending_viewport_dst = Some(Some(ass_core::Size { w, h }));
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

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn noop_none(_c: *mut ffi::wl_client, _r: *mut ffi::wl_resource) {}

pub(crate) unsafe extern "C" fn noop_obj(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _obj: *mut ffi::wl_resource,
) {
}

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn noop_obj_serial(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _obj: *mut ffi::wl_resource,
    _serial: u32,
) {
}

pub(crate) unsafe extern "C" fn noop_str(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _s: *const std::os::raw::c_char,
) {
}

#[allow(dead_code)]
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

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn noop_uu(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _a: u32,
    _b: u32,
) {
}

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn noop_rect(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _x: i32,
    _y: i32,
    _w: i32,
    _h: i32,
) {
}

#[allow(dead_code)]
pub(crate) unsafe extern "C" fn noop_region(
    _c: *mut ffi::wl_client,
    _r: *mut ffi::wl_resource,
    _reg: *mut ffi::wl_resource,
) {
}

#[allow(dead_code)]
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

#[allow(dead_code)]
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
