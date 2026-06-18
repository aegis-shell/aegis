# ADR-0020: Apply buffer_scale at composite time

- Status: Accepted
- Date: 2026-06-18

## Context

[ADR-0014](0014-buffer-transform-and-viewport-crop.md) landed a real
`wl_surface.set_buffer_scale` handler that stored the value on
`SurfaceRec::pending_scale`, but explicitly left the renderer untouched: the
value was read into a dead local at commit (`let _ = applied_scale;`). The
stated plan was to wait for the GPU-side transform path in flux before doing
anything with scale.

Two facts have since made that deferral unnecessary:

1. **Scale and transform are independent.** `buffer_transform` needs a
   matrix push-constant or per-vertex UVs because it reshuffles *which
   source pixel lands at which destination pixel*. `buffer_scale` only
   rescales destination dimensions — it is a uniform divide of `(dst_w,
   dst_h)` by the scale factor and has no shader implications at all. The
   CPU staging path for transforms is unaffected.

2. **`SurfaceGeometry` already carries the field.** It was added in
   ADR-0014's geometry plumbing (`buffer_scale: i32`) but never populated
   by the four `toplevel_*_frames` construction sites — they used
   `..Default::default()` for it. Closing the gap is a one-field wiring
   plus a divide at the draw call.

Without this, a HiDPI client (anything that calls `set_buffer_scale(2)`
and commits at 2× resolution) renders at 2× its intended on-screen size —
visible breakage for any real client on a HiDPI output.

## Decision

Apply `buffer_scale` at composite time, in three small changes:

1. **`SurfaceGeometry::buffer_scale` defaults to 1**, not the `i32`
   default of 0. Replaced the derived `Default` with a manual impl that
   sets `buffer_scale: 1`. A partially populated geometry stays unscaled
   rather than dividing by zero.

2. **Plumb `pending_scale` through all four geometry construction sites**
   in `ass-server` (shm toplevels, dmabuf toplevels, shm subsurfaces,
   dmabuf subsurfaces). The dead `applied_scale` local at commit is
   removed; the comment that explained the deferral is gone with it.

3. **The renderer divides destination dimensions by scale** when
   `viewport_dst` is unset, mirroring `weston_surface_update_size`:
   - `viewport_dst` set: taken as-is (already in logical pixels).
   - `viewport_dst` unset, `viewport_src` set: source size / scale.
   - both unset: post-transform buffer dims / scale.

   The computation is factored into a pure `destination_size` helper in
   `ass-render` so the shm and dm-buf paths share one tested code path,
   and so the scale/viewport matrix is exercisable without a flux device.

The source UV rect computation is unchanged because `viewport_src` is
expressed in post-transform buffer pixel coords — scale affects only the
destination, not the source sampling.

### Damage interaction

The incremental-upload path (`flux::Image::update_region` per damaged
rect) is bypassed when `buffer_scale > 1`, mirroring the existing
`transform != Normal` bypass. Rationale: the server's `pending_damage`
Vec interleaves rects from `wl_surface.damage` (surface-local coords)
and `wl_surface.damage_buffer` (buffer coords) without distinction; under
scale > 1 they are in different coordinate spaces and the renderer cannot
tell them apart. The full-upload path (taken on every generation change)
remains correct. This narrows but does not close the gap
[ADR-0015](0015-damage-tracking.md) flagged.

## Alternatives

- **Wait for the flux GPU transform path before touching scale.**
  Rejected: the two are independent. Scale is a destination-size divide
  with no shader or staging implications; coupling it to the (still
  deferred) flux work would leave HiDPI broken indefinitely for no
  benefit.

- **Fix `wl_surface.damage` to multiply rects by `pending_scale` at
  queue time, normalising everything to buffer coords.** Deferred: it
  touches the damage bookkeeping and is orthogonal to the composite-time
  fix. Once done, the `buffer_scale > 1` bypass in the renderer can be
  lifted. Recorded as the natural follow-up.

- **Add `wp_fractional_scale_manager_v1` alongside this.** Out of scope:
  fractional scale is a separate protocol that lets clients render at
  non-integer scale factors. It builds on top of working integer scale,
  so this ADR is a prerequisite, not a substitute.

## Consequences

- HiDPI clients that commit at integer scale now composite at the
  intended on-screen size on both the shm and dma-buf paths.
- The damage/scale interaction documented in ADR-0015 becomes a real
  (rather than hypothetical) correctness gap for incremental uploads.
  Mitigated by bypassing the incremental path under `buffer_scale > 1`;
  full uploads remain correct. Lifting the bypass is a follow-up tied to
  normalising damage coords in the server.
- Subsurface `pending_scale` is now plumbed end-to-end too. Subsurfaces
  of a HiDPI toplevel that themselves set `buffer_scale` will composite
  at the right size; subsurfaces that omit it default to scale 1.
