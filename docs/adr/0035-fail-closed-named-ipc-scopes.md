# ADR-0035: Fail-closed named IPC scope resolution

- Status: Accepted
- Date: 2026-07-16

## Context

[ADR-0034](0034-scoped-capabilities.md) introduced named IPC scopes and made
both an absent scope and an unknown explicit scope resolve to the unrestricted
default. The absent case preserves compatibility for ordinary tools such as
`ass-ctl`. The unknown case is different: a misspelled, removed, or stale
scope name requests a restriction but receives broader authority than the
caller named.

Scopes are hot-reloaded from the trusted compositor configuration. Resolving a
scope only during the handshake also lets an existing connection retain a
grant after the user restricts or removes that scope.

## Decision

Keep an absent scope name unrestricted for compatibility. Fail closed when a
client explicitly presents a scope name:

- The handshake succeeds only when the name exists in the current
  configuration. An unknown name returns an error and closes the connection.
- The server echoes the resolved scope in a successful handshake.
- Every command on a named connection resolves the scope again. A hot-reloaded
  restriction applies to the next command, and removing the scope revokes the
  connection's mutation authority.
- Invalid operation names never widen a configured allowlist. Valid operation
  names remain effective and invalid entries grant nothing.

Capability negotiation remains unchanged. A connection without a scope name
still receives the unrestricted resource scope bounded by its granted
capability classes.

## Alternatives

- **Keep unknown names unrestricted.** Rejected because configuration mistakes
  and stale clients silently gain more authority than requested.
- **Resolve only at handshake.** Rejected because configuration reload could
  not revoke or narrow existing connections.
- **Disconnect every scoped client on any configuration reload.** Rejected
  because per-command resolution provides immediate enforcement without
  disrupting connections whose scopes did not change.

## Consequences

- Scoped clients must handle handshake refusal when their configured name no
  longer exists.
- Scope lookup occurs on each mutation. The configuration is small and the
  lookup is bounded, so the security property outweighs the lock and map
  lookup cost.
- Removing a scope is an immediate revocation mechanism for control commands.
- Query-only tools and existing unscoped clients remain compatible.
