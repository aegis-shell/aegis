# VT/DRM Manual Testing

Use this guide after a change works in the nested backend and you want to try
the actual desktop experience on real display and input hardware.

## Build Before Switching VTs

Build the release binaries in the graphical development session. Run from the
repository root:

```bash
meson compile -C ../optics/build
cargo build --locked --release -p tessera
```

Do not compile on the test VT. Building first keeps compiler latency and build
failures out of the hardware test.

## Run on a VT

1. Keep another VT or an SSH connection available in case the display becomes
   unusable.
2. Switch to a free VT, such as `Ctrl+Alt+F3`, and log in as the normal user.
3. Change to the tessera repository root.
4. Confirm that another tessera process is not running with
   `pgrep -a -x tessera`.
5. Start the DRM backend:

```bash
TESSERA_BACKEND=drm RUST_BACKTRACE=1 RUST_LOG=info \
  target/release/tessera
```

On a multi-GPU machine, select the intended DRM card explicitly:

```bash
TESSERA_DRM_DEVICE=/dev/dri/card1 \
TESSERA_BACKEND=drm RUST_BACKTRACE=1 RUST_LOG=info \
  target/release/tessera
```

Replace `/dev/dri/card1` with the card that owns the display connectors being
tested.

Stage the development prefix from [Setup](setup.md#build-and-run) before
switching VTs when this test must include discovery of the Settings
executable and its XDG metadata.

For Interaction Domain launch testing, start the installed user service instead of the
direct command:

```bash
systemctl --user start --wait tessera.service
```

`--wait` returns once the compositor reports readiness (`Type=notify`), not
when the process forks.

The service used for development must point at the binary under test and
delegate the CPU, memory, and process controllers. A directly started binary
may test graphics, but the Interaction Domain launch warning is expected because it does
not own that delegated systemd service.

## Try the Desktop

Check the areas affected by the change. A useful general pass is:

1. Confirm that the shell appears at the expected resolution and refresh
   rate without flicker or repeated page-flip warnings.
2. Move and click the pointer, type into a client, change focus, and move a
   window with `Super+drag`.
3. Open a terminal with `Super+A`, launch and close applications, and try the
   dock, launcher, Prism, overview, HUD, and command panel.
4. Switch to another VT, wait a few seconds, and switch back. Rendering and
   input should resume without restarting tessera.
5. If the change affects outputs, test display settings, multiple monitors,
   or hotplug as appropriate. Do not unplug the only recovery display.

For presentation-loop changes, also complete this focused pass:

1. Run a continuously updating client, then move the pointer and type while a
   frame is in flight. Input must remain responsive; event dispatch no longer
   waits inside the backend for the page flip.
2. Leave a static client idle, then expose and cover it repeatedly. The log
   must not show repeated empty presents or
   `previous atomic commit still busy`.
3. Switch VTs while animation or a game is active. On return, the first frame
   must be complete, and press or release edges from before the switch must
   not replay into shell chrome.
4. On multiple monitors, unplug or reconfigure one display while a frame is
   in flight. Tessera must wait for every CRTC owned by the old atomic batch,
   rebuild the output surface, and present one full frame.
5. When outputs have different refresh rates, expect the current shared
   atomic presentation domain to retire at the slowest active CRTC. Record
   the modes in the test result; independent mixed-refresh pacing requires
   separately submitted presentation domains.

For presentation-plan or KMS plane changes, complete this additional pass:

1. Start with a normal multi-window desktop. Confirm the startup
   `plane allocation` message reports one primary plane per active output,
   the maximum compatible cursor-plane assignment, and overlay offload as
   disabled.
2. Open one opaque dma-buf client that covers the complete physical domain,
   then hide all shell chrome. Confirm `direct scanout active` appears once
   and does not repeat every frame.
3. Move the hardware cursor. Confirm the client remains in direct scanout and
   the update does not trigger a full compositor frame. Repeat with a setup
   that lacks a usable cursor plane and confirm a visible software cursor
   reports `software-cursor` instead.
4. Open the window switcher with `Super+Tab`. Confirm
   `compositor reclaimed the primary plane` appears on the first successful
   switcher frame. The old client must retain fullscreen state and keyboard
   focus while the selection is only being previewed.
5. Cancel the switcher and confirm the original client remains focused. Open
   it again, choose another client, and confirm focus and stacking change only
   when the switcher closes.
6. Trigger a screenshot or output capture while a client is otherwise
   eligible. Confirm composition is selected and the capture contains shell
   and client pixels from the same presented frame.
7. Switch VTs or hotplug an output while direct scanout is active. Confirm the
   resumed session begins with a full compositor frame and does not reuse
   framebuffer ownership from the retired backend epoch.

Use the
[Rendering and KMS Plane Reference](../reference/rendering.md) for the full
eligibility matrix, rejection-label meanings, and state transitions.

From a terminal running inside tessera, inspect the live state and take a
screenshot:

```bash
./target/release/tessera display
./target/release/tessera window
./target/release/tessera display capture /tmp/tessera-vt.png
```

Open `/tmp/tessera-vt.png` after leaving the session and confirm that it matches
the visible output.

## Verify Client GPU Acceleration

Check the client's renderer before profiling frame pacing. OpenGL is not
itself a problem: Mesa's Wayland EGL path exports dma-bufs that Flux imports
with Vulkan. A software renderer means device selection failed before
compositing.

From a terminal inside tessera, inspect linux-dmabuf:

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
reports hardware rendering while tessera reports `llvmpipe`, investigate
tessera's linux-dmabuf feedback before compositor timing.

Use the following fault split:

- No linux-dmabuf v4 or no main device: inspect the Tessera startup log for the
  selected Vulkan DRM node and feedback-table errors.
- Version 4 is correct but the client still uses `llvmpipe`: check
  `flatpak info --show-permissions sh.ppy.osu | rg 'devices|dri'`, the Flatpak
  graphics runtime, and whether `LIBGL_ALWAYS_SOFTWARE` is set.
- The client uses the hardware GPU but remains slow: then measure frame
  pacing, repeated dma-buf imports, acquire-fence waits, KMS page flips, and
  direct-scanout eligibility. Do not use compositor FPS as a proxy for the
  client's renderer.

The warning
`presentation batch still owns scanout after 1s`
means the page-flip watchdog fired. Tessera deliberately keeps the ownership
boundary instead of reusing a buffer that KMS may still scan. Check kernel DRM
messages, connector state, and whether every CRTC in the atomic batch emitted
its page-flip event.

Mesa diagnostics such as `failed to get driver name for fd -1` followed by
`llvmpipe` are strong evidence that the client did not receive a usable DRM
device identity.

## Quit

Request a normal shutdown from a terminal inside tessera:

```bash
./target/release/tessera quit
```

You can also press `Super+Ctrl+Q` (or the alternate
`Super+Shift+Return` binding). Tessera disables its outputs and releases the
seat and DRM device before returning to the TTY. See
[System Shortcuts](../reference/keyboard-shortcuts.md) for the complete
shortcut reference.

## Notes

- Run tessera as the normal user through `seatd` or `logind`; do not use root to
  bypass device permissions.
- Only one tessera process under the same `$XDG_RUNTIME_DIR` can provide the IPC
  socket. Stop an existing tessera session before this test.
- When running tessera on a separate VT while another compositor (e.g. Niri or Sway)
  is actively running under the same user account, wrap the command in
  `dbus-run-session` (e.g.
  `dbus-run-session env TESSERA_BACKEND=drm target/release/tessera`).
  This creates an isolated D-Bus session bus so Flatpak apps
  (`flatpak-session-helper`), D-Bus portals, and single-instance applications
  (like Firefox) do not route their windows back to the other compositor's
  session.
- Nested mode does not test atomic modesetting, libinput, libseat, physical
  hotplug, or VT suspend and resume.
- Interaction Domain application isolation must be tested through the packaged
  `tessera.service`, not a directly started binary.
- If the screen becomes unusable, switch to the reserved VT or connect over
  SSH. Run `pgrep -a -x tessera`, verify the exact PID, then use
  `kill -TERM <pid>`. Use `kill -KILL <pid>` only if that verified process does
  not stop.
