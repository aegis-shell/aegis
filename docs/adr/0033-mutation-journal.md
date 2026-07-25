# ADR-0033: The mutation journal

- Status: Accepted
- Date: 2026-06-24

## Context

[ADR-0027](0027-ipc-and-introspection.md) ships an event stream of
`WindowsChanged` and `WorkspaceChanged`: precise about *that* something
changed, silent about *what*. A status bar re-queries and moves on; an
agent cannot. To reason about state the agent must reconstruct history:
"the user focused window 5, then moved it to workspace 2, then closed it" —
information the compositor has, but does not expose.

[ADR-0031](0031-agent-as-scoped-ipc-client.md) names a "journaled mutation
log the agent can replay" as one of M10's three deliverables. The journal
makes the compositor's *decisions* visible: every `Command`
([ADR-0027](0027-ipc-and-introspection.md)) the compositor applies, tagged
with origin and outcome, in order.

The journal is deliberately not a log of everything that happened in the
world. A client mapping a new `xdg_toplevel` is not a `Command`; it is
external state arriving. The compositor's window-map signal (today's
`WindowsChanged`) reports it. The journal reports what the compositor *did*,
the map signal reports what *happened*. An agent subscribes to both.

## Decision

ass maintains an **in-memory, append-only ring buffer of `JournalEntry`
records**, one per `Command` applied by the compositor, subscribable over
the IPC. The coarse `WindowsChanged` / `WorkspaceChanged` stream is
retained for status-bar clients that only need a re-query signal.

**Entry shape.** Each entry carries a monotonic sequence number, a
timestamp, the origin of the command, the command itself, and the effect
the compositor observed when applying it.

```text
JournalEntry {
    seq: u64,
    ts_mono_ms: u64,
    origin: Origin,
    cmd: Command,
    effect: Effect,
}
```

`Origin` distinguishes who caused the command: `Chrome`, `Keybinding`,
`Ipc(conn_id)`, or `Internal` (for cleanup the compositor drives itself,
such as closing a window whose client vanished). The agent filters its own
echoes (commands it issued return as `Origin::Ipc(self)`) and models user
intent (a `Keybinding` origin is a direct user signal).

`Effect` records the outcome: `Applied`, `Refused(reason)`, or `NoOp`.
Without `Effect`, the agent must re-query after every command to verify;
the field halves the round trips and removes the race between "command
queued" and "state visible in `GetWindows`".

**Capacity.** The ring is bounded; default 4096 entries, configurable
through the configuration system
([ADR-0026](0026-configuration-system.md)). `seq` is monotonic across
wraps: a subscriber requests `seq >= last_seen`, and if `last_seen` is
older than the ring's oldest entry, the server sends one gap marker so the
client knows to re-query rather than reason over a partial history.
Unbounded and file-backed are both rejected: the journal is
recent-history-for-reasoning, not an audit log, and the agent reconnects
and re-queries across compositor restarts.

**Single application chokepoint.** The compositor routes every mutation —
from chrome, keybindings, the IPC, or internal cleanup — through one
`apply(cmd, origin) -> Effect` function on the main loop. `apply` appends
to the journal, dispatches to the existing window-management code, and
returns the effect. Today chrome and keybindings call window-management
functions directly; this ADR requires them to call `apply` instead, so the
journal sees every mutation regardless of origin. The IPC
`Handler::command` already queues onto the main loop; it calls `apply`
with `Origin::Ipc(conn_id)`.

**IPC surface.** Three additions to the schema, additive under schema v2
([ADR-0032](0032-durable-window-identifiers.md)):

- `Request::GetJournal { since: u64 }` returns entries with `seq > since`,
  plus `oldest_seq` and `latest_seq` so the client detects gaps.
- `Request::SubscribeJournal` opts into a new `Event::Journal { entry }`
  push. It is separate from `Request::Subscribe` (the coarse stream):
  status bars want one signal per visible change, not a flood of
  per-command entries; agents want the flood. Folding would force each
  consumer to filter the other's noise.
- `Response::Journal { entries, oldest_seq, latest_seq }`.

**What is journaled.** Every `Command` variant that
[ADR-0027](0027-ipc-and-introspection.md) defines. `Quit` is journaled
(origin-tagged) so the agent can observe "the user quit" in the last entry
before the connection drops. What is *not* journaled: window map/unmap,
title changes, and focus shifts caused by a window closing rather than by a
`Focus` command. Those are state changes the compositor *observes*, not
decisions it *makes*; the coarse event stream already signals them.

## Alternatives

- **Fold the journal into the existing event stream.** Rejected: status-bar
  clients would have to filter per-command entries to recover the
  "something changed, re-query" signal they want today. The two streams
  serve different consumers and stay separate.

- **An unbounded, file-backed audit log.** Rejected: the journal is for
  reasoning about recent history, not for compliance. A multi-gigabyte log
  hurts the compositor and buys nothing the agent needs; cross-restart
  continuity is not a goal
  ([ADR-0031](0031-agent-as-scoped-ipc-client.md)).

- **Journal every state change, including maps and unmaps.** Rejected: it
  conflates the compositor's decisions with the world's events. The agent
  that wants both subscribes to both; conflating them produces a stream
  whose entries have incompatible shapes and unclear semantics.

- **A separate journal socket, distinct from the introspection IPC.**
  Rejected for the same reason as
  [ADR-0031](0031-agent-as-scoped-ipc-client.md)'s refusal of a separate
  agent socket: two surfaces diverge. The journal is an extension of the
  introspection IPC.

- **Skip `Effect`, let the agent verify by re-query.** Rejected: doubles
  the round trips and opens a race between "command queued" and "state
  visible in `GetWindows`". The compositor knows the effect at apply time;
  recording it is cheap.

## Consequences

- `aegis-core` gains a `JournalEntry` type and an `apply` chokepoint; the
  compositor's chrome and keybinding paths route through `apply`. The IPC
  gains `GetJournal`, `SubscribeJournal`, and the `Journal` event.
- Every chrome and keybinding mutation becomes observable by the agent,
  which closes the "implicit state" gap
  [Vision](../explanation/vision.md#the-agent-phase) names: there are no
  hidden mutations.
- The coarse event stream and the journal coexist; the agent subscribes to
  both, the status bar to one.
- The `Origin` field makes the agent's own echoes filterable, which is
  required for the agent to learn from user-initiated commands without
  fighting itself.
- The ring capacity is a tunable; the gap marker is the contract that lets
  a client detect it has fallen behind and recover by re-query.
- Scoped capabilities
  ([ADR-0034](0034-scoped-capabilities.md)) record refusals through
  `Effect::Refused`, so a scope violation is visible in the journal as a
  refused entry, not silently dropped.
