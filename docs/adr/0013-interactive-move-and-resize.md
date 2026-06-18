# ADR-0013: Interactive move and resize

- Status: Accepted
- Date: 2026-06-18

## Context

[ADR-0012](0012-toplevel-metadata-and-state-machine.md) shipped the metadata
and state-machine half of M3 but left interactive `xdg_toplevel.move` and
`xdg_toplevel.resize` as no-ops. Without them, users cannot position or size
windows from the chrome: the placeholder diagonal cascade from M2 is the only
placement, and once mapped a window's size is whatever the client first
committed.

Interactive move/resize in Wayland is driven by the compositor: the client
calls `xdg_toplevel.move(seat, serial)` or `resize(seat, serial, edges)` in
response to a pointer button press; the compositor validates the serial
against the press that triggered the call, starts a "grab," and then
interprets subsequent pointer motion as window geometry changes until the
button releases.

## Decision

### 1. Model in `ass-core::window`

Two new types live alongside the `Window` model:

- `ResizeEdges(u32)` — a thin wrapper around the protocol's edge enum with
  axis-bit accessors (`has_top`, `has_bottom`, `has_left`, `has_right`).
- `Interactive` — a sum type describing the active grab:
  - `Move { window_id, origin, start_position }`
  - `Resize { window_id, edges, origin, start_position, start_size }`

Putting these in `ass-core` (rather than in `ass-server`) means the shell
and the introspection API can observe the grab state and, for example, draw
a different cursor or pause overview animations while one is in progress.

### 2. Server-side grab state

`State.interactive: Option<Interactive>` holds the active grab. It is
populated by `toplevel_move` / `toplevel_resize` when the supplied serial
matches `last_button_serial`, and cleared by `pointer_button` on release.

### 3. Serial validation

The grab is started only if the request's serial matches the most recent
pointer-button serial the server posted. This is the protocol's defense
against clients initiating grabs out of band — e.g. capturing the pointer
without an actual user click. A mismatched serial silently ignores the
request (the spec does not require a protocol error).

### 4. Pointer-motion interpretation

`Server::pointer_motion` checks for an active grab before hit-testing:

- If a grab is active, the motion is interpreted as a window-geometry
  update via `apply_interactive_motion`. For `Move`, the new position is
  `start_position + (current - origin)`, rounded to integer logical pixels.
  For `Resize`, the size grows or shrinks along the edges in the request
  (right/bottom grow the size; left/top grow the size *and* move the
  opposite-corner anchor). The client's `wl_pointer.motion` is still
  forwarded so its button state stays consistent.
- If no grab is active, motion takes the normal path: hit-test, possibly
  change pointer focus, post `wl_pointer.motion` to the focused client.

### 5. Resize clamping and configure

The new size is clamped against the toplevel's `size_hints` (`set_min_size`
/ `set_max_size`) with a 1×1 floor. If a max constraint pulls a growing
edge back, the opposite-side anchor moves to keep that edge pinned (the
behaviour users expect from tiling and floating WMs alike).

After clamping, the server posts `xdg_toplevel.configure(width, height,
states)` and `xdg_surface.configure(serial)`. The client reallocates its
buffer at the new size and commits; the renderer picks up the new
geometry from the next frame's `SurfacePixels`.

### 6. Button release ends the grab

`Server::pointer_button` on a release edge clears `State.interactive`. The
release event itself is still forwarded to the client so its pointer state
stays consistent.

## Alternatives

- **Client-side move/resize only.** Rejected: xdg-shell explicitly puts
  interactive geometry in compositor hands so the grab is robust against
  slow clients. A client lagging behind the pointer would otherwise drop
  updates and the window would jitter.
- **Frame-based move/resize (title bar drag, border drag).** Deferred:
  server-side decorations are tracked as separate Phase 5 work. When SSD
  lands, it will call the same `toplevel_move` / `toplevel_resize` paths
  internally; the grab mechanism is unchanged.
- **Concurrent grabs.** Rejected: a single `interactive: Option<Interactive>`
  enforces one grab at a time. Multiseat would require one grab per seat;
  M3 has one seat, so this is sufficient.

## Consequences

- Windows can now be moved and resized interactively. A pointer-button
  press in the client that triggers `xdg_toplevel.move` or `resize`
  starts the grab; subsequent pointer motion updates the window; release
  ends it.
- The grab is compositor-driven, so a slow client does not cause the
  window to stutter — the server updates `position` directly each motion
  event, and the renderer reads it on the next frame.
- Resize sends a configure event per motion event during the grab. Clients
  that reallocate slowly may queue buffers behind the geometry change;
  this is the standard Wayland pattern and clients are expected to handle
  it.
- `xdg_toplevel.show_window_menu` and `set_minimized` remain inert;
  decorations, overview, and task-list are still separate Phase 5 work.
- The grab state is observable via `ass_core::window::Interactive`, so a
  future shell can change the cursor (e.g. to a resize caret) while a
  grab is active.
