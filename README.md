# ass

**ass** — *autonomous surface shell* — is a Wayland compositor for Linux,
written in Rust.

## What It Is

ass composites client windows and draws its own shell chrome through
[flux](../flux/core) (a Vulkan-first graphics library) and
[flux-ui](../flux/ui) (an immediate-mode UI library on flux). The
compositor owns the Wayland server, input, output, and window management;
flux and flux-ui own rendering and UI.

The near-term goal is a good experience for human users. A later phase
adapts the compositor so an AI agent can understand and operate the
machine through it.

## Quick Start

ass builds against the sibling flux monorepo's meson build trees. Build
`libflux` (in `core/`) and `libflux-ui` (in `ui/`) first, then run the
nested backend inside an existing Wayland session:

```bash
meson compile -C ../flux/core/build
meson compile -C ../flux/ui/build
cargo run
```

`cargo run` opens a nested window on `$WAYLAND_DISPLAY` and presents the
shell. See [Setup](docs/dev/setup.md) for prerequisites and details.

## Documentation

- [Documentation index](docs/index.md)
- [Architecture](docs/explanation/architecture.md)
- [Architecture Decision Records](docs/adr/index.md)

## License

Apache-2.0.
