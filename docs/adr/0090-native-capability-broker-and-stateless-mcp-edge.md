# ADR-0090: Native capability broker and stateless MCP edge

- Status: Accepted
- Date: 2026-08-01

## Context

MCP is useful as an agent-facing discovery and tool-call protocol, but it
does not define the desktop security boundary Aegis needs. In particular,
an MCP server name and a tool label are not authenticated identities, MCP
sessions do not own compositor resources, and transport cancellation is not
authority revocation. Treating the bridge as the authority would couple
desktop security to one evolving ecosystem protocol and would still leave
Realm ownership implicit.

The capability-borrowing design in ADR-0088 introduced compositor-issued
principals, ceilings, and runtime grants. Its first implementation still
allowed several unsafe states: Realm recovery relied on a cosmetic label,
administrator input could place dangerous operations in the pregranted set,
long-lived connections retained stale ceilings, unrelated MCP hosts could
share one default identity file, and anonymous IPC privilege was enabled by
default.

MCP `2026-07-28` introduces a stateless core, per-request metadata, explicit
handles, extensions, and task-oriented long-running work. Aegis needs to
adopt that protocol shape without moving authority out of the platform.

## Decision

Aegis makes the compositor-owned IPC an agent capability broker. MCP remains
a northbound protocol adapter. The bridge translates protocol shapes;
it does not decide identity, consent, resource ownership, or whether an
operation is allowed.

Every Agent Realm is bound atomically to the authenticated registry subject
that created it. The subject comes from the credential-bound IPC connection,
never from a Realm request. The IPC server reauthorizes Realm lifecycle,
capture, launch, and input against that binding. Transfers may cross the
human Realm boundary only when every non-human Realm involved belongs to the
same subject. Cosmetic labels are never recovery keys.

Each MCP connector installation has a stable instance id. The id partitions
the credential file, recovery lock, and local Realm state. Generated client
configuration contains a random explicit id, and startup fails if the id is
absent or invalid. Recovery requires the persisted instance identity, the
authenticated subject, and the live Realm binding to agree.

The capability broker owns these invariants:

- the dangerous operation set can appear only in the runtime-gated portion
  of an agent ceiling;
- explicit administrator ceilings reject unsupported, duplicate,
  overlapping, or dangerously pregranted operations instead of silently
  sanitizing them;
- a live request refreshes the principal ceiling, so forgetting a principal
  or narrowing its ceiling affects existing connections;
- principal, grant, and bridge identity files use owner-only directories,
  atomic replacement, file and directory synchronization, and in-memory
  rollback when persistence fails;
- anonymous connections are query-only by default; owner, Realm, agent, and
  portal administration use separate built-in scopes and time-bounded
  leases; and
- the mutation journal remains the audit record for accepted and refused
  authority changes.

The MCP adapter implements only `2026-07-28`. Every request carries version,
client identity, and capabilities independently; `server/discover` is
side-effect free, successful results use the required complete-result and
cache envelopes, and the broker-backed catalog is private and immediately
stale. There is no connection-level negotiation, version downgrade, retry
restart, or inferred connector identity. The adapter uses standard stdio EOF
shutdown, bounds and drains malformed frames without attacker-proportional
allocation, refreshes the tool catalog from the broker, and aligns client
timeouts with the compositor's user-consent timeout. A managed Realm persists
across a normal connector restart by default; explicit `realm_reset` is the
authority destruction operation.

MCP Tasks or another agent protocol map onto broker-owned operation and
resource handles. They do not replace the broker. Raw compositor ids remain
internal implementation details at the agent-facing edge.

The same-uid boundary remains explicit. Built-in scopes identify trusted
Aegis components on the owner-only socket but are not cryptographic process
identity; a malicious process already running with the user's full account
can impersonate one or read owner files. Strong isolation against that actor
requires a separately sandboxed principal with broker-delivered file
descriptors or an external policy authority. This design protects against
model and agent behavior within the declared tool boundary and does not
claim to defeat a compromised Unix account.

## Alternatives

- **Make MCP the platform authority.** Rejected because MCP does not own
  compositor subjects or resources, and protocol evolution would become a
  security migration.
- **Recover Realms by bridge or Realm label.** Rejected because labels are
  cosmetic, collide naturally, and are self-asserted.
- **Expose raw Realm ids as agent handles.** Rejected because numeric ids are
  compositor-lifetime implementation details and encourage confused-deputy
  behavior across connectors.
- **Keep anonymous privilege enabled for compatibility.** Rejected as a
  production default. Explicit built-in owner scopes preserve first-party
  tools without making every unnamed connection privileged.
- **Carry older MCP versions.** Rejected because parallel lifecycle and
  negotiation paths enlarge the state space and security review surface.
  Clients must implement the one protocol version Aegis declares.

## Consequences

Agent and MCP implementations can change independently of the security
model. Realm recovery is deterministic and cannot adopt another principal's
authority. Ceiling changes and revocation take effect without waiting for a
bridge restart, and persistence failures no longer leave memory claiming a
state that was not committed.

Connector configuration requires `AEGIS_MCP_INSTANCE_ID`, normal EOF
preserves the Realm unless cleanup is explicitly requested, MCP exposes only
`2026-07-28`, and the IPC protocol moves to version 19. Recovery records that
contain only a label are ignored and recovered only when the live subject
binding proves ownership.

The connector accepts cancellation notifications, but the current serial
adapter cannot observe one while a synchronous compositor consent operation
is in progress. Cooperative cancellation requires a broker operation handle.
Adding that handle and mapping it to MCP Tasks is follow-up work; the current
path remains bounded by the same 300-second consent deadline plus transport
margin and does not advertise task support.
