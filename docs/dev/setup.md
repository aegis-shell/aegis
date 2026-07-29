# Setup

How to build and run aegis for development.

## Prerequisites

| Requirement | Notes |
|-------------|-------|
| Rust toolchain | `rustc` and `cargo`, edition 2024 (1.88+) |
| Optics `0.0.4` | Use a sibling checkout for cross-repository development or installed native libraries for the canonical build |
| meson and a C23 compiler | To build the optics libraries |
| Vulkan 1.3 runtime and loader | flux is Vulkan-first |
| Wayland client and protocols | `wayland-client`, `wayland-protocols`, and `wayland-scanner` for the nested backend |
| libxkbcommon | Server compiles the default keymap at startup for `wl_keyboard` |
| A running Wayland session | `$WAYLAND_DISPLAY` must be set to run the nested backend |
| bubblewrap + systemd user manager | Required for real Realm application sandbox tests |

## Choose your workflow

Aegis development splits into two roles. Pick the row that describes you;
everything below this table follows from it.

| You are… | Workflow | Why |
|----------|----------|-----|
| **Contributing to Aegis only** | [Canonical workflow](#canonical-workflow-contributor) | You never touch Optics, so the locked `v0.0.4` bindings and the system-installed native libraries are all you need. No sibling checkout, no Cargo patch. |
| **Maintaining Aegis and Optics together** | [Local workflow](#local-workflow-dual-maintainer) | You isolate the live sibling Optics patch, lockfile, and target directory in a linked Aegis worktree. |

### How the two roles differ

Aegis deliberately keeps Rust source selection separate from native library
discovery. That separation is what makes both roles coexist on the same
checkout without one forcing its setup on the other:

| Concern | Canonical workflow | Local workflow |
|---------|--------------------|----------------|
| Rust bindings | Locked Optics `v0.0.4` Git source | `[patch]` entries for `../optics/bindings` |
| Native libraries | System `pkg-config` and dynamic-loader paths | The sibling uninstalled Meson tree (via `meson-uninstalled/*.pc`) |
| `.cargo/config.toml` | Absent | A copy of `.cargo/optics-local.toml` |
| `Cargo.lock` | Canonical and committed | Worktree-local and excluded from commits |
| `target/` | Primary worktree cache | Linked-worktree cache |
| Sibling `../optics` required | No | Yes |

The root `Cargo.toml` always records the canonical Git dependencies.
`.cargo/optics-local.toml` is an explicit, opt-in development override, so an
independent Aegis checkout never requires a sibling repository. Distribution
packaging and CI always use the canonical workflow; see
[Distribution Packaging](packaging.md).

### Local workflow (dual maintainer)

For maintainers who edit Aegis and Optics together. Keep the primary Aegis
worktree in canonical mode and create one long-lived linked development
worktree:

```bash
git worktree add -b dev ../aegis-dev main
cd ../aegis-dev
cp .cargo/optics-local.toml .cargo/config.toml
git config core.hooksPath .githooks
```

Run these commands only in the linked worktree. They install the local
`[patch]` configuration and enable the repository commit hook. The hook
preserves ordinary `git add .` usage by automatically removing the local
`Cargo.lock` and `.cargo/config.toml` from the staged set. Keep the worktree
on its long-lived local `dev` branch and fast-forward `main` to completed
`dev` commits.

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

Resolve the patched graph once without `--locked`, then use locked commands
until an Optics manifest changes:

```bash
cargo check -p aegis
cargo check --locked --workspace
cargo test --locked --workspace
```

The local lockfile and target directory remain inside the linked worktree.
Follow
[Aegis and Optics Cross-Repository Development](cross-repository-development.md)
for daily commits, rebases, release promotion, and merging `dev` into
`main`.

### Canonical workflow (contributor)

For everyone contributing to Aegis only. You do **not** need a sibling
`optics` checkout. Build the matching Optics `0.0.4` once so its native
libraries, headers, and `.pc` files land in the system paths, then drive Aegis
with ordinary `cargo` commands against the locked bindings.

**One-time Optics install** (or upgrade it whenever a project bump moves to a
new Optics tag):

```bash
meson setup ../optics/build-release ../optics \
  -Dtests=false --buildtype=release
meson compile -C ../optics/build-release
sudo meson install -C ../optics/build-release
sudo ldconfig
pkg-config --modversion flux flux-scene-graph lens iris   # sanity check
```

> If your distribution already ships an Optics `0.0.4` package, install that
> instead and skip the manual build. What matters is that the four
> `pkg-config --modversion` checks above each report a `0.0.4`-compatible
> version.

**Daily development** needs no further Optics work — just Cargo:

```bash
cargo run --locked -p aegis          # build & run (see Build and run below)
cargo test --locked --workspace
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
```

This is the exact boundary the full CI job exercises: it installs the tagged
Optics C libraries, verifies their `pkg-config` metadata, and builds the
locked remote Rust bindings without any local Cargo override.

Building distribution packages is a separate concern with its own rules
(vendoring, offline builds, install manifests, integration triggers). Do not
apply them to contributor development; see
[Distribution Packaging](packaging.md) instead.

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
the shell. Local-workflow binaries and test harnesses re-emit the uninstalled
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
the rest need either the sibling Optics Meson tree in the local workflow or
the installed libraries in the canonical workflow.

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
| Missing a `flux`, `flux-scene-graph`, `lens`, or `iris` pkg-config file | In the local workflow, build the sibling tree; in the canonical workflow, install the matching Optics release |
| `vkCreateSwapchainKHR: function pointer was NULL` | `VK_KHR_swapchain` not enabled; the backend requests it, so check the flux device extensions |
| `error while loading shared libraries: libflux*.so` / `liblens*.so` / `libiris*.so` | In the local workflow, rebuild after moving the Meson tree; in the canonical workflow, refresh the loader cache or configure the installed prefix |
| `Realm cgroup isolation is unavailable` | Run Aegis in the packaged systemd user service; a shared terminal scope cannot satisfy controller delegation |

## See Also

- [Aegis and Optics Cross-Repository Development](cross-repository-development.md)
- [Project Layout](project-layout.md)
- [First-Party Application Development](first-party-applications.md)
- [VT/DRM Manual Testing](vt-drm-testing.md)
- [Architecture](../explanation/architecture.md)
- [README quick start](../../README.md)
