# ADR-0012: Toplevel metadata and state machine (M3 partial)

- Status: Accepted
- Date: 2026-06-18

## Context

Before this ADR, `xdg_toplevel` requests other than `destroy` were all no-ops.
Clients called `set_title` / `set_app_id` / `set_min_size` / `set_max_size` /
`set_parent` and the server discarded the arguments. `set_maximized`,
`unset_maximized`, `set_fullscreen`, and `unset_fullscreen` did nothing, and
no `xdg_toplevel.configure` event with state bits was ever sent after the
initial empty one. `XDG_TOPLEVEL_CLOSE` was defined but never posted, so
chrome had no way to ask a client to close.

This made ass unusable for any client that depends on state feedback
(browsers maximizing, video players going fullscreen, terminals reading their
own title back from the compositor) and left the shell with no API to drive
window lifecycle.

## Decision

### 1. Window model in `aegis-core`

A new `ass_core::window` module owns the protocol-agnostic view of a toplevel:

- `Window { id, title, app_id, parent, size_hints, state }` — the metadata
  the shell and the future introspection API read.
- `WindowState { maximized, fullscreen, activated }` — the bits advertised
  to the client via the `xdg_toplevel.configure.states` array. The
  `to_state_array()` method serializes to the protocol's enum order
  (maximized=1, fullscreen=2, activated=4) skipping unset bits.
- `SizeHints { min_w, min_h, max_w, max_h }` — `0` means unconstrained, per
  the protocol.

The model lives in `aegis-core` so the shell can read it without a server
dependency, matching the architecture's existing split between server-owned
surface state and renderer-owned compositing.

### 2. Server owns one `Window` per toplevel

`SurfaceRec.window` is initialized when `xdg_surface.get_toplevel` is called,
keyed by the surface record's address (matching `SurfacePixels.id` and
`SurfaceDmabuf.id` so the shell can correlate the rendered frame with the
window metadata).

### 3. Real handlers for metadata requests

`set_title`, `set_app_id`, `set_min_size`, `set_max_size`, and `set_parent`
now write into the `Window`. None of them require a response configure;
clients do not expect one for metadata-only updates.

### 4. State transitions emit a state-bit configure

`set_maximized`, `unset_maximized`, `set_fullscreen`, and
`unset_fullscreen` flip the corresponding bit and call a shared
`reconfigure_with_state` helper that:

1. Serializes the current `WindowState` into a `wl_array` of `u32` values
   (one per active state, in protocol enum order).
2. Posts `xdg_toplevel.configure` with `width=0, height=0` ("you pick") and
   the states array.
3. Posts `xdg_surface.configure` with a fresh serial so the client acks the
   new state.

The `wl_array` is built from a `malloc`'d buffer that is freed immediately
after the post; libwayland copies the contents into the event marshalling
buffer, so the caller-owned storage is short-lived.

### 5. New `Server` API for the shell

- `windows()` — snapshot of all live toplevel windows; the chrome overview
  and any introspection consumer reads from this.
- `close_toplevel(surface_id)` — posts `xdg_toplevel.close`. The client is
  expected to destroy its `xdg_toplevel` (and usually the surface) in
  response.
- `set_toplevel_activated(surface_id, activated)` — flips the activated bit
  and reconfigures. Complements keyboard focus enter/leave with the
  toplevel-state side, so clients can render their title bar differently
  when focused.

## Alternatives

- **A separate `aegis-core::WindowManager` owning placement policy.** Deferred:
   M3 partial is about state correctness, not policy. The placeholder
   cascade placement from M2 remains until a real window manager lands.
- **Server-side decorations in this phase.** Deferred: SSD needs
   `zxdg_decoration_manager_v1` and flux-ui widgets for title bars; that is
   shell work tracked separately.
- **Interactive `move` / `resize`.** Deferred: those need a pointer-state
   machine that tracks the implicit grab from button press through release,
   with serial validation against the seat. Implementing them without
   interactive test infrastructure would risk subtle bugs (lost grabs,
   serial races).
- **`show_window_menu` and `set_minimized`.** Accepted but inert until the
   shell grows a window-list / window-menu surface.

## Consequences

- Clients that maximize / un-maximize / fullscreen now reconfigure
  correctly. The compositor advertises the change and the client acks.
- The shell has the API it needs to enumerate windows, request close, and
  drive activation — enough to build a basic overview or task bar without
  further server changes.
- The activated bit is **not** automatically driven by keyboard focus yet;
  the shell must call `set_toplevel_activated` on focus changes for the
  client to learn its activated state. M3 polish could wire this directly
  into `change_keyboard_focus`.
- Interactive move and resize remain no-ops. Clients that call them get no
  response and the window does not move; users cannot drag or resize
  windows from the chrome. That is the largest remaining gap before M3 is
  usable as a daily driver.
- Server-side decorations and overview launcher are still unbuilt; their
  absence is the other half of M3 not covered here.
