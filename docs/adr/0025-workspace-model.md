# ADR-0025: Workspace model — dynamic and per-output

- Status: Accepted
- Date: 2026-06-23

## Context

[M3](../explanation/roadmap.md#m3-window-management-and-first-chrome) has no
workspace concept: every mapped toplevel lives on a single global space, and
the chrome lists them all. The [comparative survey](../explanation/comparative-survey.md#workspaces-and-outputs)
records four credible workspace models in the field: GNOME's dynamic global
list; KDE's Activities as a second axis; sway/i3 numbered static per-output
workspaces; river's tag bits; and niri's dynamic per-output workspaces that
survive monitor unplug and replug. macOS Spaces are per-display virtual
desktops managed through Mission Control.

The window manager already needs an answer for the multi-output milestone
([ADR-0028](0028-output-and-monitor-model.md)), and the agent phase needs
stable, queryable identifiers for "where a window is". A workspace model
that hides behind global state — GNOME's single list — is hard to expose
cleanly to an agent and forces per-output behavior into special cases.

## Decision

ass adopts **dynamic, per-output workspaces** in the niri lineage, with one
workspace always kept empty at the end of each output's list so the user can
always switch to a fresh space.

Each `Output` owns an ordered list of `Workspace`s and a current-workspace
index. A `Workspace` owns the set of toplevels placed on it. A toplevel
belongs to exactly one workspace on exactly one output. Switching the
current workspace changes which toplevels are mapped and rendered; the
underlying `xdg_toplevel` objects stay alive.

Workspaces are **dynamic**: creating one when the user moves past the last
non-empty one, and removing an empty one when the user leaves it, keeps the
list tight. The empty workspace at the end is the GNOME behavior; the
per-output ownership is the niri behavior.

On output unplug, the output's workspaces relocate to a surviving output in
a deterministic order (primary, then first surviving). On replug of the same
output, the relocated workspaces move back if their remembered output id
matches, restoring the prior arrangement. Each workspace remembers its
output id for this purpose.

A workspace identifier is stable for the life of the workspace and exposed
through the IPC ([ADR-0027](0027-ipc-and-introspection.md)); the agent and
external tools address workspaces by id, never by number. The visible
number is a presentation detail owned by the chrome.

Workspaces interact with layout ([ADR-0024](0024-layout-model.md)) at the
policy level: a workspace may select a tiling policy that applies to its
`tiled` windows, configured per-workspace through the configuration system
([ADR-0026](0026-configuration-system.md)).

## Alternatives

- **A single global workspace list (GNOME).** Rejected: it forces every
  per-output behavior into a special case and exposes a global coordinate
  space that an agent must reconstruct. The niri project's stated motivation
  for leaving the GNOME/PaperWM global space applies here.
- **Numbered static workspaces (sway/i3).** Rejected as the default: numbers
  are stable but rigid, and the "always one empty workspace ahead" affordance
  is lost. Static workspaces are reachable as a configuration of the dynamic
  model (a fixed count, no auto-create) without becoming the model.
- **Tag bits (river, dwm).** Rejected: dynamic workspaces are easier to
  reason about, to render in an overview, and to expose to the agent. Tags
  trade expressive power for cognitive cost that the project does not need
  to pay.
- **Activities as a second axis (KDE).** Rejected: a second orthogonal axis
  doubles the state space the user and the agent must track. The same effect
  is reachable with more workspaces and window rules, without making
  "where is this window" a two-part question.
- **Per-application full-screen Spaces (macOS).** Rejected as structural:
  full-screen is a window state (`WindowState::fullscreen`), not a workspace.

## Consequences

- `aegis-core` gains `Output` and `Workspace` types and the window manager
  routes map and unmap through the current workspace of the focused output.
  The renderer and chrome receive a per-output snapshot, not a global list.
- The `WindowList`, `Dock`, and `Launcher` chrome components
  ([ADR-0021](0021-chrome-component-trait.md)) are extended to show
  workspaces; a future overview chrome surfaces them all at once.
- Output unplug and replug behavior is part of the model, not a recovery
  path, which raises the implementation cost of
  [ADR-0028](0028-output-and-monitor-model.md) and is the reason the two
  milestones are ordered as they are in the
  [roadmap](../explanation/roadmap.md).
- The IPC and the agent address workspaces by stable id; the chrome may show
  numbers, but they are not the identity.
- The empty-workspace-at-end invariant is a small, constant source of
  bookkeeping that pays for itself in the "always a fresh space" affordance
  GNOME users expect.
