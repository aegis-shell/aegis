# Setup

How to build and run ass for development.

## Prerequisites

| Requirement | Notes |
|-------------|-------|
| Rust toolchain | `rustc` and `cargo`, edition 2024 (1.88+) |
| flux + lens + iris source tree | Sibling `../optics` Meson project, containing `libs/flux`, `libs/lens`, `libs/iris`, and `bindings/` |
| meson and a C23 compiler | To build the optics libraries |
| Vulkan 1.3 runtime and loader | flux is Vulkan-first |
| Wayland client and protocols | `wayland-client`, `wayland-protocols`, and `wayland-scanner` for the nested backend |
| libxkbcommon | Server compiles the default keymap at startup for `wl_keyboard` |
| A running Wayland session | `$WAYLAND_DISPLAY` must be set to run the nested backend |
| bubblewrap + systemd user manager | Required for real Realm application sandbox tests |

## Build the dependencies

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

## Build and run

Build and run directly from the repository root:

```bash
cargo build -p ass
cargo run -p ass
```

With the compositor running, open the standalone settings application from a
second terminal:

```bash
cargo run -p ass-control-center
```

`cargo run -p ass` opens a nested window on `$WAYLAND_DISPLAY`, creates a
`VkSurfaceKHR` on flux's Vulkan instance, and presents the shell. The binary
and the relevant test harnesses re-emit the build-tree rpaths published by
the binding crates. No environment setup or `LD_LIBRARY_PATH` change is
required for the default sibling layout.

Use the [Nested Backend Development](nested-backend.md) workflow for daily
iteration, Cargo command selection, automatic rebuild-and-restart, inner
client launch, and the boundary between nested and DRM/KMS validation.

The compositor logs through the `log` facade; `RUST_LOG` controls verbosity
(default `info`):

```bash
RUST_LOG=debug cargo run -p ass      # verbose, including per-surface diagnostics
RUST_LOG=warn cargo run -p ass       # quiet: only warnings and errors
```

## Tests

```bash
cargo test --workspace
```

`ass-core` and `ass-server` unit tests run without the flux dependency;
the rest need the sibling optics Meson tree to be built first, same
as `cargo build`.

The ordinary workspace run skips kernel-level Realm launcher tests when the
test process is not alone in a controller-delegated cgroup. Run those tests
in the production topology with:

```bash
scripts/test-realm-sandbox.sh
```

The script starts the compiled `ass-launch` test binary as a transient
systemd user service with delegated `cpu`, `memory`, and `pids` controllers.
It verifies mount-scoped multi-connection Wayland portals, mandatory resource
limits, cgroup freeze/resume, and `cgroup.kill` against a worker that escapes
its process group.

## Troubleshooting

| Symptom | First check |
|---------|-----|
| `cannot connect to host Wayland display` | `$WAYLAND_DISPLAY` is unset or points at no compositor |
| Missing a `flux`, `flux-scene-graph`, `lens`, or `iris` uninstalled pkg-config file | The Meson build tree is not built with the required components; build the dependencies first |
| `vkCreateSwapchainKHR: function pointer was NULL` | `VK_KHR_swapchain` not enabled; the backend requests it, so check the flux device extensions |
| `error while loading shared libraries: libflux*.so` / `liblens*.so` / `libiris*.so` | Run through `cargo run` so the rpath relay applies, or rebuild after moving the Meson tree |
| `Realm cgroup isolation is unavailable` | Run ASS in the packaged systemd user service; a shared terminal scope cannot satisfy controller delegation |

## See Also

- [Project Layout](project-layout.md)
- [VT/DRM Manual Testing](vt-drm-testing.md)
- [Architecture](../explanation/architecture.md)
- [README quick start](../../README.md)
