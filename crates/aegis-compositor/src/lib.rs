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
use std::sync::atomic::{AtomicU64, Ordering};

use aegis_model::interaction_domain::{
    AuthorityTransfer, HUMAN_INTERACTION_DOMAIN, HUMAN_PRINCIPAL, HUMAN_SEAT,
    InteractionDomainBundle, InteractionDomainError, InteractionDomainId, InteractionDomainModel,
    InteractionDomainMutation, InteractionDomainMutationResult, InteractionDomainRevocation,
    InteractionDomainSnapshot, InteractionDomainTransactionReceipt, InteractionPrincipalId,
    PresentationTarget, SeatCapabilities, SeatId, TransferOptions, VirtualOutput,
};
use aegis_model::layout::Layout;
use aegis_model::{SurfaceDmabuf, SurfacePixels};

/// Security-visible phase of the ext-session-lock protocol.
///
/// Request acceptance hides normal content immediately. `Securing` persists
/// until a newly secure frame reaches every active output; only then may the
/// compositor emit `locked` and enter `Locked`. Keeping the presentation
/// receipt in the phase prevents impossible combinations such as "confirmed
/// but still waiting for the first frame".
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SessionLockPhase {
    Unlocked,
    Securing {
        requested_at: std::time::Instant,
        frame_pending: bool,
    },
    Locked {
        frame_pending: bool,
    },
}

impl SessionLockPhase {
    fn begin(&mut self, now: std::time::Instant) {
        debug_assert_eq!(*self, Self::Unlocked);
        *self = Self::Securing {
            requested_at: now,
            frame_pending: false,
        };
    }

    pub(crate) fn is_active(self) -> bool {
        !matches!(self, Self::Unlocked)
    }

    pub(crate) fn is_confirmed(self) -> bool {
        matches!(self, Self::Locked { .. })
    }

    pub(crate) fn frame_pending(self) -> bool {
        matches!(
            self,
            Self::Securing {
                frame_pending: true,
                ..
            } | Self::Locked {
                frame_pending: true
            }
        )
    }

    pub(crate) fn request_secure_frame(&mut self) {
        match self {
            Self::Unlocked => {}
            Self::Securing { frame_pending, .. } | Self::Locked { frame_pending } => {
                *frame_pending = true;
            }
        }
    }

    pub(crate) fn expire_surface_grace(
        &mut self,
        now: std::time::Instant,
        grace: std::time::Duration,
    ) {
        if let Self::Securing {
            requested_at,
            frame_pending,
        } = self
            && now.duration_since(*requested_at) >= grace
        {
            *frame_pending = true;
        }
    }

    /// Record presentation of the requested secure frame.
    ///
    /// Returns `true` only for the first receipt, when the protocol's
    /// `locked` event becomes legal. Later receipts merely retire replacement
    /// or fallback-frame work while the session remains locked.
    pub(crate) fn secure_frame_presented(&mut self) -> bool {
        match *self {
            Self::Securing {
                frame_pending: true,
                ..
            } => {
                *self = Self::Locked {
                    frame_pending: false,
                };
                true
            }
            Self::Locked {
                frame_pending: true,
            } => {
                *self = Self::Locked {
                    frame_pending: false,
                };
                false
            }
            Self::Unlocked
            | Self::Securing {
                frame_pending: false,
                ..
            }
            | Self::Locked {
                frame_pending: false,
            } => false,
        }
    }

    pub(crate) fn unlock(&mut self) {
        *self = Self::Unlocked;
    }
}

/// Single-plane dma-buf parameters backing a `wl_buffer`, or accumulating in a
/// `zwp_linux_buffer_params_v1`. Owns the imported file descriptor.
struct DmabufBuffer {
    /// Monotonic identity independent of the `wl_resource` address. Wayland
    /// may recycle resource addresses after destruction, while renderer
    /// caches can legitimately outlive several in-flight frames.
    buffer_id: u64,
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

static NEXT_DMABUF_BUFFER_ID: AtomicU64 = AtomicU64::new(1);

fn next_dmabuf_buffer_id() -> u64 {
    let id = NEXT_DMABUF_BUFFER_ID.fetch_add(1, Ordering::Relaxed);
    if id == 0 {
        NEXT_DMABUF_BUFFER_ID.fetch_add(1, Ordering::Relaxed)
    } else {
        id
    }
}

impl DmabufBuffer {
    fn empty(state: *mut State) -> DmabufBuffer {
        DmabufBuffer {
            buffer_id: next_dmabuf_buffer_id(),
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
            buffer_id: self.buffer_id,
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
    size: Option<aegis_model::Size>,
    anchor_rect: Option<aegis_model::Rect>,
    anchor: u32,
    gravity: u32,
    constraint_adjustment: u32,
    offset: aegis_model::Point,
}

#[derive(Default)]
struct RegionRec {
    rects: Vec<aegis_model::Rect>,
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
    client_id: aegis_model::interaction_domain::ClientId,
    pending_buffer: *mut ffi::wl_resource,
    pending_buffer_set: bool,
    pending_attach_offset: aegis_model::Point,
    attach_offset: aegis_model::Point,
    pub mapped: bool,
    pub width: i32,
    pub height: i32,
    /// Logical position of the window rect's top-left corner in compositor
    /// space. For surfaces without a client-declared window geometry this is
    /// also the buffer's draw origin; for CSD surfaces that exclude shadows
    /// via `set_window_geometry` the buffer is drawn up-left of this point
    /// (see `surface_draw_origin`). M1 assigns a placeholder cascade on
    /// map; M3's window manager will own placement policy.
    pub position: aegis_model::Point,
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
    subsurface_offset: aegis_model::Point,
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
    pub window: aegis_model::window::Window,
    /// Imported xdg-foreign object that most recently established
    /// `window.parent`; null for xdg_toplevel.set_parent and unparented
    /// windows. This makes relationship revocation precise.
    foreign_parent_owner: *mut ffi::wl_resource,
    /// Tiling target (ADR-0024): the layout rect the tiling policy last
    /// configured this surface to, or `None` when not under active tiling.
    /// The apply path reconfigures only when the target moves.
    pub layout_target: Option<aegis_model::Rect>,
    /// Saved floating position and size prior to maximizing or full-screening,
    /// restored when unmaximized/unfullscreened.
    pub saved_floating_rect: Option<aegis_model::Rect>,
    // ----- wp_viewport state -----
    /// Source rectangle in surface pixel coords, or None for "whole buffer".
    /// Set by `wp_viewport.set_source`. Coordinates arrive as 24.8
    /// fixed-point; we store them as f32.
    pub viewport_src: Option<aegis_model::Rect>,
    pending_viewport_src: Option<Option<aegis_model::Rect>>,
    /// Destination size in logical pixels, or None for "source size".
    /// Set by `wp_viewport.set_destination`.
    pub viewport_dst: Option<aegis_model::Size>,
    pending_viewport_dst: Option<Option<aegis_model::Size>>,
    viewport_resource: *mut ffi::wl_resource,
    // ----- wp_fractional_scale_v1 state -----
    /// The `wp_fractional_scale_v1` resource bound for this surface, if any.
    /// The server posts `preferred_scale` here when the output's scale changes.
    pub fractional_scale: *mut ffi::wl_resource,
    /// Committed xdg-shell window geometry (excluding client shadows). Its
    /// size is the window rect's size; its origin is the frame inset by
    /// which the buffer sits up-left of the window rect (see
    /// `surface_draw_origin`).
    window_geometry: Option<aegis_model::Rect>,
    pending_window_geometry: Option<aegis_model::Rect>,
    /// `None` means the whole surface accepts input; `Some` is the union of
    /// rectangles copied from the last committed `wl_region`.
    input_region: Option<Vec<aegis_model::Rect>>,
    pending_input_region: Option<Option<Vec<aegis_model::Rect>>>,
    /// `None` means no opacity guarantee; `Some` is the union of rectangles
    /// copied from the last committed `wl_region`, in surface-local logical
    /// coordinates. Unlike the input-region default, a null opaque region is
    /// deliberately empty.
    opaque_region: Option<Vec<aegis_model::Rect>>,
    pending_opaque_region: Option<Option<Vec<aegis_model::Rect>>>,
    // ----- pending buffer transform / scale -----
    /// Pending buffer transform from `wl_surface.set_buffer_transform`,
    /// applied on the next commit.
    pending_transform: aegis_model::Transform,
    buffer_transform: aegis_model::Transform,
    /// Pending buffer scale from `wl_surface.set_buffer_scale`.
    pending_scale: i32,
    buffer_scale: i32,
    // ----- damage tracking -----
    /// Damage rectangles accumulated by `wl_surface.damage` since the last
    /// commit, in surface-local logical coordinates;
    /// empty means "client did not report damage, renderer should
    /// re-upload the whole texture on a generation change".
    pending_damage: Vec<aegis_model::Rect>,
    /// Raw buffer-coordinate rectangles accumulated by
    /// `wl_surface.damage_buffer`. Kept separate until commit because buffer
    /// scale/transform requests may be interleaved with damage requests.
    pending_buffer_damage: Vec<aegis_model::Rect>,
    /// Damage accumulated across every commit since the last successfully
    /// presented compositor frame, surfaced via `Server::toplevel_frames`.
    /// Multiple client commits can be dispatched before one render, so
    /// replacing this at each commit would make both texture upload and KMS
    /// damage miss earlier changed pixels.
    committed_damage: Vec<aegis_model::Rect>,
    /// Empty `committed_damage` normally means no outstanding damage. This
    /// flag distinguishes the conservative "damage is unknown/full" state.
    committed_damage_full: bool,
}

impl SurfaceRec {
    fn new(resource: *mut ffi::wl_resource) -> SurfaceRec {
        SurfaceRec {
            resource,
            client_id: aegis_model::interaction_domain::ClientId::default(),
            pending_buffer: std::ptr::null_mut(),
            pending_buffer_set: false,
            pending_attach_offset: aegis_model::Point::default(),
            attach_offset: aegis_model::Point::default(),
            mapped: false,
            width: 0,
            height: 0,
            position: aegis_model::Point::default(),
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
            subsurface_offset: aegis_model::Point::default(),
            subsurface_above_parent: true,
            subsurface_sync: true,
            subsurface_cached_commit: false,
            subsurface_applying_cached: false,
            window: aegis_model::window::Window::default(),
            foreign_parent_owner: std::ptr::null_mut(),
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
            opaque_region: None,
            pending_opaque_region: None,
            pending_transform: aegis_model::Transform::Normal,
            buffer_transform: aegis_model::Transform::Normal,
            pending_scale: 1,
            buffer_scale: 1,
            pending_damage: Vec::new(),
            pending_buffer_damage: Vec::new(),
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

fn surface_logical_size(surface: &SurfaceRec) -> aegis_model::Size {
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
    aegis_model::Size {
        w: (width as f32 / scale).round().max(1.0) as i32,
        h: (height as f32 / scale).round().max(1.0) as i32,
    }
}

fn intersect_rect(a: aegis_model::Rect, b: aegis_model::Rect) -> Option<aegis_model::Rect> {
    let ax1 = i64::from(a.origin.x) + i64::from(a.size.w.max(0));
    let ay1 = i64::from(a.origin.y) + i64::from(a.size.h.max(0));
    let bx1 = i64::from(b.origin.x) + i64::from(b.size.w.max(0));
    let by1 = i64::from(b.origin.y) + i64::from(b.size.h.max(0));
    let x0 = i64::from(a.origin.x).max(i64::from(b.origin.x));
    let y0 = i64::from(a.origin.y).max(i64::from(b.origin.y));
    let x1 = ax1.min(bx1);
    let y1 = ay1.min(by1);
    (x1 > x0 && y1 > y0)
        .then(|| aegis_model::Rect::new(x0 as i32, y0 as i32, (x1 - x0) as i32, (y1 - y0) as i32))
}

/// Clip, deduplicate and bound one Interaction Domain's damage metadata. The compositor
/// never exposes unchecked client coordinates through IPC.
fn normalize_interaction_domain_damage(
    rects: &mut Vec<aegis_model::Rect>,
    output: aegis_model::Rect,
) {
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
    rects.push(aegis_model::Rect::new(
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
pub(crate) fn surface_draw_origin(surface: &SurfaceRec) -> aegis_model::Point {
    surface_draw_origin_depth(surface, 0)
}

fn surface_draw_origin_depth(surface: &SurfaceRec, depth: u32) -> aegis_model::Point {
    // The depth cap only breaks reference cycles defensively; the destroy
    // path orphans children, so a live parent pointer is always valid.
    if !surface.parent.is_null() && depth < 32 {
        let parent = unsafe { &*surface.parent };
        let origin = surface_draw_origin_depth(parent, depth + 1);
        return aegis_model::Point {
            x: origin.x + surface.subsurface_offset.x,
            y: origin.y + surface.subsurface_offset.y,
        };
    }
    match surface.window_geometry {
        Some(geometry) => aegis_model::Point {
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
            rect.contains(aegis_model::Point {
                x: local_x as i32,
                y: local_y as i32,
            })
        })
    })
}

/// `wp_cursor_shape_device_v1.shape` value for one xdg-shell resize edge set.
fn resize_cursor_shape(edges: aegis_model::window::ResizeEdges) -> u32 {
    use aegis_model::window::ResizeEdges;
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
    rule_role: Option<aegis_model::layout::LayoutRole>,
) -> aegis_model::layout::LayoutRole {
    use aegis_model::layout::LayoutRole;
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
                (*rec).position = aegis_model::Point {
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
                (*rec).position = aegis_model::Point {
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
    info: aegis_model::output::OutputInfo,
    /// `None` for a physical backend output; directed virtual outputs belong
    /// to exactly one Interaction Domain.
    interaction_domain: Option<InteractionDomainId>,
    global: *mut ffi::wl_global,
    active: bool,
}

#[derive(Clone, Copy)]
struct TopBorderClick {
    window_id: aegis_model::window::WindowId,
    released_at_ms: u64,
    position: (f32, f32),
}

#[derive(Clone, Copy)]
struct PendingTopBorderDoubleClick {
    window_id: aegis_model::window::WindowId,
    press_position: (f32, f32),
    start_position: aegis_model::Point,
}

/// Runtime protocol and input state for one logical `wl_seat`.
///
/// The authority model in `aegis-model` owns durable identities and policy.
/// This structure owns the libwayland resources and ephemeral protocol state
/// for exactly one seat. Keeping these records separate is what prevents
/// agent input, focus, grabs, clipboard, and cursor state from contending
/// with the physical user's state.
pub(crate) struct SeatRuntime {
    id: SeatId,
    interaction_domain: InteractionDomainId,
    principal: InteractionPrincipalId,
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
    pending_text_input_states: Vec<aegis_model::input::TextInputState>,
    input_methods: Vec<*mut ffi::wl_resource>,
    virtual_keyboards: Vec<*mut ffi::wl_resource>,
    cursor_shape: u32,
    cursor_surface: *mut ffi::wl_resource,
    cursor_hotspot: aegis_model::Point,
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
    depressed_mods: aegis_model::input::Mods,
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
    interactive: Option<aegis_model::window::Interactive>,
    compositor_pointer_grab: bool,
    last_top_border_click: Option<TopBorderClick>,
    pending_top_border_double_click: Option<PendingTopBorderDoubleClick>,
    /// Window-local automation pins hit-testing to one authorized root for
    /// the duration of an atomic input batch. This prevents overlapping
    /// surfaces in another workspace (or another virtual placement) from
    /// stealing agent pointer focus while coordinates are translated through
    /// the client's compositor-global surface position.
    synthetic_target: Option<aegis_model::window::WindowId>,
}

impl SeatRuntime {
    fn new(
        id: SeatId,
        interaction_domain: InteractionDomainId,
        principal: InteractionPrincipalId,
        capabilities: SeatCapabilities,
    ) -> Self {
        Self {
            id,
            interaction_domain,
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
            cursor_hotspot: aegis_model::Point::default(),
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
            depressed_mods: aegis_model::input::Mods::NONE,
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
    id: aegis_model::interaction_domain::ClientId,
}

/// Frozen MRU order while the physical user holds Super for window cycling.
/// Selection is compositor-local until the session is committed, so browsing
/// never raises windows or transfers keyboard focus underneath the overlay.
struct WindowSwitcherSession {
    order: Vec<aegis_model::window::WindowId>,
    selected: usize,
    last_forward: bool,
}

const WORKSPACE_SLIDE_DURATION_MS: u64 = 220;

#[derive(Debug, Clone, Copy)]
struct WorkspaceSlideLayer {
    workspace: aegis_model::workspace::WorkspaceId,
    from_x: f32,
    to_x: f32,
}

/// Render-only horizontal motion for a committed workspace switch. Input,
/// focus, IPC, and the workspace model all move to the destination
/// immediately; this record keeps the source surfaces paintable just long
/// enough for both desktops to cross the output edge.
#[derive(Debug)]
struct WorkspaceSlide {
    output: aegis_model::Rect,
    layers: Vec<WorkspaceSlideLayer>,
    started_ms: u64,
    duration_ms: u64,
}

/// One independently composited workspace page during a horizontal switch.
#[derive(Debug, Clone)]
pub struct WorkspaceSlideLayerPresentation {
    pub windows: Vec<aegis_model::window::WindowId>,
    pub offset_x: f32,
}

/// Render-time workspace strip. Each layer must be clipped and drawn
/// independently so windows from separate workspaces never share one Z-order.
#[derive(Debug, Clone)]
pub struct WorkspaceSlidePresentation {
    pub output: aegis_model::Rect,
    pub layers: Vec<WorkspaceSlideLayerPresentation>,
}

impl WorkspaceSlide {
    fn is_active_at(&self, now_ms: u64) -> bool {
        self.duration_ms > 0 && now_ms.saturating_sub(self.started_ms) < self.duration_ms
    }

    fn offset_at(
        &self,
        workspace: aegis_model::workspace::WorkspaceId,
        now_ms: u64,
    ) -> Option<f32> {
        if !self.is_active_at(now_ms) {
            return None;
        }
        let layer = self
            .layers
            .iter()
            .find(|layer| layer.workspace == workspace)?;
        let elapsed = now_ms.saturating_sub(self.started_ms);
        let progress =
            aegis_model::transition::ease_out_cubic(elapsed as f32 / self.duration_ms as f32);
        Some(layer.from_x + (layer.to_x - layer.from_x) * progress)
    }
}

/// A deferred per-seat keyboard-focus transition requested from inside a
/// protocol callback. Callbacks cannot construct a second mutable `Server`,
/// so `Server::dispatch` applies the newest entry for each seat after they
/// return.
///
/// `restoring_from` distinguishes restoration entries — focus returning after
/// a dismissed popup or a closed/minimized toplevel — from grabs and explicit
/// focus requests (null). A restoration is dropped at apply time when the
/// seat's focus has meanwhile moved to a different mapped toplevel, such as a
/// dialog mapped in the same dispatch batch or an explicit window switch, so
/// it never steals focus from a newer window.
#[derive(Clone, Copy)]
pub(crate) struct DeferredKeyboardFocus {
    pub target: *mut ffi::wl_resource,
    pub restoring_from: *mut ffi::wl_resource,
}

/// Server-wide state. Its address is handed to the C bind callbacks, so it is
/// boxed and never moved out.
pub(crate) struct State {
    pub(crate) display: *mut ffi::wl_display,
    authority: InteractionDomainModel,
    seats: std::collections::BTreeMap<SeatId, Box<SeatRuntime>>,
    /// Seat whose event/request path is currently executing. The server main
    /// loop is single-threaded; an RAII guard changes this only for the bounded
    /// duration of one routed input batch or seat-owned protocol callback.
    active_seat: SeatId,
    #[allow(clippy::vec_box)]
    seat_globals: Vec<Box<SeatGlobal>>,
    /// Registry globals that are physical-session authority and must never be
    /// advertised to clients launched through an Interaction Domain portal.
    interaction_domain_hidden_globals: std::collections::HashSet<usize>,
    /// Reverse lookup for every seat-owned protocol resource. Entries remain
    /// until the resource destroy callback, so stale protocol objects fail
    /// closed after an interaction domain is revoked.
    seat_resource_owners: std::collections::HashMap<usize, SeatId>,
    /// Client-facing seat from which a routed child resource was created.
    /// Compatibility routing may change its runtime owner; native multi-seat
    /// rebinding restores the resource to this advertised origin.
    seat_resource_origins: std::collections::HashMap<usize, SeatId>,
    clients: std::collections::HashMap<usize, aegis_model::interaction_domain::ClientId>,
    /// Kernel-authenticated process id captured while a Wayland client is
    /// live. This is exposed only through the trusted accessibility binding
    /// seam, never through the general window snapshot.
    client_process_ids: std::collections::HashMap<aegis_model::interaction_domain::ClientId, u32>,
    /// Trusted launch-portal origin for clients accepted on a private Interaction Domain
    /// listener.
    /// Human/default-socket clients are omitted.
    client_initial_interaction_domains:
        std::collections::HashMap<aegis_model::interaction_domain::ClientId, InteractionDomainId>,
    client_bound_seats: std::collections::HashMap<usize, std::collections::BTreeSet<SeatId>>,
    /// Validated application accessibility trees. The compositor owns only
    /// the bounded projection and routing metadata; D-Bus stays in the
    /// out-of-process adapter.
    semantic_trees: aegis_semantic::SemanticTreeRegistry,
    interaction_domain_placements: std::collections::BTreeMap<
        (InteractionDomainId, aegis_model::window::WindowId),
        aegis_model::Rect,
    >,
    /// Interaction Domain layouts are recomputed after the current Wayland dispatch batch.
    /// Deferring keeps role creation and surface commits atomic from the
    /// client's perspective while ensuring newly mapped Interaction Domain windows receive
    /// a virtual-output placement before observers are notified.
    pending_interaction_domain_layouts: std::collections::BTreeSet<InteractionDomainId>,
    /// Windows whose committed scene content changed during this dispatch
    /// batch. `Server::take_interaction_domain_damage` maps these durable ids into each
    /// observing Interaction Domain's virtual-output coordinate space after layouts settle.
    damaged_windows: std::collections::BTreeSet<aegis_model::window::WindowId>,
    /// Conservative damage queued for topology changes where an old placement
    /// may no longer be recoverable (remove, transfer, output reconfigure).
    pending_interaction_domain_damage:
        std::collections::BTreeMap<InteractionDomainId, Vec<aegis_model::Rect>>,
    /// Surface pointers in stacking order (bottom to top). Entries are nulled
    /// when a surface's destroy notify fires; focusing a toplevel moves its
    /// pointer to the end and updates affected live records' slot indices.
    /// Iterators must skip null entries.
    surfaces: Vec<*mut SurfaceRec>,
    /// Monotonic millisecond timestamp of the last compatibility frame
    /// callback sent to surfaces that were not visible on the physical
    /// output.  Hidden clients get a low-rate heartbeat instead of being
    /// driven at the output refresh rate (which otherwise lets a covered
    /// browser/video keep producing buffers and waking the compositor).
    last_background_frame_callback_ms: u32,
    window_switcher: Option<WindowSwitcherSession>,
    workspace_slide: Option<WorkspaceSlide>,
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
    /// Live linux-dmabuf v4 feedback objects. Unlike one-shot globals, a
    /// feedback object stays subscribed: whenever the active KMS plane
    /// capabilities change the compositor sends another complete feedback
    /// batch and the client atomically adopts it at `done`.
    pub(crate) dmabuf_feedback_resources: Vec<DmabufFeedbackResource>,
    /// Live ext-idle-notify and idle-inhibit protocol resources. Per-object
    /// timer/role state is owned by resource user data in `extensions.rs`.
    pub(crate) idle_notifications: Vec<*mut ffi::wl_resource>,
    pub(crate) idle_inhibitors: Vec<*mut ffi::wl_resource>,
    /// Effective surfaceless global idle inhibitor held by scoped IPC
    /// connections. Unlike the per-surface protocol inhibitors above it has
    /// no visibility rules. The compositor clears each contribution when its
    /// IPC connection dies.
    pub(crate) ipc_idle_inhibit: bool,
    /// Physical tablet tools seen so far, with their announced info. A tool
    /// is announced to every seat the first time it enters proximity.
    pub(crate) known_tools: Vec<(u64, aegis_model::input::TabletToolInfo)>,
    retired_buffer_releases: Vec<RetiredBufferRelease>,
    /// Bound `ext_foreign_toplevel_list_v1` resources. New toplevels, title
    /// changes, and removals are pushed to each.
    foreign_toplevel_lists: Vec<*mut ffi::wl_resource>,
    /// Per-toplevel foreign handle resources, keyed by window id. Lets the
    /// server push title/app_id/closed updates to the right handle.
    foreign_handles: std::collections::HashMap<u64, Vec<*mut ffi::wl_resource>>,
    /// xdg-foreign-v2 capability handles and imports. The resource callbacks
    /// own their records; these indexes support lookup and surface teardown.
    xdg_foreign_exports: std::collections::HashMap<String, *mut ffi::wl_resource>,
    xdg_foreign_imports: Vec<*mut ffi::wl_resource>,
    activation_tokens: std::collections::HashMap<String, SeatId>,
    pending_activation: Option<(SeatId, *mut ffi::wl_resource)>,
    /// First-map workspace placements registered by `Command::LaunchApp`.
    /// Each entry places exactly one root toplevel: the first map whose
    /// client pid or app_id matches consumes it (FIFO), so a user's later
    /// manual launch of the same app can only steal a stale entry within
    /// the TTL.
    pub(crate) pending_launch_placements: Vec<server::PendingLaunchPlacement>,
    /// Keyboard-focus transitions requested from protocol callbacks, applied
    /// by `dispatch` after the callbacks return. This covers popup grabs and
    /// focus returning from a dismissed popup or closed toplevel; see
    /// [`DeferredKeyboardFocus`] for the restoration semantics.
    pending_keyboard_focus: std::collections::BTreeMap<SeatId, DeferredKeyboardFocus>,
    /// Active ext-session-lock object and fail-closed visibility phase.
    ///
    /// The object pointer may be null in `Locked` after its client dies. A
    /// replacement first-party locker can then assume responsibility without
    /// exposing the session between clients.
    pub(crate) session_lock: *mut c_void,
    pub(crate) session_lock_phase: SessionLockPhase,
    /// Development-only escape hatch (`[dev] allow_quit_while_locked`): while
    /// the session is locked, the global Quit binding still matches. Will be
    /// removed before release.
    pub(crate) allow_quit_while_locked: bool,
    pub(crate) lock_focus_dirty: bool,
    pub(crate) pending_lock_focus: *mut ffi::wl_resource,
    pub(crate) pre_lock_keyboard_focus: *mut ffi::wl_resource,
    /// Pending console VT switch requested by a Ctrl+Alt+Fn key press
    /// (XF86Switch_VT_N). The kernel never sees these keys once libinput owns
    /// evdev, so the compositor performs the session switch through libseat.
    /// Drained by the main loop via [`Server::take_vt_switch`].
    pending_vt_switch: Option<i32>,
    /// Parameters for the tiling policy (gaps, master ratio). Per-workspace
    /// tiling on/off lives on each workspace in the model (ADR-0024).
    layout_params: aegis_model::layout::LayoutParams,
    /// Accessibility reduced-motion policy (ADR-0029): when true, window
    /// transitions resolve in one frame and none are recorded.
    reduced_motion: bool,
    /// The configured minimize flight style (`[dock] minimize_animation`).
    pub(crate) minimize_animation: aegis_model::dock::MinimizeAnimationStyle,
    /// Resting dock-icon rectangles per window, pushed by the shell every
    /// frame: the minimize flight targets. While empty (startup, no dock),
    /// minimize falls back to a screen-edge stub target.
    pub(crate) minimize_targets:
        std::collections::HashMap<aegis_model::window::WindowId, aegis_model::Rect>,
    /// Effective decoration ownership announced to xdg-decoration clients.
    /// Borderless is compositor-owned: clients omit CSDs while window
    /// controls remain available through gestures and shell surfaces.
    decoration_policy: aegis_model::window::DecorationPolicy,
    /// Config-driven window rules (ADR-0026). Evaluated on first map; the
    /// first match prescribes a workspace move and/or a forced layout role.
    window_rules: Vec<aegis_model::window_rule::WindowRule>,
    /// The focused output's geometry (ADR-0028): the tiling work-area is its
    /// logical rect. Updated by the backend on resize; defaults to identity.
    pub(crate) output_geometry: aegis_model::output::OutputGeometry,
    /// Backend-reported connector geometry in global logical coordinates.
    /// The first entry is the primary/focused output exposed through the
    /// legacy single wl_output global until per-global resources are split.
    output_infos: Vec<aegis_model::output::OutputInfo>,
    /// Bumped on every `output_infos` mutation so the frame loop can skip
    /// re-cloning the list while it is unchanged.
    outputs_revision: u64,
    /// Per-connector output policies from `[[output]]` config entries
    /// (ADR-0028). Applied to every backend-reported output set in
    /// `set_outputs`.
    output_policies: std::collections::HashMap<String, aegis_model::output::OutputPolicy>,
    /// Dynamic per-output workspaces (ADR-0025). Toplevels are placed on the
    /// current workspace at first map; rendering and input see only the
    /// visible set (`visible_toplevels`).
    workspaces: aegis_model::workspace::WorkspaceModel,
    /// Focused output for new surfaces and workspace commands.
    output: aegis_model::workspace::OutputId,
    /// Monotonic counter for durable window identifiers (ADR-0032). Starts
    /// at 1 so `WindowId(0)` remains reserved for the `Window::default()`
    /// that non-toplevel surfaces carry.
    /// Cached chrome-aware work area bounds for maximized windows.
    pub(crate) last_work_area: aegis_model::Rect,
    pub(crate) epoch: std::time::Instant,
    /// Last remembered floating window position and size per application ID.
    pub(crate) last_app_geometries: std::collections::HashMap<String, aegis_model::Rect>,
    /// Persistent window state store across restarts.
    pub(crate) window_state_store: window_state::WindowStateStore,
    /// Path to persistent window state file.
    pub(crate) window_state_path: std::path::PathBuf,
    /// Global toggle for remembering window positions across restarts.
    pub(crate) remember_window_positions: bool,
    /// Format/modifier table advertised over `zwp_linux_dmabuf_v1`. Built by
    /// the renderer from the Vulkan device's real capabilities so clients
    /// allocate GPU-optimal (tiled/compressed) buffers instead of LINEAR.
    /// Drives the format/modifier events in `dmabuf_bind`.
    pub(crate) dmabuf_formats: Vec<aegis_model::dmabuf::DmabufFormat>,
    /// Format/modifier pairs that every active primary plane accepts for
    /// direct scanout. Feedback intersects this with the renderer table before
    /// advertising the preferred SCANOUT tranche.
    pub(crate) dmabuf_scanout_formats: Vec<aegis_model::dmabuf::DmabufFormat>,
    /// Linux `dev_t` of the renderer's preferred DRM node. When present the
    /// linux-dmabuf global is advertised at v4 and feedback objects use this
    /// as `main_device` and the renderer fallback tranche target. Without
    /// this, Mesa cannot reliably select the compositor's GPU and may fall
    /// back to llvmpipe.
    pub(crate) dmabuf_main_device: Option<u64>,
    /// Linux `dev_t` of the KMS node targeted by the SCANOUT tranche.
    pub(crate) dmabuf_scanout_device: Option<u64>,
    next_window_id: u64,
}

mod state;
mod window_state;

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
            state.client_initial_interaction_domains.remove(&record.id);
            state.client_process_ids.remove(&record.id);
            let _ = state.authority.disconnect_client(record.id);
        }
    }
}

/// One-shot Interaction Domain launch connections are a trusted client-identity portal.
/// Such clients see only their own `wl_seat` global, so a sandboxed app cannot
/// bind the physical user's seat even if it deliberately enumerates every
/// registry object. Ordinary desktop clients remain unrestricted for
/// compatibility with native multi-seat software.
unsafe extern "C" fn interaction_domain_global_filter(
    client: *const ffi::wl_client,
    global: *const ffi::wl_global,
    data: *mut c_void,
) -> bool {
    unsafe {
        let state = data as *mut State;
        if state.is_null() || client.is_null() || global.is_null() {
            return true;
        }
        let interaction_domain = (*state)
            .clients
            .get(&(client as usize))
            .and_then(|client_id| (*state).client_initial_interaction_domains.get(client_id))
            .copied();
        if interaction_domain.is_some()
            && (*state)
                .interaction_domain_hidden_globals
                .contains(&(global as usize))
        {
            return false;
        }
        if let Some(output) = (*state)
            .output_globals
            .iter()
            .find(|output| std::ptr::eq(output.global as *const ffi::wl_global, global))
        {
            return match interaction_domain {
                Some(interaction_domain) => output.interaction_domain == Some(interaction_domain),
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
            return interaction_domain.is_none_or(|interaction_domain| {
                (*state)
                    .authority
                    .seat(seat.seat)
                    .is_some_and(|seat| seat.interaction_domain == interaction_domain)
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
    frame: aegis_model::input::PointerAxisFrame,
) -> Vec<PointerAxisWireEvent> {
    use aegis_model::input::{
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
    interaction_domain_portals: Vec<InteractionDomainPortal>,
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
    state: aegis_model::input::ButtonState,
    outcome: keyboard::KeyOutcome,
    consumed_by_vt_switch: bool,
}

impl PreparedKeyboardEvent {
    /// Character view used by compositor chrome for a prepared press.
    ///
    /// VT-switch keysyms are compositor control events rather than text and
    /// therefore intentionally have no character view.
    pub fn key_char(self) -> Option<aegis_model::input::KeyChar> {
        (!self.consumed_by_vt_switch).then_some(aegis_model::input::KeyChar {
            keysym: self.outcome.keysym,
            ch: self.outcome.utf8,
            mods: aegis_model::input::Mods(self.outcome.depressed),
        })
    }
}

/// Prepared capability endpoint for one sandboxed application instance.
///
/// Before activation the socket has a short-lived randomized host pathname so
/// bubblewrap can bind-mount the socket inode. The launcher must unlink that
/// pathname and close all connections queued before the sandbox gate opens.
/// Once activated, only the bind mount inside that sandbox can reach it.
pub struct InteractionDomainPortal {
    interaction_domain: InteractionDomainId,
    path: PathBuf,
    listener: UnixListener,
}

impl InteractionDomainPortal {
    pub fn interaction_domain(&self) -> InteractionDomainId {
        self.interaction_domain
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn try_clone_listener(&self) -> std::io::Result<UnixListener> {
        self.listener.try_clone()
    }
}

impl Drop for InteractionDomainPortal {
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

/// Errors changing live interaction domain/seat topology after server startup.
#[derive(Debug, thiserror::Error)]
pub enum InteractionDomainRuntimeError {
    #[error(transparent)]
    Model(#[from] InteractionDomainError),
    #[error("failed to initialize independent XKB state: {0}")]
    Keyboard(String),
    #[error("wl_global_create failed for the agent seat")]
    SeatGlobal,
    #[error("wl_global_create failed for the InteractionDomain virtual output")]
    OutputGlobal,
    #[error("seat {} is unknown, paused, or revoked", .0.0)]
    SeatUnavailable(SeatId),
    #[error("interaction_domain {} has no logical seat", .0.0)]
    InteractionDomainHasNoSeat(InteractionDomainId),
    #[error("InteractionDomain semantic observation exceeds the {limit}-object safety bound")]
    SemanticObservationTooLarge { limit: usize },
    #[error("failed to create InteractionDomain launch portal: {0}")]
    Portal(String),
}
