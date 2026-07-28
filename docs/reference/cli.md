# Command-Line Reference

`aegis-ctl` queries and controls a running aegis session through
`$XDG_RUNTIME_DIR/aegis.sock`. `help`, `--help`, `-h`, and `--version` work
without a running compositor or an `XDG_RUNTIME_DIR` value.

The CLI is implemented with `clap`; running `aegis-ctl <command> --help`
prints per-subcommand usage, and `aegis-ctl completions <shell>` prints
shell completion scripts for bash, zsh, fish, PowerShell, and elvish.

## Commands

| Command | Result |
|---------|--------|
| `aegis-ctl help`, `--help`, `-h` | Print local help without connecting. |
| `aegis-ctl windows` | List visible toplevels. |
| `aegis-ctl workspaces` | List outputs, workspaces, and window counts. |
| `aegis-ctl outputs` | List output modes, scales, transforms, and logical sizes, plus the modes each connector advertises (the live one marked `current`). |
| `aegis-ctl notifications` | List active notifications. |
| `aegis-ctl journal [since]` | List mutation entries with a sequence greater than `since`; the default is `0`. |
| `aegis-ctl system status` | Show volume, mute, connectivity, radios, battery, brightness, Do Not Disturb, and layout state. |
| `aegis-ctl system mute` | Toggle mute on the default audio sink. |
| `aegis-ctl system step-volume <-100..100>` | Adjust the default audio sink by a signed percentage step. |
| `aegis-ctl system volume <0..100>` | Set the default audio-sink volume percentage. |
| `aegis-ctl system brightness <1..100>` | Set the backlight percentage. |
| `aegis-ctl system wifi <on\|off>` | Enable or disable Wi-Fi. |
| `aegis-ctl system bluetooth <on\|off>` | Enable or disable Bluetooth radios. |
| `aegis-ctl system do-not-disturb <on\|off>` | Enable or disable notification suppression. |
| `aegis-ctl system tiling <on\|off>` | Set the current workspace to tiled or floating layout. |
| `aegis-ctl realm list` | List the authority revision, Realms, states, seats, and controlled-window counts. |
| `aegis-ctl realm create [label]` | Create an active agent Realm with a 1920×1080 virtual output and pointer/keyboard seat. |
| `aegis-ctl realm pause <realm>` | Atomically pause a Realm and freeze its managed cgroups. |
| `aegis-ctl realm resume <realm>` | Resume a paused Realm and its managed cgroups. |
| `aegis-ctl realm transfer <window> <realm> [--no-mirror]` | Transfer the window's complete interaction group. The source stays a read-only observer unless `--no-mirror` is set. |
| `aegis-ctl realm launch <realm> <desktop-id>` | Launch an enumerated desktop entry through a private mount-scoped portal and process sandbox. |
| `aegis-ctl realm capture <realm> [path.png] [--region x,y,w,h]` | Capture the Realm's directed virtual output. The default path is a timestamped PNG in the screenshot directory. |
| `aegis-ctl realm revoke <realm> [fallback]` | Permanently revoke a Realm and return controlled groups to `fallback` (default Realm `1`). |
| `aegis-ctl focus <id>` | Focus and raise a toplevel. |
| `aegis-ctl minimize <id>` | Minimize a toplevel while keeping its client alive. |
| `aegis-ctl close <id>` | Request that a toplevel close. |
| `aegis-ctl set-geometry <id> <x> <y> <w> <h>` | Set floating-window geometry in compositor logical pixels. Negative coordinates are accepted positionally. |
| `aegis-ctl switch <next\|prev\|previous>` | Switch to an adjacent workspace. |
| `aegis-ctl switch-to <workspace>` | Switch directly to a workspace id. |
| `aegis-ctl move-to <window> <workspace>` | Move a toplevel to a workspace id. |
| `aegis-ctl tiling` | Toggle tiling on the current workspace. |
| `aegis-ctl notify <summary> [body]` | Post a notification. |
| `aegis-ctl dismiss <id>` | Dismiss an active notification. |
| `aegis-ctl screenshot [path.png] [--region x,y,w,h]` | Capture the focused output to a PNG file (default: a timestamped file in `$XDG_PICTURES_DIR/screenshots`, falling back to `~/Pictures/screenshots`). This IPC command does not modify the physical clipboard. |
| `aegis-ctl overview` | Toggle the window/workspace overview. |
| `aegis-ctl subscribe` | Stream coarse window, output-space-use, workspace, notification, settings, live-system, and Realm events. |
| `aegis-ctl subscribe-journal` | Stream detailed mutation-journal events. |
| `aegis-ctl completions <shell>` | Print shell completions for `bash`, `zsh`, `fish`, `powershell`, or `elvish`. |
| `aegis-ctl quit` | Request compositor shutdown. |

## JSON Output

Add `--json` or `-j` anywhere in a query command to print the IPC model as
JSON. The flag applies to `windows`, `workspaces`, `outputs`,
`notifications`, `journal`, `system status`, `realm list`, Realm lifecycle
receipts, and Realm capture metadata.

```bash
aegis-ctl windows --json
aegis-ctl journal 120 -j
aegis-ctl system status --json
aegis-ctl realm list --json
```

Control commands print a short text acknowledgment. An acknowledgment means
the command was queued; re-query or subscribe to observe the applied state.
Realm lifecycle commands are different: they print a synchronous commit
receipt and fail on a stale optimistic authority revision.

## Live System Controls

The `system` group is the reference UI-independent path for immediate
controls. It sends the same typed actions as the status bar and does not write
persistent compositor settings. Control acknowledgments mean the action was
queued; run `aegis-ctl system status` or subscribe for
`SystemStatusChanged` to observe the reconciled service state.

## Realm Administration

Realm commands automatically request the built-in
`aegis-ctl-realm-admin` scope and a 15-minute connection-bound lease. The
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
