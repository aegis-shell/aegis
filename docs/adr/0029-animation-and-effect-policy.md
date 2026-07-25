# ADR-0029: Animation and effect policy

- Status: Accepted
- Date: 2026-06-23

## Context

Through [M3](../explanation/roadmap.md#m3-window-management-and-first-chrome)
the chrome is immediate-mode and stateless across frames: each frame is
drawn from the current window snapshot and input. There are no animations,
no transitions, and no retained visual state. The
[comparative survey](../explanation/comparative-survey.md#input-and-rendering)
records that every shell a user considers finished animates: GNOME and KDE
through Clutter and the QtQuick scene graph; niri through a built-in
animation system with custom shaders; macOS through Core Animation. sway and
river are largely static, and users perceive the difference.

The [vision](../explanation/vision.md#design-principles) commits ass to
keeping rendering and UI in flux and lens. An animation layer that lives in
`aegis-core` (which has no rendering dependency) or in the binary cannot draw;
one that lives in flux duplicates lens's role. The seam must be in lens,
with ass owning only the policy and the triggers.

A second constraint is accessibility. A non-trivial number of users disable
animations, and a finished desktop respects that preference end to end rather
than per-effect.

## Decision

ass adopts a **declarative animation policy owned by ass, executed by lens**.
ass decides what animates, when it starts, and which easing curve applies;
lens interpolates and draws. The policy is data: a set of named transitions
(workspace switch, window open and close, launcher reveal, focus change)
each mapped to a duration and an easing curve, with a global reduced-motion
flag.

Reduced-motion is read from the configuration
([ADR-0026](0026-configuration-system.md)) and from the freedesktop.org
`org.freedesktop.appearance color-scheme` / accessibility hints where
available. When reduced-motion is on, every transition resolves to its end
state in at most one frame, with no fade, no scale, and no parallax. The
policy is the single switch; individual effects do not each decide.

The ass side of the policy is **retained per-target state**: when a window
moves, the window manager records the previous and target rectangles and a
start time, and publishes both in the snapshot. lens interpolates between
them for the transition's duration. When the snapshot no longer carries an
in-flight transition for a target, the window renders at its target. The
model the chrome and the IPC ([ADR-0027](0027-ipc-and-introspection.md))
read still reports the logical (target) position; the in-flight interpolation
is a rendering detail that does not mutate the model.

Effects beyond motion (blur, shadow, rounded corners, gradient borders) are
lens drawing capabilities, not ass policy. ass enables or disables them by
name through the configuration and supplies parameters (blur radius, corner
radius); lens renders them. A missing effect is added to lens following
[ADR-0001](0001-scope-and-responsibility-boundary.md), not worked around in
ass.

Custom shaders, in the niri style, are a future lens capability enabled
through the same name-and-parameter mechanism, not a separate path.

## Alternatives

- **No animations, ever.** Rejected: it meets the accessibility preference
  for some users but fails the "feature richness of a mainstream shell"
  target in [Vision and Scope](../explanation/vision.md#product-target) for
  everyone else, and reduces-motion users are better served by a switch that
  works than by an absence.
- **An animation framework in `aegis-core`.** Rejected: `aegis-core` has no
  rendering dependency by design
  ([ADR-0001](0001-scope-and-responsibility-boundary.md)), and an animation
  framework that cannot draw is in the wrong crate.
- **Animations in flux.** Rejected: flux is the renderer, not the UI layer;
  putting chrome animation in flux duplicates lens's role and blurs the
  seam fixed by [ADR-0023](0023-split-flux-lens-stack.md).
- **A retained-mode scene graph in ass.** Rejected: it reverses the
  immediate-mode chrome design ([ADR-0021](0021-chrome-component-trait.md))
  and forces ass to own layout and widget state it deliberately delegates to
  lens.
- **Per-effect reduced-motion switches.** Rejected: a forest of per-effect
  flags is unmaintainable and inconsistent. One global switch, applied
  uniformly, is the contract users can rely on.
- **Scriptable animations.** Rejected: it reintroduces in-process scripting,
  which [ADR-0027](0027-ipc-and-introspection.md) rules out. Custom shaders
  are a data-driven lens capability, not user code in the compositor.

## Consequences

- The per-frame snapshot the chrome reads gains an optional in-flight
  transition field per target, so lens can interpolate without breaking the
  one-model principle: the logical state is unchanged.
- lens gains an interpolation pass driven by the named transitions ass
  supplies; the immediate-mode `Ui::frame` envelope
  ([ADR-0021](0021-chrome-component-trait.md)) is extended to carry the
  active transition set.
- Reduced-motion becomes a first-class configuration key and an accessibility
  commitment, tested as part of the milestone rather than added late.
- Effects (blur, shadow, corners, gradients) become named lens capabilities
  toggled through configuration, which keeps ass policy-only and avoids
  renderer state leaking into the compositor.
- Input latency remains the hard constraint: an animation that delays input
  feedback is a bug, and the interpolation pass must not block the event
  loop, which becomes a measurable requirement of the milestone.
