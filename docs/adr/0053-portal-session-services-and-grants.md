# ADR-0053: Portal session services, connection-scoped idle inhibition, and portal-owned grants

- Status: Proposed
- Date: 2026-07-25

## Context

[ADR-0051](0051-portal-backend-dbus-bridge.md) Phase 3 asks the portal
backend for the session-level interfaces sandboxed applications expect:
`Background` (run in background, autostart), `Inhibit` (hold off idle and
session events), and ScreenCast persistence (`persist_mode` /
`restore_token`). Three forces shape each:

- aegis has no session manager and no running-application tracking, so the
  logout / user-switch / suspend Inhibit flags have nothing to land on, and
  "run in the background" has no enforcement mechanism behind it.
- aegis has no PermissionStore and no portal confirmation UI. Persisted
  grants therefore have no canonical home, and a restore token cannot be a
  shortcut past a dialog — there is no dialog.
- The idle machinery the Inhibit portal must drive lives in the
  compositor's Wayland server (`ext-idle-notify-v1`, gated by per-surface
  `zwp_idle_inhibit_v1` objects). The portal backend owns no Wayland
  surface, and per
  [ADR-0037](0037-scoped-pixel-capture-over-ipc.md)-era boundaries it must
  not gain Wayland privileges; its only channel inward is the scoped IPC.

## Decision

**Inhibit: one new fail-closed IPC operation.** The IPC gains
`Request::SetIdleInhibit { inhibit }` → `Response::IdleInhibitSet
{ inhibited }`, a connection-scoped, surfaceless global idle inhibitor.
Authorization mirrors `StreamOutput` exactly: `control` capability, live
lease, and an explicit `IdleInhibit` op in the connection's named scope —
never inherited through `None`-means-all. The built-in `aegis-portal` scope
gains that one op. The compositor main loop keeps per-connection entries
and folds them into a single flag on the Wayland server, where it counts
like an active per-surface inhibitor; disconnecting a connection releases
its inhibitor fail-closed, exactly like its output streams. The protocol
stays at version 5: the addition is one internally tagged variant pair,
backward-compatible for peers that recompile.

The portal serves `org.freedesktop.impl.portal.Inhibit` v1 with flag 4
(idle) only. Flags 1/2/8 are logged and ignored — they name session-manager
features that do not exist. Because v1 has no uninhibit call, per-app
counts are released when the caller's bus name vanishes (polled via
`NameHasOwner`), and portal shutdown releases the compositor-side inhibitor
through the IPC disconnect path.

**Portal-owned grant persistence.** The backend persists its own
authorization state as JSON documents under `$XDG_DATA_HOME/aegis-portal`,
written mode `0600` via create-temp-then-rename (the capture cache's
discipline). Two documents: `background.json` (per-app_id Background
decisions) and `screencast-tokens.json` (mode 2 restore tokens). Corrupt or
missing documents read as empty; a missing state directory degrades a
feature to memory-only, never to a failed request.

**Background: grant-and-record.** `org.freedesktop.impl.portal.Background`
v1 grants requested `background = true` by default and records the
decision; repeated requests from the same app_id answer from the record.
`autostart = true` is materialized by copying the application's desktop
file into `$XDG_CONFIG_HOME/autostart/` (the freedesktop convention both
GNOME and KDE follow) and reported `false` when no source desktop file
exists; `autostart = false` removes the file.

**ScreenCast v2: the token is the credential.** The ScreenCast interface
serves version 2. `persist_mode` 1 keeps a token in memory (with no
application-exit tracking, "until the application exits" degrades to "until
the portal restarts"); mode 2 persists it. A presented token that validates
restores the session with no further check — the unguessable
(`/dev/urandom`) token *is* the authorization credential, since there is
no confirmation UI to skip. An unknown token is treated as no token and a
fresh one is minted. `Start` returns the session's `restore_token` in its
results.

**Settings: mtime watcher for `SettingChanged`.** Appearance lives in the
config file, not the revisioned IPC settings snapshot, so there is no IPC
event to subscribe to. A two-second mtime poll of the configuration file
emits `SettingChanged` when the mapped color-scheme value changes. No new
dependency (`notify`) is pulled for one file.

## Alternatives

- **A per-surface `zwp_idle_inhibit_v1` object created by the portal
  backend.** Rejected: the backend owns no surface, and giving it a
  Wayland connection with capture-adjacent privileges breaks the
  ADR-0051 boundary — the scoped IPC is its only inward channel.
- **Idle inhibit as a journaled `Command`.** Rejected: commands are
  fire-and-forget with no connection affinity, and the inhibitor must be
  released when the owning connection dies — a request/response with
  server-side connection tracking, the `StreamOutput` shape, gives both
  confirmation and lifecycle for free.
- **Implementing the PermissionStore interfaces.** Deferred, per the phase
  scope: portal frontends only require the backend to return a restore
  token. Portal-owned JSON documents answer the same persistence question
  with one file per grant family and no new bus surface; a PermissionStore
  can later read the same documents.
- **Deny-by-default Background without a permission UI.** Rejected: with
  no dialog and no permission store, denial would make the interface
  uniformly useless while looking supported. Grant-and-record keeps the
  answer honest (the grant is recorded and reversible) and matches what
  the compositor can actually enforce today: nothing.
- **An async signal subscription (`NameOwnerChanged`) for gone
  applications.** Rejected for this phase: it would pull zbus's async
  signal stream into a blocking-connection process for one signal. A
  two-second `NameHasOwner` poll is adequate for releasing an idle
  inhibit.

## Consequences

- `aegis-ipc` gains `OpClass::IdleInhibit`, the `SetIdleInhibit` request
  pair, and disconnect-time release; `aegis-compositor` gains one surfaceless
  inhibitor flag folded into `update_idle_notifications`; the compositor
  main loop gains a small per-connection registry drained each iteration.
  The built-in `aegis-portal` scope now grants exactly three operations.
- The backend serves five interfaces: Settings v1 (now emitting
  `SettingChanged`), Screenshot v1, ScreenCast v2, Background v1, Inhibit
  v1; `aegis.portal` lists Background and Inhibit.
- `$XDG_DATA_HOME/aegis-portal` becomes session state the user may want to
  clear to revoke grants; revocation is documented in the portal how-to.
  Deleting `screencast-tokens.json` revokes every persisted cast grant.
- Follow-up work this decision creates: a PermissionStore or control-center
  surface to list and revoke recorded grants; ScreenCast persist mode 1
  tracking actual application exit; `QueryEndResponse` emission if a
  session manager ever lands.
