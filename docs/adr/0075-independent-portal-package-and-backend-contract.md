# ADR-0075: Independent portal package and backend contract

- Status: Accepted
- Date: 2026-07-29

## Context

`aegis-portal` is an xdg-desktop-portal backend, not a user-facing command or
part of the compositor process. It adds PipeWire and portal-runtime
dependencies that the core compositor does not otherwise need. Its D-Bus
methods also implement the backend ABI, whose ownership rules differ from
the application-facing portal API: the frontend supplies exact Request and
Session paths and emits the public response signal after the backend returns
its `(response, results)` tuple.

The earlier portal work combined three concerns that need explicit
boundaries:

- packaging the optional backend and its activation metadata;
- implementing the exact backend request/session lifecycle; and
- deciding which interfaces Aegis can honestly advertise without a session
  manager or PermissionStore.

[ADR-0051](0051-portal-backend-dbus-bridge.md) established the process
boundary, and
[ADR-0053](0053-portal-session-services-and-grants.md) established scoped
idle inhibition and corrected the initial grant model. This decision
supersedes both with the complete package and ABI contract.

## Decision

**Ship a separate, version-locked package.** One source build produces
`aegis` and `aegis-portal`. The portal package depends on the exact matching
core version because its scoped IPC protocol and compositor mechanisms move
in lockstep. The core package treats the portal as optional and does not
acquire PipeWire or xdg-desktop-portal runtime dependencies solely for it.

The portal binary is a private D-Bus helper installed at
`/usr/lib/aegis/aegis-portal`. Its package owns that binary, the D-Bus
activation file, the `.portal` metadata, and the preferred-backend
configuration. Public Aegis commands remain under `/usr/bin`. The portal
crate is MIT-only because it neither embeds nor distributes the compositor's
GPL cursor artwork.

**Route interfaces explicitly.** GTK remains the default portal backend.
Aegis is selected only for Settings, Screenshot, ScreenCast, and Inhibit.
Adding metadata cannot silently make Aegis responsible for another
interface.

**Follow the backend ABI.** Screenshot and ScreenCast export Request objects
at the exact handles supplied by the frontend. ScreenCast likewise exports
Session objects at the supplied session handles. Blocking picker, capture,
and PipeWire work runs on dedicated workers; async zbus methods await a
bounded worker response and return `(response, results)`. Request objects are
removed when the method completes. Session objects remain until
`Session.Close` or the stream ends.

**Advertise only complete interface versions.**

- Settings v1 projects the compositor's effective desktop preferences and
  emits `SettingChanged`.
- Screenshot v2 supports focused-output capture, interactive region
  selection, and `PickColor`.
- ScreenCast v3 supports one interactively selected monitor or window and
  hidden-cursor mode.
- Inhibit accepts only the backend ABI's idle flag, value 8. Logout,
  user-switch, and suspend are rejected because Aegis has no session manager.

Aegis does not advertise Background. It also does not advertise ScreenCast
version 4 persistence until it has the compatible PermissionStore and policy
UI required for `restore_data`; backend-owned JSON tokens are not an
authorization substitute.

**Keep portal authority scoped.** The built-in `aegis-portal` IPC scope
grants exactly `CaptureOutput`, `StreamOutput`, `PickTarget`, and
`IdleInhibit`. It grants no general compositor control. Idle inhibition is
aggregated per live Request, periodically renewed to preserve the scoped IPC
lease, and released when the frontend closes the request or the portal
disconnects.

## Alternatives

- **Install the backend in the core package.** Rejected because it makes
  PipeWire and portal integration mandatory for users who need only the
  compositor.
- **Install `aegis-portal` in `/usr/bin`.** Rejected because it is a private
  activation helper rather than a stable user command.
- **Make Aegis the default backend with GTK fallback.** Rejected because
  metadata drift could route unsupported interfaces to Aegis.
- **Persist private ScreenCast tokens or always grant Background.** Rejected
  because neither represents user-confirmed authority that Aegis can enforce.
- **Perform capture and picker work on the D-Bus executor.** Rejected because
  a slow interactive or PipeWire operation would stall unrelated methods.

## Consequences

- Distributions produce an `aegis-portal` package from the same release as
  core Aegis and keep its D-Bus `Exec=` path synchronized with the private
  binary destination.
- Core Aegis remains usable without the portal package. Screenshot and screen
  sharing through xdg-desktop-portal require installing it.
- Unsupported interfaces continue through GTK. Background and persistent
  ScreenCast grants remain unavailable until their policy dependencies exist.
- Window ScreenCast shows the selected window's visible output region,
  including occlusion, and ends if the window closes or changes size.
- Future portal interface or persistence expansion requires updating the
  explicit routing, package dependencies, user documentation, and this
  authority decision together.
