# ADR-0040: Realms, seats, and transferable interaction authority

- Status: Superseded by [ADR-0103](0103-actor-authority-and-interaction-domain-architecture.md)
- Date: 2026-07-18

## Context

The agent automation path can address a window and inject target-local input,
but it still uses the compositor's single pointer and keyboard focus. An
agent action therefore competes with the human user for activation, modifier
state, implicit grabs, selection, drag-and-drop, and text input even when the
physical pointer does not move.

Running a second nested compositor gives the agent an independent seat, but a
Wayland surface belongs to the client connection that created it. An already
mapped surface cannot move to another compositor connection. A nested server
would consequently make "drag this live window into the agent workspace"
either a relaunch or a visual proxy rather than a transfer of the same
window.

The core Wayland protocol already models independent input subjects as
multiple `wl_seat` globals. The compositor also already owns every mapped
surface, its render placement, and its input routing. The missing abstraction
is not another display server; it is a compositor-internal authority and
presentation domain.

## Decision

ass becomes a single **surface capability kernel** with first-class
**realms**, **principals**, **seats**, and **interaction groups**.

A realm is an isolated presentation and interaction domain. The physical
desktop is the human realm. Each agent workspace is an agent realm with its
own virtual outputs and one or more independent seats. The secure lock state
is a secure realm. Workspaces remain the layout subdivision within a realm;
they are not an authority boundary.

A principal identifies one subject that may hold authority: a human, an
agent, or the compositor system. A seat belongs to exactly one principal and
one realm. Pointer focus, keyboard focus, held input, modifiers, grabs,
selection, primary selection, drag-and-drop, gestures, tablet state, and text
input are all per-seat state.

An interaction group is the smallest unit whose control authority transfers
atomically. It contains a complete interactive surface tree and may contain
multiple toplevels when protocol correctness requires them to move together.
Every live window belongs to exactly one interaction group. Every interaction
group has exactly one controlling realm and zero or more observing realms.
The controlling realm receives input. Observing realms may render a read-only
mirror and never receive client input for that group.

Dragging a window into an agent workspace transfers its interaction group's
control realm. The transfer:

1. quiesces held input, grabs, drag-and-drop, and text input on the source
   seat;
2. commits one model revision that changes the controlling realm;
3. optionally retains the source realm as an observer;
4. makes the group addressable only by seats attached to the target realm;
5. records the transfer in the mutation journal with the before and after
   model revisions.

The transfer never reparents or recreates a `wl_surface`. Rendering selects
the group from the shared surface graph for the target realm and renders
mirrors for its observing realms.

The compositor advertises independent Wayland seats for independent realms.
A client must bind a realm's seat to receive input from it. ass conservatively
puts every toplevel owned by one Wayland client connection in one interaction
group and transfers that group as a unit. This makes the normal case entirely
independent of application multi-seat behavior: one application instance,
including its dialogs and helper windows, has one controlling Realm at a time.
Observed native multi-seat behavior is used to route the client's seat
resources correctly, but does not automatically split its interaction group.
Any future group-splitting API must be explicit and may only operate after
native multi-seat behavior is proven; ass never fabricates contradictory
focus events on one seat.

Agent models, prompts, and tool runtimes remain out of process as required by
[ADR-0031](0031-agent-as-scoped-ipc-client.md). The IPC grants a principal a
revocable capability lease for a realm; it does not make the agent a
compositor-internal model runtime.

## Alternatives

- **One nested compositor per agent.** Rejected as the primary abstraction.
  It isolates input for newly launched clients but cannot transfer an
  existing Wayland surface, duplicates composition, and fragments the window
  and journal model across servers. The nested backend remains a development,
  compatibility, and strong process-isolation target.
- **Keep one seat and switch focus around each agent action.** Rejected
  because human and agent input still races through one modifier, focus,
  selection, and grab state. Rapid focus switching is visible to clients and
  cannot provide simultaneous interaction.
- **Send input directly to a target while preserving a different focus on the
  same seat.** Rejected because it violates the Wayland seat model and becomes
  ambiguous for serial validation, popups, pointer constraints,
  drag-and-drop, and text input.
- **Make a workspace the authority boundary.** Rejected because workspaces
  organize windows for layout while realms isolate subjects, input, capture,
  and security policy. Combining the axes prevents an agent realm from having
  normal workspaces and makes human observation of an agent realm a special
  case.
- **Require every toplevel to transfer independently.** Rejected because a
  single client connection may not bind multiple seats and transient surface
  trees must remain interactive as one unit. The production policy keeps one
  Wayland client connection in one interaction group. Per-window splitting is
  outside this decision and would require a separate explicit contract even
  for a client with proven native multi-seat behavior.

## Consequences

- [ADR-0009](0009-input-pipeline-and-pointer-focus.md) and
  [ADR-0010](0010-keyboard-pipeline-and-xkbcommon-ownership.md) remain the
  history of the first physical seat but their single-focus state model is
  superseded by this decision.
- `aegis-core` gains durable identifiers and pure models for realms,
  principals, seats, clients, interaction groups, observation, lifecycle,
  and atomic authority transfer.
- `aegis-compositor` replaces global input and data-device fields with per-seat
  state and advertises multiple `wl_seat` globals. The existing physical
  input path becomes the human seat rather than a special global path.
- Rendering becomes realm-selective. Physical and virtual outputs consume
  the same surface graph; observing realms draw mirrors without adding those
  mirrors to input hit-testing.
- IPC authorization becomes default-deny and lease-based. A lease binds a
  principal, realm, operation set, and lifetime. Scope names remain
  configuration references, not credentials.
- Capture is realm- and window-scoped, damage-driven, and correlated with a
  model revision and journal sequence. PNG screenshots remain a user tool,
  not the primary continuous agent transport.
- Locking, pausing, revocation, client destruction, and realm teardown must
  cancel held state before changing authority. Fail-closed teardown is a
  production invariant, not an optional polish path.
- Process, filesystem, network, and system-bus isolation remain the sandbox
  runtime's responsibility. Realm transfer changes display and interaction
  authority; it does not retroactively move a process into Linux namespaces.
