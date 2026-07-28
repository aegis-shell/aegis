# ADR-0064: Output space-use state and chrome policy

- Status: Accepted
- Date: 2026-07-28

## Context

The xdg-toplevel state machine records maximized and fullscreen as independent
protocol bits. Shell chrome previously derived its own behavior from those
bits, and the Dock treated both states as complete output ownership. That
removed the Dock's reveal affordance for maximized windows even though the
status bar remained visible.

The generic `WindowsChanged` IPC event also omitted maximized, fullscreen, and
minimized transitions from its comparison signature. Subscribers could read
the state from `GetWindows`, but they were not reliably notified when only
space use changed.

## Decision

Add one compositor-wide `SpaceUse` state derived from the visible,
non-minimized window snapshot:

- `Available` when no window consumes an output edge;
- `Maximized` when at least one window is maximized; and
- `Fullscreen` when at least one window is fullscreen.

`Fullscreen` has precedence when visible windows report different states.
The underlying xdg-toplevel bits remain independent so a fullscreen window can
return to its prior maximized state.

Apply this state consistently:

| State | Dock | Status bar |
|-------|------|------------|
| `Available` | Follow the configured persistent or autohide policy. | Visible. |
| `Maximized` | Collapse to a centered capsule; reveal only from the capsule's local hover region. | Visible. |
| `Fullscreen` | Hide completely with no capsule or hover target. | Hide completely. |

IPC protocol version 8 adds `SpaceUseChanged { state }`. Window comparison for
`WindowsChanged` also includes maximized, fullscreen, and minimized state so
the complete window snapshot remains observable through its existing
invalidation event.

## Alternatives

- **Treat maximized and fullscreen as the same output-cover state.** Rejected:
  maximized mode keeps persistent top chrome and needs a discoverable Dock
  affordance, while fullscreen grants the client the complete output.
- **Let each chrome component derive its own booleans.** Rejected: independent
  derivation lets reservation, rendering, blur, and input-capture policy drift
  apart.
- **Emit only `WindowsChanged`.** Rejected: consumers interested in output
  chrome policy would need to re-query and re-derive a canonical state after
  every unrelated window change.

## Consequences

- Maximized and fullscreen transitions now drive one shared precedence rule.
- The Dock's reserved edge, blur region, rendering, animation, and pointer
  capture all follow the same state.
- Fullscreen windows remove both persistent shell edges; maximized windows
  retain the status bar and a local Dock reveal target.
- IPC clients must use protocol version 8 and can subscribe directly to
  output-space-use transitions.
