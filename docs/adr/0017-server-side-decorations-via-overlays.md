# ADR-0017: Server-side decorations via flux-ui overlays

- Status: Superseded by ADR-0063
- Date: 2026-06-18

## Context

[ADR-0016](0016-shell-server-window-management-bridge.md) shipped a
window-list panel in the chrome but no per-window chrome around the
client surfaces themselves. Without server-side decorations (SSD), every
toplevel appears as a bare buffer with no title bar, no close gadget, and
no visual indication of which window has focus (other than the side
panel's `▶` marker).

The blocker for SSD was the absence of an absolute-positioning widget in
flux-ui: the existing chrome uses `Frame::column` and `row`, which
auto-layout from the top-left of the output. Drawing a title bar at an
arbitrary `(x, y)` per window requires either raw `flux::Canvas`
primitives (bypassing flux-ui's input handling) or a new flux-ui API.

## Decision

### 1. Use `Frame::overlay(id, anchor: Rect, opts, body)`

flux-ui already exposes an overlay API for popups, dropdowns, and
tooltips. The anchor is an absolute `Rect` in canvas pixel space; the
overlay's body runs as a normal `Frame` layout (column/row/widgets) but
positioned at the anchor. Input hit-testing is handled by flux-ui
automatically.

For SSD, each mapped toplevel gets one overlay anchored just above its
surface position:

```text
bar_rect = Rect {
    x: window.position.x,
    y: window.position.y - TITLE_BAR_HEIGHT,
    w: window.size.w,
    h: TITLE_BAR_HEIGHT,
}
```

### 2. Visual differentiation by activation state

`OverlayOpts` carries `bg`, `border`, `border_width`, and `radius`. The
title bar's `bg` is a brighter color when `window.state.activated` is
true, giving the user a focus indicator at the window itself (not just
in the side panel).

### 3. Title bar interactions

Inside the overlay body, a row layout holds:

- A `selectable(label, false)` that grows with `flex(1.0)` to fill the
  title area. Click returns `true`, which sets `move_requested = Some(id)`.
- A `button("x")` sized to `CLOSE_BUTTON_WIDTH × TITLE_BAR_HEIGHT - 8`
  for the close gadget. Click sets `closed_window = Some(id)`.

The shell exposes `take_move_requested()` and reuses the existing
`take_closed_window()` drain. The main loop forwards `move_requested`
to `Server::start_interactive_move`, which begins an interactive move
grab without serial validation (compositor-initiated, like the existing
shell-driven grab API from ADR-0013).

### 4. Constant frame geometry

`TITLE_BAR_HEIGHT = 24.0` and `CLOSE_BUTTON_WIDTH = 24.0` are module-level
constants in `aegis-shell`. Real compositors expose these as
`xdg_toplevel_window_geometry` — the client's `set_window_geometry`
request is supposed to account for them. M3 leaves that protocol
interaction unimplemented; the constants are visual-only.

## Alternatives

- **Draw decorations via raw `flux::Canvas` primitives.** Rejected: would
   bypass flux-ui's input handling. Hit-testing the title bar would have
   to be re-implemented in aegis-shell, duplicating flux-ui's work.
- **Add a new `Frame::absolute_position(x, y, body)` API to flux-ui.**
   Rejected as redundant: `overlay` already does this and supports
   popups, dropdowns, and now SSD from the same primitive.
- **Honour `xdg_toplevel.set_window_geometry` for frame insets.** Deferred:
   requires the server to subtract frame extents from the client's
   committed buffer region. The current visual layer (title bar drawn
   above the surface) is correct without that; the protocol-level
   integration is polish.

## Consequences

- Each mapped toplevel now has a title bar above it showing its title
  (or `<untitled>`), with a close gadget at the right. Activated windows
  get a brighter bar.
- Clicking the title area starts an interactive move via the existing
  `Server::start_interactive_move` grab; pointer motion then moves the
  window.
- Clicking the close gadget posts `xdg_toplevel.close` via the existing
  `Server::close_toplevel` API.
- **The click that initiates a move is also forwarded to the client** as
  a `wl_pointer.button` event, because the shell's input handling runs
  during the frame render, after `Server::forward_input`. The client
  should ignore presses outside its surface; in practice CSD-aware
  clients do, and pure-SSD clients have no non-client area to confuse.
  Properly consuming the click before the server sees it requires
  restructuring the main loop's input routing; tracked as future polish.
- The title bar is drawn at `window.position.y - TITLE_BAR_HEIGHT`. For
  windows placed near the top of the output the bar may extend offscreen;
  the cascade placement policy (from M2) starts at `y = 60`, so this is
  not a problem in practice.
- Frame extents are visual constants; the protocol-level
  `set_window_geometry` integration is not implemented. CSD-aware
  clients continue to draw their own decorations if they prefer.
- Touch and resize-from-border are not wired. Resize is available only
  via client-initiated `xdg_toplevel.resize` (e.g. a corner drag handle
  the client draws in CSD mode).
