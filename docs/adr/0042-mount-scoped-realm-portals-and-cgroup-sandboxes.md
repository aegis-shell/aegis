# ADR-0042: Mount-scoped Realm portals and cgroup sandboxes

- Status: Accepted
- Date: 2026-07-18

## Context

Realm authority isolates compositor input and presentation, but an
agent-launched process also needs an endpoint that cannot be reused by another
same-user process. A mode-`0600` Wayland pathname authenticates the Unix user,
not the Realm. One inherited connected socket is a strong capability, but an
application instance may contain several processes and open several Wayland
connections.

Process groups are also insufficient lifecycle boundaries. A child can start
a new session or process group and escape group-directed signals. An
unbounded agent process tree can exhaust memory or process identifiers even
when its Wayland authority is correct.

## Decision

Each Realm application launch receives a compositor-created Unix listener.
Bubblewrap bind-mounts the socket inode at `/run/ass/wayland-0` inside the new
mount namespace. A two-way launch gate then:

1. waits until namespace and mount setup has completed;
2. unlinks the randomized host pathname;
3. closes every connection queued before the unlink; and
4. releases application execution.

The compositor accepts every later connection from the retained listener and
assigns it the portal's Realm before the client can enumerate globals. The
listener supports all connections made by that application sandbox, while no
host pathname remains available to another same-user process.

Realm launches use fail-closed bubblewrap user, mount, PID, IPC, UTS, cgroup,
and network namespaces; no Linux capabilities; an ephemeral home and
temporary directory; no session bus; and no host network or user files by
default. GPU render nodes are exposed without KMS card nodes. Explicit
per-desktop-entry configuration may grant network or canonical host paths to
new launches.

Every managed sandbox enters a dedicated cgroup v2 subtree before `exec`.
Memory, process-count, and CPU-weight controllers are mandatory. ASS runs in
its own systemd user service with `cpu`, `memory`, and `pids` delegation,
moves the compositor into a host leaf, and creates Realm cgroups as siblings
under the delegated root. Pause, session lock, and inactive VT use
`cgroup.freeze`; revocation and compositor shutdown use `cgroup.kill` and
reap the supervisor.

## Alternatives

- **A permanent owner-only Realm socket.** Rejected because another process
  with the same Unix user can connect.
- **One connected socket per application launch.** Rejected as the complete
  solution because it prevents a single sandboxed application instance from
  opening additional Wayland connections.
- **A Wayland byte proxy.** Rejected because it must correctly relay every
  message and ancillary file descriptor while adding no authority benefit
  over the mount-scoped listener.
- **Process-group signals.** Rejected because `setsid` or a new process group
  escapes the boundary.
- **Continue without delegated resource controllers.** Rejected. Lifecycle
  control without memory and PID bounds admits denial of service.
- **A virtual machine per Realm.** Not required for the desktop authority
  boundary. A VM remains appropriate when the threat model includes kernel or
  GPU-driver compromise.

## Consequences

- One application instance can use normal multi-process Wayland behavior
  without app-side multi-seat support.
- Realm application launch requires bubblewrap, cgroup v2, and a dedicated
  systemd service with controller delegation.
- Missing namespace, portal, cgroup lifecycle, or resource-limit mechanisms
  reject the launch instead of degrading to an unsandboxed process.
- Network and filesystem policy changes apply to new launches; existing
  sandboxes must be revoked and relaunched to narrow those kernel grants.
- Render-node access retains the kernel and GPU driver in the trusted
  computing base; Realm isolation is not a substitute for a virtual machine
  against hostile native code.
