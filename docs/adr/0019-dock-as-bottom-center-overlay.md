# ADR-0019: macOS-style dock via a bottom-center overlay

- Status: Accepted
- Date: 2026-06-18

## Context

[ADR-0016](0016-shell-server-window-management-bridge.md) shipped a
top-left window-list panel and [ADR-0017](0017-server-side-decorations-via-overlays.md)
added per-window title bars, but the only "application indicator" the
user sees is the side panel's text list. There is no always-visible,
glanceable surface that shows what is running and lets the user act on
it in one click — the role the macOS dock plays.

The chrome already has the building block for absolute-positioned UI:
`Frame::overlay(id, anchor: Rect, opts, body)`, adopted for SSD in
ADR-0017. The missing piece is anchoring a panel to the bottom-center of
the output rather than to a window's position, and sizing it to its
contents.

## Decision

### 1. Render the dock as a flux-ui overlay anchored to a bottom-center rect

Each frame, the shell computes the dock's absolute `Rect` from the
input's `display_size` and opens an `aegis-dock` overlay there:

```text
n        = max(1, windows.len())
dock_w   = max(DOCK_MIN_WIDTH,
              n*DOCK_TILE + (n-1)*DOCK_TILE_GAP + 2*DOCK_PAD)
dock.x   = (display_w - dock_w) / 2
dock.y   = display_h - DOCK_HEIGHT - DOCK_BOTTOM_MARGIN
```

The panel uses a translucent rounded background (`OverlayOpts` with `bg`,
`border`, `border_width`, and `radius`), so it reads as a floating bar
over the wallpaper and client surfaces.

### 2. One tile per mapped toplevel, click to focus

Inside the overlay, a `row_ex` with `gap = DOCK_TILE_GAP` holds one tile
per window. Each tile is a `size_next(DOCK_TILE, DOCK_TILE)` followed by
`icon_button_active(Icon::FileText, window.state.activated)`. A click
sets the same `clicked_window` slot the side panel uses, so the main loop focuses it through the existing `Server::focus_surface_by_id`
path — no new server API is introduced.

### 3. Activation as the running/focus indicator

`icon_button_active(..., true)` paints a steady highlight (fill plus
accent) on the activated window's tile, the same affordance the activity-
bar idiom uses. This serves as both the "running" dot and the "focused"
marker, since every dock tile is a running window and exactly one is
activated at a time. A separate indicator dot is deferred until there is
a notion of pinned apps that run independently of open windows.

### 4. Empty state

With no mapped toplevels the dock still renders (so its presence is
stable) but shows a muted `label_sized("no apps", 12.0)` and no tiles.

## Alternatives

- **Pin the dock to the top-left auto-layout flow.** Rejected: it would
   stack under the existing side panel and is not where users expect a
   dock. The overlay gives precise bottom-center placement.
- **Draw the dock with raw `flux::Canvas` primitives.** Rejected for the
   same reason as ADR-0017: it bypasses flux-ui's input handling, forcing
   aegis-shell to re-implement hit-testing and hover.
- **A separate "indicator dot" widget under each tile.** Deferred:
   `icon_button_active` already conveys activation, and ass has no app
   registry of pinned-but-not-running entries that a dot would
   distinguish from a focused window.
- **Minimize-on-reclick of the focused tile.** Deferred: ass does not
   yet implement `set_minimized`. Click focuses; reclick re-focuses.

## Consequences

- The chrome now has a bottom-center dock that lists every mapped
  toplevel as an icon tile and focuses it on click. The activated
  window's tile is highlighted.
- The dock reuses `clicked_window` and `Server::focus_surface_by_id`, so
  no new window-management API or main-loop plumbing is needed.
- Dock geometry is visual constants (`DOCK_HEIGHT = 64.0`,
  `DOCK_TILE = 44.0`, etc.); it does not participate in
  `set_window_geometry` frame insets any more than the SSD title bar
  does.
- Tiles share the single `Icon::FileText` glyph because ass has no
  per-app icon source yet. Real application icons (from `app_id` /
  `.desktop` lookup) are future work; the tile layout already sizes for
  them.
- As with the SSD title bar, the click is also forwarded to the client
  as a `wl_pointer.button` event because the shell handles input during
  the frame render, after `Server::forward_input`. Tracked as shared
  polish across the chrome.
