# How to Connect Neenee to ASS

Use the ASS MCP bridge to give the existing Neenee product scoped desktop and
Agent Realm tools. Neenee continues to own its provider and session
configuration; ASS remains the authority for every desktop action.

## Build the Bridge

From the ASS repository:

```bash
cargo build --release -p ass-neenee
```

The binary is `target/release/ass-neenee-mcp`.

## Configure the ASS Scope

Add a named scope to the ASS configuration:

```toml
[[agent.scope]]
name = "neenee"
ops = [
  "Focus",
  "Minimize",
  "MoveToWorkspace",
  "SwitchWorkspace",
  "SwitchWorkspaceTo",
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

Add `Close` only if Neenee should be allowed to close windows. Omit
`InjectRealmInput` for observation-only use. Leave `realms` unset because the
managed Realm id is allocated at runtime.

Restart ASS after introducing the scope. Later scope edits reload live;
narrowing applies immediately, while broader grants require reconnecting the
MCP server so Neenee discovers the additional tools.

## Check the Grant

With ASS running:

```bash
target/release/ass-neenee-mcp check
```

The JSON output shows the compositor-granted capabilities, allowlists, and
exact connector-local tool names. Fix an unknown scope or missing operation
before configuring Neenee.

## Run the Visual Smoke Test

Run a live, reversible smoke test before connecting Neenee:

```bash
target/release/ass-neenee-mcp smoke
```

Watch for a Neenee notification and a temporary `Neenee · Active` or
`Neenee · Paused` indicator in the status bar. Click the indicator while it
is present. Control Center opens directly on **AI Workspaces**, where the
temporary Realm id, state, and controlled-window count are visible.

The command posts a start notification and verifies it by reading it back
from compositor state. It then verifies a temporary Realm through
`active → paused → active → revoked`, keeps the visual state available for
eight seconds by default, and confirms cleanup. Use
`--observe-seconds 15` when more inspection time is needed. If a managed
Realm already exists, the test preserves it instead of changing or revoking
user work. A second verified notification reports completion after cleanup,
so the visible result remains available after the Agent indicator disappears.

Do Not Disturb may suppress the transient toast. The notification remains
visible from the status bar bell, and the command still verifies its
presence in compositor state. A successful command prints a JSON report with
`"mode": "live"` and `"status": "passed"`.

To verify the real Agent operation feedback path, first open a disposable
window and find its id with `ass-control windows`, then run:

```bash
target/release/ass-neenee-mcp smoke --input-window <window-id> --observe-seconds 10
```

This opt-in probe temporarily transfers only that window, retains it as a
read-only mirror on the physical desktop, and applies one pointer move at its
center. The user's XDG cursor does not move. Look for a colored circular
crosshair, a short movement trail, and an `AGENT · Neenee · Pointer move`
label. The command waits for an `Applied` journal entry and returns the window
to the human Realm even if the probe fails. It does not click, type, close, or
launch anything. Use a disposable window because authority changes briefly
even though cleanup is automatic.

## Configure Neenee

Print a starting MCP entry:

```bash
target/release/ass-neenee-mcp print-config
```

Add it to Neenee's `config.toml`, then add the shipped skill path:

```toml
[mcp.ass]
command = ["/absolute/path/to/ass/target/release/ass-neenee-mcp"]
enabled = true
read_only = false

[skills]
paths = ["/absolute/path/to/ass/integrations/neenee/skills"]
```

Start Neenee from its own repository or installed binary. Open `/mcp` and
confirm `ass` is connected, then run `/skills reload` and load
`ass-desktop-realm` when the task concerns desktop interaction.

## Exercise the Realm Loop

Ask Neenee to list applications and launch one in its private Realm. A safe
interaction loop is:

1. `apps_list` resolves a real desktop id.
2. `realm_launch_app` creates or recovers the bridge-managed Realm and queues
   the sandboxed launch.
3. `realm_status` waits for the interaction group to appear.
4. `realm_capture` observes only the Realm's directed output.
5. `realm_input` uses the captured placement and target-local coordinates.
6. another capture verifies the effect.

If the Neenee version exposes only capture metadata, pass the returned
`image_path` to Neenee's built-in `read_image` tool. Stop before coordinate
input rather than guessing if neither that compatibility path nor the MCP
image block becomes visible. See [Capture
Compatibility](../reference/neenee.md#capture-compatibility).

## Recover or Reset

The connector stores the managed Realm id under
`$XDG_RUNTIME_DIR/ass-neenee/`. If an MCP refresh kills the child before a
graceful shutdown, the next connector for the same scope recovers that Realm.

Ask Neenee to call `realm_reset` only when you want to end the Realm and return
its controlled windows to the human Realm. Normal graceful EOF also revokes by
default. Use `ASS_NEENEE_REVOKE_ON_EXIT=false` only when Realm continuity
across graceful connector restarts is intentional.

For the complete scope, environment, tool, and exit-status contract, see the
[Neenee Integration Reference](../reference/neenee.md).
