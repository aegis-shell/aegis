# Command-Line Reference

`ass-control` queries and controls a running ass session through
`$XDG_RUNTIME_DIR/ass.sock`. `help`, `--help`, `-h`, and `--version` work
without a running compositor or an `XDG_RUNTIME_DIR` value.

The CLI is implemented with `clap`; running `ass-control <command> --help`
prints per-subcommand usage, and `ass-control completions <shell>` prints
shell completion scripts for bash, zsh, fish, PowerShell, and elvish.

## Commands

| Command | Result |
|---------|--------|
| `ass-control help`, `--help`, `-h` | Print local help without connecting. |
| `ass-control windows` | List visible toplevels. |
| `ass-control workspaces` | List outputs, workspaces, and window counts. |
| `ass-control outputs` | List output modes, scales, transforms, and logical sizes, plus the modes each connector advertises (the live one marked `current`). |
| `ass-control notifications` | List active notifications. |
| `ass-control journal [since]` | List mutation entries with a sequence greater than `since`; the default is `0`. |
| `ass-control realm list` | List the authority revision, Realms, states, seats, and controlled-window counts. |
| `ass-control realm create [label]` | Create an active agent Realm with a 1920×1080 virtual output and pointer/keyboard seat. |
| `ass-control realm pause <realm>` | Atomically pause a Realm and freeze its managed cgroups. |
| `ass-control realm resume <realm>` | Resume a paused Realm and its managed cgroups. |
| `ass-control realm transfer <window> <realm> [--no-mirror]` | Transfer the window's complete interaction group. The source stays a read-only observer unless `--no-mirror` is set. |
| `ass-control realm launch <realm> <desktop-id>` | Launch an enumerated desktop entry through a private mount-scoped portal and process sandbox. |
| `ass-control realm capture <realm> [path.png] [--region x,y,w,h]` | Capture the Realm's directed virtual output. The default path is a timestamped PNG in the screenshot directory. |
| `ass-control realm revoke <realm> [fallback]` | Permanently revoke a Realm and return controlled groups to `fallback` (default Realm `1`). |
| `ass-control focus <id>` | Focus and raise a toplevel. |
| `ass-control minimize <id>` | Minimize a toplevel while keeping its client alive. |
| `ass-control close <id>` | Request that a toplevel close. |
| `ass-control set-geometry <id> <x> <y> <w> <h>` | Set floating-window geometry in compositor logical pixels. Negative coordinates are accepted positionally. |
| `ass-control switch <next\|prev\|previous>` | Switch to an adjacent workspace. |
| `ass-control switch-to <workspace>` | Switch directly to a workspace id. |
| `ass-control move-to <window> <workspace>` | Move a toplevel to a workspace id. |
| `ass-control tiling` | Toggle tiling on the current workspace. |
| `ass-control notify <summary> [body]` | Post a notification. |
| `ass-control dismiss <id>` | Dismiss an active notification. |
| `ass-control screenshot [path.png] [--region x,y,w,h]` | Capture the focused output to a PNG file (default: a timestamped file in `$XDG_PICTURES_DIR/screenshots`, falling back to `~/Pictures/screenshots`). This IPC command does not modify the physical clipboard. |
| `ass-control overview` | Toggle the window/workspace overview. |
| `ass-control subscribe` | Stream coarse window, workspace, and notification events. |
| `ass-control subscribe-journal` | Stream detailed mutation-journal events. |
| `ass-control completions <shell>` | Print shell completions for `bash`, `zsh`, `fish`, `powershell`, or `elvish`. |
| `ass-control quit` | Request compositor shutdown. |

## JSON Output

Add `--json` or `-j` anywhere in a query command to print the IPC model as
JSON. The flag applies to `windows`, `workspaces`, `outputs`,
`notifications`, `journal`, `realm list`, Realm lifecycle receipts, and Realm
capture metadata.

```bash
ass-control windows --json
ass-control journal 120 -j
ass-control realm list --json
```

Control commands print a short text acknowledgment. An acknowledgment means
the command was queued; re-query or subscribe to observe the applied state.
Realm lifecycle commands are different: they print a synchronous commit
receipt and fail on a stale optimistic authority revision.

## Realm Administration

Realm commands automatically request the built-in
`ass-control-realm-admin` scope and a 15-minute connection-bound lease. The
compositor socket is owner-only. Agent integrations should use a narrower
configured `[[agent.scope]]` instead of this local recovery scope.

`realm capture` writes through a mode-`0600` temporary file, synchronizes it,
and atomically renames it to the destination. A failed capture does not leave
a partial PNG at the requested path.

## Exit Status

| Status | Meaning |
|--------|---------|
| `0` | The query, command, or local help request succeeded. |
| `1` | Connection, protocol, or stream processing failed. |
| `2` | Argument parsing failed (unknown subcommand, missing argument, malformed value). |
