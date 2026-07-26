# VT/DRM Manual Testing

Use this guide after a change works in the nested backend and you want to try
the actual desktop experience on real display and input hardware.

## Build Before Switching VTs

Build the release binaries in the graphical development session. Run from the
repository root:

```bash
meson compile -C ../optics/build
cargo build --release -p aegis -p aegis-settings -p aegis-ctl
```

Do not compile on the test VT. Building first keeps compiler latency and build
failures out of the hardware test.

## Run on a VT

1. Keep another VT or an SSH connection available in case the display becomes
   unusable.
2. Switch to a free VT, such as `Ctrl+Alt+F3`, and log in as the normal user.
3. Change to the ass repository root.
4. Confirm that another ass process is not running with
   `pgrep -a -x aegis`.
5. Start the DRM backend:

```bash
RUST_BACKTRACE=1 RUST_LOG=info \
  scripts/dev.sh --backend drm --release --no-build
```

On a multi-GPU machine, select the intended DRM card explicitly:

```bash
ASS_DRM_DEVICE=/dev/dri/card1 \
RUST_BACKTRACE=1 RUST_LOG=info \
  scripts/dev.sh --backend drm --release --no-build
```

Replace `/dev/dri/card1` with the card that owns the display connectors being
tested.

The runner stages the prebuilt Settings executable and XDG metadata, prepends
that private prefix to `PATH` and `XDG_DATA_DIRS`, clears
`WAYLAND_DISPLAY`, and starts exactly one DRM session. It does not install
files into the user or system prefix.

## Try the Desktop

Check the areas affected by the change. A useful general pass is:

1. Confirm that the shell appears at the expected resolution and refresh
   rate without flicker or repeated page-flip warnings.
2. Move and click the pointer, type into a client, change focus, and move a
   window with `Super+drag`.
3. Open a terminal with `Super+Return`, launch and close applications, and try
   the dock, launcher, overview, status bar controls, and AI Workspaces.
4. Switch to another VT, wait a few seconds, and switch back. Rendering and
   input should resume without restarting ass.
5. If the change affects outputs, test display settings, multiple monitors,
   or hotplug as appropriate. Do not unplug the only recovery display.

From a terminal running inside ass, inspect the live state and take a
screenshot:

```bash
./target/release/aegis-ctl outputs
./target/release/aegis-ctl windows
./target/release/aegis-ctl screenshot /tmp/ass-vt.png
```

Open `/tmp/ass-vt.png` after leaving the session and confirm that it matches
the visible output.

## Quit

Request a normal shutdown from a terminal inside ass:

```bash
./target/release/aegis-ctl quit
```

You can also press the default `Super+Shift+Return` binding. ass disables its
outputs and releases the seat and DRM device before returning to the TTY.

## Notes

- Run ass as the normal user through `seatd` or `logind`; do not use root to
  bypass device permissions.
- Only one ass process under the same `$XDG_RUNTIME_DIR` can provide the IPC
  socket. Stop an existing ass session before this test.
- When running ass on a separate VT while another compositor (e.g. Niri or Sway)
  is actively running under the same user account, wrap the command in
  `dbus-run-session` (e.g.
  `dbus-run-session scripts/dev.sh --backend drm --release --no-build`).
  This creates an isolated D-Bus session bus so Flatpak apps
  (`flatpak-session-helper`), D-Bus portals, and single-instance applications
  (like Firefox) do not route their windows back to the other compositor's
  session.
- Nested mode does not test atomic modesetting, libinput, libseat, physical
  hotplug, or VT suspend and resume.
- Realm application isolation must be tested through the packaged
  `aegis.service`, not a directly started binary.
- If the screen becomes unusable, switch to the reserved VT or connect over
  SSH. Run `pgrep -a -x aegis`, verify the exact PID, then use
  `kill -TERM <pid>`. Use `kill -KILL <pid>` only if that verified process does
  not stop.
