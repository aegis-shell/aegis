# Portal Backend Reference

`xdg-desktop-portal-aegis` is the private xdg-desktop-portal backend for an
Aegis session. It is built from the independent
[xdg-desktop-portal-aegis repository](https://github.com/aegis-shell/xdg-desktop-portal-aegis),
distributed separately from the compositor, and activated on the session
D-Bus as needed.

## Identifiers and Paths

| Item | Value |
|------|-------|
| Source repository | `https://github.com/aegis-shell/xdg-desktop-portal-aegis` |
| Release compatibility | Portal `v0.0.1` supports Aegis `v0.0.9`; the Portal manifest is authoritative |
| Distribution package | `xdg-desktop-portal-aegis` |
| Backend executable | `/usr/lib/xdg-desktop-portal-aegis` |
| Backend bus name | `org.freedesktop.impl.portal.desktop.aegis` |
| Public frontend bus name | `org.freedesktop.portal.Desktop` |
| Backend and frontend object path | `/org/freedesktop/portal/desktop` |
| Compositor IPC socket | `$XDG_RUNTIME_DIR/aegis.sock` |
| Backend metadata | `/usr/share/xdg-desktop-portal/portals/aegis.portal` |
| Backend selection | `/usr/share/xdg-desktop-portal/aegis-portals.conf` |
| D-Bus activation | `/usr/share/dbus-1/services/org.freedesktop.impl.portal.desktop.aegis.service` |
| Secret Service compatibility activation | `/usr/share/dbus-1/services/org.freedesktop.secrets.service` |
| Screenshot cache | `$XDG_CACHE_HOME/xdg-desktop-portal-aegis/`, falling back to `$XDG_RUNTIME_DIR/xdg-desktop-portal-aegis/` |
| Secret vault | `$XDG_DATA_HOME/aegis/secrets/` |

Per-user discovery paths are
`$XDG_DATA_HOME/xdg-desktop-portal/portals/`,
`$XDG_CONFIG_HOME/xdg-desktop-portal/`, and
`$XDG_DATA_HOME/dbus-1/services/`.

The backend-selection file uses `default=aegis;gtk`. Aegis is tried first
for every interface that its `.portal` metadata advertises; GTK supplies
unadvertised interfaces and configured fallbacks.

## Runtime Dependencies

| Feature | Required runtime |
|---------|------------------|
| Public portal API | Session D-Bus and `xdg-desktop-portal` |
| Aegis selection | `XDG_CURRENT_DESKTOP=aegis` in the activation environment |
| Compositor-backed operations | Matching Aegis compositor and `$XDG_RUNTIME_DIR/aegis.sock` |
| ScreenCast | PipeWire session |
| Unsupported interface fallback | `xdg-desktop-portal-gtk` |
| Email | `xdg-email`, or the command selected by `AEGIS_PORTAL_MAILER` |
| Password-vault auto-unlock | Optional `pam_aegis.so` integration |

## Interface Coverage

| Backend interface | Version | Behavior and current limit |
|-------------------|---------|----------------------------|
| `org.freedesktop.impl.portal.Settings` | 1 | Exposes standardized appearance keys and a curated `org.gnome.desktop.interface` projection from the compositor's effective settings snapshot. The public frontend supplies Settings v2 and `ReadOne`. |
| `org.freedesktop.impl.portal.Screenshot` | 2 | Supports focused-output capture, interactive region selection, and `PickColor`. Interactive operations use compositor chrome and accept Escape to cancel. |
| `org.freedesktop.impl.portal.ScreenCast` | 3 | Supports one interactively selected monitor or window and hidden-cursor mode. It does not advertise version 4 persistence or `restore_data`. Window streams include visible occlusion. |
| `org.freedesktop.impl.portal.Inhibit` | implementation ABI | Accepts the idle flag `8`. Logout, user-switch, and suspend flags `1`, `2`, and `4` are rejected. The request-scoped lease is released when the request closes or its connection disappears. |
| `org.freedesktop.impl.portal.Secret` | 1 | Returns an HKDF-derived application secret from the encrypted Aegis vault. The raw vault master key does not cross D-Bus. |
| `org.freedesktop.impl.portal.Lockdown` | implementation ABI | Returns `false` for every restriction. Aegis does not currently provide a kiosk policy engine. |
| `org.freedesktop.impl.portal.FileChooser` | 3 | Supports `OpenFile`, `SaveFile`, and `SaveFiles` through native compositor chrome, including navigation, multi-select, filters, and save filenames. GTK remains a fallback. |
| `org.freedesktop.impl.portal.AppChooser` | 2 | Uses the native application picker and honors `last_choice`. GTK remains a fallback. |
| `org.freedesktop.impl.portal.Email` | 2 | Hands compose requests and staged attachments to the session mail client through `xdg-email`. GTK remains a fallback. |
| `org.freedesktop.impl.portal.Notification` | 2 | Adds and removes baseline title/body notifications in the compositor queue. Icons and action buttons are not supported. |
| `org.freedesktop.impl.portal.Account` | 1 | Returns `getpwuid` identity and the canonical Aegis avatar only after a compositor consent dialog. GTK remains a fallback. |
| `org.freedesktop.impl.portal.DynamicLauncher` | 1 | `PrepareInstall` requires compositor consent. `RequestInstallToken` is always refused. GTK remains a fallback. |
| `org.freedesktop.impl.portal.Wallpaper` | 1 | Accepts local `file://` URIs after compositor consent and applies decodable still images live. glTF wallpaper remains startup-only. GTK remains a fallback. |

Aegis does not advertise any interface outside this table. The GTK backend
supplies the interfaces it supports, including Access, Background, Print, and
Location, when installed.

## Lock and Seat Behavior

| Operation | Locked or inactive behavior |
|-----------|-----------------------------|
| Screenshot and color picking | Refused with a cancelled/failed portal response. |
| ScreenCast | Existing streams pause and resume after the session becomes active and unlocked. |
| File, application, account, launcher, secret, and wallpaper consent | Compositor IPC authorization fails closed while the lock or VT gate is active. |
| Settings | The last effective settings snapshot remains readable. |
| Secret vault storage | The vault remains encrypted at rest; lock-screen policy does not currently re-lock an already unlocked vault. |

## Secret Service Compatibility

The same process also implements the classic `org.freedesktop.secrets` API
for un-sandboxed libsecret clients. It requests that well-known name with
`DoNotQueue`. When another provider already owns the name, Aegis logs a
warning and leaves the existing provider in place; the portal-native Secret
interface remains available.

First use creates a mode-0600 keyfile vault that unlocks automatically.
Password-mode vaults unlock through a masked compositor prompt. The optional
`pam_aegis.so` module can instead supply a short-lived, mode-0600 token after
successful login or screen unlock; the portal consumes and deletes it.

See [How to Install and Verify the Portal Backend](../how-to/portals.md) for
installation and end-to-end checks.
