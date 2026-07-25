# ADR-0034: Scoped capabilities

- Status: Superseded by ADR-0035
- Date: 2026-06-24

## Context

[ADR-0027](0027-ipc-and-introspection.md) defines three capability classes —
`query`, `control`, `session` — and grants them at the `Hello` handshake.
The classes bound *what kind* of action a client may take. They do not bound
*which resources* a client may touch: a `control` client can `Focus`,
`Close`, or `Move` any window in the compositor, on any workspace, on any
output.

A status bar holds `query` and the distinction is moot. An agent holds
`control` ([ADR-0031](0031-agent-as-scoped-ipc-client.md)) and the
distinction is the whole game: an agent trusted to focus one window must not
be trusted to close every window. "The agent connects as a `control`-class
client under a user-approved scope"
([ADR-0027](0027-ipc-and-introspection.md)) commits ass to a scope model
that ADR-0027 names but does not define. This ADR defines it.

The constraint, restated: scope must bound the agent without making it a
special client. The compositor has no UI to ask "do you trust this
connection?"; scope must come from a source the compositor already trusts.

## Decision

Add a **`Scope`**, separate from the three capability classes, granted at
the `Hello` handshake and checked on every `Command` dispatch. A client's
effective permission is `caps AND scope`. The default scope (no scope
presented) is "all resources, all operations within `caps`" — preserves the
existing behavior for status bars and `ass-ctl`.

**Shape.** A scope is an allowlist of resources and operation classes.
`None` at any field means "unrestricted at this axis"; `Some(set)` means
"only these".

```text
Scope {
    windows: Option<Vec<WindowId>>,
    workspaces: Option<Vec<WorkspaceId>>,
    outputs: Option<Vec<OutputId>>,
    ops: Option<Vec<OpClass>>,
}
```

`OpClass` enumerates the operation families behind the `Command` variants
that [ADR-0027](0027-ipc-and-introspection.md) defines: `Focus`, `Close`,
`Move`, `Cycle`, `SwitchWorkspace`, `SwitchWorkspaceTo`,
`MoveToWorkspace`, `ToggleTiling`, `Notify`, `DismissNotification`. The
list is closed (one variant per relevant `Command`); a new `Command`
variant adds a matching `OpClass` in the same change.

**Caps vs. scope.** Caps are *classes* — coarse, stable, about action
kind. Scope is *resources* — fine, dynamic, about which object. Conflating
them would force every per-resource grant to become a new cap class,
exploding the enum. They stay separate; both must allow an action.

**Static grant from configuration.** Scopes are declared in the
configuration file ([ADR-0026](0026-configuration-system.md)) as named
records. The client presents a scope *name* (an opaque string) at `Hello`;
the server resolves the name to a `Scope` from config. The client does not
self-assert a `Scope`: a self-asserted allowlist is self-granting and the
model collapses. The compositor has no UI; the configuration file is the
trusted source.

```toml
[[agent.scopes]]
name = "browser-helper"
ops = ["Focus", "Close"]
# windows omitted → unrestricted at this axis

[[agent.scopes]]
name = "workspace-rover"
ops = ["SwitchWorkspace", "SwitchWorkspaceTo", "MoveToWorkspace"]
# workspaces omitted → unrestricted
```

**Handshake.** `Request::Hello` gains an optional `scope: Option<String>`
(the name). The server looks up the name; if unknown or absent, the
default unscoped `Scope` is used (back-compat for every existing client).
`Response::Hello` echoes the granted `Scope` so the client knows its
actual bounds, not just the name it asked for.

**Enforcement.** Every `Command` is checked at dispatch:

- A window-targeted command (`Focus`, `Close`, `Move`, `MoveToWorkspace`)
  requires its `WindowId`
  ([ADR-0032](0032-durable-window-identifiers.md)) to be in
  `scope.windows`, or `scope.windows` to be `None`.
- A workspace-targeted command (`SwitchWorkspaceTo`, `MoveToWorkspace`)
  requires its `WorkspaceId` to be in `scope.workspaces`, or the field to
  be `None`.
- `ToggleTiling` requires the focused output's current workspace to be in
  `scope.workspaces` (or the field `None`).
- Every command requires its `OpClass` to be in `scope.ops`, or `scope.ops`
  to be `None`.

Failure produces `Response::Error { message: "out of scope" }` and a
journal entry ([ADR-0033](0033-mutation-journal.md)) with
`Effect::Refused("out of scope")`. The agent learns the boundary by
observing refusals; the user sees the boundary enforced without UI prompts.

**Dynamic per-session binding is deferred.** The first cut is static: a
named scope from config grants a fixed bound. The common case — "scope me
to the window I just opened" — needs the client to nominate a subset of
its granted resources at `Hello` (for example,
`bind: Some(BoundResources { windows: vec![w42], .. })`), validated against
the named scope's outer bound. This refinement lands in a follow-on ADR
once the static model is exercised; it is the natural next cut but not the
first.

## Alternatives

- **A new "agent" capability class.** Rejected: caps classify *action
  kind*, not resources; an "agent" class would say nothing about *which*
  window. [ADR-0031](0031-agent-as-scoped-ipc-client.md) already refuses
  it.

- **Self-asserted scopes (client declares its own `Scope`).** Rejected: a
  self-asserted allowlist is self-granting. The compositor would have no
  grounds to refuse. Scope must be granted by a trusted source; config is
  that source.

- **A separate auth daemon (polkit-style).** Rejected: it would be a
  special client of the compositor, which
  [ADR-0031](0031-agent-as-scoped-ipc-client.md) refuses on principle. It
  would also reintroduce a system-bus model
  [ADR-0027](0027-ipc-and-introspection.md) already rejected.

- **Per-window sandboxing at the surface level (a per-surface ACL in the
  Wayland protocol).** Rejected: the window-management operations the
  agent performs (focus, close, move) are compositor-internal; the
  Wayland protocol does not see them. Per-surface ACLs would not help and
  would reach into a layer that does not own the operations.

- **OAuth-style bearer tokens, one per resource.** Rejected: explosion of
  tokens for a single-user compositor. A named scope from config gives the
  same property at a fraction of the moving parts.

## Consequences

- `aegis-ipc`'s `Hello` gains `scope: Option<String>` (the name); the schema
  gains `Scope` and `OpClass`. `aegis-config`
  ([ADR-0026](0026-configuration-system.md)) gains an `[agent.scopes]`
  section.
- The default (no scope name presented) is unscoped, preserving
  back-compat for `ass-ctl` and any status bar. Nothing existing breaks.
- Every `Command` dispatch performs a scope check in addition to the cap
  check; a refusal is recorded in the journal
  ([ADR-0033](0033-mutation-journal.md)) with `Effect::Refused`.
- The agent learns its actual bounds from `Response::Hello`, which echoes
  the granted `Scope`; it does not infer them from refusal alone.
- Dynamic per-session binding is named as the follow-on; until it lands,
  an agent scoped to `browser-helper` may focus any window the config
  allows, which is coarser than "the one window I opened". The static
  model is the floor, not the ceiling.
- The configuration file becomes the security boundary for the agent,
  which is consistent with the desktop phase: configuration is data, not
  code, and the trusted source is the user-edited file.
