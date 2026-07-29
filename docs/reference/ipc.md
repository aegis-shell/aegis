# IPC Reference

The aegis IPC is protocol version 10, carried as length-framed JSON over the
owner-only Unix socket at `$XDG_RUNTIME_DIR/aegis.sock`. Every connection starts
with `Hello`; commands are accepted only after capability and scope checks.
JSON messages are limited to 16 MiB. Large immutable capture and frame
payloads use a separate sealed-file-descriptor transfer described under
[Capture](#capture).

## Capabilities

| Capability | Authority | Default |
|------------|-----------|---------|
| `query` | Read snapshots and subscribe to events or the journal. | Always granted. |
| `control` | Mutate windows, workspaces, live-system state, layout, and notifications. | Server policy. |
| `input` | Inject bounded actions into a target window. | Named scope required. |
| `session` | Quit, persist compositor settings, or perform other session-level operations. | Server policy. |
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
| `GetSettings` | `Settings` with a revisioned snapshot | `query` |
| `GetSystemStatus` | `SystemStatus` with a live snapshot | `query` |
| `Realm { action }` | `Realm` with a commit receipt | `realm` + explicit scope op |
| `Settings { expected_revision, action }` | `SettingsApplied` with a commit receipt | `session` |
| `CaptureOutput` | `CaptureOutput` | `control` + explicit scope op |
| `CaptureRealm { realm, region }` | `CaptureRealm` | `realm` + `CaptureRealm` scope op |
| `StreamOutputStart { max_fps }` | `StreamOutputStarted` | `control` + `StreamOutput` scope op |
| `StreamOutputStop { stream_id }` | `StreamOutputStopped` | `control` + `StreamOutput` scope op |
| `SetIdleInhibit { inhibit }` | `IdleInhibitSet { inhibited }` | `control` + `IdleInhibit` scope op |

`Subscribe` enables coarse events:

- `WindowsChanged`, `WorkspaceChanged`, `RealmsChanged { revision }`,
  `SettingsChanged { revision }`, and `SystemStatusChanged`
  invalidate the corresponding snapshots.
- `SpaceUseChanged { state }` reports the strongest visible output-space
  consumer. `state` is `available`, `maximized`, or `fullscreen`; fullscreen
  has precedence over maximized.
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
- `Realm { action, before_revision, after_revision }`; or
- `Settings { action, before_revision, after_revision }`.

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
| `System { action }` | `control` | `SystemControl` | Live host or compositor-owned session state |
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

## Live System Controls

`GetSystemStatus` returns one normalized observation shared by status-bar
chrome and external IPC clients:

| Field | Type | Meaning |
|-------|------|---------|
| `volume` | optional percentage | Default audio-sink volume; absent when unavailable. |
| `muted` | boolean | Default audio-sink mute state. |
| `network` | `Offline`, `Wifi`, or `Wired` | Coarse active connectivity. |
| `battery` | optional `{ percent, charging }` | Battery state when a battery is present. |
| `wifi_enabled` | optional boolean | Wi-Fi radio state; absent when its service is unavailable. |
| `bluetooth_enabled` | optional boolean | Bluetooth radio state; absent when its service is unavailable. |
| `brightness` | optional percentage | Backlight level; absent without a controllable backlight. |
| `do_not_disturb` | boolean | Current notification suppression state. |
| `tiled` | boolean | Layout mode for the current workspace. |
| `touchpad`, `display` | status objects | Host-probe data shared with settings surfaces; persistent editors should use `GetSettings` for revisioned state. |

`System { action }` accepts one immediate action:

| Action | Payload | Bounds or effect |
|--------|---------|------------------|
| `ToggleMute` | — | Toggle the default audio sink. |
| `StepVolume` | `delta` | Signed percentage step from -100 through 100. |
| `SetVolume` | `level` | Percentage from 0 through 100. |
| `SetBrightness` | `level` | Percentage from 1 through 100. |
| `SetWifi` | `enabled` | Enable or disable the Wi-Fi radio. |
| `SetBluetooth` | `enabled` | Unblock or block Bluetooth radios. |
| `SetDoNotDisturb` | `enabled` | Change notification suppression. |
| `SetTiling` | `enabled` | Set the current workspace layout mode. |

The command requires `control`, a live privileged lease, and permission for
the `SystemControl` operation when a named scope restricts `ops`. The server
validates bounds before dispatch, starts host-service commands without
blocking the compositor, publishes an optimistic snapshot, and reconciles it
through the host status poller. `SystemStatusChanged` tells subscribers to
re-query; it carries no partial snapshot. These actions do not write the
revisioned compositor configuration.

## Persistent Settings

`GetSettings` returns one coherent snapshot:

| Field | Type | Meaning |
|-------|------|---------|
| `revision` | unsigned integer | Monotonic settings revision. |
| `touchpad` | `TouchpadStatus` | Effective profile, detected devices, capabilities, and configurability. |
| `display` | `DisplayStatus` | Connected outputs, advertised modes, configurability, and the last apply error. |
| `preferences` | `DesktopPreferences` | Complete effective desktop profile after configuration defaults and explicit startup overrides. |

`DesktopPreferences` contains:

| Field | Type | Bounds or values |
|-------|------|------------------|
| `color_scheme` | enum | `system`, `dark`, or `light` |
| `accent_color` | optional RGB object | `{ red, green, blue }`, each 0–255 |
| `contrast` | enum | `normal` or `high` |
| `reduced_motion` | boolean | Desktop and toolkit animation preference |
| `font_name`, `monospace_font_name` | string | Non-empty, at most 256 bytes |
| `text_scale` | float | 0.5–3.0 |
| `icon_theme`, `cursor_theme` | string | Non-empty, at most 256 bytes |
| `cursor_size` | unsigned integer | 8–128 logical pixels |

`Settings` submits one tagged action with an optional `expected_revision`:

| Action | Payload | Effect |
|--------|---------|--------|
| `SetTouchpad` | complete `TouchpadConfig` | Validate, persist `[input.touchpad]`, and apply the profile to live libinput devices. |
| `SetDisplay` | connector, mode, scale, position, and primary flag | Validate, atomically persist the output entry, and reconcile the live direct-DRM output. |
| `SetDesktopPreferences` | complete `DesktopPreferences` | Validate, atomically persist the `[appearance]` and preference-related `[ui]` fields, apply chrome and cursor policy, and refresh application icons. |

The operation requires `session` plus a live privileged lease. It is refused
while the session is locked. When `expected_revision` does not match the
current revision, the complete action is refused without changing state.

`SettingsApplied { receipt: { revision } }` is a confirmation, not a queue
acknowledgement. The server sends it only after the compositor main loop has
validated, persisted, and applied the action. A successful mutation increments
the revision, publishes the replacement snapshot, broadcasts
`SettingsChanged`, and records the action and before/after revisions in the
mutation journal.

Display, touchpad, and desktop appearance are settings domains in the current
snapshot. Mouse, keyboard, power, accounts, and window-rule modules remain
unavailable until their authoritative services expose typed state and actions.
See the [System Settings Reference](settings.md#modules) and
[ADR-0072](../adr/0072-desktop-preference-authority-and-toolkit-compatibility.md).

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

aegis exposes pixel capture through two fail-closed operations that share one
same-frame presentation readback path (ADR-0037). Both are refused while the
session is locked or the seat is inactive. The request copies the exact frame
being submitted; later client commits, animations, or wallpaper frames cannot
change the detached snapshot. Captures include the overview grid while
overview mode is active.

`Command::Screenshot { path, region }` is a journaled `control` command that writes
the focused output as a PNG file; `aegis-ctl screenshot` is its reference
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

Continuous physical-output frame streaming reuses the same scale-aware
frame-copy path; see [Streaming](#streaming). Realm observers do not need to
poll: `RealmDamaged` tells them when a directed scene should be recaptured.

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

## Streaming

`Request::StreamOutputStart { max_fps }` opens a continuous frame stream for
the focused physical output (ADR-0052). Authorization matches
`CaptureOutput`: the `control` capability plus an explicit `StreamOutput`
entry in the connection's scope `ops`, never inherited through the
unrestricted default. The reply `Response::StreamOutputStarted { stream_id,
width, height, format }` fixes the output geometry for the stream's lifetime.
`max_fps` throttles delivery; it defaults to 30 and is clamped to 1–60.

Each presented frame arrives as `Event::StreamFrame { stream_id, sequence,
width, height, stride, format, damage, dropped, byte_len }` followed
immediately by one sealed memfd of `byte_len` tightly packed pixels
(`height` rows of `stride` bytes), transferred with the same sealed-blob
rules as one-shot captures. `format` is `Bgra8` today; `damage` is
conservative and reports one full-frame rectangle in this version. `dropped`
is the cumulative count of frames lost to backpressure since the stream
started: delivery runs over a bounded two-frame lane per stream, and excess
frames are dropped rather than queued.

`Request::StreamOutputStop { stream_id }` ends a stream owned by the calling
connection and answers `Response::StreamOutputStopped`. The server ends a
stream with `Event::StreamEnded { stream_id, reason }` when the connection's
scope is revoked or narrowed, its lease expires, the output geometry
changes, or the compositor shuts down. Session lock and an inactive VT pause
delivery instead of ending the stream; resuming restarts it transparently.
Disconnecting the connection stops every stream it owned. Frame events,
lease-renewal replies, and end events interleave on the streaming
connection, so streaming clients read one continuous message stream instead
of one reply per request.

Frame readback currently goes through the same CPU path as one-shot
captures; a zero-copy dmabuf path (`flux_surface_export_dmabuf`) is future
work tracked in ADR-0052.

## Idle Inhibition

`Request::SetIdleInhibit { inhibit }` sets or clears the calling
connection's global, surfaceless idle inhibitor (ADR-0053), built for the
portal backend's Inhibit interface. Authorization matches `CaptureOutput`:
the `control` capability, a live lease, and an explicit `IdleInhibit` entry
in the connection's scope `ops`, never inherited through the unrestricted
default. While any connection holds an inhibitor, ext-idle-notify
notifications stay resumed, exactly as if a visible per-surface
`zwp_idle_inhibit_v1` inhibitor were active; a locked session suppresses
its effect the same way. The reply `Response::IdleInhibitSet { inhibited }`
confirms the state the connection now holds. The inhibitor is
connection-scoped: disconnecting releases it, so a crashed holder can never
keep the session out of idle.

## Named Scopes

Named scopes are configured with `[[agent.scope]]`. Every mutation and
capture resolves the named scope again, including final pixel delivery, so a
configuration reload can narrow or revoke authority without reconnecting. An
explicit unknown or removed scope fails closed.

`aegis-ctl` uses the built-in owner-only `aegis-ctl-realm-admin` scope for Realm
recovery commands. It grants the local user all Realm ids and the explicit
Realm operation set; it does not weaken the socket's mode `0600` boundary.

`aegis-portal` (the xdg-desktop-portal backend, ADR-0051) uses the built-in
owner-only `aegis-portal` scope, which grants exactly three operations —
`CaptureOutput` for the Screenshot portal, `StreamOutput` for the
ScreenCast portal (ADR-0052), and `IdleInhibit` for the Inhibit portal
(ADR-0053) — and nothing else. Both built-in
scopes follow the same fail-closed rule as configured scopes.

See the [Configuration Reference](config.md#agent-scopes) for fields and
operation names.
