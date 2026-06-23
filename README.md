# ass

**ass** — *autonomous surface shell* — is a Wayland compositor for Linux,
written in Rust.

## What It Is

ass composites client windows and draws its own shell chrome through
[flux](../optics/flux) (a Vulkan-first rendering engine) and
[lens](../optics/lens) (an immediate-mode UI engine that draws through flux).
The compositor owns the Wayland server, input, output, and window management;
flux and lens own rendering and UI.

The near-term goal is a good experience for human users. A later phase
adapts the compositor so an AI agent can understand and operate the
machine through it.

## Quick Start

ass builds against two sibling C libraries under `../optics`: **flux**
(`libflux`, the Vulkan rendering engine) and **lens** (`liblens`, the
immediate-mode UI engine that draws the shell chrome). Each is wrapped by an
out-of-tree Rust binding crate (`../optics/flux-rs`, `../optics/lens-rs`).

Build the C libraries with meson first, then run the nested backend inside an
existing Wayland session:

```bash
meson compile -C ../optics/flux/build
meson compile -C ../optics/lens/build
source scripts/env.sh
cargo run
```

`scripts/env.sh` exports the `FLUX_BUILD_DIR` / `LENS_BUILD_DIR` variables the
`-sys` build scripts use to locate the freshly-built libraries without a
`meson install`. `cargo run` opens a nested window on `$WAYLAND_DISPLAY` and
presents the shell. See [Setup](docs/dev/setup.md) for prerequisites and
details.

## Documentation

- [Documentation index](docs/index.md)
- [Architecture](docs/explanation/architecture.md)
- [Architecture Decision Records](docs/adr/index.md)

## License

Apache-2.0.
