# ADR-0115: Command panel scope — desktop-computer behavior, not compositor internals

- Status: Accepted
- Date: 2026-08-09

## Context

[ADR-0080](0080-hud-status-chips-and-sao-command-panel.md) concentrated the
chrome interactions in one modal command panel, and
[ADR-0114](0114-panel-hosted-settings-and-hud-command-panel.md) moved the
persistent settings modules into it. The panel now carries quick settings,
settings module tabs, notifications, the tray, the persona header, the
machine monitor, and the Agent Workspaces status row — but neither record
states what *belongs* there. Because the project is a compositor, the
panel's gravity pulls toward a compositor console: every new compositor
capability looks panel-worthy, and the surface drifts from the user's
mental model toward the implementation's.

The user's mental model is the desktop computer. Daily desktop use involves
a stable set of domains — sound, displays, network and Bluetooth, power,
the session, notifications, the tray, the user's persona, machine
resources, and desktop preferences. A panel scoped to those domains reads
as "the computer's status and controls"; a panel scoped to the compositor
reads as a debug console.

## Decision

1. **The command panel is the display-and-control surface for
   desktop-computer behavior.** Its display dimension reports the
   computer's state across the daily-use domains; its control dimension
   takes immediate, user-level action over them — volume, monitor layout,
   radios, power policy, session actions, notification dismissal, tray
   activation, and desktop preferences.
2. **The scope test is the computer, not the compositor.** A surface
   belongs when its subject exists on any desktop computer regardless of
   which compositor runs it. Surfaces whose subject is the compositor
   itself — window-tree internals, protocol or IPC state, debugging and
   introspection, developer tooling — do not enter the panel.
3. **Domains follow the user's model, not the implementing component.**
   Window tiling and Agent Workspaces remain in scope: window layout and
   the machine's live workloads are desktop-behavior domains even though
   the compositor implements them. What stays out is compositor
   *mechanism*: Interaction Domain lifecycle, IPC introspection, and
   protocol state remain CLI/MCP territory, with the panel showing at most
   their user-model status projection.

This record amends
[ADR-0080](0080-hud-status-chips-and-sao-command-panel.md) and
[ADR-0114](0114-panel-hosted-settings-and-hud-command-panel.md), which
establish the panel and its contents without stating its scope.

## Alternatives

- **Leave the scope implicit.** Rejected because every addition so far has
  been justified case by case; without a written test the panel accumulates
  whatever the compositor happens to grow, and its identity dissolves.
- **Scope the panel to compositor-owned state only** (the narrow reading).
  Rejected because the settings modules already reach host account,
  backlight, and logind services under the settings boundary of ADR-0114;
  the panel's value is precisely that it covers the whole computer through
  one mental model. A compositor-only panel would orphan exactly the
  controls users expect to find here.
- **Add a separate compositor-debug chrome surface.** Rejected for now:
  introspection already exists over the IPC, the `aegis` CLI, and the MCP
  tools; a debug chrome surface would blur the panel's identity without
  serving a user need.

## Consequences

- Every proposed tab, section, or status row justifies itself against the
  scope test; compositor-mechanism UI is directed to the `aegis` CLI, the
  IPC, or the MCP tools.
- The `aegis-settings` module registry inherits the scope for hosted tabs:
  a module's domain must be a desktop-computer domain — display, touchpad,
  appearance, and power all are.
- No existing surface moves: the current tabs, header band, and side column
  already sit inside the scope.
- The `aegis-command-panel` crate contract and the design documentation
  state the scope at the seam so future hosts and modules inherit it.
