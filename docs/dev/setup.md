# Setup

How to build and run aegis for development.

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
| Bash 4.3+ | Runs the repository-owned development commands |
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

Build, stage, and run one integrated nested session:

```bash
scripts/dev.sh
```

The runner builds `aegis` and `aegis-settings`, stages the Settings binary,
desktop entry, and icon under `target/aegis-dev`, and starts the compositor
with that private prefix on `PATH` and `XDG_DATA_DIRS`. System Settings then
appears in Applications through the same XDG discovery path used after
installation.

The development commands have distinct responsibilities:

| Command | Role |
|---------|------|
| `scripts/dev.sh` | Build, stage, and run exactly one integrated session; nested is the safe default |
| `scripts/dev.sh --backend drm` | Build, stage, and run exactly one explicit direct-display session |
| `cargo run -p aegis` | Run only the compositor crate without application staging |

Run `scripts/dev.sh --help` for the complete interface.

Run Settings directly from a second terminal only for focused UI or IPC
testing:

```bash
cargo run -p aegis-settings
```

This direct command does not register the application with the Dock or
launcher. Likewise, `cargo run -p aegis -- --backend nested` starts only the
compositor and does not stage first-party applications.

The compositor creates a `VkSurfaceKHR` on flux's Vulkan instance and presents
the shell. The binaries and relevant test harnesses re-emit the build-tree
rpaths published by the binding crates. No `LD_LIBRARY_PATH` change is
required for the default sibling layout.

Use the [Nested Backend Development](nested-backend.md) workflow for daily
iteration, Cargo command selection, inner client launch, and the boundary
between nested and DRM/KMS validation.
Use [First-Party Application Development](first-party-applications.md) for
the staging contract and focused application test matrix.

The compositor logs through the `log` facade; `RUST_LOG` controls verbosity
(default `info`):

```bash
RUST_LOG=debug scripts/dev.sh
RUST_LOG=warn scripts/dev.sh
```

## Tests

```bash
cargo test --workspace
tests/dev-workflow.sh
```

`aegis-core` and `aegis-compositor` unit tests run without the flux dependency;
the rest need the sibling optics Meson tree to be built first, same
as `cargo build`.

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
| Missing a `flux`, `flux-scene-graph`, `lens`, or `iris` uninstalled pkg-config file | The Meson build tree is not built with the required components; build the dependencies first |
| `vkCreateSwapchainKHR: function pointer was NULL` | `VK_KHR_swapchain` not enabled; the backend requests it, so check the flux device extensions |
| `error while loading shared libraries: libflux*.so` / `liblens*.so` / `libiris*.so` | Run through `cargo run` so the rpath relay applies, or rebuild after moving the Meson tree |
| `Realm cgroup isolation is unavailable` | Run Aegis in the packaged systemd user service; a shared terminal scope cannot satisfy controller delegation |

## See Also

- [Project Layout](project-layout.md)
- [First-Party Application Development](first-party-applications.md)
- [VT/DRM Manual Testing](vt-drm-testing.md)
- [Architecture](../explanation/architecture.md)
- [README quick start](../../README.md)
