# Setup

How to build and run ass for development.

## Prerequisites

| Requirement | Notes |
|-------------|-------|
| Rust toolchain | `rustc` and `cargo`, edition 2021 (1.74+) |
| flux + lens source trees | Sibling directory `../optics`, containing `flux/` (`libflux`) and `lens/` (`liblens`) meson subprojects, plus their out-of-tree Rust bindings `flux-rs/` and `lens-rs/` |
| meson and a C23 compiler | To build the flux and lens libraries |
| Vulkan 1.3 runtime and loader | flux is Vulkan-first |
| Wayland client and protocols | `wayland-client`, `wayland-protocols`, and `wayland-scanner` for the nested backend |
| libxkbcommon | Server compiles the default keymap at startup for `wl_keyboard` |
| A running Wayland session | `$WAYLAND_DISPLAY` must be set to run the nested backend |

## Build the dependencies

The Rust bindings build against the flux and lens meson build trees, so build
`libflux` and `liblens` first:

```bash
meson compile -C ../optics/flux/build
meson compile -C ../optics/lens/build
```

If a build tree does not exist yet, configure it once before compiling:

```bash
meson setup ../optics/flux/build ../optics/flux
meson setup ../optics/lens/build ../optics/lens
```

The `-sys` build scripts locate each tree in dev mode through its uninstalled
pkg-config file (`meson-uninstalled/flux-uninstalled.pc`,
`meson-uninstalled/lens-uninstalled.pc`); set `FLUX_BUILD_DIR` or
`LENS_BUILD_DIR` to override the default location.

## Build and run

Source `scripts/env.sh` to export the dev-mode variables, then build and run
from the repository root:

```bash
source scripts/env.sh
cargo build
cargo run
```

`cargo run` opens a nested window on `$WAYLAND_DISPLAY`, creates a
`VkSurfaceKHR` on flux's Vulkan instance, and presents the shell. The binary
re-emits the rpaths the binding crates publish, so it finds `libflux.so` and
`liblens.so` in the meson build trees without `LD_LIBRARY_PATH`.

If you have run `meson install` for both flux and lens into a prefix on
`PKG_CONFIG_PATH`, source the env script with
`ASS_DEV_ENV_USE_INSTALLED=1` to skip the build-tree probe and link the
installed libraries.

The compositor logs through the `log` facade; `RUST_LOG` controls verbosity
(default `info`):

```bash
RUST_LOG=debug cargo run      # verbose, including per-surface diagnostics
RUST_LOG=warn cargo run       # quiet: only warnings and errors
```

## Tests

```bash
cargo test --workspace
```

`ass-core` and `ass-server` unit tests run without the flux dependency;
the rest need the sibling flux and lens meson trees to be built first, same
as `cargo build`. Source `scripts/env.sh` first so the `-sys` crates locate
the build trees.

## Troubleshooting

| Symptom | First check |
|---------|-----|
| `cannot connect to host Wayland display` | `$WAYLAND_DISPLAY` is unset or points at no compositor |
| `missing flux-uninstalled.pc` or `lens-uninstalled.pc` | The meson build tree is not built; build the dependencies first |
| `vkCreateSwapchainKHR: function pointer was NULL` | `VK_KHR_swapchain` not enabled; the backend requests it, so check the flux device extensions |
| `error while loading shared libraries: libflux*.so` / `liblens*.so` | Run through `cargo run` so the rpath relay applies, or rebuild after moving the meson trees |

## See Also

- [Project Layout](project-layout.md)
- [Architecture](../explanation/architecture.md)
- [README quick start](../../README.md)
