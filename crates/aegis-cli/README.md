# aegis-cli

`aegis-cli` is the reference command-line client for querying and
controlling a running aegis session through `aegis-ipc`.

## Responsibilities

- Parse command-line commands and arguments via `clap` derive.
- Connect to the compositor IPC socket and negotiate capabilities.
- Format query results for humans or as JSON (`--json` / `-j`).
- Stream compositor events and mutation-journal entries.
- Inspect and control normalized live-system state through typed IPC actions.
- Administer Realm lifecycle, authority transfer, sandbox launch, and capture
  through the built-in owner-only recovery scope.
- Generate shell completions (`completions <shell>`).
- Expose the command dispatcher as a library for loopback tests.

## Boundaries

This crate is an IPC client, not a second compositor control plane. Protocol
types and framing belong to `aegis-ipc`; window-management behavior belongs to
the running compositor.

## Runtime Effect

Query commands read compositor state. Control commands enqueue typed actions
such as focus, minimize, close, workspace switching, notification, tiling,
live-system control, and shutdown.
Realm lifecycle commands return synchronous optimistic commit receipts.
The `help`, `--help`, `-h`, `--version`, and `completions` commands are
local and do not require a running compositor.

## Use

```bash
cargo run --locked -p aegis-cli -- help
cargo run --locked -p aegis-cli -- windows
cargo run --locked -p aegis-cli -- windows --json
cargo run --locked -p aegis-cli -- focus 42
cargo run --locked -p aegis-cli -- minimize 42
cargo run --locked -p aegis-cli -- system status
cargo run --locked -p aegis-cli -- system volume 50
cargo run --locked -p aegis-cli -- system wifi off
cargo run --locked -p aegis-cli -- realm create "Research"
cargo run --locked -p aegis-cli -- realm transfer 42 2
cargo run --locked -p aegis-cli -- realm capture 2
cargo run --locked -p aegis-cli -- realm capture 2 --region 0,0,1280,720 out.png
cargo run --locked -p aegis-cli -- subscribe
cargo run --locked -p aegis-cli -- completions bash
```

Installed binaries use the same commands without
`cargo run --locked -p aegis-cli --`
prefix.

## Subcommand Groups

Immediate controls are grouped under `aegis-cli system`; they use the same
`SystemStatus` and `SystemAction` IPC model as the command panel:

```bash
aegis-cli system status
aegis-cli system mute
aegis-cli system step-volume -2
aegis-cli system volume 50
aegis-cli system brightness 75
aegis-cli system wifi on
aegis-cli system bluetooth off
aegis-cli system do-not-disturb on
aegis-cli system tiling off
```

Realm administration is grouped under `aegis-cli realm` so the
owner-only admin scope and lease negotiation happen in one place:

```bash
aegis-cli realm list
aegis-cli realm create [label]
aegis-cli realm pause <id>
aegis-cli realm resume <id>
aegis-cli realm transfer <window> <realm> [--no-mirror]
aegis-cli realm launch <realm> <desktop-id>
aegis-cli realm capture <realm> [path] [--region x,y,w,h]
aegis-cli realm revoke <realm> [fallback]
```

Run `aegis-cli realm --help` for the per-subcommand usage.

## Related Documentation

- [Command-line reference](../../docs/reference/cli.md)
- [How to Use Agent Workspaces](../../docs/how-to/ai-workspaces.md)
- [IPC and introspection decision](../../docs/adr/0027-ipc-and-introspection.md)
- [Status bar system controls](../../docs/adr/0060-statusbar-system-controls-and-live-system-ipc.md)
