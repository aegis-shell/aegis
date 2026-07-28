# Distribution Packaging

Use this guide to build distribution packages from an Aegis source release.
Daily source-tree development does not install Aegis; see
[Setup](setup.md) instead.

## Dependency Contract

Aegis has two separate Optics dependency surfaces:

- `Cargo.lock` pins the Rust bindings to the Optics `v0.0.3` Git release.
- The native `flux`, `flux-scene-graph`, `lens`, and `iris` libraries are
  system build and runtime dependencies discovered through `pkg-config` and
  the dynamic loader.

A package build must not depend on an `../optics` checkout or enable
`.cargo/optics-local.toml`. Package the matching Optics release first and
verify the native development files before building Aegis:

```bash
pkg-config --modversion flux flux-scene-graph lens iris
```

Each command must report a version compatible with `0.0.3`. Distribution
package names vary, so express the dependency in terms of the shared
libraries, headers, and `.pc` files rather than copying files from an Optics
build tree.

The remaining native build dependencies include a C toolchain, `pkg-config`,
Wayland and its protocols, Vulkan, xkbcommon, libinput, libseat, PipeWire/SPA,
and libclang for binding generation. Runtime integration also expects
`xdg-desktop-portal`; install the GTK portal backend as a fallback for portal
interfaces Aegis does not implement.

## Reproducible Source Preparation

Keep the committed `Cargo.lock`. A network-enabled source-preparation phase
may fetch the locked Cargo graph directly or vendor it for an offline builder:

```bash
cargo vendor --locked vendor
```

Record Cargo's emitted source-replacement configuration in the package build
environment, include `vendor/` in the prepared source, and build with
`--frozen --offline`. Do not rewrite the Optics Git dependencies to local
paths in a distribution patch.

## Build

Build all workspace binaries from the source root:

```bash
cargo build --frozen --offline --release --workspace
```

Use `cargo build --locked --release --workspace` when the package builder is
allowed to use its pre-populated Cargo cache without a vendored source tree.

## Install Manifest

The canonical logical prefix is `/usr`. Stage files below `DESTDIR`; do not
run the built binaries, `systemctl`, `ldconfig`, or desktop database updates
inside the package build root.

| Source | Destination | Suggested component |
|--------|-------------|---------------------|
| `target/release/aegis` | `/usr/bin/aegis` | core |
| `target/release/aegis-settings` | `/usr/bin/aegis-settings` | core |
| `target/release/aegis-ctl` | `/usr/bin/aegis-ctl` | core |
| `target/release/aegis-portal` | `/usr/bin/aegis-portal` | portal |
| `target/release/aegis-fuji-mcp` | `/usr/bin/aegis-fuji-mcp` | agent integration |
| `target/release/fuji` | `/usr/bin/fuji` | agent integration |
| `contrib/systemd/user/aegis.service` | `/usr/lib/systemd/user/aegis.service` | core |
| `contrib/io.github.ming2k.aegis.Settings.desktop` | `/usr/share/applications/io.github.ming2k.aegis.Settings.desktop` | core |
| `contrib/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg` | `/usr/share/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg` | core |
| `contrib/dbus-1/services/org.freedesktop.impl.portal.desktop.aegis.service` | `/usr/share/dbus-1/services/org.freedesktop.impl.portal.desktop.aegis.service` | portal |
| `contrib/xdg-desktop-portal/portals/aegis.portal` | `/usr/share/xdg-desktop-portal/portals/aegis.portal` | portal |
| `contrib/xdg-desktop-portal/aegis-portals.conf` | `/usr/share/xdg-desktop-portal/aegis-portals.conf` | portal |

For example, a simple package recipe can stage binaries with:

```bash
package_root=${DESTDIR:?set DESTDIR to the package staging root}
install -Dm0755 target/release/aegis \
  "$package_root/usr/bin/aegis"
install -Dm0755 target/release/aegis-settings \
  "$package_root/usr/bin/aegis-settings"
install -Dm0755 target/release/aegis-ctl \
  "$package_root/usr/bin/aegis-ctl"
install -Dm0755 target/release/aegis-portal \
  "$package_root/usr/bin/aegis-portal"
install -Dm0755 target/release/aegis-fuji-mcp \
  "$package_root/usr/bin/aegis-fuji-mcp"
install -Dm0755 target/release/fuji \
  "$package_root/usr/bin/fuji"
```

Install the data files according to the table using mode `0644`. Splitting
the portal or agent binaries into subpackages is valid, but each subpackage
must own its complete activation metadata and declare the matching runtime
dependencies.

The supplied systemd and D-Bus service files intentionally execute binaries
from `/usr/bin`. A distribution using another logical prefix must patch those
two files consistently; changing only the file destinations creates broken
activation.

## Package Integration

Use package-manager hooks or distribution triggers, not the package build
phase, to:

- reload the systemd user-unit catalog when required by the distribution;
- refresh the desktop application and icon caches;
- refresh the dynamic-loader cache for Optics shared libraries; and
- restart an existing desktop portal session after an upgrade when required.

The user service delegates the cgroup controllers required by Realm
sandboxing. Keep its `Delegate=cpu memory pids` and session-target ordering
unless the distribution supplies an equivalent unit.

## Validation

Test the completed packages in a clean environment rather than running out of
`target/release`:

```bash
systemd-analyze --user verify aegis.service
desktop-file-validate \
  /usr/share/applications/io.github.ming2k.aegis.Settings.desktop
pkg-config --modversion flux flux-scene-graph lens iris
systemctl --user daemon-reload
systemctl --user start aegis.service
```

Confirm that System Settings appears with its icon, `aegis-portal` activates
through D-Bus, the preferred portal configuration is selected for an Aegis
session, and `aegis-ctl` reaches the compositor. Run the Realm sandbox test
through the packaged service topology as described in
[Setup](setup.md#tests).
