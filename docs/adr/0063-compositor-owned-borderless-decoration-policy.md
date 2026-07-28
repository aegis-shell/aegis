# ADR-0063: Compositor-owned borderless decoration policy

- Status: Accepted
- Date: 2026-07-28

## Context

[ADR-0017](0017-server-side-decorations-via-overlays.md) introduced
per-window title-bar overlays. The executable later stopped registering that
component to establish a borderless desktop, but the Wayland server still
forced every decoration-aware client into client-side decoration mode. The
two policies contradicted each other: Aegis drew no server frame while
applications such as foot were instructed to add title bars.

Aegis already provides compositor-owned window controls through modifier
drags, invisible resize borders, the Dock, the launcher, key bindings, and
scoped IPC. A per-window frame is not required for move, resize, close, or
state operations.

The `xdg-decoration` protocol treats the client's requested mode as a
preference and lets the compositor select the effective mode. Omitting the
protocol does not establish a borderless policy; clients then self-decorate
as they see fit.

## Decision

Aegis owns toplevel decorations and selects borderless server-side ownership
by default. On the `xdg-decoration` wire, the compositor reports
`server_side`, so decoration-aware clients omit their own frames. Aegis
intentionally draws no per-window title bar; its existing gestures, invisible
borders, and shell surfaces provide the window controls.

The live-reloadable `[ui] window_decorations` setting exposes two policies:

- `borderless` selects compositor-owned controls without a per-window frame.
- `client-side` tells decoration-aware clients to draw their own frames.

The policy has one shared model in `aegis-core`. The Wayland server applies it
when a decoration object is created, when a client expresses a mode
preference, and when configuration reload changes the effective policy.

The unused title-bar `Chrome` component and its dedicated close and move
event path are removed. Window actions continue through the Dock, launcher,
overview, gestures, key bindings, and IPC.

## Alternatives

- **Force client-side decorations.** Rejected because it contradicts the
  borderless product policy and overrides applications that prefer
  compositor-owned decorations.
- **Stop advertising `xdg-decoration`.** Rejected because the protocol states
  that clients may self-decorate when negotiation is unavailable. That
  produces application-dependent behavior instead of one desktop policy.
- **Restore the title-bar overlay component.** Rejected because it duplicates
  controls already available through the shell and returns to a visual model
  the executable intentionally removed.
- **Claim server-side ownership without compositor window controls.** Rejected
  because removing both client frames and compositor controls would strand
  users. Borderless mode depends on the existing move, resize, close, and
  state paths remaining available.

## Consequences

- Decoration-aware applications such as foot open without client-side title
  bars under the default configuration.
- Applications that require their own frame can opt into `client-side`.
- Changing the setting reconfigures existing decoration-aware windows without
  restarting them.
- Clients that do not implement `xdg-decoration` remain free to self-decorate;
  the compositor cannot negotiate ownership with those clients.
- Decoration policy has one owner in the Wayland server instead of competing
  protocol and shell implementations.
