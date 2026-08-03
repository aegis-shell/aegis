# ADR-0102: Actor-scoped semantic observation and transactional actions

- Status: Accepted
- Date: 2026-08-03

## Context

The Realm, multi-seat, subject binding, capability broker, and mutation
journal already prevent an Agent from becoming an unbounded physical input
source. The first Realm-input contract still treated capture metadata as an
informal hint: an Agent could submit a later target-local input command
without proving which state it had observed. A window could move, change
authority, become disabled, or be replaced between observation and dispatch.
The asynchronous acknowledgement also could not prove that the main loop had
accepted the action.

The query capability was similarly too coarse for authenticated Actors. It
distinguished reading from mutation at the connection level, but it did not
separate notification, journal, window, workspace, output, settings, system,
and Realm observations. Resource allowlists were enforced on mutation while
some snapshot replies still exposed global state. The journal identified the
connection but not the authenticated principal.

An Agent is an Actor with identity, perception, actions, permissions, and a
lifetime. It is not an input device. The compositor must own the security and
routing invariants without absorbing reasoning, planning, prompts, or
long-term Agent memory.

## Decision

Aegis introduces an Actor-scoped interaction context assembled from existing
authorities:

- **identity** is the compositor-issued principal plus the live IPC
  connection;
- **capability context** is the refreshed operation ceiling, runtime grant,
  resource allowlists, and connection-bound lease;
- **view and input context** is the subject-owned Realm, its directed output,
  independent seat, and interaction-group authority;
- **observation context** is a short-lived compositor-owned semantic
  snapshot; and
- **storage context** is the Realm sandbox policy and its explicitly granted
  filesystem and network resources. Reasoning state and long-term memory stay
  in the out-of-process Agent runtime.

Observation and action are independent operation families. Authenticated
Actors must hold explicit `ObserveWindows`, `ObserveWorkspaces`,
`ObserveOutputs`, `ObserveNotifications`, `ObserveJournal`, `ObserveRealms`,
`ObserveSettings`, or `ObserveSystem` operations for the corresponding
snapshot. Semantic Realm observation requires `ObserveRealm`; acting requires
`InjectRealmInput`. Neither direction implies the other. Snapshot replies are
also filtered by the scope's window, workspace, output, and Realm allowlists.
An Actor sees only its own Agent Realms and its own principal-originated
journal events. The human Realm remains visible only where an allowed
authority transition needs it.

The compositor exposes a semantic Realm tree before framebuffer capture. The
guaranteed roots are durable window objects with role, name, application id,
Realm-output bounds, target-local extent, state, declared actions, content
revision, and Realm authority revision. Application-internal descendants may
come only from a future accessibility or application protocol adapter. Pixel
inference never becomes semantic authority. Framebuffer capture remains a
separate, explicitly granted compatibility observation and carries the same
semantic observation metadata when used.

Every semantic observation receives a cryptographically random, opaque bearer
token. The compositor stores the authoritative snapshot and binds the token
to the authenticated principal, connection, and Realm for 15 seconds. A token
is usable once. Disconnect, timeout, lock, inactive session, or the first
action attempt by its owning Actor revokes it. Another Actor cannot consume or
revoke it even if the bearer value leaks.

`ActInRealm` is an optimistic GUI transaction. Before dispatch, the
compositor main loop consumes the token and rechecks the Actor binding, Realm
state, live capability ceiling and runtime grant, authority revision, complete
semantic target state, declared action families, target-local coordinates,
current seat, and resource authority. A capability or resource-scope refusal
also consumes the token. It
prepares the entire bounded action batch before delivering it. Any mismatch
aborts the batch. Success returns an authoritative commit receipt with a
monotonic action id; the old unguarded `InjectRealmInput` command is removed
at the IPC boundary.

This transaction protects the observation-to-dispatch boundary. It cannot
roll back business state that an application changes after receiving a GUI
event. Agents must observe a new semantic snapshot before a dependent action,
and application-specific protocols should provide stronger transactions when
available.

The append-only journal records `Origin::Actor { conn_id, principal }` and a
dedicated `ActorAction` mutation for every accepted or refused attempt. The
event contains the Realm, semantic target, bounded actions, effect, and, after
commit, action id and authority revision. It never contains the bearer token.
Authenticated Actors use filtered journal polling; unfiltered event and
journal subscription lanes reject Actor connections until they can preserve
the same per-principal and per-resource filtering.

The compositor remains a microkernel-like authority: it owns identity,
security, routing, isolation, precondition validation, lifecycle cleanup, and
the auditable event log. Agent runtimes own inference, planning, retries, and
memory. One Actor's queue, tokens, seat, Realm, sandbox, or principal never
becomes another Actor's implicit global GUI context.

## Alternatives

- **Treat the Agent as another pointer and keyboard.** Rejected because an
  input device has no principal, scoped perception, resource ownership,
  lifecycle, or auditable intent.
- **Authorize actions from a previous framebuffer and coordinates.** Rejected
  because pixels do not carry stable object identity or a checkable state
  precondition and make layout races indistinguishable from intent.
- **Let `query` reveal every snapshot.** Rejected because permission to read
  one observation family must not disclose unrelated notifications, journal
  history, settings, or Realm topology.
- **Use a pessimistic global GUI lock.** Rejected because it would freeze the
  human session and still could not lock application-owned business state.
  Revisioned optimistic validation aborts stale intent without serializing
  unrelated Actors.
- **Put accessibility inference and Agent memory in the compositor.** Rejected
  because model-dependent code and unbounded retained state enlarge the
  trusted computing base and couple compositor stability to an Agent runtime.

## Consequences

Realm actions are synchronous, exactly-once with respect to an observation
token, and fail closed when the observed target or authority changes. Agents
can prefer a structured semantic path and request pixels only when separately
authorized. Audit consumers can attribute actions to durable principals and
correlate successful operations by action id without exposing credentials.

The IPC protocol moves to version 21. Agent ceilings must add the observation
families they use. MCP adds `realm_observe`; `realm_input` requires an
`observation_token` and returns a committed receipt instead of a queued
acknowledgement. `realm_capture` also returns its observation token and
semantic snapshot.

The guaranteed semantic tree currently stops at compositor-owned window
roots. Application controls require an accessibility-tree adapter with stable
node identity and revision semantics. Filtered push lanes are also required
before authenticated Actors can use subscriptions instead of scoped polling.
