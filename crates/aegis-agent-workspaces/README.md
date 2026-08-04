# aegis-agent-workspaces

`aegis-agent-workspaces` is the compositor-owned **Agent Workspaces**
management surface. “Agent Workspaces” is the product metaphor; an
`InteractionDomain` is the compositor-enforced authority boundary it
presents.

## Responsibilities

- Present the authoritative Agent Interaction Domain snapshot as workspaces.
- Emit typed create, pause, resume, and confirmed-revoke intents.
- Display lifecycle state, seat capabilities, and controlled window groups.
- Open through `BuiltInApplication::AgentWorkspaces`.

## Boundaries

This crate owns presentation and local confirmation state only. It does not
authorize Actors, mutate the domain model, manage processes, capture pixels,
or write the event journal. Those effects remain at the compositor commit
boundary and in `aegis-security::authority`.

Creating an empty Agent Workspace does not start or connect an Agent. Agent
runtimes remain out of process and acquire explicit capabilities through IPC.
Window authority transfer remains part of the compositor-owned Overview.

## Use

```rust
shell.add(Box::new(
    aegis_agent_workspaces::AgentWorkspaces::new(),
));
```

## Related Documentation

- [Agent Workspace operations](../../docs/how-to/ai-workspaces.md)
- [Actor-scoped authority architecture](../../docs/adr/0103-actor-authority-and-interaction-domain-architecture.md)
- [Historical Realm decision](../../docs/adr/0040-realms-seats-and-transferable-interaction-authority.md)
