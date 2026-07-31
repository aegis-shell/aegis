# ADR-0082: Configurable touchpad swipe bindings

- Status: Accepted
- Date: 2026-07-30

## Context

ADR-0080 made four-finger touchpad swipes compositor-owned for the command
panel and left three-finger swipes forwarding to
`zwp_pointer_gestures_v1` clients. In practice no client running under
Aegis uses three-finger swipes, while the compositor still lacked a
touchpad path for its two most frequent navigation actions: switching
workspaces and cycling the current workspace's windows. The four-finger
panel gesture was hardcoded in the runtime, and adding more hardcoded
finger counts would couple each gesture event to one fixed function,
leaving no room for users — or future compositor features — to rebind a
swipe.

## Decision

Touchpad swipes bind to actions through a **gesture map**, mirroring the
keybinding architecture: `aegis-core` owns a pure `GestureMap` of
(finger count, axis) → `GestureAction` with name resolvers, the
configuration file layers `[[gesture]]` entries over the built-in
defaults, and the runtime claims a swipe in the gesture drain when its
finger count has any binding. The built-in defaults are:

- Three fingers, horizontal: `workspace_switch` — left moves to the next
  workspace, right to the previous one.
- Three fingers, vertical: `window_cycle` — up focuses the next window,
  down the previous one, through the window switcher held open until the
  gesture ends.
- Four fingers, vertical: `command_panel` — down opens the panel, up
  closes it (unchanged from ADR-0080).

Gesture actions are directional pairs: the swipe's dominant sign selects
the direction inside the action, so one binding covers both directions.
The runtime owns the per-gesture state — accumulators, the 30 px axis
latch, 120 px steps, the switcher session, and the panel's fire-once
latch — and each step goes through the command journal with the new
`Origin::Gesture`, distinct from `Origin::Keybinding`. Binding an axis to
the `none` action shadows a default while keeping the swipe
compositor-owned. No gesture runs while the session is locked.

## Alternatives

- **Keep hardcoding finger counts in the runtime.** Each new gesture
  consumer would edit the compositor's input path, and users could not
  disable or rebind a gesture; the gesture map costs one lookup table and
  removes both problems.
- **Reuse `[[keybind]]` with per-direction entries.** Splitting a swipe
  into `left`/`right`/`up`/`down` bindings would need four entries per
  gesture and cannot express stateful behaviors — the switcher session
  held open across a swipe, the panel's fire-once latch — which are
  properties of the gesture, not of one direction.
- **Position-driven (1:1) workspace animation during the swipe.** The
  workspace model switches atomically and ADR-0029's workspace-switch
  transition is still unimplemented; stepped switching matches the
  current instantaneous behavior and can be revisited with the animation
  work.

## Consequences

- Three-finger swipes no longer reach client
  `zwp_pointer_gestures_v1` objects, narrowing the ADR-0080 consequence
  that only four-finger swipes are compositor-owned. A claimed finger
  count stays compositor-owned even on an axis with no binding; swipes
  with unbound finger counts still forward unchanged.
- New swipe consumers become data, not input-path changes: a future
  `GestureAction` variant plus a name resolver makes the action bindable
  in `config.toml` and through the same hot-reload path that rebuilds
  the keymap.
- The gesture-driven window switcher must survive Super not being held,
  so the keyboard path's release-to-finish check yields while a swipe
  holds the switcher open.
- The mutation journal gains `Origin::Gesture` so the agent can model
  gesture-driven intent separately from keybindings.
