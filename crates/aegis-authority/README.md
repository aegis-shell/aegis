# aegis-authority

`aegis-authority` is the transport-neutral policy kernel for Actor-scoped GUI
authority. It owns Actor capability vocabulary, authenticated live bindings,
identity profiles, bounded Actor sessions, exact resource grants, semantic
observation leases, resource scopes, and optimistic action-transaction
validation.

It does not parse IPC, access Wayland objects, render chrome, run agents, or
dispatch input. The IPC crate adapts its values to the wire, while the
compositor runtime supplies the current semantic snapshot and commits only a
successfully validated action batch.

The observation registry is bounded, uses cryptographically random tokens,
binds every lease to one connection and principal, and consumes an owning
Actor's token before checking semantic preconditions. A different Actor
cannot consume or revoke the lease.

Actor sessions have independent TTL and idle deadlines plus observation and
pending-action quotas. Filesystem paths, network origins, secret purposes,
and payment bounds are represented as exact resources behind random,
session/principal-bound, TTL/use-count-limited handles. A capability ceiling
alone never makes a concrete resource reachable.

`ActorCapability` answers what an Actor may attempt. Resource scope answers
where. `SemanticObservation` answers what the Actor perceived. An
`ActorActionIntent` binds all three to a single-use precondition; only the
compositor main loop can turn the validated intent into an
`ActorActionReceipt`.
