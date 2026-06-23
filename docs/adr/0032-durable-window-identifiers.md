# ADR-0032: Durable window identifiers

- Status: Accepted
- Date: 2026-06-24

## Context

[ADR-0031](0031-agent-as-scoped-ipc-client.md) makes durable window
identifiers the foundation of M10: the mutation journal
([ADR-0033](0033-mutation-journal.md)) must reference windows unambiguously,
and scoped capabilities
([ADR-0034](0034-scoped-capabilities.md)) must enumerate windows by stable
id. A journal entry that says "focused window 0x4002" is meaningless if
`0x4002` can refer to two different windows depending on when it is read.

Today `Window.id` (`crates/ass-core/src/window.rs`) is the wl_surface
resource's address as `usize`, set by the server at `Window::new`. The
resource address is stable for the life of the surface, but libwayland
recycles addresses after `wl_resource_destroy`, so a window mapped later can
reuse a closed window's id. The comment on the field claims stability; the
property does not hold across window lifetimes.

`WorkspaceId(pub u64)` and `OutputId(pub u64)`
([ADR-0025](0025-workspace-model.md),
[ADR-0028](0028-output-and-monitor-model.md)) already solve the analogous
problem: a `WorkspaceModel` allocates them monotonically and never reuses
them within the process lifetime. Windows are the remaining first-class
model entity without that property.

The agent is not the only consumer. `Workspace.toplevels: Vec<usize>`
(`crates/ass-core/src/workspace.rs`) and every IPC `Command` variant that
targets a window (`Focus { id }`, `Close { id }`, `Move { id }`,
`MoveToWorkspace { window, .. }`) currently use the fragile `usize` and
would silently misaddress a window after an id is recycled.

## Decision

Introduce `WindowId(pub u64)` in `ass-core::window`, allocated monotonically
by the compositor, never reused within the process lifetime. The wl_surface
resource address stays internal to the server and is no longer the window's
identity.

**Allocation.** The server holds a `next_window_id: u64` counter and
assigns a fresh `WindowId` on each `xdg_toplevel` map. The counter mirrors
`WorkspaceModel::next_workspace_id` and is allocated on the main loop, not
from a connection thread.

**Mapping.** The server keeps a per-surface `WindowId` for the life of the
surface. On unmap or destroy the `WindowId` is *retired*: it remains valid
as a reference (the journal may cite it; `GetWindows` no longer returns
it) but is never reassigned. The agent addresses a window by id without
concerning itself with whether the window is currently mapped.

**Type changes.** `Window.id` becomes `WindowId`.
`Workspace.toplevels` and `WorkspaceEntry.toplevels` become
`Vec<WindowId>`. The IPC `Command` variants that target a window (`Focus`,
`Close`, `Move`, `MoveToWorkspace`) take `WindowId`. The newtype is
`Copy + Eq + Hash + serde::Serialize + serde::Deserialize`, matching
`WorkspaceId`.

**Wire compatibility.** The change replaces a `usize` (serialized as a JSON
number) with a `WindowId` (the inner `u64`, also a JSON number). The
on-wire encoding is unchanged; the semantic guarantee changes. Per
[ADR-0027](0027-ipc-and-introspection.md), a client written against schema
vN continues to work against vN.x — but the guarantee an agent now relies
on (non-reuse) did not exist in v1, so the compositor bumps
`PROTOCOL_VERSION` to `2` and refuses v1 at the `Hello` handshake. The
v1→v2 migration note names this ADR: existing in-tree clients (`ass-ctl`)
recompile against the new `ass-core`; the wire is byte-compatible, the
contract is strengthened.

**Cross-process durability is not a goal.** A `WindowId` is stable within
one compositor process. If the compositor restarts, ids reset; the agent
reconnects and re-queries. Persisting ids across restarts would require a
stable on-disk store and would buy nothing the agent needs: an agent cannot
act on a window that no longer exists.

## Alternatives

- **Keep `Window.id` as the surface address; add a separate durable id.**
  Rejected: two identities for one entity always diverge. Every consumer
  (renderer, chrome, IPC, journal, scope) would have to remember which to
  use, and the wrong choice would silently corrupt references. There is one
  window; there is one id.

- **Use a `Uuid` for window ids.** Rejected: `WorkspaceId` and `OutputId`
  are `u64` for the same reason — opaque, `Copy`, cheap to compare and hash,
  and large enough (`2^64` windows at one million per second is 584
  millennia) that exhaustion is not a concern. A `Uuid` adds 16 bytes per
  reference for a property the agent does not need.

- **Make the wl_surface address durable by preventing libwayland's
  recycling.** Rejected: it reaches into a dependency's allocator to change
  a property the dependency does not promise, and couples ass's identity
  model to libwayland's implementation detail. Allocating the id in
  `ass-core` is cheaper and owned.

- **Encode creation metadata (timestamp, app_id) into the id.** Rejected:
  ids stay opaque, matching `WorkspaceId` / `OutputId`. Metadata lives on
  the `Window` struct and in the journal entry, not in the id.

## Consequences

- `ass-core::window` gains `WindowId(pub u64)`; `Window.id`,
  `Workspace.toplevels`, `WorkspaceEntry.toplevels`, and the IPC `Command`
  variants change type. `ass-ctl` and any out-of-tree consumer recompile.
- `PROTOCOL_VERSION` becomes `2`. The v1→v2 migration note names this ADR.
  v1 clients are refused at the handshake, loudly, as
  [ADR-0027](0027-ipc-and-introspection.md) already promises for any
  major-version mismatch.
- The mutation journal ([ADR-0033](0033-mutation-journal.md)) cites retired
  ids without ambiguity; a journal reader can tell "window 42 closed" from
  "window 42 focused" because 42 is never reassigned.
- Scoped capabilities
  ([ADR-0034](0034-scoped-capabilities.md)) enumerate windows by id; a
  scope held against a closed window simply fails to match anything live.
- The agent addressing a window by an id read from an old snapshot may find
  the window gone; it discovers this by `GetWindows` no longer listing the
  id, or by a journal `Refused` effect, not by misaddressing a different
  window.
- A future on-disk session-restore mechanism would need to remap ids rather
  than persist them; this is consistent with workspace and output ids,
  which also do not persist across restarts.
