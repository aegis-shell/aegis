# aegis-security

`aegis-security` contains the transport-neutral security mechanisms used by
the Aegis compositor and IPC broker.

The `authority` module owns Actor capability vocabulary, authenticated live
bindings, identity profiles, bounded sessions, exact resource grants,
semantic observation leases, resource scopes, and optimistic action
validation.

The `audit` module owns bounded live event projections and owner-only,
append-only, SHA-256-chained durable event storage. Callers supply their event
schema and reducer; the module owns ordering, durability, filesystem safety,
and replay verification.

The crate does not parse IPC, access Wayland objects, render chrome, run
agents, or dispatch input. IPC and compositor code adapt these mechanisms at
their respective boundaries.
