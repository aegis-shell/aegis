# Settings Reference

Persistent settings live in the command panel, the compositor's modal
chrome surface. The `tessera-settings` crate is the settings module library:
the module contract, the module registry, and the built-in pages, rendered
in-process by the panel. There is no standalone settings application.

## Access

| Action | Result |
|--------|--------|
| Press `Super+S` | Open or close the command panel. |
| Four-finger touchpad swipe down | Open the command panel; swipe up closes it. |
| Select a module tab | Show that module's page in the main panel. |

The tab bar holds the **System** quick-controls tab plus one tab per
available settings module. Modules without a working backend keep their
registered route and metadata but render no tab. There are no
command-line deep links; external tools use the settings IPC instead.

## Modules

| Route | Category | Apply behavior | Backend |
|-------|----------|----------------|---------|
| `display` | Hardware | Explicit Apply button | Available; tessera output model and direct DRM backend |
| `input` | Hardware | Instant | Available; tessera input policy and libinput backend (keyboard repeat, mouse and touchpad speed/scrolling) |
| `appearance` | Personalization | Explicit Apply button | Available; Tessera desktop-preference authority and portal projection |
| `dock` | Personalization | Instant | Available; Tessera `[dock]` configuration authority |
| `power` | System | Explicit Apply button | Available; Tessera idle policy, session lock, output power, and logind sleep coordination |
| `users` | System | Explicit | Not available yet; AccountsService and authorization adapter required |
| `window-rules` | System | Explicit | Not available yet; revisioned window-rule settings backend required |

## Settings Transactions

The panel holds one revisioned settings snapshot, pushed by the
compositor on startup and after every commit. Display, input,
appearance, and idle-policy changes are submitted as typed actions with
the observed revision through the panel's in-process chrome channel; the
compositor main loop drains them into the same commit path the settings
IPC uses. A successful commit means the compositor validated, persisted,
applied, and journaled the change. A stale revision or backend failure
preserves the newer authoritative snapshot and displays an error.
Settings edits are refused while the session is locked.

Display, appearance, and power edits use explicit apply. The appearance
transaction contains the complete color scheme, optional accent, contrast,
reduced-motion state, fonts, text scale, icon theme, and cursor profile. The
power transaction contains the automatic-idle switch, ordered dim, lock,
display-off, and suspend timeouts, and dimmed brightness. Touchpad edits
apply immediately. Multiple unsent actions of one type are coalesced to the
newest value while a previous transaction is in flight.

External clients use the same transactions over the wire: see
[persistent settings](ipc.md#persistent-settings) in the IPC Reference for
the wire schema. Panel edits and IPC edits share one authority, one
revision counter, and one mutation journal.

## Non-settings Controls

| Surface | Access | Scope |
|---------|--------|-------|
| Command panel System tab | `Super+S`; also available over IPC | Live volume, brightness, Wi-Fi, Bluetooth, Do Not Disturb, and current-workspace layout |
| Agent Workspaces status row | The command panel's System tab | Display-only Interaction Domain authority status |

Neither the live controls nor the Agent Workspaces status row is a
settings module or writes persistent configuration. See
[live system controls](ipc.md#live-system-controls) in the IPC Reference
for their wire schema.

[ADR-0114](../adr/0114-panel-hosted-settings-and-hud-command-panel.md)
records the panel-hosted settings decision, the removal of the standalone
application, and the crash-containment trade-off against
[ADR-0049](../adr/0049-standalone-modular-control-center.md).
[ADR-0060](../adr/0060-statusbar-system-controls-and-live-system-ipc.md)
records the surviving split between live controls and persistent settings.
