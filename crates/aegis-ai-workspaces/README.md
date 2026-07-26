# aegis-ai-workspaces

`aegis-ai-workspaces` is the compositor-owned modal application for Agent
Realm lifecycle and authority management.

## Responsibilities

- Present the authoritative Agent Realm snapshot as AI Workspaces.
- Emit typed create, pause, resume, and confirmed revoke intents.
- Display seat capabilities and the number of controlled window groups.
- Open through the stable `BuiltInApplication::AiWorkspaces` identity.

## Boundaries

This crate owns presentation and local confirmation state only. Realm
authority, revision validation, process lifecycle, capture invalidation, and
journaling remain in the compositor. AI Workspaces are not persistent
settings and do not belong in `aegis-settings` or `aegis-config`.

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
- [AI Workspace operations](../../docs/how-to/ai-workspaces.md)
- [Realms and interaction authority](../../docs/adr/0040-realms-seats-and-transferable-interaction-authority.md)
