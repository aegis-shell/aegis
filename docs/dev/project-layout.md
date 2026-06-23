# Project Layout

Where code lives and where new files belong. For the conceptual design, see
[Architecture](../explanation/architecture.md).

## Source Tree

```text
ass/
  Cargo.toml            workspace
  crates/
    ass-core/          shared model: geometry, surface tree, outputs, focus, apps
    ass-protocols/     shared Wayland protocol interface tables (xdg-shell)
    ass-server/        Wayland server: socket, globals, object lifecycle
    ass-backend/       presentation + input targets (nested now, DRM/KMS later)
    ass-render/        compositing through flux
    ass-shell/         compositor chrome through lens
    ass-config/        declarative configuration: TOML schema, loader, live reload
    ass-ipc/           versioned IPC and introspection over a unix socket
    ass-ctl/           command-line driver for the IPC (reference external tool)
    ass-apps/          freedesktop.org desktop-entry enumeration + icon lookup
    ass-launch/        detached, XDG-environment-aware app launching
    ass/               the binary: wiring and event loop
  docs/                 documentation (see docs/index.md)
```

flux and lens live as sibling C libraries under `../optics` (`flux/` and
`lens/`), each built with meson and wrapped by an out-of-tree Rust binding
crate (`../optics/flux-rs`, `../optics/lens-rs`). ass consumes the bindings
as path dependencies.

## Modules

| Crate | Purpose | Design reference |
|-------|---------|------------------|
| `ass-core` | Backend- and renderer-agnostic types | [ADR-0001](../adr/0001-scope-and-responsibility-boundary.md) |
| `ass-protocols` | Shared xdg-shell interface tables for client and server | [ADR-0002](../adr/0002-hand-rolled-wayland-server.md) |
| `ass-server` | Wayland server socket, globals, and object lifecycle | [ADR-0002](../adr/0002-hand-rolled-wayland-server.md) |
| `ass-backend` | The `Backend` trait and its implementations | [ADR-0002](../adr/0002-hand-rolled-wayland-server.md), [ADR-0003](../adr/0003-nested-first-bring-up.md) |
| `ass-render` | Client buffers to flux textures, scene to output | [ADR-0004](../adr/0004-client-buffers-via-flux-dmabuf-import.md) |
| `ass-shell` | lens chrome bound to the compositor device | [ADR-0001](../adr/0001-scope-and-responsibility-boundary.md) |
| `ass-config` | Versioned TOML schema, loader, and mtime-based live reload | [ADR-0026](../adr/0026-configuration-system.md) |
| `ass-ipc` | Versioned schema and codec over a unix socket; the extension/automation surface | [ADR-0027](../adr/0027-ipc-and-introspection.md) |
| `ass-ctl` | Command-line driver for the IPC; the reference external tool | [ADR-0027](../adr/0027-ipc-and-introspection.md) |
| `ass-apps` | freedesktop.org desktop-entry enumeration and icon-theme lookup | [ADR-0022](../adr/0022-application-launcher.md) |
| `ass-launch` | detached, XDG-environment-aware launching of desktop applications | [ADR-0022](../adr/0022-application-launcher.md) |
| `ass` | Process entry point and frame loop | [Architecture](../explanation/architecture.md) |

## Placement Rules

- Code with no flux, lens, or Wayland dependency belongs in `ass-core`.
- A new presentation or input target is a `Backend` implementation in
  `ass-backend`, not a special case in the binary.
- Compositing and texture handling belong in `ass-render`; chrome belongs
  in `ass-shell`.
- A rendering or texture capability missing from flux is added to flux, not
  worked around in ass; see
  [ADR-0001](../adr/0001-scope-and-responsibility-boundary.md).
- Cross-binding pointer casts (between the `flux` and `lens` `flux_*`
  types) stay localized at the call seam, not spread through the code.

## Documentation

New documentation follows the
[documentation governance](documentation/index.md). Route content with the
governance's routing rules before writing.
