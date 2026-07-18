# ADR-0009: Input pipeline and pointer focus model

- Status: Superseded by [ADR-0040](0040-realms-seats-and-transferable-interaction-authority.md)
- Date: 2026-06-18

## Context

Milestone M1 needs input routed to a focused client. The pieces in place after
Phases 0–2 were:

- A `Backend` trait with a `take_input` accessor ([ADR-0007](0007-logging-and-backend-input-contract.md))
- A `Vec<InputEvent>` drain seam in the nested backend
- A server that advertised zero seat capabilities and stubbed `get_pointer`
- A main loop that constructed `flux_ui::Input::default()` once outside the loop
  and never updated it

None of those actually moved input. The nested backend never bound the host
`wl_seat`; the server never created pointer resources; the shell's Quit button
was permanently unclickable.

## Decision

The input pipeline is built in four layers, each owning a single concern.

1. **Backend translation.** The nested backend binds the host `wl_seat` at v4
   and installs `wl_seat_listener` and `wl_pointer_listener`. Host pointer
   events (`enter`, `leave`, `motion`, `button`) are translated to
   backend-agnostic `InputEvent` values and buffered into `state.input_events`.
   `Backend::take_input` drains the buffer. The host's `axis` (scroll) event
   is accepted but not yet translated — that is M2 work.

2. **Server state and resource tracking.** The server's `State` gains
   `pointer_resources: Vec<*mut wl_resource>`, `pointer_focus`, and the
   current pointer position. `wl_seat` advertises pointer capability
   (`caps = WL_SEAT_CAPABILITY_POINTER`). `seat_get_pointer` creates a real
   `wl_pointer` resource, tracks it in `state.pointer_resources`, and installs
   a destroy notify that nulls the slot when the client releases it. The same
   lifecycle pattern as surfaces
   ([ADR-0006](0006-ffi-soundness-discipline.md)) is reused.

3. **Focus model and hit-test.** A single `pointer_focus` tracks the surface
   resource currently under the cursor. `Server::forward_input` drives
   transitions: motion updates the cached position, runs a hit-test against
   mapped toplevels, and on focus change posts `wl_pointer.leave` to the old
   client's pointer resources and `wl_pointer.enter` to the new client's.
   Button events go to the focused client's pointer resources with a fresh
   serial. Pointer focus is **exclusive** in M1: at most one client receives
   pointer events at a time. Multiseat and split focus are deferred.

4. **Shell mirror.** The main loop drains backend events once per frame and
   mirrors them into `flux_ui::Input` *before* handing them to the server.
   This lets chrome intercept clicks (the Quit button) before they reach a
   client. Pointer motion sets the cursor position; Linux `BTN_*` codes are
   mapped to `MouseButton::{Left,Right,Middle}`.

The placeholder surface geometry for hit-test is a diagonal cascade
(`(60+i*32, 60+i*32)`), matching the renderer's existing stacking offset.
Phase 4 replaces this with real per-surface geometry from
`xdg_surface.set_window_geometry`, at which point hit-test and rendering draw
from the same source of truth.

## Alternatives

- **Per-client wl_pointer resource lookup via a `wl_client` walk.** Rejected:
   libwayland's API for enumerating clients is awkward and O(N); keeping our
   own `pointer_resources` Vec with destroy-notify cleanup is the same
   pattern surfaces already use and gives O(1) fan-out per event.
- **Hit-test in the renderer.** Rejected: the renderer has no knowledge of
   clients or surfaces, only of pixels. Hit-test belongs in the server, which
   owns the surface model.
- **Multiseat from day one.** Rejected: M1 has one pointer and one keyboard;
   the single-focus model is simpler to verify. Multiseat comes when DRM/KMS
   arrives and a single compositor instance may host more than one seat.
- **Keyboard in this phase.** Deferred: a real `wl_keyboard` requires a
   keymap (xkbcommon compile of the default rules) and modifier-state
   tracking through `xkb_state`. That is its own phase of work; pointer-only
   M1 is enough to prove the pipeline and ship a usable test target.

## Consequences

- Pointer motion and buttons reach the focused client end-to-end. The shell's
  Quit button starts working.
- Keyboard capability is not advertised in M1, so conforming clients do not
  request a keyboard. The `get_keyboard` handler remains a defensive inert
  stub.
- Scroll wheel events arrive at the backend but are dropped; clients that
  depend on scrolling (browsers, terminals) will not scroll under ass until
  M2 wires `wl_pointer.axis` and `axis_value120`.
- The hit-test geometry is a placeholder and disagrees with where surfaces
  appear in the output for any non-cascaded client. Phase 4's geometry work
  makes the two consistent.
- A real client integration test (a `wayland-client`-based test harness that
  connects, binds the seat, and asserts receipt of motion/button events) is
  identified as future test infrastructure; it does not block Phase 3
  acceptance because no Linux distribution ships a stock `weston-simple-shm`
  equivalent the test could call.
