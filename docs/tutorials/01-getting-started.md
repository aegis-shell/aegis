# Getting Started with Aegis

This tutorial builds Aegis, starts a nested compositor, launches System
Settings through the application catalog, and queries compositor state.
Run every command from the Aegis repository root inside an existing Wayland
session.

## Prerequisites

Install:

- Rust 1.88 or later;
- Meson, Ninja, `pkg-config`, a C23 compiler, and libclang;
- Vulkan, Wayland, xkbcommon, libinput, libseat, and Linux PAM development
  files; and
- the Optics `v0.0.7` native libraries.

Building the independent `aegis-portal` backend additionally requires
PipeWire and SPA development files. The nested compositor tutorial does not
build or install that package.

Build and install the matching Optics release when the distribution does not
provide it:

```bash
git clone --branch v0.0.7 --depth 1 \
  https://github.com/ming2k/optics.git ../optics
meson setup ../optics/build ../optics \
  -Dtests=false -Dbuildtype=debugoptimized
meson compile -C ../optics/build
sudo meson install -C ../optics/build
sudo ldconfig
pkg-config --modversion flux flux-scene-graph lens iris
```

Each version printed by the final command must be compatible with `0.0.7`.

## Stage System Settings

Cargo builds binaries but does not install desktop metadata. Stage System
Settings in a temporary prefix so the launcher can discover the executable,
desktop entry, and icon without changing `~/.local`:

```bash
aegis_stage=$(mktemp -d)
cargo build --locked -p aegis-settings
install -Dm0755 target/debug/aegis-settings \
  "$aegis_stage/bin/aegis-settings"
install -Dm0644 contrib/io.github.ming2k.aegis.Settings.desktop \
  "$aegis_stage/share/applications/io.github.ming2k.aegis.Settings.desktop"
install -Dm0644 \
  contrib/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg \
  "$aegis_stage/share/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg"
export PATH="$aegis_stage/bin:$PATH"
export XDG_DATA_DIRS="$aegis_stage/share:${XDG_DATA_DIRS:-/usr/local/share:/usr/share}"
```

## Start the Nested Compositor

Build the two supervised session clients, then run:

```bash
cargo build --locked -p aegis-idle -p aegis-lock
cargo run --locked -p aegis
```

Because `WAYLAND_DISPLAY` is set, the default `AEGIS_BACKEND=auto` opens a
nested Aegis window. The background, HUD chips, and Dock confirm that the
session is ready.

## Launch System Settings

Open Applications and select **System Settings**. Its independent window
appears and groups under the System Settings Dock identity. This verifies
desktop discovery, icon lookup, detached launch, Wayland identity, and IPC
connectivity together.

## Query Compositor State

From a second terminal in the same user session, run:

```bash
cargo run --locked -p aegis-ctl -- outputs
cargo run --locked -p aegis-ctl -- windows
```

Both commands print state from the running compositor. Stop Aegis with
`Ctrl+C`, then remove the temporary prefix:

```bash
rm -rf -- "$aegis_stage"
```

## Next Steps

- Use the [daily-use guides](../how-to/index.md) for window management,
  configuration, and display setup.
- Read the [Architecture](../explanation/architecture.md) for the component
  and security model.
