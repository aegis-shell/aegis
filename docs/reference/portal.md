# Portal Backend Reference

`xdg-desktop-portal-atrium` is the private xdg-desktop-portal backend for an
Tessera session. It is built from the independent
[xdg-desktop-portal-atrium repository](https://github.com/aegis-shell/xdg-desktop-portal-atrium),
distributed separately from the compositor, and activated on the session
D-Bus as needed.

## Identifiers and Paths

| Item | Value |
|------|-------|
| Source repository | `https://github.com/aegis-shell/xdg-desktop-portal-atrium` |
| Release compatibility | Portal `v0.0.10` supports Tessera `v0.0.22` and its IPC protocol 27; the Portal compatibility reference remains authoritative |
| Distribution package | `xdg-desktop-portal-atrium` |
| Backend executable | `/usr/libexec/xdg-desktop-portal-atrium` by default; distributions may configure `libexecdir` |
| FileChooser prompter | `/usr/libexec/atrium-portal-prompter` by default; distributions may configure `libexecdir` |
| Backend bus name | `org.freedesktop.impl.portal.desktop.atrium` |
| Public frontend bus name | `org.freedesktop.portal.Desktop` |
| Backend and frontend object path | `/org/freedesktop/portal/desktop` |
| Compositor IPC socket | `$XDG_RUNTIME_DIR/tessera.sock` |
| Backend metadata | `/usr/share/xdg-desktop-portal/portals/atrium.portal` |
| Backend selection | `/usr/share/xdg-desktop-portal/atrium-portals.conf` |
| D-Bus activation | `/usr/share/dbus-1/services/org.freedesktop.impl.portal.desktop.atrium.service` |
| Screenshot cache | `$XDG_CACHE_HOME/xdg-desktop-portal-atrium/`, falling back to `$XDG_RUNTIME_DIR/xdg-desktop-portal-atrium/` |
| Secret vault | `$XDG_DATA_HOME/tessera/secrets/` |

Per-user discovery paths are
`$XDG_DATA_HOME/xdg-desktop-portal/portals/`,
`$XDG_CONFIG_HOME/xdg-desktop-portal/`, and
`$XDG_DATA_HOME/dbus-1/services/`.

The backend-selection file uses `default=tessera;gtk`. Tessera is tried first
for every interface that its `.portal` metadata advertises; GTK supplies
unadvertised interfaces and configured fallbacks.

## Runtime Dependencies

| Feature | Required runtime |
|---------|------------------|
| Public portal API | Session D-Bus and `xdg-desktop-portal` |
| Tessera selection | `XDG_CURRENT_DESKTOP=tessera` in the activation environment |
| Compositor-backed operations | Matching Tessera compositor and `$XDG_RUNTIME_DIR/tessera.sock` |
| ScreenCast | PipeWire session |
| FileChooser | GTK4 and Tessera xdg-foreign-v2 support |
| Unsupported interface fallback | `xdg-desktop-portal-gtk` |
| Email | `xdg-email`, or the command selected by `TESSERA_PORTAL_MAILER` |
| Password-vault auto-unlock | Optional `pam_sigil.so` integration |

## Interface Coverage

| Backend interface | Version | Behavior and current limit |
|-------------------|---------|----------------------------|
| `org.freedesktop.impl.portal.Settings` | 1 | Exposes standardized appearance keys and a curated `org.gnome.desktop.interface` projection from the compositor's effective settings snapshot. The public frontend supplies Settings v2 and `ReadOne`. |
| `org.freedesktop.impl.portal.Screenshot` | 3 | Supports area selection, color picking, and consent-checked legacy output capture. |
| `org.freedesktop.impl.portal.ScreenCast` | 6 | Supports one monitor stream, hidden cursor mode, and stable `pipewire-serial` values. |
| `org.freedesktop.impl.portal.Secret` | 1 | Returns an HKDF-derived application secret from the encrypted Tessera vault. The raw vault master key does not cross D-Bus. |
| `org.freedesktop.impl.portal.Lockdown` | current seven-property ABI | Exposes all seven read-write, process-resident properties. |
| `org.freedesktop.impl.portal.FileChooser` | current backend ABI | Handles open, save, directory, and multiple-file flows in a supervised one-shot GTK4 process. No path crosses compositor IPC. |
| `org.freedesktop.impl.portal.Email` | current backend ABI | Validates attachment URIs and hands compose requests to the session mail client through `xdg-email`. |
| `org.freedesktop.impl.portal.Account` | current backend ABI | Returns the account name and optional avatar after explicit compositor confirmation. |

Tessera does not advertise any interface outside this table. Inhibit,
AppChooser, Notification, DynamicLauncher, and Wallpaper route to GTK because
the Tessera implementations do not cover their complete portal contracts. The
GTK backend also supplies other unadvertised interfaces, including Access,
Background, Print, and Location.

## Lock and Seat Behavior

| Operation | Locked or inactive behavior |
|-----------|-----------------------------|
| Screenshot and color picking | Refused with a cancelled/failed portal response. |
| ScreenCast | Existing streams pause and resume after the session becomes active and unlocked. |
| File selection | The prompter is an ordinary transient Wayland client below the lock surface; `Request.Close` terminates it. |
| Account consent | Compositor IPC authorization fails closed while the lock or VT gate is active. |
| Settings | The last effective settings snapshot remains readable. |
| Secret vault storage | The vault remains encrypted at rest; lock-screen policy does not currently re-lock an already unlocked vault. |

## Persistent Secret Storage

The portal-native Secret interface stores one encrypted vault under
`$XDG_DATA_HOME/tessera/secrets/`. The backend rejects symlinks, unsafe owners
or modes, oversized files, corrupt vaults, and orphan ciphertext. The
optional `pam_sigil.so` module can unlock the vault automatically
unlock. The Portal does not implement the separate
`org.freedesktop.secrets` API; distributions must provide a complete keyring
service for un-sandboxed Secret Service clients.

See [How to Install and Verify the Portal Backend](../how-to/portals.md) for
installation and end-to-end checks.
