# aegis-commands

`aegis-commands` implements the domain-oriented management surface exposed by
the `aegis` executable. It is a library crate and does not install a separate
binary.

## Responsibilities

- Parse display, window, workspace, notification, Interaction Domain, permission, system,
  event, and journal commands.
- Connect to the compositor IPC socket and negotiate capabilities.
- Format query results for humans or as JSON.
- Stream compositor and mutation-journal events.
- Generate shell completions for the unified `aegis` command.
- Keep command dispatch testable without Flux, Lens, Vulkan, or Wayland server
  dependencies.

## Boundaries

Protocol types and framing belong to `aegis-ipc`. Window-management behavior
belongs to the running compositor. This crate translates native resource
commands into typed IPC requests; it is not a second control plane.

The `aegis` binary selects compositor or management mode before initializing
either runtime. Resource commands still execute out of process through the
versioned IPC boundary.

## Development

Run the flux-free loopback suite from the repository root:

```bash
cargo test --locked -p aegis-commands
```

Run the full binary-entry tests when the Optics libraries are available:

```bash
cargo test --locked -p aegis --test command_entry
```

## Related Documentation

- [Command-Line Reference](../../docs/reference/cli.md)
- [IPC Reference](../../docs/reference/ipc.md)
- [Unified Command Surface Decision](../../docs/adr/0093-unified-domain-oriented-aegis-command-surface.md)
