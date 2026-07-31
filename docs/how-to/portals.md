# How to Install and Verify the Portal Backend

`aegis-portal` is the independently packaged xdg-desktop-portal backend for
Aegis
([ADR-0075](../adr/0075-independent-portal-package-and-backend-contract.md)).
It lets portal-aware and sandboxed (Flatpak) applications read the desktop
appearance preference, take screenshots, cast the screen, choose files and
applications with native dialogs, post notifications, and retrieve secrets.
This guide installs the integration files and verifies the backend end to
end.

Install the `aegis-portal` system package in addition to the core `aegis`
package. The backend is D-Bus-activated on demand; there is no service to
enable or start.

## Install

Install the binary, its license, and the files shipped under `contrib/`:

| Source | Install to |
|--------|-----------|
| `target/release/aegis-portal` | `/usr/lib/aegis/aegis-portal` |
| `contrib/xdg-desktop-portal/portals/aegis.portal` | `/usr/share/xdg-desktop-portal/portals/aegis.portal` |
| `contrib/xdg-desktop-portal/aegis-portals.conf` | `/usr/share/xdg-desktop-portal/aegis-portals.conf` |
| `contrib/dbus-1/services/org.freedesktop.impl.portal.desktop.aegis.service` | `/usr/share/dbus-1/services/org.freedesktop.impl.portal.desktop.aegis.service` |
| `contrib/dbus-1/services/org.freedesktop.secrets.service` | `/usr/share/dbus-1/services/org.freedesktop.secrets.service` |
| `LICENSE` | `/usr/share/licenses/aegis-portal/LICENSE` |

For a per-user development install, place the integration files under
`~/.local/share/xdg-desktop-portal/portals/`,
`~/.config/xdg-desktop-portal/aegis-portals.conf`, and
`~/.local/share/dbus-1/services/` respectively. Point `Exec=` at the
development binary; the system package remains the supported installation.

The `org.freedesktop.secrets.service` file is transitional: it activates
the same `aegis-portal` process for the classic Secret Service API until
portal-native secret retrieval is universal
([ADR-0085](../adr/0085-portal-secret-absorption-and-secret-service-compat.md)).

`aegis-portals.conf` defaults to `aegis;gtk`: Aegis is the default backend
for every interface it advertises, and GTK is the safety net for the rest
(Access, Print, Location, Background, …). The frontend only asks Aegis for
the interfaces listed in `aegis.portal`. Aegis currently serves:

- `org.freedesktop.impl.portal.Settings` v1 — the standardized
  `org.freedesktop.appearance` keys `color-scheme`, `accent-color`,
  `contrast`, and `reduced-motion`, plus a curated
  `org.gnome.desktop.interface` projection for GTK color, fonts, text scale,
  icons, cursors, and animation policy. The backend reads the compositor's
  effective IPC snapshot and subscribes to `SettingsChanged`; it does not
  read TOML, GSettings, dconf, or KDE configuration. See the
  [Configuration Reference](../reference/config.md#appearance) and
  [ADR-0072](../adr/0072-desktop-preference-authority-and-toolkit-compatibility.md).
  The backend interface remains v1; the public
  `org.freedesktop.portal.Settings` frontend is v2 and provides `ReadOne`.
- `org.freedesktop.impl.portal.Screenshot` v2 — focused-output screenshots,
  interactive region selection, and `PickColor`. The interactive operations
  use the compositor's picker and can be cancelled with Escape.
- `org.freedesktop.impl.portal.ScreenCast` v3 — monitor and window sources
  with a hidden cursor. Source selection always uses the compositor's
  interactive picker. Each cast republishes the
  compositor's scoped output-frame stream
  ([ADR-0052](../adr/0052-scoped-output-frame-streaming.md)) as a PipeWire
  producer, so a running PipeWire session is required. Window sharing crops
  the selected window's visible region from its output; occluding windows
  remain visible in the stream. Persistence is not advertised because the
  version 4 `restore_data` contract requires a compatible PermissionStore.
- `org.freedesktop.impl.portal.Inhibit` — flag 8 (idle) only, held
  through the compositor's scoped `SetIdleInhibit` IPC op
  ([ADR-0075](../adr/0075-independent-portal-package-and-backend-contract.md)).
  Flags 1/2/4 (logout, user switch, suspend) require a session manager Aegis
  does not have and are rejected. Each accepted call remains active until
  the frontend closes its request object; the backend periodically renews
  the scoped IPC lease and reconnects after a compositor restart.
- `org.freedesktop.impl.portal.Secret` v1 — sandboxed secret retrieval from
  an encrypted at-rest vault under `$XDG_DATA_HOME/aegis/secrets`. The first
  run creates a mode-0600 keyfile and unlocks automatically; password-mode
  vaults unlock through a masked compositor prompt, or silently when the
  `pam_aegis` PAM module (installed into the login and `aegis-lock` PAM
  stacks) cached the just-verified password. The classic
  `org.freedesktop.secrets` API is served too, as a transitional
  compatibility layer for un-sandboxed libsecret clients
  ([ADR-0085](../adr/0085-portal-secret-absorption-and-secret-service-compat.md)).
- `org.freedesktop.impl.portal.Lockdown` — stateless; every restriction
  flag reads `false` (Aegis has no kiosk policy engine).
- `org.freedesktop.impl.portal.FileChooser` v3 — `OpenFile`, `SaveFile`,
  and `SaveFiles` run the compositor's native file-picker chrome
  ([ADR-0086](../adr/0086-full-stack-portal-via-user-consent-pick-chains.md)):
  directory navigation, multi-select, file filters, and a filename field
  for saves. GTK stays configured as the fallback while the picker proves
  itself.
- `org.freedesktop.impl.portal.AppChooser` v2 — `ChooseApplication` runs
  the native app-picker chrome with catalog-resolved names and icons;
  `last_choice` pre-highlights. GTK fallback as above.
- `org.freedesktop.impl.portal.Email` v2 — compose requests hand off to
  the session mail client through `xdg-email`, attachments included. GTK
  keeps a compose-dialog fallback.
- `org.freedesktop.impl.portal.Notification` v2 — `AddNotification` posts
  into the compositor's own notification queue (toast strip, command
  panel, HUD count); `RemoveNotification` withdraws by the application's
  own id. Baseline `title`/`body` only; icons and action buttons are not
  supported yet.
- `org.freedesktop.impl.portal.Account` v1 — `GetUserInformation` answers
  only after a yes/no consent dialog approves sharing; identity comes
  from `getpwuid` plus the canonical avatar locations
  (`$XDG_DATA_HOME/aegis/avatars/face.*`, `~/.face`). GTK fallback while
  the consent flow proves itself.
- `org.freedesktop.impl.portal.DynamicLauncher` v1 — `PrepareInstall`
  consents through the same dialog, then echoes the proposed name/icon
  with a fresh install token. `RequestInstallToken` is always refused, so
  no launcher install bypasses consent. GTK fallback as above.
- `org.freedesktop.impl.portal.Wallpaper` v1 — `SetWallpaperURI` consents
  through the confirmation dialog, then decodes and swaps the image live
  on the compositor main loop. Only `file://` URIs; glTF stays
  startup-only. GTK fallback as above.

Aegis does not advertise the Background portal. GTK serves it and the
remaining unsupported interfaces (Access, Print, Location) through the
`aegis;gtk` fallback default.

Restart `xdg-desktop-portal` after installing so it re-scans the portal
files:

```sh
systemctl --user restart xdg-desktop-portal
```

## Verify

The backend is D-Bus-activated, so query it and the bus starts it:

```sh
busctl --user introspect org.freedesktop.impl.portal.desktop.aegis /org/freedesktop/portal/desktop
```

The output lists `org.freedesktop.impl.portal.Settings`,
`org.freedesktop.impl.portal.Screenshot` (version 2),
`org.freedesktop.impl.portal.ScreenCast` (version 3),
`org.freedesktop.impl.portal.Inhibit`,
`org.freedesktop.impl.portal.Secret` (version 1),
`org.freedesktop.impl.portal.Lockdown`,
`org.freedesktop.impl.portal.FileChooser` (version 3),
`org.freedesktop.impl.portal.Email` (version 2),
`org.freedesktop.impl.portal.AppChooser` (version 2),
`org.freedesktop.impl.portal.Notification` (version 2),
`org.freedesktop.impl.portal.Account` (version 1),
`org.freedesktop.impl.portal.DynamicLauncher` (version 1), and
`org.freedesktop.impl.portal.Wallpaper` (version 1). Every interface
reports its `version` property in lowercase exactly; a backend that gets
this wrong is skipped by the frontend entirely.

Read the appearance setting (with `color_scheme = "dark"` configured this
prints `variant u 1`; the default prints `u 0`):

```sh
busctl --user call org.freedesktop.impl.portal.desktop.aegis \
  /org/freedesktop/portal/desktop org.freedesktop.impl.portal.Settings \
  Read ss org.freedesktop.appearance color-scheme
```

Read the GTK-compatible effective icon theme:

```sh
busctl --user call org.freedesktop.impl.portal.desktop.aegis \
  /org/freedesktop/portal/desktop org.freedesktop.impl.portal.Settings \
  Read ss org.gnome.desktop.interface icon-theme
```

Change a value in the Appearance page or in `config.toml`, then repeat the
calls. The backend receives the replacement snapshot over IPC and emits
`SettingChanged` for each affected exported key.

Exercise the Screenshot path through the frontend, the same call a
sandboxed app makes:

```sh
gdbus call --session \
  --dest org.freedesktop.portal.Desktop \
  --object-path /org/freedesktop/portal/desktop \
  --method org.freedesktop.portal.Screenshot.Screenshot "" {}
```

The result contains a `file://` URI under
`$XDG_CACHE_HOME/aegis-portal/` (or `$XDG_RUNTIME_DIR/aegis-portal/` when the
cache directory is unset). The capture fails closed — response code 2 —
while the session is locked or the seat is inactive, exactly like
`aegis-ctl screenshot`.

Exercise the ScreenCast path the way applications do: share a screen or
window from a portal-aware client (for example `getDisplayMedia()` in
Firefox or Chromium). The chooser accepts an eligible monitor or window;
once sharing starts, the stream appears as a PipeWire node named
`aegis-portal-screencast`:

```sh
pw-dump | grep -A5 aegis-portal-screencast
```

Confirm that the node exposes an output port:

```sh
pw-link -o | grep aegis-portal-screencast
```

OBS and other capture consumers link this output port to receive frames. If
the node appears only as an input, replace the installed `aegis-portal`
binary with the current build and restart `xdg-desktop-portal`.

Sharing pauses while the session is locked or the seat is inactive and
resumes afterwards; it ends when the app stops sharing, the app exits, or
the portal session is closed.

Exercise the Inhibit path by requesting idle inhibition from a test client
(for example `gdbus` against the frontend's
`org.freedesktop.portal.Inhibit.Inhibit` with the frontend's idle flag), then
confirm the backend receives flag `8` and the
compositor no longer reports idle: a client subscribed to
`ext-idle-notify-v1` sees no `idled` event past its timeout while the
inhibiting application is alive, and sees it again once that application
exits.

If the name does not activate, check that the `.service` file's `Exec=`
path exists and that the compositor is running: the backend captures through
the aegis IPC socket at `$XDG_RUNTIME_DIR/aegis.sock`.
