//! Hand-rolled Wayland server for aegis.
//!
//! Drives libwayland-server directly over FFI: it creates the display and
//! socket, advertises the core globals, and owns protocol object lifecycle. The
//! shm implementation and the core `wl_*` interface tables come from
//! libwayland-server; aegis implements the request handlers.
//!
//! libwayland callbacks receive the stable boxed `State` as a raw pointer.
//! Accessing human-seat fields through `State`'s `Deref<SeatRuntime>` creates
//! the same short-lived references that direct fields previously created.
//! The callback lifetime invariant is documented on `State`.
#![allow(dangerous_implicit_autorefs)]

mod extensions;
mod ffi;
mod keyboard;
mod protocol;
mod server;

#[cfg(test)]
mod tests;

pub(crate) use protocol::*;
pub use server::ClipboardError;

use std::ffi::{CStr, CString, c_void};
use std::ops::{Deref, DerefMut};
use std::os::fd::IntoRawFd;
use std::os::raw::{c_int, c_ulong};
use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
use std::os::unix::net::UnixListener;
use std::path::{Path, PathBuf};

use aegis_core::layout::Layout;
use aegis_core::realm::{
    AuthorityTransfer, HUMAN_PRINCIPAL, HUMAN_REALM, HUMAN_SEAT, PresentationTarget, PrincipalId,
    RealmBundle, RealmError, RealmId, RealmModel, RealmMutation, RealmMutationResult,
    RealmRevocation, RealmSnapshot, RealmTransactionReceipt, SeatCapabilities, SeatId,
    TransferOptions, VirtualOutput,
};
use aegis_core::{SurfaceDmabuf, SurfacePixels};

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

/// The active clipboard selection and its advertised MIME types. A selection
/// is owned either by a client's `wl_data_source` or by immutable compositor
/// payloads, and is advertised to the focused client through `wl_data_offer`.
struct Selection {
    source: *mut ffi::wl_resource,
    mime_types: Vec<String>,
    /// Immutable compositor-owned payloads. Client-owned selections leave
    /// this unset and transfer through `wl_data_source.send` instead.
    owned: Option<OwnedSelection>,
}

/// One immutable snapshot of compositor-owned clipboard data. Offers retain
/// this snapshot independently, so replacing the current selection cannot
/// invalidate an already-issued capability.
#[derive(Clone)]
struct OwnedSelection {
    payloads: std::sync::Arc<Vec<(String, std::sync::Arc<[u8]>)>>,
}

impl OwnedSelection {
    fn payload(&self, mime_type: &str) -> Option<std::sync::Arc<[u8]>> {
        self.payloads
            .iter()
            .find_map(|(mime, bytes)| (mime == mime_type).then(|| std::sync::Arc::clone(bytes)))
    }
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
    owned: Option<OwnedSelection>,
    is_drag: bool,
    accepted: bool,
    destination_actions: u32,
    preferred_action: u32,
    selected_action: u32,
    dropped: bool,
    finished: bool,
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
    size: Option<aegis_core::Size>,
    anchor_rect: Option<aegis_core::Rect>,
    anchor: u32,
    gravity: u32,
    constraint_adjustment: u32,
    offset: aegis_core::Point,
}

#[derive(Default)]
struct RegionRec {
    rects: Vec<aegis_core::Rect>,
}

// Minimal close() without pulling the libc crate.
unsafe extern "C" {
    fn close(fd: std::os::raw::c_int) -> std::os::raw::c_int;
    fn dup(fd: std::os::raw::c_int) -> std::os::raw::c_int;
    fn malloc(size: usize) -> *mut c_void;
    fn free(ptr: *mut c_void);
    fn ioctl(fd: c_int, request: c_ulong, ...) -> c_int;
}
pub(crate) unsafe fn libc_close(fd: i32) {
    unsafe {
        close(fd);
    }
}

/// A client surface: its pending buffer, the last committed contents copied out
/// of shm, and its xdg role.
pub struct SurfaceRec {
    pub resource: *mut ffi::wl_resource,
    client_id: aegis_core::realm::ClientId,
    pending_buffer: *mut ffi::wl_resource,
    pending_buffer_set: bool,
    pending_attach_offset: aegis_core::Point,
    attach_offset: aegis_core::Point,
    pub mapped: bool,
    pub width: i32,
    pub height: i32,
    /// Logical position of the window rect's top-left corner in compositor
    /// space. For surfaces without a client-declared window geometry this is
    /// also the buffer's draw origin; for CSD surfaces that exclude shadows
    /// via `set_window_geometry` the buffer is drawn up-left of this point
    /// (see [`surface_draw_origin`]). M1 assigns a placeholder cascade on
    /// map; M3's window manager will own placement policy.
    pub position: aegis_core::Point,
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
    /// Routed seat that owns this popup's explicit grab. A grabbing popup is
    /// the keyboard-focus target for this seat from map until dismissal.
    popup_grab_seat: Option<SeatId>,
    cursor_role: bool,
    drag_icon_role: bool,
    /// The permanent input-popup role. The protocol object may be destroyed,
    /// but a Wayland surface role remains assigned for the surface lifetime.
    input_popup_role: bool,
    /// Active `InputPopupRec`, owned by its protocol resource.
    input_popup_surface: *mut c_void,
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
    subsurface_offset: aegis_core::Point,
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
    pub window: aegis_core::window::Window,
    /// Tiling target (ADR-0024): the layout rect the tiling policy last
    /// configured this surface to, or `None` when not under active tiling.
    /// The apply path reconfigures only when the target moves.
    pub layout_target: Option<aegis_core::Rect>,
    /// Saved floating position and size prior to maximizing or full-screening,
    /// restored when unmaximized/unfullscreened.
    pub saved_floating_rect: Option<aegis_core::Rect>,
    // ----- wp_viewport state -----
    /// Source rectangle in surface pixel coords, or None for "whole buffer".
    /// Set by `wp_viewport.set_source`. Coordinates arrive as 24.8
    /// fixed-point; we store them as f32.
    pub viewport_src: Option<aegis_core::Rect>,
    pending_viewport_src: Option<Option<aegis_core::Rect>>,
    /// Destination size in logical pixels, or None for "source size".
    /// Set by `wp_viewport.set_destination`.
    pub viewport_dst: Option<aegis_core::Size>,
    pending_viewport_dst: Option<Option<aegis_core::Size>>,
    viewport_resource: *mut ffi::wl_resource,
    // ----- wp_fractional_scale_v1 state -----
    /// The `wp_fractional_scale_v1` resource bound for this surface, if any.
    /// The server posts `preferred_scale` here when the output's scale changes.
    pub fractional_scale: *mut ffi::wl_resource,
    /// Committed xdg-shell window geometry (excluding client shadows). Its
    /// size is the window rect's size; its origin is the frame inset by
    /// which the buffer sits up-left of the window rect (see
    /// [`surface_draw_origin`]).
    window_geometry: Option<aegis_core::Rect>,
    pending_window_geometry: Option<aegis_core::Rect>,
    /// `None` means the whole surface accepts input; `Some` is the union of
    /// rectangles copied from the last committed `wl_region`.
    input_region: Option<Vec<aegis_core::Rect>>,
    pending_input_region: Option<Option<Vec<aegis_core::Rect>>>,
    // ----- pending buffer transform / scale -----
    /// Pending buffer transform from `wl_surface.set_buffer_transform`,
    /// applied on the next commit.
    pending_transform: aegis_core::Transform,
    buffer_transform: aegis_core::Transform,
    /// Pending buffer scale from `wl_surface.set_buffer_scale`.
    pending_scale: i32,
    buffer_scale: i32,
    // ----- damage tracking -----
    /// Damage rectangles accumulated by `wl_surface.damage` /
    /// `damage_buffer` since the last commit. Surface-local pixel coords;
    /// empty means "client did not report damage, renderer should
    /// re-upload the whole texture on a generation change".
    pending_damage: Vec<aegis_core::Rect>,
    /// Damage accumulated across every commit since the last successfully
    /// presented compositor frame, surfaced via `Server::toplevel_frames`.
    /// Multiple client commits can be dispatched before one render, so
    /// replacing this at each commit would make both texture upload and KMS
    /// damage miss earlier changed pixels.
    committed_damage: Vec<aegis_core::Rect>,
    /// Empty `committed_damage` normally means no outstanding damage. This
    /// flag distinguishes the conservative "damage is unknown/full" state.
    committed_damage_full: bool,
}

impl SurfaceRec {
    fn new(resource: *mut ffi::wl_resource) -> SurfaceRec {
        SurfaceRec {
            resource,
            client_id: aegis_core::realm::ClientId::default(),
            pending_buffer: std::ptr::null_mut(),
            pending_buffer_set: false,
            pending_attach_offset: aegis_core::Point::default(),
            attach_offset: aegis_core::Point::default(),
            mapped: false,
            width: 0,
            height: 0,
            position: aegis_core::Point::default(),
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
            popup_grab_seat: None,
            cursor_role: false,
            drag_icon_role: false,
            input_popup_role: false,
            input_popup_surface: std::ptr::null_mut(),
            session_lock_surface: std::ptr::null_mut(),
            xdg_configured: false,
            xdg_configure_acked: false,
            pending_xdg_configures: Vec::new(),
            display: std::ptr::null_mut(),
            state: std::ptr::null_mut(),
            index: 0,
            parent: std::ptr::null_mut(),
            children: Vec::new(),
            subsurface_offset: aegis_core::Point::default(),
            subsurface_above_parent: true,
            subsurface_sync: true,
            subsurface_cached_commit: false,
            subsurface_applying_cached: false,
            window: aegis_core::window::Window::default(),
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
            pending_transform: aegis_core::Transform::Normal,
            buffer_transform: aegis_core::Transform::Normal,
            pending_scale: 1,
            buffer_scale: 1,
            pending_damage: Vec::new(),
            committed_damage: Vec::new(),
            committed_damage_full: false,
            // Tiling (ADR-0024): the last layout rect we configured this
            // surface to. `None` until applied; the apply path reconfigures
            // only when the target moves, so steady state sends no configures.
            layout_target: None,
            saved_floating_rect: None,
        }
    }
}

fn surface_logical_size(surface: &SurfaceRec) -> aegis_core::Size {
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
    aegis_core::Size {
        w: (width as f32 / scale).round().max(1.0) as i32,
        h: (height as f32 / scale).round().max(1.0) as i32,
    }
}

fn intersect_rect(a: aegis_core::Rect, b: aegis_core::Rect) -> Option<aegis_core::Rect> {
    let ax1 = i64::from(a.origin.x) + i64::from(a.size.w.max(0));
    let ay1 = i64::from(a.origin.y) + i64::from(a.size.h.max(0));
    let bx1 = i64::from(b.origin.x) + i64::from(b.size.w.max(0));
    let by1 = i64::from(b.origin.y) + i64::from(b.size.h.max(0));
    let x0 = i64::from(a.origin.x).max(i64::from(b.origin.x));
    let y0 = i64::from(a.origin.y).max(i64::from(b.origin.y));
    let x1 = ax1.min(bx1);
    let y1 = ay1.min(by1);
    (x1 > x0 && y1 > y0)
        .then(|| aegis_core::Rect::new(x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32))
}

/// Clip, deduplicate and bound one Realm's damage metadata. The compositor
/// never exposes unchecked client coordinates through IPC.
fn normalize_realm_damage(rects: &mut Vec<aegis_core::Rect>, output: aegis_core::Rect) {
    *rects = rects
        .drain(..)
        .filter_map(|rect| intersect_rect(rect, output))
        .collect();
    rects.sort_by_key(|rect| (rect.origin.x, rect.origin.y, rect.size.w, rect.size.h));
    rects.dedup();
    const MAX_DAMAGE_RECTS: usize = 64;
    if rects.len() <= MAX_DAMAGE_RECTS {
        return;
    }
    let x0 = rects.iter().map(|rect| rect.origin.x).min().unwrap_or(0);
    let y0 = rects.iter().map(|rect| rect.origin.y).min().unwrap_or(0);
    let x1 = rects
        .iter()
        .map(|rect| i64::from(rect.origin.x) + i64::from(rect.size.w))
        .max()
        .unwrap_or(i64::from(x0));
    let y1 = rects
        .iter()
        .map(|rect| i64::from(rect.origin.y) + i64::from(rect.size.h))
        .max()
        .unwrap_or(i64::from(y0));
    rects.clear();
    rects.push(aegis_core::Rect::new(
        x0,
        y0,
        (x1 - i64::from(x0)) as i32,
        (y1 - i64::from(y0)) as i32,
    ));
}

/// Draw origin of the surface's buffer in compositor space. For surfaces
/// with a client-declared window geometry (xdg-shell `set_window_geometry`,
/// used by client-side-decorated windows to exclude shadows), the buffer is
/// drawn up-left of the window rect by the geometry's insets. A subsurface
/// is anchored in its parent's buffer space, so its origin resolves through
/// the parent chain — this is what makes nested subsurfaces (a subsurface
/// with its own subsurfaces) land at the right compositor position.
pub(crate) fn surface_draw_origin(surface: &SurfaceRec) -> aegis_core::Point {
    surface_draw_origin_depth(surface, 0)
}

fn surface_draw_origin_depth(surface: &SurfaceRec, depth: u32) -> aegis_core::Point {
    // The depth cap only breaks reference cycles defensively; the destroy
    // path orphans children, so a live parent pointer is always valid.
    if !surface.parent.is_null() && depth < 32 {
        let parent = unsafe { &*surface.parent };
        let origin = surface_draw_origin_depth(parent, depth + 1);
        return aegis_core::Point {
            x: origin.x + surface.subsurface_offset.x,
            y: origin.y + surface.subsurface_offset.y,
        };
    }
    match surface.window_geometry {
        Some(geometry) => aegis_core::Point {
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
            rect.contains(aegis_core::Point {
                x: local_x as i32,
                y: local_y as i32,
            })
        })
    })
}

/// `wp_cursor_shape_device_v1.shape` value for one xdg-shell resize edge set.
fn resize_cursor_shape(edges: aegis_core::window::ResizeEdges) -> u32 {
    use aegis_core::window::ResizeEdges;
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
        || surface.input_popup_role
        || !surface.session_lock_surface.is_null()
}

/// Resolve a newly mapped toplevel's layout role: an explicit window
/// rule wins; a transient (dialog) always floats (ADR-0024 floating
/// exception); otherwise the workspace's tiled flag decides.
fn resolve_layout_role(
    workspace_tiled: bool,
    is_transient: bool,
    rule_role: Option<aegis_core::layout::LayoutRole>,
) -> aegis_core::layout::LayoutRole {
    use aegis_core::layout::LayoutRole;
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
    unsafe {
        update_overlay_positions_for_seat(state, (*state).active_seat);
    }
}

unsafe fn update_overlay_positions_for_seat(state: *mut State, seat: SeatId) {
    unsafe {
        let Some(runtime) = (*state).seat_runtime(seat) else {
            return;
        };
        let cursor_surface = runtime.cursor_surface;
        let pointer_x = runtime.pointer_x;
        let pointer_y = runtime.pointer_y;
        let cursor_hotspot = runtime.cursor_hotspot;
        let drag = runtime.drag;

        if !cursor_surface.is_null() {
            let rec = ffi::wl_resource_get_user_data(cursor_surface) as *mut SurfaceRec;
            if !rec.is_null() {
                (*rec).position = aegis_core::Point {
                    x: pointer_x.round() as i32 - cursor_hotspot.x + (*rec).attach_offset.x,
                    y: pointer_y.round() as i32 - cursor_hotspot.y + (*rec).attach_offset.y,
                };
            }
        }
        if let Some(drag) = drag
            && !drag.icon.is_null()
        {
            let rec = ffi::wl_resource_get_user_data(drag.icon) as *mut SurfaceRec;
            if !rec.is_null() {
                (*rec).position = aegis_core::Point {
                    x: pointer_x.round() as i32 + (*rec).attach_offset.x,
                    y: pointer_y.round() as i32 + (*rec).attach_offset.y,
                };
            }
        }
    }
}

unsafe fn send_xdg_surface_configure(rec: *mut SurfaceRec) -> Option<u32> {
    unsafe {
        if rec.is_null() || (*rec).xdg_surface.is_null() || (*rec).display.is_null() {
            return None;
        }
        let serial = ffi::wl_display_next_serial((*rec).display);
        (*rec).pending_xdg_configures.push(serial);
        ffi::wl_resource_post_event((*rec).xdg_surface, ffi::XDG_SURFACE_CONFIGURE, serial);
        Some(serial)
    }
}

unsafe fn surface_root_toplevel(mut surface: *mut SurfaceRec) -> *mut SurfaceRec {
    unsafe {
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
}

/// Keep every protocol whose target follows keyboard focus on the same
/// transition. Call this only after the corresponding `wl_keyboard` events and
/// authoritative `SeatRuntime::keyboard_focus` update.
pub(crate) unsafe fn keyboard_focus_dependencies_changed(
    state: *mut State,
    old_focus: *mut ffi::wl_resource,
    new_focus: *mut ffi::wl_resource,
) {
    unsafe {
        extensions::text_input_focus_changed(state, old_focus, new_focus);
        data_device_focus_changed(state, old_focus, new_focus);
        extensions::keyboard_shortcuts_focus_changed(state, new_focus);
    }
}

/// One dynamically advertised wl_output global. Boxes remain allocated after
/// hot-unplug until server teardown because clients may retain resources whose
/// user-data points here even after the registry global is removed.
pub(crate) struct OutputGlobal {
    state: *mut State,
    info: aegis_core::output::OutputInfo,
    /// `None` for a physical backend output; directed virtual outputs belong
    /// to exactly one Realm.
    realm: Option<RealmId>,
    global: *mut ffi::wl_global,
    active: bool,
}

#[derive(Clone, Copy)]
struct TopBorderClick {
    window_id: aegis_core::window::WindowId,
    released_at_ms: u64,
    position: (f32, f32),
}

#[derive(Clone, Copy)]
struct PendingTopBorderDoubleClick {
    window_id: aegis_core::window::WindowId,
    press_position: (f32, f32),
    start_position: aegis_core::Point,
}

/// Runtime protocol and input state for one logical `wl_seat`.
///
/// The authority model in `aegis-core` owns durable identities and policy.
/// This structure owns the libwayland resources and ephemeral protocol state
/// for exactly one seat. Keeping these records separate is what prevents
/// agent input, focus, grabs, clipboard, and cursor state from contending
/// with the physical user's state.
pub(crate) struct SeatRuntime {
    id: SeatId,
    realm: RealmId,
    principal: PrincipalId,
    capabilities: SeatCapabilities,
    seat_resources: Vec<*mut ffi::wl_resource>,
    pointer_resources: Vec<*mut ffi::wl_resource>,
    keyboard_resources: Vec<*mut ffi::wl_resource>,
    touch_resources: Vec<*mut ffi::wl_resource>,
    data_devices: Vec<*mut ffi::wl_resource>,
    data_offers: Vec<*mut ffi::wl_resource>,
    selection: Option<Selection>,
    drag: Option<DragState>,
    relative_pointers: Vec<*mut ffi::wl_resource>,
    pointer_constraints: Vec<*mut ffi::wl_resource>,
    pointer_gesture_swipes: Vec<*mut ffi::wl_resource>,
    pointer_gesture_pinches: Vec<*mut ffi::wl_resource>,
    pointer_gesture_holds: Vec<*mut ffi::wl_resource>,
    cursor_shape_devices: Vec<*mut ffi::wl_resource>,
    swipe_gesture_client: *mut ffi::wl_client,
    pinch_gesture_client: *mut ffi::wl_client,
    hold_gesture_client: *mut ffi::wl_client,
    keyboard_shortcut_inhibitors: Vec<*mut ffi::wl_resource>,
    pub(crate) tablet_seats: Vec<*mut ffi::wl_resource>,
    pub(crate) tablet_devices: Vec<*mut ffi::wl_resource>,
    pub(crate) tablet_tools: Vec<*mut ffi::wl_resource>,
    pub(crate) tablet_device_seen: bool,
    pub(crate) tablet_focus: *mut ffi::wl_resource,
    text_inputs: Vec<*mut ffi::wl_resource>,
    pending_text_input_states: Vec<aegis_core::input::TextInputState>,
    input_methods: Vec<*mut ffi::wl_resource>,
    virtual_keyboards: Vec<*mut ffi::wl_resource>,
    cursor_shape: u32,
    cursor_surface: *mut ffi::wl_resource,
    cursor_hotspot: aegis_core::Point,
    cursor_hidden: bool,
    last_pointer_enter_serial: u32,
    pointer_focus: *mut ffi::wl_resource,
    keyboard_focus: *mut ffi::wl_resource,
    pointer_x: f32,
    pointer_y: f32,
    raw_pointer_x: f32,
    raw_pointer_y: f32,
    last_button_serial: u32,
    implicit_grab_active: bool,
    depressed_mods: aegis_core::input::Mods,
    /// Presses consumed by compositor shortcuts. Their matching releases are
    /// consumed too so a newly focused client never receives a release for a
    /// key press it did not receive.
    suppressed_shortcut_keys: std::collections::HashSet<u32>,
    /// Keys currently down in the client-facing logical keyboard stream.
    ///
    /// This is deliberately distinct from xkb's physical state: input-method
    /// grabs and compositor shortcuts may consume physical keys. Focus enter
    /// snapshots and virtual-keyboard forwarding must describe only keys
    /// whose presses entered the client stream, so their later releases are
    /// never orphaned.
    client_pressed_keys: std::collections::BTreeSet<u32>,
    keyboard: Option<keyboard::Keyboard>,
    interactive: Option<aegis_core::window::Interactive>,
    compositor_pointer_grab: bool,
    last_top_border_click: Option<TopBorderClick>,
    pending_top_border_double_click: Option<PendingTopBorderDoubleClick>,
    /// Window-local automation pins hit-testing to one authorized root for
    /// the duration of an atomic input batch. This prevents overlapping
    /// surfaces in another workspace (or another virtual placement) from
    /// stealing agent pointer focus while coordinates are translated through
    /// the client's compositor-global surface position.
    synthetic_target: Option<aegis_core::window::WindowId>,
}

impl SeatRuntime {
    fn new(
        id: SeatId,
        realm: RealmId,
        principal: PrincipalId,
        capabilities: SeatCapabilities,
    ) -> Self {
        Self {
            id,
            realm,
            principal,
            capabilities,
            seat_resources: Vec::new(),
            pointer_resources: Vec::new(),
            keyboard_resources: Vec::new(),
            touch_resources: Vec::new(),
            data_devices: Vec::new(),
            data_offers: Vec::new(),
            selection: None,
            drag: None,
            relative_pointers: Vec::new(),
            pointer_constraints: Vec::new(),
            pointer_gesture_swipes: Vec::new(),
            pointer_gesture_pinches: Vec::new(),
            pointer_gesture_holds: Vec::new(),
            cursor_shape_devices: Vec::new(),
            swipe_gesture_client: std::ptr::null_mut(),
            pinch_gesture_client: std::ptr::null_mut(),
            hold_gesture_client: std::ptr::null_mut(),
            keyboard_shortcut_inhibitors: Vec::new(),
            tablet_seats: Vec::new(),
            tablet_devices: Vec::new(),
            tablet_tools: Vec::new(),
            tablet_device_seen: false,
            tablet_focus: std::ptr::null_mut(),
            text_inputs: Vec::new(),
            pending_text_input_states: Vec::new(),
            input_methods: Vec::new(),
            virtual_keyboards: Vec::new(),
            cursor_shape: 0,
            cursor_surface: std::ptr::null_mut(),
            cursor_hotspot: aegis_core::Point::default(),
            cursor_hidden: false,
            last_pointer_enter_serial: 0,
            pointer_focus: std::ptr::null_mut(),
            keyboard_focus: std::ptr::null_mut(),
            pointer_x: 0.0,
            pointer_y: 0.0,
            raw_pointer_x: 0.0,
            raw_pointer_y: 0.0,
            last_button_serial: 0,
            implicit_grab_active: false,
            depressed_mods: aegis_core::input::Mods::NONE,
            suppressed_shortcut_keys: std::collections::HashSet::new(),
            client_pressed_keys: std::collections::BTreeSet::new(),
            keyboard: None,
            interactive: None,
            compositor_pointer_grab: false,
            last_top_border_click: None,
            pending_top_border_double_click: None,
            synthetic_target: None,
        }
    }
}

/// Stable callback data for one dynamically advertised `wl_seat` global.
/// Boxes are retained after revocation because already-bound resources can
/// outlive the registry global.
struct SeatGlobal {
    state: *mut State,
    seat: SeatId,
    global: *mut ffi::wl_global,
    active: bool,
}

/// `wl_client` destroy listener. `listener` must remain the first field so the
/// libwayland callback can recover the allocation with a direct pointer cast.
#[repr(C)]
struct ClientDestroyRecord {
    listener: ffi::wl_listener,
    state: *mut State,
    client: *mut ffi::wl_client,
    id: aegis_core::realm::ClientId,
}

/// Server-wide state. Its address is handed to the C bind callbacks, so it is
/// boxed and never moved out.
pub(crate) struct State {
    pub(crate) display: *mut ffi::wl_display,
    authority: RealmModel,
    seats: std::collections::BTreeMap<SeatId, Box<SeatRuntime>>,
    /// Seat whose event/request path is currently executing. The server main
    /// loop is single-threaded; an RAII guard changes this only for the bounded
    /// duration of one routed input batch or seat-owned protocol callback.
    active_seat: SeatId,
    #[allow(clippy::vec_box)]
    seat_globals: Vec<Box<SeatGlobal>>,
    /// Registry globals that are physical-session authority and must never be
    /// advertised to clients launched through a Realm portal.
    realm_hidden_globals: std::collections::HashSet<usize>,
    /// Reverse lookup for every seat-owned protocol resource. Entries remain
    /// until the resource destroy callback, so stale protocol objects fail
    /// closed after a realm is revoked.
    seat_resource_owners: std::collections::HashMap<usize, SeatId>,
    /// Client-facing seat from which a routed child resource was created.
    /// Compatibility routing may change its runtime owner; native multi-seat
    /// rebinding restores the resource to this advertised origin.
    seat_resource_origins: std::collections::HashMap<usize, SeatId>,
    clients: std::collections::HashMap<usize, aegis_core::realm::ClientId>,
    /// Trusted launch-portal origin for clients accepted on a private Realm
    /// listener.
    /// Human/default-socket clients are omitted.
    client_initial_realms: std::collections::HashMap<aegis_core::realm::ClientId, RealmId>,
    client_bound_seats: std::collections::HashMap<usize, std::collections::BTreeSet<SeatId>>,
    realm_placements:
        std::collections::BTreeMap<(RealmId, aegis_core::window::WindowId), aegis_core::Rect>,
    /// Realm layouts are recomputed after the current Wayland dispatch batch.
    /// Deferring keeps role creation and surface commits atomic from the
    /// client's perspective while ensuring newly mapped Realm windows receive
    /// a virtual-output placement before observers are notified.
    pending_realm_layouts: std::collections::BTreeSet<RealmId>,
    /// Windows whose committed scene content changed during this dispatch
    /// batch. `Server::take_realm_damage` maps these durable ids into each
    /// observing Realm's virtual-output coordinate space after layouts settle.
    damaged_windows: std::collections::BTreeSet<aegis_core::window::WindowId>,
    /// Conservative damage queued for topology changes where an old placement
    /// may no longer be recoverable (remove, transfer, output reconfigure).
    pending_realm_damage: std::collections::BTreeMap<RealmId, Vec<aegis_core::Rect>>,
    /// Surface pointers in stacking order (bottom to top). Entries are nulled
    /// when a surface's destroy notify fires; focusing a toplevel moves its
    /// pointer to the end and updates affected live records' slot indices.
    /// Iterators must skip null entries.
    surfaces: Vec<*mut SurfaceRec>,
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
    /// Live ext-idle-notify and idle-inhibit protocol resources. Per-object
    /// timer/role state is owned by resource user data in `extensions.rs`.
    pub(crate) idle_notifications: Vec<*mut ffi::wl_resource>,
    pub(crate) idle_inhibitors: Vec<*mut ffi::wl_resource>,
    /// Surfaceless global idle inhibitor held by the portal backend over the
    /// scoped IPC (`SetIdleInhibit`, ADR-0053). Unlike the per-surface
    /// protocol inhibitors above it has no visibility rules: while set, idle
    /// notifications stay resumed. The compositor clears it when the owning
    /// IPC connection dies.
    pub(crate) portal_idle_inhibit: bool,
    /// Physical tablet tools seen so far, with their announced info. A tool
    /// is announced to every seat the first time it enters proximity.
    pub(crate) known_tools: Vec<(u64, aegis_core::input::TabletToolInfo)>,
    retired_buffer_releases: Vec<RetiredBufferRelease>,
    /// Bound `ext_foreign_toplevel_list_v1` resources. New toplevels, title
    /// changes, and removals are pushed to each.
    foreign_toplevel_lists: Vec<*mut ffi::wl_resource>,
    /// Per-toplevel foreign handle resources, keyed by window id. Lets the
    /// server push title/app_id/closed updates to the right handle.
    foreign_handles: std::collections::HashMap<u64, Vec<*mut ffi::wl_resource>>,
    activation_tokens: std::collections::HashMap<String, SeatId>,
    pending_activation: Option<(SeatId, *mut ffi::wl_resource)>,
    /// Keyboard-focus transitions requested from xdg-popup protocol
    /// callbacks. Callbacks cannot construct a second mutable `Server`, so
    /// dispatch applies the newest target for each seat after they return.
    pending_popup_focus: std::collections::BTreeMap<SeatId, *mut ffi::wl_resource>,
    /// Active ext-session-lock object and fail-closed visibility state.
    pub(crate) session_lock: *mut c_void,
    pub(crate) session_locked: bool,
    pub(crate) lock_focus_dirty: bool,
    pub(crate) pending_lock_focus: *mut ffi::wl_resource,
    pub(crate) pre_lock_keyboard_focus: *mut ffi::wl_resource,
    pub(crate) session_lock_requested_at: Option<std::time::Instant>,
    pub(crate) lock_frame_pending: bool,
    /// Pending console VT switch requested by a Ctrl+Alt+Fn key press
    /// (XF86Switch_VT_N). The kernel never sees these keys once libinput owns
    /// evdev, so the compositor performs the session switch through libseat.
    /// Drained by the main loop via [`Server::take_vt_switch`].
    pending_vt_switch: Option<i32>,
    /// Parameters for the tiling policy (gaps, master ratio). Per-workspace
    /// tiling on/off lives on each workspace in the model (ADR-0024).
    layout_params: aegis_core::layout::LayoutParams,
    /// Accessibility reduced-motion policy (ADR-0029): when true, window
    /// transitions resolve in one frame and none are recorded.
    reduced_motion: bool,
    /// Effective decoration ownership announced to xdg-decoration clients.
    /// Borderless is compositor-owned: clients omit CSDs while window
    /// controls remain available through gestures and shell surfaces.
    decoration_policy: aegis_core::window::DecorationPolicy,
    /// Config-driven window rules (ADR-0026). Evaluated on first map; the
    /// first match prescribes a workspace move and/or a forced layout role.
    window_rules: Vec<aegis_core::window_rule::WindowRule>,
    /// The focused output's geometry (ADR-0028): the tiling work-area is its
    /// logical rect. Updated by the backend on resize; defaults to identity.
    pub(crate) output_geometry: aegis_core::output::OutputGeometry,
    /// Backend-reported connector geometry in global logical coordinates.
    /// The first entry is the primary/focused output exposed through the
    /// legacy single wl_output global until per-global resources are split.
    output_infos: Vec<aegis_core::output::OutputInfo>,
    /// Bumped on every `output_infos` mutation so the frame loop can skip
    /// re-cloning the list while it is unchanged.
    outputs_revision: u64,
    /// Per-connector output policies from `[[output]]` config entries
    /// (ADR-0028). Applied to every backend-reported output set in
    /// `set_outputs`.
    output_policies: std::collections::HashMap<String, aegis_core::output::OutputPolicy>,
    /// Dynamic per-output workspaces (ADR-0025). Toplevels are placed on the
    /// current workspace at first map; rendering and input see only the
    /// visible set (`visible_toplevels`).
    workspaces: aegis_core::workspace::WorkspaceModel,
    /// Focused output for new surfaces and workspace commands.
    output: aegis_core::workspace::OutputId,
    /// Monotonic counter for durable window identifiers (ADR-0032). Starts
    /// at 1 so `WindowId(0)` remains reserved for the `Window::default()`
    /// that non-toplevel surfaces carry.
    /// Cached chrome-aware work area bounds for maximized windows.
    pub(crate) last_work_area: aegis_core::Rect,
    pub(crate) epoch: std::time::Instant,
    /// Last remembered floating window position and size per application ID.
    pub(crate) last_app_geometries: std::collections::HashMap<String, aegis_core::Rect>,
    /// Persistent window state store across restarts.
    pub(crate) window_state_store: aegis_core::window_state_store::WindowStateStore,
    /// Path to persistent window state file.
    pub(crate) window_state_path: std::path::PathBuf,
    /// Global toggle for remembering window positions across restarts.
    pub(crate) remember_window_positions: bool,
    next_window_id: u64,
}

impl State {
    pub(crate) fn now_ms(&self) -> u64 {
        self.epoch.elapsed().as_millis() as u64
    }

    pub(crate) fn persist_app_geometry(
        &mut self,
        app_id: &str,
        rect: aegis_core::Rect,
        workspace: Option<u32>,
        layout_role: Option<aegis_core::layout::LayoutRole>,
    ) {
        if app_id.is_empty() {
            return;
        }
        self.last_app_geometries.insert(app_id.to_owned(), rect);
        self.window_state_store.update(
            app_id.to_owned(),
            aegis_core::window_state_store::SavedWindowState {
                position: Some(rect.origin),
                size: Some(rect.size),
                workspace,
                layout_role,
                maximized: None,
            },
        );
        let _ = self
            .window_state_store
            .save_to_path(&self.window_state_path);
    }

    pub(crate) fn workspace_number_for_window(
        &self,
        window: aegis_core::window::WindowId,
    ) -> Option<u32> {
        let workspace = self.workspaces.workspace_of(window)?;
        let output = self.workspaces.workspace(workspace)?.output;
        let index = self
            .workspaces
            .output(output)?
            .workspaces
            .iter()
            .position(|candidate| *candidate == workspace)?;
        u32::try_from(index).ok()?.checked_add(1)
    }

    fn new(display: *mut ffi::wl_display) -> State {
        let mut workspaces = aegis_core::workspace::WorkspaceModel::new();
        let output = workspaces.add_output("nested");
        let authority = RealmModel::new();
        let human_seat = Box::new(SeatRuntime::new(
            HUMAN_SEAT,
            HUMAN_REALM,
            HUMAN_PRINCIPAL,
            SeatCapabilities::ALL,
        ));
        let window_state_path = aegis_core::window_state_store::WindowStateStore::default_path();
        let window_state_store =
            aegis_core::window_state_store::WindowStateStore::load_from_path(&window_state_path);
        let mut last_app_geometries = std::collections::HashMap::new();
        for (app_id, entry) in &window_state_store.entries {
            if let (Some(pos), Some(sz)) = (entry.position, entry.size) {
                last_app_geometries.insert(
                    app_id.clone(),
                    aegis_core::Rect {
                        origin: pos,
                        size: sz,
                    },
                );
            }
        }
        State {
            display,
            authority,
            seats: std::collections::BTreeMap::from([(HUMAN_SEAT, human_seat)]),
            active_seat: HUMAN_SEAT,
            seat_globals: Vec::new(),
            realm_hidden_globals: std::collections::HashSet::new(),
            seat_resource_owners: std::collections::HashMap::new(),
            seat_resource_origins: std::collections::HashMap::new(),
            clients: std::collections::HashMap::new(),
            client_initial_realms: std::collections::HashMap::new(),
            client_bound_seats: std::collections::HashMap::new(),
            realm_placements: std::collections::BTreeMap::new(),
            pending_realm_layouts: std::collections::BTreeSet::new(),
            damaged_windows: std::collections::BTreeSet::new(),
            pending_realm_damage: std::collections::BTreeMap::new(),
            surfaces: Vec::new(),
            output_resources: Vec::new(),
            output_globals: Vec::new(),
            xdg_output_resources: Vec::new(),
            xdg_output_links: std::collections::HashMap::new(),
            idle_notifications: Vec::new(),
            idle_inhibitors: Vec::new(),
            portal_idle_inhibit: false,
            known_tools: Vec::new(),
            retired_buffer_releases: Vec::new(),
            foreign_toplevel_lists: Vec::new(),
            foreign_handles: std::collections::HashMap::new(),
            activation_tokens: std::collections::HashMap::new(),
            pending_activation: None,
            pending_popup_focus: std::collections::BTreeMap::new(),
            session_lock: std::ptr::null_mut(),
            session_locked: false,
            lock_focus_dirty: false,
            pending_lock_focus: std::ptr::null_mut(),
            pre_lock_keyboard_focus: std::ptr::null_mut(),
            session_lock_requested_at: None,
            lock_frame_pending: false,
            pending_vt_switch: None,
            workspaces,
            output,
            layout_params: aegis_core::layout::LayoutParams::default(),
            reduced_motion: false,
            decoration_policy: aegis_core::window::DecorationPolicy::default(),
            window_rules: Vec::new(),
            output_geometry: aegis_core::output::OutputGeometry::default(),
            output_infos: vec![aegis_core::output::OutputInfo {
                connector: "nested".to_owned(),
                geometry: aegis_core::output::OutputGeometry::default(),
                available_modes: Vec::new(),
            }],
            outputs_revision: 0,
            output_policies: std::collections::HashMap::new(),
            last_work_area: aegis_core::Rect::default(),
            epoch: std::time::Instant::now(),
            last_app_geometries,
            window_state_store,
            window_state_path,
            remember_window_positions: true,
            next_window_id: 1,
        }
    }

    fn seat_runtime(&self, seat: SeatId) -> Option<&SeatRuntime> {
        self.seats.get(&seat).map(Box::as_ref)
    }

    fn seat_runtime_mut(&mut self, seat: SeatId) -> Option<&mut SeatRuntime> {
        self.seats.get_mut(&seat).map(Box::as_mut)
    }

    fn seat_for_resource(&self, resource: *mut ffi::wl_resource) -> Option<SeatId> {
        self.seat_resource_owners.get(&(resource as usize)).copied()
    }

    fn seat_origin_for_resource(&self, resource: *mut ffi::wl_resource) -> Option<SeatId> {
        self.seat_resource_origins
            .get(&(resource as usize))
            .copied()
    }

    fn track_seat_resource(&mut self, resource: *mut ffi::wl_resource, seat: SeatId) {
        self.seat_resource_owners.insert(resource as usize, seat);
        self.seat_resource_origins
            .entry(resource as usize)
            .or_insert(seat);
    }

    fn track_routed_seat_resource(
        &mut self,
        resource: *mut ffi::wl_resource,
        advertised: SeatId,
        routed: SeatId,
    ) {
        self.seat_resource_owners.insert(resource as usize, routed);
        self.seat_resource_origins
            .insert(resource as usize, advertised);
    }

    fn untrack_seat_resource(&mut self, resource: *mut ffi::wl_resource) -> Option<SeatId> {
        self.seat_resource_origins.remove(&(resource as usize));
        self.seat_resource_owners.remove(&(resource as usize))
    }

    unsafe fn ensure_client(&mut self, client: *mut ffi::wl_client) -> aegis_core::realm::ClientId {
        unsafe { self.ensure_client_with_realm(client, None) }
    }

    unsafe fn ensure_client_with_realm(
        &mut self,
        client: *mut ffi::wl_client,
        realm: Option<RealmId>,
    ) -> aegis_core::realm::ClientId {
        unsafe {
            if let Some(id) = self.clients.get(&(client as usize)).copied() {
                return id;
            }
            let security_context = realm.map(|realm| format!("aegis.realm.{}", realm.0));
            let id = self.authority.register_client(security_context);
            self.clients.insert(client as usize, id);
            if let Some(realm) = realm {
                self.client_initial_realms.insert(id, realm);
            }
            let record = Box::new(ClientDestroyRecord {
                listener: ffi::wl_listener {
                    link: ffi::wl_list {
                        prev: std::ptr::null_mut(),
                        next: std::ptr::null_mut(),
                    },
                    notify: Some(client_destroyed),
                },
                state: self as *mut State,
                client,
                id,
            });
            let record = Box::into_raw(record);
            ffi::wl_client_add_destroy_listener(client, &mut (*record).listener);
            id
        }
    }

    fn client_view_realm(&self, client: *mut ffi::wl_client) -> RealmId {
        self.clients
            .get(&(client as usize))
            .and_then(|client| self.client_initial_realms.get(client))
            .copied()
            .unwrap_or(HUMAN_REALM)
    }

    fn client_observes_window(
        &self,
        client: *mut ffi::wl_client,
        window: aegis_core::window::WindowId,
    ) -> bool {
        self.authority
            .realm_observes_window(self.client_view_realm(client), window)
    }

    fn register_window(
        &mut self,
        client: aegis_core::realm::ClientId,
        window: aegis_core::window::WindowId,
    ) -> Result<(), RealmError> {
        let existing = self
            .authority
            .interaction_groups_for_client(client)
            .next()
            .map(|group| group.id);
        if let Some(group) = existing {
            self.authority.add_window_to_group(group, window)?;
        } else {
            let initial_realm = self
                .client_initial_realms
                .get(&client)
                .copied()
                .unwrap_or(HUMAN_REALM);
            self.authority
                .create_interaction_group(client, &[window], initial_realm)?;
        }
        self.queue_realm_layouts_for_window(window);
        Ok(())
    }

    fn unregister_window(&mut self, window: aegis_core::window::WindowId) {
        if self
            .authority
            .interaction_group_for_window(window)
            .is_some()
        {
            let realms = self.realms_for_window(window);
            for realm in realms {
                self.queue_full_realm_damage(realm);
                self.pending_realm_layouts.insert(realm);
            }
            self.damaged_windows.remove(&window);
            let _ = self.authority.remove_window(window);
            self.realm_placements
                .retain(|(_, placement_window), _| *placement_window != window);
        }
    }

    fn realms_for_window(&self, window: aegis_core::window::WindowId) -> Vec<RealmId> {
        let Some(group) = self.authority.interaction_group_for_window(window) else {
            return Vec::new();
        };
        let mut realms = Vec::with_capacity(group.observer_realms.len() + 1);
        realms.push(group.control_realm);
        realms.extend(group.observer_realms.iter().copied());
        realms.sort_unstable();
        realms.dedup();
        realms
    }

    fn queue_realm_layouts_for_window(&mut self, window: aegis_core::window::WindowId) {
        for realm in self.realms_for_window(window) {
            if realm != HUMAN_REALM {
                self.pending_realm_layouts.insert(realm);
            }
        }
    }

    fn queue_full_realm_damage(&mut self, realm: RealmId) {
        let Some(record) = self.authority.realm(realm) else {
            return;
        };
        let PresentationTarget::Virtual { output } = record.presentation else {
            return;
        };
        self.pending_realm_damage
            .entry(realm)
            .or_default()
            .push(aegis_core::Rect::new(
                0,
                0,
                output.width as i32,
                output.height as i32,
            ));
    }

    unsafe fn note_client_used_seat(&mut self, client: *mut ffi::wl_client, seat: SeatId) {
        unsafe {
            let id = self.ensure_client(client);
            let bound = self.client_bound_seats.entry(client as usize).or_default();
            if self.client_initial_realms.contains_key(&id) {
                bound.insert(seat);
                return;
            }
            let was_multi_seat = bound.len() > 1;
            bound.insert(seat);
            if !was_multi_seat && bound.len() > 1 {
                let _ = self
                    .authority
                    .set_client_multi_seat(id, aegis_core::realm::MultiSeatSupport::Supported);
                self.restore_native_multiseat_resources(client);
            }
        }
    }

    fn client_routed_seat(&self, client: *mut ffi::wl_client, advertised: SeatId) -> SeatId {
        let Some(client_id) = self.clients.get(&(client as usize)).copied() else {
            return advertised;
        };
        if self.authority.client(client_id).is_some_and(|client| {
            client.multi_seat == aegis_core::realm::MultiSeatSupport::Supported
        }) {
            return advertised;
        }
        let realm = self
            .authority
            .interaction_groups_for_client(client_id)
            .next()
            .map(|group| group.control_realm)
            .or_else(|| self.client_initial_realms.get(&client_id).copied());
        let Some(realm) = realm else {
            return advertised;
        };
        self.authority
            .snapshot()
            .seats
            .into_iter()
            .find(|seat| seat.realm == realm && seat.enabled)
            .map(|seat| seat.id)
            .unwrap_or(advertised)
    }

    unsafe fn migrate_compatibility_resources(
        &mut self,
        client_id: aegis_core::realm::ClientId,
        target: SeatId,
    ) {
        unsafe {
            if self.authority.client(client_id).is_some_and(|client| {
                client.multi_seat == aegis_core::realm::MultiSeatSupport::Supported
            }) {
                return;
            }
            let Some(client) = self
                .clients
                .iter()
                .find_map(|(raw, id)| (*id == client_id).then_some(*raw as *mut ffi::wl_client))
            else {
                return;
            };
            if !self.seats.contains_key(&target) {
                return;
            }

            // Revoke offers that originated in the source realm before moving the
            // destination devices. Already-issued offers are capabilities and
            // must not remain usable across an authority boundary.
            for (seat, runtime) in &mut self.seats {
                if *seat == target {
                    continue;
                }
                for device in runtime.data_devices.iter().copied().filter(|resource| {
                    !resource.is_null() && ffi::wl_resource_get_client(*resource) == client
                }) {
                    ffi::wl_resource_post_event(
                        device,
                        ffi::WL_DATA_DEVICE_SELECTION,
                        std::ptr::null_mut::<ffi::wl_resource>(),
                    );
                }
                for offer in runtime.data_offers.iter().copied().filter(|resource| {
                    !resource.is_null() && ffi::wl_resource_get_client(*resource) == client
                }) {
                    let record = ffi::wl_resource_get_user_data(offer) as *mut DataOfferRec;
                    if !record.is_null() {
                        (*record).source = std::ptr::null_mut();
                    }
                }
            }

            macro_rules! migrate {
                ($field:ident) => {{
                    let mut moved = Vec::new();
                    for (seat, runtime) in &mut self.seats {
                        if *seat == target {
                            continue;
                        }
                        runtime.$field.retain(|resource| {
                            let belongs = !resource.is_null()
                                && ffi::wl_resource_get_client(*resource) == client;
                            if belongs {
                                moved.push(*resource);
                            }
                            !belongs
                        });
                    }
                    for resource in &moved {
                        self.seat_resource_owners.insert(*resource as usize, target);
                    }
                    self.seats
                        .get_mut(&target)
                        .expect("validated target seat disappeared")
                        .$field
                        .extend(moved);
                }};
            }

            migrate!(pointer_resources);
            migrate!(keyboard_resources);
            migrate!(touch_resources);
            migrate!(data_devices);
            migrate!(relative_pointers);
            migrate!(pointer_constraints);
            migrate!(pointer_gesture_swipes);
            migrate!(pointer_gesture_pinches);
            migrate!(pointer_gesture_holds);
            migrate!(cursor_shape_devices);
            migrate!(keyboard_shortcut_inhibitors);
            migrate!(tablet_seats);
            migrate!(tablet_devices);
            migrate!(tablet_tools);
            migrate!(text_inputs);
        }
    }

    unsafe fn restore_native_multiseat_resources(&mut self, client: *mut ffi::wl_client) {
        unsafe {
            let live_seats = self
                .seats
                .keys()
                .copied()
                .collect::<std::collections::BTreeSet<_>>();
            macro_rules! restore {
                ($field:ident) => {{
                    let mut moved = Vec::<(SeatId, *mut ffi::wl_resource)>::new();
                    for (current, runtime) in &mut self.seats {
                        runtime.$field.retain(|resource| {
                            if resource.is_null()
                                || ffi::wl_resource_get_client(*resource) != client
                            {
                                return true;
                            }
                            let origin = self
                                .seat_resource_origins
                                .get(&(*resource as usize))
                                .copied()
                                .unwrap_or(*current);
                            if origin != *current && live_seats.contains(&origin) {
                                moved.push((origin, *resource));
                                false
                            } else {
                                true
                            }
                        });
                    }
                    for (origin, resource) in moved {
                        self.seat_resource_owners.insert(resource as usize, origin);
                        self.seats
                            .get_mut(&origin)
                            .expect("validated original seat disappeared")
                            .$field
                            .push(resource);
                    }
                }};
            }

            restore!(pointer_resources);
            restore!(keyboard_resources);
            restore!(touch_resources);
            restore!(data_devices);
            restore!(relative_pointers);
            restore!(pointer_constraints);
            restore!(pointer_gesture_swipes);
            restore!(pointer_gesture_pinches);
            restore!(pointer_gesture_holds);
            restore!(cursor_shape_devices);
            restore!(keyboard_shortcut_inhibitors);
            restore!(tablet_seats);
            restore!(tablet_devices);
            restore!(tablet_tools);
            restore!(text_inputs);
        }
    }

    /// Allocate a fresh, never-reused `WindowId` (ADR-0032). Called on the
    /// main loop when a toplevel role is acquired.
    fn alloc_window_id(&mut self) -> aegis_core::window::WindowId {
        let id = self.next_window_id;
        self.next_window_id += 1;
        aegis_core::window::WindowId(id)
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

/// Existing compositor paths are the physical human-seat path. Dereferencing
/// `State` to that runtime keeps those paths source-compatible while the
/// generic seat-aware entry points use `seat_runtime(_mut)` explicitly.
impl Deref for State {
    type Target = SeatRuntime;

    fn deref(&self) -> &Self::Target {
        self.seat_runtime(self.active_seat)
            .expect("active seat runtime is missing")
    }
}

impl DerefMut for State {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.seat_runtime_mut(self.active_seat)
            .expect("active seat runtime is missing")
    }
}

unsafe extern "C" fn client_destroyed(listener: *mut ffi::wl_listener, _data: *mut c_void) {
    unsafe {
        let record = listener as *mut ClientDestroyRecord;
        if record.is_null() {
            return;
        }
        let record = Box::from_raw(record);
        if !record.state.is_null() {
            let state = &mut *record.state;
            if state.clients.get(&(record.client as usize)) == Some(&record.id) {
                state.clients.remove(&(record.client as usize));
            }
            state.client_bound_seats.remove(&(record.client as usize));
            state.client_initial_realms.remove(&record.id);
            let _ = state.authority.disconnect_client(record.id);
        }
    }
}

/// One-shot Realm launch connections are a trusted client-identity portal.
/// Such clients see only their own `wl_seat` global, so a sandboxed app cannot
/// bind the physical user's seat even if it deliberately enumerates every
/// registry object. Ordinary desktop clients remain unrestricted for
/// compatibility with native multi-seat software.
unsafe extern "C" fn realm_global_filter(
    client: *const ffi::wl_client,
    global: *const ffi::wl_global,
    data: *mut c_void,
) -> bool {
    unsafe {
        let state = data as *mut State;
        if state.is_null() || client.is_null() || global.is_null() {
            return true;
        }
        let realm = (*state)
            .clients
            .get(&(client as usize))
            .and_then(|client_id| (*state).client_initial_realms.get(client_id))
            .copied();
        if realm.is_some() && (*state).realm_hidden_globals.contains(&(global as usize)) {
            return false;
        }
        if let Some(output) = (*state)
            .output_globals
            .iter()
            .find(|output| std::ptr::eq(output.global as *const ffi::wl_global, global))
        {
            return match realm {
                Some(realm) => output.realm == Some(realm),
                // Ordinary host clients need virtual outputs announced just like
                // hot-plugged monitors: an already-running surface can transfer
                // there and must receive correct enter/leave and scale state.
                // Portal clients remain confined to their own directed output.
                None => true,
            };
        }
        if let Some(seat) = (*state)
            .seat_globals
            .iter()
            .find(|seat| std::ptr::eq(seat.global as *const ffi::wl_global, global))
        {
            return realm.is_none_or(|realm| {
                (*state)
                    .authority
                    .seat(seat.seat)
                    .is_some_and(|seat| seat.realm == realm)
            });
        }
        true
    }
}

struct ActiveSeatGuard {
    state: *mut State,
    previous: SeatId,
}

impl ActiveSeatGuard {
    fn enter(state: &mut State, seat: SeatId) -> Option<Self> {
        if !state
            .authority
            .seat(seat)
            .is_some_and(|model| model.enabled)
            || !state.seats.contains_key(&seat)
        {
            return None;
        }
        let previous = state.active_seat;
        state.active_seat = seat;
        Some(Self {
            state: state as *mut State,
            previous,
        })
    }

    fn enter_existing(state: &mut State, seat: SeatId) -> Option<Self> {
        if !state.seats.contains_key(&seat) {
            return None;
        }
        let previous = state.active_seat;
        state.active_seat = seat;
        Some(Self {
            state: state as *mut State,
            previous,
        })
    }

    unsafe fn for_resource(
        state: *mut State,
        resource: *mut ffi::wl_resource,
        require_enabled: bool,
    ) -> Option<Self> {
        unsafe {
            if state.is_null() {
                return None;
            }
            let seat = (*state).seat_for_resource(resource)?;
            if require_enabled {
                Self::enter(&mut *state, seat)
            } else {
                Self::enter_existing(&mut *state, seat)
            }
        }
    }

    unsafe fn for_client_seat_resource(
        state: *mut State,
        client: *mut ffi::wl_client,
        seat_resource: *mut ffi::wl_resource,
        require_enabled: bool,
    ) -> Option<Self> {
        unsafe {
            if state.is_null() {
                return None;
            }
            let advertised = (*state).seat_for_resource(seat_resource)?;
            let routed = (*state).client_routed_seat(client, advertised);
            if require_enabled {
                Self::enter(&mut *state, routed)
            } else {
                Self::enter_existing(&mut *state, routed)
            }
        }
    }
}

impl Drop for ActiveSeatGuard {
    fn drop(&mut self) {
        unsafe {
            (*self.state).active_seat = self.previous;
        }
    }
}

unsafe fn create_seat_global(state: &mut State, seat: SeatId) -> *mut ffi::wl_global {
    unsafe {
        let mut record = Box::new(SeatGlobal {
            state: state as *mut State,
            seat,
            global: std::ptr::null_mut(),
            active: true,
        });
        let data = record.as_mut() as *mut SeatGlobal as *mut c_void;
        record.global =
            ffi::wl_global_create(state.display, &ffi::wl_seat_interface, 9, data, seat_bind);
        if record.global.is_null() {
            return std::ptr::null_mut();
        }
        let global = record.global;
        state.seat_globals.push(record);
        global
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum PointerAxisWireEvent {
    Source(u32),
    Discrete { axis: u32, value: i32 },
    Value120 { axis: u32, value: i32 },
    RelativeDirection { axis: u32, direction: u32 },
    Axis { time: u32, axis: u32, value: f32 },
    Stop { time: u32, axis: u32 },
    Frame,
}

fn pointer_axis_wire_events(
    version: i32,
    frame: aegis_core::input::PointerAxisFrame,
) -> Vec<PointerAxisWireEvent> {
    use aegis_core::input::{
        PointerAxisRelativeDirection as Direction, PointerAxisSource as Source,
    };

    let mut events = Vec::with_capacity(10);
    if version >= 5 {
        let source = frame.source.map(|source| match source {
            Source::Wheel => ffi::WL_POINTER_AXIS_SOURCE_WHEEL,
            Source::Finger => ffi::WL_POINTER_AXIS_SOURCE_FINGER,
            Source::Continuous => ffi::WL_POINTER_AXIS_SOURCE_CONTINUOUS,
            Source::WheelTilt => ffi::WL_POINTER_AXIS_SOURCE_WHEEL_TILT,
        });
        if let Some(source) = source {
            events.push(PointerAxisWireEvent::Source(source));
        }
    }

    for (axis_id, axis) in [
        (ffi::WL_POINTER_AXIS_HORIZONTAL_SCROLL, frame.horizontal),
        (ffi::WL_POINTER_AXIS_VERTICAL_SCROLL, frame.vertical),
    ] {
        let value = axis
            .value
            .filter(|value| *value != 0.0)
            .or_else(|| {
                axis.value120
                    .filter(|value| *value != 0)
                    .map(|value| value as f32 / 12.0)
            })
            .or_else(|| {
                axis.discrete
                    .filter(|value| *value != 0)
                    .map(|value| value as f32 * 10.0)
            });

        if let Some(value) = value {
            if version >= 8 {
                let value120 = axis
                    .value120
                    .or_else(|| axis.discrete.map(|value| value.saturating_mul(120)))
                    .filter(|value| *value != 0);
                if let Some(value120) = value120 {
                    events.push(PointerAxisWireEvent::Value120 {
                        axis: axis_id,
                        value: value120,
                    });
                }
            } else if version >= 5 {
                let discrete = axis
                    .discrete
                    .or_else(|| {
                        axis.value120
                            .filter(|value| *value % 120 == 0)
                            .map(|value| value / 120)
                    })
                    .filter(|value| *value != 0);
                if let Some(discrete) = discrete {
                    // The protocol requires discrete/value120 metadata before
                    // its corresponding continuous axis event.
                    events.push(PointerAxisWireEvent::Discrete {
                        axis: axis_id,
                        value: discrete,
                    });
                }
            }
            if version >= 9 {
                let direction = axis.relative_direction.map(|direction| match direction {
                    Direction::Identical => ffi::WL_POINTER_AXIS_RELATIVE_DIRECTION_IDENTICAL,
                    Direction::Inverted => ffi::WL_POINTER_AXIS_RELATIVE_DIRECTION_INVERTED,
                });
                if let Some(direction) = direction {
                    events.push(PointerAxisWireEvent::RelativeDirection {
                        axis: axis_id,
                        direction,
                    });
                }
            }
            events.push(PointerAxisWireEvent::Axis {
                time: frame.time,
                axis: axis_id,
                value,
            });
        }

        if version >= 5 && axis.stop {
            events.push(PointerAxisWireEvent::Stop {
                time: frame.time,
                axis: axis_id,
            });
        }
    }

    if version >= 5
        && events
            .iter()
            .any(|event| !matches!(event, PointerAxisWireEvent::Source(_)))
    {
        events.push(PointerAxisWireEvent::Frame);
    }
    events
}

unsafe fn post_pointer_axis_wire_event(
    pointer: *mut ffi::wl_resource,
    event: PointerAxisWireEvent,
) {
    unsafe {
        match event {
            PointerAxisWireEvent::Source(source) => {
                ffi::wl_resource_post_event(pointer, ffi::WL_POINTER_AXIS_SOURCE, source);
            }
            PointerAxisWireEvent::Discrete { axis, value } => {
                ffi::wl_resource_post_event(pointer, ffi::WL_POINTER_AXIS_DISCRETE, axis, value);
            }
            PointerAxisWireEvent::Value120 { axis, value } => {
                ffi::wl_resource_post_event(pointer, ffi::WL_POINTER_AXIS_VALUE120, axis, value);
            }
            PointerAxisWireEvent::RelativeDirection { axis, direction } => {
                ffi::wl_resource_post_event(
                    pointer,
                    ffi::WL_POINTER_AXIS_RELATIVE_DIRECTION,
                    axis,
                    direction,
                );
            }
            PointerAxisWireEvent::Axis { time, axis, value } => {
                ffi::wl_resource_post_event(
                    pointer,
                    ffi::WL_POINTER_AXIS,
                    time,
                    axis,
                    ffi::wl_fixed_from_f32(value),
                );
            }
            PointerAxisWireEvent::Stop { time, axis } => {
                ffi::wl_resource_post_event(pointer, ffi::WL_POINTER_AXIS_STOP, time, axis);
            }
            PointerAxisWireEvent::Frame => {
                ffi::wl_resource_post_event(pointer, ffi::WL_POINTER_FRAME);
            }
        }
    }
}

/// The Wayland server: socket, globals, and object lifecycle.
pub struct Server {
    state: Box<State>,
    socket: String,
    realm_portals: Vec<RealmPortal>,
    /// Monotonic epoch for pointer buttons, touch, relative motion, and
    /// synthetic events that do not carry a backend timestamp. Axis frames
    /// retain their DRM/libinput or nested-host timestamps end to end.
    epoch: std::time::Instant,
}

/// One physical keyboard edge after it has advanced the seat's XKB state.
///
/// The runtime prepares every hardware key in arrival order before deciding
/// whether compositor chrome or the focused client owns that sequence. The
/// opaque snapshot prevents either route from advancing XKB a second time or
/// reordering modifier transitions when one backend batch crosses a routing
/// boundary.
#[derive(Debug, Clone, Copy)]
pub struct PreparedKeyboardEvent {
    evdev_code: u32,
    state: aegis_core::input::ButtonState,
    outcome: keyboard::KeyOutcome,
    consumed_by_vt_switch: bool,
}

impl PreparedKeyboardEvent {
    /// Character view used by compositor chrome for a prepared press.
    ///
    /// VT-switch keysyms are compositor control events rather than text and
    /// therefore intentionally have no character view.
    pub fn key_char(self) -> Option<aegis_core::input::KeyChar> {
        (!self.consumed_by_vt_switch).then_some(aegis_core::input::KeyChar {
            keysym: self.outcome.keysym,
            ch: self.outcome.utf8,
            mods: aegis_core::input::Mods(self.outcome.depressed),
        })
    }
}

/// Prepared capability endpoint for one sandboxed application instance.
///
/// Before activation the socket has a short-lived randomized host pathname so
/// bubblewrap can bind-mount the socket inode. The launcher must unlink that
/// pathname and close all connections queued before the sandbox gate opens.
/// Once activated, only the bind mount inside that sandbox can reach it.
pub struct RealmPortal {
    realm: RealmId,
    path: PathBuf,
    listener: UnixListener,
}

impl RealmPortal {
    pub fn realm(&self) -> RealmId {
        self.realm
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn try_clone_listener(&self) -> std::io::Result<UnixListener> {
        self.listener.try_clone()
    }
}

impl Drop for RealmPortal {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
        if let Some(parent) = self.path.parent() {
            let _ = std::fs::remove_dir(parent);
        }
    }
}

fn random_portal_token() -> std::io::Result<String> {
    let mut bytes = [0u8; 16];
    let mut random = std::fs::File::open("/dev/urandom")?;
    std::io::Read::read_exact(&mut random, &mut bytes)?;
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        write!(&mut token, "{byte:02x}").expect("writing to a String cannot fail");
    }
    Ok(token)
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
    #[error("wl_global_create failed for the bootstrap seat")]
    SeatGlobal,
}

/// Errors changing live realm/seat topology after server startup.
#[derive(Debug, thiserror::Error)]
pub enum RealmRuntimeError {
    #[error(transparent)]
    Model(#[from] RealmError),
    #[error("failed to initialize independent XKB state: {0}")]
    Keyboard(String),
    #[error("wl_global_create failed for the agent seat")]
    SeatGlobal,
    #[error("wl_global_create failed for the Realm virtual output")]
    OutputGlobal,
    #[error("seat {} is unknown, paused, or revoked", .0.0)]
    SeatUnavailable(SeatId),
    #[error("realm {} has no logical seat", .0.0)]
    RealmHasNoSeat(RealmId),
    #[error("failed to create Realm launch portal: {0}")]
    Portal(String),
}
