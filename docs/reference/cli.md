# Command-Line Reference

`ass-ctl` queries and controls a running ass session through
`$XDG_RUNTIME_DIR/ass.sock`. `help`, `--help`, and `-h` work without a running
compositor or an `XDG_RUNTIME_DIR` value.

## Commands

| Command | Result |
|---------|--------|
| `ass-ctl help`, `--help`, `-h` | Print local help without connecting. |
| `ass-ctl windows` | List visible toplevels. |
| `ass-ctl workspaces` | List outputs, workspaces, and window counts. |
| `ass-ctl outputs` | List output modes, scales, transforms, and logical sizes. |
| `ass-ctl notifications` | List active notifications. |
| `ass-ctl journal [since]` | List mutation entries with a sequence greater than `since`; the default is `0`. |
| `ass-ctl focus <id>` | Focus and raise a toplevel. |
| `ass-ctl minimize <id>` | Minimize a toplevel while keeping its client alive. |
| `ass-ctl close <id>` | Request that a toplevel close. |
| `ass-ctl set-geometry <id> <x> <y> <w> <h>` | Set floating-window geometry in compositor logical pixels. |
| `ass-ctl switch <next\|prev>` | Switch to an adjacent workspace. |
| `ass-ctl switch-to <workspace>` | Switch directly to a workspace id. |
| `ass-ctl move-to <window> <workspace>` | Move a toplevel to a workspace id. |
| `ass-ctl tiling` | Toggle tiling on the current workspace. |
| `ass-ctl notify <summary> [body]` | Post a notification. |
| `ass-ctl dismiss <id>` | Dismiss an active notification. |
| `ass-ctl subscribe` | Stream coarse window, workspace, and notification events. |
| `ass-ctl subscribe-journal` | Stream detailed mutation-journal events. |
| `ass-ctl quit` | Request compositor shutdown. |

## JSON Output

Add `--json` or `-j` anywhere in a query command to print the IPC model as
JSON. The flag applies to `windows`, `workspaces`, `outputs`,
`notifications`, and `journal`.

```bash
ass-ctl windows --json
ass-ctl journal 120 -j
```

Control commands print a short text acknowledgment. An acknowledgment means
the command was queued; re-query or subscribe to observe the applied state.

## Exit Status

| Status | Meaning |
|--------|---------|
| `0` | The query, command, or local help request succeeded. |
| `1` | Connection, protocol, argument, or stream processing failed. |
| `2` | `XDG_RUNTIME_DIR` is unavailable for a command that needs the socket. |
