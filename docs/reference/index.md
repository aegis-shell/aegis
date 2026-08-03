# Reference

Exact lookup for aegis: configuration keys, schemas, runtime contracts, and
option tables. For the design behind these interfaces, see the
[Architecture Decision Records](../adr/index.md); for narrative background,
see [Explanation](../explanation/index.md).

## Pages

| Page | Purpose |
|------|---------|
| [Configuration](config.md) | The `config.toml` schema: fields, key names, modifier names, action names, and defaults |
| [System Shortcuts](keyboard-shortcuts.md) | Default global keyboard, pointer, quit, and VT controls |
| [Rendering and KMS Planes](rendering.md) | Presentation plans, direct-scanout eligibility, rejection labels, plane roles, state transitions, and diagnostics |
| [Command-Line Reference](cli.md) | Native `aegis` startup, resource commands, event streams, JSON, and exit-status reference |
| [IPC Reference](ipc.md) | Protocol capabilities, queries, commands, geometry, synthetic input, and scope behavior |
| [Actor and Interaction Glossary](glossary.md) | Canonical Actor, capability, context, Interaction Domain, observation, and action terms |
| [System Settings](settings.md) | Standalone app invocation, module routes, backend availability, and apply behavior |
| [Session Service Commands](session-services.md) | Lock-screen and idle-coordinator invocation, options, defaults, and exit behavior |
| [Portal Backend](portal.md) | Installation identifiers, runtime dependencies, interface versions, and limitations |
| [aegis-mcp Bridge](aegis-mcp.md) | MCP command, environment, capability borrowing and pairing, Interaction Domain lifecycle, tools, and compatibility |
| [Aegis Agent](agent.md) | `aegis-agent` CLI, configuration, permissions, built-in tools, and sessions |
