# ADR-0145: Drag tear-off dynamic re-anchoring for maximized and fullscreen toplevels

- Status: Accepted
- Date: 2026-04-18

## Context

When a toplevel window is maximized or fullscreen, its geometry matches the output/work area, and the original floating geometry is stored in `SurfaceRec.saved_floating_rect`.

Previously, initiating an interactive move on a maximized or fullscreen window (via CSD `xdg_toplevel.move`, SSD top-border drag, or compositor Super+drag) cleared the maximize/fullscreen state bit and invoked `reconfigure_with_state`. That helper unconditionally restored the saved floating position (`saved.origin`).

This created a severe user experience flaw during drag gestures: if the window was unmaximized by dragging from a point distant from `saved.origin` (such as the right half of a wide display), the window immediately snapped to its old floating coordinates far away from the cursor. The cursor would end up outside the window bounds during the drag, breaking continuity and spatial affordance.

Conversely, double-clicking to restore without dragging expects the exact historical position and size to be restored.

## Decision

We distinguish the two user intents and implement proportional dynamic re-anchoring for tear-off drag operations:

### 1. Pure restoration on double-click release
Double-clicking without dragging (or programmatic unmaximize requests) continues to restore `saved.origin` and `saved.size` verbatim via `apply_state_geometry`.

### 2. Proportional re-anchoring on tear-off drag (`reanchor_tear_off`)
When an interactive move is initiated on a maximized or fullscreen window:
- **Horizontal ratio**: The cursor's relative horizontal position $\alpha_x = \frac{X_{\text{cursor}} - X_{\text{max}}}{W_{\text{max}}}$ is computed and clamped to $[0.0, 1.0]$. The floating window's new horizontal origin is placed at:
  $$X_{\text{new}} = X_{\text{cursor}} - \alpha_x \times W_{\text{float}}$$
  This guarantees that the cursor remains at the exact same horizontal percentage across the floating window.
- **Vertical alignment**: For grab offsets within the floating height (such as title bar drags), the absolute vertical grab depth $\Delta Y = Y_{\text{cursor}} - Y_{\text{max}}$ is preserved:
  $$Y_{\text{new}} = Y_{\text{cursor}} - \Delta Y$$
  For deep grabs (e.g. Super+drag in fullscreen mode below the floating height), the vertical position scales proportionally to keep the cursor safely inside the window bounds.

### 3. Unified entry point (`begin_toplevel_interactive_move`)
All paths that begin an interactive move (`xdg_toplevel.move`, `start_interactive_move`, `start_interactive_move_for_seat`, and `start_mirror_move`) share `begin_toplevel_interactive_move`. It atomically consumes `saved_floating_rect`, repositions the surface and popups to the re-anchored origin, posts the floating `xdg_toplevel.configure`, and starts `Interactive::Move` anchored at the recomputed origin.

## Alternatives

- **Snap to center of title bar**: Always placing the center of the restored title bar under the cursor. Rejected because dragging from the left or right edge of a wide maximized window would cause an abrupt jump toward the center.
- **Do not unmaximize on drag**: Requiring users to unmaximize explicitly before moving. Rejected because drag-to-unmaximize (tear-off) is standard desktop behavior across modern Wayland and desktop environments.

## Consequences

- Dragging a maximized or fullscreen window via CSD, SSD top-border drag, or Super+drag cleanly transitions the window to floating mode with zero cursor drift.
- Popups attached to the toplevel stay synchronized during the tear-off transition.
- Double-click restore remains bit-exact to the pre-maximization state.
