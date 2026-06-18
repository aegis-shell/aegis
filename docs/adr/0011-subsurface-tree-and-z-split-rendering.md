# ADR-0011: Subsurface tree model and z-split rendering

- Status: Accepted
- Date: 2026-06-18

## Context

Before M2, `wl_subcompositor.get_subsurface` accepted the request and created
a `wl_subsurface` resource, but discarded both its `surface` and `parent`
arguments. The server had no parent/child relationship, `set_position` was
silent, `place_above` / `place_below` were no-ops, and the renderer only
walked the flat list of toplevels. Clients that bound subsurfaces (GTK
subwindows, mpv OSD, Firefox popup-internal surfaces) committed buffers that
never reached the output.

## Decision

The subsurface tree is modeled directly on `SurfaceRec`, and the server
emits four subsurface frame lists per frame so the renderer can draw them
around their parent toplevel without learning about the tree.

1. **`SurfaceRec` gains `parent: *mut SurfaceRec` and
   `children: Vec<*mut SurfaceRec>`**, plus `subsurface_offset: Point` and
   `subsurface_above_parent: bool`. The shared rec is the user-data for both
   the `wl_surface` and the `wl_subsurface` resource, so handlers reach the
   same state either way.

2. **`get_subsurface` links parent and child.** The handler reads both
   resource user-data pointers, sets `child.parent = parent`, pushes the
   child pointer onto `parent.children`, and installs `SUBSURFACE_IMPL`
   with the child rec as user-data.

3. **Real handlers for `set_position`, `place_above`, `place_below`.**
   `set_position` writes the offset onto `subsurface_offset`.
   `place_above` / `place_below` flip `subsurface_above_parent`. Sibling
   relative reordering within a group is not modeled in M2 — the boolean
   splits children into below-parent and above-parent draws, which covers
   the common cases (panel below main view, popup above).

4. **Destroy detaches the link from both ends.** `subsurface.destroy` and
   the surface's destroy notify both call `detach_from_parent`, which
   removes the child from the parent's `children` Vec. A surface being
   destroyed also nulls the `parent` field of each of its children so they
   do not keep a dangling pointer; orphaned children stay in the surfaces
   list and may be re-parented.

5. **Server emits four lists per frame:** `subsurface_frames_below`,
   `subsurface_frames_above`, `subsurface_dmabuf_frames_below`, and
   `subsurface_dmabuf_frames_above`. Each entry carries an *absolute*
   position computed as `parent.position + child.subsurface_offset`, so the
   renderer treats subsurfaces exactly like toplevels for the draw call.

6. **The main loop interleaves draws in z-order:** below-subsurfaces,
   toplevels, above-subsurfaces (for both shm and dmabuf lists). The
   renderer's per-id texture cache is shared across all four.

7. **M2 surfaces only direct children of mapped toplevels.** Nested
   subsurface-of-subsurface chains are deferred. The protocol allows them;
   the implementation walks only one level of the tree. This is enough for
   every real client we have tested, all of which attach subsurfaces
   directly under a toplevel.

## Alternatives

- **Walk the full tree recursively in the renderer.** Rejected: the
   renderer would have to learn about parent/child relationships, inverting
   the architecture's split between server-owned surface model and
   renderer-owned compositing. Emitting flat lists with precomputed
   positions keeps the renderer stateless about the tree.
- **Track sibling order in `children` for true `place_above` / `place_below`
   compliance.** Deferred: the protocol's `place_above(sibling)` requests
   reordering relative to a named sibling. M2's boolean split handles the
   common cases; full sibling order is a polish item that lands with
   interactive-move testing.
- **Sync-mode cascade.** Deferred: `wl_subsurface.set_sync` is accepted but
   treated as `set_desync`. Real sync mode requires pending-state tracking
   per surface and a cascade on parent commit; that's a separate piece of
   work tied to interactive window management.

## Consequences

- Clients that use subsurfaces for panels, popups, OSD overlays, and
  drag-and-drop icons now see their buffers composite in the right place
  and stacking.
- A subsurface that attaches and commits a buffer appears on its parent's
  next frame. There is no protocol event telling the client "you are now
  visible"; the client's frame callback fires as for any surface.
- Subsurface hit-testing is not wired: the focus model only considers
  toplevels. A pointer motion over a subsurface will not change focus to
  that subsurface's client. This matters for subsurface-internal widgets
  (e.g. a popup's text field); tracked as Phase 5 input work.
- Sync-mode cascade is documented as not implemented. Clients that rely on
  sync mode (rare in practice — most use desync or commit-and-pray) will
  see their state apply on their own commit rather than their parent's.
- The full subsurface tree is still one level deep. Nested subsurfaces do
  not composite. Real compositors handle this; ass defers it.
