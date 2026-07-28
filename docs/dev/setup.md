# Setup

How to build and run aegis for development.

## Prerequisites

| Requirement | Notes |
|-------------|-------|
| Rust toolchain | `rustc` and `cargo`, edition 2024 (1.88+) |
| Optics `0.0.3` | Use a sibling checkout for cross-repository development or installed native libraries for the canonical build |
| meson and a C23 compiler | To build the optics libraries |
| Vulkan 1.3 runtime and loader | flux is Vulkan-first |
| Wayland client and protocols | `wayland-client`, `wayland-protocols`, and `wayland-scanner` for the nested backend |
| libxkbcommon | Server compiles the default keymap at startup for `wl_keyboard` |
| A running Wayland session | `$WAYLAND_DISPLAY` must be set to run the nested backend |
| bubblewrap + systemd user manager | Required for real Realm application sandbox tests |

## Dependency modes

Aegis keeps Rust source selection separate from native library discovery:

| Mode | Rust bindings | Native libraries | Intended use |
|------|---------------|------------------|--------------|
| Local | Path overrides into `../optics/bindings` | The sibling uninstalled Meson tree | Cross-repository development |
| Canonical | Locked Optics `v0.0.3` Git source | System `pkg-config` and loader paths | CI, releases, and distribution packages |

The root `Cargo.toml` always records the canonical Git dependencies.
`.cargo/optics-local.toml` is an explicit development override, so an
independent Aegis checkout never requires a sibling repository.

## Local Optics development

The Rust bindings build against the unified optics Meson build tree, so build
`libflux`, `libflux-scene-graph`, `liblens`, and `libiris` first:

```bash
meson compile -C ../optics/build
```

If a build tree does not exist yet, configure it once before compiling:

```bash
meson setup ../optics/build ../optics -Dtests=false -Dbuildtype=debugoptimized
```

`debugoptimized` keeps assertions but compiles the C libraries with `-O2`;
a plain `debug` (`-O0`) build is visibly slow on HiDPI outputs. An existing
tree can be switched in place with `meson configure ../optics/build
-Dbuildtype=debugoptimized && meson compile -C ../optics/build`.

The `-sys` build scripts locate the tree through
`meson-uninstalled/flux-uninstalled.pc` and
`meson-uninstalled/flux-scene-graph-uninstalled.pc`, plus
`meson-uninstalled/lens-uninstalled.pc` and
`meson-uninstalled/iris-uninstalled.pc`.

Activate the local bindings once for ordinary Cargo commands:

```bash
cp .cargo/optics-local.toml .cargo/config.toml
cargo check --workspace --locked
cargo test --locked --workspace
```

`.cargo/config.toml` is ignored by Git. Remove it to return to the canonical
locked Git sources and installed native libraries.

## Canonical and package builds

Install the matching Optics release before building Aegis:

```bash
meson setup ../optics/build-release ../optics \
  -Dtests=false --buildtype=release
meson compile -C ../optics/build-release
sudo meson install -C ../optics/build-release
sudo ldconfig
pkg-config --modversion flux flux-scene-graph lens iris
cargo build --locked --release --workspace
```

Distribution packages should declare the Optics `0.0.3` shared libraries,
headers, and `.pc` files as system build/runtime dependencies. Fetch or
vendor the locked Rust Git dependencies during the package source-preparation
phase, then run Cargo with `--locked`; do not require an `../optics` source
path in the package build directory.

The full CI job exercises this exact boundary: it installs the tagged Optics
C libraries, verifies their `pkg-config` metadata, and builds the locked
remote Rust bindings without a local Cargo override.

## Build and run

Run the compositor from a terminal in an existing Wayland session:

```bash
cargo run --locked -p aegis
```

The default `AEGIS_BACKEND=auto` selects nested presentation when
`$WAYLAND_DISPLAY` is present and direct DRM/KMS otherwise. Set
`AEGIS_BACKEND=nested` or `AEGIS_BACKEND=drm` only when a test must force one
backend.

The development commands have distinct responsibilities:

| Command | Role |
|---------|------|
| `cargo run --locked -p aegis` | Build and run the compositor with automatic backend selection |
| `AEGIS_BACKEND=drm cargo run --locked -p aegis` | Force direct-display testing |
| `cargo run --locked -p aegis-settings` | Run System Settings directly |

Run Settings directly from a second terminal only for focused UI or IPC
testing:

```bash
cargo run --locked -p aegis-settings
```

This direct command does not register the application with the Dock or
launcher. Stage the development artifacts into a throwaway prefix when a test
needs desktop-entry discovery, icon resolution, `TryExec`, and detached
launch:

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
cargo run --locked -p aegis
rm -rf -- "$aegis_stage"
```

The temporary prefix keeps the user's `~/.local` clean: the desktop entry's
`Exec` resolves through `PATH`, its `Icon` through `XDG_DATA_DIRS`, and the
whole prefix is removed when the shell exits. Distribution installation,
which owns systemd units, D-Bus services, and portal metadata, is documented
separately in [Distribution Packaging](packaging.md).

The compositor creates a `VkSurfaceKHR` on flux's Vulkan instance and presents
the shell. Local-mode binaries and test harnesses re-emit the uninstalled
build-tree rpaths published by the binding crates. Canonical builds use the
system dynamic-loader configuration.

Use the [Nested Backend Development](nested-backend.md) workflow for daily
iteration, Cargo command selection, inner client launch, and the boundary
between nested and DRM/KMS validation.
Use [First-Party Application Development](first-party-applications.md) for
the staging contract and focused application test matrix.

The compositor logs through the `log` facade; `RUST_LOG` controls verbosity
(default `info`):

```bash
RUST_LOG=debug cargo run --locked -p aegis
RUST_LOG=warn cargo run --locked -p aegis
```

## Tests

```bash
cargo test --locked --workspace
```

`aegis-core` and `aegis-compositor` unit tests run without the flux dependency;
the rest need either the sibling Optics Meson tree in local mode or the
installed libraries in canonical mode.

The ordinary workspace run skips kernel-level Realm launcher tests when the
test process is not alone in a controller-delegated cgroup. Run those tests
in the production topology with:

```bash
scripts/test-realm-sandbox.sh
```

The script starts the compiled `aegis-launcher` test binary as a transient
systemd user service with delegated `cpu`, `memory`, and `pids` controllers.
It verifies mount-scoped multi-connection Wayland portals, mandatory resource
limits, cgroup freeze/resume, and `cgroup.kill` against a worker that escapes
its process group.

## Troubleshooting

| Symptom | First check |
|---------|-----|
| `cannot connect to host Wayland display` | `$WAYLAND_DISPLAY` is unset or points at no compositor |
| `$XDG_RUNTIME_DIR is unset` | Log in through PAM/logind; do not create a shared runtime directory under `/tmp` |
| DRM runner rejects root | Log in as the normal seat user; do not bypass logind or seatd with `sudo` |
| Missing a `flux`, `flux-scene-graph`, `lens`, or `iris` pkg-config file | Build the sibling tree for local mode, or install the matching Optics release for canonical mode |
| `vkCreateSwapchainKHR: function pointer was NULL` | `VK_KHR_swapchain` not enabled; the backend requests it, so check the flux device extensions |
| `error while loading shared libraries: libflux*.so` / `liblens*.so` / `libiris*.so` | In local mode, rebuild after moving the Meson tree; in canonical mode, refresh the loader cache or configure the installed prefix |
| `Realm cgroup isolation is unavailable` | Run Aegis in the packaged systemd user service; a shared terminal scope cannot satisfy controller delegation |

## See Also

- [Project Layout](project-layout.md)
- [First-Party Application Development](first-party-applications.md)
- [VT/DRM Manual Testing](vt-drm-testing.md)
- [Architecture](../explanation/architecture.md)
- [README quick start](../../README.md)
