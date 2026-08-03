# ADR-0074: Generic Agent Workspaces status surface

- Status: Superseded by [ADR-0103](0103-actor-authority-and-interaction-domain-architecture.md)
- Date: 2026-07-29

## Context

[ADR-0050](0050-fuji-agent-product-and-bridge-rename.md) defines fuji as one
concrete, out-of-process agent product.
[ADR-0060](0060-statusbar-system-controls-and-live-system-ipc.md) retains a
compositor-owned workspace manager, but describes its status-bar entry as the
Fuji indicator.

The status bar receives the authoritative Agent Realm snapshot. It does not
receive fuji process presence, session state, or a stable owner identity for
each Realm. Presenting an empty snapshot as "Fuji Ready" therefore claims
liveness that the compositor cannot verify. Aggregating every Agent Realm
under the Fuji name also makes a first-party agent product appear to own
workspaces created by the user, `aegis-cli`, or another scoped client.

The compositor-owned manager must remain reachable when an agent is stopped
or broken so the user can inspect and revoke interaction authority.

## Decision

The user-facing generic surface is named **Agent Workspaces**. Its stable
implementation identities remain `aegis-ai-workspaces` and
`BuiltInApplication::AiWorkspaces` for compatibility.

The status bar exposes one permanent Agent Workspaces entry derived only from
the Agent Realm snapshot:

- no live Realm reports no active workspaces;
- one live Realm shows that Realm's own label and state;
- multiple live Realms show a localized count and their aggregate state; and
- a mixture of active and paused Realms reports a partially paused state.

The entry opens the generic Agent Workspaces manager. The manager describes
manual creation as creating an empty workspace because that operation does
not start or connect an agent.

The Fuji name is reserved for the fuji agent product, its bridge, its
notifications, and its bridge-managed Realm label. Compositor chrome must not
infer fuji liveness or Realm ownership from a display label. A future
Fuji-specific status surface requires an explicit, stable agent identity and
presence or lease signal.

The existing process boundary remains unchanged: Agent Workspaces presents
human-owned Realm authority, the compositor enforces it, and fuji reaches it
only through the scoped out-of-process bridge.

## Alternatives

- **Keep the generic aggregate branded as Fuji.** Rejected because the label
  asserts agent identity and liveness that the available snapshot does not
  contain.
- **Treat a Realm labeled `Fuji` as proof of ownership and presence.**
  Rejected because labels are mutable presentation data, not durable
  identities.
- **Merge Agent Workspaces into fuji.** Rejected because authority recovery
  must remain available when fuji is not running, and model, credential, and
  network code must stay outside the compositor.
- **Remove the generic launcher and status entry.** Rejected because it would
  remove the agent-independent recovery path for inspecting and revoking
  authority.

## Consequences

- The status bar and launcher describe the same generic authority surface.
- A Fuji-managed Realm still appears by its `Fuji` label without causing
  unrelated Realms to inherit Fuji branding.
- Empty workspace creation is explicit and no longer implies that an agent
  was started.
- The compositor still cannot show whether the fuji process or chat session
  is online. Adding that state requires a separate identity and presence
  contract rather than UI heuristics.
