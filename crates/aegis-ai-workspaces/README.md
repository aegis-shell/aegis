# aegis-ai-workspaces

`aegis-ai-workspaces` is the compositor-owned **Agent Workspaces** modal
application for Agent Realm lifecycle and authority management. The crate and
built-in identity retain their established `ai-workspaces` names for
compatibility.

## Responsibilities

- Present the authoritative Agent Realm snapshot as Agent Workspaces.
- Emit typed create, pause, resume, and confirmed revoke intents.
- Display seat capabilities and the number of controlled window groups.
- Open through the stable `BuiltInApplication::AiWorkspaces` identity.

## Boundaries

This crate owns presentation and local confirmation state only. Realm
authority, revision validation, process lifecycle, capture invalidation, and
journaling remain in the compositor. Agent Workspaces are not persistent
settings and do not belong in `aegis-settings` or `aegis-config`.

Creating an empty workspace does not start or connect an agent. Agent
products such as fuji remain out of process and acquire scoped authority
through IPC.

Immediate service controls are owned by the status bar and the live-system
IPC.

## Runtime Effect

The compositor registers `AiWorkspaces` as trusted modal chrome and supplies
the complete Realm snapshot through `Chrome::update_realms`. User actions are
returned through `ChromeEvents::realm_intents`.

## Use

```rust
shell.add(Box::new(aegis_ai_workspaces::AiWorkspaces::new()));
```

## Related Documentation

- [Status bar system controls](../../docs/adr/0060-statusbar-system-controls-and-live-system-ipc.md)
- [Agent Workspace operations](../../docs/how-to/ai-workspaces.md)
- [Generic status surface decision](../../docs/adr/0074-generic-agent-workspaces-status-surface.md)
- [Realms and interaction authority](../../docs/adr/0040-realms-seats-and-transferable-interaction-authority.md)
