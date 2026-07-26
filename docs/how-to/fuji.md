# How to Connect fuji to ASS

Use the fuji agent and its MCP bridge to operate the ASS desktop with scoped
desktop and Agent Realm tools. fuji owns its provider and session
configuration; ASS remains the authority for every desktop action.

## Build the Bridge and the Agent

From the ASS repository:

```bash
cargo build --release -p aegis-fuji
```

The binaries are `target/release/aegis-fuji-mcp` and `target/release/fuji`.

## Configure the ASS Scope

Add a named scope to the ASS configuration:

```toml
[[agent.scope]]
name = "fuji"
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

Add `Close` only if fuji should be allowed to close windows. Omit
`InjectRealmInput` for observation-only use. Leave `realms` unset because the
managed Realm id is allocated at runtime.

Restart ASS after introducing the scope. Later scope edits reload live;
narrowing applies immediately, while broader grants require reconnecting the
MCP server so fuji discovers the additional tools.

## Check the Grant

With ASS running:

```bash
target/release/aegis-fuji-mcp check
```

The JSON output shows the compositor-granted capabilities, allowlists, and
exact connector-local tool names. Fix an unknown scope or missing operation
before configuring fuji.

## Run the Visual Smoke Test

Run a live, reversible smoke test before connecting fuji:

```bash
target/release/aegis-fuji-mcp smoke
```

Watch for a fuji notification and a temporary `Fuji · Active` or
`Fuji · Paused` indicator in the status bar. Click the indicator while it
is present. **AI Workspaces** opens directly, where the temporary Realm id,
state, and controlled-window count are visible.

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
window and find its id with `aegis-ctl windows`, then run:

```bash
target/release/aegis-fuji-mcp smoke --input-window <window-id> --observe-seconds 10
```

This opt-in probe temporarily transfers only that window, retains it as a
read-only mirror on the physical desktop, and applies one pointer move at its
center. The user's XDG cursor does not move. Look for a colored circular
crosshair, a short movement trail, and an `AGENT · Fuji · Pointer move`
label. The command waits for an `Applied` journal entry and returns the window
to the human Realm even if the probe fails. It does not click, type, close, or
launch anything. Use a disposable window because authority changes briefly
even though cleanup is automatic.

## Configure fuji

Print a starting configuration:

```bash
target/release/fuji print-config
```

Write it to `$XDG_CONFIG_HOME/fuji/config.toml`, set your provider
credential in the environment, and point the MCP entry at the bridge and the
shipped skills:

```toml
[provider]
kind = "anthropic"
model = "claude-sonnet-4-5"

[mcp.ass]
command = ["/absolute/path/to/ass/target/release/aegis-fuji-mcp"]
enabled = true
read_only = false

[skills]
paths = ["/absolute/path/to/ass/integrations/fuji/skills"]
```

Validate the setup:

```bash
target/release/fuji check
```

The command reports the resolved provider, the credential variable, the
discovered skills, and every enabled MCP server with its tool count. Then
run a prompt:

```bash
target/release/fuji run "list the visible windows"
```

## Exercise the Realm Loop

Ask fuji to list applications and launch one in its private Realm. A safe
interaction loop is:

1. `apps_list` resolves a real desktop id.
2. `realm_launch_app` creates or recovers the bridge-managed Realm and queues
   the sandboxed launch.
3. `realm_status` waits for the interaction group to appear.
4. `realm_capture` observes only the Realm's directed output.
5. `realm_input` uses the captured placement and target-local coordinates.
6. another capture verifies the effect.

The capture arrives as MCP image content, which fuji forwards to the model
directly. When a client exposes only capture metadata, pass the returned
`image_path` to fuji's built-in `read_image` tool. Stop before coordinate
input rather than guessing if neither route makes pixels model-visible. See
[Capture Compatibility](../reference/fuji.md#capture-compatibility).

## Recover or Reset

The bridge stores the managed Realm id under `$XDG_RUNTIME_DIR/aegis-fuji/`.
If a refresh kills the bridge before a graceful shutdown, the next bridge
for the same scope recovers that Realm.

Ask fuji to call `realm_reset` only when you want to end the Realm and return
its controlled windows to the human Realm. Normal graceful EOF also revokes by
default. Use `ASS_FUJI_REVOKE_ON_EXIT=false` only when Realm continuity
across graceful bridge restarts is intentional.

For the complete scope, environment, tool, and exit-status contract, see the
[fuji Bridge Reference](../reference/fuji.md). For the agent CLI and
configuration, see the [fuji Agent Reference](../reference/fuji-agent.md).
