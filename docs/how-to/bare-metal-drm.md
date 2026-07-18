# How to Run ass on Bare Metal (DRM/KMS)

The DRM/KMS backend drives display hardware directly from a TTY, with no
host session. The nested backend stays the development default; this guide
is the bare-metal bring-up and smoke checklist.

## Prerequisites

- A seat manager: `seatd` running (or `logind`), and your user in the
  `seat` group. Verify with `seatd-launch true` or `loginctl seat-status`.
- KMS permissions: your user can open `/dev/dri/cardN` through the seat
  manager (no root needed when the seat manager works).
- The libraries built the usual way (`meson compile -C ../optics/build`)
  and `scripts/env.sh` sourced.

## Start

Switch to a free TTY (`Ctrl+Alt+F3`), then:

```bash
source scripts/env.sh
cargo run --release -- --backend drm
```

`--backend auto` (the default) picks DRM when `$WAYLAND_DISPLAY` is unset,
so plain `cargo run` from a TTY also works. `ASS_DRM_DEVICE=/dev/dri/card1`
overrides the GPU when there is more than one. `RUST_LOG=info` shows the
device, seat, connector, and modifier choices as they happen.

Use the packaged `ass.service` instead of direct `cargo run` when testing
Realm application launch. The service delegates the cgroup controllers that
mandatory memory, process, CPU, freeze, and revoke boundaries require.

## Configure displays

Open **Control Center**, select **Sound & Display**, then use the **Display**
card to:

1. Select a connected monitor.
2. Choose one of its advertised resolution and refresh-rate modes.
3. Set the logical scale.
4. Select the primary display when needed.
5. Place an extended display relative to the primary display, or enter custom
   logical X and Y coordinates.
6. Select **Apply Display Settings**.

The change is written atomically to `~/.config/ass/config.toml`. Direct DRM
sessions apply it after the current page flip retires. In a nested session the
card is read-only because the outer compositor owns the physical monitors.
Run `ass-ctl outputs` to inspect exact connector names and advertised modes.

## Smoke checklist

First bare-metal run, in order:

1. **Lights up.** The mode is set on the first usable connector and the
   shell chrome appears. Watch for `EINVAL` on the first atomic commit —
   a modifier or format mismatch is logged with the plane/CRTC details.
2. **Input under load.** Wiggle the mouse continuously from the first
   frame; the compositor must keep pacing frames (60 Hz) and must not
   exit. This exercises the flip-wait deadline loop.
3. **Clients.** Launch a terminal from the launcher (`Super+Return`) and
   confirm keyboard focus, typing, and window move (`Super+drag`).
4. **VT switch.** `Ctrl+Alt+F2` away, then back (`Ctrl+Alt+F3` returns to
   ass — the compositor forwards these keys to libseat itself). The session
   resumes with a fresh modeset; at most one skipped frame is logged at
   warn level.
5. **Hotplug.** Unplug and replug the monitor (or dock). The display set
   is reprobed, the surface recreated if the modifier set changed, and
   workspaces return to their home connector (ADR-0025).
6. **Session lock.** Lock, confirm the screen shows only the lock client,
   and unlock. While locked, `ass-ctl` commands are refused.
7. **Screenshot.** `ass-ctl screenshot /tmp/tty.png` produces a PNG of the
   desktop (also exercises the CPU readback path).

## If it fails

- `seat: waiting for seat to become active` — you are on the wrong VT or
  the seat manager is not running; switch VT or start `seatd`.
- Immediate exit after the first frame — report the log; the flip-wait
  path is the first suspect.
- Black screen with a running log — check the modifier intersection in the
  log; set `ASS_DRM_DEVICE` to the other card on hybrid-GPU machines.
