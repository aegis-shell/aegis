# ADR-0024: Layout model — floating base with an optional tiling policy

- Status: Accepted
- Date: 2026-06-23

## Context

The window manager in [M3](../explanation/roadmap.md#m3-window-management-and-first-chrome)
places toplevels as free rectangles (cascade on map, interactive move and
resize, maximize, fullscreen). That is enough for the floating desktop, but
it leaves the project without an answer to the layout question every modern
compositor settles. The [comparative survey](../explanation/comparative-survey.md#layout-model)
records the field: macOS, GNOME, KDE, and Xfce are floating with snapping;
sway is manual tree tiling; river is dynamic tiling with an external layout
generator; niri is scrollable tiling. Each model is coherent on its own
terms and each rejects a class of applications or workflows the others
accept.

Three constraints narrow the choice. First, the agent phase in
[Vision and Scope](../explanation/vision.md#the-agent-phase) needs one
consistent scene that every consumer reads, so a layout model that forks the
window representation (tiling trees versus floating rectangles) is a cost.
Second, the chrome is a component, not a privilege
([ADR-0021](0021-chrome-component-trait.md)), so the layout must be
expressible through the same window snapshot the chrome already reads.
Third, the project rejects in-process scripting
([ADR-0027](0027-ipc-and-introspection.md)), so layout policy cannot be "an
extension computes the scene" the way a KWin script or a GNOME extension
would.

## Decision

ass adopts **floating as the universal base**, with two refinements layered
on top in priority order:

1. **Window snapping.** Edge and corner drag, half and quarter regions, and
   maximize, applied as state transitions on the existing `Window` and
   `WindowState` model ([`ass-core::window`](../../crates/ass-core/src/window.rs)).
   Snapping does not change the data model; it changes the size and position
   a floating window resolves to.
2. **An optional tiling policy.** A toplevel or a workspace may declare a
   tiling policy. When active, the window manager arranges the affected
   toplevels into a layout the policy computes each time the set changes.
   Tiling is one policy among possible policies, not a separate mode of the
   compositor.

The tiling policy is **first-class in the window manager**, not an external
process. The policy receives the live window set and the configured
parameters (gaps, splits, primary ratio) and emits a layout; the window
manager applies it by setting each window's position and size, exactly as an
interactive move does. Floating windows and tiled windows coexist on the
same workspace; a window carries a single `LayoutRole` (`Floating` or
`Tiled`), and the policy only touches windows whose role is `Tiled`.

The configuration system ([ADR-0026](0026-configuration-system.md)) selects
the policy per workspace and the IPC
([ADR-0027](0027-ipc-and-introspection.md)) exposes the role, so an external
tool or the agent can promote a window into tiling or float it back.

The window representation stays uniform: a tiled window is still a `Window`
with a position and size, decorated by the same chrome and tracked by the
same focus model. There is no separate tiling-container type.

## Alternatives

- **Manual tree tiling (sway/i3).** Rejected as the base: it rejects
  applications that assume free placement (dialog-heavy tools, floating
  palettes) and imposes a keyboard-driven workflow on every user. ass keeps
  it available as a future policy, not as the model.
- **Dynamic tiling with an external layout generator (river).** Rejected as
  the first implementation: it is attractive and will be revisited as an
  extension point once the IPC is stable, but it pushes the layout decision
  out of the model the chrome and the agent read, which breaks the
  one-model principle for the cases that matter first.
- **Scrollable tiling (niri).** Rejected as the default: it is an excellent
  fit for a keyboard-heavy workflow and a strong candidate for a policy, but
  it couples layout to the workspace model
  ([ADR-0025](0025-workspace-model.md)) in a way that should be opt-in, not
  structural.
- **Tiling-only, no floating.** Rejected outright: it fails the "feature
  richness of a mainstream shell" target in
  [Vision and Scope](../explanation/vision.md#product-target), which
  includes arbitrary applications.
- **A separate tiling container type alongside floating windows.** Rejected:
  it forks the window representation and forces every consumer (renderer,
  chrome, IPC, agent) to handle two shapes.

## Consequences

- The `Window` model gains a `LayoutRole` field. Interactive move and resize
  already mutate position and size; the tiling policy does the same through
  a privileged path, so the renderer and chrome are unchanged.
- Window snapping is implementable on the current model with no schema
  change, and lands first.
- The tiling policy interface lives in `ass-core` (pure, no flux or Wayland
  dependency), with the default policy implemented there. A layout-policy
  trait keeps the door open to a river-style external generator over IPC in
  a later milestone without redesign.
- Mixed floating and tiled windows on one workspace is supported from day
  one, because the model does not distinguish them at the structural level.
- The agent and the IPC see one window shape, so tiling adds capability
  without adding representation.
