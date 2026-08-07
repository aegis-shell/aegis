# ADR-0108: Agent Workspaces presentation naming

- Status: Superseded by [ADR-0113](0113-platform-ai-backend-and-agent-product-removal.md)
- Date: 2026-08-04

## Context

[ADR-0103](0103-actor-authority-and-interaction-domain-architecture.md)
established **Interaction Domain** as the canonical name for the
compositor-enforced GUI authority boundary and **Agent Workspaces** as its
user-facing product metaphor. It also named the presentation crate
`aegis-interaction-manager` and its built-in application
`InteractionManager`.

That implementation owns presentation, local confirmation state, and typed
user intents. It does not own generic interaction handling, mutate authority,
or manage Agent processes. The word “interaction” omits “domain” and can
suggest input or gesture management, while “manager” suggests effect
ownership that belongs to the compositor and `aegis-authority`. The existing
Agent Workspaces label describes the component more directly and already
distinguishes it from human desktop workspaces.

## Decision

This decision amends the presentation-naming portion of ADR-0103. The
canonical crate is `aegis-agent-workspaces`, its primary type is
`AgentWorkspaces`, and its built-in application identity is
`BuiltInApplication::AgentWorkspaces`. New application catalogs use the
stable id `aegis-agent-workspaces`.

`InteractionDomain` remains the canonical security-model and protocol term.
The compositor, authority kernel, CLI, and IPC continue to use Interaction
Domain names where they describe the enforcement primitive rather than the
product surface.

Persisted Dock configuration may resolve the former
`aegis-interaction-manager` built-in id as a compatibility alias. The old id
is not emitted for new state, and the former crate, Rust type, localization
keys, and built-in enum variant are not retained as aliases.

## Alternatives

### Keep `aegis-interaction-manager`

Rejected because the name neither identifies an Interaction Domain nor
matches the component's presentation-only ownership.

### Use `aegis-interaction-domain-manager`

Rejected because it still implies ownership of authority mutation and
process lifecycle. It also exposes a kernel term for a user-facing product
surface.

### Use `aegis-interaction-domain-ui`

Rejected because it is technically precise but less consistent with the
existing Agent Workspaces label and documentation.

## Consequences

The crate, Rust symbols, built-in application identity, localization keys,
and current documentation share one product-oriented name. Contributors can
reserve Interaction Domain terminology for the underlying authority boundary
without inferring that the presentation crate owns that boundary.

Workspace consumers of the former internal crate and Rust symbols must move
to the new names. Existing Dock pins continue to resolve, while newly written
pins use the canonical id.
