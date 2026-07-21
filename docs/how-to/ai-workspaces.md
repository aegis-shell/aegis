# How to Use AI Workspaces

AI Workspaces isolate an agent's pointer, keyboard, focus, rendering, and
application process tree from the physical desktop.

## Create a Workspace

1. Open **Control Center** from the launcher.
2. Select **AI Workspaces**.
3. Select **New AI Workspace**.

The workspace starts with an independent pointer/keyboard seat and a
1920×1080 virtual output. Each application launched into it receives a
private, mount-scoped Wayland portal.

The command-line equivalent is:

```bash
ass-control realm create "Research"
ass-control realm list
```

## Transfer a Running Window

1. Press `Super+O` to open Overview.
2. Drag a window thumbnail to an active Realm on the right shelf.
3. Release the pointer over the Realm.

ass transfers the window's complete interaction group in one transaction.
The agent Realm becomes the only input authority. The physical desktop keeps
a read-only mirror by default, so the window stays visible but does not
receive physical clicks or keystrokes. The mirror is also an input barrier:
clicks do not pass through it to a window underneath.

Drag the mirror to **Physical desktop** in Overview to return control.

Use the CLI when the graphical shell is unavailable:

```bash
ass-control realm transfer 42 2
ass-control realm transfer 42 1
```

Add `--no-mirror` to remove the source presentation after transfer.

## Launch an Isolated Application

Run ass through the packaged systemd user service. Realm launches require
delegated `cpu`, `memory`, and `pids` cgroup v2 controllers; starting ass
directly from a shared terminal scope keeps desktop use available but makes
Realm application launch fail closed.

```bash
systemctl --user daemon-reload
systemctl --user start ass.service
```

Launch a desktop entry directly inside a Realm when the application process
also needs isolation:

```bash
ass-control realm launch 2 org.mozilla.firefox.desktop
```

Realm launches deny network and host-file access by default. They expose only
the sandbox's mount-scoped Wayland portal and GPU render nodes, and run
without Linux capabilities in isolated user, mount, PID, IPC, UTS, cgroup,
and network namespaces. The host must provide `/usr/bin/bwrap`.

Grant only the capabilities one desktop entry needs in
`~/.config/ass/config.toml`:

```toml
[realm_sandbox]
memory_max_mib = 8192
pids_max = 1024
cpu_weight = 100

[[realm_sandbox.app]]
desktop_id = "org.mozilla.firefox.desktop"
network = true
readable_paths = ["/home/alice/Research"]
writable_paths = ["/home/alice/Downloads"]
```

These grants apply to new launches. Revoke and relaunch an existing sandbox
after narrowing its policy.

Transferring an already running window changes compositor input and
presentation authority. It cannot retroactively place that existing process
inside Linux namespaces. Relaunch the application with `realm launch` when
process, filesystem, or network isolation is required.

## Observe a Workspace

Capture the directed virtual output without exposing physical-desktop chrome
or another Realm:

```bash
ass-control realm capture 2
ass-control realm capture 2 /tmp/research.png
```

Realm captures are refused while the session is locked, the seat is inactive,
or the Realm is paused or revoked. In-flight captures are invalidated when
the security state changes.

Long-running observers can use `ass-control subscribe`. A `RealmDamaged` event
identifies the changed Realm and virtual-output damage; request
`realm capture` only after that event instead of polling continuously.

## Pause or Revoke a Workspace

Use **Pause** in Control Center, or run:

```bash
ass-control realm pause 2
ass-control realm resume 2
```

Pausing disables the Realm seat and freezes every compositor-managed sandbox
cgroup. Session lock and an inactive virtual terminal apply the same
suspension automatically.

Use **Revoke**, confirm the destructive action, or run:

```bash
ass-control realm revoke 2
```

Revocation is permanent. It transfers controlled interaction groups back to
the physical desktop, closes private Wayland listeners and protocol globals,
invalidates captures, and kills and reaps managed sandbox process trees.

## Interaction-Group Behavior

A single application connection may own several toplevels, popups, and
transient dialogs. ass moves the complete interaction group when separating
those windows would make seat focus or protocol serials contradictory.
This is observable as several windows moving after one drag; it does not
create a second application instance.

An application does not need multi-seat support: ass conservatively places
all toplevels owned by one Wayland client connection in one interaction group,
and that complete group has one controlling Realm at a time. Native multi-seat
behavior is detected only so seat resources can be routed correctly; ass does
not automatically split one client connection across Realms. A sandbox portal
supports multiple Wayland connections made by one multi-process application
instance; that transport behavior is separate from multi-seat input.

See the [Configuration Reference](../reference/config.md#realm-sandbox) for
launch policy and the [IPC Reference](../reference/ipc.md#realm-authority)
for Realm transactions, output limits, leases, and scope rules.
