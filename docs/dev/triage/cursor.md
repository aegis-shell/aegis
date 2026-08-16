# Cursor Issue Triage

How to attribute a cursor appearance or behavior bug between the client
and the compositor before changing any code, and what to do with the
result. The rule throughout: fix what aegis gets wrong, never paper over
what a client gets wrong.

## Who draws the cursor

Three sources can produce the pixels under the pointer:

- **`wp_cursor_shape_manager_v1`** — the client asserts a shape
  (`default`, `pointer`, `text`, ...) after each `wl_pointer.enter`;
  aegis rasterizes the matching themed cursor itself. A wrong *image*
  (theme, size, hotspot) here is compositor-side; a wrong *shape value*
  is whatever the client requested.
- **`wl_pointer.set_cursor`** — the client supplies its own cursor
  surface; aegis only positions and composites it. The image is entirely
  client content.
- **Compositor-owned overlays** — interactive move/resize and shell
  chrome assert their own shapes, bypassing client state.

## Server-side invariants

The drawn cursor changes only when one of these fires:

1. The focus client issues `set_shape` or `set_cursor` carrying the
   serial of the latest `wl_pointer.enter` it received.
2. Pointer focus moves to a surface of a *different* client, or to no
   surface; the cursor falls back to the default arrow.
3. The client's `set_cursor` surface is destroyed; fallback to the
   default arrow.
4. Seat-level state is torn down (interaction-domain revocation,
   session lock).

Two deliberate behaviors matter for triage:

- Same-client focus transitions — including context-menu popup to
  submenu popup — keep the last-asserted cursor until the client
  re-asserts. A cursor that snaps to the arrow across such a transition
  *without* a client request is a compositor bug.
- Mapping a popup re-hit-tests the stationary pointer. When the pointer
  is over an already-mapped surface of the same client, focus and cursor
  state do not change at all.

## Diagnostic recipe

Run the affected client with protocol debugging and reproduce:

```bash
WAYLAND_DEBUG=1 <client> 2>&1 | grep -E 'wl_pointer\.set_cursor|set_shape'
```

Watch the exact moment the cursor changes unexpectedly:

- The client issued `set_shape(serial, default)` or `set_cursor` with
  an arrow surface → **client-side**. The compositor honored a valid
  request.
- The client issued nothing and the cursor changed →
  **compositor-side**. Save the full trace and file it; do not patch
  around it.

Quick symptom table:

| Symptom | Likely owner | Why |
|---------|--------------|-----|
| Cursor flips to the arrow when a submenu opens while the pointer stays on the parent item (Qt/Telegram menus) | Client | Qt popup windows grab the mouse at the widgets level; the cursor is re-evaluated against the grabbing window, where the mapped position hits no item, so Qt asserts the default cursor. See the worked example below. |
| Cursor stuck as the arrow after entering a surface | Client | The client must assert a cursor on every `wl_pointer.enter`; aegis can only preserve or reset, never invent one. |
| Wrong cursor image (theme, size, hotspot) for a shape-protocol client | Compositor | aegis owns rasterization of themed shapes. |
| Cursor flickers or resets when crossing two surfaces of the *same* client | Compositor | Same-client transitions must preserve the cursor; see the invariants above. |
| Cursor is stale or wrong after focus moves to another application | Compositor | Cross-client transitions own the reset path. |

## Worked example: Qt submenu flips the hover cursor to the arrow

Observed with Telegram Desktop context menus: hovering "Copy Filename"
shows the pointing hand; when hovering "Save to..." opens its submenu,
the pointer — still on the parent menu item — shows the default arrow.

Trace through the invariants:

1. The submenu `xdg_popup` maps. aegis re-hit-tests the stationary
   pointer, which is over the *parent* menu surface. Pointer focus does
   not change; no `leave`/`enter` pair is sent.
2. Focus, had it moved, would have moved between two surfaces of the
   same client, which preserves the cursor by design.
3. Therefore no server-side path fires. The arrow can only come from
   the client: the new `Qt::Popup` window grabs the mouse inside Qt
   widgets, the cursor is evaluated against the grabbing (submenu)
   window where the mapped position covers no item, and Qt asserts the
   default cursor with the parent menu's enter serial — a request aegis
   must and does accept.

Conclusion: application-level behavior. No compositor change is
appropriate; on other compositors the same application produces the
same flip.

## Policy: no client-bug workarounds

This follows the responsibility boundary of
[ADR-0001](../../adr/0001-scope-and-responsibility-boundary.md):

- Land a compositor change only when a trace shows aegis violating the
  protocol or the invariants above. Regression coverage for cursor
  focus behavior lives in `crates/aegis-compositor/src/tests/`
  (`popup_e2e.rs`); extend it instead of tuning behavior by hand.
- Do not add heuristics that second-guess a valid client request — for
  example ignoring a `set_shape(default)` because a submenu "probably"
  caused it. Such workarounds mask upstream bugs, surprise well-behaved
  clients, and become untestable debt.
- For a confirmed client bug, report it upstream and add a row to the
  symptom table so the next report triages in minutes.

Pointer-focus semantics themselves are specified in
[ADR-0009](../../adr/0009-input-pipeline-and-pointer-focus.md).
