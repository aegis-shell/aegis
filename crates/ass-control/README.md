# ass-control

`ass-control` is the reference command-line client for querying and
controlling a running ass session through `ass-ipc`.

## Responsibilities

- Parse command-line commands and arguments via `clap` derive.
- Connect to the compositor IPC socket and negotiate capabilities.
- Format query results for humans or as JSON (`--json` / `-j`).
- Stream compositor events and mutation-journal entries.
- Administer Realm lifecycle, authority transfer, sandbox launch, and capture
  through the built-in owner-only recovery scope.
- Generate shell completions (`completions <shell>`).
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
The `help`, `--help`, `-h`, `--version`, and `completions` commands are
local and do not require a running compositor.

## Use

```bash
cargo run -p ass-control -- help
cargo run -p ass-control -- windows
cargo run -p ass-control -- windows --json
cargo run -p ass-control -- focus 42
cargo run -p ass-control -- minimize 42
cargo run -p ass-control -- realm create "Research"
cargo run -p ass-control -- realm transfer 42 2
cargo run -p ass-control -- realm capture 2
cargo run -p ass-control -- realm capture 2 --region 0,0,1280,720 out.png
cargo run -p ass-control -- subscribe
cargo run -p ass-control -- completions bash
```

Installed binaries use the same commands without the `cargo run -p ass-control --`
prefix.

## Subcommand Groups

Realm administration is grouped under `ass-control realm` so the
owner-only admin scope and lease negotiation happen in one place:

```bash
ass-control realm list
ass-control realm create [label]
ass-control realm pause <id>
ass-control realm resume <id>
ass-control realm transfer <window> <realm> [--no-mirror]
ass-control realm launch <realm> <desktop-id>
ass-control realm capture <realm> [path] [--region x,y,w,h]
ass-control realm revoke <realm> [fallback]
```

Run `ass-control realm --help` for the per-subcommand usage.

## Related Documentation

- [Command-line reference](../../docs/reference/cli.md)
- [How to Use AI Workspaces](../../docs/how-to/ai-workspaces.md)
- [IPC and introspection decision](../../docs/adr/0027-ipc-and-introspection.md)
