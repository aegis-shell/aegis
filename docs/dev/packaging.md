# Distribution Packaging

Use this guide to build coordinated distribution packages from matching
Tessera and Tessera Portal source releases. Daily source-tree development does
not install Tessera; see [Setup](setup.md) instead.

## Dependency Contract

Tessera has two separate Optics dependency surfaces:

- `Cargo.lock` pins the Rust bindings to the Optics tag reported by
  `scripts/optics-release-ref.sh`.
- The native `flux`, `flux-scene-graph`, `lens`, and `iris` libraries are
  system build and runtime dependencies discovered through `pkg-config` and
  the dynamic loader.

A package build must not depend on an `../optics` checkout or enable
`.cargo/optics-local.toml`. Package the matching Optics release first and
verify the native development files before building Tessera:

```bash
pkg-config --modversion flux flux-scene-graph lens iris
```

Each command must report a version compatible with `<OPTICS_VERSION>`. Distribution
package names vary, so express the dependency in terms of the shared
libraries, headers, and `.pc` files rather than copying files from an Optics
build tree. (On Arch this is the separate `optics` package; see
[Arch Linux](#arch-linux).)

The remaining Tessera build dependencies include a C toolchain, `pkg-config`,
Wayland and its protocols, Vulkan, xkbcommon, libinput, libseat, and libclang
for binding generation. The core runtime uses Linux PAM, logind, and
`brightnessctl` for authentication, sleep coordination, and backlight
dimming.

The compatible
[Tessera Portal source release](https://github.com/aegis-shell/xdg-desktop-portal-atrium)
has its own `Cargo.lock` and additionally requires GTK 4.10, PipeWire/SPA,
Meson, and Ninja. Linux PAM development files are required only for the
optional unlock module. Its Portal-owned protocol projection has no Tessera
source dependency; the compositor remains a runtime provider for protocol 24
settings, capture, picking, and stream resources. The portal runtime requires
`xdg-desktop-portal`, WirePlumber, and `xdg-email`; install the GTK backend as
a fallback for interfaces Tessera does not implement.

## Reproducible Source Preparation

Keep each repository's committed `Cargo.lock`. Run source preparation once
from the Tessera root and once from the compatible Tessera Portal root. A
network-enabled phase may fetch each locked graph directly or vendor it for
an offline builder:

Prepare an Tessera release commit in canonical Optics mode. The local path
override must be absent, and the regenerated lockfile must resolve the full
remote dependency graph without modification:

```bash
test ! -e .cargo/config.toml
cargo generate-lockfile
cargo metadata --locked --format-version 1 > /dev/null
```

Do not tag a release until the same metadata command succeeds from a clean
source export. A lockfile produced with `.cargo/optics-local.toml` is local
worktree state because it omits the remote Optics source identities.

```bash
cargo vendor --locked vendor
```

Record Cargo's emitted source-replacement configuration in the corresponding
package build environment, include each `vendor/` tree in its prepared
source, and build with `--frozen --offline`. Do not rewrite the Optics or
Tessera Git dependencies to local paths in a distribution patch.

## Build

Build the core workspace from the Tessera source root:

```bash
cargo build --frozen --offline --release --workspace
```

Use `cargo build --locked --release --workspace` when the package builder is
allowed to use its pre-populated Cargo cache without a vendored source tree.

Build the matching Tessera Portal release through its production Meson
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
| `target/release/tessera` | `/usr/bin/tessera` | core |
| `target/release/tessera-idle` | `/usr/bin/tessera-idle` | core |
| `target/release/tessera-atspi` | `/usr/bin/tessera-atspi` | core |
| `target/release/tessera-lock` | `/usr/bin/tessera-lock` | core |
| Portal: Meson portal executable | `/usr/libexec/xdg-desktop-portal-atrium` by default | portal |
| Portal: Meson FileChooser executable | `/usr/libexec/atrium-portal-prompter` by default | portal |
| `target/release/tessera-mcp` | `/usr/bin/tessera-mcp` | agent integration |
| `contrib/systemd/user/tessera.service` | `/usr/lib/systemd/user/tessera.service` | core |
| `contrib/systemd/user/tessera-shutdown.target` | `/usr/lib/systemd/user/tessera-shutdown.target` | core |
| `contrib/systemd/tessera-session` | `/usr/bin/tessera-session` | core |
| `contrib/pam/tessera-lock` | `/etc/pam.d/tessera-lock` | core |
| Portal: generated D-Bus service | `/usr/share/dbus-1/services/org.freedesktop.impl.portal.desktop.atrium.service` | portal |
| Portal: `contrib/xdg-desktop-portal/portals/atrium.portal` | `/usr/share/xdg-desktop-portal/portals/atrium.portal` | portal |
| Portal: `contrib/xdg-desktop-portal/atrium-portals.conf` | `/usr/share/xdg-desktop-portal/atrium-portals.conf` | portal |
| `LICENSE` | `/usr/share/licenses/tessera/LICENSE` | core |
| Portal: `LICENSE` | `/usr/share/licenses/xdg-desktop-portal-atrium/LICENSE` | portal |

`tessera-lock-preview` is a feature-gated contributor tool, not a distribution
artifact. Package builds must not enable the `dev-preview` feature and must
not install `target/release/tessera-lock-preview`. Keep the explicit
`target/release/tessera-lock` install entry above; do not replace it with an
`tessera-*` wildcard.

The bundled Aegis cursor theme is embedded into the `tessera` binary via
`include_dir`. The art is original and MIT-licensed (generated by
`scripts/prepare-aegis-cursors.py`), so the shipped binary is MIT through and
through and no third-party license staging is required for it.

Each independently installable package also carries the project `LICENSE`
under its own `/usr/share/licenses/<package>/` directory.

For example, a simple package recipe can stage binaries with:

```bash
package_root=${DESTDIR:?set DESTDIR to the package staging root}
install -Dm0755 target/release/tessera \
  "$package_root/usr/bin/tessera"
install -Dm0755 target/release/tessera-idle \
  "$package_root/usr/bin/tessera-idle"
install -Dm0755 target/release/tessera-atspi \
  "$package_root/usr/bin/tessera-atspi"
install -Dm0755 target/release/tessera-lock \
  "$package_root/usr/bin/tessera-lock"
install -Dm0755 target/release/tessera-mcp \
  "$package_root/usr/bin/tessera-mcp"
```

Stage the portal executable and all portal-owned data files from the separate
Tessera Portal build root using the same `package_root` destination.

Install the data files according to the table using mode `0644`.
The PAM profile lives under `/etc`, outside the logical `/usr` prefix. A
distribution may replace its `login` includes with the distribution's
canonical authentication and account stacks, but it must keep both service
classes: `tessera-lock` calls PAM authentication followed by account
management. Omitting or misnaming this profile leaves a securely locked
session unable to authenticate.

`xdg-desktop-portal-atrium` is a separate source and runtime component. Its
package owns the private executable, the generated D-Bus activation file,
the `.portal` metadata, and the backend-selection file. The core package must not own those
files or require the portal frontend and PipeWire solely for this backend.
The core package continues to own `/etc/pam.d/tessera-lock`; its optional
`pam_sigil.so` line is safe whether sigil is present or absent.

Vault auto-unlock is provided by `sigil` (`pam_sigil.so`, ADR-0001 / portal ADR-0020).
The supplied `tessera-lock` profile loads it as `optional`, so the module never
becomes the screen authenticator. A distribution that enables login-time
vault auto-unlock must add the same optional line after its primary login
authentication stack through the distribution's normal PAM integration
mechanism; do not replace or take ownership of another package's login
profile.

The supplied systemd user service executes `tessera` from `/usr/bin`; the
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
systemd-analyze --user verify tessera.service tessera-shutdown.target
pkg-config --modversion flux flux-scene-graph lens iris
test -x /usr/bin/tessera-idle
test -x /usr/bin/tessera-atspi
test -x /usr/bin/tessera-lock
test -r /etc/pam.d/tessera-lock
systemctl --user daemon-reload
systemctl --user start tessera.service
```

Confirm that the command panel's Power Management tab
persists an idle policy, `Super+L` authenticates through the installed PAM
stack, `xdg-desktop-portal-atrium` activates through D-Bus, the preferred portal
configuration is selected for an Tessera session, and `tessera window` reaches
the compositor. Run the Interaction Domain sandbox test through the packaged service
topology as described in [Setup](setup.md#tests).

## Distribution recipes

The sections above are distribution-neutral. The recipes below specialize them
to a specific distribution. Each recipe consumes the same source-preparation,
build, install-manifest, and validation contract; only the dependency names,
package format, and integration hooks change.

- [Arch Linux](#arch-linux) — separate `optics`, `tessera`, and
  `xdg-desktop-portal-atrium` source packages.

### Arch Linux

Tessera on Arch uses three installable packages from three source repositories.
Package Optics first, then package the explicitly compatible Tessera and Tessera
Portal tags independently.

Replace `<OPTICS_VERSION>`, `<TESSERA_VERSION>`, and `<PORTAL_VERSION>` below
with the compatible release versions being packaged.

| Package | Provides | Built by |
|---------|----------|----------|
| `optics` | `libflux.so`, `libflux-scene-graph.so`, `liblens.so`, `libiris.so`, headers, `flux.pc` … `iris.pc` | Meson/Ninja from the `ming2k/optics` `v<OPTICS_VERSION>` tag |
| `tessera` | Compositor, CLI and agent integration binaries, systemd user unit, and cursor license disclosure | Cargo from the `atrium-desktop/tessera` `v<TESSERA_VERSION>` tag |
| `xdg-desktop-portal-atrium` | Private portal backend plus its D-Bus activation, `.portal`, backend-selection files, and PAM helper | Cargo from the `tessera-shell/xdg-desktop-portal-atrium` `v<PORTAL_VERSION>` tag |

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

#### `tessera` PKGBUILD

```bash
# Maintainer: <you>
pkgbase=tessera
pkgname=tessera
pkgver='<TESSERA_VERSION>'
_portalver='<PORTAL_VERSION>'
pkgrel=1
arch=(x86_64)
url='https://github.com/atrium-desktop/tessera'
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
  # Project code and the embedded Aegis cursor art are all MIT.
  license=(MIT)
  depends=(optics vulkan-icd-loader wayland libxkbcommon libinput seatd
           systemd-libs dbus pam brightnessctl)
  optdepends=(
    "xdg-desktop-portal-atrium=$_portalver: screenshots and screen sharing through xdg-desktop-portal"
    'vulkan-mesa-layers: validation and layers for development'
  )

  cd "$pkgbase-$pkgver"
  local dest="$pkgdir/usr"

  install -Dm0755 target/release/tessera          "$dest/bin/tessera"
  install -Dm0755 target/release/tessera-idle     "$dest/bin/tessera-idle"
  install -Dm0755 target/release/tessera-atspi    "$dest/bin/tessera-atspi"
  install -Dm0755 target/release/tessera-lock     "$dest/bin/tessera-lock"
  install -Dm0755 target/release/tessera-mcp "$dest/bin/tessera-mcp"

  install -Dm0644 contrib/systemd/user/tessera.service \
    "$pkgdir/usr/lib/systemd/user/tessera.service"
  install -Dm0644 contrib/systemd/user/tessera-shutdown.target \
    "$pkgdir/usr/lib/systemd/user/tessera-shutdown.target"
  install -Dm0755 contrib/systemd/tessera-session \
    "$pkgdir/usr/bin/tessera-session"
  install -Dm0644 contrib/pam/tessera-lock \
    "$pkgdir/etc/pam.d/tessera-lock"

  install -Dm0644 LICENSE \
    "$pkgdir/usr/share/licenses/tessera/LICENSE"
}
```

#### `xdg-desktop-portal-atrium` PKGBUILD

```bash
# Maintainer: <you>
pkgname=xdg-desktop-portal-atrium
pkgver='<PORTAL_VERSION>'
_tesseraver='<TESSERA_VERSION>'
pkgrel=1
pkgdesc='xdg-desktop-portal backend for the Tessera compositor'
arch=(x86_64)
url='https://github.com/aegis-shell/xdg-desktop-portal-atrium'
license=(MIT GPL-3.0-only)
depends=("tessera=$_tesseraver" gtk4 pam pipewire wireplumber xdg-desktop-portal xdg-email)
makedepends=(rust meson ninja pkgconf clang pipewire pam)
optdepends=(
  'xdg-desktop-portal-gtk: fallback for portal interfaces Tessera does not implement'
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
    "$dest/share/licenses/xdg-desktop-portal-atrium/LICENSE"
}
```

#### Recipe notes

- **Optics first.** `tessera` declares `optics` in both `makedepends` and
  `depends`; never build Tessera against an in-tree Optics checkout.
- **Locked Cargo.** `cargo build --locked` honors the committed `Cargo.lock`.
  For a fully offline build (e.g. an air-gapped build server), vendor in
  `prepare()` instead: `cargo vendor --locked vendor`, then build with
  `--frozen --offline` and include `vendor/` in `source=`.
- **The package boundary is intentional.** `tessera` works without the portal
  package. The separate `xdg-desktop-portal-atrium` PKGBUILD depends on the
  exact compatible core package declared by the Portal release because its
  scoped IPC protocol and compositor mechanisms move in lockstep.
- **`/usr` prefix is fixed.** The systemd unit runs `/usr/bin/tessera`; the
  generated D-Bus service uses Meson's configured portal `libexecdir`. Keep
  both destinations synchronized when changing the prefix.
- **Keep `Delegate=cpu memory pids`** in `tessera.service`; Interaction Domain sandboxing
  depends on it.
- **Use distribution hooks.** The packages that own systemd, desktop, icon,
  and loader catalogs provide the standard `libalpm` hooks. Do not place
  `systemctl --user` calls in the PKGBUILD or an install script.
- **Validate** with the commands in [Validation](#validation) after
  installing, and run `makepkg --printsrcinfo > .SRCINFO` before publishing to
  the AUR.
