# Development environment for building ass against the sibling flux and lens
# trees. Source it once per shell, then run cargo as usual:
#
#     source scripts/env.sh
#     cargo run
#     cargo test --workspace
#
# ass depends on two C libraries in the unified optics meson project, each
# wrapped by an out-of-tree Rust binding crate:
#
#     ../optics/libs/flux       libflux       (rendering engine)
#     ../optics/libs/lens       liblens       (immediate-mode UI)
#     ../optics/bindings/       Rust bindings
#
# The -sys build scripts locate the C libraries in dev mode: FLUX_BUILD_DIR /
# LENS_BUILD_DIR point at meson build trees whose `meson-uninstalled/*.pc`
# lets pkg-config link the freshly-built library without `meson install`.
# Absolute paths are required because cargo runs each build script with its
# crate directory as the working directory.
#
# If you have run `meson install` for both flux and lens into a prefix on
# PKG_CONFIG_PATH instead, source this with `ASS_DEV_ENV_USE_INSTALLED=1` to
# skip the build-tree probe.

ASS_ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
OPTICS="$ASS_ROOT/../optics"

if [ "${ASS_DEV_ENV_USE_INSTALLED:-0}" != "1" ]; then
    export OPTICS_BUILD_DIR="${OPTICS_BUILD_DIR:-$OPTICS/build}"
    export FLUX_SOURCE_DIR="$OPTICS/libs/flux"
    export FLUX_BUILD_DIR="$OPTICS_BUILD_DIR"
    export LENS_SOURCE_DIR="$OPTICS"
    export LENS_BUILD_DIR="$OPTICS_BUILD_DIR"

    # theseus / wright keeps libwayland (headers + scanner + .pc) in a build
    # sysroot under ~/.cache/wright rather than in /usr. If that sysroot is
    # present, put it on PKG_CONFIG_PATH so pkg-config (and the ass-protocols
    # build script) find wayland-server.pc; on CPATH so the C compiler finds
    # wayland-util.h (its .pc advertises /usr/include, which pkg-config treats
    # as a default path and drops, so the build script cannot recover the
    # sysroot include from cflags alone); and on PATH for wayland-scanner.
    # Harmless on conventional distros where wayland is in /usr.
    wright_sysroot="$HOME/.cache/wright/sysroot.tmp/usr"
    if [ -d "$wright_sysroot/lib/pkgconfig" ]; then
        export PKG_CONFIG_PATH="$wright_sysroot/lib/pkgconfig${PKG_CONFIG_PATH:+:$PKG_CONFIG_PATH}"
        export CPATH="$wright_sysroot/include${CPATH:+:$CPATH}"
        # Runtime: the binary's rpath covers flux/lens but not libwayland,
        # which on theseus lives here rather than in /usr/lib.
        export LD_LIBRARY_PATH="$wright_sysroot/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
        export PATH="$wright_sysroot/bin:$PATH"
    fi

    # Each build tree must already be configured and compiled: its
    # meson-uninstalled *.pc is what the -sys build scripts feed pkg-config.
    check_pc() {
        local dir="$1" pc="$2"
        if [ ! -f "$dir/meson-uninstalled/$pc" ]; then
            echo "ass env: $dir/meson-uninstalled/$pc missing" >&2
            echo "  build it first:  meson setup $dir <source> && meson compile -C $dir" >&2
            return 1
        fi
    }
    check_pc "$OPTICS_BUILD_DIR" flux-uninstalled.pc || return 1
    check_pc "$OPTICS_BUILD_DIR" flux-scene-graph-uninstalled.pc || return 1
    check_pc "$OPTICS_BUILD_DIR" lens-uninstalled.pc || return 1

    # Cargo binaries receive rpaths from the bindings, but library test
    # harnesses do not consistently inherit them.
    export LD_LIBRARY_PATH="$OPTICS_BUILD_DIR/libs/flux:$OPTICS_BUILD_DIR/libs/flux/scene_graph:$OPTICS_BUILD_DIR/libs/lens${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
else
    export FLUX_USE_INSTALLED=1
    export LENS_USE_INSTALLED=1
fi
