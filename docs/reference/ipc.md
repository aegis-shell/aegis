# IPC Reference

The ass IPC is protocol version 3, carried as length-framed JSON over the
owner-only Unix socket at `$XDG_RUNTIME_DIR/ass.sock`. Every connection starts
with `Hello`; commands are accepted only after capability and scope checks.
JSON messages are limited to 16 MiB. Large immutable capture payloads use a
separate sealed-file-descriptor transfer described under [Capture](#capture).

## Capabilities

| Capability | Authority | Default |
|------------|-----------|---------|
| `query` | Read snapshots and subscribe to events or the journal. | Always granted. |
| `control` | Mutate windows, workspaces, layout, and notifications. | Server policy. |
| `input` | Inject bounded actions into a target window. | Named scope required. |
| `session` | Quit or perform other session-level operations. | Server policy. |
| `realm` | Create, configure, capture, pause, transfer, launch into, and revoke Realms. | Named scope and explicit operation required. |

Absent `input` and `realm` fields are `false`. An unscoped connection never
receives `input`; Realm operations also require an explicit operation in a
named scope.

Every privileged connection supplies `lease: { ttl_ms }` in `Hello`.
Omitting it strips `control`, `input`, `session`, and `realm`. The allowed
duration is 1,000 through 86,400,000 milliseconds. `RenewLease { ttl_ms }`
renews a live connection-bound lease; an expired lease cannot be renewed.
The reference client requests 900,000 milliseconds by default.

## Queries

| Request | Response | Capability |
|---------|----------|------------|
| `GetWindows` | `Windows` | `query` |
| `GetWorkspaces` | `Workspaces` | `query` |
| `GetNotifications` | `Notifications` | `query` |
| `GetOutputs` | `Outputs` | `query` |
| `GetJournal { since }` | `Journal` | `query` |
| `GetRealms` | `Realms` | `query` |
| `Realm { action }` | `Realm` with a commit receipt | `realm` + explicit scope op |
| `CaptureOutput` | `CaptureOutput` | `control` + explicit scope op |
| `CaptureRealm { realm, region }` | `CaptureRealm` | `realm` + `CaptureRealm` scope op |

`Subscribe` enables coarse events:

- `WindowsChanged`, `WorkspaceChanged`, and `RealmsChanged { revision }`
  invalidate the corresponding snapshots.
- `RealmDamaged { realm, sequence, revision, damage }` reports that an active
  Realm's directed scene changed. `damage` contains at most 64 rectangles in
  virtual-output logical coordinates. Surface commits conservatively
  invalidate the complete Realm-local window placement; mapping, removal,
  transfer, observer, and output-layout changes may invalidate the complete
  output. Pixels remain pull-based through `CaptureRealm`.

`SubscribeJournal` additionally enables one ordered `Journal` event per
mutation decision. Each `JournalEntry` contains a real per-connection
`Origin::Ipc { conn_id }`, an `effect`, and one tagged `mutation`:

- `Command { cmd }`; or
- `Realm { action, before_revision, after_revision }`.

Capability, lease, validation, and scope refusals are journaled even when the
mutation never reaches the compositor main loop. Realm actions rejected by
live state carry the unchanged revision in both revision fields.

## Commands

| Command | Capability | Operation class | Target |
|---------|------------|-----------------|--------|
| `Focus { id }` | `control` | `Focus` | Window |
| `Minimize { id }` | `control` | `Minimize` | Window |
| `Close { id }` | `control` | `Close` | Window |
| `Move { id }` | `control` | `Move` | Window |
| `SetWindowGeometry { id, rect }` | `control` | `SetWindowGeometry` | Window |
| `InjectInput { id, actions }` | `input` | `InjectInput` | Window |
| `InjectRealmInput { realm, id, actions }` | `input` | `InjectRealmInput` | Realm and window |
| `LaunchInRealm { realm, desktop_id }` | `realm` | `LaunchInRealm` | Realm |
| `Cycle { forward }` | `control` | `Cycle` | — |
| `SwitchWorkspace { dir }` | `control` | `SwitchWorkspace` | Focused output |
| `SwitchWorkspaceTo { id }` | `control` | `SwitchWorkspaceTo` | Workspace |
| `MoveToWorkspace { window, workspace }` | `control` | `MoveToWorkspace` | Window and workspace |
| `ToggleTiling` | `control` | `ToggleTiling` | Current workspace |
| `ToggleOverview` | `control` | `ToggleOverview` | — |
| `Notify { summary, body, app_id }` | `control` | `Notify` | — |
| `DismissNotification { id }` | `control` | `DismissNotification` | Notification |
| `Screenshot { path }` | `control` | `Screenshot` | Focused output |
| `Quit` | `session` | — | Session |

`Do` returns `Ok` after the command is queued, not after it is applied. Read
the next snapshot or journal entry to observe the result.
Window-targeted physical commands are reauthorized on the compositor thread.
If the human Realm is only an observer, focus, minimize, close, move,
geometry, and workspace mutations produce `Effect::Refused` and do not reach
the client. Its mirror also blocks physical hit-testing, so a refused click
cannot fall through to an unrelated window underneath.

## Realm Authority

A Realm is an interaction and presentation authority domain. Realm `1` is
the physical human desktop. Each agent Realm owns an independent `wl_seat`,
a directed virtual output, and private mount-scoped launch portals.

`GetRealms` returns one `RealmSnapshot` with:

- `revision`;
- principals and Realms;
- seats and their enabled state;
- connected Wayland clients and observed multi-seat support; and
- interaction groups, their controlling Realm, observing Realms, and windows.

`Realm { action }` is synchronous. It returns only after the compositor main
loop commits or rejects the operation and records that decision in the
mutation journal.

| Action | Operation class | Result |
|--------|-----------------|--------|
| `Create { label, capabilities, output }` | `CreateRealm` | `Created { bundle }` |
| `Transact { expected_revision, mutations }` | `TransactRealm` | `TransactionCommitted { receipt }` |
| `Revoke { realm, fallback, expected_revision }` | `RevokeRealm` | `Revoked { receipt }` |

A transaction contains 1–64 mutations and commits all or none:

| Mutation | Effect |
|----------|--------|
| `TransferWindow { window, target, retain_source_as_observer }` | Transfers the complete interaction group containing `window`. |
| `SetObserver { group, realm, observe }` | Adds or removes a read-only presentation Realm. |
| `ConfigureOutput { realm, output }` | Changes a virtual output. |
| `SetState { realm, state }` | Pauses or resumes a Realm. Permanent revocation is a separate action. |

Scope authorization expands `TransferWindow` and `SetObserver` to the
complete interaction group before commit. If any affected sibling window is
outside `scope.windows`, the whole action is refused; allowlisting one
toplevel cannot smuggle another toplevel on the same client connection.

`expected_revision` is optional on the wire. When present, a stale value
rejects the complete operation. The reference shell and CLI always supply
the revision they observed.

Virtual output dimensions are logical pixels. `scale_milli` is scale times
1,000 and `refresh_mhz` is millihertz. Width and height are each limited to
16,384, scale to 0.25–8.0, refresh to 1–1,000 Hz, and one physical RGBA frame
to 256 MiB.

`InjectRealmInput` uses target-window-local coordinates and the Realm's
independent seat. It never changes physical pointer or keyboard focus and
does not execute compositor shortcuts.

`LaunchInRealm` accepts an enumerated desktop-entry id. The compositor
launches it through a private mount-scoped Wayland listener and a fail-closed
Linux namespace sandbox. The randomized host socket path is removed and
pre-gate connections are dropped before application code runs. One sandbox
may open several Wayland connections without exposing a reusable host
pathname. Network and host filesystem access are denied unless
`[realm_sandbox]` policy explicitly grants them.

Every managed launch receives mandatory cgroup memory, process-count, and CPU
weight controls. Realm pause, session lock, and inactive VT freeze the
complete cgroup; revocation terminates and reaps it. Missing bubblewrap,
cgroup v2, controller delegation, or portal setup refuses the launch.

## Window Geometry

`SetWindowGeometry` uses compositor-global logical pixels. `rect.size.w` and
`rect.size.h` must each be between `1` and `32768`. The compositor:

- changes the window to floating layout;
- clears maximized and fullscreen state;
- clamps size to the client's minimum and maximum hints;
- preserves the requested origin; and
- exposes the resulting rectangle through `GetWindows`.

```json
{
  "type": "Do",
  "cmd": {
    "type": "SetWindowGeometry",
    "id": 7,
    "rect": {
      "origin": { "x": 120, "y": 80 },
      "size": { "w": 1280, "h": 720 }
    }
  }
}
```

## Synthetic Input

`InjectInput` requires a named scope that contains `InjectInput` and the target
window id. The operation must be listed explicitly: an omitted `ops` field does
not grant synthetic input. Coordinates are logical pixels relative to the
target window's top-left corner.

| Action | Fields | Effect |
|--------|--------|--------|
| `PointerMove` | `position` | Move the logical pointer. |
| `Click` | `position`, `button` | Move, press, and release. |
| `Scroll` | `position`, `dx`, `dy` | Move and deliver a smooth scroll. |
| `KeyPress` | `code` | Press and release one evdev key. |

Validation limits:

- `actions` contains 1–64 entries.
- Pointer positions must be inside the live, visible target and must hit that
  target rather than an overlapping window.
- Shell chrome must not cover any pointer position; keyboard-owning chrome
  rejects key input.
- Click buttons are Linux codes `0x110` through `0x117`.
- Key codes are at most `0x2ff`.
- Scroll deltas are finite and have an absolute value no greater than `1000`.
- Injected keys are refused while a physical modifier is held.
- All input is refused while a physical modifier, button grab, drag, or window
  move/resize is active.

```json
{
  "type": "Do",
  "cmd": {
    "type": "InjectInput",
    "id": 7,
    "actions": [
      {
        "type": "Click",
        "position": { "x": 40, "y": 32 },
        "button": 272
      }
    ]
  }
}
```

Input commands bypass compositor global key bindings. A live-state rejection
after queuing appears as `Effect::Refused` in the mutation journal.

## Capture

ass exposes pixel capture through two fail-closed operations that share one
same-frame presentation readback path (ADR-0037). Both are refused while the
session is locked or the seat is inactive. The request copies the exact frame
being submitted; later client commits, animations, or wallpaper frames cannot
change the detached snapshot. Captures include the overview grid while
overview mode is active.

`Command::Screenshot { path, region }` is a journaled `control` command that writes
the focused output as a PNG file; `ass-ctl screenshot` is its reference
frontend. `Request::CaptureOutput` is a synchronous query returning
`Response::CaptureOutput { width, height, png_bytes }` followed by one sealed
PNG `memfd` transferred with `SCM_RIGHTS`. The request requires the `control`
capability and an explicit `CaptureOutput` entry in the connection's scope
`ops`; like `InjectInput`, the operation is never inherited through the
unrestricted default.

The receiver must read the descriptor immediately after the JSON response,
check that its file length equals `png_bytes`, and require
`F_SEAL_SEAL | F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_WRITE`. Capture blobs are
limited to 288 MiB independently of the 16 MiB JSON frame limit.

Capture regions use compositor logical pixels. The returned PNG and
`width`/`height` use physical output pixels, so a region captured at 200%
scale has twice the logical width and height.

Continuous physical-output frame streaming (screencast,
`xdg-desktop-portal`) is future work and can reuse the same scale-aware
frame-copy path. Realm observers do not need to poll: `RealmDamaged` tells
them when a directed scene should be recaptured.

`CaptureRealm` reads only the selected Realm's directed virtual output. It
does not contain physical-desktop chrome, the cursor, or another Realm. The
response contains `capture` metadata with `realm`, physical `width` and
`height`, `scale_milli`, the logical `region`, `placements`, `png_bytes`, and
the authority `revision`, followed by the sealed PNG descriptor. Each
placement contains a window id, its rectangle in virtual-output logical
coordinates, and the target-local surface size. The metadata, scene, and
revision are one atomic observation, so an agent can map captured pixels to
`InjectRealmInput` coordinates without racing a layout change.

A security generation invalidates in-flight pixels across session-lock,
inactive-seat, pause, and revocation boundaries, including a lock followed by
a quick unlock. Encoding runs in a bounded worker. The compositor main thread
checks scope, lease, security generation, and Realm state before queuing the
result; the sole IPC writer checks the live scope, lock/VT gate, Realm state,
authority revision, and lease again immediately before it sends the sealed
descriptor.

## Named Scopes

Named scopes are configured with `[[agent.scope]]`. Every mutation and
capture resolves the named scope again, including final pixel delivery, so a
configuration reload can narrow or revoke authority without reconnecting. An
explicit unknown or removed scope fails closed.

`ass-ctl` uses the built-in owner-only `ass-ctl-realm-admin` scope for Realm
recovery commands. It grants the local user all Realm ids and the explicit
Realm operation set; it does not weaken the socket's mode `0600` boundary.

See the [Configuration Reference](config.md#agent-scopes) for fields and
operation names.
