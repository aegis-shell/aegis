# How to Connect aegis-agent to Aegis

Use the `aegis-agent` CLI and the `aegis-mcp` platform bridge to operate the Aegis desktop with scoped
desktop and Agent Interaction Domain tools. `aegis-agent` owns its provider and session
configuration; Aegis remains the authority for every desktop action. The internal agent persona identity is fuji (宓姬).

## Build the Bridge and the Agent

From the Aegis repository:

```bash
cargo build --locked --release -p aegis-mcp -p aegis-agent
```

The binaries are `target/release/aegis-mcp` and `target/release/aegis-agent`.

## Understand First-Run Pairing

There is nothing to declare in the Aegis configuration. The first time the
bridge connects — during `check`, `smoke`, or the first agent prompt — the
compositor opens a **pairing prompt**: a checklist of the capability groups
`aegis-agent` wants to borrow (window management, workspaces, notifications, and the
sensitive groups marked "confirmed again on first use"). Untick anything you
do not want to lend, then choose **Allow**.

The approved set becomes `aegis-agent`'s ceiling, and the compositor issues a
credential the bridge persists under its data directory. Later starts,
upgrades, and script edits never prompt again. Observation capabilities are
approved independently from action capabilities. The sensitive groups —
closing windows, Interaction Domain capture, semantic Interaction Domain observation, Interaction Domain input,
Interaction Domain management, sandboxed launches — ask once more at first use, with
*Deny / Allow once / Allow session / Always allow* options; "Always" is
remembered until you revoke it.

Review or change everything later with `aegis permissions list`, revoke a
remembered grant with `aegis permissions revoke <principal> <op>`, trim
the ceiling with `set-ceiling`, rename the principal, or `forget` it entirely
to force re-pairing. The same state appears in the Agent Workspaces
application.

## Check the Grant

With Aegis running:

```bash
target/release/aegis-mcp check
```

The JSON output shows the compositor-granted capabilities, allowlists, and
exact connector-local tool names. Dismiss the pairing prompt by mistake and
the command fails closed; run it again to re-trigger pairing.

## Run the Visual Smoke Test

Run a live, reversible smoke test before connecting `aegis-agent`:

```bash
target/release/aegis-mcp smoke
```

Watch for an agent notification and a temporary `Aegis Agent · Active` or
`Aegis Agent · Paused` label in the command panel's Agent Workspaces status row
(`Super+S`, **System** section). Open **Agent Workspaces** from the
application launcher (`Super+A`), where the temporary Interaction Domain id,
state, and controlled-window count are visible. The row reports Interaction Domain
authority, not whether the agent process is online. The first smoke run after
pairing prompts for the sensitive Interaction Domain operations one by one; *Allow
session* covers the rest of the session.

The command posts a start notification and verifies it by reading it back
from compositor state. It then verifies a temporary Interaction Domain through
`active → paused → active → revoked`, keeps the visual state available for
eight seconds by default, and confirms cleanup. Use
`--observe-seconds 15` when more inspection time is needed. If a managed
Interaction Domain already exists, the test preserves it instead of changing or revoking
user work. A second verified notification reports completion after cleanup,
so the visible result remains available after the Agent indicator disappears.

Do Not Disturb may suppress the transient toast. The notification remains
visible from the HUD notification count, and the command still verifies its
presence in compositor state. A successful command prints a JSON report with
`"mode": "live"` and `"status": "passed"`.

To verify the real Agent operation feedback path, first open a disposable
window and find its id with `aegis window`, then run:

```bash
target/release/aegis-mcp smoke --input-window <window-id> --observe-seconds 10
```

This opt-in probe temporarily transfers only that window, retains it as a
read-only mirror on the physical desktop, and applies one pointer move at its
center. The user's XDG cursor does not move. Look for a colored circular
crosshair, a short movement trail, and an `AGENT · Fuji · Pointer move`
label. The command obtains a semantic observation, commits the pointer move
with its single-use token, waits for the matching `ActorAction` journal entry,
and returns the window to the human Interaction Domain even if the probe fails. It does not
click, type, close, or launch anything. Use a disposable window because
authority changes briefly even though cleanup is automatic.

## Configure aegis-agent

Print a starting configuration:

```bash
target/release/aegis-agent print-config
```

Write it to `$XDG_CONFIG_HOME/aegis/agent.toml`, set your provider
credential in the environment, and point the MCP entry at the bridge and the
shipped skills:

```toml
[provider]
kind = "anthropic"
model = "claude-sonnet-4-5"

[mcp.aegis]
command = ["/absolute/path/to/aegis/target/release/aegis-mcp"]
enabled = true
read_only = false
environment = { AEGIS_MCP_INSTANCE_ID = "stable-random-connector-id" }

[skills]
paths = ["/absolute/path/to/aegis/integrations/agent/skills"]
```

Validate the setup:

```bash
target/release/aegis-agent check
```

The command reports the resolved provider, the credential variable, the
discovered skills, and every enabled MCP server with its tool count. Then
run a prompt:

```bash
target/release/aegis-agent run "list the visible windows"
```

## Exercise the Interaction Domain Loop

Ask `aegis-agent` to list applications and launch one in its private Interaction Domain. A safe
interaction loop is:

1. `apps_list` resolves a real desktop id.
2. `interaction_domain_launch_app` creates or recovers the bridge-managed Interaction Domain and queues
   the sandboxed launch.
3. `interaction_domain_status` waits for the interaction group to appear.
4. `interaction_domain_observe` reads stable semantic window objects and returns a
   short-lived, single-use observation token.
5. `interaction_domain_capture` is optional when application state is not available
   semantically; it observes only the Interaction Domain's directed output and returns a
   correlated token too.
6. `interaction_domain_input` passes the token, semantic target id, and target-local
   actions. The compositor aborts if the observed state changed and returns a
   commit receipt on success.
7. a new semantic observation, or a capture when necessary, verifies the
   effect.

The capture arrives as MCP image content, which `aegis-agent` forwards to the model
directly. When a client exposes only capture metadata, pass the returned
`image_path` to `aegis-agent`'s built-in `read_image` tool. Stop before coordinate
input rather than guessing if neither route makes pixels model-visible. See
[Capture Compatibility](../reference/aegis-mcp.md#capture-compatibility).

## Recover or Reset

The bridge stores the managed Interaction Domain handle under
`$XDG_RUNTIME_DIR/aegis-mcp/`. The next process recovers it only when the
stable instance id, authenticated principal, and live Interaction Domain owner all agree;
a matching cosmetic label is insufficient.

Ask `aegis-agent` to call `interaction_domain_reset` only when you want to end the Interaction Domain and return
its controlled windows to the human Interaction Domain. Normal graceful EOF preserves the
Interaction Domain so connector refresh is non-destructive. Set
`AEGIS_MCP_REVOKE_ON_EXIT=true` only when process exit must destroy its Interaction Domain.

For the complete scope, environment, tool, and exit-status contract, see the
[aegis-mcp Bridge Reference](../reference/aegis-mcp.md). For the agent CLI and
configuration, see the [Agent Reference](../reference/agent.md).
