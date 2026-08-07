# Actor and Interaction Glossary

Canonical terms for Aegis authority, automation, and Agent integration.

| Term | Meaning | Not the same as |
|---|---|---|
| Actor | An authenticated subject that can perceive or attempt actions: a human, Agent, or trusted system component. | An input device or Agent runtime process. |
| Principal | Durable identity used by the compositor authority model. | A display label. |
| Persona | User-facing profile content shared by shell surfaces, including account defaults, display name, portrait, and motion behavior. | An authenticated principal or authorization source. |
| Agent identity | A paired Agent principal and its approved capability ceiling. | One live IPC connection. |
| Actor binding | One principal bound to one live broker connection. Observation leases use this binding. | Durable Agent identity. |
| Actor session | One TTL/idle-bounded execution context tied to a connection and optional principal, with quotas and cascading revocation. | Durable principal or desktop login session. |
| Actor capability | One independently grantable operation, such as `ObserveInteractionDomain` or `InjectInteractionDomainInput`. It answers what may be attempted. | Connection capability or resource scope. |
| Connection capabilities | Coarse IPC admission classes (`query`, `control`, `input`, `session`, `interaction_domain`). | The operation-level Actor capability set. |
| Resource scope | The windows, workspaces, outputs, and Interaction Domains on which capabilities may operate. It answers where. | Capability itself. |
| Exact resource grant | A random, short-lived, use-counted handle bound to one Actor session, capability, and exact filesystem path, network origin, secret purpose, or payment bound. | Ambient sandbox or process access. |
| Actor context | The composition of identity, capability, view, input, observation, storage, and network facets for one live Actor. | A single global context or god object. |
| Interaction Domain | A compositor-enforced GUI interaction authority boundary: controller, lifecycle, seat, presentation target, controlled groups, observers, and revision. | Authentication realm, VM, Linux namespace, or desktop workspace. |
| Agent Workspace | The user-facing status metaphor for an Agent Interaction Domain, shown as a status label in the command panel. | The kernel security primitive or a human desktop workspace. |
| Interaction group | The smallest set of related surfaces whose control authority transfers atomically. | One arbitrary rectangle or individual subsurface. |
| Observation | Actor-scoped semantic state or separately authorized pixels. It grants no action authority. | An action or framebuffer-global visibility. |
| Semantic provider | An authenticated out-of-process adapter that publishes complete bounded accessibility revisions and executes routed semantic actions. | An Agent runtime or trusted self-asserted application label. |
| Action intent | Semantic target, bounded actions, and a single-use observation precondition submitted for optimistic validation. | A coordinate click sent directly to a global input device. |
| Action receipt | Authoritative evidence that the compositor main loop revalidated and committed the complete action batch. | A queued acknowledgement. |
| Audit event | Privacy-minimized, principal-attributed decision stored in the bounded projection and hash-chained durable log. | Raw input content, Agent memory, or a restorable snapshot of external clients. |

See [Architecture](../explanation/architecture.md#actor-boundary) and
[ADR-0103](../adr/0103-actor-authority-and-interaction-domain-architecture.md)
and [ADR-0104](../adr/0104-actor-sessions-resource-grants-and-accessibility-adapter.md).
The presentation/security distinction and module placement are recorded in
[ADR-0109](../adr/0109-module-first-security-and-presentation-identity-boundaries.md)
and [ADR-0111](../adr/0111-persona-as-shell-domain-with-feature-gated-portrait-runtime.md).
