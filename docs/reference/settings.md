# System Settings Reference

`aegis-settings` is the standalone System Settings application. It connects to
the compositor at `$XDG_RUNTIME_DIR/aegis.sock` and opens a normal Wayland
window.

## Invocation

| Command | Result |
|---------|--------|
| `aegis-settings` | Open the first available settings page. |
| `aegis-settings display` | Open the display page. |
| `aegis-settings --module touchpad` | Open the touchpad page. |
| `aegis-settings --module appearance` | Open the appearance page. |
| `aegis-settings --module=window-rules` | Open the window-rules page. |

An unknown module id falls back to the first registered page. The desktop
application id is `io.github.ming2k.aegis.Settings`; the packaged desktop file
is `io.github.ming2k.aegis.Settings.desktop`.

## Application Installation

| Artifact | Path relative to the installation prefix |
|----------|------------------------------------------|
| Executable | `bin/aegis-settings` |
| Desktop entry | `share/applications/io.github.ming2k.aegis.Settings.desktop` |
| Icon | `share/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg` |

The executable, desktop entry, and icon are one packaging unit. The launcher
discovers System Settings through the installed XDG metadata.

## Modules

| Route | Category | Apply behavior | Backend |
|-------|----------|----------------|---------|
| `display` | Hardware | Explicit Apply button | Available; aegis output model and direct DRM backend |
| `mouse` | Hardware | Instant | Not available yet; libinput mouse policy required |
| `touchpad` | Hardware | Instant | Available; aegis input policy and libinput backend |
| `keyboard` | Hardware | Explicit | Not available yet; keymap, repeat, compose, and shortcut backend required |
| `appearance` | Personalization | Explicit Apply button | Available; Aegis desktop-preference authority and portal projection |
| `power` | System | Instant | Not available yet; logind, UPower, and power-profile service adapters required |
| `users` | System | Explicit | Not available yet; AccountsService and authorization adapter required |
| `window-rules` | System | Explicit | Not available yet; revisioned window-rule settings backend required |

Unavailable modules expose their stable route, title, category, and search
metadata, but render no editable controls.

## Settings Transactions

The application loads one revisioned settings snapshot. Display, touchpad,
and appearance changes are submitted as typed actions with the observed
revision. A successful response means the compositor main loop persisted and
applied the change. A stale revision or backend failure preserves the newer
authoritative snapshot and displays an error.

Display and appearance edits use explicit apply. The appearance transaction
contains the complete color scheme, optional accent, contrast, reduced-motion
state, fonts, text scale, icon theme, and cursor profile. Touchpad edits apply
immediately. Multiple unsent actions of one type are coalesced to the newest
value while a previous transaction is in flight.

## Non-settings Controls

| Surface | Built-in identity | Access | Scope |
|---------|-------------------|--------|-------|
| Status bar system controls | None; part of compositor chrome | Audio, network, or notification controls in the status bar; also available over IPC | Live volume, brightness, Wi-Fi, Bluetooth, Do Not Disturb, and current-workspace layout |
| AI Workspaces | `BuiltInApplication::AiWorkspaces` | Launcher or the status bar Fuji indicator | Realm creation, pause, resume, revocation, and authority status |

Neither the live controls nor AI Workspaces are System Settings modules or
write persistent configuration.

See [persistent settings](ipc.md#persistent-settings) and
[live system controls](ipc.md#live-system-controls) in the IPC Reference for
their wire schemas,
[ADR-0056](../adr/0056-system-settings-identity-and-boundary.md) for the
application boundary, and
[ADR-0069](../adr/0069-documentation-owned-installation-and-throwaway-development-staging.md)
for the canonical namespace and installation model. The separation between
status-bar controls, IPC, and independent application surfaces is recorded in
[ADR-0060](../adr/0060-statusbar-system-controls-and-live-system-ipc.md).
