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

The `ass-neenee-mcp` process connects the Neenee agent product to that phase.
It exposes scoped desktop and Agent Realm tools through Neenee's MCP runtime
without loading inference into the compositor. See
[Connect Neenee to ASS](docs/how-to/neenee.md).

## Quick Start

ass builds against **flux**, **lens**, and **iris** in the sibling `../optics`
Meson project. Rust bindings live under `../optics/bindings/`. Neenee and its
Praxion runtime remain separate products; the ASS workspace builds only the
platform-side MCP bridge.

Build the C libraries with meson first, then run the nested backend inside an
existing Wayland session:

```bash
meson setup ../optics/build ../optics -Dtests=false -Dbuildtype=debugoptimized
meson compile -C ../optics/build
cargo run -p ass
```

Skip `meson setup` when `../optics/build` already exists.

Open the standalone settings application from a second terminal:

```bash
cargo run -p ass-control-center
```

The Rust bindings automatically discover the sibling build tree and publish
its runtime library paths, so no shell environment setup is required.
`cargo run -p ass` opens a nested window on `$WAYLAND_DISPLAY` and presents
the shell. `cargo run -p ass-control-center` opens the standalone settings
application and connects it to the running compositor over its owner-only IPC.
On a bare TTY (no host session) the compositor binary drives the display
directly through the DRM/KMS backend; force a target with
`--backend auto|drm|nested` or `ASS_BACKEND`. See [Setup](docs/dev/setup.md)
for prerequisites and details.

Realm application sandboxes require ASS to run in its own systemd user
service with delegated `cpu`, `memory`, and `pids` cgroup v2 controllers. The
packaging unit is [ass.service](contrib/systemd/user/ass.service). A direct
`cargo run` remains suitable for compositor development, but `realm-launch`
fails closed there when the containing scope is shared or not delegated. See
[How to Use AI Workspaces](docs/how-to/ai-workspaces.md).

## Documentation

- [Documentation index](docs/index.md)
- [Daily-use guides](docs/how-to/index.md)
- [Architecture](docs/explanation/architecture.md)
- [Architecture Decision Records](docs/adr/index.md)

## License

Apache-2.0.
