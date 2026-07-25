# ADR-0031: The agent as a scoped IPC client (M10 framing)

- Status: Accepted
- Date: 2026-06-24

## Context

[Vision and Scope](../explanation/vision.md#the-agent-phase) commits ass to a
second phase after the desktop is stable: the same compositor an agent can
understand and operate through. The phase is described at the level of intent
as "an automation contract layered on the IPC: stable identifiers for windows
and workspaces, a journal of mutations the agent can replay, and a capability
model so the agent can act only where permitted. The agent is never a special
client of the compositor; it is an IPC client with a defined scope."

[ADR-0027](0027-ipc-and-introspection.md) already fixes the seam the agent
will use: a versioned, schema-driven IPC over `$XDG_RUNTIME_DIR/aegis.sock`,
with three capability classes (`query`, `control`, `session`). It states
explicitly that "the agent in M10 connects as a `control`-class client under
a user-approved scope", and that accessibility is handled as a separate M9
output path, not as the extension surface. [ADR-0001](0001-scope-and-responsibility-boundary.md)
closes with the same intent: the shared model in `aegis-core` "can later grow
the semantic surface the AI-adaptation phase needs."

Today's state, measured against that intent:

| Intent (vision.md) | Today |
|--------------------|-------|
| Stable identifiers for windows, workspaces, outputs | `WorkspaceId` and `OutputId` are stable for the life of their object ([ADR-0025](0025-workspace-model.md), [ADR-0028](0028-output-and-monitor-model.md)). `Window.id` is the surface resource's address as `usize` (`crates/aegis-core/src/window.rs`): stable for a window's life, but reused after the surface is destroyed. |
| A journaled mutation log the agent can replay | Absent. Mutations arrive from chrome, keybindings, and the IPC; the only trace is the coarse `WindowsChanged` / `WorkspaceChanged` event stream, which names that *something* changed but not *what*. |
| A capability model that bounds what the agent may do | Three flat classes ([ADR-0027](0027-ipc-and-introspection.md)). A `control` client can focus, close, move, or quit any window. There is no per-resource, per-operation, or per-session scope. |
| The agent is an IPC client, not a special client | The seam is in place; no special-client code path exists. |

The desktop-phase milestones M3 through M9 build the model the agent reads.
M10 is the contract that lets an out-of-process agent act on that model
without the compositor gaining a model, a prompt, or a tool runtime of its
own. This ADR records the framing decision and names the follow-on ADRs that
will deliver it.

## Decision

M10 is delivered as **extensions to `aegis-core` and `aegis-ipc`**, not as an
agent runtime inside the compositor. The agent is, and remains, an
out-of-process IPC client. The compositor gains four things and refuses a
fifth.

**Four additions, each in its own ADR:**

1. **[ADR-0032](0032-durable-window-identifiers.md): Durable window
   identifiers.** A window id is never reused, even after the surface is
   destroyed and its address is recycled. The journal, scoped capabilities,
   and any external agent all address windows by an id whose meaning does not
   depend on when it is read. This is foundational; every other M10
   construct refers to window ids.

2. **[ADR-0033](0033-mutation-journal.md): The mutation journal.** An
   append-only, subscribable log of every `Command`
   ([ADR-0027](0027-ipc-and-introspection.md)) the compositor applies,
   regardless of origin (chrome, keybinding, or IPC). The agent subscribes,
   replays, and reasons about history. Replaces the current
   `WindowsChanged` / `WorkspaceChanged` "something changed" signal with a
   precise record of *what* changed.

3. **[ADR-0034](0034-scoped-capabilities.md): Scoped capabilities.**
   Per-resource and per-operation grants layered on the three classes from
   ADR-0027. A user-approved scope is "this client may focus and close
   windows on workspace 3; nothing else". The agent is bounded by the scope
   it was granted, not by what it asks for.

4. **A perceptual-path decision (follow-on ADR, number to be assigned).**
   Whether M10 exposes any pixel-level capture to the agent, or relies
   entirely on the structured model plus the M9 accessibility output, is left
   open. The default is the structured path; raw pixels are a privacy-sensitive
   exception that must earn their own capability gate. The follow-on ADR
   records the criteria (which agent tasks the structured path cannot satisfy)
   and the gate if the answer is yes.

**One refusal, stated outright:** the compositor does not gain an in-process
skill, prompt, tool-definition, or model layer. The "skill layer" or "tool
wrapper" that turns IPC primitives into agent-callable functions lives out of
tree, consumes the IPC, and churns at the rate models churn — not at the rate
the compositor ships. The compositor's contract with the agent is the IPC.

**Three non-decisions, kept consistent with ADR-0027:**

- No new "agent" capability class. The agent holds `control` (and, if the
  perceptual-path ADR grants it, a new perceptual capability) under a scope.
  It is not promoted to a peer of `session`.
- No special "agent" code path in the server. The agent's `Do` requests are
  dispatched through the same main-loop handler as a status bar's.
- No in-process model, inference, or prompt storage. The compositor is
  model-free; the agent brings whatever model it runs.

## Alternatives

- **A model-aware compositor (inherit AI in-process).** Rejected. It couples
  the slowest-moving layer (the compositor binary, on a multi-year cadence)
  to the fastest (models, on a weekly cadence); couples the smallest surface
  (the binary) to the largest (model weights, prompts, tool schemas); and
  makes the agent vendor-specific by construction. The skill layer's churn
  would destabilize the compositor, or the compositor would freeze the skill
  layer — either failure is unacceptable.

- **A bespoke agent socket separate from the introspection IPC.** Rejected.
  Two surfaces always diverge; every new chrome operation would have to be
  exposed twice, and the "one model, many readers" principle
  ([Vision](../explanation/vision.md#design-principles)) would be violated.
  The introspection IPC *is* the agent surface.

- **The accessibility API as the primary agent surface (the macOS model).**
  Rejected as primary, retained as output, exactly as
  [ADR-0027](0027-ipc-and-introspection.md) already records. Accessibility is
  observability-heavy and control-light; the agent needs both. Accessibility
  lands as an M9 output path the agent may read, not as the seam it speaks.

- **An in-tree skill / tool layer owned by the compositor.** Rejected.
  Skills, prompts, and tool schemas change at model cadence; the compositor
  ships at OS cadence. Bundling them either freezes the agent or
  destabilizes the compositor. The skill layer is one or more separate
  projects that consume this IPC.

- **Deferring the perceptual-path decision into M10's framing.** Rejected as
  premature. The structured model plus M9 accessibility may cover the agent's
  needs; the case for pixels has to be made against measured coverage, not
  assumed. The decision gets its own ADR once the structured path is
  exercisable end to end.

## Consequences

- M10 is delivered across three named follow-on ADRs (0032 durable window
  ids, 0033 mutation journal, 0034 scoped capabilities) plus the
  perceptual-path ADR when it opens. This ADR is the entry point; the
  follow-ons are the work.
- `aegis-core` gains durable window id support and the journal's append-only
  record type. `aegis-ipc` gains the journal subscription, the scoped
  capability handshake, and (only if the perceptual ADR grants it) a
  capture request. The binary gains the server-side handlers. No new
  long-lived agent process is added in tree.
- Schema versioning ([ADR-0027](0027-ipc-and-introspection.md)) becomes
  load-bearing for the agent: an agent pinned against schema vN must keep
  working against vN.x. The major-version handshake at `Hello` is the
  contract.
- The `WindowsChanged` / `WorkspaceChanged` event stream is superseded for
  agent use by the journal; it remains for status-bar clients that only
  need a re-query signal. The journal is additive, not a replacement.
- The agent's "skills" are out-of-tree. The compositor's contract is the
  IPC; anything an agent does that the IPC cannot express is out of scope
  for M10 and is surfaced as a follow-on ADR, not as a special client.
- **M10 verification.** An external agent process — using only the
  documented IPC, with no in-tree code changes for that agent — can (a)
  read the window/workspace/output model by stable id, (b) subscribe to the
  mutation journal and reconstruct recent history, (c) perform a
  user-scoped sequence of focus/close/move operations bounded by its
  granted scope, and (d) be refused any operation outside that scope. The
  agent runs unmodified against a schema vN.x bump.
