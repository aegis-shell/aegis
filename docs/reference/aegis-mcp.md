# aegis-mcp Bridge Reference

`aegis-mcp` is the scoped Aegis platform bridge: the standard MCP access
point for any agent operating the desktop. It is an MCP stdio server, not
a model runtime: agent products such as fuji own providers, credentials,
sessions, and user interaction on their side of the connection.

## Commands

```text
aegis-mcp [serve]
aegis-mcp check
aegis-mcp smoke [--observe-seconds <1..=30>] [--input-window <id>]
aegis-mcp print-config
```

With no subcommand, `serve` reads one JSON-RPC object per stdin line and
writes one response per stdout line. Stdout contains protocol messages only;
diagnostics go to stderr.

| Exit status | Meaning |
|-------------|---------|
| `0` | The command completed or the MCP client closed normally. |
| `1` | IPC, Realm recovery, smoke verification, protocol I/O, or graceful cleanup failed. |
| `2` | Required environment configuration is invalid. |

`check` probes the compositor, prints the effective scope and advertised tool
names as JSON, and does not create a Realm. `print-config` prints an MCP client
`[mcp.aegis]` entry using the current executable path.

`smoke` performs a live start-notification mutation and reads the notification
back from compositor state. With no existing managed Realm, it creates a
temporary Realm, verifies `active → paused → active`, exposes it in the status
bar for the observation interval, then revokes it and verifies cleanup. A
second notification is posted and verified after completion. The default
interval is 8 seconds. A pre-existing managed Realm is observed and preserved.
The command requires `Notify`, `CreateRealm`, `TransactRealm`, and
`RevokeRealm` in the named scope and prints both notification ids in a JSON
evidence report.

With `--input-window`, `smoke` additionally requires `TransactRealm` and
`InjectRealmInput`. It accepts only an explicitly selected, visible,
human-controlled window; transfers that window while retaining the human as an
observer; applies one target-local pointer move at its center; waits for an
`Applied` journal entry; and restores the window to the human Realm during
cleanup. It never clicks or types. The JSON report includes the local position,
journal sequence, and restoration result.

## User-Visible State

| Surface | Behavior |
|---------|----------|
| Notification toast and history | Shows agent-originated notifications. Do Not Disturb suppresses only the toast. |
| Command panel Agent Workspaces status row | In the System section. One live Realm shows its own label; multiple Realms show a count and aggregate state. It does not report whether an agent process is online. |
| Agent operation feedback | After Realm input is applied, shows a labeled circular Agent crosshair, movement trail, click pulse, or scroll/keyboard label over a visible read-only mirror. Hidden targets and positionless input use a background-operation pill. This never moves the user's XDG cursor and is excluded from Realm capture. |
| Agent Workspaces | Opens from the application launcher (`Super+A`). Shows each Realm id, state, controlled-window count, pointer/keyboard/touch seat capabilities, and lifecycle controls. |
| Overview Realm shelf | Shows live Realm targets and window authority during overview and drag transfer. |

Committed Realm receipts and observed state are authoritative. A transient
toast or a queued response alone is not proof that an operation was applied.

## Environment

| Variable | Default | Description |
|----------|---------|-------------|
| `AEGIS_MCP_SOCKET` | `$XDG_RUNTIME_DIR/aegis.sock` | Aegis IPC socket. |
| `AEGIS_MCP_SCOPE` | `desktop-operator` | Named `[[agent.scope]]` presented during the IPC handshake. |
| `AEGIS_MCP_REALM_LABEL` | `Fuji` | Unique label used to create and recover this bridge's Agent Realm; 1–128 bytes. |
| `AEGIS_MCP_IPC_TIMEOUT_SECS` | `5` | Per-connection handshake and I/O timeout; valid range `1..=60`. |
| `AEGIS_MCP_REVOKE_ON_EXIT` | `true` | Revoke the managed Realm after a graceful MCP EOF or shutdown request. |

The connector has no provider or credential variables. Realm recovery files
and the per-scope process lock live in the owner-only
`$XDG_RUNTIME_DIR/aegis-mcp/` directory.

## Named Scope

The bridge never falls back to an unnamed privileged connection. A complete
scope is:

```toml
[[agent.scope]]
name = "desktop-operator"
ops = [
  "Focus",
  "Minimize",
  "Close",
  "MoveToWorkspace",
  "SwitchWorkspace",
  "SwitchWorkspaceTo",
  "SetWindowGeometry",
  "ToggleTiling",
  "ToggleOverview",
  "Notify",
  "InjectRealmInput",
  "CreateRealm",
  "TransactRealm",
  "RevokeRealm",
  "CaptureRealm",
  "LaunchInRealm",
]
```

Remove operations the product should not have. In particular, omit `Close`
unless the agent may close windows, and omit `InjectRealmInput` for an
observation-only deployment. `CreateRealm`, `TransactRealm`, `RevokeRealm`,
`CaptureRealm`, `LaunchInRealm`, and `InjectRealmInput` are fail-closed and
must be named explicitly.

Do not set a `realms` allowlist for a bridge that creates its Realm at runtime:
the new durable id cannot be known in advance. A `windows` allowlist may be
used, but it also limits which windows can be transferred or receive input.
See [Configuration](config.md#agent-scopes).

The bridge probes the grant for `tools/list`. Scope narrowing applies on the
next tool call because each call reconnects. Restart the MCP connector after
broadening a scope so the new tools are advertised.

## MCP Client Configuration

An MCP client loads the connector as a write-capable server because one
catalog contains both observation and mutation tools. A fuji configuration
entry is:

```toml
[mcp.aegis]
command = ["/absolute/path/to/aegis-mcp"]
enabled = true
read_only = false
environment = { AEGIS_MCP_SCOPE = "desktop-operator" }
```

With the server key `aegis`, fuji prefixes the public tool names as
`mcp__aegis__<tool>`.

## Tools

| Connector-local name | Required capability / operation | Contract |
|----------------------|---------------------------------|----------|
| `desktop_snapshot` | `query` | Windows, workspaces, outputs, all Realms, and the granted scope. |
| `desktop_journal` | `query` | Up to 200 ordered mutations and a pagination cursor. |
| `apps_list` | `query` | Filtered XDG applications with trusted `desktop_id` values. |
| `focus_window` | `control` / `Focus` | Queue focus. |
| `minimize_window` | `control` / `Minimize` | Queue minimization. |
| `close_window` | `control` / `Close` | Queue an explicit close request. |
| `move_window_to_workspace` | `control` / `MoveToWorkspace` | Queue a move by durable ids. |
| `switch_workspace` | `control` / `SwitchWorkspace` | Queue an adjacent switch. |
| `switch_workspace_to` | `control` / `SwitchWorkspaceTo` | Queue a switch by id. |
| `set_window_geometry` | `control` / `SetWindowGeometry` | Queue a floating logical rectangle. |
| `toggle_tiling` | `control` / `ToggleTiling` | Queue a layout toggle. |
| `toggle_overview` | `control` / `ToggleOverview` | Queue the overview. |
| `post_notification` | `control` / `Notify` | Queue an agent notification. |
| `realm_status` | `query` | Inspect the bridge-managed Realm without creating it. |
| `realm_ensure` | `realm` / `CreateRealm` | Create or recover the private Agent Realm. |
| `realm_launch_app` | `realm` / `LaunchInRealm` | Launch a catalogued application in its sandbox. |
| `realm_transfer_window` | `realm` / `TransactRealm` | Transfer authority only to the agent or the human Realm. |
| `realm_set_state` | `realm` / `TransactRealm` | Pause or resume with an optimistic receipt. |
| `realm_capture` | `realm` / `CaptureRealm` | Return directed PNG image content, an owner-only compatibility path, placements, and revision. |
| `realm_input` | `input` / `InjectRealmInput` | Queue at most 64 target-local actions through the Realm seat. |
| `realm_reset` | `realm` / `RevokeRealm` | Revoke and atomically return controlled groups to the human Realm. |

`realm_transfer_window` accepts `agent` or `human` as its `target`.
`realm_input` accepts `pointer_move`, `click`, `scroll`, and `key_press`.
Pointer positions are target-window-local logical coordinates. Click buttons
are `left`, `right`, `middle`, `side`, or `extra`; key codes are Linux evdev
codes from 0 through 767. The compositor validates the target, placement,
authority, and bounds again when applying the command.

Ordinary desktop and input responses with `status = "queued"` are not proof
of applied state. Use a fresh snapshot, journal, or Realm capture to verify.
Realm lifecycle and authority transactions return committed receipts.

## Capture Compatibility

The connector emits standard MCP `image` content and JSON metadata, which
fuji forwards to the model as image input. It also atomically stores the
same directed PNG in the owner-only runtime directory and returns
`image_path`. Clients whose MCP adapter renders only text can pass that path
to fuji's built-in `read_image` tool. If neither route makes pixels
model-visible, the agent must not guess coordinates or use visual input.
Inline MCP images are capped at 32 MiB; larger captures set
`image_attached = false` and remain available through `image_path`. The
compatibility file is replaced by each capture and removed when the managed
Realm is reset or cleanly revoked.
