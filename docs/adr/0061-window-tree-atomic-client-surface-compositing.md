# ADR-0061: Window-tree-atomic client surface compositing

- Status: Accepted
- Date: 2026-07-28

## Context

[ADR-0011](0011-subsurface-tree-and-z-split-rendering.md) introduced the
server-owned subsurface tree and split client frames into global
below-parent, xdg-role, and above-parent passes. That split preserves a
subsurface's position relative to its own parent, but it does not preserve
the boundary between overlapping windows.

In a bottom-to-top stack containing windows A and B, the global passes paint
A's above-parent subsurfaces after B's toplevel. Client-side decorations
commonly use above-parent subsurfaces, so A's title bar can appear through
foreground window B. Separating shm and dma-buf subsurface passes can cause
the same inversion between backing types.

The server already owns parent relationships, sibling order, popup ownership,
and toplevel z-order. The renderer does not need that Wayland state, but it
does need one authoritative order covering every frame payload.

## Decision

The compositor emits one flat client-surface paint order in addition to the
shm and dma-buf frame payloads. The order contains resource identifiers and
is constructed as follows:

1. Traverse toplevels from bottom to top.
2. For each toplevel, emit its below-parent subsurface subtrees, the
   toplevel surface, and its above-parent subsurface subtrees.
3. Emit each owned xdg-popup and its complete subsurface tree before
   advancing to the next toplevel.
4. Traverse every subsurface subtree recursively in protocol sibling order.

The renderer interleaves shm and dma-buf payloads against that single order.
Physical output, overview rendering, and directed Realm rendering use the
same contract. Compositor-owned overlays remain a separate pass above the
complete client scene.

The server continues to own all Wayland tree semantics and absolute geometry.
The renderer remains stateless about parent relationships.

## Alternatives

- **Retain global below, root, and above passes.** Rejected because an
  above-parent surface is only above its own parent, not every other window.
- **Clip lower-window subsurfaces against higher windows.** Rejected because
  clipping treats a paint-order defect as geometry, adds redundant work, and
  does not solve translucent overlap or mixed backing order.
- **Teach the renderer the Wayland surface tree.** Rejected because protocol
  ownership belongs in the compositor server and would duplicate mutable
  scene state across crates.
- **Draw shm and dma-buf surface trees separately.** Rejected because backing
  storage does not define z-order.

## Consequences

- A toplevel, its client-side decorations, nested subsurfaces, and popups are
  occluded as one desktop stacking unit.
- Mixed shm and dma-buf surfaces preserve protocol paint order.
- Desktop, overview, and Realm output cannot diverge in their client-scene
  ordering policy.
- New client surface classes must join the authoritative order before the
  renderer draws their payloads.
- The legacy split frame accessors remain available as payload-building
  primitives, but callers must not composite them as global z layers.
