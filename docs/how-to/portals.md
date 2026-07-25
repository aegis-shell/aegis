# How to Install and Verify the Portal Backend

`aegis-portal` is the xdg-desktop-portal backend for ass
([ADR-0051](../adr/0051-portal-backend-dbus-bridge.md)). It lets
portal-aware and sandboxed (Flatpak) applications read the desktop
appearance preference, take screenshots, and cast the screen. This guide
installs the three integration files and verifies the backend end to end.

The backend itself is the `aegis-portal` binary, D-Bus-activated on demand —
there is no service to enable or start.

## Install

Install the binary and the three files shipped under `contrib/`:

| Source | Install to |
|--------|-----------|
| `target/release/aegis-portal` | `/usr/bin/aegis-portal` |
| `contrib/xdg-desktop-portal/portals/aegis.portal` | `/usr/share/xdg-desktop-portal/portals/aegis.portal` |
| `contrib/xdg-desktop-portal/aegis-portals.conf` | `/usr/share/xdg-desktop-portal/aegis-portals.conf` |
| `contrib/dbus-1/services/org.freedesktop.impl.portal.desktop.aegis.service` | `/usr/share/dbus-1/services/org.freedesktop.impl.portal.desktop.aegis.service` |

For a per-user install, the same files work under
`~/.local/share/xdg-desktop-portal/portals/`,
`~/.config/xdg-desktop-portal/aegis-portals.conf`, and
`~/.local/share/dbus-1/services/` respectively; point `Exec=` at wherever
the binary lives.

`aegis-portals.conf` sets `default=aegis;gtk`, so interfaces aegis does not serve
yet (file chooser, notifications, …) fall back to the GTK backend. aegis
currently serves:

- `org.freedesktop.impl.portal.Settings` v1 — `org.freedesktop.appearance`
  `color-scheme`, from `[appearance] color_scheme` in
  `~/.config/aegis/config.toml` (see the
  [Configuration Reference](../reference/config.md#appearance)).
  `SettingChanged` is emitted when the value changes (a two-second mtime
  poll of the configuration file).
- `org.freedesktop.impl.portal.Screenshot` v1 — non-interactive captures
  only. `interactive = true` requests fail with response code 2 until the
  interactive dialog lands (Phase 3).
- `org.freedesktop.impl.portal.ScreenCast` v2 — monitor sources only,
  no cursor capture. Each cast republishes the
  compositor's scoped output-frame stream
  ([ADR-0052](../adr/0052-scoped-output-frame-streaming.md)) as a PipeWire
  producer, so a running PipeWire session is required. Source selection is
  non-interactive until Phase 3. `persist_mode` 1 and 2 are accepted: mode
  1 tokens live until the portal restarts, mode 2 tokens persist (see
  [Persisted grants](#persisted-grants)). `Start` returns the session's
  `restore_token`; presenting a valid token in `SelectSources` restores the
  cast with no further check — the token is the authorization credential.
- `org.freedesktop.impl.portal.Background` v1 — `background = true` is
  granted by default and recorded (aegis has no running-application tracking;
  the grant is a bookkeeping answer, not enforced confinement).
  `autostart = true` copies the application's desktop file into
  `$XDG_CONFIG_HOME/autostart/` and is reported `false` when the
  application ships no desktop file; `autostart = false` removes the file.
- `org.freedesktop.impl.portal.Inhibit` v1 — flag 4 (idle) only, held
  through the compositor's scoped `SetIdleInhibit` IPC op
  ([ADR-0053](../adr/0053-portal-session-services-and-grants.md)). Flags
  1/2/8 (logout, user switch, suspend) need a session manager aegis does not
  have; they are logged and ignored. An application's idle inhibits are
  released when it exits (its bus name vanishes) or when the portal
  restarts. `QueryEndResponse` is declared but never emitted — there is no
  session manager to end the session.

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
`org.freedesktop.impl.portal.Screenshot`,
`org.freedesktop.impl.portal.ScreenCast` (version 2),
`org.freedesktop.impl.portal.Background`, and
`org.freedesktop.impl.portal.Inhibit` (version 1 each).

Read the appearance setting (with `color_scheme = "dark"` configured this
prints `variant u 1`; an unset key prints `u 0`):

```sh
busctl --user call org.freedesktop.impl.portal.desktop.aegis \
  /org/freedesktop/portal/desktop org.freedesktop.impl.portal.Settings \
  Read ss org.freedesktop.appearance color-scheme
```

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
Firefox or Chromium). The chooser offers the monitor; once sharing starts,
the stream appears as a PipeWire node named `aegis-portal-screencast`:

```sh
pw-dump | grep -A5 aegis-portal-screencast
```

Sharing pauses while the session is locked or the seat is inactive and
resumes afterwards; it ends when the app stops sharing, the app exits, or
the portal session is closed.

Exercise the Background path through the frontend (the call a Flatpak
application makes when it asks to keep running without a window):

```sh
gdbus call --session \
  --dest org.freedesktop.portal.Desktop \
  --object-path /org/freedesktop/portal/desktop \
  --method org.freedesktop.portal.Background.RequestBackground "" \
  "{'reason': <'portal smoke test'>, 'autostart': <false>}"
```

The response carries `background: true` once the frontend resolves it
through the ass backend; with `autostart: <true>` the application's desktop
file appears under `$XDG_CONFIG_HOME/autostart/`.

Exercise the Inhibit path by requesting idle inhibition from a test client
(for example `gdbus` against the frontend's
`org.freedesktop.portal.Inhibit.Inhibit` with flags `4`), then confirm the
compositor no longer reports idle: a client subscribed to
`ext-idle-notify-v1` sees no `idled` event past its timeout while the
inhibiting application is alive, and sees it again once that application
exits.

## Persisted Grants

ass has no PermissionStore; the backend records its own authorization
decisions as JSON under `$XDG_DATA_HOME/aegis-portal/`
([ADR-0053](../adr/0053-portal-session-services-and-grants.md)):

- `background.json` — one recorded `{background, autostart}` decision per
  app_id, answered on every repeat request.
- `screencast-tokens.json` — mode 2 ScreenCast restore tokens. Delete the
  file (or individual entries) to revoke persisted cast grants; a presented
  token that no longer validates is replaced by a freshly minted one.

Both files are mode `0600` and written atomically.

If the name does not activate, check that the `.service` file's `Exec=`
path exists and that the compositor is running: the backend captures through
the ass IPC socket at `$XDG_RUNTIME_DIR/aegis.sock`.
