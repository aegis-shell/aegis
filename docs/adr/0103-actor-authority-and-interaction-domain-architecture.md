# ADR-0103: Actor authority kernel and Interaction Domain architecture

- Status: Accepted
- Date: 2026-08-03

## Context

Aegis already gives Agents independent seats, scoped observations,
single-use action preconditions, authenticated identities, lifecycle control,
and an auditable mutation journal. The implementation nevertheless described
the main isolation boundary as a “Realm” and distributed authority concepts
between `aegis-core`, `aegis-ipc`, and the binary runtime.

“Realm” is broad and does not say which kind of isolation it provides. It can
mean an authentication realm, network realm, storage namespace, VM, tenant,
or product workspace. The Aegis object is narrower: it is the
compositor-enforced domain in which one principal may perceive, present, and
interact with a set of surfaces through an independent seat. Treating Realm
as the ubiquitous language also leaked into mechanism names such as
`aegis-ai-workspaces` and `aegis-protocols`, obscuring their actual ownership.

The capability broker additionally treated operation families as an IPC
schema detail even though they are transport-neutral authority policy. That
made the transport appear to own security decisions and made it harder to
reuse the same invariants at the compositor commit boundary.

An Agent is not an input device. It is an Actor with identity, capabilities,
perception, actions, memory, and a bounded lifetime. The architecture and
names must preserve that distinction.

This decision supersedes [ADR-0040](0040-realms-seats-and-transferable-interaction-authority.md)
while retaining its multi-seat, interaction-group, and transferable-authority
invariants. It also supersedes the implementation-identity portion of
[ADR-0074](0074-generic-agent-workspaces-status-surface.md), and amends
[ADR-0090](0090-native-capability-broker-and-stateless-mcp-edge.md),
[ADR-0093](0093-unified-domain-oriented-aegis-command-surface.md), and
[ADR-0102](0102-actor-scoped-semantic-observation-and-transactional-actions.md).

## Decision

Aegis adopts the following ubiquitous language:

- An **Actor** is an authenticated authority subject. Humans, Agents, and
  trusted system components are Actors; devices are not.
- An **Agent identity** is the durable capability ceiling associated with an
  authenticated principal. An **Actor binding** ties that principal to one
  live broker connection.
- An **Actor capability** names one independently grantable operation. It
  answers *what* an Actor may attempt; resource scope answers *where*.
- An **Interaction Domain** is a compositor-enforced GUI interaction
  authority boundary. It binds a controller, lifecycle state, presentation
  target, logical seat, controlled interaction groups, and observation
  relationships under one authority revision.
- An **Actor context** is the composition of identity, capability, view,
  input, observation, storage, and network facets for one live Actor. It is
  an architectural composition, not a single mutable god object.
- **Agent Workspaces** remains the user-facing product metaphor for managing
  Agent Interaction Domains. A workspace is not the security primitive.

The compositor follows capability-first authorization. Observation and action
are separate capabilities. Semantic compositor-owned observations are the
default perception channel; framebuffer capture is an independent,
privacy-sensitive fallback. No action capability is implied by observation,
and no observation capability is implied by action.

GUI actions use optimistic transactions:

```text
observe semantic state
        │
        ▼
issue Actor-bound, short-lived token
        │
        ▼
submit target + actions + token
        │
        ▼
consume token → reauthorize → check revision and semantic preconditions
        │
        ├── changed: abort
        └── stable: dispatch complete batch → journal → commit receipt
```

Action targets are semantic object identities, not framebuffer coordinates.
Coordinates inside an action are target-local data validated against the
observed semantic bounds. Tokens are Actor-bound, single-use, short-lived,
and consumed before precondition validation so a failed attempt cannot be
replayed.

The mutation journal remains the append-only audit projection for successful
and refused authority changes and Actor actions. It records authenticated
origin and authoritative revisions; it is not Agent memory. Agent reasoning,
planning, prompts, and long-term memory remain out of process.

The implementation boundary is:

| Crate | Owns | Must not own |
|---|---|---|
| `aegis-core` | Pure Interaction Domain, semantic, window, workspace, and durable identity models | Transports, Wayland objects, policy stores |
| `aegis-authority` | Actor capability vocabulary, live bindings, identity profiles, observation leases, and action precondition validation | IPC framing, Wayland dispatch, rendering, Agent reasoning |
| `aegis-semantic` | Validated application semantic trees, window-local identity namespacing, and provider routing | D-Bus, IPC framing, compositor effects |
| `aegis-audit` | Generic bounded event projection and hash-chained durable persistence | IPC vocabulary, GUI policy, raw private inputs |
| `aegis-compositor` | Wayland resources, independent seats, semantic projection, routing, isolation, and domain enforcement | Agent planning or durable Agent memory |
| `aegis-ipc` | Versioned wire schema, framing, broker adapters, leases, and transport-level admission | Canonical authority policy or compositor effects |
| `aegis-interaction-manager` | Trusted Agent Workspaces presentation and typed user intents | Authority mutation, process lifecycle, capture, journaling |
| `aegis-wayland-protocols` | Shared generated Wayland ABI metadata | Aegis IPC or policy |
| `aegis-atspi` | Out-of-process AT-SPI tree and action adaptation | Compositor policy or Agent reasoning |
| `aegis-mcp` | Stateless MCP adaptation over the native broker | Compositor policy or Agent reasoning |
| `aegis-agent` and other Agent runtimes | Reasoning, planning, prompts, and Agent memory | Compositor security invariants |

The compositor and authority kernel form the trusted computing base for GUI
interaction. The runtime may assemble Actor-context facets, but each facet
keeps a narrow owner and can fail closed independently. Interaction Domains
and observation leases have explicit lifecycle transitions and are revoked on
disconnect, expiry, principal removal, or authority change as appropriate.

The canonical crate and API names are `aegis-authority`,
`aegis-interaction-manager`, `aegis-wayland-protocols`,
`InteractionDomain`, `ActorCapability`, `AuthorizationDecision`, and
`ConnectionCapabilities`. IPC protocol 24 carries the current wire contract;
protocol 22 introduced the renamed vocabulary. Current capability, CLI, MCP,
configuration, and built-in application APIs accept only canonical
Interaction Domain names. Documentation uses Realm only for historical
records and migration explanation.

## Alternatives

### Keep Realm as the canonical term

Rejected because it does not identify the isolated resource or authority and
collides with authentication, storage, network, and tenancy vocabulary.

### Use Workspace as the kernel term

Rejected because Aegis already has human desktop workspaces, and the Agent
Workspaces product surface is only one presentation of the authority model.

### Use Sandbox or Namespace

Rejected as the primary term because those describe isolation mechanisms.
An Interaction Domain can use mount, cgroup, network, storage, and Wayland
namespaces without being identical to any one of them.

### Put all facets in one ActorContext crate or struct

Rejected because it would centralize unrelated storage, network, Wayland,
transport, and presentation lifecycles. Actor context remains a composition
of capabilities owned by narrow components.

### Leave capability and transaction policy in IPC

Rejected because authorization must remain valid when called from another
transport or rechecked directly at the compositor commit boundary.

## Consequences

Security-sensitive authority concepts now have one transport-neutral owner.
IPC and MCP are adapters instead of policy authorities, while the compositor
remains the only component that commits GUI effects.

The domain language distinguishes the user-facing product from the kernel
primitive and makes crate ownership visible from names. Contributors must use
“Interaction Domain” in prose and `InteractionDomain` in Rust identifiers;
“Realm” is limited to historical records and migration explanation.

Protocol 24 is intentionally incompatible with older clients. Config schema 2
and the current capability, CLI, MCP, and built-in application identities
reject the former Realm-era names instead of keeping two active domain
languages.

The architecture adds one small crate and an additional policy seam. That is
an accepted cost: authority invariants can now be tested without IPC,
Wayland, rendering, or the compositor binary.
