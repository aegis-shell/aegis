# Command-Line Reference

`tessera` starts the compositor and provides the native management surface for
a running Tessera session. Running it with no subcommand, or with `run`, starts
the compositor. Resource subcommands connect to
`$XDG_RUNTIME_DIR/tessera.sock`.

`--help`, `-h`, `--version`, `completions`, and `config` work without a
running compositor or an `XDG_RUNTIME_DIR` value. Run `tessera <domain> --help`
for per-domain usage.

## Startup and Local Commands

| Command | Result |
|---------|--------|
| `tessera` | Start the compositor. |
| `tessera run` | Start the compositor explicitly. |
| `tessera help`, `tessera --help`, `tessera -h` | Print local help without connecting. |
| `tessera completions <shell>` | Print completions for `bash`, `zsh`, `fish`, `powershell`, or `elvish`. |

## Configuration Commands

| Command | Result |
|---------|--------|
| `tessera config` | Validate the standard XDG Tessera configuration file. |
| `tessera config validate [path]` | Validate the current schema and every semantic invariant without contacting the compositor. |
| `tessera config migrate [path]` | Explicitly migrate a supported legacy schema after creating a durable, non-overwriting owner-only backup. |

## Audit Commands

Local operations on the durable security-audit history. They never contact a
running compositor; a live session holds the store's advisory lock, so run
them while the session is stopped.

| Command | Description |
|---------|-------------|
| `tessera audit status` | Summarize the active stream, sealed segments, compressed sizes, pruned history, and the last export destination. |
| `tessera audit verify [--full]` | Verify sealed segments against the authenticated manifest; `--full` decompresses every segment and replays the complete chain. |
| `tessera audit export <destination>` | Record an export acknowledgement for every sealed segment in the manifest. Required before pruning without `--force`. |
| `tessera audit prune <keep> [--force]` | Delete old sealed segments, keeping the newest `keep`. Refuses to remove export-unacknowledged segments unless `--force` is given. |

Migration preserves comments and refuses legacy ambient network or host-path
authority instead of silently dropping it. See the
[configuration migration contract](config.md#migrating-schema-1).

## Display Commands

`display` is the user-facing name for physical and virtual presentation
targets. IPC and Wayland schemas retain the internal term `output`.

| Command | Result |
|---------|--------|
| `tessera display` | List displays. |
| `tessera display list` | List modes, scales, transforms, logical sizes, and advertised connector modes. |
| `tessera display capture [path.png] [--region x,y,w,h]` | Capture the focused display. The default path is a timestamped PNG in the screenshot directory. |

Display capture does not modify the physical clipboard. The default screenshot
directory is `$XDG_PICTURES_DIR/screenshots`, falling back to
`~/Pictures/screenshots`.

## Window Commands

| Command | Result |
|---------|--------|
| `tessera window` | List visible windows. |
| `tessera window list` | List visible windows explicitly. |
| `tessera window focus <id>` | Focus and raise a window. |
| `tessera window minimize <id>` | Minimize a window while keeping its client alive. |
| `tessera window always-on-top <id> <on\|off>` | Set or clear the session-scoped always-on-top flag. |
| `tessera window fullscreen <id> <on\|off>` | Set or clear compositor-managed fullscreen. |
| `tessera window close <id>` | Request that a window close. |
| `tessera window geometry <id> <x> <y> <width> <height>` | Set floating geometry in compositor logical pixels. Negative coordinates are accepted positionally. |

## Workspace Commands

| Command | Result |
|---------|--------|
| `tessera workspace` | List displays, workspaces, and window counts. |
| `tessera workspace list` | List workspace state explicitly. |
| `tessera workspace switch <next\|prev\|previous\|id>` | Switch to an adjacent workspace or a concrete workspace id. |
| `tessera workspace move-window <window> <workspace>` | Move a window to a workspace id. |
| `tessera workspace layout <toggle\|tiled\|floating>` | Toggle or set the current workspace layout. |

## Notification Commands

| Command | Result |
|---------|--------|
| `tessera notification` | List active notifications. |
| `tessera notification list` | List active notifications explicitly. |
| `tessera notification send <summary> [body]` | Post a notification. |
| `tessera notification dismiss <id>` | Dismiss an active notification. |

## Journal and Event Commands

| Command | Result |
|---------|--------|
| `tessera journal` | List all retained mutation-journal entries. |
| `tessera journal list [--since <sequence>]` | List entries with a sequence greater than the supplied value. |
| `tessera journal follow` | Stream detailed mutation-journal events. |
| `tessera events` | Stream coarse window, display, workspace, notification, settings, live-system, and Interaction Domain events. |

## Live System Commands

These commands apply immediate service or session state. They do not write
persistent compositor settings.

| Command | Result |
|---------|--------|
| `tessera system` | Show the normalized live-system snapshot. |
| `tessera system status` | Show volume, mute, connectivity, radios, battery, brightness, Do Not Disturb, and layout state. |
| `tessera system mute` | Toggle mute on the default audio sink. |
| `tessera system step-volume <-100..100>` | Adjust the default audio sink by a signed percentage step. |
| `tessera system volume <0..100>` | Set the default audio-sink volume percentage. |
| `tessera system brightness <1..100>` | Set the backlight percentage. |
| `tessera system wifi <on\|off>` | Enable or disable Wi-Fi. |
| `tessera system bluetooth <on\|off>` | Enable or disable Bluetooth radios. |
| `tessera system do-not-disturb <on\|off>` | Enable or disable notification suppression. |
| `tessera system power-mode <balanced\|awake\|secure>` | Select the session power mode (ADR-0140): which idle stages stay armed. Session-scoped; not persisted. |

Control acknowledgments mean the action was queued. Run
`tessera system status` or `tessera events` to observe reconciled service state.

## Interaction Domain Commands

| Command | Result |
|---------|--------|
| `tessera interaction-domain` | List Interaction Domains and authority state. |
| `tessera interaction-domain list` | List the authority revision, Interaction Domains, states, seats, and controlled-window counts. |
| `tessera interaction-domain create [label]` | Create an active agent Interaction Domain with a 1920×1080 virtual output and pointer/keyboard seat. The label defaults to `Agent Workspace`. |
| `tessera interaction-domain pause <domain-id>` | Atomically pause an Interaction Domain and freeze its managed cgroups. |
| `tessera interaction-domain resume <domain-id>` | Resume a paused Interaction Domain and its managed cgroups. |
| `tessera interaction-domain transfer <window> <domain-id> [--no-mirror]` | Transfer the window's complete interaction group. The source remains a read-only observer unless `--no-mirror` is set. |
| `tessera interaction-domain launch <domain-id> <desktop-id>` | Launch an enumerated desktop entry through a private mount-scoped portal and process sandbox. |
| `tessera interaction-domain capture <domain-id> [path.png] [--region x,y,w,h]` | Capture the Interaction Domain's directed virtual output. |
| `tessera interaction-domain revoke <domain-id> [fallback]` | Permanently revoke an Interaction Domain and return controlled groups to `fallback` (default Interaction Domain `1`). |

Interaction Domain commands request the built-in `tessera-interaction-domain-admin` scope and a 15-minute
connection-bound lease. The compositor socket is owner-only. Agent
integrations borrow capabilities through pairing instead; see the
[tessera-mcp bridge reference](tessera-mcp.md).

Interaction Domain capture writes through a mode-`0600` temporary file, synchronizes it,
and atomically renames it to the destination. A failed capture does not leave
a partial PNG at the requested path.

## Permission Commands

| Command | Result |
|---------|--------|
| `tessera permissions` | List paired principals, capability ceilings, and grants. |
| `tessera permissions list` | List permission state explicitly. |
| `tessera permissions revoke <principal> <operation>` | Drop one recorded runtime grant. |
| `tessera permissions forget <principal>` | Forget a principal and its grants. |
| `tessera permissions rename <principal> [label]` | Set or clear a principal display label. |
| `tessera permissions set-ceiling <principal> [--pregrant ops] [--gated ops]` | Replace the approved capability ceiling. |
| `tessera permissions register [label] [--pregrant ops] [--gated ops]` | Pre-provision a principal and print its credential. |

## Session Commands

| Command | Result |
|---------|--------|
| `tessera overview` | Toggle the window/workspace overview. |
| `tessera quit` | Request compositor shutdown. |

## JSON Output

Add `--json` or `-j` anywhere in a command to select machine-readable output.
Queries print the IPC model, successful one-shot mutations print an
`{"ok":true,"message":"..."}` receipt, and event streams print one JSON
event per line. Interaction Domain lifecycle commands retain their richer structured
receipts and capture metadata.

```bash
tessera window --json
tessera journal list --since 120 -j
tessera system status --json
tessera window focus 42 --json
tessera interaction-domain --json
```

Human-readable output is intended for interactive use. Scripts should use
JSON and tolerate additional fields.

## Exit Status

| Status | Meaning |
|--------|---------|
| `0` | Startup, query, command, or local help succeeded. |
| `1` | Connection, protocol, filesystem, or stream processing failed. |
| `2` | Argument parsing failed or the runtime directory was unavailable. |
