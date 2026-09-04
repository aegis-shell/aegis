# Popup Positioning Issue Triage

How to attribute popup and floating card placement issues between client
toolkits and the compositor before changing any code, and what to do with the
result. The rule throughout: fix what tessera gets wrong, never paper over what a
client gets wrong.

## How popups and subsurfaces are positioned

Wayland defines strict, distinct roles for independent windows versus dependent
surfaces:

- **`xdg_popup` + `xdg_positioner`** — the protocol mechanism for context
  menus, dropdowns, and tooltips. The client specifies an `anchor_rect` in
  parent coordinates, anchor edges, gravity, and constraint adjustments.
  `tessera` positions the popup relative to the parent window's geometry.
- **`wl_subsurface`** — a child surface fixed at a pixel offset relative to its
  parent surface.
- **`xdg_toplevel`** — an independent top-level window. In the Wayland
  architecture, `xdg_toplevel` windows have no protocol request for setting
  absolute coordinates or requesting specific desktop placement. `tessera` owns
  toplevel window placement (cascading, centering, or layout-driven).

## Server-side invariants

Popup and toplevel placement obey these invariants:

1. An `xdg_popup` with a valid `xdg_positioner` is placed relative to its parent
   `xdg_surface` window geometry, offset by the positioner rules and constrained
   to output bounds.
2. An `xdg_toplevel` has no global coordinate authority. Its position is assigned
   by the compositor's placement policy when mapped.
3. Wayland deliberately provides no global coordinate space (`[0, 0]` to `[w, h]`
   is surface-local). `win.getPosition()` in toolkits cannot return desktop-global
   coordinates.

## Diagnostic recipe

Run the affected client with Wayland protocol debugging to observe how the
popup surface is created and configured:

```bash
WAYLAND_DEBUG=1 <client> 2>&1 | grep -E 'xdg_wm_base|xdg_surface|xdg_toplevel|xdg_popup|xdg_positioner'
```

Watch the requests sent when the popup or hover card opens:

- The client creates an `xdg_surface` and requests `get_toplevel` (or creates a
  new `BrowserWindow`) rather than `get_popup` → **client-side toolkit limitation**.
  The client attempted to simulate a popup with an independent window, which
  Wayland places according to default compositor policy (e.g. top-left cascade).
- The client requests `get_popup` with an `xdg_positioner`, but `tessera` computes
  wrong coordinates or ignores constraint adjustments → **compositor-side**.
  File a bug and write regression coverage in `crates/tessera-compositor/src/tests/popup_e2e.rs`.

Quick symptom table:

| Symptom | Likely owner | Why |
|---------|--------------|-----|
| Electron / QQ floating profile card or hover menu appears at the top-left cascade position instead of next to the cursor / item | Client | Electron implements floating cards as independent borderless `BrowserWindow` instances (`xdg_toplevel`) and relies on `win.setPosition()`. On Wayland, `xdg_toplevel` position requests are non-existent, so the compositor places it at the default window origin. |
| Web page popup (`window.open`) detaches from Chrome and covers an unrelated window | Client / Web | `window.open` creates an independent `xdg_toplevel` whose JS `left`/`top` parameters are discarded on Wayland. |
| Context menu flips or shifts off-screen despite output bounds having space | Compositor | Positioner constraint logic in `constrain_positioner` failed to satisfy `flip` or `slide` rules. |
| Popup does not move when parent toplevel moves | Compositor | `reposition_toplevel_with_popups` failed to recurse down child popup trees. |

## Worked example: Electron QQ hover card renders at top-left

Observed with Linux QQ (Electron / NT architecture): hovering over a contact or
group item in the chat list triggers a floating information card. Instead of
anchoring to the right of the hovered item, the card appears at `(60, 60)` near
the top-left corner of the screen/window.

Trace through the invariants:

1. Electron's application logic instantiates a new `BrowserWindow({ frame: false, transparent: true })`.
2. Ozone-Wayland receives the request and creates an `xdg_surface` with the
   `xdg_toplevel` role rather than an `xdg_popup`.
3. Electron attempts to call `win.setPosition(x, y)` calculated from the
   hovered item's bounding rect and the main window position.
4. Because Wayland provides no global coordinate queries (`getPosition()` returns
   zeros) and `xdg_toplevel` has no `set_position` request, no coordinate
   information reaches `tessera`.
5. `tessera` treats the card as a regular new window and assigns it the default
   cascade origin `(60, 60)`.

Conclusion: application and toolkit limitation. The application must render
floating cards inside the existing WebContents DOM or use `xdg_popup` /
`wl_subsurface` protocols. Running under XWayland (`--ozone-platform=x11`) restores
X11 global coordinate emulation.

## Policy: no client-bug workarounds

Following [ADR-0001](../../adr/0001-scope-and-responsibility-boundary.md):

- Do not introduce heuristics to guess the intended coordinates of unparented
  or unpositioned `xdg_toplevel` windows based on the current cursor position.
- Retain strict separation between `xdg_toplevel` (compositor-managed placement)
  and `xdg_popup` (parent-relative anchor placement).
