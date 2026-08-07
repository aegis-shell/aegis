# ADR-0113: Platform AI backend and agent product removal

- Status: Accepted
- Date: 2026-08-07

## Context

[ADR-0031](0031-agent-as-scoped-ipc-client.md),
[ADR-0047](0047-neenee-agent-realm-platform-bridge.md),
[ADR-0087](0087-aegis-mcp-standalone-platform-bridge-crate.md), and
[ADR-0090](0090-native-capability-broker-and-stateless-mcp-edge.md)
established the compositor as a model-free platform: an authority backend
plus scoped IPC, with an out-of-process MCP bridge as the standard agent
access point. The in-tree agent runtime of the
[ADR-0050](0050-fuji-agent-product-and-bridge-rename.md) /
[ADR-0089](0089-aegis-agent-product-and-fuji-identity-rename.md) lineage
was removed in commit `e2d3346`; no in-tree agent runtime remains.

The `aegis-agent-workspaces` chrome application, named by
[ADR-0108](0108-agent-workspaces-presentation-naming.md), remained as the
last in-tree agent product surface: a management console whose
create-empty-workspace, pause, resume, and revoke actions duplicate what
the `aegis interaction-domain *` CLI and the `interaction_domain_*` MCP
tools already expose through the authority backend.

## Decision

Aegis ships AI interfaces and the authority backend, and never an agent
product or agent frontend.

Remove the `aegis-agent-workspaces` crate, its `chrome-agent-workspaces`
Cargo feature, the `BuiltInApplication::AgentWorkspaces` catalog entry,
and the Create/SetState/Revoke chrome intents. Interaction Domain
lifecycle management lives in the `aegis interaction-domain *` CLI and
the `interaction_domain_*` MCP tools.

The trust chrome stays: the agent operation feedback layer
([ADR-0048](0048-compositor-owned-agent-operation-feedback.md)), the
capability-borrowing consent prompt
([ADR-0088](0088-agent-capability-borrowing-and-runtime-grants.md),
[ADR-0090](0090-native-capability-broker-and-stateless-mcp-edge.md)), the
controlled-window guard
([ADR-0091](0091-agent-controlled-window-physical-mirrors-and-guard.md)),
and the command-panel status row
([ADR-0083](0083-frameless-transient-toasts-and-hud-consolidation.md)).
These surfaces are the human half of the authority model, not an agent
frontend.

This decision supersedes
[ADR-0108](0108-agent-workspaces-presentation-naming.md) and amends the
"Agent Workspaces remains the user-facing product metaphor" line of
[ADR-0103](0103-actor-authority-and-interaction-domain-architecture.md):
the metaphor survives only as a status label; no in-tree product surface
remains.

## Alternatives

### Keep the management application

Rejected. Platforms ship capability, not an agent product. Every action
the application offered duplicates the CLI and MCP authority paths.

### Remove the trust chrome as well

Rejected. It would destroy the ADR-0088 interactive grant flow and
operation transparency: a security regression, not a simplification.

### Keep the crate but feature-gate it off by default

Rejected. Dead code in-tree is the historical baggage this decision
eliminates.

## Consequences

ADR-0108 is superseded. The legacy Dock pin id `aegis-interaction-manager`
no longer resolves. Integrators building an agent product manage
Interaction Domains through the scoped IPC and the `aegis-mcp` bridge.

Documentation is updated to match: the `aegis-agent` CLI reference is
removed, the agent how-to becomes generic MCP client guidance, the Agent
Workspaces how-to drops the graphical management paths, and the
explanation, contributor-layout, packaging, and glossary pages no longer
name in-tree agent product surfaces.
