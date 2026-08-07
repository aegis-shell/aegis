# Getting Started with Aegis

This tutorial builds Aegis, starts a nested compositor, opens the command
panel's settings tabs, and queries compositor state.
Run every command from the Aegis repository root inside an existing Wayland
session.

## Prerequisites

Install:

- Rust 1.88 or later;
- Meson, Ninja, `pkg-config`, a C23 compiler, and libclang;
- Vulkan, Wayland, xkbcommon, libinput, and libseat development files; and
- the Optics `v<OPTICS_VERSION>` native libraries.

The independently sourced
[xdg-desktop-portal-aegis](https://github.com/aegis-shell/xdg-desktop-portal-aegis)
additionally requires PipeWire and SPA development files. The nested
compositor tutorial does not clone, build, or install that package.

Build and install the matching Optics release when the distribution does not
provide it:

```bash
git clone --branch "v<OPTICS_VERSION>" --depth 1 \
  https://github.com/ming2k/optics.git ../optics
meson setup ../optics/build ../optics \
  -Dtests=false -Dbuildtype=debugoptimized
meson compile -C ../optics/build
sudo meson install -C ../optics/build
sudo ldconfig
pkg-config --modversion flux flux-scene-graph lens iris
```

Each version printed by the final command must be compatible with
`<OPTICS_VERSION>`.

## Start the Nested Compositor

Build the two supervised session clients, then run:

```bash
cargo build --locked -p aegis-idle -p aegis-lock
cargo run --locked -p aegis
```

Because `WAYLAND_DISPLAY` is set, the default `AEGIS_BACKEND=auto` opens a
nested Aegis window. The background, HUD chips, and Dock confirm that the
session is ready.

## Open the Settings Tabs

Press `Super+S` inside the nested session. The command panel opens over a
dark blurred scrim. Select the **Display** or **Touchpad** tab in the main
panel's tab bar: the settings page renders in place and its edits commit
through the compositor's revisioned settings transaction. This verifies the
panel chrome, the hosted settings modules, and the commit path together.

## Query Compositor State

From a second terminal in the same user session, run:

```bash
cargo run --locked -p aegis -- display
cargo run --locked -p aegis -- window
```

Both commands print state from the running compositor. Stop Aegis with
`Ctrl+C`.

## Next Steps

- Use the [daily-use guides](../how-to/index.md) for window management,
  configuration, and display setup.
- Read the [Architecture](../explanation/architecture.md) for the component
  and security model.
