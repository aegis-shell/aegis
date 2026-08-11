# ADR-0119: Four-finger command panel default restored

- Status: Accepted
- Date: 2026-08-11

## Context

ADR-0116 rebound the built-in four-finger vertical swipe from the command
panel to the new overview: up opened the picker, down closed it. Two days
of daily use exposed three problems with the rebind:

- The downward swipe had opened the command panel since ADR-0080. That
  muscle memory is older than the overview, and the panel is the more
  frequent destination: quick settings, tray, and notifications live
  there.
- A downward swipe with the overview closed did nothing at all. The
  gesture read as broken, not as disarmed — the failure mode this record
  was written to answer.
- The overview already closes on `Escape`, the same convention every
  other modal chrome surface follows, so the swipe-down close path
  duplicated an existing dismissal channel.

## Decision

1. **The built-in four-finger vertical default returns to
   `command_panel`** — down opens the panel, up closes it, unchanged from
   [ADR-0080](0080-hud-status-chips-and-sao-command-panel.md) and
   [ADR-0082](0082-configurable-touchpad-swipe-bindings.md).
   `GestureAction::Overview` stays in the binding table and remains
   assignable through `[[gesture]]` configuration.
2. **The overview has no built-in touchpad gesture.** `Super+O` opens and
   closes it and `Escape` closes it, matching the other modal chrome
   surfaces; a touchpad entry is configuration, not code.

This record amends
[ADR-0116](0116-overview-gesture-top-rail-and-spatial-slots.md), whose
remaining decisions — the top workspace rail, fly-in thumbnails, and
closest-slot assignment — are unchanged.

## Alternatives

- **Split the axis per direction** (up opens the overview, down opens the
  command panel). ADR-0082 rejected per-direction bindings: fire-once
  latches and held-open sessions are properties of the gesture, not of a
  direction, and a composite action would conflate two features with
  different feature gates.
- **Keep the overview default.** The gesture only acted in one state and
  presented as broken in the common case; an affordance that requires
  the picker to already be open is not an entry point.

## Consequences

- Users who adopted the ADR-0116 gesture restore it with a `[[gesture]]`
  entry (`fingers = 4`, `axis = "vertical"`, `action = "overview"`);
  `docs/reference/config.md` carries the snippet.
- Builds without the `chrome-command-panel` feature release the
  four-finger claim to client `zwp_pointer_gestures_v1` objects again, as
  they did before ADR-0116.
- `docs/reference/keyboard-shortcuts.md` documents the restored gesture
  rows and the overview's `Escape` dismissal.
