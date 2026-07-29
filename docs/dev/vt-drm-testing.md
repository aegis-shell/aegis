# VT/DRM Manual Testing

Use this guide after a change works in the nested backend and you want to try
the actual desktop experience on real display and input hardware.

## Build Before Switching VTs

Build the release binaries in the graphical development session. Run from the
repository root:

```bash
meson compile -C ../optics/build
cargo build --locked --release -p aegis -p aegis-settings -p aegis-ctl
```

Do not compile on the test VT. Building first keeps compiler latency and build
failures out of the hardware test.

## Run on a VT

1. Keep another VT or an SSH connection available in case the display becomes
   unusable.
2. Switch to a free VT, such as `Ctrl+Alt+F3`, and log in as the normal user.
3. Change to the aegis repository root.
4. Confirm that another aegis process is not running with
   `pgrep -a -x aegis`.
5. Start the DRM backend:

```bash
AEGIS_BACKEND=drm RUST_BACKTRACE=1 RUST_LOG=info \
  target/release/aegis
```

On a multi-GPU machine, select the intended DRM card explicitly:

```bash
AEGIS_DRM_DEVICE=/dev/dri/card1 \
AEGIS_BACKEND=drm RUST_BACKTRACE=1 RUST_LOG=info \
  target/release/aegis
```

Replace `/dev/dri/card1` with the card that owns the display connectors being
tested.

Stage the development prefix from [Setup](setup.md#build-and-run) before
switching VTs when this test must include discovery of the Settings
executable and its XDG metadata.

For Realm launch testing, start the installed user service instead of the
direct command:

```bash
systemctl --user start --wait aegis.service
```

The service used for development must point at the binary under test and
delegate the CPU, memory, and process controllers. A directly started binary
may test graphics, but the Realm launch warning is expected because it does
not own that delegated systemd service.

## Try the Desktop

Check the areas affected by the change. A useful general pass is:

1. Confirm that the shell appears at the expected resolution and refresh
   rate without flicker or repeated page-flip warnings.
2. Move and click the pointer, type into a client, change focus, and move a
   window with `Super+drag`.
3. Open a terminal with `Super+A`, launch and close applications, and try the
   dock, launcher, Prism, overview, status bar controls, and Agent Workspaces.
4. Switch to another VT, wait a few seconds, and switch back. Rendering and
   input should resume without restarting aegis.
5. If the change affects outputs, test display settings, multiple monitors,
   or hotplug as appropriate. Do not unplug the only recovery display.

From a terminal running inside aegis, inspect the live state and take a
screenshot:

```bash
./target/release/aegis-ctl outputs
./target/release/aegis-ctl windows
./target/release/aegis-ctl screenshot /tmp/aegis-vt.png
```

Open `/tmp/aegis-vt.png` after leaving the session and confirm that it matches
the visible output.

## Verify Client GPU Acceleration

Check the client's renderer before profiling frame pacing. OpenGL is not
itself a problem: Mesa's Wayland EGL path exports dma-bufs that Flux imports
with Vulkan. A software renderer means device selection failed before
compositing.

From a terminal inside aegis, inspect linux-dmabuf:

```bash
wayland-info |
  sed -n "/interface: 'zwp_linux_dmabuf_v1'/,+12p"
```

Expect version 4, a main device, and a render tranche. Start the OpenGL client
with Mesa diagnostics enabled. For osu!:

```bash
flatpak run \
  --env=LIBGL_DEBUG=verbose \
  --env=EGL_LOG_LEVEL=debug \
  sh.ppy.osu
```

After exiting the application, inspect its newest runtime log:

```bash
runtime_log=$(ls -t \
  ~/.var/app/sh.ppy.osu/data/osu/logs/*runtime.log |
  head -1)
rg 'GL Version|GL Renderer' "$runtime_log"
```

The renderer should name the physical Intel, AMD, or NVIDIA GPU, not
`llvmpipe`. When comparing against Niri, use the same application build,
Flatpak runtime, graphics settings, display mode, and frame limiter. If Niri
reports hardware rendering while aegis reports `llvmpipe`, investigate
aegis's linux-dmabuf feedback before compositor timing.

Use the following fault split:

- No linux-dmabuf v4 or no main device: inspect the Aegis startup log for the
  selected Vulkan DRM node and feedback-table errors.
- Version 4 is correct but the client still uses `llvmpipe`: check
  `flatpak info --show-permissions sh.ppy.osu | rg 'devices|dri'`, the Flatpak
  graphics runtime, and whether `LIBGL_ALWAYS_SOFTWARE` is set.
- The client uses the hardware GPU but remains slow: then measure frame
  pacing, repeated dma-buf imports, acquire-fence waits, KMS page flips, and
  direct-scanout eligibility. Do not use compositor FPS as a proxy for the
  client's renderer.

Mesa diagnostics such as `failed to get driver name for fd -1` followed by
`llvmpipe` are strong evidence that the client did not receive a usable DRM
device identity.

## Quit

Request a normal shutdown from a terminal inside aegis:

```bash
./target/release/aegis-ctl quit
```

You can also press `Super+Shift+Q` (or the alternate
`Super+Shift+Return` binding). Aegis disables its outputs and releases the
seat and DRM device before returning to the TTY. See
[System Shortcuts](../reference/keyboard-shortcuts.md) for the complete
shortcut reference.

## Notes

- Run aegis as the normal user through `seatd` or `logind`; do not use root to
  bypass device permissions.
- Only one aegis process under the same `$XDG_RUNTIME_DIR` can provide the IPC
  socket. Stop an existing aegis session before this test.
- When running aegis on a separate VT while another compositor (e.g. Niri or Sway)
  is actively running under the same user account, wrap the command in
  `dbus-run-session` (e.g.
  `dbus-run-session env AEGIS_BACKEND=drm target/release/aegis`).
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
