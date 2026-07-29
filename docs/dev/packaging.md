# Distribution Packaging

Use this guide to build distribution packages from an Aegis source release.
Daily source-tree development does not install Aegis; see
[Setup](setup.md) instead.

## Dependency Contract

Aegis has two separate Optics dependency surfaces:

- `Cargo.lock` pins the Rust bindings to the Optics tag reported by
  `scripts/optics-release-ref.sh`.
- The native `flux`, `flux-scene-graph`, `lens`, and `iris` libraries are
  system build and runtime dependencies discovered through `pkg-config` and
  the dynamic loader.

A package build must not depend on an `../optics` checkout or enable
`.cargo/optics-local.toml`. Package the matching Optics release first and
verify the native development files before building Aegis:

```bash
pkg-config --modversion flux flux-scene-graph lens iris
```

Each command must report a version compatible with `0.0.4`. Distribution
package names vary, so express the dependency in terms of the shared
libraries, headers, and `.pc` files rather than copying files from an Optics
build tree. (On Arch this is the separate `optics` package; see
[Arch Linux](#arch-linux).)

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
| `assets/cursors/Bibata-Modern-Ice/LICENSE` | `/usr/share/licenses/aegis/Bibata-Modern-Ice/LICENSE` | core |
| `assets/cursors/Bibata-Modern-Ice/NOTICE` | `/usr/share/licenses/aegis/Bibata-Modern-Ice/NOTICE` | core |

The bundled Bibata-Modern-Ice cursor theme (GPL-3.0) is embedded into the
`aegis` binary via `include_dir`. Distributing that binary requires preserving
the theme's license disclosure, so the `LICENSE` and `NOTICE` files must be
staged under `/usr/share/licenses/aegis/Bibata-Modern-Ice/`. The project code
itself is MIT-licensed; the shipped binary is a combined work under
`MIT AND GPL-3.0-only`.

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

On Arch these map to pacman `post_install`/`post_upgrade` scripts or
`/usr/share/libalpm/hooks/*`; see the [Arch Linux](#arch-linux) recipe for a
concrete mapping.

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

## Distribution recipes

The sections above are distribution-neutral. The recipes below specialize them
to a specific distribution. Each recipe consumes the same source-preparation,
build, install-manifest, and validation contract; only the dependency names,
package format, and integration hooks change.

- [Arch Linux](#arch-linux) — `optics` + `aegis` PKGBUILDs.

### Arch Linux

Aegis on Arch is two packages. Package Optics first, because Aegis needs its
native libraries, headers, and `.pc` files as build and runtime dependencies.

| Package | Provides | Built by |
|---------|----------|----------|
| `optics` | `libflux.so`, `libflux-scene-graph.so`, `liblens.so`, `libiris.so`, headers, `flux.pc` … `iris.pc` | Meson/Ninja from the `ming2k/optics` `v0.0.4` tag |
| `aegis` | `/usr/bin/aegis{,-settings,-ctl,-portal,-fuji-mcp}`, `/usr/bin/fuji`, plus the systemd, desktop, D-Bus, and xdg-desktop-portal metadata from `contrib/` | Cargo from the `ming2k/aegis` `v0.0.7` tag |

CI installs the same Debian packages for both (`.github/workflows/ci.yml`,
`full-workspace` job); the `makedepends`/`depends` lists below are their Arch
equivalents.

#### `optics` PKGBUILD

```bash
# Maintainer: <you>
pkgname=optics
pkgver=0.0.4
pkgrel=1
pkgdesc='Vulkan-first rendering stack: flux, flux-scene-graph, lens, iris'
arch=(x86_64)
url='https://github.com/ming2k/optics'
license=(MIT)
depends=(glibc vulkan-icd-loader wayland libxkbcommon pipewire libinput
         seatd freetype2 harfbuzz fontconfig fribidi)
makedepends=(meson ninja pkgconf gcc glslang clang
             vulkan-headers wayland-protocols systemd-libs glfw-x11)
source=("$url/archive/refs/tags/v$pkgver.tar.gz")
sha256sums=('SKIP')   # replace with the real release-tarball checksum

build() {
  meson setup build "$pkgname-$pkgver" \
    -Dexamples=false -Dtests=false --buildtype=release
  meson compile -C build
}

package() {
  DESTDIR="$pkgdir" meson install -C build
}
```

`meson install` lays the libraries, headers, and `.pc` files under `/usr`, so
`pkg-config --modversion flux flux-scene-graph lens iris` resolves after the
package is installed.

#### `aegis` PKGBUILD

```bash
# Maintainer: <you>
pkgname=aegis
pkgver=0.0.7
pkgrel=1
pkgdesc='Wayland compositor and desktop shell'
arch=(x86_64)
url='https://github.com/ming2k/aegis'
# Project code is MIT; the bundled Bibata-Modern-Ice cursor theme is GPL-3.0-only.
license=(MIT GPL-3.0-only)
depends=(optics vulkan-icd-loader wayland libxkbcommon libinput seatd
         pipewire systemd-libs dbus
         xdg-desktop-portal xdg-desktop-portal-gtk   # GTK backend is the portal fallback
         hicolor-icon-theme)
makedepends=(rust pkgconf clang wayland wayland-protocols optics)
optdepends=('vulkan-mesa-layers: validation and layers for development')
source=("$url/archive/refs/tags/v$pkgver.tar.gz")
sha256sums=('SKIP')   # replace with the real release-tarball checksum

build() {
  cd "$pkgname-$pkgver"
  # Hard contract from the dependency section: Optics native libs must be present.
  pkg-config --modversion flux flux-scene-graph lens iris
  cargo build --locked --release --workspace
}

package() {
  cd "$pkgname-$pkgver"
  local dest="$pkgdir/usr"

  # Binaries → /usr/bin (mode 0755), per the install manifest.
  install -Dm0755 target/release/aegis          "$dest/bin/aegis"
  install -Dm0755 target/release/aegis-settings "$dest/bin/aegis-settings"
  install -Dm0755 target/release/aegis-ctl      "$dest/bin/aegis-ctl"
  install -Dm0755 target/release/aegis-portal   "$dest/bin/aegis-portal"
  install -Dm0755 target/release/aegis-fuji-mcp "$dest/bin/aegis-fuji-mcp"
  install -Dm0755 target/release/fuji           "$dest/bin/fuji"

  # Metadata (mode 0644).
  install -Dm0644 contrib/systemd/user/aegis.service \
    "$pkgdir/usr/lib/systemd/user/aegis.service"
  install -Dm0644 contrib/io.github.ming2k.aegis.Settings.desktop \
    "$dest/share/applications/io.github.ming2k.aegis.Settings.desktop"
  install -Dm0644 \
    contrib/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg \
    "$dest/share/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg"
  install -Dm0644 \
    contrib/dbus-1/services/org.freedesktop.impl.portal.desktop.aegis.service \
    "$dest/share/dbus-1/services/org.freedesktop.impl.portal.desktop.aegis.service"
  install -Dm0644 contrib/xdg-desktop-portal/portals/aegis.portal \
    "$dest/share/xdg-desktop-portal/portals/aegis.portal"
  install -Dm0644 contrib/xdg-desktop-portal/aegis-portals.conf \
    "$dest/share/xdg-desktop-portal/aegis-portals.conf"

  # Bundled Bibata-Modern-Ice (GPL-3.0) license disclosure — embedded into the
  # aegis binary via include_dir, so the disclosure must ship with it.
  install -Dm0644 assets/cursors/Bibata-Modern-Ice/LICENSE \
    "$pkgdir/usr/share/licenses/aegis/Bibata-Modern-Ice/LICENSE"
  install -Dm0644 assets/cursors/Bibata-Modern-Ice/NOTICE \
    "$pkgdir/usr/share/licenses/aegis/Bibata-Modern-Ice/NOTICE"
}

# pacman hooks: the package build never runs these; they run on the target.
post_install() {
  systemctl --user daemon-reload        2>/dev/null || true
  update-desktop-database -q            2>/dev/null || true
  gtk-update-icon-cache -q -t -f usr/share/icons/hicolor 2>/dev/null || true
}
post_upgrade() { post_install "$@"; }
```

#### Recipe notes

- **Optics first.** `aegis` declares `optics` in both `makedepends` and
  `depends`; never build Aegis against an in-tree Optics checkout.
- **Locked Cargo.** `cargo build --locked` honors the committed `Cargo.lock`.
  For a fully offline build (e.g. an air-gapped build server), vendor in
  `prepare()` instead: `cargo vendor --locked vendor`, then build with
  `--frozen --offline` and include `vendor/` in `source=`.
- **`/usr` prefix is fixed.** `contrib/systemd/user/aegis.service` and
  `contrib/dbus-1/services/...aegis.service` hard-code `/usr/bin/...`. If you
  ever change the logical prefix, patch both files consistently or D-Bus
  activation breaks.
- **Keep `Delegate=cpu memory pids`** in `aegis.service`; Realm sandboxing
  depends on it.
- **Hooks, not build steps.** `systemctl --user daemon-reload`,
  `update-desktop-database`, `gtk-update-icon-cache`, and `ldconfig` for
  Optics belong in pacman hooks (`post_install`/`post_upgrade` or
  `/usr/share/libalpm/hooks/*`), never in `build()`/`package()`.
- **Optional subpackages.** Splitting `aegis-portal` and the `fuji`/agent
  binaries into their own packages is valid, but each subpackage must own its
  activation metadata (the D-Bus/xdg-desktop-portal files for the portal
  subpackage) and declare its runtime dependencies.
- **Validate** with the commands in [Validation](#validation) after
  installing, and run `makepkg --printsrcinfo > .SRCINFO` before publishing to
  the AUR.
