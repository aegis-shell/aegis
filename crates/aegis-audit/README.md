# aegis-audit

`aegis-audit` owns the transport-independent event-log mechanism used by
Aegis security and interaction domains. It provides a bounded in-memory
projection for live queries and an owner-only, append-only, SHA-256 chained
event store for durable audit and deterministic replay.

The crate does not know IPC commands, Wayland objects, Agent prompts, or GUI
policy. Callers supply serializable event payloads and own the reducer that
turns verified events into domain state.

Persistent logs fail closed on sequence gaps, hash-chain mismatches,
oversized records, unsafe file ownership or permissions, symlinks, malformed
JSON, and incomplete trailing records. A durable append is synced before it
is returned to the caller.

The store is event sourcing for compositor decisions and lifecycle events,
not a checkpoint of external Wayland client processes. Consumers can replay
verified events into their own reducer, but Aegis does not claim it can
resurrect application-owned state after a restart.
