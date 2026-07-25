# Control Center Reference

`aegis-ctl-center` is the standalone settings application. It connects to
the compositor at `$XDG_RUNTIME_DIR/aegis.sock` and opens a normal Wayland
window.

## Invocation

| Command | Result |
|---------|--------|
| `aegis-ctl-center` | Open the first available settings page. |
| `aegis-ctl-center display` | Open the display page. |
| `aegis-ctl-center --module touchpad` | Open the touchpad page. |
| `aegis-ctl-center --module=window-rules` | Open the window-rules page. |

An unknown module id falls back to the first registered page. The desktop
application id is `io.github.ming.ass.ControlCenter`; the packaged desktop
file is `io.github.ming.ass.ControlCenter.desktop`.

## Modules

| Route | Category | Apply behavior | Backend |
|-------|----------|----------------|---------|
| `display` | Hardware | Explicit Apply button | Available; ass output model and direct DRM backend |
| `mouse` | Hardware | Instant | Not available yet; libinput mouse policy required |
| `touchpad` | Hardware | Instant | Available; ass input policy and libinput backend |
| `keyboard` | Hardware | Explicit | Not available yet; keymap, repeat, compose, and shortcut backend required |
| `appearance` | Personalization | Explicit | Not available yet; UI policy and desktop theme service required |
| `power` | System | Instant | Not available yet; logind, UPower, and power-profile service adapters required |
| `users` | System | Explicit | Not available yet; AccountsService and authorization adapter required |
| `window-rules` | System | Explicit | Not available yet; revisioned window-rule settings backend required |

Unavailable modules expose their stable route, title, category, and search
metadata, but render no editable controls.

## Settings Transactions

The application loads one revisioned settings snapshot. Display and touchpad
changes are submitted as typed actions with the observed revision. A
successful response means the compositor main loop persisted and applied the
change. A stale revision or backend failure preserves the newer authoritative
snapshot and displays an error.

Display edits use explicit apply. Touchpad edits apply immediately. Multiple
unsent touchpad changes are coalesced to the newest value while a previous
transaction is in flight.

## Non-settings Controls

Quick Settings is not a Control Center module. Volume, brightness, Wi-Fi,
Bluetooth, notifications, and session actions remain live compositor chrome.
AI Workspace creation, transfer, pause, and revocation remain Realm
management operations.

See [IPC Reference](ipc.md#persistent-settings) for the wire schema and
[ADR-0049](../adr/0049-standalone-modular-control-center.md) for the
architecture decision.
