# ass-ctl

`ass-ctl` is the reference command-line client for querying and controlling a
running ass session through `ass-ipc`.

## Responsibilities

- Parse command-line commands and arguments.
- Connect to the compositor IPC socket and negotiate capabilities.
- Format query results for humans or as JSON.
- Stream compositor events and mutation-journal entries.
- Administer Realm lifecycle, authority transfer, sandbox launch, and capture
  through the built-in owner-only recovery scope.
- Expose the command dispatcher as a library for loopback tests.

## Boundaries

This crate is an IPC client, not a second compositor control plane. Protocol
types and framing belong to `ass-ipc`; window-management behavior belongs to
the running compositor.

## Runtime Effect

Query commands read compositor state. Control commands enqueue typed actions
such as focus, minimize, close, workspace switching, notification, tiling,
and shutdown.
Realm lifecycle commands return synchronous optimistic commit receipts.
The `help` command is local and does not require a running compositor.

## Use

```bash
cargo run -p ass-ctl -- help
cargo run -p ass-ctl -- windows
cargo run -p ass-ctl -- windows --json
cargo run -p ass-ctl -- focus 42
cargo run -p ass-ctl -- minimize 42
cargo run -p ass-ctl -- realm-create "Research"
cargo run -p ass-ctl -- realm-transfer 42 2
cargo run -p ass-ctl -- realm-capture 2
cargo run -p ass-ctl -- subscribe
```

Installed binaries use the same commands without the `cargo run -p ass-ctl --`
prefix.

## Related Documentation

- [Command-line reference](../../docs/reference/cli.md)
- [How to Use AI Workspaces](../../docs/how-to/ai-workspaces.md)
- [IPC and introspection decision](../../docs/adr/0027-ipc-and-introspection.md)
