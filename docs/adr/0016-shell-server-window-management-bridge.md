# ADR-0016: Shell ↔ server window-management bridge

- Status: Accepted
- Date: 2026-06-18

## Context

[ADR-0012](0012-toplevel-metadata-and-state-machine.md) gave the server a
`Window` model and the API to enumerate (`windows()`), close
(`close_toplevel(id)`), and activate (`set_toplevel_activated(id, bool)`)
toplevels. But the shell — the only consumer of that API in M3 — was still
hard-coded to draw just a Quit button. There was no path from a click in the
chrome to a server-side window-management action, and no visible list of
windows for the user to act on.

Real server-side decorations (per-window title bars drawn around the
client surface) need careful hit-testing against the decoration frame and
flux-ui widget work for the title-bar layout; both are deferred. But the
shell can already provide a window-list panel that exercises the
window-management API end-to-end, proving the bridge works before SSD
lands.

## Decision

### 1. Shell owns a per-frame snapshot of windows

`Shell::set_windows(Vec<Window>)` replaces the snapshot each frame. The
main loop calls it with `server.windows()` immediately before
`shell.render()`, so the chrome always reflects the current set.

### 2. Shell renders a window-list panel

The chrome's column now has, after the Quit button:

- A separator and a "Windows" label.
- Per window: a row containing a title button (grows with `flex(1.0)`)
  and a close button (fixed-width "x"). The title is prefixed with a
  right-pointing triangle when `state.activated` so the user can see
  keyboard focus at a glance. Missing titles fall back to
  `"<untitled>"`.

### 3. Shell collects click/close requests and drains them via accessors

Two new fields on `Shell`:

- `clicked_window: Option<usize>` — set when the title button is clicked.
- `closed_window: Option<usize>` — set when the close button is clicked.

The main loop drains them after `render()`:

```text
shell.set_windows(server.windows());
shell.render(...);
if let Some(id) = shell.take_clicked_window() { server.focus_surface_by_id(id); }
if let Some(id) = shell.take_closed_window() { server.close_toplevel(id); }
```

### 4. New `Server::focus_surface_by_id` for shell-driven focus

Equivalent to the click-to-focus pointer path, but initiated from chrome.
Calls `change_keyboard_focus(resource)` (which posts keyboard enter/leave
and flips the activated bit via the existing
`set_activated_for_surface` path). No pointer motion is synthesized; the
pointer stays where it is, only keyboard focus moves.

## Alternatives

- **Per-window title bars (full SSD).** Deferred: needs hit-testing of
   the decoration frame in the server's pointer-motion path, plus a
   `zxdg_decoration_manager_v1` implementation, plus flux-ui widget work
   for the title-bar layout. The window-list panel covers the
   window-management interactions (focus, close) without that visual
   chrome; SSD is still tracked as the next M3 increment.
- **Direct pointer injection for focus.** Rejected: synthesizing a
   `wl_pointer` motion + button event to drive focus through the existing
   click path would double-count input and risk messing with the
   interactive-grab state machine. A dedicated `focus_surface_by_id` is
   cleaner.
- **CSD-only (no chrome window list).** Rejected: clients that do not
   implement CSD (test clients, simple shm clients) would have no way
   to be focused or closed from the compositor. The window list is the
   universal fallback.

## Consequences

- The chrome now lists every mapped toplevel by title and lets the user
  click to focus or close. Title-bar visibility (`state.activated`)
  updates as focus changes.
- `xdg_toplevel.close` is now reachable from the chrome; clients that
  respond by destroying their `xdg_toplevel` and surface get cleaned
  up via the existing destroy path.
- Real server-side decorations (title bars around windows) remain
  deferred. Clients that draw their own CSD continue to do so; clients
  without CSD draw a bare buffer with no frame.
- The window list is the seed of the future overview / launcher; once
  SSD lands and the launcher grows a wayland-client spawn path, the
  shell will have the full M3 chrome surface.
