# Command-Line Reference

`aegis` starts the compositor and provides the native management surface for
a running Aegis session. Running it with no subcommand, or with `run`, starts
the compositor. Resource subcommands connect to
`$XDG_RUNTIME_DIR/aegis.sock`.

`--help`, `-h`, `--version`, `completions`, and `config` work without a
running compositor or an `XDG_RUNTIME_DIR` value. Run `aegis <domain> --help`
for per-domain usage.

## Startup and Local Commands

| Command | Result |
|---------|--------|
| `aegis` | Start the compositor. |
| `aegis run` | Start the compositor explicitly. |
| `aegis help`, `aegis --help`, `aegis -h` | Print local help without connecting. |
| `aegis completions <shell>` | Print completions for `bash`, `zsh`, `fish`, `powershell`, or `elvish`. |

## Configuration Commands

| Command | Result |
|---------|--------|
| `aegis config` | Validate the standard XDG Aegis configuration file. |
| `aegis config validate [path]` | Validate the current schema and every semantic invariant without contacting the compositor. |
| `aegis config migrate [path]` | Explicitly migrate a supported legacy schema after creating a durable, non-overwriting owner-only backup. |

## Audit Commands

Local operations on the durable security-audit history. They never contact a
running compositor; a live session holds the store's advisory lock, so run
them while the session is stopped.

| Command | Description |
|---------|-------------|
| `aegis audit status` | Summarize the active stream, sealed segments, compressed sizes, pruned history, and the last export destination. |
| `aegis audit verify [--full]` | Verify sealed segments against the authenticated manifest; `--full` decompresses every segment and replays the complete chain. |
| `aegis audit export <destination>` | Record an export acknowledgement for every sealed segment in the manifest. Required before pruning without `--force`. |
| `aegis audit prune <keep> [--force]` | Delete old sealed segments, keeping the newest `keep`. Refuses to remove export-unacknowledged segments unless `--force` is given. |

Migration preserves comments and refuses legacy ambient network or host-path
authority instead of silently dropping it. See the
[configuration migration contract](config.md#migrating-schema-1).

## Display Commands

`display` is the user-facing name for physical and virtual presentation
targets. IPC and Wayland schemas retain the internal term `output`.

| Command | Result |
|---------|--------|
| `aegis display` | List displays. |
| `aegis display list` | List modes, scales, transforms, logical sizes, and advertised connector modes. |
| `aegis display capture [path.png] [--region x,y,w,h]` | Capture the focused display. The default path is a timestamped PNG in the screenshot directory. |

Display capture does not modify the physical clipboard. The default screenshot
directory is `$XDG_PICTURES_DIR/screenshots`, falling back to
`~/Pictures/screenshots`.

## Window Commands

| Command | Result |
|---------|--------|
| `aegis window` | List visible windows. |
| `aegis window list` | List visible windows explicitly. |
| `aegis window focus <id>` | Focus and raise a window. |
| `aegis window minimize <id>` | Minimize a window while keeping its client alive. |
| `aegis window always-on-top <id> <on\|off>` | Set or clear the session-scoped always-on-top flag. |
| `aegis window fullscreen <id> <on\|off>` | Set or clear compositor-managed fullscreen. |
| `aegis window close <id>` | Request that a window close. |
| `aegis window geometry <id> <x> <y> <width> <height>` | Set floating geometry in compositor logical pixels. Negative coordinates are accepted positionally. |

## Workspace Commands

| Command | Result |
|---------|--------|
| `aegis workspace` | List displays, workspaces, and window counts. |
| `aegis workspace list` | List workspace state explicitly. |
| `aegis workspace switch <next\|prev\|previous\|id>` | Switch to an adjacent workspace or a concrete workspace id. |
| `aegis workspace move-window <window> <workspace>` | Move a window to a workspace id. |
| `aegis workspace layout <toggle\|tiled\|floating>` | Toggle or set the current workspace layout. |

## Notification Commands

| Command | Result |
|---------|--------|
| `aegis notification` | List active notifications. |
| `aegis notification list` | List active notifications explicitly. |
| `aegis notification send <summary> [body]` | Post a notification. |
| `aegis notification dismiss <id>` | Dismiss an active notification. |

## Journal and Event Commands

| Command | Result |
|---------|--------|
| `aegis journal` | List all retained mutation-journal entries. |
| `aegis journal list [--since <sequence>]` | List entries with a sequence greater than the supplied value. |
| `aegis journal follow` | Stream detailed mutation-journal events. |
| `aegis events` | Stream coarse window, display, workspace, notification, settings, live-system, and Interaction Domain events. |

## Live System Commands

These commands apply immediate service or session state. They do not write
persistent compositor settings.

| Command | Result |
|---------|--------|
| `aegis system` | Show the normalized live-system snapshot. |
| `aegis system status` | Show volume, mute, connectivity, radios, battery, brightness, Do Not Disturb, and layout state. |
| `aegis system mute` | Toggle mute on the default audio sink. |
| `aegis system step-volume <-100..100>` | Adjust the default audio sink by a signed percentage step. |
| `aegis system volume <0..100>` | Set the default audio-sink volume percentage. |
| `aegis system brightness <1..100>` | Set the backlight percentage. |
| `aegis system wifi <on\|off>` | Enable or disable Wi-Fi. |
| `aegis system bluetooth <on\|off>` | Enable or disable Bluetooth radios. |
| `aegis system do-not-disturb <on\|off>` | Enable or disable notification suppression. |

Control acknowledgments mean the action was queued. Run
`aegis system status` or `aegis events` to observe reconciled service state.

## Interaction Domain Commands

| Command | Result |
|---------|--------|
| `aegis interaction-domain` | List Interaction Domains and authority state. |
| `aegis interaction-domain list` | List the authority revision, Interaction Domains, states, seats, and controlled-window counts. |
| `aegis interaction-domain create [label]` | Create an active agent Interaction Domain with a 1920×1080 virtual output and pointer/keyboard seat. The label defaults to `Agent Workspace`. |
| `aegis interaction-domain pause <domain-id>` | Atomically pause an Interaction Domain and freeze its managed cgroups. |
| `aegis interaction-domain resume <domain-id>` | Resume a paused Interaction Domain and its managed cgroups. |
| `aegis interaction-domain transfer <window> <domain-id> [--no-mirror]` | Transfer the window's complete interaction group. The source remains a read-only observer unless `--no-mirror` is set. |
| `aegis interaction-domain launch <domain-id> <desktop-id>` | Launch an enumerated desktop entry through a private mount-scoped portal and process sandbox. |
| `aegis interaction-domain capture <domain-id> [path.png] [--region x,y,w,h]` | Capture the Interaction Domain's directed virtual output. |
| `aegis interaction-domain revoke <domain-id> [fallback]` | Permanently revoke an Interaction Domain and return controlled groups to `fallback` (default Interaction Domain `1`). |

Interaction Domain commands request the built-in `aegis-interaction-domain-admin` scope and a 15-minute
connection-bound lease. The compositor socket is owner-only. Agent
integrations borrow capabilities through pairing instead; see the
[aegis-mcp bridge reference](aegis-mcp.md).

Interaction Domain capture writes through a mode-`0600` temporary file, synchronizes it,
and atomically renames it to the destination. A failed capture does not leave
a partial PNG at the requested path.

## Permission Commands

| Command | Result |
|---------|--------|
| `aegis permissions` | List paired principals, capability ceilings, and grants. |
| `aegis permissions list` | List permission state explicitly. |
| `aegis permissions revoke <principal> <operation>` | Drop one recorded runtime grant. |
| `aegis permissions forget <principal>` | Forget a principal and its grants. |
| `aegis permissions rename <principal> [label]` | Set or clear a principal display label. |
| `aegis permissions set-ceiling <principal> [--pregrant ops] [--gated ops]` | Replace the approved capability ceiling. |
| `aegis permissions register [label] [--pregrant ops] [--gated ops]` | Pre-provision a principal and print its credential. |

## Session Commands

| Command | Result |
|---------|--------|
| `aegis overview` | Toggle the window/workspace overview. |
| `aegis quit` | Request compositor shutdown. |

## JSON Output

Add `--json` or `-j` anywhere in a command to select machine-readable output.
Queries print the IPC model, successful one-shot mutations print an
`{"ok":true,"message":"..."}` receipt, and event streams print one JSON
event per line. Interaction Domain lifecycle commands retain their richer structured
receipts and capture metadata.

```bash
aegis window --json
aegis journal list --since 120 -j
aegis system status --json
aegis window focus 42 --json
aegis interaction-domain --json
```

Human-readable output is intended for interactive use. Scripts should use
JSON and tolerate additional fields.

## Exit Status

| Status | Meaning |
|--------|---------|
| `0` | Startup, query, command, or local help succeeded. |
| `1` | Connection, protocol, filesystem, or stream processing failed. |
| `2` | Argument parsing failed or the runtime directory was unavailable. |
