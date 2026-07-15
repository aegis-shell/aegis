# ass

**ass** — *autonomous surface shell* — is a Wayland compositor for Linux,
written in Rust.

## What It Is

ass composites client windows and draws its own shell chrome through
[flux](../optics/libs/flux) (a Vulkan-first rendering engine) and
[lens](../optics/libs/lens) (an immediate-mode UI engine that draws through
flux).
The compositor owns the Wayland server, input, output, and window management;
flux and lens own rendering and UI.

The near-term goal is a good experience for human users. A later phase
adapts the compositor so an AI agent can understand and operate the
machine through it.

## Quick Start

ass builds against **flux** and **lens** in the sibling `../optics` Meson
project. Rust bindings live under `../optics/bindings/`.

Build the C libraries with meson first, then run the nested backend inside an
existing Wayland session:

```bash
meson setup ../optics/build ../optics -Dtests=false
meson compile -C ../optics/build
source scripts/env.sh
cargo run
```

Skip `meson setup` when `../optics/build` already exists.

`scripts/env.sh` points the flux, scene-graph, and lens bindings at the unified
build tree and exposes their shared libraries to test harnesses. `cargo run`
opens a nested window on `$WAYLAND_DISPLAY` and presents the shell. See
[Setup](docs/dev/setup.md) for prerequisites and details.

## Documentation

- [Documentation index](docs/index.md)
- [Architecture](docs/explanation/architecture.md)
- [Architecture Decision Records](docs/adr/index.md)

## License

Apache-2.0.
