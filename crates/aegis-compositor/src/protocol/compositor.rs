use crate::*;

// ----- wl_compositor ------------------------------------------------------

static COMPOSITOR_IMPL: ffi::wl_compositor_interface_impl = ffi::wl_compositor_interface_impl {
    create_surface: compositor_create_surface,
    create_region: compositor_create_region,
};

pub(crate) unsafe extern "C" fn compositor_bind(
    client: *mut ffi::wl_client,
    data: *mut c_void,
    version: u32,
    id: u32,
) {
    unsafe {
        let state = data as *mut State;
        if state.is_null() {
            return;
        }
        (*state).ensure_client(client);
        let res =
            ffi::wl_resource_create(client, &ffi::wl_compositor_interface, version as c_int, id);
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
}

unsafe extern "C" fn compositor_create_surface(
    client: *mut ffi::wl_client,
    compositor: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
        let state = ffi::wl_resource_get_user_data(compositor) as *mut State;
        let ver = ffi::wl_resource_get_version(compositor);
        let surface = ffi::wl_resource_create(client, &ffi::wl_surface_interface, ver, id);
        if surface.is_null() {
            return;
        }
        let rec = Box::into_raw(Box::new(SurfaceRec::new(surface)));
        (*rec).state = state;
        (*rec).client_id = (*state).ensure_client(client);
        (*rec).index = (*state).surfaces.len();
        ffi::wl_resource_set_implementation(
            surface,
            &SURFACE_IMPL as *const _ as *const c_void,
            rec as *mut c_void,
            Some(surface_resource_destroy),
        );
        (*state).surfaces.push(rec);
    }
}

unsafe extern "C" fn compositor_create_region(
    client: *mut ffi::wl_client,
    compositor: *mut ffi::wl_resource,
    id: u32,
) {
    unsafe {
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
}

// ----- wl_surface ---------------------------------------------------------

static SURFACE_IMPL: ffi::wl_surface_interface_impl = ffi::wl_surface_interface_impl {
    destroy: surface_destroy,
    attach: surface_attach,
    damage: surface_damage,
    frame: surface_frame,
    set_opaque_region: surface_set_opaque_region,
    set_input_region: surface_set_input_region,
    commit: surface_commit,
    set_buffer_transform: surface_set_buffer_transform,
    set_buffer_scale: surface_set_buffer_scale,
    damage_buffer: surface_damage_buffer,
};

/// Maximum number of exact rectangles retained for one surface between
/// presentations. Wayland clients control the damage list, so the compositor
/// must put a hard bound on both memory and region-subtraction work. Crossing
/// the bound degrades conservatively to one bounding box; normal clients keep
/// their exact disjoint damage all the way to output/effect invalidation.
pub(crate) const MAX_COMMITTED_DAMAGE_RECTS: usize = 64;

fn normalise_damage_rect(rect: aegis_model::Rect) -> Option<aegis_model::Rect> {
    if rect.is_empty() {
        return None;
    }
    // Keep Rect's later `origin + size` arithmetic representable. Damage is
    // clipped to the actual surface before use, so coordinates beyond the i32
    // domain carry no additional information.
    let x0 = i64::from(rect.origin.x);
    let y0 = i64::from(rect.origin.y);
    let x1 = (x0 + i64::from(rect.size.w)).min(i64::from(i32::MAX));
    let y1 = (y0 + i64::from(rect.size.h)).min(i64::from(i32::MAX));
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    Some(aegis_model::Rect::new(
        rect.origin.x,
        rect.origin.y,
        (x1 - x0) as i32,
        (y1 - y0) as i32,
    ))
}

fn damage_bbox(rects: &[aegis_model::Rect]) -> Option<aegis_model::Rect> {
    let first = *rects.first()?;
    let mut x0 = i64::from(first.origin.x);
    let mut y0 = i64::from(first.origin.y);
    let mut x1 = x0 + i64::from(first.size.w);
    let mut y1 = y0 + i64::from(first.size.h);
    for rect in &rects[1..] {
        let rx0 = i64::from(rect.origin.x);
        let ry0 = i64::from(rect.origin.y);
        x0 = x0.min(rx0);
        y0 = y0.min(ry0);
        x1 = x1.max(rx0 + i64::from(rect.size.w));
        y1 = y1.max(ry0 + i64::from(rect.size.h));
    }
    let width = x1 - x0;
    let height = y1 - y0;
    if width <= 0 || height <= 0 || width > i64::from(i32::MAX) || height > i64::from(i32::MAX) {
        // A single Rect cannot conservatively represent this span. The caller
        // must promote the surface to unknown/full damage rather than silently
        // truncating one side of the region.
        return None;
    }
    Some(aegis_model::Rect::new(
        x0.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        y0.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        width as i32,
        height as i32,
    ))
}

/// Add `rect` to an exact, disjoint damage region. Subtracting existing
/// coverage from the new rectangle avoids both duplicate work and the false
/// dirty pixels introduced by a bounding-box union.
fn insert_damage_rect(region: &mut Vec<aegis_model::Rect>, rect: aegis_model::Rect) -> bool {
    let Some(rect) = normalise_damage_rect(rect) else {
        return false;
    };
    let mut fragments = vec![rect];
    for existing in region.iter().copied() {
        let mut next = Vec::new();
        for fragment in fragments {
            next.extend(fragment.subtract(existing));
        }
        fragments = next;
        if fragments.is_empty() {
            return false;
        }
    }
    region.extend(fragments);
    if region.len() > MAX_COMMITTED_DAMAGE_RECTS {
        let bbox = damage_bbox(region);
        region.clear();
        if let Some(bbox) = bbox {
            region.push(bbox);
        } else {
            return true;
        }
    }
    false
}

/// Merge one commit's damage into the not-yet-presented surface damage.
/// Rectangles remain exact and disjoint under the bounded region budget so a
/// small video update cannot invalidate unrelated backdrop/chrome pixels.
pub(crate) fn accumulate_committed_damage(
    rec: &mut SurfaceRec,
    pending: Vec<aegis_model::Rect>,
    unknown_full: bool,
) {
    if rec.committed_damage_full {
        return;
    }
    if unknown_full {
        rec.committed_damage.clear();
        rec.committed_damage_full = true;
        return;
    }
    for rect in pending {
        if insert_damage_rect(&mut rec.committed_damage, rect) {
            rec.committed_damage.clear();
            rec.committed_damage_full = true;
            break;
        }
    }
}

pub(crate) fn reset_xdg_configure_state_after_unmap(rec: &mut SurfaceRec) {
    rec.xdg_configured = false;
    rec.xdg_configure_acked = false;
    rec.pending_xdg_configures.clear();
}

unsafe extern "C" fn surface_destroy(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn surface_attach(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    buffer: *mut ffi::wl_resource,
    x: i32,
    y: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        (*rec).pending_buffer = buffer;
        (*rec).pending_buffer_set = true;
        (*rec).pending_attach_offset = aegis_model::Point { x, y };
    }
}

unsafe fn retire_surface_buffer(rec: *mut SurfaceRec) {
    unsafe {
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
}

fn valid_policy_size(size: aegis_model::Size) -> Option<aegis_model::Size> {
    (size.w >= 100 && size.h >= 100).then_some(size)
}

/// Size to advertise in the first toplevel configure.
///
/// At the initial no-buffer commit, xdg-shell metadata (including
/// `set_parent`) is complete. This is the first safe point to restore a
/// main-window size: doing it from `set_app_id` races the transient
/// relationship, while doing it after the first buffer maps visibly resizes
/// the window.
pub(crate) unsafe fn initial_toplevel_size(rec: *mut SurfaceRec) -> Option<aegis_model::Size> {
    unsafe {
        if rec.is_null() || (*rec).state.is_null() {
            return None;
        }
        if (*rec).window.state.maximized || (*rec).window.state.fullscreen {
            return valid_policy_size((*rec).window.size);
        }
        let state = &*(*rec).state;
        let rule = state
            .window_rules
            .iter()
            .find(|rule| {
                rule.matches(
                    (*rec).window.app_id.as_deref(),
                    (*rec).window.title.as_deref(),
                )
            })
            .cloned();
        if let Some(size) = rule.as_ref().and_then(|rule| rule.size) {
            return valid_policy_size(size);
        }
        // Application-level remembered state belongs to primary windows,
        // never to an xdg_toplevel transient. Dialogs commonly share the
        // exact app_id of their parent.
        if (*rec).window.parent.is_some()
            || !state.remember_window_positions
            || !rule.as_ref().and_then(|rule| rule.remember).unwrap_or(true)
        {
            return None;
        }
        (*rec)
            .window
            .app_id
            .as_deref()
            .and_then(|app_id| state.window_state_store.get(app_id))
            .and_then(|saved| saved.size)
            .and_then(valid_policy_size)
    }
}

/// Center a transient in its parent and keep it inside the output whenever
/// the transient fits. Both rectangles use compositor-logical coordinates.
pub(crate) fn centered_transient_position(
    parent: aegis_model::Rect,
    child: aegis_model::Size,
    output: aegis_model::Rect,
) -> aegis_model::Point {
    let centered = aegis_model::Point {
        x: parent.origin.x + (parent.size.w - child.w) / 2,
        y: parent.origin.y + (parent.size.h - child.h) / 2,
    };
    let max_x = output
        .origin
        .x
        .saturating_add(output.size.w)
        .saturating_sub(child.w)
        .max(output.origin.x);
    let max_y = output
        .origin
        .y
        .saturating_add(output.size.h)
        .saturating_sub(child.h)
        .max(output.origin.y);
    aegis_model::Point {
        x: centered.x.clamp(output.origin.x, max_x),
        y: centered.y.clamp(output.origin.y, max_y),
    }
}

/// Diagonal nudge step, in logical pixels, for resolving origin collisions
/// between newly mapped windows (ADR-0131). Matches the legacy fallback
/// cascade's step so staggered windows keep the familiar rhythm.
pub(crate) const PLACEMENT_NUDGE_STEP: i32 = 32;

/// Maximum nudge attempts before the collision is accepted as-is. Bounds the
/// scan to a single output diagonal strip; a densely packed screen accepts
/// the last candidate rather than failing the map.
pub(crate) const PLACEMENT_NUDGE_MAX_STEPS: i32 = 8;

/// Resolve a newly mapped root toplevel's origin when it exactly collides
/// with a live window's origin (ADR-0131). Walks the diagonal from `base` in
/// `PLACEMENT_NUDGE_STEP` increments and returns the first origin that no
/// live mapped root toplevel occupies, clamped into `output`. `None` means
/// the origin does not collide or every candidate collides; either way the
/// window keeps `base`.
///
/// Pure and testable: `occupied` is the set of origins the caller considers
/// taken, in compositor-logical coordinates. Note this is an origin test, not
/// a rect-intersection test — remembered placement is per-application, so the
/// realistic collision is an exact stack of same-app or same-rule windows,
/// and a full overlap solver was rejected (see ADR-0116's rejection of
/// relaxation layouts).
pub(crate) fn nudged_origin_if_colliding(
    base: aegis_model::Point,
    occupied: &[aegis_model::Point],
    output: aegis_model::Rect,
) -> Option<aegis_model::Point> {
    if !occupied.contains(&base) {
        return None;
    }
    // Clamp bounds keep the title bar reachable: the same 100 px inset the
    // remembered-position path uses.
    let max_x = output
        .origin
        .x
        .saturating_add(output.size.w)
        .saturating_sub(100)
        .max(output.origin.x);
    let max_y = output
        .origin
        .y
        .saturating_add(output.size.h)
        .saturating_sub(100)
        .max(output.origin.y);
    let mut candidate = base;
    for _ in 0..PLACEMENT_NUDGE_MAX_STEPS {
        candidate = aegis_model::Point {
            x: candidate.x.saturating_add(PLACEMENT_NUDGE_STEP).min(max_x),
            y: candidate.y.saturating_add(PLACEMENT_NUDGE_STEP).min(max_y),
        };
        if !occupied.contains(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Fold a floating rect's origin back to its pre-nudge base (ADR-0131).
/// Called by both persistence sites before `persist_app_geometry`: while the
/// rect still rests exactly on the nudged origin — whether read from the
/// live position or from a `saved_floating_rect` captured there — the
/// compositor-invented offset is removed so it is never written to the
/// remembered-geometry store or `last_app_geometries`. Any other origin
/// (the user moved the window, or it was never nudged) passes through
/// unchanged.
pub(crate) fn fold_nudged_origin(
    rec: &SurfaceRec,
    mut rect: aegis_model::Rect,
) -> aegis_model::Rect {
    if let Some(nudge) = rec.placement_nudge
        && rect.origin == nudge.nudged
    {
        rect.origin = nudge.base;
    }
    rect
}

/// Consume a placement nudge once the window is explicitly repositioned by
/// the user or an agent (ADR-0131): interactive move, interactive resize,
/// `set-geometry`. From that moment the operator owns the position, so the
/// persistence fold-back no longer applies. A reposition to exactly the
/// nudged origin is a no-op and keeps the fold-back armed; policy-driven
/// moves (tiling, maximize/fullscreen, minimize) do not call this at all.
pub(crate) fn consume_placement_nudge(rec: &mut SurfaceRec, new_origin: aegis_model::Point) {
    if let Some(nudge) = rec.placement_nudge
        && new_origin != nudge.nudged
    {
        rec.placement_nudge = None;
    }
}

/// The live, mapped parent toplevel record behind a transient's protocol
/// parent pointer. Verify that the protocol-object pointer still names a live
/// surface before dereferencing it; a parent may be destroyed first.
unsafe fn live_transient_parent(rec: *mut SurfaceRec) -> Option<*mut SurfaceRec> {
    unsafe {
        let parent = (*rec).window.parent? as *mut SurfaceRec;
        if parent.is_null() || (*rec).state.is_null() {
            return None;
        }
        let live = (*(*rec).state)
            .live_surfaces()
            .any(|candidate| candidate == parent);
        if !live || !(*parent).mapped || (*parent).xdg_toplevel.is_null() {
            return None;
        }
        Some(parent)
    }
}

unsafe fn transient_parent_rect(rec: *mut SurfaceRec) -> Option<aegis_model::Rect> {
    unsafe {
        let parent = live_transient_parent(rec)?;
        Some(aegis_model::Rect {
            origin: (*parent).position,
            size: (*parent).window.size,
        })
    }
}

/// A cross-client or in-client dialog: a toplevel with a live mapped parent
/// (an `xdg_toplevel.set_parent` child, or a portal prompter that imported the
/// app's surface through `zxdg_importer_v2`). Dialogs are answers to
/// something the focused window asked for, so they are always user-solicited
/// from the seat's point of view.
///
/// Re-resolves the live parent here rather than trusting the caller: the
/// `transient_parent_id` captured before the `st` borrow can go stale if an
/// earlier branch of this dispatch reaped the parent.
pub(crate) unsafe fn toplevel_has_live_parent(rec: *mut SurfaceRec) -> bool {
    unsafe { live_transient_parent(rec).is_some() }
}

/// Move a toplevel to `new_origin`, carrying its whole popup subtree with
/// the same delta so menus, tooltips, and combo boxes stay anchored to the
/// window they belong to. Subsurfaces need no handling here: their draw
/// origin is already derived from the parent's position at render time.
///
/// This is the xdg-shell contract for a compositor that does not implement
/// `xdg_positioner.set_reactive` (v3): without reconstraint, the popup must
/// at minimum keep its position *relative to the parent* — leaving it at a
/// stale absolute origin is how menus end up floating over other windows.
///
/// Only `position` (the compositor-side origin) moves; no configure is sent,
/// because popup coordinates in `xdg_popup.configure` are parent-relative
/// and have not changed.
pub(crate) unsafe fn reposition_toplevel_with_popups(
    rec: *mut SurfaceRec,
    new_origin: aegis_model::Point,
) {
    unsafe {
        if rec.is_null() {
            return;
        }
        let old = (*rec).position;
        if old == new_origin {
            return;
        }
        let delta = aegis_model::Point {
            x: new_origin.x.saturating_sub(old.x),
            y: new_origin.y.saturating_sub(old.y),
        };
        (*rec).position = new_origin;
        (*rec).window.position = new_origin;
        shift_popup_subtree(rec, delta, 0);
    }
}

/// Recursively shift every live popup anchored (directly or through another
/// popup) to `rec`, preserving the parent-relative placement the positioner
/// computed at `get_popup` time. The depth cap breaks reference cycles
/// defensively; the destroy path detaches popups, so live chains are short.
unsafe fn shift_popup_subtree(rec: *mut SurfaceRec, delta: aegis_model::Point, depth: u32) {
    unsafe {
        if rec.is_null() || depth >= 32 || (*rec).state.is_null() {
            return;
        }
        for ptr in (*(*rec).state).live_surfaces_pub().filter(|p| {
            let p = *p;
            !p.is_null() && !(*p).popup_parent.is_null() && (*p).popup_parent == rec && (*p).mapped
        }) {
            (*ptr).position = aegis_model::Point {
                x: (*ptr).position.x.saturating_add(delta.x),
                y: (*ptr).position.y.saturating_add(delta.y),
            };
            shift_popup_subtree(ptr, delta, depth + 1);
        }
    }
}

/// Inputs to [`should_focus_mapped_toplevel`], one per policy question.
/// Grouped as a struct so the call site reads as a policy record and the
/// predicate signature stays clippy-clean as the policy grows.
#[derive(Debug, Clone, Copy)]
pub(crate) struct MappedToplevelFocusInputs {
    /// The toplevel is on a currently visible workspace.
    pub(crate) visible: bool,
    /// The human seat controls the window (not read-only / observed).
    pub(crate) human_controls: bool,
    /// The toplevel is minimized.
    pub(crate) minimized: bool,
    /// A pending launch placement matched this map (explicit user launch).
    pub(crate) is_user_launch: bool,
    /// The mapping client already owns keyboard focus (in-app child window).
    pub(crate) is_focused_client: bool,
    /// A live mapped parent exists (dialog, incl. cross-client portal
    /// prompters via `zxdg_importer_v2`).
    pub(crate) is_dialog: bool,
    /// No other live root toplevel shares this app_id: the app was not
    /// running, so this map is the user launching it.
    pub(crate) is_first_app_window: bool,
    /// The target workspace has no other mapped toplevel.
    pub(crate) is_empty_workspace: bool,
    /// An xdg-activation token already targets this surface.
    pub(crate) has_pending_activation: bool,
}

/// Determine whether a newly mapped toplevel should receive initial focus and raise
/// (`DirectFocus`) versus being placed passively in the background (`PassivePlacement`).
///
/// Focus Stealing Prevention (FSP): A newly mapped window only takes initial focus if
/// it was explicitly launched by the user (pending launch placement), belongs to the
/// currently focused client (in-app child window / dialog), is a dialog of a live
/// parent (including cross-client portal prompters), is the first window of an
/// application not yet running this session (a launch the user just performed by any
/// means — dock, launcher, terminal, D-Bus activation), is mapping to a previously
/// empty workspace, or holds an explicit activation token. Unsolicited background
/// windows are placed passively without stealing focus.
pub(crate) fn should_focus_mapped_toplevel(input: MappedToplevelFocusInputs) -> bool {
    let MappedToplevelFocusInputs {
        visible,
        human_controls,
        minimized,
        is_user_launch,
        is_focused_client,
        is_dialog,
        is_first_app_window,
        is_empty_workspace,
        has_pending_activation,
    } = input;
    visible
        && human_controls
        && !minimized
        && (is_user_launch
            || is_focused_client
            || is_dialog
            || is_first_app_window
            || is_empty_workspace
            || has_pending_activation)
}

/// Whether this map is the first live toplevel of its application in the
/// session. An app that is not running cannot have just decided to spawn a
/// window on its own: a first map is, by construction, the consequence of the
/// user launching that app (dock, launcher, terminal, D-Bus activation, or a
/// portal spawn whose launch placement did not match). Focusing it is what
/// every mainstream compositor does and what the user expects from "open an
/// app"; FSP demotion stays reserved for *additional* windows of an app that
/// is already running and is not the focused client.
pub(crate) unsafe fn is_first_toplevel_of_app(rec: *mut SurfaceRec) -> bool {
    unsafe {
        let app_id = (*rec).window.app_id.as_deref();
        match app_id {
            Some(app_id) if !app_id.is_empty() => {
                let state = (*rec).state;
                if state.is_null() {
                    return false;
                }
                !(*state).live_surfaces().any(|p| {
                    p != rec
                        && (*p).mapped
                        && !(*p).xdg_toplevel.is_null()
                        && (*p).window.parent.is_none()
                        && (*p).window.app_id.as_deref() == Some(app_id)
                })
            }
            // No app_id (X11-ish or bespoke clients): keep the conservative
            // FSP read. Such clients are rarely user launches through the
            // launcher, and a wrong demotion here is recoverable by clicking.
            _ => false,
        }
    }
}

/// Move a newly mapped background toplevel and its surface tree to the bottom
/// of the stacking order (index 0 of normal windows, before any existing windows).
/// Retained for callers that explicitly want a bottom-of-stack placement; the
/// first-map FSP path no longer uses it (ADR-0133).
#[cfg(test)]
pub(crate) unsafe fn lower_toplevel_surfaces(state: &mut State, resource: *mut ffi::wl_resource) {
    let Some(pos) = state.surfaces.iter().position(|p| {
        !p.is_null() && unsafe { (**p).resource == resource && !(**p).xdg_toplevel.is_null() }
    }) else {
        return;
    };
    let root = state.surfaces[pos];
    let mut lowered = Vec::new();
    let mut rest = Vec::with_capacity(state.surfaces.len());
    for ptr in state.surfaces.drain(..) {
        if !ptr.is_null() && unsafe { surface_root_toplevel(ptr) == root } {
            lowered.push(ptr);
        } else {
            rest.push(ptr);
        }
    }
    lowered.append(&mut rest);
    state.surfaces = lowered;
    for (index, ptr) in state.surfaces.iter().copied().enumerate() {
        if !ptr.is_null() {
            unsafe { (*ptr).index = index };
        }
    }
}

/// Return focus to the closest live, mapped transient parent after an
/// `xdg_toplevel` disappears. Walking past an already-unmapped intermediate
/// dialog handles nested dialog teardown in one dispatch batch.
pub(crate) unsafe fn transient_parent_keyboard_focus(
    surface: *mut SurfaceRec,
) -> *mut ffi::wl_resource {
    unsafe {
        if surface.is_null() || (*surface).state.is_null() {
            return std::ptr::null_mut();
        }
        let state = &*(*surface).state;
        let mut parent = (*surface)
            .window
            .parent
            .map_or(std::ptr::null_mut(), |parent| parent as *mut SurfaceRec);
        for _ in 0..32 {
            if parent.is_null() || !state.live_surfaces().any(|candidate| candidate == parent) {
                return std::ptr::null_mut();
            }
            if (*parent).mapped && !(*parent).xdg_toplevel.is_null() {
                return (*parent).resource;
            }
            parent = (*parent)
                .window
                .parent
                .map_or(std::ptr::null_mut(), |parent| parent as *mut SurfaceRec);
        }
        std::ptr::null_mut()
    }
}

/// Focus target when a focused window (or its popup) disappears without a
/// transient parent to return to: the topmost mapped, visible, non-minimized
/// toplevel the seat controls. The stacking Vec doubles as the focus MRU
/// because every focus change raises the focused window's tree, so scanning
/// from the tail finds the window that held focus before. This keeps typing
/// continuity instead of leaving the seat unfocused.
pub(crate) unsafe fn keyboard_focus_fallback(
    state: &State,
    seat: SeatId,
    exclude: *mut ffi::wl_resource,
) -> *mut ffi::wl_resource {
    let visible = state.workspaces.visible_toplevels();
    for ptr in state.surfaces.iter().rev() {
        if ptr.is_null() {
            continue;
        }
        let rec = *ptr;
        unsafe {
            if (*rec).resource == exclude
                || !(*rec).mapped
                || (*rec).xdg_toplevel.is_null()
                || (*rec).window.minimized
            {
                continue;
            }
            let id = (*rec).window.id;
            if !visible.contains(&id) || !state.authority.seat_controls_window(seat, id) {
                continue;
            }
            return (*rec).resource;
        }
    }
    std::ptr::null_mut()
}

/// SurfaceRec backing a wl_surface resource, resolved by walking the live
/// surface list instead of `wl_resource_get_user_data`, so state-level unit
/// tests can use synthetic resource pointers. Null for stale resources.
pub(crate) unsafe fn surface_rec_for_resource(
    state: &State,
    resource: *mut ffi::wl_resource,
) -> *mut SurfaceRec {
    if resource.is_null() {
        return std::ptr::null_mut();
    }
    state
        .live_surfaces()
        .find(|p| unsafe { (**p).resource == resource })
        .unwrap_or(std::ptr::null_mut())
}

/// Whether a deferred focus restoration still applies when `dispatch` drains
/// it. Restoration must not steal focus once the seat has moved to a
/// different mapped toplevel — a dialog mapped in the same dispatch batch, or
/// the user explicitly switching windows while the popup was open. Focus on
/// the dismissed surface itself, no focus, or focus anywhere inside the
/// target's own surface tree (a parent popup in a nested menu) all still
/// apply.
pub(crate) unsafe fn deferred_focus_restoration_applies(
    state: &State,
    current: *mut ffi::wl_resource,
    restoring_from: *mut ffi::wl_resource,
    target: *mut ffi::wl_resource,
) -> bool {
    if restoring_from.is_null() || current.is_null() || current == restoring_from {
        return true;
    }
    unsafe {
        let current_root = surface_root_toplevel(surface_rec_for_resource(state, current));
        if current_root.is_null()
            || !(*current_root).mapped
            || (*current_root).xdg_toplevel.is_null()
        {
            return true;
        }
        let target_root = surface_root_toplevel(surface_rec_for_resource(state, target));
        current_root == target_root
    }
}

/// Defer the focus transition required when a toplevel unmaps or loses its
/// role. Only seats whose focus rests on that surface's tree (or that already
/// have a pending transition onto it) are affected. A transient returns to
/// its nearest mapped parent; otherwise focus falls back to the most recently
/// raised window the seat controls, so closing a window never strands the
/// seat without focus while another candidate remains.
pub(crate) unsafe fn defer_keyboard_focus_after_toplevel_unmap(surface: *mut SurfaceRec) {
    unsafe {
        if surface.is_null() || (*surface).state.is_null() {
            return;
        }
        let state = &mut *(*surface).state;
        let resource = (*surface).resource;
        let transient_parent = transient_parent_keyboard_focus(surface);
        if state
            .pending_activation
            .is_some_and(|(_, pending)| pending == resource)
        {
            state.pending_activation = None;
        }
        let affected_seats = state
            .seats
            .iter()
            .filter_map(|(seat, runtime)| {
                let focused_root =
                    surface_root_toplevel(surface_rec_for_resource(state, runtime.keyboard_focus));
                (!focused_root.is_null() && focused_root == surface
                    || state
                        .pending_keyboard_focus
                        .get(seat)
                        .is_some_and(|pending| pending.target == resource))
                .then_some(*seat)
            })
            .collect::<Vec<_>>();
        for seat in affected_seats {
            let target = if transient_parent.is_null() {
                keyboard_focus_fallback(state, seat, resource)
            } else {
                transient_parent
            };
            state.pending_keyboard_focus.insert(
                seat,
                DeferredKeyboardFocus {
                    target,
                    restoring_from: resource,
                },
            );
        }
    }
}

pub(crate) unsafe extern "C" fn surface_commit(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if !(*rec).parent.is_null() && (*rec).subsurface_sync && !(*rec).subsurface_applying_cached
        {
            (*rec).subsurface_cached_commit = true;
            return;
        }
        (*rec).subsurface_cached_commit = false;
        let visual_metadata_changed = (*rec).pending_window_geometry.is_some()
            || (*rec).pending_viewport_src.is_some()
            || (*rec).pending_viewport_dst.is_some()
            || (*rec).pending_opaque_region.is_some()
            || (*rec).pending_transform != (*rec).buffer_transform
            || (*rec).pending_scale != (*rec).buffer_scale
            || (*rec).pending_image_description.is_some()
            || !(*rec).pending_damage.is_empty()
            || !(*rec).pending_buffer_damage.is_empty();
        let old_window_size = (*rec).window.size;
        if let Some(region) = (*rec).pending_input_region.take() {
            (*rec).input_region = region;
        }
        if let Some(region) = (*rec).pending_opaque_region.take() {
            (*rec).opaque_region = region;
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
        // wp_color_management_v1: the tag applies at commit like every
        // other double-buffered surface property.
        if let Some(description) = (*rec).pending_image_description.take() {
            (*rec).image_description = description;
        }
        (*rec).buffer_transform = (*rec).pending_transform;
        (*rec).buffer_scale = (*rec).pending_scale;

        // xdg-shell initial configure: on the first commit of a surface that has an
        // xdg role, send a configure and wait for the client to ack and attach a
        // buffer. The initial commit carries no buffer, so mapping happens on a
        // later commit.
        if !(*rec).xdg_surface.is_null() && !(*rec).xdg_configured {
            if !(*rec).xdg_toplevel.is_null() {
                let initial_size = initial_toplevel_size(rec);
                if let Some(size) = initial_size {
                    (*rec).window.size = size;
                }
                let mut states = ffi::wl_array::empty();
                ffi::wl_resource_post_event(
                    (*rec).xdg_toplevel,
                    ffi::XDG_TOPLEVEL_CONFIGURE,
                    initial_size.map(|size| size.w).unwrap_or(0),
                    initial_size.map(|size| size.h).unwrap_or(0),
                    &mut states as *mut ffi::wl_array,
                );
            }
            send_xdg_surface_configure(rec);
            (*rec).xdg_configured = true;
        }

        let was_mapped = (*rec).mapped;
        let buffer = (*rec).pending_buffer;
        let buffer_set = std::mem::take(&mut (*rec).pending_buffer_set);
        let scene_changed = buffer_set || visual_metadata_changed;
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
                c"buffer committed before the initial xdg_surface.configure was acknowledged"
                    .as_ptr(),
            );
            return;
        }
        if buffer_set {
            (*rec).attach_offset = (*rec).pending_attach_offset;
        }
        // The pending transform and scale are surfaced to the renderer via
        // `SurfaceGeometry` (see toplevel_*_frames below); the renderer applies
        // them at composite time.
        // Accumulate until a compositor frame is actually presented. The
        // event loop can dispatch several commits before rendering once; only
        // retaining the newest commit's damage would leave earlier pixels
        // stale in the retained shm snapshot/GPU texture and under-report the
        // KMS damage hint. Empty damage on a new buffer carries no usable
        // information and therefore poisons the aggregate to full.
        let mut pending_damage = std::mem::take(&mut (*rec).pending_damage);
        let pending_buffer_damage = std::mem::take(&mut (*rec).pending_buffer_damage);
        // buffer damage (wl_surface.damage_buffer) is in buffer-pixel space. It
        // is mappable to surface-local logical pixels whenever the pending
        // buffer's own dimensions are known (so the transform can be applied)
        // and the surface has no wp_viewport crop/destination — viewport
        // changes the buffer↔surface sample relationship in a way a simple
        // rect transform cannot capture. The historical guard also bailed on
        // any buffer_transform != Normal; that is now handled by a real
        // 8-way geometric remap, so rotated/flipped clients no longer force a
        // whole-output repaint.
        let buffer_dims = if !buffer.is_null() {
            pending_buffer_dimensions(buffer)
        } else {
            None
        };
        let buffer_damage_unmappable = !pending_buffer_damage.is_empty()
            && (buffer_dims.is_none()
                || (*rec).viewport_src.is_some()
                || (*rec).viewport_dst.is_some());
        if !buffer_damage_unmappable {
            let (bw, bh) = buffer_dims.unwrap_or(((*rec).width, (*rec).height));
            let transform = (*rec).buffer_transform;
            let scale = (*rec).buffer_scale;
            pending_damage.extend(pending_buffer_damage.into_iter().map(|damage| {
                let mapped = transform.map_buffer_rect_to_surface(damage, (bw, bh));
                buffer_damage_to_surface(mapped, scale)
            }));
        }
        let unknown_full = buffer_damage_unmappable
            || (buffer_set && !buffer.is_null() && pending_damage.is_empty());
        accumulate_committed_damage(&mut *rec, pending_damage, unknown_full);
        if buffer_set && buffer.is_null() {
            // ADR-0029 close transition: snapshot the last frame before the
            // shm pixels are cleared so the ghost can keep rendering it.
            if was_mapped && !(*rec).xdg_toplevel.is_null() && !(*rec).state.is_null() {
                (*(*rec).state).note_close_transition(rec);
            }
            retire_surface_buffer(rec);
            (*rec).dmabuf = None;
            (*rec).mapped = false;
            if was_mapped && !(*rec).xdg_surface.is_null() {
                reset_xdg_configure_state_after_unmap(&mut *rec);
            }
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
                if !db.is_null()
                    && (*db).have_plane
                    && let Some(owned) = (*db).duplicate()
                {
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
                // Empty damage carries no information and forces a full copy,
                // as does a transform (surface-local rectangles do not map
                // axis-for-axis onto the retained buffer). Integer buffer scale
                // is mapped explicitly below. The guard mirrors the renderer's
                // incremental-upload guard exactly; the two paths must always
                // agree or the texture would tear.
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
                            && (*rec).buffer_transform == aegis_model::Transform::Normal
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
                            // Damage rects are surface-local logical
                            // coordinates; map them to buffer pixels (rounded
                            // outward) before copying, so a HiDPI client's
                            // incremental refresh is no longer forced into a
                            // whole-buffer copy. The factor is the effective
                            // logical→buffer scale: fractional-scale clients
                            // keep buffer_scale at 1 and carry the density in
                            // a wp_viewport destination, which buffer_scale
                            // alone would under-cover, stranding the far side
                            // of every alternated buffer (read: truncated
                            // Chrome tooltips).
                            let geometry = aegis_model::SurfaceGeometry {
                                transform: (*rec).buffer_transform,
                                buffer_scale: (*rec).buffer_scale,
                                viewport_src: (*rec).viewport_src,
                                viewport_dst: (*rec).viewport_dst,
                                ..Default::default()
                            };
                            let (scale_x, scale_y) = geometry.logical_to_buffer_scale(w, h);
                            for d in &damage {
                                let x =
                                    ((d.origin.x as f32 * scale_x).floor() as i32).max(0).min(w);
                                let y =
                                    ((d.origin.y as f32 * scale_y).floor() as i32).max(0).min(h);
                                let x1 = (((d.origin.x + d.size.w) as f32 * scale_x).ceil() as i32)
                                    .max(0)
                                    .min(w);
                                let y1 = (((d.origin.y + d.size.h) as f32 * scale_y).ceil() as i32)
                                    .max(0)
                                    .min(h);
                                let cw = (x1 - x) as usize;
                                let ch = (y1 - y) as usize;
                                if cw == 0 || ch == 0 {
                                    continue;
                                }
                                let x = x as usize;
                                let y = y as usize;
                                for row in 0..ch {
                                    std::ptr::copy_nonoverlapping(
                                        src.add((y + row) * stride + x * 4),
                                        pixels.as_mut_ptr().add((y + row) * tight + x * 4),
                                        cw * 4,
                                    );
                                }
                                // XRGB8888 has undefined alpha; force opaque on
                                // the refreshed rows.
                                if aegis_model::dmabuf::is_wl_shm_format_xrgb(format) {
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
                            if aegis_model::dmabuf::is_wl_shm_format_xrgb(format) {
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
                    let release = std::mem::replace(
                        &mut (*rec).committed_explicit_release,
                        std::ptr::null_mut(),
                    );
                    ffi::wl_resource_post_event(
                        release,
                        ffi::ZWP_LINUX_BUFFER_RELEASE_V1_IMMEDIATE_RELEASE,
                    );
                    ffi::wl_resource_destroy(release);
                }
                (*rec).pending_buffer = std::ptr::null_mut();
            }
        }

        // Buffer transform/scale/viewport/window-geometry commits alter the
        // composed pixels even when no new wl_buffer is attached. Give the
        // render/damage generation tracker an edge to observe; its geometry
        // guard conservatively promotes these cases to full damage.
        if visual_metadata_changed && !buffer_set {
            (*rec).generation = (*rec).generation.wrapping_add(1);
        }

        if (*rec).mapped && !was_mapped && !(*rec).xdg_toplevel.is_null() {
            let app_id = (*rec).window.app_id.clone();
            let title = (*rec).window.title.clone();
            let rule = if !(*rec).state.is_null() {
                let st = &mut *(*rec).state;
                st.window_rules
                    .iter()
                    .find(|r| r.matches(app_id.as_deref(), title.as_deref()))
                    .cloned()
            } else {
                None
            };

            let rule_pos = rule.as_ref().and_then(|r| r.position);
            let rule_size = rule.as_ref().and_then(|r| r.size);
            let rule_remember = rule.as_ref().and_then(|r| r.remember);
            let is_transient = (*rec).window.parent.is_some();

            let allow_remember = !is_transient
                && rule_remember.unwrap_or(true)
                && if !(*rec).state.is_null() {
                    (*(*rec).state).remember_window_positions
                } else {
                    true
                };

            let remembered_store_entry = if allow_remember && !(*rec).state.is_null() {
                app_id
                    .as_deref()
                    .and_then(|id| (*(*rec).state).window_state_store.get(id).cloned())
            } else {
                None
            };
            let last_app_rect = if allow_remember {
                app_id.as_deref().and_then(|id| {
                    if !(*rec).state.is_null() {
                        (*(*rec).state).last_app_geometries.get(id).copied()
                    } else {
                        None
                    }
                })
            } else {
                None
            };
            let parent_rect = transient_parent_rect(rec);
            // The live, mapped transient parent, if any. Computed here, before
            // `st` is borrowed below, because the guard re-dereferences state.
            let transient_parent_id = live_transient_parent(rec).map(|parent| (*parent).window.id);

            // Resolve the placement origin (ADR-0131): rule position or
            // remembered position, then the session's last geometry for this
            // app, then the fallback diagonal cascade. Whatever origin this
            // chain produces is the *base*; if it exactly collides with a
            // live window's origin, nudge diagonally to a free origin. The
            // nudge is session-scoped and never persisted (see
            // `placement_nudge`).
            let output = if !(*rec).state.is_null() {
                (*(*rec).state).output_geometry.logical_rect()
            } else {
                aegis_model::Rect::new(0, 0, 1920, 1080)
            };
            let target_pos =
                rule_pos.or_else(|| remembered_store_entry.as_ref().and_then(|s| s.position));
            let base_pos = if let Some(pos) = target_pos {
                let max_x = output
                    .origin
                    .x
                    .saturating_add(output.size.w)
                    .saturating_sub(100)
                    .max(output.origin.x);
                let max_y = output
                    .origin
                    .y
                    .saturating_add(output.size.h)
                    .saturating_sub(100)
                    .max(output.origin.y);
                aegis_model::Point {
                    x: pos.x.clamp(output.origin.x, max_x),
                    y: pos.y.clamp(output.origin.y, max_y),
                }
            } else if let Some(rect) = last_app_rect {
                rect.origin
            } else if parent_rect.is_none() {
                let count = if (*rec).state.is_null() {
                    0
                } else {
                    (*(*rec).state)
                        .live_surfaces()
                        .filter(|p| !(**p).xdg_toplevel.is_null() && (**p).mapped)
                        .count()
                };
                let idx = count.min(8) as i32;
                aegis_model::Point {
                    x: 60 + idx * 32,
                    y: 60 + idx * 32,
                }
            } else {
                // Transients keep their eventual centered position; the
                // parent-centering pass below owns them.
                (*rec).position
            };
            (*rec).position = base_pos;
            (*rec).window.position = base_pos;
            if parent_rect.is_none() {
                let occupied = if (*rec).state.is_null() {
                    Vec::new()
                } else {
                    (*(*rec).state)
                        .live_surfaces()
                        .filter(|p| {
                            *p != rec
                                && !(**p).xdg_toplevel.is_null()
                                && (**p).mapped
                                && (**p).window.parent.is_none()
                        })
                        .map(|p| (*p).position)
                        .collect::<Vec<_>>()
                };
                if let Some(nudged) = nudged_origin_if_colliding(base_pos, &occupied, output) {
                    (*rec).placement_nudge = Some(PlacementNudge {
                        base: base_pos,
                        nudged,
                    });
                    (*rec).position = nudged;
                    (*rec).window.position = nudged;
                }
            }

            let target_size =
                rule_size.or_else(|| remembered_store_entry.as_ref().and_then(|s| s.size));
            let mapped_size = (*rec)
                .window_geometry
                .map(|geometry| geometry.size)
                .unwrap_or_else(|| surface_logical_size(&*rec));
            if let Some(size) = target_size.and_then(valid_policy_size) {
                (*rec).window.size = size;
                // Normally this was already advertised in the initial
                // configure. Only correct a client that mapped a different
                // size; do not send the redundant post-map configure that
                // made windows visibly "open, then shrink".
                if mapped_size != size {
                    reconfigure_with_size(rec, size.w, size.h);
                }
            } else {
                (*rec).window.size = mapped_size;
            }

            if rule_pos.is_none()
                && let Some(parent) = parent_rect
            {
                let output = if !(*rec).state.is_null() {
                    (*(*rec).state).output_geometry.logical_rect()
                } else {
                    aegis_model::Rect::new(0, 0, 1920, 1080)
                };
                let centered = centered_transient_position(parent, (*rec).window.size, output);
                (*rec).position = centered;
                (*rec).window.position = centered;
            }

            log::info!(
                "[server] surface mapped at {:?}: {}x{}",
                (*rec).position,
                (*rec).width,
                (*rec).height
            );

            // ADR-0029 open transition: fade-and-scale in from a slightly
            // inset rect. Recorded after placement settles so the flight
            // starts at the final mapped rect.
            if !(*rec).state.is_null() {
                (*(*rec).state).note_open_transition(rec);
            }

            if !(*rec).state.is_null() {
                let id = (*rec).window.id;
                let st = &mut *(*rec).state;
                if let Some(wid) = st.workspaces.current_workspace(st.output) {
                    st.workspaces.place_toplevel(wid, id);
                }
                // A dialog must never open on a different workspace than its
                // parent: previously a moved or re-targeted parent's dialogs
                // opened on the user's current workspace.
                if let Some(parent_ws) =
                    transient_parent_id.and_then(|parent| st.workspaces.workspace_of(parent))
                {
                    st.workspaces.move_toplevel(id, parent_ws);
                }
                // An explicit launch placement (ADR-0118) beats the
                // rule/store workspace override below.
                // Transients follow their parent instead, so only root
                // toplevels consume a pending entry.
                let launch_placement_applied = if is_transient {
                    false
                } else {
                    let pid = {
                        let mut pid = 0;
                        let mut uid = 0;
                        let mut gid = 0;
                        ffi::wl_client_get_credentials(
                            ffi::wl_resource_get_client((*rec).resource),
                            &mut pid,
                            &mut uid,
                            &mut gid,
                        );
                        // pid 0 means the credentials lookup failed.
                        u32::try_from(pid).ok().filter(|pid| *pid != 0)
                    };
                    match server::take_pending_launch_placement(
                        &mut st.pending_launch_placements,
                        app_id.as_deref(),
                        pid,
                        std::time::Instant::now(),
                    ) {
                        Some(aegis_model::workspace::LaunchPlacement::Workspace { id: target })
                            // The target may be gone by map time; fall through
                            // to the rule/store override then.
                            if st.workspaces.workspace(target).is_some() =>
                        {
                            st.workspaces.move_toplevel(id, target);
                            true
                        }
                        Some(aegis_model::workspace::LaunchPlacement::FreshWorkspace {
                            label,
                        }) => st
                            .workspaces
                            // `None` (unknown output) likewise falls through.
                            .place_toplevel_on_fresh_workspace(st.output, id, label)
                            .is_some(),
                        _ => false,
                    }
                };
                if !launch_placement_applied {
                    let target_workspace = rule.as_ref().and_then(|r| r.workspace).or_else(|| {
                        if allow_remember {
                            remembered_store_entry.as_ref().and_then(|s| s.workspace)
                        } else {
                            None
                        }
                    });
                    if let Some(ws_idx1) = target_workspace {
                        let idx = (ws_idx1 as usize).saturating_sub(1);
                        if let Some(o) = st.workspaces.output(st.output)
                            && let Some(&target) = o.workspaces.get(idx)
                        {
                            st.workspaces.move_toplevel(id, target);
                        }
                    }
                }

                let rule_role = rule.as_ref().and_then(|r| r.role).or_else(|| {
                    if allow_remember {
                        remembered_store_entry.as_ref().and_then(|s| s.layout_role)
                    } else {
                        None
                    }
                });

                let workspace_tiled = st
                    .workspaces
                    .workspace_of(id)
                    .and_then(|wid| st.workspaces.workspace(wid))
                    .map(|ws| ws.tiled)
                    .unwrap_or(false);
                (*rec).window.layout_role =
                    resolve_layout_role(workspace_tiled, (*rec).window.parent.is_some(), rule_role);

                let is_user_launch = launch_placement_applied;
                let client = ffi::wl_resource_get_client((*rec).resource);
                let is_focused_client = !st.keyboard_focus.is_null()
                    && ffi::wl_resource_get_client(st.keyboard_focus) == client;
                // Re-resolve the live parent here: `transient_parent_id` was
                // captured before the `st` borrow and can go stale within
                // this dispatch batch.
                let is_dialog = toplevel_has_live_parent(rec);
                let is_first_app_window = is_first_toplevel_of_app(rec);
                let target_ws = st.workspaces.workspace_of(id);
                let is_empty_workspace = target_ws.is_some_and(|ws| {
                    !st.surfaces.iter().any(|&p| {
                        !p.is_null()
                            && (*p).mapped
                            && !(*p).xdg_toplevel.is_null()
                            && (*p).window.id != id
                            && st.workspaces.workspace_of((*p).window.id) == Some(ws)
                    })
                });
                let has_pending_activation = st
                    .pending_activation
                    .is_some_and(|(_, pending)| pending == (*rec).resource);

                let visible = st.workspaces.visible_toplevels().contains(&id);
                let human_controls = st.authority.seat_controls_window(HUMAN_SEAT, id);

                let should_focus = should_focus_mapped_toplevel(MappedToplevelFocusInputs {
                    visible,
                    human_controls,
                    minimized: (*rec).window.minimized,
                    is_user_launch,
                    is_focused_client,
                    is_dialog,
                    is_first_app_window,
                    is_empty_workspace,
                    has_pending_activation,
                });

                if should_focus && st.pending_activation.is_none() {
                    st.pending_activation = Some((HUMAN_SEAT, (*rec).resource));
                }
                // FSP rejection keeps the window out of `pending_activation`
                // (no focus steal) but leaves its stacking alone: the map path
                // above already appended it on top of its workspace, and a
                // deliberate demotion-to-bottom proved too aggressive — it
                // buried dialogs and freshly launched apps behind every
                // existing window (see ADR-0133).
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
        if !(*rec).state.is_null() && !(*rec).xdg_toplevel.is_null() {
            let window = (*rec).window.id;
            if was_mapped != (*rec).mapped || old_window_size != (*rec).window.size {
                (*(*rec).state).queue_interaction_domain_layouts_for_window(window);
            }
        }
        if !(*rec).mapped && was_mapped && !(*rec).xdg_toplevel.is_null() {
            defer_keyboard_focus_after_toplevel_unmap(rec);
        }
        if !(*rec).mapped && was_mapped && !(*rec).xdg_popup.is_null() && (*rec).popup_grabbed {
            let mut focus_after_dismissal = popup_keyboard_focus_after_dismissal(rec);
            if let Some(seat) = (*rec).popup_grab_seat.take()
                && !(*rec).state.is_null()
            {
                if focus_after_dismissal.is_null() {
                    focus_after_dismissal =
                        keyboard_focus_fallback(&*(*rec).state, seat, (*rec).resource);
                }
                (*(*rec).state).pending_keyboard_focus.insert(
                    seat,
                    DeferredKeyboardFocus {
                        target: focus_after_dismissal,
                        restoring_from: (*rec).resource,
                    },
                );
            }
            (*rec).popup_grabbed = false;
        }
        if (*rec).mapped
            && !was_mapped
            && !(*rec).xdg_popup.is_null()
            && (*rec).popup_grabbed
            && let Some(seat) = (*rec).popup_grab_seat
            && !(*rec).state.is_null()
        {
            // The xdg-shell grab contract requires the topmost grabbing popup
            // to hold keyboard focus. Defer out of this libwayland callback so
            // no second mutable `Server` aliases `State`.
            (*(*rec).state).pending_keyboard_focus.insert(
                seat,
                DeferredKeyboardFocus {
                    target: (*rec).resource,
                    restoring_from: std::ptr::null_mut(),
                },
            );
        }
        if (*rec).mapped && !was_mapped && !(*rec).state.is_null() {
            let root = surface_root_toplevel(rec);
            if !root.is_null() {
                let state = &*(*rec).state;
                for interaction_domain in
                    output_interaction_domains_for_window(state, (*root).window.id)
                {
                    post_surface_output_event(
                        state,
                        (*rec).resource,
                        interaction_domain,
                        ffi::WL_SURFACE_ENTER,
                    );
                }
            }
        } else if !(*rec).mapped && was_mapped && !(*rec).state.is_null() {
            let root = surface_root_toplevel(rec);
            if !root.is_null() {
                let state = &*(*rec).state;
                for interaction_domain in
                    output_interaction_domains_for_window(state, (*root).window.id)
                {
                    post_surface_output_event(
                        state,
                        (*rec).resource,
                        interaction_domain,
                        ffi::WL_SURFACE_LEAVE,
                    );
                }
            }
        }
        // A surface appearing or disappearing changes what is under a
        // stationary cursor. Defer a pointer re-hit to dispatch (protocol
        // callbacks cannot re-enter the Server here): without it a popup
        // that maps under the cursor — a Qt menu, a Chrome bubble — never
        // receives wl_pointer.enter until the next motion, so the client's
        // pointer tracking stays on the owning toplevel and the first click
        // after the menu opens is misrouted. Cursor and drag-icon surfaces
        // track the pointer by design and are excluded.
        if was_mapped != (*rec).mapped
            && !(*rec).cursor_role
            && !(*rec).drag_icon_role
            && !(*rec).state.is_null()
        {
            schedule_pointer_rehit((*rec).state, None);
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
        extensions::input_popup_surface_committed(rec);
        if scene_changed && (was_mapped || (*rec).mapped) && !(*rec).state.is_null() {
            let root = surface_root_toplevel(rec);
            if !root.is_null() && (*root).window.id.0 != 0 {
                (*(*rec).state).damaged_windows.insert((*root).window.id);
            }
        }
        extensions::session_lock_surface_committed(rec);
    }
}

unsafe extern "C" fn surface_frame(
    client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    callback_id: u32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        let cb = ffi::wl_resource_create(client, &ffi::wl_callback_interface, 1, callback_id);
        if !cb.is_null() {
            (*rec).frame_callbacks.push(cb);
        }
    }
}

unsafe extern "C" fn surface_set_opaque_region(
    _client: *mut ffi::wl_client,
    surface: *mut ffi::wl_resource,
    region: *mut ffi::wl_resource,
) {
    unsafe {
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
        (*rec).pending_opaque_region = Some(value);
    }
}

unsafe extern "C" fn surface_set_input_region(
    _client: *mut ffi::wl_client,
    surface: *mut ffi::wl_resource,
    region: *mut ffi::wl_resource,
) {
    unsafe {
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
}

/// `wl_surface.set_buffer_transform` (v2+): records how the client has
/// pre-rotated the buffer. The renderer applies the inverse at composite
/// time via CPU staging (aegis-render) — a GPU-side transform in flux's image
/// shader is the long-term path.
unsafe extern "C" fn surface_set_buffer_transform(
    _c: *mut ffi::wl_client,
    r: *mut ffi::wl_resource,
    value: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(r) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        let transform = match value as u32 {
            0 => aegis_model::Transform::Normal,
            1 => aegis_model::Transform::Rotate90,
            2 => aegis_model::Transform::Rotate180,
            3 => aegis_model::Transform::Rotate270,
            4 => aegis_model::Transform::FlipHorizontal,
            5 => aegis_model::Transform::FlipRotate90,
            6 => aegis_model::Transform::FlipRotate180,
            7 => aegis_model::Transform::FlipRotate270,
            _ => {
                ffi::wl_resource_post_error(r, 1, c"invalid wl_output.transform value".as_ptr());
                return;
            }
        };
        (*rec).pending_transform = transform;
    }
}

/// `wl_surface.set_buffer_scale` (v2+): records the HiDPI scale.
unsafe extern "C" fn surface_set_buffer_scale(
    _c: *mut ffi::wl_client,
    r: *mut ffi::wl_resource,
    value: i32,
) {
    unsafe {
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
}
/// `wl_surface.damage` (v1): damage in surface-local logical coordinates. The renderer's
/// texture is in buffer pixel coords, so under buffer_scale > 1 these rects
/// cover only a fraction of the buffer. The renderer bypasses the
/// incremental-upload path when `buffer_scale != 1` (see aegis-render); a
/// generation change still triggers a correct full upload.
unsafe extern "C" fn surface_damage(
    _c: *mut ffi::wl_client,
    r: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(r) as *mut SurfaceRec;
        if rec.is_null() || w <= 0 || h <= 0 {
            return;
        }
        (*rec)
            .pending_damage
            .push(aegis_model::Rect::new(x, y, w, h));
    }
}

/// Map normal-orientation buffer damage to the surface-local coordinate space
/// shared with `wl_surface.damage`. Rounding outward preserves every touched
/// logical pixel when a HiDPI buffer rectangle is not scale-aligned.
pub(crate) fn buffer_damage_to_surface(damage: aegis_model::Rect, scale: i32) -> aegis_model::Rect {
    if scale <= 1 {
        return damage;
    }
    let scale = i64::from(scale);
    let x0 = i64::from(damage.origin.x).div_euclid(scale);
    let y0 = i64::from(damage.origin.y).div_euclid(scale);
    let buffer_x1 = i64::from(damage.origin.x) + i64::from(damage.size.w);
    let buffer_y1 = i64::from(damage.origin.y) + i64::from(damage.size.h);
    let x1 = -(-buffer_x1).div_euclid(scale);
    let y1 = -(-buffer_y1).div_euclid(scale);
    let clamp_i32 = |value: i64| value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    aegis_model::Rect::new(
        clamp_i32(x0),
        clamp_i32(y0),
        clamp_i32(x1.saturating_sub(x0)),
        clamp_i32(y1.saturating_sub(y0)),
    )
}

/// `wl_surface.damage_buffer` (v4): damage in buffer coordinates. Keep it
/// separate from surface damage until commit, when the final pending
/// scale/transform/viewport state is known.
///
/// Query the pixel dimensions of a pending `wl_buffer` so buffer-coordinate
/// damage can be geometrically transformed at commit. Returns `None` for buffer
/// types whose dimensions are not locally known (e.g. an EGL/legacy buffer the
/// compositor does not recognise), in which case damage is conservatively
/// treated as unmappable. SHM buffers expose width/height via libwayland; dma-buf
/// buffers carry them on the `DmabufBuffer` user-data.
unsafe fn pending_buffer_dimensions(buffer: *mut ffi::wl_resource) -> Option<(i32, i32)> {
    unsafe {
        let shm = ffi::wl_shm_buffer_get(buffer);
        if !shm.is_null() {
            let w = ffi::wl_shm_buffer_get_width(shm);
            let h = ffi::wl_shm_buffer_get_height(shm);
            return Some((w, h));
        }
        let is_dmabuf = ffi::wl_resource_instance_of(
            buffer,
            &ffi::wl_buffer_interface,
            &WL_BUFFER_IMPL as *const _ as *const c_void,
        ) != 0;
        if is_dmabuf {
            let db = ffi::wl_resource_get_user_data(buffer) as *const DmabufBuffer;
            if !db.is_null() && (*db).have_plane {
                return Some(((*db).width, (*db).height));
            }
        }
        None
    }
}

unsafe extern "C" fn surface_damage_buffer(
    _c: *mut ffi::wl_client,
    r: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    w: i32,
    h: i32,
) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(r) as *mut SurfaceRec;
        if rec.is_null() || w <= 0 || h <= 0 {
            return;
        }
        (*rec)
            .pending_buffer_damage
            .push(aegis_model::Rect::new(x, y, w, h));
    }
}

unsafe extern "C" fn surface_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let rec = ffi::wl_resource_get_user_data(resource) as *mut SurfaceRec;
        if rec.is_null() {
            return;
        }
        if !(*rec).xdg_toplevel.is_null() {
            defer_keyboard_focus_after_toplevel_unmap(rec);
            // ADR-0029 close transition: the wl_surface is being destroyed
            // while still mapped (no null-buffer unmap happened first).
            // Snapshot before `retire_surface_buffer` clears the contents.
            if !(*rec).state.is_null() {
                (*(*rec).state).note_close_transition(rec);
            }
        }
        if !(*rec).viewport_resource.is_null() {
            let viewport =
                ffi::wl_resource_get_user_data((*rec).viewport_resource) as *mut ViewportRec;
            if !viewport.is_null() {
                (*viewport).surface = std::ptr::null_mut();
            }
            (*rec).viewport_resource = std::ptr::null_mut();
        }
        extensions::fractional_scale_surface_destroyed(rec);
        extensions::color_management_surface_destroyed(rec);
        extensions::session_lock_surface_destroyed(rec);
        extensions::idle_inhibit_surface_destroyed(rec);
        extensions::explicit_sync_surface_destroyed(rec);
        extensions::input_popup_surface_destroyed(rec);
        extensions::xdg_foreign_surface_destroyed(rec, (*rec).state);
        retire_surface_buffer(rec);
        if (*rec).committed_acquire_fence >= 0 {
            libc_close((*rec).committed_acquire_fence);
            (*rec).committed_acquire_fence = -1;
        }
        if !(*rec).committed_explicit_release.is_null() {
            let release =
                std::mem::replace(&mut (*rec).committed_explicit_release, std::ptr::null_mut());
            ffi::wl_resource_post_event(
                release,
                ffi::ZWP_LINUX_BUFFER_RELEASE_V1_IMMEDIATE_RELEASE,
            );
            ffi::wl_resource_destroy(release);
        }
        // Drop the toplevel from its workspace (ADR-0025). Idempotent: a no-op
        // for surfaces that never mapped or had no toplevel role. Run before the
        // slot is nulled so the resource address is still readable.
        if !(*rec).state.is_null() {
            let state = &mut *(*rec).state;
            if (*rec).window.parent.is_none()
                && let Some(app_id) = (*rec).window.app_id.as_deref()
                && !app_id.is_empty()
            {
                let rect = fold_nudged_origin(
                    &*rec,
                    (*rec).saved_floating_rect.unwrap_or(aegis_model::Rect {
                        origin: (*rec).position,
                        size: (*rec).window.size,
                    }),
                );
                if rect.size.w > 0 && rect.size.h > 0 {
                    let ws_idx = state.workspace_number_for_window((*rec).window.id);

                    state.persist_app_geometry(
                        app_id,
                        rect,
                        ws_idx,
                        Some((*rec).window.layout_role),
                    );
                }
            }
            let seats = state.seats.keys().copied().collect::<Vec<_>>();
            for seat in seats {
                let Some(_guard) = ActiveSeatGuard::enter_existing(state, seat) else {
                    continue;
                };
                extensions::keyboard_shortcuts_surface_destroyed(state, resource);
                if state.pointer_focus == resource {
                    extensions::pointer_constraint_focus_changed(
                        state,
                        resource,
                        std::ptr::null_mut(),
                    );
                }
                if state.keyboard_focus == resource {
                    keyboard_focus_dependencies_changed(state, resource, std::ptr::null_mut());
                }
            }
            for runtime in state.seats.values_mut().map(Box::as_mut) {
                if runtime.cursor_surface == resource {
                    runtime.cursor_surface = std::ptr::null_mut();
                    runtime.cursor_hidden = false;
                    runtime.cursor_shape = 1;
                }
                if let Some(drag) = runtime.drag.as_mut()
                    && drag.icon == resource
                {
                    drag.icon = std::ptr::null_mut();
                }
                if runtime.pointer_focus == resource {
                    runtime.pointer_focus = std::ptr::null_mut();
                }
                if runtime.keyboard_focus == resource {
                    runtime.keyboard_focus = std::ptr::null_mut();
                }
                if runtime.tablet_focus == resource {
                    runtime.tablet_focus = std::ptr::null_mut();
                }
            }
            let id = (*rec).window.id;
            state.unregister_window(id);
            if state
                .pending_activation
                .is_some_and(|(_, pending)| pending == resource)
            {
                state.pending_activation = None;
            }
            // Session-lock focus keeps non-owning resource pointers across
            // dispatch batches. Revoke them at the surface's single reclaim
            // point so neither pre-lock restoration nor a pending lock-focus
            // transition can dereference a destroyed wl_resource.
            revoke_session_lock_focus(state, resource);
            state
                .pending_keyboard_focus
                .retain(|_, pending| pending.target != resource);
            state.workspaces.remove_toplevel(id);
            // Notify foreign-toplevel listeners the window is gone.
            if !(*rec).xdg_toplevel.is_null() {
                extensions::foreign_toplevel_removed(id.0, state);
            }
            for child in state.live_surfaces() {
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
        // reclaim the allocation. Removal is order-preserving because the Vec
        // order is the stacking order (see raise_toplevel), and the shifted tail
        // is renumbered so every live record's destroy-slot `index` keeps
        // matching its position. A long session churns surfaces (popups,
        // tooltips, drag icons); leaving a tombstone here made every frame's
        // scene walks grow with cumulative churn instead of the live count.
        if !(*rec).state.is_null() {
            let surfaces = &mut (*(*rec).state).surfaces;
            let idx = (*rec).index;
            let pos = if idx < surfaces.len() && surfaces[idx] == rec {
                Some(idx)
            } else {
                surfaces.iter().position(|p| *p == rec)
            };
            if let Some(pos) = pos {
                surfaces.remove(pos);
                for (offset, ptr) in surfaces[pos..].iter().copied().enumerate() {
                    (*ptr).index = pos + offset;
                }
            }
        }
        drop(Box::from_raw(rec));
    }
}

pub(crate) fn revoke_session_lock_focus(state: &mut State, resource: *mut ffi::wl_resource) {
    if state.pre_lock_keyboard_focus == resource {
        state.pre_lock_keyboard_focus = std::ptr::null_mut();
    }
    if state.pending_lock_focus == resource {
        state.pending_lock_focus = std::ptr::null_mut();
        state.lock_focus_dirty = true;
    }
}

// ----- wl_region ----------------------------------------------------------

static REGION_IMPL: ffi::wl_region_interface_impl = ffi::wl_region_interface_impl {
    destroy: region_destroy,
    add: region_add,
    subtract: region_subtract,
};

unsafe extern "C" fn region_destroy(_client: *mut ffi::wl_client, resource: *mut ffi::wl_resource) {
    unsafe {
        ffi::wl_resource_destroy(resource);
    }
}

unsafe extern "C" fn region_add(
    _client: *mut ffi::wl_client,
    resource: *mut ffi::wl_resource,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
) {
    unsafe {
        let region = ffi::wl_resource_get_user_data(resource) as *mut RegionRec;
        if !region.is_null() && width > 0 && height > 0 {
            (*region)
                .rects
                .push(aegis_model::Rect::new(x, y, width, height));
        }
    }
}

pub(crate) fn subtract_rect(
    source: aegis_model::Rect,
    cut: aegis_model::Rect,
) -> Vec<aegis_model::Rect> {
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
        aegis_model::Rect::new(sx1, sy1, source.size.w, cy1 - sy1),
        aegis_model::Rect::new(sx1, cy2, source.size.w, sy2 - cy2),
        aegis_model::Rect::new(sx1, cy1, cx1 - sx1, cy2 - cy1),
        aegis_model::Rect::new(cx2, cy1, sx2 - cx2, cy2 - cy1),
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
    unsafe {
        let region = ffi::wl_resource_get_user_data(resource) as *mut RegionRec;
        if region.is_null() || width <= 0 || height <= 0 {
            return;
        }
        let cut = aegis_model::Rect::new(x, y, width, height);
        (*region).rects = std::mem::take(&mut (*region).rects)
            .into_iter()
            .flat_map(|rect| subtract_rect(rect, cut))
            .collect();
    }
}

unsafe extern "C" fn region_resource_destroy(resource: *mut ffi::wl_resource) {
    unsafe {
        let region = ffi::wl_resource_get_user_data(resource) as *mut RegionRec;
        if !region.is_null() {
            drop(Box::from_raw(region));
        }
    }
}
