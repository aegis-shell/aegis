# ADR-0060: Status bar system controls and live-system IPC

- Status: Accepted
- Date: 2026-07-26

## Context

[ADR-0058](0058-independent-quick-settings-and-ai-workspaces.md) gave Quick
Settings an independent compositor-owned application surface after the
Control Center was removed. The status bar already presented the same live
audio, network, battery, and notification state, so the additional launcher
entry and modal surface duplicated the product boundary without creating a
separate authority.

Immediate controls and persistent settings have different state and failure
models. That distinction requires separate typed protocols, but it does not
require separate applications. The status bar is always present and already
receives the normalized live-system snapshot. External clients also need a
UI-independent way to observe and request the same controls.

## Decision

The independent Quick Settings product, crate, built-in application identity,
and launcher entry are retired. The status bar owns the compact live-system
control surface for volume, mute, brightness, Wi-Fi, Bluetooth, Do Not
Disturb, and current-workspace layout.

The backend-neutral `SystemStatus` and `SystemAction` types live in
`aegis-core`. Status bar intents and IPC commands converge on one compositor
runtime handler. The in-process status bar emits typed actions directly; it
does not connect back to the compositor's own Unix socket.

IPC protocol version 7 adds:

- `GetSystemStatus` and the `SystemStatus` response;
- `Command::System { action }`;
- `SystemStatusChanged`; and
- the `SystemControl` operation class for named scopes.

System controls remain immediate and service-owned. They do not enter the
revisioned persistent-settings snapshot. The standalone System Settings
application remains the canonical editor for persistent display and input
configuration. A future System Settings page may consume the live-system IPC
without becoming the authority for those services.

AI Workspaces remains an independent compositor-owned application because it
manages Realm lifecycle and interaction authority rather than system status.

## Alternatives

- **Keep the independent Quick Settings application.** Rejected because it
  duplicates status already presented by the status bar and adds another
  launcher identity for controls that do not need a separate lifecycle.
- **Make the status bar use IPC internally.** Rejected because both endpoints
  are in the compositor process. Serializing and scheduling a loopback request
  would add failure modes without strengthening the authority boundary.
- **Move immediate controls only into System Settings.** Rejected because the
  normal Wayland application may be closed, unavailable, or still starting.
  The always-present status bar remains the appropriate quick-access surface.
- **Add live controls to the persistent settings transaction.** Rejected
  because host services, not the compositor configuration, own their source
  of truth and do not share revisioned persistence semantics.

## Consequences

- One status-bar panel presents status, immediate controls, and recent
  notifications.
- External tools can observe and request the same live controls without
  depending on a UI crate.
- System-control requests use the existing capability, lease, scope,
  validation, and mutation-journal path.
- The compositor keeps host probes and command execution off the render
  thread and reconciles optimistic values through the status poller.
- Quick Settings compatibility aliases are not provided because no tagged
  release exposed its identity.
