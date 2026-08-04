# Distribution Packaging

Use this guide to build coordinated distribution packages from matching
Aegis and Aegis Portal source releases. Daily source-tree development does
not install Aegis; see [Setup](setup.md) instead.

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

Each command must report a version compatible with `<OPTICS_VERSION>`. Distribution
package names vary, so express the dependency in terms of the shared
libraries, headers, and `.pc` files rather than copying files from an Optics
build tree. (On Arch this is the separate `optics` package; see
[Arch Linux](#arch-linux).)

The remaining Aegis build dependencies include a C toolchain, `pkg-config`,
Wayland and its protocols, Vulkan, xkbcommon, libinput, libseat, and libclang
for binding generation. The core runtime uses Linux PAM, logind, and
`brightnessctl` for authentication, sleep coordination, and backlight
dimming.

The compatible
[Aegis Portal source release](https://github.com/aegis-shell/xdg-desktop-portal-aegis)
has its own `Cargo.lock` and additionally requires GTK 4.10, PipeWire/SPA,
Meson, and Ninja. Linux PAM development files are required only for the
optional unlock module. Its manifest pins `aegis-model`, `aegis-ipc`, and
`aegis-logging` to the declared supported Aegis Git tag. The portal runtime
requires `xdg-desktop-portal`, WirePlumber, and `xdg-email`; install the GTK
backend as a fallback for interfaces Aegis does not implement.

## Reproducible Source Preparation

Keep each repository's committed `Cargo.lock`. Run source preparation once
from the Aegis root and once from the compatible Aegis Portal root. A
network-enabled phase may fetch each locked graph directly or vendor it for
an offline builder:

```bash
cargo vendor --locked vendor
```

Record Cargo's emitted source-replacement configuration in the corresponding
package build environment, include each `vendor/` tree in its prepared
source, and build with `--frozen --offline`. Do not rewrite the Optics or
Aegis Git dependencies to local paths in a distribution patch.

## Build

Build the core workspace from the Aegis source root:

```bash
cargo build --frozen --offline --release --workspace
```

Use `cargo build --locked --release --workspace` when the package builder is
allowed to use its pre-populated Cargo cache without a vendored source tree.

Build the matching Aegis Portal release through its production Meson
installer. Meson owns the configured executable paths and generated D-Bus
activation metadata:

```bash
meson setup build-package --buildtype=release --prefix=/usr -Dpam=false
meson compile -C build-package
```

Enable `-Dpam=true` only for a package variant that includes the optional PAM
module and declares the resulting GPL-3.0-only distribution license.

## Install Manifest

The canonical logical prefix is `/usr`. Stage files below `DESTDIR`; do not
run the built binaries, `systemctl`, `ldconfig`, or desktop database updates
inside the package build root.

| Source | Destination | Suggested component |
|--------|-------------|---------------------|
| `target/release/aegis` | `/usr/bin/aegis` | core |
| `target/release/aegis-idle` | `/usr/bin/aegis-idle` | core |
| `target/release/aegis-atspi` | `/usr/bin/aegis-atspi` | core |
| `target/release/aegis-lock` | `/usr/bin/aegis-lock` | core |
| `target/release/aegis-settings` | `/usr/bin/aegis-settings` | core |
| Portal: Meson portal executable | `/usr/libexec/xdg-desktop-portal-aegis` by default | portal |
| Portal: Meson FileChooser executable | `/usr/libexec/aegis-portal-prompter` by default | portal |
| `target/release/aegis-mcp` | `/usr/bin/aegis-mcp` | agent integration |
| `target/release/aegis-agent` | `/usr/bin/aegis-agent` | agent integration |
| `contrib/systemd/user/aegis.service` | `/usr/lib/systemd/user/aegis.service` | core |
| `contrib/io.github.ming2k.aegis.Settings.desktop` | `/usr/share/applications/io.github.ming2k.aegis.Settings.desktop` | core |
| `contrib/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg` | `/usr/share/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg` | core |
| `contrib/pam/aegis-lock` | `/etc/pam.d/aegis-lock` | core |
| Portal: generated D-Bus service | `/usr/share/dbus-1/services/org.freedesktop.impl.portal.desktop.aegis.service` | portal |
| Portal: optional Meson PAM artifact | `/usr/lib/security/pam_aegis.so` by default | portal |
| Portal: `contrib/xdg-desktop-portal/portals/aegis.portal` | `/usr/share/xdg-desktop-portal/portals/aegis.portal` | portal |
| Portal: `contrib/xdg-desktop-portal/aegis-portals.conf` | `/usr/share/xdg-desktop-portal/aegis-portals.conf` | portal |
| `LICENSE` | `/usr/share/licenses/aegis/LICENSE` | core |
| Portal: `LICENSE` | `/usr/share/licenses/xdg-desktop-portal-aegis/LICENSE` | portal |
| `assets/cursors/Bibata-Modern-Ice/LICENSE` | `/usr/share/licenses/aegis/Bibata-Modern-Ice/LICENSE` | core |
| `assets/cursors/Bibata-Modern-Ice/NOTICE` | `/usr/share/licenses/aegis/Bibata-Modern-Ice/NOTICE` | core |

`aegis-lock-preview` is a feature-gated contributor tool, not a distribution
artifact. Package builds must not enable the `dev-preview` feature and must
not install `target/release/aegis-lock-preview`. Keep the explicit
`target/release/aegis-lock` install entry above; do not replace it with an
`aegis-*` wildcard.

The bundled Bibata-Modern-Ice cursor theme (GPL-3.0) is embedded into the
`aegis` binary via `include_dir`. Distributing that binary requires preserving
the theme's license disclosure, so the `LICENSE` and `NOTICE` files must be
staged under `/usr/share/licenses/aegis/Bibata-Modern-Ice/`. The project code
itself is MIT-licensed; the shipped binary is a combined work under
`MIT AND GPL-3.0-only`.

Each independently installable package also carries the project `LICENSE`
under its own `/usr/share/licenses/<package>/` directory.

For example, a simple package recipe can stage binaries with:

```bash
package_root=${DESTDIR:?set DESTDIR to the package staging root}
install -Dm0755 target/release/aegis \
  "$package_root/usr/bin/aegis"
install -Dm0755 target/release/aegis-idle \
  "$package_root/usr/bin/aegis-idle"
install -Dm0755 target/release/aegis-atspi \
  "$package_root/usr/bin/aegis-atspi"
install -Dm0755 target/release/aegis-lock \
  "$package_root/usr/bin/aegis-lock"
install -Dm0755 target/release/aegis-settings \
  "$package_root/usr/bin/aegis-settings"
install -Dm0755 target/release/aegis-mcp \
  "$package_root/usr/bin/aegis-mcp"
install -Dm0755 target/release/aegis-agent \
  "$package_root/usr/bin/aegis-agent"
```

Stage the portal executable and all portal-owned data files from the separate
Aegis Portal build root using the same `package_root` destination.

Install the data files according to the table using mode `0644`.
The PAM profile lives under `/etc`, outside the logical `/usr` prefix. A
distribution may replace its `login` includes with the distribution's
canonical authentication and account stacks, but it must keep both service
classes: `aegis-lock` calls PAM authentication followed by account
management. Omitting or misnaming this profile leaves a securely locked
session unable to authenticate.

`xdg-desktop-portal-aegis` is a separate source and runtime component. Its
package owns both private executables, the generated D-Bus activation file,
the `.portal` metadata, the backend-selection file, and the optional
`pam_aegis.so` secret auto-unlock module. The core package must not own those
files or require the portal frontend and PipeWire solely for this backend.
The core package continues to own `/etc/pam.d/aegis-lock`; its optional
`pam_aegis.so` line is safe when the portal package is absent.

The Portal repository's own source is MIT. A package that ships
`pam_aegis.so` must also declare GPL-3.0-only because the module links the
GPL-licensed `pamsm` dependency.

Install `pam_aegis.so` in the distribution's canonical PAM module directory.
The supplied `aegis-lock` profile loads it as `optional`, so the module never
becomes the screen authenticator. A distribution that enables login-time
vault auto-unlock must add the same optional line after its primary login
authentication stack through the distribution's normal PAM integration
mechanism; do not replace or take ownership of another package's login
profile.

The supplied systemd user service executes `aegis` from `/usr/bin`; the
Portal's Meson build generates its D-Bus activation file from the configured
`libexecdir`. A distribution using another logical prefix must configure both
packages consistently; changing only a binary destination creates broken
activation.

## Package Integration

Use package-manager hooks or distribution triggers, not the package build
phase, to:

- reload the systemd user-unit catalog when required by the distribution;
- refresh the desktop application and icon caches;
- refresh the dynamic-loader cache for Optics shared libraries; and
- restart an existing desktop portal session after an upgrade when required.

On Arch the packages that own these caches and catalogs already provide the
appropriate `libalpm` hooks. Do not invoke `systemctl --user` from a package
install script: the transaction does not run inside every affected user's
session.

The user service delegates the cgroup controllers required by Interaction Domain
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
test -x /usr/bin/aegis-idle
test -x /usr/bin/aegis-atspi
test -x /usr/bin/aegis-lock
test -r /etc/pam.d/aegis-lock
systemctl --user daemon-reload
systemctl --user start aegis.service
```

Confirm that System Settings appears with its icon, the Power Management page
persists an idle policy, `Super+L` authenticates through the installed PAM
stack, `xdg-desktop-portal-aegis` activates through D-Bus, the preferred portal
configuration is selected for an Aegis session, and `aegis window` reaches
the compositor. Run the Interaction Domain sandbox test through the packaged service
topology as described in [Setup](setup.md#tests).

## Distribution recipes

The sections above are distribution-neutral. The recipes below specialize them
to a specific distribution. Each recipe consumes the same source-preparation,
build, install-manifest, and validation contract; only the dependency names,
package format, and integration hooks change.

- [Arch Linux](#arch-linux) — separate `optics`, `aegis`, and
  `xdg-desktop-portal-aegis` source packages.

### Arch Linux

Aegis on Arch uses three installable packages from three source repositories.
Package Optics first, then package the explicitly compatible Aegis and Aegis
Portal tags independently.

Replace `<OPTICS_VERSION>`, `<AEGIS_VERSION>`, and `<PORTAL_VERSION>` below
with the compatible release versions being packaged.

| Package | Provides | Built by |
|---------|----------|----------|
| `optics` | `libflux.so`, `libflux-scene-graph.so`, `liblens.so`, `libiris.so`, headers, `flux.pc` … `iris.pc` | Meson/Ninja from the `ming2k/optics` `v<OPTICS_VERSION>` tag |
| `aegis` | Compositor, System Settings, CLI and agent integration binaries, systemd user unit, desktop entry, icon, and cursor license disclosure | Cargo from the `ming2k/aegis` `v<AEGIS_VERSION>` tag |
| `xdg-desktop-portal-aegis` | Private portal backend plus its D-Bus activation, `.portal`, backend-selection files, and PAM helper | Cargo from the `aegis-shell/xdg-desktop-portal-aegis` `v<PORTAL_VERSION>` tag |

Each repository's CI validates its own dependency surface; the
`makedepends`/`depends` lists below are their Arch equivalents.

#### `optics` PKGBUILD

```bash
# Maintainer: <you>
pkgname=optics
pkgver='<OPTICS_VERSION>'
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
pkgbase=aegis
pkgname=aegis
pkgver='<AEGIS_VERSION>'
_portalver='<PORTAL_VERSION>'
pkgrel=1
arch=(x86_64)
url='https://github.com/ming2k/aegis'
makedepends=(rust pkgconf clang wayland wayland-protocols optics)
source=("$url/archive/refs/tags/v$pkgver.tar.gz")
sha256sums=('SKIP')   # replace with the real release-tarball checksum

build() {
  cd "$pkgbase-$pkgver"
  # Hard contract from the dependency section: Optics native libs must be present.
  pkg-config --modversion flux flux-scene-graph lens iris
  cargo build --locked --release --workspace
}

package() {
  pkgdesc='Wayland compositor and desktop shell'
  # Project code is MIT; the embedded Bibata cursor is GPL-3.0-only.
  license=(MIT GPL-3.0-only)
  depends=(optics vulkan-icd-loader wayland libxkbcommon libinput seatd
           systemd-libs dbus pam brightnessctl hicolor-icon-theme)
  optdepends=(
    "xdg-desktop-portal-aegis=$_portalver: screenshots and screen sharing through xdg-desktop-portal"
    'vulkan-mesa-layers: validation and layers for development'
  )

  cd "$pkgbase-$pkgver"
  local dest="$pkgdir/usr"

  install -Dm0755 target/release/aegis          "$dest/bin/aegis"
  install -Dm0755 target/release/aegis-idle     "$dest/bin/aegis-idle"
  install -Dm0755 target/release/aegis-atspi    "$dest/bin/aegis-atspi"
  install -Dm0755 target/release/aegis-lock     "$dest/bin/aegis-lock"
  install -Dm0755 target/release/aegis-settings "$dest/bin/aegis-settings"
  install -Dm0755 target/release/aegis-mcp "$dest/bin/aegis-mcp"
  install -Dm0755 target/release/aegis-agent    "$dest/bin/aegis-agent"

  install -Dm0644 contrib/systemd/user/aegis.service \
    "$pkgdir/usr/lib/systemd/user/aegis.service"
  install -Dm0644 contrib/io.github.ming2k.aegis.Settings.desktop \
    "$dest/share/applications/io.github.ming2k.aegis.Settings.desktop"
  install -Dm0644 \
    contrib/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg \
    "$dest/share/icons/hicolor/scalable/apps/io.github.ming2k.aegis.Settings.svg"
  install -Dm0644 contrib/pam/aegis-lock \
    "$pkgdir/etc/pam.d/aegis-lock"

  install -Dm0644 LICENSE \
    "$pkgdir/usr/share/licenses/aegis/LICENSE"
  install -Dm0644 assets/cursors/Bibata-Modern-Ice/LICENSE \
    "$pkgdir/usr/share/licenses/aegis/Bibata-Modern-Ice/LICENSE"
  install -Dm0644 assets/cursors/Bibata-Modern-Ice/NOTICE \
    "$pkgdir/usr/share/licenses/aegis/Bibata-Modern-Ice/NOTICE"
}
```

#### `xdg-desktop-portal-aegis` PKGBUILD

```bash
# Maintainer: <you>
pkgname=xdg-desktop-portal-aegis
pkgver='<PORTAL_VERSION>'
_aegisver='<AEGIS_VERSION>'
pkgrel=1
pkgdesc='xdg-desktop-portal backend for the Aegis compositor'
arch=(x86_64)
url='https://github.com/aegis-shell/xdg-desktop-portal-aegis'
license=(MIT GPL-3.0-only)
depends=("aegis=$_aegisver" gtk4 pam pipewire wireplumber xdg-desktop-portal xdg-email)
makedepends=(rust meson ninja pkgconf clang pipewire pam)
optdepends=(
  'xdg-desktop-portal-gtk: fallback for portal interfaces Aegis does not implement'
)
source=("$url/archive/refs/tags/v$pkgver.tar.gz")
sha256sums=('SKIP')   # replace with the real release-tarball checksum

build() {
  cd "$pkgname-$pkgver"
  meson setup build --buildtype=release --prefix=/usr -Dpam=true
  meson compile -C build
}

package() {
  cd "$pkgname-$pkgver"
  local dest="$pkgdir/usr"

  DESTDIR="$pkgdir" meson install -C build
  install -Dm0644 LICENSE \
    "$dest/share/licenses/xdg-desktop-portal-aegis/LICENSE"
}
```

#### Recipe notes

- **Optics first.** `aegis` declares `optics` in both `makedepends` and
  `depends`; never build Aegis against an in-tree Optics checkout.
- **Locked Cargo.** `cargo build --locked` honors the committed `Cargo.lock`.
  For a fully offline build (e.g. an air-gapped build server), vendor in
  `prepare()` instead: `cargo vendor --locked vendor`, then build with
  `--frozen --offline` and include `vendor/` in `source=`.
- **The package boundary is intentional.** `aegis` works without the portal
  package. The separate `xdg-desktop-portal-aegis` PKGBUILD depends on the
  exact compatible core package declared by the Portal release because its
  scoped IPC protocol and compositor mechanisms move in lockstep.
- **`/usr` prefix is fixed.** The systemd unit runs `/usr/bin/aegis`; the
  generated D-Bus service uses Meson's configured portal `libexecdir`. Keep
  both destinations synchronized when changing the prefix.
- **Keep `Delegate=cpu memory pids`** in `aegis.service`; Interaction Domain sandboxing
  depends on it.
- **Use distribution hooks.** The packages that own systemd, desktop, icon,
  and loader catalogs provide the standard `libalpm` hooks. Do not place
  `systemctl --user` calls in the PKGBUILD or an install script.
- **Validate** with the commands in [Validation](#validation) after
  installing, and run `makepkg --printsrcinfo > .SRCINFO` before publishing to
  the AUR.
