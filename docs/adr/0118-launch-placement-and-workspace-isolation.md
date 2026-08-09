# ADR-0118: Launch placement and workspace isolation

- Status: Accepted
- Date: 2026-08-09

## Context

An Agent operates the desktop through MCP: it searches the application
catalog, opens windows, and drives them on the user's behalf. Every launch
path Aegis offered placed the new window on the user's current workspace,
and the newly mapped window typically took keyboard focus through the
ordinary activation path. An Agent launch therefore hijacked the user's
view and
focus — the exact opposite of the intended workflow, where the Agent owns
a workspace of its own (for example workspace 2) while the user keeps
working on another (for example workspace 1) with no mutual interference.

The required behavior is placement at launch time: the window opens on its
target workspace even while that workspace is hidden, and the user's view
changes only when the user explicitly switches workspaces. A move after
the fact cannot provide this, because the window would map, render, and
possibly activate on the user's workspace first.

The workspace model ([ADR-0025](0025-workspace-model.md)) is dynamic and
per-output, so workspaces are addressable by durable id but not by a
stable position. The Interaction Domain architecture
([ADR-0103](0103-actor-authority-and-interaction-domain-architecture.md))
already provides the stronger isolation mechanism — an independent seat,
a directed virtual output, and a sandboxed `LaunchInInteractionDomain` —
but transferring a window into an Interaction Domain replaces the human
desktop context instead of composing with it.

This decision amends [ADR-0025](0025-workspace-model.md),
[ADR-0027](0027-ipc-and-introspection.md), and
[ADR-0090](0090-native-capability-broker-and-stateless-mcp-edge.md).

## Decision

Aegis adds launch-time workspace placement, decided before the launched
application maps its first window:

- A `LaunchPlacement` model shared by the IPC schema and the compositor:
  `Workspace { id }` targets an existing workspace by durable id (if the
  workspace is gone by map time, the default current-workspace placement
  applies), and `FreshWorkspace { label }` creates a fresh workspace
  directly after the current one on the window's output and places the
  window there. No placement variant ever switches the user's view.
- IPC protocol 27 adds `Command::LaunchApp { desktop_id, placement }`, a
  `control` command carrying the new `ActorCapability::LaunchApp`. When
  the placement names a workspace, scope authorization checks the
  connection's workspace resource axis. `Command::Focus` gains the
  additive `reveal` flag, serde-defaulted to `true` for older peers; with
  `reveal = false` the compositor never switches the user's view — a
  hidden window is only raised within its own workspace and never
  receives keyboard focus.
- The runtime resolves the desktop entry in the launcher catalog (an
  unknown id is a journaled refusal), spawns it unsandboxed with the
  session's `WAYLAND_DISPLAY`, and registers a pending launch placement
  keyed by the entry's `startup_wm_class`, the desktop-id stem, the full
  desktop id, and the spawned process id. Every decision is journaled as
  `AuditedCommand::LaunchApp { desktop_id, workspace }`, and the command
  is refused while the session is locked like every other command.
- The compositor holds pending placements in a registry with a 60-second
  TTL. The first root toplevel to map consumes one entry: an exact
  client-pid match wins, otherwise a case-sensitive app_id match consumes
  the oldest entry (FIFO). A placement beats `[[window_rule]]` and the
  remembered-workspace store. Transients (dialogs) always map onto their
  parent's workspace and never consume a placement. A window placed off
  the current workspace is invisible and never takes `pending_activation`,
  so it cannot steal the user's keyboard focus.
- `ActorCapability::LaunchApp` is agent-requestable and runtime-gated:
  the first use asks the user interactively, however the ceiling was
  approved, exactly like `LaunchInInteractionDomain` ([ADR-0090](0090-native-capability-broker-and-stateless-mcp-edge.md)).
- The MCP bridge exposes `launch_app { desktop_id, workspace_id?,
  new_workspace?, workspace_label? }` (`workspace_id` conflicts with
  `new_workspace`/`workspace_label`; `workspace_label` requires
  `new_workspace`) and a `switch_workspace` argument on `focus_window`
  (default `true`, mapping to `reveal`).

## Alternatives

### Move the window after it maps

Rejected because it races the user: the window maps on the current
workspace, may render and take focus there, and only then moves. Window
rules and the activation path would fire for the wrong workspace, and the
user sees the disruption the feature exists to prevent. Placement must be
decided at first map, before any of that happens.

### Require an Interaction Domain for every Agent window

Rejected as the only path. The Interaction Domain remains the stronger
isolation mechanism and is the right choice when the Agent needs input
injection, directed capture, or the sandbox; but it isolates by
transferring authority away from the human desktop. Opening a window on
another workspace of the same desktop is a placement problem, not an
authority problem, and should not require an Interaction Domain, a seat,
or a sandbox. The two mechanisms compose: launch placement covers
lightweight background windows, Interaction Domains cover supervised
workloads.

### Address workspaces by 1-based index, like `[[window_rule]]`

Rejected because workspace indices are positional, not identities. Under
the dynamic model they shift as workspaces are created and reaped, so an
Agent cannot durably name "workspace 2" by index — the same index may
mean a different workspace by map time. Launch placement uses durable
workspace ids for existing targets, or creates a fresh workspace
outright, which is exactly addressable.

## Consequences

An Agent can open and focus windows on a workspace it owns without moving
the user's view or stealing keyboard focus, and the user reaches those
windows only through an explicit workspace switch. Fresh workspaces are
ordinary dynamic workspaces, so the existing reaping and trailing-empty
invariants of the workspace model apply to them unchanged.

Placement matching is heuristic by construction. A user's manual launch
of the same application within the TTL can consume a stale pending entry
through the app_id FIFO, sending the user's window to the Agent's
workspace. This is accepted: the exposure is bounded by the 60-second TTL
and by single consumption — each entry places exactly one root toplevel,
so a stolen entry cannot affect more than one window and cannot recur.

A placement targets only the launch's first root toplevel; later windows
follow the ordinary rules, and transients follow their parent's
workspace. Launches through this path are unsandboxed desktop launches;
workloads that need the namespace sandbox, cgroup controls, or input
authority still require an Interaction Domain.
