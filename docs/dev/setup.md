# Setup

How to build and run ass for development.

## Prerequisites

| Requirement | Notes |
|-------------|-------|
| Rust toolchain | `rustc` and `cargo`, edition 2021 (1.74+) |
| flux monorepo checkout | Sibling directory `../flux`, containing `core/` (libflux) and `ui/` (libflux-ui) subprojects |
| meson and a C23 compiler | To build the flux and flux-ui libraries |
| Vulkan 1.3 runtime and loader | flux is Vulkan-first |
| Wayland client and protocols | `wayland-client`, `wayland-protocols`, and `wayland-scanner` for the nested backend |
| libxkbcommon | Server compiles the default keymap at startup for `wl_keyboard` |
| A running Wayland session | `$WAYLAND_DISPLAY` must be set to run the nested backend |

## Build the dependencies

The Rust bindings build against the flux monorepo's meson build trees, so
build `libflux` and `libflux-ui` first:

```bash
meson compile -C ../flux/core/build
meson compile -C ../flux/ui/build
```

If a build tree does not exist yet, configure it once with
`meson setup ../flux/core/build` (and the same for `../flux/ui/build`)
before compiling. The bindings locate each tree through its uninstalled
pkg-config file; set `FLUX_BUILD_DIR` or `FLUX_UI_BUILD_DIR` to override
the default location.

## Build and run

```bash
cargo build
cargo run
```

`cargo run` opens a nested window on `$WAYLAND_DISPLAY`, creates a
`VkSurfaceKHR` on flux's Vulkan instance, and presents the shell. The
binary re-emits the rpaths the binding crates publish, so it finds
`libflux.so` and `libflux-ui.so` in the meson build trees without
`LD_LIBRARY_PATH`.

The compositor logs through the `log` facade; `RUST_LOG` controls
verbosity (default `info`):

```bash
RUST_LOG=debug cargo run      # verbose, including per-surface diagnostics
RUST_LOG=warn cargo run       # quiet: only warnings and errors
```

## Tests

```bash
cargo test --workspace
```

`ass-core` and `ass-server` unit tests run without the flux dependency;
the rest need the sibling flux meson trees to be built first, same as
`cargo build`.

## Troubleshooting

| Symptom | First check |
|---------|-------------|
| `cannot connect to host Wayland display` | `$WAYLAND_DISPLAY` is unset or points at no compositor |
| `missing flux-uninstalled.pc` or `flux-ui-uninstalled.pc` | The meson build tree is not built; build the dependencies first |
| `vkCreateSwapchainKHR: function pointer was NULL` | `VK_KHR_swapchain` not enabled; the backend requests it, so check the flux device extensions |
| `error while loading shared libraries: libflux*.so` | Run through `cargo run` so the rpath relay applies, or rebuild after moving the meson trees |

## See Also

- [Project Layout](project-layout.md)
- [Architecture](../explanation/architecture.md)
- [README quick start](../../README.md)
