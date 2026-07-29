# ADR-0053: Portal backend ABI ownership and scoped session services

- Status: Superseded by ADR-0075
- Date: 2026-07-29

## Context

[ADR-0051](0051-portal-backend-dbus-bridge.md) established
`aegis-portal` as an xdg-desktop-portal backend. The backend ABI is not the
same as the application-facing portal API: the frontend supplies exact
Request and Session object paths, backend methods return
`(response, results)` tuples, and the frontend owns the public response
signal.

The earlier draft for this decision mixed those responsibilities. It
derived request paths, emitted frontend-style response signals, advertised
Background without an enforceable background policy, and treated private
JSON files as ScreenCast authorization. That shape was not a faithful
backend contract:

- Aegis has no session manager, running-application policy, or
  PermissionStore.
- ScreenCast persistence belongs to the version 4 `restore_data`
  PermissionStore contract, not to an opaque backend-owned token.
- The portal backend owns no Wayland surface. Idle inhibition must continue
  to cross the compositor's scoped IPC boundary.
- D-Bus activation needs one stable private executable path that is not
  presented as a user command.

## Decision

**Follow the backend request/session ABI.** Screenshot and ScreenCast accept
the exact Request and Session paths supplied by the portal frontend.
Request objects exist while their method is in flight, support cancellation
through `Request.Close`, and are removed before the backend method returns
its `(response, results)` tuple. Session objects remain until
`Session.Close` or the compositor ends the stream. Blocking capture,
selection, and PipeWire work stays on dedicated workers while zbus methods
await one bounded response channel.

**Advertise only implemented versions.** Screenshot serves version 2 with
interactive region selection and `PickColor`. ScreenCast serves version 3
with monitor and window source types and hidden-cursor mode. Every source
selection uses the compositor picker. Version 4 persistence is deferred
until Aegis has a compatible PermissionStore and can implement
`restore_data` without inventing a second authority model.

**Do not advertise Background.** Aegis cannot enforce background execution
or provide a trustworthy autostart grant, so `aegis.portal` omits the
interface. The preferred-portal configuration keeps GTK as the default and
routes only Settings, Screenshot, ScreenCast, and Inhibit to Aegis.

**Keep Inhibit request-scoped and fail closed.** The backend accepts only
the freedesktop backend ABI's idle bit, flag 8. Logout, user-switch, and
suspend flags are rejected because Aegis has no session manager. Each
accepted call exports its supplied Request path until `Request.Close`; the
worker aggregates live requests, periodically renews the scoped IPC lease,
and reconnects after a compositor restart.

The IPC operation remains connection-scoped:
`Request::SetIdleInhibit { inhibit }` produces
`Response::IdleInhibitSet { inhibited }`. The compositor folds all live IPC
connections into one surfaceless inhibitor and releases a connection's
contribution when it disconnects.

**Install the backend as a private helper.** Distribution packages place
the binary at `/usr/lib/aegis/aegis-portal`, matching the D-Bus activation
file. User-facing Aegis commands remain under `/usr/bin`.

## Alternatives

- **Continue backend-owned grant files.** Rejected because the files are not
  the portal PermissionStore ABI and cannot establish user-confirmed
  authority.
- **Advertise Background and always grant it.** Rejected because a positive
  response would claim a policy Aegis cannot enforce.
- **Give the portal a Wayland surface for idle inhibition.** Rejected because
  it would widen the backend's authority past the scoped IPC boundary.
- **Run blocking portal operations on the D-Bus executor.** Rejected because
  picker, frame capture, and PipeWire negotiation can stall unrelated portal
  methods.

## Consequences

- The backend serves Settings v1, Screenshot v2, ScreenCast v3, and Inhibit
  v1. Unsupported interfaces continue through the configured GTK backend.
- Background decision files and ScreenCast token files are removed. A future
  persistence implementation requires a compatible PermissionStore and a
  new accepted decision.
- ScreenCast sessions support one interactively selected monitor or window.
  Window streams show the selected window's visible output region, including
  occlusion.
- Idle inhibition survives compositor restarts through periodic renewal and
  ends when the frontend closes the request or the portal disconnects.
- Packages must keep the portal binary and D-Bus `Exec=` path synchronized
  at `/usr/lib/aegis/aegis-portal`.
