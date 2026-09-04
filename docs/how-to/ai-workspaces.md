# How to Use Agent Workspaces

Agent Workspaces isolate an agent's pointer, keyboard, focus, rendering, and
application process tree from the physical desktop.

“Agent Workspace” is the user-facing term for an Agent Interaction Domain,
shown as a status label in the command panel. Each workspace is backed by an
`InteractionDomain`, the compositor's security and routing primitive; it is
not a normal desktop workspace. Lifecycle management uses the
`tessera interaction-domain *` CLI or the `interaction_domain_*` MCP tools.

## Create a Workspace

```bash
tessera interaction-domain create "Research"
tessera interaction-domain list
```

The workspace starts with an independent pointer/keyboard seat and a
1920×1080 virtual output. Creating it does not start or connect an agent.
Each application launched into it receives a private, mount-scoped Wayland
portal.

## Transfer a Running Window

1. Press `Super+O` to open Overview.
2. Drag a window thumbnail to an active Interaction Domain on the right shelf.
3. Release the pointer over the Interaction Domain.

tessera transfers the window's complete interaction group in one transaction.
The agent Interaction Domain becomes the only input authority. The physical desktop keeps
a read-only mirror by default, so the window stays visible but does not
receive physical clicks or keystrokes. The mirror is also an input barrier:
clicks do not pass through it to a window underneath. You can still
reposition it: press-and-drag anywhere on the mirror moves it like an
ordinary window, without focusing it or delivering content input.

Identify the mirror by its subdued guard and Interaction Domain status label. Hover over
it to see the `not-allowed` cursor. The guard remains visible between Agent
operations and while the Interaction Domain is paused; the Agent's short-lived
operation feedback (a semi-transparent mask, an arrow cursor, and an
operation label) appears separately above it.

Drag the mirror to **Physical desktop** in Overview to return control.

Use the CLI when the graphical shell is unavailable:

```bash
tessera interaction-domain transfer 42 2
tessera interaction-domain transfer 42 1
```

Add `--no-mirror` to remove the source presentation after transfer.

## Launch an Isolated Application

Run tessera through the packaged systemd user service. Interaction Domain launches require
delegated `cpu`, `memory`, and `pids` cgroup v2 controllers; starting tessera
directly from a shared terminal scope keeps desktop use available but makes
Interaction Domain application launch fail closed.

```bash
systemctl --user daemon-reload
systemctl --user start tessera.service
```

Launch a desktop entry directly inside an Interaction Domain when the application process
also needs isolation:

```bash
tessera interaction-domain launch 2 org.mozilla.firefox.desktop
```

The new window appears on the physical desktop as the same guarded read-only
mirror used for transferred windows. The Agent Interaction Domain's independent seat owns
its pointer, keyboard, focus, modifiers, and grabs from the first toplevel.
Physical clicks, typing, window gestures, and Dock window actions cannot
operate it. Return control by opening Overview and dragging the mirror to
**Physical desktop**.

Interaction Domain launches deny network and host-file access. They expose only
the sandbox's mount-scoped Wayland portal and GPU render nodes, and run
without Linux capabilities in isolated user, mount, PID, IPC, UTS, cgroup,
and network namespaces. The host must provide `/usr/bin/bwrap`.

Set only the resource budget one desktop entry needs in
`~/.config/tessera/config.toml`:

```toml
[interaction_domain_sandbox]
memory_max_mib = 8192
pids_max = 1024
cpu_weight = 100

[[interaction_domain_sandbox.app]]
desktop_id = "org.mozilla.firefox.desktop"
memory_max_mib = 4096
```

Host network sharing and static host-path mounts are intentionally not
configurable. They would be ambient application authority and could not
enforce an exact Actor capability such as `https://amazon.com` or one file.
Resource-aware services must instead consume a short-lived, exact grant bound
to the requesting Actor session.

Transferring an already running window changes compositor input and
presentation authority. It cannot retroactively place that existing process
inside Linux namespaces. Relaunch the application with `interaction-domain launch` when
process, filesystem, or network isolation is required.

## Observe a Workspace

Agents should read semantic state before requesting pixels. In `tessera-mcp`,
call `interaction_domain_observe` to receive the Interaction Domain's compositor-owned window objects
and a short-lived observation token. Each object includes a stable semantic
id, state, declared actions, bounds, target-local size, and revision.

To act, pass that single-use token to `interaction_domain_input` with the selected
semantic target and bounded actions. The compositor consumes the token and aborts
the complete batch if the connection, principal, Interaction Domain authority, semantic
target, seat, or coordinates changed. Obtain a new observation before every
dependent action. A successful response contains a committed action receipt;
a queued response or old screenshot is not sufficient evidence.

Capture the directed virtual output without exposing physical-desktop chrome
or another Interaction Domain:

```bash
tessera interaction-domain capture 2
tessera interaction-domain capture 2 /tmp/research.png
```

Interaction Domain captures are refused while the session is locked, the seat is inactive,
or the Interaction Domain is paused or revoked. In-flight captures are invalidated when
the security state changes. A capture includes a correlated semantic
observation token, but pixel access remains a separate capability and is a
fallback for applications whose internal controls are not semantically
available. Pixels and coordinates alone never authorize input.

Trusted local operators can use `tessera events`. An `InteractionDomainDamaged` event
identifies the changed Interaction Domain and virtual-output damage; request
`tessera interaction-domain capture` only after that event instead of polling continuously.
Authenticated Actors use filtered snapshot and journal polling until a
principal- and resource-filtered push lane is available.

## Pause or Revoke a Workspace

Pause and resume with:

```bash
tessera interaction-domain pause 2
tessera interaction-domain resume 2
```

Pausing disables the Interaction Domain seat and freezes every compositor-managed sandbox
cgroup. Session lock and an inactive virtual terminal apply the same
suspension automatically.

Revoke with:

```bash
tessera interaction-domain revoke 2
```

Revocation is permanent. It transfers controlled interaction groups back to
the physical desktop, closes private Wayland listeners and protocol globals,
invalidates captures, and kills and reaps managed sandbox process trees.

## Interaction-Group Behavior

A single application connection may own several toplevels, popups, and
transient dialogs. tessera moves the complete interaction group when separating
those windows would make seat focus or protocol serials contradictory.
This is observable as several windows moving after one drag; it does not
create a second application instance.

An application does not need multi-seat support: tessera conservatively places
all toplevels owned by one Wayland client connection in one interaction group,
and that complete group has one controlling Interaction Domain at a time. Native multi-seat
behavior is detected only so seat resources can be routed correctly; tessera does
not automatically split one client connection across Interaction Domains. A sandbox portal
supports multiple Wayland connections made by one multi-process application
instance; that transport behavior is separate from multi-seat input.

See the [Configuration Reference](../reference/config.md#interaction-domain-sandbox) for
launch policy and the [IPC Reference](../reference/ipc.md#interaction-domain-authority)
for Interaction Domain transactions, output limits, leases, and scope rules.
