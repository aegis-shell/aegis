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
| `ass-ctl outputs` | List output modes, scales, transforms, and logical sizes, plus the modes each connector advertises (the live one marked `current`). |
| `ass-ctl notifications` | List active notifications. |
| `ass-ctl journal [since]` | List mutation entries with a sequence greater than `since`; the default is `0`. |
| `ass-ctl realms` | List the authority revision, Realms, states, seats, and controlled-window counts. |
| `ass-ctl realm-create [label]` | Create an active agent Realm with a 1920×1080 virtual output and pointer/keyboard seat. |
| `ass-ctl realm-pause <realm>` | Atomically pause a Realm and freeze its managed cgroups. |
| `ass-ctl realm-resume <realm>` | Resume a paused Realm and its managed cgroups. |
| `ass-ctl realm-transfer <window> <realm> [--no-mirror]` | Transfer the window's complete interaction group. The source stays a read-only observer unless `--no-mirror` is set. |
| `ass-ctl realm-launch <realm> <desktop-id>` | Launch an enumerated desktop entry through a private mount-scoped portal and process sandbox. |
| `ass-ctl realm-capture <realm> [path.png]` | Capture the Realm's directed virtual output. The default path is a timestamped PNG in the screenshot directory. |
| `ass-ctl realm-revoke <realm> [fallback]` | Permanently revoke a Realm and return controlled groups to `fallback` (default Realm `1`). |
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
| `ass-ctl screenshot [path.png]` | Capture the focused output to a PNG file (default: a timestamped file in `$XDG_PICTURES_DIR/screenshots`, falling back to `~/Pictures/screenshots`). |
| `ass-ctl overview` | Toggle the window/workspace overview. |
| `ass-ctl subscribe` | Stream coarse window, workspace, and notification events. |
| `ass-ctl subscribe-journal` | Stream detailed mutation-journal events. |
| `ass-ctl quit` | Request compositor shutdown. |

## JSON Output

Add `--json` or `-j` anywhere in a query command to print the IPC model as
JSON. The flag applies to `windows`, `workspaces`, `outputs`,
`notifications`, `journal`, `realms`, Realm lifecycle receipts, and Realm
capture metadata.

```bash
ass-ctl windows --json
ass-ctl journal 120 -j
ass-ctl realms --json
```

Control commands print a short text acknowledgment. An acknowledgment means
the command was queued; re-query or subscribe to observe the applied state.
Realm lifecycle commands are different: they print a synchronous commit
receipt and fail on a stale optimistic authority revision.

## Realm Administration

Realm commands automatically request the built-in
`ass-ctl-realm-admin` scope and a 15-minute connection-bound lease. The
compositor socket is owner-only. Agent integrations should use a narrower
configured `[[agent.scope]]` instead of this local recovery scope.

`realm-capture` writes through a mode-`0600` temporary file, synchronizes it,
and atomically renames it to the destination. A failed capture does not leave
a partial PNG at the requested path.

## Exit Status

| Status | Meaning |
|--------|---------|
| `0` | The query, command, or local help request succeeded. |
| `1` | Connection, protocol, argument, or stream processing failed. |
| `2` | `XDG_RUNTIME_DIR` is unavailable for a command that needs the socket. |
