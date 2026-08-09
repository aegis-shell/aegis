# aegis-mcp Bridge Reference

`aegis-mcp` is the scoped Aegis platform bridge: the standard MCP access
point for any agent operating the desktop. It is an MCP stdio server, not
a model runtime: the agent product owns providers, credentials, sessions,
and user interaction on its side of the connection.

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
free: the bridge does not connect, pair, prompt, or acquire Interaction Domain authority
until `tools/list` or `tools/call`. Stdout contains protocol messages only;
diagnostics go to stderr. Closing stdin is the standard graceful shutdown
signal.

| Exit status | Meaning |
|-------------|---------|
| `0` | The command completed or the MCP client closed normally. |
| `1` | IPC, Interaction Domain recovery, smoke verification, protocol I/O, or explicitly requested cleanup failed. |
| `2` | Required environment configuration is invalid. |

`check` probes the compositor, prints the effective ceiling and advertised tool
names as JSON, and does not create an Interaction Domain. `print-config` prints an MCP client
`[mcp.aegis]` entry using the current executable path.

`smoke` performs a live start-notification mutation and reads the notification
back from compositor state. With no existing managed Interaction Domain, it creates a
temporary Interaction Domain, verifies `active → paused → active`, exposes it in the status
bar for the observation interval, then revokes it and verifies cleanup. A
second notification is posted and verified after completion. The default
interval is 8 seconds. A pre-existing managed Interaction Domain is observed and preserved.
The command requires `Notify`, `CreateInteractionDomain`, `TransactInteractionDomain`, and
`RevokeInteractionDomain` in the approved ceiling and prints both notification ids in a JSON
evidence report.

With `--input-window`, `smoke` additionally requires `TransactInteractionDomain`,
`ObserveInteractionDomain`, and `InjectInteractionDomainInput`. It accepts only an explicitly selected,
visible, human-controlled window; transfers that window while retaining the
human as an observer; obtains a semantic observation; commits one
observation-bound, target-local pointer move at its center; waits for the
matching `ActorAction` journal entry; and restores the window to the human
Interaction Domain during cleanup. It never clicks or types. The JSON report includes the
local position, journal sequence, and restoration result.

## User-Visible State

| Surface | Behavior |
|---------|----------|
| Notification toast and history | Shows agent-originated notifications. Do Not Disturb suppresses only the toast. |
| Command panel Agent Workspaces status row | In the System tab. One live Interaction Domain shows its own label; multiple Interaction Domains show a count and aggregate state. It does not report whether an agent process is online. |
| Agent operation feedback | After Interaction Domain input is applied, shows a labeled circular Agent crosshair, movement trail, click pulse, or scroll/keyboard label over a visible read-only mirror. Hidden targets and positionless input use a background-operation pill. This never moves the user's XDG cursor and is excluded from Interaction Domain capture. |
| `aegis interaction-domain list` | Shows each Interaction Domain's authority revision, state, seat, and controlled-window count. |
| Overview Interaction Domain shelf | Shows live Interaction Domain targets and window authority during overview and drag transfer. |

Committed Interaction Domain receipts and observed state are authoritative. A transient
toast or a queued response alone is not proof that an operation was applied.

## Environment

| Variable | Default | Description |
|----------|---------|-------------|
| `AEGIS_MCP_SOCKET` | `$XDG_RUNTIME_DIR/aegis.sock` | Aegis IPC socket. |
| `AEGIS_MCP_LABEL` | the Interaction Domain label | Cosmetic display label presented at pairing; set it per agent product so principals stay distinguishable. 1–128 bytes. |
| `AEGIS_MCP_INSTANCE_ID` | required | Stable connector-installation namespace for the credential, lock, and recovery state. `print-config` emits a random value; keep it stable and unique per MCP entry. 1–128 bytes. |
| `AEGIS_MCP_INTERACTION_DOMAIN_LABEL` | `Aegis Agent` | Cosmetic label used for this bridge's Agent Interaction Domain; 1–128 bytes. Recovery also requires the authenticated subject binding. |
| `AEGIS_MCP_DATA_DIR` | `$XDG_DATA_HOME/aegis-mcp` | Durable directory for the pairing identity. Without a resolvable data directory the identity is session-only and every start re-pairs. |
| `AEGIS_MCP_IPC_TIMEOUT_SECS` | `5` | Per-connection handshake and I/O timeout for query calls; valid range `1..=60`. Mutation calls that may block on a grant prompt use a 360-second bound. |
| `AEGIS_MCP_REVOKE_ON_EXIT` | `false` | Revoke the managed Interaction Domain after graceful MCP EOF. Prefer explicit `interaction_domain_reset`; enabling this may wait for runtime consent during shutdown. |

The connector has no provider or credential variables. Interaction Domain recovery files
and the per-instance process lock live in the owner-only
`$XDG_RUNTIME_DIR/aegis-mcp/` directory; the pairing identity lives in the
owner-only data directory. Identity loading uses `O_NOFOLLOW`, validates the
owner, mode, regular-file type, link count, size, principal, and bounded
credential before accepting it. Serialized/read credential buffers and
in-memory credential copies are zeroized after use; debug output is redacted.

## Capability Borrowing and Pairing

The bridge never falls back to an unnamed privileged connection, and there
is nothing to configure on the Aegis side. On first contact the bridge
declares its display label and the operation families it can use; the
compositor opens the **pairing prompt**, a capability checklist in
compositor chrome:

> **"Aegis Agent" wants to borrow desktop capabilities**
> ☐ window observations ☐ window management ☐ workspaces ☐ notifications
> ☐ sensitive (confirmed again on first use): close windows, Interaction Domain capture,
> semantic Interaction Domain observation, Interaction Domain input, Interaction Domain management, sandboxed launch

The set approved here becomes the principal's **ceiling**, stored in the
compositor-held registry. The compositor then issues the principal id and
credential; the bridge persists the credential in its data directory and
presents it on every later connection, so restarts, upgrades, and script
edits never re-prompt. A label is cosmetic: it authenticates nothing, the
user may rename principals, and a different installation reusing a known
label triggers a warning in the prompt.

A platform-owned **dangerous set** — `Close`, `InjectInteractionDomainInput`,
`CreateInteractionDomain`, `TransactInteractionDomain`, `RevokeInteractionDomain`, `CaptureInteractionDomain`,
`CaptureWindow`,
`ObserveInteractionDomain`, `LaunchInInteractionDomain`, `LaunchApp` — always requires an interactive **runtime
grant** on first use, however the ceiling was approved:

- **Deny** refuses and is remembered for the session (no prompt spam).
- **Allow once** applies to that single call.
- **Allow session** applies until the compositor restarts or the decision is
  revoked by the user.
- **Always allow** persists in `$XDG_DATA_HOME/aegis/grants.json` until
  revoked.

Manage everything with `aegis permissions` (`list`, `revoke`, `forget`,
`rename`, `set-ceiling`, `register`). Pairings, grants, revocations, and
renames are recorded in the mutation journal.

The bridge refreshes the credential-bound ceiling for every `tools/list`,
and the IPC server reauthorizes the principal for every live request.
Forgetting a principal or narrowing its ceiling therefore applies to both
new and already connected clients. Gated tools remain advertised because
calling one opens the runtime-grant path.

## MCP Client Configuration

An MCP client loads the connector as a write-capable server because one
catalog contains both observation and mutation tools. A client
configuration entry is:

```toml
[mcp.aegis]
command = ["/absolute/path/to/aegis-mcp"]
enabled = true
read_only = false
environment = { AEGIS_MCP_INSTANCE_ID = "stable-random-connector-id" }
```

With the server key `aegis`, an MCP client that namespaces tools exposes
the public tool names as `mcp__aegis__<tool>`.

## Tools

Operations marked ▲ are in the platform dangerous set: the first use of the
tool prompts for a runtime grant (once / session / always).

| Connector-local name | Required capability / operation | Contract |
|----------------------|---------------------------------|----------|
| `desktop_snapshot` | `query` / `ObserveWindows`, `ObserveWorkspaces`, `ObserveOutputs`, `ObserveInteractionDomains` | Actor-scoped windows, workspaces, outputs, Interaction Domains, and the granted ceiling. |
| `desktop_journal` | `query` / `ObserveJournal` | Up to 200 principal- and resource-filtered mutations and a pagination cursor. |
| `apps_list` | `query` | Filtered XDG applications with trusted `desktop_id` values. |
| `launch_app` ▲ | `control` / `LaunchApp` | Launch a catalogued application on the desktop. `workspace_id` places its first window on an existing workspace, `new_workspace` (with optional `workspace_label`) on a fresh one; placement never switches the user's view. |
| `focus_window` | `control` / `Focus` | Queue focus. `switch_workspace` (default true) reveals the window's workspace; `false` raises a hidden window within its own workspace without switching the user's view or taking keyboard focus. |
| `minimize_window` | `control` / `Minimize` | Queue minimization. |
| `close_window` ▲ | `control` / `Close` | Queue an explicit close request. |
| `move_window_to_workspace` | `control` / `MoveToWorkspace` | Queue a move by durable ids. |
| `switch_workspace` | `control` / `SwitchWorkspace` | Queue an adjacent switch. |
| `switch_workspace_to` | `control` / `SwitchWorkspaceTo` | Queue a switch by id. |
| `set_window_geometry` | `control` / `SetWindowGeometry` | Queue a floating logical rectangle. |
| `toggle_tiling` | `control` / `ToggleTiling` | Queue a layout toggle. |
| `toggle_overview` | `control` / `ToggleOverview` | Queue the overview. |
| `post_notification` | `control` / `Notify` | Queue an agent notification. |
| `interaction_domain_status` | `query` / `ObserveInteractionDomains` | Inspect the bridge-managed Interaction Domain without creating it. |
| `interaction_domain_ensure` ▲ | `interaction_domain` / `CreateInteractionDomain` | Create or recover the private Agent Interaction Domain. |
| `interaction_domain_launch_app` ▲ | `interaction_domain` / `LaunchInInteractionDomain` | Launch a catalogued application in its sandbox. |
| `interaction_domain_transfer_window` ▲ | `interaction_domain` / `TransactInteractionDomain` | Transfer authority only to the agent or the human Interaction Domain. |
| `interaction_domain_set_state` ▲ | `interaction_domain` / `TransactInteractionDomain` | Pause or resume with an optimistic receipt. |
| `interaction_domain_observe` ▲ | `query` / `ObserveInteractionDomain` | Return compositor roots plus validated accessibility descendants and one short-lived observation token without pixels. |
| `interaction_domain_capture` ▲ | `interaction_domain` / `CaptureInteractionDomain` | Return directed PNG image content, a compatibility path, placements, revision, and the correlated semantic observation. |
| `interaction_domain_input` ▲ | `input` / `InjectInteractionDomainInput` | Revalidate one observation token and commit semantic actions through the target's owning provider, or bounded target-local fallback actions through the Interaction Domain seat. |
| `interaction_domain_reset` ▲ | `interaction_domain` / `RevokeInteractionDomain` | Revoke and atomically return controlled groups to the human Interaction Domain. |
| `window_capture` ▲ | `control` / `CaptureWindow` | Return one window's real PNG content, a compatibility path, and its geometry, whether the window is visible, occluded, minimized, or on another workspace. |

`interaction_domain_transfer_window` accepts `agent` or `human` as its `target`.
`interaction_domain_input` requires `target_window_id`, optional
`target_local_id` (zero is the compositor window root),
`observation_token`, and `actions`. Obtain
the single-use token from `interaction_domain_observe` or `interaction_domain_capture`; do not reuse a
token after success or refusal. Accessibility targets accept one of `invoke`,
`focus`, `set_value`, `type_text`, `select`, `expand`, or `collapse`. Window
roots accept bounded `pointer_move`, `click`, `scroll`, and `key_press`
fallback actions.
The bridge retains the authenticated IPC channel that issued each token until
commit or expiry, preserving the compositor's connection binding across
separate MCP tool calls.
Pointer positions are semantic-target-local logical coordinates. Click buttons
are `left`, `right`, `middle`, `side`, or `extra`; key codes are Linux evdev
codes from 0 through 767. The compositor validates the Actor, Interaction Domain,
authority revision, complete semantic target, declared actions, active seat,
and bounds again before delivering the complete batch. For accessibility
targets, the out-of-process provider re-reads the live AT-SPI precondition
again immediately before toolkit dispatch. State changes abort
instead of clicking stale coordinates. Success returns `status = "committed"`,
`verified = true`, and the compositor's action receipt.

Ordinary desktop responses with `status = "queued"` are not proof of applied
state. Use a fresh snapshot or journal to verify them. Interaction Domain lifecycle,
authority transactions, and `interaction_domain_input` return committed receipts.

## Capture Compatibility

Semantic observation is the primary interaction path. Pixel capture is a
separately granted compatibility path for applications whose internal state
is unavailable through the supervised accessibility adapter. A framebuffer is never
treated as semantic or action authority.

The connector emits standard MCP `image` content and JSON metadata, which
the agent product forwards to the model as image input. It also atomically
stores the same directed PNG in the owner-only runtime directory and returns
`image_path`. Clients whose MCP adapter renders only text can pass that path
to an image-reading tool the agent product provides. If neither route makes
pixels model-visible, the agent must not guess coordinates or use visual
input.
Inline MCP images are capped at 32 MiB; larger captures set
`image_attached = false` and remain available through `image_path`. The
compatibility file is replaced by each capture and removed when the managed
Interaction Domain is reset or cleanly revoked.

`window_capture` follows the same dual-channel contract for windows on the
human desktop: standard MCP `image` content plus an atomically written
owner-only file at `{key}.window-capture.png` under the connector's runtime
directory (the directed Interaction Domain capture keeps `{key}.capture.png`,
so the two kinds never overwrite each other). Its JSON metadata carries
`window_id`, physical `width`/`height`, `scale_milli`, the toplevel's logical
`rect`, `image_bytes`, `image_attached`, and `image_path`. Unlike
`interaction_domain_capture` it returns no observation token: pixels from the
human desktop never carry input authority.
