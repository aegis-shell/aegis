# aegis-mcp Bridge Reference

`aegis-mcp` is the scoped Aegis platform bridge: the standard MCP access
point for any agent operating the desktop. It is an MCP stdio server, not
a model runtime: agent products such as `aegis-agent` own providers,
credentials, sessions, and user interaction on their side of the connection.

## Commands

```text
aegis-mcp [serve]
aegis-mcp check
aegis-mcp smoke [--observe-seconds <1..=30>] [--input-window <id>]
aegis-mcp print-config
```

With no subcommand, `serve` reads one JSON-RPC object per stdin line and
writes one response per stdout line. It implements only MCP `2026-07-28`:
`server/discover` and every request use per-request metadata, with no
connection-level negotiation or version downgrade. Discovery is side-effect
free: the bridge does not connect, pair, prompt, or acquire Realm authority
until `tools/list` or `tools/call`. Stdout contains protocol messages only;
diagnostics go to stderr. Closing stdin is the standard graceful shutdown
signal.

| Exit status | Meaning |
|-------------|---------|
| `0` | The command completed or the MCP client closed normally. |
| `1` | IPC, Realm recovery, smoke verification, protocol I/O, or explicitly requested cleanup failed. |
| `2` | Required environment configuration is invalid. |

`check` probes the compositor, prints the effective ceiling and advertised tool
names as JSON, and does not create a Realm. `print-config` prints an MCP client
`[mcp.aegis]` entry using the current executable path.

`smoke` performs a live start-notification mutation and reads the notification
back from compositor state. With no existing managed Realm, it creates a
temporary Realm, verifies `active → paused → active`, exposes it in the status
bar for the observation interval, then revokes it and verifies cleanup. A
second notification is posted and verified after completion. The default
interval is 8 seconds. A pre-existing managed Realm is observed and preserved.
The command requires `Notify`, `CreateRealm`, `TransactRealm`, and
`RevokeRealm` in the approved ceiling and prints both notification ids in a JSON
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
| `AEGIS_MCP_LABEL` | the Realm label | Cosmetic display label presented at pairing; set it per agent product so principals stay distinguishable. 1–128 bytes. |
| `AEGIS_MCP_INSTANCE_ID` | required | Stable connector-installation namespace for the credential, lock, and recovery state. `print-config` emits a random value; keep it stable and unique per MCP entry. 1–128 bytes. |
| `AEGIS_MCP_REALM_LABEL` | `Aegis Agent` | Cosmetic label used for this bridge's Agent Realm; 1–128 bytes. Recovery also requires the authenticated subject binding. |
| `AEGIS_MCP_DATA_DIR` | `$XDG_DATA_HOME/aegis-mcp` | Durable directory for the pairing identity. Without a resolvable data directory the identity is session-only and every start re-pairs. |
| `AEGIS_MCP_IPC_TIMEOUT_SECS` | `5` | Per-connection handshake and I/O timeout for query calls; valid range `1..=60`. Mutation calls that may block on a grant prompt use a 360-second bound. |
| `AEGIS_MCP_REVOKE_ON_EXIT` | `false` | Revoke the managed Realm after graceful MCP EOF. Prefer explicit `realm_reset`; enabling this may wait for runtime consent during shutdown. |

The connector has no provider or credential variables. Realm recovery files
and the per-instance process lock live in the owner-only
`$XDG_RUNTIME_DIR/aegis-mcp/` directory; the pairing identity lives in the
owner-only data directory.

## Capability Borrowing and Pairing

The bridge never falls back to an unnamed privileged connection, and there
is nothing to configure on the Aegis side. On first contact the bridge
declares its display label and the operation families it can use; the
compositor opens the **pairing prompt**, a capability checklist in
compositor chrome:

> **"Aegis Agent" wants to borrow desktop capabilities**
> ☐ window management ☐ workspaces ☐ notifications
> ☐ sensitive (confirmed again on first use): close windows, Realm capture,
> Realm input, Realm management, sandboxed launch

The set approved here becomes the principal's **ceiling**, stored in the
compositor-held registry. The compositor then issues the principal id and
credential; the bridge persists the credential in its data directory and
presents it on every later connection, so restarts, upgrades, and script
edits never re-prompt. A label is cosmetic: it authenticates nothing, the
user may rename principals, and a different installation reusing a known
label triggers a warning in the prompt.

A platform-owned **dangerous set** — `Close`, `InjectRealmInput`,
`CreateRealm`, `TransactRealm`, `RevokeRealm`, `CaptureRealm`,
`LaunchInRealm` — always requires an interactive **runtime grant** on first
use, however the ceiling was approved:

- **Deny** refuses and is remembered for the session (no prompt spam).
- **Allow once** applies to that single call.
- **Allow session** applies until the compositor restarts or the decision is
  revoked by the user.
- **Always allow** persists in `$XDG_DATA_HOME/aegis/grants.json` until
  revoked.

Manage everything with `aegis-cli permissions` (`list`, `revoke`, `forget`,
`rename`, `set-ceiling`, `register`) or the Permissions section of the Agent
Workspaces application. Pairings, grants, revocations, and renames are
recorded in the mutation journal.

The bridge refreshes the credential-bound ceiling for every `tools/list`,
and the IPC server reauthorizes the principal for every live request.
Forgetting a principal or narrowing its ceiling therefore applies to both
new and already connected clients. Gated tools remain advertised because
calling one opens the runtime-grant path.

## MCP Client Configuration

An MCP client loads the connector as a write-capable server because one
catalog contains both observation and mutation tools. An `aegis-agent`
configuration entry is:

```toml
[mcp.aegis]
command = ["/absolute/path/to/aegis-mcp"]
enabled = true
read_only = false
environment = { AEGIS_MCP_INSTANCE_ID = "stable-random-connector-id" }
```

With the server key `aegis`, aegis-agent prefixes the public tool names as
`mcp__aegis__<tool>`.

## Tools

Operations marked ▲ are in the platform dangerous set: the first use of the
tool prompts for a runtime grant (once / session / always).

| Connector-local name | Required capability / operation | Contract |
|----------------------|---------------------------------|----------|
| `desktop_snapshot` | `query` | Windows, workspaces, outputs, all Realms, and the granted ceiling. |
| `desktop_journal` | `query` | Up to 200 ordered mutations and a pagination cursor. |
| `apps_list` | `query` | Filtered XDG applications with trusted `desktop_id` values. |
| `focus_window` | `control` / `Focus` | Queue focus. |
| `minimize_window` | `control` / `Minimize` | Queue minimization. |
| `close_window` ▲ | `control` / `Close` | Queue an explicit close request. |
| `move_window_to_workspace` | `control` / `MoveToWorkspace` | Queue a move by durable ids. |
| `switch_workspace` | `control` / `SwitchWorkspace` | Queue an adjacent switch. |
| `switch_workspace_to` | `control` / `SwitchWorkspaceTo` | Queue a switch by id. |
| `set_window_geometry` | `control` / `SetWindowGeometry` | Queue a floating logical rectangle. |
| `toggle_tiling` | `control` / `ToggleTiling` | Queue a layout toggle. |
| `toggle_overview` | `control` / `ToggleOverview` | Queue the overview. |
| `post_notification` | `control` / `Notify` | Queue an agent notification. |
| `realm_status` | `query` | Inspect the bridge-managed Realm without creating it. |
| `realm_ensure` ▲ | `realm` / `CreateRealm` | Create or recover the private Agent Realm. |
| `realm_launch_app` ▲ | `realm` / `LaunchInRealm` | Launch a catalogued application in its sandbox. |
| `realm_transfer_window` ▲ | `realm` / `TransactRealm` | Transfer authority only to the agent or the human Realm. |
| `realm_set_state` ▲ | `realm` / `TransactRealm` | Pause or resume with an optimistic receipt. |
| `realm_capture` ▲ | `realm` / `CaptureRealm` | Return directed PNG image content, an owner-only compatibility path, placements, and revision. |
| `realm_input` ▲ | `input` / `InjectRealmInput` | Queue at most 64 target-local actions through the Realm seat. |
| `realm_reset` ▲ | `realm` / `RevokeRealm` | Revoke and atomically return controlled groups to the human Realm. |

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
`aegis-agent` forwards to the model as image input. It also atomically stores the
same directed PNG in the owner-only runtime directory and returns
`image_path`. Clients whose MCP adapter renders only text can pass that path
to the agent's built-in `read_image` tool. If neither route makes pixels
model-visible, the agent must not guess coordinates or use visual input.
Inline MCP images are capped at 32 MiB; larger captures set
`image_attached = false` and remain available through `image_path`. The
compatibility file is replaced by each capture and removed when the managed
Realm is reset or cleanly revoked.
