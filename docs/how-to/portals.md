# How to Install and Verify the Portal Backend

Install `xdg-desktop-portal-atrium` when portal-aware and sandboxed
applications must use Tessera settings, screenshots, screen sharing, file
selection, email handoff, account data, and secrets. The compositor works
without this package. Installing only the backend executable is not enough:
xdg-desktop-portal must discover its metadata, select it for an Tessera
session, and activate it on the session bus.

## Prepare the Runtime

Install these runtime components:

- compatible `tessera` and `xdg-desktop-portal-atrium` releases from the
  [Portal Backend Reference](../reference/portal.md#identifiers-and-paths);
- `xdg-desktop-portal` as the public frontend;
- `xdg-desktop-portal-gtk` as the fallback for interfaces that Tessera does not
  implement;
- GTK 4.10 or newer for file selection;
- PipeWire and WirePlumber for screen sharing; and
- `xdg-email` for email handoff.

The compatibility mapping is strict even though the components use separate
source repositories and version sequences. The backend connects to the
compositor through the versioned socket at
`$XDG_RUNTIME_DIR/tessera.sock`; independently upgrading either side can make
that private protocol incompatible.

## Install the System Package

Use the distribution package when one is available. The
`xdg-desktop-portal-atrium` package must install all of these files from the
[xdg-desktop-portal-atrium repository](https://github.com/aegis-shell/xdg-desktop-portal-atrium):

| Source | Destination |
|--------|-------------|
| Meson portal executable | `/usr/libexec/xdg-desktop-portal-atrium` by default |
| Meson FileChooser executable | `/usr/libexec/atrium-portal-prompter` by default |
| Generated D-Bus service | `/usr/share/dbus-1/services/org.freedesktop.impl.portal.desktop.atrium.service` |
| `contrib/xdg-desktop-portal/portals/atrium.portal` | `/usr/share/xdg-desktop-portal/portals/atrium.portal` |
| `contrib/xdg-desktop-portal/atrium-portals.conf` | `/usr/share/xdg-desktop-portal/atrium-portals.conf` |
| `LICENSE` | `/usr/share/licenses/xdg-desktop-portal-atrium/LICENSE` |

The D-Bus service activates the portal backend. The `atrium.portal` file
declares the eight native interfaces, and `atrium-portals.conf` routes the
remaining interfaces to GTK. The Portal does not install a partial
`org.freedesktop.secrets` service.

There is no `xdg-desktop-portal-atrium.service` to enable. D-Bus starts the
private helper on demand.

### Install a source build system-wide

Clone the compatible Portal tag listed in the reference, then build from the
Portal repository root:

```bash
git clone --branch "v<PORTAL_VERSION>" --depth 1 \
  https://github.com/aegis-shell/xdg-desktop-portal-atrium.git \
  ../xdg-desktop-portal-atrium
cd ../xdg-desktop-portal-atrium
meson setup build --buildtype=release --prefix=/usr -Dpam=false
meson compile -C build
sudo meson install -C build
```

Meson generates the D-Bus service with the configured `libexecdir`, installs
both private executables, and installs the portal metadata and routing file.
Use `DESTDIR` instead of `sudo` when staging a package:

```bash
DESTDIR="$PWD/stage" meson install -C build
```

The generated service keeps its `Exec=` value synchronized when a
distribution configures another prefix or `libexecdir`.

## Stage a Per-User Development Build

Use a per-user installation to exercise backend changes without replacing
the system package. Run these commands from the
`xdg-desktop-portal-atrium` repository root:

```bash
meson setup build-user --buildtype=debug \
  --prefix="$HOME/.local" -Dpam=false
meson compile -C build-user
meson install -C build-user
```

Meson writes an absolute `Exec=` path into the per-user D-Bus service. D-Bus
does not expand `~`, `$HOME`, or shell expressions in `Exec=`; inspect the
installed service and confirm the path is absolute.

This staging changes the user's normal portal selection. Remove or rename the
per-user files after testing to return to the system configuration.

## Publish the Tessera Session Environment

Start a direct Tessera session before restarting the portal frontend. At
startup, Tessera publishes `WAYLAND_DISPLAY`, `XDG_SESSION_TYPE=wayland`, and
`XDG_CURRENT_DESKTOP=tessera` to the D-Bus activation environment and the
systemd user manager.

For the packaged session, run:

```bash
systemctl --user start tessera.service
systemctl --user show-environment | \
  grep -E '^(WAYLAND_DISPLAY|XDG_SESSION_TYPE|XDG_CURRENT_DESKTOP)='
systemctl --user restart xdg-desktop-portal.service
```

Do not continue until the output includes `XDG_CURRENT_DESKTOP=tessera` and the
Tessera `WAYLAND_DISPLAY`. Otherwise the frontend can select another desktop's
configuration or activate the backend against the wrong Wayland socket.

A nested Tessera session deliberately does not replace the outer desktop's
D-Bus or systemd activation environment. Direct backend calls can still test
that build, but the shared `org.freedesktop.portal.Desktop` frontend remains
owned by the outer desktop. Use a direct Tessera session or an isolated session
bus for an end-to-end nested portal test.

## Verify Backend Activation

Inspect the private backend name. This call asks the session bus to activate
the process:

```bash
busctl --user introspect org.freedesktop.impl.portal.desktop.atrium \
  /org/freedesktop/portal/desktop
```

The output must list the interfaces in the
[Portal Backend Reference](../reference/portal.md). Each interface must expose
the properties defined by its backend ABI. Every versioned interface must
spell its `version` property in lowercase; incorrect casing makes the frontend
skip that interface.

This private call proves D-Bus activation, but it does not prove that the
public frontend selected Tessera. Make a public Settings request next:

```bash
gdbus call --session \
  --dest org.freedesktop.portal.Desktop \
  --object-path /org/freedesktop/portal/desktop \
  --method org.freedesktop.portal.Settings.ReadOne \
  org.freedesktop.appearance color-scheme
```

The default returns `uint32 0`; a configured dark preference returns
`uint32 1`. Change the color scheme on the command panel's **Appearance** tab
(`Super+S`) and repeat the call to
verify the frontend, backend, and compositor IPC chain together.

## Verify an Interactive Request

Request a screenshot through the public frontend:

```bash
gdbus call --session \
  --dest org.freedesktop.portal.Desktop \
  --object-path /org/freedesktop/portal/desktop \
  --method org.freedesktop.portal.Screenshot.Screenshot "" {}
```

Complete the Tessera picker. A successful response contains a `file://` URI
under `$XDG_CACHE_HOME/xdg-desktop-portal-atrium/`, or under
`$XDG_RUNTIME_DIR/xdg-desktop-portal-atrium/` when the cache directory is
unset. The
request fails closed while the session is locked or its seat is inactive.

Verify screen sharing from a portal-aware browser or conferencing client.
After choosing an Tessera monitor or window, inspect the PipeWire producer:

```bash
pw-dump | grep -A5 xdg-desktop-portal-atrium-screencast
pw-link -o | grep xdg-desktop-portal-atrium-screencast
```

The stream pauses while the session is locked or inactive and resumes after
unlock. Window sharing renders the selected window itself, so the capture
keeps the window's real content even when it is occluded, minimized, or on
another workspace.

## Enable Secret Auto-Unlock Separately

Portal activation and secret prompting do not require `pam_tessera.so`.
`pam_tessera` is an optional, independently built PAM module that caches a
just-verified password in an owner-only runtime token. The portal consumes
and deletes that token to unlock a password-mode secret vault without a
second prompt.

Install the matching `tessera-pam` build from the Tessera Portal repository in
the distribution's PAM module directory, then add this line *after* the
primary authentication stack in the login and `tessera-lock` service profiles:

```text
auth optional pam_tessera.so
```

The Tessera core package's supplied `/etc/pam.d/tessera-lock` profile already
contains the optional line. The module is not the screen authenticator;
missing it must not prevent PAM from unlocking the session. Follow
[How to Install and Verify the Lock Screen](lock-screen.md) before changing a
PAM stack.

## Troubleshoot Activation

Inspect both the frontend and compositor journals:

```bash
journalctl --user -b -u xdg-desktop-portal.service --no-pager
journalctl --user -b -u tessera.service --no-pager
```

| Symptom | Check |
|---------|-------|
| The private bus name does not activate | Confirm that the D-Bus `.service` file is visible and its absolute `Exec=` target exists and is executable. |
| The backend activates but public interfaces are absent | Confirm `XDG_CURRENT_DESKTOP=tessera`, inspect `atrium-portals.conf`, then restart `xdg-desktop-portal.service`. |
| Settings work but capture or pickers fail | Confirm that the matching compositor is running and owns `$XDG_RUNTIME_DIR/tessera.sock`. |
| Screen sharing opens a chooser but produces no node | Confirm that the PipeWire user session is running and inspect the portal journal. |
| Unsupported interfaces disappear | Install `xdg-desktop-portal-gtk`; it is the configured fallback. |
| A nested test reaches the outer desktop | Use a direct session or isolate the session bus; nested Tessera intentionally does not overwrite the host activation environment. |

See the [Portal Backend Reference](../reference/portal.md) for interface
versions, dependencies, paths, and current limitations.
