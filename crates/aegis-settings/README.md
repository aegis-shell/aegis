# aegis-settings

`aegis-settings` is the settings module library for aegis: the
`SettingsModule` contract, the `ModuleRegistry`, and the built-in settings
modules. It is hosted in-process by the command panel; the former standalone
application and its IPC worker are gone.

## Responsibilities

- Provide stable module metadata, navigation ids, categories, keywords,
  backend availability, and instant or explicit apply policy.
- Render module pages through Lens against the shared design system.
- Expose honest unavailable pages for domains whose authoritative backend is
  not implemented.

## Boundaries

A settings module owns presentation and local draft state. It does not read or
write `config.toml`, probe hardware, call system services, or choose its
transport. The command panel drains module intents into the compositor's
confirmed settings commit path — the same revisioned transaction external
clients reach over IPC. Future system-owned modules use authorized service
adapters.

Live volume, brightness, radio, notification, and current-session controls
use the separate system-control model and appear in the command panel.
Agent Interaction Domain lifecycle is authority management and lives in the
`aegis interaction-domain *` CLI and the `interaction_domain_*` MCP tools,
not in settings.

Built-in modules are registered statically because Rust has no stable dynamic
library ABI. A future third-party module boundary must be a versioned process
protocol rather than an in-process Rust `.so`.

## Runtime Effect

The command panel embeds the registry and renders the selected module inline.
The host owns navigation and submits typed settings actions; modules never
touch the socket themselves.

The `display`, `touchpad`, `appearance`, and `power` modules are editable.
Appearance submits the complete desktop preference profile as one
explicit-apply transaction. `mouse`, `keyboard`, `users`, and `window-rules`
keep stable routes but remain unavailable until their backends exist.

## Use

Depend on the crate and build the built-in module set:

```rust
let registry = aegis_settings::builtin_settings_modules();
```

## Related Documentation

- [Settings Reference](../../docs/reference/settings.md)
- [Panel-hosted settings and the HUD command panel](../../docs/adr/0114-panel-hosted-settings-and-hud-command-panel.md)
- [Status bar system controls](../../docs/adr/0060-statusbar-system-controls-and-live-system-ipc.md)
- [System Settings identity and boundary](../../docs/adr/0056-system-settings-identity-and-boundary.md)
